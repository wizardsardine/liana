//! Owner self-recovery via Keychain — gate matrix + envelope fetch
//! (PLAN-owner-keychain-recovery PR 3).
//!
//! The owner-side analogue of [`super::keyholder`]. Where the heir path fetches
//! someone else's escrowed material, this fetches the **owner's own** ECIES
//! envelope set (sealed to their `owner-self` Keychain key) from the gated
//! release endpoint. Like the heir path it is server-blind: the bytes come back
//! as ciphertext and the owner's Keychain does the ECDH; the desktop opens the
//! AES-GCM ciphertext (reusing [`crate::services::inheritance::heir`]).
//!
//! This module is just the fetch plus a typed mapping of the server's gate
//! matrix. The `423 DURESS_LOCKED` case is collapsed to a neutral
//! "unavailable, try later" — the surface must never explain *why* (invariant
//! I3), even to the owner, since a coercer could be watching.

use crate::services::coincube::{CoincubeClient, CoincubeError, InheritanceEnvelopeWire};

/// Typed outcome of an owner self-recovery envelope fetch. Each variant maps a
/// server gate response to a distinct, display-safe UI state.
///
/// `Clone` + `Debug` mirror [`super::keyholder::KeyholderRecoveryError`] (Iced
/// clones messages between update/task/view); no variant carries secrets.
#[derive(Debug, Clone)]
pub enum OwnerKeychainRecoveryError {
    /// `403` — the signed-in account is not the owner of this Cube. Also the
    /// fail-closed default for any unrecognised `403`.
    NotOwner,
    /// `423 DURESS_LOCKED` — the account is under a duress lock. Render neutral
    /// copy; never explain duress. `retry_at` is the optional
    /// `data.duress_unlock_at` hint, surfaced only as a soft "try again later".
    Unavailable {
        retry_at: Option<chrono::DateTime<chrono::Utc>>,
    },
    /// `404` — no envelope set has been uploaded for this Cube (the owner never
    /// chose "protect with my phone", or it was removed).
    NoEnvelope,
    /// `503` — the owner-recovery surface is off server-side, or the feature
    /// isn't deployed.
    Unsupported,
    /// `429` — rate limited.
    RateLimited { retry_after: std::time::Duration },
    /// Anything else — auth, network, 5xx, parse.
    Api(String),
}

impl std::fmt::Display for OwnerKeychainRecoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotOwner => write!(
                f,
                "You're not signed in as the owner of this Cube on this account."
            ),
            // Neutral, duress-safe copy — matches the recovery-kit download flow's
            // wording so the two paths are indistinguishable (invariant I3).
            Self::Unavailable { .. } => write!(
                f,
                "Recovery is unavailable right now. Please try again later."
            ),
            Self::NoEnvelope => write!(
                f,
                "This Cube isn't set up for phone recovery — back it up with your phone first."
            ),
            Self::Unsupported => write!(f, "Phone recovery isn't available on this server."),
            Self::RateLimited { retry_after } => {
                write!(
                    f,
                    "Too many attempts — try again in {}s.",
                    retry_after.as_secs()
                )
            }
            Self::Api(msg) => write!(f, "Recovery error: {}", msg),
        }
    }
}

impl std::error::Error for OwnerKeychainRecoveryError {}

impl From<CoincubeError> for OwnerKeychainRecoveryError {
    fn from(e: CoincubeError) -> Self {
        match e {
            // The client maps 404 → NotFound and 429 → RateLimited before we see
            // them (`parse_recovery_response`).
            CoincubeError::NotFound => Self::NoEnvelope,
            CoincubeError::RateLimited { retry_after } => Self::RateLimited { retry_after },
            CoincubeError::Unsuccessful(ref info) => match info.status_code {
                // Any 403 fails closed to "not the owner" — never leak a reason.
                403 => Self::NotOwner,
                423 => Self::Unavailable {
                    retry_at: duress_unlock_at(&info.text),
                },
                503 => Self::Unsupported,
                _ => Self::Api(e.to_string()),
            },
            other => Self::Api(other.to_string()),
        }
    }
}

