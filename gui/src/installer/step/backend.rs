use iced::Command;

use liana::miniscript::bitcoin::Network;
use liana_ui::{component::form, widget::Element};

use crate::{
    hw::HardwareWallets,
    installer::{
        context::{self, Context, RemoteBackend},
        message::{self, Message},
        step::Step,
        view, Error,
    },
    lianalite::client::{
        self,
        auth::{AuthClient, AuthError},
        backend::{api, BackendClient},
    },
};

pub enum ConnectionStep {
    EnterEmail {
        email: form::Value<String>,
    },
    EnterOtp {
        client: AuthClient,
        backend_api_url: String,
        email: String,
        otp: form::Value<String>,
    },
    Connected {
        email: String,
        remote_backend: context::RemoteBackend,
        wallet: Option<api::Wallet>,
        remote_backend_is_selected: bool,
    },
}

pub struct ChooseBackend {
    network: Network,
    processing: bool,
    step: ConnectionStep,
    connection_error: Option<Error>,
    auth_error: Option<&'static str>,
}

impl ChooseBackend {
    pub fn new(network: Network) -> Self {
        Self {
            network,
            step: ConnectionStep::EnterEmail {
                email: form::Value::default(),
            },
            connection_error: None,
            auth_error: None,
            processing: false,
        }
    }
}

impl From<ChooseBackend> for Box<dyn Step> {
    fn from(s: ChooseBackend) -> Box<dyn Step> {
        Box::new(s)
    }
}

impl Step for ChooseBackend {
    fn update(&mut self, _hws: &mut HardwareWallets, message: Message) -> Command<Message> {
        if matches!(
            message,
            Message::SelectBackend(message::SelectBackend::ContinueWithLocalWallet)
        ) {
            if let ConnectionStep::Connected {
                remote_backend_is_selected,
                ..
            } = &mut self.step
            {
                *remote_backend_is_selected = false;
            }
            return Command::perform(async move {}, |_| Message::Next);
        }
        match &mut self.step {
            ConnectionStep::EnterEmail { email } => match message {
                Message::SelectBackend(message::SelectBackend::EmailEdited(value)) => {
                    email.valid = value.is_empty()
                        || email_address::EmailAddress::parse_with_options(
                            &value,
                            email_address::Options::default().with_required_tld(),
                        )
                        .is_ok();
                    email.value = value;
                }
                Message::SelectBackend(message::SelectBackend::RequestOTP) => {
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
                                let config =
                                    client::get_service_config(network).await.map_err(|e| {
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
                            |res| Message::SelectBackend(message::SelectBackend::OTPRequested(res)),
                        );
                    }
                }
                Message::SelectBackend(message::SelectBackend::OTPRequested(res)) => {
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
                Message::SelectBackend(message::SelectBackend::EditEmail) => {
                    self.step = ConnectionStep::EnterEmail {
                        email: form::Value {
                            value: email.clone(),
                            valid: true,
                        },
                    };
                }
                Message::SelectBackend(message::SelectBackend::RequestOTP) => {
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
                        message::SelectBackend::OTPResent,
                    )
                    .map(Message::SelectBackend);
                }
                Message::SelectBackend(message::SelectBackend::OTPResent(res)) => {
                    self.processing = false;
                    if let Err(e) = res {
                        self.connection_error = Some(e);
                    }
                }
                Message::SelectBackend(message::SelectBackend::OTPEdited(value)) => {
                    otp.value = value.trim().to_string();
                    if otp.value.len() == 6 {
                        let client = client.clone();
                        let otp = otp.value.clone();
                        let backend_api_url = backend_api_url.clone();
                        self.processing = true;
                        self.connection_error = None;
                        self.auth_error = None;
                        return Command::perform(
                            async move { connect(client, otp, backend_api_url).await },
                            message::SelectBackend::Connected,
                        )
                        .map(Message::SelectBackend);
                    }
                }

                Message::SelectBackend(message::SelectBackend::Connected(res)) => {
                    self.processing = false;
                    match res {
                        Ok((remote_backend, wallet)) => {
                            self.step = ConnectionStep::Connected {
                                email: email.clone(),
                                remote_backend,
                                wallet,
                                remote_backend_is_selected: false,
                            };
                        }
                        Err(e) => {
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
                _ => {}
            },
            ConnectionStep::Connected {
                remote_backend_is_selected,
                ..
            } => match message {
                Message::SelectBackend(message::SelectBackend::EditEmail) => {
                    self.step = ConnectionStep::EnterEmail {
                        email: form::Value::default(),
                    }
                }
                Message::SelectBackend(message::SelectBackend::ContinueWithRemoteBackend) => {
                    *remote_backend_is_selected = true;
                    return Command::perform(async move {}, |_| Message::Next);
                }
                _ => {}
            },
        }

        Command::none()
    }

    fn apply(&mut self, ctx: &mut Context) -> bool {
        if let ConnectionStep::Connected {
            remote_backend,
            remote_backend_is_selected,
            ..
        } = &self.step
        {
            if *remote_backend_is_selected {
                ctx.remote_backend = Some(remote_backend.clone());
            }
        } else {
            ctx.remote_backend = None;
        }

        true
    }

    fn view<'a>(
        &'a self,
        _hws: &'a HardwareWallets,
        progress: (usize, usize),
        _email: Option<&'a str>,
    ) -> Element<Message> {
        view::choose_backend(
            progress,
            match &self.step {
                ConnectionStep::EnterEmail { email } => view::connection_step_enter_email(
                    email,
                    self.processing,
                    self.connection_error.as_ref(),
                    self.auth_error,
                ),
                ConnectionStep::EnterOtp { email, otp, .. } => view::connection_step_enter_otp(
                    email,
                    otp,
                    self.processing,
                    self.connection_error.as_ref(),
                    self.auth_error,
                ),
                ConnectionStep::Connected { email, wallet, .. } => view::connection_step_connected(
                    email,
                    self.processing,
                    wallet.as_ref().map(|w| w.name.as_str()),
                    self.connection_error.as_ref(),
                    self.auth_error,
                ),
            },
        )
    }
}

pub async fn connect(
    auth: AuthClient,
    token: String,
    backend_api_url: String,
) -> Result<(context::RemoteBackend, Option<api::Wallet>), Error> {
    let access = auth.verify_otp(token.trim_end()).await?;
    let client = BackendClient::connect(auth, backend_api_url, access.clone()).await?;

    if !client.list_wallets().await?.is_empty() {
        let (wallet_client, wallet) = client.connect_first().await?;
        Ok((RemoteBackend::WithWallet(wallet_client), Some(wallet)))
    } else {
        Ok((RemoteBackend::WithoutWallet(client), None))
    }
}
