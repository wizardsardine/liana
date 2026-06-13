use serde::{Deserialize, Serialize};

pub mod client;
pub use client::CoincubeClient;

/// Matches `{"success":false,"error":{"code":"...","message":"..."}}` error bodies
/// returned by the coincube-api on non-2xx responses.
#[derive(Debug, Deserialize)]
struct ApiErrorBody {
    message: String,
}

#[derive(Debug, Deserialize)]
struct ApiErrorEnvelope {
    error: ApiErrorBody,
}

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
            CoincubeError::Unsuccessful(e) => {
                if let Ok(env) = serde_json::from_str::<ApiErrorEnvelope>(&e.text) {
                    write!(f, "{}", env.error.message)
                } else {
                    write!(f, "{}", e.text)
                }
            }
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

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanEntitlements {
    pub free_signing_key_count: i32,
    pub policy_editing: bool,
    pub legacy_invites: bool,
    pub linked_keychains: bool,
    pub duress_remote_lock: bool,
    pub business_orgs: bool,
    /// Estate-only: duress-activation alert contacts (SMS/WhatsApp/email
    /// fan-out when duress fires). See `PLAN-estate-notifications.md` PR 1.
    ///
    /// `#[serde(default)]` so a pre-estate-notifications API that doesn't
    /// emit this field deserialises to `false` rather than failing the
    /// whole `ConnectPlan` parse — the desktop treats an absent
    /// entitlement as "not entitled", which is the safe default.
    #[serde(default)]
    pub duress_alerts: bool,
    /// Estate-only: vault recovery-path monitoring (descriptor escrow or
    /// timelock heartbeat → keyholder emails). See
    /// `PLAN-estate-notifications.md` PR 2. Same forward-compat tolerance
    /// as [`Self::duress_alerts`].
    #[serde(default)]
    pub recovery_alerts: bool,
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

/// Request body for POST /api/v1/connect/cubes
#[derive(Debug, Clone, Serialize)]
pub struct RegisterCubeRequest {
    pub uuid: String,
    pub name: String,
    pub network: String,
}

/// Request body for PUT /api/v1/connect/cubes/{id}
#[derive(Debug, Clone, Serialize)]
pub struct UpdateCubeRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
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
    pub xpub: String,
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
}

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
    pub user_agent: Option<String>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContactRole {
    Keyholder,
    Beneficiary,
    Observer,
}

impl std::fmt::Display for ContactRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ContactRole::Keyholder => write!(f, "Keyholder"),
            ContactRole::Beneficiary => write!(f, "Beneficiary"),
            ContactRole::Observer => write!(f, "Observer"),
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
    pub xpub: String,
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

/// Error code string returned by the backend's W9 guard. Public so callers
/// can match on it when routing 409s.
pub const ERR_KEY_ALREADY_USED_IN_VAULT: &str = "KEY_ALREADY_USED_IN_VAULT";

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
    /// 401 — the session was rejected; bounce to login rather than hold the
    /// gate closed forever.
    Unauthorized,
}

impl DuressCheckOutcome {
    /// Classify a failed `get_duress_state` call. (Success is constructed
    /// directly as [`DuressCheckOutcome::Ok`].)
    pub fn from_err(e: &CoincubeError) -> Self {
        match e {
            CoincubeError::Parse(_) => Self::Incompatible,
            CoincubeError::Unsuccessful(info) if info.status_code == 401 => Self::Unauthorized,
            _ => Self::Unreachable,
        }
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
    /// Wrong password / malformed request (4xx other than 423).
    Invalid,
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
            DownloadError::Other(e) => write!(f, "{}", e),
        }
    }
}

impl std::error::Error for DownloadError {}

