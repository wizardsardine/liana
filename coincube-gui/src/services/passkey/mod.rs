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
/// The distinguishable outcomes are therefore distinct types, not one string:
/// [`PasskeyError::Cancelled`], [`PasskeyError::PrfNotSupported`],
/// [`PasskeyError::CredentialNotFound`] and
/// [`PasskeyError::AppIdentityMissing`]. [`PasskeyError::user_message`] is what
/// the UI shows for each; none of them says the Cube is lost.
///
/// The split between the last two is the reason this is a taxonomy and not a
/// severity scale. Both are "the ceremony did not happen", and they are told
/// apart only by the detail string, but one asks the user to sign in to an Apple
/// ID and the other tells them the build is at fault. Getting that backwards
/// wastes the user's time on a machine that is working correctly.
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
    /// macOS refused the ceremony because the running process carries no
    /// application identifier: `ASAuthorizationError` 1004 with the detail
    /// "The calling process does not have an application identifier".
    ///
    /// This is a **build or installation fault, never a user fault**, and it is
    /// split out of [`Self::CredentialNotFound`] for exactly that reason. Two
    /// distinct things produce it:
    ///
    /// - A bare binary: `cargo run` and `target/*/coincube` are not bundles and
    ///   have no code-signed app identity at all.
    /// - A signed bundle whose entitlements omit
    ///   `com.apple.application-identifier`. The provisioning profile granting
    ///   the key is not enough — an entitlement has to be *requested* in
    ///   `contrib/release/macos/coincube.entitlements` to reach the signature.
    ///   Every build before 2026-08-07 was in this state, including notarized
    ///   release artifacts, and nothing else objected: the signature was valid,
    ///   the hardened runtime was on, `keychain-access-groups` worked, `spctl`
    ///   and notarization passed.
    ///
    /// Folding this into `CredentialNotFound` told users to check their Apple ID
    /// and iCloud Keychain, which is advice they cannot act on and which sends
    /// them looking for a fault on their own machine. It cost two rounds of
    /// misdiagnosis on 2026-08-07 before the entitlement was found.
    AppIdentityMissing(String),
}

