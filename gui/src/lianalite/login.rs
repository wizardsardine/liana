use std::{path::PathBuf, sync::Arc};

use iced::{Alignment, Command, Length};

use liana::miniscript::bitcoin::Network;
use liana_ui::{
    color,
    component::{button, form, network_banner, notification, text::*},
    icon,
    widget::*,
};

use crate::{
    app::settings::{AuthConfig, Settings, SettingsError, WalletSetting},
    daemon::DaemonError,
};

use super::client::{
    auth::{AuthClient, AuthError},
    backend::{api, BackendClient, BackendWalletClient},
};

#[derive(Debug, Clone)]
pub enum Error {
    Auth(AuthError),
    // DaemonError does not implement Clone.
    // TODO: maybe Arc is overkill
    Backend(Arc<DaemonError>),
    Settings(SettingsError),
    Unexpected(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Auth(e) => write!(f, "Authentication error: {}", e),
            Self::Backend(e) => write!(f, "Remote backend error: {}", e),
            Self::Settings(e) => write!(f, "Settings file error: {}", e),
            Self::Unexpected(e) => write!(f, "Unexpected error: {}", e),
        }
    }
}

impl From<DaemonError> for Error {
    fn from(value: DaemonError) -> Self {
        Self::Backend(Arc::new(value))
    }
}

impl From<AuthError> for Error {
    fn from(value: AuthError) -> Self {
        Self::Auth(value)
    }
}

impl From<SettingsError> for Error {
    fn from(value: SettingsError) -> Self {
        Self::Settings(value)
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    View(ViewMessage),
    OTPRequested(Result<(AuthClient, String), Error>),
    OTPResent(Result<(), Error>),
    // wallet_id and result of the connect command.
    Connected(Result<BackendState, Error>),
    // redirect to the installer with the remote backend connection.
    Install(Option<BackendClient>),
    // redirect to the app runner with the remote backend connection.
    Run(Result<(BackendWalletClient, api::Wallet), Error>),
}

#[derive(Debug, Clone)]
pub enum ViewMessage {
    RequestOTP,
    EditEmail,
    EmailEdited(String),
    OTPEdited(String),
    BackToLauncher(Network),
}

#[derive(Debug, Clone)]
pub enum BackendState {
    NoWallet(BackendClient),
    WalletExists(BackendWalletClient, api::Wallet),
}

pub struct LianaLiteLogin {
    pub datadir: PathBuf,
    pub network: Network,

    wallet_id: String,

    processing: bool,
    step: ConnectionStep,

    // Error due to connection
    connection_error: Option<Error>,
    // Authentification Error
    auth_error: Option<&'static str>,
}

pub enum ConnectionStep {
    CheckingAuthFile,
    EnterEmail {
        email: form::Value<String>,
    },
    EnterOtp {
        client: AuthClient,
        backend_api_url: String,
        email: String,
        otp: form::Value<String>,
    },
}

impl LianaLiteLogin {
    pub fn new(datadir: PathBuf, network: Network, settings: Settings) -> (Self, Command<Message>) {
        match settings
            .wallets
            .first()
            .cloned()
            .and_then(|w| w.remote_backend_auth)
            .ok_or(Error::Unexpected(
                "Missing auth configuration in settings.json".to_string(),
            )) {
            Err(e) => (
                Self {
                    network,
                    datadir: datadir.clone(),
                    step: ConnectionStep::EnterEmail {
                        email: form::Value::default(),
                    },
                    wallet_id: String::new(),
                    connection_error: Some(e),
                    auth_error: None,
                    processing: true,
                },
                Command::none(),
            ),
            Ok(auth_config) => (
                Self {
                    network,
                    datadir: datadir.clone(),
                    step: ConnectionStep::CheckingAuthFile,
                    connection_error: None,
                    wallet_id: auth_config.wallet_id.clone(),
                    auth_error: None,
                    processing: true,
                },
                Command::perform(
                    async move {
                        let service_config = super::client::get_service_config(network)
                            .await
                            .map_err(|e| Error::Unexpected(e.to_string()))?;
                        let client = AuthClient::new(
                            service_config.auth_api_url,
                            service_config.auth_api_public_key,
                            auth_config.email,
                        );
                        connect_with_refresh_token(
                            client,
                            auth_config.refresh_token,
                            auth_config.wallet_id,
                            service_config.backend_api_url,
                            network,
                        )
                        .await
                    },
                    Message::Connected,
                ),
            ),
        }
    }

