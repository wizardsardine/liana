//! Passkey ceremony service for WebAuthn + PRF-based master key derivation.
//!
//! On macOS, this uses the native AuthenticationServices framework via
//! `objc2-authentication-services` (see [`macos`] submodule). On other
//! platforms, it falls back to an embedded webview pointing at the hosted
//! ceremony page at `coincube.io/passkey`.

#[cfg(target_os = "macos")]
pub mod macos;

use std::sync::{mpsc, Arc};

use zeroize::Zeroizing;

/// Base URL for the passkey ceremony page.
const CEREMONY_BASE_URL: &str = "https://coincube.io/passkey";

/// Relying Party ID — must match the ceremony page's domain.
pub const RP_ID: &str = "coincube.io";

/// Errors that can occur during a passkey ceremony.
#[derive(Debug, Clone)]
pub enum PasskeyError {
    /// The ceremony page reported an error via IPC.
    CeremonyFailed(String),
    /// The webview failed to initialize.
    WebviewFailed(String),
    /// The IPC response could not be parsed.
    InvalidResponse(String),
    /// The PRF output was not the expected 32 bytes.
    InvalidPrfOutput,
    /// The user cancelled the ceremony.
    Cancelled,
    /// The PRF extension is not supported on this platform.
    PrfNotSupported,
}

impl std::fmt::Display for PasskeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CeremonyFailed(msg) => write!(f, "Passkey ceremony failed: {}", msg),
            Self::WebviewFailed(msg) => write!(f, "Webview initialization failed: {}", msg),
            Self::InvalidResponse(msg) => write!(f, "Invalid ceremony response: {}", msg),
            Self::InvalidPrfOutput => write!(f, "PRF output is not 32 bytes"),
            Self::Cancelled => write!(f, "Passkey ceremony was cancelled"),
            Self::PrfNotSupported => write!(f, "PRF extension is not supported on this platform"),
        }
    }
}

/// Result of a successful passkey registration ceremony.
#[derive(Clone)]
pub struct PasskeyRegistration {
    /// Base64-encoded WebAuthn credential ID.
    pub credential_id: String,
    /// 32-byte PRF output (secret — zeroized on drop).
    pub prf_output: Zeroizing<[u8; 32]>,
}

/// Result of a successful passkey authentication ceremony.
#[derive(Clone)]
pub struct PasskeyAuthentication {
    /// 32-byte PRF output (secret — zeroized on drop).
    pub prf_output: Zeroizing<[u8; 32]>,
}

/// Parsed IPC message from the ceremony page.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "type")]
enum CeremonyIpcMessage {
    #[serde(rename = "register_success")]
    RegisterSuccess {
        credential_id: String,
        prf_output: Vec<u8>,
    },
    #[serde(rename = "authenticate_success")]
    AuthenticateSuccess { prf_output: Vec<u8> },
    #[serde(rename = "error")]
    Error { message: String },
}

/// The kind of passkey ceremony to perform.
#[derive(Debug, Clone)]
pub enum CeremonyMode {
    /// Register a new passkey for a new Cube.
    Register { user_id: String, user_name: String },
    /// Authenticate with an existing passkey to open a Cube.
    Authenticate { credential_id: String },
}

/// Percent-encode a string for use in URL query parameters.
fn url_encode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
            _ => {
                result.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    result
}

impl CeremonyMode {
    /// Build the full URL for the ceremony page.
    pub fn url(&self) -> String {
        match self {
            Self::Register { user_id, user_name } => {
                format!(
                    "{}?mode=register&user_id={}&user_name={}",
                    CEREMONY_BASE_URL,
                    url_encode(user_id),
                    url_encode(user_name),
                )
            }
            Self::Authenticate { credential_id } => {
                format!(
                    "{}?mode=authenticate&credential_id={}",
                    CEREMONY_BASE_URL,
                    url_encode(credential_id),
                )
            }
        }
    }
}

/// Shared state for receiving IPC messages from the webview.
///
/// The sender is captured by the webview's IPC handler closure;
/// the receiver is polled by the iced subscription.
pub struct PasskeyCeremonyChannel {
    sender: mpsc::Sender<String>,
    receiver: mpsc::Receiver<String>,
}

impl Default for PasskeyCeremonyChannel {
    fn default() -> Self {
        Self::new()
    }
}

impl PasskeyCeremonyChannel {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self { sender, receiver }
    }

    /// Get a clone of the sender for use in the IPC handler closure.
    pub fn sender(&self) -> mpsc::Sender<String> {
        self.sender.clone()
    }

    /// Try to receive an IPC message (non-blocking).
    pub fn try_recv(&self) -> Option<String> {
        self.receiver.try_recv().ok()
    }
}

