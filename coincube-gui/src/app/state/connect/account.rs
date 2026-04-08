use iced::widget::qr_code;

use crate::{
    app::{
        menu::ConnectSubMenu,
        message::Message,
        view::{self, ConnectAccountMessage},
    },
    services::coincube::{
        BillingCycle, BillingHistoryEntry, ChargeStatus, CheckoutRequest, CheckoutResponse,
        CoincubeClient, ConnectPlan, FeaturesResponse, LoginActivity, LoginResponse, OtpRequest,
        OtpVerifyRequest, User, VerifiedDevice,
    },
};

use super::{CONNECT_KEYRING_SERVICE, CONNECT_KEYRING_USER};

// ── Checkout state machine ──────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum CheckoutPhase {
    /// Waiting for POST /checkout response.
    Creating,
    /// Invoice received, awaiting payment.
    AwaitingPayment,
    /// Server reported "processing" (mempool confirmation pending).
    Processing,
    /// Payment confirmed.
    Paid,
    /// Invoice expired before payment.
    Expired,
    /// API error during checkout creation or polling.
    Failed(String),
}

#[derive(Debug)]
pub struct CheckoutState {
    pub phase: CheckoutPhase,
    pub checkout: Option<CheckoutResponse>,
    pub lightning_qr: Option<qr_code::Data>,
}

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
    // ── Plan & Billing ──
    /// Cached plan features from GET /connect/features.
    pub features: Option<FeaturesResponse>,
    /// The currently selected billing cycle for the upgrade cards.
    pub selected_billing_cycle: BillingCycle,
    /// Active checkout flow (None when no checkout in progress).
    pub checkout: Option<CheckoutState>,
    /// Billing history entries.
    pub billing_history: Option<Vec<BillingHistoryEntry>>,
    /// Whether the billing history sub-view is shown.
    pub show_billing_history: bool,
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
            features: None,
            selected_billing_cycle: BillingCycle::Monthly,
            checkout: None,
            billing_history: None,
            show_billing_history: false,
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
                // Fetch plan + features in background (non-blocking)
                let gen = self.session_generation;
                let c1 = self.client.clone();
                let c2 = self.client.clone();
                return iced::Task::batch([
                    iced::Task::perform(
                        async move { (c1.get_connect_plan().await.ok(), gen) },
                        |(plan, g)| {
                            Message::View(view::Message::ConnectAccount(
                                ConnectAccountMessage::PlanLoaded(plan, g),
                            ))
                        },
                    ),
                    iced::Task::perform(
                        async move { (c2.get_connect_features().await.ok(), gen) },
                        |(features, g)| {
                            Message::View(view::Message::ConnectAccount(
                                ConnectAccountMessage::FeaturesLoaded(features, g),
                            ))
                        },
                    ),
                ]);
            }

            ConnectAccountMessage::PlanLoaded(plan, gen) => {
                if gen == self.session_generation && plan.is_some() {
                    self.plan = plan;
                }
            }

            ConnectAccountMessage::LogOut => {
                let was_logged_in = self.user.is_some();
                self.session_generation += 1;
                self.user = None;
                self.plan = None;
                self.verified_devices = None;
                self.login_activity = None;
                self.features = None;
                self.checkout = None;
                self.billing_history = None;
                self.show_billing_history = false;
                self.clear_keyring_session();
                self.client = CoincubeClient::new();
                self.step = ConnectFlowStep::Login {
                    email: String::new(),
                    loading: false,
                };
                // Only notify BuySell if this logout originated here (not
                // forwarded back from BuySell) to avoid a redundant cycle.
                if was_logged_in {
                    return iced::Task::done(Message::View(view::Message::BuySell(
                        view::BuySellMessage::LogOut,
                    )));
                }
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

            // ── Plan & Billing ──────────────────────────────────────────

            ConnectAccountMessage::FeaturesLoaded(features, gen) => {
                if gen == self.session_generation {
                    self.features = features;
                }
            }

            ConnectAccountMessage::BillingCycleSelected(cycle) => {
                self.selected_billing_cycle = cycle;
            }

            ConnectAccountMessage::StartCheckout(tier) => {
                self.checkout = Some(CheckoutState {
                    phase: CheckoutPhase::Creating,
                    checkout: None,
                    lightning_qr: None,
                });
                let gen = self.session_generation;
                let client = self.client.clone();
                let req = CheckoutRequest {
                    plan: tier,
                    billing_cycle: self.selected_billing_cycle,
                };
                return iced::Task::perform(
                    async move { client.create_checkout(req).await.map_err(|e| e.to_string()) },
                    move |result| {
                        Message::View(view::Message::ConnectAccount(
                            ConnectAccountMessage::CheckoutCreated(result, gen),
                        ))
                    },
                );
            }

            ConnectAccountMessage::CheckoutCreated(result, gen) => {
                if gen != self.session_generation {
                    return iced::Task::none();
                }
                match result {
                    Ok(resp) => {
                        let qr = qr_code::Data::new(&resp.lightning_invoice).ok();
                        self.checkout = Some(CheckoutState {
                            phase: CheckoutPhase::AwaitingPayment,
                            checkout: Some(resp),
                            lightning_qr: qr,
                        });
                        return iced::Task::done(Message::View(view::Message::ConnectAccount(
                            ConnectAccountMessage::PollChargeStatus,
                        )));
                    }
                    Err(e) => {
                        if let Some(cs) = &mut self.checkout {
                            cs.phase = CheckoutPhase::Failed(e);
                        }
                    }
                }
            }

            ConnectAccountMessage::PollChargeStatus => {
                let should_poll = self
                    .checkout
                    .as_ref()
                    .map(|cs| {
                        matches!(
                            cs.phase,
                            CheckoutPhase::AwaitingPayment | CheckoutPhase::Processing
                        )
                    })
                    .unwrap_or(false);
                if !should_poll {
                    return iced::Task::none();
                }
                let charge_id = self
                    .checkout
                    .as_ref()
                    .and_then(|cs| cs.checkout.as_ref())
                    .map(|c| c.charge_id.clone())
                    .unwrap_or_default();
                let gen = self.session_generation;
                let client = self.client.clone();
                return iced::Task::perform(
                    async move {
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                        client
                            .get_charge_status(&charge_id)
                            .await
                            .map_err(|e| e.to_string())
                    },
                    move |result| {
                        Message::View(view::Message::ConnectAccount(
                            ConnectAccountMessage::ChargeStatusUpdated(result, gen),
                        ))
                    },
                );
            }

            ConnectAccountMessage::ChargeStatusUpdated(result, gen) => {
                if gen != self.session_generation || self.checkout.is_none() {
                    return iced::Task::none();
                }
                match result {
                    Ok(status) => {
                        let cs = self.checkout.as_mut().unwrap();
                        match status.status {
                            ChargeStatus::Unpaid => {
                                // Keep polling
                                return iced::Task::done(Message::View(
                                    view::Message::ConnectAccount(
                                        ConnectAccountMessage::PollChargeStatus,
                                    ),
                                ));
                            }
                            ChargeStatus::Processing => {
                                cs.phase = CheckoutPhase::Processing;
                                return iced::Task::done(Message::View(
                                    view::Message::ConnectAccount(
                                        ConnectAccountMessage::PollChargeStatus,
                                    ),
                                ));
                            }
                            ChargeStatus::Paid => {
                                cs.phase = CheckoutPhase::Paid;
                                // Refresh plan
                                let g = self.session_generation;
                                let c = self.client.clone();
                                return iced::Task::perform(
                                    async move { (c.get_connect_plan().await.ok(), g) },
                                    |(plan, g)| {
                                        Message::View(view::Message::ConnectAccount(
                                            ConnectAccountMessage::PlanLoaded(plan, g),
                                        ))
                                    },
                                );
                            }
                            ChargeStatus::Expired => {
                                cs.phase = CheckoutPhase::Expired;
                            }
                        }
                    }
                    Err(e) => {
                        log::warn!("[CONNECT] Charge poll error: {}", e);
                        // Continue polling on transient errors
                        return iced::Task::done(Message::View(view::Message::ConnectAccount(
                            ConnectAccountMessage::PollChargeStatus,
                        )));
                    }
                }
            }

            ConnectAccountMessage::DismissCheckout => {
                self.checkout = None;
            }

            ConnectAccountMessage::OpenCheckoutUrl(url) => {
                return iced::Task::done(Message::View(view::Message::OpenUrl(url)));
            }

            ConnectAccountMessage::ToggleBillingHistory => {
                self.show_billing_history = !self.show_billing_history;
                if self.show_billing_history && self.billing_history.is_none() {
                    let gen = self.session_generation;
                    let client = self.client.clone();
                    return iced::Task::perform(
                        async move { client.get_billing_history().await.map_err(|e| e.to_string()) },
                        move |result| {
                            Message::View(view::Message::ConnectAccount(
                                ConnectAccountMessage::BillingHistoryLoaded(result, gen),
                            ))
                        },
                    );
                }
            }

            ConnectAccountMessage::BillingHistoryLoaded(result, gen) => {
                if gen == self.session_generation {
                    match result {
                        Ok(history) => self.billing_history = Some(history),
                        Err(e) => self.error = Some(e),
                    }
                }
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
