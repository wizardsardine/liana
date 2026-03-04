use coincube_ui::{component::form, widget::*};
use coincubed::config::{BitcoinBackend, EsploraConfig};
use iced::Task;

use crate::{
    hw::HardwareWallets,
    installer::{
        context::Context,
        message::{CoincubeRelayMsg, Message},
        step::Step,
        view,
    },
    services::coincube::{CoincubeClient, OtpRequest, OtpVerifyRequest},
};

pub struct CoincubeRelayStep {
    client: CoincubeClient,
    email: form::Value<String>,
    otp: form::Value<String>,
    otp_sent: bool,
    is_signup: bool,
    jwt: Option<String>,
    processing: bool,
    error: Option<String>,
}

impl CoincubeRelayStep {
    pub fn new() -> Self {
        Self {
            client: CoincubeClient::new(),
            email: form::Value::default(),
            otp: form::Value::default(),
            otp_sent: false,
            is_signup: true,
            jwt: None,
            processing: false,
            error: None,
        }
    }
}

impl Default for CoincubeRelayStep {
    fn default() -> Self {
        Self::new()
    }
}

impl From<CoincubeRelayStep> for Box<dyn Step> {
    fn from(s: CoincubeRelayStep) -> Box<dyn Step> {
        Box::new(s)
    }
}

impl Step for CoincubeRelayStep {
    fn skip(&self, ctx: &Context) -> bool {
        !ctx.use_coincube_relay
            || ctx.network == coincube_core::miniscript::bitcoin::Network::Regtest
    }

    fn apply(&mut self, ctx: &mut Context) -> bool {
        if let Some(token) = &self.jwt {
            ctx.bitcoin_backend = Some(BitcoinBackend::Esplora(EsploraConfig {
                addr: super::super::relay_url(ctx.network),
                token: Some(token.clone()),
            }));
            true
        } else {
            false
        }
    }

    fn update(&mut self, _hws: &mut HardwareWallets, message: Message) -> Task<Message> {
        if let Message::CoincubeRelay(msg) = message {
            match msg {
                CoincubeRelayMsg::EmailEdited(value) => {
                    self.email.value = value;
                    self.email.valid =
                        !self.email.value.is_empty() && self.email.value.contains('@');
                }
                CoincubeRelayMsg::ToggleMode => {
                    if !self.processing {
                        self.is_signup = !self.is_signup;
                        self.error = None;
                    }
                }
                CoincubeRelayMsg::RequestOtp => {
                    let client = self.client.clone();
                    let email = self.email.value.clone();
                    let is_signup = self.is_signup;
                    self.processing = true;
                    self.error = None;
                    return Task::perform(
                        async move {
                            let req = OtpRequest { email };
                            if is_signup {
                                client.signup_send_otp(req).await
                            } else {
                                client.login_send_otp(req).await
                            }
                            .map_err(|e| e.to_string())
                        },
                        |res| Message::CoincubeRelay(CoincubeRelayMsg::OtpRequested(res)),
                    );
                }
                CoincubeRelayMsg::ResendOtp => {
                    let client = self.client.clone();
                    let email = self.email.value.clone();
                    let is_signup = self.is_signup;
                    self.processing = true;
                    self.error = None;
                    return Task::perform(
                        async move {
                            let req = OtpRequest { email };
                            if is_signup {
                                client.signup_send_otp(req).await
                            } else {
                                client.login_send_otp(req).await
                            }
                            .map_err(|e| e.to_string())
                        },
                        |res| Message::CoincubeRelay(CoincubeRelayMsg::OtpResent(res)),
                    );
                }
                CoincubeRelayMsg::OtpRequested(res) => {
                    self.processing = false;
                    match res {
                        Ok(()) => {
                            self.otp_sent = true;
                            self.otp = form::Value::default();
                            self.error = None;
                        }
                        Err(e) => {
                            self.error = Some(e);
                        }
                    }
                }
                CoincubeRelayMsg::OtpResent(res) => {
                    self.processing = false;
                    if let Err(e) = res {
                        self.error = Some(e);
                    }
                }
                CoincubeRelayMsg::OtpEdited(value) => {
                    self.otp.value = value.trim().to_string();
                    self.otp.valid = true;
                    if self.otp.value.len() == 6 && !self.processing {
                        let client = self.client.clone();
                        let email = self.email.value.clone();
                        let otp = self.otp.value.clone();
                        let is_signup = self.is_signup;
                        self.processing = true;
                        self.error = None;
                        return Task::perform(
                            async move {
                                let req = OtpVerifyRequest { email, otp };
                                if is_signup {
                                    client.signup_verify_otp(req).await
                                } else {
                                    client.login_verify_otp(req).await
                                }
                                .map(|resp| resp.token)
                                .map_err(|e| e.to_string())
                            },
                            |res| Message::CoincubeRelay(CoincubeRelayMsg::OtpVerified(res)),
                        );
                    }
                }
                CoincubeRelayMsg::OtpVerified(res) => {
                    self.processing = false;
                    match res {
                        Ok(token) => {
                            self.jwt = Some(token);
                            return Task::done(Message::Next);
                        }
                        Err(e) => {
                            self.otp.valid = false;
                            self.error = Some(e);
                        }
                    }
                }
            }
        }
        Task::none()
    }

    fn view<'a>(
        &'a self,
        _hws: &'a HardwareWallets,
        progress: (usize, usize),
        _email: Option<&'a str>,
    ) -> Element<'a, Message> {
        view::define_coincube_relay(
            progress,
            &self.email,
            &self.otp,
            self.otp_sent,
            self.is_signup,
            self.processing,
            self.error.as_deref(),
        )
    }
}
