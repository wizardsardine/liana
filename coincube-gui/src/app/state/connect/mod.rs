use std::sync::Arc;

use crate::{
    app::{
        breez::BreezClient,
        cache::Cache,
        menu::{ConnectSubMenu, Menu},
        message::Message,
        state::State,
        view::{self, ConnectMessage},
    },
    daemon::Daemon,
    services::coincube::{
        ClaimLightningAddressRequest, CoincubeClient, ConnectPlan, LightningAddress, LoginActivity,
        LoginResponse, OtpRequest, OtpVerifyRequest, User, VerifiedDevice,
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
    // Lightning Address
    pub lightning_address: Option<LightningAddress>,
    pub ln_username_input: String,
    pub ln_username_available: Option<bool>,
    pub ln_username_error: Option<String>,
    pub ln_claiming: bool,
    pub ln_checking: bool,
    ln_check_version: u32,
    breez_client: Arc<BreezClient>,
}

impl ConnectPanel {
    pub fn new(breez_client: Arc<BreezClient>) -> Self {
        ConnectPanel {
            step: ConnectFlowStep::CheckingSession,
            active_sub: ConnectSubMenu::Overview,
            client: CoincubeClient::new(),
            user: None,
            plan: None,
            verified_devices: None,
            login_activity: None,
            error: None,
            lightning_address: None,
            ln_username_input: String::new(),
            ln_username_available: None,
            ln_username_error: None,
            ln_claiming: false,
            ln_checking: false,
            ln_check_version: 0,
            breez_client,
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
                // Fetch lightning address in background
                let client = self.client.clone();
                return iced::Task::perform(
                    async move { client.get_lightning_address().await.ok() },
                    |ln_addr| {
                        Message::View(view::Message::Connect(
                            ConnectMessage::LightningAddressLoaded(ln_addr),
                        ))
                    },
                );
            }

            ConnectMessage::LogOut => {
                self.user = None;
                self.plan = None;
                self.verified_devices = None;
                self.login_activity = None;
                self.lightning_address = None;
                self.ln_username_input.clear();
                self.ln_username_available = None;
                self.ln_username_error = None;
                self.ln_claiming = false;
                self.ln_checking = false;
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

            // --- Lightning Address ---
            ConnectMessage::LightningAddressLoaded(ln_addr) => {
                self.lightning_address = ln_addr;
            }

            ConnectMessage::LnUsernameChanged(input) => {
                self.ln_username_input = input.to_lowercase();
                self.ln_username_available = None;
                self.ln_username_error = None;

                // Client-side validation
                if let Some(err) = validate_ln_username(&self.ln_username_input) {
                    self.ln_username_error = Some(err);
                    return iced::Task::none();
                }

                // Debounced availability check
                self.ln_check_version += 1;
                let version = self.ln_check_version;
                let client = self.client.clone();
                let username = self.ln_username_input.clone();
                self.ln_checking = true;
                return iced::Task::perform(
                    async move {
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                        let res = client.check_lightning_address(&username).await;
                        (res, version)
                    },
                    move |(res, v)| match res {
                        Ok(check) => Message::View(view::Message::Connect(
                            ConnectMessage::LnUsernameChecked {
                                available: check.available,
                                error_message: check.error_message,
                                version: v,
                            },
                        )),
                        Err(e) => Message::View(view::Message::Connect(ConnectMessage::Error(
                            e.to_string(),
                        ))),
                    },
                );
            }

            ConnectMessage::CheckLnUsername => {
                // Manual check trigger (not currently used, debounce handles it)
            }

            ConnectMessage::LnUsernameChecked {
                available,
                error_message,
                version,
            } => {
                // Discard stale results
                if version == self.ln_check_version {
                    self.ln_checking = false;
                    self.ln_username_available = Some(available);
                    if !available {
                        self.ln_username_error =
                            Some(error_message.unwrap_or_else(|| "Username is taken".to_string()));
                    }
                }
            }

            ConnectMessage::ClaimLightningAddress => {
                if self.ln_claiming {
                    return iced::Task::none();
                }
                self.ln_claiming = true;
                self.error = None;
                let username = self.ln_username_input.clone();
                let client = self.client.clone();
                let breez = self.breez_client.clone();
                return iced::Task::perform(
                    async move {
                        // First get the BOLT12 offer from Breez SDK
                        let bolt12_offer = breez
                            .receive_bolt12_offer()
                            .await
                            .map_err(|e| e.to_string())?;
                        // Then claim the address via API
                        client
                            .claim_lightning_address(ClaimLightningAddressRequest {
                                username,
                                bolt12_offer,
                            })
                            .await
                            .map_err(|e| e.to_string())
                    },
                    |res| match res {
                        Ok(ln_addr) => Message::View(view::Message::Connect(
                            ConnectMessage::LightningAddressClaimed(ln_addr),
                        )),
                        Err(e) => Message::View(view::Message::Connect(ConnectMessage::Error(
                            e.to_string(),
                        ))),
                    },
                );
            }

            ConnectMessage::LightningAddressClaimed(ln_addr) => {
                self.ln_claiming = false;
                self.lightning_address = Some(ln_addr);
                self.ln_username_input.clear();
                self.ln_username_available = None;
                self.ln_username_error = None;
            }

            ConnectMessage::CopyToClipboard(text) => {
                return iced::clipboard::write(text);
            }

            ConnectMessage::Error(e) => {
                log::error!("[CONNECT] Error: {}", e);
                self.error = Some(e);
                self.ln_claiming = false;
                self.ln_checking = false;
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

/// Validate a lightning address username client-side.
/// Returns `Some(error_message)` if invalid, `None` if valid.
fn validate_ln_username(username: &str) -> Option<String> {
    if username.is_empty() {
        return Some("Username is required".to_string());
    }
    if username.len() < 3 {
        return Some("Username must be at least 3 characters".to_string());
    }
    if username.len() > 64 {
        return Some("Username must be at most 64 characters".to_string());
    }
    if !username.chars().next().unwrap().is_ascii_alphanumeric() {
        return Some("Must start with a letter or number".to_string());
    }
    if !username.chars().last().unwrap().is_ascii_alphanumeric() {
        return Some("Must end with a letter or number".to_string());
    }
    let special = ['.', '_', '-'];
    for c in username.chars() {
        if !c.is_ascii_alphanumeric() && !special.contains(&c) {
            return Some(format!("Invalid character: '{}'", c));
        }
    }
    // No consecutive special characters
    let chars: Vec<char> = username.chars().collect();
    for w in chars.windows(2) {
        if special.contains(&w[0]) && special.contains(&w[1]) {
            return Some("No consecutive special characters allowed".to_string());
        }
    }
    None
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
