//! Windows-specific handle/ACL operations. No impersonation or network fallback.
use super::*;
use std::{
    ffi::c_void,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr,
};
use tokio::{
    net::windows::named_pipe::{ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions},
    task::JoinSet,
};
use windows_sys::Win32::{
    Foundation::{ERROR_PIPE_BUSY, LocalFree},
    Security::{
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            SDDL_REVISION_1,
        },
        GetTokenInformation, SECURITY_ATTRIBUTES, TOKEN_GROUPS, TOKEN_QUERY, TokenLogonSid,
    },
    Storage::FileSystem::SECURITY_IDENTIFICATION,
    System::{
        Pipes::GetNamedPipeServerProcessId,
        Threading::{GetCurrentProcess, OpenProcessToken},
    },
};

const PREFIX: &str = r"\\.\pipe\coincube-mcp-";

fn user_sid() -> io::Result<String> {
    use windows_sys::Win32::Security::{TOKEN_USER, TokenUser};
    unsafe {
        let mut token = ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(io::Error::last_os_error());
        }
        let token = OwnedHandle::from_raw_handle(token);
        let mut size = 0;
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            ptr::null_mut(),
            0,
            &mut size,
        );
        if size < size_of::<TOKEN_USER>() as u32 || size > 65536 {
            return Err(io::Error::other("user identity unavailable"));
        }
        let mut bytes = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
        if GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            bytes.as_mut_ptr().cast(),
            size,
            &mut size,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        sid_text((&*bytes.as_ptr().cast::<TOKEN_USER>()).User.Sid)
    }
}

// SAFETY: caller supplies an OS-validated SID that remains alive during conversion.
unsafe fn sid_text(sid: *mut c_void) -> io::Result<String> {
    unsafe {
        let mut text = ptr::null_mut();
        if ConvertSidToStringSidW(sid, &mut text) == 0 {
            return Err(io::Error::last_os_error());
        }
        let _allocation = LocalAllocation(text.cast());
        let mut len = 0;
        while len < 256 && *text.add(len) != 0 {
            len += 1;
        }
        if len == 256 {
            return Err(io::Error::other("invalid identity"));
        }
        String::from_utf16(std::slice::from_raw_parts(text, len))
            .map_err(|_| io::Error::other("invalid identity"))
    }
}

/// Pairing files use the user SID, not a transient logon SID, across restarts.
pub(crate) fn protect_new_path(path: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Security::Authorization::{SE_FILE_OBJECT, SetNamedSecurityInfoW};
    use windows_sys::Win32::Security::{
        ACL, DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, GetSecurityDescriptorOwner,
        OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
    };
    let sid = user_sid()?;
    let sddl: Vec<u16> = format!("O:{sid}D:P(A;;FA;;;{sid})")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        let mut descriptor = ptr::null_mut();
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            ptr::null_mut(),
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let _allocation = LocalAllocation(descriptor);
        let (mut present, mut defaulted) = (0, 0);
        let mut dacl: *mut ACL = ptr::null_mut();
        if GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted) == 0
            || present == 0
            || dacl.is_null()
        {
            return Err(io::Error::other("private ACL unavailable"));
        }
        let mut owner = ptr::null_mut();
        if GetSecurityDescriptorOwner(descriptor, &mut owner, &mut defaulted) == 0
            || owner.is_null()
        {
            return Err(io::Error::other("private owner unavailable"));
        }
        let error = SetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION
                | DACL_SECURITY_INFORMATION
                | PROTECTED_DACL_SECURITY_INFORMATION,
            owner,
            ptr::null_mut(),
            dacl,
            ptr::null_mut(),
        );
        if error != 0 {
            return Err(io::Error::from_raw_os_error(error as i32));
        }
    }
    check_private_path(path)
}

