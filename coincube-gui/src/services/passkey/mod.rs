//! Passkey ceremony service for WebAuthn + PRF-based master key derivation.
//!
//! On macOS, this uses the native AuthenticationServices framework via
//! `objc2-authentication-services` (see [`macos`] submodule). On other
//! platforms, it falls back to an embedded webview pointing at the hosted
//! ceremony page at `coincube.io/passkey`.
//!
//! # Registration and unlock, on macOS
//!
//! Both halves work on macOS: [`macos::NativePasskeyCeremony::register`] mints
//! the credential at Cube creation, and
//! [`macos::NativePasskeyCeremony::authenticate`] re-derives the same master
//! seed at every unlock (`crate::passkey_unlock`) and whenever the Recovery Kit
//! or the Backup-Master-Seed screen needs the seed in hand.
//!
//! The credential is **synced**, not device-bound —
//! `ASAuthorizationPlatformPublicKeyCredentialProvider` produces an
//! iCloud-Keychain passkey, so the same Cube opens on any Mac signed into the
//! same Apple ID. That is deliberate
//! (`company-brain/decisions/2026-08-04-tenshu-passkey-apple-id-bound.md`), and
//! it is *not* a recovery promise: iCloud Keychain can be off, and we can
//! neither require nor detect that. The promise is the Recovery Kit, which
//! carries the encrypted master seed for a passkey Cube (**I11**).
//!
//! Windows and Linux have no unlock path yet, so the passkey option is not
//! offered there — see
//! `company-brain/decisions/2026-08-03-passkey-macos-only-for-now.md`.

#[cfg(target_os = "macos")]
pub mod macos;
pub mod reauth;
pub mod security_key;
#[cfg(windows)]
pub mod windows;

/// PRF eval input for Tenshu's Cube master seed:
/// `SHA-256("coincube-tenshu/v1/master-seed")`.
///
/// **Treat as stable.** Registered in the PRF domain registry
/// (`company-brain/decisions/2026-08-01-webauthn-prf-domain-registry.md`).
/// Every backend — platform passkey on macOS or Windows, or a FIDO2 security
/// key — feeds the authenticator these exact 32 bytes, so one credential
/// derives one seed regardless of which code path ran. `coincube-core`'s test
/// suite pins the same value.
pub const PRF_SALT: &[u8] = &[
    0x41, 0x68, 0xdc, 0x1c, 0xd4, 0x88, 0xa3, 0x7d, 0xe3, 0x0c, 0xdf, 0xca, 0x87, 0x42, 0x13, 0x02,
    0x42, 0xcd, 0xee, 0x47, 0x28, 0x32, 0x42, 0xfe, 0xab, 0xcb, 0x98, 0x11, 0xf2, 0x39, 0x9c, 0x72,
];

use std::sync::{mpsc, Arc};

use zeroize::Zeroizing;

use crate::feature_flags::non_empty_or;

/// Base URL for the passkey ceremony page.
///
/// Configured at build time via `COINCUBE_PASSKEY_CEREMONY_URL` (forwarded by
/// `build.rs` from `.env`, or exported directly by CI). Defaults to a local dev
/// URL so non-production builds don't point at a non-existent hosted endpoint.
/// Production deploys must set this to the actual ceremony page URL.
///
/// An **empty** value is treated as unset and falls back to the default. See
/// [`non_empty_or`]: a bare `match option_env!(..)` would bind `Some("")` to the
/// value arm and hand callers the empty string, which CI produces routinely.
pub const CEREMONY_BASE_URL: &str = non_empty_or(
    option_env!("COINCUBE_PASSKEY_CEREMONY_URL"),
    "http://localhost:8080/passkey",
);

