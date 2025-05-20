use std::sync::Arc;

use iced::{Alignment, Length, Task};

use liana::miniscript::bitcoin::Network;
use liana_ui::{
    component::{button, form, network_banner, notification, text::*},
    icon, theme,
    widget::*,
};
use lianad::commands::ListCoinsResult;

use crate::{
    app::{
        cache::coins_to_cache,
        settings::{SettingsError, WalletSettings},
    },
    daemon::DaemonError,
    dir::LianaDirectory,
};

use super::client::{
    auth::{AuthClient, AuthError},
    backend::{api, BackendClient, BackendWalletClient},
    cache::{self, update_connect_cache, ConnectCacheError},
};

#[derive(Debug, Clone)]
pub enum Error {
    Auth(AuthError),
    CredentialsMissing,
    // DaemonError does not implement Clone.
    // TODO: maybe Arc is overkill
    Backend(Arc<DaemonError>),
    Settings(SettingsError),
    Cache(cache::ConnectCacheError),
    Unexpected(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Auth(e) => write!(f, "Authentication error: {}", e),
            Self::CredentialsMissing => write!(f, "credentials missing"),
            Self::Backend(e) => write!(f, "Remote backend error: {}", e),
            Self::Settings(e) => write!(f, "Settings file error: {}", e),
            Self::Cache(e) => write!(f, "Connect cache file error: {}", e),
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

impl From<ConnectCacheError> for Error {
    fn from(value: ConnectCacheError) -> Self {
        Self::Cache(value)
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
    Run(
        Result<
            (
                BackendWalletClient,
                api::Wallet,
                /* coins to cache */ ListCoinsResult,
            ),
            Error,
        >,
    ),
}

#[derive(Debug, Clone)]
pub enum ViewMessage {
    RequestOTP,
    OTPEdited(String),
    BackToLauncher(Network),
}

#[derive(Debug, Clone)]
pub enum BackendState {
    NoWallet(BackendClient),
    WalletExists(BackendWalletClient, api::Wallet, ListCoinsResult),
}

pub struct LianaLiteLogin {
    pub datadir: LianaDirectory,
    pub network: Network,
    pub settings: WalletSettings,

    wallet_id: String,
    email: String,

    processing: bool,
    step: ConnectionStep,

    // Error due to connection
    connection_error: Option<Error>,
    // Authentication Error
    auth_error: Option<&'static str>,
}

pub enum ConnectionStep {
    CheckingAuthFile,
    CheckEmail,
    EnterOtp {
        client: AuthClient,
        backend_api_url: String,
        otp: form::Value<String>,
    },
}

impl LianaLiteLogin {
    pub fn new(
        datadir: LianaDirectory,
        network: Network,
        settings: WalletSettings,
    ) -> (Self, Task<Message>) {
        let auth = settings.remote_backend_auth.clone().unwrap();
        (
            Self {
                network,
                datadir: datadir.clone(),
                step: ConnectionStep::CheckingAuthFile,
                connection_error: None,
                settings,
                wallet_id: auth.wallet_id.clone(),
                email: auth.email.clone(),
                auth_error: None,
                processing: true,
            },
            Task::perform(
                async move {
                    let service_config = super::client::get_service_config(network)
                        .await
                        .map_err(|e| Error::Unexpected(e.to_string()))?;
                    let client = AuthClient::new(
                        service_config.auth_api_url,
                        service_config.auth_api_public_key,
                        auth.email,
                    );
                    connect_with_credentials(
                        client,
                        auth.wallet_id,
                        service_config.backend_api_url,
                        network,
                        datadir,
                    )
                    .await
                },
                Message::Connected,
            ),
        )
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match &mut self.step {
            ConnectionStep::CheckingAuthFile => {
                if let Message::Connected(res) = message {
                    self.processing = false;
                    match res {
                        Ok(BackendState::NoWallet(_)) => {
                            self.auth_error = Some("No wallet found for the given email");
                        }
                        Ok(BackendState::WalletExists(client, wallet, coins)) => {
                            return Task::perform(
                                async move { (client, wallet, coins) },
                                |(c, w, coins)| Message::Run(Ok((c, w, coins))),
                            );
                        }
                        Err(e) => {
                            // Do not display error, if the Liana-Connect cache does not exist,
                            // simply ask user to do the authentication steps.
                            if !matches!(e, Error::CredentialsMissing) {
                                self.connection_error = Some(e);
                            }
                            self.step = ConnectionStep::CheckEmail;
                        }
                    }
                }
            }
            ConnectionStep::CheckEmail => match message {
                Message::View(ViewMessage::RequestOTP) => {
                    let email = self.email.clone();
                    let network = self.network;
                    self.processing = true;
                    self.connection_error = None;
                    self.auth_error = None;
                    return Task::perform(
                        async move {
                            let config =
                                super::client::get_service_config(network)
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
                Message::OTPRequested(res) => {
                    self.processing = false;
                    match res {
                        Ok((client, backend_api_url)) => {
                            self.step = ConnectionStep::EnterOtp {
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
                otp,
                backend_api_url,
            } => match message {
                Message::View(ViewMessage::RequestOTP) => {
                    *otp = form::Value::default();
                    let client = client.clone();
                    self.processing = true;
                    self.connection_error = None;
                    self.auth_error = None;
                    return Task::perform(
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
                        let datadir = self.datadir.clone();
                        return Task::perform(
                            async move {
                                connect(client, otp, wallet_id, backend_api_url, network, datadir)
                                    .await
                            },
                            Message::Connected,
                        );
                    }
                }

                Message::Connected(res) => {
                    self.processing = false;
                    match res {
                        Ok(BackendState::NoWallet(client)) => {
                            return Task::perform(async move { Some(client) }, Message::Install);
                        }
                        Ok(BackendState::WalletExists(client, wallet, coins)) => {
                            return Task::perform(
                                async move { Ok((client, wallet, coins)) },
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

        Task::none()
    }

    pub fn view(&self) -> Element<Message> {
        let content = Into::<Element<ViewMessage>>::into(
            Container::new(
                Column::new().spacing(100).align_x(Alignment::Center).push(
                    Column::new()
                        .align_x(Alignment::Center)
                        .spacing(20)
                        .width(Length::Fill)
                        .push(h2("Liana Connect"))
                        .push(
                            Column::new()
                                .max_width(500)
                                .spacing(20)
                                .push(match &self.step {
                                    ConnectionStep::CheckingAuthFile => Column::new(),
                                    ConnectionStep::CheckEmail => Column::new()
                                        .spacing(20)
                                        .align_x(Alignment::Center)
                                        .push_maybe(
                                            self.auth_error
                                                .map(|e| text(e).style(theme::text::warning)),
                                        )
                                        .push(text(&self.email))
                                        .push(
                                            button::secondary(None, "Login")
                                                .width(Length::Fixed(200.0))
                                                .on_press_maybe(if self.processing {
                                                    None
                                                } else {
                                                    Some(ViewMessage::RequestOTP)
                                                }),
                                        ),
                                    ConnectionStep::EnterOtp { otp, .. } => Column::new()
                                        .spacing(20)
                                        .align_x(Alignment::Center)
                                        .push(text("An authentication was sent to your email:"))
                                        .push(text(&self.email))
                                        .push_maybe(
                                            self.auth_error
                                                .map(|e| text(e).style(theme::text::warning)),
                                        )
                                        .push(
                                            form::Form::new_trimmed("Token", otp, |msg| {
                                                ViewMessage::OTPEdited(msg)
                                            })
                                            .size(P1_SIZE)
                                            .padding(10)
                                            .warning("Token is not valid"),
                                        )
                                        .push(
                                            Row::new().spacing(10).push(
                                                button::secondary(None, "Resend token")
                                                    .width(Length::Fixed(200.0))
                                                    .on_press_maybe(if self.processing {
                                                        None
                                                    } else {
                                                        Some(ViewMessage::RequestOTP)
                                                    }),
                                            ),
                                        ),
                                }),
                        ),
                ),
            )
            .padding(50)
            .center_x(Length::Fill)
            .center_y(Length::Fill),
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

        col.push_maybe(if !matches!(self.step, ConnectionStep::CheckingAuthFile) {
            Some(
                Container::new(
                    button::secondary(Some(icon::previous_icon()), "Go back")
                        .width(Length::Fixed(200.0))
                        .on_press(Message::View(ViewMessage::BackToLauncher(self.network))),
                )
                .padding(20),
            )
        } else {
            None
        })
        .push(content)
        .into()
    }
}

pub async fn connect(
    auth: AuthClient,
    token: String,
    wallet_id: String,
    backend_api_url: String,
    network: Network,
    liana_directory: LianaDirectory,
) -> Result<BackendState, Error> {
    let network_dir = liana_directory.network_directory(network);
    let access = auth.verify_otp(token.trim_end()).await?;
    let client =
        BackendClient::connect(auth.clone(), backend_api_url, access.clone(), network).await?;

    update_connect_cache(&network_dir, &access, &auth, false).await?;

    let wallets = client.list_wallets().await?;
    if wallets.is_empty() {
        return Ok(BackendState::NoWallet(client));
    }

    if wallet_id.is_empty() {
        let first = wallets.first().cloned().ok_or(DaemonError::NoAnswer)?;
        let (wallet_client, wallet) = client.connect_wallet(first);
        let coins = coins_to_cache(Arc::new(wallet_client.clone())).await?;

        Ok(BackendState::WalletExists(wallet_client, wallet, coins))
    } else if let Some(wallet) = wallets.into_iter().find(|w| w.id == wallet_id) {
        let (wallet_client, wallet) = client.connect_wallet(wallet);
        let coins = coins_to_cache(Arc::new(wallet_client.clone())).await?;

        Ok(BackendState::WalletExists(wallet_client, wallet, coins))
    } else {
        Ok(BackendState::NoWallet(client))
    }
}

pub async fn connect_with_credentials(
    auth: AuthClient,
    wallet_id: String,
    backend_api_url: String,
    network: Network,
    liana_directory: LianaDirectory,
) -> Result<BackendState, Error> {
    let network_dir = liana_directory.network_directory(network);
    let mut tokens = cache::Account::from_cache(&network_dir, &auth.email)
        .map_err(|e| match e {
            ConnectCacheError::NotFound => Error::CredentialsMissing,
            _ => e.into(),
        })?
        .ok_or(Error::CredentialsMissing)?
        .tokens;

    if tokens.expires_at < chrono::Utc::now().timestamp() {
        tokens = cache::update_connect_cache(&network_dir, &tokens, &auth, true).await?;
    }

    let client = BackendClient::connect(auth, backend_api_url, tokens, network).await?;

    if let Some(wallet) = client
        .list_wallets()
        .await?
        .into_iter()
        .find(|w| w.id == wallet_id)
    {
        let (wallet_client, wallet) = client.connect_wallet(wallet);
        let coins = coins_to_cache(Arc::new(wallet_client.clone())).await?;
        Ok(BackendState::WalletExists(wallet_client, wallet, coins))
    } else {
        Ok(BackendState::NoWallet(client))
    }
}
