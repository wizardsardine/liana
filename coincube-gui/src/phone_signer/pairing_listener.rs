//! Pairing dial — the desktop is now the TLS **client** during
//! pairing, matching the steady-state direction.
//!
//! Lifecycle:
//!   1. Caller has already browsed mDNS and picked a phone.
//!   2. Re-dial loop ([`run_pairing`]): each iteration dials the
//!      phone's `SocketAddr` over TLS, captures the cert hash via
//!      an accept-any-cert verifier, and reads one length-prefixed
//!      `LocalEnvelope` expecting `PairingComplete`.
//!   3. On `NetworkError` (TCP failed, TLS failed, peer closed
//!      before sending), back off briefly and try again. The phone
//!      typically closes immediately on connections that arrive
//!      *before* the user scans the QR; without the retry loop the
//!      QR vanishes on the first such failure and the user never
//!      has time to scan.
//!   4. Validate the wallet fingerprint claim against the local
//!      wallet's keys.
//!   5. Return the would-be `PairedPhone` row; the caller decides
//!      whether to persist (see
//!      [`crate::app::state::settings::local_signing::LocalSigningState::apply_pairing_completed`]).
//!      Persisting here would leak a paired row if the user
//!      cancelled the wizard while this future was still in flight —
//!      `Task::perform` futures aren't cancellable from the caller,
//!      so we gate persistence at the synchronous message-apply
//!      point instead.
//!   6. Drop the connection — the next 2s discovery tick redials via
//!      the steady-state path.
//!
//! See `plans/PLAN-local-signer-lan-interop-fixes-desktop.md` §1.4.

use std::time::Duration;

use prost::Message as _;

use coincube_core::miniscript::bitcoin::bip32::Fingerprint;

use crate::phone_signer::errors::PairingError;
use crate::phone_signer::identity::DesktopIdentity;
use crate::phone_signer::mdns;
use crate::phone_signer::pairing::PairingOffer;
use crate::phone_signer::pairing_store::PairedPhone;
use crate::phone_signer::protocol::{local_v1, LocalEnvelope};
use crate::phone_signer::transport::PairedTransport;

/// How long the retry loop waits between consecutive failed dials.
/// Picked so a phone that closes immediately doesn't get hammered
/// (~1.3 dials/sec) but a user who scans the QR sees pairing
/// complete within a beat of their tap. The offer's 120s TTL caps
/// the total number of attempts.
pub(crate) const REDIAL_BACKOFF: Duration = Duration::from_millis(750);

/// Dial the phone selected during the picker step, read its
/// `PairingComplete`, validate. Returns the would-be
/// [`PairedPhone`] on success; the caller persists it (gated by the
/// run-id + Waiting-state check in
/// [`crate::app::state::settings::local_signing::LocalSigningState::apply_pairing_completed`]).
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
/// We surface this list on `PairedPhone.wallet_fingerprints` so the
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
) -> Result<PairedPhone, PairingError> {
    if crate::phone_signer::pairing::is_expired(&offer) {
        return Err(PairingError::OfferExpired);
    }

    // Re-dial loop. The phone closes inbound TLS sessions
    // immediately when no QR has been scanned yet, so the very
    // first dial after the user clicks "Pair" almost always fails.
    // Without the loop the wizard would surface that error and
    // tear the QR down before the user got a chance to scan it.
    // Anything except `NetworkError` is treated as terminal
    // (cert-mismatch, wallet-fingerprint mismatch, expired offer),
    // so we don't loop forever on a real fault.
    //
    // When the loop exits via expiry we always return
    // `OfferExpired`, never the last `NetworkError` we were
    // retrying through. The TTL is what actually stopped us — the
    // network errors are just noise from the dials that happened
    // before the QR ran out. Surfacing one of them would route the
    // user to the "Network error" toast with Try Again, even
    // though the QR they were scanning is already dead and the
    // only remedy is a fresh offer (which the `OfferExpired`
    // branch's copy spells out).
    loop {
        if crate::phone_signer::pairing::is_expired(&offer) {
            return Err(PairingError::OfferExpired);
        }
        // Re-resolve the target from mDNS on every attempt. Some
        // phone-side implementations rebind their TLS listener to
        // a fresh ephemeral port between connections (we've seen
        // ports go 57334 → 59288 → 60531 across consecutive
        // attempts in a single pairing window). Without this
        // re-resolve the loop would keep dialing the stale port
        // from the picker snapshot and get `Connection refused`
        // forever. Also covers a phone that picks up a new DHCP
        // lease mid-pairing.
        let current = current_target_for(&phone);
        match try_pair_once(
            &identity,
            &offer,
            &current,
            expected_vault_id,
            &signer_fingerprints,
        )
        .await
        {
            Ok(paired) => return Ok(paired),
            Err(e) if is_dial_retriable(&e) => {
                tracing::debug!(
                    target: "phone_signer::pairing",
                    "pairing dial failed (will redial): {}",
                    e,
                );
                tokio::time::sleep(REDIAL_BACKOFF).await;
            }
            Err(e) => return Err(e),
        }
    }
}

