use iced::widget::qr_code;

use crate::{
    app::{
        menu::ConnectSubMenu,
        message::Message,
        view::{self, ConnectAccountMessage, ContactsMessage, DuressMessage},
    },
    services::coincube::{
        BillingCycle, BillingHistoryEntry, ChargeStatus, CheckoutRequest, CheckoutResponse,
        CoincubeClient, ConnectPlan, Contact, ContactCube, ContactRole, CreateInviteRequest,
        FeaturesResponse, Invite, LoginActivity, LoginResponse, OtpRequest, OtpVerifyRequest,
        PlanStatus, PlanTier, ReceivedInvite, User, VerifiedDevice,
    },
};

use super::{
    delete_connect_secret, read_connect_secret, write_connect_secret, CONNECT_KEYRING_USER,
};

/// Stored session for per-cube auto-connect
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct StoredSession {
    #[serde(flatten)]
    login: LoginResponse,
}

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

// ── Plan lifecycle (renewal banner / expired state) ─────────────────────────

/// Number of days before `renewal_at` at which the pre-expiry renewal
/// banner begins showing (PLAN-billing-desktop D1).
pub const PLAN_RENEWAL_BANNER_DAYS: i64 = 7;

/// Highest pricing-schema version this build can fully render. A
/// `/connect/features` payload advertising a higher version triggers the
/// soft "update available" note in the plan picker (D4).
pub const SUPPORTED_PRICING_SCHEMA_VERSION: u32 = 1;

/// At launch the July-4 Estate promo suppresses the pre-expiry renewal
/// banner for promo accounts (PLAN-estate-promo PR1) — the API likewise
/// suppresses reminder emails, and the year-one cliff (Jul 2027) is too far
/// out to nag about now. Flip to `false` for year-two GA to re-enable the
/// banner with year-two renewal copy.
pub const PROMO_SUPPRESS_RENEWAL_BANNER: bool = true;

/// Where the user's plan sits in its lifecycle, derived from the
/// `/connect/plan` response plus the current time. Drives the renewal
/// banner (D1) and the expired-state UX (D3); kept as a pure projection
/// so that branching stays unit-testable without a running clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanLifecycle {
    /// Free tier with no lapsed paid plan — the ordinary free experience.
    Free,
    /// Active paid plan, comfortably before its renewal date.
    Active,
    /// Active paid plan within `PLAN_RENEWAL_BANNER_DAYS` of renewal.
    /// `days_remaining` is clamped at 0 (today, or just past but not yet
    /// demoted, reads as "0 days").
    RenewalDue { days_remaining: i64 },
    /// A paid plan that lapsed — the backend demoted it (reported as
    /// `past_due`, typically alongside the Free tier).
    Expired,
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
    /// Pending invites addressed to the authenticated user (inbound).
    /// `None` while the initial fetch is in flight; `Some(vec)` once
    /// loaded (possibly empty). The backend filters server-side to
    /// pending + non-expired
    /// (`coincube-api/services/connect/invite/handlers/invite.go:374-429`),
    /// so the view renders it as-is without a second filter pass.
    pub received_invites: Option<Vec<ReceivedInvite>>,
    /// Invite ids whose Accept request is currently in flight. Prevents
    /// a double-tap from firing two `accept_invite_by_id` calls (the
    /// second would race against the server-side row lock and surface
    /// an opaque error); also drives the per-row "Accepting…" label.
    pub accepting_invite_ids: std::collections::HashSet<u64>,
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
    /// Network of the currently-active Cube, synced from
    /// `ConnectCubePanel` by the parent `ConnectPanel`. Drives the
    /// network filter on the W12 cube multi-select and the W14
    /// "Add to Cube(s)…" dialog. `None` when the user is on a
    /// Connect-only surface with no active cube — in that case callers
    /// fall back to "all cubes" so the form doesn't render empty.
    pub active_network: Option<String>,
    /// Server-side numeric id of the currently-loaded Cube, synced
    /// from `ConnectCubePanel` once the cube has registered with the
    /// backend. Drives the W14 "Add to Current Cube" one-click path:
    /// the action targets this exact cube rather than guessing from a
    /// network-matching list (which breaks when the user has several
    /// cubes on the same network).
    pub active_cube_server_id: Option<u64>,
    // --- W14: add-existing-contact-to-cube dialog ---
    /// Active multi-select dialog for the "Add to Cube(s)…" flow.
    /// `None` when closed. See `AddToCubeDialog`.
    pub add_to_cube_target: Option<AddToCubeDialog>,
    /// Transient status for the W14 one-click "Add to Current Cube"
    /// action (keyed by contact id so concurrent row-clicks don't
    /// overwrite each other). `Ok(())` = in-flight or complete,
    /// `Err(msg)` = last attempt failed.
    pub add_to_current_cube_errors: std::collections::HashMap<u64, String>,
    /// Contact ids whose one-click "Add to Current Cube" task is
    /// currently in flight. Prevents a double-click from firing two
    /// `create_cube_invite` requests (the second would 409, but it
    /// still wastes a round-trip and would briefly flash the duplicate
    /// error in the UI).
    pub add_to_current_cube_pending: std::collections::HashSet<u64>,
}

/// State for the W14 "Add to Cube(s)…" multi-select dialog. Opened via
/// `ContactsMessage::OpenAddToCubeDialog(contact_id)` from the contact
/// detail view; closed via `CloseAddToCubeDialog` or on successful
/// `ConfirmAddToCube`.
#[derive(Debug, Clone)]
pub struct AddToCubeDialog {
    pub contact_id: u64,
    pub contact_email: String,
    /// Candidate cubes: network-filtered, unjoined-only, callable by
    /// the current user. `None` while the candidate fetch is in
    /// flight; `Some(vec)` once loaded (possibly empty).
    pub candidate_cubes: Option<Vec<InviteCubeOption>>,
    pub selections: Vec<u64>,
    pub submitting: bool,
    /// Per-cube errors from the last submit (keyed by cube id).
    /// Populated when some of the parallel `create_cube_invite` calls
    /// fail; the dialog stays open so the user can retry.
    pub failures: std::collections::HashMap<u64, String>,
}

impl ContactsState {
    pub fn new() -> Self {
        Self {
            step: ContactsStep::List,
            contacts: None,
            invites: None,
            received_invites: None,
            accepting_invite_ids: std::collections::HashSet::new(),
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
            active_network: None,
            active_cube_server_id: None,
            add_to_cube_target: None,
            add_to_current_cube_errors: std::collections::HashMap::new(),
            add_to_current_cube_pending: std::collections::HashSet::new(),
        }
    }

    pub fn clear(&mut self) {
        *self = Self::new();
    }

    /// True when the detail view's Associated-Cubes list includes the
    /// currently-active cube, i.e. the contact is already a member.
    /// Used by the contact detail view to hide the "Add to Current
    /// Cube" button when the action would be a no-op.
    ///
    /// Returns `false` when:
    ///   * the user isn't viewing this contact's detail (no
    ///     `detail_cubes` loaded for them),
    ///   * there's no active cube (`active_cube_server_id` is `None`),
    ///   * the `detail_cubes` fetch hasn't completed yet (optimistic —
    ///     better to show the button and let the backend 409 than to
    ///     blink it in once data arrives).
    pub fn contact_is_in_active_cube(&self, contact_id: u64) -> bool {
        let Some(active_id) = self.active_cube_server_id else {
            return false;
        };
        if !matches!(self.step, ContactsStep::Detail(id) if id == contact_id) {
            return false;
        }
        self.detail_cubes
            .as_deref()
            .map(|cubes| cubes.iter().any(|c| c.id == active_id))
            .unwrap_or(false)
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
    /// Post-auth, pre-dashboard gate (Phase 6): block on `get_duress_state` so
    /// the dashboard is never shown to a possibly-in-duress account. `failed`
    /// is set once retries are exhausted, switching the view to an error +
    /// Retry affordance. We fail CLOSED here (no dashboard) rather than open —
    /// the whole Connect dashboard is server-backed, so if this check is
    /// unreachable the dashboard is non-functional anyway.
    CheckingDuress {
        failed: bool,
    },
    Dashboard,
    /// Post-lockout recovery (Phase 6). Shown as the FIRST thing after sign-in
    /// when the account is in duress — there is no normal dashboard until the
    /// account is cleared from a trusted device.
    DuressRecovery {
        /// When the account can be cleared with the all-clear passphrase.
        unlock_at: Option<chrono::DateTime<chrono::Utc>>,
        passphrase: String,
        submitting: bool,
        /// Set once `clear_duress` succeeds — shows the "download your Cube
        /// Recovery Kit" hand-off.
        cleared: bool,
    },
}

/// Which credentials the enrollment wizard collects, derived from the account's
/// duress entitlement (and, for sovereign, the absence of Connect).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnrollTier {
    /// Connect + recovery kit: full flow incl. the account-level duress CRK
    /// password (Approach C).
    Tier1,
    /// Connect, no recovery kit: same as Tier 1 minus the CRK password, with a
    /// BIG warning that recovery depends on the seed-phrase backup.
    Tier2,
    /// No Connect: local-only wipe; the wizard opens with a Connect-
    /// encouragement screen and a type-to-confirm friction step.
    Sovereign,
}

/// Steps of the duress enrollment wizard. Sovereign opens at `Encourage`;
/// Connect tiers skip straight to `SetDuressPin`. `SetCrkPassword` is Tier 1
/// only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuressEnrollStep {
    /// Sovereign Step 0 — Connect-encouragement (primary CTA "Sign up for
    /// Connect", secondary "Continue without Connect").
    Encourage,
    /// Sovereign friction — type "I have my seed-phrase backup".
    SovereignConfirm,
    SetDuressPin,
    SetAllClear,
    SetCrkPassword,
    PickDelay,
    Confirm,
}

/// In-flight enrollment wizard state (Phases 2 & 8). `None` on the panel when
/// the wizard isn't open.
#[derive(Debug)]
pub struct DuressEnrollState {
    pub tier: EnrollTier,
    pub step: DuressEnrollStep,
    /// The user's regular PIN, re-entered so distinctness can be validated.
    pub regular_pin: String,
    pub duress_pin: String,
    pub all_clear: String,
    pub crk_password: String,
    pub delay: crate::services::duress::enroll::DuressDelay,
    pub sovereign_confirm: String,
    pub memorized: bool,
    pub submitting: bool,
    pub error: Option<String>,
    /// This device's generated duress code, held between SubmitEnrollment and a
    /// successful server EnrollResult. For Connect tiers the code is NOT
    /// persisted locally until the server confirms enrollment, so a server
    /// failure can't leave a half-armed duress PIN on disk.
    pub pending_code: Option<String>,
}

impl DuressEnrollState {
    /// Scrubs the in-memory secret fields (PINs, passphrases, and the generated
    /// duress code) so they don't linger on the heap after the wizard is torn
    /// down — e.g. on logout, where the rest of the session is reset.
    fn zeroize_secrets(&mut self) {
        use zeroize::Zeroize;
        self.regular_pin.zeroize();
        self.duress_pin.zeroize();
        self.all_clear.zeroize();
        self.crk_password.zeroize();
        self.sovereign_confirm.zeroize();
        if let Some(code) = self.pending_code.as_mut() {
            code.zeroize();
        }
    }
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
    /// Active duress enrollment wizard (Phases 2 & 8); `None` when closed.
    pub duress_enroll: Option<DuressEnrollState>,
    /// Whether the user dismissed the pre-expiry renewal banner this
    /// session (D1). Not persisted — re-shows on next launch while the
    /// plan is still within its renewal window.
    pub renewal_banner_dismissed: bool,
}

