use serde::{Deserialize, Serialize};

pub mod client;
pub use client::CoincubeClient;

#[derive(Debug)]
pub enum CoincubeError {
    Network(reqwest::Error),
    Unsuccessful(crate::services::http::NotSuccessResponseInfo),
    Api(String),
    Parse(serde_json::Error),
    SseError(reqwest_sse::error::EventSourceError),
    /// Typed variant for W16-desktop's 409
    /// `VAULT_KEYHOLDER_LOCKED`. Reclassified from `Unsuccessful` by
    /// `add_vault_member` so the dialog handler can match by variant
    /// instead of re-parsing the error body. `vault_id` is the
    /// backend's numeric id for the locked vault (extracted from the
    /// 409 body); `0` when the body is malformed.
    VaultKeyholderLocked {
        vault_id: u64,
    },
    /// 404 from an endpoint where the caller expects the resource may
    /// legitimately be absent (e.g. `get_recovery_kit` when no kit exists
    /// yet). Only the recovery-kit methods emit this variant today; other
    /// callers continue to route 404 through `Unsuccessful` as before.
    NotFound,
    /// 429 from a rate-limited endpoint. `retry_after` is parsed from
    /// the `Retry-After` response header per RFC 7231 §7.1.3 —
    /// both *delta-seconds* (e.g. `60`) and *HTTP-date* (IMF-fixdate,
    /// e.g. `Wed, 21 Oct 2026 07:28:00 GMT`) forms are accepted. An
    /// HTTP-date that's already in the past is clamped to zero; a
    /// missing or unparseable header falls back to 60s so the UI
    /// always has a usable cooldown.
    RateLimited {
        retry_after: std::time::Duration,
    },
}

impl From<serde_json::Error> for CoincubeError {
    fn from(v: serde_json::Error) -> Self {
        Self::Parse(v)
    }
}

impl From<crate::services::http::NotSuccessResponseInfo> for CoincubeError {
    fn from(v: crate::services::http::NotSuccessResponseInfo) -> Self {
        Self::Unsuccessful(v)
    }
}

impl From<reqwest::Error> for CoincubeError {
    fn from(e: reqwest::Error) -> Self {
        Self::Network(e)
    }
}

impl From<reqwest_sse::error::EventSourceError> for CoincubeError {
    fn from(e: reqwest_sse::error::EventSourceError) -> Self {
        Self::SseError(e)
    }
}

impl std::fmt::Display for CoincubeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CoincubeError::Network(msg) => write!(f, "Network error: {:?}", msg),
            CoincubeError::Unsuccessful(e) => write!(f, "{}", e.message()),
            CoincubeError::Api(msg) => write!(f, "API error: {}", msg),
            CoincubeError::Parse(msg) => write!(f, "Parse error: {}", msg),
            CoincubeError::SseError(e) => write!(f, "SSE Error: {}", e),
            CoincubeError::VaultKeyholderLocked { vault_id } => write!(
                f,
                "Can't add a keyholder to Vault #{} — the signing quorum is fixed at build time.",
                vault_id
            ),
            CoincubeError::NotFound => write!(f, "Not found"),
            CoincubeError::RateLimited { retry_after } => {
                write!(f, "Rate limited — retry after {}s", retry_after.as_secs())
            }
        }
    }
}

impl std::error::Error for CoincubeError {}

impl CoincubeError {
    /// Returns `true` when the error indicates that the credentials (token) are
    /// definitively rejected by the server (401 Unauthorized / 403 Forbidden).
    pub fn is_auth_error(&self) -> bool {
        matches!(
            self,
            CoincubeError::Unsuccessful(crate::services::http::NotSuccessResponseInfo {
                status_code: 401 | 403,
                ..
            })
        )
    }

    /// True when the error is the typed 404 variant. Today only the
    /// recovery-kit endpoints produce this; generic 404s still surface
    /// as `Unsuccessful`.
    pub fn is_not_found(&self) -> bool {
        matches!(self, CoincubeError::NotFound)
    }

