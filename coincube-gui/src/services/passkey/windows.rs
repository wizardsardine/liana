//! Native Windows passkey ceremony via `webauthn.dll`.
//!
//! Replaces the `iced_wry` webview on Windows. The webview path was never a
//! good fit — it needed a deployed web origin, a TLS certificate and a
//! browser-shaped WebAuthn stack, and its "is PRF available?" check was
//! **string-matching an error message** out of the page
//! (`services::passkey::mod`). This talks to the platform API directly and
//! probes the version number instead.
//!
//! # Capability, not assumption
//!
//! Windows Hello only gained PRF in **Windows 11 25H2, build 26200.7840+
//! (February 2026)**. `WebAuthNGetApiVersionNumber()` reports:
//!
//! | API version | What it adds |
//! |---|---|
//! | ≥ 6 | the "Enable PRF" make-credential field |
//! | ≥ 8 | `pPRFGlobalEval` on get-assertion |
//!
//! Below that floor — every Windows 10 machine, and every Windows 11 before
//! 25H2 — this returns a clear [`Capability::Unavailable`] and the caller falls
//! back to PIN + device secret. It must never guess and produce a wrong seed.
//!
//! # Unverified on hardware
//!
//! This module has not been run against a real 25H2 machine. The version gate
//! is written so that a machine below the floor gets a capability error rather
//! than a crash, but the register/assert round trip above the floor is
//! documentation-faith until someone runs it. Do not enable
//! `COINCUBE_ENABLE_PASSKEY` on Windows before that happens.

use windows::core::PCWSTR;
use windows::Win32::Foundation::HWND;
use windows::Win32::Networking::WindowsWebServices::*;
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
use zeroize::Zeroizing;

/// API version that first exposes PRF on make-credential.
const PRF_MAKE_CREDENTIAL_API_VERSION: u32 = 6;
/// API version that first exposes `pPRFGlobalEval` on get-assertion. Both
/// halves are needed: registering a PRF credential we can never evaluate is
/// worse than refusing up front.
const PRF_ASSERTION_API_VERSION: u32 = 8;

/// What this machine's WebAuthn stack can do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Capability {
    Available { api_version: u32 },
    Unavailable(String),
}

impl Capability {
    pub fn is_available(&self) -> bool {
        matches!(self, Capability::Available { .. })
    }
}

/// Probe the platform. A real version query, replacing the previous approach of
/// matching substrings in a webview's error text.
pub fn capability() -> Capability {
    // SAFETY: no arguments, no out-params; returns 0 when the API is absent.
    let version = unsafe { WebAuthNGetApiVersionNumber() };
    classify_version(version)
}

/// Split out from [`capability`] so the version matrix is testable without a
/// Windows host.
fn classify_version(version: u32) -> Capability {
    if version == 0 {
        return Capability::Unavailable(
            "This version of Windows doesn't provide the WebAuthn platform API. \
             Coincube will use your PIN and this device's system keychain instead."
                .to_string(),
        );
    }
    if version < PRF_ASSERTION_API_VERSION {
        return Capability::Unavailable(format!(
            "Windows Hello on this system can't produce the key material Coincube needs \
             (WebAuthn API version {version}; version {PRF_ASSERTION_API_VERSION} or later \
             is required, which means Windows 11 25H2 or newer). Coincube will use your \
             PIN and this device's system keychain instead."
        ));
    }
    Capability::Available {
        api_version: version,
    }
}

/// Outcome of a native Windows ceremony.
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

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn parent_window() -> HWND {
    // The API needs a parent HWND to anchor the Hello prompt. The foreground
    // window is ours while the ceremony is user-initiated; a null handle makes
    // the prompt appear behind the app.
    unsafe { GetForegroundWindow() }
}

