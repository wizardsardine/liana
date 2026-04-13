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
    pub email_verified: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Contact {
    pub id: u64,
    pub user_id: u64,
    pub contact_user_id: u64,
    pub invite_id: Option<u64>,
    pub role: ContactRole,
    pub contact_user: ContactUser,
    pub created_at: String,
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
pub struct CreateInviteRequest {
    pub email: String,
    pub role: ContactRole,
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
