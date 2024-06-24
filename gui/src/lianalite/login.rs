use std::path::PathBuf;

use iced::{Alignment, Command, Length};

use liana::miniscript::bitcoin::Network;
use liana_ui::{
    color,
    component::{button, form, network_banner, text::*},
    icon, image,
    widget::*,
};

use crate::{app, daemon::DaemonError, datadir::create_directory};

use super::{
    client::{
        auth::{AuthClient, AuthError},
        backend::{api, BackendClient, BackendWalletClient},
    },
    AuthConfig, ConfigError,
};

#[derive(Debug)]
pub enum Error {
    Auth(AuthError),
    Backend(DaemonError),
    Config(ConfigError),
    Unexpected(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Auth(e) => write!(f, "{}", e),
            Self::Backend(e) => write!(f, "{}", e),
            Self::Config(e) => write!(f, "{}", e),
            Self::Unexpected(e) => write!(f, "{}", e),
        }
    }
}

impl From<DaemonError> for Error {
    fn from(value: DaemonError) -> Self {
        Self::Backend(value)
    }
}

impl From<AuthError> for Error {
    fn from(value: AuthError) -> Self {
        Self::Auth(value)
    }
}

impl From<ConfigError> for Error {
    fn from(value: ConfigError) -> Self {
        Self::Config(value)
    }
}

#[derive(Debug)]
pub enum Message {
    View(ViewMessage),
    OTPRequested(Result<(AuthClient, String), Error>),
    OTPResent(Result<(), Error>),
    Connected(Result<BackendState, Error>),
    // If user has already an existing wallet and choose to use it
    // and the gui.toml and auth.json are not yet created, a Command
    // doing the setup is launched and this message is the result.
    InstallConfigAndRun(Result<(BackendWalletClient, api::Wallet), Error>),
    // redirect to the installer with the remote backend connection.
    Install(Option<BackendClient>),
    // redirect to the app runner with the remote backend connection.
    Run(BackendWalletClient, api::Wallet),
}

#[derive(Debug, Clone)]
pub enum ViewMessage {
    RequestOTP,
    EditEmail,
    EmailEdited(String),
    OTPEdited(String),
    RunExistingWallet,
    BackToLauncher,
    InstallLocalWallet,
}

#[derive(Debug)]
pub enum BackendState {
    NoWallet(BackendClient),
    WalletExists(BackendWalletClient, api::Wallet),
}

pub struct LianaLiteLogin {
    pub datadir: PathBuf,
    pub network: Network,
    pub new_wallet: bool,

    processing: bool,
    step: ConnectionStep,
    error: Option<Error>,
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
    WalletAlreadyExists {
        client: Option<BackendWalletClient>,
        wallet: api::Wallet,
    },
}

impl LianaLiteLogin {
    pub fn new(datadir: PathBuf, network: Network, new_wallet: bool) -> (Self, Command<Message>) {
        if new_wallet {
            (
                Self {
                    new_wallet,
                    network,
                    datadir,
                    step: ConnectionStep::EnterEmail {
                        email: form::Value::default(),
                    },
                    error: None,
                    processing: false,
                },
                Command::none(),
            )
        } else {
            (
                Self {
                    new_wallet,
                    network,
                    datadir: datadir.clone(),
                    step: ConnectionStep::CheckingAuthFile,
                    error: None,
                    processing: true,
                },
                Command::perform(
                    async move {
                        let auth_config = AuthConfig::from_file(&datadir, network)?;
                        let service_config = super::client::get_service_config(network)
                            .await
                            .map_err(|e| Error::Unexpected(e.to_string()))?;
                        let client = AuthClient::new(
                            service_config.auth_api_url,
                            service_config.auth_api_public_key,
                            auth_config.email,
                        );
                        connect_with_refresh_token(
                            datadir,
                            network,
                            client,
                            auth_config.refresh_token,
                            service_config.backend_api_url,
                        )
                        .await
                    },
                    Message::Connected,
                ),
            )
        }
    }

    pub fn stop(&mut self) {}