pub(crate) fn check_private_path(path: &Path) -> io::Result<()> {
    use std::os::windows::{ffi::OsStrExt, fs::MetadataExt};
    use windows_sys::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{
        ACCESS_ALLOWED_ACE, DACL_SECURITY_INFORMATION, GetAce, GetSecurityDescriptorControl,
        GetSecurityDescriptorDacl, INHERIT_ONLY_ACE, OWNER_SECURITY_INFORMATION, SE_DACL_PROTECTED,
    };
    use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
    if std::fs::symlink_metadata(path)?.file_attributes() & 0x400 != 0 {
        return Err(io::Error::other("reparse path denied"));
    }
    let sid = user_sid()?;
    let name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        let (mut descriptor, mut owner) = (ptr::null_mut(), ptr::null_mut());
        let error = GetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | OWNER_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut descriptor,
        );
        if error != 0 {
            return Err(io::Error::from_raw_os_error(error as i32));
        }
        let _allocation = LocalAllocation(descriptor);
        if owner.is_null() || sid_text(owner)? != sid {
            return Err(io::Error::other("private owner mismatch"));
        }
        // Inspect the binary ACL. SDDL is a display format and can render a
        // well-known account as an alias instead of its numeric SID.
        let (mut control, mut revision) = (0, 0);
        let (mut present, mut defaulted) = (0, 0);
        let mut dacl = ptr::null_mut();
        if GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) == 0
            || control & SE_DACL_PROTECTED == 0
            || GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted) == 0
            || present == 0
            || dacl.is_null()
            || (*dacl).AceCount != 1
        {
            return Err(io::Error::other("shared pairing path denied"));
        }
        let mut ace = ptr::null_mut();
        if GetAce(dacl, 0, &mut ace) == 0 || ace.is_null() {
            return Err(io::Error::other("private ACE unavailable"));
        }
        let header = &*ace.cast::<windows_sys::Win32::Security::ACE_HEADER>();
        // Check the type/size before interpreting the allowed-ACE layout.
        if header.AceType != 0 || usize::from(header.AceSize) < size_of::<ACCESS_ALLOWED_ACE>() {
            return Err(io::Error::other("private ACE unavailable"));
        }
        let ace = &*ace.cast::<ACCESS_ALLOWED_ACE>();
        // ACCESS_ALLOWED_ACE_TYPE is zero. Only the owner may access this path.
        if ace.Header.AceType != 0
            || u32::from(ace.Header.AceFlags) & INHERIT_ONLY_ACE != 0
            || ace.Mask != FILE_ALL_ACCESS
            || sid_text(ptr::addr_of!(ace.SidStart).cast_mut().cast())? != sid
        {
            return Err(io::Error::other("shared pairing path denied"));
        }
    }
    Ok(())
}

struct LocalAllocation(*mut c_void);
impl Drop for LocalAllocation {
    fn drop(&mut self) {
        // SAFETY: these pointers come only from LocalAlloc-backed Win32 conversions.
        unsafe {
            LocalFree(self.0);
        }
    }
}

fn logon_sid() -> io::Result<String> {
    // SAFETY: all output pointers refer to initialized locals; the owned handle
    // and aligned token buffer remain alive while their contents are inspected.
    unsafe {
        let mut token = ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(io::Error::last_os_error());
        }
        let token = OwnedHandle::from_raw_handle(token);
        let mut length = 0;
        GetTokenInformation(
            token.as_raw_handle(),
            TokenLogonSid,
            ptr::null_mut(),
            0,
            &mut length,
        );
        if length < size_of::<TOKEN_GROUPS>() as u32 || length > 65536 {
            return Err(io::Error::other("logon identity unavailable"));
        }
        let mut buffer = vec![0usize; (length as usize).div_ceil(size_of::<usize>())];
        if GetTokenInformation(
            token.as_raw_handle(),
            TokenLogonSid,
            buffer.as_mut_ptr().cast(),
            length,
            &mut length,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let groups = &*buffer.as_ptr().cast::<TOKEN_GROUPS>();
        if groups.GroupCount != 1 {
            return Err(io::Error::other("logon identity unavailable"));
        }
        let mut text = ptr::null_mut();
        if ConvertSidToStringSidW(groups.Groups[0].Sid, &mut text) == 0 {
            return Err(io::Error::last_os_error());
        }
        let _allocated = LocalAllocation(text.cast());
        // A string SID is bounded by Win32's SID maximum; do not scan unbounded.
        let mut len = 0;
        while len < 256 && *text.add(len) != 0 {
            len += 1;
        }
        if len == 256 {
            return Err(io::Error::other("invalid logon identity"));
        }
        String::from_utf16(std::slice::from_raw_parts(text, len))
            .map_err(|_| io::Error::other("invalid logon identity"))
    }
}

