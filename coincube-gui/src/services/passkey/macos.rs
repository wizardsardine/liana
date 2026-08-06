//! Native macOS passkey ceremony via the AuthenticationServices framework.
//!
//! This implementation calls Apple's `ASAuthorizationController` directly
//! through `objc2` bindings. It bypasses the WebAuthn-via-WebView path
//! (which is broken in WKWebView without the browser entitlement) and uses
//! the platform authenticator (Touch ID / Face ID via iCloud Keychain).
//!
//! # Minimum macOS version
//!
//! **macOS 15 (Sequoia)**, not 14. The module header used to say Sonoma, which
//! was wrong and would have become a support statement: the
//! `ASAuthorizationPublicKeyCredentialPRF…` input and output types this file
//! depends on are marked `macos(15.0)` in the SDK. On macOS 14 the selectors
//! are absent, `setPrf:` is an unrecognised-selector crash rather than a
//! graceful degradation, so the version floor is load-bearing.
//!
//! Module gating lives on the `pub mod macos;` declaration in
//! `services/passkey/mod.rs` — a redundant inner `#![cfg]` would be a
//! clippy `duplicated_attributes` warning.

#![allow(unexpected_cfgs)]

use std::cell::OnceCell;
use std::sync::mpsc;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_authentication_services::{
    ASAuthorization, ASAuthorizationController, ASAuthorizationControllerDelegate,
    ASAuthorizationControllerPresentationContextProviding,
    ASAuthorizationPlatformPublicKeyCredentialAssertion,
    ASAuthorizationPlatformPublicKeyCredentialDescriptor,
    ASAuthorizationPlatformPublicKeyCredentialProvider,
    ASAuthorizationPlatformPublicKeyCredentialRegistration,
    ASAuthorizationPublicKeyCredentialPRFAssertionInput,
    ASAuthorizationPublicKeyCredentialPRFAssertionInputValues,
    ASAuthorizationPublicKeyCredentialPRFRegistrationInput, ASPublicKeyCredential,
};
// IMPORTANT: ASPresentationAnchor is `objc2::runtime::NSObject`, NOT
// `objc2_foundation::NSObject`. To avoid name collisions in `define_class!`,
// we use the runtime NSObject as the superclass and skip importing
// objc2_foundation::NSObject entirely.
use objc2::runtime::NSObject;
use objc2_foundation::{
    NSArray, NSData, NSError, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
};

use rand::RngCore;
use zeroize::Zeroizing;

/// Result delivered by the delegate to the polling caller.
#[derive(Debug, Clone)]
pub enum NativeOutcome {
    Registered {
        credential_id: Vec<u8>,
        prf_output: Zeroizing<[u8; 32]>,
    },
    Authenticated {
        prf_output: Zeroizing<[u8; 32]>,
    },
    /// A **classified** failure, not a string. Invariant **I12** turns on the
    /// caller being able to tell a cancelled Touch ID prompt apart from a
    /// PRF-less authenticator apart from an absent credential — collapsing them
    /// into one message is how a user concludes their wallet is gone.
    Failed(super::PasskeyError),
}

/// `ASAuthorizationError` codes, from `AuthenticationServices/ASFoundation.h`.
/// Declared here rather than imported because `objc2-authentication-services`
/// exposes the error domain but not the code enum.
mod as_error {
    pub const UNKNOWN: isize = 1000;
    pub const CANCELED: isize = 1001;
    pub const INVALID_RESPONSE: isize = 1002;
    pub const NOT_HANDLED: isize = 1003;
    pub const FAILED: isize = 1004;
    pub const NOT_INTERACTIVE: isize = 1005;
}

