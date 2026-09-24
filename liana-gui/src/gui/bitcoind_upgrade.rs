use std::{
    fs::{File, OpenOptions},
    path::Path,
    time::Duration,
};

use fs2::{lock_contended_error, FileExt};
use iced::{
    task,
    widget::{column, row, Space},
    Subscription, Task,
};
use liana::miniscript::bitcoin::Network;
use liana_ui::{
    component::{
        button::{btn_retry, btn_skip},
        card, loading,
        text::new,
    },
    spacing::{HSpacing, VSpacing},
    theme,
    widget::{Element, SpaceExt},
};

use crate::{
    app::{config::Config, settings::LianaWalletSettings},
    dir::{create_directory, LianaDirectory},
    download::{self, Progress},
    installer::step::install_bitcoind,
    loader::DAEMON_START_PROGRESS,
    node::bitcoind::{
        self, bitcoind_exe_path, bitcoind_version_directory, internal_bitcoind_directory,
        internal_bitcoind_exe_path, VERSION,
    },
    t,
};

const STAGING_DIRECTORY_NAME: &str = "bitcoind_upgrade";

/// Rename attempts before failing the install, Windows antivirus may briefly hold the new
/// executable open.
const RENAME_ATTEMPTS: usize = 10;
const RENAME_RETRY_DELAY: Duration = Duration::from_millis(500);

/// Share of the progress bar filled by the download, the install fills it up to the daemon start.
const DOWNLOAD_SHARE: f32 = 0.9;

/// Whether the wallet runs the managed bitcoind, the only one we upgrade.
fn uses_managed_node(wallet: &LianaWalletSettings, config: &Config) -> bool {
    let remote_backend = wallet.remote_backend_auth.is_some();
    let managed_bitcoind = wallet
        .start_internal_bitcoind
        .unwrap_or(config.start_internal_bitcoind);
    !remote_backend && managed_bitcoind
}

// NOTE: if an higher version of bitcoind is installed we still install
// the targeted version, to avoid potential breaking changes on core side
fn needs_upgrade(datadir: &LianaDirectory, version: &str) -> bool {
    !internal_bitcoind_exe_path(datadir, version).is_file()
}

/// Takes the upgrade lock, or returns `None` if another wallet holds it.
fn acquire_lock(datadir: &LianaDirectory) -> Result<Option<File>, String> {
    let directory = datadir.bitcoind_directory();
    directory.init().map_err(|e| e.to_string())?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.path().join("upgrade.lock"))
        .map_err(|e| e.to_string())?;
    match file.try_lock_exclusive() {
        Ok(()) => {}
        Err(e) if e.raw_os_error() == lock_contended_error().raw_os_error() => return Ok(None),
        Err(e) => return Err(e.to_string()),
    }
    let staging = directory.path().join(STAGING_DIRECTORY_NAME);
    if staging.exists() {
        // Clean up the remains of a failed attempt.
        std::fs::remove_dir_all(&staging).map_err(|e| e.to_string())?;
    }
    Ok(Some(file))
}

fn rename_with_retry(from: &Path, to: &Path) -> std::io::Result<()> {
    for _ in 1..RENAME_ATTEMPTS {
        match std::fs::rename(from, to) {
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                std::thread::sleep(RENAME_RETRY_DELAY)
            }
            result => return result,
        }
    }
    std::fs::rename(from, to)
}

#[derive(Debug, Clone)]
pub enum Message {
    Check,
    Retry,
    StartDownload,
    Progress(Result<Progress, download::DownloadError>),
    Installed(Result<(), String>),
    Skip,
    Continue,
}

#[derive(Debug)]
enum Stage {
    Waiting,
    ReadyToUpgrade,
    Downloading(f32),
    Installing,
    Installed,
    Failed(String),
}

pub struct Upgrade {
    pub datadir: LianaDirectory,
    pub config: Config,
    pub network: Network,
    pub wallet: LianaWalletSettings,
    lock: Option<File>,
    stage: Stage,
    // Aborts the download when the upgrade is dropped, e.g. when the user skips it.
    download: Option<task::Handle>,
}