    /// When the error is a typed 429 rate-limit, returns the cooldown
    /// `Duration` computed from the server's `Retry-After` header.
    /// Accepts both RFC 7231 forms (delta-seconds and HTTP-date);
    /// past dates and missing/malformed headers are normalised to
    /// safe defaults. Callers can use this to delay a retry or
    /// display a countdown to the user.
    pub fn rate_limit_retry_after(&self) -> Option<std::time::Duration> {
        match self {
            CoincubeError::RateLimited { retry_after } => Some(*retry_after),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct DownloadStats {
    pub total: u32,
    pub breakdown: std::collections::HashMap<String, u32>,
    pub last_updated: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TodayStats {
    pub count: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TimeseriesPoint {
    pub date: String,
    pub count: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TimeseriesResponse {
    pub points: Vec<TimeseriesPoint>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatsPeriod {
    Day,
    Week,
    Month,
    Year,
}

impl StatsPeriod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
            Self::Year => "year",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Day => "Day",
            Self::Week => "Week",
            Self::Month => "Month",
            Self::Year => "Year",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveQuoteRequest<'a, T: Serialize> {
    pub quote_id: &'a str,
    pub quote: T,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SaveQuoteResponse {
    pub success: bool,
}

#[derive(Serialize)]
pub struct OtpRequest {
    pub email: String,
}

#[derive(Serialize)]
pub struct OtpVerifyRequest {
    pub email: String,
    pub otp: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshTokenRequest<'a> {
    pub refresh_token: &'a str,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: u32,
    pub email: String,
    pub email_verified: Option<bool>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct LoginResponse {
    pub requires_2fa: bool,
    pub token: String,
    pub refresh_token: String,
    pub user: User,
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
pub struct Country {
    pub name: &'static str,
    pub code: &'static str,
    pub flag: &'static str,
    pub currency: Currency,
}

impl std::fmt::Display for Country {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.name, self.code)
    }
}

#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
pub struct Currency {
    pub code: &'static str,
    pub name: &'static str,
    pub symbol: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlanTier {
    Free,
    Pro,
    #[serde(alias = "legacy")]
    Estate,
}

impl std::fmt::Display for PlanTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanTier::Free => write!(f, "Free"),
            PlanTier::Pro => write!(f, "Pro"),
            PlanTier::Estate => write!(f, "Estate"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Active,
    PastDue,
    Canceled,
}

/// Server-authored display metadata for how the current plan was granted,
/// from `GET /connect/plan`'s `plan_provenance` (campaign engine, v2). The
/// desktop renders these strings verbatim and knows nothing about specific
/// campaigns — a campaign's label/badge/expiry are authored server-side, so
/// display never requires an app release. Absent (`None`) for ordinary
/// purchased/free plans and older backends → existing paid/free UX.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanProvenance {
    /// Primary descriptive line, e.g. "Free for your first year". Required
    /// when provenance is present.
    pub label: String,
    /// RFC-3339 instant the grant lapses, if it expires. Rendered as an
    /// "Expires {date}" line; `None`/absent → no expiry line.
    #[serde(default)]
    pub expires_at: Option<String>,
    /// Short tag shown beside the plan tier, e.g. "Founding member".
    /// `None`/absent → no badge.
    #[serde(default)]
    pub badge: Option<String>,
}

/// Per-plan entitlements from `GET /connect/plan`, mirroring the API's
/// `Entitlements` (the canonical model in `documentation/PRICING_AND_TIERS.md`).
///
/// EVERY field is `#[serde(default)]` on purpose: an entitlements object that
/// adds or renames a field must never fail the whole `ConnectPlan` parse and
/// silently drop the account to the Free tier. That is exactly the regression
/// this struct replaced — the previous (boolean-feature) model required six
/// fields the API had stopped sending, so every account rendered as Free. An
/// absent entitlement defaulting to "off"/0 is the safe direction.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanEntitlements {
    /// Personal (free) signing keys allowed on the tier.
    #[serde(default)]
    pub personal_key_limit: u32,
    /// Cubes allowed on the tier. (The home screen's live limit comes from
    /// `get_cube_limits`; this is the plan-catalog value.)
    #[serde(default)]
    pub cube_limit: u32,
    #[serde(default)]
    pub recovery_kit_limit: u32,
    /// Avatar regenerations allowed: `None` = unlimited (Estate), `Some(0)` =
    /// disabled (Free).
    #[serde(default)]
    pub avatar_regeneration_limit: Option<u32>,
    /// Whether the tier includes duress — gates the duress enrollment UI.
    #[serde(default)]
    pub duress: bool,
    #[serde(default)]
    pub attach_policies: bool,
    #[serde(default)]
    pub collaborative_invitations: bool,
    /// Estate-only: duress-activation alert contacts (SMS/WhatsApp/email
    /// fan-out when duress fires). See `PLAN-estate-notifications.md` PR 1.
    #[serde(default)]
    pub duress_alerts: bool,
    /// Vault recovery-path monitoring (timelock heartbeat → keyholder emails).
    /// After the recovery-alerts cleanup (API PR 3) the server returns this
    /// `true` on **all** plans, so the alerts toggle is available to everyone;
    /// the desktop keeps reading it as a defense-in-depth gate. See
    /// `PLAN-estate-notifications.md` PR 2 and `PLAN-recovery-alerts-cleanup.md`.
    #[serde(default)]
    pub recovery_alerts: bool,
    /// Estate-only: the server-blind ECIES **inheritance escrow** — the
    /// encrypted recovery kit (descriptor, optionally seed) sealed to each
    /// keyholder's key. Gates the "What keyholders can recover" tier selector on
    /// the Recovery Alerts card, separately from the (universal) alerts toggle
    /// above. `#[serde(default)]` means an older API that omits it fails
    /// **closed** (the selector shows its locked affordance) — the safe
    /// direction for a paid feature. Wire key `inheritanceEscrow` (the API's
    /// canonical name in `entitlements.go`; via the struct-level camelCase). See
    /// `PLAN-recovery-alerts-cleanup.md` PR 2 + ADDENDUM.
    #[serde(default)]
    pub inheritance_escrow: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectPlan {
    pub plan: PlanTier,
    pub status: PlanStatus,
    pub renewal_at: Option<String>,
    pub entitlements: PlanEntitlements,
    /// Billing cycle of the current plan. `None` for free tier (no charge).
    #[serde(default)]
    pub billing_cycle: Option<BillingCycle>,
    /// Server-authored display metadata for a campaign-granted plan (v2).
    /// `None`/absent for purchased/free plans and older backends — the
    /// desktop renders this verbatim and never special-cases campaigns.
    /// (`ConnectPlan` is camelCase, so the wire key is `planProvenance`; the
    /// alias also accepts a snake_case `plan_provenance`.)
    #[serde(default, alias = "plan_provenance")]
    pub plan_provenance: Option<PlanProvenance>,
}

impl ConnectPlan {
    /// Convenience accessor — returns `&self.plan` so existing call sites
    /// that used the old `tier` field can migrate with minimal churn.
    pub fn tier(&self) -> &PlanTier {
        &self.plan
    }
}

// ── Plan Features (public pricing endpoint) ─────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct PlanPrice {
    pub monthly: u32,
    pub annual: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanFeatureInfo {
    pub name: String,
    pub price: Option<PlanPrice>,
    pub features: Vec<String>,
    #[serde(default)]
    pub included_linked_participants: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FeaturesResponse {
    pub plans: Vec<PlanFeatureInfo>,
    /// Version of the pricing schema the backend emitted. The desktop
    /// build understands up to `SUPPORTED_PRICING_SCHEMA_VERSION`; a
    /// higher value means the server is describing plans/prices with a
    /// newer contract this build can't fully render, so the picker shows
    /// a soft "update available" note. `None`/absent (older backends, or
    /// the field unset) is treated as version 0 — never outdated.
    #[serde(
        default,
        alias = "schemaVersion",
        alias = "pricingSchemaVersion",
        alias = "pricing_schema_version"
    )]
    pub pricing_schema_version: Option<u32>,
    /// Whether self-service purchasing is currently available. The July-4
    /// Estate promo disables checkout server-side; when this is
    /// `Some(false)` the desktop hides every purchase path so it never
    /// routes anyone to a `POST /connect/checkout` the API will reject.
    /// Absent/`None` (older backends, or purchasing simply on) is treated
    /// as enabled — keeps the existing flow intact for fall GA. See
    /// `ConnectAccountPanel::purchasing_enabled`.
    #[serde(default, alias = "purchasingEnabled", alias = "purchasing_enabled")]
    pub purchasing_enabled: Option<bool>,
    /// Master server switch for the whole Marketplace section (Buy/Sell +
    /// P2P). When `Some(false)` — or absent/unreachable — the desktop hides
    /// the Marketplace nav entirely and makes every route under it
    /// unreachable. Unlike `purchasing_enabled`, this fails **closed**: an
    /// absent flag reads as *off*, so a launch build never surfaces the
    /// untested money feature on a stale or missing API response. See
    /// `ConnectAccountPanel::marketplace_server_flags` and
    /// `crate::app::features::MarketplaceServerFlags`.
    #[serde(default, alias = "marketplaceEnabled", alias = "marketplace_enabled")]
    pub marketplace_enabled: Option<bool>,
    /// Server switch for Buy/Sell (fiat on/off-ramp). Only consulted when
    /// `marketplace_enabled` is on. Fails closed (absent → off).
    #[serde(default, alias = "buySellEnabled", alias = "buy_sell_enabled")]
    pub buy_sell_enabled: Option<bool>,
    /// Server switch for P2P trading. Only consulted when
    /// `marketplace_enabled` is on. Fails closed (absent → off).
    #[serde(default, alias = "p2pEnabled", alias = "p2p_enabled")]
    pub p2p_enabled: Option<bool>,
    /// Account-scoped grandfather flag for the Liquid wallet (sunset phase 1).
    /// `Some(true)` means this account may create/see a Liquid wallet on a
    /// fresh install; absent/`Some(false)` means it may not.
    ///
    /// **Only meaningful on an authenticated call.** `/connect/features` is
    /// wrapped in *optional* JWT auth, so an anonymous request returns
    /// `false` silently rather than erroring — there is no way to distinguish
    /// "not granted" from "we forgot the bearer token". The desktop only ever
    /// fetches features after `set_token` (see
    /// `ConnectAccountPanel::post_login_tasks`), which is what makes this
    /// readable at all.
    ///
    /// A `false` here never hides an *existing* Liquid wallet — see
    /// [`crate::app::features::LiquidGate`], which OR's this with local state.
    #[serde(default, alias = "liquidEnabled", alias = "liquid_enabled")]
    pub liquid_enabled: Option<bool>,
    /// Per-user launch flag for the Duress Mode surface (`duressEnabled`).
    /// `Some(true)` means the server permits this account to see the duress
    /// enrollment/management UI; absent/`Some(false)` means it doesn't.
    ///
    /// Fails **closed** like the Marketplace flags: absent, unloaded, or an
    /// unreachable API all read as *off*, so the public launch build ships
    /// duress dark and never surfaces the untested setup on a stale response.
    ///
    /// The same authenticated-only caveat as `liquid_enabled` applies — the
    /// desktop only fetches features after `set_token` (see
    /// `ConnectAccountPanel::post_login_tasks`), so this is only meaningful on
    /// a signed-in call.
    ///
    /// A `false` here never hides duress from an *already-enrolled* account —
    /// see [`crate::app::features::DuressGate`], which OR's this with the
    /// account's enrollment state (the client mirror of the server's
    /// grandfather rule).
    #[serde(default, alias = "duressEnabled", alias = "duress_enabled")]
    pub duress_enabled: Option<bool>,
}

// ── Checkout / Billing ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BillingCycle {
    Monthly,
    Annual,
}

impl std::fmt::Display for BillingCycle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BillingCycle::Monthly => write!(f, "Monthly"),
            BillingCycle::Annual => write!(f, "Annual"),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutRequest {
    pub plan: PlanTier,
    pub billing_cycle: BillingCycle,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutResponse {
    pub charge_id: String,
    pub lightning_invoice: String,
    pub on_chain_address: String,
    pub amount_sats: u64,
    pub amount_fiat: f64,
    pub fiat_currency: String,
    pub plan: PlanTier,
    pub billing_cycle: BillingCycle,
    pub checkout_url: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChargeStatus {
    Unpaid,
    Processing,
    Paid,
    Expired,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChargeStatusResponse {
    pub charge_id: String,
    pub status: ChargeStatus,
    pub plan: PlanTier,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BillingHistoryEntry {
    pub charge_id: String,
    pub plan: PlanTier,
    pub billing_cycle: BillingCycle,
    pub amount_sats: u64,
    pub amount_fiat: f64,
    pub fiat_currency: String,
    pub status: ChargeStatus,
    pub created_at: String,
    pub paid_at: Option<String>,
}

// ── Campaign code redemption (v2 campaign engine) ───────────────────────────

/// Request body for `POST /api/v1/connect/campaigns/redeem`. The desktop
/// surface is campaign-agnostic — it just forwards whatever code the user
/// typed; the server validates window/limits/enabled and applies the
/// benefit.
#[derive(Debug, Clone, Serialize)]
pub struct RedeemCampaignRequest {
    pub code: String,
}

/// Success response for a redeemed campaign code. `message` is an optional
/// server-authored confirmation line (rendered verbatim); the desktop
/// refreshes `GET /connect/plan` afterwards to pick up the granted tier and
/// provenance, so no other fields are needed here. Failures arrive as the
/// usual typed error (`invalid | expired | exhausted | already-redeemed`)
/// and surface through `CoincubeError`'s message, rendered generically.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedeemCampaignResponse {
    #[serde(default)]
    pub message: Option<String>,
}

/// The monotonic, upgrade-only Vault-presence report for a Cube
/// (PLAN-duress-vault-gate PR 3). Returns `Some(true)` only when this device
/// holds the Vault (`local_has_vault`) and the server doesn't already show it
/// — otherwise `None` (leave the server value untouched).
///
/// Never returns `false`: a device that lacks the Vault locally can't tell a
/// genuinely vaultless Cube from one whose Vault lives on another device, so
/// asserting `false` could clobber a `true` reported elsewhere and silently
/// unblock that Cube's duress gate. `server_has_vault` is the value from
/// `list_cubes` (or `None` at pure registration, where there's nothing to
/// clobber yet).
pub fn vault_presence_report(
    local_has_vault: bool,
    server_has_vault: Option<bool>,
) -> Option<bool> {
    (local_has_vault && server_has_vault != Some(true)).then_some(true)
}

/// Request body for POST /api/v1/connect/cubes
#[derive(Debug, Clone, Serialize)]
pub struct RegisterCubeRequest {
    pub uuid: String,
    pub name: String,
    pub network: String,
    /// Vault presence for the duress vault gate (PLAN-duress-vault-gate
    /// PR 3), reported so other devices — which can't see this device's
    /// local `settings.json` — can tell whether a duress wipe of this Cube
    /// would be irreversible.
    ///
    /// **Monotonic / upgrade-only**: send `Some(true)` *only* when this
    /// device actually holds the Vault (`vault_wallet_id.is_some()`);
    /// otherwise `None` (omitted). A device that lacks the Vault locally
    /// can't tell "this Cube has no Vault" from "the Vault lives on another
    /// device", so it must never assert `false` and clobber a `true` some
    /// other device reported. A brand-new Cube's `false` baseline comes from
    /// the server default; from there `hasVault` only ever ratchets up —
    /// which for a security gate fails safe (stale-`true` over-blocks; it
    /// never wrongly unblocks). Vault *removal* is out of scope (v1).
    #[serde(rename = "hasVault", skip_serializing_if = "Option::is_none")]
    pub has_vault: Option<bool>,
}

/// Request body for PUT /api/v1/connect/cubes/{id}
#[derive(Debug, Clone, Serialize)]
pub struct UpdateCubeRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Re-report Vault presence without a full re-registration — set when a
    /// Vault is created on an already-registered Cube so the server's
    /// `hasVault` flips immediately (PLAN-duress-vault-gate PR 3). `None`
    /// leaves the server value untouched (e.g. a name-only rename).
    #[serde(rename = "hasVault", skip_serializing_if = "Option::is_none")]
    pub has_vault: Option<bool>,
}

/// Request body for `PUT /api/v1/connect/cubes/{cubeId}/encryption-pubkey` —
/// the Cube's Connect-blinding encryption pubkey
/// (`SPEC-cube-xpub-envelope-v1` §3).
///
/// Public material: 33-byte compressed secp256k1, lowercase hex. The server
/// canonicalises to lowercase and validates the point before storing. The
/// private half is derived from the Cube's master seed on demand and never
/// leaves the device.
#[derive(Debug, Clone, Serialize)]
pub struct PutCubeEncryptionPubkeyRequest {
    #[serde(rename = "encryptionPubkey")]
    pub encryption_pubkey: String,
}

/// Response of both the `PUT` and `GET` on
/// `/connect/cubes/{cubeId}/encryption-pubkey`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CubeEncryptionPubkeyResponse {
    pub cube_id: u64,
    /// `None` when the owner hasn't registered yet — the signal that the
    /// registration wave still needs to run, and the reason envelope-mode
    /// enrolment is refused for this Cube until it does.
    pub encryption_pubkey: Option<String>,
}

/// Request body for
/// `POST /api/v1/connect/cubes/{cubeId}/keys/{keyId}/envelope-invalid`.
///
/// `reason` is a closed set (`"decrypt_failed"` | `"xpub_invalid"`) — it is
/// echoed into the audit trail and the keyholder's re-enrol email, so free text
/// would be an injection surface. Build it with
/// [`crate::services::connect::crypto::KeyResolveError::report_reason`].
#[derive(Debug, Clone, Serialize)]
pub struct ReportEnvelopeInvalidRequest {
    pub reason: String,
}

/// Response from POST/GET /api/v1/connect/cubes/{id}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CubeResponse {
    pub id: u64,
    pub uuid: String,
    pub name: String,
    pub network: String,
    pub lightning_address: Option<String>,
    pub status: String,
    /// Whether the cube has a Cube Recovery Kit on the server. Sent by both
    /// `list_cubes()` and `GET /connect/cubes/{id}` (Go `hasRecoveryKit`,
    /// set from `RecoveryKit != nil`). `#[serde(default)]` keeps older
    /// payloads parsing (treated as no kit).
    #[serde(default)]
    pub has_recovery_kit: bool,
    /// Server-reported Vault presence for this Cube (the duress vault gate;
    /// PLAN-duress-vault-gate PR 3). Sent by `list_cubes` once devices report
    /// it via register/update. `Option` with `#[serde(default)]` so an older
    /// API that omits the field parses as `None` → `Unknown` vault state for
    /// *other-device* Cubes only (this device always knows its own Cubes from
    /// local `settings.json`, which wins over this value).
    #[serde(default)]
    pub has_vault: Option<bool>,
    /// The Cube's registered Connect-blinding encryption pubkey (33-byte
    /// compressed secp256k1, lowercase hex) when one has been registered.
    /// `None` on an API that predates the field, or on a Cube whose owner
    /// hasn't run the registration wave yet — in which case Contacts' Keychains
    /// have nothing to seal their xpubs to. Public material.
    #[serde(default)]
    pub encryption_pubkey: Option<String>,
    /// Populated by `GET /connect/cubes/{id}` (not by `list_cubes`). Defaults
    /// to empty so existing list-based code paths keep working.
    #[serde(default)]
    pub members: Vec<CubeMember>,
    #[serde(default)]
    pub pending_invites: Vec<CubeInviteSummary>,
    /// The cube's attached Vault when one exists. Populated by
    /// `GET /connect/cubes/{id}`; `None` when the cube has no vault
    /// yet or when served from `list_cubes()` (which omits the
    /// association). Drives the W16-desktop "Joined after Vault"
    /// badge and the Keyholder-role gate.
    #[serde(default)]
    pub vault: Option<ConnectVaultResponse>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CubeMember {
    pub id: u64,
    pub user_id: u64,
    pub user: CubeMemberUser,
    pub joined_at: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CubeMemberUser {
    pub email: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CubeInviteSummary {
    pub id: u64,
    pub cube_id: u64,
    pub email: String,
    pub status: String,
    pub expires_at: String,
    pub created_at: String,
}

/// Result of `POST /connect/cubes/{cubeId}/invites`. The backend returns
/// `{status, member, invite}` where exactly one of `member`/`invite` is set
/// depending on `status`. We normalise that into an enum.
#[derive(Debug, Clone, Deserialize)]
#[serde(try_from = "CubeInviteOrAddResultRaw")]
pub enum CubeInviteOrAddResult {
    /// The invitee was already a contact — they were added as a member
    /// immediately.
    Added(CubeMember),
    /// The invitee is not yet a contact — an invite was created and the
    /// pending-cube-attachment row will be fanned out on accept.
    Invited(CubeInviteSummary),
}

#[derive(Debug, Clone, Deserialize)]
struct CubeInviteOrAddResultRaw {
    status: String,
    #[serde(default)]
    member: Option<CubeMember>,
    #[serde(default)]
    invite: Option<CubeInviteSummary>,
}

impl std::convert::TryFrom<CubeInviteOrAddResultRaw> for CubeInviteOrAddResult {
    type Error = String;

    fn try_from(raw: CubeInviteOrAddResultRaw) -> Result<Self, Self::Error> {
        match raw.status.as_str() {
            "added" => raw
                .member
                .map(CubeInviteOrAddResult::Added)
                .ok_or_else(|| "expected `member` when status=added".to_string()),
            "invited" => raw
                .invite
                .map(CubeInviteOrAddResult::Invited)
                .ok_or_else(|| "expected `invite` when status=invited".to_string()),
            other => Err(format!("unexpected cube-invite status: {}", other)),
        }
    }
}

/// A key returned by `GET /api/v1/connect/cubes/{cubeUuid}/keys`.
///
/// Two backend shapes coexist during the W3 rollout:
///
/// 1. **Legacy** — the flat `models.Key` dump with `primaryOwnerId`,
///    `keychainId`, `curve`, `taproot`, `cubeId`, `createdAt`,
///    `updatedAt`. Owner resolution (self vs. contact) is done client-side.
/// 2. **W3 (post-PLAN-cube-membership-backend)** — a purpose-built
///    `CubeKeyResponse` that drops most of the above and adds the
///    viewer-relative `ownerUserId` / `ownerEmail` / `isOwnKey` /
///    `usedByVault` fields.
///
/// Fields that appear in *both* shapes (`id`, `name`, `xpub`,
/// `fingerprint`, `derivationPath`, `network`, `status`) are required —
/// missing them indicates a broken backend response and should fail
/// deserialisation fast. Rollout-specific fields (the legacy-only and
/// W3-only sets below) are individually `#[serde(default)]` so the
/// desktop keeps working against whichever shape the server happens to
/// serve. See `plans/PLAN-cube-membership-desktop.md` §2.3.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CubeKeyRaw {
    // --- Required fields (present in both legacy and W3 shapes) ---
    pub id: u64,
    pub name: String,
    /// Plaintext xpub. **Empty once the key is blinded** — Connect serves
    /// [`Self::xpub_envelope`] instead (`PLAN-connect-blinding` Track A). It
    /// stays populated during the dual-write window and for keys enrolled
    /// before blinding shipped, so both shapes must be tolerated. Read it
    /// through [`crate::services::connect::crypto::resolve_key_xpub`], which
    /// prefers the envelope and applies the post-decrypt validation the server
    /// can no longer do; never parse this field directly.
    #[serde(default)]
    pub xpub: String,
    /// The blinded xpub, sealed by the key owner's Keychain to this Cube's
    /// encryption pubkey (`SPEC-cube-xpub-envelope-v1`). Present instead of
    /// [`Self::xpub`] once the key is enrolled in envelope mode.
    #[serde(default)]
    pub xpub_envelope: Option<crate::services::connect::crypto::XpubEnvelope>,
    pub fingerprint: String,
    pub derivation_path: String,
    pub network: String,
    pub status: String,

    // --- Legacy fields (may disappear post-W3) ---
    #[serde(default)]
    pub primary_owner_id: u64,
    #[serde(default)]
    pub keychain_id: Option<u64>,
    #[serde(default)]
    pub curve: String,
    #[serde(default)]
    pub taproot: bool,
    #[serde(default)]
    pub cube_id: u64,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,

    // --- W3 fields (post-PLAN-cube-membership-backend) ---
    /// Server-supplied owner id; falls back to `primary_owner_id` when
    /// talking to a pre-W3 backend.
    #[serde(default)]
    pub owner_user_id: u64,
    /// Email of the key's primary owner. Empty on a pre-W3 backend; the
    /// desktop falls back to a contact-list lookup in that case.
    #[serde(default)]
    pub owner_email: String,
    /// `true` iff the authenticated caller is the owner of this key.
    /// Pre-W3 this is always `false` from the server; the desktop computes
    /// it locally.
    #[serde(default)]
    pub is_own_key: bool,
    /// `true` iff this key is currently referenced by any active Vault.
    /// Drives the W9 pre-check in the Vault Builder key picker.
    #[serde(default)]
    pub used_by_vault: bool,
    /// Recovery-recipient annotation: `"owner-self"` when this key backs the
    /// Cube's owner-self recovery recipient (a key that *restores* the Cube but
    /// must never be a Vault signer — invariant I2), empty otherwise. On the
    /// `/keys` endpoint the API only annotates owner-self keys; heir-recipient
    /// keys are left blank because an heir's key legitimately signs today.
    /// `#[serde(default)]` so a pre-annotation backend deserialises as before.
    #[serde(default)]
    pub recovery_role: String,
}

/// `recoveryRole` value marking a key as the Cube's owner-self recovery
/// recipient. Mirrors the API's `models.RecoveryRecipientRoleOwnerSelf`.
pub const RECOVERY_ROLE_OWNER_SELF: &str = "owner-self";

/// `status` value for a key whose xpub envelope the owner reported as
/// unopenable (`models.KeyStatusEnvelopeInvalid`, `coincube-api` PR A4).
///
/// **Not a revocation.** The key material is presumed fine; the *ciphertext*
/// isn't. The server clears the stale envelope with the flag, so such a row
/// arrives with neither an `xpub` nor an `xpubEnvelope` and stays unusable
/// until its owner re-enrols from their Keychain.
pub const KEY_STATUS_ENVELOPE_INVALID: &str = "envelope_invalid";

impl CubeKeyRaw {
    /// Returns the server-supplied `ownerUserId` when present, falling back
    /// to the legacy `primaryOwnerId`. Callers should prefer this over
    /// reading either field directly.
    pub fn effective_owner_user_id(&self) -> u64 {
        if self.owner_user_id != 0 {
            self.owner_user_id
        } else {
            self.primary_owner_id
        }
    }

    /// `true` iff this key is the Cube's owner-self recovery key. Such a key
    /// restores the Cube but can never be a Vault signer (I2), so the picker
    /// shows it as a disabled row and the selection handler refuses it.
    pub fn is_owner_self_recovery(&self) -> bool {
        self.recovery_role == RECOVERY_ROLE_OWNER_SELF
    }

    /// `true` iff this key is awaiting re-enrolment after its xpub envelope was
    /// reported unopenable. It has no readable key material at all, so it can't
    /// be placed in a Vault until its owner re-shares it.
    pub fn is_envelope_invalid(&self) -> bool {
        self.status == KEY_STATUS_ENVELOPE_INVALID
    }
}

/// Response from GET /api/v1/connect/cubes/limits
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CubeLimitsResponse {
    pub network: String,
    pub current_count: i64,
    pub max_allowed: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifiedDevice {
    pub id: u32,
    pub device_name: Option<String>,
    pub created_at: String,
    pub last_used_at: Option<String>,
    #[serde(default)]
    pub is_current: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginActivity {
    pub id: u32,
    pub ip_address: Option<String>,
    pub device_name: Option<String>,
    pub created_at: String,
    pub success: Option<bool>,
}

/// Generic wrapper for API responses: `{ "success": true, "data": T }`
#[derive(Debug, Clone, Deserialize)]
pub struct ApiResponse<T> {
    pub data: T,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LightningAddress {
    pub lightning_address: Option<String>,
}

/// Response of `GET /api/v1/connect/cubes/{id}/lightning-address/check`.
/// Our API's answer is authoritative for `@coincube.io` usernames — it is
/// the same conflict source the reserve step hits, including reservations
/// that never made it to the Breez-hosted LNURL server.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LightningAddressAvailability {
    pub available: bool,
    pub username: String,
}

/// Error response shape: `{ "success": false, "error": { "code": "...", "message": "..." } }`
#[derive(Debug, Clone, Deserialize)]
pub struct ApiErrorResponse {
    pub error: ApiErrorDetail,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiErrorDetail {
    pub code: String,
    pub message: String,
}

/// Reserve-only step of the Phase 4g claim flow. The server stores
/// the pending username against the cube but does NOT stamp the
/// record confirmed until a follow-up `/confirm` call lands.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReserveLightningAddressRequest {
    pub username: String,
}

/// Body for `PUT /api/v1/connect/cubes/{id}/lightning-address`.
/// Atomic server-side username swap on a cube that already has a
/// confirmed Lightning Address.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateLightningAddressRequest {
    pub username: String,
}

pub fn get_countries() -> &'static [Country] {
    static COUNTRIES_JSON: &str = include_str!("../countries.json");
    static COUNTRIES: std::sync::OnceLock<Vec<Country>> = std::sync::OnceLock::new();

    COUNTRIES
        .get_or_init(|| serde_json::from_str(COUNTRIES_JSON).unwrap())
        .as_slice()
}

// =============================================================================
// Avatar System Types
// =============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AvatarArchetype {
    Ronin,
    Samurai,
    Shogun,
}

impl std::fmt::Display for AvatarArchetype {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AvatarArchetype::Ronin => write!(f, "Ronin"),
            AvatarArchetype::Samurai => write!(f, "Samurai"),
            AvatarArchetype::Shogun => write!(f, "Shogun"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AvatarGender {
    Man,
    Woman,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AvatarAgeFeel {
    Young,
    Mature,
    Elder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AvatarDemeanor {
    Calm,
    Fierce,
    Mysterious,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AvatarArmorStyle {
    Light,
    Standard,
    Heavy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AvatarAccentMotif {
    OrangeSun,
    Splatter,
    Seal,
    Calligraphy,
}

/// User-selected questionnaire inputs. Serialized as the request body for
/// POST /api/v1/connect/avatar/generate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvatarUserTraits {
    pub gender: AvatarGender,
    pub archetype: AvatarArchetype,
    pub age_feel: AvatarAgeFeel,
    pub demeanor: AvatarDemeanor,
    pub armor_style: AvatarArmorStyle,
    pub accent_motif: AvatarAccentMotif,
    pub laser_eyes: bool,
}

impl Default for AvatarUserTraits {
    fn default() -> Self {
        Self {
            gender: AvatarGender::Man,
            archetype: AvatarArchetype::Ronin,
            age_feel: AvatarAgeFeel::Mature,
            demeanor: AvatarDemeanor::Mysterious,
            armor_style: AvatarArmorStyle::Light,
            accent_motif: AvatarAccentMotif::Calligraphy,
            laser_eyes: false,
        }
    }
}

/// Traits derived deterministically from the Lightning address seed (read-only).
#[derive(Debug, Clone, Deserialize)]
pub struct AvatarDerivedTraits {
    pub pose: String,
    pub crop_style: String,
    pub hat_style: String,
    pub face_visibility: String,
    pub eye_visibility: String,
    pub weapon_mode: String,
    pub shoulder_profile: String,
    pub cloak_presence: String,
    pub armor_wear: String,
    pub enso_style: String,
    pub ink_density: String,
    pub brush_texture: String,
    pub splash_intensity: String,
    pub orange_placement: String,
    pub ornament_level: String,
}

/// Human-readable prompt directives (read-only, server-side provenance).
#[derive(Debug, Clone, Deserialize)]
pub struct AvatarResolvedDirectives {
    pub composition: String,
    pub silhouette: String,
    pub face_treatment: String,
    pub armor_treatment: String,
    pub mood: String,
    pub orange_treatment: String,
    pub ink_treatment: String,
    pub eyes_treatment: String,
    pub background: String,
    pub archetype_flavor: String,
}

/// Full avatar identity object returned by the API and cached locally.
#[derive(Debug, Clone, Deserialize)]
pub struct AvatarIdentity {
    pub version: u32,
    pub seed_version: u32,
    pub seed_hash: String,
    pub lightning_address: String,
    pub archetype: String,
    pub user_traits: AvatarUserTraits,
    pub derived_traits: AvatarDerivedTraits,
    pub resolved_directives: AvatarResolvedDirectives,
}

/// A single generated variant. `id` is the stable database ID used for
/// select and image-serve endpoints.
#[derive(Debug, Clone, Deserialize)]
pub struct AvatarVariant {
    pub id: u64,
    pub index: u32,
    pub image_url: String,
}

/// Request body for POST /api/v1/connect/avatar/generate.
/// Only user_traits is sent — lightning address and variant count are
/// resolved server-side from the JWT.
#[derive(Debug, Clone, Serialize)]
pub struct AvatarGenerateRequest {
    pub user_traits: AvatarUserTraits,
}

/// Data returned by POST /api/v1/connect/avatar/generate.
#[derive(Debug, Clone, Deserialize)]
pub struct AvatarGenerateData {
    pub identity: AvatarIdentity,
    pub variant: AvatarVariant,
}

/// Request body for POST /api/v1/connect/avatar/select.
/// Only variant_id is sent — lightning address is resolved server-side.
#[derive(Debug, Clone, Serialize)]
pub struct AvatarSelectRequest {
    pub variant_id: u64,
}

/// Data returned by POST /api/v1/connect/avatar/select.
#[derive(Debug, Clone, Deserialize)]
pub struct AvatarSelectData {
    pub active_avatar_url: String,
    pub variant_id: u64,
}

/// Data returned by GET /api/v1/connect/avatar.
#[derive(Debug, Clone, Deserialize)]
pub struct GetAvatarData {
    pub has_avatar: bool,
    #[serde(default)]
    pub active_avatar_url: Option<String>,
    pub identity: Option<AvatarIdentity>,
    #[serde(default)]
    pub variants: Vec<AvatarVariant>,
    pub regenerations_remaining: i32,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// Data returned by GET /api/v1/connect/avatar/public/{lightning_address}.
#[derive(Debug, Clone, Deserialize)]
pub struct PublicAvatarData {
    pub lightning_address: String,
    pub avatar_url: String,
    pub archetype: String,
}

/// Data returned by GET /api/v1/connect/avatar/regenerations.
/// Plan tier is NOT included (op-sec).
#[derive(Debug, Clone, Deserialize)]
pub struct RegenerationData {
    pub total_allowed: i32,
    pub used: i32,
    pub remaining: i32,
}

// =============================================================================
// Contacts System Types
// =============================================================================

/// A contact's role as reported by the API (lowercase on the wire).
///
/// Deserialization is deliberately **lenient**: an unrecognised value maps
/// to [`ContactRole::Unknown`] instead of erroring. Serde aborts the entire
/// response on an unknown variant, so a single new server-side role would
/// otherwise blank the whole contact list — and everything that awaits it.
/// That is not hypothetical: the backend added `"owner"` and it took out the
/// keychain key picker, which fails on `get_contacts()` before it can build
/// the (contact-independent) "My Keychain Keys" list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ContactRole {
    Keyholder,
    Beneficiary,
    Observer,
    /// Reciprocal role the API writes on the *invitee's* side of the contact
    /// pair when an invite is accepted — "this contact is the person who
    /// invited me" (`coincube-api/.../invite/handlers/invite.go`, `contact2`).
    /// Only ever received, never sent: the invite form offers Keyholder alone,
    /// and the server rejects it on `POST /connect/invites` (`invite.go:87`).
    Owner,
    /// A role this build does not know about. Only produced by
    /// deserialization; treated as "no capability" everywhere it is matched.
    Unknown,
}

impl<'de> Deserialize<'de> for ContactRole {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Matched case-insensitively so a backend that ever switches to
        // "Keyholder" doesn't silently degrade every row to Unknown.
        let raw = String::deserialize(deserializer)?;
        Ok(match raw.to_ascii_lowercase().as_str() {
            "keyholder" => ContactRole::Keyholder,
            "beneficiary" => ContactRole::Beneficiary,
            "observer" => ContactRole::Observer,
            "owner" => ContactRole::Owner,
            _ => ContactRole::Unknown,
        })
    }
}

impl std::fmt::Display for ContactRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ContactRole::Keyholder => write!(f, "Keyholder"),
            ContactRole::Beneficiary => write!(f, "Beneficiary"),
            ContactRole::Observer => write!(f, "Observer"),
            ContactRole::Owner => write!(f, "Owner"),
            ContactRole::Unknown => write!(f, "Unknown"),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContactUser {
    pub id: u64,
    pub email: String,
    /// Backend's `ContactResponse.ContactUser` omits this (it's a
    /// `UserSummary`, not a full user); desktop was overly strict before.
    #[serde(default)]
    pub email_verified: Option<bool>,
}

/// A contact row returned by `GET /api/v1/connect/contacts`.
///
/// The backend's `ContactResponse` is intentionally a lean summary —
/// only `{id, contactUser, role, createdAt}`. The flat fields
/// `userId`, `contactUserId`, `inviteId` aren't part of the wire shape;
/// they're marked `#[serde(default)]` so legacy payloads still
/// deserialise. Callers that need the contact's user id should use
/// [`Contact::effective_contact_user_id`] which prefers the nested
/// `contact_user.id` over the flat field.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Contact {
    pub id: u64,
    /// Relationship-owner's user id — tautological from the caller's
    /// perspective. Not in the current backend response.
    #[serde(default)]
    pub user_id: u64,
    /// Flat `contactUserId` from the legacy shape. Use
    /// [`Contact::effective_contact_user_id`] rather than reading this
    /// field directly — it will be zero when talking to the current
    /// backend.
    #[serde(default)]
    pub contact_user_id: u64,
    #[serde(default)]
    pub invite_id: Option<u64>,
    pub role: ContactRole,
    /// Nested user summary. The current backend marks this optional
    /// (`omitempty`); an entry without a contact user is skippable at
    /// the call site.
    #[serde(default)]
    pub contact_user: Option<ContactUser>,
    pub created_at: String,
}

impl Contact {
    /// Returns the contact's user id, preferring the nested
    /// `contact_user.id` (the source of truth in the current backend's
    /// `ContactResponse`) and falling back to the legacy flat
    /// `contact_user_id` only when the nested object is missing.
    /// Returns `None` when the contact has no linked user at all.
    pub fn effective_contact_user_id(&self) -> Option<u64> {
        self.contact_user
            .as_ref()
            .map(|u| u.id)
            .filter(|id| *id != 0)
            .or_else(|| (self.contact_user_id != 0).then_some(self.contact_user_id))
    }
}

/// How a [`CubeKeyRaw`]'s owner resolves relative to the authenticated viewer.
///
/// Produced by [`classify_cube_key_ownership`] — the single home for the
/// self/contact classification shared by the Vault Builder key picker
/// (`resolve_cube_keys`) and the sign-flow membership reconcile
/// (`reconcile_vault_members`).
#[derive(Debug)]
pub enum CubeKeyOwnership<'a> {
    /// The authenticated viewer owns this key.
    SelfOwned { owner_id: u64 },
    /// A contact of the viewer owns this key. `contact` is the matched row,
    /// carrying the `contact_id` callers need to address the owner.
    ContactOwned { owner_id: u64, contact: &'a Contact },
    /// The owner is neither the viewer nor any of the viewer's contacts, so
    /// there is no `contact_id` to address them with. Callers skip such keys:
    /// the picker can't offer them and the reconcile can't attach them
    /// (`AddVaultMember` would reject a contact-less non-own key).
    Unresolved { owner_id: u64 },
}

/// Classifies a Cube key by ownership relative to `current_user_id`.
///
/// Ownership prefers the server's viewer-relative `is_own_key` flag when set,
/// falling back to a local id comparison for pre-W3 backends where the field
/// is always `false`.
///
/// A non-own key is matched to a contact by **identity alone** — deliberately
/// never by [`ContactRole`]. The role is a property of the *contact
/// relationship*, not of any Cube: the API's cube-invite handler instant-adds
/// an already-existing contact as a cube member without re-stamping the role
/// (`.../cube_member/handlers/cube_member.go`, the `contact != nil` branch),
/// and the reciprocal row written on accept carries role `owner`
/// (`.../invite/handlers/invite.go`, `contact2`). So a genuine Cube keyholder
/// routinely has a non-keyholder contact role, and filtering on it silently
/// hid their key. The authorisation that matters is enforced server-side:
/// `AddVaultMember` re-validates that the contact belongs to the caller and
/// that the key belongs to that contact's user — it does not check the role
/// either. The lookup goes through [`Contact::effective_contact_user_id`]
/// because the backend's lean `ContactResponse` omits the flat
/// `contactUserId`, exposing the id only via the nested `contactUser`.
pub fn classify_cube_key_ownership<'a>(
    key: &CubeKeyRaw,
    contacts: &'a [Contact],
    current_user_id: u64,
) -> CubeKeyOwnership<'a> {
    let owner_id = key.effective_owner_user_id();
    let is_own = key.is_own_key || owner_id == current_user_id;
    if is_own {
        return CubeKeyOwnership::SelfOwned { owner_id };
    }
    match contacts
        .iter()
        .find(|c| c.effective_contact_user_id() == Some(owner_id))
    {
        Some(contact) => CubeKeyOwnership::ContactOwned { owner_id, contact },
        None => CubeKeyOwnership::Unresolved { owner_id },
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Invite {
    pub id: u64,
    pub owner_user_id: u64,
    pub invitee_email: String,
    pub invitee_user_id: Option<u64>,
    pub role: ContactRole,
    pub status: String,
    pub expires_at: String,
    pub created_at: String,
}

/// A pending invite addressed to the authenticated user, returned by
/// `GET /api/v1/connect/invites/received`. Distinct from [`Invite`] —
/// `Invite` is outbound (sender's view) while `ReceivedInvite` is
/// inbound (recipient's view). The backend filters this list to
/// pending, non-expired invites only
/// (`coincube-api/services/connect/invite/handlers/invite.go:374-429`),
/// so the desktop renders it as-is without further filtering.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceivedInvite {
    pub id: u64,
    pub owner_email: String,
    pub role: ContactRole,
    pub expires_at: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateInviteRequest {
    pub email: String,
    pub role: ContactRole,
    /// Optional list of cube ids to pre-attach the invitee to. When empty
    /// the field is omitted from the JSON body so older staging servers
    /// (pre-W10, which don't recognise the field) keep working.
    /// See `plans/PLAN-cube-membership-desktop.md` §2.7.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cube_ids: Vec<u64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContactCube {
    pub id: u64,
    pub uuid: String,
    pub name: String,
    pub network: String,
    pub has_recovery_kit: bool,
}

// =============================================================================
// Connect Vault types
// =============================================================================
//
// The backend's `ConnectVault` is attached to a cube. A vault owns many
// `ConnectVaultMember` rows, each referencing a `ConnectContact` and/or a
// `Key`. The desktop installer creates the vault shell via
// `POST /connect/cubes/{cubeId}/vault` and fans out member rows via
// `POST /connect/cubes/{cubeId}/vault/members`.
//
// W9 guard: adding a member with a `keyId` that's already attached to
// another vault returns 409 with error code `KEY_ALREADY_USED_IN_VAULT`.
// The helper `CoincubeError::is_key_already_used_in_vault()` (below)
// lets callers route that into the Vault Builder's "key conflict" dialog.

/// Role a contact plays on a vault (mirrors `models.InviteRole`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VaultMemberRole {
    Keyholder,
    Beneficiary,
    Observer,
}

impl std::fmt::Display for VaultMemberRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Keyholder => write!(f, "Keyholder"),
            Self::Beneficiary => write!(f, "Beneficiary"),
            Self::Observer => write!(f, "Observer"),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateConnectVaultRequest {
    pub timelock_days: i32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddVaultMemberRequest {
    /// `Some` for contact-scoped members (a contact's key is being added).
    /// `None` when the vault owner adds their own key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contact_id: Option<u64>,
    /// Backend key id. `None` for contact-only members (e.g. Beneficiary)
    /// that don't contribute a signing key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_id: Option<u64>,
    pub role: VaultMemberRole,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultMemberKeySummary {
    pub id: u64,
    pub name: String,
    /// Plaintext xpub. **Empty once the key is blinded** — Connect serves
    /// [`Self::xpub_envelope`] instead (`PLAN-connect-blinding` Track A).
    /// `#[serde(default)]` so an envelope-only response still deserialises.
    /// Read it through [`crate::services::connect::crypto::resolve_key_xpub`],
    /// never directly.
    #[serde(default)]
    pub xpub: String,
    /// The blinded xpub, sealed by the keyholder's Keychain to the owning
    /// Cube's encryption pubkey (`SPEC-cube-xpub-envelope-v1`). Present instead
    /// of [`Self::xpub`] once the key is enrolled in envelope mode.
    #[serde(default)]
    pub xpub_envelope: Option<crate::services::connect::crypto::XpubEnvelope>,
    pub derivation_path: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultMemberContactSummary {
    pub id: u64,
    #[serde(default)]
    pub contact_user: Option<VaultMemberContactUserSummary>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultMemberContactUserSummary {
    pub id: u64,
    pub email: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultMemberResponse {
    pub id: u64,
    #[serde(default)]
    pub contact_id: Option<u64>,
    #[serde(default)]
    pub key_id: Option<u64>,
    pub role: VaultMemberRole,
    #[serde(default)]
    pub contact: Option<VaultMemberContactSummary>,
    #[serde(default)]
    pub key: Option<VaultMemberKeySummary>,
    pub created_at: String,
}

/// Vault lifecycle status. Drives W16-desktop's Keyholder-role gate:
/// the signing quorum is immutable on `Active` vaults, so the UI must
/// hide the Keyholder role option there.
///
/// `Other(String)` is a forward-compat fallback so an unknown backend
/// value deserialises as a readable string instead of failing the
/// whole `ConnectVaultResponse`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(from = "String")]
pub enum VaultStatus {
    Active,
    Expired,
    Archived,
    Other(String),
}

impl From<String> for VaultStatus {
    fn from(s: String) -> Self {
        match s.as_str() {
            "active" => VaultStatus::Active,
            "expired" => VaultStatus::Expired,
            "archived" => VaultStatus::Archived,
            _ => VaultStatus::Other(s),
        }
    }
}

impl VaultStatus {
    /// True for vaults whose signing quorum is still sealed — the
    /// Keyholder-role gate hides the option for these.
    pub fn is_active(&self) -> bool {
        matches!(self, VaultStatus::Active)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectVaultResponse {
    pub id: u64,
    pub cube_id: u64,
    pub timelock_days: i32,
    pub timelock_expires_at: String,
    pub last_reset_at: String,
    pub status: VaultStatus,
    #[serde(default)]
    pub members: Vec<VaultMemberResponse>,
    pub created_at: String,
    pub updated_at: String,
}

/// Which `VaultMemberRole` options the Vault-member add UI should
/// expose, given the target Vault's current status.
///
/// W16-desktop (2026-04-20 product decision): the Bitcoin multisig
/// descriptor is sealed at Vault-build time. Adding a Keyholder after
/// the fact would create a DB row that has no effect on signing, so
/// we hide the option on `Active` vaults and the backend 409s if it
/// slips through.
///
/// On `Expired` / `Archived` vaults (and on any unknown status —
/// fail-open) Keyholder stays in the list because the backend will
/// accept it.
pub fn allowed_vault_member_roles(vault_status: Option<&VaultStatus>) -> Vec<VaultMemberRole> {
    let mut roles = vec![VaultMemberRole::Beneficiary, VaultMemberRole::Observer];
    let hide_keyholder = vault_status.is_some_and(|s| s.is_active());
    if !hide_keyholder {
        roles.insert(0, VaultMemberRole::Keyholder);
    }
    roles
}

/// True when `member.joined_at` lands strictly after the Vault's
/// `created_at`. Callers can pass both values as RFC 3339 strings
/// (what the backend emits); the comparison falls back to
/// string-lexical order when either value fails to parse, which is
/// still correct for the `2006-01-02T15:04:05Z` layout the backend
/// uses.
pub fn member_joined_after_vault(member_joined_at: &str, vault_created_at: &str) -> bool {
    // Parse both as RFC 3339; if either fails, fall back to
    // lex-compare — the backend's fixed `yyyy-MM-ddTHH:mm:ssZ`
    // format sorts correctly lexically.
    let member = chrono::DateTime::parse_from_rfc3339(member_joined_at).ok();
    let vault = chrono::DateTime::parse_from_rfc3339(vault_created_at).ok();
    match (member, vault) {
        (Some(m), Some(v)) => m > v,
        _ => member_joined_at > vault_created_at,
    }
}

/// A person the API's recovery sweep would email when this Vault's recovery
/// window approaches or opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryRecipient {
    pub email: String,
    pub role: VaultMemberRole,
}

/// Outcome of [`recovery_recipients`]: who'd be emailed, plus how many Vault
/// members hold a notifiable role but can't be reached.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecoveryRecipients {
    /// The distinct people the sweep would email, keyholders first.
    pub notified: Vec<RecoveryRecipient>,
    /// Keyholder/beneficiary members with no contact email on file. The server
    /// skips these silently, so they're counted rather than listed — a Vault
    /// whose keyholders were all added by key alone has nobody to alert even
    /// though the member rows exist.
    pub unreachable: usize,
}

/// The people the API would email about this Vault's recovery window, derived
/// from its member rows.
///
/// Mirrors the server's own resolution in
/// `coincube-api/services/connect/vault/monitoring/repository.go`
/// (`Repository.RecoveryRecipients`) so the desktop shows exactly the set the
/// sweep will mail, and nothing else:
///
/// - only `keyholder` and `beneficiary` roles (observers are never notified);
/// - the address is the member's contact's user email — a member with no
///   contact row, or a contact with no email, is skipped;
/// - deduped by email, with the **keyholder** row winning a role conflict
///   (the server's `role DESC, id ASC` ordering puts "keyholder" first).
///
/// Note this is a different set from the escrow recipients that
/// `services::inheritance::escrow::keyholders_from_vault` builds: escrow seals
/// only to keyholders with a *registered key*, whereas alerts go to anyone with
/// a reachable email. A beneficiary is alerted but holds no envelope.
pub fn recovery_recipients(members: &[VaultMemberResponse]) -> RecoveryRecipients {
    let mut out = RecoveryRecipients::default();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    // Two passes rather than a sort, so keyholders win a duplicate email
    // regardless of the order the server listed the rows in.
    for role in [VaultMemberRole::Keyholder, VaultMemberRole::Beneficiary] {
        for m in members.iter().filter(|m| m.role == role) {
            let email = m
                .contact
                .as_ref()
                .and_then(|c| c.contact_user.as_ref())
                .map(|u| u.email.as_str())
                .unwrap_or("");
            if email.is_empty() {
                out.unreachable += 1;
                continue;
            }
            if seen.insert(email.to_string()) {
                out.notified.push(RecoveryRecipient {
                    email: email.to_string(),
                    role,
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod recovery_recipients_tests {
    use super::*;

    /// A member row with the given role and, optionally, a contact email.
    fn member(id: u64, role: VaultMemberRole, email: Option<&str>) -> VaultMemberResponse {
        VaultMemberResponse {
            id,
            contact_id: email.map(|_| id),
            key_id: None,
            role,
            contact: email.map(|e| VaultMemberContactSummary {
                id,
                contact_user: Some(VaultMemberContactUserSummary {
                    id,
                    email: e.to_string(),
                }),
            }),
            key: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn lists_keyholders_and_beneficiaries_keyholders_first() {
        let got = recovery_recipients(&[
            member(1, VaultMemberRole::Beneficiary, Some("bea@example.com")),
            member(2, VaultMemberRole::Keyholder, Some("kay@example.com")),
        ]);
        assert_eq!(
            got.notified,
            vec![
                RecoveryRecipient {
                    email: "kay@example.com".into(),
                    role: VaultMemberRole::Keyholder,
                },
                RecoveryRecipient {
                    email: "bea@example.com".into(),
                    role: VaultMemberRole::Beneficiary,
                },
            ]
        );
        assert_eq!(got.unreachable, 0);
    }

    #[test]
    fn observers_are_never_notified() {
        // The server's role filter is `IN (keyholder, beneficiary)` — an
        // observer is neither listed nor counted as unreachable.
        let got = recovery_recipients(&[member(
            1,
            VaultMemberRole::Observer,
            Some("obs@example.com"),
        )]);
        assert!(got.notified.is_empty());
        assert_eq!(got.unreachable, 0);
    }

    #[test]
    fn duplicate_email_keeps_the_keyholder_row() {
        // One person invited under two roles is one recipient, and the
        // more-capable role wins (it selects the escrow-aware alert copy).
        let got = recovery_recipients(&[
            member(1, VaultMemberRole::Beneficiary, Some("same@example.com")),
            member(2, VaultMemberRole::Keyholder, Some("same@example.com")),
        ]);
        assert_eq!(got.notified.len(), 1);
        assert_eq!(got.notified[0].role, VaultMemberRole::Keyholder);
        // The de-duplicated row is not "unreachable" — that person IS alerted.
        assert_eq!(got.unreachable, 0);
    }

    #[test]
    fn members_without_a_contact_email_are_counted_not_listed() {
        // A keyholder added by key alone (the Vault Builder's `contact_id:
        // None` path) has no address, so the sweep skips it silently.
        let mut no_email_contact = member(3, VaultMemberRole::Keyholder, Some("x@example.com"));
        no_email_contact.contact = Some(VaultMemberContactSummary {
            id: 3,
            contact_user: None,
        });
        let got = recovery_recipients(&[
            member(1, VaultMemberRole::Keyholder, None),
            member(2, VaultMemberRole::Beneficiary, None),
            no_email_contact,
        ]);
        assert!(got.notified.is_empty());
        assert_eq!(got.unreachable, 3);
    }

    #[test]
    fn a_vault_with_no_members_notifies_nobody() {
        let got = recovery_recipients(&[]);
        assert!(got.notified.is_empty());
        assert_eq!(got.unreachable, 0);
    }

    #[test]
    fn recipients_survive_a_round_trip_through_the_real_wire_shape() {
        // Guards the contract this whole feature rests on: the emails must
        // actually arrive on `GET /connect/cubes/{id}/vault`. Payload mirrors
        // the API's `VaultMemberResponse` / `ContactSummary` json tags
        // (coincube-api/services/connect/vault/types/types.go:35 and
        // services/connect/types/types.go:10) — a rename on either side breaks
        // this test rather than silently emptying the card.
        //
        // It's the *whole* server payload, including `fingerprint`, which
        // `ConnectVaultResponse` deliberately doesn't model: the point is that
        // this deserializes as the API actually sends it, not as a
        // desktop-shaped subset would.
        let vault: ConnectVaultResponse = serde_json::from_str(
            r#"{
                "id": 42,
                "cubeId": 7,
                "fingerprint": "1a2b3c4d",
                "timelockDays": 90,
                "timelockExpiresAt": "2026-11-01T00:00:00Z",
                "lastResetAt": "2026-08-01T00:00:00Z",
                "status": "active",
                "createdAt": "2026-08-01T00:00:00Z",
                "updatedAt": "2026-08-01T00:00:00Z",
                "members": [
                    {
                        "id": 1,
                        "contactId": 11,
                        "keyId": 21,
                        "role": "keyholder",
                        "contact": {
                            "id": 11,
                            "contactUser": { "id": 101, "email": "kay@example.com" }
                        },
                        "createdAt": "2026-08-01T00:00:00Z"
                    },
                    {
                        "id": 2,
                        "keyId": 22,
                        "role": "keyholder",
                        "createdAt": "2026-08-01T00:00:00Z"
                    },
                    {
                        "id": 3,
                        "contactId": 13,
                        "role": "observer",
                        "contact": {
                            "id": 13,
                            "contactUser": { "id": 103, "email": "obs@example.com" }
                        },
                        "createdAt": "2026-08-01T00:00:00Z"
                    }
                ]
            }"#,
        )
        .expect("the vault payload must deserialize");

        let got = recovery_recipients(&vault.members);
        assert_eq!(got.notified.len(), 1);
        assert_eq!(got.notified[0].email, "kay@example.com");
        // The key-only keyholder (no contact) is unreachable; the observer is
        // simply not a recipient.
        assert_eq!(got.unreachable, 1);
    }
}

/// Error code string returned by the backend's W9 guard. Public so callers
/// can match on it when routing 409s.
pub const ERR_KEY_ALREADY_USED_IN_VAULT: &str = "KEY_ALREADY_USED_IN_VAULT";

/// Error code returned by the backend's I2 guard: 409 from
/// `POST /connect/cubes/{cubeId}/vault/members` when the key is registered as a
/// recovery recipient and therefore may never be a Vault signer. Mirrors the
/// API's `responses.ErrKeyIsRecoveryRecipient`. Retrying is useless — the
/// sealed descriptor must be rebuilt without the recovery key — so the caller
/// rolls the just-created vault back (see `installer/connect_vault.rs`).
pub const ERR_KEY_IS_RECOVERY_RECIPIENT: &str = "KEY_IS_RECOVERY_RECIPIENT";

/// Error code returned by the backend's W16 guard (see
/// `coincube-api` PR 8): 409 from
/// `POST /connect/cubes/{cubeId}/vault/members` when `role=keyholder`
/// targets a Vault whose status is `active`. The 409 body carries the
/// `vaultId` of the locked vault; `add_vault_member` reclassifies
/// these into `CoincubeError::VaultKeyholderLocked { vault_id }`.
pub const ERR_VAULT_KEYHOLDER_LOCKED: &str = "VAULT_KEYHOLDER_LOCKED";

/// Body shape of the 409 response for `VAULT_KEYHOLDER_LOCKED`. The
/// backend inlines `vaultId` at the top level alongside the usual
/// `error: {code, message}` envelope (same pattern as
/// `KEY_ALREADY_USED_IN_VAULT`).
#[derive(Debug, Deserialize)]
struct VaultKeyholderLockedBody {
    #[serde(rename = "vaultId", default)]
    vault_id: u64,
}

/// Returns `Some(vault_id)` when `info` is a 409 whose error envelope
/// carries the `VAULT_KEYHOLDER_LOCKED` code. Used by
/// `add_vault_member` to reclassify the raw `Unsuccessful` into the
/// typed `CoincubeError::VaultKeyholderLocked` variant.
pub(crate) fn vault_keyholder_locked_vault_id(
    info: &crate::services::http::NotSuccessResponseInfo,
) -> Option<u64> {
    if info.status_code != 409 {
        return None;
    }
    let env = serde_json::from_str::<ApiErrorResponse>(&info.text).ok()?;
    if env.error.code != ERR_VAULT_KEYHOLDER_LOCKED {
        return None;
    }
    // vault_id is best-effort: if the backend omits it or sends a
    // non-u64, fall back to 0 — the caller still gets the typed
    // variant which is the whole point.
    let vault_id = serde_json::from_str::<VaultKeyholderLockedBody>(&info.text)
        .map(|b| b.vault_id)
        .unwrap_or(0);
    Some(vault_id)
}

impl CoincubeError {
    /// Returns `true` if this error is a W9 "key already used in another
    /// vault" conflict from `POST /connect/cubes/{id}/vault/members`.
    /// Drives the Vault Builder's key-conflict dialog.
    pub fn is_key_already_used_in_vault(&self) -> bool {
        let CoincubeError::Unsuccessful(info) = self else {
            return false;
        };
        if info.status_code != 409 {
            return false;
        }
        if let Ok(env) = serde_json::from_str::<ApiErrorResponse>(&info.text) {
            return env.error.code == ERR_KEY_ALREADY_USED_IN_VAULT;
        }
        false
    }

    /// Returns `true` if this error is the I2 "key is a recovery recipient"
    /// conflict from `POST /connect/cubes/{id}/vault/members` — the key backs a
    /// recovery recipient and can't be a Vault signer. Drives the vault
    /// rollback in the installer (a retry can't help; the descriptor must be
    /// rebuilt without the recovery key).
    pub fn is_key_is_recovery_recipient(&self) -> bool {
        let CoincubeError::Unsuccessful(info) = self else {
            return false;
        };
        if info.status_code != 409 {
            return false;
        }
        if let Ok(env) = serde_json::from_str::<ApiErrorResponse>(&info.text) {
            return env.error.code == ERR_KEY_IS_RECOVERY_RECIPIENT;
        }
        false
    }
}

// =============================================================================
// Cube Recovery Kit (W7)
// =============================================================================
//
// Backs the Settings → "Cube Recovery Kit" card and the installer restore
// flow. See `plans/PLAN-cube-recovery-kit-desktop.md` §2.2.
//
// The `encrypted_*` fields are opaque base64 envelopes produced by
// `services::recovery::envelope::encrypt`; the server stores and
// returns them verbatim.

/// Identifier for the only envelope scheme this client speaks today.
/// Sent to the backend on upsert so the server can refuse kits it can't
/// later hand back to older clients if the scheme ever changes.
pub const RECOVERY_KIT_SCHEME_AES_256_GCM: &str = "aes-256-gcm";

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryKitStatus {
    pub has_recovery_kit: bool,
    pub has_encrypted_seed: bool,
    pub has_encrypted_wallet_descriptor: bool,
    pub encryption_scheme: String,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    /// Owner-keychain ("phone") recovery summary, folded into the same
    /// `/recovery-kit/status` response by the API (presence/tier only, no
    /// ciphertext). `None` when the Cube has never used the envelope path.
    #[serde(default)]
    pub owner_self: Option<OwnerSelfRecoverySummary>,
}

/// Presence/tier summary of the owner-keychain recovery mode, mirrored from the
/// API's `ownerSelf` block on `/recovery-kit/status`. Carries no ciphertext.
/// A registered recipient (`has_recipient`) means a phone key exists; a
/// non-empty `envelope_kinds` means recovery material has actually been sealed
/// and uploaded to it — the signal that drives the "Keychain" backup pill.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnerSelfRecoverySummary {
    #[serde(default)]
    pub has_recipient: bool,
    #[serde(default)]
    pub tier: String,
    /// Distinct artifact kinds escrowed to the phone key (`"descriptor"`,
    /// `"seed"`). Empty when a recipient is registered but nothing sealed yet.
    #[serde(default)]
    pub envelope_kinds: Vec<String>,
    /// When the phone (keychain) envelope set was last sealed/updated, RFC 3339.
    /// Folded into the card's "Last updated" line alongside the password kit's
    /// `updated_at` (the later of the two wins). Absent on older APIs that don't
    /// yet emit it (API PR 1c) → `None`, which keeps the card's timestamp on the
    /// password kit's value.
    #[serde(default)]
    pub updated_at: Option<String>,
}

impl OwnerSelfRecoverySummary {
    /// Whether recovery material has actually been sealed to the phone key
    /// (not merely a recipient registered). Drives the "Keychain" backup pill.
    pub fn has_envelope(&self) -> bool {
        !self.envelope_kinds.is_empty()
    }
}

/// Deserialize a field that may arrive as:
///   - missing entirely (paired with `#[serde(default)]`),
///   - explicit JSON `null`, or
///   - a normal string.
///
/// All three reduce to the empty `String`, preserving the
/// `.is_empty()` convention the rest of the codebase uses to
/// detect "no half backed up". The current backend serialises
/// absent halves as `""` and never emits null or omits the field,
/// but `UpdateRecoveryKitRequest` already uses `*string` with
/// `omitempty` on the request side — the response side may trend
/// the same way, and this deserializer keeps the client robust
/// across that evolution without an API break.
fn null_as_empty_string<'de, D>(d: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(d)?.unwrap_or_default())
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryKit {
    pub id: u64,
    pub cube_id: u64,
    /// Opaque base64 envelope for the seed half; the empty string
    /// means "this half isn't backed up" (e.g. a passkey cube that
    /// can't extract its seed). Tolerates `null` / missing on the
    /// wire via `null_as_empty_string`; callers should continue to
    /// check `.is_empty()` rather than `.is_some()`.
    #[serde(default, deserialize_with = "null_as_empty_string")]
    pub encrypted_cube_seed: String,
    /// Opaque base64 envelope for the descriptor half; empty when
    /// the kit is seed-only (no Vault created yet, or the Vault
    /// wizard "skip" path). Same wire-tolerance as `encrypted_cube_seed`.
    #[serde(default, deserialize_with = "null_as_empty_string")]
    pub encrypted_wallet_descriptor: String,
    pub encryption_scheme: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Body for POST / PUT `/api/v1/connect/cubes/{cubeId}/recovery-kit`. Omits
/// `encryptedCubeSeed` / `encryptedWalletDescriptor` when `None`, which
/// the backend's partial-field create path (backend PR 1) uses to decide
/// which half of the kit the caller is touching.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpsertRecoveryKitRequest<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypted_cube_seed: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypted_wallet_descriptor: Option<&'a str>,
    pub encryption_scheme: &'a str,
}

// =============================================================================
// Duress (desktop) — Phase 0 client plumbing
// =============================================================================
//
// The desktop is the surface where duress *happens*. These DTOs back the
// Connect REST client methods in `client.rs`. Trust-posture notes that bind
// the shapes below:
//
//   * Every desktop generates its OWN ~128-bit duress code locally with a
//     CSPRNG, argon2id-hashes it, and sends only the hash. The server stores
//     N per-device hashes per account and never sees plaintext, so a DB breach
//     reveals only argon2id hashes of 128-bit inputs (infeasible to brute
//     force → no grief-triggering duress).
//   * `trigger-with-code` is UNAUTHENTICATED on purpose: the Cube-unlock
//     surface may be reached without a live Connect session, and even with one
//     we don't want activation to depend on session validity at the moment of
//     coercion.

/// Body for `POST /api/v1/connect/duress/enroll` (authenticated).
///
/// The enrolling desktop has already generated its own duress code and
/// argon2id-hashed it; only `duress_code_hash` crosses the wire. The raw code
/// lives solely in this desktop's `DuressLocalState`. `duress_crk_password_hash`
/// is `None` for Tier 2/3 (no CRK), `Some(..)` for Tier 1 (Approach C).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnrollDuressRequest {
    pub all_clear_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duress_crk_password_hash: Option<String>,
    pub unlock_delay_minutes: u32,
    pub device_fingerprint: String,
    pub duress_code_hash: String,
}

/// Body for `POST /api/v1/connect/duress/register-device-code` (authenticated).
/// Called by every desktop OTHER than the enrolling one, on its first sign-in
/// after the account has duress enrolled.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterDeviceDuressCodeRequest {
    pub device_fingerprint: String,
    pub duress_code_hash: String,
}

/// Body for `POST /api/v1/connect/duress/trigger-with-code` (UNAUTHENTICATED).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TriggerWithCodeRequest {
    pub account_id: String,
    pub duress_code: String,
}

/// Body for `POST /api/v1/connect/duress/clear` (authenticated).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClearDuressRequest {
    pub all_clear_passphrase_hash: String,
}

/// Returned by the trigger routes — the timestamp after which the account can
/// be cleared with the all-clear passphrase (the lockout-window expiry).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuressUnlockAt {
    pub unlock_at: chrono::DateTime<chrono::Utc>,
}

/// `GET /api/v1/connect/duress` (authenticated).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuressState {
    pub active: bool,
    #[serde(default)]
    pub unlock_at: Option<chrono::DateTime<chrono::Utc>>,
    pub enrolled: bool,
    /// Whether THIS desktop (by device fingerprint) already has a code hash
    /// registered server-side. `enrolled && !this_device_registered` means
    /// "new device on an enrolled account" → generate + register a code.
    #[serde(default)]
    pub this_device_registered: bool,
}

/// Classified result of the post-sign-in duress gate check (Phase 6).
///
/// Carried in a `Message`, so it must be `Clone` — `CoincubeError` wraps a
/// non-`Clone` `reqwest::Error` and can't be. Collapsing every failure to a
/// bare `None` (as the gate previously did) conflated "the server returned a
/// body I can't decode" (permanent — retrying is futile) with "the network is
/// down" (transient — retry) and "my token was rejected" (re-auth), so a
/// one-field contract typo became a silent, un-retryable lockout. This keeps
/// just enough to branch correctly.
#[derive(Debug, Clone)]
pub enum DuressCheckOutcome {
    /// Decoded the server's duress state.
    Ok(DuressState),
    /// Network / timeout / 5xx / rate-limit — transient; a bounded retry may
    /// succeed.
    Unreachable,
    /// A 200 whose body didn't match the contract (decode error) — the body is
    /// logged at the call site. Auto-retrying in a tight loop is futile, but a
    /// manual retry can still recover if the server is hotfixed.
    Incompatible,
    /// 401/403 — the session/token was definitively rejected; bounce to login
    /// rather than hold the gate closed forever or retry a credential the
    /// server won't accept.
    Unauthorized,
}

impl DuressCheckOutcome {
    /// Classify a failed `get_duress_state` call. (Success is constructed
    /// directly as [`DuressCheckOutcome::Ok`].)
    pub fn from_err(e: &CoincubeError) -> Self {
        match e {
            CoincubeError::Parse(_) => Self::Incompatible,
            // Mirror `CoincubeError::is_auth_error` (401/403): a rejected token
            // won't recover by retrying, so re-auth instead of backing off as if
            // the network were down. Keep these in sync to avoid 403 silently
            // falling through to a futile retry loop.
            _ if e.is_auth_error() => Self::Unauthorized,
            _ => Self::Unreachable,
        }
    }
}

#[cfg(test)]
mod duress_check_outcome_tests {
    use super::*;
    use crate::services::http::NotSuccessResponseInfo;

    fn unsuccessful(status_code: u16) -> CoincubeError {
        CoincubeError::Unsuccessful(NotSuccessResponseInfo {
            status_code,
            text: String::new(),
        })
    }

    #[test]
    fn auth_errors_map_to_unauthorized() {
        // Both 401 and 403 are "token definitively rejected" (see
        // is_auth_error) and must re-auth, not retry-with-backoff.
        assert!(matches!(
            DuressCheckOutcome::from_err(&unsuccessful(401)),
            DuressCheckOutcome::Unauthorized
        ));
        assert!(matches!(
            DuressCheckOutcome::from_err(&unsuccessful(403)),
            DuressCheckOutcome::Unauthorized
        ));
    }

    #[test]
    fn transient_and_decode_errors_classify_distinctly() {
        // 5xx / rate-limit / other non-auth statuses are transient.
        assert!(matches!(
            DuressCheckOutcome::from_err(&unsuccessful(503)),
            DuressCheckOutcome::Unreachable
        ));
        assert!(matches!(
            DuressCheckOutcome::from_err(&unsuccessful(429)),
            DuressCheckOutcome::Unreachable
        ));
        // A decode failure is a contract mismatch, not transient.
        let parse_err: CoincubeError = serde_json::from_str::<DuressState>("not json")
            .unwrap_err()
            .into();
        assert!(matches!(
            DuressCheckOutcome::from_err(&parse_err),
            DuressCheckOutcome::Incompatible
        ));
    }
}

/// Typed failure modes for the password-gated recovery-kit download
/// (Approach C, Phase 7). The server returns `423 Locked` with a
/// discriminating `error.code` for both the duress-lock and
/// trusted-device-delay cases; everything else collapses to `Invalid`
/// (wrong password / malformed) or `Other`.
#[derive(Debug)]
pub enum DownloadError {
    /// `423 DURESS_LOCKED` — the account is in duress; the kit is withheld
    /// until `unlock_at`.
    DuressLocked {
        unlock_at: Option<chrono::DateTime<chrono::Utc>>,
    },
    /// `423 TRUSTED_DEVICE_DELAY` — a fresh device must wait until
    /// `available_at` even with the correct password.
    TrustedDeviceDelay {
        available_at: Option<chrono::DateTime<chrono::Utc>>,
    },
    /// Wrong password / malformed request (`400`, `401`, `403`, or `422`).
    Invalid,
    /// `404 Not Found` — no kit exists for this cube.
    NotFound,
    /// `429 Too Many Requests` — the caller should back off. `retry_after`
    /// is the parsed `Retry-After` duration (defaults to 60s when the header
    /// is missing or malformed).
    RateLimited { retry_after: std::time::Duration },
    /// Network, 5xx, or parse failure.
    Other(CoincubeError),
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DownloadError::DuressLocked { .. } => {
                write!(f, "Recovery kit cannot be downloaded at this time.")
            }
            DownloadError::TrustedDeviceDelay { .. } => {
                write!(f, "Recovery kit download is delayed on new devices.")
            }
            DownloadError::Invalid => write!(f, "Incorrect recovery kit password."),
            DownloadError::NotFound => write!(f, "Recovery kit not found."),
            DownloadError::RateLimited { retry_after } => {
                write!(f, "Rate limited — try again in {}s.", retry_after.as_secs())
            }
            DownloadError::Other(e) => write!(f, "{}", e),
        }
    }
}

impl std::error::Error for DownloadError {}

/// `423 Locked` body shape, used to discriminate `DURESS_LOCKED` from
/// `TRUSTED_DEVICE_DELAY` and to recover the "wait until" timestamp.
///
/// The server (`responses.ErrorWithData`) puts the code and message in `error`
/// and the timestamp in a **sibling `data` object, snake_case**:
///
/// ```json
/// { "success": false,
///   "error": { "code": "TRUSTED_DEVICE_DELAY", "message": "…" },
///   "data":  { "available_at": "2026-08-09T14:00:00Z" } }
/// ```
///
/// This client used to look for `error.availableAt` / `error.unlockAt` — wrong
/// object *and* wrong casing — so the timestamp silently parsed as `None` on
/// every lock, and the UI showed a bare "delayed on new devices" with no
/// indication of how long. The `error`-object fields are still read as a
/// fallback so a server that adopts that shape keeps working.
#[derive(Debug, Deserialize)]
struct DuressLockEnvelope {
    error: DuressLockBody,
    /// `null` on locks that carry no timestamp (e.g. a zero `unlock_at`).
    #[serde(default)]
    data: Option<DuressLockData>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DuressLockBody {
    code: String,
    #[serde(default)]
    unlock_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    available_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// The `data` sibling of `error`. Snake_case on the wire, matching the Go
/// handlers' literal map keys (`available_at`, `duress_unlock_at`) rather than
/// the camelCase convention the rest of the API uses.
#[derive(Debug, Deserialize)]
struct DuressLockData {
    #[serde(default)]
    available_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    duress_unlock_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl DownloadError {
    /// Parses a `423 Locked` body into the discriminated variant. Falls back
    /// to `DuressLocked { unlock_at: None }` when the body can't be parsed —
    /// the safe default is to treat an opaque 423 as a duress lock rather than
    /// leak the kit.
    pub(crate) fn from_locked_body(body: &str) -> Self {
        match serde_json::from_str::<DuressLockEnvelope>(body) {
            Ok(env) if env.error.code == "TRUSTED_DEVICE_DELAY" => {
                DownloadError::TrustedDeviceDelay {
                    available_at: env
                        .data
                        .as_ref()
                        .and_then(|d| d.available_at)
                        .or(env.error.available_at),
                }
            }
            Ok(env) => DownloadError::DuressLocked {
                unlock_at: env
                    .data
                    .as_ref()
                    .and_then(|d| d.duress_unlock_at)
                    .or(env.error.unlock_at),
            },
            Err(_) => DownloadError::DuressLocked { unlock_at: None },
        }
    }
}

// =============================================================================
// Duress alert contacts (Estate Notifications — PR 1)
// =============================================================================
//
// Account-scoped contacts who receive a one-time intro message on
// enrollment and a single alert if duress activates. Estate-gated
// (`duress_alerts` entitlement). Backs the "Emergency contacts" panel in
// the duress settings surface. See `plans/PLAN-estate-notifications.md`
// PR 1 (desktop) and the coincube-api counterpart PR 1.
//
// Trust-posture notes:
//   * The contacts list is account-scoped PII (names, phones, emails) and
//     is ONLY ever rendered in normal-mode settings — never on the duress
//     activation/cryptic screen, where it would leak who gets alerted to a
//     coercer. The view layer enforces this; the data simply isn't fetched
//     while the panel is in a duress-active flow.
//   * `intro_sent_at` / `opted_out_at` are server-managed; the desktop
//     reads them to render delivery state but never sets them. A contact
//     with `opted_out_at` set has replied STOP and is never messaged again.

/// Channel bitmask bits for [`DuressAlertContact::channels`]. Matches the
/// coincube-api "channels mask" wire field. SMS/WhatsApp require a phone;
/// Email requires an email — the UI enforces that pairing before letting a
/// bit be set.
pub const DURESS_CHANNEL_SMS: &str = "sms";
pub const DURESS_CHANNEL_WHATSAPP: &str = "whatsapp";
pub const DURESS_CHANNEL_EMAIL: &str = "email";

/// A duress alert contact as returned by
/// `GET /api/v1/connect/duress/contacts`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuressAlertContact {
    pub id: u64,
    pub display_name: String,
    /// E.164 phone (e.g. `+15551234567`). `None` when the contact is
    /// email-only. At least one of `phone`/`email` is always set
    /// (enforced server-side and in the desktop add/edit form).
    #[serde(default)]
    pub phone: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    /// Bitmask of [`DURESS_CHANNEL_SMS`] / `_WHATSAPP` / `_EMAIL`.
    #[serde(default)]
    pub channels: Vec<String>,
    /// RFC 3339 timestamp of when the one-time intro message was sent,
    /// or `None` if it hasn't gone out yet (just-created contact).
    #[serde(default)]
    pub intro_sent_at: Option<String>,
    /// RFC 3339 timestamp of when the contact replied STOP. When set, the
    /// contact is permanently opted out and never messaged again.
    #[serde(default)]
    pub opted_out_at: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

impl DuressAlertContact {
    /// True when the contact has replied STOP and will not be messaged.
    pub fn is_opted_out(&self) -> bool {
        self.opted_out_at.is_some()
    }
    pub fn has_channel(&self, channel: &str) -> bool {
        self.channels.iter().any(|c| c == channel)
    }
}

/// Body for `POST /api/v1/connect/duress/contacts` (Estate-gated). At
/// least one of `phone`/`email` must be `Some`; `channels` must reference
/// only contact methods that are present. Both are validated client-side
/// before the call and re-checked server-side.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateDuressAlertContactRequest {
    pub display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    pub channels: Vec<String>,
}

/// Body for `PATCH /api/v1/connect/duress/contacts/{id}`. Every field is
/// optional — only the ones the user changed are sent. The API plan scopes
/// PATCH to "channel prefs", but the desktop edit form can also amend the
/// name / phone / email, so all four are partial-update fields. Fields left
/// `None` are omitted from the JSON body and untouched server-side.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateDuressAlertContactRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channels: Option<Vec<String>>,
}

/// Maximum duress alert contacts per account. Cost + abuse bound, mirrored
/// from the coincube-api cap (`PLAN-estate-notifications.md` PR 1).
pub const MAX_DURESS_ALERT_CONTACTS: usize = 5;

/// Validates a phone number as loosely-E.164: a leading `+`, a non-zero
/// first digit, and 1–15 digits total (ITU-T E.164 max). This is a
/// format gate for the input field, not a line-reachability check — the
/// server / sent.dm does the authoritative validation. Returns `true` for
/// the empty string so an email-only contact (no phone) passes; callers
/// separately enforce "at least one of phone/email".
pub fn is_valid_e164(phone: &str) -> bool {
    let p = phone.trim();
    if p.is_empty() {
        return true;
    }
    let Some(rest) = p.strip_prefix('+') else {
        return false;
    };
    let digits: Vec<char> = rest.chars().collect();
    if digits.is_empty() || digits.len() > 15 {
        return false;
    }
    if !digits.iter().all(|c| c.is_ascii_digit()) {
        return false;
    }
    // E.164 country codes never start with 0.
    digits[0] != '0'
}

#[cfg(test)]
mod duress_alert_contact_tests {
    use super::*;

    #[test]
    fn e164_accepts_well_formed_numbers() {
        assert!(is_valid_e164("+15551234567"));
        assert!(is_valid_e164("+447911123456"));
        assert!(is_valid_e164("+5491123456789"));
        // Empty = "no phone provided", which is allowed (email-only contact).
        assert!(is_valid_e164(""));
        assert!(is_valid_e164("  +15551234567 "));
    }

    #[test]
    fn e164_rejects_malformed_numbers() {
        assert!(!is_valid_e164("5551234567")); // no leading +
        assert!(!is_valid_e164("+0123456789")); // leading 0 after +
        assert!(!is_valid_e164("+1 555 123 4567")); // spaces
        assert!(!is_valid_e164("+1555123456789012")); // 16 digits, too long
        assert!(!is_valid_e164("+")); // no digits
        assert!(!is_valid_e164("+1-555-1234")); // dashes
    }

    #[test]
    fn channel_bits_are_distinct() {
        let c = DuressAlertContact {
            id: 1,
            display_name: "Jane".into(),
            phone: Some("+15551234567".into()),
            email: None,
            channels: vec![
                DURESS_CHANNEL_SMS.to_string(),
                DURESS_CHANNEL_WHATSAPP.to_string(),
            ],
            intro_sent_at: None,
            opted_out_at: None,
            created_at: Some("2026-06-11T00:00:00Z".to_string()),
            updated_at: Some("2026-06-11T00:00:00Z".to_string()),
        };
        assert!(c.has_channel(DURESS_CHANNEL_SMS));
        assert!(c.has_channel(DURESS_CHANNEL_WHATSAPP));
        assert!(!c.has_channel(DURESS_CHANNEL_EMAIL));
        assert!(!c.is_opted_out());
    }

    #[test]
    fn deserialises_minimal_and_tolerates_missing_optionals() {
        // Server may omit nullable fields entirely.
        let v = serde_json::json!({
            "id": 7,
            "displayName": "Sam",
            "email": "sam@example.com",
            "channels": [DURESS_CHANNEL_EMAIL],
        });
        let c: DuressAlertContact = serde_json::from_value(v).unwrap();
        assert_eq!(c.display_name, "Sam");
        assert!(c.phone.is_none());
        assert_eq!(c.email.as_deref(), Some("sam@example.com"));
        assert!(c.has_channel(DURESS_CHANNEL_EMAIL));
        assert!(c.intro_sent_at.is_none());
        assert!(c.created_at.is_none());
        assert!(c.updated_at.is_none());
    }
}

// =============================================================================
// Vault recovery monitoring (Estate Notifications — PR 2)
// =============================================================================
//
// Three-tier, per-vault opt-in for recovery-path monitoring. Keyed by the
// Connect vault numeric id (`ConnectVaultResponse::id`). Estate-gated
// (`recovery_alerts` entitlement). See `plans/PLAN-estate-notifications.md`
// PR 2 (desktop) and the coincube-api counterpart PRs 3–5.
//
// Trust-posture: "Full" uploads a service-encrypted copy of the vault
// descriptor so COINCUBE can watch the chain (it can see this vault's
// addresses + balances, never spend). "Alerts only" sends only a periodic
// timelock heartbeat (`earliest_recovery_height`), never the descriptor.
// "Off" is a true delete of any stored descriptor record. The opt-in copy
// in the UI states this trade plainly — no euphemisms.

/// Per-vault monitoring tier. Wire values `off` / `heartbeat` / `full`
/// match the coincube-api `monitoring_level` column (PR 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum VaultMonitoringLevel {
    /// No monitoring. Any stored descriptor record is true-deleted.
    #[default]
    Off,
    /// "Alerts only" — periodic timelock heartbeat. The server learns only
    /// the block height at which the recovery window opens, never the
    /// vault's addresses or balances. Keyholders still need the recovery
    /// password.
    Heartbeat,
    /// "Full" — a service-encrypted copy of the descriptor is escrowed so
    /// COINCUBE watches the chain and keyholders can recover without the
    /// owner's password.
    Full,
}

impl VaultMonitoringLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Heartbeat => "heartbeat",
            Self::Full => "full",
        }
    }
}

/// Status returned by `GET /api/v1/connect/cubes/{cubeId}/vault/monitoring`.
///
/// The API's `MonitoringStatusResponse` names these `monitoringLevel` and
/// `state` (not `level` / `lastNotifiedState`) — explicit `rename`s below
/// override the struct-level `camelCase` for those two fields.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultMonitoringStatus {
    #[serde(rename = "monitoringLevel", default)]
    pub level: VaultMonitoringLevel,
    /// Server's per-vault recovery state machine value, when the sweep has
    /// run: `none` / `approaching` / `available` / `reminding`. `None` when
    /// the API doesn't expose it (nice-to-have; the UI degrades silently).
    #[serde(rename = "state", default)]
    pub last_notified_state: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    /// Which ECIES artifact kinds the owner currently has escrowed for this
    /// Vault's keyholders. Drives the desktop's server-derived escrow tier
    /// (no session-tracked guess):
    /// - `["descriptor"]` → Vault-only,
    /// - `["descriptor","seed"]` → Full-Cube,
    /// - `[]` (present but empty) → alerts-only (nothing escrowed).
    ///
    /// `None` when the field is **absent** — an older API that predates the
    /// escrowed-artifacts report. The desktop then can't tell which tier is
    /// enrolled and falls back to the "on, tier unknown" copy rather than
    /// asserting a tier the server didn't confirm (C2). Wire key
    /// `escrowedArtifacts` (struct-level camelCase); `#[serde(default)]` keeps
    /// older payloads parsing.
    #[serde(default)]
    pub escrowed_artifacts: Option<Vec<String>>,
}

impl Default for VaultMonitoringStatus {
    fn default() -> Self {
        Self {
            level: VaultMonitoringLevel::Off,
            last_notified_state: None,
            updated_at: None,
            escrowed_artifacts: None,
        }
    }
}

/// Body for `POST /api/v1/connect/cubes/{cubeId}/vault/monitoring`
/// (Estate-gated). Sets the monitoring tier. `descriptor` is required for
/// [`VaultMonitoringLevel::Full`] (the escrowed copy) and omitted for
/// `Heartbeat`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetVaultMonitoringRequest {
    pub level: VaultMonitoringLevel,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub descriptor: Option<String>,
    /// Gap-limit hint so the server's sweep derives enough addresses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gap_limit: Option<u32>,
}

/// Body for `POST /api/v1/connect/cubes/{cubeId}/vault/heartbeat`
/// (Estate-gated, PR 5). Fire-and-forget after each vault sync for
/// Heartbeat-tier (and Full, as a cross-check) vaults.
/// `earliest_recovery_height` is the block height at which this vault's
/// earliest recovery branch opens; a newer report always wins server-side
/// (monotonic-staleness rule).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultHeartbeatRequest {
    pub earliest_recovery_height: u32,
    pub computed_at: chrono::DateTime<chrono::Utc>,
    /// Chain whose tip the server's sweep compares the reported height against,
    /// in the Esplora-proxy id form the API expects (`bitcoin-mainnet` /
    /// `bitcoin-testnet`). REQUIRED: an omitted/empty value makes the server
    /// assume mainnet, which would key a testnet vault's recovery height against
    /// the wrong tip and fire (or suppress) alerts incorrectly.
    pub network: String,
}

// =============================================================================
// Inheritance recovery — heir/keyholder discovery + descriptor release (COIN-377)
// =============================================================================
//
// The heir signs in to their OWN account and discovers Vaults they are a
// keyholder/beneficiary of (but do not own); once the recovery window is open
// on-chain they pull the owner's descriptor — server-decrypted, NO password —
// to drive the existing recovery-sweep screen as a watch-only vault. Two
// endpoints back this:
//
//   GET /api/v1/connect/cubes/recoverable                         (PR 1 list; NET-NEW)
//   GET /api/v1/connect/cubes/{cubeId}/vault/recovery-descriptor  (PR 2 fetch; built)
//
// The second endpoint already exists and is gated server-side
// (`coincube-api/services/connect/vault/monitoring/handler.go::GetRecoveryDescriptor`);
// the first is net-new on both sides and owned by the API counterpart plan.

/// Heir-facing recovery-window state for a vault. Collapses the API's
/// `RecoveryMonitoringState` machine (`none`/`approaching`/`available`/
/// `reminding`) to the two states the discovery UI acts on. Any unknown wire
/// value maps to `Approaching` — fail-closed, so we never render a "Recover"
/// button for a state this client doesn't understand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryState {
    /// `none`/`approaching` (and unknown) — visible but not actionable; show
    /// the expected-open date and "we'll email you", no descriptor access.
    Approaching,
    /// `available`/`reminding` — the recovery path is open on-chain; the
    /// descriptor can be released. The heir gets a "Recover" button.
    Open,
}

impl RecoveryState {
    /// Maps a raw server `state` / `last_notified_state` string to the
    /// heir-facing state. `available`/`reminding` → `Open`; everything else
    /// (`none`, `approaching`, unknown, empty) → `Approaching` (fail-closed).
    pub fn from_wire(s: &str) -> Self {
        match s {
            "available" | "reminding" => Self::Open,
            _ => Self::Approaching,
        }
    }

    pub fn is_open(self) -> bool {
        matches!(self, Self::Open)
    }
}

/// One row from `GET /api/v1/connect/cubes/recoverable` — a vault the
/// signed-in account is a keyholder/beneficiary of (but does not own) and may
/// be able to recover. **Net-new endpoint** (COIN-377 / API counterpart plan);
/// until it ships the desktop drives PR 1 off this documented shape behind the
/// capability flag, with fixtures for tests. Tolerant `#[serde(default)]` on
/// the optional/forward-compat fields so a slightly older/newer server shape
/// still deserialises.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoverableVault {
    /// The owner's numeric cube id — the path segment for the descriptor fetch.
    pub cube_id: u64,
    /// Owner-chosen label for the vault (display only).
    #[serde(default)]
    pub owner_label: Option<String>,
    /// Owner's monitoring tier. `Full` → password-free recovery (v1);
    /// `Heartbeat` → recovery password required (deferred to COIN-375).
    #[serde(default)]
    pub monitoring_level: VaultMonitoringLevel,
    /// The caller's own membership role on this vault (`keyholder` /
    /// `beneficiary`). Only keyholders can perform the password-free pull-down —
    /// the descriptor-release endpoint 403s everyone else — so a non-keyholder
    /// row is never a live "Recover" button. Defaults to non-keyholder (fails
    /// closed) if an older server omits it.
    #[serde(default)]
    pub role: String,
    /// Raw server recovery-window state (`none`/`approaching`/`available`/
    /// `reminding`). Use [`RecoverableVault::recovery_state`] for the
    /// heir-facing collapse.
    #[serde(default)]
    pub state: String,
    /// **Deprecated under the ECIES pivot (rev 3).** The old KEK model had a
    /// "recovery password" path; ECIES heir-escrow has none — the heir's
    /// Keychain decrypts. Retained only so a pre-pivot server payload still
    /// deserialises; actionability now keys off [`Self::available_tiers`], not
    /// this field. Defaults `true` (fails closed) when absent.
    #[serde(default = "default_requires_recovery_password")]
    pub requires_recovery_password: bool,
    /// "Owner last active" hint, when the server exposes it (display only).
    #[serde(default)]
    pub owner_last_active: Option<chrono::DateTime<chrono::Utc>>,
    /// Expected/known date the recovery window opens, for `approaching` rows.
    #[serde(default)]
    pub expected_open_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Optional address gap-limit hint for the recovered watch-only sync. The
    /// release endpoint returns no gap hint (it's owner-only), so if the API
    /// surfaces one it rides on this list row; otherwise the recovered vault
    /// syncs with a generous default gap.
    #[serde(default)]
    pub gap_limit: Option<u32>,
    /// Which ECIES artifact kinds are escrowed **for this caller** (PR C):
    /// `["descriptor"]` → Vault-only (heir recovers the watch-only Vault and
    /// sweeps); `["descriptor","seed"]` → Full-Cube (heir restores the whole
    /// Cube). Empty/absent → nothing escrowed for me, so not recoverable.
    /// Drives the row label ("Recover Vault" vs "Recover full Cube").
    #[serde(default)]
    pub available_tiers: Vec<String>,
}

impl RecoverableVault {
    /// Collapses the raw wire `state` into the heir-facing two-state view.
    pub fn recovery_state(&self) -> RecoveryState {
        RecoveryState::from_wire(&self.state)
    }

    /// Whether the caller is a keyholder (the only role the release endpoint
    /// serves envelopes to — beneficiaries are 403'd).
    pub fn is_keyholder(&self) -> bool {
        self.role == "keyholder"
    }

    /// Whether a descriptor envelope is escrowed for this caller. Every
    /// escrowed tier carries the descriptor, so this is the "anything to
    /// recover" check.
    pub fn has_descriptor_tier(&self) -> bool {
        self.available_tiers.iter().any(|t| t == "descriptor")
    }

    /// Whether the master seed is escrowed for this caller — the Full-Cube
    /// tier, which restores the entire Cube (Liquid + Spark + Vault) rather
    /// than just the watch-only Vault.
    pub fn has_seed_tier(&self) -> bool {
        self.available_tiers.iter().any(|t| t == "seed")
    }

    /// Full-Cube (seed escrowed) vs Vault-only (descriptor only). Drives the
    /// row label and which restore scope the heir flow uses.
    pub fn is_full_cube(&self) -> bool {
        self.has_seed_tier()
    }

    /// Whether the heir can act on this row now: the caller is a keyholder AND
    /// the window is open on-chain AND something is escrowed for them. Under
    /// ECIES there is no password gate — the heir's Keychain decrypts.
    pub fn is_recoverable_now(&self) -> bool {
        self.is_keyholder() && self.recovery_state().is_open() && self.has_descriptor_tier()
    }
}

/// Serde default for [`RecoverableVault::requires_recovery_password`]: a missing
/// field fails closed (assume a recovery password is required) so an
/// older/partial payload never makes a Heartbeat row look password-free.
fn default_requires_recovery_password() -> bool {
    true
}

/// Body of `GET /api/v1/connect/cubes/{cubeId}/vault/recovery-descriptor` on
/// 200 — the **plaintext** descriptor. The server decrypts the escrowed copy
/// under its KEK and returns it directly; the keyholder path carries no
/// password and does no client-side decryption.
///
/// **Superseded by the ECIES pivot (rev 3):** the server is now blind and
/// returns *ciphertext* envelopes via the recovery-envelope endpoint
/// ([`InheritanceEnvelopeWire`]); the heir's Keychain decrypts. Retained while
/// the pre-pivot endpoint is still deployed.
#[derive(Debug, Clone, Deserialize)]
pub struct RecoveryDescriptorResponse {
    pub descriptor: String,
}

/// One ECIES heir-escrow envelope on the wire (camelCase JSON; byte fields
/// lowercase hex, SPEC §5). The desktop **defines** this contract; `coincube-api` stores the
/// bytes opaquely (it never parses or decrypts them) and `keychain-app`
/// decrypts. Shared by owner upload (`PUT …/vault/escrow`) and gated heir
/// release (`GET …/vault/recovery-envelope`, which returns only the caller's
/// own envelopes). `keyholderKeyId` is bound into the ECIES AAD at seal time
/// (SPEC §1), so it MUST be present in **both** directions — the heir needs it
/// to rebuild the AAD and open the envelope (see the field doc).
///
/// `Debug` is manual: `ciphertext` is encrypted, but we still avoid dumping
/// the blob; the other fields are non-secret (public key, path, scheme).
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InheritanceEnvelopeWire {
    /// Which keyholder's xpub this is sealed to (`models.Key` id). Bound into
    /// the ECIES AAD at seal time (SPEC §1), so the heir **requires** it on the
    /// gated release to rebuild the AAD and open the envelope — `coincube-api`
    /// MUST include it on `GET …/vault/recovery-envelope` (the caller's own key
    /// id), not just echo it on upload. `Option` because the wire field can be
    /// absent (e.g. an older server); [`crate::services::inheritance`]'s
    /// `heir::open_blob` then fails closed with a clear error rather than
    /// silently producing a wrong AAD.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keyholder_key_id: Option<u64>,
    /// `"descriptor"` | `"seed"`.
    pub artifact_kind: String,
    /// ECIES scheme tag, e.g. `"ecies-secp256k1-hkdf-sha256-aes256gcm-v1"`.
    pub scheme: String,
    /// Lowercase hex (SPEC §5) of the 33-byte compressed ephemeral secp256k1
    /// public key.
    pub ephemeral_pubkey: String,
    /// Lowercase hex (SPEC §5) of `ciphertext || GCM tag`.
    pub ciphertext: String,
    /// Lowercase hex (SPEC §5) of the 12-byte GCM nonce.
    pub nonce: String,
    /// The non-hardened encryption child path (relative to the keyholder xpub).
    pub derivation: String,
}

impl std::fmt::Debug for InheritanceEnvelopeWire {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InheritanceEnvelopeWire")
            .field("keyholder_key_id", &self.keyholder_key_id)
            .field("artifact_kind", &self.artifact_kind)
            .field("scheme", &self.scheme)
            .field("ephemeral_pubkey", &self.ephemeral_pubkey)
            .field("derivation", &self.derivation)
            .field("ciphertext", &"<redacted>")
            .field("nonce", &"<redacted>")
            .finish()
    }
}

/// Body of `PUT /api/v1/connect/cubes/{cubeId}/vault/escrow` — the owner
/// uploads the **whole** envelope set for the cube's current keyholders. The
/// server idempotently replaces the stored set (handles keyholder
/// add/remove/key-rotate), validating structure + that each `keyholderKeyId`
/// is a current member; it never decrypts.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PutVaultEscrowRequest {
    pub envelopes: Vec<InheritanceEnvelopeWire>,
}

// =============================================================================
// Owner keychain recovery — "protect with my phone" (PLAN-owner-keychain-recovery)
// =============================================================================
//
// Distinct from the inheritance heir-escrow above: here the recovery recipient
// is the **owner's own** Keychain (role `owner-self`), not a designated heir.
// The owner mints + attaches an `owner-self` key, registers it as a recovery
// recipient, then seals their own seed/descriptor to it (ECIES, reusing
// `services::inheritance`). On a wiped install the owner pulls their own
// envelope set and decrypts it by approving on the Keychain — no password.
//
// Net-new endpoints (owned by the coincube-api counterpart plan), behind the
// `OWNER_KEYCHAIN_RECOVERY_ENABLED` flag until they ship:
//
//   POST /api/v1/connect/cubes/{cubeId}/recovery-kit/recipients   (register key)
//   GET  /api/v1/connect/cubes/{cubeId}/recovery-kit/recipients   (read xpub)
//   PUT  /api/v1/connect/cubes/{cubeId}/recovery-kit/envelope      (owner uploads set)
//   GET  /api/v1/connect/cubes/{cubeId}/recovery-kit/envelope      (owner downloads set)

/// Wire role string for an owner self-recovery recipient. Bound by the API plan;
/// `coincube-api` validates that this row is **not** a Vault signer (invariant
/// I2). Registration is phone-initiated (COIN-390) — the desktop only ever
/// *matches* this value when detecting the recipient ([`RecoveryKitRecipient::is_owner_self`]).
pub const RECOVERY_RECIPIENT_ROLE_OWNER_SELF: &str = "owner-self";

/// Which artifacts the owner intends to seal to their `owner-self` key. Mirrors
/// the inheritance escrow tier but without an `Off` state (registering a
/// recipient is always "on"). Wire values `vault_only` / `full_cube` match the
/// coincube-api `recovery_recipient.tier` column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnerRecoveryTier {
    /// Descriptor only — the owner can restore the watch-only Vault.
    VaultOnly,
    /// Seed + descriptor — the owner can restore the entire Cube.
    FullCube,
}

