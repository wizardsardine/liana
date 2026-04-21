use super::{
    get_countries, AddVaultMemberRequest, ApiErrorResponse, ApiResponse, AvatarGenerateData,
    AvatarGenerateRequest, AvatarSelectData, AvatarSelectRequest, BillingHistoryEntry,
    ChargeStatusResponse, CheckUsernameResponse, CheckoutRequest, CheckoutResponse,
    ClaimLightningAddressRequest, CoincubeError, ConnectPlan, ConnectVaultResponse, Contact,
    ContactCube, Country, CreateConnectVaultRequest, CreateInviteRequest, CubeInviteOrAddResult,
    CubeKeyRaw, CubeLimitsResponse, CubeResponse, DownloadStats, FeaturesResponse, GetAvatarData,
    Invite, LightningAddress, LoginActivity, LoginResponse, OtpRequest, OtpVerifyRequest,
    PublicAvatarData, RefreshTokenRequest, RegenerationData, RegisterCubeRequest, SaveQuoteRequest,
    SaveQuoteResponse, StatsPeriod, TimeseriesResponse, TodayStats, UpdateCubeRequest, User,
    VaultMemberResponse, VerifiedDevice,
};
use reqwest::{Client, Method};
use serde::Deserialize;

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