fn create_pipe(path: &Path, sid: &str, first: bool) -> io::Result<NamedPipeServer> {
    // Protected DACL: only the current logon SID. A bearer grant is still
    // required; membership in this OS session is not wallet authorization.
    let sddl: Vec<u16> = format!("D:P(A;;GA;;;{sid})")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    unsafe {
        let mut descriptor = ptr::null_mut();
        // SAFETY: NUL-terminated SDDL and valid out pointers; conversion allocates.
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            ptr::null_mut(),
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let _allocated = LocalAllocation(descriptor);
        let mut attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        // SAFETY: CreateNamedPipe copies the security descriptor during this call;
        // both attributes and descriptor remain alive until it returns.
        ServerOptions::new()
            .first_pipe_instance(first)
            .reject_remote_clients(true)
            .max_instances(9)
            .in_buffer_size(REQUEST_LIMIT as u32)
            .out_buffer_size(RESPONSE_LIMIT as u32)
            .create_with_security_attributes_raw(
                path,
                (&mut attributes as *mut SECURITY_ATTRIBUTES).cast(),
            )
    }
}

pub(super) fn start(reader: Arc<dyn Reader>, grant: Grant) -> io::Result<Server> {
    let sid = logon_sid()?;
    let mut random = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut random);
    let random: String = random.iter().map(|b| format!("{b:02x}")).collect();
    let socket = PathBuf::from(format!("{PREFIX}{}-{random}", std::process::id()));
    let mut listener = create_pipe(&socket, &sid, true)?;
    let grant = Arc::new(grant);
    let permission = grant.clone();
    let path = socket.clone();
    let task = tokio::spawn(async move {
        let mut workers = JoinSet::new();
        loop {
            tokio::select! {
                _ = permission.wait_expiry() => { permission.revoke(); break; }
                Some(_) = workers.join_next(), if !workers.is_empty() => {}
                connected = listener.connect(), if workers.len() < 8 => {
                    if connected.is_err() { permission.revoke(); break; }
                    // Keep an instance alive while creating its successor so
                    // another process can never acquire the first-instance slot.
                    let Ok(next) = create_pipe(&path, &sid, false) else { permission.revoke(); break; };
                    let stream = std::mem::replace(&mut listener, next);
                    let reader = reader.clone();
                    let grant = permission.clone();
                    workers.spawn(async move { let _ = tokio::time::timeout(DEADLINE, serve(stream, reader, grant)).await; });
                }
            }
        }
        // Dropping the listener and JoinSet closes every pipe instance.
    });
    Ok(Server {
        grant,
        socket,
        task,
        _directory: None,
    })
}

fn expected_pid(path: &Path) -> io::Result<u32> {
    let parsed = path
        .to_str()
        .and_then(|p| p.strip_prefix(PREFIX))
        .and_then(|p| p.split_once('-'))
        .filter(|(_, nonce)| nonce.len() == 64 && nonce.bytes().all(|b| b.is_ascii_hexdigit()))
        .and_then(|(pid, _)| pid.parse::<u32>().ok())
        .filter(|pid| *pid != 0);
    parsed.ok_or_else(|| io::Error::other("invalid local endpoint"))
}