impl PasskeyError {
    /// Copy for the user when a passkey ceremony **unlocks** an existing Cube.
    ///
    /// Per **I12**, none of these may read as a lost or corrupt Cube, and each
    /// of the three passkey-specific outcomes says something different about
    /// what to do next.
    ///
    /// Do not use this for a *registration* failure — see
    /// [`Self::registration_message`]. Every message here speaks about "this
    /// Cube" and offers the Recovery Kit, which during creation describes a
    /// Cube that does not exist and a Kit the user cannot have.
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
            Self::AppIdentityMissing(_) => {
                "This copy of COINCUBE can't use passkeys: macOS requires an app identity \
                 that this build doesn't carry, so it can't run the Touch ID check. \
                 Nothing is wrong with this Cube and nothing on this Mac needs changing — \
                 it takes a fixed build. Reinstall COINCUBE from an official release to \
                 unlock with your passkey, or restore this Cube from your Recovery Kit \
                 and its recovery password."
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

    /// Copy for the user when the ceremony was **registering** a passkey for a
    /// Cube being created.
    ///
    /// The registration mirror of [`Self::user_message`], and the reason the
    /// two cannot be one function: at registration time nothing has been
    /// written to disk, so the only true statement is "your Cube was not
    /// created". The unlock copy instead reassures the user that "the Cube
    /// itself is fine" and points them at their Recovery Kit — during creation
    /// that names a Cube that does not exist and sends them looking for a Kit
    /// they were never issued.
    ///
    /// Every arm therefore says the Cube was not created, offers the PIN path
    /// as the way forward, and mentions no Recovery Kit.
    ///
    /// # The failure a developer will hit first
    ///
    /// A build with no application identifier — a bare `cargo run` binary, or a
    /// signed bundle whose entitlements omit
    /// `com.apple.application-identifier` — fails every registration with
    /// `ASAuthorizationError` 1004. That is [`Self::AppIdentityMissing`], and it
    /// says so: the copy names the build as the fault and does not send anyone
    /// to check their Apple ID.
    ///
    /// It used to land in [`Self::CredentialNotFound`], whose copy asks the user
    /// to verify their Apple ID and iCloud Keychain. Both are irrelevant here,
    /// and the misdirection is expensive — it cost two rounds of diagnosis on
    /// 2026-08-07 while the real cause sat in the entitlements file. If a new
    /// 1004 detail string appears, classify it rather than letting it fall back;
    /// see `classify_authorization_error` in [`super::macos`].
    pub fn registration_message(&self) -> String {
        match self {
            Self::Cancelled => "Passkey setup was cancelled. Your Cube was not created — \
                 try again, or create it with a PIN instead."
                .to_string(),
            Self::PrfNotSupported => {
                "This Mac's passkeys can't produce the key material COINCUBE needs, so \
                 this Cube can't be secured with one. Your Cube was not created — \
                 create it with a PIN instead."
                    .to_string()
            }
            Self::CredentialNotFound(_) => {
                "This Mac couldn't create a passkey. Check that you're signed in to your \
                 Apple ID with iCloud Keychain on, then try again — or create this Cube \
                 with a PIN instead. Your Cube was not created."
                    .to_string()
            }
            Self::AppIdentityMissing(_) => {
                "This copy of COINCUBE can't create passkeys: macOS requires an app \
                 identity that this build doesn't carry. Nothing on this Mac needs \
                 changing and trying again won't help — it takes a fixed build. Your Cube \
                 was not created: create it with a PIN instead, or reinstall COINCUBE from \
                 an official release."
                    .to_string()
            }
            Self::InvalidPrfOutput => {
                "The new passkey returned key material COINCUBE couldn't use, so your \
                 Cube was not created. Try again, or create it with a PIN instead."
                    .to_string()
            }
            other => format!(
                "Couldn't set up a passkey, so your Cube was not created. Try again, \
                 or create it with a PIN instead. ({other})"
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
            Self::AppIdentityMissing(detail) => write!(
                f,
                "this build carries no application identifier, so macOS refused the \
                 passkey ceremony (fix: com.apple.application-identifier in \
                 contrib/release/macos/coincube.entitlements): {}",
                detail
            ),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every classified outcome, so a new variant cannot be added without a
    /// decision about what it says at registration time.
    fn every_error() -> Vec<PasskeyError> {
        vec![
            PasskeyError::Cancelled,
            PasskeyError::PrfNotSupported,
            PasskeyError::CredentialNotFound("code 1004".to_string()),
            PasskeyError::AppIdentityMissing(
                "The calling process does not have an application identifier.".to_string(),
            ),
            PasskeyError::InvalidPrfOutput,
            PasskeyError::CeremonyFailed("boom".to_string()),
            PasskeyError::WebviewFailed("boom".to_string()),
            PasskeyError::InvalidResponse("boom".to_string()),
        ]
    }

    /// The registration mirror of **I12**. Nothing exists yet when registration
    /// fails, so no message may describe a Cube the user has or a backup they
    /// were never issued — and every one must say the Cube was not created, or
    /// the user is left unable to tell whether a half-made Cube is sitting on
    /// disk.
    #[test]
    fn registration_failures_never_name_a_cube_that_does_not_exist() {
        for error in every_error() {
            let msg = error.registration_message();
            let lower = msg.to_lowercase();

            assert!(
                !lower.contains("recovery kit"),
                "registration copy sends the user to a Recovery Kit they \
                 cannot have yet: {}",
                msg
            );
            assert!(
                !lower.contains("this cube's passkey")
                    && !lower.contains("the cube itself is fine")
                    && !lower.contains("stayed locked"),
                "registration copy talks about an existing Cube: {}",
                msg
            );
            assert!(
                lower.contains("was not created"),
                "registration copy must say the Cube was not created, so the \
                 user knows nothing was left behind: {}",
                msg
            );
            // The I12 rule itself still holds on this side.
            for forbidden in ["lost", "gone", "corrupt", "deleted", "destroyed"] {
                assert!(
                    !lower.contains(forbidden),
                    "registration copy says {:?}, which reads as a \
                     wallet the user has lost: {}",
                    forbidden,
                    msg
                );
            }
        }
    }

    /// The two message sets are for different moments and must not be swapped
    /// by a later refactor that "deduplicates" them.
    #[test]
    fn registration_and_unlock_copy_differ_for_every_error() {
        for error in every_error() {
            assert_ne!(
                error.registration_message(),
                error.user_message(),
                "registration and unlock copy are identical for {:?}",
                error
            );
        }
    }

    /// A build fault must never be dressed up as something the user can fix on
    /// their own machine.
    ///
    /// This is the regression guard for the 2026-08-07 misdiagnosis: the
    /// no-application-identifier refusal was folded into `CredentialNotFound`,
    /// whose copy asks the user to check their Apple ID and iCloud Keychain.
    /// Both were working. The fault was a missing entitlement, and the copy sent
    /// two rounds of debugging at the wrong machine.
    #[test]
    fn app_identity_failure_blames_the_build_not_the_user() {
        let err = PasskeyError::AppIdentityMissing(
            "The calling process does not have an application identifier.".to_string(),
        );

        for msg in [err.registration_message(), err.user_message()] {
            let lower = msg.to_lowercase();
            // Not even to rule them out. A first draft of this copy read "not in
            // your Mac and not in your Apple ID", which this assertion rejected
            // — correctly: naming the thing is what sends someone to go check
            // it, and a reader skimming a red error block takes away the nouns,
            // not the negations.
            for forbidden in ["apple id", "icloud"] {
                assert!(
                    !lower.contains(forbidden),
                    "copy points the user at {:?}, which is not the fault and \
                     cannot be acted on: {}",
                    forbidden,
                    msg
                );
            }
            assert!(
                lower.contains("build"),
                "copy must name the build as the fault, or the user has nothing \
                 actionable: {}",
                msg
            );
        }

        // And it must not be confusable with the variant it was split out of —
        // that identity is the whole point of the split.
        let credential = PasskeyError::CredentialNotFound("code 1004".to_string());
        assert_ne!(
            err.registration_message(),
            credential.registration_message()
        );
        assert_ne!(err.user_message(), credential.user_message());
    }

    /// Cancelling is the most common failure and the one most likely to be
    /// misread; it stays distinguishable from the rest here too.
    #[test]
    fn cancelled_registration_is_distinguishable() {
        let cancelled = PasskeyError::Cancelled.registration_message();
        for other in every_error()
            .into_iter()
            .filter(|e| e != &PasskeyError::Cancelled)
        {
            assert_ne!(cancelled, other.registration_message());
        }
    }
}