impl OwnerRecoveryTier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::VaultOnly => "vault_only",
            Self::FullCube => "full_cube",
        }
    }

    /// Whether this tier escrows the master seed (Full-Cube only).
    pub fn includes_seed(self) -> bool {
        matches!(self, Self::FullCube)
    }
}

/// The registered key behind a recovery recipient — the xpub + derivation path
/// the owner needs to seal envelopes (PR 2). The owner derives the dedicated
/// encryption child **xpub-only** from this (SPEC §2, child 7000); no private
/// material is ever on the owner side.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryRecipientKey {
    pub id: u64,
    pub xpub: String,
    pub derivation_path: String,
}

/// One recovery recipient row returned by
/// `GET /connect/cubes/{cubeId}/recovery-kit/recipients`. For owner self-recovery
/// there is a single `owner-self` row; the desktop reads its `key` to seal to.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryKitRecipient {
    pub id: u64,
    pub key_id: u64,
    pub role: String,
    #[serde(default)]
    pub tier: Option<OwnerRecoveryTier>,
    /// The registered key (xpub + derivation). `Option` because an older server
    /// could omit the join; the seal path then fails closed with a clear error
    /// rather than guessing an xpub.
    #[serde(default)]
    pub key: Option<RecoveryRecipientKey>,
}

