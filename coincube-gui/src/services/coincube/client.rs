use super::{
    get_countries, AddVaultMemberRequest, ApiErrorResponse, ApiResponse, AvatarGenerateData,
    AvatarGenerateRequest, AvatarSelectData, AvatarSelectRequest, BillingHistoryEntry,
    ChargeStatusResponse, CheckUsernameResponse, CheckoutRequest, CheckoutResponse,
    ClaimLightningAddressRequest, CoincubeError, ConnectPlan, ConnectVaultResponse, Contact,
    ContactCube, Country, CreateConnectVaultRequest, CreateInviteRequest, CubeInviteOrAddResult,
    CubeKeyRaw, CubeLimitsResponse, CubeResponse, DownloadStats, FeaturesResponse, GetAvatarData,
    Invite, LightningAddress, LoginActivity, LoginResponse, OtpRequest, OtpVerifyRequest,
    PublicAvatarData, RecoveryKit, RecoveryKitStatus, RefreshTokenRequest, RegenerationData,
    RegisterCubeRequest, SaveQuoteRequest, SaveQuoteResponse, StatsPeriod, TimeseriesResponse,
    TodayStats, UpdateCubeRequest, UpsertRecoveryKitRequest, User, VaultMemberResponse,
    VerifiedDevice,
};
use reqwest::{Client, Method};
use serde::Deserialize;
use std::time::Duration;

use crate::services::http::ResponseExt;

#[cfg(not(debug_assertions))]
const _: () = {
    if option_env!("COINCUBE_API_URL").is_none() {
        panic!("COINCUBE_API_URL must be set at build time for release builds");
    }
};

#[derive(Debug, Clone)]
pub struct CoincubeClient {
    pub client: Client,
    pub base_url: String,
    token: Option<String>,
}

impl Default for CoincubeClient {
    fn default() -> Self {
        Self::new()
    }
}

impl CoincubeClient {
    pub fn new() -> Self {
        let base_url = crate::services::coincube_api_base_url();

        log::info!(
            "Coincube Base URL: {}, Release = {}",
            base_url,
            cfg!(not(debug_assertions))
        );

        let https_only = !base_url.starts_with("http://");

        Self {
            client: reqwest::ClientBuilder::new()
                .timeout(std::time::Duration::from_secs(20))
                .https_only(https_only)
                .build()
                .unwrap(),
            base_url,
            token: None,
        }
    }

    /// A JWT is needed for some authenticated endpoints, acquired after a user successfully logs in
    pub fn set_token(&mut self, token: &str) {
        self.token = Some(token.to_string());

        let mut headers = reqwest::header::HeaderMap::new();
        headers.append(
            "Authorization",
            reqwest::header::HeaderValue::from_str(&format!("Bearer {}", token)).unwrap(),
        );

        let https_only = !self.base_url.starts_with("http://");
        self.client = reqwest::ClientBuilder::new()
            .timeout(std::time::Duration::from_secs(20))
            .https_only(https_only)
            .default_headers(headers)
            .build()
            .unwrap();
    }

    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    /// Save a Mavapay quote to coincube-api
    pub async fn save_quote<T: serde::Serialize>(
        &self,
        quote_id: &str,
        quote: T,
    ) -> Result<SaveQuoteResponse, CoincubeError> {
        let url = format!("{}/api/v1/mavapay/quotes", self.base_url);
        let request = SaveQuoteRequest { quote_id, quote };

        let response = self
            .client
            .request(Method::POST, &url)
            .json(&request)
            .send()
            .await?;

        let response = response.check_success().await?;
        Ok(response.json().await?)
    }
}

impl CoincubeClient {
    pub async fn refresh_login(&self, refresh_token: &str) -> Result<LoginResponse, CoincubeError> {
        let request = RefreshTokenRequest { refresh_token };

        let response = {
            let url = format!("{}{}", self.base_url, "/api/v1/auth/token/refresh");
            self.client.post(&url).json(&request).send()
        }
        .await?;
        let response = response.check_success().await?;

        Ok(response.json().await?)
    }

    pub async fn login_send_otp(&self, request: OtpRequest) -> Result<(), CoincubeError> {
        let response = {
            let url = format!("{}{}", self.base_url, "/api/v1/auth/login/request-otp");
            self.client.post(&url).json(&request).send()
        }
        .await?;
        response.check_success().await?;

        Ok(())
    }

    pub async fn login_verify_otp(
        &self,
        request: OtpVerifyRequest,
    ) -> Result<LoginResponse, CoincubeError> {
        let response = {
            let url = format!("{}{}", self.base_url, "/api/v1/auth/login/verify-otp");
            self.client.post(&url).json(&request).send()
        }
        .await?;
        let response = response.check_success().await?;

        Ok(response.json().await?)
    }

    pub async fn signup_send_otp(&self, request: OtpRequest) -> Result<(), CoincubeError> {
        let response = {
            let url = format!("{}{}", self.base_url, "/api/v1/auth/signup/request-otp");
            self.client.post(&url).json(&request).send()
        }
        .await?;
        response.check_success().await?;

        Ok(())
    }

