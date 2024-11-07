use reqwest::{Error, IntoUrl, Method, RequestBuilder, Response};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct SignInOtp<'a> {
    email: &'a str,
    create_user: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct VerifyOtp<'a, 'b> {
    email: &'a str,
    token: &'b str,
    #[serde(rename = "type")]
    kind: &'static str,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ResendOtp<'a> {
    email: &'a str,
    #[serde(rename = "type")]
    kind: &'static str,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RefreshToken<'a> {
    refresh_token: &'a str,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessTokenResponse {
    pub access_token: String,
    pub expires_at: i64,
    pub refresh_token: String,
}

#[derive(Debug, Clone)]
pub struct AuthClient {
    http: reqwest::Client,
    url: String,
    api_public_key: String,
    pub email: String,
}

#[derive(Debug, Clone)]
pub struct AuthError {
    pub http_status: Option<u16>,
    pub error: String,
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        if let Some(status) = self.http_status {
            write!(f, "{}: {}", status, self.error)
        } else {
            write!(f, "{}", self.error)
        }
    }
}

impl From<Error> for AuthError {
    fn from(value: Error) -> Self {
        AuthError {
            http_status: None,
            error: value.to_string(),
        }
    }
}

impl AuthClient {
    pub fn new(url: String, api_public_key: String, email: String) -> Self {
        AuthClient {
            http: reqwest::Client::new(),
            url,
            api_public_key,
            email,
        }
    }

    fn request<U: IntoUrl>(&self, method: Method, url: U) -> RequestBuilder {
        let req = self
            .http
            .request(method, url)
            .header("apikey", &self.api_public_key)
            .header("Content-Type", "application/json");
        tracing::debug!("Sending http request: {:?}", req);
        req
    }

    /// the redirect_to is setup so the supabase html template has the information
    /// that user is using the desktop to authenticate and will display the token
    /// instead of the confirmation link button.
    pub async fn sign_in_otp(&self) -> Result<(), AuthError> {
        let response: Response = self
            .request(
                Method::POST,
                &format!(
                    "{}/auth/v1/otp?redirect_to=https://desktop.lianalite.com",
                    self.url
                ),
            )
            .json(&SignInOtp {
                email: &self.email,
                create_user: true,
            })
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(AuthError {
                http_status: Some(response.status().into()),
                error: response.text().await?,
            });
        }

        Ok(())
    }

    /// Resend method has to trigger a signInWithOTP method instead of using the resend endpoint.
    /// The resend endpoint is used to resend an existing signup confirmation email, email change email, SMS OTP phone signup)
    /// or phone change OTP. This method will only resend an email or phone OTP to the user if there was an initial signup,
    /// email change or phone change request being made.
    /// If we want to resend a passwordless sign-in OTP or Magic Link, we have to use the signInWithOtp() method again.
    pub async fn resend_otp(&self) -> Result<(), AuthError> {
        self.sign_in_otp().await
    }

    pub async fn verify_otp(&self, token: &str) -> Result<AccessTokenResponse, AuthError> {
        let response: Response = self
            .http
            .post(&format!("{}/auth/v1/verify", self.url))
            .header("apikey", &self.api_public_key)
            .header("Content-Type", "application/json")
            .json(&VerifyOtp {
                email: &self.email,
                token,
                kind: "email",
            })
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(AuthError {
                http_status: Some(response.status().into()),
                error: response.text().await?,
            });
        }

        Ok(response.json().await?)
    }

    pub async fn refresh_token(
        &self,
        refresh_token: &str,
    ) -> Result<AccessTokenResponse, AuthError> {
        let response: Response = self
            .http
            .post(&format!(
                "{}/auth/v1/token?grant_type=refresh_token",
                self.url
            ))
            .header("apikey", &self.api_public_key)
            .header("Content-Type", "application/json")
            .json(&RefreshToken { refresh_token })
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(AuthError {
                http_status: Some(response.status().into()),
                error: response.text().await?,
            });
        }
        Ok(response.json().await?)
    }
}
