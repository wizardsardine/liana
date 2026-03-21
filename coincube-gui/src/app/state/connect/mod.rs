use std::sync::Arc;

use crate::{
    app::{
        cache::Cache,
        menu::{ConnectSubMenu, Menu},
        message::Message,
        state::State,
        view::{self, ConnectMessage},
    },
    daemon::Daemon,
    services::coincube::{
        CoincubeClient, ConnectPlan, LoginActivity, LoginResponse, OtpRequest, OtpVerifyRequest,
        User, VerifiedDevice,
    },
};

const KEYRING_SERVICE_NAME: &str = if cfg!(debug_assertions) {
    "dev.coincube.Connect"
} else {
    "io.coincube.Connect"
};

const KEYRING_USER_KEY: &str = "global_session";

#[derive(Debug)]
pub enum ConnectFlowStep {
    CheckingSession,
    Login {
        email: String,
        loading: bool,
    },
    Register {
        email: String,
        loading: bool,
    },
    OtpVerification {
        email: String,
        otp: String,
        sending: bool,
        is_signup: bool,
        cooldown: u8,
    },
    Dashboard,
}

pub struct ConnectPanel {
    pub step: ConnectFlowStep,
    pub active_sub: ConnectSubMenu,
    pub client: CoincubeClient,
    pub user: Option<User>,
    pub plan: Option<ConnectPlan>,
    pub verified_devices: Option<Vec<VerifiedDevice>>,
    pub login_activity: Option<Vec<LoginActivity>>,
    pub error: Option<String>,
}

impl ConnectPanel {
    pub fn new() -> Self {
        ConnectPanel {
            step: ConnectFlowStep::CheckingSession,
            active_sub: ConnectSubMenu::Overview,
            client: CoincubeClient::new(),
            user: None,
            plan: None,
            verified_devices: None,
            login_activity: None,
            error: None,
        }
    }

    fn load_session_from_keyring(&mut self) -> Option<LoginResponse> {
        match keyring::Entry::new(KEYRING_SERVICE_NAME, KEYRING_USER_KEY) {
            Ok(entry) => match entry.get_secret() {
                Ok(bytes) => match serde_json::from_slice::<LoginResponse>(&bytes) {
                    Ok(l) => Some(l),
                    Err(e) => {
                        log::error!("[CONNECT] Failed to parse keyring session: {:?}", e);
                        None
                    }
                },
                Err(_) => None,
            },
            Err(e) => {
                log::error!("[CONNECT] Keyring inaccessible: {}", e);
                None
            }
        }
    }

    fn save_session_to_keyring(&self, login: &LoginResponse) {
        match keyring::Entry::new(KEYRING_SERVICE_NAME, KEYRING_USER_KEY) {
            Ok(entry) => {
                let _ = entry.delete_credential();
                if let Ok(bytes) = serde_json::to_vec(login) {
                    if let Err(e) = entry.set_secret(&bytes) {
                        log::error!("[CONNECT] Failed to save session to keyring: {}", e);
                    }
                }
            }
            Err(e) => log::error!("[CONNECT] Keyring inaccessible for save: {}", e),
        }
    }

    fn clear_keyring_session(&self) {
        if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE_NAME, KEYRING_USER_KEY) {
            let _ = entry.delete_credential();
        }
    }

    fn post_login_tasks(&mut self, login: LoginResponse) -> iced::Task<Message> {
        self.save_session_to_keyring(&login);
        self.client.set_token(&login.token);
        let client = self.client.clone();
        iced::Task::perform(
            async move {
                let user = client.get_user().await;
                let plan = client.get_connect_plan().await;
                (user, plan)
            },
            |(user_res, plan_res)| {
                let user = match user_res {
                    Ok(u) => u,
                    Err(e) => {
                        return Message::View(view::Message::Connect(ConnectMessage::Error(
                            e.to_string(),
                        )));
                    }
                };
                let plan = plan_res.ok();
                Message::View(view::Message::Connect(ConnectMessage::SessionLoaded {
                    user,
                    plan,
                }))
            },
        )
    }
}

