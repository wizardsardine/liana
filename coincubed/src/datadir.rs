use std::path::{Path, PathBuf};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

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
        // On macOS/iOS, SUN_LEN is 104 bytes. The full datadir path easily
        // exceeds this, so we derive a short stable socket path in temp_dir()
        // by hashing the datadir path. This keeps the socket name to ~18 chars
        // (e.g. /tmp/cc1a2b3c4d5e6f78.sock) which is always safe.
        let mut hasher = DefaultHasher::new();
        self.0.hash(&mut hasher);
        let hash = hasher.finish();
        std::env::temp_dir().join(format!("cc{:016x}.sock", hash))
    }
}