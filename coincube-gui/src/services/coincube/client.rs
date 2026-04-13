use super::{
    get_countries, ApiErrorResponse, ApiResponse, AvatarGenerateData, AvatarGenerateRequest,
    AvatarSelectData, AvatarSelectRequest, BillingHistoryEntry, ChargeStatusResponse,
    CheckUsernameResponse, CheckoutRequest, CheckoutResponse, ClaimLightningAddressRequest,
    CoincubeError, ConnectPlan, Contact, ContactCube, Country, CreateInviteRequest,
    CubeLimitsResponse, CubeResponse, DownloadStats, FeaturesResponse, GetAvatarData, Invite,
    LightningAddress, LoginActivity, LoginResponse, OtpRequest, OtpVerifyRequest, PublicAvatarData,
    RefreshTokenRequest, RegenerationData, RegisterCubeRequest, SaveQuoteRequest,
    SaveQuoteResponse, StatsPeriod, TimeseriesResponse, TodayStats, UpdateCubeRequest, User,
    VerifiedDevice,
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
