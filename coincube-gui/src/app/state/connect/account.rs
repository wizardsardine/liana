use iced::widget::qr_code;

use crate::{
    app::{
        menu::ConnectSubMenu,
        message::Message,
        view::{
            self, ConnectAccountMessage, ContactsMessage, DuressContactsMessage, DuressMessage,
            RetryAction,
        },
    },
    services::coincube::{
        BillingCycle, BillingHistoryEntry, ChargeState, ChargeStatusResponse, CheckoutRequest,
        CheckoutResponse, CoincubeClient, ConnectPlan, Contact, ContactCube, ContactRole,
        CreateDuressAlertContactRequest, CreateInviteRequest, CubeResponse, DuressAlertContact,
        DuressCheckOutcome, FeaturesResponse, Invite, LoginActivity, LoginResponse, OtpRequest,
        OtpVerifyRequest, PlanStatus, PlanTier, ReceivedInvite, RecoveryKitStatus,
        UpdateDuressAlertContactRequest, User, VerifiedDevice, DURESS_CHANNEL_EMAIL,
        DURESS_CHANNEL_SMS, DURESS_CHANNEL_WHATSAPP, MAX_DURESS_ALERT_CONTACTS,
    },
    user_error::UserError,
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

/// A stored session waiting to be checked (see `ConnectAccountPanel::unverified`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnverifiedSession {
    /// The account the stored session belongs to.
    pub email: String,
    /// Why it couldn't be checked: no network, or Connect not answering.
    pub trouble: crate::services::connect::grpc::trouble::Trouble,
    /// Failed checks in a row; sets the backoff before the next one.
    pub attempts: u32,
}

impl UnverifiedSession {
    /// Ticks after the backoff, for the subscription of whichever screen
    /// shows this account; it maps the tick to `RecheckSession`.
    pub fn recheck(&self) -> iced::Subscription<std::time::Instant> {
        iced::time::every(self.backoff())
    }

    /// The wait before the next check: 1 s doubling to 30 s, like the
    /// realtime stream's reconnects.
    pub fn backoff(&self) -> std::time::Duration {
        let exp = self.attempts.saturating_sub(1).min(5);
        std::time::Duration::from_secs((1u64 << exp).min(30))
    }
}

/// Whether a failed refresh never reached Connect, and why: transport
/// failures and Connect's 5xx keep the stored session; anything else is a
/// real failure.
fn refresh_trouble(
    error: &crate::services::coincube::CoincubeError,
) -> Option<crate::services::connect::grpc::trouble::Trouble> {
    use crate::services::coincube::CoincubeError;
    use crate::services::connect::grpc::trouble::{trouble, Trouble};
    match error {
        CoincubeError::Network(e) if e.is_connect() || e.is_timeout() || e.is_request() => {
            Some(trouble(e))
        }
        CoincubeError::Unsuccessful(info) if info.status_code >= 500 => Some(Trouble::Unreachable),
        _ => None,
    }
}

/// Read an existing ordinary desktop login without changing the global session.
/// The namespace selects local credential storage, never the API environment.
#[cfg(feature = "integration-tests")]
pub fn read_session_for_integration(namespace: &str) -> Result<LoginResponse, &'static str> {
    let service = match namespace {
        "development" => "dev.coincube.Connect",
        "production" => "io.coincube.Connect",
        _ => return Err("invalid_session_namespace"),
    };
    let entry = keyring::Entry::new(service, CONNECT_KEYRING_USER)
        .map_err(|_| "desktop_session_unavailable")?;
    let bytes = zeroize::Zeroizing::new(
        entry
            .get_secret()
            .map_err(|_| "desktop_session_unavailable")?,
    );
    serde_json::from_slice::<StoredSession>(&bytes)
        .map(|session| session.login)
        .map_err(|_| "desktop_session_invalid")
}

// ── Checkout state machine ──────────────────────────────────────────────────

/// Where a checkout is, one variant per BTCPay board (B1–B10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckoutPhase {
    /// Review step (board 3): plan, amount, dates covered and how to pay.
    /// Nothing has been sent to Connect yet.
    Review,
    /// Waiting for POST /checkout response (B1).
    Creating,
    /// Invoice received, awaiting payment (B2, B3). Lightning or on-chain is
    /// [`CheckoutState::network`].
    AwaitingPayment,
    /// The BTCPay page is open in the browser (B4). Polling keeps running.
    PayingInBrowser,
    /// Some sats arrived; the rest is due on the same address (B5).
    PartlyPaid { received_sats: u64, due_sats: u64 },
    /// Payment seen, waiting for confirmation (B6).
    Processing,
    /// Paid (B7). An overpayment activates too, with a support note
    /// (decision 14).
    Paid { overpaid_sats: u64 },
    /// Expired with nothing received (B8).
    Expired,
    /// Money arrived but can't be applied automatically: paid late, partly
    /// paid then expired, or invalid (B9, decision 13).
    NeedsSupport {
        invoice_id: String,
        received_sats: u64,
    },
    /// The invoice couldn't be created (B10). No invoice exists. `refused`
    /// is a 4xx from Connect (for example a plan change already scheduled):
    /// retrying the same request would be refused again, so the view offers
    /// Change plan instead of Try again.
    Failed { message: String, refused: bool },
    /// Polling stopped after errors. The invoice may still be paid; "Check
    /// again" resumes polling the same invoice instead of creating another.
    PollFailed(String),
}

impl CheckoutPhase {
    /// Phases in which the poll chain keeps running.
    pub fn polls(&self) -> bool {
        matches!(
            self,
            CheckoutPhase::AwaitingPayment
                | CheckoutPhase::PayingInBrowser
                | CheckoutPhase::PartlyPaid { .. }
                | CheckoutPhase::Processing
        )
    }
}

/// Lightning or on-chain on the Awaiting payment board (view state).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PaymentNetwork {
    #[default]
    Lightning,
    OnChain,
}

/// `bitcoin:` URI for `sats` to `address` (BIP 21), for the on-chain QR.
pub fn bip21_uri(address: &str, sats: u64) -> String {
    let whole = sats / 100_000_000;
    let frac = sats % 100_000_000;
    if frac == 0 {
        return format!("bitcoin:{address}?amount={whole}");
    }
    let frac = format!("{frac:08}");
    format!(
        "bitcoin:{address}?amount={whole}.{}",
        frac.trim_end_matches('0')
    )
}

/// The phase a charge-status poll moves a checkout to, whether the poll
/// chain continues, and whether the plan was paid. Pure, so every BTCPay
/// outcome is unit-testable.
pub fn next_checkout_phase(
    current: &CheckoutPhase,
    status: &ChargeStatusResponse,
) -> (CheckoutPhase, bool, bool) {
    match status.charge_state() {
        ChargeState::PartiallyPaid => (
            CheckoutPhase::PartlyPaid {
                received_sats: status.amount_received_sats,
                due_sats: status.amount_due_sats,
            },
            true,
            false,
        ),
        ChargeState::Processing => (CheckoutPhase::Processing, true, false),
        ChargeState::Paid => (
            CheckoutPhase::Paid {
                overpaid_sats: status.overpaid_sats,
            },
            false,
            true,
        ),
        ChargeState::Expired => (CheckoutPhase::Expired, false, false),
        ChargeState::NeedsSupport => (
            CheckoutPhase::NeedsSupport {
                invoice_id: status
                    .invoice_id
                    .clone()
                    .filter(|id| !id.is_empty())
                    .unwrap_or_else(|| status.charge_id.clone()),
                received_sats: status.amount_received_sats,
            },
            false,
            false,
        ),
        // Unpaid, or a state this build doesn't know: keep waiting, and
        // keep the page the user is on (browser or QR).
        ChargeState::Unpaid | ChargeState::Unknown => (
            match current {
                CheckoutPhase::PayingInBrowser
                | CheckoutPhase::PartlyPaid { .. }
                | CheckoutPhase::Processing => current.clone(),
                _ => CheckoutPhase::AwaitingPayment,
            },
            true,
            false,
        ),
    }
}

#[derive(Debug)]
pub struct CheckoutState {
    /// This checkout's generation. Every create and poll result carries it,
    /// so a result from a dismissed or replaced checkout is dropped.
    pub id: u64,
    /// The tier being bought (or reviewed).
    pub tier: PlanTier,
    /// The billing cycle snapshotted when Review opened; every invoice for
    /// this checkout (including "Create new invoice" after expiry) uses it.
    pub cycle: BillingCycle,
    /// `/connect/plan` as it was when Review opened. Review and the pay
    /// screens describe the purchase against it, so the Paid screen still
    /// says what was bought after the plan reloads with the new tier.
    pub plan: Option<ConnectPlan>,
    pub phase: CheckoutPhase,
    pub checkout: Option<CheckoutResponse>,
    pub lightning_qr: Option<qr_code::Data>,
    /// The on-chain QR: the full amount, or what is left once partly paid.
    pub onchain_qr: Option<qr_code::Data>,
    pub network: PaymentNetwork,
    /// The last charge status Connect returned (amounts, invoice id).
    pub last_status: Option<ChargeStatusResponse>,
    pub poll_errors: u8,
    /// The polling phase a `PollFailed` interrupted (Awaiting, Paying in
    /// browser, Partly paid or Processing). "Check again" restores it, so a
    /// partly paid invoice keeps its remaining amount and a seen payment
    /// isn't shown as unpaid again.
    pub resume_phase: Option<CheckoutPhase>,
    /// When Review opened. Covers dates for "starts now" purchases are
    /// computed from it, so they don't move while the invoice stays open.
    pub opened_at: chrono::DateTime<chrono::Utc>,
}

impl CheckoutState {
    /// A checkout at the Review step.
    pub fn new(id: u64, tier: PlanTier, cycle: BillingCycle, plan: Option<ConnectPlan>) -> Self {
        Self {
            id,
            tier,
            cycle,
            plan,
            phase: CheckoutPhase::Review,
            checkout: None,
            lightning_qr: None,
            onchain_qr: None,
            network: PaymentNetwork::Lightning,
            last_status: None,
            poll_errors: 0,
            resume_phase: None,
            opened_at: chrono::Utc::now(),
        }
    }
}

// ── Campaign code redemption (v2 campaign engine) ───────────────────────────

/// State for the generic "promo or referral code" field in Settings → Plan.
/// Campaign-agnostic — the desktop forwards the typed code and renders the
/// server's success/error message verbatim.
#[derive(Debug, Default)]
pub struct CampaignRedeemState {
    /// Current text in the code field.
    pub code: String,
    /// A redeem request is in flight.
    pub submitting: bool,
    /// Last outcome: `Ok(server message)` on success, `Err(server message)`
    /// on a typed failure (invalid / expired / exhausted / already-redeemed).
    /// `None` before any attempt.
    pub result: Option<Result<String, String>>,
}

// ── Plan lifecycle (renewal banner / expired state) ─────────────────────────

/// Number of days before `renewal_at` at which the pre-expiry renewal
/// banner begins showing (PLAN-billing-desktop D1).
pub const PLAN_RENEWAL_BANNER_DAYS: i64 = 7;

/// Highest pricing-schema version this build can fully render. A
/// `/connect/features` payload advertising a higher version triggers the
/// soft "update available" note in the plan picker (D4).
///
/// v2 (cubes terminology, `estate` tier, `IncludedLinkedParticipants`
/// dropped) is the current API schema and the model this build renders —
/// bumped from 1 so the desktop stops flagging the live pricing as
/// "newer than supported".
pub const SUPPORTED_PRICING_SCHEMA_VERSION: u32 = 2;

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