/// `423 Locked` body shape, used to discriminate `DURESS_LOCKED` from
/// `TRUSTED_DEVICE_DELAY`. Both timestamp fields are optional — the
/// duress case carries `unlock_at`, the trusted-device case `available_at`.
#[derive(Debug, Deserialize)]
struct DuressLockEnvelope {
    error: DuressLockBody,
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

impl DownloadError {
    /// Parses a `423 Locked` body into the discriminated variant. Falls back
    /// to `DuressLocked { unlock_at: None }` when the body can't be parsed —
    /// the safe default is to treat an opaque 423 as a duress lock rather than
    /// leak the kit.
    pub(crate) fn from_locked_body(body: &str) -> Self {
        match serde_json::from_str::<DuressLockEnvelope>(body) {
            Ok(env) if env.error.code == "TRUSTED_DEVICE_DELAY" => {
                DownloadError::TrustedDeviceDelay {
                    available_at: env.error.available_at,
                }
            }
            Ok(env) => DownloadError::DuressLocked {
                unlock_at: env.error.unlock_at,
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
pub const DURESS_CHANNEL_SMS: u8 = 1 << 0;
pub const DURESS_CHANNEL_WHATSAPP: u8 = 1 << 1;
pub const DURESS_CHANNEL_EMAIL: u8 = 1 << 2;

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
    pub channels: u8,
    /// RFC 3339 timestamp of when the one-time intro message was sent,
    /// or `None` if it hasn't gone out yet (just-created contact).
    #[serde(default)]
    pub intro_sent_at: Option<String>,
    /// RFC 3339 timestamp of when the contact replied STOP. When set, the
    /// contact is permanently opted out and never messaged again.
    #[serde(default)]
    pub opted_out_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl DuressAlertContact {
    /// True when the contact has replied STOP and will not be messaged.
    pub fn is_opted_out(&self) -> bool {
        self.opted_out_at.is_some()
    }

    pub fn has_channel(&self, bit: u8) -> bool {
        self.channels & bit != 0
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
    pub channels: u8,
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
    pub channels: Option<u8>,
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
        assert_eq!(DURESS_CHANNEL_SMS, 1);
        assert_eq!(DURESS_CHANNEL_WHATSAPP, 2);
        assert_eq!(DURESS_CHANNEL_EMAIL, 4);
        let c = DuressAlertContact {
            id: 1,
            display_name: "Jane".into(),
            phone: Some("+15551234567".into()),
            email: None,
            channels: DURESS_CHANNEL_SMS | DURESS_CHANNEL_WHATSAPP,
            intro_sent_at: None,
            opted_out_at: None,
            created_at: "2026-06-11T00:00:00Z".into(),
            updated_at: "2026-06-11T00:00:00Z".into(),
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
            "channels": 4,
            "createdAt": "2026-06-11T00:00:00Z",
            "updatedAt": "2026-06-11T00:00:00Z"
        });
        let c: DuressAlertContact = serde_json::from_value(v).unwrap();
        assert_eq!(c.display_name, "Sam");
        assert!(c.phone.is_none());
        assert_eq!(c.email.as_deref(), Some("sam@example.com"));
        assert!(c.has_channel(DURESS_CHANNEL_EMAIL));
        assert!(c.intro_sent_at.is_none());
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

/// Per-vault owner policy for when keyholders may download the encrypted
/// recovery kit. Wire values `anytime` / `at_approaching` match the
/// coincube-api `crk_keyholder_download` column (PR 3). Default is the
/// privacy-preserving `at_approaching`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum KeyholderDownloadPolicy {
    /// Keyholders can download anytime — lets family prepare/verify early,
    /// but with the pre-shared password that also means balance visibility.
    Anytime,
    /// Keyholders can only download once recovery is approaching/open —
    /// keeps balances private until the recovery window nears.
    #[default]
    AtApproaching,
}

impl KeyholderDownloadPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Anytime => "anytime",
            Self::AtApproaching => "at_approaching",
        }
    }
}

/// Status returned by `GET /api/v1/connect/vaults/{id}/monitoring`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultMonitoringStatus {
    #[serde(default)]
    pub level: VaultMonitoringLevel,
    #[serde(default)]
    pub crk_keyholder_download: KeyholderDownloadPolicy,
    /// Server's per-vault recovery state machine value, when the sweep has
    /// run: `none` / `approaching` / `available` / `reminding`. `None` when
    /// the API doesn't expose it (nice-to-have; the UI degrades silently).
    #[serde(default)]
    pub last_notified_state: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

impl Default for VaultMonitoringStatus {
    fn default() -> Self {
        Self {
            level: VaultMonitoringLevel::Off,
            crk_keyholder_download: KeyholderDownloadPolicy::AtApproaching,
            last_notified_state: None,
            updated_at: None,
        }
    }
}

/// Body for `POST /api/v1/connect/vaults/{id}/monitoring` (Estate-gated).
/// Sets the monitoring tier. `descriptor` is required for
/// [`VaultMonitoringLevel::Full`] (the escrowed copy) and omitted for
/// `Heartbeat`. `crk_keyholder_download` is included when the owner changes
/// the download policy alongside the level.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetVaultMonitoringRequest {
    pub level: VaultMonitoringLevel,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub descriptor: Option<String>,
    /// Gap-limit hint so the server's sweep derives enough addresses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gap_limit: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crk_keyholder_download: Option<KeyholderDownloadPolicy>,
}

/// Body for `PUT /api/v1/connect/vaults/{id}/keyholder-download-policy`
/// (Estate-gated). Sets the keyholder recovery-kit download policy
/// independently of the monitoring level — the policy governs the existing
/// recovery-kit GET for keyholder callers, so it's meaningful even when
/// chain monitoring is off.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetKeyholderDownloadPolicyRequest {
    pub crk_keyholder_download: KeyholderDownloadPolicy,
}