impl Default for ConnectPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl State for ConnectPanel {
    fn view<'a>(
        &'a self,
        menu: &'a Menu,
        cache: &'a Cache,
    ) -> coincube_ui::widget::Element<'a, view::Message> {
        view::dashboard(
            menu,
            cache,
            view::connect::connect_panel(self).map(view::Message::Connect),
        )
    }

    fn reload(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        _wallet: Option<Arc<crate::app::wallet::Wallet>>,
    ) -> iced::Task<Message> {
        iced::Task::done(Message::View(view::Message::Connect(ConnectMessage::Init)))
    }

    fn update(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        _cache: &Cache,
        message: Message,
    ) -> iced::Task<Message> {
        let msg = match message {
            Message::View(view::Message::Connect(m)) => m,
            _ => return iced::Task::none(),
        };

        match msg {
            ConnectMessage::Init => {
                if let Some(session) = self.load_session_from_keyring() {
                    let refresh_token = session.refresh_token.clone();
                    return iced::Task::done(Message::View(view::Message::Connect(
                        ConnectMessage::RefreshSession { refresh_token },
                    )));
                }
                self.step = ConnectFlowStep::Login {
                    email: String::new(),
                    loading: false,
                };
            }

            ConnectMessage::RefreshSession { refresh_token } => {
                let client = self.client.clone();
                return iced::Task::perform(
                    async move { client.refresh_login(&refresh_token).await },
                    |res| match res {
                        Ok(login) => {
                            Message::View(view::Message::Connect(ConnectMessage::SetSession(login)))
                        }
                        Err(_) => Message::View(view::Message::Connect(ConnectMessage::LogOut)),
                    },
                );
            }

            ConnectMessage::SetSession(login) => {
                return self.post_login_tasks(login);
            }

            ConnectMessage::SessionLoaded { user, plan } => {
                self.user = Some(user);
                self.plan = plan;
                self.step = ConnectFlowStep::Dashboard;
                self.error = None;
            }

            ConnectMessage::LogOut => {
                self.user = None;
                self.plan = None;
                self.verified_devices = None;
                self.login_activity = None;
                self.clear_keyring_session();
                self.client = CoincubeClient::new();
                self.step = ConnectFlowStep::Login {
                    email: String::new(),
                    loading: false,
                };
            }

            ConnectMessage::EmailChanged(email) => match &mut self.step {
                ConnectFlowStep::Login { email: e, .. }
                | ConnectFlowStep::Register { email: e, .. } => *e = email,
                _ => {}
            },

            ConnectMessage::SubmitLogin => {
                let ConnectFlowStep::Login { email, loading } = &mut self.step else {
                    return iced::Task::none();
                };
                *loading = true;
                let email_val = email.clone();
                let client = self.client.clone();
                return iced::Task::perform(
                    async move { client.login_send_otp(OtpRequest { email: email_val }).await },
                    |res| match res {
                        Ok(()) => Message::View(view::Message::Connect(
                            ConnectMessage::OtpChanged(String::new()),
                        )),
                        Err(e) => Message::View(view::Message::Connect(ConnectMessage::Error(
                            e.to_string(),
                        ))),
                    },
                );
            }

            ConnectMessage::SubmitRegistration => {
                let ConnectFlowStep::Register { email, loading } = &mut self.step else {
                    return iced::Task::none();
                };
                *loading = true;
                let email_val = email.clone();
                let client = self.client.clone();
                return iced::Task::perform(
                    async move {
                        client
                            .signup_send_otp(OtpRequest { email: email_val })
                            .await
                    },
                    |res| match res {
                        Ok(()) => Message::View(view::Message::Connect(
                            ConnectMessage::OtpChanged(String::new()),
                        )),
                        Err(e) => Message::View(view::Message::Connect(ConnectMessage::Error(
                            e.to_string(),
                        ))),
                    },
                );
            }

            ConnectMessage::CreateAccount => {
                self.step = ConnectFlowStep::Register {
                    email: String::new(),
                    loading: false,
                };
            }

            ConnectMessage::OtpChanged(otp) => {
                if let ConnectFlowStep::OtpVerification { otp: o, .. } = &mut self.step {
                    *o = otp;
                } else {
                    // Transition into OTP step (email came from Login/Register)
                    let email = match &self.step {
                        ConnectFlowStep::Login { email, .. } => email.clone(),
                        ConnectFlowStep::Register { email, .. } => email.clone(),
                        _ => String::new(),
                    };
                    let is_signup = matches!(self.step, ConnectFlowStep::Register { .. });
                    self.step = ConnectFlowStep::OtpVerification {
                        email,
                        otp,
                        sending: false,
                        is_signup,
                        cooldown: 60,
                    };
                    return iced::Task::done(Message::View(view::Message::Connect(
                        ConnectMessage::OtpCooldownTick,
                    )));
                }
            }

            ConnectMessage::OtpCooldownTick => {
                if let ConnectFlowStep::OtpVerification { cooldown, .. } = &mut self.step {
                    if *cooldown > 0 {
                        *cooldown -= 1;
                        return iced::Task::perform(
                            async { tokio::time::sleep(std::time::Duration::from_secs(1)).await },
                            |_| {
                                Message::View(view::Message::Connect(
                                    ConnectMessage::OtpCooldownTick,
                                ))
                            },
                        );
                    }
                }
            }

            ConnectMessage::VerifyOtp => {
                let ConnectFlowStep::OtpVerification {
                    email,
                    otp,
                    sending,
                    is_signup,
                    ..
                } = &mut self.step
                else {
                    return iced::Task::none();
                };
                *sending = true;
                let req = OtpVerifyRequest {
                    email: email.clone(),
                    otp: otp.clone(),
                };
                let is_signup = *is_signup;
                let client = self.client.clone();
                return iced::Task::perform(
                    async move {
                        if is_signup {
                            client.signup_verify_otp(req).await
                        } else {
                            client.login_verify_otp(req).await
                        }
                    },
                    |res| match res {
                        Ok(login) => {
                            Message::View(view::Message::Connect(ConnectMessage::SetSession(login)))
                        }
                        Err(e) => Message::View(view::Message::Connect(ConnectMessage::Error(
                            e.to_string(),
                        ))),
                    },
                );
            }

            ConnectMessage::VerifiedDevicesLoaded(devices) => {
                self.verified_devices = Some(devices);
            }

            ConnectMessage::LoginActivityLoaded(activity) => {
                self.login_activity = Some(activity);
            }

            ConnectMessage::Error(e) => {
                log::error!("[CONNECT] Error: {}", e);
                self.error = Some(e);
                // Reset loading state
                match &mut self.step {
                    ConnectFlowStep::Login { loading, .. } => *loading = false,
                    ConnectFlowStep::Register { loading, .. } => *loading = false,
                    ConnectFlowStep::OtpVerification { sending, .. } => *sending = false,
                    _ => {}
                }
            }
        }

        iced::Task::none()
    }
}

/// Load Security tab data (verified devices + login activity).
pub fn load_security_data(client: &CoincubeClient) -> iced::Task<Message> {
    let c1 = client.clone();
    let c2 = client.clone();
    iced::Task::batch([
        iced::Task::perform(
            async move { c1.get_verified_devices().await },
            |res| match res {
                Ok(devices) => Message::View(view::Message::Connect(
                    ConnectMessage::VerifiedDevicesLoaded(devices),
                )),
                Err(e) => {
                    Message::View(view::Message::Connect(ConnectMessage::Error(e.to_string())))
                }
            },
        ),
        iced::Task::perform(
            async move { c2.get_login_activity().await },
            |res| match res {
                Ok(activity) => Message::View(view::Message::Connect(
                    ConnectMessage::LoginActivityLoaded(activity),
                )),
                Err(e) => {
                    Message::View(view::Message::Connect(ConnectMessage::Error(e.to_string())))
                }
            },
        ),
    ])
}
