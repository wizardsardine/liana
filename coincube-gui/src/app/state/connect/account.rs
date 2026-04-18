use iced::widget::qr_code;

use crate::{
    app::{
        menu::ConnectSubMenu,
        message::Message,
        view::{self, ConnectAccountMessage, ContactsMessage},
    },
    services::coincube::{
        BillingCycle, BillingHistoryEntry, ChargeStatus, CheckoutRequest, CheckoutResponse,
        CoincubeClient, ConnectPlan, Contact, ContactCube, ContactRole, CreateInviteRequest,
        FeaturesResponse, Invite, LoginActivity, LoginResponse, OtpRequest, OtpVerifyRequest, User,
        VerifiedDevice,
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
    pub poll_errors: u8,
}

/// Which sub-view of the Contacts section is shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContactsStep {
    List,
    InviteForm,
    Detail(u64),
}

/// One cube option shown in the invite-form's "Also add to Cube(s)"
/// multi-select. Kept lightweight — we only need `{id, label}` for the
/// checkbox row.
#[derive(Debug, Clone)]
pub struct InviteCubeOption {
    pub id: u64,
    pub name: String,
    pub network: String,
}

/// State for the Contacts section within ConnectAccountPanel.
pub struct ContactsState {
    pub step: ContactsStep,
    pub contacts: Option<Vec<Contact>>,
    pub invites: Option<Vec<Invite>>,
    pub invite_email: String,
    pub invite_role: ContactRole,
    pub invite_sending: bool,
    pub detail_cubes: Option<Vec<ContactCube>>,
    pub detail_cubes_error: Option<String>,
    pub loading: bool,
    pub error: Option<String>,
    // --- W12: cube multi-select on the invite form ---
    /// Cubes the authenticated user owns or is a member of. Populated
    /// on `ShowInviteForm`. Empty (or `None` pending load) hides the
    /// section entirely per the plan.
    pub invite_available_cubes: Option<Vec<InviteCubeOption>>,
    /// Cube ids the user has ticked on the invite form. Cleared when
    /// the user navigates away from the form.
    pub invite_cube_selections: Vec<u64>,
    /// Non-`None` when the last submit 403'd on a cube id — drives the
    /// "one or more selected cubes is no longer available" dialog.
    pub invite_cube_error: Option<String>,
}

impl ContactsState {
    pub fn new() -> Self {
        Self {
            step: ContactsStep::List,
            contacts: None,
            invites: None,
            invite_email: String::new(),
            invite_role: ContactRole::Keyholder,
            invite_sending: false,
            detail_cubes: None,
            detail_cubes_error: None,
            loading: false,
            error: None,
            invite_available_cubes: None,
            invite_cube_selections: Vec::new(),
            invite_cube_error: None,
        }
    }

    pub fn clear(&mut self) {
        *self = Self::new();
    }
}

impl Default for ContactsState {
    fn default() -> Self {
        Self::new()
    }
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
    pub contacts_state: ContactsState,
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
            contacts_state: ContactsState::new(),
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

    /// Returns `true` if a session has been previously stored in the OS keyring,
    /// indicating the user has (or had) a Connect account on this device.
    pub fn has_stored_session() -> bool {
        keyring::Entry::new(CONNECT_KEYRING_SERVICE, CONNECT_KEYRING_USER)
            .ok()
            .and_then(|e| e.get_secret().ok())
            .is_some()
    }

    pub fn session_generation(&self) -> u64 {
        self.session_generation
    }