/// Register a new device-bound passkey and return its PRF output.
///
/// Device-bound is requested explicitly via
/// `WEBAUTHN_AUTHENTICATOR_ATTACHMENT_PLATFORM` plus
/// `bPreferResidentKey`. On Windows, platform credentials are Hello-resident
/// and do not sync — which is the property the design needs — but that is
/// still a claim to verify on real hardware, not to assert from docs.
pub fn register(rp_id: &str, rp_name: &str, user_id: &[u8], user_name: &str) -> NativeOutcome {
    match capability() {
        Capability::Available { .. } => {}
        Capability::Unavailable(why) => return NativeOutcome::Error(why),
    }
    if user_id.is_empty() {
        return NativeOutcome::Error("Missing user id for passkey registration".to_string());
    }

    let rp_id_w = wide(rp_id);
    let rp_name_w = wide(rp_name);
    let user_name_w = wide(user_name);

    let mut challenge = [0u8; 32];
    {
        use rand::RngCore;
        rand::rngs::OsRng.fill_bytes(&mut challenge);
    }

    unsafe {
        let rp = WEBAUTHN_RP_ENTITY_INFORMATION {
            dwVersion: WEBAUTHN_RP_ENTITY_INFORMATION_CURRENT_VERSION,
            pwszId: PCWSTR(rp_id_w.as_ptr()),
            pwszName: PCWSTR(rp_name_w.as_ptr()),
            pwszIcon: PCWSTR::null(),
        };

        let user = WEBAUTHN_USER_ENTITY_INFORMATION {
            dwVersion: WEBAUTHN_USER_ENTITY_INFORMATION_CURRENT_VERSION,
            cbId: user_id.len() as u32,
            pbId: user_id.as_ptr() as *mut u8,
            pwszName: PCWSTR(user_name_w.as_ptr()),
            pwszIcon: PCWSTR::null(),
            pwszDisplayName: PCWSTR(user_name_w.as_ptr()),
        };

        // ES256 only. Coincube never verifies the attestation signature — the
        // PRF output is the whole point — but offering one algorithm keeps the
        // authenticator from picking something exotic.
        let mut cose_params = [WEBAUTHN_COSE_CREDENTIAL_PARAMETER {
            dwVersion: WEBAUTHN_COSE_CREDENTIAL_PARAMETER_CURRENT_VERSION,
            pwszCredentialType: PCWSTR(wide("public-key").as_ptr()),
            lAlg: WEBAUTHN_COSE_ALGORITHM_ECDSA_P256_WITH_SHA256,
        }];
        let cose = WEBAUTHN_COSE_CREDENTIAL_PARAMETERS {
            cCredentialParameters: cose_params.len() as u32,
            pCredentialParameters: cose_params.as_mut_ptr(),
        };

        let client_data_json = client_data_json(&challenge, rp_id, "webauthn.create");
        let client_data = WEBAUTHN_CLIENT_DATA {
            dwVersion: WEBAUTHN_CLIENT_DATA_CURRENT_VERSION,
            cbClientDataJSON: client_data_json.len() as u32,
            pbClientDataJSON: client_data_json.as_ptr() as *mut u8,
            pwszHashAlgId: PCWSTR(wide("SHA-256").as_ptr()),
        };

        // The "Enable PRF" make-credential extension (API ≥ 6). Registration
        // only turns PRF *on*; the salt is evaluated at assertion time.
        let mut enable_prf: u8 = 1;
        let mut extensions = [WEBAUTHN_EXTENSION {
            pwszExtensionIdentifier: PCWSTR(wide(WEBAUTHN_EXTENSIONS_IDENTIFIER_PRF).as_ptr()),
            cbExtension: std::mem::size_of::<u8>() as u32,
            pvExtension: &mut enable_prf as *mut u8 as *mut core::ffi::c_void,
        }];

        let mut options = WEBAUTHN_AUTHENTICATOR_MAKE_CREDENTIAL_OPTIONS {
            dwVersion: WEBAUTHN_AUTHENTICATOR_MAKE_CREDENTIAL_OPTIONS_CURRENT_VERSION,
            dwTimeoutMilliseconds: 120_000,
            // Platform authenticator only — never a roaming key here, and never
            // a hybrid/QR flow to a phone, which would put the credential on a
            // different device.
            dwAuthenticatorAttachment: WEBAUTHN_AUTHENTICATOR_ATTACHMENT_PLATFORM,
            bRequireResidentKey: windows::Win32::Foundation::TRUE,
            dwUserVerificationRequirement:
                WEBAUTHN_USER_VERIFICATION_REQUIREMENT_REQUIRED,
            Extensions: WEBAUTHN_EXTENSIONS {
                cExtensions: extensions.len() as u32,
                pExtensions: extensions.as_mut_ptr(),
            },
            ..Default::default()
        };

        let mut credential = std::ptr::null_mut();
        let hr = WebAuthNAuthenticatorMakeCredential(
            parent_window(),
            &rp,
            &user,
            &cose,
            &client_data,
            Some(&mut options),
            &mut credential,
        );
        if hr.is_err() || credential.is_null() {
            return NativeOutcome::Error(describe_hresult(hr));
        }

        let attestation = &*credential;
        let credential_id = std::slice::from_raw_parts(
            attestation.pbCredentialId,
            attestation.cbCredentialId as usize,
        )
        .to_vec();

        WebAuthNFreeCredentialAttestation(Some(credential));

        // Registration does not return PRF output — only "PRF is enabled". Run
        // an assertion immediately to obtain it, exactly as the credential's
        // every subsequent use will.
        match assert(rp_id, &credential_id) {
            NativeOutcome::Authenticated { prf_output } => NativeOutcome::Registered {
                credential_id,
                prf_output,
            },
            NativeOutcome::Registered { prf_output, .. } => NativeOutcome::Registered {
                credential_id,
                prf_output,
            },
            NativeOutcome::Error(e) => NativeOutcome::Error(format!(
                "The passkey was created but Coincube couldn't read its key material \
                 ({e}). The Cube was not created."
            )),
        }
    }
}

