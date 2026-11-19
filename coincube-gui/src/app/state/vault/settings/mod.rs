pub(crate) mod bitcoind;
mod wallet;

use std::convert::From;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use iced::Task;

use coincube_ui::widget::Element;

use bitcoind::BitcoindSettingsState;
use wallet::{update_aliases, WalletSettingsState};

use crate::app::state::settings::local_signing::LocalSigningState;

use crate::{
    app::{
        cache::Cache,
        error::Error,
        menu::Menu,
        message::Message,
        state::State,
        view::{self},
        wallet::Wallet,
        Config,
    },
    daemon::Daemon,
    dir::CoincubeDirectory,
    export::{ImportExportMessage, ImportExportType},
};

use super::export::VaultExportModal;

pub struct SettingsState {
    data_dir: CoincubeDirectory,
    wallet: Arc<Wallet>,
    setting: Option<Box<dyn State>>,
    internal_bitcoind: bool,
    managed_owner: Arc<Mutex<Option<PathBuf>>>,
    config: Arc<Config>,
}

impl SettingsState {
    pub fn new(
        data_dir: CoincubeDirectory,
        wallet: Arc<Wallet>,
        internal_bitcoind: bool,
        config: Arc<Config>,
    ) -> Self {
        Self {
            data_dir,
            wallet,
            setting: None,
            internal_bitcoind,
            managed_owner: Arc::new(Mutex::new(None)),
            config,
        }
    }

    pub(crate) fn with_managed_owner(mut self, owner: Option<PathBuf>) -> Self {
        self.set_managed_owner(owner);
        self
    }

    pub(crate) fn set_managed_owner(&mut self, owner: Option<PathBuf>) {
        *self.managed_owner.lock().unwrap_or_else(|e| e.into_inner()) = owner;
    }

    #[cfg(test)]
    pub(crate) fn managed_owner_path(&self) -> Option<PathBuf> {
        self.managed_owner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

impl State for SettingsState {
    fn update(
        &mut self,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        // A device registration started in Wallet settings finishes there
        // even if the daemon stopped while the device answered.
        if matches!(message, Message::DeviceRegistered { .. }) {
            return self
                .setting
                .as_mut()
                .map(|s| s.update(daemon, cache, message))
                .unwrap_or_else(Task::none);
        }
        let Some(daemon) = daemon else {
            tracing::warn!("SettingsState::update called without daemon");
            return Task::none();
        };
        match &message {
            Message::View(view::Message::Settings(view::SettingsMessage::EditBitcoindSettings)) => {
                self.setting = Some(
                    BitcoindSettingsState::new(
                        daemon.config().cloned(),
                        cache,
                        !daemon.backend().is_embedded(),
                        self.internal_bitcoind,
                    )
                    .with_managed_owner(self.managed_owner.clone())
                    .into(),
                );
                let wallet = self.wallet.clone();
                self.setting
                    .as_mut()
                    .map(|s| s.reload(Some(daemon), Some(wallet)))
                    .unwrap_or_else(Task::none)
            }
            Message::View(view::Message::Settings(view::SettingsMessage::ImportExportSection)) => {
                self.setting = Some(
                    ImportExportSettingsState::new(self.wallet.clone(), self.config.clone()).into(),
                );
                Task::none()
            }
            Message::View(view::Message::Settings(view::SettingsMessage::EditWalletSettings)) => {
                self.setting = Some(
                    WalletSettingsState::new(
                        self.data_dir.clone(),
                        self.wallet.clone(),
                        self.config.clone(),
                    )
                    .into(),
                );
                let wallet = self.wallet.clone();
                self.setting
                    .as_mut()
                    .map(|s| s.reload(Some(daemon), Some(wallet)))
                    .unwrap_or_else(Task::none)
            }
            Message::View(view::Message::Settings(view::SettingsMessage::LocalSigningSection)) => {
                self.setting = Some(LocalSigningState::default().into());
                let wallet = self.wallet.clone();
                self.setting
                    .as_mut()
                    .map(|s| s.reload(Some(daemon), Some(wallet)))
                    .unwrap_or_else(Task::none)
            }
            Message::WalletUpdated(Ok(wallet)) => {
                self.wallet = wallet.clone();
                self.setting
                    .as_mut()
                    .map(|s| s.update(Some(daemon), cache, message))
                    .unwrap_or_else(Task::none)
            }
            _ => self
                .setting
                .as_mut()
                .map(|s| s.update(Some(daemon), cache, message))
                .unwrap_or_else(Task::none),
        }
    }

    fn subscription(&self) -> iced::Subscription<Message> {
        if let Some(setting) = &self.setting {
            setting.subscription()
        } else {
            iced::Subscription::none()
        }
    }

    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        if let Some(setting) = &self.setting {
            setting.view(menu, cache)
        } else {
            view::vault::settings::list(menu, cache)
        }
    }

    fn reload(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        wallet: Option<Arc<Wallet>>,
    ) -> Task<Message> {
        let Some(wallet) = wallet else {
            tracing::warn!("SettingsState::reload called without wallet");
            return Task::none();
        };
        self.setting = None;
        self.wallet = wallet;
        Task::none()
    }
}

impl From<SettingsState> for Box<dyn State> {
    fn from(s: SettingsState) -> Box<dyn State> {
        Box::new(s)
    }
}

pub struct ImportExportSettingsState {
    #[allow(dead_code)] // Reserved for future error handling
    warning: Option<Error>,
    modal: Option<VaultExportModal>,
    plaintext_export_review: bool,
    wallet: Arc<Wallet>,
    config: Arc<Config>,
}

impl ImportExportSettingsState {
    pub fn new(wallet: Arc<Wallet>, config: Arc<Config>) -> Self {
        Self {
            warning: None,
            modal: None,
            plaintext_export_review: false,
            wallet,
            config,
        }
    }
}

macro_rules! launch {
    ($s:ident, $m: ident, $write:ident) => {
        let launch = $m.launch($write);
        $s.modal = Some($m);
        return launch
    };
}

impl ImportExportSettingsState {
    fn export_encrypted_descriptor(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
    ) -> Task<Message> {
        if self.modal.is_none() {
            let Some(descriptor) = self.wallet.inheritance_descriptor().cloned() else {
                return Task::none();
            };
            let modal = VaultExportModal::new(
                Some(daemon),
                ImportExportType::ExportEncryptedDescriptor(Box::new(descriptor)),
            );
            launch!(self, modal, true);
        }
        Task::none()
    }

