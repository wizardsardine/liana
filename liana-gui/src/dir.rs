use crate::app::settings::WalletId;
use liana::miniscript::bitcoin::Network;
use lianad::datadir::DataDirectory;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct LianaDirectory(PathBuf);

impl LianaDirectory {
    pub fn new(p: PathBuf) -> Self {
        LianaDirectory(p)
    }
    pub fn new_default() -> Result<Self, Box<dyn std::error::Error>> {
        default_datadir().map(LianaDirectory::new)
    }
}

impl LianaDirectory {
    pub fn exists(&self) -> bool {
        self.0.as_path().exists()
    }
    pub fn init(&self) -> Result<(), Box<dyn std::error::Error>> {
        create_directory(self.0.as_path())
    }
    pub fn path(&self) -> &Path {
        self.0.as_path()
    }

    pub fn network_directory(&self, network: Network) -> NetworkDirectory {
        let mut path = self.0.clone();
        path.push(network.to_string());
        NetworkDirectory::new(path)
    }

    pub fn bitcoind_directory(&self) -> BitcoindDirectory {
        let mut path = self.0.clone();
        path.push("bitcoind");
        BitcoindDirectory::new(path)
    }
}

// Get the absolute path to the liana configuration folder.
///
/// This a "liana" directory in the XDG standard configuration directory for all OSes but
/// Linux-based ones, for which it's `~/.liana`.
/// Rationale: we want to have the database, RPC socket, etc.. in the same folder as the
/// configuration file but for Linux the XDG specify a data directory (`~/.local/share/`) different
/// from the configuration one (`~/.config/`).
fn default_datadir() -> Result<PathBuf, Box<dyn std::error::Error>> {
    #[cfg(target_os = "linux")]
    let configs_dir = dirs::home_dir();

    #[cfg(not(target_os = "linux"))]
    let configs_dir = dirs::config_dir();

    if let Some(mut path) = configs_dir {
        #[cfg(target_os = "linux")]
        path.push(".liana");

        #[cfg(not(target_os = "linux"))]
        path.push("Liana");

        return Ok(path);
    }

    Err("Failed to get default data directory".into())
}

#[derive(Clone, Debug)]
pub struct NetworkDirectory(PathBuf);

impl NetworkDirectory {
    pub fn new(p: PathBuf) -> Self {
        NetworkDirectory(p)
    }
}

impl NetworkDirectory {
    pub fn exists(&self) -> bool {
        self.0.as_path().exists()
    }
    pub fn init(&self) -> Result<(), Box<dyn std::error::Error>> {
        create_directory(self.0.as_path())?;
        create_directory(&self.0.as_path().join("data"))
    }
    pub fn path(&self) -> &Path {
        self.0.as_path()
    }
    pub fn lianad_data_directory(&self, wallet_id: &WalletId) -> DataDirectory {
        let mut path = self.0.clone();
        if !wallet_id.is_legacy() {
            path.push("data");
            path.push(wallet_id.to_string());
        }
        DataDirectory::new(path)
    }
}

#[derive(Clone, Debug)]
pub struct BitcoindDirectory(PathBuf);

impl BitcoindDirectory {
    pub fn new(p: PathBuf) -> Self {
        BitcoindDirectory(p)
    }
    pub fn exists(&self) -> bool {
        self.0.as_path().exists()
    }
    pub fn init(&self) -> Result<(), Box<dyn std::error::Error>> {
        create_directory(self.0.as_path())
    }
    pub fn path(&self) -> &Path {
        self.0.as_path()
    }
}

fn create_directory(datadir_path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(unix)]
    return {
        use std::fs::DirBuilder;
        use std::os::unix::fs::DirBuilderExt;

        let mut builder = DirBuilder::new();
        builder.mode(0o700).recursive(true).create(datadir_path)?;
        Ok(())
    };

    // TODO: permissions on Windows..
    #[cfg(not(unix))]
    return {
        std::fs::create_dir_all(datadir_path)?;
        Ok(())
    };
}