    pub async fn signup_verify_otp(
        &self,
        request: OtpVerifyRequest,
    ) -> Result<LoginResponse, CoincubeError> {
        let response = {
            let url = format!("{}{}", self.base_url, "/api/v1/auth/signup/verify-otp");
            self.client.post(&url).json(&request).send()
        }
        .await?;
        let response = response.check_success().await?;

        Ok(response.json().await?)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CountryResponse {
    iso_code: String,
}

impl CoincubeClient {
    pub async fn fetch_download_stats(&self) -> Result<DownloadStats, CoincubeError> {
        let url = format!("{}/api/v1/downloads", self.base_url);
        let res = self.client.get(&url).send().await?;
        let res = res.check_success().await?;
        Ok(res.json().await?)
    }

    pub async fn fetch_today_stats(&self) -> Result<TodayStats, CoincubeError> {
        let url = format!("{}/api/v1/downloads/today", self.base_url);
        let res = self.client.get(&url).send().await?;
        let res = res.check_success().await?;
        Ok(res.json().await?)
    }

    pub async fn fetch_timeseries(
        &self,
        period: StatsPeriod,
    ) -> Result<TimeseriesResponse, CoincubeError> {
        let url = format!(
            "{}/api/v1/downloads/timeseries?period={}",
            self.base_url,
            period.as_str()
        );
        let res = self.client.get(&url).send().await?;
        let res = res.check_success().await?;
        Ok(res.json().await?)
    }
}

impl CoincubeClient {
    pub async fn get_user(&self) -> Result<User, CoincubeError> {
        let url = format!("{}/api/v1/user", self.base_url);
        let res = self.client.get(&url).send().await?;
        let res = res.check_success().await?;
        Ok(res.json().await?)
    }

    pub async fn get_connect_plan(&self) -> Result<ConnectPlan, CoincubeError> {
        let url = format!("{}/api/v1/connect/plan", self.base_url);
        let res = self.client.get(&url).send().await?;
        let res = res.check_success().await?;
        let resp: ApiResponse<ConnectPlan> = res.json().await?;
        Ok(resp.data)
    }

    /// GET /api/v1/connect/features (public — no auth required)
    pub async fn get_connect_features(&self) -> Result<FeaturesResponse, CoincubeError> {
        let url = format!("{}/api/v1/connect/features", self.base_url);
        let res = self.client.get(&url).send().await?;
        let res = res.check_success().await?;
        let resp: ApiResponse<FeaturesResponse> = res.json().await?;
        Ok(resp.data)
    }

    /// POST /api/v1/connect/checkout (authenticated)
    pub async fn create_checkout(
        &self,
        req: CheckoutRequest,
    ) -> Result<CheckoutResponse, CoincubeError> {
        let url = format!("{}/api/v1/connect/checkout", self.base_url);
        let res = self.client.post(&url).json(&req).send().await?;
        let res = res.check_success().await?;
        let resp: ApiResponse<CheckoutResponse> = res.json().await?;
        Ok(resp.data)
    }

    /// GET /api/v1/connect/checkout/{chargeId} (authenticated)
    pub async fn get_charge_status(
        &self,
        charge_id: &str,
    ) -> Result<ChargeStatusResponse, CoincubeError> {
        let url = format!("{}/api/v1/connect/checkout/{}", self.base_url, charge_id);
        let res = self.client.get(&url).send().await?;
        let res = res.check_success().await?;
        let resp: ApiResponse<ChargeStatusResponse> = res.json().await?;
        Ok(resp.data)
    }

    /// GET /api/v1/connect/billing/history (authenticated)
    pub async fn get_billing_history(&self) -> Result<Vec<BillingHistoryEntry>, CoincubeError> {
        let url = format!("{}/api/v1/connect/billing/history", self.base_url);
        let res = self.client.get(&url).send().await?;
        let res = res.check_success().await?;
        let resp: ApiResponse<Vec<BillingHistoryEntry>> = res.json().await?;
        Ok(resp.data)
    }

    pub async fn get_verified_devices(&self) -> Result<Vec<VerifiedDevice>, CoincubeError> {
        let url = format!("{}/api/v1/verified-device/", self.base_url);
        let res = self.client.get(&url).send().await?;
        let res = res.check_success().await?;
        Ok(res.json().await?)
    }

    pub async fn get_login_activity(&self) -> Result<Vec<LoginActivity>, CoincubeError> {
        let url = format!("{}/api/v1/login-activity/", self.base_url);
        let res = self.client.get(&url).send().await?;
        let res = res.check_success().await?;
        Ok(res.json().await?)
    }

    /// POST /api/v1/connect/cubes — register (or retrieve) a cube on the backend.
    /// Idempotent on UUID: returns the existing cube if same user+UUID.
    pub async fn register_cube(
        &self,
        req: RegisterCubeRequest,
    ) -> Result<CubeResponse, CoincubeError> {
        let url = format!("{}/api/v1/connect/cubes", self.base_url);
        let res = self.client.post(&url).json(&req).send().await?;
        let res = res.check_success().await?;
        let resp: ApiResponse<CubeResponse> = res.json().await?;
        Ok(resp.data)
    }

    /// GET /api/v1/connect/cubes — list all cubes for the authenticated user.
    pub async fn list_cubes(&self) -> Result<Vec<CubeResponse>, CoincubeError> {
        let url = format!("{}/api/v1/connect/cubes", self.base_url);
        let res = self.client.get(&url).send().await?;
        let res = res.check_success().await?;
        let resp: ApiResponse<Vec<CubeResponse>> = res.json().await?;
        Ok(resp.data)
    }

    /// PUT /api/v1/connect/cubes/{cubeId} — update a cube's name or status.
    pub async fn update_cube(
        &self,
        cube_id: &str,
        req: UpdateCubeRequest,
    ) -> Result<CubeResponse, CoincubeError> {
        let url = format!("{}/api/v1/connect/cubes/{}", self.base_url, cube_id);
        let res = self.client.put(&url).json(&req).send().await?;
        let res = res.check_success().await?;
        let resp: ApiResponse<CubeResponse> = res.json().await?;
        Ok(resp.data)
    }

    /// DELETE /api/v1/connect/cubes/{cubeId} — delete a cube.
    pub async fn delete_cube(&self, cube_id: &str) -> Result<(), CoincubeError> {
        let url = format!("{}/api/v1/connect/cubes/{}", self.base_url, cube_id);
        let res = self.client.delete(&url).send().await?;
        res.check_success().await?;
        Ok(())
    }

    /// GET /api/v1/connect/cubes/limits?network={network} — get cube limits for the authenticated user.
    pub async fn get_cube_limits(
        &self,
        network: &str,
    ) -> Result<CubeLimitsResponse, CoincubeError> {
        let url = format!("{}/api/v1/connect/cubes/limits", self.base_url);
        let res = self
            .client
            .get(&url)
            .query(&[("network", network)])
            .send()
            .await?;
        let res = res.check_success().await?;
        let resp: ApiResponse<CubeLimitsResponse> = res.json().await?;
        Ok(resp.data)
    }

    /// Test-only constructor that points the client at an arbitrary base URL
    /// (typically an `httpmock` MockServer). Skips `https_only` so `http://`
    /// loopback URLs are accepted.
    #[cfg(test)]
    pub fn for_test(base_url: impl Into<String>) -> Self {
        Self {
            client: reqwest::ClientBuilder::new()
                .timeout(std::time::Duration::from_secs(5))
                .https_only(false)
                .build()
                .unwrap(),
            base_url: base_url.into(),
            token: None,
        }
    }

    // --- Cube-scoped endpoints (Lightning Address, Avatar) ---
    // All use /connect/cubes/{cubeId}/... paths (server-side numeric ID)

    pub async fn get_lightning_address(
        &self,
        cube_id: &str,
    ) -> Result<LightningAddress, CoincubeError> {
        let url = format!(
            "{}/api/v1/connect/cubes/{}/lightning-address",
            self.base_url, cube_id
        );
        let res = self.client.get(&url).send().await?;
        let res = res.check_success().await?;
        let resp: ApiResponse<LightningAddress> = res.json().await?;
        Ok(resp.data)
    }

    pub async fn check_lightning_address(
        &self,
        cube_id: &str,
        username: &str,
    ) -> Result<CheckUsernameResponse, CoincubeError> {
        let url = format!(
            "{}/api/v1/connect/cubes/{}/lightning-address/check",
            self.base_url, cube_id
        );
        let res = self
            .client
            .get(&url)
            .query(&[("username", username)])
            .send()
            .await?;
        let status = res.status();
        let body = res.text().await.map_err(CoincubeError::Network)?;

        if status.is_success() {
            let resp: ApiResponse<CheckUsernameResponse> = serde_json::from_str(&body)?;
            Ok(resp.data)
        } else if status.is_client_error() && !matches!(status.as_u16(), 401 | 403) {
            // Validation errors (400, 409, 422, etc.) — treat as "not available"
            if let Ok(err_resp) = serde_json::from_str::<ApiErrorResponse>(&body) {
                Ok(CheckUsernameResponse {
                    available: false,
                    username: username.to_string(),
                    error_message: Some(err_resp.error.message),
                })
            } else {
                Err(CoincubeError::Api(body))
            }
        } else {
            // Auth errors (401/403), server errors (5xx), etc.
            Err(CoincubeError::Api(body))
        }
    }

    pub async fn claim_lightning_address(
        &self,
        cube_id: &str,
        req: ClaimLightningAddressRequest,
    ) -> Result<LightningAddress, CoincubeError> {
        let url = format!(
            "{}/api/v1/connect/cubes/{}/lightning-address",
            self.base_url, cube_id
        );
        let res = self.client.post(&url).json(&req).send().await?;
        let res = res.check_success().await?;
        let resp: ApiResponse<LightningAddress> = res.json().await?;
        Ok(resp.data)
    }
}

impl CoincubeClient {
    /// Builds an Authorization HeaderMap from the stored token.
    fn auth_headers(&self) -> reqwest::header::HeaderMap {
        let mut map = reqwest::header::HeaderMap::new();
        if let Some(ref t) = self.token {
            if let Ok(val) = reqwest::header::HeaderValue::from_str(&format!("Bearer {}", t)) {
                map.insert("Authorization", val);
            }
        }
        map
    }

    /// GET /api/v1/connect/cubes/{cubeId}/avatar
    pub async fn get_avatar(&self, cube_id: &str) -> Result<GetAvatarData, CoincubeError> {
        let url = format!("{}/api/v1/connect/cubes/{}/avatar", self.base_url, cube_id);
        let res = self.client.get(&url).send().await?;
        let res = res.check_success().await?;
        let resp: ApiResponse<GetAvatarData> = res.json().await?;
        Ok(resp.data)
    }

    /// POST /api/v1/connect/cubes/{cubeId}/avatar/generate
    pub async fn post_avatar_generate(
        &self,
        cube_id: &str,
        req: AvatarGenerateRequest,
    ) -> Result<AvatarGenerateData, CoincubeError> {
        let slow_client = reqwest::ClientBuilder::new()
            .timeout(std::time::Duration::from_secs(120))
            .https_only(!self.base_url.starts_with("http://"))
            .default_headers(self.auth_headers())
            .build()
            .map_err(CoincubeError::Network)?;

        let url = format!(
            "{}/api/v1/connect/cubes/{}/avatar/generate",
            self.base_url, cube_id
        );
        let res = slow_client.post(&url).json(&req).send().await?;
        let res = res.check_success().await?;
        let resp: ApiResponse<AvatarGenerateData> = res.json().await?;
        Ok(resp.data)
    }

    /// POST /api/v1/connect/cubes/{cubeId}/avatar/select
    pub async fn post_avatar_select(
        &self,
        cube_id: &str,
        req: AvatarSelectRequest,
    ) -> Result<AvatarSelectData, CoincubeError> {
        let url = format!(
            "{}/api/v1/connect/cubes/{}/avatar/select",
            self.base_url, cube_id
        );
        let res = self.client.post(&url).json(&req).send().await?;
        let res = res.check_success().await?;
        let resp: ApiResponse<AvatarSelectData> = res.json().await?;
        Ok(resp.data)
    }

    /// GET /api/v1/connect/cubes/{cubeId}/avatar/regenerations
    pub async fn get_avatar_regenerations(
        &self,
        cube_id: &str,
    ) -> Result<RegenerationData, CoincubeError> {
        let url = format!(
            "{}/api/v1/connect/cubes/{}/avatar/regenerations",
            self.base_url, cube_id
        );
        let res = self.client.get(&url).send().await?;
        let res = res.check_success().await?;
        let resp: ApiResponse<RegenerationData> = res.json().await?;
        Ok(resp.data)
    }

    /// GET /api/v1/connect/avatar/public/{lightning_address} (public, no cube scope)
    pub async fn get_public_avatar(
        &self,
        lightning_address: &str,
    ) -> Result<PublicAvatarData, CoincubeError> {
        let url = format!(
            "{}/api/v1/connect/avatar/public/{}",
            self.base_url, lightning_address
        );
        let res = self.client.get(&url).send().await?;
        let res = res.check_success().await?;
        let resp: ApiResponse<PublicAvatarData> = res.json().await?;
        Ok(resp.data)
    }

    /// GET /api/v1/connect/avatar/image/{id} (public, no cube scope)
    pub async fn fetch_avatar_image(&self, variant_id: u64) -> Result<Vec<u8>, CoincubeError> {
        let url = format!(
            "{}/api/v1/connect/avatar/image/{}",
            self.base_url, variant_id
        );
        let res = self.client.get(&url).send().await?;
        let res = res.check_success().await?;
        Ok(res.bytes().await.map_err(CoincubeError::Network)?.to_vec())
    }
}

impl CoincubeClient {
    /// GET /api/v1/config/sideshift — fetch the SideShift affiliate ID.
    pub async fn get_sideshift_affiliate_id(&self) -> Result<String, String> {
        let url = format!("{}/api/v1/config/sideshift", self.base_url);
        let res = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| e.to_string())?
            .check_success()
            .await
            .map_err(|e| format!("HTTP {}: {}", e.status_code, e.text))?;
        let config: crate::services::sideshift::SideshiftConfig =
            res.json().await.map_err(|e| e.to_string())?;
        Ok(config.affiliate_id)
    }
}

impl CoincubeClient {
    /// Detects the user's country and returns (country_name, iso_code)
    pub async fn locate(&self) -> Result<&'static Country, CoincubeError> {
        // allow users (and developers) to override detected ISO_CODE
        let iso_code = match std::env::var("FORCE_ISOCODE") {
            Ok(iso) => iso,
            Err(_) => {
                let url = format!("{}/api/v1/geolocation/country", self.base_url);

                let res = self.client.get(url).send().await?;
                let res = res.check_success().await?;

                let detected: CountryResponse = res.json().await?;
                detected.iso_code
            }
        };