/// Map an `ASAuthorizationController` failure onto the I12 taxonomy.
///
/// `Canceled` is the only code with an unambiguous meaning, and it is the one
/// that matters most: it is by far the most common failure and the one most
/// likely to be misread as a lost Cube.
///
/// The rest — `Unknown`, `NotHandled`, `Failed`, `NotInteractive` — are how
/// macOS reports "there was nothing here to authorise with" (no matching
/// credential for this RP, the Apple ID not signed in, iCloud Keychain off).
/// They are not distinguishable from one another at the framework level and
/// they call for the same remedy, so they share
/// [`super::PasskeyError::CredentialNotFound`], which keeps the raw detail for
/// the log.
fn classify_authorization_error(code: isize, detail: String) -> super::PasskeyError {
    use super::PasskeyError;
    match code {
        as_error::CANCELED => PasskeyError::Cancelled,
        as_error::UNKNOWN
        | as_error::NOT_HANDLED
        | as_error::FAILED
        | as_error::NOT_INTERACTIVE => PasskeyError::CredentialNotFound(detail),
        as_error::INVALID_RESPONSE => PasskeyError::InvalidResponse(detail),
        _ => PasskeyError::CeremonyFailed(detail),
    }
}

/// Whether this machine can actually run a PRF ceremony.
///
/// The version floor in the module header is a **compile-time** statement about
/// the SDK; this is the runtime half of it. `objc2` resolves
/// `ASAuthorizationPublicKeyCredentialPRF…` lazily by selector, so a binary
/// built against the macOS 15 SDK links and launches fine on macOS 14 — and
/// then `setPrf:` is an unrecognised selector, which is a **crash**, not a
/// graceful degradation.
///
/// Probing for the class turns that into a capability answer the caller can
/// act on. It is checked before the request is built, so the failure arrives as
/// [`super::PasskeyError::PrfNotSupported`] and reads as "this Mac can't do
/// this" rather than as a lost Cube (**I12**).
///
/// The registration input class is the one probed: it appeared in the same SDK
/// as the assertion input, and both ceremonies need both.
pub fn prf_supported() -> bool {
    // A literal `c"…"` would be cleaner, but this crate is on an edition
    // without C-string literals.
    const NAME: &[u8] = b"ASAuthorizationPublicKeyCredentialPRFRegistrationInput\0";
    std::ffi::CStr::from_bytes_with_nul(NAME)
        .ok()
        .and_then(objc2::runtime::AnyClass::get)
        .is_some()
}

/// Re-export of the shared, registered PRF eval input. Every backend must use
/// the same 32 bytes — see [`super::PRF_SALT`] for why, and for the history of
/// the Breez-borrowed constant it replaced.
use super::PRF_SALT;