impl Upgrade {
    /// Returns `None` if the wallet does not run the managed bitcoind or if its version is
    /// already installed.
    pub fn new(
        datadir: &LianaDirectory,
        config: &Config,
        network: Network,
        wallet: &LianaWalletSettings,
    ) -> Option<(Self, Task<Message>)> {
        if !uses_managed_node(wallet, config) || !needs_upgrade(datadir, VERSION) {
            return None;
        }
        let mut upgrade = Self {
            datadir: datadir.clone(),
            config: config.clone(),
            network,
            wallet: wallet.clone(),
            lock: None,
            stage: Stage::Waiting,
            download: None,
        };
        let task = upgrade.check();
        Some((upgrade, task))
    }

    fn start_download(&mut self) -> Task<Message> {
        let (task, handle) = Task::run(
            download::download(bitcoind::download_url()),
            Message::Progress,
        )
        .abortable();
        self.download = Some(handle.abort_on_drop());
        task
    }

    /// Installs through a staging directory, so a failed install leaves no partial version.
    fn install(&self, bytes: Vec<u8>) -> Task<Message> {
        let directory = internal_bitcoind_directory(&self.datadir);
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let staging = directory.join(STAGING_DIRECTORY_NAME);
                    create_directory(&staging).map_err(|e| e.to_string())?;
                    let result = install_bitcoind(&staging, &bytes)
                        .map_err(|e| e.to_string())
                        .and_then(|()| {
                            if !bitcoind_exe_path(&staging, VERSION).is_file() {
                                return Err(t!("installer-bitcoind-executable-not-found"));
                            }
                            // A single rename, so the version never shows up half installed.
                            rename_with_retry(
                                &bitcoind_version_directory(&staging, VERSION),
                                &bitcoind_version_directory(&directory, VERSION),
                            )
                            .map_err(|e| e.to_string())
                        });
                    if let Err(e) = std::fs::remove_dir_all(&staging) {
                        tracing::warn!("Could not remove Bitcoin Core staging directory: {e}");
                    }
                    result
                })
                .await
                .map_err(|e| e.to_string())
                .and_then(|result| result)
            },
            Message::Installed,
        )
    }

    fn check(&mut self) -> Task<Message> {
        if self.lock.is_none() {
            match acquire_lock(&self.datadir) {
                Ok(Some(file)) => self.lock = Some(file),
                // Another wallet is running the upgrade, the subscription checks again every
                // second.
                Ok(None) => return Task::none(),
                Err(e) => {
                    self.stage = Stage::Failed(e);
                    return Task::none();
                }
            }
        }
        // Checked again under the lock, the previous holder may have installed it.
        if needs_upgrade(&self.datadir, VERSION) {
            self.stage = Stage::ReadyToUpgrade;
            Task::done(Message::StartDownload)
        } else {
            Task::done(Message::Continue)
        }
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match (message, &self.stage) {
            // Take the upgrade lock, then download the managed bitcoind if this wallet needs it.
            (Message::Check, Stage::Waiting) => return self.check(),
            (Message::Retry, Stage::Failed(_)) => {
                self.stage = Stage::Waiting;
                return self.check();
            }
            (Message::StartDownload, Stage::ReadyToUpgrade) => {
                self.stage = Stage::Downloading(0.0);
                return self.start_download();
            }
            (
                Message::Progress(Ok(Progress::Downloading(progress))),
                Stage::Downloading(previous),
            ) => {
                if progress < *previous {
                    tracing::warn!(
                        "Bitcoin Core download progress went back from {previous}% to {progress}%"
                    );
                } else {
                    self.stage = Stage::Downloading(progress);
                }
            }
            (Message::Progress(Ok(Progress::Finished(bytes))), Stage::Downloading(_)) => {
                self.stage = Stage::Installing;
                return self.install(bytes);
            }
            (Message::Progress(Err(e)), Stage::Downloading(_)) => {
                self.stage = Stage::Failed(e.to_string());
            }
            (Message::Installed(Ok(())), Stage::Installing) => {
                self.stage = Stage::Installed;
                self.lock = None;
                return Task::done(Message::Continue);
            }
            (Message::Installed(Err(e)), Stage::Installing) => {
                self.stage = Stage::Failed(e);
            }
            _ => {}
        }
        Task::none()
    }

    pub fn subscription(&self) -> Subscription<Message> {
        match self.stage {
            Stage::Waiting => iced::time::every(Duration::from_secs(1)).map(|_| Message::Check),
            _ => Subscription::none(),
        }
    }

    pub fn view(&self) -> Element<'_, Message> {
        let hint = matches!(
            self.stage,
            Stage::ReadyToUpgrade | Stage::Downloading(_) | Stage::Installing | Stage::Installed
        )
        .then(|| {
            column![
                new::caption(t!("bitcoind-upgrade-hint")).style(theme::text::secondary),
                new::caption(t!("bitcoind-upgrade-verified")).style(theme::text::secondary),
            ]
            .spacing(VSpacing::S)
            .into()
        });
        match &self.stage {
            Stage::Downloading(progress) => loading::progress(
                t!(
                    "installer-downloading-bitcoin-core-progress",
                    version = VERSION,
                    progress = format!("{progress:.2}")
                ),
                progress / 100.0 * DOWNLOAD_SHARE,
                hint,
            ),
            Stage::Installing => {
                loading::progress(t!("installer-installing-bitcoind"), DOWNLOAD_SHARE, hint)
            }
            Stage::Installed => loading::progress(
                t!("installer-installation-complete"),
                DAEMON_START_PROGRESS,
                hint,
            ),
            Stage::Failed(e) => {
                let actions = row![
                    Space::fill_width(),
                    btn_retry(Some(Message::Retry)),
                    btn_skip(Some(Message::Skip))
                ]
                .spacing(HSpacing::M);
                loading::centered(card::invalid(new::caption(e)), Some(actions.into()))
            }
            Stage::Waiting => loading::centered(new::caption(t!("bitcoind-upgrade-waiting")), None),
            Stage::ReadyToUpgrade => loading::centered(
                new::caption(t!("bitcoind-upgrade-title", version = VERSION)),
                hint,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        app::{
            config::Config,
            settings::{AuthConfig, LianaWalletSettings},
        },
        dir::{create_directory, LianaDirectory},
        gui::bitcoind_upgrade::{
            acquire_lock, needs_upgrade, uses_managed_node, Message, Stage, Upgrade,
            STAGING_DIRECTORY_NAME,
        },
        node::bitcoind::{
            tests::{install, test_directory},
            VERSION,
        },
    };
    use liana::miniscript::bitcoin::Network;

    fn wallet(remote_backend: bool, start_internal_bitcoind: Option<bool>) -> LianaWalletSettings {
        LianaWalletSettings {
            name: "wallet".to_string(),
            alias: None,
            descriptor_checksum: "checksum".to_string(),
            pinned_at: None,
            keys: Vec::new(),
            hardware_wallets: Vec::new(),
            remote_backend_auth: remote_backend.then(|| {
                AuthConfig::new(
                    "user".to_string(),
                    "user@example.com".to_string(),
                    "wallet".to_string(),
                )
            }),
            start_internal_bitcoind,
            fiat_price: None,
        }
    }

    /// Opens a managed bitcoind wallet and runs the upgrade check.
    fn check_managed_wallet(directory: &LianaDirectory) -> Upgrade {
        let (upgrade, _) = Upgrade::new(
            directory,
            &Config::new(false),
            Network::Bitcoin,
            &wallet(false, Some(true)),
        )
        .unwrap();
        upgrade
    }

    #[test]
    fn only_managed_node_wallets_are_upgraded() {
        // The wallet starts the managed bitcoind
        assert!(uses_managed_node(
            &wallet(false, Some(true)),
            &Config::new(false)
        ));
        // The wallet uses its own bitcoind or electrum
        assert!(!uses_managed_node(
            &wallet(false, Some(false)),
            &Config::new(true)
        ));
        // Legacy wallet settings fall back to gui.toml
        assert!(uses_managed_node(&wallet(false, None), &Config::new(true)));
        assert!(!uses_managed_node(
            &wallet(false, None),
            &Config::new(false)
        ));
        // Remote backend wallets never use the managed bitcoind
        assert!(!uses_managed_node(
            &wallet(true, Some(true)),
            &Config::new(true)
        ));
    }

    #[test]
    fn upgrade_offered_in_empty_directory() {
        let directory = test_directory(&[]);
        let upgrade = check_managed_wallet(&directory);
        assert!(matches!(upgrade.stage, Stage::ReadyToUpgrade));
    }

    #[test]
    fn stale_staging_directory_is_removed() {
        let directory = test_directory(&[]);
        let staging = directory
            .bitcoind_directory()
            .path()
            .join(STAGING_DIRECTORY_NAME);
        create_directory(&staging.join("bitcoin-31.1")).unwrap();

        check_managed_wallet(&directory);
        assert!(!staging.exists());
    }

    #[test]
    fn no_upgrade_screen_when_installed() {
        let directory = test_directory(&[VERSION]);
        assert!(Upgrade::new(
            &directory,
            &Config::new(false),
            Network::Bitcoin,
            &wallet(false, Some(true))
        )
        .is_none());
    }

    #[test]
    fn waiting_wallet_rechecks_under_lock() {
        let directory = test_directory(&[]);
        let upgrading = check_managed_wallet(&directory);

        let mut waiting = check_managed_wallet(&directory);
        assert!(matches!(waiting.stage, Stage::Waiting));
        assert!(waiting.lock.is_none());

        // The other wallet installed the version: nothing left to do.
        install(&directory, VERSION);
        drop(upgrading);
        let _ = waiting.check();
        assert!(waiting.lock.is_some());
        assert!(matches!(waiting.stage, Stage::Waiting));
    }

    #[test]
    fn waiting_wallet_upgrades_if_other_wallet_did_not() {
        let directory = test_directory(&[]);
        let upgrading = check_managed_wallet(&directory);
        let mut waiting = check_managed_wallet(&directory);

        drop(upgrading);
        let _ = waiting.check();
        assert!(matches!(waiting.stage, Stage::ReadyToUpgrade));
    }

    #[test]
    fn installation_releases_lock_before_continuing() {
        let directory = test_directory(&[]);
        let mut upgrade = check_managed_wallet(&directory);
        assert!(acquire_lock(&directory).unwrap().is_none());

        upgrade.stage = Stage::Installing;
        let _ = upgrade.update(Message::Installed(Ok(())));
        assert!(matches!(upgrade.stage, Stage::Installed));
        assert!(acquire_lock(&directory).unwrap().is_some());
    }

    #[test]
    fn installed_bitcoind_version() {
        let directory = test_directory(&[]);

        // No installed bitcoind, we install 31.1
        assert!(needs_upgrade(&directory, "31.1"));

        install(&directory, "31.1");

        // 31.1 is now installed
        assert!(!needs_upgrade(&directory, "31.1"));
    }

    #[test]
    fn older_liana_installs_its_own_bitcoind() {
        let directory = test_directory(&[]);

        // A newer Liana installs Bitcoin Core 32.0.
        assert!(needs_upgrade(&directory, "32.0"));
        install(&directory, "32.0");
        assert!(!needs_upgrade(&directory, "32.0"));

        // An older Liana still installs its own Bitcoin Core 31.1.
        assert!(needs_upgrade(&directory, "31.1"));
        install(&directory, "31.1");
        assert!(!needs_upgrade(&directory, "31.1"));
        assert!(!needs_upgrade(&directory, "32.0"));
    }
}