    fn export_plaintext_descriptor(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
    ) -> Task<Message> {
        if self.plaintext_export_review && self.modal.is_none() {
            self.plaintext_export_review = false;
            let Some(descriptor) = self.wallet.inheritance_descriptor().cloned() else {
                return Task::none();
            };
            let modal =
                VaultExportModal::new(Some(daemon), ImportExportType::Descriptor(descriptor));
            launch!(self, modal, true);
        }
        Task::none()
    }

    fn export_wallet_backup(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
    ) -> Task<Message> {
        if !self.wallet.supports_inheritance() {
            return Task::none();
        }
        if self.modal.is_none() {
            let datadir = cache.datadir_path.clone();
            let config = self.config.clone();
            let wallet = self.wallet.clone();
            let daemon_clone = daemon.clone();
            let modal = VaultExportModal::new(
                Some(daemon_clone),
                ImportExportType::ExportProcessBackup(datadir, config, wallet),
            );
            launch!(self, modal, true);
        }
        Task::none()
    }
}

impl State for ImportExportSettingsState {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        let content =
            view::vault::settings::import_export(menu, cache, self.wallet.supports_inheritance());
        if self.plaintext_export_review {
            return coincube_ui::widget::modal::Modal::new(
                content,
                view::vault::settings::plaintext_export_review(),
            )
            .on_blur(Some(view::Message::Settings(
                view::SettingsMessage::CancelPlaintextDescriptorExport,
            )))
            .into();
        }
        if let Some(modal) = &self.modal {
            modal.view(content)
        } else {
            content
        }
    }

    fn subscription(&self) -> iced::Subscription<Message> {
        if let Some(modal) = &self.modal {
            if let Some(sub) = modal.subscription() {
                return sub.map(|m| {
                    Message::View(view::Message::Settings(
                        view::SettingsMessage::ImportExport(ImportExportMessage::Progress(m)),
                    ))
                });
            }
        }
        iced::Subscription::none()
    }

