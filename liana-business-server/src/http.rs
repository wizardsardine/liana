//! HTTP request handling for auth and config endpoints

use crate::auth::AuthManager;
use crate::state::ServerState;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::net::TcpStream;
use std::sync::Arc;

/// Service configuration returned by /v1/desktop
#[derive(Debug, Serialize)]
pub struct ServiceConfig {
    pub auth_api_url: String,
    pub auth_api_public_key: String,
    pub backend_api_url: String,
}

/// Sign-in OTP request body
#[derive(Debug, Deserialize)]
pub struct SignInOtpRequest {
    pub email: String,
    #[serde(default)]
    #[allow(unused)]
    pub create_user: bool,
}

/// Verify OTP request body
#[derive(Debug, Deserialize)]
pub struct VerifyOtpRequest {
    pub email: String,
    pub token: String,
    #[serde(rename = "type")]
    #[allow(unused)]
    pub kind: String,
}

/// Refresh token request body
#[allow(unused)]
#[derive(Debug, Deserialize)]
pub struct RefreshTokenRequest {
    pub refresh_token: String,
}

/// Access token response
#[derive(Debug, Serialize)]
pub struct AccessTokenResponse {
    pub access_token: String,
    pub expires_at: i64,
    pub refresh_token: String,
}

/// The dummy API key (any key is accepted)
pub const DUMMY_API_KEY: &str = "dummy-api-key";

/// Send an HTTP response
pub fn send_response(stream: &mut TcpStream, status: u16, status_text: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {} {}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Access-Control-Allow-Methods: GET, POST, OPTIONS\r\n\
         Access-Control-Allow-Headers: Content-Type, apikey, User-Agent\r\n\
         \r\n\
         {}",
        status,
        status_text,
        body.len(),
        body
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

/// Handle HTTP request and return response
/// Returns true if request was handled, false if it should be passed to WebSocket
#[allow(clippy::too_many_arguments)]
pub fn handle_http_request(
    stream: &mut TcpStream,
    method: &str,
    path: &str,
    headers: &[(String, String)],
    body: &[u8],
    server_url: &str,
    auth: &Arc<AuthManager>,
    state: &ServerState,
) -> bool {
    log::debug!("HTTP {} {}", method, path);

    // Handle CORS preflight
    if method == "OPTIONS" {
        send_response(stream, 200, "OK", "");
        return true;
    }

    // Route request
    match (method, path) {
        ("GET", "/v1/desktop") => {
            // Use Host header if available, otherwise fall back to server_url
            let effective_url = headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("host"))
                .map(|(_, v)| format!("http://{}", v))
                .unwrap_or_else(|| server_url.to_string());

            let config = ServiceConfig {
                auth_api_url: effective_url.clone(),
                auth_api_public_key: DUMMY_API_KEY.to_string(),
                backend_api_url: effective_url,
            };
            let body = serde_json::to_string(&config).unwrap();
            send_response(stream, 200, "OK", &body);
            true
        }
        ("POST", path) if path.starts_with("/auth/v1/otp") => {
            // Parse request body
            log::debug!("OTP request body: {:?}", String::from_utf8_lossy(body));
            match serde_json::from_slice::<SignInOtpRequest>(body) {
                Ok(req) => {
                    if auth.is_registered(&req.email) {
                        log::info!("OTP requested for registered email: {}", req.email);
                        // In a real system, we'd send an email here
                        // The OTP code is printed at server startup
                        send_response(stream, 200, "OK", "{}");
                    } else {
                        log::warn!("OTP requested for unknown email: {}", req.email);
                        // Still return success to not leak which emails are registered
                        send_response(stream, 200, "OK", "{}");
                    }
                }
                Err(e) => {
                    log::error!("Failed to parse OTP request: {}", e);
                    send_response(
                        stream,
                        400,
                        "Bad Request",
                        &format!("{{\"error\": \"{}\"}}", e),
                    );
                }
            }
            true
        }
        ("POST", "/auth/v1/verify") => {
            // Parse request body
            log::debug!("Verify request body: {:?}", String::from_utf8_lossy(body));
            match serde_json::from_slice::<VerifyOtpRequest>(body) {
                Ok(req) => {
                    if let Some(user) = auth.validate_otp(&req.email, &req.token) {
                        log::info!(
                            "OTP verified for email: {} (role: {:?})",
                            req.email,
                            user.role
                        );
                        // Look up user UUID from state
                        let users = state.users.lock().unwrap();
                        let user_uuid = users
                            .values()
                            .find(|u| u.email == req.email)
                            .map(|u| u.uuid);
                        drop(users);

                        match user_uuid {
                            Some(uuid) => {
                                // Include UUID in token for user identification
                                let response = AccessTokenResponse {
                                    access_token: format!("access-token-{}", uuid),
                                    expires_at: chrono::Utc::now().timestamp() + 3600, // 1 hour
                                    refresh_token: format!(
                                        "refresh-token-{}",
                                        uuid::Uuid::new_v4()
                                    ),
                                };
                                let body = serde_json::to_string(&response).unwrap();
                                send_response(stream, 200, "OK", &body);
                            }
                            None => {
                                log::error!("User {} not found in state", req.email);
                                send_response(
                                    stream,
                                    500,
                                    "Internal Server Error",
                                    "{\"error\": \"User not found\"}",
                                );
                            }
                        }
                    } else {
                        log::warn!("Invalid OTP code '{}' for email: {}", req.token, req.email);
                        send_response(
                            stream,
                            401,
                            "Unauthorized",
                            "{\"error\": \"Invalid OTP code\"}",
                        );
                    }
                }
                Err(e) => {
                    log::error!("Failed to parse verify request: {}", e);
                    send_response(
                        stream,
                        400,
                        "Bad Request",
                        &format!("{{\"error\": \"{}\"}}", e),
                    );
                }
            }
            true
        }
        ("POST", path) if path.starts_with("/auth/v1/token") => {
            // Refresh token endpoint
            match serde_json::from_slice::<RefreshTokenRequest>(body) {
                Ok(_req) => {
                    log::info!("Token refresh requested");
                    let response = AccessTokenResponse {
                        access_token: format!("access-token-{}", uuid::Uuid::new_v4()),
                        expires_at: chrono::Utc::now().timestamp() + 3600, // 1 hour
                        refresh_token: format!("refresh-token-{}", uuid::Uuid::new_v4()),
                    };
                    let body = serde_json::to_string(&response).unwrap();
                    send_response(stream, 200, "OK", &body);
                }
                Err(e) => {
                    log::error!("Failed to parse refresh request: {}", e);
                    send_response(
                        stream,
                        400,
                        "Bad Request",
                        &format!("{{\"error\": \"{}\"}}", e),
                    );
                }
            }
            true
        }
        _ => {
            // Unknown endpoint
            send_response(stream, 404, "Not Found", "{\"error\": \"Not found\"}");
            true
        }
    }
}
