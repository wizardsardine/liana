//! Pairing dial — the desktop is now the TLS **client** during
//! pairing, matching the steady-state direction.
//!
//! Lifecycle:
//!   1. Caller has already browsed mDNS and picked a phone.
//!   2. Dial the phone's `SocketAddr` over TLS, with an
//!      accept-any-cert verifier that captures the cert hash so we
//!      can pin it post-handshake.
//!   3. Read one length-prefixed `LocalEnvelope` expecting
//!      `PairingComplete`.
//!   4. Validate the wallet fingerprint claim against the local
//!      wallet's keys.
//!   5. Persist a `PairedPhone` row.
//!   6. Drop the connection — the next 2s discovery tick redials via
//!      the steady-state path.
//!
//! See `plans/PLAN-local-signer-lan-interop-fixes-desktop.md` §1.4.

use prost::Message as _;

use coincube_core::miniscript::bitcoin::bip32::Fingerprint;

use crate::dir::CoincubeDirectory;
use crate::phone_signer::errors::PairingError;
use crate::phone_signer::identity::DesktopIdentity;
use crate::phone_signer::mdns;
use crate::phone_signer::pairing::PairingOffer;
use crate::phone_signer::pairing_store::{self, PairedPhone};
use crate::phone_signer::protocol::{local_v1, LocalEnvelope};
use crate::phone_signer::transport::PairedTransport;

/// Dial the phone selected during the picker step, read its
/// `PairingComplete`, validate, persist. Returns the persisted
/// [`PairedPhone`] on success.
///
/// The caller is responsible for confirming `offer.expires_at_unix`
/// hasn't passed before invoking this; we double-check below but
/// the wizard's countdown should be doing it too.
///
/// `expected_vault_id` is the local wallet's [`Wallet::id_fingerprint`]:
/// the offer's `wallet_fingerprint` claim MUST equal it, otherwise we
/// raise `WalletFingerprintMismatch`. This catches the "QR was
/// generated for a different vault" case (e.g. user scanned an old
/// offer after switching wallets).
///
/// `signer_fingerprints` is the local wallet's `descriptor_keys()` —
/// the real BIP-32 master fingerprints that appear in the descriptor.
/// We persist this list as `PairedPhone.wallet_fingerprints` so the
/// steady-state hw refresh tick has a real signer fp to put on
/// `HardwareWallet::Supported`; otherwise the phone would be
/// downgraded to `Unsupported(NotPartOfWallet)` because the vault id
/// is by construction NOT one of the descriptor keys.
pub async fn run_pairing(
    identity: DesktopIdentity,
    offer: PairingOffer,
    phone: mdns::DiscoveredPhone,
    expected_vault_id: Fingerprint,
    signer_fingerprints: Vec<Fingerprint>,
    dir: CoincubeDirectory,
) -> Result<PairedPhone, PairingError> {
    if crate::phone_signer::pairing::is_expired(&offer) {
        return Err(PairingError::OfferExpired);
    }

    // Dial unpinned so we accept whatever cert the phone presents;
    // we'll capture and pin its SHA-256 right after the handshake.
    let transport = PairedTransport::connect_unpinned(phone.addr, &identity)
        .await
        .map_err(|e| PairingError::NetworkError(format!("dial phone: {}", e)))?;
    let phone_pin = transport
        .peer_cert_fingerprint()
        .ok_or_else(|| PairingError::NetworkError("phone presented no cert".into()))?;

    // Split into reader/writer so we can read PairingComplete and
    // (best-effort) send a Pong ack without one half blocking the
    // other. The connection drops when both halves go out of scope
    // at function end.
    let (mut reader, mut writer) = transport.split();
    // Bound the read by the offer's remaining lifetime. A phone
    // that completes TLS but stalls before sending PairingComplete
    // would otherwise hang this future indefinitely, and pairing
    // could complete long after the QR expired.
    let remaining_secs = crate::phone_signer::pairing::seconds_remaining(&offer);
    if remaining_secs == 0 {
        return Err(PairingError::OfferExpired);
    }
    let recv = tokio::time::timeout(
        std::time::Duration::from_secs(remaining_secs),
        reader.recv(),
    );
    let envelope = match recv.await {
        Ok(Ok(env)) => env,
        Ok(Err(e)) => {
            return Err(PairingError::NetworkError(format!(
                "recv pairing_complete: {}",
                e
            )));
        }
        Err(_) => return Err(PairingError::OfferExpired),
    };
    let complete = match envelope.payload {
        Some(local_v1::local_envelope::Payload::PairingComplete(c)) => c,
        _ => {
            return Err(PairingError::InternalError(
                "expected PairingComplete envelope".into(),
            ));
        }
    };

    // Enforce the proto's "MUST match" contract: the phone-reported
    // cert fp has to agree with the bytes we pinned from the live
    // TLS handshake. A divergence signals a buggy or misconfigured
    // phone — persisting `phone_pin` regardless would hide the
    // contract violation and let a bad pairing survive.
    let expected_pin_hex: String = phone_pin.iter().map(|b| format!("{:02x}", b)).collect();
    let reported_normalised = complete.phone_cert_fp.trim().to_ascii_lowercase();
    if reported_normalised != expected_pin_hex {
        return Err(PairingError::InternalError(format!(
            "phone-reported cert fp {:?} doesn't match TLS handshake {}",
            complete.phone_cert_fp, expected_pin_hex,
        )));
    }
    tracing::debug!(
        target: "phone_signer::pairing",
        "phone-reported cert fp matches handshake: {}",
        expected_pin_hex,
    );

    // The offer's `wallet_fingerprint` is the desktop's vault id
    // (`Wallet::id_fingerprint`) — a 4-byte digest of the descriptor.
    // It must equal the locally-loaded wallet's vault id; otherwise
    // the user scanned a QR meant for a different vault. (When the
    // proto grows a phone-reported signer fingerprint we'll also
    // validate that against `signer_fingerprints`.)
    let claimed_fp = offer.wallet_fingerprint;
    if claimed_fp != expected_vault_id {
        return Err(PairingError::WalletFingerprintMismatch {
            expected: vec![expected_vault_id],
            claimed: claimed_fp,
        });
    }

    // Best-effort ack so the phone can render "pairing complete".
    let ack = LocalEnvelope {
        payload: Some(local_v1::local_envelope::Payload::Pong(
            crate::services::connect::grpc::connect_v1::Pong { ts_unix_ms: 0 },
        )),
    };
    let _ = writer.send(&ack).await;

    let name = if complete.device_name.is_empty() {
        "Keychain phone".to_string()
    } else {
        complete.device_name
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let paired = PairedPhone {
        cert_pin: phone_pin,
        name,
        paired_at_unix: now,
        // Persist the descriptor's real signer fingerprints. The hw
        // refresh tick reads `.first()` of this list for the
        // `HardwareWallet::Supported.fingerprint` and the
        // descriptor-keys filter at the end of the tick keeps the
        // phone listed as Supported.
        wallet_fingerprints: signer_fingerprints,
        fallback_addr: None,
    };
    pairing_store::upsert(&dir, paired.clone())
        .map_err(|e| PairingError::InternalError(format!("persist: {}", e)))?;

    // Both halves of `transport` drop here; the next discovery tick
    // redials via the steady-state pinned path.
    let _ = (reader, writer);
    Ok(paired)
}

// `prost::Message` is used implicitly via the generated proto types'
// methods (encode/decode/encoded_len). Keep the import to avoid a
// future refactor accidentally dropping it.
#[allow(dead_code)]
fn _force_prost_import(env: &LocalEnvelope) -> usize {
    env.encoded_len()
}