    pub fn update(&mut self, message: Message) -> Command<Message> {
        match &mut self.step {
            ConnectionStep::CheckingAuthFile => {
                if let Message::Connected(res) = message {
                    self.processing = false;
                    match res {
                        Ok(BackendState::NoWallet(_)) => {
                            self.auth_error = Some("No wallet found for the given email");
                        }
                        Ok(BackendState::WalletExists(client, wallet)) => {
                            return Command::perform(async move { (client, wallet) }, |(c, w)| {
                                Message::Run(Ok((c, w)))
                            });
                        }
                        Err(e) => {
                            self.connection_error = Some(e);
                            self.step = ConnectionStep::EnterEmail {
                                email: form::Value::default(),
                            };
                        }
                    }
                }
            }
            ConnectionStep::EnterEmail { email } => match message {
                Message::View(ViewMessage::EmailEdited(value)) => {
                    email.valid = value.is_empty()
                        || email_address::EmailAddress::parse_with_options(
                            &value,
                            email_address::Options::default().with_required_tld(),
                        )
                        .is_ok();
                    email.value = value;
                }
                Message::View(ViewMessage::RequestOTP) => {
                    if email.value.is_empty() {
                        email.valid = false;
                    } else if email.valid {
                        let email = email.value.clone();
                        let network = self.network;
                        self.processing = true;
                        self.connection_error = None;
                        self.auth_error = None;
                        return Command::perform(
                            async move {
                                let config = super::client::get_service_config(network)
                                    .await
                                    .map_err(|e| {
                                        if e.status() == Some(reqwest::StatusCode::NOT_FOUND) {
                                            Error::Unexpected(
                                                "Remote servers are unresponsive".to_string(),
                                            )
                                        } else {
                                            Error::Unexpected(e.to_string())
                                        }
                                    })?;
                                let client = AuthClient::new(
                                    config.auth_api_url,
                                    config.auth_api_public_key,
                                    email,
                                );
                                client.sign_in_otp().await?;
                                Ok((client, config.backend_api_url))
                            },
                            Message::OTPRequested,
                        );
                    }
                }
                Message::OTPRequested(res) => {
                    self.processing = false;
                    match res {
                        Ok((client, backend_api_url)) => {
                            self.step = ConnectionStep::EnterOtp {
                                email: email.value.to_owned(),
                                otp: form::Value::default(),
                                client,
                                backend_api_url,
                            };
                        }
                        Err(e) => {
                            self.connection_error = Some(e);
                        }
                    }
                }
                _ => {}
            },
            ConnectionStep::EnterOtp {
                client,
                email,
                otp,
                backend_api_url,
            } => match message {
                Message::View(ViewMessage::EditEmail) => {
                    self.step = ConnectionStep::EnterEmail {
                        email: form::Value {
                            value: email.clone(),
                            valid: true,
                        },
                    };
                }
                Message::View(ViewMessage::RequestOTP) => {
                    *otp = form::Value::default();
                    let client = client.clone();
                    self.processing = true;
                    self.connection_error = None;
                    self.auth_error = None;
                    return Command::perform(
                        async move {
                            client.resend_otp().await?;
                            Ok(())
                        },
                        Message::OTPResent,
                    );
                }
                Message::OTPResent(res) => match res {
                    Ok(()) => {
                        self.processing = false;
                    }
                    Err(e) => {
                        tracing::warn!("{}", e);
                        self.processing = false;
                        self.connection_error = Some(e);
                    }
                },
                Message::View(ViewMessage::OTPEdited(value)) => {
                    otp.value = value.trim().to_string();
                    otp.valid = true;
                    if otp.value.len() == 6 {
                        let client = client.clone();
                        let otp = otp.value.clone();
                        let backend_api_url = backend_api_url.clone();
                        self.processing = true;
                        self.connection_error = None;
                        self.auth_error = None;
                        let wallet_id = self.wallet_id.clone();
                        let network = self.network;
                        return Command::perform(
                            async move {
                                connect(client, otp, wallet_id, backend_api_url, network).await
                            },
                            Message::Connected,
                        );
                    }
                }

                Message::Connected(res) => {
                    self.processing = false;
                    match res {
                        Ok(BackendState::NoWallet(client)) => {
                            return Command::perform(async move { Some(client) }, Message::Install);
                        }
                        Ok(BackendState::WalletExists(client, wallet)) => {
                            let datadir = self.datadir.clone();
                            let network = self.network;
                            return Command::perform(
                                async move {
                                    update_wallet_auth_settings(
                                        datadir,
                                        network,
                                        wallet.clone(),
                                        client.user_email().to_string(),
                                        client.auth().await.refresh_token,
                                    )
                                    .await?;

                                    Ok((client, wallet))
                                },
                                Message::Run,
                            );
                        }
                        Err(e) => {
                            tracing::warn!("{}", e);
                            if let Error::Auth(AuthError { http_status, .. }) = e {
                                if http_status == Some(403) {
                                    self.auth_error = Some("Token is expired or is invalid")
                                } else {
                                    self.connection_error = Some(e);
                                }
                            } else {
                                self.connection_error = Some(e);
                            }
                        }
                    }
                }
                // Message::Run::Ok is handled by the upper level wrapping the LianaLiteLogin
                // state.
                Message::Run(Err(e)) => {
                    self.connection_error = Some(e);
                }
                _ => {}
            },
        }

        Command::none()
    }

