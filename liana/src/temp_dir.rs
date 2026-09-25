use crate::random::random_bytes;
use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

/// A new randomly named directory in the system temporary directory, removed with its content
/// on drop.
#[derive(Debug)]
pub struct TempDir(PathBuf);

/// Name draws before giving up, reached only if the temporary directory is full of leftovers.
const MAX_DRAWS: usize = 10;

impl TempDir {
    /// Draws random names until one is free, so that concurrent tests and leftovers from previous
    /// runs never share a directory. Panics after `MAX_DRAWS` taken names or on any other error,
    /// as this is only meant for tests.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        for _ in 0..MAX_DRAWS {
            let bytes = random_bytes().expect("random bytes");
            let name: String = bytes[..8].iter().map(|b| format!("{b:02x}")).collect();
            let path = env::temp_dir().join(format!("liana-{name}"));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => panic!(
                    "Failed to create temporary directory '{}': {e}",
                    path.display()
                ),
            }
        }
        panic!("No free temporary directory name after {} draws", MAX_DRAWS);
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if let Err(e) = fs::remove_dir_all(&self.0) {
            eprintln!(
                "Failed to remove temporary directory '{}': {e}",
                self.0.display()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::temp_dir::TempDir;
    use std::fs;

    #[test]
    fn removed_on_drop() {
        let dir = TempDir::new();
        let path = dir.path().to_path_buf();
        assert!(path.is_dir());
        fs::create_dir_all(path.join("nested")).unwrap();
        fs::write(path.join("nested").join("file"), b"content").unwrap();

        drop(dir);
        assert!(!path.exists());
    }

    #[test]
    fn unique_paths() {
        let first = TempDir::new();
        let second = TempDir::new();
        assert_ne!(first.path(), second.path());
        assert!(first.path().is_dir());
        assert!(second.path().is_dir());
    }
}
