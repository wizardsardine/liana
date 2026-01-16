mod bitcoind;
mod wallet;

use std::convert::From;
use std::sync::Arc;

use iced::Task;

use coincube_ui::{component::form, widget::Element};

use bitcoind::BitcoindSettingsState;
use wallet::{update_aliases, WalletSettingsState};

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
    daemon::{Daemon, DaemonBackend},
    dir::CoincubeDirectory,
    export::{ImportExportMessage, ImportExportType},
};

use super::export::VaultExportModal;

pub struct SettingsState {
    data_dir: CoincubeDirectory,
    wallet: Arc<Wallet>,
    setting: Option<Box<dyn State>>,
    daemon_backend: DaemonBackend,
    internal_bitcoind: bool,
    config: Arc<Config>,
}

impl SettingsState {
    pub fn new(
        data_dir: CoincubeDirectory,
        wallet: Arc<Wallet>,
        daemon_backend: DaemonBackend,
        internal_bitcoind: bool,
        config: Arc<Config>,
    ) -> Self {
        Self {
            data_dir,
            wallet,
            setting: None,
            daemon_backend,
            internal_bitcoind,
            config,
        }
    }
}

impl State for SettingsState {
    fn update(
        &mut self,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        let daemon = daemon.expect("Daemon required for vault settings");
        match &message {
            Message::View(view::Message::Settings(view::SettingsMessage::EditBitcoindSettings)) => {
                self.setting = Some(
                    BitcoindSettingsState::new(
                        daemon.config().cloned(),
                        cache,
                        !daemon.backend().is_embedded(),
                        self.internal_bitcoind,
                    )
                    .into(),
                );
                let wallet = self.wallet.clone();
                self.setting
                    .as_mut()
                    .map(|s| s.reload(Some(daemon), Some(wallet)))
                    .unwrap_or_else(Task::none)
            }
            Message::View(view::Message::Settings(
                view::SettingsMessage::EditRemoteBackendSettings,
            )) => {
                self.setting = Some(BackendSettingsState::new().into());
                Task::none()
            }
            Message::View(view::Message::Settings(view::SettingsMessage::ImportExportSection)) => {
                self.setting = Some(
                    ImportExportSettingsState::new(self.wallet.clone(), self.config.clone()).into(),
                );
                Task::none()
            }
            Message::View(view::Message::Settings(view::SettingsMessage::AboutSection)) => {
                self.setting = Some(AboutSettingsState::default().into());
                let wallet = self.wallet.clone();
                self.setting
                    .as_mut()
                    .map(|s| s.reload(Some(daemon), Some(wallet)))
                    .unwrap_or_else(Task::none)
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
            view::vault::settings::list(
                menu,
                cache,
                self.daemon_backend == DaemonBackend::RemoteBackend,
            )
        }
    }

    fn reload(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        wallet: Option<Arc<Wallet>>,
    ) -> Task<Message> {
        let wallet = wallet.expect("Vault panels require wallet");
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
    wallet: Arc<Wallet>,
    config: Arc<Config>,
}

impl ImportExportSettingsState {
    pub fn new(wallet: Arc<Wallet>, config: Arc<Config>) -> Self {
        Self {
            warning: None,
            modal: None,
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

impl State for ImportExportSettingsState {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        let content = view::vault::settings::import_export(menu, cache, None); // Errors now shown via global toast
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
        let daemon = daemon.expect("Daemon required for vault import/export settings");
        match message {
            Message::View(view::Message::ImportExport(ImportExportMessage::Close)) => {
                self.modal = None;
            }
            Message::View(view::Message::ImportExport(m)) => {
                if let ImportExportMessage::UpdateAliases(aliases) = m {
                    return Task::perform(
                        update_aliases(
                            cache.datadir_path.clone(),
                            cache.network,
                            self.wallet.clone(),
                            None,
                            aliases.into_iter().map(|(fg, ks)| (fg, ks.name)).collect(),
                            daemon.clone(),
                        ),
                        Message::WalletUpdated,
                    );
                } else if let Some(modal) = self.modal.as_mut() {
                    return modal.update(m);
                };
            }
            Message::View(view::Message::Settings(view::SettingsMessage::ImportExport(m))) => {
                if let Some(modal) = self.modal.as_mut() {
                    return modal.update(m);
                };
            }
            Message::View(view::Message::Settings(
                view::SettingsMessage::ExportEncryptedDescriptor,
            )) => {
                if self.modal.is_none() {
                    let modal = VaultExportModal::new(
                        Some(daemon),
                        ImportExportType::ExportEncryptedDescriptor(Box::new(
                            self.wallet.main_descriptor.clone(),
                        )),
                    );
                    launch!(self, modal, true);
                }
            }
            Message::View(view::Message::Settings(
                view::SettingsMessage::ExportPlaintextDescriptor,
            )) => {
                if self.modal.is_none() {
                    let modal = VaultExportModal::new(
                        Some(daemon),
                        ImportExportType::Descriptor(self.wallet.main_descriptor.clone()),
                    );
                    launch!(self, modal, true);
                }
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
                if self.modal.is_none() {
                    let datadir = cache.datadir_path.clone();
                    let network = cache.network;
                    let config = self.config.clone();
                    let wallet = self.wallet.clone();
                    let daemon_clone = daemon.clone();
                    let modal = VaultExportModal::new(
                        Some(daemon_clone),
                        ImportExportType::ExportProcessBackup(datadir, network, config, wallet),
                    );
                    launch!(self, modal, true);
                }
            }
            Message::View(view::Message::Settings(view::SettingsMessage::ImportWallet)) => {
                if self.modal.is_none() {
                    let modal = VaultExportModal::new(
                        Some(daemon.clone()),
                        ImportExportType::ImportBackup {
                            network_dir: cache.datadir_path.network_directory(cache.network),
                            wallet: self.wallet.clone(),
                            overwrite_labels: None,
                            overwrite_aliases: None,
                        },
                    );
                    launch!(self, modal, false);
                }
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

#[derive(Default)]
pub struct AboutSettingsState {
    daemon_version: Option<String>,
    warning: Option<Error>,
}

impl State for AboutSettingsState {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        view::vault::settings::about_section(
            menu,
            cache,
            None, // Errors now shown via global toast
            self.daemon_version.as_ref(),
        )
    }

    fn update(
        &mut self,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        _cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        let daemon = daemon.expect("Daemon required for vault about settings");
        if let Message::Info(res) = message {
            match res {
                Ok(info) => {
                    if daemon.backend() == DaemonBackend::RemoteBackend {
                        self.daemon_version = None;
                    } else {
                        self.daemon_version = Some(info.version)
                    }
                }
                Err(e) => {
                    let err_msg = e.to_string();
                    self.warning = Some(e);
                    return Task::done(Message::View(view::Message::ShowError(err_msg)));
                }
            }
        }

        Task::none()
    }

    fn reload(
        &mut self,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        _wallet: Option<Arc<Wallet>>,
    ) -> Task<Message> {
        let daemon = daemon.expect("Vault panels require daemon");
        Task::perform(
            async move { daemon.get_info().await.map_err(|e| e.into()) },
            Message::Info,
        )
    }
}

impl From<AboutSettingsState> for Box<dyn State> {
    fn from(s: AboutSettingsState) -> Box<dyn State> {
        Box::new(s)
    }
}

#[derive(Default)]
pub struct BackendSettingsState {
    email_form: form::Value<String>,
    processing: bool,
    success: bool,
    warning: Option<Error>,
}

impl BackendSettingsState {
    pub fn new() -> Self {
        Self {
            email_form: form::Value::default(),
            processing: false,
            success: false,
            warning: None,
        }
    }
}

impl State for BackendSettingsState {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        view::vault::settings::remote_backend_section(
            menu,
            cache,
            &self.email_form,
            self.processing,
            self.success,
            None, // Errors now shown via global toast
        )
    }

    fn update(
        &mut self,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        _cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        let daemon = daemon.expect("Daemon required for vault backend settings");
        match message {
            Message::View(view::Message::Settings(
                view::SettingsMessage::RemoteBackendSettings(message),
            )) => match message {
                view::RemoteBackendSettingsMessage::SendInvitation => {
                    if !self.email_form.valid {
                        return Task::none();
                    }
                    let email = self.email_form.value.clone();
                    self.processing = true;
                    self.success = false;
                    self.warning = None;
                    Task::perform(
                        async move {
                            daemon.send_wallet_invitation(&email).await?;
                            Ok(())
                        },
                        Message::Updated,
                    )
                }
                view::RemoteBackendSettingsMessage::EditInvitationEmail(email) => {
                    if !self.processing {
                        self.email_form.valid = email_address::EmailAddress::parse_with_options(
                            &email,
                            email_address::Options::default().with_required_tld(),
                        )
                        .is_ok();
                        self.email_form.value = email;
                        self.success = false;
                    }
                    Task::none()
                }
            },
            Message::Updated(res) => {
                self.processing = false;
                match res {
                    Ok(()) => {
                        self.success = true;
                        Task::none()
                    }
                    Err(e) => {
                        self.success = false;
                        let err_msg = e.to_string();
                        self.warning = Some(e);
                        Task::done(Message::View(view::Message::ShowError(err_msg)))
                    }
                }
            }
            _ => Task::none(),
        }
    }
}

impl From<BackendSettingsState> for Box<dyn State> {
    fn from(s: BackendSettingsState) -> Box<dyn State> {
        Box::new(s)
    }
}