impl RecoveryKitRecipient {
    /// True when this row is the owner's own self-recovery recipient.
    pub fn is_owner_self(&self) -> bool {
        self.role == RECOVERY_RECIPIENT_ROLE_OWNER_SELF
    }
}

/// Body for `PUT /connect/cubes/{cubeId}/recovery-kit/envelope` — the owner
/// uploads their own ECIES envelope set sealed to the `owner-self` key (PR 2).
/// Shares the opaque [`InheritanceEnvelopeWire`] shape with the heir escrow; the
/// server stores the bytes blind and never decrypts.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PutRecoveryKitEnvelopeRequest {
    pub envelopes: Vec<InheritanceEnvelopeWire>,
}

#[cfg(test)]
mod plan_entitlements_tests {
    use super::*;

    #[test]
    fn escrow_entitlement_reads_canonical_inheritance_escrow_key() {
        // Regression lock (PLAN-recovery-alerts-cleanup ADDENDUM): the escrow
        // selector gates on the API's canonical `inheritanceEscrow` key — NOT
        // the `recoveryEscrow` name the desktop first coined. A drift here silently
        // fails the selector closed even for Estate, so pin the exact wire key.
        let estate: PlanEntitlements = serde_json::from_value(serde_json::json!({
            "recoveryAlerts": true,
            "inheritanceEscrow": true
        }))
        .unwrap();
        assert!(estate.inheritance_escrow);
        assert!(estate.recovery_alerts);

        // The old (wrong) key must NOT satisfy the gate — proves the rename stuck.
        let wrong_key: PlanEntitlements =
            serde_json::from_value(serde_json::json!({ "recoveryEscrow": true })).unwrap();
        assert!(!wrong_key.inheritance_escrow);
    }

