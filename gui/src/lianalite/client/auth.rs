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

    pub async fn sign_in_otp(&self) -> Result<(), AuthError> {
        let response: Response = self
            .request(Method::POST, &format!("{}/auth/v1/otp", self.url))
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

    pub async fn resend_otp(&self) -> Result<Response, AuthError> {
        let response: Response = self
            .request(Method::POST, &format!("{}/auth/v1/resend", self.url))
            .json(&ResendOtp {
                email: &self.email,
                kind: "signup",
            })
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(AuthError {
                http_status: Some(response.status().into()),
                error: response.text().await?,
            });
        }
        Ok(response)
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