/// Relying Party ID — must match the ceremony page's domain.
///
/// Configured at build time via `COINCUBE_PASSKEY_RP_ID`. Production deploys
/// must set this to the actual domain hosting the ceremony page.
///
/// An **empty** value is treated as unset and falls back to `localhost`, for the
/// reason given on [`CEREMONY_BASE_URL`]. Falling back is the safe half of the
/// fix, not the whole of it — `localhost` never matches the AASA at
/// coincube.io either, so a release build that reached this fallback would fail
/// every ceremony at runtime with nothing upstream to catch it. That is what the
/// `const _: () = assert!` in [`crate::feature_flags`] is for: with the passkey
/// path on, an unset or empty RP id fails the build instead.
pub const RP_ID: &str = non_empty_or(option_env!("COINCUBE_PASSKEY_RP_ID"), "localhost");

/// Errors that can occur during a passkey ceremony.
///
/// # Invariant I12
///
/// A failed, cancelled, or unsupported passkey assertion is **never** reported
/// as a lost or corrupt Cube. That is the passkey-path mirror of I7: a user who
/// taps Cancel on a Touch ID prompt must not walk away believing their wallet
/// is gone.
///
/// Three outcomes are therefore distinct types, not one string:
/// [`PasskeyError::Cancelled`], [`PasskeyError::PrfNotSupported`] and
/// [`PasskeyError::CredentialNotFound`]. [`PasskeyError::user_message`] is what
/// the UI shows for each; none of them says the Cube is lost.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// The PRF extension is not supported on this platform, or the credential
    /// that answered was registered without it. Never derive a seed from
    /// anything else in this case: it would produce a valid-looking wallet that
    /// is not the user's.
    PrfNotSupported,
    /// This Cube's credential could not be offered — the Cube has no stored
    /// credential id, or the platform authenticator has no such passkey.
    ///
    /// On macOS this covers the `ASAuthorizationError` codes that mean "there
    /// was nothing here to authorise with" (`Unknown`, `NotHandled`, `Failed`,
    /// `NotInteractive`). They cannot be told apart from each other at the
    /// framework level, and they call for the same thing from the user: either
    /// sign in to the Apple ID that holds the passkey, or restore from the
    /// Recovery Kit. The underlying detail is preserved for the log.
    CredentialNotFound(String),
}

impl PasskeyError {
    /// Copy for the user. Per **I12**, none of these may read as a lost or
    /// corrupt Cube, and each of the three passkey-specific outcomes says
    /// something different about what to do next.
    pub fn user_message(&self) -> String {
        match self {
            Self::Cancelled => "Touch ID was cancelled, so this Cube stayed locked. \
                 Your Cube and its funds are untouched — try again when you're ready."
                .to_string(),
            Self::PrfNotSupported => {
                "This passkey can't produce the key material COINCUBE needs, so it can't \
                 unlock this Cube. Nothing is wrong with the Cube itself: restore it \
                 from your Recovery Kit and its recovery password."
                    .to_string()
            }
            Self::CredentialNotFound(_) => {
                "This Mac couldn't find this Cube's passkey. Sign in to the Apple ID \
                 that created it (with iCloud Keychain on), or restore this Cube from \
                 your Recovery Kit and its recovery password. The Cube itself is fine."
                    .to_string()
            }
            Self::InvalidPrfOutput => {
                "This Cube's passkey returned key material COINCUBE couldn't use. The \
                 Cube itself is fine — restore it from your Recovery Kit and its \
                 recovery password."
                    .to_string()
            }
            other => format!(
                "Couldn't complete the passkey check, so this Cube stayed locked. \
                 The Cube itself is fine — try again, or restore it from your Recovery \
                 Kit. ({other})"
            ),
        }
    }
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
            Self::CredentialNotFound(detail) => {
                write!(f, "This Cube's passkey was not available: {}", detail)
            }
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
///
/// `prf_output` is sent by the ceremony page as a JSON array of byte values
/// (e.g. `[0,1,2,...,31]`). Exact 32-byte length is validated after
/// deserialization before converting into `Zeroizing<[u8; 32]>`.
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
                let prf_output = Zeroizing::new(prf_output);
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
                let prf_output = Zeroizing::new(prf_output);
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