/// Return the current best target for `phone`'s cert fingerprint
/// from the live mDNS cache, falling back to the snapshot the
/// picker captured if no fresh record is on offer. Pulled out so
/// the resolution policy can be unit-tested without a real mDNS
/// daemon.
fn current_target_for(snapshot: &mdns::DiscoveredPhone) -> mdns::DiscoveredPhone {
    let discovered = mdns::browse();
    pick_current_target(&snapshot.cert_fp8, &discovered, snapshot)
}

/// Choose between a fresh mDNS record and the picker-time snapshot.
/// Prefer the fresh record when it matches the phone's cert fp
/// (covers port rebinds and DHCP renewals mid-pairing); fall back
/// to the snapshot when mDNS hasn't surfaced anything yet (race
/// between the phone unbinding the old listener and publishing
/// the new SRV).
fn pick_current_target(
    fp8: &str,
    fresh: &[mdns::DiscoveredPhone],
    snapshot: &mdns::DiscoveredPhone,
) -> mdns::DiscoveredPhone {
    let picked = fresh
        .iter()
        .find(|d| d.cert_fp8 == fp8)
        .cloned()
        .unwrap_or_else(|| snapshot.clone());
    if picked.addr != snapshot.addr {
        tracing::debug!(
            target: "phone_signer::pairing",
            "mdns target moved during retry: {} -> {}",
            snapshot.addr,
            picked.addr,
        );
    }
    picked
}

/// `true` when the error is "phone wasn't ready yet" — worth
/// re-dialing inside the offer TTL. Non-retriable variants
/// (`WalletFingerprintMismatch`, `InternalError`, …) represent a
/// real fault that won't fix itself by trying again, so we surface
/// them immediately.
fn is_dial_retriable(err: &PairingError) -> bool {
    matches!(err, PairingError::NetworkError(_))
}