/// Body for `POST /api/v1/connect/vaults/{id}/heartbeat` (Estate-gated,
/// PR 5). Fire-and-forget after each vault sync for Heartbeat-tier (and
/// Full, as a cross-check) vaults. `earliest_recovery_height` is the block
/// height at which this vault's earliest recovery branch opens; a newer
/// report always wins server-side (monotonic-staleness rule).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultHeartbeatRequest {
    pub earliest_recovery_height: u32,
    pub computed_at: chrono::DateTime<chrono::Utc>,
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
    fn download_policy_wire_values() {
        assert_eq!(
            serde_json::to_string(&KeyholderDownloadPolicy::AtApproaching).unwrap(),
            "\"at_approaching\""
        );
        assert_eq!(
            serde_json::to_string(&KeyholderDownloadPolicy::Anytime).unwrap(),
            "\"anytime\""
        );
        // Default is the privacy-preserving option.
        assert_eq!(
            KeyholderDownloadPolicy::default(),
            KeyholderDownloadPolicy::AtApproaching
        );
    }

    #[test]
    fn monitoring_status_tolerates_minimal_body() {
        // A vault with no monitoring record: server may send just the level.
        let v = serde_json::json!({ "level": "off" });
        let s: VaultMonitoringStatus = serde_json::from_value(v).unwrap();
        assert_eq!(s.level, VaultMonitoringLevel::Off);
        // Absent download policy defaults to at_approaching.
        assert_eq!(
            s.crk_keyholder_download,
            KeyholderDownloadPolicy::AtApproaching
        );
        assert!(s.last_notified_state.is_none());
    }

    #[test]
    fn set_request_omits_descriptor_for_heartbeat() {
        let req = SetVaultMonitoringRequest {
            level: VaultMonitoringLevel::Heartbeat,
            descriptor: None,
            gap_limit: Some(20),
            crk_keyholder_download: None,
        };
        let body = serde_json::to_value(&req).unwrap();
        assert_eq!(body["level"], "heartbeat");
        assert!(body.get("descriptor").is_none());
        assert_eq!(body["gapLimit"], 20);
        assert!(body.get("crkKeyholderDownload").is_none());
    }

    #[test]
    fn set_request_includes_descriptor_for_full() {
        let req = SetVaultMonitoringRequest {
            level: VaultMonitoringLevel::Full,
            descriptor: Some("wsh(...)".into()),
            gap_limit: None,
            crk_keyholder_download: Some(KeyholderDownloadPolicy::Anytime),
        };
        let body = serde_json::to_value(&req).unwrap();
        assert_eq!(body["level"], "full");
        assert_eq!(body["descriptor"], "wsh(...)");
        assert_eq!(body["crkKeyholderDownload"], "anytime");
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