/// Instance variables for the delegate.
struct DelegateIvars {
    /// Channel to send the result back to Rust async code.
    sender: OnceCell<mpsc::Sender<NativeOutcome>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "CoincubePasskeyDelegate"]
    #[ivars = DelegateIvars]
    struct PasskeyDelegate;

    unsafe impl NSObjectProtocol for PasskeyDelegate {}

    unsafe impl ASAuthorizationControllerDelegate for PasskeyDelegate {
        #[unsafe(method(authorizationController:didCompleteWithAuthorization:))]
        fn did_complete_with_authorization(
            &self,
            _controller: &ASAuthorizationController,
            authorization: &ASAuthorization,
        ) {
            let outcome = unsafe { extract_outcome(authorization) };
            if let Some(sender) = self.ivars().sender.get() {
                let _ = sender.send(outcome);
            }
        }

        #[unsafe(method(authorizationController:didCompleteWithError:))]
        fn did_complete_with_error(
            &self,
            _controller: &ASAuthorizationController,
            error: &NSError,
        ) {
            let desc = error.localizedDescription();
            let msg = desc.to_string();
            let code = error.code();
            let full = format!("{} (code {})", msg, code);
            let classified = classify_authorization_error(code, full);
            if let Some(sender) = self.ivars().sender.get() {
                let _ = sender.send(NativeOutcome::Failed(classified));
            }
        }
    }

    unsafe impl ASAuthorizationControllerPresentationContextProviding for PasskeyDelegate {
        #[unsafe(method_id(presentationAnchorForAuthorizationController:))]
        fn presentation_anchor_for_authorization_controller(
            &self,
            _controller: &ASAuthorizationController,
        ) -> Retained<NSObject> {
            // Return the app's key window (or main window as fallback) via raw
            // msg_send! to avoid pulling in objc2-app-kit (which conflicts with
            // the older version from iced/winit). The selectors here are
            // standard Cocoa: NSApplication.sharedApplication, then -keyWindow.
            unsafe {
                use objc2::class;
                use objc2::runtime::AnyObject;

                let mut window: *mut AnyObject = std::ptr::null_mut();

                let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
                if !app.is_null() {
                    window = msg_send![app, keyWindow];
                    if window.is_null() {
                        window = msg_send![app, mainWindow];
                    }
                    if window.is_null() {
                        let windows: *mut AnyObject = msg_send![app, windows];
                        if !windows.is_null() {
                            window = msg_send![windows, firstObject];
                        }
                    }
                } else {
                    tracing::warn!(
                        "NSApplication.sharedApplication returned nil; \
                         passkey presentation will use a fallback hidden window"
                    );
                }

                if window.is_null() {
                    if !app.is_null() {
                        tracing::warn!(
                            "No NSWindow available for passkey presentation; \
                             creating a fallback hidden window"
                        );
                    }
                    // Fallback: create a minimal hidden NSWindow so the
                    // authorization controller has a valid presentation anchor.
                    let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1.0, 1.0));
                    let alloc: *mut AnyObject = msg_send![class!(NSWindow), alloc];
                    window = msg_send![
                        alloc,
                        initWithContentRect: rect,
                        styleMask: 0u64,  // NSWindowStyleMaskBorderless
                        backing: 2u64,    // NSBackingStoreBuffered
                        defer: true,
                    ];
                }

                // Retain the window and return as Retained<RuntimeNSObject>
                // (which is what ASPresentationAnchor aliases to).
                let _: () = msg_send![window, retain];
                Retained::from_raw(window as *mut NSObject)
                    .expect("Failed to obtain or create NSWindow for passkey presentation")
            }
        }
    }
);

impl PasskeyDelegate {
    fn new(mtm: MainThreadMarker, sender: mpsc::Sender<NativeOutcome>) -> Retained<Self> {
        let cell = OnceCell::new();
        let _ = cell.set(sender);
        let ivars = DelegateIvars { sender: cell };
        let this = Self::alloc(mtm).set_ivars(ivars);
        unsafe { msg_send![super(this), init] }
    }
}

/// Narrow 32 bytes of PRF output out of a `first` value, or explain why not.
fn prf_bytes(first: &NSData) -> Result<Zeroizing<[u8; 32]>, String> {
    let bytes = Zeroizing::new(first.to_vec());
    if bytes.len() < 32 {
        return Err(format!(
            "PRF output too short: {} bytes (expected at least 32)",
            bytes.len()
        ));
    }
    let mut arr = Zeroizing::new([0u8; 32]);
    arr.copy_from_slice(&bytes[..32]);
    Ok(arr)
}

