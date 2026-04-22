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
    /// 429 from a rate-limited endpoint. `retry_after` is parsed from the
    /// `Retry-After` response header (seconds form); falls back to 60s
    /// when the header is missing or unparsable.
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
            CoincubeError::RateLimited { retry_after } => write!(
                f,
                "Rate limited — retry after {}s",
                retry_after.as_secs()
            ),
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

    /// When the error is a typed 429 rate-limit, returns the server's
    /// `Retry-After` duration (falling back to 60s when the header was
    /// missing). Callers can use this to delay a retry or to display a
    /// cooldown counter to the user.
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
    Legacy,
}

impl std::fmt::Display for PlanTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanTier::Free => write!(f, "Free"),
            PlanTier::Pro => write!(f, "Pro"),
            PlanTier::Legacy => write!(f, "Legacy"),
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

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanEntitlements {
    pub free_signing_key_count: i32,
    pub policy_editing: bool,
    pub legacy_invites: bool,
    pub linked_keychains: bool,
    pub duress_remote_lock: bool,
    pub business_orgs: bool,
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
    pub bolt12_offer: Option<String>,
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
    pub bolt12_offer: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CheckUsernameResponse {
    pub available: bool,
    pub username: String,
    /// Set when the API returns an error (e.g. reserved/invalid username)
    #[serde(default)]
    pub error_message: Option<String>,
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimLightningAddressRequest {
    pub username: String,
    pub bolt12_offer: String,
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

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryKit {
    pub id: u64,
    pub cube_id: u64,
    /// May be the empty string when the kit is descriptor-only (e.g. a
    /// passkey-seed cube backs up its wallet descriptor without the
    /// seed, which it cannot extract).
    pub encrypted_cube_seed: String,
    /// May be the empty string when the kit is seed-only (no Vault
    /// created yet, or the Vault wizard "skip" path).
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
