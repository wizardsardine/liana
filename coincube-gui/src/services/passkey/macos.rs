//! Native macOS passkey ceremony via the AuthenticationServices framework.
//!
//! This implementation calls Apple's `ASAuthorizationController` directly
//! through `objc2` bindings. It bypasses the WebAuthn-via-WebView path
//! (which is broken in WKWebView without the browser entitlement) and uses
//! the platform authenticator (Touch ID / Face ID via iCloud Keychain).
//!
//! Requires macOS 14 (Sonoma) or later for the PRF extension.

#![cfg(target_os = "macos")]
#![allow(unexpected_cfgs)]

use std::cell::OnceCell;
use std::sync::mpsc;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_authentication_services::{
    ASAuthorization, ASAuthorizationController, ASAuthorizationControllerDelegate,
    ASAuthorizationControllerPresentationContextProviding,
    ASAuthorizationPlatformPublicKeyCredentialProvider,
    ASAuthorizationPlatformPublicKeyCredentialRegistration,
    ASAuthorizationPublicKeyCredentialPRFAssertionInputValues,
    ASAuthorizationPublicKeyCredentialPRFRegistrationInput, ASPublicKeyCredential,
};
// IMPORTANT: ASPresentationAnchor is `objc2::runtime::NSObject`, NOT
// `objc2_foundation::NSObject`. To avoid name collisions in `define_class!`,
// we use the runtime NSObject as the superclass and skip importing
// objc2_foundation::NSObject entirely.
use objc2::runtime::NSObject;
use objc2_foundation::{NSArray, NSData, NSError, NSObjectProtocol, NSString};

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
    Error(String),
}

/// Salt used by the Breez passkey-login spec — "NYOASTRTSAOYN".
const PRF_SALT: &[u8] = &[
    0x4e, 0x59, 0x4f, 0x41, 0x53, 0x54, 0x52, 0x54, 0x53, 0x41, 0x4f, 0x59, 0x4e,
];

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
            if let Some(sender) = self.ivars().sender.get() {
                let _ = sender.send(NativeOutcome::Error(full));
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

                let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
                if app.is_null() {
                    panic!("NSApplication.sharedApplication returned nil");
                }

                let mut window: *mut AnyObject = msg_send![app, keyWindow];
                if window.is_null() {
                    window = msg_send![app, mainWindow];
                }
                if window.is_null() {
                    let windows: *mut AnyObject = msg_send![app, windows];
                    if !windows.is_null() {
                        window = msg_send![windows, firstObject];
                    }
                }
                if window.is_null() {
                    panic!("No NSWindow available for passkey presentation");
                }

                // Retain the window and return as Retained<RuntimeNSObject>
                // (which is what ASPresentationAnchor aliases to).
                let _: () = msg_send![window, retain];
                Retained::from_raw(window as *mut NSObject).expect("Failed to retain NSWindow")
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

/// Extract the credential ID and PRF output from a successful authorization.
unsafe fn extract_outcome(authorization: &ASAuthorization) -> NativeOutcome {
    let credential = unsafe { authorization.credential() };

    // ProtocolObject implements AsRef<AnyObject>, so we go through that.
    let any_obj: &objc2::runtime::AnyObject = credential.as_ref();
    let reg = match any_obj.downcast_ref::<ASAuthorizationPlatformPublicKeyCredentialRegistration>()
    {
        Some(r) => r,
        None => {
            return NativeOutcome::Error(
                "Unexpected credential type returned by AuthenticationServices".to_string(),
            )
        }
    };

    // credentialID() comes from the ASPublicKeyCredential trait.
    let credential_id_data = unsafe { reg.credentialID() };
    let credential_id = credential_id_data.to_vec();

    let prf = match unsafe { reg.prf() } {
        Some(p) => p,
        None => {
            return NativeOutcome::Error("PRF extension not supported by this passkey".to_string())
        }
    };

    let first = match unsafe { prf.first() } {
        Some(d) => d,
        None => return NativeOutcome::Error("PRF output missing first value".to_string()),
    };

    let bytes = first.to_vec();
    if bytes.len() < 32 {
        return NativeOutcome::Error(format!(
            "PRF output too short: {} bytes (expected at least 32)",
            bytes.len()
        ));
    }

    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes[..32]);

    NativeOutcome::Registered {
        credential_id,
        prf_output: Zeroizing::new(arr),
    }
}

/// Active passkey ceremony — holds the controller, delegate, and channel receiver.
///
/// Drop this to cancel the ceremony.
pub struct NativePasskeyCeremony {
    controller: Retained<ASAuthorizationController>,
    _delegate: Retained<PasskeyDelegate>,
    receiver: mpsc::Receiver<NativeOutcome>,
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
            let request_super: &objc2_authentication_services::ASAuthorizationRequest = &**request;
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
                receiver: rx,
            })
        }
    }

    /// Start a passkey authentication ceremony.
    pub fn authenticate(rp_id: &str, _credential_id: &[u8]) -> Result<Self, String> {
        // TODO: Implement assertion (authentication) flow.
        let _ = rp_id;
        Err("Native passkey authentication not yet implemented".to_string())
    }

    /// Poll for a result (non-blocking).
    pub fn try_recv(&self) -> Option<NativeOutcome> {
        self.receiver.try_recv().ok()
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