/// Extract the credential ID and PRF output from a successful authorization.
///
/// Handles **both** credential types. It used to downcast only to
/// `…CredentialRegistration`, so an assertion — the type every unlock produces
/// — fell through to "Unexpected credential type". That, plus the missing
/// assertion request itself, is why passkey Cubes could be created and never
/// opened.
unsafe fn extract_outcome(authorization: &ASAuthorization) -> NativeOutcome {
    let credential = unsafe { authorization.credential() };

    // ProtocolObject implements AsRef<AnyObject>, so we go through that.
    let any_obj: &objc2::runtime::AnyObject = credential.as_ref();

    if let Some(reg) =
        any_obj.downcast_ref::<ASAuthorizationPlatformPublicKeyCredentialRegistration>()
    {
        // credentialID() comes from the ASPublicKeyCredential trait.
        let credential_id = unsafe { reg.credentialID() }.to_vec();

        let Some(prf) = (unsafe { reg.prf() }) else {
            return NativeOutcome::Failed(super::PasskeyError::PrfNotSupported);
        };
        let Some(first) = (unsafe { prf.first() }) else {
            return NativeOutcome::Failed(super::PasskeyError::PrfNotSupported);
        };
        return match prf_bytes(&first) {
            Ok(prf_output) => NativeOutcome::Registered {
                credential_id,
                prf_output,
            },
            Err(e) => {
                tracing::error!("passkey registration PRF output unusable: {e}");
                NativeOutcome::Failed(super::PasskeyError::InvalidPrfOutput)
            }
        };
    }

    if let Some(assertion) =
        any_obj.downcast_ref::<ASAuthorizationPlatformPublicKeyCredentialAssertion>()
    {
        let Some(prf) = (unsafe { assertion.prf() }) else {
            // A credential registered without PRF, or an authenticator that
            // dropped the extension. This must be a clear capability error and
            // never a silently different seed — deriving from anything else
            // here would produce a valid-looking wallet that is not the user's.
            // I12: `PrfNotSupported` is its own outcome precisely so this can
            // never render as "your Cube is gone".
            return NativeOutcome::Failed(super::PasskeyError::PrfNotSupported);
        };
        // The assertion output's `first` is non-optional in the SDK.
        let first = unsafe { prf.first() };
        return match prf_bytes(&first) {
            Ok(prf_output) => NativeOutcome::Authenticated { prf_output },
            Err(e) => {
                tracing::error!("passkey assertion PRF output unusable: {e}");
                NativeOutcome::Failed(super::PasskeyError::InvalidPrfOutput)
            }
        };
    }

    NativeOutcome::Failed(super::PasskeyError::InvalidResponse(
        "Unexpected credential type returned by AuthenticationServices".to_string(),
    ))
}

/// Active passkey ceremony — holds the controller, delegate, and channel receiver.
///
/// Drop this to cancel the ceremony.
pub struct NativePasskeyCeremony {
    controller: Retained<ASAuthorizationController>,
    _delegate: Retained<PasskeyDelegate>,
    /// `None` once [`NativePasskeyCeremony::take_receiver`] has handed the
    /// channel to a waiter on another thread. The controller and delegate stay
    /// here regardless — they are the request, and dropping them cancels it.
    receiver: Option<mpsc::Receiver<NativeOutcome>>,
}

