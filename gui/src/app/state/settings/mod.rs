mod bitcoind;
mod wallet;

use std::convert::From;
use std::path::PathBuf;
use std::sync::Arc;

use iced::Command;

use liana_ui::{component::form, widget::Element};

use bitcoind::BitcoindSettingsState;
use wallet::WalletSettingsState;

use crate::{
    app::{
        cache::Cache,
        error::Error,
        message::Message,
        state::State,
        view::{self},
        wallet::Wallet,
    },
    daemon::{Daemon, DaemonBackend},
};

pub struct SettingsState {
    data_dir: PathBuf,
    wallet: Arc<Wallet>,
    setting: Option<Box<dyn State>>,
    daemon_backend: DaemonBackend,
    internal_bitcoind: bool,
}

impl SettingsState {
    pub fn new(
        data_dir: PathBuf,
        wallet: Arc<Wallet>,
        daemon_backend: DaemonBackend,
        internal_bitcoind: bool,
    ) -> Self {
        Self {
            data_dir,
            wallet,
            setting: None,
            daemon_backend,
            internal_bitcoind,
        }
    }
}

impl State for SettingsState {
    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        message: Message,
    ) -> Command<Message> {
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
                    .unwrap_or_else(Command::none)
            }
            Message::View(view::Message::Settings(
                view::SettingsMessage::EditRemoteBackendSettings,
            )) => {
                self.setting = Some(BackendSettingsState::new().into());
                Command::none()
            }
            Message::View(view::Message::Settings(view::SettingsMessage::AboutSection)) => {
                self.setting = Some(AboutSettingsState::default().into());
                let wallet = self.wallet.clone();
                self.setting
                    .as_mut()
                    .map(|s| s.reload(daemon, wallet))
                    .unwrap_or_else(Command::none)
            }
            Message::View(view::Message::Settings(view::SettingsMessage::EditWalletSettings)) => {
                self.setting = Some(
                    WalletSettingsState::new(self.data_dir.clone(), self.wallet.clone()).into(),
                );
                let wallet = self.wallet.clone();
                self.setting
                    .as_mut()
                    .map(|s| s.reload(daemon, wallet))
                    .unwrap_or_else(Command::none)
            }
            Message::WalletUpdated(Ok(wallet)) => {
                self.wallet = wallet.clone();
                self.setting
                    .as_mut()
                    .map(|s| s.update(daemon, cache, message))
                    .unwrap_or_else(Command::none)
            }
            _ => self
                .setting
                .as_mut()
                .map(|s| s.update(daemon, cache, message))
                .unwrap_or_else(Command::none),
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
    ) -> Command<Message> {
        self.setting = None;
        self.wallet = wallet;
        Command::none()
    }
}

impl From<SettingsState> for Box<dyn State> {
    fn from(s: SettingsState) -> Box<dyn State> {
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
    ) -> Command<Message> {
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

        Command::none()
    }

    fn reload(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        _wallet: Arc<Wallet>,
    ) -> Command<Message> {
        Command::perform(
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
    ) -> Command<Message> {
        match message {
            Message::View(view::Message::Settings(
                view::SettingsMessage::RemoteBackendSettings(message),
            )) => match message {
                view::RemoteBackendSettingsMessage::SendInvitation => {
                    if !self.email_form.valid {
                        return Command::none();
                    }
                    let email = self.email_form.value.clone();
                    self.processing = true;
                    self.success = false;
                    self.warning = None;
                    Command::perform(
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
                    Command::none()
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
                Command::none()
            }
            _ => Command::none(),
        }
    }
}

impl From<BackendSettingsState> for Box<dyn State> {
    fn from(s: BackendSettingsState) -> Box<dyn State> {
        Box::new(s)
    }
}