    #[test]
    fn escrow_entitlement_fails_closed_when_absent() {
        // Older API that omits the field → the selector fails closed (locked).
        let entitlements = PlanEntitlements::default();
        assert!(!entitlements.inheritance_escrow);
    }
}

#[cfg(test)]
mod vault_monitoring_tests {
    use super::*;

    #[test]
    fn level_wire_values() {
        assert_eq!(
            serde_json::to_string(&VaultMonitoringLevel::Full).unwrap(),
            "\"full\""
        );
        assert_eq!(
            serde_json::to_string(&VaultMonitoringLevel::Heartbeat).unwrap(),
            "\"heartbeat\""
        );
        assert_eq!(
            serde_json::to_string(&VaultMonitoringLevel::Off).unwrap(),
            "\"off\""
        );
        assert_eq!(VaultMonitoringLevel::default(), VaultMonitoringLevel::Off);
    }

    #[test]
    fn recoverable_vault_without_escrowed_tiers_is_not_recoverable() {
        // Under the ECIES pivot, actionability keys off `availableTiers` (what's
        // escrowed for the caller), not a recovery password. A payload that omits
        // it (older server, or nothing escrowed for this keyholder) must never
        // present a live "Recover" button — even open + keyholder.
        let v = serde_json::json!({
            "cubeId": 7,
            "role": "keyholder",
            "state": "available"
        });
        let row: RecoverableVault = serde_json::from_value(v).unwrap();
        assert!(row.available_tiers.is_empty());
        assert!(!row.has_descriptor_tier());
        assert!(!row.is_recoverable_now());
    }