    /// Reset contacts state to list view and reload data from the API.
    pub fn reload_contacts(&mut self) -> iced::Task<Message> {
        self.contacts_state.step = ContactsStep::List;
        self.contacts_state.contacts = None;
        self.contacts_state.invites = None;
        self.contacts_state.error = None;
        self.contacts_state.loading = true;
        load_contacts_data(&self.client, self.session_generation)
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
                    // Transition out of CheckingSession so re-navigation
                    // won't re-trigger Init while the refresh is in flight.
                    self.step = ConnectFlowStep::Login {
                        email: String::new(),
                        loading: true,
                    };
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
                    if let Some(cycle) = plan.as_ref().and_then(|p| p.billing_cycle) {
                        self.selected_billing_cycle = cycle;
                    }
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
                self.selected_billing_cycle = BillingCycle::Monthly;
                self.contacts_state.clear();
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
                    poll_errors: 0,
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
                if gen != self.session_generation || self.checkout.is_none() {
                    return iced::Task::none();
                }
                match result {
                    Ok(resp) => {
                        let qr = qr_code::Data::new(&resp.lightning_invoice).ok();
                        self.checkout = Some(CheckoutState {
                            phase: CheckoutPhase::AwaitingPayment,
                            checkout: Some(resp),
                            lightning_qr: qr,
                            poll_errors: 0,
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
                        client.get_charge_status(&charge_id).await.map_err(|e| {
                            use crate::services::coincube::CoincubeError;
                            match &e {
                                // 4xx errors are terminal — don't retry
                                CoincubeError::Unsuccessful(info)
                                    if (400..500).contains(&info.status_code) =>
                                {
                                    (e.to_string(), true)
                                }
                                _ => (e.to_string(), false),
                            }
                        })
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
                        cs.poll_errors = 0;
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
                                // Invalidate cached billing history so next view fetches fresh data
                                self.billing_history = None;
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
                    Err((e, terminal)) => {
                        let cs = self.checkout.as_mut().unwrap();
                        if terminal {
                            log::error!("[CONNECT] Charge poll terminal error: {}", e);
                            cs.phase = CheckoutPhase::Failed(e);
                        } else {
                            cs.poll_errors += 1;
                            if cs.poll_errors >= 3 {
                                log::error!(
                                    "[CONNECT] Charge poll failed after {} retries: {}",
                                    cs.poll_errors,
                                    e
                                );
                                cs.phase = CheckoutPhase::Failed(e);
                            } else {
                                log::warn!(
                                    "[CONNECT] Charge poll error ({}/3): {}",
                                    cs.poll_errors,
                                    e
                                );
                                return iced::Task::done(Message::View(
                                    view::Message::ConnectAccount(
                                        ConnectAccountMessage::PollChargeStatus,
                                    ),
                                ));
                            }
                        }
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
                if self.show_billing_history {
                    let gen = self.session_generation;
                    let client = self.client.clone();
                    return iced::Task::perform(
                        async move {
                            client
                                .get_billing_history()
                                .await
                                .map_err(|e| e.to_string())
                        },
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
                        Err(e) => {
                            // Leave billing_history as None so ToggleBillingHistory retries
                            self.error = Some(e);
                        }
                    }
                }
            }
            ConnectAccountMessage::Contacts(contacts_msg) => {
                return self.update_contacts(contacts_msg);
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

impl ConnectAccountPanel {
    fn update_contacts(&mut self, msg: ContactsMessage) -> iced::Task<Message> {
        match msg {
            ContactsMessage::ContactsLoaded(contacts, gen) => {
                if gen == self.session_generation {
                    self.contacts_state.contacts = Some(contacts);
                    // Clear loading only when both are done
                    if self.contacts_state.invites.is_some() {
                        self.contacts_state.loading = false;
                    }
                }
            }

            ContactsMessage::InvitesLoaded(invites, gen) => {
                if gen == self.session_generation {
                    self.contacts_state.invites = Some(invites);
                    if self.contacts_state.contacts.is_some() {
                        self.contacts_state.loading = false;
                    }
                }
            }

            ContactsMessage::ShowInviteForm => {
                self.contacts_state.step = ContactsStep::InviteForm;
                self.contacts_state.invite_email.clear();
                self.contacts_state.invite_role = ContactRole::Keyholder;
                self.contacts_state.invite_sending = false;
                self.contacts_state.error = None;
                self.contacts_state.invite_cube_selections.clear();
                self.contacts_state.invite_cube_error = None;
                // Load the user's cubes for the "Also add to Cube(s)"
                // multi-select (W12). Hidden in the view until this
                // resolves; empty Vec renders as "no cubes section".
                let client = self.client.clone();
                let gen = self.session_generation;
                return iced::Task::perform(async move { client.list_cubes().await }, move |res| {
                    match res {
                        Ok(cubes) => {
                            let options = cubes
                                .into_iter()
                                .map(|c| InviteCubeOption {
                                    id: c.id,
                                    name: c.name,
                                    network: c.network,
                                })
                                .collect::<Vec<_>>();
                            Message::View(view::Message::ConnectAccount(
                                ConnectAccountMessage::Contacts(
                                    ContactsMessage::InviteCubesAvailable(options, gen),
                                ),
                            ))
                        }
                        Err(e) => {
                            // Leave the section hidden on load failure —
                            // the user can still send a plain invite.
                            log::warn!("[CONTACTS] Failed to list cubes for invite form: {}", e);
                            Message::View(view::Message::ConnectAccount(
                                ConnectAccountMessage::Contacts(
                                    ContactsMessage::InviteCubesAvailable(Vec::new(), gen),
                                ),
                            ))
                        }
                    }
                });
            }

            ContactsMessage::InviteCubesAvailable(cubes, gen) => {
                if gen == self.session_generation {
                    // Drop any prior selection that's no longer in the
                    // authoritative list (e.g. after a 403 reload where
                    // the user lost access to a cube mid-form).
                    let valid_ids: std::collections::HashSet<u64> =
                        cubes.iter().map(|c| c.id).collect();
                    self.contacts_state
                        .invite_cube_selections
                        .retain(|id| valid_ids.contains(id));
                    self.contacts_state.invite_available_cubes = Some(cubes);
                }
            }

            ContactsMessage::ToggleInviteCube(cube_id) => {
                let selections = &mut self.contacts_state.invite_cube_selections;
                if let Some(pos) = selections.iter().position(|id| *id == cube_id) {
                    selections.remove(pos);
                } else {
                    selections.push(cube_id);
                }
                // Clear the "cube unavailable" dialog on any edit so a
                // stale message doesn't linger after the user adjusts
                // their picks.
                self.contacts_state.invite_cube_error = None;
            }

            ContactsMessage::BackToList => {
                self.contacts_state.step = ContactsStep::List;
                self.contacts_state.error = None;
                self.contacts_state.invite_cube_selections.clear();
                self.contacts_state.invite_cube_error = None;
            }

            ContactsMessage::ShowDetail(contact_id) => {
                self.contacts_state.step = ContactsStep::Detail(contact_id);
                self.contacts_state.detail_cubes = None;
                self.contacts_state.detail_cubes_error = None;
                self.contacts_state.error = None;
                let client = self.client.clone();
                let gen = self.session_generation;
                return iced::Task::perform(
                    async move { client.get_cubes_by_contact(contact_id).await },
                    move |res| match res {
                        Ok(cubes) => Message::View(view::Message::ConnectAccount(
                            ConnectAccountMessage::Contacts(ContactsMessage::ContactCubesLoaded(
                                contact_id, cubes, gen,
                            )),
                        )),
                        Err(e) => Message::View(view::Message::ConnectAccount(
                            ConnectAccountMessage::Contacts(ContactsMessage::ContactCubesFailed(
                                contact_id,
                                e.to_string(),
                            )),
                        )),
                    },
                );
            }

            ContactsMessage::InviteEmailChanged(email) => {
                self.contacts_state.invite_email = email;
                self.contacts_state.error = None;
            }

            ContactsMessage::InviteRoleChanged(role) => {
                self.contacts_state.invite_role = role;
            }

            ContactsMessage::SubmitInvite => {
                if self.contacts_state.invite_sending {
                    return iced::Task::none();
                }
                let email = self.contacts_state.invite_email.trim().to_string();
                let valid = email_address::EmailAddress::parse_with_options(
                    &email,
                    email_address::Options::default().with_required_tld(),
                )
                .is_ok();
                if !valid {
                    self.contacts_state.error = Some("Please enter a valid email address".into());
                    return iced::Task::none();
                }
                self.contacts_state.invite_sending = true;
                self.contacts_state.error = None;
                self.contacts_state.invite_cube_error = None;
                let client = self.client.clone();
                let role = self.contacts_state.invite_role;
                let cube_ids = self.contacts_state.invite_cube_selections.clone();
                let had_cubes = !cube_ids.is_empty();
                return iced::Task::perform(
                    async move {
                        client
                            .create_invite(CreateInviteRequest {
                                email,
                                role,
                                cube_ids,
                            })
                            .await
                    },
                    move |res| match res {
                        Ok(()) => Message::View(view::Message::ConnectAccount(
                            ConnectAccountMessage::Contacts(ContactsMessage::InviteCreated),
                        )),
                        // A 403 when cubes were attached almost always means
                        // the caller is no longer a member of one of them
                        // (the per-cube membership check failed). Route to
                        // the dedicated handler so the form stays open with
                        // a clear "some cubes are no longer available" msg.
                        Err(e)
                            if had_cubes
                                && matches!(
                                    &e,
                                    crate::services::coincube::CoincubeError::Unsuccessful(info)
                                        if info.status_code == 403
                                ) =>
                        {
                            Message::View(view::Message::ConnectAccount(
                                ConnectAccountMessage::Contacts(
                                    ContactsMessage::InviteCubeForbidden(e.to_string()),
                                ),
                            ))
                        }
                        Err(e) => Message::View(view::Message::ConnectAccount(
                            ConnectAccountMessage::Contacts(ContactsMessage::Error(e.to_string())),
                        )),
                    },
                );
            }

            ContactsMessage::InviteCubeForbidden(msg) => {
                self.contacts_state.invite_sending = false;
                self.contacts_state.invite_cube_error = Some(msg);
                // Refresh the cube list so the checkboxes reflect the
                // user's current memberships. We stay on the form so
                // the user can adjust and retry.
                let client = self.client.clone();
                let gen = self.session_generation;
                return iced::Task::perform(async move { client.list_cubes().await }, move |res| {
                    let options = match res {
                        Ok(cubes) => cubes
                            .into_iter()
                            .map(|c| InviteCubeOption {
                                id: c.id,
                                name: c.name,
                                network: c.network,
                            })
                            .collect(),
                        Err(_) => Vec::new(),
                    };
                    Message::View(view::Message::ConnectAccount(
                        ConnectAccountMessage::Contacts(ContactsMessage::InviteCubesAvailable(
                            options, gen,
                        )),
                    ))
                });
            }

            ContactsMessage::InviteCreated => {
                self.contacts_state.invite_sending = false;
                return self.reload_contacts();
            }

            ContactsMessage::ResendInvite(invite_id) => {
                let client = self.client.clone();
                return iced::Task::perform(
                    async move { client.resend_invite(invite_id).await },
                    move |res| match res {
                        Ok(()) => Message::View(view::Message::ConnectAccount(
                            ConnectAccountMessage::Contacts(ContactsMessage::InviteResent(
                                invite_id,
                            )),
                        )),
                        Err(e) => Message::View(view::Message::ConnectAccount(
                            ConnectAccountMessage::Contacts(ContactsMessage::Error(e.to_string())),
                        )),
                    },
                );
            }

            ContactsMessage::InviteResent(_invite_id) => {
                log::info!("[CONTACTS] Invite resent successfully");
            }

            ContactsMessage::RevokeInvite(invite_id) => {
                let client = self.client.clone();
                return iced::Task::perform(
                    async move { client.revoke_invite(invite_id).await },
                    move |res| match res {
                        Ok(()) => Message::View(view::Message::ConnectAccount(
                            ConnectAccountMessage::Contacts(ContactsMessage::InviteRevoked(
                                invite_id,
                            )),
                        )),
                        Err(e) => Message::View(view::Message::ConnectAccount(
                            ConnectAccountMessage::Contacts(ContactsMessage::Error(e.to_string())),
                        )),
                    },
                );
            }

            ContactsMessage::InviteRevoked(invite_id) => {
                if let Some(ref mut invites) = self.contacts_state.invites {
                    invites.retain(|i| i.id != invite_id);
                }
            }

            ContactsMessage::ContactCubesLoaded(contact_id, cubes, gen) => {
                // Only store if session is current and we're still viewing this contact
                if gen == self.session_generation
                    && matches!(self.contacts_state.step, ContactsStep::Detail(id) if id == contact_id)
                {
                    self.contacts_state.detail_cubes = Some(cubes);
                }
            }

            ContactsMessage::ContactCubesFailed(contact_id, e) => {
                if matches!(self.contacts_state.step, ContactsStep::Detail(id) if id == contact_id)
                {
                    log::error!("[CONTACTS] Cubes fetch failed: {}", e);
                    self.contacts_state.detail_cubes_error = Some(e);
                }
            }

            ContactsMessage::Error(e) => {
                log::error!("[CONTACTS] Error: {}", e);
                // Determine which operation failed based on current state,
                // and only reset the relevant flag.
                if self.contacts_state.invite_sending {
                    // Error from SubmitInvite
                    self.contacts_state.invite_sending = false;
                    self.contacts_state.error = Some(e);
                } else if self.contacts_state.loading {
                    // Error from initial load (contacts/invites fetch)
                    self.contacts_state.loading = false;
                    // Don't display load errors — the empty state is shown instead
                } else {
                    // Error from resend/revoke/cubes fetch — display inline
                    self.contacts_state.error = Some(e);
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

/// Load Contacts tab data (contacts + invites).
pub fn load_contacts_data(client: &CoincubeClient, generation: u64) -> iced::Task<Message> {
    let c1 = client.clone();
    let c2 = client.clone();
    iced::Task::batch([
        iced::Task::perform(
            async move { c1.get_contacts().await },
            move |res| match res {
                Ok(contacts) => Message::View(view::Message::ConnectAccount(
                    ConnectAccountMessage::Contacts(ContactsMessage::ContactsLoaded(
                        contacts, generation,
                    )),
                )),
                Err(e) => Message::View(view::Message::ConnectAccount(
                    ConnectAccountMessage::Contacts(ContactsMessage::Error(e.to_string())),
                )),
            },
        ),
        iced::Task::perform(
            async move { c2.get_invites().await },
            move |res| match res {
                Ok(invites) => Message::View(view::Message::ConnectAccount(
                    ConnectAccountMessage::Contacts(ContactsMessage::InvitesLoaded(
                        invites, generation,
                    )),
                )),
                Err(e) => Message::View(view::Message::ConnectAccount(
                    ConnectAccountMessage::Contacts(ContactsMessage::Error(e.to_string())),
                )),
            },
        ),
    ])
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

#[cfg(test)]
mod invite_form_tests {
    //! State-layer tests for the W12 cube multi-select invite form.
    //! Exercises the `ContactsMessage` dispatch path via the public
    //! `update_message` entrypoint; we don't inspect returned `Task`s
    //! (those are opaque futures) — only the resulting `ContactsState`.
    use super::*;

    fn option(id: u64, name: &str) -> InviteCubeOption {
        InviteCubeOption {
            id,
            name: name.to_string(),
            network: "bitcoin".to_string(),
        }
    }

    fn dispatch(panel: &mut ConnectAccountPanel, msg: ContactsMessage) {
        let _ = panel.update_message(ConnectAccountMessage::Contacts(msg));
    }

    #[test]
    fn available_cubes_sets_state_and_becomes_renderable() {
        let mut panel = ConnectAccountPanel::new();
        let gen = panel.session_generation();
        dispatch(
            &mut panel,
            ContactsMessage::InviteCubesAvailable(
                vec![option(1, "Alpha"), option(7, "Bravo")],
                gen,
            ),
        );
        let cubes = panel
            .contacts_state
            .invite_available_cubes
            .as_ref()
            .expect("available cubes should be Some");
        assert_eq!(cubes.len(), 2);
        assert_eq!(cubes[0].name, "Alpha");
    }

    #[test]
    fn available_cubes_empty_hides_section() {
        let mut panel = ConnectAccountPanel::new();
        let gen = panel.session_generation();
        dispatch(
            &mut panel,
            ContactsMessage::InviteCubesAvailable(Vec::new(), gen),
        );
        // `Some(empty)` means "loaded, but nothing to show" — the view
        // layer's `if !cubes.is_empty()` guard hides the section.
        assert!(matches!(
            panel.contacts_state.invite_available_cubes,
            Some(ref v) if v.is_empty()
        ));
    }

    #[test]
    fn stale_available_cubes_ignored() {
        let mut panel = ConnectAccountPanel::new();
        let stale_gen = panel.session_generation().wrapping_add(1);
        dispatch(
            &mut panel,
            ContactsMessage::InviteCubesAvailable(vec![option(1, "Alpha")], stale_gen),
        );
        assert!(panel.contacts_state.invite_available_cubes.is_none());
    }

    #[test]
    fn toggle_cube_adds_and_removes_from_selection() {
        let mut panel = ConnectAccountPanel::new();
        dispatch(&mut panel, ContactsMessage::ToggleInviteCube(7));
        assert_eq!(panel.contacts_state.invite_cube_selections, vec![7]);

        dispatch(&mut panel, ContactsMessage::ToggleInviteCube(3));
        assert_eq!(panel.contacts_state.invite_cube_selections, vec![7, 3]);

        dispatch(&mut panel, ContactsMessage::ToggleInviteCube(7));
        assert_eq!(panel.contacts_state.invite_cube_selections, vec![3]);
    }

    #[test]
    fn toggle_cube_clears_stale_conflict_banner() {
        let mut panel = ConnectAccountPanel::new();
        panel.contacts_state.invite_cube_error = Some("old".to_string());
        dispatch(&mut panel, ContactsMessage::ToggleInviteCube(7));
        assert!(panel.contacts_state.invite_cube_error.is_none());
    }

    #[test]
    fn invite_cube_forbidden_clears_sending_and_stores_message() {
        let mut panel = ConnectAccountPanel::new();
        panel.contacts_state.invite_sending = true;
        dispatch(
            &mut panel,
            ContactsMessage::InviteCubeForbidden("Cube 7 unavailable".to_string()),
        );
        assert!(!panel.contacts_state.invite_sending);
        assert_eq!(
            panel.contacts_state.invite_cube_error.as_deref(),
            Some("Cube 7 unavailable"),
        );
    }

    #[test]
    fn reload_prunes_stale_selections() {
        // User selected cubes [1, 7, 42], then the cube list reloads
        // without 7 (user lost access). Selection should drop 7 but
        // keep the others.
        let mut panel = ConnectAccountPanel::new();
        panel.contacts_state.invite_cube_selections = vec![1, 7, 42];
        let gen = panel.session_generation();
        dispatch(
            &mut panel,
            ContactsMessage::InviteCubesAvailable(
                vec![option(1, "Alpha"), option(42, "Gamma")],
                gen,
            ),
        );
        assert_eq!(panel.contacts_state.invite_cube_selections, vec![1, 42]);
    }
}
