use reqwest::Response;
use serde::{Deserialize, Serialize};

use crate::services::http::{NotSuccessResponseInfo, ResponseExt};

#[derive(Debug, Serialize, Deserialize)]
pub struct AuthDetail {
    pub provider: u8, // 1 for email provider
    pub password: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SignUpRequest {
    #[serde(rename = "accountType")]
    pub account_type: String,
    pub email: String,
    #[serde(rename = "firstName")]
    pub first_name: String,
    #[serde(rename = "lastName")]
    pub last_name: String,
    #[serde(rename = "authDetails")]
    pub auth_details: Vec<AuthDetail>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EmailVerificationStatusRequest {
    pub email: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ResendVerificationEmailRequest {
    pub email: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LoginRequest {
    pub provider: u8, // 1 for email provider
    pub email: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: u32,
    pub email: String,
    #[serde(rename = "firstName")]
    pub first_name: String,
    #[serde(rename = "lastName")]
    pub last_name: String,
    #[serde(rename = "emailVerified")]
    pub email_verified: bool,
    #[serde(rename = "needs2FASetup")]
    pub needs_2fa_setup: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignUpResponse {
    pub status: String,
    pub data: User,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailVerificationStatusResponse {
    pub email: String,
    #[serde(rename = "emailVerified")]
    pub email_verified: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResendEmailResponse {
    pub message: String,
    pub email: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginResponse {
    #[serde(rename = "requires_2fa")]
    pub requires_two_factor: bool,
    pub token: Option<String>, // JWT token for authenticated requests
    pub user: Option<User>,    // User data when login is successful
}

#[derive(Debug, Clone)]
pub struct RegistrationClient {
    http: reqwest::Client,
    base_url: String,
}

#[derive(Debug, Clone)]
pub struct RegistrationError {
    pub http_status: Option<u16>,
    pub error: String,
}

impl std::fmt::Display for RegistrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Registration error: {}", self.error)
    }
}

impl std::error::Error for RegistrationError {}

impl From<reqwest::Error> for RegistrationError {
    fn from(error: reqwest::Error) -> Self {
        Self {
            http_status: error.status().map(|s| s.as_u16()),
            error: error.to_string(),
        }
    }
}

impl From<NotSuccessResponseInfo> for RegistrationError {
    fn from(info: NotSuccessResponseInfo) -> Self {
        Self {
            http_status: Some(info.status_code),
            error: info.text,
        }
    }
}

impl RegistrationClient {
    pub fn new(base_url: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url,
        }
    }

    async fn post_json<T: Serialize>(
        &self,
        endpoint: &str,
        body: &T,
    ) -> Result<Response, RegistrationError> {
        let url = format!("{}/auth/{}", self.base_url, endpoint);

        let response = self
            .http
            .post(&url)
            .header("Content-Type", "application/json")
            .json(body)
            .send()
            .await?;

        Ok(response)
    }

    pub async fn sign_up(
        &self,
        request: SignUpRequest,
    ) -> Result<SignUpResponse, RegistrationError> {
        let response = self
            .post_json("signup", &request)
            .await?
            .check_success()
            .await?;

        let signup_response: SignUpResponse = response.json().await?;
        Ok(signup_response)
    }

    pub async fn check_email_verification_status(
        &self,
        email: &str,
    ) -> Result<EmailVerificationStatusResponse, RegistrationError> {
        let request = EmailVerificationStatusRequest {
            email: email.to_string(),
        };

        let response = self
            .post_json("email-verification-status", &request)
            .await?
            .check_success()
            .await?;

        let status_response: EmailVerificationStatusResponse = response.json().await?;
        Ok(status_response)
    }

    pub async fn resend_verification_email(
        &self,
        email: &str,
    ) -> Result<ResendEmailResponse, RegistrationError> {
        let request = ResendVerificationEmailRequest {
            email: email.to_string(),
        };

        let response = self
            .post_json("resend-verification-email", &request)
            .await?
            .check_success()
            .await?;

        let resend_response: ResendEmailResponse = response.json().await?;
        Ok(resend_response)
    }

    pub async fn login(
        &self,
        email: &str,
        password: &str,
    ) -> Result<LoginResponse, RegistrationError> {
        let request = LoginRequest {
            provider: 1, // EmailProvider = 1
            email: email.to_string(),
            password: password.to_string(),
        };

        let response = self
            .post_json("login", &request)
            .await?
            .check_success()
            .await?;

        let login_response: LoginResponse = response.json().await?;
        Ok(login_response)
    }
}
