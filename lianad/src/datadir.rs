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
        return { std::fs::create_dir_all(self.path()) };
    }
    pub fn path(&self) -> &Path {
        self.0.as_path()
    }
    pub fn sqlite_db_file_path(&self) -> PathBuf {
        let mut dir = self.0.clone();
        dir.push("lianad.sqlite3");
        dir
    }
    pub fn lianad_watchonly_wallet_path(&self) -> PathBuf {
        let mut dir = self.0.clone();
        dir.push("lianad_watchonly_wallet");
        dir
    }
    pub fn lianad_rpc_socket_path(&self) -> PathBuf {
        let mut dir = self.0.clone();
        dir.push("lianad_rpc");
        dir
    }
}