    pub fn update(&mut self, message: Message) -> Command<Message> {
        if matches!(message, Message::View(ViewMessage::InstallLocalWallet)) {
            return Command::perform(async move { None }, Message::Install);
        }
        match &mut self.step {
            ConnectionStep::CheckingAuthFile => {
                if let Message::Connected(res) = message {
                    self.processing = false;
                    match res {
                        Ok(BackendState::NoWallet(_)) => {
                            self.error = Some(Error::Unexpected(
                                "No wallet exists for this email address".to_string(),
                            ));
                        }
                        Ok(BackendState::WalletExists(client, wallet)) => {
                            return Command::perform(async move { (client, wallet) }, |(c, w)| {
                                Message::Run(c, w)
                            });
                        }
                        Err(e) => {
                            self.error = Some(e);
                            self.step = ConnectionStep::EnterEmail {
                                email: form::Value::default(),
                            };
                        }
                    }
                }
            }
            ConnectionStep::EnterEmail { email } => match message {
                Message::View(ViewMessage::EmailEdited(value)) => {
                    email.valid = true;
                    email.value = value;
                }
                Message::View(ViewMessage::RequestOTP) => {
                    if email.value.is_empty() {
                        email.valid = false;
                    } else {
                        let email = email.value.clone();
                        let network = self.network;
                        self.processing = true;
                        return Command::perform(
                            async move {
                                let config = super::client::get_service_config(network)
                                    .await
                                    .map_err(|e| Error::Unexpected(e.to_string()))?;
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
                            self.error = Some(e);
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
                    return Command::perform(
                        async move {
                            client.resend_otp().await?;
                            Ok(())
                        },
                        Message::OTPResent,
                    );
                }
                Message::OTPResent(Err(e)) => {
                    self.processing = false;
                    self.error = Some(e);
                }
                Message::View(ViewMessage::OTPEdited(value)) => {
                    otp.value = value.trim().to_string();
                    if otp.value.len() == 6 {
                        let client = client.clone();
                        let otp = otp.value.clone();
                        let backend_api_url = backend_api_url.clone();
                        self.processing = true;
                        return Command::perform(
                            async move { connect(client, otp, backend_api_url).await },
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
                            if self.new_wallet {
                                self.step = ConnectionStep::WalletAlreadyExists {
                                    client: Some(client),
                                    wallet,
                                };
                            } else {
                                return Command::perform(
                                    async move { (client, wallet) },
                                    |(c, w)| Message::Run(c, w),
                                );
                            }
                        }
                        Err(e) => self.error = Some(e),
                    }
                }
                _ => {}
            },
            ConnectionStep::WalletAlreadyExists { client, wallet } => match message {
                Message::View(ViewMessage::RunExistingWallet) => {
                    self.processing = true;
                    let clt = client.take().expect("Must be some wallet");
                    let wallet = wallet.clone();
                    let datadir = self.datadir.clone();
                    let network = self.network;
                    return Command::perform(
                        async move {
                            let network_datadir = datadir.join(network.to_string());
                            if !network_datadir.exists() {
                                create_directory(&network_datadir).map_err(|e| {
                                    Error::Unexpected(format!(
                                        "failed to create {:?}: {}",
                                        network_datadir, e
                                    ))
                                })?;
                                tracing::info!("network datadir {:?} created", network_datadir);
                            } else {
                                tracing::info!(
                                    "network datadir already exists: {:?}",
                                    network_datadir
                                );
                            }
                            let gui_config_path =
                                network_datadir.join(app::config::DEFAULT_FILE_NAME);
                            if !gui_config_path.exists() {
                                app::Config {
                                    daemon_config_path: None,
                                    daemon_rpc_path: None,
                                    log_level: None,
                                    debug: None,
                                    start_internal_bitcoind: false,
                                    use_remote_backend: true,
                                }
                                .to_file(&gui_config_path)
                                .map_err(|e| {
                                    Error::Unexpected(format!(
                                        "failed to create {:?}: {}",
                                        gui_config_path, e
                                    ))
                                })?;
                            }
                            let auth_config = AuthConfig {
                                email: clt.user_email().to_string(),
                                refresh_token: clt.auth()?.refresh_token,
                            };
                            auth_config.to_file(&datadir, network)?;
                            Ok((clt, wallet))
                        },
                        Message::InstallConfigAndRun,
                    );
                }
                Message::InstallConfigAndRun(res) => match res {
                    Ok((clt, wallet)) => {
                        self.processing = true;
                        return Command::perform(async move { (clt, wallet) }, |(c, w)| {
                            Message::Run(c, w)
                        });
                    }
                    Err(e) => {
                        self.error = Some(e);
                        self.processing = false;
                    }
                },
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
                        Row::new()
                            .spacing(50)
                            .push_maybe(if self.new_wallet {
                                Some(
                                    Column::new()
                                        .spacing(20)
                                        .align_items(Alignment::Center)
                                        .width(Length::FillPortion(1))
                                        .push(
                                            image::liana_brand_grey().height(Length::Fixed(100.0)),
                                        )
                                        .push(p2_medium(LIANA_DESC).style(color::GREY_3))
                                        .push(button::primary(None, "Install local wallet")
                                                .on_press(ViewMessage::InstallLocalWallet))
                                )
                            } else {
                                None
                            })
                            .push(
                                Column::new()
                                    .align_items(Alignment::Center)
                                    .spacing(20)
                                    .width(if self.new_wallet {
                                        Length::FillPortion(1)
                                    } else {
                                        Length::Fill
                                    })
                                    .push(
                                        image::lianalite_brand_grey().height(Length::Fixed(100.0)),
                                    )
                                    .push(
                                        Column::new()
                                            .max_width(500)
                                            .spacing(20)
                                            .push_maybe(if self.new_wallet {Some( p2_medium(LIANALITE_DESC).style(color::GREY_3)) } else { None })
                                            .push(match &self.step {
                                        ConnectionStep::CheckingAuthFile => Column::new(),
                                        ConnectionStep::EnterEmail { email } => Column::new()
                                            .spacing(20)
                                            .push_maybe(self.error.as_ref().map(|e| text(e.to_string())))
                                            .push(
                                                form::Form::new_trimmed("email", email, |msg| {
                                                    ViewMessage::EmailEdited(msg)
                                                })
                                                .size(P1_SIZE)
                                                .padding(10)
                                                .warning("Email is not valid"),
                                            )
                                            .push(
                                                button::primary(None, "Next")
                                                    .on_press_maybe(if self.processing {
                                                            None
                                                        } else {
                                                            Some(ViewMessage::RequestOTP)
                                                        }),
                                            ),
                                        ConnectionStep::EnterOtp { otp, .. } => Column::new()
                                            .push(text("An authentication was send to you mail"))
                                            .push_maybe(self.error.as_ref().map(|e| text(e.to_string())))
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
                                                        button::primary(
                                                            Some(icon::previous_icon()),
                                                            "Change email",
                                                        )
                                                        .on_press(ViewMessage::EditEmail),
                                                    )
                                                    .push(
                                                        button::primary(None, "Resend token")
                                                            .on_press_maybe(if self.processing {
                                                                None
                                                            } else {
                                                                Some(ViewMessage::RequestOTP)
                                                            }),
                                                    ),
                                            ),
                                        ConnectionStep::WalletAlreadyExists { wallet, .. } => {
                                            Column::new()
                                                .push_maybe(self.error.as_ref().map(|e| text(e.to_string())))
                                                .spacing(20)
                                                .push(text(format!(
                                                    "Wallet {} already exists",
                                                    wallet.name
                                                )))
                                                .push(
                                                    button::primary(None, "Use this one")
                                                        .on_press_maybe(if self.processing {
                                                                None
                                                            } else {
                                                                Some(ViewMessage::RunExistingWallet)
                                                        }),
                                                )
                                        }
                                    })),
                            ),
                    )
                    .push_maybe(
                        if !matches!(self.step, ConnectionStep::CheckingAuthFile) {
                            Some(button::secondary(Some(icon::previous_icon()), "Change network")
                                .width(Length::Fixed(200.0))
                                .on_press(ViewMessage::BackToLauncher))
                        } else {
                            None
                        }
                    ),
            )
            .padding(50)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x()
            .center_y(),
        )
        .map(Message::View);

        if self.network != Network::Bitcoin {
            Column::with_children(vec![network_banner(self.network).into(), content]).into()
        } else {
            content
        }
    }
}

