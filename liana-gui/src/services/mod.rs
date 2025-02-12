pub mod api;

use reqwest::{self, IntoUrl, Method, RequestBuilder};
use serde_json::json;

const KEYS_API_URL: &str = "https://keys.wizardsardine.com";

#[derive(Debug, Clone)]
pub enum Error {
    Http(Option<u16>, String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Http(kind, e) => write!(f, "Http error: [{:?}] {}", kind, e),
        }
    }
}

impl From<reqwest::Error> for Error {
    fn from(error: reqwest::Error) -> Self {
        Self::Http(None, error.to_string())
    }
}

async fn check_response_status(response: reqwest::Response) -> Result<reqwest::Response, Error> {
    if !response.status().is_success() {
        return Err(Error::Http(
            Some(response.status().into()),
            response.text().await?,
        ));
    }
    Ok(response)
}

fn request<U: reqwest::IntoUrl>(
    http: &reqwest::Client,
    method: reqwest::Method,
    url: U,
) -> reqwest::RequestBuilder {
    let req = http
        .request(method, url)
        .header("Content-Type", "application/json")
        .header("API-Version", "0.1");
    tracing::debug!("Sending http request: {:?}", req);
    req
}

#[derive(Debug, Clone)]
pub struct Client(reqwest::Client);

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    pub fn new() -> Self {
        let http = reqwest::Client::new();
        Client(http)
    }

    async fn request<U: IntoUrl>(&self, method: Method, url: U) -> RequestBuilder {
        request(&self.0, method, url)
    }

    pub async fn get_key_by_token(&self, token: String) -> Result<api::Key, Error> {
        let response = self
            .request(Method::GET, &format!("{}/v1/keys", KEYS_API_URL))
            .await
            .query(&[("token", token)])
            .send()
            .await?;
        let response = check_response_status(response).await?;
        let key = response.json().await?;
        Ok(key)
    }

    pub async fn redeem_key(&self, uuid: String, token: String) -> Result<api::Key, Error> {
        let response = self
            .request(
                Method::POST,
                &format!("{}/v1/keys/{}/redeem", KEYS_API_URL, uuid),
            )
            .await
            .json(&json!({
                "token": token,
            }))
            .send()
            .await?;
        let response = check_response_status(response).await?;
        let key = response.json().await?;
        Ok(key)
    }
}