/// Manages the passkey ceremony webview lifecycle.
///
/// Usage:
/// 1. Create with `PasskeyCeremony::new(mode)`
/// 2. Call `create_webview(window_id)` once the window ID is extracted
/// 3. Poll `try_recv_result()` in the iced subscription
/// 4. Drop to clean up the webview
pub struct PasskeyCeremony {
    pub mode: CeremonyMode,
    pub webview_manager: iced_wry::IcedWebviewManager,
    pub active_webview: Option<iced_wry::IcedWebview>,
    channel: Arc<PasskeyCeremonyChannel>,
}

impl PasskeyCeremony {
    pub fn new(mode: CeremonyMode) -> Self {
        Self {
            mode,
            webview_manager: iced_wry::IcedWebviewManager::new(),
            active_webview: None,
            #[allow(clippy::arc_with_non_send_sync)]
            channel: Arc::new(PasskeyCeremonyChannel::new()),
        }
    }

    /// Create the webview and start the ceremony.
    ///
    /// Returns `true` if the webview was created successfully.
    pub fn create_webview(&mut self, window_id: iced_wry::ExtractedWindowId) -> bool {
        let url = self.mode.url();
        let tx = self.channel.sender();

        let attrs = iced_wry::wry::WebViewAttributes {
            url: Some(url),
            incognito: true,
            devtools: cfg!(debug_assertions),
            ipc_handler: Some(Box::new(move |req| {
                let body = req.body().clone();
                let _ = tx.send(body);
            })),
            ..Default::default()
        };

        match self.webview_manager.new_webview(attrs, window_id) {
            Some(active) => {
                self.active_webview = Some(active);
                true
            }
            None => false,
        }
    }

    /// Poll for a ceremony result (non-blocking).
    ///
    /// Returns `Some(Ok(...))` on success, `Some(Err(...))` on failure,
    /// or `None` if no result yet.
    pub fn try_recv_result(&self) -> Option<Result<CeremonyOutcome, PasskeyError>> {
        let raw = self.channel.try_recv()?;

        let parsed: CeremonyIpcMessage = match serde_json::from_str(&raw) {
            Ok(msg) => msg,
            Err(e) => {
                return Some(Err(PasskeyError::InvalidResponse(format!(
                    "Failed to parse IPC: {}",
                    e
                ))))
            }
        };

        Some(match parsed {
            CeremonyIpcMessage::RegisterSuccess {
                credential_id,
                prf_output,
            } => {
                if prf_output.len() != 32 {
                    Err(PasskeyError::InvalidPrfOutput)
                } else {
                    let mut arr = [0u8; 32];
                    arr.copy_from_slice(&prf_output);
                    Ok(CeremonyOutcome::Registered(PasskeyRegistration {
                        credential_id,
                        prf_output: Zeroizing::new(arr),
                    }))
                }
            }
            CeremonyIpcMessage::AuthenticateSuccess { prf_output } => {
                if prf_output.len() != 32 {
                    Err(PasskeyError::InvalidPrfOutput)
                } else {
                    let mut arr = [0u8; 32];
                    arr.copy_from_slice(&prf_output);
                    Ok(CeremonyOutcome::Authenticated(PasskeyAuthentication {
                        prf_output: Zeroizing::new(arr),
                    }))
                }
            }
            CeremonyIpcMessage::Error { message } => {
                if message.contains("cancelled") || message.contains("NotAllowedError") {
                    Err(PasskeyError::Cancelled)
                } else if message.contains("PRF") || message.contains("not supported") {
                    Err(PasskeyError::PrfNotSupported)
                } else {
                    Err(PasskeyError::CeremonyFailed(message))
                }
            }
        })
    }

    /// Clean up the webview.
    pub fn close(&mut self) {
        if let Some(active) = self.active_webview.take() {
            self.webview_manager.clear_view(&active);
        }
    }
}

impl Drop for PasskeyCeremony {
    fn drop(&mut self) {
        self.close();
    }
}

/// The outcome of a successful ceremony.
#[derive(Clone)]
pub enum CeremonyOutcome {
    Registered(PasskeyRegistration),
    Authenticated(PasskeyAuthentication),
}

// Manual Debug impl to avoid printing PRF output.
impl std::fmt::Debug for CeremonyOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Registered(r) => f
                .debug_struct("Registered")
                .field("credential_id", &r.credential_id)
                .field("prf_output", &"<redacted>")
                .finish(),
            Self::Authenticated(_) => f
                .debug_struct("Authenticated")
                .field("prf_output", &"<redacted>")
                .finish(),
        }
    }
}
