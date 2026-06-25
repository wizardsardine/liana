use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

pub struct DataDirectory(PathBuf);

impl DataDirectory {
    pub fn new(p: PathBuf) -> Self {
        DataDirectory(p)
    }
}

impl DataDirectory {
    pub fn exists(&self) -> bool {
        self.0.as_path().exists()
    }
    pub fn init(&self) -> Result<(), std::io::Error> {
        #[cfg(unix)]
        return {
            use std::fs::DirBuilder;
            use std::os::unix::fs::DirBuilderExt;

            let mut builder = DirBuilder::new();
            builder.mode(0o700).recursive(true).create(self.path())
        };

        // TODO: permissions on Windows..
        #[cfg(not(unix))]
        return std::fs::create_dir_all(self.path());
    }
    pub fn path(&self) -> &Path {
        self.0.as_path()
    }
    pub fn sqlite_db_file_path(&self) -> PathBuf {
        let mut dir = self.0.clone();
        dir.push("coincubed.sqlite3");
        dir
    }
    pub fn coincubed_watchonly_wallet_path(&self) -> PathBuf {
        let mut dir = self.0.clone();
        dir.push("coincubed_watchonly_wallet");
        dir
    }
    pub fn coincubed_rpc_socket_path(&self) -> PathBuf {
        let mut hasher = DefaultHasher::new();
        self.0.hash(&mut hasher);
        let hash = hasher.finish();

        let socket_name = format!("cc{:016x}.sock", hash);

        let path = std::env::temp_dir().join(&socket_name);

        #[cfg(unix)]
        {
            // macOS uses 104 bytes, Linux usually 108.
            // Use the stricter limit so the path is always safe.
            const MAX_SUN_PATH: usize = 104;

            if path.as_os_str().as_encoded_bytes().len() >= MAX_SUN_PATH {
                return PathBuf::from("/tmp").join(socket_name);
            }
        }

        path
    }
}