        match get_countries().iter().find(|c| c.code == iso_code) {
            Some(country) => Ok(country),
            None => Err(CoincubeError::Api(format!(
                "Unknown country iso code: ({})",
                iso_code
            ))),
        }
    }
}

// =============================================================================
// Contacts & Invites
// =============================================================================

impl CoincubeClient {
    /// GET /api/v1/connect/contacts
    pub async fn get_contacts(&self) -> Result<Vec<Contact>, CoincubeError> {
        let url = format!("{}/api/v1/connect/contacts", self.base_url);
        let res = self.client.get(&url).send().await?;
        let res = res.check_success().await?;
        let resp: ApiResponse<Vec<Contact>> = res.json().await?;
        Ok(resp.data)
    }

    /// GET /api/v1/connect/cubes/{cubeUuid}/keys — retrieve Keychain keys
    /// attached to a Cube.  Returns a flat array of keys; owner resolution
    /// (self vs. contact) is done client-side.
    pub async fn get_cube_keys(&self, cube_uuid: &str) -> Result<Vec<CubeKeyRaw>, CoincubeError> {
        let url = format!("{}/api/v1/connect/cubes/{}/keys", self.base_url, cube_uuid);
        let res = self.client.get(&url).send().await?;
        let res = res.check_success().await?;
        let resp: ApiResponse<Vec<CubeKeyRaw>> = res.json().await?;
        Ok(resp.data)
    }

    /// GET /api/v1/connect/invites
    pub async fn get_invites(&self) -> Result<Vec<Invite>, CoincubeError> {
        let url = format!("{}/api/v1/connect/invites", self.base_url);
        let res = self.client.get(&url).send().await?;
        let res = res.check_success().await?;
        let resp: ApiResponse<Vec<Invite>> = res.json().await?;
        Ok(resp.data)
    }

    /// POST /api/v1/connect/invites
    pub async fn create_invite(&self, req: CreateInviteRequest) -> Result<(), CoincubeError> {
        let url = format!("{}/api/v1/connect/invites", self.base_url);
        let res = self.client.post(&url).json(&req).send().await?;
        res.check_success().await?;
        Ok(())
    }

    /// POST /api/v1/connect/invites/{id}/resend
    pub async fn resend_invite(&self, invite_id: u64) -> Result<(), CoincubeError> {
        let url = format!(
            "{}/api/v1/connect/invites/{}/resend",
            self.base_url, invite_id
        );
        let res = self
            .client
            .post(&url)
            .json(&serde_json::json!({}))
            .send()
            .await?;
        res.check_success().await?;
        Ok(())
    }

    /// POST /api/v1/connect/invites/{id}/revoke
    pub async fn revoke_invite(&self, invite_id: u64) -> Result<(), CoincubeError> {
        let url = format!(
            "{}/api/v1/connect/invites/{}/revoke",
            self.base_url, invite_id
        );
        let res = self
            .client
            .post(&url)
            .json(&serde_json::json!({}))
            .send()
            .await?;
        res.check_success().await?;
        Ok(())
    }

    /// GET /api/v1/connect/cubes/by-contact/{contactId}
    pub async fn get_cubes_by_contact(
        &self,
        contact_id: u64,
    ) -> Result<Vec<ContactCube>, CoincubeError> {
        let url = format!(
            "{}/api/v1/connect/cubes/by-contact/{}",
            self.base_url, contact_id
        );
        let res = self.client.get(&url).send().await?;
        let res = res.check_success().await?;
        let resp: ApiResponse<Vec<ContactCube>> = res.json().await?;
        Ok(resp.data)
    }
}

// =============================================================================
// Cube members & cube-scoped invites (W8)
// =============================================================================

impl CoincubeClient {
    /// GET /api/v1/connect/cubes/{cubeId} — fetch a single cube, including
    /// members and pending invites.
    pub async fn get_cube(&self, cube_id: u64) -> Result<CubeResponse, CoincubeError> {
        let url = format!("{}/api/v1/connect/cubes/{}", self.base_url, cube_id);
        let res = self.client.get(&url).send().await?;
        let res = res.check_success().await?;
        let resp: ApiResponse<CubeResponse> = res.json().await?;
        Ok(resp.data)
    }

    /// POST /api/v1/connect/cubes/{cubeId}/invites — smart invite. If `email`
    /// matches an existing contact the user is added as a member immediately
    /// (`Added`); otherwise an invite is created and a pending-attachment row
    /// is recorded against this cube (`Invited`).
    pub async fn create_cube_invite(
        &self,
        cube_id: u64,
        email: &str,
    ) -> Result<CubeInviteOrAddResult, CoincubeError> {
        let url = format!("{}/api/v1/connect/cubes/{}/invites", self.base_url, cube_id);
        let body = serde_json::json!({ "email": email });
        let res = self.client.post(&url).json(&body).send().await?;
        let res = res.check_success().await?;
        let resp: ApiResponse<CubeInviteOrAddResult> = res.json().await?;
        Ok(resp.data)
    }

    /// DELETE /api/v1/connect/cubes/{cubeId}/invites/{inviteId} — revoke a
    /// pending cube-scoped invite.
    pub async fn revoke_cube_invite(
        &self,
        cube_id: u64,
        invite_id: u64,
    ) -> Result<(), CoincubeError> {
        let url = format!(
            "{}/api/v1/connect/cubes/{}/invites/{}",
            self.base_url, cube_id, invite_id
        );
        let res = self.client.delete(&url).send().await?;
        res.check_success().await?;
        Ok(())
    }

    /// DELETE /api/v1/connect/cubes/{cubeId}/members/{memberId} — remove a
    /// cube member. Fails 409 if the member's keys are in an active Vault
    /// (stranded-vault guard, W4).
    pub async fn remove_cube_member(
        &self,
        cube_id: u64,
        member_id: u64,
    ) -> Result<(), CoincubeError> {
        let url = format!(
            "{}/api/v1/connect/cubes/{}/members/{}",
            self.base_url, cube_id, member_id
        );
        let res = self.client.delete(&url).send().await?;
        res.check_success().await?;
        Ok(())
    }
}

