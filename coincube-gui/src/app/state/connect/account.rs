use crate::{
    app::{
        menu::ConnectSubMenu,
        message::Message,
        view::{self, ConnectAccountMessage},
    },
    services::coincube::{
        CoincubeClient, ConnectPlan, LoginActivity, LoginResponse, OtpRequest, OtpVerifyRequest,
        User, VerifiedDevice,
    },
};

use super::{CONNECT_KEYRING_SERVICE, CONNECT_KEYRING_USER};

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

pub struct ConnectAccountPanel {
    pub step: ConnectFlowStep,
    pub active_sub: ConnectSubMenu,
    pub client: CoincubeClient,
    pub user: Option<User>,
    pub plan: Option<ConnectPlan>,
    pub verified_devices: Option<Vec<VerifiedDevice>>,
    pub login_activity: Option<Vec<LoginActivity>>,
    pub error: Option<String>,
    /// Incremented on each login/logout so stale async completions can be discarded.
    session_generation: u64,
}

impl ConnectAccountPanel {
    pub fn new() -> Self {
        ConnectAccountPanel {
            step: ConnectFlowStep::CheckingSession,
            active_sub: ConnectSubMenu::LightningAddress,
            client: CoincubeClient::new(),
            user: None,
            plan: None,
            verified_devices: None,
            login_activity: None,
            error: None,
            session_generation: 0,
        }
    }

    /// Returns a clone of the authenticated client (with JWT set).
    /// Used by ConnectCubePanel to make API calls.
    pub fn authenticated_client(&self) -> Option<CoincubeClient> {
        if self.user.is_some() {
            Some(self.client.clone())
        } else {
            None
        }
    }

    pub fn is_authenticated(&self) -> bool {
        matches!(self.step, ConnectFlowStep::Dashboard)
    }

    pub fn session_generation(&self) -> u64 {
        self.session_generation
    }