pub(super) async fn connect(path: &Path) -> io::Result<NamedPipeClient> {
    let expected = expected_pid(path)?;
    let client = loop {
        match ClientOptions::new()
            .security_qos_flags(SECURITY_IDENTIFICATION)
            .open(path)
        {
            Ok(client) => break client,
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(e) => return Err(e),
        }
    };
    let mut actual = 0;
    // SAFETY: the owned client handle stays alive and actual is a valid output.
    if unsafe { GetNamedPipeServerProcessId(client.as_raw_handle(), &mut actual) } == 0
        || actual != expected
    {
        return Err(io::Error::other("local server identity mismatch"));
    }
    // No credential has been sent before this OS-provided identity check.
    Ok(client)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn path(pid: u32) -> PathBuf {
        let grant = Grant::new("fixture".into(), [true; 3], Duration::from_secs(60));
        format!("{PREFIX}{pid}-{}", grant.credential()).into()
    }
    #[test]
    fn only_strict_local_endpoints_are_accepted() {
        for invalid in [
            r"\\remote\pipe\coincube-mcp-1",
            r"\\localhost\pipe\anything",
            "C:\\file",
            "tcp://127.0.0.1",
            r"\\.\pipe\coincube-mcp-0-abc",
        ] {
            assert!(expected_pid(Path::new(invalid)).is_err());
        }
        assert_eq!(
            expected_pid(&path(std::process::id())).unwrap(),
            std::process::id()
        );
    }
    #[tokio::test]
    async fn first_instance_is_exclusive_and_peer_pid_must_match() {
        let endpoint = path(u32::MAX);
        let sid = logon_sid().unwrap();
        let server = create_pipe(&endpoint, &sid, true).unwrap();
        assert!(create_pipe(&endpoint, &sid, true).is_err());
        // A pipe at the configured name cannot receive a credential if its
        // OS-reported owning PID differs from the trusted configuration.
        assert!(connect(&endpoint).await.is_err());
        drop(server);
    }
    #[tokio::test]
    async fn protected_acl_grants_only_this_logon_and_rejects_other_logon() {
        use windows_sys::Win32::Security::{
            Authorization::{
                ConvertSecurityDescriptorToStringSecurityDescriptorW, GetSecurityInfo,
                SE_KERNEL_OBJECT,
            },
            DACL_SECURITY_INFORMATION,
        };
        let sid = logon_sid().unwrap();
        let endpoint = path(std::process::id());
        let server = create_pipe(&endpoint, &sid, true).unwrap();
        unsafe {
            let mut descriptor = ptr::null_mut();
            // SAFETY: valid owned pipe and initialized output pointer. Win32
            // returns one LocalAlloc allocation, freed after the conversion.
            assert_eq!(
                GetSecurityInfo(
                    server.as_raw_handle(),
                    SE_KERNEL_OBJECT,
                    DACL_SECURITY_INFORMATION,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    &mut descriptor
                ),
                0
            );
            let _descriptor = LocalAllocation(descriptor);
            let mut text = ptr::null_mut();
            let mut length = 0;
            assert_ne!(
                ConvertSecurityDescriptorToStringSecurityDescriptorW(
                    descriptor,
                    SDDL_REVISION_1,
                    DACL_SECURITY_INFORMATION,
                    &mut text,
                    &mut length
                ),
                0
            );
            let _text = LocalAllocation(text.cast());
            let text =
                String::from_utf16(std::slice::from_raw_parts(text, length as usize - 1)).unwrap();
            assert!(text.starts_with("D:P"), "DACL must not inherit entries");
            assert!(text.contains(&sid));
            assert_eq!(
                text.matches('(').count(),
                1,
                "only the logon SID may have an ACE"
            );
        }
        assert!(connect(&endpoint).await.is_ok());
        let (prefix, suffix) = sid.rsplit_once('-').unwrap();
        let other = format!(
            "{prefix}-{}",
            suffix.parse::<u32>().unwrap().wrapping_add(1)
        );
        let other_path = path(std::process::id());
        let _other = create_pipe(&other_path, &other, true).unwrap();
        // The same caller that succeeds above is rejected by a different
        // logon-session ACL. This exercises actual kernel access enforcement.
        assert_eq!(
            connect(&other_path).await.unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
    }
}