    fn update(
        &mut self,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        let Some(daemon) = daemon else {
            tracing::warn!("ImportExportSettingsState::update called without daemon");
            return Task::none();
        };
        match message {
            Message::View(view::Message::Settings(
                view::SettingsMessage::ReviewPlaintextDescriptorExport,
            )) => {
                self.plaintext_export_review =
                    self.modal.is_none() && self.wallet.inheritance_descriptor().is_some();
                return Task::none();
            }
            Message::View(view::Message::Settings(
                view::SettingsMessage::CancelPlaintextDescriptorExport,
            )) => {
                self.plaintext_export_review = false;
                return Task::none();
            }
            Message::View(view::Message::ImportExport(ImportExportMessage::Close)) => {
                self.modal = None;
            }
            Message::View(view::Message::ImportExport(m)) => {
                if let ImportExportMessage::UpdateAliases(aliases) = m {
                    return Task::perform(
                        update_aliases(
                            cache.datadir_path.clone(),
                            self.wallet.clone(),
                            None,
                            aliases.into_iter().map(|(fg, ks)| (fg, ks.name)).collect(),
                        ),
                        Message::WalletUpdated,
                    );
                } else if let Some(modal) = self.modal.as_mut() {
                    return modal.update(m);
                }
            }
            Message::View(view::Message::Settings(view::SettingsMessage::ImportExport(m))) => {
                if let Some(modal) = self.modal.as_mut() {
                    return modal.update(m);
                }
            }
            Message::View(view::Message::Settings(
                view::SettingsMessage::ExportEncryptedDescriptor,
            )) => {
                return self.export_encrypted_descriptor(daemon);
            }
            Message::View(view::Message::Settings(
                view::SettingsMessage::ExportPlaintextDescriptor,
            )) => {
                return self.export_plaintext_descriptor(daemon);
            }
            Message::View(view::Message::Settings(view::SettingsMessage::ExportTransactions)) => {
                if self.modal.is_none() {
                    let modal = VaultExportModal::new(Some(daemon), ImportExportType::Transactions);
                    launch!(self, modal, true);
                }
            }
            Message::View(view::Message::Settings(view::SettingsMessage::ExportLabels)) => {
                if self.modal.is_none() {
                    let modal = VaultExportModal::new(Some(daemon), ImportExportType::ExportLabels);
                    launch!(self, modal, true);
                }
            }
            Message::View(view::Message::Settings(view::SettingsMessage::ExportWallet)) => {
                return self.export_wallet_backup(daemon, cache);
            }
            Message::View(view::Message::Settings(view::SettingsMessage::ImportWallet))
                if self.modal.is_none() && self.wallet.supports_inheritance() =>
            {
                let modal = VaultExportModal::new(
                    Some(daemon.clone()),
                    ImportExportType::ImportBackup {
                        network_dir: cache.datadir_path.network_directory(self.wallet.chain),
                        wallet: self.wallet.clone(),
                        overwrite_labels: None,
                        overwrite_aliases: None,
                    },
                );
                launch!(self, modal, false);
            }
            _ => {}
        }

        Task::none()
    }
}

impl From<ImportExportSettingsState> for Box<dyn State> {
    fn from(s: ImportExportSettingsState) -> Box<dyn State> {
        Box::new(s)
    }
}

#[cfg(test)]
mod chain_tests {
    use super::*;

    #[tokio::test]
    async fn plaintext_export_requires_review_and_an_inheritance_descriptor() {
        let descriptor = crate::app::state::vault::test_support::unified::fixture().descriptor;
        let wallet = Arc::new(Wallet::new(descriptor).with_chain(crate::chain::ChainId::Bitcoin));
        let daemon: Arc<dyn Daemon + Sync + Send> = Arc::new(
            crate::daemon::client::Coincubed::new(crate::utils::mock::Daemon::new(vec![]).run()),
        );
        let config = Arc::new(Config::new(false));
        let cache = Cache::default();
        let mut state = ImportExportSettingsState::new(wallet.clone(), config.clone());
        let send = |state: &mut ImportExportSettingsState, intent| {
            state.update(
                Some(daemon.clone()),
                &cache,
                Message::View(view::Message::Settings(intent)),
            )
        };

        assert!(iced_runtime::task::into_stream(send(
            &mut state,
            view::SettingsMessage::ExportPlaintextDescriptor,
        ))
        .is_none());
        assert!(state.modal.is_none());
        drop(send(
            &mut state,
            view::SettingsMessage::ReviewPlaintextDescriptorExport,
        ));
        drop(send(
            &mut state,
            view::SettingsMessage::CancelPlaintextDescriptorExport,
        ));
        assert!(iced_runtime::task::into_stream(send(
            &mut state,
            view::SettingsMessage::ExportPlaintextDescriptor,
        ))
        .is_none());
        assert!(state.modal.is_none());

        let mut reentered = ImportExportSettingsState::new(wallet, config.clone());
        assert!(!reentered.plaintext_export_review);
        let intent = |intent| Message::View(view::Message::Settings(intent));
        drop(reentered.update(
            Some(daemon.clone()),
            &cache,
            intent(view::SettingsMessage::ReviewPlaintextDescriptorExport),
        ));
        let launch = reentered.update(
            Some(daemon.clone()),
            &cache,
            intent(view::SettingsMessage::ExportPlaintextDescriptor),
        );
        assert!(iced_runtime::task::into_stream(launch).is_some());
        assert_eq!(
            reentered.modal.as_ref().map(VaultExportModal::modal_title),
            Some("Export Descriptor"),
        );
        assert!(!reentered.plaintext_export_review);
        assert!(iced_runtime::task::into_stream(reentered.update(
            Some(daemon.clone()),
            &cache,
            intent(view::SettingsMessage::ExportPlaintextDescriptor),
        ))
        .is_none());

        let secp = coincube_core::miniscript::bitcoin::secp256k1::Secp256k1::new();
        let master = coincube_core::miniscript::bitcoin::bip32::Xpriv::new_master(
            coincube_core::miniscript::bitcoin::Network::Bitcoin,
            &[44; 32],
        )
        .unwrap();
        let path: coincube_core::miniscript::bitcoin::bip32::DerivationPath =
            "m/84'/0'/0'".parse().unwrap();
        let account = master.derive_priv(&secp, &path).unwrap();
        let key = format!(
            "[{}/84'/0'/0']{}",
            master.fingerprint(&secp),
            coincube_core::miniscript::bitcoin::bip32::Xpub::from_priv(&secp, &account)
        );
        let imported = coincube_core::wallet_descriptor::ImportedWalletDescriptor::parse(
            &format!("wpkh({key}/0/*)"),
            Some(&format!("wpkh({key}/1/*)")),
            crate::chain::ChainId::Bitcoin,
        )
        .unwrap();
        let imported_wallet = Arc::new(Wallet::new(
            coincube_core::wallet_descriptor::WalletDescriptor::Imported(imported),
        ));
        assert!(!imported_wallet.supports_inheritance());
        let mut ineligible = ImportExportSettingsState::new(imported_wallet, config);
        drop(ineligible.update(
            Some(daemon.clone()),
            &cache,
            intent(view::SettingsMessage::ReviewPlaintextDescriptorExport),
        ));
        assert!(!ineligible.plaintext_export_review);
        assert!(iced_runtime::task::into_stream(ineligible.update(
            Some(daemon.clone()),
            &cache,
            intent(view::SettingsMessage::ExportPlaintextDescriptor),
        ))
        .is_none());
        assert!(ineligible.modal.is_none());
    }

