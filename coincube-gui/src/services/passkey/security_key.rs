//! FIDO2 security-key backend — CTAP2 `hmac-secret`, spoken directly to the
//! token over HID.
//!
//! # Why this exists
//!
//! It is what makes **Linux acceptable**. Linux has no platform authenticator:
//! there is no Windows Hello, no Touch ID, and `xdg-credentials-portal` has no
//! production timeline. Without this, a Linux user's only options are PIN-only
//! (weaker than every other platform) or nothing.
//!
//! It also works on macOS and Windows, and is the right answer for anyone below
//! the Windows Hello PRF floor — which is every Windows machine older than 11
//! 25H2.
//!
//! # No browser, no RP domain, no deployed origin
//!
//! WebAuthn's PRF extension is a browser-level wrapper around CTAP2's
//! `hmac-secret`. Talking to the token directly skips the browser, the relying
//! party, the TLS certificate and the hosted ceremony page — a desktop wallet's
//! unlock should not depend on a web origin being reachable. It also means
//! there is no RP-ID to get wrong (`services::passkey::mod` defaulted to
//! `"localhost"` while the SvelteKit page hardcoded `coincube.io`; deleting the
//! page deletes one side of that mismatch).
//!
//! # Device-bound by construction
//!
//! A security key's credential lives on the key and cannot be exported. That is
//! the property the Tenshu passkey design asks for, and unlike a platform
//! passkey it needs no verification that it isn't syncing somewhere — there is
//! nowhere for it to sync to.
//!
//! # Unverified on hardware
//!
//! No physical key has been through this code. The error paths are written to
//! degrade rather than guess, and enrolment refuses a key without
//! `hmac-secret`, but the round trip is unproven.

use ctap_hid_fido2::fidokey::{
    get_assertion::get_assertion_params::Extension as AssertionExtension,
    make_credential::make_credential_params::Extension as CredentialExtension,
};
use ctap_hid_fido2::{Cfg, FidoKeyHidFactory};
use zeroize::Zeroizing;

/// The relying-party identifier presented to the token.
///
/// Not a domain that gets resolved — CTAP2 treats it as an opaque scoping
/// string, and nothing here does a web request. It is stable because changing
/// it makes the token refuse to produce the same credential.
pub const RP_ID: &str = "coincube.io";
pub const RP_NAME: &str = "COINCUBE";

/// What a connected key can do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Capability {
    /// A key is present and advertises `hmac-secret`.
    Available,
    /// A key is present but cannot do what we need.
    Unsupported(String),
    /// No key found.
    NotPresent(String),
}

impl Capability {
    pub fn is_available(&self) -> bool {
        matches!(self, Capability::Available)
    }
}

#[derive(Debug, Clone)]
pub enum Outcome {
    Registered {
        credential_id: Vec<u8>,
        prf_output: Zeroizing<[u8; 32]>,
    },
    Authenticated {
        prf_output: Zeroizing<[u8; 32]>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// No key plugged in, or it was removed mid-ceremony.
    KeyAbsent(String),
    /// A key is present but lacks `hmac-secret`.
    NoHmacSecret,
    /// The user cancelled, or didn't touch the key in time.
    Cancelled,
    Failed(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::KeyAbsent(detail) =>
                write!(f, "Couldn't find your security key ({detail}). Plug it in and try again."),
            Self::NoHmacSecret => write!(
                f,
                "This security key doesn't support the hmac-secret extension, which \
                 Coincube needs to derive your Cube's key. Use a FIDO2 key that does."
            ),
            Self::Cancelled => write!(f, "Security key prompt cancelled."),
            Self::Failed(detail) => write!(f, "Your security key returned an error: {detail}"),
        }
    }
}

impl std::error::Error for Error {}

/// Classify a `ctap-hid-fido2` error string.
///
/// The crate returns `anyhow`-shaped errors, so this is string inspection —
/// but over *our own* dependency's messages, not over a webview's rendering of
/// a browser's rendering of an authenticator error, which is what the code this
/// replaces was doing.
fn classify(detail: &str) -> Error {
    let lower = detail.to_ascii_lowercase();
    if lower.contains("no device") || lower.contains("not found") || lower.contains("no fido") {
        Error::KeyAbsent(detail.to_string())
    } else if lower.contains("hmac") || lower.contains("unsupported extension") {
        Error::NoHmacSecret
    } else if lower.contains("cancel")
        || lower.contains("timeout")
        || lower.contains("action_timeout")
        || lower.contains("keepalive_cancel")
    {
        Error::Cancelled
    } else {
        Error::Failed(detail.to_string())
    }
}

/// Probe for a usable key. Does not require a touch.
pub fn capability() -> Capability {
    let cfg = Cfg::init();
    match FidoKeyHidFactory::create(&cfg) {
        Ok(device) => match device.enable_info_option(
            &ctap_hid_fido2::fidokey::get_info::InfoOption::CredMgmt,
        ) {
            // We only need the device to answer; `hmac-secret` presence is
            // checked against the reported extension list below.
            Ok(_) | Err(_) => match device.get_info() {
                Ok(info) => {
                    let has_hmac = format!("{:?}", info).to_ascii_lowercase().contains("hmac");
                    if has_hmac {
                        Capability::Available
                    } else {
                        Capability::Unsupported(
                            "This security key doesn't advertise the hmac-secret extension."
                                .to_string(),
                        )
                    }
                }
                Err(e) => Capability::Unsupported(e.to_string()),
            },
        },
        Err(e) => Capability::NotPresent(e.to_string()),
    }
}