    fn load_session_from_keyring(&mut self) -> Option<LoginResponse> {
        match keyring::Entry::new(CONNECT_KEYRING_SERVICE, CONNECT_KEYRING_USER) {
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
        match keyring::Entry::new(CONNECT_KEYRING_SERVICE, CONNECT_KEYRING_USER) {
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
        if let Ok(entry) = keyring::Entry::new(CONNECT_KEYRING_SERVICE, CONNECT_KEYRING_USER) {
            let _ = entry.delete_credential();
        }
    }

    fn post_login_tasks(&mut self, login: LoginResponse) -> iced::Task<Message> {
        self.save_session_to_keyring(&login);
        self.client.set_token(&login.token);

        let user = login.user;
        iced::Task::done(Message::View(view::Message::ConnectAccount(
            ConnectAccountMessage::SessionLoaded { user, plan: None },
        )))
    }

    pub fn update_message(&mut self, msg: ConnectAccountMessage) -> iced::Task<Message> {
        match msg {
            ConnectAccountMessage::Init => {
                if let Some(session) = self.load_session_from_keyring() {
                    let refresh_token = session.refresh_token.clone();
                    return iced::Task::done(Message::View(view::Message::ConnectAccount(
                        ConnectAccountMessage::RefreshSession { refresh_token },
                    )));
                }
                self.step = ConnectFlowStep::Login {
                    email: String::new(),
                    loading: false,
                };
            }

            ConnectAccountMessage::RefreshSession { refresh_token } => {
                let client = self.client.clone();
                return iced::Task::perform(
                    async move { client.refresh_login(&refresh_token).await },
                    |res| match res {
                        Ok(login) => Message::View(view::Message::ConnectAccount(
                            ConnectAccountMessage::SetSession(login),
                        )),
                        Err(e) => {
                            if e.is_auth_error() {
                                Message::View(view::Message::ConnectAccount(
                                    ConnectAccountMessage::LogOut,
                                ))
                            } else {
                                Message::View(view::Message::ConnectAccount(
                                    ConnectAccountMessage::RefreshFailed(e.to_string()),
                                ))
                            }
                        }
                    },
                );
            }

            ConnectAccountMessage::RefreshFailed(err) => {
                log::warn!("[CONNECT] Session refresh failed (transient): {}", err);
                self.error = Some(format!("Connection error: {}. Tap to retry.", err));
                self.step = ConnectFlowStep::Login {
                    email: String::new(),
                    loading: false,
                };
            }

            ConnectAccountMessage::SetSession(login) => {
                return self.post_login_tasks(login);
            }

            ConnectAccountMessage::SessionLoaded { user, plan } => {
                self.session_generation += 1;
                self.user = Some(user);
                self.plan = plan;
                self.step = ConnectFlowStep::Dashboard;
                self.error = None;
                // Fetch plan in background (non-blocking)
                let gen = self.session_generation;
                let c1 = self.client.clone();
                return iced::Task::perform(
                    async move { (c1.get_connect_plan().await.ok(), gen) },
                    |(plan, g)| {
                        Message::View(view::Message::ConnectAccount(
                            ConnectAccountMessage::PlanLoaded(plan, g),
                        ))
                    },
                );
            }

            ConnectAccountMessage::PlanLoaded(plan, gen) => {
                if gen == self.session_generation && plan.is_some() {
                    self.plan = plan;
                }
            }

            ConnectAccountMessage::LogOut => {
                self.session_generation += 1;
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

            ConnectAccountMessage::EmailChanged(email) => match &mut self.step {
                ConnectFlowStep::Login { email: e, .. }
                | ConnectFlowStep::Register { email: e, .. } => *e = email,
                _ => {}
            },

            ConnectAccountMessage::SubmitLogin => {
                let ConnectFlowStep::Login { email, loading } = &mut self.step else {
                    return iced::Task::none();
                };
                *loading = true;
                let email_val = email.clone();
                let client = self.client.clone();
                return iced::Task::perform(
                    async move {
                        client
                            .login_send_otp(OtpRequest {
                                email: email_val.clone(),
                            })
                            .await
                            .map(|()| email_val)
                    },
                    |res| match res {
                        Ok(email) => Message::View(view::Message::ConnectAccount(
                            ConnectAccountMessage::OtpRequested {
                                email,
                                is_signup: false,
                            },
                        )),
                        Err(e) => Message::View(view::Message::ConnectAccount(
                            ConnectAccountMessage::Error(e.to_string()),
                        )),
                    },
                );
            }

            ConnectAccountMessage::SubmitRegistration => {
                let ConnectFlowStep::Register { email, loading } = &mut self.step else {
                    return iced::Task::none();
                };
                *loading = true;
                let email_val = email.clone();
                let client = self.client.clone();
                return iced::Task::perform(
                    async move {
                        client
                            .signup_send_otp(OtpRequest {
                                email: email_val.clone(),
                            })
                            .await
                            .map(|()| email_val)
                    },
                    |res| match res {
                        Ok(email) => Message::View(view::Message::ConnectAccount(
                            ConnectAccountMessage::OtpRequested {
                                email,
                                is_signup: true,
                            },
                        )),
                        Err(e) => Message::View(view::Message::ConnectAccount(
                            ConnectAccountMessage::Error(e.to_string()),
                        )),
                    },
                );
            }

            ConnectAccountMessage::CreateAccount => {
                self.step = ConnectFlowStep::Register {
                    email: String::new(),
                    loading: false,
                };
            }

            ConnectAccountMessage::OtpRequested { email, is_signup } => {
                self.step = ConnectFlowStep::OtpVerification {
                    email,
                    otp: String::new(),
                    sending: false,
                    is_signup,
                    cooldown: 60,
                };
                return iced::Task::done(Message::View(view::Message::ConnectAccount(
                    ConnectAccountMessage::OtpCooldownTick,
                )));
            }

            ConnectAccountMessage::OtpChanged(otp) => {
                if let ConnectFlowStep::OtpVerification { otp: o, .. } = &mut self.step {
                    *o = otp;
                }
            }

            ConnectAccountMessage::OtpCooldownTick => {
                if let ConnectFlowStep::OtpVerification { cooldown, .. } = &mut self.step {
                    if *cooldown > 0 {
                        *cooldown -= 1;
                        return iced::Task::perform(
                            async { tokio::time::sleep(std::time::Duration::from_secs(1)).await },
                            |_| {
                                Message::View(view::Message::ConnectAccount(
                                    ConnectAccountMessage::OtpCooldownTick,
                                ))
                            },
                        );
                    }
                }
            }

            ConnectAccountMessage::ResendOtp => {
                let ConnectFlowStep::OtpVerification {
                    email,
                    sending,
                    is_signup,
                    cooldown,
                    ..
                } = &mut self.step
                else {
                    return iced::Task::none();
                };
                if *cooldown > 0 || *sending {
                    return iced::Task::none();
                }
                *sending = true;
                let email_val = email.clone();
                let is_signup = *is_signup;
                let client = self.client.clone();
                return iced::Task::perform(
                    async move {
                        if is_signup {
                            client
                                .signup_send_otp(OtpRequest { email: email_val })
                                .await
                        } else {
                            client.login_send_otp(OtpRequest { email: email_val }).await
                        }
                    },
                    |res| match res {
                        Ok(()) => Message::View(view::Message::ConnectAccount(
                            ConnectAccountMessage::OtpResent,
                        )),
                        Err(e) => Message::View(view::Message::ConnectAccount(
                            ConnectAccountMessage::Error(e.to_string()),
                        )),
                    },
                );
            }

            ConnectAccountMessage::OtpResent => {
                if let ConnectFlowStep::OtpVerification {
                    otp,
                    sending,
                    cooldown,
                    ..
                } = &mut self.step
                {
                    *otp = String::new();
                    *sending = false;
                    *cooldown = 60;
                    return iced::Task::done(Message::View(view::Message::ConnectAccount(
                        ConnectAccountMessage::OtpCooldownTick,
                    )));
                }
            }

            ConnectAccountMessage::VerifyOtp => {
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
                        Ok(login) => Message::View(view::Message::ConnectAccount(
                            ConnectAccountMessage::SetSession(login),
                        )),
                        Err(e) => Message::View(view::Message::ConnectAccount(
                            ConnectAccountMessage::Error(e.to_string()),
                        )),
                    },
                );
            }

            ConnectAccountMessage::VerifiedDevicesLoaded(devices, gen) => {
                if gen == self.session_generation && matches!(self.step, ConnectFlowStep::Dashboard)
                {
                    self.verified_devices = Some(devices);
                }
            }

            ConnectAccountMessage::LoginActivityLoaded(activity, gen) => {
                if gen == self.session_generation && matches!(self.step, ConnectFlowStep::Dashboard)
                {
                    self.login_activity = Some(activity);
                }
            }

            ConnectAccountMessage::CopyToClipboard(text) => {
                return iced::clipboard::write(text);
            }

            ConnectAccountMessage::Error(e) => {
                log::error!("[CONNECT] Error: {}", e);
                self.error = Some(e);
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

impl Default for ConnectAccountPanel {
    fn default() -> Self {
        Self::new()
    }
}

/// Load Security tab data (verified devices + login activity).
pub fn load_security_data(client: &CoincubeClient, generation: u64) -> iced::Task<Message> {
    let c1 = client.clone();
    let c2 = client.clone();
    iced::Task::batch([
        iced::Task::perform(
            async move { c1.get_verified_devices().await },
            move |res| match res {
                Ok(devices) => Message::View(view::Message::ConnectAccount(
                    ConnectAccountMessage::VerifiedDevicesLoaded(devices, generation),
                )),
                Err(e) => Message::View(view::Message::ConnectAccount(
                    ConnectAccountMessage::Error(e.to_string()),
                )),
            },
        ),
        iced::Task::perform(
            async move { c2.get_login_activity().await },
            move |res| match res {
                Ok(activity) => Message::View(view::Message::ConnectAccount(
                    ConnectAccountMessage::LoginActivityLoaded(activity, generation),
                )),
                Err(e) => Message::View(view::Message::ConnectAccount(
                    ConnectAccountMessage::Error(e.to_string()),
                )),
            },
        ),
    ])
}