    #[test]
    fn recoverable_vault_tier_flags_drive_actionability() {
        // Vault-only: descriptor escrowed → recoverable, not full-cube.
        let vault_only: RecoverableVault = serde_json::from_value(serde_json::json!({
            "cubeId": 7,
            "role": "keyholder",
            "state": "reminding",
            "availableTiers": ["descriptor"]
        }))
        .unwrap();
        assert!(vault_only.is_recoverable_now());
        assert!(vault_only.has_descriptor_tier());
        assert!(!vault_only.is_full_cube());

        // Full-Cube: seed escrowed too → recoverable and full-cube.
        let full_cube: RecoverableVault = serde_json::from_value(serde_json::json!({
            "cubeId": 8,
            "role": "keyholder",
            "state": "available",
            "availableTiers": ["descriptor", "seed"]
        }))
        .unwrap();
        assert!(full_cube.is_recoverable_now());
        assert!(full_cube.is_full_cube());

        // Escrowed but window not open → not recoverable yet.
        let not_open: RecoverableVault = serde_json::from_value(serde_json::json!({
            "cubeId": 9,
            "role": "keyholder",
            "state": "approaching",
            "availableTiers": ["descriptor", "seed"]
        }))
        .unwrap();
        assert!(!not_open.is_recoverable_now());

        // Escrowed + open, but caller is a beneficiary → not recoverable.
        let beneficiary: RecoverableVault = serde_json::from_value(serde_json::json!({
            "cubeId": 10,
            "role": "beneficiary",
            "state": "available",
            "availableTiers": ["descriptor"]
        }))
        .unwrap();
        assert!(!beneficiary.is_recoverable_now());
    }