/// One dial-and-read attempt. Pulled out of [`run_pairing`] so the
/// retry loop can re-invoke it cheaply on a transient failure
/// without dragging the whole loop state along.
async fn try_pair_once(
    identity: &DesktopIdentity,
    offer: &PairingOffer,
    phone: &mdns::DiscoveredPhone,
    expected_vault_id: Fingerprint,
    signer_fingerprints: &[Fingerprint],
) -> Result<PairedPhone, PairingError> {
    // Dial unpinned so we accept whatever cert the phone presents;
    // we'll capture and pin its SHA-256 right after the handshake.
    let transport = PairedTransport::connect_unpinned(phone.addr, identity)
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
    let remaining_secs = crate::phone_signer::pairing::seconds_remaining(offer);
    if remaining_secs == 0 {
        return Err(PairingError::OfferExpired);
    }
    let recv = tokio::time::timeout(Duration::from_secs(remaining_secs), reader.recv());
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

    // Proof-of-QR-scan (protocol v2). The phone returns an HMAC over
    // both cert fingerprints keyed by the per-offer `psk` carried in
    // the QR. Only a device that actually scanned this QR knows the
    // psk, so a valid proof is the desktop's evidence that the cert
    // it just captured belongs to the phone the user is holding —
    // without this, the unpinned dial (`connect_unpinned`) would pin
    // whatever cert answered on the LAN, letting an active attacker
    // who wins the dial race become a permanent MITM. Binding the MAC
    // to the **handshake** cert fp (`expected_pin_hex`, not the
    // phone-reported string) means a relay/substitution attacker —
    // who terminates TLS with a different cert — can't forward a
    // genuine phone's proof. See
    // plans/PLAN-local-signer-pairing-phone-auth.md.
    if crate::phone_signer::pairing::verify_pairing_proof(
        &offer.psk_b64,
        &offer.cert_fp,
        &expected_pin_hex,
        complete.pairing_proof.trim(),
    )
    .is_err()
    {
        tracing::warn!(
            target: "phone_signer::pairing",
            "pairing proof verification failed for cert {} — refusing to pin",
            expected_pin_hex,
        );
        return Err(PairingError::PhoneVerificationFailed);
    }

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
        // The descriptor's real signer fingerprints. The hw refresh
        // tick reads `.first()` of this list for the
        // `HardwareWallet::Supported.fingerprint` and the
        // descriptor-keys filter at the end of the tick keeps the
        // phone listed as Supported.
        wallet_fingerprints: signer_fingerprints.to_vec(),
        fallback_addr: None,
    };

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;

    fn discovered(fp8: &str, addr: &str) -> mdns::DiscoveredPhone {
        mdns::DiscoveredPhone {
            cert_fp8: fp8.into(),
            addr: addr.parse::<SocketAddr>().expect("parse addr"),
            instance_name: format!("test-{}", fp8),
        }
    }

    /// Regression: phone rebinds its listener to a fresh ephemeral
    /// port mid-pairing. The picker snapshot pointed at port A; by
    /// the time we retry, mDNS shows the phone on port B. Without
    /// `pick_current_target` the loop kept dialing port A and got
    /// `Connection refused` until the offer expired.
    #[test]
    fn pick_current_target_picks_fresh_record_when_port_moves() {
        let snapshot = discovered("c5bf643c", "192.168.1.67:59288");
        let fresh = vec![discovered("c5bf643c", "192.168.1.67:60531")];
        let picked = pick_current_target("c5bf643c", &fresh, &snapshot);
        assert_eq!(picked.addr.to_string(), "192.168.1.67:60531");
    }

    /// DHCP renewal mid-pairing: address changes, fp stays.
    #[test]
    fn pick_current_target_picks_fresh_record_when_ip_moves() {
        let snapshot = discovered("c5bf643c", "192.168.1.67:50000");
        let fresh = vec![discovered("c5bf643c", "192.168.1.99:50000")];
        let picked = pick_current_target("c5bf643c", &fresh, &snapshot);
        assert_eq!(picked.addr.to_string(), "192.168.1.99:50000");
    }

    /// Race between phone unbinding the old listener and
    /// publishing the new SRV — mDNS cache is empty for a tick.
    /// Fall back to the snapshot so the next attempt still tries
    /// *something*; if the snapshot is stale too, the retry loop
    /// keeps going.
    #[test]
    fn pick_current_target_falls_back_to_snapshot_when_mdns_silent() {
        let snapshot = discovered("c5bf643c", "192.168.1.67:50000");
        let picked = pick_current_target("c5bf643c", &[], &snapshot);
        assert_eq!(picked.addr, snapshot.addr);
    }

    /// mDNS only knows about a *different* phone (e.g. a second
    /// Keychain instance on the same LAN). Must not accidentally
    /// dial that phone for our pairing.
    #[test]
    fn pick_current_target_ignores_unrelated_records() {
        let snapshot = discovered("c5bf643c", "192.168.1.67:50000");
        let other = discovered("deadbeef", "192.168.1.42:50000");
        let picked = pick_current_target("c5bf643c", &[other], &snapshot);
        assert_eq!(picked.addr, snapshot.addr);
    }
}