// =============================================================================
// Connect Vault lifecycle
// =============================================================================
//
// The Vault Builder (installer/step/descriptor/editor) uses these to
// create the server-side `ConnectVault` shell after the local descriptor
// is persisted. See `plans/PLAN-cube-membership-desktop.md` §2.6 for the
// W9 race-to-409 fallback flow.

impl CoincubeClient {
    /// POST /api/v1/connect/cubes/{cubeId}/vault — create the vault shell
    /// for a cube. Owner-only. Returns the (empty-members) vault.
    pub async fn create_connect_vault(
        &self,
        cube_id: u64,
        req: CreateConnectVaultRequest,
    ) -> Result<ConnectVaultResponse, CoincubeError> {
        let url = format!("{}/api/v1/connect/cubes/{}/vault", self.base_url, cube_id);
        let res = self.client.post(&url).json(&req).send().await?;
        let res = res.check_success().await?;
        let resp: ApiResponse<ConnectVaultResponse> = res.json().await?;
        Ok(resp.data)
    }

    /// GET /api/v1/connect/cubes/{cubeId}/vault — fetch the existing vault
    /// (404s if none exists).
    pub async fn get_connect_vault(
        &self,
        cube_id: u64,
    ) -> Result<ConnectVaultResponse, CoincubeError> {
        let url = format!("{}/api/v1/connect/cubes/{}/vault", self.base_url, cube_id);
        let res = self.client.get(&url).send().await?;
        let res = res.check_success().await?;
        let resp: ApiResponse<ConnectVaultResponse> = res.json().await?;
        Ok(resp.data)
    }

    /// DELETE /api/v1/connect/cubes/{cubeId}/vault — tear down the vault.
    /// Useful as a rollback when `add_vault_member` fails partway through
    /// a Vault Builder finalisation.
    pub async fn delete_connect_vault(&self, cube_id: u64) -> Result<(), CoincubeError> {
        let url = format!("{}/api/v1/connect/cubes/{}/vault", self.base_url, cube_id);
        let res = self.client.delete(&url).send().await?;
        res.check_success().await?;
        Ok(())
    }

    /// POST /api/v1/connect/cubes/{cubeId}/vault/members — attach a member
    /// (contact + key, contact-only, or key-only) to a vault.
    ///
    /// Fails 409 with `KEY_ALREADY_USED_IN_VAULT` when the supplied `keyId`
    /// is already referenced by any vault (W9). Callers should check
    /// `CoincubeError::is_key_already_used_in_vault()` and surface the
    /// conflict dialog.
    ///
    /// W16-desktop: a 409 with code `VAULT_KEYHOLDER_LOCKED` (from
    /// adding `role=keyholder` to an `active` Vault) is reclassified
    /// into `CoincubeError::VaultKeyholderLocked { vault_id }` before
    /// returning — callers pattern-match on the variant rather than
    /// re-parsing the error body.
    pub async fn add_vault_member(
        &self,
        cube_id: u64,
        req: AddVaultMemberRequest,
    ) -> Result<VaultMemberResponse, CoincubeError> {
        let url = format!(
            "{}/api/v1/connect/cubes/{}/vault/members",
            self.base_url, cube_id
        );
        let res = self.client.post(&url).json(&req).send().await?;
        let res = res.check_success().await.map_err(|e| {
            // Reclassify the W16 409 at the boundary so downstream
            // handlers can `match` on a typed variant.
            if let Some(vault_id) = super::vault_keyholder_locked_vault_id(&e) {
                return CoincubeError::VaultKeyholderLocked { vault_id };
            }
            CoincubeError::from(e)
        })?;
        let resp: ApiResponse<VaultMemberResponse> = res.json().await?;
        Ok(resp.data)
    }

    /// DELETE /api/v1/connect/cubes/{cubeId}/vault/members/{memberId} —
    /// remove a vault member. Used by rollback paths.
    pub async fn remove_vault_member(
        &self,
        cube_id: u64,
        member_id: u64,
    ) -> Result<(), CoincubeError> {
        let url = format!(
            "{}/api/v1/connect/cubes/{}/vault/members/{}",
            self.base_url, cube_id, member_id
        );
        let res = self.client.delete(&url).send().await?;
        res.check_success().await?;
        Ok(())
    }
}

// =============================================================================
// Cube Recovery Kit (W7)
// =============================================================================
//
// Unlike the rest of the client, these methods intercept 404 / 429 before
// `check_success` drains the response body, because:
//
// - 404 is not an error for the state machine driving the Settings card —
//   a fresh cube legitimately has no kit yet, and the card uses that to
//   pick the "Create Recovery Kit" copy. Surfacing it as the typed
//   `CoincubeError::NotFound` lets callers match directly instead of
//   pattern-matching on `Unsuccessful { status_code: 404, .. }`.
//
// - 429 carries a `Retry-After` header the UI needs to show a cooldown.
//   `check_success` consumes the whole response, so the header is parsed
//   before the body.
//
// Every other status follows the established flow (`check_success` →
// `Unsuccessful` → existing error rendering).

impl CoincubeClient {
    /// Parses a recovery-kit response into either the expected success
    /// body or one of the typed error variants (`NotFound`, `RateLimited`,
    /// auth errors via `Unsuccessful`, etc.). Body is only read on the
    /// non-NotFound failure paths.
    async fn parse_recovery_response<T: serde::de::DeserializeOwned>(
        res: reqwest::Response,
    ) -> Result<T, CoincubeError> {
        let status = res.status();
        if status.is_success() {
            let resp: ApiResponse<T> = res.json().await?;
            return Ok(resp.data);
        }
        match status.as_u16() {
            404 => Err(CoincubeError::NotFound),
            429 => Err(CoincubeError::RateLimited {
                retry_after: parse_retry_after(res.headers()),
            }),
            _ => Err(CoincubeError::Unsuccessful(
                crate::services::http::NotSuccessResponseInfo {
                    status_code: status.as_u16(),
                    text: res.text().await.unwrap_or_default(),
                },
            )),
        }
    }

    /// GET /api/v1/connect/cubes/{cubeId}/recovery-kit/status — lightweight
    /// existence/presence probe used to drive the Settings card copy. Never
    /// returns the ciphertext.
    pub async fn get_recovery_kit_status(
        &self,
        cube_id: u64,
    ) -> Result<RecoveryKitStatus, CoincubeError> {
        let url = format!(
            "{}/api/v1/connect/cubes/{}/recovery-kit/status",
            self.base_url, cube_id
        );
        let res = self.client.get(&url).send().await?;
        Self::parse_recovery_response(res).await
    }

    /// GET /api/v1/connect/cubes/{cubeId}/recovery-kit — fetch the
    /// ciphertext envelopes for restore. 404 → `CoincubeError::NotFound`;
    /// 429 → `CoincubeError::RateLimited` with parsed `Retry-After`.
    pub async fn get_recovery_kit(&self, cube_id: u64) -> Result<RecoveryKit, CoincubeError> {
        let url = format!(
            "{}/api/v1/connect/cubes/{}/recovery-kit",
            self.base_url, cube_id
        );
        let res = self.client.get(&url).send().await?;
        Self::parse_recovery_response(res).await
    }

    /// Upsert the kit: creates via POST when no kit exists on the server,
    /// otherwise updates via PUT. The status-check is a separate cheap
    /// round-trip; the state machine (§2.4) already has a cached status
    /// by the time it reaches this call path.
    ///
    /// `encrypted_cube_seed` / `encrypted_wallet_descriptor` are opaque
    /// base64 envelopes from `services::recovery::envelope::encrypt`; pass
    /// `None` for a half that isn't being touched this call (e.g. a
    /// passkey cube never uploads a seed envelope).
    pub async fn put_recovery_kit(
        &self,
        cube_id: u64,
        encrypted_cube_seed: Option<&str>,
        encrypted_wallet_descriptor: Option<&str>,
        encryption_scheme: &str,
    ) -> Result<RecoveryKit, CoincubeError> {
        // Try PUT first (the common case: users who back up a second
        // time already have a kit on the server), then fall back to
        // POST on 404. This skips the pre-upsert `status` probe —
        // which cost an extra round-trip and opened a race window
        // (kit could be deleted/created between probe and write) —
        // without changing the outward-facing method signature.
        let url = format!(
            "{}/api/v1/connect/cubes/{}/recovery-kit",
            self.base_url, cube_id
        );
        let body = UpsertRecoveryKitRequest {
            encrypted_cube_seed,
            encrypted_wallet_descriptor,
            encryption_scheme,
        };

        let put_res = self
            .client
            .request(Method::PUT, &url)
            .json(&body)
            .send()
            .await?;
        match Self::parse_recovery_response::<RecoveryKit>(put_res).await {
            Ok(kit) => Ok(kit),
            // 404 on PUT → no kit exists yet. Fall back to POST
            // once to create it. Any other error (auth, 429, 5xx)
            // propagates untouched so retries are the caller's
            // decision.
            Err(CoincubeError::NotFound) => {
                let post_res = self
                    .client
                    .request(Method::POST, &url)
                    .json(&body)
                    .send()
                    .await?;
                Self::parse_recovery_response(post_res).await
            }
            Err(e) => Err(e),
        }
    }