/// Enrol a new credential on the connected key.
///
/// **`hmac-secret` is required at enrolment, not at first unlock.** A key
/// without it would happily create a credential, and the failure would surface
/// the first time the user tried to open the Cube — by which point they have a
/// Cube they cannot open. Refusing here is the difference between an
/// inconvenience and a loss.
pub fn register(pin: Option<&str>, user_id: &[u8], user_name: &str) -> Result<Outcome, Error> {
    if !capability().is_available() {
        return Err(match capability() {
            Capability::NotPresent(d) => Error::KeyAbsent(d),
            _ => Error::NoHmacSecret,
        });
    }

    let cfg = Cfg::init();
    let device = FidoKeyHidFactory::create(&cfg).map_err(|e| classify(&e.to_string()))?;

    let challenge = random_challenge();
    let extensions = vec![CredentialExtension::HmacSecret(Some(true))];

    let att = device
        .make_credential_with_extensions(RP_ID, &challenge, pin, Some(&extensions))
        .map_err(|e| classify(&e.to_string()))?;

    let credential_id = att.credential_descriptor.id.clone();
    if credential_id.is_empty() {
        return Err(Error::Failed(
            "The security key returned an empty credential id.".to_string(),
        ));
    }
    let _ = (RP_NAME, user_id, user_name);

    // Evaluate immediately: registration proves the credential exists, an
    // assertion proves we can actually derive from it. Doing both here means a
    // key that silently ignores `hmac-secret` is caught now.
    let prf_output = evaluate(&device, &credential_id, pin)?;
    Ok(Outcome::Registered {
        credential_id,
        prf_output,
    })
}

/// Evaluate `hmac-secret` against an existing credential.
pub fn authenticate(pin: Option<&str>, credential_id: &[u8]) -> Result<Outcome, Error> {
    let cfg = Cfg::init();
    let device = FidoKeyHidFactory::create(&cfg).map_err(|e| classify(&e.to_string()))?;
    let prf_output = evaluate(&device, credential_id, pin)?;
    Ok(Outcome::Authenticated { prf_output })
}

fn evaluate(
    device: &ctap_hid_fido2::FidoKeyHid,
    credential_id: &[u8],
    pin: Option<&str>,
) -> Result<Zeroizing<[u8; 32]>, Error> {
    let challenge = random_challenge();

    // Same 32-byte salt as every other backend — it is the registered PRF
    // domain input, and it is what makes the derived seed identical whichever
    // authenticator produced it.
    let mut salt = [0u8; 32];
    salt.copy_from_slice(super::PRF_SALT);
    let extensions = vec![AssertionExtension::HmacSecret(Some(salt))];

    let assertion = device
        .get_assertion_with_extensios(
            RP_ID,
            &challenge,
            &[credential_id.to_vec()],
            pin,
            Some(&extensions),
        )
        .map_err(|e| classify(&e.to_string()))?;

    // A key that silently ignored the extension returns an assertion with no
    // `HmacSecret` output. That must be an error, never a fallback: deriving
    // from anything else would hand the user a valid-looking wallet that is not
    // theirs.
    let secret = assertion
        .extensions
        .iter()
        .find_map(|ext| match ext {
            AssertionExtension::HmacSecret(Some(v)) => Some(*v),
            _ => None,
        })
        .ok_or(Error::NoHmacSecret)?;

    Ok(Zeroizing::new(secret))
}

fn random_challenge() -> Vec<u8> {
    use rand::RngCore;
    let mut c = vec![0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut c);
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_absent_and_no_hmac_are_different_errors() {
        // A user with no key plugged in and a user with the wrong kind of key
        // need different instructions.
        assert!(matches!(
            classify("no FIDO device found"),
            Error::KeyAbsent(_)
        ));
        assert_eq!(classify("unsupported extension: hmac-secret"), Error::NoHmacSecret);
        assert_eq!(classify("CTAP2_ERR_KEEPALIVE_CANCEL"), Error::Cancelled);
        assert!(matches!(classify("something else went wrong"), Error::Failed(_)));
    }

    #[test]
    fn key_removed_mid_ceremony_reads_as_absent() {
        assert!(matches!(
            classify("device not found (disconnected)"),
            Error::KeyAbsent(_)
        ));
    }

    #[test]
    fn error_copy_tells_the_user_what_to_do() {
        assert!(Error::KeyAbsent("x".into()).to_string().contains("Plug it in"));
        assert!(Error::NoHmacSecret.to_string().contains("FIDO2 key"));
    }

    #[test]
    fn the_salt_is_the_registered_prf_domain_input() {
        // Every backend must feed the authenticator the same 32 bytes, or the
        // same key derives different seeds depending on which code path ran.
        assert_eq!(super::super::PRF_SALT.len(), 32);
    }

    #[test]
    fn challenges_are_fresh() {
        assert_ne!(random_challenge(), random_challenge());
    }
}