/// Evaluate PRF against an existing credential.
pub fn assert(rp_id: &str, credential_id: &[u8]) -> NativeOutcome {
    match capability() {
        Capability::Available { .. } => {}
        Capability::Unavailable(why) => return NativeOutcome::Error(why),
    }

    let rp_id_w = wide(rp_id);
    let mut challenge = [0u8; 32];
    {
        use rand::RngCore;
        rand::rngs::OsRng.fill_bytes(&mut challenge);
    }

    unsafe {
        let client_data_json = client_data_json(&challenge, rp_id, "webauthn.get");
        let client_data = WEBAUTHN_CLIENT_DATA {
            dwVersion: WEBAUTHN_CLIENT_DATA_CURRENT_VERSION,
            cbClientDataJSON: client_data_json.len() as u32,
            pbClientDataJSON: client_data_json.as_ptr() as *mut u8,
            pwszHashAlgId: PCWSTR(wide("SHA-256").as_ptr()),
        };

        let mut allow_entry = WEBAUTHN_CREDENTIAL_EX {
            dwVersion: WEBAUTHN_CREDENTIAL_EX_CURRENT_VERSION,
            cbId: credential_id.len() as u32,
            pbId: credential_id.as_ptr() as *mut u8,
            pwszCredentialType: PCWSTR(wide("public-key").as_ptr()),
            dwTransports: WEBAUTHN_CTAP_TRANSPORT_INTERNAL,
        };
        let mut allow_ptrs = [&mut allow_entry as *mut WEBAUTHN_CREDENTIAL_EX];
        let allow_list = WEBAUTHN_CREDENTIAL_LIST {
            cCredentials: allow_ptrs.len() as u32,
            ppCredentials: allow_ptrs.as_mut_ptr(),
        };

        // `pPRFGlobalEval` (API ≥ 8): one salt, applied to whichever credential
        // answers. `second` is deliberately unused — the PRF domain registry
        // specifies `first` only.
        let mut global_eval = WEBAUTHN_HMAC_SECRET_SALT {
            cbFirst: super::PRF_SALT.len() as u32,
            pbFirst: super::PRF_SALT.as_ptr() as *mut u8,
            cbSecond: 0,
            pbSecond: std::ptr::null_mut(),
        };

        let mut options = WEBAUTHN_AUTHENTICATOR_GET_ASSERTION_OPTIONS {
            dwVersion: WEBAUTHN_AUTHENTICATOR_GET_ASSERTION_OPTIONS_CURRENT_VERSION,
            dwTimeoutMilliseconds: 120_000,
            dwAuthenticatorAttachment: WEBAUTHN_AUTHENTICATOR_ATTACHMENT_PLATFORM,
            dwUserVerificationRequirement:
                WEBAUTHN_USER_VERIFICATION_REQUIREMENT_REQUIRED,
            pAllowCredentialList: &allow_list as *const _ as *mut _,
            pHmacSecretSaltValues: &mut global_eval as *mut _ as *mut _,
            ..Default::default()
        };

        let mut assertion = std::ptr::null_mut();
        let hr = WebAuthNAuthenticatorGetAssertion(
            parent_window(),
            PCWSTR(rp_id_w.as_ptr()),
            &client_data,
            Some(&mut options),
            &mut assertion,
        );
        if hr.is_err() || assertion.is_null() {
            return NativeOutcome::Error(describe_hresult(hr));
        }

        let a = &*assertion;
        let salt = a.pHmacSecret;
        if salt.is_null() || (*salt).cbFirst < 32 {
            WebAuthNFreeAssertion(Some(assertion));
            return NativeOutcome::Error(
                "This passkey didn't return the key material Coincube needs (no PRF \
                 output). Restore this Cube from your Recovery Kit or written seed phrase."
                    .to_string(),
            );
        }
        let bytes = Zeroizing::new(
            std::slice::from_raw_parts((*salt).pbFirst, (*salt).cbFirst as usize).to_vec(),
        );
        let mut prf_output = Zeroizing::new([0u8; 32]);
        prf_output.copy_from_slice(&bytes[..32]);
        WebAuthNFreeAssertion(Some(assertion));

        NativeOutcome::Authenticated { prf_output }
    }
}