impl ConnectAccountPanel {
    pub fn new() -> Self {
        ConnectAccountPanel {
            step: ConnectFlowStep::CheckingSession,
            active_sub: ConnectSubMenu::Overview,
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
            duress_enroll: None,
            renewal_banner_dismissed: false,
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

    /// Returns `true` if a Connect session is stored in the OS keyring
    /// under the shared global key AND parses as a valid `StoredSession`.
    /// Mirrors `Init`'s restoration check so callers (e.g.
    /// `can_restore_connect_session`) don't treat unparseable bytes as a
    /// restorable session — that would skip the Home-tab handoff while
    /// `Init` silently falls back to the Login step, leaving the user
    /// stuck on an inline prompt with no login form.
    pub fn has_stored_session(&self) -> bool {
        self.load_session_from_keyring().is_some()
    }

    pub fn session_generation(&self) -> u64 {
        self.session_generation
    }

    /// Set the active Cube's network, used by the invite-form and
    /// add-to-cube flows to filter candidate cubes to network-matching
    /// ones. Called by the parent `ConnectPanel` when it wires up or
    /// updates the cube side.
    pub fn set_active_network(&mut self, network: Option<String>) {
        self.contacts_state.active_network = network;
    }

    /// Set the active Cube's server-side numeric id. Populated by the
    /// parent `ConnectPanel` once `ConnectCubePanel::server_cube_id`
    /// resolves (post-registration). Drives the W14 "Add to Current
    /// Cube" one-click action.
    pub fn set_active_cube_server_id(&mut self, cube_id: Option<u64>) {
        self.contacts_state.active_cube_server_id = cube_id;
    }

    /// Kick off a `get_cubes_by_contact(contact_id)` fetch and wire
    /// the result into `ContactCubesLoaded` / `ContactCubesFailed`.
    /// Shared by `ShowDetail` (initial load) and by post-mutation
    /// handlers that need to refresh the Associated Cubes section
    /// after a successful add.
    fn fetch_contact_cubes(&self, contact_id: u64) -> iced::Task<Message> {
        let client = self.client.clone();
        let gen = self.session_generation;
        iced::Task::perform(
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
        )
    }

    /// Reset contacts state to list view and reload data from the API.
    pub fn reload_contacts(&mut self) -> iced::Task<Message> {
        self.contacts_state.step = ContactsStep::List;
        self.contacts_state.contacts = None;
        self.contacts_state.invites = None;
        self.contacts_state.received_invites = None;
        self.contacts_state.error = None;
        self.contacts_state.loading = true;
        load_contacts_data(&self.client, self.session_generation)
    }

    fn load_session_from_keyring(&self) -> Option<StoredSession> {
        let bytes = read_connect_secret(CONNECT_KEYRING_USER)?;
        serde_json::from_slice::<StoredSession>(&bytes).ok()
    }

    fn save_session_to_keyring(&self, session: &StoredSession) {
        let bytes = match serde_json::to_vec(session) {
            Ok(b) => b,
            Err(e) => {
                log::error!("[CONNECT] Failed to serialize session for keyring: {}", e);
                return;
            }
        };
        if let Err(e) = write_connect_secret(CONNECT_KEYRING_USER, &bytes) {
            log::error!("[CONNECT] Failed to save session to keyring: {}", e);
        }
    }

    fn clear_keyring_session(&self) {
        delete_connect_secret(CONNECT_KEYRING_USER);
    }

    fn post_login_tasks(&mut self, session: StoredSession) -> iced::Task<Message> {
        self.save_session_to_keyring(&session);
        self.client.set_token(&session.login.token);

        let user = session.login.user.clone();
        // Fire two messages: the in-panel state transition + the
        // app-level bootstrap that persists JWTs to `connect.json`,
        // registers the signer device via gRPC, and starts the
        // realtime stream. The home login path does this at
        // app-init via `register_signer_device_best_effort`; the
        // in-app login path previously skipped it, which left
        // "Sign via Keychain" unreachable until a full app restart.
        iced::Task::batch([
            iced::Task::done(Message::View(view::Message::ConnectAccount(
                ConnectAccountMessage::SessionLoaded { user, plan: None },
            ))),
            iced::Task::done(Message::InAppConnectLoginCompleted {
                token: session.login.token.clone(),
                refresh_token: session.login.refresh_token.clone(),
                email: session.login.user.email.clone(),
            }),
        ])
    }

    pub fn update_message(&mut self, msg: ConnectAccountMessage) -> iced::Task<Message> {
        match msg {
            ConnectAccountMessage::Init => {
                // Already authenticated (e.g. broadcast Init from a
                // sibling tab that just signed in): nothing to do.
                if matches!(self.step, ConnectFlowStep::Dashboard) {
                    return iced::Task::none();
                }
                // A RefreshSession spawned by a previous Init is still
                // in flight. Firing another now would race two token
                // refreshes against each other and double the downstream
                // post-login bootstrap. Wait for the in-flight one to
                // resolve (success → Dashboard; failure → Login with
                // loading=false, which Init can pick up again).
                if matches!(self.step, ConnectFlowStep::Login { loading: true, .. }) {
                    return iced::Task::none();
                }
                if let Some(session) = self.load_session_from_keyring() {
                    let refresh_token = session.login.refresh_token.clone();
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
                // No session stored - show Login form.
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
                self.scrub_recovery_passphrase();
                self.step = ConnectFlowStep::Login {
                    email: String::new(),
                    loading: false,
                };
            }

            ConnectAccountMessage::SetSession(login) => {
                let session = StoredSession { login };
                return self.post_login_tasks(session);
            }

            ConnectAccountMessage::SessionLoaded { user, plan } => {
                self.session_generation += 1;
                self.user = Some(user);
                self.plan = plan;
                // Phase 6: gate on the duress check FIRST — don't reveal the
                // dashboard until we've confirmed the account isn't in duress.
                self.step = ConnectFlowStep::CheckingDuress { failed: false };
                self.error = None;
                // Fetch plan + features in background (non-blocking)
                let gen = self.session_generation;
                let c1 = self.client.clone();
                let c2 = self.client.clone();
                let c3 = self.client.clone();
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
                    // Phase 6: gate on duress state. If the account is in
                    // duress, the recovery flow replaces the dashboard. A
                    // failed check retries (see the handler) so a transient
                    // error doesn't leave a duress account on the dashboard.
                    duress_state_check_task(c3, gen, 0),
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
                self.renewal_banner_dismissed = false;
                self.selected_billing_cycle = BillingCycle::Monthly;
                self.contacts_state.clear();
                // Scrub any in-flight enrollment wizard secrets (PINs,
                // passphrases, generated code) and the recovery all-clear
                // passphrase before dropping them, so they don't survive the
                // session reset.
                self.clear_duress_enroll();
                self.scrub_recovery_passphrase();
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
                ConnectFlowStep::Login { email: e, loading }
                | ConnectFlowStep::Register { email: e, loading } => {
                    *e = email;
                    *loading = false;
                }
                _ => {}
            },

            ConnectAccountMessage::SubmitLogin => {
                self.error = None;

                let ConnectFlowStep::Login { email, loading } = &mut self.step else {
                    return iced::Task::none();
                };
                *loading = true;
                let email_val = email.clone();
                let client = self.client.clone();
                return iced::Task::perform(
                    async move {
                        let result = client
                            .login_send_otp(OtpRequest {
                                email: email_val.clone(),
                            })
                            .await;
                        (email_val, result)
                    },
                    |(email, res)| match res {
                        Ok(()) => Message::View(view::Message::ConnectAccount(
                            ConnectAccountMessage::OtpRequested {
                                email,
                                is_signup: false,
                            },
                        )),
                        Err(e) => {
                            let is_unverified = matches!(
                                &e,
                                crate::services::coincube::CoincubeError::Unsuccessful(info)
                                    if info.status_code == 401
                                        && info.text.contains("Email not verified")
                            );
                            if is_unverified {
                                Message::View(view::Message::ConnectAccount(
                                    ConnectAccountMessage::EmailNotVerified { email },
                                ))
                            } else {
                                Message::View(view::Message::ConnectAccount(
                                    ConnectAccountMessage::Error(e.to_string()),
                                ))
                            }
                        }
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
                self.error = None;

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
                            client.resend_signup_otp(&email_val).await
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

            ConnectAccountMessage::EmailNotVerified { email } => {
                self.step = ConnectFlowStep::OtpVerification {
                    email: email.clone(),
                    otp: String::new(),
                    sending: true,
                    is_signup: true,
                    cooldown: 0,
                };
                let client = self.client.clone();
                return iced::Task::perform(
                    async move { client.resend_signup_otp(&email).await },
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
                // Defense-in-depth (PLAN-estate-promo PR2): the picker hides
                // upgrade CTAs and the renewal banner is suppressed while
                // purchasing is disabled, but this is the single chokepoint
                // every checkout flows through — never open one the API would
                // reject even if a stale message slips through.
                if !self.purchasing_enabled() {
                    log::warn!("[CONNECT] StartCheckout ignored — purchasing disabled");
                    return iced::Task::none();
                }
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

            ConnectAccountMessage::DismissRenewalBanner => {
                // Per-session only — `renewal_banner_dismissed` resets on
                // logout and isn't persisted, so the banner re-shows on the
                // next launch while the plan is still in-window.
                self.renewal_banner_dismissed = true;
            }

            ConnectAccountMessage::OpenPlanBilling => {
                // The view dispatches the body off `active_sub`, so flipping
                // it here switches the visible sub-view to the picker. Used
                // by the expired-state renew CTA (D3), which opens the
                // picker rather than pre-filling an invoice. Clear the
                // billing-history toggle so the picker actually renders —
                // `plan_billing_ux` shows history ahead of the picker, and a
                // session-stale `show_billing_history` would otherwise route
                // "View plans" to the history list instead.
                self.active_sub = ConnectSubMenu::PlanBilling;
                self.show_billing_history = false;
            }

            ConnectAccountMessage::RenewCurrentPlan => {
                // D1 banner CTA: jump to Plan & Billing and open checkout
                // pre-selected to the user's current tier + cycle.
                let Some(plan) = self.plan.as_ref() else {
                    return iced::Task::none();
                };
                let tier = plan.plan.clone();
                let cycle = plan.billing_cycle;
                self.active_sub = ConnectSubMenu::PlanBilling;
                // Same routing concern as OpenPlanBilling: a stale history
                // toggle would mask the picker on the Free/lapsed fallback
                // path below (where no checkout is started).
                self.show_billing_history = false;
                // A Free/lapsed plan has no tier to pre-fill an invoice for
                // — fall back to the picker.
                if matches!(tier, PlanTier::Free) {
                    return iced::Task::none();
                }
                if let Some(cycle) = cycle {
                    self.selected_billing_cycle = cycle;
                }
                // Reuse the existing StartCheckout path so the invoice
                // creation + polling stay in one place.
                return iced::Task::done(Message::View(view::Message::ConnectAccount(
                    ConnectAccountMessage::StartCheckout(tier),
                )));
            }

            ConnectAccountMessage::ToggleBillingHistory => {
                self.show_billing_history = !self.show_billing_history;
                if self.show_billing_history {
                    let gen = self.session_generation;
                    let c1 = self.client.clone();
                    let c2 = self.client.clone();
                    return iced::Task::batch([
                        iced::Task::perform(
                            async move { c1.get_billing_history().await },
                            move |res| match res {
                                Ok(history) => Message::View(view::Message::ConnectAccount(
                                    ConnectAccountMessage::BillingHistoryLoaded(Ok(history), gen),
                                )),
                                Err(e) => Message::View(view::Message::ConnectAccount(
                                    ConnectAccountMessage::BillingHistoryLoaded(
                                        Err(e.to_string()),
                                        gen,
                                    ),
                                )),
                            },
                        ),
                        iced::Task::perform(
                            async move {
                                match c2.get_user().await {
                                    Ok(u) => Message::View(view::Message::ConnectAccount(
                                        ConnectAccountMessage::UserProfileLoaded(u),
                                    )),
                                    Err(e) => Message::View(view::Message::ConnectAccount(
                                        ConnectAccountMessage::UserProfileFailed(e.to_string()),
                                    )),
                                }
                            },
                            |m| m,
                        ),
                    ]);
                }
            }
            ConnectAccountMessage::Contacts(contacts_msg) => {
                return self.update_contacts(contacts_msg);
            }
            ConnectAccountMessage::Error(error_msg) => {
                self.error = Some(error_msg);

                match &mut self.step {
                    ConnectFlowStep::Login { loading, .. }
                    | ConnectFlowStep::Register { loading, .. } => {
                        *loading = false;
                    }

                    ConnectFlowStep::OtpVerification { sending, .. } => {
                        *sending = false;
                    }

                    _ => {}
                }
            }
            ConnectAccountMessage::BillingHistoryLoaded(result, gen) => {
                if gen == self.session_generation {
                    match result {
                        Ok(history) => {
                            self.billing_history = Some(history);
                            self.error = None;
                        }
                        Err(e) => {
                            self.error = Some(e);
                        }
                    }
                }
            }
            ConnectAccountMessage::UserProfileLoaded(user) => {
                // Non-auth profile refresh - only update user, no step/session changes
                self.user = Some(user);
            }
            ConnectAccountMessage::UserProfileFailed(error) => {
                // Non-auth error - just show error, don't redirect to login
                self.error = Some(error);
            }
            ConnectAccountMessage::DuressStateChecked(state, gen, attempt) => {
                // Phase 6: post-sign-in gate. We sit in `CheckingDuress` until
                // this resolves, so the dashboard is never shown to a possibly-
                // in-duress account.
                if gen != self.session_generation {
                    return iced::Task::none();
                }
                match state {
                    Some(s) if s.active => {
                        // This signed-in device is the trusted recovery vehicle:
                        // show the all-clear entry. Do NOT persist
                        // DuressLocalState.active here — the launch reconcile
                        // keys off that flag and would route this device into the
                        // cryptic dead-end (which has no all-clear entry) on the
                        // next restart, making recovery impossible. The flag is
                        // owned by the paths that actually lock this device: the
                        // local duress PIN and the in-Cube gRPC remote handler.
                        self.step = ConnectFlowStep::DuressRecovery {
                            unlock_at: s.unlock_at,
                            passphrase: String::new(),
                            submitting: false,
                            cleared: false,
                        };
                    }
                    // Confirmed not in duress — NOW reveal the dashboard.
                    Some(s) => {
                        self.step = ConnectFlowStep::Dashboard;
                        // Phase 0: a signed-in device on an already-enrolled
                        // account that isn't yet registered server-side mints +
                        // registers its OWN duress code (per-device, never
                        // shared), so it can fire trigger-with-code on its own
                        // activation. Fire-and-forget; idempotent (skips if this
                        // device already holds a code).
                        if s.enrolled && !s.this_device_registered {
                            if let Some(account_id) = self.user.as_ref().map(|u| u.id.to_string()) {
                                return register_device_duress_task(
                                    self.client.clone(),
                                    account_id,
                                );
                            }
                        }
                    }
                    // Failed / unreachable check — retry with backoff.
                    None if attempt + 1 < DURESS_CHECK_MAX_ATTEMPTS => {
                        return duress_state_check_task(self.client.clone(), gen, attempt + 1);
                    }
                    // Retries exhausted. Fail CLOSED: do not reveal the
                    // dashboard. Show an error + Retry on the checking screen.
                    // (The dashboard is server-backed anyway, so it would be
                    // non-functional during this outage.)
                    None => {
                        log::warn!(
                            "[CONNECT] duress state check failed after {} attempts; \
                             holding at the verification gate",
                            attempt + 1
                        );
                        self.step = ConnectFlowStep::CheckingDuress { failed: true };
                    }
                }
            }
            ConnectAccountMessage::RetryDuressCheck => {
                // From the CheckingDuress error screen.
                self.step = ConnectFlowStep::CheckingDuress { failed: false };
                let gen = self.session_generation;
                return duress_state_check_task(self.client.clone(), gen, 0);
            }
            ConnectAccountMessage::DuressDeviceRegistered => {
                // Side-effect-only task completed (Phase 0 device-code register,
                // or syncing DuressLocalState.active to the server's duress
                // state — set on the post-sign-in mirror, reset on recovery).
            }
            ConnectAccountMessage::Duress(m) => return self.update_duress(m),
        }

        iced::Task::none()
    }

    /// Opens the duress enrollment wizard at `step` for `tier`, resetting all
    /// inputs. Shared by the Tier 1 / Tier 2 / Sovereign entry points.
    fn open_enroll_wizard(&mut self, tier: EnrollTier, step: DuressEnrollStep) {
        // Scrub any wizard already in flight before replacing it, so re-opening
        // never drops its secrets unzeroized.
        self.clear_duress_enroll();
        self.duress_enroll = Some(DuressEnrollState {
            tier,
            step,
            regular_pin: String::new(),
            duress_pin: String::new(),
            all_clear: String::new(),
            crk_password: String::new(),
            delay: crate::services::duress::enroll::DuressDelay::default(),
            sovereign_confirm: String::new(),
            memorized: false,
            submitting: false,
            error: None,
            pending_code: None,
        });
    }

    /// Zeroizes the in-memory all-clear passphrase when the panel is in the
    /// duress recovery flow. Call before replacing `self.step` so the secret
    /// (it clears duress) doesn't linger on the heap after the user leaves
    /// recovery. No-op when not in recovery.
    fn scrub_recovery_passphrase(&mut self) {
        if let ConnectFlowStep::DuressRecovery { passphrase, .. } = &mut self.step {
            zeroize::Zeroize::zeroize(passphrase);
        }
    }

    /// The single teardown path for the enrollment wizard: zeroize its secrets
    /// (PINs, passphrases, pending code) before dropping it, so no branch that
    /// cancels, completes, or replaces `duress_enroll` leaves them on the heap.
    /// Completion paths clone the few values they forward into a `Zeroizing`
    /// payload first, then call this to scrub the originals.
    fn clear_duress_enroll(&mut self) {
        if let Some(mut wizard) = self.duress_enroll.take() {
            wizard.zeroize_secrets();
        }
    }

    /// Recovery flow (Phase 6) + enrollment wizard (Phases 2 & 8) dispatcher.
    fn update_duress(&mut self, msg: DuressMessage) -> iced::Task<Message> {
        use crate::services::duress::enroll;
        match msg {
            // ── Recovery (Phase 6) ──
            DuressMessage::RecoveryPassphraseChanged(text) => {
                if let ConnectFlowStep::DuressRecovery { passphrase, .. } = &mut self.step {
                    *passphrase = text;
                }
            }
            DuressMessage::SubmitClear => {
                let ConnectFlowStep::DuressRecovery {
                    passphrase,
                    submitting,
                    ..
                } = &mut self.step
                else {
                    return iced::Task::none();
                };
                let hash = match enroll::hash_duress_secret(passphrase) {
                    Ok(h) => h,
                    Err(e) => {
                        self.error = Some(e);
                        return iced::Task::none();
                    }
                };
                *submitting = true;
                let client = self.client.clone();
                let gen = self.session_generation;
                return iced::Task::perform(
                    async move { client.clear_duress(&hash).await },
                    move |res| {
                        Message::View(view::Message::ConnectAccount(
                            ConnectAccountMessage::Duress(DuressMessage::ClearResult(
                                res.map_err(|e| e.to_string()),
                                gen,
                            )),
                        ))
                    },
                );
            }
            DuressMessage::ClearResult(res, gen) => {
                if gen != self.session_generation {
                    return iced::Task::none();
                }
                let mut cleared_ok = false;
                if let ConnectFlowStep::DuressRecovery {
                    submitting,
                    cleared,
                    ..
                } = &mut self.step
                {
                    *submitting = false;
                    match res {
                        Ok(()) => {
                            *cleared = true;
                            cleared_ok = true;
                        }
                        Err(e) => self.error = Some(e),
                    }
                }
                if cleared_ok {
                    // The server confirmed the all-clear. The post-sign-in mirror
                    // may have set DuressLocalState.active = true on this device,
                    // so reset it durably — otherwise the next launch reconcile
                    // routes back into the cryptic screen for an account that's
                    // already been cleared. Mirrors the gRPC DuressCleared and
                    // cryptic-screen poll resets.
                    return iced::Task::perform(
                        async move {
                            if let Ok(dir) = crate::dir::CoincubeDirectory::active() {
                                let root = dir.path();
                                match crate::services::duress::DuressLocalState::load(root) {
                                    Ok(mut st) if st.active => {
                                        st.active = false;
                                        st.unlock_at = None;
                                        if let Err(e) = st.save(root) {
                                            log::warn!(
                                                "[CONNECT] failed to clear local duress \
                                                 active state: {e}"
                                            );
                                        }
                                    }
                                    Ok(_) => {}
                                    Err(e) => log::warn!(
                                        "[CONNECT] reading duress state failed; \
                                         not overwriting: {e}"
                                    ),
                                }
                            }
                        },
                        |_| {
                            Message::View(view::Message::ConnectAccount(
                                ConnectAccountMessage::DuressDeviceRegistered,
                            ))
                        },
                    );
                }
            }
            DuressMessage::ForgotAllClear => {
                return iced::Task::done(Message::View(view::Message::OpenUrl(
                    "https://coincube.io/support/duress-recovery".to_string(),
                )));
            }
            DuressMessage::FinishRecovery => {
                self.scrub_recovery_passphrase();
                self.step = ConnectFlowStep::Dashboard;
                self.error = None;
            }

            // ── Enrollment wizard (Phases 2 & 8) ──
            DuressMessage::StartEnrollment => {
                let entitled = self
                    .plan
                    .as_ref()
                    .map(|p| p.entitlements.duress_remote_lock)
                    .unwrap_or(false);
                // Entitled (signed-in) users default to Tier 1, which collects
                // the account-level duress recovery-kit password — that password
                // covers current AND future Cubes, so it's safe to collect even
                // before a CRK exists. A user who explicitly has no recovery kit
                // takes the Tier 2 path via StartEnrollmentWithoutCrk. Non-
                // Connect users get the sovereign encouragement flow.
                if entitled {
                    self.open_enroll_wizard(EnrollTier::Tier1, DuressEnrollStep::SetDuressPin);
                } else {
                    self.open_enroll_wizard(EnrollTier::Sovereign, DuressEnrollStep::Encourage);
                }
            }
            DuressMessage::StartEnrollmentWithoutCrk => {
                // Tier 2 — Connect, no recovery kit: same as Tier 1 minus the
                // CRK-password step (see `enroll_steps`). The BIG "set up a
                // recovery kit first" warning is shown on the eligibility gate.
                self.open_enroll_wizard(EnrollTier::Tier2, DuressEnrollStep::SetDuressPin);
            }
            DuressMessage::SignUpForConnect => {
                self.clear_duress_enroll();
                self.step = ConnectFlowStep::Register {
                    email: String::new(),
                    loading: false,
                };
            }
            DuressMessage::CancelEnrollment => {
                self.clear_duress_enroll();
            }
            DuressMessage::RegularPinChanged(v) => {
                if let Some(e) = &mut self.duress_enroll {
                    e.regular_pin = v;
                }
            }
            DuressMessage::DuressPinChanged(v) => {
                if let Some(e) = &mut self.duress_enroll {
                    e.duress_pin = v;
                }
            }
            DuressMessage::AllClearChanged(v) => {
                if let Some(e) = &mut self.duress_enroll {
                    e.all_clear = v;
                }
            }
            DuressMessage::CrkPasswordChanged(v) => {
                if let Some(e) = &mut self.duress_enroll {
                    e.crk_password = v;
                }
            }
            DuressMessage::DelaySelected(d) => {
                if let Some(e) = &mut self.duress_enroll {
                    e.delay = d;
                }
            }
            DuressMessage::SovereignConfirmChanged(v) => {
                if let Some(e) = &mut self.duress_enroll {
                    e.sovereign_confirm = v;
                }
            }
            DuressMessage::MemorizedToggled(v) => {
                if let Some(e) = &mut self.duress_enroll {
                    e.memorized = v;
                }
            }
            DuressMessage::EnrollBack => {
                if let Some(e) = &mut self.duress_enroll {
                    e.error = None;
                    e.step = prev_enroll_step(e.tier, e.step);
                }
            }
            DuressMessage::EnrollNext => {
                if let Some(e) = &mut self.duress_enroll {
                    if let Err(msg) = validate_enroll_step(e) {
                        e.error = Some(msg);
                    } else {
                        e.error = None;
                        e.step = next_enroll_step(e.tier, e.step);
                    }
                }
            }
            DuressMessage::SubmitEnrollment => {
                let Some(e) = &mut self.duress_enroll else {
                    return iced::Task::none();
                };
                if let Err(msg) = validate_enroll_step(e) {
                    e.error = Some(msg);
                    return iced::Task::none();
                }
                e.submitting = true;
                e.error = None;
                let tier = e.tier;
                let gen = self.session_generation;

                // Generate this device's duress code ONCE: its hash goes to the
                // server, the same plaintext is persisted (encrypted) locally.
                let code = enroll::generate_duress_code();

                if tier == EnrollTier::Sovereign {
                    // No Connect call — local wipe + cryptic only. Persist now.
                    let regular_pin = e.regular_pin.clone();
                    let duress_pin = e.duress_pin.clone();
                    self.clear_duress_enroll();
                    return iced::Task::done(Message::CompleteDuressEnrollment(
                        crate::app::message::DuressEnrollmentPayload {
                            regular_pin: zeroize::Zeroizing::new(regular_pin),
                            duress_pin: zeroize::Zeroizing::new(duress_pin),
                            duress_code: zeroize::Zeroizing::new(code),
                            account_id: None,
                            gen,
                        },
                    ));
                }

                // Connect tiers: enroll on the server FIRST. The duress PIN +
                // code are persisted locally only after a successful
                // EnrollResult, so a server failure can't leave a half-armed
                // duress PIN on disk. Stash the code for that success handler.
                // Zeroize any code stashed by a prior submit (retry) before
                // replacing it, so the superseded plaintext doesn't linger.
                if let Some(mut old) = e.pending_code.replace(code.clone()) {
                    zeroize::Zeroize::zeroize(&mut old);
                }
                let all_clear_hash = enroll::hash_duress_secret(&e.all_clear);
                let crk_hash = if tier == EnrollTier::Tier1 {
                    Some(enroll::hash_duress_secret(&e.crk_password))
                } else {
                    None
                };
                let code_hash = enroll::hash_duress_secret(&code);
                let delay_minutes = e.delay.minutes();

                let (all_clear_hash, code_hash) = match (all_clear_hash, code_hash) {
                    (Ok(a), Ok(c)) => (a, c),
                    _ => {
                        e.error = Some("Failed to hash credentials.".to_string());
                        e.submitting = false;
                        return iced::Task::none();
                    }
                };
                let crk_hash = match crk_hash {
                    Some(Ok(h)) => Some(h),
                    Some(Err(_)) => {
                        e.error = Some("Failed to hash credentials.".to_string());
                        e.submitting = false;
                        return iced::Task::none();
                    }
                    None => None,
                };
                // A stable device fingerprint is required so the server can
                // recognise this desktop. If it can't be resolved, fail the
                // enrollment rather than send an unstable one.
                let fingerprint = match device_fingerprint() {
                    Ok(fp) => fp,
                    Err(msg) => {
                        e.error = Some(msg);
                        e.submitting = false;
                        return iced::Task::none();
                    }
                };
                let client = self.client.clone();
                let req = crate::services::coincube::EnrollDuressRequest {
                    all_clear_hash,
                    duress_crk_password_hash: crk_hash,
                    unlock_delay_minutes: delay_minutes,
                    device_fingerprint: fingerprint,
                    duress_code_hash: code_hash,
                };
                return iced::Task::perform(
                    async move { client.enroll_duress(req).await },
                    move |res| {
                        Message::View(view::Message::ConnectAccount(
                            ConnectAccountMessage::Duress(DuressMessage::EnrollResult(
                                res.map_err(|e| e.to_string()),
                                gen,
                            )),
                        ))
                    },
                );
            }
            DuressMessage::EnrollResult(res, gen) => {
                if gen != self.session_generation {
                    return iced::Task::none();
                }
                match res {
                    Ok(()) => {
                        // Server enrolled — NOW persist locally (PIN hash +
                        // encrypted code) from the wizard state, then close it.
                        // Doing this only on success means a server failure
                        // never leaves a half-armed duress PIN on disk.
                        let account_id = self.user.as_ref().map(|u| u.id.to_string());
                        // Clone the forwarded secrets into the Zeroizing payload,
                        // then scrub the wizard via the shared helper — this path
                        // must not drop the leftover fields (all_clear, CRK
                        // password, sovereign_confirm) unzeroized.
                        let payload = self.duress_enroll.as_ref().map(|e| {
                            crate::app::message::DuressEnrollmentPayload {
                                regular_pin: zeroize::Zeroizing::new(e.regular_pin.clone()),
                                duress_pin: zeroize::Zeroizing::new(e.duress_pin.clone()),
                                duress_code: zeroize::Zeroizing::new(
                                    e.pending_code.clone().unwrap_or_default(),
                                ),
                                account_id,
                                gen,
                            }
                        });
                        if let Some(payload) = payload {
                            self.clear_duress_enroll();
                            return iced::Task::done(Message::CompleteDuressEnrollment(payload));
                        }
                    }
                    Err(msg) => {
                        // No local state was written — just surface the error
                        // and keep the wizard open for a retry.
                        if let Some(e) = &mut self.duress_enroll {
                            e.submitting = false;
                            e.error = Some(msg);
                        }
                    }
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
                    // Clear loading only when all three sibling fetches
                    // (contacts, sent invites, received invites) have
                    // landed; otherwise the view flips from spinner to
                    // empty-state before the late arrival paints.
                    if self.contacts_state.invites.is_some()
                        && self.contacts_state.received_invites.is_some()
                    {
                        self.contacts_state.loading = false;
                    }
                }
            }

            ContactsMessage::InvitesLoaded(invites, gen) => {
                if gen == self.session_generation {
                    self.contacts_state.invites = Some(invites);
                    if self.contacts_state.contacts.is_some()
                        && self.contacts_state.received_invites.is_some()
                    {
                        self.contacts_state.loading = false;
                    }
                }
            }

            ContactsMessage::ReceivedInvitesLoaded(received, gen) => {
                if gen == self.session_generation {
                    self.contacts_state.received_invites = Some(received);
                    if self.contacts_state.contacts.is_some()
                        && self.contacts_state.invites.is_some()
                    {
                        self.contacts_state.loading = false;
                    }
                }
            }

            ContactsMessage::AcceptReceivedInvite(invite_id) => {
                // Guard against double-tap. The button is disabled
                // while the id is in-flight, but the message can still
                // arrive twice via keyboard activation; we drop the
                // second silently.
                if !self.contacts_state.accepting_invite_ids.insert(invite_id) {
                    return iced::Task::none();
                }
                self.contacts_state.error = None;
                let client = self.client.clone();
                return iced::Task::perform(
                    async move { client.accept_invite_by_id(invite_id).await },
                    move |res| match res {
                        Ok(()) => Message::View(view::Message::ConnectAccount(
                            ConnectAccountMessage::Contacts(
                                ContactsMessage::ReceivedInviteAccepted(invite_id),
                            ),
                        )),
                        Err(e) => Message::View(view::Message::ConnectAccount(
                            ConnectAccountMessage::Contacts(
                                ContactsMessage::AcceptReceivedInviteFailed(
                                    invite_id,
                                    e.to_string(),
                                ),
                            ),
                        )),
                    },
                );
            }

            ContactsMessage::ReceivedInviteAccepted(invite_id) => {
                self.contacts_state.accepting_invite_ids.remove(&invite_id);
                // Reload from the API. This resets `received_invites = None`
                // and shows the skeleton briefly before the refetch lands —
                // the same UX as the InviteCreated / Resend flows. (An
                // optimistic `retain()` here would be a dead store: the view
                // only renders after `update()` returns, by which point
                // `reload_contacts()` has already cleared the list.)
                return self.reload_contacts();
            }

            ContactsMessage::AcceptReceivedInviteFailed(invite_id, msg) => {
                self.contacts_state.accepting_invite_ids.remove(&invite_id);
                self.contacts_state.error = Some(msg);
            }

            ContactsMessage::ShowInviteForm => {
                self.contacts_state.step = ContactsStep::InviteForm;
                self.contacts_state.invite_email.clear();
                self.contacts_state.invite_role = ContactRole::Keyholder;
                self.contacts_state.invite_sending = false;
                self.contacts_state.error = None;
                self.contacts_state.invite_cube_selections.clear();
                self.contacts_state.invite_cube_error = None;
                // Drop the prior cube list so re-opens don't briefly
                // render stale checkboxes from a previous session while
                // the fresh `list_cubes()` call is in flight. The view
                // hides the "Also add to Cube(s)" section entirely when
                // this is `None`.
                self.contacts_state.invite_available_cubes = None;
                // Load the user's cubes for the "Also add to Cube(s)"
                // multi-select (W12). Hidden in the view until this
                // resolves; empty Vec renders as "no cubes section".
                return load_invite_cubes(
                    &self.client,
                    self.session_generation,
                    self.contacts_state.active_network.clone(),
                );
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
                return self.fetch_contact_cubes(contact_id);
            }

            ContactsMessage::InviteEmailChanged(email) => {
                self.contacts_state.invite_email = email;
                self.contacts_state.error = None;
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
                return load_invite_cubes(
                    &self.client,
                    self.session_generation,
                    self.contacts_state.active_network.clone(),
                );
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
                self.contacts_state.error = None;
                return iced::Task::done(Message::View(view::Message::ShowSuccess(
                    "Invite resent".to_string(),
                )));
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

            // ── W14: Add-existing-contact-to-Cube ─────────────────────
            ContactsMessage::OpenAddToCubeDialog(contact_id) => {
                let Some(contact) = self
                    .contacts_state
                    .contacts
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .find(|c| c.id == contact_id)
                    .cloned()
                else {
                    log::warn!("[CONTACTS] OpenAddToCubeDialog: contact {contact_id} not found");
                    return iced::Task::none();
                };
                let email = match contact.contact_user.as_ref() {
                    Some(u) if !u.email.is_empty() => u.email.clone(),
                    _ => {
                        self.contacts_state
                            .add_to_current_cube_errors
                            .insert(contact_id, "Contact has no linked user".to_string());
                        return iced::Task::none();
                    }
                };
                self.contacts_state
                    .add_to_current_cube_errors
                    .remove(&contact_id);
                self.contacts_state.add_to_cube_target = Some(AddToCubeDialog {
                    contact_id,
                    contact_email: email,
                    candidate_cubes: None,
                    selections: Vec::new(),
                    submitting: false,
                    failures: std::collections::HashMap::new(),
                });
                return load_add_to_cube_candidates(
                    &self.client,
                    contact_id,
                    contact.effective_contact_user_id(),
                    self.session_generation,
                    self.contacts_state.active_network.clone(),
                );
            }

            ContactsMessage::AddToCubeCandidatesLoaded(contact_id, cubes, gen) => {
                // Stale-guard: session turnover OR a different contact's
                // dialog opened in the meantime.
                if gen != self.session_generation {
                    return iced::Task::none();
                }
                if let Some(dialog) = self.contacts_state.add_to_cube_target.as_mut() {
                    if dialog.contact_id == contact_id {
                        dialog.candidate_cubes = Some(cubes);
                    }
                }
            }

            ContactsMessage::ToggleAddToCubeSelection(cube_id) => {
                if let Some(dialog) = self.contacts_state.add_to_cube_target.as_mut() {
                    if let Some(pos) = dialog.selections.iter().position(|id| *id == cube_id) {
                        dialog.selections.remove(pos);
                    } else {
                        dialog.selections.push(cube_id);
                    }
                    dialog.failures.clear();
                }
            }

            ContactsMessage::ConfirmAddToCube => {
                let Some(dialog) = self.contacts_state.add_to_cube_target.as_mut() else {
                    return iced::Task::none();
                };
                if dialog.submitting || dialog.selections.is_empty() {
                    return iced::Task::none();
                }
                dialog.submitting = true;
                dialog.failures.clear();
                let email = dialog.contact_email.clone();
                let cube_ids = dialog.selections.clone();
                let contact_id = dialog.contact_id;
                let gen = self.session_generation;
                let client = self.client.clone();
                return iced::Task::perform(
                    async move {
                        // Sequential per-cube calls — N is small and
                        // sequential keeps the result order deterministic
                        // for the failures map.
                        let mut results = Vec::with_capacity(cube_ids.len());
                        for cube_id in cube_ids {
                            let r = client.create_cube_invite(cube_id, &email).await;
                            results.push((cube_id, r.map(|_| ()).map_err(|e| e.to_string())));
                        }
                        results
                    },
                    move |results| {
                        Message::View(view::Message::ConnectAccount(
                            ConnectAccountMessage::Contacts(ContactsMessage::AddToCubeResult(
                                contact_id, gen, results,
                            )),
                        ))
                    },
                );
            }

            ContactsMessage::AddToCubeResult(contact_id, gen, results) => {
                if gen != self.session_generation {
                    return iced::Task::none();
                }
                let Some(dialog) = self.contacts_state.add_to_cube_target.as_mut() else {
                    return iced::Task::none();
                };
                if dialog.contact_id != contact_id {
                    return iced::Task::none();
                }
                dialog.submitting = false;
                let mut failures = std::collections::HashMap::new();
                let mut succeeded_ids: Vec<u64> = Vec::new();
                for (cube_id, r) in results {
                    match r {
                        Ok(()) => succeeded_ids.push(cube_id),
                        Err(msg) => {
                            failures.insert(cube_id, msg);
                        }
                    }
                }
                if failures.is_empty() {
                    // Full success: close the dialog and refresh the
                    // Associated-Cubes section so the UI reflects the
                    // new memberships.
                    let contact_id = dialog.contact_id;
                    self.contacts_state.add_to_cube_target = None;
                    if matches!(
                        self.contacts_state.step,
                        ContactsStep::Detail(id) if id == contact_id
                    ) {
                        return self.fetch_contact_cubes(contact_id);
                    }
                    return iced::Task::none();
                }
                // Partial / full failure: keep the dialog open and surface
                // per-cube messages. Drop succeeded selections so the user
                // can't re-submit them (re-submit would 409 on duplicate).
                dialog.selections.retain(|id| !succeeded_ids.contains(id));
                dialog.failures = failures;
            }

            ContactsMessage::CloseAddToCubeDialog => {
                self.contacts_state.add_to_cube_target = None;
            }

            ContactsMessage::AddContactToCurrentCube(contact_id) => {
                if self
                    .contacts_state
                    .add_to_current_cube_pending
                    .contains(&contact_id)
                {
                    return iced::Task::none();
                }
                let Some(contact) = self
                    .contacts_state
                    .contacts
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .find(|c| c.id == contact_id)
                    .cloned()
                else {
                    log::warn!(
                        "[CONTACTS] AddContactToCurrentCube: contact {contact_id} not found"
                    );
                    return iced::Task::none();
                };
                let email = match contact.contact_user.as_ref() {
                    Some(u) if !u.email.is_empty() => u.email.clone(),
                    _ => {
                        self.contacts_state
                            .add_to_current_cube_errors
                            .insert(contact_id, "Contact has no linked user".to_string());
                        return iced::Task::none();
                    }
                };
                // Target the server-side id of the cube the user is
                // actually loaded into. This works regardless of how
                // many other cubes the user owns on the same network —
                // the prior "match-by-network" heuristic broke when
                // there were multiple mainnet cubes.
                let Some(cube_id) = self.contacts_state.active_cube_server_id else {
                    self.contacts_state.add_to_current_cube_errors.insert(
                        contact_id,
                        "Current cube isn't registered yet — please retry in a moment.".to_string(),
                    );
                    return iced::Task::none();
                };
                // Clear any prior error for this contact so retries show
                // fresh state.
                self.contacts_state
                    .add_to_current_cube_errors
                    .remove(&contact_id);
                self.contacts_state
                    .add_to_current_cube_pending
                    .insert(contact_id);
                let client = self.client.clone();
                return iced::Task::perform(
                    async move {
                        client
                            .create_cube_invite(cube_id, &email)
                            .await
                            .map(|_| cube_id)
                            .map_err(|e| e.to_string())
                    },
                    move |res| {
                        Message::View(view::Message::ConnectAccount(
                            ConnectAccountMessage::Contacts(
                                ContactsMessage::AddContactToCurrentCubeResult(contact_id, res),
                            ),
                        ))
                    },
                );
            }

            ContactsMessage::AddContactToCurrentCubeResult(contact_id, res) => {
                self.contacts_state
                    .add_to_current_cube_pending
                    .remove(&contact_id);
                match res {
                    Ok(_cube_id) => {
                        self.contacts_state
                            .add_to_current_cube_errors
                            .remove(&contact_id);
                        // Refresh the Associated Cubes section if we're on
                        // that contact's detail view.
                        if matches!(
                            self.contacts_state.step,
                            ContactsStep::Detail(id) if id == contact_id
                        ) {
                            return self.fetch_contact_cubes(contact_id);
                        }
                    }
                    Err(msg) => {
                        self.contacts_state
                            .add_to_current_cube_errors
                            .insert(contact_id, msg);
                    }
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

// ── Plan lifecycle / pricing-schema helpers (D1 / D3 / D4) ──────────────────
impl ConnectAccountPanel {
    /// Classify the current plan against `now`. Pure (takes the clock as
    /// an argument) so the banner/expired-state branching is unit-testable.
    pub fn plan_lifecycle_at(&self, now: chrono::DateTime<chrono::Utc>) -> PlanLifecycle {
        let Some(plan) = self.plan.as_ref() else {
            return PlanLifecycle::Free;
        };
        // The backend demotes a lapsed paid plan and reports it as
        // `past_due` — surface that as Expired regardless of the tier it
        // was reset to.
        if matches!(plan.status, PlanStatus::PastDue) {
            return PlanLifecycle::Expired;
        }
        if matches!(plan.plan, PlanTier::Free) {
            return PlanLifecycle::Free;
        }
        // Paid + active: decide on the renewal window.
        let Some(renewal_raw) = plan.renewal_at.as_deref() else {
            return PlanLifecycle::Active;
        };
        let Ok(renewal) = chrono::DateTime::parse_from_rfc3339(renewal_raw) else {
            return PlanLifecycle::Active;
        };
        let days_remaining = (renewal.with_timezone(&chrono::Utc) - now).num_days();
        if days_remaining <= PLAN_RENEWAL_BANNER_DAYS {
            PlanLifecycle::RenewalDue {
                days_remaining: days_remaining.max(0),
            }
        } else {
            PlanLifecycle::Active
        }
    }

    /// Lifecycle against the wall clock — used by the view layer.
    pub fn plan_lifecycle(&self) -> PlanLifecycle {
        self.plan_lifecycle_at(chrono::Utc::now())
    }

    /// True when the authenticated account currently holds the July-4 promo
    /// grant (Estate free for year one). Drives the promo manage-plan
    /// variant, the collapsed picker, and renewal-banner suppression
    /// (PLAN-estate-promo PR1).
    pub fn is_promo_plan(&self) -> bool {
        self.plan
            .as_ref()
            .map(|p| p.is_active_promo())
            .unwrap_or(false)
    }

    /// Whether self-service purchasing is currently available. Sourced from
    /// `GET /connect/features` (`purchasing_enabled`); absent → enabled, so
    /// the existing checkout flow stays intact for backends that don't send
    /// the flag (and for fall GA once the promo ends). The July-4 promo sets
    /// it `false`, which hides every purchase surface (PLAN-estate-promo
    /// PR2).
    pub fn purchasing_enabled(&self) -> bool {
        self.features
            .as_ref()
            .and_then(|f| f.purchasing_enabled)
            .unwrap_or(true)
    }

    /// Whether the pre-expiry renewal banner should render: the plan is
    /// within its renewal window AND the user hasn't dismissed it this
    /// session. The expired state has its own dedicated UX (D3), so the
    /// banner intentionally does not cover `Expired`.
    ///
    /// Suppressed entirely for promo accounts at launch (config-flagged via
    /// [`PROMO_SUPPRESS_RENEWAL_BANNER`]) and whenever purchasing is
    /// disabled — in both cases there is no purchase path, so a "Renew now"
    /// nag would be a dead end (PLAN-estate-promo PR1/PR2).
    pub fn show_renewal_banner(&self) -> bool {
        if (PROMO_SUPPRESS_RENEWAL_BANNER && self.is_promo_plan()) || !self.purchasing_enabled() {
            return false;
        }
        !self.renewal_banner_dismissed
            && matches!(self.plan_lifecycle(), PlanLifecycle::RenewalDue { .. })
    }

    /// True when `/connect/features` advertised a pricing schema newer
    /// than this build understands — drives the soft "update available"
    /// note in the plan picker (D4).
    pub fn pricing_schema_outdated(&self) -> bool {
        self.features
            .as_ref()
            .and_then(|f| f.pricing_schema_version)
            .map(|v| v > SUPPORTED_PRICING_SCHEMA_VERSION)
            .unwrap_or(false)
    }
}

/// Load Contacts tab data (contacts + invites).
pub fn load_contacts_data(client: &CoincubeClient, generation: u64) -> iced::Task<Message> {
    let c1 = client.clone();
    let c2 = client.clone();
    let c3 = client.clone();
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
        iced::Task::perform(
            async move { c3.get_received_invites().await },
            move |res| match res {
                Ok(received) => Message::View(view::Message::ConnectAccount(
                    ConnectAccountMessage::Contacts(ContactsMessage::ReceivedInvitesLoaded(
                        received, generation,
                    )),
                )),
                Err(e) => Message::View(view::Message::ConnectAccount(
                    ConnectAccountMessage::Contacts(ContactsMessage::Error(e.to_string())),
                )),
            },
        ),
    ])
}

/// Load the user's cubes for the W12 invite-form multi-select, mapping
/// the raw `CubeResponse`s into lightweight `InviteCubeOption`s. Used by
/// both the initial `ShowInviteForm` load and the `InviteCubeForbidden`
/// reload after a 403 — any backend error silently resolves to an empty
/// list so the invite form degrades to the plain (cube-less) path.
///
/// When `active_network` is `Some`, only cubes on that network are
/// returned (PR 5 §2.7 tweak #1 — prevents cross-network invites).
/// When it's `None` (e.g. the user is on a Connect-only surface with no
/// active cube selected), no filter is applied so the form doesn't
/// render empty.
fn load_invite_cubes(
    client: &CoincubeClient,
    generation: u64,
    active_network: Option<String>,
) -> iced::Task<Message> {
    let client = client.clone();
    iced::Task::perform(async move { client.list_cubes().await }, move |res| {
        let options = match res {
            Ok(cubes) => cubes
                .into_iter()
                .filter(|c| {
                    active_network
                        .as_deref()
                        .map(|net| c.network == net)
                        .unwrap_or(true)
                })
                .map(|c| InviteCubeOption {
                    id: c.id,
                    name: c.name,
                    network: c.network,
                })
                .collect(),
            Err(e) => {
                log::warn!("[CONTACTS] Failed to list cubes for invite form: {}", e);
                Vec::new()
            }
        };
        Message::View(view::Message::ConnectAccount(
            ConnectAccountMessage::Contacts(ContactsMessage::InviteCubesAvailable(
                options, generation,
            )),
        ))
    })
}

/// Load candidate cubes for the W14 "Add to Cube(s)…" dialog.
///
/// Applies three filters (in order):
///   1. **Network filter**: when `active_network` is `Some`, keep only
///      cubes on that network. When `None`, keep all (same fallback rule
///      as `load_invite_cubes`).
///   2. **Owner-or-member filter**: the list is returned by
///      `list_cubes()` which the backend scopes to cubes the caller
///      can administer, so this is effectively a noop today —
///      documented here so the invariant is obvious if backend scope
///      ever changes.
///   3. **Unjoined filter**: iterate each candidate, fetch `get_cube(id)`
///      to populate its `members`, and drop cubes where the contact's
///      user id already appears in the member list.
///
/// On any backend error, the candidate list silently resolves to
/// empty so the dialog just shows "no eligible cubes" rather than a
/// scary error. Individual `get_cube` failures drop that single cube.
fn load_add_to_cube_candidates(
    client: &CoincubeClient,
    contact_id: u64,
    contact_user_id: Option<u64>,
    generation: u64,
    active_network: Option<String>,
) -> iced::Task<Message> {
    let client = client.clone();
    iced::Task::perform(
        async move {
            let Ok(cubes) = client.list_cubes().await else {
                return Vec::new();
            };
            let mut out: Vec<InviteCubeOption> = Vec::new();
            for cube in cubes {
                // (1) network filter
                if let Some(net) = active_network.as_deref() {
                    if cube.network != net {
                        continue;
                    }
                }
                // (3) unjoined filter: per-cube fetch to resolve members.
                // Skip when we don't know the contact's user id (e.g.
                // contact has no linked user) — we can't tell if they're
                // already a member, so be permissive.
                if let Some(contact_uid) = contact_user_id {
                    match client.get_cube(cube.id).await {
                        Ok(full) => {
                            if full.members.iter().any(|m| m.user_id == contact_uid) {
                                continue;
                            }
                        }
                        Err(e) => {
                            log::warn!(
                                "[CONTACTS] get_cube({}) failed while filtering candidates: {}",
                                cube.id,
                                e
                            );
                            continue;
                        }
                    }
                }
                out.push(InviteCubeOption {
                    id: cube.id,
                    name: cube.name,
                    network: cube.network,
                });
            }
            out
        },
        move |options| {
            Message::View(view::Message::ConnectAccount(
                ConnectAccountMessage::Contacts(ContactsMessage::AddToCubeCandidatesLoaded(
                    contact_id, options, generation,
                )),
            ))
        },
    )
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

// ── Duress recovery + enrollment helpers (Phases 2, 6 & 8) ──

/// How many times the post-sign-in duress-state check is retried before giving
/// up (and leaving the dashboard, with the gRPC stream / relaunch reconcile as
/// the remaining safety nets).
const DURESS_CHECK_MAX_ATTEMPTS: u8 = 3;

/// Delay before each duress-state-check retry.
const DURESS_CHECK_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(2);

/// Issues a post-sign-in `get_duress_state` check (Phase 6). `attempt > 0`
/// sleeps first so a transient failure backs off before retrying. A failed
/// request collapses to `None`, which the handler turns into a bounded retry.
fn duress_state_check_task(client: CoincubeClient, gen: u64, attempt: u8) -> iced::Task<Message> {
    iced::Task::perform(
        async move {
            if attempt > 0 {
                tokio::time::sleep(DURESS_CHECK_RETRY_DELAY).await;
            }
            (client.get_duress_state().await.ok(), gen, attempt)
        },
        |(state, g, a)| {
            Message::View(view::Message::ConnectAccount(
                ConnectAccountMessage::DuressStateChecked(state, g, a),
            ))
        },
    )
}

/// Phase 0 device-code registration: for a signed-in desktop on an
/// already-enrolled account that the server doesn't yet recognise, mint a fresh
/// ~128-bit duress code, send only its argon2id hash via
/// `register_device_duress_code`, and persist the raw code (encrypted) +
/// account id into `DuressLocalState` so this device can fire
/// `trigger-with-code` on its own activation. Per-device — codes are never
/// shared. Best-effort and idempotent (skips when a code is already held).
fn register_device_duress_task(client: CoincubeClient, account_id: String) -> iced::Task<Message> {
    iced::Task::perform(
        async move {
            use crate::services::duress::{cipher::DeviceKey, enroll, DuressLocalState};
            let Ok(datadir) = crate::dir::CoincubeDirectory::active() else {
                log::warn!("[CONNECT] device duress register skipped: no data directory");
                return;
            };
            let root = datadir.path();
            // Skip on a real read error (vs a missing file): registering off a
            // default would overwrite valid state (enrolled / account_id /
            // existing code) on the save below. Retry happens on the next check.
            let mut st = match DuressLocalState::load(root) {
                Ok(st) => st,
                Err(e) => {
                    log::warn!(
                        "[CONNECT] device duress register: reading state failed; \
                         not overwriting: {e}"
                    );
                    return;
                }
            };
            let fingerprint = match crate::services::duress::device_fingerprint(root) {
                Ok(f) => f,
                Err(e) => {
                    log::warn!("[CONNECT] device duress register: fingerprint error: {e}");
                    return;
                }
            };
            let key = match DeviceKey::load_or_create(root) {
                Ok(k) => k,
                Err(e) => {
                    log::warn!("[CONNECT] device duress register: key error: {e}");
                    return;
                }
            };
            // Reuse an existing local code (re-hash it for the server) when one
            // is already held, so a re-fire after a failed registration — or a
            // stale `this_device_registered` from the server — re-registers the
            // SAME code instead of churning it. Otherwise mint a fresh one.
            let (enc, code_hash) = match st.duress_code.as_ref().and_then(|e| key.decrypt(e).ok()) {
                Some(existing) => match enroll::hash_duress_secret(&existing) {
                    Ok(h) => (st.duress_code.clone().unwrap(), h),
                    Err(e) => {
                        log::warn!("[CONNECT] device duress register: hash error: {e}");
                        return;
                    }
                },
                None => {
                    let code = enroll::generate_duress_code();
                    let enc = match key.encrypt(&code) {
                        Ok(enc) => enc,
                        Err(e) => {
                            log::warn!("[CONNECT] device duress register: encrypt error: {e}");
                            return;
                        }
                    };
                    let hash = match enroll::hash_duress_secret(&code) {
                        Ok(h) => h,
                        Err(e) => {
                            log::warn!("[CONNECT] device duress register: hash error: {e}");
                            return;
                        }
                    };
                    (enc, hash)
                }
            };
            // Persist the code locally BEFORE registering with the server, so
            // the server never marks this device registered while this install
            // lacks the matching code — which would block both activation's
            // trigger-with-code and the auto re-registration retry (gated on the
            // server's `!this_device_registered`). If the local save fails the
            // server is left untouched; if the server register fails, local
            // keeps the code and the next sign-in re-registers it.
            st.enrolled = true;
            st.account_id = Some(account_id);
            st.duress_code = Some(enc);
            if let Err(e) = st.save(root) {
                log::warn!("[CONNECT] device duress register: local save failed: {e}");
                return;
            }
            if let Err(e) = client
                .register_device_duress_code(&fingerprint, &code_hash)
                .await
            {
                log::warn!("[CONNECT] register_device_duress_code failed: {e}");
            }
        },
        |_| {
            Message::View(view::Message::ConnectAccount(
                ConnectAccountMessage::DuressDeviceRegistered,
            ))
        },
    )
}

/// This device's **stable** per-device fingerprint for duress enrollment. The
/// server keys its per-device rows and `this_device_registered` on this value,
/// so it must be the same across launches, repeat enrollments, and
/// re-registrations — hence it's loaded from (or minted into) a persisted file
/// at the data-directory root.
///
/// Errors are **propagated**, not masked: falling back to a fresh UUID on an
/// I/O failure would silently send a different fingerprint each time and defeat
/// the stability guarantee, so the wizard surfaces the failure and lets the
/// user retry instead.
fn device_fingerprint() -> Result<String, String> {
    // Use the process's ACTIVE data directory (honours a custom `--datadir`),
    // not the OS default — otherwise the fingerprint would be persisted at a
    // different path than the Cubes / DuressLocalState and diverge from what the
    // server was told.
    let dir = crate::dir::CoincubeDirectory::active()
        .map_err(|e| format!("data directory unavailable: {e}"))?;
    crate::services::duress::device_fingerprint(dir.path())
        .map_err(|e| format!("device fingerprint unavailable: {e}"))
}

/// The ordered steps for a tier. Sovereign opens with the Connect
/// encouragement + friction confirm; Connect tiers skip those. `SetCrkPassword`
/// is Tier 1 only.
fn enroll_steps(tier: EnrollTier) -> &'static [DuressEnrollStep] {
    use DuressEnrollStep::*;
    match tier {
        EnrollTier::Tier1 => &[
            SetDuressPin,
            SetAllClear,
            SetCrkPassword,
            PickDelay,
            Confirm,
        ],
        EnrollTier::Tier2 => &[SetDuressPin, SetAllClear, PickDelay, Confirm],
        EnrollTier::Sovereign => &[Encourage, SovereignConfirm, SetDuressPin, Confirm],
    }
}

fn next_enroll_step(tier: EnrollTier, cur: DuressEnrollStep) -> DuressEnrollStep {
    let steps = enroll_steps(tier);
    match steps.iter().position(|s| *s == cur) {
        Some(i) if i + 1 < steps.len() => steps[i + 1],
        _ => cur,
    }
}

fn prev_enroll_step(tier: EnrollTier, cur: DuressEnrollStep) -> DuressEnrollStep {
    let steps = enroll_steps(tier);
    match steps.iter().position(|s| *s == cur) {
        Some(i) if i > 0 => steps[i - 1],
        _ => cur,
    }
}

/// Validates the current step's inputs before advancing. Returns the spec error
/// string on failure.
fn validate_enroll_step(e: &DuressEnrollState) -> Result<(), String> {
    use crate::services::duress::enroll;
    match e.step {
        DuressEnrollStep::Encourage => Ok(()),
        DuressEnrollStep::SovereignConfirm => {
            if e.sovereign_confirm.trim() == "I have my seed-phrase backup" {
                Ok(())
            } else {
                Err("Type the confirmation phrase exactly to continue.".to_string())
            }
        }
        DuressEnrollStep::SetDuressPin => {
            enroll::validate_duress_pin(&e.regular_pin, &e.duress_pin)
        }
        DuressEnrollStep::SetAllClear => {
            enroll::validate_all_clear(&e.all_clear, &e.regular_pin, &e.duress_pin)
        }
        DuressEnrollStep::SetCrkPassword => enroll::validate_duress_crk_password(
            &e.crk_password,
            &e.regular_pin,
            &e.duress_pin,
            &e.all_clear,
        ),
        DuressEnrollStep::PickDelay => Ok(()),
        DuressEnrollStep::Confirm => {
            if e.memorized {
                Ok(())
            } else {
                Err("Confirm you have memorized all credentials.".to_string())
            }
        }
    }
}

#[cfg(test)]
mod duress_enroll_tests {
    use super::*;

    fn state(tier: EnrollTier, step: DuressEnrollStep) -> DuressEnrollState {
        DuressEnrollState {
            tier,
            step,
            regular_pin: "1234".to_string(),
            duress_pin: String::new(),
            all_clear: String::new(),
            crk_password: String::new(),
            delay: crate::services::duress::enroll::DuressDelay::default(),
            sovereign_confirm: String::new(),
            memorized: false,
            submitting: false,
            error: None,
            pending_code: None,
        }
    }

    #[test]
    fn zeroize_secrets_clears_all_sensitive_fields() {
        let mut s = state(EnrollTier::Tier1, DuressEnrollStep::Confirm);
        s.regular_pin = "1234".to_string();
        s.duress_pin = "8765".to_string();
        s.all_clear = "correct horse battery".to_string();
        s.crk_password = "a-long-crk-password".to_string();
        s.sovereign_confirm = "I have my seed-phrase backup".to_string();
        s.pending_code = Some("deadbeefcafebabe".to_string());

        s.zeroize_secrets();

        assert!(s.regular_pin.is_empty());
        assert!(s.duress_pin.is_empty());
        assert!(s.all_clear.is_empty());
        assert!(s.crk_password.is_empty());
        assert!(s.sovereign_confirm.is_empty());
        assert!(s.pending_code.as_deref().unwrap_or("").is_empty());
    }

    #[test]
    fn tier1_step_order() {
        let mut s = DuressEnrollStep::SetDuressPin;
        let order = [
            DuressEnrollStep::SetDuressPin,
            DuressEnrollStep::SetAllClear,
            DuressEnrollStep::SetCrkPassword,
            DuressEnrollStep::PickDelay,
            DuressEnrollStep::Confirm,
        ];
        for expected in &order[1..] {
            s = next_enroll_step(EnrollTier::Tier1, s);
            assert_eq!(s, *expected);
        }
        // Saturates at the end.
        assert_eq!(
            next_enroll_step(EnrollTier::Tier1, DuressEnrollStep::Confirm),
            DuressEnrollStep::Confirm
        );
    }

    #[test]
    fn tier2_skips_crk_password() {
        assert_eq!(
            next_enroll_step(EnrollTier::Tier2, DuressEnrollStep::SetAllClear),
            DuressEnrollStep::PickDelay
        );
    }

    #[test]
    fn sovereign_opens_with_encourage_and_confirm() {
        assert_eq!(
            enroll_steps(EnrollTier::Sovereign)[0],
            DuressEnrollStep::Encourage
        );
        assert_eq!(
            next_enroll_step(EnrollTier::Sovereign, DuressEnrollStep::Encourage),
            DuressEnrollStep::SovereignConfirm
        );
    }

    #[test]
    fn validate_rejects_close_duress_pin() {
        let mut s = state(EnrollTier::Tier1, DuressEnrollStep::SetDuressPin);
        s.duress_pin = "1235".to_string(); // Levenshtein 1
        assert!(validate_enroll_step(&s).is_err());
        s.duress_pin = "8765".to_string();
        assert!(validate_enroll_step(&s).is_ok());
    }

    #[test]
    fn sovereign_confirm_requires_exact_phrase() {
        let mut s = state(EnrollTier::Sovereign, DuressEnrollStep::SovereignConfirm);
        s.sovereign_confirm = "nope".to_string();
        assert!(validate_enroll_step(&s).is_err());
        s.sovereign_confirm = "I have my seed-phrase backup".to_string();
        assert!(validate_enroll_step(&s).is_ok());
    }

    #[test]
    fn confirm_requires_memorized_checkbox() {
        let mut s = state(EnrollTier::Tier1, DuressEnrollStep::Confirm);
        assert!(validate_enroll_step(&s).is_err());
        s.memorized = true;
        assert!(validate_enroll_step(&s).is_ok());
    }
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

    // ── PR 5 §2.7 tweak #2 — role selector removal ────────────────
    #[test]
    fn invite_role_defaults_to_keyholder_and_stays_keyholder() {
        // After dropping the role selector the state's `invite_role`
        // must initialise to — and stay at — Keyholder across a
        // typical invite form lifecycle.
        let mut panel = ConnectAccountPanel::new();
        assert_eq!(
            panel.contacts_state.invite_role,
            crate::services::coincube::ContactRole::Keyholder
        );
        dispatch(&mut panel, ContactsMessage::ShowInviteForm);
        assert_eq!(
            panel.contacts_state.invite_role,
            crate::services::coincube::ContactRole::Keyholder
        );
        dispatch(
            &mut panel,
            ContactsMessage::InviteEmailChanged("friend@example.com".to_string()),
        );
        assert_eq!(
            panel.contacts_state.invite_role,
            crate::services::coincube::ContactRole::Keyholder
        );
    }

    // ── PR 5 §2.7 tweak #1 — network filter plumbing ──────────────
    #[test]
    fn set_active_network_writes_to_contacts_state() {
        // The parent `ConnectPanel` calls `set_active_network` on
        // construction to propagate the cube's network down. Without
        // it, `load_invite_cubes` has no filter anchor and the dialog
        // would mix mainnet/regtest cubes.
        let mut panel = ConnectAccountPanel::new();
        assert!(panel.contacts_state.active_network.is_none());
        panel.set_active_network(Some("bitcoin".to_string()));
        assert_eq!(
            panel.contacts_state.active_network.as_deref(),
            Some("bitcoin")
        );
        panel.set_active_network(None);
        assert!(panel.contacts_state.active_network.is_none());
    }
}

// =============================================================================
// W14 state tests — "Add to Cube(s)…" dialog (PR 5 §2.8)
// =============================================================================
//
// Exercises the `ContactsMessage` dispatch path via the public
// `update_message` entrypoint, same pattern as `invite_form_tests` above.
// Tests never touch the network — `OpenAddToCubeDialog` also triggers an
// async `list_cubes()` fetch whose returned `Task` is discarded; we feed
// the follow-up `AddToCubeCandidatesLoaded` message manually to drive
// the state machine through its useful transitions.
#[cfg(test)]
mod add_to_cube_tests {
    use super::*;
    use crate::services::coincube::{ContactRole, ContactUser};

    fn dispatch(panel: &mut ConnectAccountPanel, msg: ContactsMessage) {
        let _ = panel.update_message(ConnectAccountMessage::Contacts(msg));
    }

    fn option(id: u64, name: &str, network: &str) -> InviteCubeOption {
        InviteCubeOption {
            id,
            name: name.to_string(),
            network: network.to_string(),
        }
    }

    fn sample_contact(contact_id: u64, user_id: u64, email: &str) -> Contact {
        Contact {
            id: contact_id,
            user_id: 0,
            contact_user_id: 0,
            invite_id: None,
            role: ContactRole::Keyholder,
            contact_user: Some(ContactUser {
                id: user_id,
                email: email.to_string(),
                email_verified: Some(true),
            }),
            created_at: "2026-04-20T00:00:00Z".to_string(),
        }
    }

    fn panel_with_contact(contact: Contact, network: Option<&str>) -> ConnectAccountPanel {
        let mut panel = ConnectAccountPanel::new();
        panel.contacts_state.contacts = Some(vec![contact]);
        panel.contacts_state.active_network = network.map(|s| s.to_string());
        panel
    }

    #[test]
    fn open_add_to_cube_dialog_initialises_target_and_loads_candidates() {
        let contact = sample_contact(7, 99, "alice@example.com");
        let mut panel = panel_with_contact(contact, Some("bitcoin"));

        // `OpenAddToCubeDialog` should install the dialog struct
        // synchronously and kick off the async candidate fetch (whose
        // result we ignore in this test — covered separately below).
        dispatch(&mut panel, ContactsMessage::OpenAddToCubeDialog(7));

        let dialog = panel
            .contacts_state
            .add_to_cube_target
            .as_ref()
            .expect("dialog should be open");
        assert_eq!(dialog.contact_id, 7);
        assert_eq!(dialog.contact_email, "alice@example.com");
        assert!(dialog.candidate_cubes.is_none(), "candidates pending load");
        assert!(dialog.selections.is_empty());
        assert!(!dialog.submitting);
    }

    #[test]
    fn open_add_to_cube_dialog_noop_when_contact_missing() {
        // No contact with id 99 in the panel — the handler should log
        // and bail without installing a dialog.
        let contact = sample_contact(7, 99, "alice@example.com");
        let mut panel = panel_with_contact(contact, Some("bitcoin"));
        dispatch(&mut panel, ContactsMessage::OpenAddToCubeDialog(99));
        assert!(panel.contacts_state.add_to_cube_target.is_none());
    }

    #[test]
    fn add_to_cube_candidates_loaded_populates_dialog() {
        let contact = sample_contact(7, 99, "alice@example.com");
        let mut panel = panel_with_contact(contact, Some("bitcoin"));
        dispatch(&mut panel, ContactsMessage::OpenAddToCubeDialog(7));

        let gen = panel.session_generation();
        dispatch(
            &mut panel,
            ContactsMessage::AddToCubeCandidatesLoaded(
                7,
                vec![option(1, "Alpha", "bitcoin"), option(2, "Bravo", "bitcoin")],
                gen,
            ),
        );
        let cubes = panel
            .contacts_state
            .add_to_cube_target
            .as_ref()
            .unwrap()
            .candidate_cubes
            .as_ref()
            .expect("candidates should be populated");
        assert_eq!(cubes.len(), 2);
        assert_eq!(cubes[0].name, "Alpha");
    }

    #[test]
    fn add_to_cube_candidates_loaded_stale_gen_ignored() {
        let contact = sample_contact(7, 99, "alice@example.com");
        let mut panel = panel_with_contact(contact, Some("bitcoin"));
        dispatch(&mut panel, ContactsMessage::OpenAddToCubeDialog(7));
        let stale_gen = panel.session_generation().wrapping_add(1);
        dispatch(
            &mut panel,
            ContactsMessage::AddToCubeCandidatesLoaded(
                7,
                vec![option(1, "Alpha", "bitcoin")],
                stale_gen,
            ),
        );
        assert!(panel
            .contacts_state
            .add_to_cube_target
            .as_ref()
            .unwrap()
            .candidate_cubes
            .is_none());
    }

    #[test]
    fn add_to_cube_candidates_loaded_wrong_contact_ignored() {
        // Dialog opened for contact 7, but the load result arrives for
        // contact 99 (race with a concurrent Open). Should be dropped.
        let contact = sample_contact(7, 99, "alice@example.com");
        let mut panel = panel_with_contact(contact, Some("bitcoin"));
        dispatch(&mut panel, ContactsMessage::OpenAddToCubeDialog(7));
        let gen = panel.session_generation();
        dispatch(
            &mut panel,
            ContactsMessage::AddToCubeCandidatesLoaded(
                99,
                vec![option(1, "Alpha", "bitcoin")],
                gen,
            ),
        );
        assert!(panel
            .contacts_state
            .add_to_cube_target
            .as_ref()
            .unwrap()
            .candidate_cubes
            .is_none());
    }

    #[test]
    fn toggle_add_to_cube_selection_adds_and_removes() {
        let contact = sample_contact(7, 99, "alice@example.com");
        let mut panel = panel_with_contact(contact, Some("bitcoin"));
        dispatch(&mut panel, ContactsMessage::OpenAddToCubeDialog(7));
        dispatch(&mut panel, ContactsMessage::ToggleAddToCubeSelection(1));
        dispatch(&mut panel, ContactsMessage::ToggleAddToCubeSelection(3));
        assert_eq!(
            panel
                .contacts_state
                .add_to_cube_target
                .as_ref()
                .unwrap()
                .selections,
            vec![1, 3]
        );
        dispatch(&mut panel, ContactsMessage::ToggleAddToCubeSelection(1));
        assert_eq!(
            panel
                .contacts_state
                .add_to_cube_target
                .as_ref()
                .unwrap()
                .selections,
            vec![3]
        );
    }

    #[test]
    fn add_to_cube_result_full_success_closes_dialog() {
        let contact = sample_contact(7, 99, "alice@example.com");
        let mut panel = panel_with_contact(contact, Some("bitcoin"));
        dispatch(&mut panel, ContactsMessage::OpenAddToCubeDialog(7));
        // Stand in for the async branch: pretend the user picked cubes
        // 1 and 2 and the parallel `create_cube_invite` calls all
        // succeeded.
        if let Some(d) = panel.contacts_state.add_to_cube_target.as_mut() {
            d.selections = vec![1, 2];
            d.submitting = true;
        }
        dispatch(
            &mut panel,
            ContactsMessage::AddToCubeResult(7, 0, vec![(1, Ok(())), (2, Ok(()))]),
        );
        assert!(
            panel.contacts_state.add_to_cube_target.is_none(),
            "full-success should close the dialog"
        );
    }

    #[test]
    fn add_to_cube_result_partial_failure_keeps_dialog_open_with_error_summary() {
        let contact = sample_contact(7, 99, "alice@example.com");
        let mut panel = panel_with_contact(contact, Some("bitcoin"));
        dispatch(&mut panel, ContactsMessage::OpenAddToCubeDialog(7));
        if let Some(d) = panel.contacts_state.add_to_cube_target.as_mut() {
            d.selections = vec![1, 2, 3];
            d.submitting = true;
        }
        dispatch(
            &mut panel,
            ContactsMessage::AddToCubeResult(
                7,
                0,
                vec![
                    (1, Ok(())),
                    (2, Err("already a member".to_string())),
                    (3, Err("forbidden".to_string())),
                ],
            ),
        );
        let dialog = panel
            .contacts_state
            .add_to_cube_target
            .as_ref()
            .expect("partial failure must keep the dialog open");
        assert!(!dialog.submitting);
        assert_eq!(
            dialog.failures.get(&2).map(String::as_str),
            Some("already a member")
        );
        assert_eq!(
            dialog.failures.get(&3).map(String::as_str),
            Some("forbidden")
        );
        // Succeeded cube (id 1) dropped from selections so it isn't
        // resubmitted on a retry.
        assert_eq!(dialog.selections, vec![2, 3]);
    }

    #[test]
    fn close_add_to_cube_dialog_clears_target() {
        let contact = sample_contact(7, 99, "alice@example.com");
        let mut panel = panel_with_contact(contact, Some("bitcoin"));
        dispatch(&mut panel, ContactsMessage::OpenAddToCubeDialog(7));
        dispatch(&mut panel, ContactsMessage::CloseAddToCubeDialog);
        assert!(panel.contacts_state.add_to_cube_target.is_none());
    }

    #[test]
    fn add_contact_to_current_cube_proceeds_when_server_id_is_set() {
        // Regression: the prior implementation refused the one-click
        // action when the user had multiple cubes on the same network.
        // After routing to the specific `active_cube_server_id`, the
        // handler must proceed (no per-contact error set) regardless
        // of how many other cubes exist.
        let contact = sample_contact(7, 99, "alice@example.com");
        let mut panel = panel_with_contact(contact, Some("bitcoin"));
        panel.contacts_state.active_cube_server_id = Some(42);
        dispatch(&mut panel, ContactsMessage::AddContactToCurrentCube(7));
        // The async `create_cube_invite` Task fires but we don't drive
        // it; assert on what the synchronous handler setup did:
        // — no error inserted in the per-contact map
        // — any prior error cleared
        assert!(!panel
            .contacts_state
            .add_to_current_cube_errors
            .contains_key(&7));
    }

    #[test]
    fn add_contact_to_current_cube_without_server_id_surfaces_not_ready_error() {
        // If the cube hasn't registered with the backend yet
        // (`active_cube_server_id` is still None), fail fast with a
        // human-readable message rather than firing a speculative
        // network request that would also fail.
        let contact = sample_contact(7, 99, "alice@example.com");
        let mut panel = panel_with_contact(contact, Some("bitcoin"));
        panel.contacts_state.active_cube_server_id = None;
        dispatch(&mut panel, ContactsMessage::AddContactToCurrentCube(7));
        let msg = panel
            .contacts_state
            .add_to_current_cube_errors
            .get(&7)
            .map(String::as_str);
        assert_eq!(
            msg,
            Some("Current cube isn't registered yet — please retry in a moment.")
        );
    }

    #[test]
    fn add_contact_to_current_cube_without_linked_user_surfaces_error() {
        // Contact has no `contact_user` (backend omitempty'd it) —
        // there's no email to invite with, so the handler must drop an
        // inline error keyed by contact id and skip the network call.
        let mut contact = sample_contact(7, 99, "alice@example.com");
        contact.contact_user = None;
        let mut panel = panel_with_contact(contact, Some("bitcoin"));
        dispatch(&mut panel, ContactsMessage::AddContactToCurrentCube(7));
        assert!(panel
            .contacts_state
            .add_to_current_cube_errors
            .contains_key(&7));
    }

    #[test]
    fn add_contact_to_current_cube_result_ok_clears_any_prior_error() {
        let contact = sample_contact(7, 99, "alice@example.com");
        let mut panel = panel_with_contact(contact, Some("bitcoin"));
        panel
            .contacts_state
            .add_to_current_cube_errors
            .insert(7, "earlier failure".to_string());
        dispatch(
            &mut panel,
            ContactsMessage::AddContactToCurrentCubeResult(7, Ok(42)),
        );
        assert!(!panel
            .contacts_state
            .add_to_current_cube_errors
            .contains_key(&7));
    }

    #[test]
    fn add_contact_to_current_cube_result_err_stores_message() {
        let contact = sample_contact(7, 99, "alice@example.com");
        let mut panel = panel_with_contact(contact, Some("bitcoin"));
        dispatch(
            &mut panel,
            ContactsMessage::AddContactToCurrentCubeResult(7, Err("backend 500".to_string())),
        );
        assert_eq!(
            panel
                .contacts_state
                .add_to_current_cube_errors
                .get(&7)
                .map(String::as_str),
            Some("backend 500")
        );
    }

    // ── `contact_is_in_active_cube` helper ────────────────────────
    // The contact-detail view uses this to hide the "Add to Current
    // Cube" button when clicking it would no-op / 409.
    use crate::services::coincube::ContactCube;

    fn sample_contact_cube(id: u64, network: &str) -> ContactCube {
        ContactCube {
            id,
            uuid: format!("cube-{id}"),
            name: format!("Cube {id}"),
            network: network.to_string(),
            has_recovery_kit: false,
        }
    }

    #[test]
    fn contact_is_in_active_cube_false_when_no_active_cube() {
        let contact = sample_contact(7, 99, "alice@example.com");
        let mut panel = panel_with_contact(contact, Some("bitcoin"));
        panel.contacts_state.step = ContactsStep::Detail(7);
        panel.contacts_state.detail_cubes = Some(vec![sample_contact_cube(42, "bitcoin")]);
        // No active_cube_server_id → helper returns false (we can't say
        // the contact is in "the active cube" when there isn't one).
        panel.contacts_state.active_cube_server_id = None;
        assert!(!panel.contacts_state.contact_is_in_active_cube(7));
    }

    #[test]
    fn contact_is_in_active_cube_false_while_detail_cubes_loading() {
        // Optimistic: show the button until the associated-cubes fetch
        // completes, rather than flashing it in once data arrives.
        let contact = sample_contact(7, 99, "alice@example.com");
        let mut panel = panel_with_contact(contact, Some("bitcoin"));
        panel.contacts_state.step = ContactsStep::Detail(7);
        panel.contacts_state.active_cube_server_id = Some(42);
        panel.contacts_state.detail_cubes = None;
        assert!(!panel.contacts_state.contact_is_in_active_cube(7));
    }

    #[test]
    fn contact_is_in_active_cube_false_when_not_on_detail_step() {
        // Defensive: only the Detail(contact_id) step has `detail_cubes`
        // scoped to this contact. Returning true on any other step
        // would mis-gate the button on the list view.
        let contact = sample_contact(7, 99, "alice@example.com");
        let mut panel = panel_with_contact(contact, Some("bitcoin"));
        panel.contacts_state.active_cube_server_id = Some(42);
        panel.contacts_state.detail_cubes = Some(vec![sample_contact_cube(42, "bitcoin")]);
        panel.contacts_state.step = ContactsStep::List;
        assert!(!panel.contacts_state.contact_is_in_active_cube(7));
    }

    #[test]
    fn contact_is_in_active_cube_false_when_active_cube_not_in_associated_list() {
        // Contact is a member of cubes 1 and 2, but the user is loaded
        // into cube 42 — the contact is NOT in the active cube, so the
        // button should remain visible.
        let contact = sample_contact(7, 99, "alice@example.com");
        let mut panel = panel_with_contact(contact, Some("bitcoin"));
        panel.contacts_state.step = ContactsStep::Detail(7);
        panel.contacts_state.active_cube_server_id = Some(42);
        panel.contacts_state.detail_cubes = Some(vec![
            sample_contact_cube(1, "bitcoin"),
            sample_contact_cube(2, "bitcoin"),
        ]);
        assert!(!panel.contacts_state.contact_is_in_active_cube(7));
    }

    #[test]
    fn contact_is_in_active_cube_true_when_active_cube_in_associated_list() {
        // The core regression target: contact is already a member of
        // the active cube, so the button should hide.
        let contact = sample_contact(7, 99, "alice@example.com");
        let mut panel = panel_with_contact(contact, Some("bitcoin"));
        panel.contacts_state.step = ContactsStep::Detail(7);
        panel.contacts_state.active_cube_server_id = Some(42);
        panel.contacts_state.detail_cubes = Some(vec![
            sample_contact_cube(1, "bitcoin"),
            sample_contact_cube(42, "bitcoin"),
        ]);
        assert!(panel.contacts_state.contact_is_in_active_cube(7));
    }
}

// =============================================================================
// Plan lifecycle / pricing-schema tests (PLAN-billing-desktop §3)
// =============================================================================
//
// Cover the pure `plan_lifecycle_at` projection across renewal-date windows
// (outside window, within window, expired), the `past_due` → Expired branch,
// banner visibility/dismissal, and the schema-version soft-update trigger.
#[cfg(test)]
mod plan_lifecycle_tests {
    use super::*;
    use crate::services::coincube::{PlanEntitlements, PlanFeatureInfo, PlanSource};

    /// Parse a fixed RFC-3339 instant for deterministic `now`/renewal math.
    fn at(iso: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(iso)
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    fn plan(
        tier: PlanTier,
        status: PlanStatus,
        renewal_at: Option<&str>,
        cycle: Option<BillingCycle>,
    ) -> ConnectPlan {
        ConnectPlan {
            plan: tier,
            status,
            renewal_at: renewal_at.map(|s| s.to_string()),
            entitlements: PlanEntitlements {
                free_signing_key_count: 0,
                policy_editing: false,
                legacy_invites: false,
                linked_keychains: false,
                duress_remote_lock: false,
                business_orgs: false,
            },
            billing_cycle: cycle,
            plan_source: None,
        }
    }

    /// A `ConnectPlan` carrying explicit promo provenance, for the
    /// estate-promo cases.
    fn promo_plan(
        status: PlanStatus,
        renewal_at: Option<&str>,
        source: PlanSource,
    ) -> ConnectPlan {
        let mut p = plan(PlanTier::Estate, status, renewal_at, Some(BillingCycle::Annual));
        p.plan_source = Some(source);
        p
    }

    fn panel_with_plan(plan: ConnectPlan) -> ConnectAccountPanel {
        let mut panel = ConnectAccountPanel::new();
        panel.plan = Some(plan);
        panel
    }

    const NOW: &str = "2026-06-08T00:00:00Z";

    #[test]
    fn no_plan_is_free() {
        let panel = ConnectAccountPanel::new();
        assert_eq!(panel.plan_lifecycle_at(at(NOW)), PlanLifecycle::Free);
    }

    #[test]
    fn free_active_plan_is_free() {
        let panel = panel_with_plan(plan(PlanTier::Free, PlanStatus::Active, None, None));
        assert_eq!(panel.plan_lifecycle_at(at(NOW)), PlanLifecycle::Free);
    }

    #[test]
    fn paid_plan_outside_window_is_active() {
        // Renewal a month out — well beyond the 7-day banner threshold.
        let panel = panel_with_plan(plan(
            PlanTier::Pro,
            PlanStatus::Active,
            Some("2026-07-08T00:00:00Z"),
            Some(BillingCycle::Monthly),
        ));
        assert_eq!(panel.plan_lifecycle_at(at(NOW)), PlanLifecycle::Active);
    }

    #[test]
    fn paid_plan_within_window_is_renewal_due() {
        // Renewal 5 days out → inside the window, days_remaining == 5.
        let panel = panel_with_plan(plan(
            PlanTier::Pro,
            PlanStatus::Active,
            Some("2026-06-13T00:00:00Z"),
            Some(BillingCycle::Annual),
        ));
        assert_eq!(
            panel.plan_lifecycle_at(at(NOW)),
            PlanLifecycle::RenewalDue { days_remaining: 5 }
        );
    }

    #[test]
    fn paid_plan_exactly_at_threshold_is_renewal_due() {
        // Boundary: renewal exactly PLAN_RENEWAL_BANNER_DAYS out is in-window.
        let panel = panel_with_plan(plan(
            PlanTier::Estate,
            PlanStatus::Active,
            Some("2026-06-15T00:00:00Z"),
            Some(BillingCycle::Monthly),
        ));
        assert_eq!(
            panel.plan_lifecycle_at(at(NOW)),
            PlanLifecycle::RenewalDue {
                days_remaining: PLAN_RENEWAL_BANNER_DAYS
            }
        );
    }

    #[test]
    fn paid_plan_past_renewal_but_not_yet_demoted_clamps_to_zero() {
        // Renewal already elapsed but the backend hasn't demoted yet —
        // days_remaining clamps at 0 rather than going negative.
        let panel = panel_with_plan(plan(
            PlanTier::Pro,
            PlanStatus::Active,
            Some("2026-06-06T00:00:00Z"),
            Some(BillingCycle::Monthly),
        ));
        assert_eq!(
            panel.plan_lifecycle_at(at(NOW)),
            PlanLifecycle::RenewalDue { days_remaining: 0 }
        );
    }

    #[test]
    fn free_past_due_is_expired() {
        // The D3 case: backend demoted a lapsed paid plan to Free + past_due.
        let panel = panel_with_plan(plan(
            PlanTier::Free,
            PlanStatus::PastDue,
            Some("2026-06-01T00:00:00Z"),
            None,
        ));
        assert_eq!(panel.plan_lifecycle_at(at(NOW)), PlanLifecycle::Expired);
    }

    #[test]
    fn paid_past_due_is_expired() {
        // Defensive: a paid tier still flagged past_due also reads Expired.
        let panel = panel_with_plan(plan(
            PlanTier::Pro,
            PlanStatus::PastDue,
            Some("2026-06-01T00:00:00Z"),
            Some(BillingCycle::Monthly),
        ));
        assert_eq!(panel.plan_lifecycle_at(at(NOW)), PlanLifecycle::Expired);
    }

    #[test]
    fn paid_plan_without_renewal_date_is_active() {
        // No renewal_at to reason about — treat as active (not in-window).
        let panel = panel_with_plan(plan(
            PlanTier::Pro,
            PlanStatus::Active,
            None,
            Some(BillingCycle::Monthly),
        ));
        assert_eq!(panel.plan_lifecycle_at(at(NOW)), PlanLifecycle::Active);
    }

    // ── Banner visibility / dismissal ─────────────────────────────────
    #[test]
    fn dismiss_renewal_banner_sets_flag() {
        let mut panel = panel_with_plan(plan(
            PlanTier::Pro,
            PlanStatus::Active,
            Some("2026-06-13T00:00:00Z"),
            Some(BillingCycle::Monthly),
        ));
        assert!(!panel.renewal_banner_dismissed);
        let _ = panel.update_message(ConnectAccountMessage::DismissRenewalBanner);
        assert!(panel.renewal_banner_dismissed);
        // Dismissed banner never shows, regardless of lifecycle.
        assert!(!panel.show_renewal_banner());
    }

    #[test]
    fn open_plan_billing_clears_stale_history_toggle() {
        // Regression: "View plans" must land on the picker even if the user
        // left billing history open earlier in the session — `plan_billing_ux`
        // renders history ahead of the picker.
        let mut panel = ConnectAccountPanel::new();
        panel.show_billing_history = true;
        let _ = panel.update_message(ConnectAccountMessage::OpenPlanBilling);
        assert_eq!(panel.active_sub, ConnectSubMenu::PlanBilling);
        assert!(!panel.show_billing_history);
    }

    #[test]
    fn renew_current_plan_clears_stale_history_toggle() {
        // The Free/lapsed fallback path starts no checkout, so a stale
        // history toggle would otherwise mask the picker there too.
        let mut panel = panel_with_plan(plan(PlanTier::Free, PlanStatus::Active, None, None));
        panel.show_billing_history = true;
        let _ = panel.update_message(ConnectAccountMessage::RenewCurrentPlan);
        assert_eq!(panel.active_sub, ConnectSubMenu::PlanBilling);
        assert!(!panel.show_billing_history);
    }

    // ── Pricing schema soft-update note (D4) ──────────────────────────
    fn features(version: Option<u32>) -> FeaturesResponse {
        features_with_purchasing(version, None)
    }

    fn features_with_purchasing(
        version: Option<u32>,
        purchasing_enabled: Option<bool>,
    ) -> FeaturesResponse {
        FeaturesResponse {
            plans: vec![PlanFeatureInfo {
                name: "pro".to_string(),
                price: None,
                features: Vec::new(),
                included_linked_participants: None,
            }],
            pricing_schema_version: version,
            purchasing_enabled,
        }
    }

    #[test]
    fn schema_not_outdated_when_features_absent() {
        let panel = ConnectAccountPanel::new();
        assert!(!panel.pricing_schema_outdated());
    }

    #[test]
    fn schema_not_outdated_at_or_below_supported_version() {
        let mut panel = ConnectAccountPanel::new();
        panel.features = Some(features(None));
        assert!(!panel.pricing_schema_outdated());
        panel.features = Some(features(Some(SUPPORTED_PRICING_SCHEMA_VERSION)));
        assert!(!panel.pricing_schema_outdated());
    }

    #[test]
    fn schema_outdated_above_supported_version() {
        let mut panel = ConnectAccountPanel::new();
        panel.features = Some(features(Some(SUPPORTED_PRICING_SCHEMA_VERSION + 1)));
        assert!(panel.pricing_schema_outdated());
    }

    // ── Estate promo: provenance detection (PR1) ──────────────────────
    #[test]
    fn active_promo_is_detected() {
        let panel = panel_with_plan(promo_plan(
            PlanStatus::Active,
            Some("2027-07-04T00:00:00Z"),
            PlanSource::PromoEstateY1,
        ));
        assert!(panel.plan.as_ref().unwrap().is_promo());
        assert!(panel.plan.as_ref().unwrap().is_active_promo());
        assert!(panel.is_promo_plan());
    }

    #[test]
    fn paid_plan_is_not_promo() {
        // Explicit `paid` provenance must never read as promo.
        let panel = panel_with_plan(promo_plan(
            PlanStatus::Active,
            Some("2027-07-04T00:00:00Z"),
            PlanSource::Paid,
        ));
        assert!(!panel.plan.as_ref().unwrap().is_promo());
        assert!(!panel.is_promo_plan());
    }

    #[test]
    fn missing_plan_source_is_not_promo() {
        // Backward compatibility: older API omits `plan_source` → `None` →
        // ordinary (non-promo) UX.
        let panel = panel_with_plan(plan(
            PlanTier::Estate,
            PlanStatus::Active,
            Some("2027-07-04T00:00:00Z"),
            Some(BillingCycle::Annual),
        ));
        assert!(panel.plan.as_ref().unwrap().plan_source.is_none());
        assert!(!panel.plan.as_ref().unwrap().is_promo());
        assert!(!panel.is_promo_plan());
    }

    #[test]
    fn unknown_plan_source_is_not_promo() {
        // A future/unrecognized provenance value must not hide purchasing.
        let panel = panel_with_plan(promo_plan(
            PlanStatus::Active,
            Some("2027-07-04T00:00:00Z"),
            PlanSource::Unknown,
        ));
        assert!(!panel.is_promo_plan());
    }

    #[test]
    fn lapsed_promo_is_not_active_promo_and_reads_expired() {
        // At the year-one cliff the backend demotes the promo to `past_due`;
        // it must fall through to the ordinary expired UX, not the promo card.
        let panel = panel_with_plan(promo_plan(
            PlanStatus::PastDue,
            Some("2026-06-01T00:00:00Z"),
            PlanSource::PromoEstateY1,
        ));
        assert!(panel.plan.as_ref().unwrap().is_promo());
        assert!(!panel.plan.as_ref().unwrap().is_active_promo());
        assert!(!panel.is_promo_plan());
        assert_eq!(panel.plan_lifecycle_at(at(NOW)), PlanLifecycle::Expired);
    }

    #[test]
    fn promo_account_suppresses_renewal_banner() {
        // Even inside the renewal window, a promo account suppresses the
        // pre-expiry banner at launch (config-flagged).
        assert!(
            PROMO_SUPPRESS_RENEWAL_BANNER,
            "test assumes launch config; update if flipped for GA"
        );
        let panel = panel_with_plan(promo_plan(
            PlanStatus::Active,
            Some("2026-06-13T00:00:00Z"), // 5 days out → RenewalDue
            PlanSource::PromoEstateY1,
        ));
        assert_eq!(
            panel.plan_lifecycle_at(at(NOW)),
            PlanLifecycle::RenewalDue { days_remaining: 5 }
        );
        assert!(!panel.show_renewal_banner());
    }

    // ── Estate promo: purchasing gate (PR2) ───────────────────────────
    #[test]
    fn purchasing_enabled_defaults_true() {
        // Absent features, and a features payload without the field, both
        // read as enabled — the existing flow stays intact for fall GA.
        let mut panel = ConnectAccountPanel::new();
        assert!(panel.purchasing_enabled());
        panel.features = Some(features_with_purchasing(None, None));
        assert!(panel.purchasing_enabled());
        panel.features = Some(features_with_purchasing(None, Some(true)));
        assert!(panel.purchasing_enabled());
    }

    #[test]
    fn purchasing_disabled_when_flag_false() {
        let mut panel = ConnectAccountPanel::new();
        panel.features = Some(features_with_purchasing(None, Some(false)));
        assert!(!panel.purchasing_enabled());
    }

    #[test]
    fn purchasing_disabled_suppresses_renewal_banner() {
        // A paid account in its renewal window, but purchasing is closed —
        // suppress the "Renew" nag rather than dangle a dead-end CTA.
        let mut panel = panel_with_plan(plan(
            PlanTier::Pro,
            PlanStatus::Active,
            Some("2026-06-13T00:00:00Z"),
            Some(BillingCycle::Monthly),
        ));
        panel.features = Some(features_with_purchasing(None, Some(false)));
        assert_eq!(
            panel.plan_lifecycle_at(at(NOW)),
            PlanLifecycle::RenewalDue { days_remaining: 5 }
        );
        assert!(!panel.show_renewal_banner());
    }

    #[test]
    fn start_checkout_ignored_when_purchasing_disabled() {
        let mut panel = panel_with_plan(plan(PlanTier::Free, PlanStatus::Active, None, None));
        panel.features = Some(features_with_purchasing(None, Some(false)));
        let _ = panel.update_message(ConnectAccountMessage::StartCheckout(PlanTier::Pro));
        assert!(
            panel.checkout.is_none(),
            "checkout must not open while purchasing is disabled"
        );
    }

    #[test]
    fn start_checkout_proceeds_when_purchasing_enabled() {
        // Default (no features / flag absent) keeps the existing checkout
        // path working — regression guard for fall GA.
        let mut panel = panel_with_plan(plan(PlanTier::Free, PlanStatus::Active, None, None));
        let _ = panel.update_message(ConnectAccountMessage::StartCheckout(PlanTier::Pro));
        assert!(matches!(
            panel.checkout.as_ref().map(|c| &c.phase),
            Some(CheckoutPhase::Creating)
        ));
    }
}