    /// DELETE /api/v1/connect/cubes/{cubeId}/recovery-kit — tears down the
    /// server-side kit. The caller is responsible for clearing any local
    /// drift-fingerprint cache (§2.7) on success.
    pub async fn delete_recovery_kit(&self, cube_id: u64) -> Result<(), CoincubeError> {
        let url = format!(
            "{}/api/v1/connect/cubes/{}/recovery-kit",
            self.base_url, cube_id
        );
        let res = self.client.delete(&url).send().await?;
        let status = res.status();
        if status.is_success() {
            return Ok(());
        }
        match status.as_u16() {
            404 => Err(CoincubeError::NotFound),
            429 => Err(CoincubeError::RateLimited {
                retry_after: parse_retry_after(res.headers()),
            }),
            _ => Err(CoincubeError::Unsuccessful(
                crate::services::http::NotSuccessResponseInfo {
                    status_code: status.as_u16(),
                    text: res.text().await.unwrap_or_default(),
                },
            )),
        }
    }
}

/// Parses a response's `Retry-After` header per RFC 7231 §7.1.3.
///
/// Accepts both documented forms:
///   - *delta-seconds*: e.g. `Retry-After: 60`
///   - *HTTP-date* (IMF-fixdate): e.g.
///     `Retry-After: Wed, 21 Oct 2026 07:28:00 GMT`. The returned
///     `Duration` is `date - now()`, clamped to zero when the date
///     has already passed (the server is saying "retry whenever").
///
/// Falls back to 60 seconds when the header is missing or doesn't
/// parse either form, so the UI always has a usable cooldown to
/// render rather than panicking or hanging.
pub(crate) fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Duration {
    let raw = match headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
    {
        Some(s) => s.trim(),
        None => return Duration::from_secs(60),
    };

    // Form 1: delta-seconds. The common case — every Cloudflare /
    // nginx rate limiter emits this shape.
    if let Ok(secs) = raw.parse::<u64>() {
        return Duration::from_secs(secs);
    }

    // Form 2: HTTP-date (IMF-fixdate). `DateTime::parse_from_rfc2822`
    // accepts the fixed-length IMF subset that RFC 7231 requires.
    if let Ok(at) = chrono::DateTime::parse_from_rfc2822(raw) {
        let now = chrono::Utc::now();
        let delta = at.with_timezone(&chrono::Utc) - now;
        // Negative or zero → the date is in the past or now.
        // `std::time::Duration` is unsigned, so `to_std` errors on
        // a negative `chrono::Duration`; that's the clamp-to-zero
        // case.
        return delta.to_std().unwrap_or_else(|_| Duration::from_secs(0));
    }

    // Unparseable — use the default cooldown rather than retrying
    // immediately and slamming the server again.
    Duration::from_secs(60)
}

#[cfg(test)]
mod retry_after_tests {
    use super::parse_retry_after;
    use reqwest::header::{HeaderMap, HeaderValue, RETRY_AFTER};
    use std::time::Duration;

    fn hdr(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(RETRY_AFTER, HeaderValue::from_str(value).unwrap());
        h
    }

    #[test]
    fn delta_seconds_form() {
        assert_eq!(parse_retry_after(&hdr("90")), Duration::from_secs(90));
        // Whitespace permitted per HTTP header conventions.
        assert_eq!(parse_retry_after(&hdr("  15  ")), Duration::from_secs(15));
    }

    #[test]
    fn http_date_form_parses_future_date() {
        // Regression test for the RFC 7231 IMF-fixdate branch that
        // was previously unreachable — the old implementation only
        // accepted delta-seconds. We build the header from "now + 30s"
        // so the assertion is deterministic even though wall-clock
        // time advances during the test.
        let at = chrono::Utc::now() + chrono::Duration::seconds(30);
        let hdr_value = at.format("%a, %d %b %Y %H:%M:%S GMT").to_string();
        let d = parse_retry_after(&hdr(&hdr_value));
        // Allow a generous lower bound for test-runner scheduling
        // jitter; the upper bound is our "now + 30s" ceiling.
        assert!(
            d >= Duration::from_secs(25) && d <= Duration::from_secs(30),
            "expected ~30s, got {}s",
            d.as_secs(),
        );
    }

    #[test]
    fn http_date_in_the_past_clamps_to_zero() {
        let at = chrono::Utc::now() - chrono::Duration::seconds(60);
        let hdr_value = at.format("%a, %d %b %Y %H:%M:%S GMT").to_string();
        assert_eq!(parse_retry_after(&hdr(&hdr_value)), Duration::from_secs(0));
    }

    #[test]
    fn missing_header_falls_back_to_60s() {
        assert_eq!(
            parse_retry_after(&HeaderMap::new()),
            Duration::from_secs(60)
        );
    }

    #[test]
    fn malformed_header_falls_back_to_60s() {
        // Neither a valid delta-seconds nor an IMF-fixdate.
        assert_eq!(parse_retry_after(&hdr("soon-ish")), Duration::from_secs(60));
    }
}

#[cfg(test)]
mod recovery_kit_tests {
    use super::*;
    use crate::services::coincube::RECOVERY_KIT_SCHEME_AES_256_GCM;
    use httpmock::{Method as MockMethod, MockServer};
    use serde_json::json;