    pub fn view(&self) -> Element<Message> {
        let content = Into::<Element<ViewMessage>>::into(
            Container::new(
                Column::new()
                    .spacing(100)
                    .align_items(Alignment::Center)
                    .push(
                        Column::new()
                            .align_items(Alignment::Center)
                            .spacing(20)
                            .width(Length::Fill)
                            .push(h2("Liana Connect"))
                            .push(
                                Column::new()
                                    .max_width(500)
                                    .spacing(20)
                                    .push(match &self.step {
                                        ConnectionStep::CheckingAuthFile => Column::new(),
                                        ConnectionStep::EnterEmail { email } => Column::new()
                                            .spacing(20)
                                            .push_maybe(
                                                self.auth_error
                                                    .map(|e| text(e).style(color::ORANGE)),
                                            )
                                            .push(
                                                form::Form::new_trimmed("email", email, |msg| {
                                                    ViewMessage::EmailEdited(msg)
                                                })
                                                .size(P1_SIZE)
                                                .padding(10)
                                                .warning("Email is not valid"),
                                            )
                                            .push(button::secondary(None, "Next").on_press_maybe(
                                                if self.processing {
                                                    None
                                                } else {
                                                    Some(ViewMessage::RequestOTP)
                                                },
                                            )),
                                        ConnectionStep::EnterOtp { otp, .. } => Column::new()
                                            .push(text("An authentication was send to your email"))
                                            .push_maybe(
                                                self.auth_error
                                                    .map(|e| text(e).style(color::ORANGE)),
                                            )
                                            .spacing(20)
                                            .push(
                                                form::Form::new_trimmed("Token", otp, |msg| {
                                                    ViewMessage::OTPEdited(msg)
                                                })
                                                .size(P1_SIZE)
                                                .padding(10)
                                                .warning("Token is not valid"),
                                            )
                                            .push(
                                                Row::new()
                                                    .spacing(10)
                                                    .push(
                                                        button::secondary(
                                                            Some(icon::previous_icon()),
                                                            "Change email",
                                                        )
                                                        .on_press(ViewMessage::EditEmail),
                                                    )
                                                    .push(
                                                        button::secondary(None, "Resend token")
                                                            .on_press_maybe(if self.processing {
                                                                None
                                                            } else {
                                                                Some(ViewMessage::RequestOTP)
                                                            }),
                                                    ),
                                            ),
                                    }),
                            ),
                    )
                    .push_maybe(if !matches!(self.step, ConnectionStep::CheckingAuthFile) {
                        Some(
                            button::secondary(Some(icon::previous_icon()), "Change network")
                                .width(Length::Fixed(200.0))
                                .on_press(ViewMessage::BackToLauncher(self.network)),
                        )
                    } else {
                        None
                    }),
            )
            .padding(50)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x()
            .center_y(),
        )
        .map(Message::View);

        let mut col = Column::new();
        if self.network != Network::Bitcoin {
            col = col.push(network_banner(self.network));
        }
        if let Some(error) = &self.connection_error {
            col = col.push(
                notification::warning("Connection failed".to_string(), error.to_string())
                    .width(Length::Fill),
            );
        }

        col.push(content).into()
    }
}

async fn update_wallet_auth_settings(
    datadir: PathBuf,
    network: Network,
    wallet: api::Wallet,
    email: String,
    refresh_token: String,
) -> Result<(), Error> {
    let mut settings = Settings::from_file(datadir.clone(), network)?;

    let descriptor_checksum = wallet
        .descriptor
        .to_string()
        .split_once('#')
        .map(|(_, checksum)| checksum)
        .expect("Failed to get checksum from a valid LianaDescriptor")
        .to_string();

    let remote_backend_auth = Some(AuthConfig {
        email,
        wallet_id: wallet.id.clone(),
        refresh_token,
    });

    if let Some(wallet_settings) = settings.wallets.iter_mut().find(|w| {
        if let Some(auth) = &w.remote_backend_auth {
            auth.wallet_id == wallet.id
        } else {
            false
        }
    }) {
        wallet_settings.remote_backend_auth = remote_backend_auth;
    } else {
        tracing::info!("Wallet id was not found in the settings, adding now the wallet settings to the settings.json file");
        settings.wallets.insert(
            0,
            WalletSetting {
                name: wallet.name,
                descriptor_checksum,
                keys: Vec::new(),
                hardware_wallets: Vec::new(),
                remote_backend_auth,
            },
        );
    }

    settings.to_file(datadir, network).map_err(|e| {
        DaemonError::Unexpected(format!("Cannot access to settings.json file: {}", e))
    })?;

    Ok(())
}

pub async fn connect(
    auth: AuthClient,
    token: String,
    wallet_id: String,
    backend_api_url: String,
    network: Network,
) -> Result<BackendState, Error> {
    let access = auth.verify_otp(token.trim_end()).await?;
    let client = BackendClient::connect(auth, backend_api_url, access.clone(), network).await?;

    let wallets = client.list_wallets().await?;
    if wallets.is_empty() {
        return Ok(BackendState::NoWallet(client));
    }

    if wallet_id.is_empty() {
        let first = wallets.first().cloned().ok_or(DaemonError::NoAnswer)?;
        let (wallet_client, wallet) = client.connect_wallet(first);
        Ok(BackendState::WalletExists(wallet_client, wallet))
    } else if let Some(wallet) = wallets.into_iter().find(|w| w.id == wallet_id) {
        let (wallet_client, wallet) = client.connect_wallet(wallet);
        Ok(BackendState::WalletExists(wallet_client, wallet))
    } else {
        Ok(BackendState::NoWallet(client))
    }
}

pub async fn connect_with_refresh_token(
    auth: AuthClient,
    refresh_token: String,
    wallet_id: String,
    backend_api_url: String,
    network: Network,
) -> Result<BackendState, Error> {
    let access = auth.refresh_token(&refresh_token).await?;
    let client = BackendClient::connect(auth, backend_api_url, access.clone(), network).await?;

    if let Some(wallet) = client
        .list_wallets()
        .await?
        .into_iter()
        .find(|w| w.id == wallet_id)
    {
        let (wallet_client, wallet) = client.connect_wallet(wallet);
        Ok(BackendState::WalletExists(wallet_client, wallet))
    } else {
        Ok(BackendState::NoWallet(client))
    }
}
