mod bitcoind;
mod wallet;

use std::convert::From;
use std::sync::Arc;

use iced::Task;

use liana_ui::{component::form, widget::Element};

use bitcoind::BitcoindSettingsState;
use wallet::{update_aliases, WalletSettingsState};

use crate::{
    app::{
        cache::Cache,
        error::Error,
        message::Message,
        state::State,
        view::{self},
        wallet::Wallet,
        Config,
    },
    daemon::{Daemon, DaemonBackend},
    dir::LianaDirectory,
    export::{ImportExportMessage, ImportExportType},
};

use super::export::ExportModal;

pub struct SettingsState {
    data_dir: LianaDirectory,
    wallet: Arc<Wallet>,
    setting: Option<Box<dyn State>>,
    daemon_backend: DaemonBackend,
    internal_bitcoind: bool,
    config: Arc<Config>,
}

impl SettingsState {
    pub fn new(
        data_dir: LianaDirectory,
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
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
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
                    .map(|s| s.reload(daemon, wallet))
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
                    .map(|s| s.reload(daemon, wallet))
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
                    .map(|s| s.reload(daemon, wallet))
                    .unwrap_or_else(Task::none)
            }
            Message::WalletUpdated(Ok(wallet)) => {
                self.wallet = wallet.clone();
                self.setting
                    .as_mut()
                    .map(|s| s.update(daemon, cache, message))
                    .unwrap_or_else(Task::none)
            }
            _ => self
                .setting
                .as_mut()
                .map(|s| s.update(daemon, cache, message))
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

    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        if let Some(setting) = &self.setting {
            setting.view(cache)
        } else {
            view::settings::list(cache, self.daemon_backend == DaemonBackend::RemoteBackend)
        }
    }

    fn reload(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        wallet: Arc<Wallet>,
    ) -> Task<Message> {
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
    warning: Option<Error>,
    modal: Option<ExportModal>,
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
    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        let content = view::settings::import_export(cache, self.warning.as_ref());
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
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
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
                            daemon,
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
            Message::View(view::Message::Settings(view::SettingsMessage::ExportDescriptor)) => {
                if self.modal.is_none() {
                    let modal = ExportModal::new(
                        Some(daemon),
                        ImportExportType::Descriptor(self.wallet.main_descriptor.clone()),
                    );
                    launch!(self, modal, true);
                }
            }
            Message::View(view::Message::Settings(view::SettingsMessage::ExportTransactions)) => {
                if self.modal.is_none() {
                    let modal = ExportModal::new(Some(daemon), ImportExportType::Transactions);
                    launch!(self, modal, true);
                }
            }
            Message::View(view::Message::Settings(view::SettingsMessage::ExportLabels)) => {
                if self.modal.is_none() {
                    let modal = ExportModal::new(Some(daemon), ImportExportType::ExportLabels);
                    launch!(self, modal, true);
                }
            }
            Message::View(view::Message::Settings(view::SettingsMessage::ExportWallet)) => {
                if self.modal.is_none() {
                    let datadir = cache.datadir_path.clone();
                    let network = cache.network;
                    let config = self.config.clone();
                    let wallet = self.wallet.clone();
                    let daemon = daemon.clone();
                    let modal = ExportModal::new(
                        Some(daemon),
                        ImportExportType::ExportProcessBackup(datadir, network, config, wallet),
                    );
                    launch!(self, modal, true);
                }
            }
            Message::View(view::Message::Settings(view::SettingsMessage::ImportWallet)) => {
                if self.modal.is_none() {
                    let modal = ExportModal::new(
                        Some(daemon),
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
    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        view::settings::about_section(cache, self.warning.as_ref(), self.daemon_version.as_ref())
    }

    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        if let Message::Info(res) = message {
            match res {
                Ok(info) => {
                    if daemon.backend() == DaemonBackend::RemoteBackend {
                        self.daemon_version = None;
                    } else {
                        self.daemon_version = Some(info.version)
                    }
                }
                Err(e) => self.warning = Some(e),
            }
        }

        Task::none()
    }

    fn reload(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        _wallet: Arc<Wallet>,
    ) -> Task<Message> {
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
    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        view::settings::remote_backend_section(
            cache,
            &self.email_form,
            self.processing,
            self.success,
            self.warning.as_ref(),
        )
    }

    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
        message: Message,
    ) -> Task<Message> {
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
                    Ok(()) => self.success = true,
                    Err(e) => {
                        self.success = false;
                        self.warning = Some(e);
                    }
                }
                Task::none()
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