    #[test]
    fn monitoring_status_tolerates_minimal_body() {
        // A vault with no monitoring record: server may send just the level,
        // under its real field name `monitoringLevel` (not `level` — see the
        // explicit `rename` on `VaultMonitoringStatus::level`).
        let v = serde_json::json!({ "monitoringLevel": "off" });
        let s: VaultMonitoringStatus = serde_json::from_value(v).unwrap();
        assert_eq!(s.level, VaultMonitoringLevel::Off);
        assert!(s.last_notified_state.is_none());
    }

    #[test]
    fn monitoring_status_reads_escrowed_artifacts() {
        // New API (API PR 1) reports the escrowed artifact kinds so the desktop
        // derives the tier from the server instead of a session-tracked guess.
        let vault_only: VaultMonitoringStatus = serde_json::from_value(serde_json::json!({
            "monitoringLevel": "heartbeat",
            "escrowedArtifacts": ["descriptor"]
        }))
        .unwrap();
        assert_eq!(
            vault_only.escrowed_artifacts.as_deref(),
            Some(&["descriptor".to_string()][..])
        );

        let full_cube: VaultMonitoringStatus = serde_json::from_value(serde_json::json!({
            "monitoringLevel": "heartbeat",
            "escrowedArtifacts": ["descriptor", "seed"]
        }))
        .unwrap();
        assert_eq!(
            full_cube.escrowed_artifacts.as_deref(),
            Some(&["descriptor".to_string(), "seed".to_string()][..])
        );

        // Present but empty → alerts-only (nothing escrowed) — distinct from absent.
        let alerts_only: VaultMonitoringStatus = serde_json::from_value(serde_json::json!({
            "monitoringLevel": "heartbeat",
            "escrowedArtifacts": []
        }))
        .unwrap();
        assert_eq!(alerts_only.escrowed_artifacts.as_deref(), Some(&[][..]));
    }

    #[test]
    fn monitoring_status_tolerates_absent_escrowed_artifacts() {
        // Old API (pre-PR-1) omits the field entirely → None, so the desktop
        // can distinguish "on, tier unknown" from "on, nothing escrowed".
        let s: VaultMonitoringStatus =
            serde_json::from_value(serde_json::json!({ "monitoringLevel": "heartbeat" })).unwrap();
        assert!(s.escrowed_artifacts.is_none());
    }

    #[test]
    fn monitoring_status_ignores_unrenamed_field_name() {
        // Regression guard: before the explicit `rename`, a body keyed on the
        // wrong field name (`level`/`lastNotifiedState` instead of the API's
        // real `monitoringLevel`/`state`) silently deserialized to the
        // defaults rather than erroring, which is exactly how a successful
        // enable could still render "Off" on the settings card.
        let v = serde_json::json!({ "level": "full", "lastNotifiedState": "approaching" });
        let s: VaultMonitoringStatus = serde_json::from_value(v).unwrap();
        assert_eq!(s.level, VaultMonitoringLevel::Off);
        assert!(s.last_notified_state.is_none());
    }