pub const LIANALITE_DESC: &str = "Use the connection to the Bitcoin network provided by Wizardsardine. This removes the need for running a Bitcoin full node on your machine. It also provides synchronisation of your wallet data (labels, transactions, etc..) across your machines and participants in your wallet. We will also keep a backup of your wallet descriptor for you. This is the most convenient option but has privacy implications: your data would be stored on our servers (but never shared with a third party).";

pub const LIANA_DESC: &str = "This option creates a wallet on your machine. The wallet will never access any of our servers, we would not even be able to know you use our wallet. This option requires a local Bitcoin full node. A full node is necessary to use Bitcoin in a sovereign way, but it is more accessible than it sounds. The Liana wallet can download and run one for you so you don't have to manage it yourself. It will never use more than a couple GB of disk space. The initial synchronisation of the node takes time and is computationally intensive, but past this point running a Bitcoin full node on your machine is seamless.";

pub async fn connect(
    auth: AuthClient,
    token: String,
    backend_api_url: String,
) -> Result<BackendState, Error> {
    let access = auth.verify_otp(token.trim_end()).await?;
    let client = BackendClient::connect(auth, backend_api_url, access.clone()).await?;

    if !client.list_wallets().await?.is_empty() {
        let (wallet_client, wallet) = client.connect_first().await?;
        Ok(BackendState::WalletExists(wallet_client, wallet))
    } else {
        Ok(BackendState::NoWallet(client))
    }
}

pub async fn connect_with_refresh_token(
    datadir: PathBuf,
    network: Network,
    auth: AuthClient,
    refresh_token: String,
    backend_api_url: String,
) -> Result<BackendState, Error> {
    let access = auth.refresh_token(&refresh_token).await?;
    let config = AuthConfig {
        email: auth.email.clone(),
        refresh_token: access.refresh_token.clone(),
    };

    config.to_file(&datadir, network)?;

    let client = BackendClient::connect(auth, backend_api_url, access.clone()).await?;

    if !client.list_wallets().await?.is_empty() {
        let (wallet_client, wallet) = client.connect_first().await?;
        Ok(BackendState::WalletExists(wallet_client, wallet))
    } else {
        Ok(BackendState::NoWallet(client))
    }
}