impl NativePasskeyCeremony {
    /// Start a passkey registration ceremony.
    ///
    /// `rp_id` is the relying party identifier (e.g. "coincube.io").
    /// `user_id` is the unique user identifier (Cube UUID as bytes).
    /// `user_name` is the display name shown in the system UI.
    pub fn register(rp_id: &str, user_id: &[u8], user_name: &str) -> Result<Self, String> {
        let mtm = MainThreadMarker::new()
            .ok_or_else(|| "Passkey ceremony must be started on the main thread".to_string())?;
        // Before anything is built: `setPrf:` below would be an unrecognised
        // selector on macOS 14, which crashes the app rather than failing.
        if !prf_supported() {
            return Err(super::PasskeyError::PrfNotSupported.user_message());
        }

        unsafe {
            // Build provider
            let rp_ns = NSString::from_str(rp_id);
            let provider =
                ASAuthorizationPlatformPublicKeyCredentialProvider::initWithRelyingPartyIdentifier(
                    ASAuthorizationPlatformPublicKeyCredentialProvider::alloc(),
                    &rp_ns,
                );

            // Random 32-byte challenge
            let mut challenge_bytes = [0u8; 32];
            rand::thread_rng().fill_bytes(&mut challenge_bytes);
            let challenge = NSData::with_bytes(&challenge_bytes);

            let user_id_data = NSData::with_bytes(user_id);
            let user_name_ns = NSString::from_str(user_name);

            // Create registration request
            let request = provider.createCredentialRegistrationRequestWithChallenge_name_userID(
                &challenge,
                &user_name_ns,
                &user_id_data,
            );

            // Attach PRF extension with our salt
            let salt_data = NSData::with_bytes(PRF_SALT);
            let prf_values =
                ASAuthorizationPublicKeyCredentialPRFAssertionInputValues::initWithSaltInput1_saltInput2(
                    ASAuthorizationPublicKeyCredentialPRFAssertionInputValues::alloc(),
                    &salt_data,
                    None,
                );
            let prf_input =
                ASAuthorizationPublicKeyCredentialPRFRegistrationInput::initWithInputValues(
                    ASAuthorizationPublicKeyCredentialPRFRegistrationInput::alloc(),
                    Some(&prf_values),
                );
            request.setPrf(Some(&prf_input));

            // Build the requests array. We need NSArray<ASAuthorizationRequest>.
            // The registration request is a subclass of ASAuthorizationRequest,
            // so we cast through the superclass relationship.
            let request_super: &objc2_authentication_services::ASAuthorizationRequest = &request;
            let requests_array = NSArray::from_slice(&[request_super]);

            let controller = ASAuthorizationController::initWithAuthorizationRequests(
                ASAuthorizationController::alloc(),
                &requests_array,
            );

            // Set up delegate with channel
            let (tx, rx) = mpsc::channel();
            let delegate = PasskeyDelegate::new(mtm, tx);
            let delegate_proto: &ProtocolObject<dyn ASAuthorizationControllerDelegate> =
                ProtocolObject::from_ref(&*delegate);
            controller.setDelegate(Some(delegate_proto));

            // The same delegate also provides the presentation anchor
            // (the NSWindow over which the passkey sheet is shown).
            let presentation_proto: &ProtocolObject<
                dyn ASAuthorizationControllerPresentationContextProviding,
            > = ProtocolObject::from_ref(&*delegate);
            controller.setPresentationContextProvider(Some(presentation_proto));

            // Start the ceremony
            controller.performRequests();

            Ok(Self {
                controller,
                _delegate: delegate,
                receiver: Some(rx),
            })
        }
    }