    #[test]
    fn set_request_omits_descriptor_for_heartbeat() {
        let req = SetVaultMonitoringRequest {
            level: VaultMonitoringLevel::Heartbeat,
            descriptor: None,
            gap_limit: Some(20),
        };
        let body = serde_json::to_value(&req).unwrap();
        assert_eq!(body["level"], "heartbeat");
        assert!(body.get("descriptor").is_none());
        assert_eq!(body["gapLimit"], 20);
    }

    #[test]
    fn set_request_includes_descriptor_for_full() {
        let req = SetVaultMonitoringRequest {
            level: VaultMonitoringLevel::Full,
            descriptor: Some("wsh(...)".into()),
            gap_limit: None,
        };
        let body = serde_json::to_value(&req).unwrap();
        assert_eq!(body["level"], "full");
        assert_eq!(body["descriptor"], "wsh(...)");
    }

    #[test]
    fn heartbeat_request_carries_network_id() {
        // The server requires the Esplora-proxy network id and rejects any value
        // outside `bitcoin-mainnet` / `bitcoin-testnet`; a missing field would
        // silently default it to mainnet server-side. Lock the wire field name
        // (`network`) and a representative value.
        let req = VaultHeartbeatRequest {
            earliest_recovery_height: 850_000,
            computed_at: chrono::Utc::now(),
            network: "bitcoin-testnet".to_string(),
        };
        let body = serde_json::to_value(&req).unwrap();
        assert_eq!(body["earliestRecoveryHeight"], 850_000);
        assert_eq!(body["network"], "bitcoin-testnet");
    }
}

#[cfg(test)]
mod recovery_kit_response_tests {
    //! Regression tests for `RecoveryKit` deserialisation tolerance.
    //! The current backend always sends both ciphertext fields as
    //! (possibly empty) strings, but the wire shape could evolve
    //! toward nullable/omitted halves (request side already uses
    //! `*string` with `omitempty`). Any of the four shapes below
    //! must deserialise; `.is_empty()` is the caller's existing
    //! "no half backed up" check.
    use super::RecoveryKit;
    use serde_json::json;

    fn kit_with_halves(
        seed: serde_json::Value,
        descriptor: serde_json::Value,
    ) -> serde_json::Value {
        json!({
            "id": 1,
            "cubeId": 42,
            "encryptedCubeSeed": seed,
            "encryptedWalletDescriptor": descriptor,
            "encryptionScheme": "aes-256-gcm",
            "createdAt": "2026-04-23T00:00:00Z",
            "updatedAt": "2026-04-23T00:00:00Z"
        })
    }

    #[test]
    fn deserialises_string_halves() {
        let v = kit_with_halves(json!("CIPHER_A"), json!("CIPHER_D"));
        let kit: RecoveryKit = serde_json::from_value(v).unwrap();
        assert_eq!(kit.encrypted_cube_seed, "CIPHER_A");
        assert_eq!(kit.encrypted_wallet_descriptor, "CIPHER_D");
    }

    #[test]
    fn deserialises_empty_halves() {
        // Current backend wire shape when one half isn't backed up.
        let v = kit_with_halves(json!("CIPHER_A"), json!(""));
        let kit: RecoveryKit = serde_json::from_value(v).unwrap();
        assert_eq!(kit.encrypted_cube_seed, "CIPHER_A");
        assert!(kit.encrypted_wallet_descriptor.is_empty());
    }

    #[test]
    fn deserialises_null_halves() {
        // Future-proofing: a server that serialises absent halves as
        // JSON null instead of "" must not break the client.
        let v = kit_with_halves(json!(null), json!(null));
        let kit: RecoveryKit = serde_json::from_value(v).unwrap();
        assert!(kit.encrypted_cube_seed.is_empty());
        assert!(kit.encrypted_wallet_descriptor.is_empty());
    }

    #[test]
    fn deserialises_missing_halves() {
        // Future-proofing: a server with `omitempty` on the response
        // (like `UpdateRecoveryKitRequest` already has on the request
        // side) would omit the field entirely. `#[serde(default)]`
        // handles that.
        let v = json!({
            "id": 1,
            "cubeId": 42,
            "encryptionScheme": "aes-256-gcm",
            "createdAt": "2026-04-23T00:00:00Z",
            "updatedAt": "2026-04-23T00:00:00Z"
        });
        let kit: RecoveryKit = serde_json::from_value(v).unwrap();
        assert!(kit.encrypted_cube_seed.is_empty());
        assert!(kit.encrypted_wallet_descriptor.is_empty());
    }
}

#[cfg(test)]
mod cube_has_vault_tests {
    //! The duress vault gate (PLAN-duress-vault-gate PR 3) reports and
    //! consumes a Cube's Vault presence over the wire as `hasVault`.
    use super::{vault_presence_report, CubeResponse, RegisterCubeRequest, UpdateCubeRequest};
    use serde_json::json;

    #[test]
    fn vault_presence_report_is_upgrade_only() {
        // This device holds the Vault and the server hasn't recorded it →
        // upgrade to true.
        assert_eq!(vault_presence_report(true, Some(false)), Some(true));
        assert_eq!(vault_presence_report(true, None), Some(true));

        // Server already shows the Vault → nothing to send.
        assert_eq!(vault_presence_report(true, Some(true)), None);

        // Reviewer's scenario: another device already reported the Vault
        // (server true), but this device's copy of the Cube has no local Vault.
        // Must NOT clobber the server's true — return None, never Some(false).
        assert_eq!(vault_presence_report(false, Some(true)), None);
        // And never assert false even when the server also shows false/absent.
        assert_eq!(vault_presence_report(false, Some(false)), None);
        assert_eq!(vault_presence_report(false, None), None);
    }

    #[test]
    fn register_request_reports_vault_upgrade_only() {
        // Device holds the Vault → assert `hasVault: true`.
        let with_vault = RegisterCubeRequest {
            uuid: "u1".to_string(),
            name: "Cube".to_string(),
            network: "mainnet".to_string(),
            has_vault: Some(true),
        };
        let v = serde_json::to_value(&with_vault).unwrap();
        assert_eq!(v["hasVault"], json!(true));
        assert_eq!(v["uuid"], json!("u1"));
        assert_eq!(v["network"], json!("mainnet"));

        // No local Vault → omit the field entirely (never assert `false`, which
        // would clobber a `true` reported by another device).
        let no_vault = RegisterCubeRequest {
            uuid: "u1".to_string(),
            name: "Cube".to_string(),
            network: "mainnet".to_string(),
            has_vault: None,
        };
        let v = serde_json::to_value(&no_vault).unwrap();
        assert!(v.get("hasVault").is_none());
    }

    #[test]
    fn update_request_omits_has_vault_when_none() {
        // Name-only rename must not touch server Vault presence.
        let rename = UpdateCubeRequest {
            name: Some("New".to_string()),
            status: None,
            has_vault: None,
        };
        let v = serde_json::to_value(&rename).unwrap();
        assert!(v.get("hasVault").is_none());

        // A vault-creation re-report carries the flag as `hasVault`.
        let report = UpdateCubeRequest {
            name: None,
            status: None,
            has_vault: Some(true),
        };
        let v = serde_json::to_value(&report).unwrap();
        assert_eq!(v["hasVault"], json!(true));
    }

    #[test]
    fn response_has_vault_defaults_to_none_when_absent() {
        // Older API without the field → None → Unknown for other-device Cubes.
        let v = json!({
            "id": 1,
            "uuid": "u1",
            "name": "Cube",
            "network": "mainnet",
            "status": "active"
        });
        let resp: CubeResponse = serde_json::from_value(v).unwrap();
        assert_eq!(resp.has_vault, None);

        // Present field parses through.
        let v = json!({
            "id": 1,
            "uuid": "u1",
            "name": "Cube",
            "network": "mainnet",
            "status": "active",
            "hasVault": true
        });
        let resp: CubeResponse = serde_json::from_value(v).unwrap();
        assert_eq!(resp.has_vault, Some(true));
    }
}

#[cfg(test)]
mod features_response_duress_tests {
    //! `duressEnabled` transport (PLAN-feature-flags PR 1). Mirrors the
    //! `liquidEnabled`/`marketplaceEnabled` shape: absent → `None` (treated
    //! false, fail-closed), and both the camelCase wire key and the snake_case
    //! alias parse to the same field.
    use super::FeaturesResponse;
    use serde_json::json;

    fn features_json(extra: serde_json::Value) -> serde_json::Value {
        let mut base = json!({ "plans": [] });
        if let (Some(obj), Some(add)) = (base.as_object_mut(), extra.as_object()) {
            for (k, v) in add {
                obj.insert(k.clone(), v.clone());
            }
        }
        base
    }

    #[test]
    fn duress_enabled_absent_is_none() {
        // Older backend / field omitted → None, which the gate reads as off.
        let resp: FeaturesResponse = serde_json::from_value(features_json(json!({}))).unwrap();
        assert_eq!(resp.duress_enabled, None);
    }

    #[test]
    fn duress_enabled_true_and_false_parse() {
        let on: FeaturesResponse =
            serde_json::from_value(features_json(json!({ "duressEnabled": true }))).unwrap();
        assert_eq!(on.duress_enabled, Some(true));

        let off: FeaturesResponse =
            serde_json::from_value(features_json(json!({ "duressEnabled": false }))).unwrap();
        assert_eq!(off.duress_enabled, Some(false));
    }

    #[test]
    fn duress_enabled_snake_case_alias_parses() {
        let resp: FeaturesResponse =
            serde_json::from_value(features_json(json!({ "duress_enabled": true }))).unwrap();
        assert_eq!(resp.duress_enabled, Some(true));
    }
}

#[cfg(test)]
mod locked_body_tests {
    //! The `423 Locked` bodies here are byte-for-byte what
    //! `responses.ErrorWithData` emits in coincube-api — code and message in
    //! `error`, timestamp in a sibling snake_case `data` object. Pinning the
    //! real shape is the point: the client previously read
    //! `error.availableAt` (wrong object, wrong casing), which parsed as
    //! `None` on every lock, so the restore screen said "Recovery kit download
    //! is delayed on new devices." with no hint of *how long* — and the UI code
    //! that formats "Available at …" was unreachable.
    use super::DownloadError;

    /// `enforceTrustedDeviceDelay` → `responses.ErrorWithData(..., map[string]
    /// string{"available_at": …})`.
    const TRUSTED_DEVICE_DELAY: &str = r#"{
        "data": { "available_at": "2026-08-09T14:00:00Z" },
        "success": false,
        "error": { "code": "TRUSTED_DEVICE_DELAY",
                   "message": "This device must wait before it can download recovery material" }
    }"#;

    /// `writeDuressLocked` → `map[string]string{"duress_unlock_at": …}`.
    const DURESS_LOCKED: &str = r#"{
        "data": { "duress_unlock_at": "2026-08-10T09:30:00Z" },
        "success": false,
        "error": { "code": "DURESS_LOCKED", "message": "Account is under duress lock" }
    }"#;

    #[test]
    fn trusted_device_delay_recovers_available_at() {
        match DownloadError::from_locked_body(TRUSTED_DEVICE_DELAY) {
            DownloadError::TrustedDeviceDelay {
                available_at: Some(at),
            } => assert_eq!(at.to_rfc3339(), "2026-08-09T14:00:00+00:00"),
            other => panic!("expected a dated TrustedDeviceDelay, got {:?}", other),
        }
    }

    #[test]
    fn duress_locked_recovers_unlock_at() {
        match DownloadError::from_locked_body(DURESS_LOCKED) {
            DownloadError::DuressLocked {
                unlock_at: Some(at),
            } => assert_eq!(at.to_rfc3339(), "2026-08-10T09:30:00+00:00"),
            other => panic!("expected a dated DuressLocked, got {:?}", other),
        }
    }

    #[test]
    fn a_lock_with_no_data_object_still_discriminates() {
        // `writeDuressLocked` omits `data` entirely when the unlock time is
        // zero, and a `null` data is equally legal JSON. Neither may turn the
        // typed variant back into an opaque error — we just lose the hint.
        for body in [
            r#"{"success":false,"error":{"code":"TRUSTED_DEVICE_DELAY","message":"x"}}"#,
            r#"{"data":null,"success":false,"error":{"code":"TRUSTED_DEVICE_DELAY","message":"x"}}"#,
        ] {
            assert!(
                matches!(
                    DownloadError::from_locked_body(body),
                    DownloadError::TrustedDeviceDelay { available_at: None }
                ),
                "body {} should stay a timeless TrustedDeviceDelay",
                body
            );
        }
    }

    #[test]
    fn an_error_object_timestamp_is_still_honoured() {
        // Back-compat with the shape this client always believed in, so a
        // server that moves the field into `error` doesn't regress the hint.
        match DownloadError::from_locked_body(
            r#"{"success":false,"error":{"code":"TRUSTED_DEVICE_DELAY","message":"x",
                "availableAt":"2026-08-09T14:00:00Z"}}"#,
        ) {
            DownloadError::TrustedDeviceDelay {
                available_at: Some(_),
            } => {}
            other => panic!("expected a dated TrustedDeviceDelay, got {:?}", other),
        }
    }

    #[test]
    fn an_opaque_body_fails_closed_to_duress() {
        // An unparseable 423 must never be read as the milder trusted-device
        // delay — the safe default is the duress lock.
        for body in ["not json", "{}", r#"{"error":{}}"#] {
            assert!(
                matches!(
                    DownloadError::from_locked_body(body),
                    DownloadError::DuressLocked { unlock_at: None }
                ),
                "body {:?} must fail closed to DuressLocked",
                body
            );
        }
    }
}

#[cfg(test)]
mod contact_role_tests {
    //! `ContactRole` is a wire type the backend can extend unilaterally, so
    //! its deserializer is lenient. Regression cover for the `"owner"` role,
    //! which the API writes on the invitee's side of every accepted invite
    //! and which used to abort the whole `GET /connect/contacts` response.
    use super::{Contact, ContactRole};
    use serde_json::json;

    fn contact_with_role(role: &str) -> serde_json::Value {
        json!({
            "id": 1,
            "role": role,
            "contactUser": { "id": 42, "email": "heir@example.com" },
            "createdAt": "2026-07-19T00:00:00Z",
        })
    }

    #[test]
    fn known_roles_round_trip() {
        for (wire, expected) in [
            ("keyholder", ContactRole::Keyholder),
            ("beneficiary", ContactRole::Beneficiary),
            ("observer", ContactRole::Observer),
            ("owner", ContactRole::Owner),
        ] {
            let got: ContactRole = serde_json::from_value(json!(wire)).unwrap();
            assert_eq!(got, expected, "wire value {wire:?}");
        }
    }

    #[test]
    fn owner_role_deserializes_on_a_contact_row() {
        // The exact shape that broke the keychain key picker: the reciprocal
        // contact the API creates for the person who invited us.
        let contact: Contact = serde_json::from_value(contact_with_role("owner")).unwrap();
        assert_eq!(contact.role, ContactRole::Owner);
        assert_eq!(contact.effective_contact_user_id(), Some(42));
    }

    #[test]
    fn unknown_role_does_not_abort_the_response() {
        // The regression that matters: one unrecognised role must degrade a
        // single row, not blank the entire contact list. Serde aborts the
        // whole array on a hard variant error, which is how a role addition
        // took out surfaces that never even read the role.
        let list: Vec<Contact> = serde_json::from_value(json!([
            contact_with_role("keyholder"),
            contact_with_role("executor"), // hypothetical future backend role
            contact_with_role("owner"),
        ]))
        .expect("an unknown role must not fail the whole list");

        assert_eq!(list.len(), 3);
        assert_eq!(list[0].role, ContactRole::Keyholder);
        assert_eq!(list[1].role, ContactRole::Unknown);
        assert_eq!(list[2].role, ContactRole::Owner);
    }

    #[test]
    fn role_matching_is_case_insensitive() {
        let got: ContactRole = serde_json::from_value(json!("Keyholder")).unwrap();
        assert_eq!(got, ContactRole::Keyholder);
    }

    #[test]
    fn keyholder_still_serializes_lowercase_for_the_invite_form() {
        // Send path is unchanged: the server validates this value
        // (invite.go:87) and only accepts the three invitable roles.
        let json = serde_json::to_string(&ContactRole::Keyholder).unwrap();
        assert_eq!(json, "\"keyholder\"");
    }
}
