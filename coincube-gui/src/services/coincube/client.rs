use super::*;
use reqwest::{Client, Method};

use crate::services::http::ResponseExt;

#[derive(Debug, Clone)]
pub struct CoincubeClient {
    pub client: Client,
    pub base_url: &'static str,
}

impl Default for CoincubeClient {
    fn default() -> Self {
        Self::new()
    }
}

impl CoincubeClient {
    pub fn new() -> Self {
        let base_url = option_env!("COINCUBE_API_URL").unwrap_or("https://dev-api.coincube.io");

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
        }
    }

    /// A JWT is needed for some authenticated endpoints, acquired after a user successfully logs in
    pub fn set_token(&mut self, token: &str) {
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
    pub async fn fetch_download_stats(&self) -> Result<super::DownloadStats, super::CoincubeError> {
        let url = format!("{}/api/v1/downloads", self.base_url);
        let res = self.client.get(&url).send().await?;
        let res = res.check_success().await?;
        Ok(res.json().await?)
    }

    pub async fn fetch_today_stats(&self) -> Result<super::TodayStats, super::CoincubeError> {
        let url = format!("{}/api/v1/downloads/today", self.base_url);
        let res = self.client.get(&url).send().await?;
        let res = res.check_success().await?;
        Ok(res.json().await?)
    }

    pub async fn fetch_timeseries(
        &self,
        period: super::StatsPeriod,
    ) -> Result<super::TimeseriesResponse, super::CoincubeError> {
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
    pub async fn get_user(&self) -> Result<super::User, CoincubeError> {
        let url = format!("{}/api/v1/user", self.base_url);
        let res = self.client.get(&url).send().await?;
        let res = res.check_success().await?;
        Ok(res.json().await?)
    }

    pub async fn get_connect_plan(&self) -> Result<super::ConnectPlan, CoincubeError> {
        let url = format!("{}/api/v1/connect/plan", self.base_url);
        let res = self.client.get(&url).send().await?;
        let res = res.check_success().await?;
        Ok(res.json().await?)
    }

    pub async fn get_verified_devices(&self) -> Result<Vec<super::VerifiedDevice>, CoincubeError> {
        let url = format!("{}/api/v1/verified-device/", self.base_url);
        let res = self.client.get(&url).send().await?;
        let res = res.check_success().await?;
        Ok(res.json().await?)
    }

    pub async fn get_login_activity(&self) -> Result<Vec<super::LoginActivity>, CoincubeError> {
        let url = format!("{}/api/v1/login-activity/", self.base_url);
        let res = self.client.get(&url).send().await?;
        let res = res.check_success().await?;
        Ok(res.json().await?)
    }

    pub async fn get_lightning_address(&self) -> Result<super::LightningAddress, CoincubeError> {
        let url = format!("{}/api/v1/connect/lightning-address", self.base_url);
        let res = self.client.get(&url).send().await?;
        let res = res.check_success().await?;
        let resp: super::ApiResponse<super::LightningAddress> = res.json().await?;
        Ok(resp.data)
    }

    pub async fn check_lightning_address(
        &self,
        username: &str,
    ) -> Result<super::CheckUsernameResponse, CoincubeError> {
        let url = format!(
            "{}/api/v1/connect/lightning-address/check?username={}",
            self.base_url, username
        );
        let res = self.client.get(&url).send().await?;
        // The API returns error HTTP status for invalid/reserved usernames.
        // Parse the body in all cases to extract structured info.
        let status = res.status();
        let body = res.text().await.map_err(|e| CoincubeError::Network(e))?;

        if status.is_success() {
            let resp: super::ApiResponse<super::CheckUsernameResponse> =
                serde_json::from_str(&body)?;
            Ok(resp.data)
        } else {
            // Try to extract the error message from the JSON body
            if let Ok(err_resp) = serde_json::from_str::<super::ApiErrorResponse>(&body) {
                Ok(super::CheckUsernameResponse {
                    available: false,
                    username: username.to_string(),
                    error_message: Some(err_resp.error.message),
                })
            } else {
                Err(CoincubeError::Api(body))
            }
        }
    }

    pub async fn claim_lightning_address(
        &self,
        req: super::ClaimLightningAddressRequest,
    ) -> Result<super::LightningAddress, CoincubeError> {
        let url = format!("{}/api/v1/connect/lightning-address", self.base_url);
        let res = self.client.post(&url).json(&req).send().await?;
        let res = res.check_success().await?;
        let resp: super::ApiResponse<super::LightningAddress> = res.json().await?;
        Ok(resp.data)
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