    #[test]
    fn open_node_settings_reads_replaced_owner_token() {
        let path = std::env::temp_dir().join(format!("settings-owner-{}", uuid::Uuid::new_v4()));
        let datadir = CoincubeDirectory::new(path.clone());
        let cache = Cache {
            datadir_path: datadir.clone(),
            ..Cache::default()
        };
        let descriptor = crate::app::state::vault::test_support::unified::fixture().descriptor;
        let wallet = Arc::new(Wallet::new(descriptor));
        let mut settings = SettingsState::new(datadir, wallet, true, Arc::new(Config::new(false)))
            .with_managed_owner(Some(PathBuf::from("old-owner.lock")));
        let open = BitcoindSettingsState::new(None, &cache, false, true)
            .with_managed_owner(settings.managed_owner.clone());
        settings.set_managed_owner(Some(PathBuf::from("replacement-owner.lock")));
        assert_eq!(
            open.managed_owner_path(),
            Some(PathBuf::from("replacement-owner.lock"))
        );
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn fork_node_navigation_opens_the_guarded_page() {
        use crate::chain::ChainId;
        for chain in [ChainId::BitcoinBlake2b, ChainId::BitcoinBlake2bTestnet4] {
            let descriptor = crate::app::state::vault::test_support::unified::fixture().descriptor;
            let wallet = Arc::new(Wallet::new(descriptor.clone()).with_chain(chain));
            let dir = CoincubeDirectory::new(std::env::temp_dir().join(format!(
                "coincube-fork-node-navigation-{}",
                uuid::Uuid::new_v4()
            )));
            let config = coincubed::config::Config::new(
                coincubed::config::BitcoinConfig::new(chain, std::time::Duration::from_secs(600)),
                Some(coincubed::config::BitcoinBackend::Esplora(
                    crate::installer::connect_esplora_config(chain, "synthetic-jwt"),
                )),
                log::LevelFilter::Info,
                descriptor,
                coincubed::datadir::DataDirectory::new(
                    dir.path().join(chain.dir_name()).join("data/wallet"),
                ),
            );
            let daemon = Arc::new(crate::daemon::embedded::EmbeddedDaemon::unstarted_for_test(
                config, None,
            ));
            let mut state =
                SettingsState::new(dir.clone(), wallet, false, Arc::new(Config::new(false)));
            let cache = Cache {
                fiat_chain: chain,
                network: chain.bitcoin_network(),
                datadir_path: dir.clone(),
                ..Cache::default()
            };
            drop(state.update(
                Some(daemon),
                &cache,
                Message::View(view::Message::Settings(
                    view::SettingsMessage::EditBitcoindSettings,
                )),
            ));
            assert!(state.setting.is_some(), "Node navigation was refused");
            let _page = state.view(
                &Menu::Vault(crate::app::menu::VaultSubMenu::Settings(Some(
                    crate::app::menu::SettingsOption::Node,
                ))),
                &cache,
            );
            assert!(
                !dir.path().exists(),
                "navigation must not start a node or write config"
            );
        }
    }
}