    /// Start a passkey authentication ceremony (the assertion flow).
    ///
    /// `credential_id` is the raw credential ID captured at registration and
    /// stored in `CubeSettings::passkey_metadata`. It is passed as the single
    /// `allowedCredentials` entry so the system offers exactly this Cube's
    /// passkey rather than a picker over every credential for the RP.
    ///
    /// Errors are [`super::PasskeyError`], not strings, so a caller can honour
    /// **I12** without re-parsing prose.
    pub fn authenticate(rp_id: &str, credential_id: &[u8]) -> Result<Self, super::PasskeyError> {
        let mtm = MainThreadMarker::new().ok_or_else(|| {
            super::PasskeyError::CeremonyFailed(
                "Passkey ceremony must be started on the main thread".to_string(),
            )
        })?;
        // See `register`: on macOS 14 this is a crash, not a failure.
        if !prf_supported() {
            return Err(super::PasskeyError::PrfNotSupported);
        }
        // A Cube whose stored credential id is empty has nothing to offer the
        // authenticator. That is the one case we can name with certainty, so it
        // gets the same classified outcome as "the platform has no such
        // passkey" rather than a bare failure.
        if credential_id.is_empty() {
            return Err(super::PasskeyError::CredentialNotFound(
                "this Cube has no stored passkey credential id".to_string(),
            ));
        }

        unsafe {
            let rp_ns = NSString::from_str(rp_id);
            let provider =
                ASAuthorizationPlatformPublicKeyCredentialProvider::initWithRelyingPartyIdentifier(
                    ASAuthorizationPlatformPublicKeyCredentialProvider::alloc(),
                    &rp_ns,
                );

            // The challenge is irrelevant to the PRF output — it only signs the
            // assertion, which nothing here verifies (there is no server in
            // this ceremony; the whole point is local key derivation). It still
            // has to be fresh and unpredictable so the ceremony is a real one.
            let mut challenge_bytes = [0u8; 32];
            rand::thread_rng().fill_bytes(&mut challenge_bytes);
            let challenge = NSData::with_bytes(&challenge_bytes);

            let request = provider.createCredentialAssertionRequestWithChallenge(&challenge);

            // Restrict to this Cube's credential.
            let cred_data = NSData::with_bytes(credential_id);
            let descriptor =
                ASAuthorizationPlatformPublicKeyCredentialDescriptor::initWithCredentialID(
                    ASAuthorizationPlatformPublicKeyCredentialDescriptor::alloc(),
                    &cred_data,
                );
            let allowed = NSArray::from_slice(&[&*descriptor]);
            request.setAllowedCredentials(&allowed);

            // Same PRF salt as registration — this is what makes the assertion
            // reproduce the registration's key material.
            let salt_data = NSData::with_bytes(PRF_SALT);
            let prf_values =
                ASAuthorizationPublicKeyCredentialPRFAssertionInputValues::initWithSaltInput1_saltInput2(
                    ASAuthorizationPublicKeyCredentialPRFAssertionInputValues::alloc(),
                    &salt_data,
                    None,
                );
            let prf_input =
                ASAuthorizationPublicKeyCredentialPRFAssertionInput::initWithInputValues_perCredentialInputValues(
                    ASAuthorizationPublicKeyCredentialPRFAssertionInput::alloc(),
                    Some(&prf_values),
                    None,
                );
            request.setPrf(Some(&prf_input));

            let request_super: &objc2_authentication_services::ASAuthorizationRequest = &request;
            let requests_array = NSArray::from_slice(&[request_super]);

            let controller = ASAuthorizationController::initWithAuthorizationRequests(
                ASAuthorizationController::alloc(),
                &requests_array,
            );

            let (tx, rx) = mpsc::channel();
            let delegate = PasskeyDelegate::new(mtm, tx);
            let delegate_proto: &ProtocolObject<dyn ASAuthorizationControllerDelegate> =
                ProtocolObject::from_ref(&*delegate);
            controller.setDelegate(Some(delegate_proto));
            let presentation_proto: &ProtocolObject<
                dyn ASAuthorizationControllerPresentationContextProviding,
            > = ProtocolObject::from_ref(&*delegate);
            controller.setPresentationContextProvider(Some(presentation_proto));

            controller.performRequests();

            Ok(Self {
                controller,
                _delegate: delegate,
                receiver: Some(rx),
            })
        }
    }

    /// Poll for a result (non-blocking). `None` once the channel has been
    /// handed off with [`Self::take_receiver`].
    pub fn try_recv(&self) -> Option<NativeOutcome> {
        self.receiver.as_ref().and_then(|r| r.try_recv().ok())
    }

    /// Take the result channel, leaving the controller and delegate in place.
    ///
    /// This is what lets a caller wait for the result on another thread: the
    /// ceremony itself is not `Send` (retained Objective-C objects), but
    /// `Receiver<NativeOutcome>` is. The ceremony must stay alive and parked on
    /// the main thread for as long as the waiter cares — dropping it drops the
    /// delegate, which drops the sender, which wakes the waiter with a
    /// disconnect. See `services::passkey::reauth`.
    ///
    /// Returns `None` on a second call.
    pub fn take_receiver(&mut self) -> Option<mpsc::Receiver<NativeOutcome>> {
        self.receiver.take()
    }

    /// Cancel the in-progress ceremony.
    pub fn cancel(&self) {
        unsafe {
            self.controller.cancel();
        }
    }
}

impl Drop for NativePasskeyCeremony {
    fn drop(&mut self) {
        self.cancel();
    }
}