    #[tokio::test]
    async fn get_recovery_kit_status_200_returns_flags() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(MockMethod::GET)
                .path("/api/v1/connect/cubes/42/recovery-kit/status");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "success": true,
                    "data": {
                        "hasRecoveryKit": true,
                        "hasEncryptedSeed": true,
                        "hasEncryptedWalletDescriptor": false,
                        "encryptionScheme": "aes-256-gcm",
                        "createdAt": "2026-04-22T00:00:00Z",
                        "updatedAt": "2026-04-22T00:00:00Z"
                    }
                }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let status = client
            .get_recovery_kit_status(42)
            .await
            .expect("status should succeed");
        mock.assert();
        assert!(status.has_recovery_kit);
        assert!(status.has_encrypted_seed);
        assert!(!status.has_encrypted_wallet_descriptor);
        assert_eq!(status.encryption_scheme, "aes-256-gcm");
    }

    #[tokio::test]
    async fn get_recovery_kit_status_404_maps_to_not_found() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(MockMethod::GET)
                .path("/api/v1/connect/cubes/42/recovery-kit/status");
            then.status(404)
                .header("content-type", "application/json")
                .json_body(json!({
                    "success": false,
                    "error": { "code": "CUBE_NOT_FOUND", "message": "no such cube" }
                }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let err = client
            .get_recovery_kit_status(42)
            .await
            .expect_err("expected NotFound");
        mock.assert();
        assert!(err.is_not_found(), "expected is_not_found, got {:?}", err);
    }

    #[tokio::test]
    async fn get_recovery_kit_status_403_maps_to_auth_error() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(MockMethod::GET)
                .path("/api/v1/connect/cubes/42/recovery-kit/status");
            then.status(403)
                .header("content-type", "application/json")
                .json_body(json!({
                    "success": false,
                    "error": { "code": "FORBIDDEN", "message": "not your cube" }
                }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let err = client
            .get_recovery_kit_status(42)
            .await
            .expect_err("expected 403");
        mock.assert();
        assert!(err.is_auth_error(), "expected auth error, got {:?}", err);
    }

    #[tokio::test]
    async fn get_recovery_kit_429_parses_retry_after() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(MockMethod::GET)
                .path("/api/v1/connect/cubes/42/recovery-kit");
            then.status(429)
                .header("content-type", "application/json")
                .header("Retry-After", "90")
                .json_body(json!({
                    "success": false,
                    "error": { "code": "RATE_LIMITED", "message": "slow down" }
                }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let err = client.get_recovery_kit(42).await.expect_err("expected 429");
        mock.assert();

        let retry = err
            .rate_limit_retry_after()
            .expect("expected RateLimited with Retry-After");
        assert_eq!(retry, Duration::from_secs(90));
    }

    #[tokio::test]
    async fn get_recovery_kit_429_without_header_falls_back_to_60s() {
        // The server may omit `Retry-After` entirely; the client should
        // still yield a typed RateLimited error rather than propagating
        // `Unsuccessful`. This keeps the state-machine pattern match
        // simple.
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(MockMethod::GET)
                .path("/api/v1/connect/cubes/42/recovery-kit");
            then.status(429)
                .header("content-type", "application/json")
                .json_body(json!({
                    "success": false,
                    "error": { "code": "RATE_LIMITED", "message": "slow down" }
                }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let err = client.get_recovery_kit(42).await.expect_err("expected 429");
        mock.assert();
        assert_eq!(
            err.rate_limit_retry_after().unwrap(),
            Duration::from_secs(60)
        );
    }

    #[tokio::test]
    async fn get_recovery_kit_200_returns_ciphertext() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(MockMethod::GET)
                .path("/api/v1/connect/cubes/42/recovery-kit");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "success": true,
                    "data": {
                        "id": 5,
                        "cubeId": 42,
                        "encryptedCubeSeed": "AAECAwQF...",
                        "encryptedWalletDescriptor": "",
                        "encryptionScheme": "aes-256-gcm",
                        "createdAt": "2026-04-22T00:00:00Z",
                        "updatedAt": "2026-04-22T00:00:00Z"
                    }
                }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let kit = client
            .get_recovery_kit(42)
            .await
            .expect("get_recovery_kit should succeed");
        mock.assert();
        assert_eq!(kit.id, 5);
        assert_eq!(kit.cube_id, 42);
        assert_eq!(kit.encrypted_cube_seed, "AAECAwQF...");
        assert!(kit.encrypted_wallet_descriptor.is_empty());
    }

    #[tokio::test]
    async fn put_recovery_kit_put_returns_kit_without_fallback() {
        // Common case — kit already exists on the server. A single
        // PUT succeeds and no POST is issued. Race-free by design:
        // no pre-upsert status probe.
        let server = MockServer::start();
        let put_mock = server.mock(|when, then| {
            when.method(MockMethod::PUT)
                .path("/api/v1/connect/cubes/42/recovery-kit")
                .json_body(json!({
                    "encryptedWalletDescriptor": "CIPHER_D",
                    "encryptionScheme": "aes-256-gcm"
                }));
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "success": true,
                    "data": {
                        "id": 5,
                        "cubeId": 42,
                        "encryptedCubeSeed": "CIPHER_A",
                        "encryptedWalletDescriptor": "CIPHER_D",
                        "encryptionScheme": "aes-256-gcm",
                        "createdAt": "2026-04-22T00:00:00Z",
                        "updatedAt": "2026-04-22T00:01:00Z"
                    }
                }));
        });
        // POST mock with no matcher beyond path — if the code
        // incorrectly fires POST, this will record a hit that we
        // assert below = 0.
        let post_mock = server.mock(|when, then| {
            when.method(MockMethod::POST)
                .path("/api/v1/connect/cubes/42/recovery-kit");
            then.status(500);
        });

        let client = CoincubeClient::for_test(server.base_url());
        let kit = client
            .put_recovery_kit(42, None, Some("CIPHER_D"), RECOVERY_KIT_SCHEME_AES_256_GCM)
            .await
            .expect("upsert should succeed");
        put_mock.assert();
        assert_eq!(post_mock.hits(), 0, "should not fall back to POST");
        assert_eq!(kit.encrypted_wallet_descriptor, "CIPHER_D");
    }

    #[tokio::test]
    async fn put_recovery_kit_falls_back_to_post_on_put_404() {
        // First-time backup: no kit on the server yet. PUT 404s;
        // client falls back to POST to create it.
        let server = MockServer::start();
        let put_mock = server.mock(|when, then| {
            when.method(MockMethod::PUT)
                .path("/api/v1/connect/cubes/42/recovery-kit");
            then.status(404);
        });
        let post_mock = server.mock(|when, then| {
            when.method(MockMethod::POST)
                .path("/api/v1/connect/cubes/42/recovery-kit")
                .json_body(json!({
                    "encryptedCubeSeed": "CIPHER_A",
                    "encryptionScheme": "aes-256-gcm"
                }));
            then.status(201)
                .header("content-type", "application/json")
                .json_body(json!({
                    "success": true,
                    "data": {
                        "id": 5,
                        "cubeId": 42,
                        "encryptedCubeSeed": "CIPHER_A",
                        "encryptedWalletDescriptor": "",
                        "encryptionScheme": "aes-256-gcm",
                        "createdAt": "2026-04-22T00:00:00Z",
                        "updatedAt": "2026-04-22T00:00:00Z"
                    }
                }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let kit = client
            .put_recovery_kit(42, Some("CIPHER_A"), None, RECOVERY_KIT_SCHEME_AES_256_GCM)
            .await
            .expect("upsert should succeed");
        put_mock.assert();
        post_mock.assert();
        assert_eq!(kit.encrypted_cube_seed, "CIPHER_A");
    }

    #[tokio::test]
    async fn put_recovery_kit_propagates_429_without_fallback() {
        // A 429 on PUT is NOT a signal to fall back to POST — the
        // server is asking us to back off, not telling us to change
        // method. Propagate the typed `RateLimited` error intact.
        let server = MockServer::start();
        let put_mock = server.mock(|when, then| {
            when.method(MockMethod::PUT)
                .path("/api/v1/connect/cubes/42/recovery-kit");
            then.status(429).header("Retry-After", "15");
        });
        let post_mock = server.mock(|when, then| {
            when.method(MockMethod::POST)
                .path("/api/v1/connect/cubes/42/recovery-kit");
            then.status(500);
        });

        let client = CoincubeClient::for_test(server.base_url());
        let err = client
            .put_recovery_kit(42, Some("X"), None, RECOVERY_KIT_SCHEME_AES_256_GCM)
            .await
            .expect_err("expected 429");
        put_mock.assert();
        assert_eq!(post_mock.hits(), 0, "429 must not trigger a POST fallback");
        assert_eq!(
            err.rate_limit_retry_after().unwrap(),
            Duration::from_secs(15)
        );
    }

    #[tokio::test]
    async fn put_recovery_kit_propagates_post_errors_after_put_404() {
        // If the fallback POST fails, surface *that* error — not the
        // PUT 404. Simulates a bad-request from the backend's
        // partial-field validator.
        let server = MockServer::start();
        let put_mock = server.mock(|when, then| {
            when.method(MockMethod::PUT)
                .path("/api/v1/connect/cubes/42/recovery-kit");
            then.status(404);
        });
        let post_mock = server.mock(|when, then| {
            when.method(MockMethod::POST)
                .path("/api/v1/connect/cubes/42/recovery-kit");
            then.status(403)
                .header("content-type", "application/json")
                .json_body(json!({
                    "success": false,
                    "error": { "code": "FORBIDDEN", "message": "not your cube" }
                }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let err = client
            .put_recovery_kit(42, Some("X"), None, RECOVERY_KIT_SCHEME_AES_256_GCM)
            .await
            .expect_err("expected 403");
        put_mock.assert();
        post_mock.assert();
        assert!(err.is_auth_error(), "expected auth error, got {:?}", err);
    }

    #[tokio::test]
    async fn delete_recovery_kit_ok_on_200() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(MockMethod::DELETE)
                .path("/api/v1/connect/cubes/42/recovery-kit");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({ "success": true, "data": { "status": "deleted" } }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        client
            .delete_recovery_kit(42)
            .await
            .expect("delete should succeed");
        mock.assert();
    }

    #[tokio::test]
    async fn delete_recovery_kit_404_maps_to_not_found() {
        // A second DELETE should be idempotent from the caller's
        // perspective — but the server is free to 404; we surface
        // that typed so the UI can treat it as "already gone".
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(MockMethod::DELETE)
                .path("/api/v1/connect/cubes/42/recovery-kit");
            then.status(404);
        });

        let client = CoincubeClient::for_test(server.base_url());
        let err = client
            .delete_recovery_kit(42)
            .await
            .expect_err("expected NotFound");
        mock.assert();
        assert!(err.is_not_found());
    }
}

#[cfg(test)]
mod cube_member_tests {
    use super::*;
    use httpmock::{Method, MockServer};
    use serde_json::json;

    #[tokio::test]
    async fn create_cube_invite_returns_added_on_existing_contact() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(Method::POST)
                .path("/api/v1/connect/cubes/42/invites")
                .json_body(json!({ "email": "new@example.com" }));
            then.status(201)
                .header("content-type", "application/json")
                .json_body(json!({
                    "success": true,
                    "data": {
                        "status": "added",
                        "member": {
                            "id": 7,
                            "userId": 99,
                            "user": { "email": "new@example.com" },
                            "joinedAt": "2026-04-18T00:00:00Z"
                        },
                        "invite": null
                    }
                }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let result = client
            .create_cube_invite(42, "new@example.com")
            .await
            .expect("create_cube_invite should succeed");

        mock.assert();
        match result {
            CubeInviteOrAddResult::Added(member) => {
                assert_eq!(member.id, 7);
                assert_eq!(member.user_id, 99);
                assert_eq!(member.user.email, "new@example.com");
            }
            CubeInviteOrAddResult::Invited(_) => panic!("expected Added, got Invited"),
        }
    }

    #[tokio::test]
    async fn create_cube_invite_returns_invited_on_new_email() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(Method::POST)
                .path("/api/v1/connect/cubes/42/invites");
            then.status(201)
                .header("content-type", "application/json")
                .json_body(json!({
                    "success": true,
                    "data": {
                        "status": "invited",
                        "member": null,
                        "invite": {
                            "id": 314,
                            "cubeId": 42,
                            "email": "brand-new@example.com",
                            "status": "pending",
                            "expiresAt": "2026-05-18T00:00:00Z",
                            "createdAt": "2026-04-18T00:00:00Z"
                        }
                    }
                }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let result = client
            .create_cube_invite(42, "brand-new@example.com")
            .await
            .expect("create_cube_invite should succeed");

        mock.assert();
        match result {
            CubeInviteOrAddResult::Invited(invite) => {
                assert_eq!(invite.id, 314);
                assert_eq!(invite.cube_id, 42);
                assert_eq!(invite.email, "brand-new@example.com");
                assert_eq!(invite.expires_at, "2026-05-18T00:00:00Z");
            }
            CubeInviteOrAddResult::Added(_) => panic!("expected Invited, got Added"),
        }
    }

    #[tokio::test]
    async fn create_cube_invite_409_on_duplicate_member() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(Method::POST)
                .path("/api/v1/connect/cubes/42/invites");
            then.status(409)
                .header("content-type", "application/json")
                .json_body(json!({
                    "success": false,
                    "error": {
                        "code": "CUBE_DUPLICATE_MEMBER",
                        "message": "User is already a member of this cube"
                    }
                }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let err = client
            .create_cube_invite(42, "dup@example.com")
            .await
            .expect_err("expected 409 error");
        mock.assert();

        let rendered = format!("{}", err);
        assert!(
            rendered.contains("already a member"),
            "error message should surface API text, got: {}",
            rendered
        );
    }

    #[tokio::test]
    async fn revoke_cube_invite_ok_on_success() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(Method::DELETE)
                .path("/api/v1/connect/cubes/42/invites/314");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "success": true,
                    "data": { "status": "revoked" }
                }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        client
            .revoke_cube_invite(42, 314)
            .await
            .expect("revoke_cube_invite should succeed");
        mock.assert();
    }

    #[tokio::test]
    async fn remove_cube_member_409_propagates_error() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(Method::DELETE)
                .path("/api/v1/connect/cubes/42/members/7");
            then.status(409)
                .header("content-type", "application/json")
                .json_body(json!({
                    "success": false,
                    "error": {
                        "code": "CONTACT_HAS_KEYS_IN_ACTIVE_VAULTS",
                        "message": "Cannot remove member — keys are signing an active Vault"
                    }
                }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let err = client
            .remove_cube_member(42, 7)
            .await
            .expect_err("expected 409 error");
        mock.assert();

        let rendered = format!("{}", err);
        assert!(
            rendered.contains("active Vault"),
            "error message should surface API text, got: {}",
            rendered
        );
    }

    #[tokio::test]
    async fn get_cube_deserializes_members_and_pending_invites() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(Method::GET).path("/api/v1/connect/cubes/42");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "success": true,
                    "data": {
                        "id": 42,
                        "uuid": "abc-123",
                        "name": "My Cube",
                        "network": "bitcoin",
                        "lightningAddress": "me@coincube.io",
                        "bolt12Offer": null,
                        "status": "active",
                        "members": [
                            {
                                "id": 7,
                                "userId": 99,
                                "user": { "email": "alice@example.com" },
                                "joinedAt": "2026-04-18T00:00:00Z"
                            }
                        ],
                        "pendingInvites": [
                            {
                                "id": 314,
                                "cubeId": 42,
                                "email": "bob@example.com",
                                "status": "pending",
                                "expiresAt": "2026-05-18T00:00:00Z",
                                "createdAt": "2026-04-18T00:00:00Z"
                            }
                        ]
                    }
                }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let cube = client.get_cube(42).await.expect("get_cube should succeed");
        mock.assert();

        assert_eq!(cube.id, 42);
        assert_eq!(cube.members.len(), 1);
        assert_eq!(cube.members[0].user.email, "alice@example.com");
        assert_eq!(cube.pending_invites.len(), 1);
        assert_eq!(cube.pending_invites[0].email, "bob@example.com");
    }

    #[tokio::test]
    async fn get_cube_keys_deserialises_w3_payload() {
        use crate::services::coincube::CubeKeyRaw;
        // W3 shape: `ownerUserId`, `ownerEmail`, `isOwnKey`, `usedByVault`
        // populated; legacy fields absent.
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(Method::GET)
                .path("/api/v1/connect/cubes/abc-uuid/keys");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "success": true,
                    "data": [
                        {
                            "id": 1,
                            "name": "Alice's Key",
                            "xpub": "xpub661...",
                            "fingerprint": "deadbeef",
                            "derivationPath": "m/48'/0'/0'/2'",
                            "network": "bitcoin",
                            "status": "active",
                            "ownerUserId": 99,
                            "ownerEmail": "alice@example.com",
                            "isOwnKey": false,
                            "usedByVault": true
                        }
                    ]
                }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let keys = client
            .get_cube_keys("abc-uuid")
            .await
            .expect("get_cube_keys should succeed");
        mock.assert();

        assert_eq!(keys.len(), 1);
        let k: &CubeKeyRaw = &keys[0];
        assert_eq!(k.owner_user_id, 99);
        assert_eq!(k.effective_owner_user_id(), 99);
        assert_eq!(k.owner_email, "alice@example.com");
        assert!(!k.is_own_key);
        assert!(k.used_by_vault);
        // Legacy fields default out.
        assert_eq!(k.primary_owner_id, 0);
    }

    #[tokio::test]
    async fn get_cube_keys_deserialises_legacy_payload() {
        use crate::services::coincube::CubeKeyRaw;
        // Pre-W3 shape: only legacy fields; new fields defaulted.
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(Method::GET)
                .path("/api/v1/connect/cubes/abc-uuid/keys");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "success": true,
                    "data": [
                        {
                            "id": 1,
                            "primaryOwnerId": 7,
                            "keychainId": 42,
                            "name": "Legacy Key",
                            "curve": "secp256k1",
                            "taproot": false,
                            "xpub": "xpub661...",
                            "fingerprint": "deadbeef",
                            "derivationPath": "m/48'/0'/0'/2'",
                            "network": "bitcoin",
                            "cubeId": 5,
                            "status": "active",
                            "createdAt": "2026-04-18T00:00:00Z",
                            "updatedAt": "2026-04-18T00:00:00Z"
                        }
                    ]
                }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let keys = client
            .get_cube_keys("abc-uuid")
            .await
            .expect("get_cube_keys should succeed");
        mock.assert();

        assert_eq!(keys.len(), 1);
        let k: &CubeKeyRaw = &keys[0];
        assert_eq!(k.primary_owner_id, 7);
        assert_eq!(k.effective_owner_user_id(), 7);
        assert!(k.owner_email.is_empty());
        assert!(!k.is_own_key);
        assert!(!k.used_by_vault);
    }

    #[tokio::test]
    async fn create_connect_vault_returns_empty_shell() {
        use crate::services::coincube::CreateConnectVaultRequest;
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(Method::POST)
                .path("/api/v1/connect/cubes/42/vault")
                .json_body(json!({ "timelockDays": 180 }));
            then.status(201)
                .header("content-type", "application/json")
                .json_body(json!({
                    "success": true,
                    "data": {
                        "id": 5,
                        "cubeId": 42,
                        "timelockDays": 180,
                        "timelockExpiresAt": "2026-10-15T00:00:00Z",
                        "lastResetAt": "2026-04-18T00:00:00Z",
                        "status": "active",
                        "members": [],
                        "createdAt": "2026-04-18T00:00:00Z",
                        "updatedAt": "2026-04-18T00:00:00Z"
                    }
                }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let vault = client
            .create_connect_vault(42, CreateConnectVaultRequest { timelock_days: 180 })
            .await
            .expect("create_connect_vault should succeed");
        mock.assert();
        assert_eq!(vault.id, 5);
        assert_eq!(vault.cube_id, 42);
        assert_eq!(vault.timelock_days, 180);
        assert!(vault.members.is_empty());
    }

    #[tokio::test]
    async fn add_vault_member_omits_optional_fields_when_none() {
        use crate::services::coincube::{AddVaultMemberRequest, VaultMemberRole};
        // Verify that `contactId` is omitted from the JSON when None
        // (self-member case). `keyId` is present.
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(Method::POST)
                .path("/api/v1/connect/cubes/42/vault/members")
                .json_body(json!({ "keyId": 99, "role": "keyholder" }));
            then.status(201)
                .header("content-type", "application/json")
                .json_body(json!({
                    "success": true,
                    "data": {
                        "id": 7,
                        "keyId": 99,
                        "role": "keyholder",
                        "createdAt": "2026-04-18T00:00:00Z"
                    }
                }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let member = client
            .add_vault_member(
                42,
                AddVaultMemberRequest {
                    contact_id: None,
                    key_id: Some(99),
                    role: VaultMemberRole::Keyholder,
                },
            )
            .await
            .expect("add_vault_member should succeed");
        mock.assert();
        assert_eq!(member.id, 7);
        assert_eq!(member.key_id, Some(99));
        assert_eq!(member.role, VaultMemberRole::Keyholder);
    }

    #[tokio::test]
    async fn add_vault_member_409_key_already_used_maps_to_helper() {
        use crate::services::coincube::{AddVaultMemberRequest, VaultMemberRole};
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(Method::POST)
                .path("/api/v1/connect/cubes/42/vault/members");
            then.status(409).header("content-type", "application/json").json_body(json!({
                "success": false,
                "error": {
                    "code": "KEY_ALREADY_USED_IN_VAULT",
                    "message": "Key has already been used in another vault; a key can participate in at most one vault."
                },
                "keyId": 99,
                "vaultId": 17
            }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let err = client
            .add_vault_member(
                42,
                AddVaultMemberRequest {
                    contact_id: None,
                    key_id: Some(99),
                    role: VaultMemberRole::Keyholder,
                },
            )
            .await
            .expect_err("expected 409");
        mock.assert();
        assert!(
            err.is_key_already_used_in_vault(),
            "is_key_already_used_in_vault() should match: {}",
            err
        );
    }

    #[tokio::test]
    async fn add_vault_member_generic_409_not_matched_by_helper() {
        use crate::services::coincube::{AddVaultMemberRequest, VaultMemberRole};
        // A 409 with a different error code must NOT trip the helper.
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(Method::POST)
                .path("/api/v1/connect/cubes/42/vault/members");
            then.status(409)
                .header("content-type", "application/json")
                .json_body(json!({
                    "success": false,
                    "error": {
                        "code": "DUPLICATE_RESOURCE",
                        "message": "This member already exists on the vault"
                    }
                }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let err = client
            .add_vault_member(
                42,
                AddVaultMemberRequest {
                    contact_id: Some(1),
                    key_id: Some(2),
                    role: VaultMemberRole::Keyholder,
                },
            )
            .await
            .expect_err("expected 409");
        mock.assert();
        assert!(
            !err.is_key_already_used_in_vault(),
            "generic 409 must not match the W9 helper"
        );
    }

    #[tokio::test]
    async fn add_vault_member_409_vault_keyholder_locked_maps_to_error_variant() {
        // W16-desktop: 409 with code `VAULT_KEYHOLDER_LOCKED` must be
        // reclassified into the typed `CoincubeError::VaultKeyholderLocked`
        // variant so the UI can render the tailored "quorum is fixed"
        // dialog rather than the generic error banner.
        use crate::services::coincube::{AddVaultMemberRequest, CoincubeError, VaultMemberRole};
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(Method::POST)
                .path("/api/v1/connect/cubes/42/vault/members");
            then.status(409)
                .header("content-type", "application/json")
                .json_body(json!({
                    "success": false,
                    "error": {
                        "code": "VAULT_KEYHOLDER_LOCKED",
                        "message": "Cannot add a keyholder to an active Vault."
                    },
                    "vaultId": 42
                }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let err = client
            .add_vault_member(
                42,
                AddVaultMemberRequest {
                    contact_id: Some(1),
                    key_id: Some(2),
                    role: VaultMemberRole::Keyholder,
                },
            )
            .await
            .expect_err("expected 409");
        mock.assert();
        assert!(
            matches!(err, CoincubeError::VaultKeyholderLocked { vault_id: 42 }),
            "expected VaultKeyholderLocked {{ vault_id: 42 }}, got: {:?}",
            err
        );
    }

    #[test]
    fn add_vault_member_role_chooser_hides_keyholder_on_active_vault() {
        // W16-desktop: the Keyholder role must not be offered when the
        // target Vault's status is `Active` — the signing quorum is
        // sealed, so adding a keyholder row would have no on-chain effect.
        use crate::services::coincube::{allowed_vault_member_roles, VaultMemberRole, VaultStatus};
        let roles = allowed_vault_member_roles(Some(&VaultStatus::Active));
        assert!(
            !roles.contains(&VaultMemberRole::Keyholder),
            "Active vault must hide the Keyholder role; got {:?}",
            roles
        );
        assert!(roles.contains(&VaultMemberRole::Beneficiary));
        assert!(roles.contains(&VaultMemberRole::Observer));
    }

    #[test]
    fn add_vault_member_role_chooser_shows_keyholder_on_expired_vault() {
        // On `Expired` (and on any status other than `Active`) the role
        // picker still offers Keyholder because rebuilding the Vault is
        // a legitimate follow-up and the backend accepts it.
        use crate::services::coincube::{allowed_vault_member_roles, VaultMemberRole, VaultStatus};
        let roles = allowed_vault_member_roles(Some(&VaultStatus::Expired));
        assert!(
            roles.contains(&VaultMemberRole::Keyholder),
            "non-Active vault must include Keyholder; got {:?}",
            roles
        );
    }

    #[tokio::test]
    async fn create_invite_with_cube_ids_posts_expected_json_body() {
        use crate::services::coincube::{ContactRole, CreateInviteRequest};
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(Method::POST)
                .path("/api/v1/connect/invites")
                .json_body(json!({
                    "email": "friend@example.com",
                    "role": "keyholder",
                    "cubeIds": [1, 7, 42]
                }));
            then.status(201)
                .header("content-type", "application/json")
                .json_body(json!({
                    "success": true,
                    "data": { "id": 100 }
                }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        client
            .create_invite(CreateInviteRequest {
                email: "friend@example.com".to_string(),
                role: ContactRole::Keyholder,
                cube_ids: vec![1, 7, 42],
            })
            .await
            .expect("create_invite should succeed");
        mock.assert();
    }

    #[tokio::test]
    async fn create_invite_without_cube_ids_omits_field() {
        // Backward-compat with pre-W10 staging backends: when cube_ids is
        // empty, the JSON body must NOT carry the field.
        use crate::services::coincube::{ContactRole, CreateInviteRequest};
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(Method::POST)
                .path("/api/v1/connect/invites")
                .json_body(json!({
                    "email": "friend@example.com",
                    "role": "keyholder"
                }));
            then.status(201)
                .header("content-type", "application/json")
                .json_body(json!({
                    "success": true,
                    "data": { "id": 100 }
                }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        client
            .create_invite(CreateInviteRequest {
                email: "friend@example.com".to_string(),
                role: ContactRole::Keyholder,
                cube_ids: Vec::new(),
            })
            .await
            .expect("create_invite should succeed");
        mock.assert();
    }

    #[tokio::test]
    async fn get_cube_tolerates_missing_members_and_invites() {
        // `list_cubes()` may return cubes without the members/pending_invites
        // fields; serde(default) should keep those call sites working.
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(Method::GET).path("/api/v1/connect/cubes/42");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "success": true,
                    "data": {
                        "id": 42,
                        "uuid": "abc-123",
                        "name": "My Cube",
                        "network": "bitcoin",
                        "lightningAddress": null,
                        "bolt12Offer": null,
                        "status": "active"
                    }
                }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let cube = client.get_cube(42).await.expect("get_cube should succeed");
        mock.assert();

        assert!(cube.members.is_empty());
        assert!(cube.pending_invites.is_empty());
    }
}