/// Body of a `423 DURESS_LOCKED` response — the optional unlock hint rides on
/// `data.duress_unlock_at` (same shape as the heir path's 423).
#[derive(serde::Deserialize)]
struct DuressLockEnvelope {
    #[serde(default)]
    data: Option<DuressLockData>,
}

#[derive(serde::Deserialize)]
struct DuressLockData {
    #[serde(default)]
    duress_unlock_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Parses the optional `data.duress_unlock_at` hint from a `423` body. Returns
/// `None` for an opaque/unparseable body — the variant is the same either way.
fn duress_unlock_at(body: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    serde_json::from_str::<DuressLockEnvelope>(body)
        .ok()
        .and_then(|env| env.data)
        .and_then(|d| d.duress_unlock_at)
}

/// Fetches the owner's own ECIES envelope set, mapping the server's gate matrix
/// to [`OwnerKeychainRecoveryError`]. The returned wires are still ciphertext —
/// the owner's Keychain decrypts them ([`crate::services::inheritance::heir::decrypt_envelopes`]).
pub async fn fetch_owner_recovery_envelope(
    client: &CoincubeClient,
    cube_id: u64,
) -> Result<Vec<InheritanceEnvelopeWire>, OwnerKeychainRecoveryError> {
    client
        .get_recovery_kit_envelope(cube_id)
        .await
        .map_err(OwnerKeychainRecoveryError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::http::NotSuccessResponseInfo;

    fn unsuccessful(status: u16, text: &str) -> CoincubeError {
        CoincubeError::Unsuccessful(NotSuccessResponseInfo {
            status_code: status,
            text: text.to_string(),
        })
    }

    #[test]
    fn forbidden_fails_closed_to_not_owner() {
        let err: OwnerKeychainRecoveryError = unsuccessful(403, "garbage").into();
        assert!(matches!(err, OwnerKeychainRecoveryError::NotOwner));
    }

    #[test]
    fn not_found_maps_to_no_envelope() {
        let err: OwnerKeychainRecoveryError = CoincubeError::NotFound.into();
        assert!(matches!(err, OwnerKeychainRecoveryError::NoEnvelope));
    }

    #[test]
    fn locked_maps_to_neutral_unavailable_with_hint() {
        let body = r#"{"success":false,"data":{"duress_unlock_at":"2026-07-01T00:00:00Z"},"error":{"code":"DURESS_LOCKED","message":"x"}}"#;
        let err: OwnerKeychainRecoveryError = unsuccessful(423, body).into();
        // The duress reason must never reach the user — only the neutral copy.
        assert_eq!(
            err.to_string(),
            "Recovery is unavailable right now. Please try again later."
        );
        match err {
            OwnerKeychainRecoveryError::Unavailable { retry_at } => {
                assert_eq!(retry_at.unwrap().to_rfc3339(), "2026-07-01T00:00:00+00:00");
            }
            other => panic!("expected Unavailable, got {:?}", other),
        }
    }

    #[test]
    fn opaque_423_maps_to_unavailable_without_hint() {
        let err: OwnerKeychainRecoveryError = unsuccessful(423, "nope").into();
        assert!(matches!(
            err,
            OwnerKeychainRecoveryError::Unavailable { retry_at: None }
        ));
    }

    #[test]
    fn service_unavailable_maps_to_unsupported() {
        let err: OwnerKeychainRecoveryError = unsuccessful(503, "off").into();
        assert!(matches!(err, OwnerKeychainRecoveryError::Unsupported));
    }

    #[test]
    fn rate_limited_preserves_retry_after() {
        let err: OwnerKeychainRecoveryError = CoincubeError::RateLimited {
            retry_after: std::time::Duration::from_secs(31),
        }
        .into();
        match err {
            OwnerKeychainRecoveryError::RateLimited { retry_after } => {
                assert_eq!(retry_after, std::time::Duration::from_secs(31));
            }
            other => panic!("expected RateLimited, got {:?}", other),
        }
    }

    #[test]
    fn server_5xx_stays_api_error() {
        let err: OwnerKeychainRecoveryError = unsuccessful(500, "boom").into();
        assert!(matches!(err, OwnerKeychainRecoveryError::Api(_)));
    }
}