/// Minimal WebAuthn client-data JSON. Nothing verifies it — there is no server
/// in this ceremony — but the platform requires a well-formed blob, and the
/// challenge must appear in it so the assertion is bound to this call.
fn client_data_json(challenge: &[u8; 32], rp_id: &str, ty: &str) -> Vec<u8> {
    use base64::Engine;
    let challenge_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(challenge);
    format!(
        r#"{{"type":"{ty}","challenge":"{challenge_b64}","origin":"https://{rp_id}","crossOrigin":false}}"#
    )
    .into_bytes()
}

/// Map an HRESULT to something a user can act on, keeping "you cancelled"
/// distinct from "it failed" — a cancel is not an error the user should be
/// told to report.
fn describe_hresult(hr: windows::core::HRESULT) -> String {
    const NTE_USER_CANCELLED: i32 = 0x8009_0035_u32 as i32;
    const NTE_NOT_SUPPORTED: i32 = 0x8009_002D_u32 as i32;
    match hr.0 {
        NTE_USER_CANCELLED => "Passkey prompt cancelled.".to_string(),
        NTE_NOT_SUPPORTED => {
            "Windows Hello on this system doesn't support what Coincube needs.".to_string()
        }
        other => format!("Windows Hello returned an error (0x{other:08X})."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The version-gate matrix. A machine below the floor must get a
    /// capability error, not a crash and not a silently different key.
    #[test]
    fn version_gate_matrix() {
        assert!(matches!(classify_version(0), Capability::Unavailable(_)));
        // Windows 10 / early Windows 11.
        for v in 1..PRF_ASSERTION_API_VERSION {
            assert!(
                matches!(classify_version(v), Capability::Unavailable(_)),
                "API version {v} must not be treated as PRF-capable"
            );
        }
        assert!(classify_version(PRF_ASSERTION_API_VERSION).is_available());
        assert!(classify_version(99).is_available());
    }

    #[test]
    fn below_floor_message_names_the_requirement_and_the_fallback() {
        let Capability::Unavailable(msg) = classify_version(6) else {
            panic!("API 6 has make-credential PRF but not assertion PRF");
        };
        assert!(msg.contains("25H2"), "{}", msg);
        assert!(msg.contains("system keychain"), "{}", msg);
    }

    #[test]
    fn make_credential_floor_is_not_mistaken_for_the_assertion_floor() {
        // API 6 enables PRF at registration but cannot evaluate it. Registering
        // a credential we could never read would create an unopenable Cube.
        assert!(PRF_MAKE_CREDENTIAL_API_VERSION < PRF_ASSERTION_API_VERSION);
        assert!(!classify_version(PRF_MAKE_CREDENTIAL_API_VERSION).is_available());
    }

    #[test]
    fn client_data_is_well_formed_and_carries_the_challenge() {
        let json = client_data_json(&[7u8; 32], "coincube.io", "webauthn.get");
        let text = String::from_utf8(json).unwrap();
        assert!(text.contains(r#""type":"webauthn.get""#));
        assert!(text.contains(r#""origin":"https://coincube.io""#));
        assert!(text.contains(r#""challenge":"#));
    }
}