/// Public, non-secret Connect metadata for a Recovery services row. R03 may
/// use `target` for the authenticated launcher handoff; R10 owns the eventual
/// Backup & protection destination. This model never describes local secrets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryServiceTarget {
    pub cube_id: u64,
    pub network: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryKitReport {
    /// Connect reported a server-side Cube Recovery Kit record. This is not
    /// proof that every secret was backed up or that this device is verified.
    Reported,
    /// Older APIs also deserialize omitted `hasRecoveryKit` as false, so a
    /// false value cannot safely be displayed as "no backup".
    NotReported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryServiceRow {
    pub target: RecoveryServiceTarget,
    pub name: String,
    pub kit: RecoveryKitReport,
}

impl From<CubeResponse> for RecoveryServiceRow {
    fn from(cube: CubeResponse) -> Self {
        let name = cube.name.trim();
        Self {
            target: RecoveryServiceTarget {
                cube_id: cube.id,
                network: cube.network,
            },
            name: if name.is_empty() {
                format!("Cube {}", cube.id)
            } else {
                name.to_string()
            },
            kit: if cube.has_recovery_kit {
                RecoveryKitReport::Reported
            } else {
                RecoveryKitReport::NotReported
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryServicesState {
    NotLoaded,
    Loading,
    Ready(Vec<RecoveryServiceRow>),
    Failed(UserError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryAlertEligibility {
    Included,
    NotIncluded,
    Unknown,
}

/// One mainnet cube shown in the Duress intro screen's "set up a Cube
/// Recovery Kit for each Cube" checklist. Carries the kit halves and the
/// local vault/passkey shape so both the checklist badge and the
/// enrollment gate can reason about backup completeness (the duress vault
/// gate — PLAN-duress-vault-gate PR 1).
#[derive(Debug, Clone)]
pub struct DuressCube {
    /// Server-side cube id (from `list_cubes`), used to fetch this cube's
    /// per-cube recovery-kit status and to re-report `has_vault` (PR 3).
    pub server_id: u64,
    /// Server-side cube uuid; the join key against local `settings.json`
    /// Cubes (whose `id` is this uuid). `None` only in unit fixtures.
    pub uuid: Option<String>,
    pub name: String,
    /// Cube-level `has_recovery_kit` from `list_cubes` — whether *any* kit
    /// material exists. The per-half detail lives in `halves`.
    pub has_recovery_kit: bool,
    /// `(has_encrypted_seed, has_encrypted_wallet_descriptor)` from the
    /// per-cube recovery-kit status endpoint. `None` when the status fetch
    /// failed (or hasn't landed yet) — completeness is then `Unknown`,
    /// never coerced to `Complete` (master invariant I7).
    pub halves: Option<(bool, bool)>,
    /// Whether this Cube has a Vault wallet. `Some` from local
    /// `settings.json` (`vault_wallet_id.is_some()`) when this device holds
    /// the Cube; `None` for other-device Cubes until PR 3 lands the
    /// server-reported flag. A `None` here fails the gate closed.
    pub has_vault: Option<bool>,
    /// Whether this Cube is passkey-derived (seed unextractable on-device,
    /// so kits are descriptor-only). `Some` from local settings; `None` for
    /// other-device Cubes.
    pub is_passkey: Option<bool>,
    /// Whether this Cube was matched to a local `settings.json` Cube on this
    /// device (i.e. its vault/passkey shape is authoritative here).
    pub local: bool,
}

/// Backup completeness of a single Cube, the single source of truth shared
/// by the checklist badge and the enrollment gate
/// (PLAN-duress-vault-gate). Derived by [`cube_backup_completeness`] from
/// the Cube's shape (`has_vault`, `is_passkey`) and its kit halves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CubeBackupCompleteness {
    /// Everything a duress wipe would destroy is backed up.
    Complete,
    /// Vault Cube whose seed is backed up but Wallet Descriptor is missing.
    MissingDescriptor,
    /// Seed half missing (mnemonic Cube with no seed backup).
    MissingSeed,
    /// No recovery kit material at all.
    NoKit,
    /// Vault presence or kit status is unknown — never coerced to
    /// `Complete`; fails the gate closed (master invariant I7 / decision 4).
    Unknown,
}

/// The completeness rule (PLAN-duress-vault-gate "single source of truth").
/// Mirrors the shape semantics encoded in `recovery_kit_card`
/// (`app/view/settings/general.rs`): a seed-only kit is complete on a
/// vaultless Cube, and a Vault Cube needs both halves.
///
/// - `has_vault == None` → vault presence unknown → `Unknown`.
/// - `halves == None`    → status unavailable    → `Unknown`.
/// - `!has_vault`: `Complete` ⇔ `seed`.
/// - `has_vault`:  `Complete` ⇔ `seed && descriptor`.
///
/// # Why there is no longer a passkey arm (**I11**)
///
/// There used to be two, and they were the weakest rows in the table: a
/// passkey Cube with a Vault was `Complete ⇔ descriptor`, and a passkey Cube
/// without one was `Complete` unconditionally — both because a passkey Cube's
/// seed was held to be unextractable, so its Kit could only ever carry a
/// descriptor.
///
/// That is no longer true. A passkey Cube's Kit carries the encrypted master
/// seed, obtained from a re-authentication ceremony at Kit time
/// (`app/state/settings/recovery_kit.rs`). Platform passkeys sync only when
/// iCloud Keychain is enabled, which we can neither require nor detect, so
/// sync-based recovery cannot be *promised* to anyone — the Kit can, and it is
/// the only recovery a passkey user is owed.
///
/// The consequence is that Cube shape no longer changes the verdict at all, so
/// the parameter is gone rather than ignored. Two things follow, and both are
/// deliberate:
///
/// 1. **Other-device Cubes get a real answer.** `DuressCube::completeness`
///    used to evaluate an unknown-shape Cube both ways and fall back to
///    `Unknown` when they disagreed. They cannot disagree now.
/// 2. **A passkey Cube with no Kit seed reads incomplete.** Per **I13** that
///    is only safe because the Kit upload path can now produce one; flipping
///    this rule without that would mark every passkey Cube incomplete and
///    block Cube creation through `creation_gate::evaluate`.
pub fn cube_backup_completeness(
    has_vault: Option<bool>,
    halves: Option<(bool, bool)>,
) -> CubeBackupCompleteness {
    use CubeBackupCompleteness::*;
    // Vault presence unknown (other-device, pre-PR 3) → can't judge.
    let Some(has_vault) = has_vault else {
        return Unknown;
    };
    // Status not (yet) available → Unknown (never coerced to Complete).
    let Some((seed, descriptor)) = halves else {
        return Unknown;
    };
    if !has_vault {
        // Vaultless: a seed-only kit is complete.
        if seed {
            Complete
        } else if descriptor {
            MissingSeed
        } else {
            NoKit
        }
    } else {
        // Vault: needs both halves.
        match (seed, descriptor) {
            (true, true) => Complete,
            (true, false) => MissingDescriptor,
            (false, true) => MissingSeed,
            (false, false) => NoKit,
        }
    }
}

/// Fold a Cube's `/recovery-kit/status` into the duress checklist's
/// `(seed, descriptor)` halves. A half counts as backed up when EITHER
/// protection mode holds restorable material on Connect: the password kit's
/// ciphertext (`has_encrypted_seed` / `has_encrypted_wallet_descriptor`) OR an
/// owner-keychain ECIES envelope actually **sealed** for that artifact kind
/// (`ownerSelf.envelopeKinds` — envelope uploaded, not merely a recipient
/// registered, mirroring `backup_methods_present` in
/// `app/view/settings/general.rs`). The duress gate asks "is everything a wipe
/// would destroy restorable from Connect?", and a phone-sealed envelope
/// restores the same material as the password blob — so either satisfies it
/// (PLAN-duress-vault-gate; the checklist half PLAN-keychain-crk-status-fixes
/// missed, fixed 2026-07-22).
///
/// A sealed kind only counts when a recipient is also registered
/// (`has_recipient`), matching `derive_phone_recovery`'s restorability gate in
/// `home.rs`: phone recovery needs the phone key present, so kinds arriving
/// without a recipient are not restorable. Failing closed here keeps the gate
/// from reading "Complete" (and permitting enrollment) for material Connect
/// could not actually restore after a wipe.
pub fn duress_kit_halves(status: &RecoveryKitStatus) -> (bool, bool) {
    let sealed = |kind: &str| {
        status
            .owner_self
            .as_ref()
            .is_some_and(|o| o.has_recipient && o.envelope_kinds.iter().any(|k| k == kind))
    };
    (
        status.has_encrypted_seed || sealed("seed"),
        status.has_encrypted_wallet_descriptor || sealed("descriptor"),
    )
}

impl DuressCube {
    /// This Cube's backup completeness (shared by badge + gate).
    ///
    /// [`Self::is_passkey`] is not consulted. It used to be, and an
    /// other-device Cube whose shape was unknown had to be evaluated both ways
    /// and fall back to `Unknown` whenever the two readings disagreed — a
    /// passkey Vault Cube's kit was descriptor-only, so reading it as a
    /// mnemonic Cube misreported a complete kit as "Master Seed Phrase
    /// missing".
    ///
    /// Since **I11** a passkey Cube's Kit carries the seed, so the two
    /// readings are the same reading and the ambiguity is gone. Every Cube now
    /// gets a real verdict, whichever device it lives on.
    pub fn completeness(&self) -> CubeBackupCompleteness {
        cube_backup_completeness(self.has_vault, self.halves)
    }

    /// Whether this Cube blocks Tier-1 enrollment. Only Vault Cubes block:
    /// vaultless Cubes never block (master I2), a Vault Cube with an
    /// incomplete kit blocks, and a Cube whose vault presence itself is
    /// unknown fails closed (master decision 4).
    pub fn blocks_gate(&self) -> bool {
        match self.has_vault {
            // Vaultless → never blocks (I2). For an other-device Cube this is
            // the server-reported `false`, which — under monotonic reporting
            // (I8) — can't be distinguished from a Vault whose holder hasn't
            // reported `true` yet. Trusting it as vaultless is a *deliberate*
            // accepted limitation (master §9): treating it as unknown would
            // dead-end every seed-only other-device Cube, the exact I2 failure.
            // This-device Cubes carry authoritative local `has_vault`, so they
            // are unaffected; other-device coverage converges as holders sync.
            Some(false) => false,
            Some(true) => self.completeness() != CubeBackupCompleteness::Complete,
            None => true,
        }
    }
}

/// Whether any Cube in the checklist blocks Tier-1 enrollment (the hard
/// gate). Used by both the view (to disable the CTA) and the state (to
/// re-check on `StartEnrollment` in case a stale view fires it).
pub fn duress_gate_blocked(cubes: &[DuressCube]) -> bool {
    cubes.iter().any(DuressCube::blocks_gate)
}

/// Whether Tier-1 enrollment should be blocked given the (possibly
/// not-yet-loaded) checklist. **Fails closed on `None`**: until the cube
/// list + kit statuses have loaded we can't confirm every Vault Cube is
/// safe, so an unverified fleet blocks (master I7 / Resolved decision 4) —
/// a failed fetch also leaves `None` and stays blocked, with the Tier-2
/// bypass as the offline escape hatch. A loaded-but-empty fleet (`Some([])`)
/// does not block. Single source of truth for both the view (disable the
/// CTA) and the state (belt-and-suspenders re-check).
pub fn duress_tier1_gate_blocked(cubes: Option<&[DuressCube]>) -> bool {
    cubes.is_none_or(duress_gate_blocked)
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

// state for Verified Devices section of ConnectAccountPanel
#[derive(Debug, Default)]
pub struct VerifiedDevicesState {
    pub deleting_ids: std::collections::HashSet<u32>,
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

/// Terminal/in-flight state of the post-login duress gate ([`ConnectFlowStep::CheckingDuress`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuressGateStatus {
    /// `get_duress_state` is in flight (or retrying).
    Checking,
    /// Transient failure after exhausting retries (network / 5xx / rate-limit).
    /// The view offers Retry.
    Unreachable,
    /// The server returned a body this build can't decode. Auto-retry is
    /// suppressed (futile in the common case), but the view still offers a
    /// manual Retry — the mismatch may be a server bug that gets hotfixed while
    /// the app is open — plus an update hint and a Sign Out escape hatch.
    Incompatible,
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
    /// the dashboard is never shown to a possibly-in-duress account. `status`
    /// distinguishes in-flight from the two terminal failure modes so the view
    /// can show the right copy and the handler can stop retrying a futile
    /// (contract-mismatch) check. We fail CLOSED here (no dashboard) on every
    /// failure — the whole Connect dashboard is server-backed, so if this check
    /// is unreachable the dashboard is non-functional anyway, and a body we
    /// can't decode must never be read as "not in duress".
    CheckingDuress {
        status: DuressGateStatus,
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

/// Steps of the duress enrollment wizard. Duress is a paid (Pro/Estate) feature,
/// gated behind a complete, server-verified Recovery Kit for every Vault Cube
/// (the hard gate — `duress_tier1_gate_blocked`), so there is a single
/// enrollment path with no tier branching and no self-attestation step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuressEnrollStep {
    SetDuressPin,
    SetAllClear,
    SetCrkPassword,
    PickDelay,
    Confirm,
}

/// In-flight enrollment wizard state (Phases 2 & 8). `None` on the panel when
/// the wizard isn't open.
pub struct DuressEnrollState {
    pub step: DuressEnrollStep,
    pub duress_pin: String,
    /// Re-entry of `duress_pin` so a memorized typo is caught at enrollment.
    pub duress_pin_confirm: String,
    pub all_clear: String,
    pub crk_password: String,
    pub delay: crate::services::duress::enroll::DuressDelay,
    pub memorized: bool,
    pub submitting: bool,
    pub error: Option<String>,
    /// This device's generated duress code, held between SubmitEnrollment and a
    /// successful server EnrollResult. The code is NOT persisted locally until
    /// the server confirms enrollment, so a server failure can't leave a
    /// half-armed duress PIN on disk.
    pub pending_code: Option<String>,
}

// Hand-written so the duress PIN, all-clear passphrase, recovery-kit password
// and generated duress code cannot reach a log file through a `{:?}` on any
// enclosing struct. The derived impl printed all five in the clear.
impl std::fmt::Debug for DuressEnrollState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DuressEnrollState")
            .field("step", &self.step)
            .field("duress_pin", &"<redacted>")
            .field("duress_pin_confirm", &"<redacted>")
            .field("all_clear", &"<redacted>")
            .field("crk_password", &"<redacted>")
            .field("delay", &self.delay)
            .field("memorized", &self.memorized)
            .field("submitting", &self.submitting)
            .field("error", &self.error)
            .field(
                "pending_code",
                &self.pending_code.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

impl std::fmt::Debug for DuressDisableState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DuressDisableState")
            .field("pin", &"<redacted>")
            .field("method", &self.method)
            .field("submitting", &self.submitting)
            .field("error", &self.error)
            .finish()
    }
}

impl DuressEnrollState {
    /// Scrubs the in-memory secret fields (PINs, passphrases, and the generated
    /// duress code) so they don't linger on the heap after the wizard is torn
    /// down — e.g. on logout, where the rest of the session is reset.
    fn zeroize_secrets(&mut self) {
        use zeroize::Zeroize;
        self.duress_pin.zeroize();
        self.duress_pin_confirm.zeroize();
        self.all_clear.zeroize();
        self.crk_password.zeroize();
        if let Some(code) = self.pending_code.as_mut() {
            code.zeroize();
        }
    }
}

/// Step-up re-auth dialog for the "Disable Duress Mode" control (Issue 2).
/// `None` when the dialog is closed.
///
/// The factor demanded depends on the device: the regular Cube unlock PIN —
/// never the duress PIN — or, on a device whose Cubes are all passkey Cubes, a
/// fresh passkey assertion. See [`crate::app::DuressStepUpMethod`].
#[derive(Default)]
pub struct DuressDisableState {
    /// The regular Cube unlock PIN re-entered to authorize the disable. Unused
    /// on the passkey path.
    pub pin: String,
    /// Which factor this device can offer. `None` while the probe is in flight,
    /// which is why the dialog opens in a neutral "checking" state rather than
    /// showing a PIN field it might have to take away.
    pub method: Option<crate::app::DuressStepUpMethod>,
    /// True while the server `disable_duress` and the subsequent local disarm
    /// are in flight; disables the dialog's confirm button.
    pub submitting: bool,
    pub error: Option<String>,
}

impl DuressDisableState {
    /// Scrubs the re-entered PIN before the dialog is dropped, so it doesn't
    /// linger on the heap after the disable flow ends.
    fn zeroize_secrets(&mut self) {
        use zeroize::Zeroize;
        self.pin.zeroize();
    }
}

/// Which sub-view of the duress "Emergency contacts" section is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DuressContactsStep {
    /// The list of configured alert contacts (and the add affordance).
    #[default]
    List,
    /// The add/edit form. `editing_id` on [`DuressContactsState`]
    /// distinguishes add (`None`) from edit (`Some`).
    Form,
}

/// State for the duress "Emergency contacts" section within
/// `ConnectAccountPanel` (Estate Notifications — PR 1). Holds the loaded
/// contacts and the transient add/edit form. Estate-gated: the data is
/// only fetched when the account carries the `duress_alerts` entitlement,
/// and the section is only ever rendered in normal-mode settings — never
/// on the duress activation/cryptic screen.
#[derive(Debug, Default)]
pub struct DuressContactsState {
    pub step: DuressContactsStep,
    /// `None` until the first load lands; `Some(vec)` afterwards (possibly
    /// empty).
    pub contacts: Option<Vec<DuressAlertContact>>,
    pub loading: bool,
    pub error: Option<String>,
    /// `Some(id)` when the form is editing an existing contact, `None`
    /// when adding a new one.
    pub editing_id: Option<u64>,
    pub form_name: String,
    pub form_phone: String,
    pub form_email: String,
    pub form_ch_sms: bool,
    pub form_ch_whatsapp: bool,
    pub form_ch_email: bool,
    pub submitting: bool,
    /// Contact ids with an in-flight delete (disables the row's button).
    pub deleting_ids: std::collections::HashSet<u64>,
}

impl DuressContactsState {
    /// Drop all session state — the loaded contacts (names, phones, emails)
    /// and any in-flight add/edit form. Called on logout so one Connect
    /// session's duress contacts can't linger in memory or flash into the
    /// next user's Duress section before its reload lands.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Reset the form fields to empty and switch to "add" mode.
    fn reset_form_for_add(&mut self) {
        self.editing_id = None;
        self.form_name.clear();
        self.form_phone.clear();
        self.form_email.clear();
        self.form_ch_sms = false;
        self.form_ch_whatsapp = false;
        self.form_ch_email = false;
        self.error = None;
    }

    /// Populate the form from an existing contact for "edit" mode.
    fn load_form_from(&mut self, c: &DuressAlertContact) {
        self.editing_id = Some(c.id);
        self.form_name = c.display_name.clone();
        self.form_phone = c.phone.clone().unwrap_or_default();
        self.form_email = c.email.clone().unwrap_or_default();
        self.form_ch_sms = c.has_channel(DURESS_CHANNEL_SMS);
        self.form_ch_whatsapp = c.has_channel(DURESS_CHANNEL_WHATSAPP);
        self.form_ch_email = c.has_channel(DURESS_CHANNEL_EMAIL);
        self.error = None;
    }

    /// Number of configured contacts (0 while unloaded).
    pub fn count(&self) -> usize {
        self.contacts.as_ref().map(|c| c.len()).unwrap_or(0)
    }

    /// True when the per-account cap is reached — the view hides the add
    /// affordance and the update handler refuses a create.
    pub fn at_cap(&self) -> bool {
        self.count() >= MAX_DURESS_ALERT_CONTACTS
    }
}

/// A failure on the Connect panel: the copy to show, and what the card's
/// "Try again" should re-run.
///
/// Splitting the two matters because they are decided in different places. The
/// copy comes from the error itself (`UserError`), which knows whether retrying
/// could possibly help; the *target* is only known by the call site that
/// failed. Wiring the button to one fixed message instead — which is what the
/// card originally did — gave a button that silently did nothing on the
/// dashboard and one that navigated away from the code-entry screen.
#[derive(Debug, Clone)]
pub struct PanelError {
    pub user: UserError,
    retry: Option<RetryAction>,
}

impl PanelError {
    /// A failure the card shows without a button: either retrying cannot help,
    /// or the screen already has its own control for re-running the action.
    pub fn settled(user: impl Into<UserError>) -> Self {
        Self {
            user: user.into(),
            retry: None,
        }
    }

    /// A failure the card can re-run itself.
    pub fn retryable(user: impl Into<UserError>, action: RetryAction) -> Self {
        Self {
            user: user.into(),
            retry: Some(action),
        }
    }

    /// The button to render, if any.
    ///
    /// A target alone is not enough: the error must also be one where re-running
    /// the same request could plausibly succeed. A 401 carrying
    /// `RetryAction::Session` still gets no button, because the refresh would be
    /// refused identically.
    pub fn retry_button(&self) -> Option<(&'static str, ConnectAccountMessage)> {
        self.retry
            .filter(|_| self.user.retryable)
            .map(|action| ("Try again", ConnectAccountMessage::Retry(action)))
    }
}

pub struct ConnectAccountPanel {
    /// Saved installation preference, supplied by Home/App and refreshed across tabs.
    pub(crate) bitcoin_blake2b_opt_in: bool,
    // An admitted fork startup carries its exact client instead of restoring
    // potentially unrelated saved credentials from the global keyring.
    admitted_client: bool,
    // Home may borrow a sibling tab's accepted session without binding a
    // Cube backend to it. It still does not own the shared keyring entry.
    shared_client: bool,
    admitted_user_loading: bool,
    admitted_reopen_required: bool,

    pub step: ConnectFlowStep,
    pub active_sub: ConnectSubMenu,
    pub client: CoincubeClient,
    pub user: Option<User>,
    pub plan: Option<ConnectPlan>,
    pub verified_devices: Option<Vec<VerifiedDevice>>,
    pub verified_devices_state: VerifiedDevicesState,
    pub login_activity: Option<Vec<LoginActivity>>,
    /// Inline account actions awaiting the user's final choice.
    pub review_sign_out: bool,
    pub review_device_removal: Option<u32>,
    /// Account Overview summary counts, fetched on entering the Overview
    /// tab (kept separate from `contacts_state` so the count doesn't
    /// disturb the Contacts tab's own load/step state). `None` until the
    /// first successful fetch (and again after logout) — the row stays
    /// hidden until then. A *later* failed refresh deliberately keeps the
    /// last successful value rather than reverting to `None`, so a
    /// transient error doesn't blank the row (see the
    /// `OverviewCountsLoaded` handler).
    pub overview_contact_count: Option<usize>,
    pub overview_cube_count: Option<usize>,
    pub recovery_services: RecoveryServicesState,
    overview_seq: u64,
    pub contacts_state: ContactsState,
    /// Presentation-ready failure for this panel's error card, with the retry
    /// it should offer. Never a raw error string — see [`crate::user_error`].
    pub error: Option<PanelError>,
    /// A stored session that couldn't be checked because there was no
    /// network or Connect didn't answer. The session stays in the keyring
    /// and is re-checked with backoff; nobody is signed out for it.
    pub unverified: Option<UnverifiedSession>,
    /// Connect, not the user, signed this computer out. The GUI mirrors it
    /// to every tab; sign-in or Not now clears it.
    pub signed_out_by: Option<crate::app::SignedOutBy>,
    /// Incremented on each login/logout so stale async completions can be discarded.
    session_generation: u64,
    /// Increments only after this panel accepts a login completion and
    /// installs its bearer. GUI uses it to broadcast after acceptance.
    accepted_auth_revision: u64,
    /// Per-tab authentication epoch. Every authentication operation this
    /// panel spawns — a keyring refresh, an OTP verification, the
    /// admitted-user load — captures it, and the completion it produces
    /// (`SetSession`, `SessionLoaded`, `AdmittedUserLoaded`) carries it. The
    /// App advances it at a global Connect auth invalidation
    /// (`App::invalidate_claim_session`), so a completion of an operation
    /// begun before that invalidation can be told from one begun after it,
    /// whatever order they arrive in. Not `session_generation`: that one is
    /// advanced by `SessionLoaded` itself, so it cannot say when the
    /// operation began. A completion whose epoch is older than the current
    /// one installs nothing here — no session, no keyring write, no client
    /// token, no user — and a refresh failure from such an operation logs
    /// nothing out (`stale_completion`).
    auth_epoch: u64,
    /// App panels persist an auth result before adopting it locally. Home
    /// panels retain their existing keyring-only completion path.
    defer_auth_cache_commit: bool,
    auth_commit_pending: bool,
    /// Invalidates delayed cooldown ticks and resend completions when the
    /// user leaves token entry or begins another OTP attempt.
    otp_attempt_generation: u64,
    // ── Plan & Billing ──
    /// Cached plan features from GET /connect/features.
    pub features: Option<FeaturesResponse>,
    /// The currently selected billing cycle for the upgrade cards.
    pub selected_billing_cycle: BillingCycle,
    /// The plan row picked on Change plan; `None` until the user picks one
    /// (the view then pre-selects the current tier).
    pub selected_plan_tier: Option<PlanTier>,
    /// Active checkout flow (None when no checkout in progress).
    pub checkout: Option<CheckoutState>,
    /// Last checkout generation handed out (see [`CheckoutState::id`]).
    checkout_seq: u64,
    /// Sequence of the latest billing plan reload; an older response that
    /// lands after a newer one is dropped (`PlanReloaded`).
    plan_seq: u64,
    /// Billing history entries.
    pub billing_history: Option<Vec<BillingHistoryEntry>>,
    /// Whether the billing history sub-view is shown.
    pub show_billing_history: bool,
    /// Explicit secondary comparison and referral details within management.
    pub show_plan_comparison: bool,
    pub show_campaign_redeem: bool,
    /// Active duress enrollment wizard (Phases 2 & 8); `None` when closed.
    pub duress_enroll: Option<DuressEnrollState>,
    /// A submitting wizard was hidden by leaving Duress. Retain its secrets
    /// only until the server result can arm this device or reject enrollment.
    pub duress_enroll_abandoned: bool,
    /// Active "Disable Duress Mode" step-up dialog (Issue 2); `None` when closed.
    pub duress_disable: Option<DuressDisableState>,
    /// Duress "Emergency contacts" section (Estate Notifications — PR 1).
    pub duress_contacts: DuressContactsState,
    /// Mainnet cubes + recovery-kit status for the Duress intro screen's
    /// per-Cube recovery-kit checklist. `None` until the first fetch on
    /// entering the Duress tab. Only mainnet cubes are tracked — testnet
    /// cubes hold no value, so their backup state doesn't matter here.
    pub duress_cubes: Option<Vec<DuressCube>>,
    /// Monotonic token identifying the latest `load_duress_cubes` fetch.
    /// `session_generation` only changes on login/logout, so two concurrent
    /// checklist fetches in one session share it — an older in-flight fetch
    /// could land after a newer one and overwrite fresh data (e.g. a
    /// pre-Vault-creation fetch clobbering the post-creation reload, reopening
    /// the Tier-1 gate). Each `reload`/`invalidate` bumps this; `CubesLoaded`
    /// applies only when it still matches, so only the newest fetch wins.
    duress_cubes_seq: u64,
    /// Server-side duress state (enrolled / active), loaded on entering the
    /// Duress tab so the screen reflects the enabled state instead of the
    /// setup flow once duress is enrolled. `None` until the first fetch.
    pub duress_state: Option<crate::services::coincube::DuressState>,
    /// Whether THIS device has actually armed the duress PIN on its Cubes
    /// (mirrors `DuressLocalState.enrolled`). Distinct from the server's
    /// account-level `enrolled`: only this can honestly say "armed on this
    /// device", since the account can be enrolled elsewhere or after a failed
    /// local persist. Refreshed on entering the Duress tab and set on a
    /// successful local persist (`EnrollmentPersisted`).
    pub duress_locally_armed: bool,
    /// Whether the user dismissed the pre-expiry renewal banner this
    /// session (D1). Not persisted — re-shows on next launch while the
    /// plan is still within its renewal window.
    pub renewal_banner_dismissed: bool,
    /// Settings → Plan promo/referral code field (v2 campaign engine).
    pub campaign_redeem: CampaignRedeemState,
    /// Optional promo/referral code typed on the account-creation screen.
    pub register_campaign_code: String,
    /// Code captured at registration, redeemed once the new session
    /// authenticates (the redeem endpoint requires a token). Consumed in
    /// `SessionLoaded`.
    pub pending_campaign_code: Option<String>,
}

impl ConnectAccountPanel {
    pub fn new() -> Self {
        ConnectAccountPanel {
            bitcoin_blake2b_opt_in: false,
            admitted_client: false,
            shared_client: false,
            admitted_user_loading: false,
            admitted_reopen_required: false,
            step: ConnectFlowStep::CheckingSession,
            active_sub: ConnectSubMenu::Overview,
            client: CoincubeClient::new(),
            user: None,
            plan: None,
            verified_devices: None,
            verified_devices_state: VerifiedDevicesState::default(),
            login_activity: None,
            review_sign_out: false,
            review_device_removal: None,
            overview_contact_count: None,
            overview_cube_count: None,
            recovery_services: RecoveryServicesState::NotLoaded,
            overview_seq: 0,
            contacts_state: ContactsState::new(),
            error: None,
            unverified: None,
            signed_out_by: None,
            session_generation: 0,
            accepted_auth_revision: 0,
            auth_epoch: 0,
            defer_auth_cache_commit: false,
            auth_commit_pending: false,
            otp_attempt_generation: 0,
            features: None,
            selected_billing_cycle: BillingCycle::Monthly,
            selected_plan_tier: None,
            checkout: None,
            checkout_seq: 0,
            plan_seq: 0,
            billing_history: None,
            show_billing_history: false,
            show_plan_comparison: false,
            show_campaign_redeem: false,
            duress_enroll: None,
            duress_enroll_abandoned: false,
            duress_disable: None,
            duress_contacts: DuressContactsState::default(),
            duress_cubes: None,
            duress_cubes_seq: 0,
            duress_state: None,
            duress_locally_armed: false,
            renewal_banner_dismissed: false,
            campaign_redeem: CampaignRedeemState::default(),
            register_campaign_code: String::new(),
            pending_campaign_code: None,
        }
    }

    pub(crate) fn defer_auth_cache_commit(&mut self) {
        self.defer_auth_cache_commit = true;
    }

    pub(crate) fn can_begin_verify_otp(&self) -> bool {
        matches!(
            self.step,
            ConnectFlowStep::OtpVerification { sending: false, .. }
        ) && !self.admitted_client
            && !self.admitted_reopen_required
    }

    /// True when duress mode is enrolled — either armed on this device or
    /// reported enrolled by the server (the account can be enrolled on
    /// another device). Drives the post-Vault-creation Recovery-Kit nudge
    /// (PLAN-duress-vault-gate PR 3 / master decision 6): a Vault created
    /// after enrollment needs its Wallet Descriptor added to the kit.
    pub fn is_duress_enrolled(&self) -> bool {
        self.duress_locally_armed
            || self
                .duress_state
                .as_ref()
                .map(|s| s.enrolled)
                .unwrap_or(false)
    }

    /// True when the account's plan includes Duress Mode (the paid `duress`
    /// entitlement, set by the server for Pro and Estate). Gates the entire
    /// duress enrollment surface — the panel CTA, the checklist fetch, and the
    /// server-state fetch — so a Free account never reaches enrollment. The
    /// codebase gates on this entitlement bool, not a `PlanTier` name check.
    pub fn is_duress_entitled(&self) -> bool {
        self.plan
            .as_ref()
            .map(|p| p.entitlements.duress)
            .unwrap_or(false)
    }

    /// True when the account carries the Estate-only `duress_alerts`
    /// entitlement. Gates both the Emergency-contacts data load and the
    /// management UI (non-entitled accounts get the locked affordance).
    pub fn is_duress_alerts_entitled(&self) -> bool {
        self.plan
            .as_ref()
            .map(|p| p.entitlements.duress_alerts)
            .unwrap_or(false)
    }

    /// True when the account carries the `recovery_alerts` entitlement. Gates
    /// the Vault Recovery Alerts card's **alerts toggle** and the heartbeat
    /// reporting. After the recovery-alerts cleanup (API PR 3) the server
    /// returns this on all plans, so it becomes universally true; the desktop
    /// keeps the check as defense-in-depth. See `PLAN-recovery-alerts-cleanup.md`.
    pub fn is_recovery_alerts_entitled(&self) -> bool {
        self.plan
            .as_ref()
            .map(|p| p.entitlements.recovery_alerts)
            .unwrap_or(false)
    }

    /// True when the account carries the Estate-only `inheritanceEscrow`
    /// entitlement — the server-blind ECIES recovery kit sealed to keyholders.
    /// Gates the "What keyholders can recover" tier selector on the Recovery
    /// Alerts card, separately from the (universal) alerts toggle. Fails closed
    /// against an older API that omits the field. See
    /// `PLAN-recovery-alerts-cleanup.md` PR 2 + ADDENDUM.
    pub fn is_inheritance_escrow_entitled(&self) -> bool {
        self.plan
            .as_ref()
            .map(|p| p.entitlements.inheritance_escrow)
            .unwrap_or(false)
    }

    /// Reset the Emergency-contacts section to its list view and reload
    /// from the API. No-op (returns an empty task) when the account isn't
    /// Estate-entitled, so we never fire a request that would just 403.
    pub fn reload_duress_contacts(&mut self) -> iced::Task<Message> {
        if !self.is_duress_alerts_entitled() {
            return iced::Task::none();
        }
        self.duress_contacts.step = DuressContactsStep::List;
        self.duress_contacts.error = None;
        self.duress_contacts.loading = true;
        load_duress_alert_contacts(&self.client, self.session_generation)
    }

    /// Reload the mainnet cube list + recovery-kit status for the Duress
    /// intro screen's per-Cube recovery-kit checklist. No-op for accounts
    /// without the duress entitlement (the checklist isn't rendered for
    /// them, so the fetch would be wasted). Best-effort: a failed fetch
    /// leaves the list empty and the screen falls back to generic copy.
    pub fn reload_duress_cubes(&mut self) -> iced::Task<Message> {
        if !self.is_duress_entitled() {
            return iced::Task::none();
        }
        // Bump the load token so any older in-flight fetch is discarded when it
        // lands (its seq will no longer match) — only this newest fetch wins.
        self.duress_cubes_seq += 1;
        load_duress_cubes(&self.client, self.session_generation, self.duress_cubes_seq)
    }

    /// Invalidate the cached duress checklist so the vault gate fails closed
    /// until a fresh fetch lands. Called when a Vault is created mid-session
    /// (PLAN-duress-vault-gate PR 3): the cached `duress_cubes` may still
    /// show this Cube as vaultless/complete, and the Tier-1 gate reads that
    /// cache — dropping it to `None` makes `duress_tier1_gate_blocked` block
    /// (master I7) until `reload_duress_cubes` repopulates with the Cube's
    /// now-present Vault (whose descriptor isn't backed up yet → blocks).
    ///
    /// Also bumps the load token so any fetch already in flight (which read
    /// the *pre-invalidation* state — e.g. a pre-Vault-creation checklist) is
    /// discarded when it lands, instead of racing back over the invalidation
    /// and reopening the gate.
    pub fn invalidate_duress_cubes(&mut self) {
        self.duress_cubes = None;
        self.duress_cubes_seq += 1;
    }

    /// Reload server-side duress state (enrolled / active) for the Duress
    /// intro screen, so it can show the enabled state instead of the setup
    /// flow. No-op for accounts without the duress entitlement. Best-effort:
    /// a failed fetch leaves the state `None` and the setup flow is shown.
    ///
    /// Gated on the entitlement only — deliberately **not** on the full
    /// [`Self::show_duress`] gate. This fetch is what *discovers* `enrolled`
    /// for an account enrolled on another device, and `show_duress()` depends
    /// on `enrolled`; gating it on the gate would be circular and would strand
    /// a grandfathered account (flag off, enrolled elsewhere) with the surface
    /// hidden forever. The surface itself is gated elsewhere.
    pub fn reload_duress_state(&mut self) -> iced::Task<Message> {
        if !self.is_duress_entitled() {
            return iced::Task::none();
        }
        // Refresh the authoritative LOCAL arm signal (whether THIS device wrote
        // Cube duress hashes) from `DuressLocalState`. Server `enrolled` is
        // account-wide and can be true on a device that never armed, so the
        // "armed on this device" view keys off this, not the server. A read
        // failure resolves to `false` — the safe direction (never falsely
        // claim "armed here").
        self.duress_locally_armed = crate::dir::CoincubeDirectory::active()
            .ok()
            .and_then(|dir| crate::services::duress::DuressLocalState::load(dir.path()).ok())
            .map(|st| st.enrolled)
            .unwrap_or(false);
        load_duress_state(&self.client, self.session_generation)
    }

    /// Reflect a just-*persisted* duress enrollment in the locally-cached
    /// `duress_state` so the Duress intro flips from the setup flow to the
    /// enabled state immediately. Called only from the `EnrollmentPersisted`
    /// handler — i.e. after `persist_duress_enrollment` has actually armed the
    /// duress PIN on every Cube — never on the optimistic pre-persist path,
    /// where a later collision / disk failure would otherwise leave a false
    /// "enabled" view with no Cube armed. Connect tiers would otherwise keep
    /// showing setup until the next server fetch, and Sovereign tiers never
    /// fetch at all. Preserves `active`/`unlock_at` if a prior fetch populated
    /// them (e.g. another device already enrolled the account).
    fn mark_duress_enrolled(&mut self) {
        match &mut self.duress_state {
            Some(s) => {
                s.enrolled = true;
                s.this_device_registered = true;
            }
            None => {
                self.duress_state = Some(crate::services::coincube::DuressState {
                    active: false,
                    unlock_at: None,
                    enrolled: true,
                    this_device_registered: true,
                });
            }
        }
    }

    /// A local node does not bind its lifetime to the admitted Connect bearer.
    pub(crate) fn use_local_backend(&mut self) {
        self.admitted_client = false;
        self.shared_client = false;
        self.admitted_reopen_required = false;
    }

    /// Discard this tab's account memory without touching the shared keyring.
    pub(crate) fn detach_local_session(&mut self) {
        self.admitted_client = true; // clear_session must not delete another tab's new login.
        self.clear_session();
        self.admitted_reopen_required = false;
        self.step = ConnectFlowStep::Login {
            email: String::new(),
            loading: false,
        };
        self.invalidate_auth();
    }

    pub fn revoke_admitted_client(&mut self) {
        if self.admitted_client {
            self.clear_session();
        }
        // App calls this after auth failure too, when clear_session has already
        // dropped the admitted flag. Revocation is sticky for this Cube instance.
        self.admitted_reopen_required = true;
    }

    pub fn requires_authenticated_reopen(&self) -> bool {
        self.admitted_reopen_required
    }

    pub fn install_admitted_client(&mut self, client: CoincubeClient) {
        self.session_generation = self.session_generation.wrapping_add(1);
        // The prior session's async enrollment result is now stale. Scrub its
        // retained credentials and release the abandoned-wizard gate before
        // the replacement account can enter Duress.
        self.clear_duress_enroll();
        self.client = client;
        self.admitted_client = true;
        self.shared_client = false;
        self.admitted_reopen_required = false;
        self.admitted_user_loading = false;
        // An adopted session supersedes one that was waiting on the network.
        self.unverified = None;
        self.user = None;
        self.plan = None;
        self.features = None;
        self.step = ConnectFlowStep::CheckingSession;
    }

    /// Home accepts this session from another tab. It must avoid a second
    /// keyring refresh, but signing out must leave Home free to sign in again.
    pub(crate) fn install_shared_client(&mut self, client: CoincubeClient) {
        self.install_admitted_client(client);
        self.shared_client = true;
    }

    /// The Home tab where the user pressed Sign out clears the global saved
    /// session. Other tabs only detach, since they might see a newer login.
    pub(crate) fn forget_shared_session_for_logout(&self) {
        if self.shared_client {
            self.clear_keyring_session();
        }
    }

    /// Returns a clone of the authenticated client (with JWT set).
    /// Used by ConnectCubePanel to make API calls.
    pub fn authenticated_client(&self) -> Option<CoincubeClient> {
        if !self.admitted_reopen_required && (self.user.is_some() || self.admitted_client) {
            Some(self.client.clone())
        } else {
            None
        }
    }

    pub fn is_authenticated(&self) -> bool {
        !self.admitted_reopen_required && matches!(self.step, ConnectFlowStep::Dashboard)
    }

    /// Session outcome not known yet: the initial keyring probe, a refresh or
    /// login in flight (`Init` parks a stored-session refresh in
    /// `Login { loading: true }`), or the post-auth duress gate. Terminal duress
    /// failures and a required reopen are settled, so they read as signed out.
    pub fn is_checking_session(&self) -> bool {
        !self.admitted_reopen_required
            && matches!(
                self.step,
                ConnectFlowStep::CheckingSession
                    | ConnectFlowStep::Login { loading: true, .. }
                    | ConnectFlowStep::CheckingDuress {
                        status: DuressGateStatus::Checking
                    }
            )
    }

    /// A Home tab may reuse a sibling's accepted session only before it owns
    /// a client or starts its own refresh. Replacing an in-flight refresh with
    /// an older sibling token would create another rotation race.
    pub(crate) fn can_adopt_shared_session(&self) -> bool {
        !self.admitted_reopen_required
            && self.authenticated_client().is_none()
            && !matches!(self.step, ConnectFlowStep::Login { loading: true, .. })
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

    pub(crate) fn accepted_auth_revision(&self) -> u64 {
        self.accepted_auth_revision
    }

    /// See the `auth_epoch` field.
    pub fn auth_epoch(&self) -> u64 {
        self.auth_epoch
    }

    /// Advance the authentication epoch: every operation spawned from here
    /// on carries the new value. Returns it. Called by the App at a global
    /// Connect auth invalidation.
    pub fn invalidate_auth(&mut self) -> u64 {
        self.auth_epoch = self.auth_epoch.wrapping_add(1);
        self.auth_commit_pending = false;
        self.auth_epoch
    }

    /// Whether a completion carrying `epoch` comes from an operation begun
    /// before the most recent invalidation. Such a completion is dropped
    /// whole: after a sign-out, or a newer sign-in, it must neither install
    /// the old credentials nor relabel the account the newer operation
    /// established.
    fn stale_completion(&self, epoch: u64, what: &str) -> bool {
        if epoch < self.auth_epoch {
            log::info!(
                "[CONNECT] dropping {what} from an authentication operation begun before the last invalidation (epoch {epoch} < {})",
                self.auth_epoch
            );
            true
        } else {
            false
        }
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

    /// Fire `POST /connect/campaigns/redeem` for `code` and route the
    /// outcome to `CampaignRedeemed`. Shared by the Settings → Plan field
    /// and the deferred post-auth redeem of an account-creation code. The
    /// success message is server-authored; a failure carries the server's
    /// typed error message, rendered verbatim (no campaign knowledge).
    fn redeem_campaign_task(&self, code: String) -> iced::Task<Message> {
        let gen = self.session_generation;
        let client = self.client.clone();
        iced::Task::perform(
            async move {
                client
                    .redeem_campaign(&code)
                    .await
                    .map(|resp| resp.message.unwrap_or_else(|| "Code applied.".to_string()))
                    .map_err(|e| e.to_string())
            },
            move |result| {
                Message::View(view::Message::ConnectAccount(
                    ConnectAccountMessage::CampaignRedeemed(result, gen),
                ))
            },
        )
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

    /// Fetch the Account Overview summary counts (contacts + cubes) on
    /// demand when the Overview tab is opened. Cheap and best-effort —
    /// a failed fetch just leaves the count `None` and the row hidden.
    pub fn reload_overview(&mut self) -> iced::Task<Message> {
        self.recovery_services = RecoveryServicesState::Loading;
        self.overview_seq = self.overview_seq.wrapping_add(1);
        load_overview_counts(
            &self.client,
            self.session_generation,
            self.overview_seq,
            self.contacts_state.active_network.clone(),
        )
    }

    /// Plan entitlement is account-level eligibility only. It does not prove
    /// that alerts are enabled on any individual Cube.
    pub fn recovery_alert_eligibility(&self) -> RecoveryAlertEligibility {
        match self.plan.as_ref() {
            Some(plan) if plan.entitlements.recovery_alerts => RecoveryAlertEligibility::Included,
            Some(_) => RecoveryAlertEligibility::NotIncluded,
            None => RecoveryAlertEligibility::Unknown,
        }
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

    /// `epoch` is the one the `SetSession` that produced this session
    /// carried; the `SessionLoaded` it queues inherits it.
    fn post_login_tasks(
        &mut self,
        session: StoredSession,
        epoch: u64,
        new_login: bool,
    ) -> iced::Task<Message> {
        self.save_session_to_keyring(&session);
        self.client.set_token(&session.login.token);
        self.accepted_auth_revision = self.accepted_auth_revision.wrapping_add(1);

        let user = session.login.user.clone();
        // A true login bootstraps the signer device and realtime stream.
        // Refresh only persists the bearer for the existing session; it
        // must leave an About registration claim and device ID untouched.
        let loaded = iced::Task::done(Message::View(view::Message::ConnectAccount(
            ConnectAccountMessage::SessionLoaded {
                user,
                plan: None,
                epoch,
            },
        )));
        if !new_login {
            return loaded;
        }
        iced::Task::batch([
            loaded,
            iced::Task::done(Message::InAppConnectLoginCompleted {
                token: session.login.token.clone(),
                refresh_token: session.login.refresh_token.clone(),
                email: session.login.user.email.clone(),
                epoch,
            }),
        ])
    }

    pub fn update_message(&mut self, msg: ConnectAccountMessage) -> iced::Task<Message> {
        if self.admitted_reopen_required {
            return iced::Task::none();
        }
        // This failure belongs to a keyring refresh queued before Home
        // accepted a sibling's bearer. It has no epoch, so drop it here.
        if self.shared_client && matches!(msg, ConnectAccountMessage::RefreshFailed(_)) {
            return iced::Task::none();
        }
        if self.admitted_client
            && matches!(
                &msg,
                ConnectAccountMessage::RefreshSession { .. }
                    | ConnectAccountMessage::SetSession(..)
                    | ConnectAccountMessage::OtpVerified(..)
                    | ConnectAccountMessage::RefreshedSession(..)
                    | ConnectAccountMessage::AuthCommitAccepted { .. }
                    | ConnectAccountMessage::AuthCommitRejected { .. }
                    | ConnectAccountMessage::SessionLoaded { .. }
                    | ConnectAccountMessage::EmailChanged(_)
                    | ConnectAccountMessage::SubmitLogin
                    | ConnectAccountMessage::SubmitRegistration
                    | ConnectAccountMessage::CreateAccount
                    | ConnectAccountMessage::OtpRequested { .. }
                    | ConnectAccountMessage::OtpChanged(_)
                    | ConnectAccountMessage::ResendOtp
                    | ConnectAccountMessage::VerifyOtp
                    | ConnectAccountMessage::EmailNotVerified { .. }
            )
        {
            // An auth operation queued before a sibling session was adopted
            // must never revoke that newer session (or replace its token).
            let completion_epoch = match &msg {
                ConnectAccountMessage::SetSession(_, epoch)
                | ConnectAccountMessage::OtpVerified(_, epoch, _)
                | ConnectAccountMessage::RefreshedSession(_, epoch, _)
                | ConnectAccountMessage::AuthCommitAccepted { epoch, .. }
                | ConnectAccountMessage::AuthCommitRejected { epoch }
                | ConnectAccountMessage::SessionLoaded { epoch, .. } => Some(*epoch),
                _ => None,
            };
            if completion_epoch.is_some_and(|epoch| self.stale_completion(epoch, "auth completion"))
                || self.shared_client
            {
                return iced::Task::none();
            }
            // A replacement login needs backend admission, not merely a new
            // panel token. Refuse before OTP I/O or global keyring persistence.
            self.revoke_admitted_client();
            return iced::Task::none();
        }
        self.dispatch_message(msg)
    }

    fn dispatch_message(&mut self, msg: ConnectAccountMessage) -> iced::Task<Message> {
        match msg {
            ConnectAccountMessage::Init => {
                if self.admitted_client {
                    if self.user.is_some() || self.admitted_user_loading {
                        return iced::Task::none();
                    }
                    self.admitted_user_loading = true;
                    let client = self.client.clone();
                    let generation = self.session_generation;
                    let epoch = self.auth_epoch;
                    return iced::Task::perform(
                        async move {
                            client
                                .get_user()
                                .await
                                .map_err(|error| ((&error).into(), error.is_auth_error()))
                        },
                        move |user| {
                            Message::View(view::Message::ConnectAccount(
                                ConnectAccountMessage::AdmittedUserLoaded {
                                    user,
                                    generation,
                                    epoch,
                                },
                            ))
                        },
                    );
                }
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
                // The session is already loaded and its post-sign-in duress
                // gate is running. A refresh now would abandon that gate and
                // mint another session, whose sign-in broadcast makes Home
                // re-adopt and re-announce, which sends this Init again: the
                // tab would never reach the dashboard.
                if matches!(
                    self.step,
                    ConnectFlowStep::CheckingDuress {
                        status: DuressGateStatus::Checking
                    }
                ) {
                    return iced::Task::none();
                }
                // Clear any previous failure. Past the re-entry guards above,
                // this path always either starts a refresh or falls back to the
                // login form, and both make the old card stale: this is also
                // the "Try again" button, so leaving it up while the retry runs
                // makes a working retry look like a no-op — and leaving it up
                // on the no-session fallback strands it on screen with nothing
                // left to retry.
                self.error = None;
                // A code stashed during an abandoned signup must not ride into
                // a restored or next session — it belongs to that signup only.
                // Clearing here (and in SubmitLogin) confines redemption to the
                // uninterrupted signup → verify → SessionLoaded path.
                self.pending_campaign_code = None;
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

            ConnectAccountMessage::AdmittedUserLoaded {
                user,
                generation,
                epoch,
            } => {
                if !self.admitted_client
                    || generation != self.session_generation
                    || self.stale_completion(epoch, "AdmittedUserLoaded")
                {
                    return iced::Task::none();
                }
                self.admitted_user_loading = false;
                match user {
                    Ok(user) => {
                        return self.dispatch_message(ConnectAccountMessage::SessionLoaded {
                            user,
                            plan: None,
                            epoch,
                        })
                    }
                    Err((error, auth_error)) => {
                        if auth_error {
                            let logout = self.update_message(ConnectAccountMessage::LogOut);
                            return iced::Task::batch([
                                iced::Task::done(Message::ConnectSignedOutBy(
                                    crate::app::SignedOutBy::SessionEnded,
                                )),
                                logout,
                            ]);
                        }
                        self.error = Some(PanelError::retryable(error, RetryAction::Session));
                    }
                }
            }

            ConnectAccountMessage::RefreshSession { refresh_token } => {
                let client = self.client.clone();
                let epoch = self.auth_epoch;
                let previous_refresh_token = refresh_token.clone();
                return iced::Task::perform(
                    async move { client.refresh_login(&refresh_token).await },
                    move |res| match res {
                        Ok(login) => Message::View(view::Message::ConnectAccount(
                            ConnectAccountMessage::RefreshedSession(
                                login,
                                epoch,
                                previous_refresh_token,
                            ),
                        )),
                        Err(e) => {
                            if e.is_auth_error() {
                                // Whether this logs out is decided against the
                                // epoch current when it arrives, not here.
                                Message::View(view::Message::ConnectAccount(
                                    ConnectAccountMessage::RefreshRejected { epoch },
                                ))
                            } else if let Some(trouble) = refresh_trouble(&e) {
                                log::warn!("[CONNECT] Session refresh didn't reach Connect: {}", e);
                                Message::View(view::Message::ConnectAccount(
                                    ConnectAccountMessage::RefreshUnreachable { trouble, epoch },
                                ))
                            } else {
                                Message::View(view::Message::ConnectAccount(
                                    ConnectAccountMessage::RefreshFailed((&e).into()),
                                ))
                            }
                        }
                    },
                );
            }

            ConnectAccountMessage::RefreshRejected { epoch } => {
                if self.stale_completion(epoch, "a refresh's authentication failure") {
                    return iced::Task::none();
                }
                // A current refresh was refused: the session is gone. Dispatched
                // as the message it always was, so the GUI broadcasts it, after
                // the notice that Connect ended it.
                return iced::Task::batch([
                    iced::Task::done(Message::ConnectSignedOutBy(
                        crate::app::SignedOutBy::SessionEnded,
                    )),
                    iced::Task::done(Message::View(view::Message::ConnectAccount(
                        ConnectAccountMessage::LogOut,
                    ))),
                ]);
            }

            ConnectAccountMessage::RefreshUnreachable { trouble, epoch } => {
                if self.stale_completion(epoch, "a refresh that didn't reach Connect") {
                    return iced::Task::none();
                }
                // Keep the stored session (L1): leave the sign-in form idle,
                // remember whose session it is, and check it again later.
                let Some(email) = self
                    .load_session_from_keyring()
                    .map(|session| session.login.user.email)
                else {
                    self.step = ConnectFlowStep::Login {
                        email: String::new(),
                        loading: false,
                    };
                    return iced::Task::none();
                };
                let attempts = self.unverified.as_ref().map_or(0, |u| u.attempts) + 1;
                let unverified = UnverifiedSession {
                    email,
                    trouble,
                    attempts,
                };
                // The Launcher's and the Cube's subscriptions send
                // `RecheckSession` after `backoff()`, only while mounted.
                self.unverified = Some(unverified);
                self.error = None;
                self.step = ConnectFlowStep::Login {
                    email: String::new(),
                    loading: false,
                };
            }

            ConnectAccountMessage::RecheckSession => {
                // Signed in or out meanwhile: nothing to re-check.
                if self.unverified.is_none()
                    || !matches!(self.step, ConnectFlowStep::Login { loading: false, .. })
                {
                    return iced::Task::none();
                }
                return self.update_message(ConnectAccountMessage::Init);
            }

            ConnectAccountMessage::RefreshFailed(err) => {
                log::warn!("[CONNECT] Session refresh failed (transient): {}", err);
                // The card renders its own "Try again" button, wired to
                // `Init` — which re-reads the keyring session and re-fires the
                // refresh. Previously this string ended in "Tap to retry" while
                // the banner had no button at all.
                self.error = Some(PanelError::retryable(err, RetryAction::Session));
                // Not a network wait any more: show the retryable error and
                // stop the automatic re-checks.
                self.unverified = None;
                self.scrub_recovery_passphrase();
                self.step = ConnectFlowStep::Login {
                    email: String::new(),
                    loading: false,
                };
            }

            ConnectAccountMessage::SetSession(login, epoch) => {
                if self.stale_completion(epoch, "SetSession") {
                    return iced::Task::none();
                }
                let session = StoredSession { login };
                return self.post_login_tasks(session, epoch, true);
            }

            ConnectAccountMessage::OtpVerified(login, epoch, attempt) => {
                if self.stale_completion(epoch, "OtpVerified")
                    || attempt != self.otp_attempt_generation
                    || !matches!(
                        self.step,
                        ConnectFlowStep::OtpVerification { sending: true, .. }
                    )
                {
                    return iced::Task::none();
                }
                if self.defer_auth_cache_commit {
                    if self.auth_commit_pending {
                        return iced::Task::none();
                    }
                    self.auth_commit_pending = true;
                    return iced::Task::done(Message::InAppConnectAuthReady {
                        login,
                        epoch,
                        previous_refresh_token: None,
                    });
                }
                return self.post_login_tasks(StoredSession { login }, epoch, true);
            }

            ConnectAccountMessage::OtpVerifyFailed(error, epoch, attempt) => {
                if self.stale_completion(epoch, "OtpVerifyFailed")
                    || attempt != self.otp_attempt_generation
                    || !matches!(self.step, ConnectFlowStep::OtpVerification { .. })
                {
                    return iced::Task::none();
                }
                return self.update_message(ConnectAccountMessage::Error(error));
            }

            ConnectAccountMessage::RefreshedSession(login, epoch, previous_refresh_token) => {
                if self.stale_completion(epoch, "RefreshedSession") {
                    return iced::Task::none();
                }
                if self.defer_auth_cache_commit {
                    if self.auth_commit_pending {
                        return iced::Task::none();
                    }
                    self.auth_commit_pending = true;
                    return iced::Task::done(Message::InAppConnectAuthReady {
                        login,
                        epoch,
                        previous_refresh_token: Some(previous_refresh_token),
                    });
                }
                return self.post_login_tasks(StoredSession { login }, epoch, false);
            }

            ConnectAccountMessage::AuthCommitAccepted {
                login,
                epoch,
                refreshed,
            } => {
                if self.stale_completion(epoch, "AuthCommitAccepted") {
                    return iced::Task::none();
                }
                if !self.auth_commit_pending {
                    return iced::Task::none();
                }
                self.auth_commit_pending = false;
                return self.post_login_tasks(StoredSession { login }, epoch, !refreshed);
            }

            ConnectAccountMessage::AuthCommitRejected { epoch } => {
                if self.stale_completion(epoch, "AuthCommitRejected") {
                    return iced::Task::none();
                }
                if !self.auth_commit_pending {
                    return iced::Task::none();
                }
                self.auth_commit_pending = false;
                self.error = Some(PanelError::retryable(
                    UserError::new(
                        "Connect session changed",
                        "Sign in again to use the current account.",
                        crate::user_error::CC_API_BADRESP,
                        true,
                    ),
                    RetryAction::Session,
                ));
                self.step = ConnectFlowStep::Login {
                    email: String::new(),
                    loading: false,
                };
            }

            // A completion of an operation begun before the last invalidation
            // installs nothing (the App reads the epoch too, for its claim
            // hold); a current one is handled the same whichever operation
            // produced it.
            ConnectAccountMessage::SessionLoaded { user, plan, epoch } => {
                if self.stale_completion(epoch, "SessionLoaded") {
                    return iced::Task::none();
                }
                self.session_generation += 1;
                self.user = Some(user);
                self.plan = plan;
                // Phase 6: gate on the duress check FIRST — don't reveal the
                // dashboard until we've confirmed the account isn't in duress.
                self.step = ConnectFlowStep::CheckingDuress {
                    status: DuressGateStatus::Checking,
                };
                self.error = None;
                // Fetch features + gate on duress in background (non-blocking).
                let gen = self.session_generation;
                let c2 = self.client.clone();
                let c3 = self.client.clone();
                let mut tasks = vec![
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
                ];
                // Redeem an account-creation promo/referral code now that the
                // session is authenticated. Best-effort: a bad code never
                // blocks sign-in — the outcome surfaces in Settings → Plan.
                if let Some(code) = self.pending_campaign_code.take() {
                    // Defer the plan fetch to the redeem (which fetches on
                    // either outcome). Firing get_connect_plan here *and* again
                    // after redemption races two PlanLoaded responses at the
                    // same generation, and a slow pre-redeem fetch could
                    // overwrite the granted tier/provenance.
                    self.campaign_redeem.submitting = true;
                    // Surface the code in the Settings → Plan field so that, if
                    // the redeem fails, the error reads against the code and
                    // the user can edit/retry without retyping. Cleared on
                    // success by `CampaignRedeemed`.
                    self.campaign_redeem.code = code.clone();
                    tasks.push(self.redeem_campaign_task(code));
                } else {
                    let c1 = self.client.clone();
                    tasks.push(iced::Task::perform(
                        async move { (c1.get_connect_plan().await.ok(), gen) },
                        |(plan, g)| {
                            Message::View(view::Message::ConnectAccount(
                                ConnectAccountMessage::PlanLoaded(plan, g),
                            ))
                        },
                    ));
                }
                return iced::Task::batch(tasks);
            }

            ConnectAccountMessage::PlanLoaded(plan, gen) => {
                if gen == self.session_generation && plan.is_some() {
                    self.apply_loaded_plan(plan);
                }
            }

            ConnectAccountMessage::PlanReloaded(plan, gen, seq) => {
                // Only the newest billing reload may set the plan: one sent
                // before a payment must not land over one sent after it.
                if gen == self.session_generation && seq == self.plan_seq && plan.is_some() {
                    self.apply_loaded_plan(plan);
                }
            }

            ConnectAccountMessage::OverviewCountsLoaded {
                contacts,
                cubes,
                recovery,
                generation,
                request_seq,
            } => {
                if generation == self.session_generation && request_seq == self.overview_seq {
                    // Only overwrite a field when its fetch succeeded, so a
                    // partial failure doesn't blank out a previously-loaded
                    // count.
                    if contacts.is_some() {
                        self.overview_contact_count = contacts;
                    }
                    if cubes.is_some() {
                        self.overview_cube_count = cubes;
                    }
                    self.recovery_services = match recovery {
                        Ok(cubes) => RecoveryServicesState::Ready(
                            cubes.into_iter().map(RecoveryServiceRow::from).collect(),
                        ),
                        Err(error) => RecoveryServicesState::Failed(error),
                    };
                }
            }

            ConnectAccountMessage::RefreshRecoveryServices => {
                if self.user.is_some() && matches!(self.step, ConnectFlowStep::Dashboard) {
                    return self.reload_overview();
                }
            }

            // The Launcher owns Cube navigation. An in-Cube Connect panel
            // cannot open another Cube or grant unlock authority itself.
            ConnectAccountMessage::OpenRecoveryService(_) => {}
            ConnectAccountMessage::ReviewSignOut(show) => {
                self.review_sign_out = show && self.user.is_some();
            }

            ConnectAccountMessage::LogOut => {
                let was_logged_in = self.user.is_some();
                self.clear_session();
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
                // A login must never redeem a code stashed for a (different)
                // new account during an abandoned signup. See `Init`.
                self.pending_campaign_code = None;

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
                                    ConnectAccountMessage::Error((&e).into()),
                                ))
                            }
                        }
                    },
                );
            }

            ConnectAccountMessage::SubmitRegistration => {
                // Stash any account-creation promo/referral code now; it's
                // redeemed once the new session authenticates (the redeem
                // endpoint requires a token). Empty → nothing to redeem.
                self.pending_campaign_code = {
                    let trimmed = self.register_campaign_code.trim();
                    (!trimmed.is_empty()).then(|| trimmed.to_string())
                };
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
                            ConnectAccountMessage::Error((&e).into()),
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
                self.otp_attempt_generation = self.otp_attempt_generation.wrapping_add(1);
                let attempt = self.otp_attempt_generation;

                self.step = ConnectFlowStep::OtpVerification {
                    email,
                    otp: String::new(),
                    sending: false,
                    is_signup,
                    cooldown: 60,
                };
                return iced::Task::done(Message::View(view::Message::ConnectAccount(
                    ConnectAccountMessage::OtpCooldownTick(attempt),
                )));
            }

            ConnectAccountMessage::OtpChanged(otp) => {
                if let ConnectFlowStep::OtpVerification { otp: o, .. } = &mut self.step {
                    *o = otp;
                }
            }

            ConnectAccountMessage::BackToEmail => {
                if let ConnectFlowStep::OtpVerification {
                    email, is_signup, ..
                } = &self.step
                {
                    self.otp_attempt_generation = self.otp_attempt_generation.wrapping_add(1);
                    self.step = if *is_signup {
                        ConnectFlowStep::Register {
                            email: email.clone(),
                            loading: false,
                        }
                    } else {
                        ConnectFlowStep::Login {
                            email: email.clone(),
                            loading: false,
                        }
                    };
                    self.error = None;
                }
            }

            ConnectAccountMessage::OtpCooldownTick(attempt) => {
                if attempt != self.otp_attempt_generation {
                    return iced::Task::none();
                }
                if let ConnectFlowStep::OtpVerification { cooldown, .. } = &mut self.step {
                    if *cooldown > 0 {
                        *cooldown -= 1;
                        return iced::Task::perform(
                            async { tokio::time::sleep(std::time::Duration::from_secs(1)).await },
                            move |_| {
                                Message::View(view::Message::ConnectAccount(
                                    ConnectAccountMessage::OtpCooldownTick(attempt),
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
                let attempt = self.otp_attempt_generation;
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
                    move |res| match res {
                        Ok(()) => Message::View(view::Message::ConnectAccount(
                            ConnectAccountMessage::OtpResent(attempt),
                        )),
                        Err(e) => Message::View(view::Message::ConnectAccount(
                            ConnectAccountMessage::OtpResendFailed((&e).into(), attempt),
                        )),
                    },
                );
            }

            ConnectAccountMessage::OtpResent(attempt) => {
                if attempt != self.otp_attempt_generation {
                    return iced::Task::none();
                }
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
                    self.otp_attempt_generation = self.otp_attempt_generation.wrapping_add(1);
                    return iced::Task::done(Message::View(view::Message::ConnectAccount(
                        ConnectAccountMessage::OtpCooldownTick(self.otp_attempt_generation),
                    )));
                }
            }

            ConnectAccountMessage::OtpResendFailed(error, attempt) => {
                if attempt == self.otp_attempt_generation
                    && matches!(self.step, ConnectFlowStep::OtpVerification { .. })
                {
                    return self.update_message(ConnectAccountMessage::Error(error));
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
                if *sending {
                    return iced::Task::none();
                }
                *sending = true;
                self.otp_attempt_generation = self.otp_attempt_generation.wrapping_add(1);
                let attempt = self.otp_attempt_generation;
                let req = OtpVerifyRequest {
                    email: email.clone(),
                    otp: otp.clone(),
                };
                let is_signup = *is_signup;
                let client = self.client.clone();
                let epoch = self.auth_epoch;
                return iced::Task::perform(
                    async move {
                        if is_signup {
                            client.signup_verify_otp(req).await
                        } else {
                            client.login_verify_otp(req).await
                        }
                    },
                    move |res| match res {
                        Ok(login) => Message::View(view::Message::ConnectAccount(
                            ConnectAccountMessage::OtpVerified(login, epoch, attempt),
                        )),
                        Err(e) => Message::View(view::Message::ConnectAccount(
                            ConnectAccountMessage::OtpVerifyFailed((&e).into(), epoch, attempt),
                        )),
                    },
                );
            }

            ConnectAccountMessage::EmailNotVerified { email } => {
                self.otp_attempt_generation = self.otp_attempt_generation.wrapping_add(1);
                let attempt = self.otp_attempt_generation;
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
                    move |res| match res {
                        Ok(()) => Message::View(view::Message::ConnectAccount(
                            ConnectAccountMessage::OtpResent(attempt),
                        )),
                        Err(e) => Message::View(view::Message::ConnectAccount(
                            ConnectAccountMessage::OtpResendFailed((&e).into(), attempt),
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

            ConnectAccountMessage::ReviewVerifiedDeviceRemoval(id) => {
                self.review_device_removal = id.filter(|id| {
                    self.verified_devices
                        .as_ref()
                        .is_some_and(|devices| devices.iter().any(|d| d.id == *id && !d.is_current))
                });
            }

            ConnectAccountMessage::DeleteVerifiedDevice(id) => {
                if self.review_device_removal != Some(id) {
                    return iced::Task::none();
                }
                self.review_device_removal = None;
                self.verified_devices_state.deleting_ids.insert(id);

                let gen = self.session_generation;
                let client = self.client.clone();
                let id_for_result = id;

                return iced::Task::perform(
                    async move { client.delete_verified_device(id).await },
                    move |res| {
                        Message::View(view::Message::ConnectAccount(
                            ConnectAccountMessage::VerifiedDeviceDeleted(
                                id_for_result,
                                gen,
                                res.map_err(|e| e.to_string()),
                            ),
                        ))
                    },
                );
            }

            ConnectAccountMessage::VerifiedDeviceDeleted(id, gen, result) => {
                if gen != self.session_generation {
                    return iced::Task::none();
                }

                self.verified_devices_state.deleting_ids.remove(&id);

                match result {
                    Ok(()) => {
                        if let Some(devices) = self.verified_devices.as_mut() {
                            devices.retain(|d| d.id != id);
                        }
                    }
                    Err(err) => {
                        return iced::Task::done(Message::View(view::Message::ConnectAccount(
                            ConnectAccountMessage::Error(UserError::logged(
                                "Couldn't remove that device",
                                "Try again. If it keeps failing, contact support and quote the reference below.",
                                crate::user_error::CC_API_BADRESP,
                                true,
                                err,
                            )),
                        )));
                    }
                }
            }

            ConnectAccountMessage::CopyToClipboard(text) => {
                return iced::clipboard::write(text);
            }

            // ── Plan & Billing ──────────────────────────────────────────
            ConnectAccountMessage::FeaturesLoaded(features, gen) => {
                if gen == self.session_generation {
                    self.features = features;
                    self.leave_hidden_duress_route();
                }
            }

            ConnectAccountMessage::FocusAction(id) => {
                return iced::widget::operation::focus(id);
            }

            ConnectAccountMessage::BillingCycleSelected(cycle) => {
                self.selected_billing_cycle = cycle;
            }

            ConnectAccountMessage::SelectPlanTier(tier) => {
                self.selected_plan_tier = Some(tier);
            }

            ConnectAccountMessage::ReviewPlanChange(tier) => {
                // Review is the step before any invoice exists. It is only
                // reachable while purchasing is open, like StartCheckout, so
                // a stale message never shows a "Create invoice" the API
                // would reject.
                // Switching to Free buys nothing, so its Review is shown
                // even while purchasing is closed.
                if tier != PlanTier::Free && !self.purchasing_enabled() {
                    log::warn!("[CONNECT] ReviewPlanChange ignored — purchasing disabled");
                    return iced::Task::none();
                }
                // Never replace an invoice that is already out for payment.
                if self
                    .checkout
                    .as_ref()
                    .is_some_and(|cs| !matches!(cs.phase, CheckoutPhase::Review))
                {
                    return iced::Task::none();
                }
                self.active_sub = ConnectSubMenu::PlanBilling;
                self.show_billing_history = false;
                self.show_plan_comparison = false;
                // Back from Review lands on the same picked row.
                self.selected_plan_tier = Some(tier.clone());
                self.checkout_seq += 1;
                self.checkout = Some(CheckoutState::new(
                    self.checkout_seq,
                    tier,
                    self.selected_billing_cycle,
                    self.plan.clone(),
                ));
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
                // Never replace an invoice that is out for payment (a double
                // press, or a stale message): only Review, Expired and Failed
                // may start a new one.
                if self.checkout.as_ref().is_some_and(|cs| {
                    !matches!(
                        cs.phase,
                        CheckoutPhase::Review
                            | CheckoutPhase::Expired
                            | CheckoutPhase::Failed { .. }
                    )
                }) {
                    log::warn!("[CONNECT] StartCheckout ignored — an invoice is in progress");
                    return iced::Task::none();
                }
                // The cycle reviewed for this tier wins over the picker's
                // current toggle, so a retry after expiry re-issues the same
                // plan the user confirmed.
                let cycle = self
                    .checkout
                    .as_ref()
                    .filter(|cs| cs.tier == tier)
                    .map(|cs| cs.cycle)
                    .unwrap_or(self.selected_billing_cycle);
                // A new generation: anything still in flight for the
                // previous invoice (an expired one, a failed create) is
                // dropped when it lands.
                self.checkout_seq += 1;
                let id = self.checkout_seq;
                // Keep the plan snapshot taken when Review opened.
                let snapshot = self
                    .checkout
                    .as_ref()
                    .filter(|cs| cs.tier == tier)
                    .and_then(|cs| cs.plan.clone())
                    .or_else(|| self.plan.clone());
                let opened_at = self
                    .checkout
                    .as_ref()
                    .filter(|cs| cs.tier == tier)
                    .map(|cs| cs.opened_at);
                let mut checkout = CheckoutState::new(id, tier.clone(), cycle, snapshot);
                if let Some(opened_at) = opened_at {
                    checkout.opened_at = opened_at;
                }
                checkout.phase = CheckoutPhase::Creating;
                self.checkout = Some(checkout);
                let gen = self.session_generation;
                let client = self.client.clone();
                let req = CheckoutRequest {
                    plan: tier,
                    billing_cycle: cycle,
                };
                return iced::Task::perform(
                    async move {
                        client.create_checkout(req).await.map_err(|e| {
                            use crate::services::coincube::CoincubeError;
                            let refused = matches!(
                                &e,
                                CoincubeError::Unsuccessful(info)
                                    if (400..500).contains(&info.status_code)
                                        && info.status_code != 401
                                        && info.status_code != 408
                                        && info.status_code != 429
                            );
                            (e.to_string(), refused)
                        })
                    },
                    move |result| {
                        Message::View(view::Message::ConnectAccount(
                            ConnectAccountMessage::CheckoutCreated(result, gen, id),
                        ))
                    },
                );
            }

            ConnectAccountMessage::CheckoutCreated(result, gen, id) => {
                if gen != self.session_generation || !self.is_current_checkout(id) {
                    return iced::Task::none();
                }
                match result {
                    Ok(resp) => {
                        let qr = qr_code::Data::new(&resp.lightning_invoice).ok();
                        let onchain =
                            qr_code::Data::new(bip21_uri(&resp.on_chain_address, resp.amount_sats))
                                .ok();
                        let cs = self.checkout.as_mut().expect("checked above");
                        cs.phase = CheckoutPhase::AwaitingPayment;
                        cs.checkout = Some(resp);
                        cs.lightning_qr = qr;
                        cs.onchain_qr = onchain;
                        cs.poll_errors = 0;
                        return iced::Task::done(Message::View(view::Message::ConnectAccount(
                            ConnectAccountMessage::PollChargeStatus(id),
                        )));
                    }
                    Err((message, refused)) => {
                        if let Some(cs) = &mut self.checkout {
                            cs.phase = CheckoutPhase::Failed { message, refused };
                        }
                    }
                }
            }

            ConnectAccountMessage::PollChargeStatus(id) => {
                let should_poll = self
                    .checkout
                    .as_ref()
                    .is_some_and(|cs| cs.id == id && cs.phase.polls());
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
                            ConnectAccountMessage::ChargeStatusUpdated(result, gen, id),
                        ))
                    },
                );
            }

            ConnectAccountMessage::ChargeStatusUpdated(result, gen, id) => {
                // Only the current checkout's poll chain may move it: a poll
                // in flight from a dismissed or replaced invoice is dropped.
                if gen != self.session_generation || !self.is_current_checkout(id) {
                    return iced::Task::none();
                }
                let poll = iced::Task::done(Message::View(view::Message::ConnectAccount(
                    ConnectAccountMessage::PollChargeStatus(id),
                )));
                let cs = self.checkout.as_mut().expect("checked above");
                if !cs.phase.polls() {
                    return iced::Task::none();
                }
                match result {
                    Ok(status) => {
                        cs.poll_errors = 0;
                        let (phase, keep_polling, paid) = next_checkout_phase(&cs.phase, &status);
                        if let CheckoutPhase::PartlyPaid { due_sats, .. } = &phase {
                            // Same address, the remaining amount.
                            if let Some(resp) = cs.checkout.as_ref() {
                                cs.onchain_qr = qr_code::Data::new(bip21_uri(
                                    &resp.on_chain_address,
                                    *due_sats,
                                ))
                                .ok();
                            }
                            cs.network = PaymentNetwork::OnChain;
                        }
                        cs.phase = phase;
                        cs.last_status = Some(status);
                        if paid {
                            // Refresh the plan and the history the receipt
                            // lands in.
                            self.billing_history = None;
                            return self.reload_plan_billing();
                        }
                        if keep_polling {
                            return poll;
                        }
                    }
                    Err((e, terminal)) => {
                        if terminal {
                            log::error!("[CONNECT] Charge poll terminal error: {}", e);
                            cs.resume_phase = Some(cs.phase.clone());
                            cs.phase = CheckoutPhase::PollFailed(e);
                        } else {
                            cs.poll_errors += 1;
                            if cs.poll_errors >= 3 {
                                log::error!(
                                    "[CONNECT] Charge poll failed after {} retries: {}",
                                    cs.poll_errors,
                                    e
                                );
                                cs.resume_phase = Some(cs.phase.clone());
                                cs.phase = CheckoutPhase::PollFailed(e);
                            } else {
                                log::warn!(
                                    "[CONNECT] Charge poll error ({}/3): {}",
                                    cs.poll_errors,
                                    e
                                );
                                return poll;
                            }
                        }
                    }
                }
            }

            ConnectAccountMessage::SetReviewCycle(cycle) => {
                if let Some(cs) = self
                    .checkout
                    .as_mut()
                    .filter(|cs| cs.phase == CheckoutPhase::Review)
                {
                    cs.cycle = cycle;
                    self.selected_billing_cycle = cycle;
                }
            }

            ConnectAccountMessage::SelectPaymentNetwork(network) => {
                if let Some(cs) = self.checkout.as_mut() {
                    cs.network = network;
                }
            }

            ConnectAccountMessage::ShowInvoiceHere => {
                if let Some(cs) = self
                    .checkout
                    .as_mut()
                    .filter(|cs| cs.phase == CheckoutPhase::PayingInBrowser)
                {
                    cs.phase = CheckoutPhase::AwaitingPayment;
                }
            }

            ConnectAccountMessage::ResumeChargePolling => {
                if let Some(cs) = self
                    .checkout
                    .as_mut()
                    .filter(|cs| matches!(cs.phase, CheckoutPhase::PollFailed(_)))
                {
                    // Back to the phase polling stopped in (never a fresh
                    // "send the full amount" after a partial payment).
                    cs.phase = cs
                        .resume_phase
                        .take()
                        .filter(CheckoutPhase::polls)
                        .unwrap_or(CheckoutPhase::AwaitingPayment);
                    cs.poll_errors = 0;
                    let id = cs.id;
                    return iced::Task::done(Message::View(view::Message::ConnectAccount(
                        ConnectAccountMessage::PollChargeStatus(id),
                    )));
                }
            }

            ConnectAccountMessage::DismissCheckout => {
                self.checkout = None;
            }

            ConnectAccountMessage::OpenCheckoutUrl(url) => {
                // B4: the BTCPay page is open. The poll chain keeps running
                // (PayingInBrowser polls), so this screen still updates.
                if let Some(cs) = self
                    .checkout
                    .as_mut()
                    .filter(|cs| cs.phase == CheckoutPhase::AwaitingPayment)
                {
                    cs.phase = CheckoutPhase::PayingInBrowser;
                }
                return iced::Task::done(Message::View(view::Message::OpenUrl(url)));
            }

            ConnectAccountMessage::DismissRenewalBanner => {
                // Per-session only — `renewal_banner_dismissed` resets on
                // logout and isn't persisted, so the banner re-shows on the
                // next launch while the plan is still in-window.
                self.renewal_banner_dismissed = true;
            }

            ConnectAccountMessage::OpenPlanBilling => {
                // The existing Duress CTA says "View plans"; preserve its
                // comparison destination without touching the Duress view.
                let from_plan_gate = matches!(self.active_sub, ConnectSubMenu::Duress);
                self.active_sub = ConnectSubMenu::PlanBilling;
                self.show_billing_history = false;
                self.show_plan_comparison = from_plan_gate;
            }

            ConnectAccountMessage::ComparePlans(show) => {
                // Leaving Review (or a finished Expired / Failed checkout)
                // returns to Change plan; an invoice that is still out for
                // payment is never dropped this way.
                if self.checkout.as_ref().is_some_and(|cs| {
                    matches!(
                        cs.phase,
                        CheckoutPhase::Review
                            | CheckoutPhase::Expired
                            | CheckoutPhase::Failed { .. }
                    )
                }) {
                    self.checkout = None;
                }
                self.active_sub = ConnectSubMenu::PlanBilling;
                self.show_billing_history = false;
                self.show_plan_comparison = show;
            }

            ConnectAccountMessage::ShowCampaignRedeem(show) => {
                self.show_campaign_redeem = show;
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
                self.show_plan_comparison = false;
                // A Free/lapsed plan has no tier to pre-fill an invoice for
                // — fall back to the picker.
                if matches!(tier, PlanTier::Free) {
                    self.show_plan_comparison = true;
                    return iced::Task::none();
                }
                if let Some(cycle) = cycle {
                    self.selected_billing_cycle = cycle;
                }
                // Renew goes through Review (board 1 "Renew Pro" → board 3)
                // so the amount and covered dates are shown before an
                // invoice is created.
                return iced::Task::done(Message::View(view::Message::ConnectAccount(
                    ConnectAccountMessage::ReviewPlanChange(tier),
                )));
            }

            ConnectAccountMessage::ToggleBillingHistory => {
                self.show_billing_history = !self.show_billing_history;
                if self.show_billing_history {
                    self.show_plan_comparison = false;
                    let gen = self.session_generation;
                    let c2 = self.client.clone();
                    return iced::Task::batch([
                        self.reload_billing_history(),
                        iced::Task::perform(
                            async move {
                                match c2.get_user().await {
                                    Ok(u) => Message::View(view::Message::ConnectAccount(
                                        ConnectAccountMessage::UserProfileLoaded(u, gen),
                                    )),
                                    Err(e) => Message::View(view::Message::ConnectAccount(
                                        ConnectAccountMessage::UserProfileFailed((&e).into(), gen),
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
                // No card button: every sender of this variant is an action the
                // user started from a control that is still on screen (Continue,
                // Verify, Resend, Remove). That control is the retry.
                self.error = Some(PanelError::settled(error_msg));

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
                        Err(user) => {
                            // Retryability and the `Ref:` come from the error
                            // itself. Hardcoding them meant a 401 or a plan-gate
                            // 403 — both perfectly likely on a *billing* screen —
                            // rendered "check your internet connection" over a
                            // "Try again" button that could only ever reproduce
                            // the same refusal.
                            self.error =
                                Some(PanelError::retryable(user, RetryAction::BillingHistory));
                        }
                    }
                }
            }
            ConnectAccountMessage::UserProfileLoaded(user, gen) => {
                // Non-auth profile refresh - only update user, no step/session changes
                if gen == self.session_generation {
                    self.user = Some(user);
                }
            }
            ConnectAccountMessage::UserProfileFailed(error, gen) => {
                // Non-auth error - just show error, don't redirect to login.
                //
                // Generation-gated like its success sibling: a profile fetch
                // that loses its race with a logout or a re-login must not put
                // a card — with a live "Try again" — on the session that
                // replaced it. `clear_session` bumps the counter precisely so
                // in-flight results can be dropped.
                if gen == self.session_generation {
                    self.error = Some(PanelError::retryable(error, RetryAction::UserProfile));
                }
            }

            ConnectAccountMessage::SecurityDataFailed(error, gen) => {
                // Same guard as `VerifiedDevicesLoaded` / `LoginActivityLoaded`,
                // which this is the failure half of.
                if gen == self.session_generation {
                    self.error = Some(PanelError::retryable(error, RetryAction::SecurityData));
                }
            }

            ConnectAccountMessage::Retry(action) => {
                // The card is gone the moment its button is pressed: leaving it
                // up while the retry runs makes a working retry look like a
                // dead button.
                self.error = None;
                return match action {
                    RetryAction::Session => {
                        // `Init` re-reads the keyring session and re-fires the
                        // refresh, and already guards against re-entry while one
                        // is in flight.
                        self.update_message(ConnectAccountMessage::Init)
                    }
                    RetryAction::SecurityData => {
                        load_security_data(&self.client, self.session_generation)
                    }
                    RetryAction::UserProfile => {
                        let gen = self.session_generation;
                        let client = self.client.clone();
                        iced::Task::perform(
                            async move {
                                match client.get_user().await {
                                    Ok(u) => Message::View(view::Message::ConnectAccount(
                                        ConnectAccountMessage::UserProfileLoaded(u, gen),
                                    )),
                                    Err(e) => Message::View(view::Message::ConnectAccount(
                                        ConnectAccountMessage::UserProfileFailed((&e).into(), gen),
                                    )),
                                }
                            },
                            |m| m,
                        )
                    }
                    RetryAction::BillingHistory => {
                        let gen = self.session_generation;
                        let client = self.client.clone();
                        iced::Task::perform(
                            async move { client.get_billing_history().await },
                            move |res| match res {
                                Ok(history) => Message::View(view::Message::ConnectAccount(
                                    ConnectAccountMessage::BillingHistoryLoaded(Ok(history), gen),
                                )),
                                Err(e) => Message::View(view::Message::ConnectAccount(
                                    ConnectAccountMessage::BillingHistoryLoaded(
                                        Err((&e).into()),
                                        gen,
                                    ),
                                )),
                            },
                        )
                    }
                };
            }

            // ── Campaign code redemption (v2 campaign engine) ─────────────
            ConnectAccountMessage::CampaignCodeChanged(code) => {
                // Ignore edits while a redeem is in flight — otherwise the
                // response for the submitted code could land against
                // newly-typed text (and a success would clear it). The view
                // also disables the input while submitting.
                if self.campaign_redeem.submitting {
                    return iced::Task::none();
                }
                self.campaign_redeem.code = code;
                // Editing past a prior outcome clears it so the field reads as
                // a fresh attempt.
                self.campaign_redeem.result = None;
            }

            ConnectAccountMessage::RegisterCampaignCodeChanged(code) => {
                // Once signup is in flight the code is already stashed in
                // `pending_campaign_code`; ignore further edits so the visible
                // field can't diverge from what gets redeemed. The view also
                // locks the field while loading.
                if matches!(self.step, ConnectFlowStep::Register { loading: true, .. }) {
                    return iced::Task::none();
                }
                self.register_campaign_code = code;
            }

            ConnectAccountMessage::RedeemCampaignCode => {
                let code = self.campaign_redeem.code.trim().to_string();
                if code.is_empty() || self.campaign_redeem.submitting {
                    return iced::Task::none();
                }
                self.campaign_redeem.submitting = true;
                self.campaign_redeem.result = None;
                return self.redeem_campaign_task(code);
            }

            ConnectAccountMessage::CampaignRedeemed(result, gen) => {
                if gen != self.session_generation {
                    // Stale: the session changed since this redeem was fired
                    // (e.g. a second SessionLoaded bumped the generation while
                    // a registration redeem was in flight). Drop the result,
                    // but still clear the in-flight flag so the field can't get
                    // stuck on "Redeeming…".
                    self.campaign_redeem.submitting = false;
                    return iced::Task::none();
                }
                self.campaign_redeem.submitting = false;
                match result {
                    Ok(msg) => {
                        self.campaign_redeem.result = Some(Ok(msg));
                        // Clear the field on success.
                        self.campaign_redeem.code = String::new();
                    }
                    Err(e) => {
                        self.campaign_redeem.result = Some(Err(e));
                    }
                }
                // Account-creation redemption starts with the disclosure
                // closed. Reveal either outcome in the normal Plan & Billing
                // view without changing the current section or sub-view.
                self.show_campaign_redeem = true;
                // Refresh the plan after every redeem — on success to pick up
                // the granted tier/provenance, and on failure because the
                // account-creation path (SessionLoaded) defers its post-signup
                // plan fetch to here, so the real plan (including any
                // server-side auto-applied grant) still loads. This is the only
                // plan fetch on that path, so there's no racing PlanLoaded.
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
            ConnectAccountMessage::DuressStateChecked(outcome, gen, attempt) => {
                // Phase 6: post-sign-in gate. We sit in `CheckingDuress` until
                // this resolves, so the dashboard is never shown to a possibly-
                // in-duress account.
                if gen != self.session_generation {
                    return iced::Task::none();
                }
                match outcome {
                    DuressCheckOutcome::Ok(s) if s.active => {
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
                        // Cache the authoritative state we just fetched so the
                        // Duress tab paints from it immediately instead of
                        // flashing the setup flow while a fresh fetch is in
                        // flight (see `duress_ux`).
                        self.duress_state = Some(s);
                    }
                    // Confirmed not in duress — NOW reveal the dashboard.
                    DuressCheckOutcome::Ok(s) => {
                        self.step = ConnectFlowStep::Dashboard;
                        self.signed_out_by = None;
                        self.unverified = None;
                        // Cache the fetched state before any early return, so the
                        // Duress tab reflects the enabled state on its first paint
                        // rather than briefly showing enrollment CTAs.
                        let needs_device_register = s.enrolled && !s.this_device_registered;
                        let server_enrolled = s.enrolled;
                        self.duress_state = Some(s);
                        // Phase 0: a signed-in device on an already-enrolled
                        // account that isn't yet registered server-side mints +
                        // registers its OWN duress code (per-device, never
                        // shared), so it can fire trigger-with-code on its own
                        // activation. Fire-and-forget; idempotent (skips if this
                        // device already holds a code).
                        if needs_device_register {
                            if let Some(account_id) = self.user.as_ref().map(|u| u.id.to_string()) {
                                return register_device_duress_task(
                                    self.client.clone(),
                                    account_id,
                                );
                            }
                        }
                        // Launch reconcile (Issue 2, PR 4): bring this device's
                        // local duress state in line with the server's, catching
                        // a disable missed while offline AND an orphaned device
                        // (Cubes armed while the local state never finished, e.g.
                        // a crash mid-enroll) whose duress PIN would otherwise
                        // still trip a wipe behind a UI that reports it inert.
                        //
                        // `reconcile_duress_disarm` (server says NOT enrolled)
                        // disarms a matching local enrollment or any orphaned
                        // armed Cubes; `reconcile_duress_orphan` (server says
                        // STILL enrolled — reached only with this device already
                        // registered) disarms ONLY an orphan, never a healthy
                        // enrollment. Both no-op cheaply for a consistent device.
                        if let Some(account_id) = self.user.as_ref().map(|u| u.id.to_string()) {
                            if let Ok(dir) = crate::dir::CoincubeDirectory::active() {
                                let gen = self.session_generation;
                                let task = if server_enrolled {
                                    iced::Task::perform(
                                        async move { crate::app::reconcile_duress_orphan(dir).await },
                                        move |res| {
                                            Message::View(view::Message::ConnectAccount(
                                                ConnectAccountMessage::Duress(
                                                    DuressMessage::DisarmComplete(res, gen),
                                                ),
                                            ))
                                        },
                                    )
                                } else {
                                    iced::Task::perform(
                                        async move {
                                            crate::app::reconcile_duress_disarm(dir, account_id)
                                                .await
                                        },
                                        move |res| {
                                            Message::View(view::Message::ConnectAccount(
                                                ConnectAccountMessage::Duress(
                                                    DuressMessage::DisarmComplete(res, gen),
                                                ),
                                            ))
                                        },
                                    )
                                };
                                return task;
                            }
                        }
                    }
                    // Transient (network / 5xx / rate-limit) — retry with
                    // backoff while attempts remain.
                    DuressCheckOutcome::Unreachable if attempt + 1 < DURESS_CHECK_MAX_ATTEMPTS => {
                        return duress_state_check_task(self.client.clone(), gen, attempt + 1);
                    }
                    // Retries exhausted. Fail CLOSED: do not reveal the
                    // dashboard. Show an error + Retry on the checking screen.
                    // (The dashboard is server-backed anyway, so it would be
                    // non-functional during this outage.)
                    DuressCheckOutcome::Unreachable => {
                        log::warn!(
                            "[CONNECT] duress state check unreachable after {} attempts; \
                             holding at the verification gate",
                            attempt + 1
                        );
                        self.step = ConnectFlowStep::CheckingDuress {
                            status: DuressGateStatus::Unreachable,
                        };
                    }
                    // The server returned a body we can't decode. Auto-retrying
                    // is futile (it'll mismatch every time), so DON'T loop —
                    // hold closed with distinct copy + an update hint. The body
                    // was already logged in `get_duress_state`. A manual Retry
                    // is still offered in case it's a server bug being hotfixed.
                    DuressCheckOutcome::Incompatible => {
                        log::error!(
                            "[CONNECT] duress state response is undecodable (contract \
                             mismatch); holding the gate closed — app may need updating"
                        );
                        self.step = ConnectFlowStep::CheckingDuress {
                            status: DuressGateStatus::Incompatible,
                        };
                    }
                    // The session was rejected mid-gate. Sitting on the duress
                    // gate would loop forever; drop to login instead.
                    DuressCheckOutcome::Unauthorized => {
                        log::warn!(
                            "[CONNECT] duress gate: session rejected (401); returning to login"
                        );
                        let was_logged_in = self.user.is_some();
                        // The session is dead — tear it all down (bumps
                        // session_generation so stale async results are ignored,
                        // drops the JWT client, keyring session, and cached
                        // user/plan) so the next Init doesn't restore it.
                        self.clear_session();
                        // Settled, not retryable: the session is gone, so the
                        // card offers no "Try again" — signing in is the only
                        // way forward.
                        self.error = Some(PanelError::settled(UserError::new(
                            "Your session has expired",
                            "Sign in again to continue.",
                            crate::user_error::CC_AUTH_EXPIRED,
                            false,
                        )));
                        self.step = ConnectFlowStep::Login {
                            email: String::new(),
                            loading: false,
                        };
                        // Mirror the normal LogOut path: Buy/Sell shares this
                        // session, so it must drop its stale in-memory token too
                        // rather than keep showing a logged-in state.
                        if was_logged_in {
                            return iced::Task::done(Message::View(view::Message::BuySell(
                                view::BuySellMessage::LogOut,
                            )));
                        }
                    }
                }
            }
            ConnectAccountMessage::RetryDuressCheck => {
                // From the CheckingDuress error screen (either failure mode).
                self.step = ConnectFlowStep::CheckingDuress {
                    status: DuressGateStatus::Checking,
                };
                let gen = self.session_generation;
                return duress_state_check_task(self.client.clone(), gen, 0);
            }
            ConnectAccountMessage::DuressDeviceRegistered => {
                // Side-effect-only task completed (Phase 0 device-code register,
                // or syncing DuressLocalState.active to the server's duress
                // state — set on the post-sign-in mirror, reset on recovery).
            }
            ConnectAccountMessage::Duress(m) => return self.update_duress(m),
            ConnectAccountMessage::DuressContacts(m) => return self.update_duress_contacts(m),
        }

        iced::Task::none()
    }

    /// Opens the duress enrollment wizard, resetting all inputs. Duress is a
    /// paid feature behind a hard, server-verified Recovery-Kit gate, so there
    /// is a single enrollment path and the wizard always opens at `SetDuressPin`.
    fn open_enroll_wizard(&mut self) {
        // Scrub any wizard already in flight before replacing it, so re-opening
        // never drops its secrets unzeroized.
        self.clear_duress_enroll();
        self.duress_enroll = Some(DuressEnrollState {
            step: ENROLL_STEPS[0],
            duress_pin: String::new(),
            duress_pin_confirm: String::new(),
            all_clear: String::new(),
            crk_password: String::new(),
            delay: crate::services::duress::enroll::DuressDelay::default(),
            memorized: false,
            submitting: false,
            error: None,
            pending_code: None,
        });
    }

    /// Tears down all session state: bumps `session_generation` (so any
    /// in-flight async results keyed to the old generation are ignored),
    /// clears cached user/plan/feature data, scrubs secrets, drops the keyring
    /// session, and replaces the JWT-bearing client with a fresh anonymous one.
    /// Does NOT set `self.step` — the caller picks the destination (Login, an
    /// error screen, etc.). Shared by `LogOut` and the duress-gate 401 path so
    /// a rejected session can't leave stale state behind for the next `Init`.
    fn clear_session(&mut self) {
        self.auth_commit_pending = false;
        self.unverified = None;
        let owns_saved_session = !self.admitted_client;
        self.session_generation += 1;
        self.user = None;
        self.plan = None;
        self.verified_devices = None;
        self.login_activity = None;
        self.review_sign_out = false;
        self.review_device_removal = None;
        self.otp_attempt_generation = self.otp_attempt_generation.wrapping_add(1);
        self.overview_contact_count = None;
        self.overview_cube_count = None;
        self.recovery_services = RecoveryServicesState::NotLoaded;
        self.features = None;
        self.checkout = None;
        self.billing_history = None;
        self.show_billing_history = false;
        self.show_plan_comparison = false;
        self.show_campaign_redeem = false;
        self.renewal_banner_dismissed = false;
        self.campaign_redeem = CampaignRedeemState::default();
        self.register_campaign_code = String::new();
        self.pending_campaign_code = None;
        self.selected_billing_cycle = BillingCycle::Monthly;
        self.selected_plan_tier = None;
        self.contacts_state.clear();
        self.duress_contacts.clear();
        self.duress_cubes = None;
        self.duress_state = None;
        self.duress_locally_armed = false;
        // Scrub any in-flight enrollment wizard secrets (PINs, passphrases,
        // generated code) and the recovery all-clear passphrase before
        // dropping them, so they don't survive the session reset.
        self.clear_duress_enroll();
        if let Some(mut d) = self.duress_disable.take() {
            d.zeroize_secrets();
        }
        self.scrub_recovery_passphrase();
        if owns_saved_session {
            self.clear_keyring_session();
        }
        self.admitted_client = false;
        self.shared_client = false;
        self.admitted_user_loading = false;
        self.client = CoincubeClient::new();
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
        self.duress_enroll_abandoned = false;
        if let Some(mut wizard) = self.duress_enroll.take() {
            wizard.zeroize_secrets();
        }
    }

    /// A failed preflight or server request may arrive after the user left the
    /// sheet. Retain retry state only while its wizard is still visible.
    fn fail_duress_enrollment(&mut self, message: String) {
        if self.duress_enroll_abandoned {
            self.clear_duress_enroll();
        } else if let Some(wizard) = &mut self.duress_enroll {
            wizard.submitting = false;
            wizard.error = Some(message);
        }
    }

    /// Cancel-equivalent teardown for the in-flight duress dialogs, for when the
    /// surface is hidden out from under the user — the launch kill-switch
    /// (`show_duress`) flipped off mid-session and the route is redirecting
    /// away. Runs the same scrubbing the Cancel buttons do, so a flag flip can't
    /// strand duress PINs/codes on the heap once the surface is unreachable:
    ///
    /// - the enrollment wizard is discarded (secrets zeroized) only when it
    ///   isn't mid-submit — a server `enroll_duress` in flight still needs its
    ///   code to persist on success, exactly as `CancelEnrollment` guards;
    /// - the step-up disable dialog is always taken and zeroized, matching
    ///   `DisableCancel`.
    pub fn scrub_duress_dialogs(&mut self) {
        if self.duress_enroll.as_ref().is_some_and(|e| e.submitting) {
            self.duress_enroll_abandoned = true;
        } else {
            self.clear_duress_enroll();
        }
        if let Some(mut d) = self.duress_disable.take() {
            crate::services::passkey::reauth::cancel();
            d.zeroize_secrets();
        }
    }

    /// A Connect sheet can remain open while an account plan or launch flag
    /// changes. Its Duress route uses the same hidden-surface rule as Home.
    fn leave_hidden_duress_route(&mut self) {
        if matches!(self.active_sub, ConnectSubMenu::Duress) && !self.show_duress() {
            self.scrub_duress_dialogs();
            self.active_sub = ConnectSubMenu::Overview;
        }
    }

    /// Duress "Emergency contacts" dispatcher (Estate Notifications — PR 1).
    ///
    /// Every mutating path re-asserts the `duress_alerts` entitlement before
    /// hitting the network, so a plan downgrade mid-session can't drive a
    /// create/update/delete that would only 403. The list view is harmless
    /// for a downgraded account (it just shows the locked affordance), so
    /// reads aren't re-gated here beyond `reload_duress_contacts`.
    fn update_duress_contacts(&mut self, msg: DuressContactsMessage) -> iced::Task<Message> {
        use crate::services::coincube::is_valid_e164;
        match msg {
            DuressContactsMessage::Loaded(res, gen) => {
                if gen != self.session_generation {
                    return iced::Task::none();
                }
                self.duress_contacts.loading = false;
                match res {
                    Ok(list) => {
                        self.duress_contacts.contacts = Some(list);
                        self.duress_contacts.error = None;
                    }
                    Err(e) => self.duress_contacts.error = Some(e),
                }
            }
            DuressContactsMessage::ShowAddForm => {
                let dc = &mut self.duress_contacts;
                if dc.at_cap() {
                    dc.error = Some(coincube_ui::localized_format!(
                        "You can configure up to {} emergency contacts.",
                        "Puedes configurar hasta {} contactos de emergencia.",
                        MAX_DURESS_ALERT_CONTACTS
                    ));
                    return iced::Task::none();
                }
                dc.reset_form_for_add();
                dc.step = DuressContactsStep::Form;
            }
            DuressContactsMessage::EditContact(id) => {
                let dc = &mut self.duress_contacts;
                let found = dc
                    .contacts
                    .as_ref()
                    .and_then(|cs| cs.iter().find(|c| c.id == id))
                    .cloned();
                if let Some(c) = found {
                    dc.load_form_from(&c);
                    dc.step = DuressContactsStep::Form;
                }
            }
            DuressContactsMessage::BackToList => {
                let dc = &mut self.duress_contacts;
                dc.step = DuressContactsStep::List;
                dc.error = None;
            }
            DuressContactsMessage::NameChanged(v) => self.duress_contacts.form_name = v,
            DuressContactsMessage::PhoneChanged(v) => {
                let dc = &mut self.duress_contacts;
                dc.form_phone = v;
                // A phone-dependent channel can't outlive its phone.
                if dc.form_phone.trim().is_empty() {
                    dc.form_ch_sms = false;
                    dc.form_ch_whatsapp = false;
                }
            }
            DuressContactsMessage::EmailChanged(v) => {
                let dc = &mut self.duress_contacts;
                dc.form_email = v;
                if dc.form_email.trim().is_empty() {
                    dc.form_ch_email = false;
                }
            }
            DuressContactsMessage::ToggleSms(b) => self.duress_contacts.form_ch_sms = b,
            DuressContactsMessage::ToggleWhatsapp(b) => self.duress_contacts.form_ch_whatsapp = b,
            DuressContactsMessage::ToggleEmailChannel(b) => self.duress_contacts.form_ch_email = b,
            DuressContactsMessage::Submit => {
                // A submit is already in flight — drop the duplicate before it
                // spawns a second request. The button is view-disabled while
                // `submitting`, but iced can enqueue two presses in one frame
                // before the re-render lands, and create isn't idempotent (a
                // double would make two contacts).
                if self.duress_contacts.submitting {
                    return iced::Task::none();
                }
                if !self.is_duress_alerts_entitled() {
                    self.duress_contacts.error =
                        Some("Emergency contacts require an Estate plan.".to_string());
                    return iced::Task::none();
                }
                // Validate the form. `validate_duress_contact_form` returns
                // the normalised (name, phone, email, channels) tuple or a
                // user-facing error.
                let dc = &mut self.duress_contacts;
                let name = dc.form_name.trim().to_string();
                let phone = dc.form_phone.trim().to_string();
                let email = dc.form_email.trim().to_string();
                if name.is_empty() {
                    dc.error = Some("Add a name for this contact.".to_string());
                    return iced::Task::none();
                }
                if phone.is_empty() && email.is_empty() {
                    dc.error =
                        Some("Add a phone number or an email so we can reach them.".to_string());
                    return iced::Task::none();
                }
                if !phone.is_empty() && !is_valid_e164(&phone) {
                    dc.error = Some(
                        "Enter the phone in international format, e.g. +15551234567.".to_string(),
                    );
                    return iced::Task::none();
                }
                if !email.is_empty()
                    && email_address::EmailAddress::parse_with_options(
                        &email,
                        email_address::Options::default().with_required_tld(),
                    )
                    .is_err()
                {
                    dc.error = Some("That email doesn't look right.".to_string());
                    return iced::Task::none();
                }
                let mut channels = Vec::new();
                if dc.form_ch_sms && !phone.is_empty() {
                    channels.push(DURESS_CHANNEL_SMS.to_string());
                }
                if dc.form_ch_whatsapp && !phone.is_empty() {
                    channels.push(DURESS_CHANNEL_WHATSAPP.to_string());
                }
                if dc.form_ch_email && !email.is_empty() {
                    channels.push(DURESS_CHANNEL_EMAIL.to_string());
                }
                if channels.is_empty() {
                    dc.error = Some(
                        "Pick at least one way to reach them (SMS, WhatsApp, or email)."
                            .to_string(),
                    );
                    return iced::Task::none();
                }
                let editing_id = dc.editing_id;
                // Cap is only enforced on create — an edit of an existing
                // contact is always allowed even at the cap.
                if editing_id.is_none() && dc.at_cap() {
                    dc.error = Some(coincube_ui::localized_format!(
                        "You can configure up to {} emergency contacts.",
                        "Puedes configurar hasta {} contactos de emergencia.",
                        MAX_DURESS_ALERT_CONTACTS
                    ));
                    return iced::Task::none();
                }
                dc.submitting = true;
                dc.error = None;
                let phone_opt = (!phone.is_empty()).then_some(phone);
                let email_opt = (!email.is_empty()).then_some(email);
                let client = self.client.clone();
                let gen = self.session_generation;
                return match editing_id {
                    None => {
                        let req = CreateDuressAlertContactRequest {
                            display_name: name,
                            phone: phone_opt,
                            email: email_opt,
                            channels,
                        };
                        iced::Task::perform(
                            async move { client.create_duress_alert_contact(req).await },
                            move |res| {
                                duress_contacts_msg(DuressContactsMessage::SubmitResult(
                                    res.map(|_| ()).map_err(|e| e.to_string()),
                                    gen,
                                ))
                            },
                        )
                    }
                    Some(id) => {
                        let req = UpdateDuressAlertContactRequest {
                            display_name: Some(name),
                            // Send explicit phone/email so clearing one in the
                            // edit form persists. A `None` here would mean
                            // "untouched"; we always send the current field
                            // values (empty string maps to null server-side).
                            phone: Some(phone_opt.unwrap_or_default()),
                            email: Some(email_opt.unwrap_or_default()),
                            channels: Some(channels),
                        };
                        iced::Task::perform(
                            async move { client.update_duress_alert_contact(id, req).await },
                            move |res| {
                                duress_contacts_msg(DuressContactsMessage::SubmitResult(
                                    res.map(|_| ()).map_err(|e| e.to_string()),
                                    gen,
                                ))
                            },
                        )
                    }
                };
            }
            DuressContactsMessage::SubmitResult(res, gen) => {
                if gen != self.session_generation {
                    return iced::Task::none();
                }
                self.duress_contacts.submitting = false;
                match res {
                    Ok(()) => {
                        // Back to the list and refetch the authoritative set
                        // (the create response's `intro_sent_at` may lag the
                        // async send, so a refetch is the source of truth).
                        return self.reload_duress_contacts();
                    }
                    Err(e) => self.duress_contacts.error = Some(e),
                }
            }
            DuressContactsMessage::Delete(id) => {
                if !self.is_duress_alerts_entitled() {
                    return iced::Task::none();
                }
                // Same-row delete already in flight — drop the duplicate
                // before issuing a second DELETE (see the Submit guard above).
                if self.duress_contacts.deleting_ids.contains(&id) {
                    return iced::Task::none();
                }
                self.duress_contacts.deleting_ids.insert(id);
                self.duress_contacts.error = None;
                let client = self.client.clone();
                let gen = self.session_generation;
                return iced::Task::perform(
                    async move { client.delete_duress_alert_contact(id).await },
                    move |res| {
                        duress_contacts_msg(DuressContactsMessage::DeleteResult(
                            id,
                            res.map_err(|e| e.to_string()),
                            gen,
                        ))
                    },
                );
            }
            DuressContactsMessage::DeleteResult(id, res, gen) => {
                if gen != self.session_generation {
                    return iced::Task::none();
                }
                self.duress_contacts.deleting_ids.remove(&id);
                match res {
                    Ok(()) => {
                        // Optimistically drop the row, then refetch.
                        if let Some(cs) = self.duress_contacts.contacts.as_mut() {
                            cs.retain(|c| c.id != id);
                        }
                        return self.reload_duress_contacts();
                    }
                    Err(e) => self.duress_contacts.error = Some(e),
                }
            }
        }
        iced::Task::none()
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
                if *submitting {
                    return iced::Task::none();
                }
                let passphrase = zeroize::Zeroizing::new(passphrase.clone());
                *submitting = true;
                let client = self.client.clone();
                let gen = self.session_generation;
                return iced::Task::perform(
                    async move { client.clear_duress(&passphrase).await },
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
                        Err(e) => {
                            // The all-clear screen has its own Submit; a
                            // second button on the card would race it.
                            self.error = Some(PanelError::settled(UserError::logged(
                                "Couldn't confirm the all-clear",
                                "Check your internet connection and try again.",
                                crate::user_error::CC_API_BADRESP,
                                true,
                                e,
                            )))
                        }
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
                self.signed_out_by = None;
                self.unverified = None;
                self.error = None;
            }

            // ── Enrollment wizard (Phases 2 & 8) ──
            DuressMessage::StartEnrollment => {
                // An abandoned submission still owns the server response and
                // its local arming payload; do not replace it with a new wizard.
                if self.duress_enroll_abandoned {
                    return iced::Task::none();
                }
                // Duress is a paid (Pro/Estate) feature behind a launch
                // kill-switch. The nav hides the surface and the CTA for anyone
                // who fails the show-rule (entitlement AND the launch gate);
                // re-check the full gate here as a defensive backstop so a stale
                // view can never open the wizard while duress is dark. Fails
                // closed for an un-enrolled account with the flag off.
                if !self.show_duress() {
                    return iced::Task::none();
                }
                // The hard vault gate (PLAN-duress-vault-gate). The CTA is
                // rendered disabled while blocked, but re-check here as a
                // belt-and-suspenders backstop: a stale view (kit status that
                // landed after the last paint) could still fire this. With the
                // Tier-2 bypass and the Sovereign path removed, this gate is the
                // single hard requirement for enrollment.
                //
                // Fail closed when the checklist hasn't loaded (`None`): an
                // unverified fleet must not open enrollment (master I7 / Resolved
                // decision 4) — `duress_tier1_gate_blocked` returns true for
                // `None`. Reload so the checklist (and any retry affordance)
                // becomes current instead of silently proceeding.
                if duress_tier1_gate_blocked(self.duress_cubes.as_deref()) {
                    // No-op: gate blocked (or fleet not yet verified). A fresh
                    // reload keeps the checklist current.
                    return self.reload_duress_cubes();
                }
                if let Err(error) = crate::app::check_import_duress_compatibility() {
                    self.error = Some(PanelError::settled(UserError::new(
                        "Duress Mode is unavailable",
                        error,
                        crate::user_error::CC_CONFIG,
                        false,
                    )));
                    return iced::Task::none();
                }
                self.open_enroll_wizard();
                // Kick a fresh checklist fetch as the user commits to enrolling.
                // The cached list the gate just read could be briefly stale (a
                // routine reload keeps the prior list in place while its refetch
                // is in flight); firing one now guarantees the `SubmitEnrollment`
                // re-check runs against current completeness by the time the
                // wizard is finished.
                return self.reload_duress_cubes();
            }
            DuressMessage::ReloadCubes => {
                return self.reload_duress_cubes();
            }
            DuressMessage::CancelEnrollment => {
                // Ignore cancel while a server enroll_duress is in flight:
                // clear_duress_enroll zeroizes the duress PIN/code, so the
                // EnrollResult success handler would have nothing left to
                // persist — leaving the server enrolled but this device
                // un-armed. The Cancel button is disabled in this state too;
                // this is the authoritative backstop.
                if self.duress_enroll.as_ref().is_none_or(|e| !e.submitting) {
                    self.clear_duress_enroll();
                }
            }
            DuressMessage::DuressPinChanged(v) => {
                if let Some(e) = &mut self.duress_enroll {
                    e.duress_pin = v.chars().filter(|c| c.is_ascii_digit()).take(4).collect();
                }
            }
            DuressMessage::DuressPinConfirmChanged(v) => {
                if let Some(e) = &mut self.duress_enroll {
                    e.duress_pin_confirm =
                        v.chars().filter(|c| c.is_ascii_digit()).take(4).collect();
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
            DuressMessage::MemorizedToggled(v) => {
                if let Some(e) = &mut self.duress_enroll {
                    e.memorized = v;
                }
            }
            DuressMessage::EnrollBack => {
                if let Some(e) = &mut self.duress_enroll {
                    e.error = None;
                    e.step = prev_enroll_step(e.step);
                }
            }
            DuressMessage::EnrollNext => {
                if let Some(e) = &mut self.duress_enroll {
                    if let Err(msg) = validate_enroll_step(e) {
                        e.error = Some(msg);
                    } else {
                        e.error = None;
                        e.step = next_enroll_step(e.step);
                    }
                }
            }
            DuressMessage::SubmitEnrollment => {
                // Vault gate backstop (PLAN-duress-vault-gate). The gate is
                // enforced at StartEnrollment, but Cube state can change while
                // the wizard is open — a mid-session Vault creation calls
                // `invalidate_duress_cubes` and expects the gate to stay closed
                // until the new Vault's kit is complete. Re-check at the actual
                // commit (the single hard-gated path), mirroring the
                // PIN-collision pre-flight below: surface the block on the wizard
                // and refresh the checklist, without firing the enroll. Fails
                // closed on an invalidated/unloaded checklist (`None`), which is
                // exactly the mid-wizard-Vault case.
                if self.duress_enroll.is_some()
                    && duress_tier1_gate_blocked(self.duress_cubes.as_deref())
                {
                    if let Some(e) = &mut self.duress_enroll {
                        e.error = Some(
                            "A Vault Cube still needs its Recovery Kit finished — its Master \
                             Seed Phrase and Vault Wallet Descriptor — before you can enable \
                             Duress Mode. Finish it, then try again."
                                .to_string(),
                        );
                    }
                    // Refresh so the intro checklist is current if the user
                    // cancels back to it.
                    return self.reload_duress_cubes();
                }
                let Some(e) = &mut self.duress_enroll else {
                    return iced::Task::none();
                };
                // Reject re-entrant submits: a second enroll while one is in
                // flight would fire a duplicate server call and replace the
                // stashed pending_code out from under the first. The button is
                // disabled while submitting; this is the backstop.
                if e.submitting {
                    return iced::Task::none();
                }
                if let Err(msg) = validate_enroll_step(e) {
                    e.error = Some(msg);
                    return iced::Task::none();
                }
                // The blocking preflight also rejects pending/imported wallets
                // before server enrollment. Keep this filesystem check there so
                // the wizard receives the error without blocking the UI thread.
                // Pre-flight: reject a duress PIN that collides with any Cube's
                // real unlock PIN BEFORE anything irreversible. Connect tiers
                // enroll on the server first, so catching the collision only in
                // the later local persist would leave the account
                // server-enrolled with no Cube armed. (Persist re-checks as the
                // authoritative guard.)
                //
                // The check is a trial decryption of every Cube's seed file now
                // that there is no stored PIN hash to compare against — ~831 ms
                // per Cube — so it runs on the blocking pool and comes back as
                // `EnrollPreflightDone`. `submitting` is set here so the button
                // stays disabled for the whole round trip.
                let dir = match crate::dir::CoincubeDirectory::active() {
                    Ok(dir) => dir,
                    Err(err) => {
                        e.error = Some(
                            UserError::logged(
                                "Couldn't open your Cube data",
                                "Tenshu can't read its data folder, so it can't verify the duress PIN. Restart the app and try again.",
                                crate::user_error::CC_CONFIG,
                                true,
                                err,
                            )
                            .toast(),
                        );
                        return iced::Task::none();
                    }
                };
                e.submitting = true;
                e.error = None;
                let gen = self.session_generation;
                let duress_pin = zeroize::Zeroizing::new(e.duress_pin.clone());
                return iced::Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            crate::app::duress_pin_collision_check_blocking(dir.path(), &duress_pin)
                        })
                        .await
                        .unwrap_or_else(|err| Err(format!("Couldn't verify the duress PIN: {err}")))
                    },
                    move |res| {
                        Message::View(view::Message::ConnectAccount(
                            ConnectAccountMessage::Duress(DuressMessage::EnrollPreflightDone(
                                res, gen,
                            )),
                        ))
                    },
                );
            }
            DuressMessage::EnrollPreflightDone(res, gen) => {
                if gen != self.session_generation {
                    return iced::Task::none();
                }
                if self.duress_enroll.is_none() {
                    return iced::Task::none();
                }
                if let Err(msg) = res {
                    self.fail_duress_enrollment(msg);
                    return iced::Task::none();
                }
                let e = self.duress_enroll.as_mut().unwrap();
                let gen = self.session_generation;

                // Generate this device's duress code ONCE: its hash goes to the
                // server, the same plaintext is persisted (encrypted) locally.
                // Held in `Zeroizing` so the local plaintext is scrubbed on drop
                // — on normal completion and on every early-return below.
                let code = zeroize::Zeroizing::new(enroll::generate_duress_code());

                // Enroll on the server FIRST. The duress PIN + code are persisted
                // locally only after a successful EnrollResult, so a server
                // failure can't leave a half-armed duress PIN on disk. Stash the
                // code for that success handler. Zeroize any code stashed by a
                // prior submit (retry) before replacing it, so the superseded
                // plaintext doesn't linger.
                if let Some(mut old) = e.pending_code.replace((*code).clone()) {
                    zeroize::Zeroize::zeroize(&mut old);
                }
                let all_clear_hash = enroll::hash_duress_secret(&e.all_clear);
                // Single path always collects the account-level duress
                // recovery-kit password (Approach C).
                let crk_hash = enroll::hash_duress_secret(&e.crk_password);
                let code_hash = enroll::hash_duress_secret(&code);
                let delay_minutes = e.delay.minutes();

                let (all_clear_hash, code_hash) = match (all_clear_hash, code_hash) {
                    (Ok(a), Ok(c)) => (a, c),
                    _ => {
                        self.fail_duress_enrollment("Failed to hash credentials.".to_string());
                        return iced::Task::none();
                    }
                };
                let crk_hash = match crk_hash {
                    Ok(h) => Some(h),
                    Err(_) => {
                        self.fail_duress_enrollment("Failed to hash credentials.".to_string());
                        return iced::Task::none();
                    }
                };
                // A stable device fingerprint is required so the server can
                // recognise this desktop. If it can't be resolved, fail the
                // enrollment rather than send an unstable one.
                let fingerprint = match device_fingerprint() {
                    Ok(fp) => fp,
                    Err(msg) => {
                        self.fail_duress_enrollment(msg);
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
                        // password) unzeroized.
                        let payload = self.duress_enroll.as_ref().map(|e| {
                            crate::app::message::DuressEnrollmentPayload {
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
                            // The server accepted enrollment, but the duress PIN
                            // still has to be armed locally on every Cube. Defer
                            // marking the cached state "enrolled" to the persist
                            // success path (`EnrollmentPersisted`) — otherwise a
                            // local failure (PIN collision, no Cubes, disk error)
                            // would leave a false "enabled" view while no Cube is
                            // actually armed.
                            return iced::Task::done(Message::CompleteDuressEnrollment(payload));
                        }
                        // No wizard to build a payload from: it was torn down
                        // mid-flight (same session — a cross-session teardown
                        // bumps `session_generation` and is caught by the guard
                        // above). All same-session teardowns now preserve a
                        // submitting wizard (Cancel is blocked while submitting;
                        // the active-duress routing skips a submitting wizard),
                        // so this should be unreachable — log loudly if it ever
                        // fires, since it means the account is server-enrolled
                        // with no local arming.
                        log::error!(
                            "[CONNECT] duress EnrollResult(Ok) with no wizard to persist — \
                             account may be server-enrolled but not armed locally"
                        );
                    }
                    Err(msg) => {
                        // A hidden wizard has no retry surface. Scrub its
                        // secrets as soon as the server has rejected it.
                        self.fail_duress_enrollment(msg);
                    }
                }
            }
            DuressMessage::CubesLoaded(cubes, gen, seq) => {
                // Apply only the newest fetch: `gen` guards against a prior
                // session, `seq` against an older in-flight fetch in this
                // session landing after a newer one (which could restore a
                // stale, e.g. pre-Vault-creation, checklist and reopen the
                // gate). Only overwrite on success (`Some`) so a transient
                // failure doesn't blank a previously-loaded checklist.
                if gen == self.session_generation && seq == self.duress_cubes_seq && cubes.is_some()
                {
                    self.duress_cubes = cubes;
                }
            }
            DuressMessage::StateLoaded(st, gen) => {
                if gen == self.session_generation {
                    if let Some(new) = st {
                        // Don't let a stale fetch downgrade a locally-confirmed
                        // enrollment. `reload_duress_state` is fired on opening
                        // the tab; a slow one issued *before* the user finished
                        // enrolling could return pre-enrollment data after
                        // `EnrollmentPersisted` and clobber the enabled view back
                        // to the setup flow. Enrolled is monotonic within a
                        // session (there is no un-enroll path), so a fetch that
                        // flips it true→false can only be stale — drop it.
                        let downgrades_enrollment = self
                            .duress_state
                            .as_ref()
                            .is_some_and(|cur| cur.enrolled && !new.enrolled);
                        if !downgrades_enrollment {
                            let active = new.active;
                            let unlock_at = new.unlock_at;
                            self.duress_state = Some(new);
                            // If the server reports the account is in ACTIVE
                            // duress, route this (signed-in, trusted) device to
                            // the all-clear recovery screen — same as the
                            // post-sign-in `DuressStateChecked` gate. Covers
                            // duress activated by another device after this
                            // session passed the sign-in gate. Skip if already
                            // in recovery so an in-flight all-clear entry isn't
                            // wiped. Like the gate, this does NOT persist
                            // `DuressLocalState.active` (that flag is owned by
                            // the paths that actually lock this device).
                            if active
                                && !matches!(self.step, ConnectFlowStep::DuressRecovery { .. })
                            {
                                // Tear down any open enrollment wizard before
                                // routing to recovery: recovery supersedes it,
                                // and `duress_ux` prioritizes a `Some`
                                // `duress_enroll` over the intro/enabled view —
                                // so a leftover wizard would re-appear (possibly
                                // mid-submit) when the user returns to the tab
                                // after clearing. Also scrubs the wizard's
                                // in-memory secrets (PINs / passphrases).
                                //
                                // EXCEPT while a server enroll is in flight
                                // (`submitting`): its `EnrollResult` reads the
                                // wizard to run local persist, then clears it.
                                // Dropping it here would orphan that enroll
                                // (server enrolled, no local arm). Leave it; the
                                // in-flight result tears it down.
                                if self.duress_enroll.as_ref().is_none_or(|e| !e.submitting) {
                                    self.clear_duress_enroll();
                                }
                                self.step = ConnectFlowStep::DuressRecovery {
                                    unlock_at,
                                    passphrase: String::new(),
                                    submitting: false,
                                    cleared: false,
                                };
                            }
                        }
                    }
                    // `None` = the fetch failed; keep the prior state rather than
                    // blanking it.
                }
            }
            DuressMessage::EnrollmentPersisted => {
                // Local persistence succeeded (duress PIN armed on every Cube on
                // THIS device), so it's now safe to reflect the enabled state.
                // `duress_locally_armed` drives the "armed on this device" view;
                // `mark_duress_enrolled` mirrors the account-level server state.
                self.duress_locally_armed = true;
                self.mark_duress_enrolled();
            }

            // ── Disable (Issue 2 — turn duress off) ──
            DuressMessage::DisableStart => {
                // Open the step-up re-auth dialog. The trigger is only surfaced
                // when enrolled & inactive (see `duress_ux`).
                self.duress_disable = Some(DuressDisableState::default());
                // Which factor to demand depends on what is on this disk, so
                // ask before drawing the form. Filesystem work, hence the
                // blocking pool.
                let dir = match crate::dir::CoincubeDirectory::active() {
                    Ok(dir) => dir,
                    Err(err) => {
                        if let Some(d) = &mut self.duress_disable {
                            d.method = Some(crate::app::DuressStepUpMethod::Unavailable);
                            d.error = Some(
                                UserError::logged(
                                    "Couldn't open your Cube data",
                                    "Tenshu can't read its data folder, so it can't verify it's you. Restart the app and try again.",
                                    crate::user_error::CC_CONFIG,
                                    true,
                                    err,
                                )
                                .toast(),
                            );
                        }
                        return iced::Task::none();
                    }
                };
                let gen = self.session_generation;
                return iced::Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            crate::app::duress_step_up_method_blocking(dir.path())
                        })
                        .await
                        .unwrap_or_else(|err| {
                            Err(format!("Couldn't check how to verify it's you: {err}"))
                        })
                    },
                    move |res| {
                        Message::View(view::Message::ConnectAccount(
                            ConnectAccountMessage::Duress(DuressMessage::DisableMethodProbed(
                                res, gen,
                            )),
                        ))
                    },
                );
            }
            DuressMessage::DisableMethodProbed(res, gen) => {
                if gen != self.session_generation {
                    return iced::Task::none();
                }
                if let Some(d) = &mut self.duress_disable {
                    match res {
                        Ok(method) => d.method = Some(method),
                        Err(msg) => {
                            // Can't classify the device, so can't ask for the
                            // right thing. Refuse rather than guess: guessing
                            // PIN would strand a passkey user, and guessing
                            // passkey would raise a prompt that cannot help.
                            d.method = Some(crate::app::DuressStepUpMethod::Unavailable);
                            d.error = Some(msg);
                        }
                    }
                }
            }
            DuressMessage::DisablePinChanged(v) => {
                if let Some(d) = &mut self.duress_disable {
                    d.pin = v;
                    d.error = None;
                }
            }
            DuressMessage::DisableCancel => {
                // Drops the parked ceremony, which cancels the system sheet and
                // wakes the waiting task — otherwise a dismissed dialog leaves a
                // Touch ID prompt on screen with nothing behind it.
                crate::services::passkey::reauth::cancel();
                if let Some(mut d) = self.duress_disable.take() {
                    d.zeroize_secrets();
                }
            }
            DuressMessage::DisableSubmit => {
                let Some(d) = &mut self.duress_disable else {
                    return iced::Task::none();
                };
                // Re-entrancy guard: the confirm button is disabled while
                // submitting, but two presses can queue in one frame.
                if d.submitting {
                    return iced::Task::none();
                }
                // Only the PIN branch may run the PIN check. On a passkey device
                // it would fail with "use a device where you unlock a Cube with
                // its PIN" — advice that is wrong here, since this device can
                // authorize with its passkey.
                if !matches!(d.method, Some(crate::app::DuressStepUpMethod::Pin)) {
                    return iced::Task::none();
                }
                // Step-up: the entered PIN must match a Cube's REAL unlock PIN
                // (never the duress PIN). Verify BEFORE any server call — a wrong
                // PIN makes no call at all.
                //
                // Verification is a trial decryption of the seed file, so it
                // runs on the blocking pool and resumes at `DisableStepUpDone`.
                let dir = match crate::dir::CoincubeDirectory::active() {
                    Ok(dir) => dir,
                    Err(err) => {
                        d.error = Some(
                            UserError::logged(
                                "Couldn't open your Cube data",
                                "Tenshu can't read its data folder, so it can't verify your PIN. Restart the app and try again.",
                                crate::user_error::CC_CONFIG,
                                true,
                                err,
                            )
                            .toast(),
                        );
                        return iced::Task::none();
                    }
                };
                d.submitting = true;
                d.error = None;
                let gen = self.session_generation;
                let pin = zeroize::Zeroizing::new(d.pin.clone());
                return iced::Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            crate::app::verify_regular_cube_pin_blocking(dir.path(), &pin)
                        })
                        .await
                        .unwrap_or_else(|err| Err(format!("Couldn't verify your PIN: {err}")))
                    },
                    move |res| {
                        Message::View(view::Message::ConnectAccount(
                            ConnectAccountMessage::Duress(DuressMessage::DisableStepUpDone(
                                res, gen,
                            )),
                        ))
                    },
                );
            }
            DuressMessage::DisablePasskeySubmit => {
                let Some(d) = &mut self.duress_disable else {
                    return iced::Task::none();
                };
                if d.submitting {
                    return iced::Task::none();
                }
                // Only the passkey branch may raise a prompt. A stale press on
                // any other method must not, or a PIN device would get a system
                // sheet no credential can answer.
                let Some(crate::app::DuressStepUpMethod::Passkey(cube)) = d.method.clone() else {
                    return iced::Task::none();
                };
                d.submitting = true;
                d.error = None;
                let gen = self.session_generation;
                // Started on the UI thread — `NativePasskeyCeremony` is
                // main-thread-only. `reauth::begin` parks it and hands back a
                // plain future, so no polling subscription is needed here.
                match crate::services::passkey::reauth::begin(&cube) {
                    Ok(fut) => {
                        return iced::Task::perform(
                            async move {
                                // The words are dropped (and zeroized) right
                                // here. What authorizes the disable is that the
                                // assertion succeeded AND derived this Cube's
                                // own fingerprint — `reauth::begin` enforces the
                                // latter. The seed itself is not wanted, and
                                // must not ride in a message iced will clone.
                                fut.await.map(drop).map_err(|e| e.user_message())
                            },
                            move |res| {
                                Message::View(view::Message::ConnectAccount(
                                    ConnectAccountMessage::Duress(
                                        DuressMessage::DisableStepUpDone(res, gen),
                                    ),
                                ))
                            },
                        );
                    }
                    Err(e) => {
                        d.submitting = false;
                        d.error = Some(e.user_message());
                        return iced::Task::none();
                    }
                }
            }
            DuressMessage::DisableStepUpDone(res, gen) => {
                if gen != self.session_generation {
                    return iced::Task::none();
                }
                let Some(d) = &mut self.duress_disable else {
                    return iced::Task::none();
                };
                if let Err(msg) = res {
                    d.submitting = false;
                    d.error = Some(msg);
                    return iced::Task::none();
                }
                let gen = self.session_generation;
                let client = self.client.clone();
                return iced::Task::perform(
                    async move { client.disable_duress().await },
                    move |res| {
                        let mapped = res.map_err(|e| match &e {
                            crate::services::coincube::CoincubeError::Unsuccessful(info)
                                if info.status_code == 423 =>
                            {
                                view::DuressDisableError::Active
                            }
                            _ => view::DuressDisableError::Failed(e.to_string()),
                        });
                        Message::View(view::Message::ConnectAccount(
                            ConnectAccountMessage::Duress(DuressMessage::DisableResult(
                                mapped, gen,
                            )),
                        ))
                    },
                );
            }
            DuressMessage::DisableResult(res, gen) => {
                if gen != self.session_generation {
                    return iced::Task::none();
                }
                match res {
                    Ok(()) => {
                        // Server disabled — now disarm THIS device locally (clear
                        // Cube PIN hashes + local state + queue), via the active
                        // datadir (same one the enroll pre-flight uses).
                        let dir = match crate::dir::CoincubeDirectory::active() {
                            Ok(d) => d,
                            Err(err) => {
                                if let Some(d) = &mut self.duress_disable {
                                    d.submitting = false;
                                    d.error = Some(
                                        UserError::logged(
                                            "Duress is off on your account, but not on this device",
                                            "Tenshu couldn't read its data folder to finish. Reopen Settings to retry.",
                                            crate::user_error::CC_CONFIG,
                                            true,
                                            err,
                                        )
                                        .toast(),
                                    );
                                }
                                return iced::Task::none();
                            }
                        };
                        return iced::Task::perform(
                            async move {
                                crate::app::clear_duress_enrollment(dir)
                                    .await
                                    .map(|()| true)
                            },
                            move |res| {
                                Message::View(view::Message::ConnectAccount(
                                    ConnectAccountMessage::Duress(DuressMessage::DisarmComplete(
                                        res, gen,
                                    )),
                                ))
                            },
                        );
                    }
                    Err(view::DuressDisableError::Active) => {
                        if let Some(d) = &mut self.duress_disable {
                            d.submitting = false;
                            d.error = Some(
                                "Duress is currently active and can't be disabled.".to_string(),
                            );
                        }
                        // Re-sync state: if duress really is active, the
                        // `StateLoaded` handler routes this device to recovery.
                        return self.reload_duress_state();
                    }
                    Err(view::DuressDisableError::Failed(msg)) => {
                        if let Some(d) = &mut self.duress_disable {
                            d.submitting = false;
                            d.error = Some(msg);
                        }
                    }
                }
            }
            DuressMessage::DisarmComplete(res, gen) => {
                if gen != self.session_generation {
                    return iced::Task::none();
                }
                let had_dialog = self.duress_disable.is_some();
                match res {
                    Ok(changed) => {
                        if changed {
                            // This device is no longer armed; reflect the
                            // disabled state in the panel immediately.
                            self.duress_locally_armed = false;
                            if let Some(s) = &mut self.duress_state {
                                s.enrolled = false;
                                s.active = false;
                                s.this_device_registered = false;
                            }
                        }
                        if let Some(mut d) = self.duress_disable.take() {
                            d.zeroize_secrets();
                        }
                        // Confirmation toast only for the explicit, user-driven
                        // disable (a dialog was open). The cross-device event and
                        // launch reconcile disarm silently.
                        if changed && had_dialog {
                            return iced::Task::done(Message::View(view::Message::ShowSuccess(
                                "Duress mode disabled.".to_string(),
                            )));
                        }
                    }
                    Err(e) => {
                        // Built once, logged once, rendered in whichever of the
                        // two places is on screen. The dialog branch used to
                        // interpolate `{e}` — the common path, and the one that
                        // actually reached a user.
                        let user = UserError::logged(
                            "Couldn't turn off duress mode",
                            "Try again. If it keeps failing, contact support and quote the reference below.",
                            crate::user_error::CC_API_BADRESP,
                            true,
                            &e,
                        );
                        if let Some(d) = &mut self.duress_disable {
                            d.submitting = false;
                            d.error = Some(user.toast());
                        } else {
                            // Settled on the panel card: the dialog is gone, so
                            // there is no control left for the user to re-press
                            // and nothing for a card button to re-run.
                            self.error = Some(PanelError::settled(user));
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
    /// Whether checkout generation `id` is the one on screen.
    fn is_current_checkout(&self, id: u64) -> bool {
        self.checkout.as_ref().is_some_and(|cs| cs.id == id)
    }

    /// What opening Plan & billing loads: the plan (a payment confirmed after
    /// the user left the pay screen shows here) and billing history.
    pub fn reload_plan_billing(&mut self) -> iced::Task<Message> {
        let gen = self.session_generation;
        self.plan_seq = self.plan_seq.wrapping_add(1);
        let seq = self.plan_seq;
        let client = self.client.clone();
        iced::Task::batch([
            iced::Task::perform(
                async move { client.get_connect_plan().await.ok() },
                move |plan| {
                    Message::View(view::Message::ConnectAccount(
                        ConnectAccountMessage::PlanReloaded(plan, gen, seq),
                    ))
                },
            ),
            self.reload_billing_history(),
        ])
    }

    fn apply_loaded_plan(&mut self, plan: Option<ConnectPlan>) {
        // Seed the picker's period from the plan, but never move it under
        // someone choosing a plan.
        if self.checkout.is_none() && !self.show_plan_comparison {
            if let Some(cycle) = plan.as_ref().and_then(|p| p.billing_cycle) {
                self.selected_billing_cycle = cycle;
            }
        }
        self.plan = plan;
        self.leave_hidden_duress_route();
    }

    /// Fetch `GET /connect/billing/history` for Plan & billing's inline
    /// list and the full history page. Tagged with the session generation.
    pub fn reload_billing_history(&self) -> iced::Task<Message> {
        let gen = self.session_generation;
        let client = self.client.clone();
        iced::Task::perform(
            async move { client.get_billing_history().await },
            move |res| match res {
                Ok(history) => Message::View(view::Message::ConnectAccount(
                    ConnectAccountMessage::BillingHistoryLoaded(Ok(history), gen),
                )),
                Err(e) => Message::View(view::Message::ConnectAccount(
                    ConnectAccountMessage::BillingHistoryLoaded(Err((&e).into()), gen),
                )),
            },
        )
    }

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

    /// Whether self-service purchasing is currently available. Sourced from
    /// `GET /connect/features` (`purchasing_enabled`).
    ///
    /// Fails closed until features are known: while the fetch is in flight
    /// (right after sign-in) or failed, this is `false`, so a promo window
    /// never momentarily offers checkout before the server's stance loads.
    /// Once features *are* loaded, an absent flag means an older backend with
    /// purchasing on — enabled, so the existing flow stays intact for
    /// backends that don't send it (and for fall GA once a campaign closes).
    /// A campaign that disables purchasing sets it `false`, hiding every
    /// purchase surface (PLAN-estate-promo PR2).
    pub fn purchasing_enabled(&self) -> bool {
        match &self.features {
            None => false,
            Some(f) => f.purchasing_enabled.unwrap_or(true),
        }
    }

    /// Server-controlled Marketplace availability, sourced from
    /// `GET /connect/features` (`marketplaceEnabled` / `buySellEnabled` /
    /// `p2pEnabled`). Mirrored into [`crate::app::cache::Cache`] so the nav
    /// rails and route guard can hide the Marketplace surface.
    ///
    /// Fails **closed**, unlike [`Self::purchasing_enabled`]: while features
    /// are unloaded (`None` — in flight after sign-in, or the fetch failed) or
    /// a flag is absent, every flag reads `false`. A launch build must never
    /// surface the untested money feature on a stale, silent, or unreachable
    /// API. The first-release ceiling is applied before the flags reach the
    /// cache, even when an account override or server response enables them.
    /// See [`crate::app::features::MarketplaceServerFlags`].
    pub fn marketplace_server_flags(&self) -> crate::app::features::MarketplaceServerFlags {
        let server_flags = match &self.features {
            None => crate::app::features::MarketplaceServerFlags::OFF,
            Some(f) => crate::app::features::MarketplaceServerFlags {
                marketplace_enabled: f.marketplace_enabled.unwrap_or(false),
                buy_sell_enabled: f.buy_sell_enabled.unwrap_or(false),
                p2p_enabled: f.p2p_enabled.unwrap_or(false),
            },
        };
        server_flags.for_release()
    }

    /// The account-scoped Liquid grandfather flag from `GET /connect/features`
    /// (`liquidEnabled`). Mirrored into [`crate::app::cache::Cache`], where it
    /// is OR'd with on-disk Liquid state to produce the visibility gate.
    ///
    /// Defaults `false` while features are unloaded (in flight after sign-in,
    /// fetch failed, or never signed in) and when the flag is absent — a fresh
    /// install gets no Liquid unless the server grants it. Unlike
    /// [`Self::marketplace_server_flags`], a `false` here is **not** the whole
    /// story: it cannot hide a Liquid wallet that already exists on this
    /// machine. See [`crate::app::features::LiquidGate`].
    ///
    /// The fetch is authenticated (`post_login_tasks` calls `set_token` before
    /// spawning it) and `clear_session` drops `features` on logout, so this
    /// reverts to `false` when the session ends rather than holding a stale
    /// grant.
    pub fn liquid_server_enabled(&self) -> bool {
        self.features
            .as_ref()
            .and_then(|f| f.liquid_enabled)
            .unwrap_or(false)
    }

    /// The account-scoped Bitcoin Blake2b flag from `GET /connect/features`
    /// (`bitcoinBlake2bEnabled`) — the server half of
    /// [`crate::app::features::bitcoin_blake2b`], read here so a running Cube
    /// can answer the same question Home answers before unlock.
    ///
    /// Fails closed while features are unloaded, exactly like
    /// [`Self::marketplace_server_flags`], and reverts on logout because
    /// `clear_session` drops `features`.
    pub fn bitcoin_blake2b_enabled(&self) -> bool {
        crate::app::features::bitcoin_blake2b_enabled(
            self.bitcoin_blake2b_server_enabled(),
            self.bitcoin_blake2b_opt_in,
        )
    }

    pub fn bitcoin_blake2b_server_enabled(&self) -> bool {
        self.is_authenticated()
            && self
                .features
                .as_ref()
                .and_then(|f| f.bitcoin_blake2b_enabled)
                .unwrap_or(false)
    }

    /// The per-user duress launch flag from `GET /connect/features`
    /// (`duressEnabled`) — the server half of [`Self::duress_gate`].
    ///
    /// Defaults `false` while features are unloaded (in flight after sign-in,
    /// fetch failed, or never signed in) and when the flag is absent — fails
    /// closed like [`Self::marketplace_server_flags`]: a launch build ships
    /// duress dark and must not surface the untested setup on a stale, silent,
    /// or unreachable API. Unlike Marketplace, a `false` here is **not** the
    /// whole story — it cannot hide duress from an already-enrolled account
    /// (see [`Self::duress_gate`]).
    ///
    /// Deliberately **not** mirrored into [`crate::app::cache::Cache`] or
    /// persisted to per-cube settings: the durable half of the gate is
    /// enrollment, which lives in server/account state, so the gate is
    /// recomputed live rather than cached (contrast the Liquid grant, which is
    /// persisted because *funds* must survive offline).
    pub fn duress_server_enabled(&self) -> bool {
        self.features
            .as_ref()
            .and_then(|f| f.duress_enabled)
            .unwrap_or(false)
    }

    /// The launch/visibility gate for the duress surface: the server flag OR'd
    /// with this account's enrollment. The `OR enrolled` half is the client
    /// mirror of the server's grandfather rule (master I4) — an enrolled
    /// account keeps duress even when prod later serves `duressEnabled: false`
    /// or Connect is unreachable. See [`crate::app::features::DuressGate`].
    pub fn duress_gate(&self) -> crate::app::features::DuressGate {
        crate::app::features::DuressGate {
            server_enabled: self.duress_server_enabled(),
            enrolled: self.is_duress_enrolled(),
        }
    }

    /// Whether any duress surface should be shown: the paid `duress`
    /// entitlement AND the launch gate. The single source of truth consulted
    /// by the nav row, the route backstop, the duress view, and the
    /// enrollment-wizard backstop.
    ///
    /// A launch kill-switch like Marketplace — when this is `false` the surface
    /// is *hidden*, not greyed. (The duress *unlock* path — a duress PIN at
    /// unlock — is not a UI surface and is deliberately untouched by this gate;
    /// it must keep working for enrolled accounts regardless. Master I4/I8.)
    pub fn show_duress(&self) -> bool {
        self.is_duress_entitled() && self.duress_gate().on()
    }

    /// Whether the pre-expiry renewal banner should render: the plan is
    /// within its renewal window AND the user hasn't dismissed it this
    /// session. The expired state has its own dedicated UX (D3), so the
    /// banner intentionally does not cover `Expired`.
    ///
    /// Suppressed whenever purchasing is disabled — there is no purchase
    /// path then, so a "Renew now" nag would be a dead end. This is the
    /// generic, campaign-agnostic gate (a campaign that grants a plan also
    /// closes purchasing, so its grantees never see the banner during the
    /// window).
    pub fn show_renewal_banner(&self) -> bool {
        if !self.purchasing_enabled() {
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

/// Fetch the Account Overview summary counts. Runs the contacts and
/// cubes list calls back-to-back and reports their lengths. The Cube response
/// is also projected into sanitized Recovery services rows; a fetch failure
/// is visible there while summary counts keep their previous values.
///
/// The cube count is scoped to `active_network` when set (same
/// convention as `load_invite_cubes`: `None` counts cubes on every
/// network, e.g. on a Connect-only surface with no active cube).
fn load_overview_counts(
    client: &CoincubeClient,
    generation: u64,
    request_seq: u64,
    active_network: Option<String>,
) -> iced::Task<Message> {
    let client = client.clone();
    iced::Task::perform(
        async move {
            let contacts = client.get_contacts().await.ok().map(|c| c.len());
            let recovery: Result<Vec<CubeResponse>, UserError> =
                client.list_cubes().await.map_err(|error| (&error).into());
            let cubes = recovery.as_ref().ok().map(|cubes| {
                cubes
                    .iter()
                    .filter(|c| {
                        active_network
                            .as_deref()
                            .map(|net| c.network == net)
                            .unwrap_or(true)
                    })
                    .count()
            });
            (contacts, cubes, recovery)
        },
        move |(contacts, cubes, recovery)| {
            Message::View(view::Message::ConnectAccount(
                ConnectAccountMessage::OverviewCountsLoaded {
                    contacts,
                    cubes,
                    recovery,
                    generation,
                    request_seq,
                },
            ))
        },
    )
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

/// Wraps a [`DuressContactsMessage`] in the full
/// `Message::View(ConnectAccount(DuressContacts(..)))` envelope. Keeps the
/// async task closures in `update_duress_contacts` terse.
fn duress_contacts_msg(m: DuressContactsMessage) -> Message {
    Message::View(view::Message::ConnectAccount(
        ConnectAccountMessage::DuressContacts(m),
    ))
}

/// Fire a `get_duress_alert_contacts` fetch and wire the result into
/// `DuressContactsMessage::Loaded`. Estate-gating is the caller's
/// responsibility (`reload_duress_contacts` and the nav trigger both
/// check `is_duress_alerts_entitled` first) so this never spuriously 403s.
pub fn load_duress_alert_contacts(client: &CoincubeClient, generation: u64) -> iced::Task<Message> {
    let client = client.clone();
    iced::Task::perform(
        async move { client.get_duress_alert_contacts().await },
        move |res| {
            duress_contacts_msg(DuressContactsMessage::Loaded(
                res.map_err(|e| e.to_string()),
                generation,
            ))
        },
    )
}

/// Fetch the user's mainnet cubes + recovery-kit status for the Duress
/// intro screen's per-Cube checklist. Filters to mainnet — testnet cubes
/// hold no value, so their backup state is irrelevant here. Best-effort:
/// any error resolves to `None` (the handler then keeps any previously
/// loaded list rather than blanking it), and an absent list leaves the
/// screen on its generic recovery-kit copy.
fn load_duress_cubes(client: &CoincubeClient, generation: u64, seq: u64) -> iced::Task<Message> {
    use crate::services::coincube::CoincubeError;
    let client = client.clone();
    iced::Task::perform(
        async move {
            // Whole-fetch failure (list_cubes) resolves to `None` so the
            // handler keeps any previously-loaded list rather than blanking it.
            let cubes = client.list_cubes().await.ok()?;
            let mainnet: Vec<_> = cubes
                .into_iter()
                .filter(|c| c.network == "mainnet")
                .collect();

            // Local vault/passkey shape for Cubes this device holds. Read once
            // (mainnet dir) and matched to server Cubes by uuid below.
            let local = load_local_cube_shapes();
            let local_ref = &local;

            // Fan out per-cube recovery-kit *status* fetches concurrently — one
            // round-trip of latency, not N. A failed probe leaves `halves =
            // None` → completeness `Unknown` (never coerced to Complete). A
            // Cube with no kit at all skips the probe and reports empty halves.
            let client_ref = &client;
            let rows = iced::futures::future::join_all(mainnet.into_iter().map(|c| async move {
                let halves = if c.has_recovery_kit {
                    match client_ref.get_recovery_kit_status(c.id).await {
                        // Password blobs OR phone-sealed envelopes — either
                        // mode makes a half restorable (duress_kit_halves).
                        Ok(s) => Some(duress_kit_halves(&s)),
                        // 404 = no kit yet — not an error for our logic.
                        Err(CoincubeError::NotFound) => Some((false, false)),
                        Err(e) => {
                            log::warn!(
                                "[DURESS] recovery-kit status probe failed for \"{}\": {}",
                                c.name,
                                e
                            );
                            None
                        }
                    }
                } else {
                    // No kit material at all → both halves absent.
                    Some((false, false))
                };
                // Merge vault presence, precedence local > server > None
                // (PLAN-duress-vault-gate PR 3). Match to this device's Cubes
                // by uuid (local `CubeSettings.id` == server `uuid`): the
                // device holding the Cube is authoritative for its vault
                // presence. For other-device Cubes (no local match) fall back
                // to the server-reported `has_vault`; an older API that omits
                // the field leaves it `None` → `Unknown`.
                let local_match = local_ref.iter().find(|l| l.uuid == c.uuid);
                let (has_vault, is_passkey, is_local) =
                    merge_cube_vault_shape(local_match, c.has_vault);
                DuressCube {
                    server_id: c.id,
                    uuid: Some(c.uuid),
                    name: c.name,
                    has_recovery_kit: c.has_recovery_kit,
                    halves,
                    has_vault,
                    is_passkey,
                    local: is_local,
                }
            }))
            .await;
            // Fold in local mainnet Cubes the server list omits (unregistered /
            // `remote_synced == false`): armed by duress but invisible to the
            // server-driven gate. Fail closed on them (`append_local_only_rows`).
            Some(append_local_only_rows(rows, &local))
        },
        move |cubes| {
            Message::View(view::Message::ConnectAccount(
                ConnectAccountMessage::Duress(DuressMessage::CubesLoaded(cubes, generation, seq)),
            ))
        },
    )
}

/// This device's Cube vault/passkey shape, read from the mainnet
/// `settings.json`. Used to merge local vault presence into the Duress
/// checklist (PLAN-duress-vault-gate PR 1). Best-effort: a missing/unreadable
/// settings file yields an empty list, so every server Cube is treated as
/// other-device (`has_vault = None`) — which fails the gate closed rather than
/// silently allowing enrollment.
struct LocalCubeShape {
    /// Local `CubeSettings.id`, which equals the server Cube `uuid`.
    uuid: String,
    /// Local `CubeSettings.name`, used to label a local-only Cube in the
    /// checklist when it has no server row to borrow a name from.
    name: String,
    has_vault: bool,
    is_passkey: bool,
}

/// Merge a Cube's vault/passkey shape with precedence **local > server >
/// None** (PLAN-duress-vault-gate PR 3). Returns `(has_vault, is_passkey,
/// is_local)`. When this device holds the Cube (`local` is `Some`) its
/// local knowledge is authoritative; otherwise fall back to the
/// server-reported `has_vault` (`None` on an older API → `Unknown`), with
/// `is_passkey` unknown for other-device Cubes.
fn merge_cube_vault_shape(
    local: Option<&LocalCubeShape>,
    server_has_vault: Option<bool>,
) -> (Option<bool>, Option<bool>, bool) {
    match local {
        Some(l) => (Some(l.has_vault), Some(l.is_passkey), true),
        None => (server_has_vault, None, false),
    }
}

fn load_local_cube_shapes() -> Vec<LocalCubeShape> {
    use coincube_core::miniscript::bitcoin::Network;
    let Ok(datadir) = crate::dir::CoincubeDirectory::active() else {
        return Vec::new();
    };
    // Duress Cubes are mainnet-only, so only the mainnet settings matter.
    let network_dir = datadir.network_directory(Network::Bitcoin);
    let Ok(settings) = crate::app::settings::Settings::from_file(&network_dir) else {
        return Vec::new();
    };
    settings
        .cubes
        .iter()
        .map(|c| LocalCubeShape {
            uuid: c.id.clone(),
            name: c.name.clone(),
            has_vault: c.vault_wallet_id.is_some(),
            is_passkey: c.is_passkey_cube(),
        })
        .collect()
}

/// Append synthetic checklist rows for local mainnet Cubes that are **not**
/// present in the server-derived `rows` — e.g. a Cube created on this device but
/// not yet registered with Connect (`remote_synced == false`), which
/// `list_cubes` therefore omits. Duress arming iterates local `settings.cubes`
/// (all Cubes, all networks — see `persist_duress_enrollment`), so such a Cube
/// **is** wiped, yet the server-driven gate never sees it: a fail-open hole.
///
/// Each synthetic row carries `halves: None` → completeness `Unknown`, so
/// [`DuressCube::blocks_gate`] fails **closed** for any local-only Vault Cube
/// (vaultless Cubes stay non-blocking per master invariant I2). The Cube also
/// surfaces in the checklist so the user can see what to finish. Pure (no I/O)
/// so it is unit-testable independently of the async `load_duress_cubes`.
fn append_local_only_rows(mut rows: Vec<DuressCube>, local: &[LocalCubeShape]) -> Vec<DuressCube> {
    for l in local {
        let already_listed = rows
            .iter()
            .any(|r| r.uuid.as_deref() == Some(l.uuid.as_str()));
        if already_listed {
            continue;
        }
        rows.push(DuressCube {
            // Sentinel: no server record exists for this Cube, so there is no
            // server id and no per-cube kit-status fetch. `halves` is already
            // `None`, so `server_id` is never used to address the server here.
            server_id: 0,
            uuid: Some(l.uuid.clone()),
            name: l.name.clone(),
            has_recovery_kit: false,
            // Status unknowable (not on Connect) → fails the gate closed.
            halves: None,
            has_vault: Some(l.has_vault),
            is_passkey: Some(l.is_passkey),
            local: true,
        });
    }
    rows
}

/// Fetch server-side duress state (enrolled / active) for the Duress intro
/// screen. Best-effort: any error resolves to `None`, which the handler
/// leaves as the prior value, so a transient failure just shows the last
/// known state (or the setup flow before the first successful fetch).
fn load_duress_state(client: &CoincubeClient, generation: u64) -> iced::Task<Message> {
    let client = client.clone();
    iced::Task::perform(
        async move { client.get_duress_state().await.ok() },
        move |state| {
            Message::View(view::Message::ConnectAccount(
                ConnectAccountMessage::Duress(DuressMessage::StateLoaded(state, generation)),
            ))
        },
    )
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
                    ConnectAccountMessage::SecurityDataFailed((&e).into(), generation),
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
                    ConnectAccountMessage::SecurityDataFailed((&e).into(), generation),
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
/// sleeps first so a transient failure backs off before retrying. The result is
/// classified into a [`DuressCheckOutcome`] (rather than collapsed to `None`) so
/// the handler can retry only what's retryable and surface the right copy.
fn duress_state_check_task(client: CoincubeClient, gen: u64, attempt: u8) -> iced::Task<Message> {
    iced::Task::perform(
        async move {
            if attempt > 0 {
                tokio::time::sleep(DURESS_CHECK_RETRY_DELAY).await;
            }
            let outcome = match client.get_duress_state().await {
                Ok(state) => DuressCheckOutcome::Ok(state),
                Err(e) => {
                    let outcome = DuressCheckOutcome::from_err(&e);
                    log::warn!(
                        "[CONNECT] duress state check failed (attempt {}): {e} → {outcome:?}",
                        attempt + 1
                    );
                    outcome
                }
            };
            (outcome, gen, attempt)
        },
        |(outcome, g, a)| {
            Message::View(view::Message::ConnectAccount(
                ConnectAccountMessage::DuressStateChecked(outcome, g, a),
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

/// The single ordered enrollment sequence: duress PIN → all-clear passphrase →
/// account-level duress recovery-kit password → unlock delay → confirm. Duress
/// is paid + behind the hard Recovery-Kit gate, so there is no tier branching
/// and no self-attestation step.
const ENROLL_STEPS: [DuressEnrollStep; 5] = [
    DuressEnrollStep::SetDuressPin,
    DuressEnrollStep::SetAllClear,
    DuressEnrollStep::SetCrkPassword,
    DuressEnrollStep::PickDelay,
    DuressEnrollStep::Confirm,
];

fn next_enroll_step(cur: DuressEnrollStep) -> DuressEnrollStep {
    match ENROLL_STEPS.iter().position(|s| *s == cur) {
        Some(i) if i + 1 < ENROLL_STEPS.len() => ENROLL_STEPS[i + 1],
        _ => cur,
    }
}

fn prev_enroll_step(cur: DuressEnrollStep) -> DuressEnrollStep {
    match ENROLL_STEPS.iter().position(|s| *s == cur) {
        Some(i) if i > 0 => ENROLL_STEPS[i - 1],
        _ => cur,
    }
}

/// Validates the current step's inputs before advancing. Returns the spec error
/// string on failure.
fn validate_enroll_step(e: &DuressEnrollState) -> Result<(), String> {
    use crate::services::duress::enroll;
    match e.step {
        DuressEnrollStep::SetDuressPin => {
            // The open Cube's real unlock PIN, in plaintext, from the session
            // that unlocked it. This is the *only* regular PIN available as a
            // plaintext — there are no stored PIN hashes any more — and it is
            // the one the user could mistype. `None` makes the validator refuse
            // rather than skip the rule; see `validate_duress_pin`.
            let regular = crate::app::session::current_pin();
            enroll::validate_duress_pin(
                &e.duress_pin,
                &e.duress_pin_confirm,
                regular.as_ref().map(|p| p.as_str()),
            )
        }
        DuressEnrollStep::SetAllClear => enroll::validate_all_clear(&e.all_clear, &e.duress_pin),
        DuressEnrollStep::SetCrkPassword => {
            enroll::validate_duress_crk_password(&e.crk_password, &e.duress_pin, &e.all_clear)
        }
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
    use crate::services::coincube::OwnerSelfRecoverySummary;

    fn state(step: DuressEnrollStep) -> DuressEnrollState {
        DuressEnrollState {
            step,
            duress_pin: String::new(),
            duress_pin_confirm: String::new(),
            all_clear: String::new(),
            crk_password: String::new(),
            delay: crate::services::duress::enroll::DuressDelay::default(),
            memorized: false,
            submitting: false,
            error: None,
            pending_code: None,
        }
    }

    #[test]
    fn zeroize_secrets_clears_all_sensitive_fields() {
        let mut s = state(DuressEnrollStep::Confirm);
        s.duress_pin = "8765".to_string();
        s.all_clear = "correct horse battery".to_string();
        s.crk_password = "a-long-crk-password".to_string();
        s.pending_code = Some("deadbeefcafebabe".to_string());

        s.zeroize_secrets();

        assert!(s.duress_pin.is_empty());
        assert!(s.all_clear.is_empty());
        assert!(s.crk_password.is_empty());
        assert!(s.pending_code.as_deref().unwrap_or("").is_empty());
    }

    #[test]
    fn enroll_is_a_single_fixed_sequence() {
        // Duress is paid + behind the hard Recovery-Kit gate, so there is one
        // path: no tier branching, no self-attestation step.
        assert_eq!(
            ENROLL_STEPS,
            [
                DuressEnrollStep::SetDuressPin,
                DuressEnrollStep::SetAllClear,
                DuressEnrollStep::SetCrkPassword,
                DuressEnrollStep::PickDelay,
                DuressEnrollStep::Confirm,
            ]
        );
    }

    #[test]
    fn enroll_step_order_walks_the_sequence_and_saturates() {
        let order = [
            DuressEnrollStep::SetDuressPin,
            DuressEnrollStep::SetAllClear,
            DuressEnrollStep::SetCrkPassword,
            DuressEnrollStep::PickDelay,
            DuressEnrollStep::Confirm,
        ];
        let mut s = DuressEnrollStep::SetDuressPin;
        for expected in &order[1..] {
            s = next_enroll_step(s);
            assert_eq!(s, *expected);
        }
        // Saturates at both ends.
        assert_eq!(
            next_enroll_step(DuressEnrollStep::Confirm),
            DuressEnrollStep::Confirm
        );
        assert_eq!(
            prev_enroll_step(DuressEnrollStep::SetDuressPin),
            DuressEnrollStep::SetDuressPin
        );
        // Backward step.
        assert_eq!(
            prev_enroll_step(DuressEnrollStep::Confirm),
            DuressEnrollStep::PickDelay
        );
    }

    /// Terse `DuressCube` fixture for the backup-ack / gate tests. Defaults to
    /// a vaultless, fully-backed-up local Cube; callers override the fields
    /// they care about.
    fn duress_cube(name: &str, has_recovery_kit: bool) -> DuressCube {
        DuressCube {
            server_id: 0,
            uuid: Some(name.to_string()),
            name: name.to_string(),
            has_recovery_kit,
            // Vaultless mnemonic: a seed backup makes it complete.
            halves: Some((has_recovery_kit, false)),
            has_vault: Some(false),
            is_passkey: Some(false),
            local: true,
        }
    }

    #[test]
    fn duress_cube_has_recovery_kit_mirrors_list_cubes_verbatim() {
        // Symptom 2 (a keychain-only Cube tagged "No recovery kit") is fixed
        // server-side: `list_cubes` now reports `hasRecoveryKit=true` for a
        // Cube whose only backup is the phone (Keychain) envelope. The desktop
        // must carry that flag through **verbatim** — `load_duress_cubes`
        // assigns `DuressCube.has_recovery_kit = c.has_recovery_kit` (this file,
        // ~line 4161) and must never re-derive it from the password `halves`,
        // or a keychain-only backup would read as unbacked again.
        use crate::services::coincube::CubeResponse;

        // A keychain-only Cube: `hasRecoveryKit` true, but NO password halves in
        // the per-cube status (both false).
        let c: CubeResponse = serde_json::from_value(serde_json::json!({
            "id": 42,
            "uuid": "keychain-only-uuid",
            "name": "Keychain-only",
            "network": "mainnet",
            "status": "active",
            "hasRecoveryKit": true,
        }))
        .expect("list_cubes cube deserializes");
        assert!(
            c.has_recovery_kit,
            "hasRecoveryKit must deserialize to has_recovery_kit"
        );

        // Model the loader's assignment: a plain copy of the server flag, with
        // the password halves empty (keychain-only).
        let dc = DuressCube {
            server_id: c.id,
            uuid: Some(c.uuid),
            name: c.name,
            has_recovery_kit: c.has_recovery_kit,
            halves: Some((false, false)),
            has_vault: Some(true),
            is_passkey: Some(false),
            local: true,
        };
        // The flag is mirrored verbatim and stays independent of the password
        // halves — true even though both halves are absent.
        assert_eq!(dc.has_recovery_kit, c.has_recovery_kit);
        assert!(
            dc.has_recovery_kit && dc.halves == Some((false, false)),
            "has_recovery_kit must come from list_cubes, not the password halves"
        );
    }

    fn local_shape(uuid: &str, has_vault: bool) -> LocalCubeShape {
        LocalCubeShape {
            uuid: uuid.to_string(),
            name: uuid.to_string(),
            has_vault,
            is_passkey: false,
        }
    }

    #[test]
    fn append_local_only_rows_fails_closed_on_unregistered_vault_cube() {
        // A local mainnet Vault Cube absent from the server list (unregistered /
        // remote_synced == false) is armed by duress but invisible to the
        // server-driven gate. It must be folded in as an Unknown row so the gate
        // fails closed.
        let server = vec![duress_cube("on-server", true)];
        let rows = append_local_only_rows(server, &[local_shape("local-only", true)]);
        assert_eq!(rows.len(), 2);
        let added = rows
            .iter()
            .find(|r| r.uuid.as_deref() == Some("local-only"))
            .expect("local-only cube folded in");
        assert!(added.local);
        assert_eq!(added.halves, None, "unknown status → fails closed");
        assert!(added.blocks_gate(), "unregistered Vault Cube must block");
        assert!(duress_gate_blocked(&rows));
    }

    #[test]
    fn append_local_only_rows_leaves_vaultless_cube_non_blocking() {
        // A vaultless local-only Cube never blocks (master invariant I2).
        let rows = append_local_only_rows(Vec::new(), &[local_shape("seed-only", false)]);
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].blocks_gate());
        assert!(!duress_gate_blocked(&rows));
    }

    #[test]
    fn append_local_only_rows_does_not_duplicate_a_server_cube() {
        // A local Cube already present on the server (matched by uuid) is not
        // re-added — the server row, with its real kit status, wins.
        let server = vec![duress_cube("dup", true)];
        let rows = append_local_only_rows(server, &[local_shape("dup", false)]);
        assert_eq!(rows.len(), 1);
        assert!(
            rows[0].halves.is_some(),
            "kept the server row, not a synthetic Unknown"
        );
    }

    #[test]
    fn append_local_only_rows_noop_on_empty_local() {
        let server = vec![duress_cube("a", true), duress_cube("b", true)];
        let rows = append_local_only_rows(server, &[]);
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn completeness_truth_table() {
        use CubeBackupCompleteness::*;
        // (has_vault, halves) → expected
        // Vaultless: complete ⇔ seed.
        assert_eq!(
            cube_backup_completeness(Some(false), Some((true, false))),
            Complete
        );
        assert_eq!(
            cube_backup_completeness(Some(false), Some((true, true))),
            Complete
        );
        assert_eq!(
            cube_backup_completeness(Some(false), Some((false, false))),
            NoKit
        );
        // Descriptor-only kit on a vaultless mnemonic: seed still missing.
        assert_eq!(
            cube_backup_completeness(Some(false), Some((false, true))),
            MissingSeed
        );

        // Vault: needs both halves.
        assert_eq!(
            cube_backup_completeness(Some(true), Some((true, true))),
            Complete
        );
        assert_eq!(
            cube_backup_completeness(Some(true), Some((true, false))),
            MissingDescriptor
        );
        assert_eq!(
            cube_backup_completeness(Some(true), Some((false, true))),
            MissingSeed
        );
        assert_eq!(
            cube_backup_completeness(Some(true), Some((false, false))),
            NoKit
        );

        // Status unavailable → Unknown (never coerced to Complete).
        assert_eq!(cube_backup_completeness(Some(false), None), Unknown);
        assert_eq!(cube_backup_completeness(Some(true), None), Unknown);

        // Vault presence unknown → Unknown regardless of halves.
        assert_eq!(cube_backup_completeness(None, Some((true, true))), Unknown);
        assert_eq!(cube_backup_completeness(None, None), Unknown);
    }

    /// **I11 / I13**, as a table. The passkey rows the plan replaced:
    /// `has_vault, passkey` moved from `Complete ⇔ descriptor` to
    /// `Complete ⇔ seed && descriptor`, and `!has_vault, passkey` moved from
    /// an unconditional `Complete` to `Complete ⇔ seed`.
    ///
    /// Both now agree with the mnemonic rows of the same shape, which is the
    /// point: a passkey Cube's Kit carries the seed, so shape stopped mattering.
    /// This test is written against the *shape* rather than the (now absent)
    /// parameter so it still fails if a passkey arm is ever reintroduced via
    /// `DuressCube::completeness`.
    #[test]
    fn a_passkey_cube_is_judged_exactly_like_a_mnemonic_cube() {
        use CubeBackupCompleteness::*;

        let passkey_with_vault = |halves| DuressCube {
            has_vault: Some(true),
            is_passkey: Some(true),
            halves,
            ..duress_cube("pk", true)
        };
        let passkey_no_vault = |halves| DuressCube {
            has_vault: Some(false),
            is_passkey: Some(true),
            halves,
            ..duress_cube("pk", true)
        };

        // Vault + passkey: a descriptor-only kit no longer passes. This is the
        // row that makes the Kit carry the seed load-bearing rather than
        // optional.
        assert_eq!(
            passkey_with_vault(Some((false, true))).completeness(),
            MissingSeed
        );
        assert_eq!(
            passkey_with_vault(Some((true, true))).completeness(),
            Complete
        );
        assert_eq!(
            passkey_with_vault(Some((true, false))).completeness(),
            MissingDescriptor
        );
        assert_eq!(
            passkey_with_vault(Some((false, false))).completeness(),
            NoKit
        );

        // No Vault + passkey: no longer "nothing to back up". There is now
        // exactly one thing to back up, and it is the seed.
        assert_eq!(
            passkey_no_vault(Some((true, false))).completeness(),
            Complete
        );
        assert_eq!(passkey_no_vault(Some((false, false))).completeness(), NoKit);
        assert_eq!(
            passkey_no_vault(Some((false, true))).completeness(),
            MissingSeed
        );
        // And an unavailable status is Unknown, where it used to be Complete —
        // a passkey Cube can no longer pass the gate on shape alone.
        assert_eq!(passkey_no_vault(None).completeness(), Unknown);
    }

    /// A Cube on another device (shape unknown) now gets a real verdict rather
    /// than the `Unknown` the dual-evaluation fallback used to produce for
    /// every shape-dependent row.
    #[test]
    fn an_unknown_shape_no_longer_forces_unknown() {
        use CubeBackupCompleteness::*;
        let other_device = |halves| DuressCube {
            has_vault: Some(true),
            is_passkey: None,
            halves,
            ..duress_cube("remote", true)
        };
        assert_eq!(
            other_device(Some((false, true))).completeness(),
            MissingSeed,
            "this row read Unknown while passkey and mnemonic disagreed on it"
        );
        assert_eq!(other_device(Some((true, true))).completeness(), Complete);
    }

    /// `RecoveryKitStatus` fixture: password-kit halves plus the artifact
    /// kinds (if any) sealed to the owner's phone key. Empty kinds → no
    /// `ownerSelf` block, as the API reports for a never-enrolled Cube.
    fn kit_status(
        pw_seed: bool,
        pw_descriptor: bool,
        envelope_kinds: &[&str],
    ) -> RecoveryKitStatus {
        RecoveryKitStatus {
            has_recovery_kit: pw_seed || pw_descriptor || !envelope_kinds.is_empty(),
            has_encrypted_seed: pw_seed,
            has_encrypted_wallet_descriptor: pw_descriptor,
            encryption_scheme: String::new(),
            created_at: None,
            updated_at: None,
            owner_self: if envelope_kinds.is_empty() {
                None
            } else {
                Some(OwnerSelfRecoverySummary {
                    has_recipient: true,
                    tier: "full_cube".to_string(),
                    envelope_kinds: envelope_kinds.iter().map(|s| s.to_string()).collect(),
                    updated_at: None,
                })
            },
        }
    }

    /// The QA defect (2026-07-22): a keychain-encrypted (phone-mode-only)
    /// Recovery Kit — no password blobs, both artifact kinds sealed to the
    /// owner's phone key — must count both halves as backed up, so the
    /// checklist reads "Backed up" and the duress gate opens.
    #[test]
    fn duress_kit_halves_counts_phone_only_kit() {
        assert_eq!(
            duress_kit_halves(&kit_status(false, false, &["seed", "descriptor"])),
            (true, true)
        );
    }

    #[test]
    fn duress_kit_halves_password_only_unchanged() {
        assert_eq!(
            duress_kit_halves(&kit_status(true, true, &[])),
            (true, true)
        );
        assert_eq!(
            duress_kit_halves(&kit_status(true, false, &[])),
            (true, false)
        );
        assert_eq!(
            duress_kit_halves(&kit_status(false, false, &[])),
            (false, false)
        );
    }

    #[test]
    fn duress_kit_halves_modes_combine_per_half() {
        // Password seed + phone-sealed descriptor → both halves covered.
        assert_eq!(
            duress_kit_halves(&kit_status(true, false, &["descriptor"])),
            (true, true)
        );
        // Phone sealed the seed only → the descriptor half is still missing.
        assert_eq!(
            duress_kit_halves(&kit_status(false, false, &["seed"])),
            (true, false)
        );
        // An unrecognised kind never satisfies a half.
        assert_eq!(
            duress_kit_halves(&kit_status(false, false, &["mystery"])),
            (false, false)
        );
    }

    /// A registered recipient with nothing sealed carries no restorable
    /// material and must not satisfy either half (mirrors
    /// `backup_methods_present`'s envelope-uploaded rule).
    #[test]
    fn duress_kit_halves_ignores_unsealed_recipient() {
        let mut s = kit_status(false, false, &[]);
        s.owner_self = Some(OwnerSelfRecoverySummary {
            has_recipient: true,
            tier: "full_cube".to_string(),
            envelope_kinds: vec![],
            updated_at: None,
        });
        assert_eq!(duress_kit_halves(&s), (false, false));
    }

    /// Kinds without a registered recipient are not restorable by phone
    /// recovery (mirrors `derive_phone_recovery`'s `has_recipient` gate in
    /// `home.rs`), so the duress gate must fail closed rather than read
    /// "Complete" for material Connect could not restore after a wipe.
    #[test]
    fn duress_kit_halves_ignores_recipientless_kinds() {
        let mut s = kit_status(false, false, &[]);
        s.owner_self = Some(OwnerSelfRecoverySummary {
            has_recipient: false,
            tier: "full_cube".to_string(),
            envelope_kinds: vec!["seed".to_string(), "descriptor".to_string()],
            updated_at: None,
        });
        assert_eq!(duress_kit_halves(&s), (false, false));
    }

    #[test]
    fn merge_vault_shape_precedence_local_over_server() {
        let local_vault = LocalCubeShape {
            uuid: "u".into(),
            name: "u".into(),
            has_vault: true,
            is_passkey: false,
        };
        let local_vaultless = LocalCubeShape {
            uuid: "u".into(),
            name: "u".into(),
            has_vault: false,
            is_passkey: true,
        };

        // Local present → local wins, even when the server disagrees.
        assert_eq!(
            merge_cube_vault_shape(Some(&local_vault), Some(false)),
            (Some(true), Some(false), true)
        );
        assert_eq!(
            merge_cube_vault_shape(Some(&local_vaultless), Some(true)),
            (Some(false), Some(true), true)
        );

        // No local match → fall back to the server value (other-device Cube).
        assert_eq!(
            merge_cube_vault_shape(None, Some(true)),
            (Some(true), None, false)
        );
        // No local, no server field (older API) → Unknown vault presence.
        assert_eq!(merge_cube_vault_shape(None, None), (None, None, false));
    }

    /// This test used to be called `completeness_with_unknown_passkey_shape`
    /// and it pinned the dual-evaluation fallback: an other-device Cube whose
    /// shape was unknown had to be read both ways, and the shape-dependent rows
    /// fell back to `Unknown` because passkey and mnemonic disagreed on them.
    ///
    /// Since **I11** they cannot disagree — a passkey Cube's Kit carries the
    /// seed, so there is one rule — and every row that used to be `Unknown`
    /// for want of a shape now gets a real verdict. The rows that stay
    /// `Unknown` are the ones that always deserved it: no kit status fetched,
    /// or vault presence itself unknown.
    #[test]
    fn completeness_no_longer_needs_to_know_the_shape() {
        use CubeBackupCompleteness::*;

        // Helper: an other-device Vault Cube (is_passkey unknown) with the
        // given kit halves.
        let other_device_vault = |halves: Option<(bool, bool)>| DuressCube {
            has_vault: Some(true),
            is_passkey: None,
            local: false,
            halves,
            ..duress_cube("other", true)
        };

        // Unchanged: unambiguous under the old dual evaluation too.
        assert_eq!(
            other_device_vault(Some((true, true))).completeness(),
            Complete
        );
        assert_eq!(
            other_device_vault(Some((false, false))).completeness(),
            NoKit
        );

        // Descriptor-only kit. Was `Unknown` (mnemonic said MissingSeed,
        // passkey said Complete). Now MissingSeed for both, so the gate can
        // name the missing half instead of saying "couldn't verify".
        assert_eq!(
            other_device_vault(Some((false, true))).completeness(),
            MissingSeed
        );
        // Seed-only kit. Was `Unknown` (MissingDescriptor vs NoKit).
        assert_eq!(
            other_device_vault(Some((true, false))).completeness(),
            MissingDescriptor
        );
        // Still Unknown, and rightly: no status was fetched. Never coerced to
        // Complete.
        assert_eq!(other_device_vault(None).completeness(), Unknown);

        // A known-passkey (this-device) Cube reads identically — the point of
        // the collapse. Descriptor-only used to be `Complete` here; it is the
        // row **I13** changed, and the reason the Kit upload path had to change
        // in the same PR.
        let local_passkey_vault = DuressCube {
            has_vault: Some(true),
            is_passkey: Some(true),
            halves: Some((false, true)),
            ..duress_cube("local-passkey", true)
        };
        assert_eq!(local_passkey_vault.completeness(), MissingSeed);
    }

    #[test]
    fn gate_blocks_only_incomplete_vault_cubes() {
        use CubeBackupCompleteness::*;

        // Vaultless Cube never blocks, even with no kit.
        let vaultless_nokit = DuressCube {
            has_vault: Some(false),
            halves: Some((false, false)),
            ..duress_cube("vaultless", false)
        };
        assert_eq!(vaultless_nokit.completeness(), NoKit);
        assert!(!vaultless_nokit.blocks_gate());

        // Vault Cube, complete → no block.
        let vault_complete = DuressCube {
            has_vault: Some(true),
            halves: Some((true, true)),
            ..duress_cube("vault-ok", true)
        };
        assert!(!vault_complete.blocks_gate());

        // Vault Cube, incomplete → block.
        let vault_incomplete = DuressCube {
            has_vault: Some(true),
            halves: Some((true, false)),
            ..duress_cube("vault-bad", true)
        };
        assert!(vault_incomplete.blocks_gate());

        // Vault Cube, status unknown → block (fails closed).
        let vault_unknown_status = DuressCube {
            has_vault: Some(true),
            halves: None,
            ..duress_cube("vault-unknown", true)
        };
        assert!(vault_unknown_status.blocks_gate());

        // Vault presence unknown (other-device) → block (fails closed).
        let other_device = DuressCube {
            has_vault: None,
            is_passkey: None,
            local: false,
            halves: Some((true, true)),
            ..duress_cube("other", true)
        };
        assert!(other_device.blocks_gate());

        // Slice-level gate: any blocker blocks.
        assert!(!duress_gate_blocked(&[
            vaultless_nokit.clone(),
            vault_complete.clone()
        ]));
        assert!(duress_gate_blocked(&[
            vault_complete.clone(),
            vault_incomplete
        ]));
    }

    #[test]
    fn tier1_gate_fails_closed_until_checklist_loads() {
        // Not loaded (or a failed fetch left `None`) → blocked. No fail-open
        // window before the fleet is verified (master I7 / decision 4).
        assert!(duress_tier1_gate_blocked(None));

        // Loaded but empty → nothing to block on → allowed.
        assert!(!duress_tier1_gate_blocked(Some(&[])));

        // Loaded, all complete → allowed.
        let complete = DuressCube {
            has_vault: Some(true),
            halves: Some((true, true)),
            ..duress_cube("ok", true)
        };
        assert!(!duress_tier1_gate_blocked(Some(std::slice::from_ref(
            &complete
        ))));

        // Loaded, an incomplete Vault Cube → blocked.
        let incomplete = DuressCube {
            has_vault: Some(true),
            halves: Some((true, false)),
            ..duress_cube("bad", true)
        };
        assert!(duress_tier1_gate_blocked(Some(&[complete, incomplete])));
    }

    #[test]
    fn invalidating_checklist_fails_the_gate_closed() {
        // Simulates a mid-session Vault creation: the cached checklist still
        // shows this Cube as vaultless (and therefore never-blocking), which
        // would open the Tier-1 gate on stale data. Invalidating the cache
        // must drop it to `None` so the gate fails closed until a fresh fetch
        // reflects the new Vault (PLAN-duress-vault-gate PR 3).
        let mut panel = ConnectAccountPanel::new();

        // Stale cache: a vaultless-complete Cube — gate would be OPEN.
        panel.duress_cubes = Some(vec![DuressCube {
            has_vault: Some(false),
            halves: Some((true, false)),
            ..duress_cube("stale", true)
        }]);
        assert!(!duress_tier1_gate_blocked(panel.duress_cubes.as_deref()));

        // After a Vault is created, the cache is invalidated → gate blocks.
        panel.invalidate_duress_cubes();
        assert!(panel.duress_cubes.is_none());
        assert!(duress_tier1_gate_blocked(panel.duress_cubes.as_deref()));
    }

    #[test]
    fn submit_enrollment_rechecks_the_vault_gate() {
        // The gate is enforced at StartEnrollment, but state can change while
        // the wizard is open (a mid-wizard Vault creation invalidates the
        // checklist). SubmitEnrollment must re-check the single gated path and
        // refuse.
        let mut panel = ConnectAccountPanel::new();
        panel.duress_enroll = Some(state(DuressEnrollStep::Confirm));

        // A Vault Cube with an incomplete kit is now in the checklist.
        panel.duress_cubes = Some(vec![DuressCube {
            has_vault: Some(true),
            halves: Some((true, false)), // descriptor missing → blocks
            ..duress_cube("vault", true)
        }]);

        let _ = panel.update_duress(DuressMessage::SubmitEnrollment);

        // Enrollment did not proceed: wizard still open, not submitting, and
        // the block is surfaced on the wizard.
        let e = panel.duress_enroll.as_ref().expect("wizard stays open");
        assert!(!e.submitting);
        assert!(e.error.is_some());

        // Fail-closed on an invalidated (None) checklist too — the exact
        // mid-wizard-Vault-creation case.
        panel.duress_enroll = Some(state(DuressEnrollStep::Confirm));
        panel.invalidate_duress_cubes();
        let _ = panel.update_duress(DuressMessage::SubmitEnrollment);
        let e = panel.duress_enroll.as_ref().expect("wizard stays open");
        assert!(!e.submitting);
        assert!(e.error.is_some());
    }

    #[test]
    fn stale_cubes_fetch_cannot_overwrite_a_newer_one() {
        // Two checklist fetches in flight in one session share
        // `session_generation`; the per-fetch `duress_cubes_seq` is what stops
        // an older one landing last and restoring stale (e.g. pre-Vault) data.
        let mut panel = ConnectAccountPanel::new();

        // Simulate: fetch A was issued (seq 1), then a newer fetch B (seq 2)
        // was issued and has already applied a fresh, gate-blocking checklist.
        panel.duress_cubes_seq = 2;
        let fresh = vec![DuressCube {
            has_vault: Some(true),
            halves: Some((true, false)), // Vault, descriptor missing → blocks
            ..duress_cube("vault", true)
        }];
        panel.duress_cubes = Some(fresh);

        // The older fetch A now lands (seq 1) carrying stale vaultless data.
        let stale = vec![DuressCube {
            has_vault: Some(false),
            halves: Some((true, false)),
            ..duress_cube("vault", true)
        }];
        let gen = panel.session_generation;
        let _ = panel.update_duress(DuressMessage::CubesLoaded(Some(stale), gen, 1));

        // Dropped — the gate stays blocked, not reopened by the stale fetch.
        assert!(duress_tier1_gate_blocked(panel.duress_cubes.as_deref()));

        // A fetch matching the current seq still applies.
        let newer = vec![DuressCube {
            has_vault: Some(true),
            halves: Some((true, true)), // complete → unblocks
            ..duress_cube("vault", true)
        }];
        let _ = panel.update_duress(DuressMessage::CubesLoaded(Some(newer), gen, 2));
        assert!(!duress_tier1_gate_blocked(panel.duress_cubes.as_deref()));
    }

    #[test]
    fn validate_duress_pin_step_requires_non_empty_and_matching_confirm() {
        // This test reads and writes the process-global session (`open` /
        // `close` below), so it runs under the same guard every other
        // session-touching test takes. Without it, a `session.rs` test that
        // has `cube-a` open under PIN 1234 at the same moment turns the
        // "no Cube open" assertion below into the distance-rule refusal.
        let _g = crate::app::session::test_guard();
        crate::app::session::close();
        // The "distance from your regular PIN" rule is gone — any non-empty
        // PIN entered twice passes the step. Collision with a real Cube PIN is
        // enforced at persist time (`persist_duress_enrollment`), where Cube
        // PIN hashes are available.
        let mut s = state(DuressEnrollStep::SetDuressPin);
        assert!(validate_enroll_step(&s).is_err());
        s.duress_pin = "1235".to_string();
        // Confirmation still empty → mismatch.
        assert!(validate_enroll_step(&s).is_err());
        s.duress_pin_confirm = "1235".to_string();

        // With no Cube open there is no regular PIN to measure against, and the
        // I3 distance rule now **fails closed** rather than skipping — so this
        // step refuses here. Silently accepting would be the rule not existing.
        let err = validate_enroll_step(&s).unwrap_err();
        assert!(err.contains("re-open the Cube"), "{}", err);

        // With a session open, the same PIN is judged on its distance from that
        // Cube's real one.
        crate::app::session::open("cube-a", zeroize::Zeroizing::new("8888".to_string()));
        assert!(validate_enroll_step(&s).is_ok());
        crate::app::session::open("cube-a", zeroize::Zeroizing::new("1234".to_string()));
        assert!(
            validate_enroll_step(&s).is_err(),
            "1235 is one keystroke from 1234 and must be refused"
        );
        crate::app::session::close();
    }

    #[test]
    fn confirm_requires_memorized_checkbox() {
        let mut s = state(DuressEnrollStep::Confirm);
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
    use crate::services::coincube::{PlanEntitlements, PlanFeatureInfo};

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
            entitlements: PlanEntitlements::default(),
            billing_cycle: cycle,
            plan_provenance: None,
            pending_plan: None,
            pending_plan_at: None,
            pending_billing_cycle: None,
        }
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
                entitlements: None,
            }],
            pricing_schema_version: version,
            purchasing_enabled,
            marketplace_enabled: None,
            liquid_enabled: None,
            buy_sell_enabled: None,
            p2p_enabled: None,
            duress_enabled: None,
            bitcoin_blake2b_enabled: None,
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

    // ── Plan provenance: server-driven display contract (v2) ──────────
    #[test]
    fn plan_provenance_deserializes_from_wire() {
        // Pure display passthrough — the only thing to verify is that the
        // `{label, expires_at, badge}` contract parses. `ConnectPlan` is
        // camelCase; `expiresAt` exercises `PlanProvenance`'s camelCase too.
        let json = r#"{
            "plan": "estate",
            "status": "active",
            "renewalAt": "2027-07-04T00:00:00Z",
            "entitlements": {
                "freeSigningKeyCount": 0, "policyEditing": true, "legacyInvites": true,
                "linkedKeychains": true, "duressRemoteLock": true, "businessOrgs": true
            },
            "planProvenance": {
                "label": "Free for your first year",
                "expiresAt": "2027-07-04T00:00:00Z",
                "badge": "Founding member"
            }
        }"#;
        let p: ConnectPlan = serde_json::from_str(json).unwrap();
        let prov = p.plan_provenance.expect("provenance present");
        assert_eq!(prov.label, "Free for your first year");
        assert_eq!(prov.badge.as_deref(), Some("Founding member"));
        assert_eq!(prov.expires_at.as_deref(), Some("2027-07-04T00:00:00Z"));
    }

    #[test]
    fn plan_without_provenance_is_backward_compatible() {
        // Older API omits `plan_provenance` → `None` → existing paid/free UX.
        let json = r#"{
            "plan": "pro",
            "status": "active",
            "renewalAt": null,
            "entitlements": {
                "freeSigningKeyCount": 0, "policyEditing": false, "legacyInvites": false,
                "linkedKeychains": false, "duressRemoteLock": false, "businessOrgs": false
            }
        }"#;
        let p: ConnectPlan = serde_json::from_str(json).unwrap();
        assert!(p.plan_provenance.is_none());
    }

    // ── Campaign code redemption (server-driven, v2) ──────────────────
    #[test]
    fn campaign_code_change_clears_prior_result() {
        let mut panel = ConnectAccountPanel::new();
        panel.campaign_redeem.result = Some(Err("invalid".into()));
        let _ = panel.update_message(ConnectAccountMessage::CampaignCodeChanged("ABC".into()));
        assert_eq!(panel.campaign_redeem.code, "ABC");
        assert!(panel.campaign_redeem.result.is_none());
    }

    #[test]
    fn code_edits_ignored_while_redeem_in_flight() {
        // While a redeem is submitting, edits are dropped so the result that
        // comes back can't land against newly-typed text.
        let mut panel = ConnectAccountPanel::new();
        panel.campaign_redeem.code = "ABC".into();
        panel.campaign_redeem.submitting = true;
        let _ = panel.update_message(ConnectAccountMessage::CampaignCodeChanged("XYZ".into()));
        assert_eq!(panel.campaign_redeem.code, "ABC");
    }

    #[test]
    fn redeem_empty_code_is_noop() {
        let mut panel = ConnectAccountPanel::new();
        panel.campaign_redeem.code = "   ".into();
        let _ = panel.update_message(ConnectAccountMessage::RedeemCampaignCode);
        assert!(!panel.campaign_redeem.submitting);
    }

    #[test]
    fn redeem_nonempty_marks_submitting_and_clears_result() {
        let mut panel = ConnectAccountPanel::new();
        panel.campaign_redeem.code = "FOUNDER".into();
        panel.campaign_redeem.result = Some(Err("old".into()));
        let _ = panel.update_message(ConnectAccountMessage::RedeemCampaignCode);
        assert!(panel.campaign_redeem.submitting);
        assert!(panel.campaign_redeem.result.is_none());
    }

    #[test]
    fn redeem_success_records_message_and_clears_code() {
        let mut panel = ConnectAccountPanel::new();
        assert!(!panel.show_campaign_redeem);
        panel.campaign_redeem.code = "FOUNDER".into();
        panel.campaign_redeem.submitting = true;
        let gen = panel.session_generation();
        let _ = panel.update_message(ConnectAccountMessage::CampaignRedeemed(
            Ok("Estate unlocked".into()),
            gen,
        ));
        assert!(!panel.campaign_redeem.submitting);
        assert_eq!(
            panel.campaign_redeem.result,
            Some(Ok("Estate unlocked".to_string()))
        );
        assert!(panel.campaign_redeem.code.is_empty());
        assert!(panel.show_campaign_redeem);
    }

    #[test]
    fn redeem_failure_records_error_and_keeps_code() {
        // The error message is the server's typed taxonomy string, rendered
        // verbatim; the code stays so the user can correct and retry.
        let mut panel = ConnectAccountPanel::new();
        assert!(!panel.show_campaign_redeem);
        panel.campaign_redeem.code = "BADCODE".into();
        panel.campaign_redeem.submitting = true;
        let gen = panel.session_generation();
        let _ = panel.update_message(ConnectAccountMessage::CampaignRedeemed(
            Err("This code has already been redeemed.".into()),
            gen,
        ));
        assert!(!panel.campaign_redeem.submitting);
        assert_eq!(
            panel.campaign_redeem.result,
            Some(Err("This code has already been redeemed.".to_string()))
        );
        assert_eq!(panel.campaign_redeem.code, "BADCODE");
        assert!(panel.show_campaign_redeem);
    }

    #[test]
    fn redeem_result_from_stale_generation_ignored() {
        let mut panel = ConnectAccountPanel::new();
        panel.campaign_redeem.submitting = true;
        let _ = panel.update_message(ConnectAccountMessage::CampaignRedeemed(
            Ok("late".into()),
            999, // not the current session generation
        ));
        // The result is dropped (wrong session)...
        assert!(panel.campaign_redeem.result.is_none());
        // ...but the in-flight flag is cleared so the field doesn't stick on
        // "Redeeming…" after a session change.
        assert!(!panel.campaign_redeem.submitting);
        assert!(!panel.show_campaign_redeem);
    }

    #[test]
    fn signup_redemption_reveals_current_result_when_returning_to_plan_management() {
        for result in [
            Ok("Estate unlocked".to_string()),
            Err("Code expired".to_string()),
        ] {
            let mut panel = ConnectAccountPanel::new();
            panel.pending_campaign_code = Some("FOUNDER".into());
            assert!(!panel.show_campaign_redeem);
            let _ = panel.update_message(ConnectAccountMessage::SessionLoaded {
                user: User {
                    id: 1,
                    email: "founder@example.invalid".into(),
                    email_verified: Some(true),
                },
                plan: None,
                epoch: 0,
            });
            assert!(panel.campaign_redeem.submitting);
            let current = panel.session_generation();

            // A late completion from an earlier session cannot open the
            // current account's disclosure or expose its result.
            let _ = panel.update_message(ConnectAccountMessage::CampaignRedeemed(
                Ok("old account result".into()),
                current - 1,
            ));
            assert!(!panel.show_campaign_redeem);
            assert!(panel.campaign_redeem.result.is_none());

            let _ = panel.update_message(ConnectAccountMessage::CampaignRedeemed(
                result.clone(),
                current,
            ));
            assert_eq!(panel.campaign_redeem.result, Some(result));
            assert!(panel.show_campaign_redeem);
            let _ = panel.update_message(ConnectAccountMessage::OpenPlanBilling);
            assert!(matches!(panel.active_sub, ConnectSubMenu::PlanBilling));
            assert!(!panel.show_billing_history);
            assert!(!panel.show_plan_comparison);
        }
    }

    #[test]
    fn register_code_is_stashed_trimmed_on_submit() {
        let mut panel = ConnectAccountPanel::new();
        panel.step = ConnectFlowStep::Register {
            email: "founder@example.com".into(),
            loading: false,
        };
        panel.register_campaign_code = "  FOUNDER  ".into();
        let _ = panel.update_message(ConnectAccountMessage::SubmitRegistration);
        assert_eq!(panel.pending_campaign_code.as_deref(), Some("FOUNDER"));
    }

    #[test]
    fn register_code_edit_ignored_while_signup_in_flight() {
        // After SubmitRegistration (loading=true), the code is already stashed
        // in pending_campaign_code; editing the visible field must not change
        // what gets redeemed.
        let mut panel = ConnectAccountPanel::new();
        panel.register_campaign_code = "OLD".into();
        panel.step = ConnectFlowStep::Register {
            email: "founder@example.com".into(),
            loading: true,
        };
        let _ = panel.update_message(ConnectAccountMessage::RegisterCampaignCodeChanged(
            "NEW".into(),
        ));
        assert_eq!(panel.register_campaign_code, "OLD");
        // Still editable before submit (loading=false).
        panel.step = ConnectFlowStep::Register {
            email: "founder@example.com".into(),
            loading: false,
        };
        let _ = panel.update_message(ConnectAccountMessage::RegisterCampaignCodeChanged(
            "NEW".into(),
        ));
        assert_eq!(panel.register_campaign_code, "NEW");
    }

    #[test]
    fn empty_register_code_stashes_nothing() {
        let mut panel = ConnectAccountPanel::new();
        panel.step = ConnectFlowStep::Register {
            email: "founder@example.com".into(),
            loading: false,
        };
        panel.register_campaign_code = "   ".into();
        let _ = panel.update_message(ConnectAccountMessage::SubmitRegistration);
        assert!(panel.pending_campaign_code.is_none());
    }

    #[test]
    fn signup_redeem_surfaces_code_for_retry() {
        // The deferred (account-creation) redeem must populate the
        // Settings → Plan field so a failure shows the error against the code
        // and the user can retry/edit without retyping.
        let mut panel = ConnectAccountPanel::new();
        panel.pending_campaign_code = Some("FOUNDER".into());
        let user = User {
            id: 1,
            email: "founder@example.com".into(),
            email_verified: Some(true),
        };
        let _ = panel.update_message(ConnectAccountMessage::SessionLoaded {
            user,
            plan: None,
            epoch: 0,
        });
        assert!(panel.pending_campaign_code.is_none(), "code consumed");
        assert!(panel.campaign_redeem.submitting, "redeem in flight");
        assert_eq!(
            panel.campaign_redeem.code, "FOUNDER",
            "code surfaced for retry/edit"
        );
    }

    #[test]
    fn logout_clears_campaign_state() {
        let mut panel = ConnectAccountPanel::new();
        panel.campaign_redeem.code = "X".into();
        panel.campaign_redeem.result = Some(Ok("y".into()));
        panel.register_campaign_code = "Z".into();
        panel.pending_campaign_code = Some("W".into());
        let _ = panel.update_message(ConnectAccountMessage::LogOut);
        assert!(panel.campaign_redeem.code.is_empty());
        assert!(panel.campaign_redeem.result.is_none());
        assert!(panel.register_campaign_code.is_empty());
        assert!(panel.pending_campaign_code.is_none());
    }

    #[test]
    fn init_drops_pending_code_from_abandoned_signup() {
        // Regression: a code stashed by SubmitRegistration must not survive a
        // re-init (navigate away & back) — otherwise a restored or next
        // session would redeem it against the wrong account. Both Init
        // branches (keyring restore / show-login) clear it, so the assertion
        // holds regardless of stored-session state.
        let mut panel = ConnectAccountPanel::new();
        panel.step = ConnectFlowStep::OtpVerification {
            email: "founder@example.com".into(),
            otp: String::new(),
            sending: false,
            is_signup: true,
            cooldown: 0,
        };
        panel.pending_campaign_code = Some("FOUNDER".into());
        let _ = panel.update_message(ConnectAccountMessage::Init);
        assert!(panel.pending_campaign_code.is_none());
    }

    #[test]
    fn init_during_the_duress_gate_does_not_restart_the_session() {
        // A sign-in rebroadcast can reach a tab whose own session is loaded and
        // whose duress gate is still running. A keyring refresh there abandons
        // the gate and mints a session whose broadcast starts the cycle again.
        let session = StoredSession {
            login: serde_json::from_value(serde_json::json!({
                "requires_2fa":false,"token":"synthetic","refresh_token":"synthetic-refresh",
                "user":{"id":7,"email":"fixture@example.invalid","email_verified":true}
            }))
            .unwrap(),
        };
        let mut panel = ConnectAccountPanel::new();
        panel.save_session_to_keyring(&session);
        panel.step = ConnectFlowStep::CheckingDuress {
            status: DuressGateStatus::Checking,
        };
        assert_eq!(
            panel.update_message(ConnectAccountMessage::Init).units(),
            0,
            "no refresh while the gate is in flight"
        );
        assert!(matches!(
            panel.step,
            ConnectFlowStep::CheckingDuress {
                status: DuressGateStatus::Checking
            }
        ));
        // A settled gate failure is still retried from Init.
        panel.step = ConnectFlowStep::CheckingDuress {
            status: DuressGateStatus::Unreachable,
        };
        assert!(panel.update_message(ConnectAccountMessage::Init).units() > 0);
        assert!(matches!(
            panel.step,
            ConnectFlowStep::Login { loading: true, .. }
        ));
    }

    #[test]
    fn submit_login_drops_pending_code_from_abandoned_signup() {
        // Regression: starting a login must drop a signup-stashed code so it
        // never redeems against the account being signed into.
        let mut panel = ConnectAccountPanel::new();
        panel.step = ConnectFlowStep::Login {
            email: "someone@example.com".into(),
            loading: false,
        };
        panel.pending_campaign_code = Some("FOUNDER".into());
        let _ = panel.update_message(ConnectAccountMessage::SubmitLogin);
        assert!(panel.pending_campaign_code.is_none());
    }

    // ── Purchasing gate (PR2 — unchanged in v2) ───────────────────────
    #[test]
    fn purchasing_enabled_requires_loaded_features() {
        let mut panel = ConnectAccountPanel::new();
        // Features not loaded yet (fetch in flight / failed) → fail closed so
        // we never offer checkout before the server's stance is known.
        assert!(!panel.purchasing_enabled());
        // Loaded but flag absent → older backend with purchasing on (the
        // existing flow stays intact for fall GA).
        panel.features = Some(features_with_purchasing(None, None));
        assert!(panel.purchasing_enabled());
        // Loaded with the flag on.
        panel.features = Some(features_with_purchasing(None, Some(true)));
        assert!(panel.purchasing_enabled());
    }

    #[test]
    fn purchasing_disabled_when_flag_false() {
        let mut panel = ConnectAccountPanel::new();
        panel.features = Some(features_with_purchasing(None, Some(false)));
        assert!(!panel.purchasing_enabled());
    }

    // ── Marketplace server flags (COIN-346) ───────────────────────────
    fn features_with_marketplace(
        marketplace: Option<bool>,
        buy_sell: Option<bool>,
        p2p: Option<bool>,
    ) -> FeaturesResponse {
        let mut f = features_with_purchasing(None, None);
        f.marketplace_enabled = marketplace;
        f.buy_sell_enabled = buy_sell;
        f.p2p_enabled = p2p;
        f
    }

    #[test]
    fn marketplace_flags_fail_closed_until_features_load() {
        let panel = ConnectAccountPanel::new();
        // Unloaded (fetch in flight / failed / unreachable) → everything off.
        assert_eq!(
            panel.marketplace_server_flags(),
            crate::app::features::MarketplaceServerFlags::OFF
        );
    }

    #[test]
    fn marketplace_flags_absent_read_as_off() {
        let mut panel = ConnectAccountPanel::new();
        // Loaded but the flags are absent (older backend) → still off, unlike
        // the purchasing gate which treats absent as on.
        panel.features = Some(features_with_marketplace(None, None, None));
        assert_eq!(
            panel.marketplace_server_flags(),
            crate::app::features::MarketplaceServerFlags::OFF
        );
    }

    #[test]
    fn marketplace_flags_cannot_override_first_release_ceiling() {
        let mut panel = ConnectAccountPanel::new();
        panel.features = Some(features_with_marketplace(
            Some(true),
            Some(true),
            Some(true),
        ));
        let flags = panel.marketplace_server_flags();
        assert_eq!(flags, crate::app::features::MarketplaceServerFlags::OFF);
        assert!(!flags.buy_sell_on());
        assert!(!flags.p2p_on());
    }

    // ── Duress launch gate (PLAN-feature-flags) ───────────────────────────
    fn features_with_duress(duress: Option<bool>) -> FeaturesResponse {
        let mut f = features_with_purchasing(None, None);
        f.duress_enabled = duress;
        f
    }

    /// A Pro plan carrying the paid `duress` entitlement.
    fn duress_entitled_plan() -> ConnectPlan {
        let mut p = plan(PlanTier::Pro, PlanStatus::Active, None, None);
        p.entitlements.duress = true;
        p
    }

    #[test]
    fn duress_flag_fail_closed_until_features_load() {
        let panel = ConnectAccountPanel::new();
        // Unloaded (fetch in flight / failed / unreachable / never signed in)
        // → the flag and the whole gate read off.
        assert!(!panel.duress_server_enabled());
        assert_eq!(panel.duress_gate(), crate::app::features::DuressGate::OFF);
        assert!(!panel.show_duress());
    }

    #[test]
    fn duress_flag_absent_reads_as_off() {
        let mut panel = ConnectAccountPanel::new();
        // Loaded but the flag is absent (older backend) → off, fail-closed.
        panel.features = Some(features_with_duress(None));
        assert!(!panel.duress_server_enabled());
    }

    #[test]
    fn duress_flag_mirrors_the_response() {
        let mut panel = ConnectAccountPanel::new();
        panel.features = Some(features_with_duress(Some(true)));
        assert!(panel.duress_server_enabled());
        panel.features = Some(features_with_duress(Some(false)));
        assert!(!panel.duress_server_enabled());
    }

    #[test]
    fn show_duress_full_matrix() {
        // show_duress() == entitled AND (server_flag OR enrolled): all 8 cells
        // of (entitled × flag × enrolled). Enrollment is supplied via the local
        // arm signal here; the cross-device server-`enrolled` path is covered
        // separately below.
        let cases = [
            // entitled, flag, enrolled, expected
            (false, false, false, false),
            (false, false, true, false),
            (false, true, false, false),
            (false, true, true, false),
            (true, false, false, false), // fresh Pro on prod (flag off) → hidden
            (true, false, true, true),   // grandfathered: enrolled beats flag-off
            (true, true, false, true),   // server granted the flag → shown
            (true, true, true, true),
        ];
        for (entitled, flag, enrolled, expected) in cases {
            let mut panel = ConnectAccountPanel::new();
            panel.plan = Some(if entitled {
                duress_entitled_plan()
            } else {
                plan(PlanTier::Free, PlanStatus::Active, None, None)
            });
            panel.features = Some(features_with_duress(Some(flag)));
            panel.duress_locally_armed = enrolled;
            assert_eq!(
                panel.show_duress(),
                expected,
                "entitled={entitled} flag={flag} enrolled={enrolled}"
            );
            // The gate half is entitlement-independent: it's the OR of inputs.
            assert_eq!(panel.duress_gate().on(), flag || enrolled);
        }
    }

    #[test]
    fn show_duress_fail_closed_before_features_load_for_unenrolled() {
        // Entitled Pro, features not yet loaded, never enrolled → hidden. This
        // is the launch-critical row: a signed-in Pro must see nothing until a
        // features fetch explicitly turns duress on.
        let mut panel = ConnectAccountPanel::new();
        panel.plan = Some(duress_entitled_plan());
        assert!(!panel.show_duress());
    }

    #[test]
    fn server_enrolled_state_grandfathers_with_flag_off() {
        // The cross-device grandfather: `duressEnabled` is off and this device
        // never armed, but the server reports the account enrolled (armed on
        // another device). The surface must stay shown. `reload_duress_state`
        // is what discovers this, which is why it is NOT gated on the gate.
        let mut panel = ConnectAccountPanel::new();
        panel.plan = Some(duress_entitled_plan());
        panel.features = Some(features_with_duress(Some(false)));
        assert!(!panel.duress_locally_armed);
        panel.duress_state = Some(crate::services::coincube::DuressState {
            active: false,
            unlock_at: None,
            enrolled: true,
            this_device_registered: false,
        });
        assert!(panel.is_duress_enrolled());
        assert!(panel.show_duress());
    }

    #[test]
    fn duress_gate_reverts_to_off_when_features_cleared() {
        // A flag granted mid-session must not survive logout: `features` is
        // dropped on `clear_session`, so the accessor reverts to off (unless the
        // account is still locally enrolled, which is the durable half).
        let mut panel = ConnectAccountPanel::new();
        panel.plan = Some(duress_entitled_plan());
        panel.features = Some(features_with_duress(Some(true)));
        assert!(panel.show_duress());
        panel.features = None;
        assert!(!panel.duress_server_enabled());
        assert!(!panel.show_duress());
    }

    #[test]
    fn scrub_duress_dialogs_zeroizes_and_drops_pending_dialogs() {
        // The teardown the route backstop runs when the surface is hidden
        // mid-flow: a non-submitting wizard and the step-up disable dialog are
        // both discarded so their secrets don't linger once duress is off.
        let mut panel = ConnectAccountPanel::new();
        panel.open_enroll_wizard();
        panel.duress_enroll.as_mut().unwrap().duress_pin = "1234".to_string();
        panel.duress_disable = Some(DuressDisableState {
            pin: "5678".to_string(),
            method: Some(crate::app::DuressStepUpMethod::Pin),
            submitting: false,
            error: None,
        });

        panel.scrub_duress_dialogs();

        assert!(panel.duress_enroll.is_none());
        assert!(panel.duress_disable.is_none());
    }

    #[test]
    fn scrub_duress_dialogs_preserves_a_submitting_wizard() {
        // A server enroll_duress in flight must survive the scrub — else its
        // code is zeroized before the success handler can persist it, leaving
        // the server enrolled but this device un-armed (mirrors CancelEnrollment).
        let mut panel = ConnectAccountPanel::new();
        panel.open_enroll_wizard();
        panel.duress_enroll.as_mut().unwrap().submitting = true;

        panel.scrub_duress_dialogs();

        assert!(panel.duress_enroll.is_some());
        assert!(panel.duress_enroll_abandoned);
    }

    #[test]
    fn abandoned_enrollment_success_still_forwards_local_arming_payload() {
        use iced::futures::StreamExt;

        let mut panel = ConnectAccountPanel::new();
        panel.open_enroll_wizard();
        let wizard = panel.duress_enroll.as_mut().unwrap();
        wizard.duress_pin = "1234".into();
        wizard.pending_code = Some("synthetic-code".into());
        wizard.submitting = true;
        panel.scrub_duress_dialogs();

        let task = panel.update_duress(DuressMessage::EnrollResult(
            Ok(()),
            panel.session_generation(),
        ));
        assert!(panel.duress_enroll.is_none());
        assert!(!panel.duress_enroll_abandoned);
        let mut stream = iced_runtime::task::into_stream(task).expect("local arming task");
        let result = iced::futures::executor::block_on(stream.next());
        let Some(iced_runtime::Action::Output(Message::CompleteDuressEnrollment(payload))) = result
        else {
            panic!("successful enrollment must arm this device");
        };
        assert_eq!(payload.duress_pin.as_str(), "1234");
        assert_eq!(payload.duress_code.as_str(), "synthetic-code");
    }

    #[test]
    fn admitted_client_replacement_scrubs_abandoned_enrollment_and_allows_new_wizard() {
        let mut panel = ConnectAccountPanel::new();
        panel.open_enroll_wizard();
        let wizard = panel.duress_enroll.as_mut().unwrap();
        wizard.duress_pin = "1234".into();
        wizard.all_clear = "synthetic-all-clear".into();
        wizard.pending_code = Some("synthetic-code".into());
        wizard.submitting = true;
        panel.scrub_duress_dialogs();
        let old_generation = panel.session_generation();
        assert!(panel.duress_enroll_abandoned);

        let mut replacement = CoincubeClient::new();
        replacement.set_token("replacement-bearer");
        panel.install_shared_client(replacement);
        assert_ne!(panel.session_generation(), old_generation);
        assert!(panel.duress_enroll.is_none());
        assert!(!panel.duress_enroll_abandoned);

        let stale = panel.update_duress(DuressMessage::EnrollResult(Ok(()), old_generation));
        assert!(iced_runtime::task::into_stream(stale).is_none());
        panel.step = ConnectFlowStep::Dashboard;
        panel.user = Some(crate::services::coincube::User {
            id: 2,
            email: "replacement@example.invalid".into(),
            email_verified: Some(true),
        });
        panel.plan = Some(duress_entitled_plan());
        panel.features = Some(features_with_duress(Some(true)));
        panel.duress_cubes = Some(Vec::new());
        // StartEnrollment reads the active Cube inventory. The test-only
        // datadir is intentionally absent until a fixture creates it.
        std::fs::create_dir_all(crate::dir::CoincubeDirectory::active().unwrap().path()).unwrap();
        let task = panel.update_duress(DuressMessage::StartEnrollment);
        drop(task);
        assert!(panel.duress_enroll.is_some());
        assert!(!panel.duress_enroll_abandoned);
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
    fn review_plan_change_opens_review_with_tier_and_cycle() {
        let mut panel = panel_with_plan(plan(
            PlanTier::Pro,
            PlanStatus::Active,
            Some("2027-01-01T00:00:00Z"),
            Some(BillingCycle::Monthly),
        ));
        panel.features = Some(features_with_purchasing(None, Some(true)));
        panel.show_plan_comparison = true;
        panel.selected_billing_cycle = BillingCycle::Annual;
        let _ = panel.update_message(ConnectAccountMessage::ReviewPlanChange(PlanTier::Estate));
        let cs = panel.checkout.as_ref().expect("review opened");
        assert!(matches!(cs.phase, CheckoutPhase::Review));
        assert_eq!(cs.tier, PlanTier::Estate);
        assert_eq!(cs.cycle, BillingCycle::Annual);
        assert!(!panel.show_plan_comparison);
        assert_eq!(panel.selected_plan_tier, Some(PlanTier::Estate));

        // Back returns to Change plan and drops the un-invoiced review.
        let _ = panel.update_message(ConnectAccountMessage::ComparePlans(true));
        assert!(panel.checkout.is_none());
        assert!(panel.show_plan_comparison);
    }

    #[test]
    fn review_plan_change_blocked_while_purchasing_closed_or_invoice_open() {
        let mut panel = panel_with_plan(plan(PlanTier::Free, PlanStatus::Active, None, None));
        panel.features = Some(features_with_purchasing(None, Some(false)));
        let _ = panel.update_message(ConnectAccountMessage::ReviewPlanChange(PlanTier::Pro));
        assert!(panel.checkout.is_none());

        panel.features = Some(features_with_purchasing(None, Some(true)));
        let _ = panel.update_message(ConnectAccountMessage::StartCheckout(PlanTier::Pro));
        let _ = panel.update_message(ConnectAccountMessage::ReviewPlanChange(PlanTier::Estate));
        let cs = panel.checkout.as_ref().unwrap();
        assert!(matches!(cs.phase, CheckoutPhase::Creating));
        assert_eq!(cs.tier, PlanTier::Pro);
        // Change plan never drops an invoice that is out for payment.
        let _ = panel.update_message(ConnectAccountMessage::ComparePlans(true));
        assert!(panel.checkout.is_some());
    }

    #[test]
    fn start_checkout_uses_the_reviewed_cycle() {
        let mut panel = panel_with_plan(plan(PlanTier::Free, PlanStatus::Active, None, None));
        panel.features = Some(features_with_purchasing(None, Some(true)));
        panel.selected_billing_cycle = BillingCycle::Annual;
        let _ = panel.update_message(ConnectAccountMessage::ReviewPlanChange(PlanTier::Pro));
        // The picker's toggle moving afterwards doesn't change the invoice.
        panel.selected_billing_cycle = BillingCycle::Monthly;
        let _ = panel.update_message(ConnectAccountMessage::StartCheckout(PlanTier::Pro));
        let cs = panel.checkout.as_ref().unwrap();
        assert!(matches!(cs.phase, CheckoutPhase::Creating));
        assert_eq!(cs.cycle, BillingCycle::Annual);
    }

    #[test]
    fn renew_current_plan_opens_review_for_the_current_tier_and_cycle() {
        let mut panel = panel_with_plan(plan(
            PlanTier::Pro,
            PlanStatus::Active,
            Some("2026-06-13T00:00:00Z"),
            Some(BillingCycle::Annual),
        ));
        panel.features = Some(features_with_purchasing(None, Some(true)));
        let task = panel.update_message(ConnectAccountMessage::RenewCurrentPlan);
        drop(task);
        // The task re-dispatches ReviewPlanChange; apply it as the runtime would.
        let _ = panel.update_message(ConnectAccountMessage::ReviewPlanChange(PlanTier::Pro));
        let cs = panel.checkout.as_ref().unwrap();
        assert!(matches!(cs.phase, CheckoutPhase::Review));
        assert_eq!(cs.cycle, BillingCycle::Annual);
    }

    // ── Billing v2 D3: BTCPay phases and the per-checkout poll guard ──

    fn charge_status(json: &str) -> ChargeStatusResponse {
        serde_json::from_str(json).expect("charge status parses")
    }

    fn checkout_resp() -> CheckoutResponse {
        serde_json::from_str(
            r#"{"chargeId":"inv-1","lightningInvoice":"lnbc1fullinvoice","onChainAddress":"bc1qaddr",
                "amountSats":100000,"amountFiat":100,"fiatCurrency":"USD","plan":"pro",
                "billingCycle":"annual","checkoutUrl":"https://pay.example/i/inv-1","expiresAt":null}"#,
        )
        .expect("checkout parses with a null expiresAt")
    }

    /// A panel with an invoice out for payment (checkout generation `id`).
    fn awaiting_panel() -> (ConnectAccountPanel, u64, u64) {
        let mut panel = panel_with_plan(plan(PlanTier::Free, PlanStatus::Active, None, None));
        panel.features = Some(features_with_purchasing(None, Some(true)));
        let _ = panel.update_message(ConnectAccountMessage::StartCheckout(PlanTier::Pro));
        let id = panel.checkout.as_ref().unwrap().id;
        let gen = panel.session_generation;
        let _ = panel.update_message(ConnectAccountMessage::CheckoutCreated(
            Ok(checkout_resp()),
            gen,
            id,
        ));
        assert_eq!(
            panel.checkout.as_ref().unwrap().phase,
            CheckoutPhase::AwaitingPayment
        );
        (panel, gen, id)
    }

    #[test]
    fn charge_states_map_to_btcpay_phases() {
        use CheckoutPhase as P;
        let awaiting = P::AwaitingPayment;
        let cases = [
            (
                r#"{"chargeId":"c","status":"unpaid","plan":"pro"}"#,
                P::AwaitingPayment,
                true,
                false,
            ),
            (
                r#"{"chargeId":"c","status":"processing","plan":"pro"}"#,
                P::Processing,
                true,
                false,
            ),
            (
                r#"{"chargeId":"c","status":"paid","plan":"pro"}"#,
                P::Paid { overpaid_sats: 0 },
                false,
                true,
            ),
            (
                r#"{"chargeId":"c","status":"expired","plan":"pro"}"#,
                P::Expired,
                false,
                false,
            ),
            (
                r#"{"chargeId":"c","status":"unpaid","state":"partially_paid","plan":"pro",
                    "amountReceivedSats":62000,"amountDueSats":38000}"#,
                P::PartlyPaid {
                    received_sats: 62_000,
                    due_sats: 38_000,
                },
                true,
                false,
            ),
            (
                r#"{"chargeId":"c","invoiceId":"INV9","status":"expired","state":"needs_support",
                    "plan":"pro","amountReceivedSats":5000,"additionalStatus":"PaidLate"}"#,
                P::NeedsSupport {
                    invoice_id: "INV9".into(),
                    received_sats: 5_000,
                },
                false,
                false,
            ),
            (
                r#"{"chargeId":"c","status":"paid","state":"paid","plan":"pro","overpaidSats":1200}"#,
                P::Paid {
                    overpaid_sats: 1_200,
                },
                false,
                true,
            ),
            // Unknown values keep waiting instead of failing the poll.
            (
                r#"{"chargeId":"c","status":"refunding","state":"refunding","plan":"pro"}"#,
                P::AwaitingPayment,
                true,
                false,
            ),
        ];
        for (json, phase, polls, paid) in cases {
            let status = charge_status(json);
            assert_eq!(
                next_checkout_phase(&awaiting, &status),
                (phase, polls, paid),
                "{}",
                json
            );
        }
        // Waiting on the BTCPay page stays there while unpaid.
        let unpaid = charge_status(r#"{"chargeId":"c","status":"unpaid","plan":"pro"}"#);
        assert_eq!(
            next_checkout_phase(&P::PayingInBrowser, &unpaid).0,
            P::PayingInBrowser
        );
        // Needs support falls back to the charge id when there is no invoice id.
        let support = charge_status(
            r#"{"chargeId":"c7","status":"expired","state":"needs_support","plan":"pro"}"#,
        );
        assert_eq!(
            next_checkout_phase(&awaiting, &support).0,
            P::NeedsSupport {
                invoice_id: "c7".into(),
                received_sats: 0
            }
        );
    }

    #[test]
    fn polling_continues_after_opening_the_btcpay_page() {
        let (mut panel, gen, id) = awaiting_panel();
        let _ = panel.update_message(ConnectAccountMessage::OpenCheckoutUrl(
            "https://pay.example/i/inv-1".into(),
        ));
        let cs = panel.checkout.as_ref().unwrap();
        assert_eq!(cs.phase, CheckoutPhase::PayingInBrowser);
        assert!(
            cs.phase.polls(),
            "the poll chain keeps running in the browser phase"
        );
        let _ = panel.update_message(ConnectAccountMessage::ChargeStatusUpdated(
            Ok(charge_status(
                r#"{"chargeId":"inv-1","status":"processing","plan":"pro"}"#,
            )),
            gen,
            id,
        ));
        assert_eq!(
            panel.checkout.as_ref().unwrap().phase,
            CheckoutPhase::Processing
        );
        let _ = panel.update_message(ConnectAccountMessage::ChargeStatusUpdated(
            Ok(charge_status(
                r#"{"chargeId":"inv-1","status":"paid","plan":"pro"}"#,
            )),
            gen,
            id,
        ));
        assert_eq!(
            panel.checkout.as_ref().unwrap().phase,
            CheckoutPhase::Paid { overpaid_sats: 0 }
        );
    }

    #[test]
    fn a_stale_checkout_poll_never_updates_a_new_checkout() {
        let (mut panel, gen, old_id) = awaiting_panel();
        let _ = panel.update_message(ConnectAccountMessage::DismissCheckout);
        let _ = panel.update_message(ConnectAccountMessage::StartCheckout(PlanTier::Pro));
        let new_id = panel.checkout.as_ref().unwrap().id;
        assert_ne!(old_id, new_id);
        // The dismissed invoice's poll and create results land late.
        let _ = panel.update_message(ConnectAccountMessage::ChargeStatusUpdated(
            Ok(charge_status(
                r#"{"chargeId":"inv-1","status":"paid","plan":"pro"}"#,
            )),
            gen,
            old_id,
        ));
        let _ = panel.update_message(ConnectAccountMessage::CheckoutCreated(
            Ok(checkout_resp()),
            gen,
            old_id,
        ));
        let cs = panel.checkout.as_ref().unwrap();
        assert_eq!(cs.phase, CheckoutPhase::Creating);
        assert!(cs.checkout.is_none());
    }

    #[test]
    fn three_transient_poll_errors_stop_polling_and_check_again_resumes() {
        let (mut panel, gen, id) = awaiting_panel();
        for _ in 0..2 {
            let _ = panel.update_message(ConnectAccountMessage::ChargeStatusUpdated(
                Err(("timeout".into(), false)),
                gen,
                id,
            ));
            assert_eq!(
                panel.checkout.as_ref().unwrap().phase,
                CheckoutPhase::AwaitingPayment
            );
        }
        let _ = panel.update_message(ConnectAccountMessage::ChargeStatusUpdated(
            Err(("timeout".into(), false)),
            gen,
            id,
        ));
        assert!(matches!(
            panel.checkout.as_ref().unwrap().phase,
            CheckoutPhase::PollFailed(_)
        ));
        // Check again resumes the same invoice; it never creates another.
        let _ = panel.update_message(ConnectAccountMessage::ResumeChargePolling);
        let cs = panel.checkout.as_ref().unwrap();
        assert_eq!(cs.phase, CheckoutPhase::AwaitingPayment);
        assert_eq!(cs.id, id);
        assert_eq!(cs.poll_errors, 0);
        // A terminal (4xx) error stops at once.
        let _ = panel.update_message(ConnectAccountMessage::ChargeStatusUpdated(
            Err(("not found".into(), true)),
            gen,
            id,
        ));
        assert!(matches!(
            panel.checkout.as_ref().unwrap().phase,
            CheckoutPhase::PollFailed(_)
        ));
    }

    #[test]
    fn expired_create_new_invoice_issues_a_new_checkout_for_the_same_plan() {
        let mut panel = panel_with_plan(plan(PlanTier::Free, PlanStatus::Active, None, None));
        panel.features = Some(features_with_purchasing(None, Some(true)));
        panel.selected_billing_cycle = BillingCycle::Annual;
        let _ = panel.update_message(ConnectAccountMessage::ReviewPlanChange(PlanTier::Pro));
        let _ = panel.update_message(ConnectAccountMessage::StartCheckout(PlanTier::Pro));
        let id = panel.checkout.as_ref().unwrap().id;
        let gen = panel.session_generation;
        let _ = panel.update_message(ConnectAccountMessage::CheckoutCreated(
            Ok(checkout_resp()),
            gen,
            id,
        ));
        let _ = panel.update_message(ConnectAccountMessage::ChargeStatusUpdated(
            Ok(charge_status(
                r#"{"chargeId":"inv-1","status":"expired","plan":"pro"}"#,
            )),
            gen,
            id,
        ));
        assert_eq!(
            panel.checkout.as_ref().unwrap().phase,
            CheckoutPhase::Expired
        );
        panel.selected_billing_cycle = BillingCycle::Monthly;
        let _ = panel.update_message(ConnectAccountMessage::StartCheckout(PlanTier::Pro));
        let cs = panel.checkout.as_ref().unwrap();
        assert_eq!(cs.phase, CheckoutPhase::Creating);
        assert!(cs.id > id);
        assert_eq!(cs.cycle, BillingCycle::Annual);
    }

    #[test]
    fn partly_paid_switches_to_on_chain_for_the_remaining_amount() {
        let (mut panel, gen, id) = awaiting_panel();
        let _ = panel.update_message(ConnectAccountMessage::ChargeStatusUpdated(
            Ok(charge_status(
                r#"{"chargeId":"inv-1","status":"unpaid","state":"partially_paid","plan":"pro",
                    "amountReceivedSats":62000,"amountDueSats":38000}"#,
            )),
            gen,
            id,
        ));
        let cs = panel.checkout.as_ref().unwrap();
        assert_eq!(
            cs.phase,
            CheckoutPhase::PartlyPaid {
                received_sats: 62_000,
                due_sats: 38_000
            }
        );
        assert_eq!(cs.network, PaymentNetwork::OnChain);
        assert!(cs.onchain_qr.is_some());
        assert_eq!(
            bip21_uri("bc1qaddr", 38_000),
            "bitcoin:bc1qaddr?amount=0.00038"
        );
        assert_eq!(
            bip21_uri("bc1qaddr", 100_000_000),
            "bitcoin:bc1qaddr?amount=1"
        );
    }

    #[test]
    fn start_checkout_never_replaces_an_invoice_out_for_payment() {
        let (mut panel, _gen, id) = awaiting_panel();
        let _ = panel.update_message(ConnectAccountMessage::StartCheckout(PlanTier::Pro));
        let cs = panel.checkout.as_ref().unwrap();
        assert_eq!(cs.id, id);
        assert_eq!(cs.phase, CheckoutPhase::AwaitingPayment);
    }

    #[test]
    fn a_refused_create_is_not_reported_as_a_network_failure() {
        let mut panel = panel_with_plan(plan(PlanTier::Free, PlanStatus::Active, None, None));
        panel.features = Some(features_with_purchasing(None, Some(true)));
        let _ = panel.update_message(ConnectAccountMessage::StartCheckout(PlanTier::Pro));
        let id = panel.checkout.as_ref().unwrap().id;
        let gen = panel.session_generation;
        let _ = panel.update_message(ConnectAccountMessage::CheckoutCreated(
            Err((
                "A plan change is already scheduled for the end of this period".into(),
                true,
            )),
            gen,
            id,
        ));
        assert_eq!(
            panel.checkout.as_ref().unwrap().phase,
            CheckoutPhase::Failed {
                message: "A plan change is already scheduled for the end of this period".into(),
                refused: true
            }
        );
    }

    #[test]
    fn change_plan_leaves_a_refused_checkout() {
        let mut panel = panel_with_plan(plan(PlanTier::Free, PlanStatus::Active, None, None));
        panel.features = Some(features_with_purchasing(None, Some(true)));
        let _ = panel.update_message(ConnectAccountMessage::StartCheckout(PlanTier::Pro));
        let id = panel.checkout.as_ref().unwrap().id;
        let gen = panel.session_generation;
        let _ = panel.update_message(ConnectAccountMessage::CheckoutCreated(
            Err(("A plan change is already scheduled".into(), true)),
            gen,
            id,
        ));
        let _ = panel.update_message(ConnectAccountMessage::ComparePlans(true));
        assert!(panel.checkout.is_none());
        assert!(panel.show_plan_comparison);
    }

    #[test]
    fn free_review_opens_while_purchasing_is_closed_and_review_cycle_is_editable() {
        let mut panel = panel_with_plan(plan(
            PlanTier::Pro,
            PlanStatus::Active,
            Some("2099-01-01T00:00:00Z"),
            None,
        ));
        panel.features = Some(features_with_purchasing(None, Some(false)));
        let _ = panel.update_message(ConnectAccountMessage::ReviewPlanChange(PlanTier::Free));
        assert!(panel.checkout.is_some());
        panel.checkout = None;
        panel.features = Some(features_with_purchasing(None, Some(true)));
        panel.selected_billing_cycle = BillingCycle::Monthly;
        let _ = panel.update_message(ConnectAccountMessage::ReviewPlanChange(PlanTier::Pro));
        let _ = panel.update_message(ConnectAccountMessage::SetReviewCycle(BillingCycle::Annual));
        let cs = panel.checkout.as_ref().unwrap();
        assert_eq!(cs.cycle, BillingCycle::Annual);
        assert!(cs.plan.is_some(), "Review snapshots the plan");
    }

    #[test]
    fn pending_plan_fields_parse_leniently() {
        let plan: ConnectPlan = serde_json::from_str(
            r#"{"plan":"estate","status":"active","renewalAt":"2027-03-14T00:00:00Z",
                "entitlements":{},"pendingPlan":"pro","pendingPlanAt":"2027-03-14T00:00:00Z",
                "pendingBillingCycle":"annual"}"#,
        )
        .unwrap();
        assert_eq!(plan.pending_plan, Some(PlanTier::Pro));
        assert_eq!(plan.pending_billing_cycle, Some(BillingCycle::Annual));
        // An empty or future value never drops the account to Free.
        let plan: ConnectPlan = serde_json::from_str(
            r#"{"plan":"estate","status":"active","renewalAt":null,"entitlements":{},
                "pendingPlan":"","pendingBillingCycle":"quarterly"}"#,
        )
        .unwrap();
        assert_eq!(plan.plan, PlanTier::Estate);
        assert_eq!(plan.pending_plan, None);
        assert_eq!(plan.pending_billing_cycle, None);
    }

    #[test]
    fn check_again_restores_a_partly_paid_invoice_with_its_amounts() {
        let (mut panel, gen, id) = awaiting_panel();
        let partial = charge_status(
            r#"{"chargeId":"inv-1","status":"unpaid","state":"partially_paid","plan":"pro",
                "amountReceivedSats":62000,"amountDueSats":38000}"#,
        );
        let _ = panel.update_message(ConnectAccountMessage::ChargeStatusUpdated(
            Ok(partial),
            gen,
            id,
        ));
        for _ in 0..3 {
            let _ = panel.update_message(ConnectAccountMessage::ChargeStatusUpdated(
                Err(("timeout".into(), false)),
                gen,
                id,
            ));
        }
        assert!(matches!(
            panel.checkout.as_ref().unwrap().phase,
            CheckoutPhase::PollFailed(_)
        ));
        let _ = panel.update_message(ConnectAccountMessage::ResumeChargePolling);
        // Never back to "send the full amount" after a partial payment.
        assert_eq!(
            panel.checkout.as_ref().unwrap().phase,
            CheckoutPhase::PartlyPaid {
                received_sats: 62_000,
                due_sats: 38_000
            }
        );
        // A seen payment resumes as Confirming, not as unpaid.
        let _ = panel.update_message(ConnectAccountMessage::ChargeStatusUpdated(
            Ok(charge_status(
                r#"{"chargeId":"inv-1","status":"processing","plan":"pro"}"#,
            )),
            gen,
            id,
        ));
        let _ = panel.update_message(ConnectAccountMessage::ChargeStatusUpdated(
            Err(("gone".into(), true)),
            gen,
            id,
        ));
        let _ = panel.update_message(ConnectAccountMessage::ResumeChargePolling);
        assert_eq!(
            panel.checkout.as_ref().unwrap().phase,
            CheckoutPhase::Processing
        );
    }

    #[test]
    fn an_older_plan_reload_never_overwrites_a_newer_one() {
        let mut panel = panel_with_plan(plan(PlanTier::Pro, PlanStatus::Active, None, None));
        let gen = panel.session_generation;
        let _first = panel.reload_plan_billing();
        let first = panel.plan_seq;
        let _second = panel.reload_plan_billing();
        let second = panel.plan_seq;
        let _ = panel.update_message(ConnectAccountMessage::PlanReloaded(
            Some(plan(PlanTier::Estate, PlanStatus::Active, None, None)),
            gen,
            second,
        ));
        let _ = panel.update_message(ConnectAccountMessage::PlanReloaded(
            Some(plan(PlanTier::Pro, PlanStatus::Active, None, None)),
            gen,
            first,
        ));
        assert_eq!(panel.plan.as_ref().unwrap().plan, PlanTier::Estate);
    }

    #[test]
    fn plan_loads_never_move_the_period_under_the_picker() {
        let mut panel = panel_with_plan(plan(PlanTier::Pro, PlanStatus::Active, None, None));
        let gen = panel.session_generation;
        let annual = || {
            plan(
                PlanTier::Pro,
                PlanStatus::Active,
                None,
                Some(BillingCycle::Annual),
            )
        };
        panel.show_plan_comparison = true;
        panel.selected_billing_cycle = BillingCycle::Monthly;
        let _ = panel.update_message(ConnectAccountMessage::PlanLoaded(Some(annual()), gen));
        assert_eq!(panel.selected_billing_cycle, BillingCycle::Monthly);
        panel.show_plan_comparison = false;
        let _ = panel.update_message(ConnectAccountMessage::PlanLoaded(Some(annual()), gen));
        assert_eq!(panel.selected_billing_cycle, BillingCycle::Annual);
    }

    #[test]
    fn billing_wire_types_parse_old_and_new_api_shapes() {
        use crate::services::coincube::{BillingHistoryEntry, ChargeStatus};
        // Old checkout: expiresAt null, no startsAt.
        let old = checkout_resp();
        assert_eq!(old.expires_at, None);
        assert_eq!(old.starts_at, None);
        // New checkout (A4 buy-ahead).
        let new: CheckoutResponse = serde_json::from_str(
            r#"{"chargeId":"i","lightningInvoice":"ln","onChainAddress":"bc1","amountSats":1,
                "amountFiat":10,"fiatCurrency":"USD","plan":"pro","billingCycle":"monthly",
                "checkoutUrl":"u","expiresAt":"2026-11-14T16:42:00Z","startsAt":"2027-03-14T00:00:00Z"}"#,
        )
        .unwrap();
        assert_eq!(new.starts_at.as_deref(), Some("2027-03-14T00:00:00Z"));
        // Old status: status only.
        let old = charge_status(r#"{"chargeId":"c","status":"processing","plan":"pro"}"#);
        assert_eq!(old.charge_state(), ChargeState::Processing);
        assert_eq!(old.amount_due_sats, 0);
        // Unknown legacy status.
        let odd = charge_status(r#"{"chargeId":"c","status":"underpaid","plan":"pro"}"#);
        assert_eq!(odd.status, ChargeStatus::Unknown);
        // Old history row: no chargeId, no state.
        let row: BillingHistoryEntry = serde_json::from_str(
            r#"{"plan":"pro","billingCycle":"annual","amountSats":100000,"amountFiat":100,
                "fiatCurrency":"USD","status":"paid","createdAt":"2026-07-01T00:00:00Z","paidAt":null}"#,
        )
        .unwrap();
        assert_eq!(row.charge_id, "");
        assert_eq!(row.charge_state(), ChargeState::Paid);
        // New history row.
        let row: BillingHistoryEntry = serde_json::from_str(
            r#"{"id":4,"chargeId":"c4","invoiceId":"c4","plan":"estate","billingCycle":"annual",
                "amountSats":200000,"amountFiat":200,"fiatCurrency":"USD","status":"expired",
                "state":"needs_support","amountReceivedSats":40000,"amountDueSats":0,"overpaidSats":0,
                "checkoutUrl":"u","expiresAt":null,"paidAt":null,"createdAt":"2026-11-01T00:00:00Z"}"#,
        )
        .unwrap();
        assert_eq!(row.charge_id, "c4");
        assert_eq!(row.charge_state(), ChargeState::NeedsSupport);
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
    fn start_checkout_blocked_before_features_load() {
        // Right after sign-in, features are still loading — don't open a
        // checkout until the server's purchasing stance is known.
        let mut panel = panel_with_plan(plan(PlanTier::Free, PlanStatus::Active, None, None));
        assert!(panel.features.is_none());
        let _ = panel.update_message(ConnectAccountMessage::StartCheckout(PlanTier::Pro));
        assert!(panel.checkout.is_none());
    }

    #[test]
    fn start_checkout_proceeds_when_purchasing_enabled() {
        // Features loaded with purchasing on (flag absent = older backend)
        // keeps the existing checkout path working — regression for fall GA.
        let mut panel = panel_with_plan(plan(PlanTier::Free, PlanStatus::Active, None, None));
        panel.features = Some(features_with_purchasing(None, None));
        let _ = panel.update_message(ConnectAccountMessage::StartCheckout(PlanTier::Pro));
        assert!(matches!(
            panel.checkout.as_ref().map(|c| &c.phase),
            Some(CheckoutPhase::Creating)
        ));
    }
}

#[cfg(test)]
mod duress_contacts_tests {
    //! State-machine tests for the duress "Emergency contacts" section
    //! (Estate Notifications — PR 1): gating, cap enforcement, form
    //! validation, and the duress-active hiding invariant.
    use super::*;
    use crate::services::coincube::{
        DuressAlertContact, PlanEntitlements, DURESS_CHANNEL_EMAIL, DURESS_CHANNEL_SMS,
    };

    fn entitlements(duress_alerts: bool) -> PlanEntitlements {
        PlanEntitlements {
            duress: true,
            duress_alerts,
            ..PlanEntitlements::default()
        }
    }

    fn plan(duress_alerts: bool) -> ConnectPlan {
        ConnectPlan {
            plan: if duress_alerts {
                PlanTier::Estate
            } else {
                PlanTier::Pro
            },
            status: PlanStatus::Active,
            renewal_at: None,
            entitlements: entitlements(duress_alerts),
            billing_cycle: Some(BillingCycle::Monthly),
            plan_provenance: None,
            pending_plan: None,
            pending_plan_at: None,
            pending_billing_cycle: None,
        }
    }

    /// Estate-entitled, authenticated panel ready to drive the section.
    fn estate_panel() -> ConnectAccountPanel {
        let mut panel = ConnectAccountPanel::new();
        panel.plan = Some(plan(true));
        panel.step = ConnectFlowStep::Dashboard;
        panel
    }

    fn contact(id: u64) -> DuressAlertContact {
        DuressAlertContact {
            id,
            display_name: format!("Contact {id}"),
            phone: Some("+15551234567".into()),
            email: None,
            channels: vec![DURESS_CHANNEL_SMS.to_string()],
            intro_sent_at: None,
            opted_out_at: None,
            created_at: Some("2026-06-11T00:00:00Z".to_string()),
            updated_at: Some("2026-06-11T00:00:00Z".to_string()),
        }
    }

    fn dispatch(panel: &mut ConnectAccountPanel, m: DuressContactsMessage) {
        let _ = panel.update_duress_contacts(m);
    }

    #[test]
    fn reload_is_noop_when_not_estate_entitled() {
        // Gating: a Pro account (duress_alerts=false) must never fire the
        // contacts fetch — `reload` leaves `loading` false so the UI shows
        // the locked affordance, not a spinner.
        let mut panel = ConnectAccountPanel::new();
        panel.plan = Some(plan(false));
        panel.step = ConnectFlowStep::Dashboard;
        assert!(!panel.is_duress_alerts_entitled());
        let _ = panel.reload_duress_contacts();
        assert!(!panel.duress_contacts.loading);
    }

    #[test]
    fn reload_starts_loading_when_estate_entitled() {
        let mut panel = estate_panel();
        assert!(panel.is_duress_alerts_entitled());
        let _ = panel.reload_duress_contacts();
        assert!(panel.duress_contacts.loading);
        assert_eq!(panel.duress_contacts.step, DuressContactsStep::List);
    }

    #[test]
    fn cap_blocks_add_form_at_five_contacts() {
        let mut panel = estate_panel();
        let five = (1..=5).map(contact).collect::<Vec<_>>();
        panel.duress_contacts.contacts = Some(five);
        assert!(panel.duress_contacts.at_cap());
        dispatch(&mut panel, DuressContactsMessage::ShowAddForm);
        // Stayed on the list with a cap message rather than opening the form.
        assert_eq!(panel.duress_contacts.step, DuressContactsStep::List);
        assert!(panel.duress_contacts.error.is_some());
    }

    #[test]
    fn add_form_opens_below_cap() {
        let mut panel = estate_panel();
        panel.duress_contacts.contacts = Some(vec![contact(1)]);
        dispatch(&mut panel, DuressContactsMessage::ShowAddForm);
        assert_eq!(panel.duress_contacts.step, DuressContactsStep::Form);
        assert!(panel.duress_contacts.editing_id.is_none());
    }

    #[test]
    fn submit_requires_a_name() {
        let mut panel = estate_panel();
        panel.duress_contacts.contacts = Some(vec![]);
        dispatch(&mut panel, DuressContactsMessage::ShowAddForm);
        dispatch(
            &mut panel,
            DuressContactsMessage::PhoneChanged("+15551234567".into()),
        );
        dispatch(&mut panel, DuressContactsMessage::ToggleSms(true));
        dispatch(&mut panel, DuressContactsMessage::Submit);
        assert!(!panel.duress_contacts.submitting);
        assert!(panel
            .duress_contacts
            .error
            .as_deref()
            .unwrap()
            .contains("name"));
    }

    #[test]
    fn submit_requires_a_reachable_method() {
        let mut panel = estate_panel();
        panel.duress_contacts.contacts = Some(vec![]);
        dispatch(&mut panel, DuressContactsMessage::ShowAddForm);
        dispatch(
            &mut panel,
            DuressContactsMessage::NameChanged("Jane".into()),
        );
        dispatch(&mut panel, DuressContactsMessage::Submit);
        assert!(!panel.duress_contacts.submitting);
        assert!(panel.duress_contacts.error.is_some());
    }

    #[test]
    fn submit_rejects_bad_phone() {
        let mut panel = estate_panel();
        panel.duress_contacts.contacts = Some(vec![]);
        dispatch(&mut panel, DuressContactsMessage::ShowAddForm);
        dispatch(
            &mut panel,
            DuressContactsMessage::NameChanged("Jane".into()),
        );
        dispatch(
            &mut panel,
            DuressContactsMessage::PhoneChanged("5551234567".into()), // no +
        );
        dispatch(&mut panel, DuressContactsMessage::ToggleSms(true));
        dispatch(&mut panel, DuressContactsMessage::Submit);
        assert!(!panel.duress_contacts.submitting);
        assert!(panel
            .duress_contacts
            .error
            .as_deref()
            .unwrap()
            .contains("international"));
    }

    #[test]
    fn submit_requires_at_least_one_channel() {
        let mut panel = estate_panel();
        panel.duress_contacts.contacts = Some(vec![]);
        dispatch(&mut panel, DuressContactsMessage::ShowAddForm);
        dispatch(
            &mut panel,
            DuressContactsMessage::NameChanged("Jane".into()),
        );
        dispatch(
            &mut panel,
            DuressContactsMessage::EmailChanged("jane@example.com".into()),
        );
        // No channel toggled.
        dispatch(&mut panel, DuressContactsMessage::Submit);
        assert!(!panel.duress_contacts.submitting);
        assert!(panel.duress_contacts.error.is_some());
    }

    #[test]
    fn valid_submit_starts_submitting() {
        let mut panel = estate_panel();
        panel.duress_contacts.contacts = Some(vec![]);
        dispatch(&mut panel, DuressContactsMessage::ShowAddForm);
        dispatch(
            &mut panel,
            DuressContactsMessage::NameChanged("Jane".into()),
        );
        dispatch(
            &mut panel,
            DuressContactsMessage::EmailChanged("jane@example.com".into()),
        );
        dispatch(&mut panel, DuressContactsMessage::ToggleEmailChannel(true));
        // A valid submit transitions to the in-flight state and clears the
        // error (the returned Task is the network call, ignored here).
        dispatch(&mut panel, DuressContactsMessage::Submit);
        assert!(panel.duress_contacts.submitting);
        assert!(panel.duress_contacts.error.is_none());
        let _ = DURESS_CHANNEL_EMAIL; // silence unused in some cfgs
    }

    #[test]
    fn submit_refused_when_entitlement_lost_midsession() {
        // Estate user opened the form, then the plan downgraded (e.g. a
        // /connect/plan refresh). Submit must refuse rather than 403.
        let mut panel = estate_panel();
        panel.duress_contacts.contacts = Some(vec![]);
        dispatch(&mut panel, DuressContactsMessage::ShowAddForm);
        dispatch(
            &mut panel,
            DuressContactsMessage::NameChanged("Jane".into()),
        );
        dispatch(
            &mut panel,
            DuressContactsMessage::EmailChanged("jane@example.com".into()),
        );
        dispatch(&mut panel, DuressContactsMessage::ToggleEmailChannel(true));
        panel.plan = Some(plan(false)); // downgrade
        dispatch(&mut panel, DuressContactsMessage::Submit);
        assert!(!panel.duress_contacts.submitting);
        assert!(panel.duress_contacts.error.is_some());
    }

    #[test]
    fn contacts_surface_hidden_while_duress_active() {
        // The Emergency-contacts surface (and its on-demand load) gate on
        // `is_authenticated()` == Dashboard. While duress is active the
        // panel sits in DuressRecovery / a non-Dashboard step, so the
        // surface is never reachable and the load never fires — a coercer
        // can't see who would be alerted.
        let mut panel = estate_panel();
        panel.step = ConnectFlowStep::DuressRecovery {
            unlock_at: None,
            passphrase: String::new(),
            submitting: false,
            cleared: false,
        };
        assert!(!panel.is_authenticated());
        // Even though the account is Estate-entitled, the nav guard keys on
        // `is_authenticated()`, so the section is not rendered/loaded.
        assert!(panel.is_duress_alerts_entitled());
    }
}

#[cfg(test)]
mod retry_card_tests {
    use super::*;
    use crate::services::coincube::CoincubeError;
    use crate::user_error::{UserError, CC_NET_TIMEOUT};

    fn timed_out() -> UserError {
        UserError::new(
            "Can't reach COINCUBE | Connect",
            "The server took too long to respond. Check your internet connection and try again.",
            CC_NET_TIMEOUT,
            true,
        )
    }

    /// A failed refresh must leave the panel in a state that actually renders
    /// the card: an error to show, and a step that is not mid-load.
    #[test]
    fn a_failed_refresh_surfaces_a_retryable_card() {
        let mut panel = ConnectAccountPanel::new();
        panel.step = ConnectFlowStep::Login {
            email: String::new(),
            loading: true,
        };

        let _ = panel.update_message(ConnectAccountMessage::RefreshFailed(timed_out()));

        let err = panel.error.as_ref().expect("the failure must be shown");
        assert!(err.user.retryable, "a timeout must offer a retry");
        assert!(
            matches!(
                err.retry_button(),
                Some((
                    "Try again",
                    ConnectAccountMessage::Retry(RetryAction::Session)
                ))
            ),
            "the card must offer a button that re-runs the refresh — the \
             original banner said 'Tap to retry' and had none"
        );
        assert!(
            matches!(panel.step, ConnectFlowStep::Login { loading: false, .. }),
            "the spinner must stop so the card is reachable, got {:?}",
            panel.step
        );
    }

    /// Pressing "Try again" takes the error card down. Whether the keyring
    /// still holds a session decides which branch the session retry takes, but
    /// neither may leave the stale card on screen: one starts a fresh refresh,
    /// the other drops to the login form, and in both the old failure is
    /// history.
    #[test]
    fn pressing_try_again_takes_the_card_down() {
        let mut panel = ConnectAccountPanel::new();
        panel.error = Some(PanelError::retryable(timed_out(), RetryAction::Session));
        panel.step = ConnectFlowStep::Login {
            email: String::new(),
            loading: false,
        };

        let _ = panel.update_message(ConnectAccountMessage::Retry(RetryAction::Session));

        assert!(
            panel.error.is_none(),
            "a retry that leaves the card up looks like a dead button"
        );
    }

    /// The session retry is guarded against double-firing: a second press while
    /// the refresh is in flight must not race a second token refresh.
    #[test]
    fn a_second_press_while_retrying_is_ignored() {
        let mut panel = ConnectAccountPanel::new();
        panel.step = ConnectFlowStep::Login {
            email: String::new(),
            loading: true,
        };

        let _ = panel.update_message(ConnectAccountMessage::Retry(RetryAction::Session));

        assert!(
            matches!(panel.step, ConnectFlowStep::Login { loading: true, .. }),
            "an in-flight refresh must not be restarted"
        );
    }

    /// The sidebar shows "Checking…" only while the outcome is unknown; settled
    /// states must read as signed in or signed out, never pending.
    #[test]
    fn checking_session_covers_only_in_flight_steps() {
        let login = |loading| ConnectFlowStep::Login {
            email: String::new(),
            loading,
        };
        let duress = |status| ConnectFlowStep::CheckingDuress { status };
        let cases = [
            (ConnectFlowStep::CheckingSession, true),
            (login(true), true),
            (duress(DuressGateStatus::Checking), true),
            (login(false), false),
            (duress(DuressGateStatus::Unreachable), false),
            (duress(DuressGateStatus::Incompatible), false),
            (ConnectFlowStep::Dashboard, false),
        ];
        for (step, checking) in cases {
            let mut panel = ConnectAccountPanel::new();
            panel.step = step;
            assert_eq!(panel.is_checking_session(), checking, "{:?}", panel.step);
        }

        let mut panel = ConnectAccountPanel::new();
        panel.admitted_reopen_required = true;
        assert!(
            !panel.is_checking_session(),
            "a required reopen is settled, not pending"
        );
    }

    /// Session expiry is not a transport blip: retrying reproduces the 401, so
    /// the card must send the user to sign in instead of offering a button —
    /// even though the failure arrived on the path that *does* carry a retry
    /// target.
    #[test]
    fn an_expired_session_offers_no_retry_button() {
        let expired: UserError = (&CoincubeError::Unsuccessful(
            crate::services::http::NotSuccessResponseInfo {
                status_code: 401,
                text: r#"{"success":false,"error":{"code":"REFRESH_TOKEN_INVALID","message":"Refresh token expired"}}"#
                    .to_string(),
            },
        ))
            .into();

        assert!(!expired.retryable);
        assert_eq!(expired.reference, "REFRESH_TOKEN_INVALID");

        let panel_error = PanelError::retryable(expired, RetryAction::Session);
        assert!(
            panel_error.retry_button().is_none(),
            "a settled failure gets no button even when a target was supplied"
        );
    }

    /// The dashboard bug. `Init` returns early once signed in, so a card wired
    /// to it rendered a button that did nothing at all and never cleared. Each
    /// dashboard failure must carry the reload that actually re-runs it.
    #[test]
    fn dashboard_failures_retry_their_own_load_not_init() {
        let cases = [
            (
                ConnectAccountMessage::SecurityDataFailed(timed_out(), 0),
                RetryAction::SecurityData,
            ),
            (
                ConnectAccountMessage::UserProfileFailed(timed_out(), 0),
                RetryAction::UserProfile,
            ),
        ];

        for (message, expected) in cases {
            let mut panel = ConnectAccountPanel::new();
            panel.step = ConnectFlowStep::Dashboard;
            assert_eq!(panel.session_generation, 0, "the fixtures assume gen 0");

            let _ = panel.update_message(message);

            let err = panel.error.as_ref().expect("the failure must be shown");
            match err.retry_button() {
                Some(("Try again", ConnectAccountMessage::Retry(action))) => {
                    assert_eq!(action, expected, "wrong reload wired to the button");
                }
                other => panic!("expected a retry button, got {:?}", other),
            }
        }
    }

    /// Pressing a dashboard card's retry must not run the session path: `Init`
    /// on the dashboard is a no-op, which is what made the button look dead.
    #[test]
    fn a_dashboard_retry_does_not_fall_through_to_init() {
        let mut panel = ConnectAccountPanel::new();
        panel.step = ConnectFlowStep::Dashboard;
        panel.error = Some(PanelError::retryable(
            timed_out(),
            RetryAction::SecurityData,
        ));

        let _ = panel.update_message(ConnectAccountMessage::Retry(RetryAction::SecurityData));

        assert!(panel.error.is_none(), "the card must come down");
        assert!(
            matches!(panel.step, ConnectFlowStep::Dashboard),
            "a dashboard reload must not change the step, got {:?}",
            panel.step
        );
    }

    /// A dashboard load that loses its race with a logout must not surface on
    /// the session that replaced it.
    ///
    /// `clear_session` bumps `session_generation` precisely so in-flight
    /// results can be dropped, and the matching success messages
    /// (`VerifiedDevicesLoaded`, `LoginActivityLoaded`) already honour it. The
    /// failure halves did not, so a request that failed after sign-out raised a
    /// card — with a live "Try again" — over the login screen.
    #[test]
    fn a_stale_dashboard_failure_is_discarded() {
        for stale in [
            ConnectAccountMessage::SecurityDataFailed(timed_out(), 0),
            ConnectAccountMessage::UserProfileFailed(timed_out(), 0),
        ] {
            let mut panel = ConnectAccountPanel::new();
            panel.step = ConnectFlowStep::Dashboard;
            // Whatever the old request was keyed to, the session has moved on.
            panel.session_generation = 7;

            let _ = panel.update_message(stale);

            assert!(
                panel.error.is_none(),
                "a result from a dead session must not raise a card"
            );
        }
    }

    /// The guard must not swallow failures that *do* belong to this session —
    /// the card is the only signal the user gets.
    #[test]
    fn a_current_dashboard_failure_still_surfaces() {
        let mut panel = ConnectAccountPanel::new();
        panel.step = ConnectFlowStep::Dashboard;
        panel.session_generation = 7;

        let _ = panel.update_message(ConnectAccountMessage::SecurityDataFailed(timed_out(), 7));

        assert!(
            panel.error.is_some(),
            "a live failure must still reach the user"
        );
    }

    /// A stale profile *success* must not overwrite the signed-in user either —
    /// the same race, in the direction that silently shows the wrong account.
    #[test]
    fn a_stale_profile_load_does_not_replace_the_current_user() {
        let mut panel = ConnectAccountPanel::new();
        panel.step = ConnectFlowStep::Dashboard;
        panel.session_generation = 7;
        assert!(panel.user.is_none());

        let _ = panel.update_message(ConnectAccountMessage::UserProfileLoaded(
            crate::services::coincube::User {
                id: 1,
                email: "stale@example.com".into(),
                email_verified: Some(true),
            },
            0,
        ));

        assert!(
            panel.user.is_none(),
            "a profile from a dead session must not land on the new one"
        );
    }

    /// Billing history failures must classify like every other API failure.
    ///
    /// The error used to be stringified at the async boundary and re-wrapped
    /// with a hardcoded `CC-API-BADRESP` and `retryable: true`, so a settled
    /// refusal got "check your internet connection" over a button that could
    /// only reproduce it. A plan gate is not a hypothetical here — it is a
    /// *billing* screen.
    #[test]
    fn billing_failures_follow_the_real_error_not_a_hardcoded_guess() {
        let of = |status: u16, code: &str, message: &str| -> UserError {
            (&CoincubeError::Unsuccessful(crate::services::http::NotSuccessResponseInfo {
                status_code: status,
                text: format!(
                    r#"{{"success":false,"error":{{"code":"{}","message":"{}"}}}}"#,
                    code, message
                ),
            }))
                .into()
        };

        // A dead session and a plan gate are both settled: no button.
        for user in [
            of(401, "REFRESH_TOKEN_INVALID", "Your session has expired"),
            of(403, "PLAN_PRO_REQUIRED", "Pro plan or above required"),
        ] {
            let mut panel = ConnectAccountPanel::new();
            panel.step = ConnectFlowStep::Dashboard;
            let reference = user.reference.clone();

            let _ = panel.update_message(ConnectAccountMessage::BillingHistoryLoaded(
                Err(user),
                panel.session_generation,
            ));

            let err = panel.error.as_ref().expect("the failure must be shown");
            assert!(
                err.retry_button().is_none(),
                "a refused request must not offer a retry: {:?}",
                err.user
            );
            // The server's own taxonomy code survives as the support reference
            // instead of a blanket CC-API-BADRESP.
            assert_eq!(err.user.reference, reference);
            assert!(
                !err.user.guidance.contains("internet connection"),
                "a server refusal is not a connectivity problem: {:?}",
                err.user.guidance
            );
        }

        // A 5xx really is worth another go, and still gets its button.
        let mut panel = ConnectAccountPanel::new();
        panel.step = ConnectFlowStep::Dashboard;
        let _ = panel.update_message(ConnectAccountMessage::BillingHistoryLoaded(
            Err(of(503, "SERVICE_UNAVAILABLE", "down")),
            panel.session_generation,
        ));
        let err = panel.error.as_ref().expect("the failure must be shown");
        assert!(matches!(
            err.retry_button(),
            Some((
                "Try again",
                ConnectAccountMessage::Retry(RetryAction::BillingHistory)
            ))
        ));
    }

    /// The code-entry bug. A network blip during verify or resend used to
    /// produce a card whose button ran `Init`, which found no stored session
    /// and dropped the user back to the email form — off the screen they were
    /// part-way through. Those failures now carry no target at all, because the
    /// screen's own Verify/Resend button is the retry.
    #[test]
    fn an_otp_screen_failure_offers_no_button_and_stays_put() {
        let mut panel = ConnectAccountPanel::new();
        panel.step = ConnectFlowStep::OtpVerification {
            email: "a@b.co".to_string(),
            otp: "123456".to_string(),
            sending: true,
            is_signup: false,
            cooldown: 0,
        };

        let _ = panel.update_message(ConnectAccountMessage::Error(timed_out()));

        let err = panel.error.as_ref().expect("the failure must be shown");
        assert!(
            err.retry_button().is_none(),
            "the screen's own Verify button is the retry; a card button here \
             navigates the user away"
        );
        assert!(
            matches!(
                panel.step,
                ConnectFlowStep::OtpVerification { sending: false, .. }
            ),
            "the user must stay on the code screen, got {:?}",
            panel.step
        );
    }
}

#[cfg(test)]
mod admitted_session_tests {
    use super::*;

    fn fixture_client(token: &str) -> CoincubeClient {
        let mut client = CoincubeClient::new();
        client.base_url = "http://127.0.0.1:1".into();
        client.set_token(token);
        client
    }

    fn fixture_login(token: &str) -> crate::services::coincube::LoginResponse {
        crate::services::coincube::LoginResponse {
            requires_2fa: false,
            token: token.into(),
            refresh_token: "synthetic-refresh".into(),
            user: crate::services::coincube::User {
                id: 1,
                email: "fixture@example.invalid".into(),
                email_verified: Some(true),
            },
        }
    }

    #[tokio::test]
    async fn app_refresh_waits_for_cache_commit_before_adoption() {
        use iced::futures::StreamExt;

        let mut panel = ConnectAccountPanel::new();
        panel.defer_auth_cache_commit();
        let keyring_before = read_connect_secret(CONNECT_KEYRING_USER);
        let task = panel.update_message(ConnectAccountMessage::RefreshedSession(
            fixture_login("refreshed-bearer"),
            panel.auth_epoch(),
            "previous-refresh".into(),
        ));
        assert_eq!(panel.client.token(), None);
        let mut stream = iced_runtime::task::into_stream(task).unwrap();
        assert!(matches!(
            stream.next().await,
            Some(iced_runtime::Action::Output(
                Message::InAppConnectAuthReady {
                    previous_refresh_token: Some(_),
                    ..
                }
            ))
        ));
        assert!(stream.next().await.is_none());
        assert_eq!(panel.client.token(), None);
        assert_eq!(read_connect_secret(CONNECT_KEYRING_USER), keyring_before);
    }

    #[test]
    fn duplicate_otp_and_old_attempt_cannot_start_two_commits() {
        let mut panel = ConnectAccountPanel::new();
        panel.defer_auth_cache_commit();
        panel.step = ConnectFlowStep::OtpVerification {
            email: "fixture@example.invalid".into(),
            otp: "123456".into(),
            sending: false,
            is_signup: false,
            cooldown: 0,
        };
        let first = panel.update_message(ConnectAccountMessage::VerifyOtp);
        assert!(iced_runtime::task::into_stream(first).is_some());
        let first_attempt = panel.otp_attempt_generation;
        let duplicate = panel.update_message(ConnectAccountMessage::VerifyOtp);
        assert!(iced_runtime::task::into_stream(duplicate).is_none());
        assert_eq!(panel.otp_attempt_generation, first_attempt);

        let _ = panel.update_message(ConnectAccountMessage::BackToEmail);
        let stale = panel.update_message(ConnectAccountMessage::OtpVerified(
            fixture_login("stale-otp"),
            panel.auth_epoch(),
            first_attempt,
        ));
        assert!(iced_runtime::task::into_stream(stale).is_none());
        assert_eq!(panel.client.token(), None);
    }

    #[test]
    fn home_shared_session_ignores_queued_auth_then_can_log_out_and_log_in() {
        let mut panel = ConnectAccountPanel::new();
        // Home advances its epoch before accepting a sibling's bearer.
        panel.invalidate_auth();
        panel.install_shared_client(fixture_client("accepted-bearer"));
        let generation = panel.session_generation();

        // A RefreshSession already queued by Home's startup has no epoch on
        // its message. Its SetSession/SessionLoaded completions do have one.
        let _ = panel.update_message(ConnectAccountMessage::RefreshSession {
            refresh_token: "old-refresh".into(),
        });
        let _ = panel.update_message(ConnectAccountMessage::RefreshFailed(UserError::new(
            "Temporary failure",
            "Try again later.",
            crate::user_error::CC_NET_TIMEOUT,
            true,
        )));
        let _ = panel.update_message(ConnectAccountMessage::SetSession(
            fixture_login("late-bearer"),
            0,
        ));
        let _ = panel.update_message(ConnectAccountMessage::SessionLoaded {
            user: fixture_login("late-bearer").user,
            plan: None,
            epoch: 0,
        });
        assert!(!panel.requires_authenticated_reopen());
        assert_eq!(panel.session_generation(), generation);
        assert_eq!(panel.client.token(), Some("accepted-bearer"));
        assert!(matches!(panel.step, ConnectFlowStep::CheckingSession));

        panel.invalidate_auth(); // global sign-out invalidates late completions
        let _ = panel.update_message(ConnectAccountMessage::LogOut);
        assert!(!panel.requires_authenticated_reopen());
        assert!(matches!(
            panel.step,
            ConnectFlowStep::Login { loading: false, .. }
        ));
        assert_eq!(panel.client.token(), None);
        let _ = panel.update_message(ConnectAccountMessage::SetSession(
            fixture_login("late-bearer"),
            0,
        ));
        let _ = panel.update_message(ConnectAccountMessage::SessionLoaded {
            user: fixture_login("late-bearer").user,
            plan: None,
            epoch: 0,
        });
        assert_eq!(panel.client.token(), None);
        assert!(!panel.has_stored_session());
        assert!(matches!(
            panel.step,
            ConnectFlowStep::Login { loading: false, .. }
        ));

        let _ = panel.update_message(ConnectAccountMessage::EmailChanged(
            "next@example.invalid".into(),
        ));
        let _ = panel.update_message(ConnectAccountMessage::SubmitLogin);
        assert!(matches!(
            &panel.step,
            ConnectFlowStep::Login { email, loading: true }
                if email == "next@example.invalid"
        ));
        assert!(!panel.requires_authenticated_reopen());
    }

    #[test]
    fn only_origin_home_logout_forgets_a_shared_saved_session() {
        let mut panel = ConnectAccountPanel::new();
        panel.save_session_to_keyring(&StoredSession {
            login: fixture_login("saved-bearer"),
        });
        panel.install_shared_client(fixture_client("saved-bearer"));
        assert!(panel.has_stored_session());

        // A sibling's broadcast only detaches this tab; it cannot delete a
        // newer session another tab may have saved in the meantime.
        let _ = panel.update_message(ConnectAccountMessage::LogOut);
        assert!(panel.has_stored_session());

        panel.install_shared_client(fixture_client("saved-bearer"));
        panel.forget_shared_session_for_logout();
        let _ = panel.update_message(ConnectAccountMessage::LogOut);
        assert!(!panel.has_stored_session());
    }

    #[test]
    fn cube_admitted_session_still_rejects_replacement_login() {
        let mut panel = ConnectAccountPanel::new();
        panel.install_admitted_client(fixture_client("cube-bearer"));
        let _ = panel.update_message(ConnectAccountMessage::SetSession(
            fixture_login("replacement-bearer"),
            panel.auth_epoch(),
        ));
        assert!(panel.requires_authenticated_reopen());
        assert!(panel.authenticated_client().is_none());
    }

    #[test]
    fn a_late_admitted_user_cannot_replace_a_new_session() {
        let mut panel = ConnectAccountPanel::new();
        let mut first = CoincubeClient::new();
        first.base_url = "http://127.0.0.1:1".into();
        first.set_token("old-fixture");
        panel.install_admitted_client(first);
        let generation = panel.session_generation;
        let mut second = CoincubeClient::new();
        second.base_url = "http://127.0.0.1:2".into();
        second.set_token("new-fixture");
        panel.install_admitted_client(second);
        let task = panel.update_message(ConnectAccountMessage::AdmittedUserLoaded {
            user: Ok(crate::services::coincube::User {
                id: 1,
                email: "old@example.invalid".into(),
                email_verified: Some(true),
            }),
            generation,
            epoch: 0,
        });
        assert!(iced_runtime::task::into_stream(task).is_none());
        assert!(panel.user.is_none());
        let client = panel.authenticated_client().unwrap();
        assert_eq!(client.base_url, "http://127.0.0.1:2");
        assert_eq!(client.token(), Some("new-fixture"));
    }
}

#[cfg(test)]
mod connect_review_tests {
    use super::*;

    #[test]
    fn prior_otp_timer_cannot_advance_a_new_login_or_signup_attempt() {
        for is_signup in [false, true] {
            let mut panel = ConnectAccountPanel::new();
            panel.register_campaign_code = "HELLO".into();
            let email = "person@example.test".to_string();
            let _ = panel.update_message(ConnectAccountMessage::OtpRequested {
                email: email.clone(),
                is_signup,
            });
            let old_attempt = panel.otp_attempt_generation;
            let pending_tick =
                panel.update_message(ConnectAccountMessage::OtpCooldownTick(old_attempt));
            assert!(iced_runtime::task::into_stream(pending_tick).is_some());

            let _ = panel.update_message(ConnectAccountMessage::BackToEmail);
            let _ = panel.update_message(ConnectAccountMessage::OtpRequested {
                email: email.clone(),
                is_signup,
            });
            let new_attempt = panel.otp_attempt_generation;
            assert_ne!(old_attempt, new_attempt);
            let stale = panel.update_message(ConnectAccountMessage::OtpCooldownTick(old_attempt));
            assert!(iced_runtime::task::into_stream(stale).is_none());
            let stale_resend = panel.update_message(ConnectAccountMessage::OtpResent(old_attempt));
            assert!(iced_runtime::task::into_stream(stale_resend).is_none());
            let _ = panel.update_message(ConnectAccountMessage::OtpResendFailed(
                UserError::new("Old request failed", "Try again", "CC-OTP-STALE", true),
                old_attempt,
            ));
            assert!(panel.error.is_none());
            assert!(matches!(
                panel.step,
                ConnectFlowStep::OtpVerification { cooldown: 60, .. }
            ));
            assert_eq!(panel.register_campaign_code, "HELLO");
            let _ = panel.update_message(ConnectAccountMessage::OtpCooldownTick(new_attempt));
            assert!(matches!(
                panel.step,
                ConnectFlowStep::OtpVerification { cooldown: 59, .. }
            ));
        }
    }

    #[test]
    fn token_back_preserves_signup_email_and_promo() {
        let mut panel = ConnectAccountPanel::new();
        panel.register_campaign_code = "HELLO".into();
        panel.step = ConnectFlowStep::OtpVerification {
            email: "person@example.test".into(),
            otp: "123456".into(),
            sending: false,
            is_signup: true,
            cooldown: 30,
        };
        let _ = panel.update_message(ConnectAccountMessage::BackToEmail);
        assert!(
            matches!(panel.step, ConnectFlowStep::Register { ref email, loading: false } if email == "person@example.test")
        );
        assert_eq!(panel.register_campaign_code, "HELLO");
    }

    #[test]
    fn device_removal_requires_review_of_a_noncurrent_device() {
        let mut panel = ConnectAccountPanel::new();
        panel.verified_devices = Some(vec![
            VerifiedDevice {
                id: 1,
                device_name: Some("This Mac".into()),
                created_at: "2026-10-01T00:00:00Z".into(),
                last_used_at: None,
                is_current: true,
            },
            VerifiedDevice {
                id: 2,
                device_name: Some("Other device".into()),
                created_at: "2026-10-01T00:00:00Z".into(),
                last_used_at: None,
                is_current: false,
            },
        ]);
        let task = panel.update_message(ConnectAccountMessage::DeleteVerifiedDevice(2));
        assert!(iced_runtime::task::into_stream(task).is_none());
        assert!(panel.verified_devices_state.deleting_ids.is_empty());

        let _ = panel.update_message(ConnectAccountMessage::ReviewVerifiedDeviceRemoval(Some(1)));
        assert_eq!(panel.review_device_removal, None);
        let _ = panel.update_message(ConnectAccountMessage::ReviewVerifiedDeviceRemoval(Some(2)));
        assert_eq!(panel.review_device_removal, Some(2));
        let _ = panel.update_message(ConnectAccountMessage::ReviewVerifiedDeviceRemoval(None));
        assert_eq!(panel.review_device_removal, None);
    }

    #[test]
    fn billing_management_precedes_explicit_comparison_and_history() {
        let mut panel = ConnectAccountPanel::new();
        assert!(!panel.show_plan_comparison);
        let _ = panel.update_message(ConnectAccountMessage::ComparePlans(true));
        assert!(panel.show_plan_comparison);
        let _ = panel.update_message(ConnectAccountMessage::ComparePlans(false));
        assert!(!panel.show_plan_comparison);
        let _ = panel.update_message(ConnectAccountMessage::ComparePlans(true));
        let _ = panel.update_message(ConnectAccountMessage::ToggleBillingHistory);
        assert!(panel.show_billing_history);
        assert!(!panel.show_plan_comparison);
        let _ = panel.update_message(ConnectAccountMessage::OpenPlanBilling);
        assert!(!panel.show_billing_history);
        assert!(!panel.show_plan_comparison);
        panel.active_sub = ConnectSubMenu::Duress;
        let _ = panel.update_message(ConnectAccountMessage::OpenPlanBilling);
        assert!(panel.show_plan_comparison);
    }

    #[test]
    fn recovery_rows_distinguish_server_record_from_backup_completeness() {
        let reported: CubeResponse = serde_json::from_str(r#"{"id":7,"uuid":"cube-7","name":"  Family  ","network":"bitcoin","status":"active","hasRecoveryKit":true}"#).unwrap();
        let missing_field: CubeResponse = serde_json::from_str(
            r#"{"id":8,"uuid":"cube-8","name":"","network":"testnet","status":"active"}"#,
        )
        .unwrap();
        let rows = [
            RecoveryServiceRow::from(reported),
            RecoveryServiceRow::from(missing_field),
        ];
        assert_eq!(rows[0].name, "Family");
        assert_eq!(rows[0].target.cube_id, 7);
        assert_eq!(rows[0].target.network, "bitcoin");
        assert_eq!(rows[0].kit, RecoveryKitReport::Reported);
        assert_eq!(rows[1].name, "Cube 8");
        assert_eq!(rows[1].kit, RecoveryKitReport::NotReported);
    }

    #[test]
    fn recovery_result_ignores_stale_session_and_clears_on_logout() {
        let mut panel = ConnectAccountPanel::new();
        panel.session_generation = 2;
        panel.overview_seq = 2;
        let cube: CubeResponse = serde_json::from_str(r#"{"id":7,"uuid":"cube-7","name":"Family","network":"bitcoin","status":"active","hasRecoveryKit":true}"#).unwrap();
        let _ = panel.update_message(ConnectAccountMessage::OverviewCountsLoaded {
            contacts: None,
            cubes: Some(1),
            recovery: Ok(vec![cube.clone()]),
            generation: 1,
            request_seq: 2,
        });
        assert!(matches!(
            panel.recovery_services,
            RecoveryServicesState::NotLoaded
        ));
        let _ = panel.update_message(ConnectAccountMessage::OverviewCountsLoaded {
            contacts: None,
            cubes: Some(1),
            recovery: Ok(vec![cube.clone()]),
            generation: 2,
            request_seq: 1,
        });
        assert!(matches!(
            panel.recovery_services,
            RecoveryServicesState::NotLoaded
        ));
        let _ = panel.update_message(ConnectAccountMessage::OverviewCountsLoaded {
            contacts: None,
            cubes: Some(1),
            recovery: Ok(vec![cube]),
            generation: 2,
            request_seq: 2,
        });
        assert!(
            matches!(panel.recovery_services, RecoveryServicesState::Ready(ref rows) if rows.len() == 1)
        );
        let _ = panel.update_message(ConnectAccountMessage::LogOut);
        assert!(matches!(
            panel.recovery_services,
            RecoveryServicesState::NotLoaded
        ));
    }

    #[test]
    fn signed_out_recovery_refresh_never_requests_cube_metadata() {
        let mut panel = ConnectAccountPanel::new();
        panel.step = ConnectFlowStep::Login {
            email: String::new(),
            loading: false,
        };
        let task = panel.update_message(ConnectAccountMessage::RefreshRecoveryServices);
        assert!(iced_runtime::task::into_stream(task).is_none());
        assert!(matches!(
            panel.recovery_services,
            RecoveryServicesState::NotLoaded
        ));
    }
}

#[cfg(test)]
mod unverified_session_tests {
    use super::*;
    use crate::services::coincube::CoincubeError;
    use crate::services::connect::grpc::trouble::Trouble;
    use crate::services::http::NotSuccessResponseInfo;

    fn unverified(attempts: u32) -> UnverifiedSession {
        UnverifiedSession {
            email: "alex@example.test".into(),
            trouble: Trouble::Offline,
            attempts,
        }
    }

    #[test]
    fn rechecks_back_off_from_one_to_thirty_seconds() {
        let secs: Vec<u64> = (1..=8).map(|n| unverified(n).backoff().as_secs()).collect();
        assert_eq!(secs, [1, 2, 4, 8, 16, 30, 30, 30]);
    }

    #[test]
    fn only_a_failure_to_reach_connect_keeps_the_session() {
        let status = |code| {
            CoincubeError::Unsuccessful(NotSuccessResponseInfo {
                status_code: code,
                text: String::new(),
            })
        };
        assert_eq!(refresh_trouble(&status(503)), Some(Trouble::Unreachable));
        assert_eq!(refresh_trouble(&status(502)), Some(Trouble::Unreachable));
        // A real answer from Connect is not a reachability problem.
        assert_eq!(refresh_trouble(&status(400)), None);
        assert_eq!(refresh_trouble(&status(401)), None);
        assert_eq!(refresh_trouble(&CoincubeError::Api("bad".into())), None);
    }

    /// Connection refused is the offline/unreachable case the status checks
    /// above do not cover. A body Connect actually sent is a real failure:
    /// keeping the session for it would retry forever instead of showing
    /// the error.
    #[tokio::test]
    async fn transport_failures_keep_the_session_and_a_delivered_body_does_not() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let refused: CoincubeError = reqwest::Client::new()
            .get(format!("http://{addr}/api/v1/auth/token/refresh"))
            .timeout(std::time::Duration::from_secs(2))
            .send()
            .await
            .expect_err("nothing is listening")
            .into();
        assert!(
            matches!(&refused, CoincubeError::Network(e) if e.is_connect() || e.is_timeout()),
            "a closed local port is a transport failure, got {:?}",
            refused
        );
        assert!(
            refresh_trouble(&refused).is_some(),
            "a transport failure must keep the stored session, got {:?}",
            refused
        );

        let answered = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let answered_addr = answered.local_addr().unwrap();
        let body = b"not-json";
        std::thread::spawn(move || {
            let (mut sock, _) = answered.accept().unwrap();
            let mut buf = [0u8; 1024];
            let _ = std::io::Read::read(&mut sock, &mut buf);
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            use std::io::Write;
            let _ = sock.write_all(header.as_bytes());
            let _ = sock.write_all(body);
        });
        let delivered: CoincubeError = reqwest::Client::new()
            .get(format!("http://{answered_addr}/api/v1/auth/token/refresh"))
            .timeout(std::time::Duration::from_secs(2))
            .send()
            .await
            .expect("the local server answers")
            .json::<serde_json::Value>()
            .await
            .expect_err("the body is not JSON")
            .into();
        assert!(
            matches!(&delivered, CoincubeError::Network(e) if e.is_decode()),
            "expected a decode error, got {:?}",
            delivered
        );
        assert_eq!(
            refresh_trouble(&delivered),
            None,
            "a body Connect delivered is not a reachability wait"
        );
    }

    #[test]
    fn a_recheck_after_sign_in_or_out_does_nothing() {
        let mut panel = ConnectAccountPanel::new();
        panel.step = ConnectFlowStep::Login {
            email: String::new(),
            loading: false,
        };
        assert_eq!(
            panel
                .update_message(ConnectAccountMessage::RecheckSession)
                .units(),
            0
        );
        panel.unverified = Some(unverified(1));
        panel.step = ConnectFlowStep::Dashboard;
        assert_eq!(
            panel
                .update_message(ConnectAccountMessage::RecheckSession)
                .units(),
            0
        );
    }

    #[test]
    fn signing_out_drops_the_waiting_session() {
        let mut panel = ConnectAccountPanel::new();
        panel.unverified = Some(unverified(3));
        drop(panel.update_message(ConnectAccountMessage::LogOut));
        assert_eq!(panel.unverified, None);
    }

    #[test]
    fn a_refresh_that_fails_outright_stops_waiting() {
        let mut panel = ConnectAccountPanel::new();
        panel.unverified = Some(unverified(2));
        drop(
            panel.update_message(ConnectAccountMessage::RefreshFailed(UserError::new(
                "Temporary failure",
                "Try again later.",
                crate::user_error::CC_NET_TIMEOUT,
                true,
            ))),
        );
        assert_eq!(panel.unverified, None);
        assert!(panel.error.is_some());
        assert_eq!(
            panel
                .update_message(ConnectAccountMessage::RecheckSession)
                .units(),
            0
        );
    }

    #[test]
    fn an_adopted_session_replaces_the_waiting_one() {
        let mut panel = ConnectAccountPanel::new();
        panel.unverified = Some(unverified(2));
        panel.install_shared_client(CoincubeClient::new());
        assert_eq!(panel.unverified, None);
        assert!(matches!(panel.step, ConnectFlowStep::CheckingSession));
    }

    /// Drops whatever session the thread-local test store is holding, including
    /// when the test panics.
    struct ClearSession;
    impl Drop for ClearSession {
        fn drop(&mut self) {
            delete_connect_secret(CONNECT_KEYRING_USER);
        }
    }

    fn store_session(email: &str) {
        let session = StoredSession {
            login: LoginResponse {
                requires_2fa: false,
                token: "synthetic-access".into(),
                refresh_token: "synthetic-refresh".into(),
                user: User {
                    id: 7,
                    email: email.into(),
                    email_verified: Some(true),
                },
            },
        };
        write_connect_secret(CONNECT_KEYRING_USER, &serde_json::to_vec(&session).unwrap()).unwrap();
    }

    #[test]
    fn a_reachability_failure_keeps_the_stored_session_and_counts_attempts() {
        delete_connect_secret(CONNECT_KEYRING_USER);
        let _clear = ClearSession;
        store_session("alex@example.test");
        let stored = read_connect_secret(CONNECT_KEYRING_USER);
        let mut panel = ConnectAccountPanel::new();
        panel.step = ConnectFlowStep::CheckingSession;
        let epoch = panel.auth_epoch();

        let first = panel.update_message(ConnectAccountMessage::RefreshUnreachable {
            trouble: Trouble::Offline,
            epoch,
        });
        assert_eq!(first.units(), 0);
        assert_eq!(
            panel.unverified,
            Some(UnverifiedSession {
                email: "alex@example.test".into(),
                trouble: Trouble::Offline,
                attempts: 1,
            })
        );
        assert!(panel.error.is_none());
        assert!(matches!(
            panel.step,
            ConnectFlowStep::Login { loading: false, .. }
        ));

        let second = panel.update_message(ConnectAccountMessage::RefreshUnreachable {
            trouble: Trouble::Unreachable,
            epoch,
        });
        assert_eq!(second.units(), 0);
        assert_eq!(
            panel.unverified,
            Some(UnverifiedSession {
                email: "alex@example.test".into(),
                trouble: Trouble::Unreachable,
                attempts: 2,
            })
        );
        assert_eq!(
            read_connect_secret(CONNECT_KEYRING_USER),
            stored,
            "the stored session stays put while Connect cannot be checked"
        );
    }

    #[test]
    fn a_reachability_failure_without_a_stored_session_does_not_wait() {
        delete_connect_secret(CONNECT_KEYRING_USER);
        let _clear = ClearSession;
        let mut panel = ConnectAccountPanel::new();
        panel.step = ConnectFlowStep::CheckingSession;
        let task = panel.update_message(ConnectAccountMessage::RefreshUnreachable {
            trouble: Trouble::Offline,
            epoch: panel.auth_epoch(),
        });
        assert_eq!(task.units(), 0);
        assert!(panel.unverified.is_none());
        assert!(matches!(
            panel.step,
            ConnectFlowStep::Login {
                loading: false,
                ref email,
            } if email.is_empty()
        ));
    }

    #[test]
    fn a_recheck_while_waiting_starts_another_session_check() {
        delete_connect_secret(CONNECT_KEYRING_USER);
        let _clear = ClearSession;
        store_session("alex@example.test");
        let mut panel = ConnectAccountPanel::new();
        panel.unverified = Some(unverified(2));
        panel.step = ConnectFlowStep::Login {
            email: String::new(),
            loading: false,
        };
        let task = panel.update_message(ConnectAccountMessage::RecheckSession);
        assert!(
            task.units() > 0,
            "a mounted wait must check the stored session again"
        );
        assert!(matches!(
            panel.step,
            ConnectFlowStep::Login { loading: true, .. }
        ));
        // The wait is still the same one until that check settles.
        assert_eq!(
            panel.unverified.as_ref().map(|session| session.attempts),
            Some(2)
        );
    }

    #[test]
    fn a_stale_refresh_cannot_sign_out_or_start_a_wait() {
        let mut panel = ConnectAccountPanel::new();
        let stale = panel.auth_epoch();
        panel.invalidate_auth();
        panel.step = ConnectFlowStep::Dashboard;
        panel.unverified = Some(unverified(4));

        let rejected =
            panel.update_message(ConnectAccountMessage::RefreshRejected { epoch: stale });
        assert_eq!(rejected.units(), 0);
        let unreachable = panel.update_message(ConnectAccountMessage::RefreshUnreachable {
            trouble: Trouble::Offline,
            epoch: stale,
        });
        assert_eq!(unreachable.units(), 0);
        assert!(matches!(panel.step, ConnectFlowStep::Dashboard));
        assert_eq!(
            panel.unverified.as_ref().map(|session| session.attempts),
            Some(4)
        );
    }

    #[tokio::test]
    async fn a_refused_refresh_signs_the_account_out() {
        use iced::futures::StreamExt;

        let mut panel = ConnectAccountPanel::new();
        panel.step = ConnectFlowStep::CheckingSession;
        panel.unverified = Some(unverified(2));
        let task = panel.update_message(ConnectAccountMessage::RefreshRejected {
            epoch: panel.auth_epoch(),
        });
        let mut stream = iced_runtime::task::into_stream(task).expect("sign-out task");
        let mut saw_signed_out = false;
        let mut saw_logout = false;
        while let Some(action) = stream.next().await {
            if let iced_runtime::Action::Output(message) = action {
                match message {
                    Message::ConnectSignedOutBy(crate::app::SignedOutBy::SessionEnded) => {
                        saw_signed_out = true;
                    }
                    Message::View(view::Message::ConnectAccount(ConnectAccountMessage::LogOut)) => {
                        saw_logout = true;
                    }
                    other => panic!("unexpected refresh rejection output: {:?}", other),
                }
            }
        }
        assert!(
            saw_signed_out,
            "a refused refresh tells every tab the session ended"
        );
        assert!(saw_logout, "a refused refresh logs the account out");
        drop(panel.update_message(ConnectAccountMessage::LogOut));
        assert_eq!(panel.unverified, None);
    }
}
