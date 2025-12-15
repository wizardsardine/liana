use super::*;
use reqwest::{Client, Method};

use crate::services::http::ResponseExt;

#[derive(Debug, Clone)]
pub struct CoincubeClient {
    client: Client,
    base_url: &'static str,
}

impl CoincubeClient {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            base_url: match cfg!(debug_assertions) {
                false => "https://api.coincube.io",
                true => option_env!("COINCUBE_API_URL").unwrap_or("https://dev-api.coincube.io"),
            },
        }
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

// registration endpoints
impl CoincubeClient {
    pub async fn sign_up(&self, request: SignUpRequest) -> Result<SignUpResponse, CoincubeError> {
        let response = {
            let url = format!("{}{}", self.base_url, "/api/v1/auth/signup");
            self.client
                .post(&url)
                .header("Content-Type", "application/json")
                .json(&request)
                .send()
        }
        .await?;
        let response = response.check_success().await?;

        Ok(response.json().await?)
    }

    pub async fn check_email_verification_status(
        &self,
        email: &str,
    ) -> Result<EmailVerificationStatusResponse, CoincubeError> {
        let request = EmailVerificationStatusRequest { email };

        let response = {
            let url = format!(
                "{}{}",
                self.base_url, "/api/v1/auth/email-verification-status"
            );
            self.client.post(&url).json(&request).send()
        }
        .await?;
        let response = response.check_success().await?;

        Ok(response.json().await?)
    }

    pub async fn send_verification_email(
        &self,
        email: &str,
    ) -> Result<VerifyEmailResponse, CoincubeError> {
        let request = ResendVerificationEmailRequest {
            email: email.to_string(),
        };

        let response = {
            let url = format!(
                "{}{}",
                self.base_url, "/api/v1/auth/resend-verification-email"
            );
            self.client.post(&url).json(&request).send()
        }
        .await?;
        let response = response.check_success().await?;

        Ok(response.json().await?)
    }

    pub async fn login(&self, email: &str, password: &str) -> Result<LoginResponse, CoincubeError> {
        let request = LoginRequest {
            provider: 1, // EmailProvider = 1
            email: email.to_string(),
            password: password.to_string(),
        };

        let response = {
            let url = format!("{}{}", self.base_url, "/api/v1/auth/login");
            self.client.post(&url).json(&request).send()
        }
        .await?;
        let response = response.check_success().await?;

        Ok(response.json().await?)
    }

    pub async fn send_password_reset_email(
        &self,
        email: &str,
    ) -> Result<PasswordResetEmailResponse, CoincubeError> {
        let request = PasswordResetEmailRequest { email };

        let response = {
            let url = format!("{}{}", self.base_url, "/api/v1/auth/forgot-password");
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
    /// Detects the user's country and returns (country_name, iso_code)
    pub async fn locate(&self) -> Result<Country, CoincubeError> {
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

        match {
            get_countries()
                .iter()
                .find(|c| c.code == &iso_code)
                .cloned()
        } {
            Some(country) => Ok(country),
            None => Err(CoincubeError::Api(format!(
                "Unknown country iso code: ({})",
                iso_code
            ))),
        }
    }
}
