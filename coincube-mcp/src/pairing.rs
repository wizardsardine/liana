//! Private reconnect locators. They contain no credential or wallet data.
use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Locator {
    socket: PathBuf,
}

/// Verify a pairing directory/file before using it to route a bearer credential.
/// Same-user malware is outside the local IPC threat model; other users and
/// symlinked/shared paths must not be able to replace this trusted locator.
pub fn check_private(path: &Path, directory: bool) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink()
        || (directory && !meta.is_dir())
        || (!directory && !meta.is_file())
    {
        return Err(io::Error::other("invalid private path"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        unsafe extern "C" {
            fn geteuid() -> u32;
        }
        // SAFETY: POSIX geteuid has no arguments or memory ownership.
        if meta.uid() != unsafe { geteuid() } || meta.permissions().mode() & 0o077 != 0 {
            return Err(io::Error::other("private path permissions"));
        }
    }
    #[cfg(windows)]
    crate::ipc::windows::check_private_path(path)?;
    Ok(())
}

/// Protect a newly created path; never change permissions on pre-existing data.
pub fn protect_new(path: &Path, directory: bool) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            path,
            fs::Permissions::from_mode(if directory { 0o700 } else { 0o600 }),
        )?;
    }
    #[cfg(windows)]
    crate::ipc::windows::protect_new_path(path)?;
    check_private(path, directory)
}

/// Resolve on every call so a running STDIO client survives ordinary relaunch.
pub fn resolve(path: &Path) -> io::Result<PathBuf> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("invalid locator"))?;
    if !path.is_absolute() || parent.canonicalize()? != parent {
        return Err(io::Error::other("invalid locator"));
    }
    check_private(parent, true)?;
    check_private(path, false)?;
    let file = fs::File::open(path)?;
    if file.metadata()?.len() > 1024 {
        return Err(io::Error::other("oversized locator"));
    }
    use io::Read;
    let mut bytes = Vec::new();
    file.take(1025).read_to_end(&mut bytes)?;
    let locator: Locator =
        serde_json::from_slice(&bytes).map_err(|_| io::Error::other("invalid locator"))?;
    #[cfg(unix)]
    {
        let directory = locator
            .socket
            .parent()
            .ok_or_else(|| io::Error::other("invalid endpoint"))?;
        if directory.parent() != Some(Path::new("/tmp"))
            || directory
                .file_name()
                .and_then(|v| v.to_str())
                .is_none_or(|v| !v.starts_with("coincube-mcp-"))
            || locator.socket.file_name().is_none_or(|v| v != "rpc")
        {
            return Err(io::Error::other("invalid endpoint"));
        }
        check_private(directory, true)?;
    }
    // The Windows connector independently validates the strict local name and
    // OS-reported pipe PID before transmitting the credential.
    Ok(locator.socket)
}

/// Atomically publish the current private endpoint, never a bearer credential.
pub fn publish(path: &Path, socket: &Path) -> io::Result<()> {
    write_private(
        path,
        &serde_json::to_vec(&Locator {
            socket: socket.to_owned(),
        })?,
    )
}

/// Replace one private file atomically and flush it before advertising success.
pub fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("invalid locator"))?;
    check_private(parent, true)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    protect_new(file.path(), false)?;
    use io::Write;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| e.error)?;
    #[cfg(unix)]
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}
