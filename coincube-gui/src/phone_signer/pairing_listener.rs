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
//!      has time to scan. A phone that removed this desktop answers
//!      with `Unpaired` instead of closing, until the QR is scanned;
//!      that is redialed the same way (see [`pairing_complete_from`]).
//!   4. Validate the wallet fingerprint claim against the local
//!      wallet's keys.
//!   5. Stage the exact binding and exchange explicit durable completion
//!      messages. Cancellation is serialized with the durable commit decision.
//!      Only a completed exchange returns a persisted row to the UI.
//!   6. Drop the connection — the next 2s discovery tick redials via
//!      the steady-state path.
//!
//! See `plans/PLAN-local-signer-lan-interop-fixes-desktop.md` §1.4.

use super::{pairing_run::PairingRun, pairing_transaction::PairingTransaction};
use crate::dir::CoincubeDirectory;
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
/// `PairingComplete`, validate, and durably complete both sides before success.
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
/// `signer_fingerprints` is the local wallet's descriptor fingerprint set,
/// used only as an additional consistency check. The phone must report the
/// exact QR-selected xpub and backend key ID before a pairing can complete.
///
/// `expected_signer_fp` is the origin fingerprint the *local descriptor*
/// records for `offer.signer_xpub`. Membership in `signer_fingerprints`
/// only proves the reported fingerprint names some key in the vault, not
/// that it names the selected one, so the phone's claim is required to
/// equal this. The caller resolves it because an origin fingerprint
/// belongs to a master key several levels above the account xpub and so
/// cannot be derived from `offer.signer_xpub` here.
// TODO(scope): 8 args because `expected_vault_id`, `signer_fingerprints` and
// `expected_signer_fp` are passed loose. They're one concept — the identity a
// phone must prove it holds — and bundling them into a `VaultTarget` would drop
// this to 6 and kill a live footgun: at call sites they read
// `wallet_fp, vec![wallet_fp], wallet_fp`, three same-typed args that transpose
// silently. Deferred rather than done here because it touches 18 call sites
// across files with in-flight edits.
#[allow(clippy::too_many_arguments)]
pub async fn run_pairing(
    identity: DesktopIdentity,
    offer: PairingOffer,
    phone: mdns::DiscoveredPhone,
    expected_vault_id: Fingerprint,
    signer_fingerprints: Vec<Fingerprint>,
    expected_signer_fp: Fingerprint,
    dir: &CoincubeDirectory,
    run: &PairingRun,
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
        run.check()?;
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
        match run
            .wait(try_pair_once(
                &identity,
                &offer,
                &current,
                expected_vault_id,
                &signer_fingerprints,
                expected_signer_fp,
                dir,
                run,
            ))
            .await
        {
            Ok(paired) => return Ok(paired),
            Err(e) if is_dial_retriable(&e) => {
                tracing::debug!(
                    target: "phone_signer::pairing",
                    "pairing dial failed (will redial): {}",
                    e,
                );
                run.wait(async {
                    tokio::time::sleep(REDIAL_BACKOFF).await;
                    Ok(())
                })
                .await?;
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
#[allow(clippy::too_many_arguments)] // see `run_pairing`
async fn try_pair_once(
    identity: &DesktopIdentity,
    offer: &PairingOffer,
    phone: &mdns::DiscoveredPhone,
    expected_vault_id: Fingerprint,
    signer_fingerprints: &[Fingerprint],
    expected_signer_fp: Fingerprint,
    dir: &CoincubeDirectory,
    run: &PairingRun,
) -> Result<PairedPhone, PairingError> {
    // Dial unpinned so we accept whatever cert the phone presents;
    // we'll capture and pin its SHA-256 right after the handshake.
    let transport = PairedTransport::connect_unpinned(phone.addr, identity)
        .await
        .map_err(|e| PairingError::NetworkError(format!("dial phone: {}", e)))?;
    run.check()?;
    let phone_pin = transport
        .peer_cert_fingerprint()
        .ok_or_else(|| PairingError::NetworkError("phone presented no cert".into()))?;

    // Keep independent framing halves for identity and the required durable
    // PairingStep exchange. Both halves close when this attempt ends.
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
    run.check()?;
    let complete = pairing_complete_from(envelope)?;

    if complete.completion_protocol != 1 {
        return Err(PairingError::InternalError(
            "Update both apps and pair again: durable pairing protocol required.".into(),
        ));
    }
    run.check()?;

    let expected_pin_hex = validate_reported_tls_pin(&phone_pin, &complete)?;

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
    if !crate::phone_signer::pairing::verify_pairing_proof(
        &offer.psk_b64,
        &offer.cert_fp,
        &expected_pin_hex,
        complete.pairing_proof.trim(),
    ) {
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
    // exact identity below is additionally checked against the selected key.)
    let claimed_fp = offer.wallet_fingerprint;
    if claimed_fp != expected_vault_id {
        return Err(PairingError::WalletFingerprintMismatch {
            expected: vec![expected_vault_id],
            claimed: claimed_fp,
        });
    }

    // The phone's ECIES transport key. Validated here rather than at first
    // sign so a phone we could never seal to fails pairing loudly, instead of
    // pairing cleanly and then failing every signing attempt — and, more to
    // the point, instead of leaving a plaintext LAN path as the only way it
    // could ever sign.
    //
    // The gate must match what `seal_to_device` will accept, not merely what
    // parses: `PublicKey::from_slice` also admits a 65-byte UNCOMPRESSED point,
    // which would pair cleanly here and then fail the 33-byte length check at
    // every sign. `is_sealable_transport_pubkey` checks length *and* curve.
    let transport_pubkey = complete.transport_pubkey.clone();
    if !crate::services::connect::crypto::is_sealable_transport_pubkey(&transport_pubkey) {
        tracing::warn!(
            target: "phone_signer::pairing",
            len = transport_pubkey.len(),
            "phone reported no usable ECIES transport key — refusing to pair",
        );
        return Err(PairingError::TransportKeyMissing);
    }

    // Checked before anything the phone sent, and reported separately: these
    // are faults in the offer *this desktop generated*, not in the response.
    // Folding them into the identity-mismatch error below sends the user to
    // re-select a key on their phone when the QR on their screen is the thing
    // that's wrong — a legacy offer built before the exact-key fields existed,
    // or one aimed at a different vault. Only regenerating fixes those.
    if offer.signer_xpub.is_empty()
        || !offer
            .descriptor_sha256
            .starts_with(&expected_vault_id.to_string())
    {
        tracing::warn!(
            target: "phone_signer::pairing",
            has_signer_xpub = !offer.signer_xpub.is_empty(),
            "pairing offer carries no usable exact-key identity — refusing to pair",
        );
        return Err(PairingError::InternalError(
            "This pairing QR predates exact-key pairing, or was generated for a \
             different vault. Start pairing again to get a fresh one."
                .into(),
        ));
    }

    // The v2 proof authenticates this TLS peer. It must report the one key
    // selected in the scanned QR; only the phone's local record supplies ID.
    let reported = complete.signer_binding.as_ref().ok_or_else(|| {
        PairingError::InternalError(
            "Pair again with an updated COINCUBE Mobile and select its exact vault key.".into(),
        )
    })?;
    if reported.xpub != offer.signer_xpub
        || reported.descriptor_sha256.len() != 32
        || hex::encode(&reported.descriptor_sha256) != offer.descriptor_sha256
        || reported
            .key_id
            .parse::<u64>()
            .ok()
            .filter(|id| *id > 0)
            .is_none()
    {
        return Err(PairingError::InternalError(
            "Exact pairing identity mismatch; pair again.".into(),
        ));
    }
    let fingerprint: Fingerprint = reported
        .fingerprint
        .parse()
        .map_err(|_| PairingError::InternalError("Invalid signer fingerprint".into()))?;
    if !signer_fingerprints.contains(&fingerprint) {
        return Err(PairingError::InternalError(
            "Selected key is not in this vault; pair again.".into(),
        ));
    }
    // Membership only proves the reported fingerprint names *some* key in
    // this vault — not the one the QR selected. Without this, a phone could
    // report the selected xpub alongside a different vault signer's
    // fingerprint and pass every check above, and we'd persist a
    // `SignerBinding` whose two halves identify different keys plus a
    // `wallet_fingerprints` entry advertising the phone for a key it can't
    // sign for. Bind them: the caller resolved this fingerprint from the
    // local descriptor entry for `offer.signer_xpub` (checked equal to
    // `reported.xpub` above), so requiring equality here ties the reported
    // xpub and fingerprint to the same key.
    if fingerprint != expected_signer_fp {
        tracing::warn!(
            target: "phone_signer::pairing",
            reported = %fingerprint,
            expected = %expected_signer_fp,
            "phone reported a fingerprint that isn't the selected key's — refusing to pair",
        );
        return Err(PairingError::InternalError(
            "Reported key fingerprint doesn't match the selected key; pair again.".into(),
        ));
    }
    let signer_binding = Some(crate::phone_signer::pairing_store::SignerBinding {
        key_id: reported.key_id.clone(),
        xpub: reported.xpub.clone(),
        fingerprint,
        descriptor_sha256: reported.descriptor_sha256.clone(),
        imported_scope: None,
    });

    let name = if complete.device_name.is_empty() {
        "Phone".to_string()
    } else {
        complete.device_name
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let paired = PairedPhone {
        signer_binding,
        cert_pin: phone_pin,
        name,
        paired_at_unix: now,
        // Advertise only the selected, phone-reported signer.
        wallet_fingerprints: vec![fingerprint],
        // The vault id we validated the offer against, so the hw
        // refresh loop can scope this phone to the vault it was
        // actually paired with (not just any vault that shares a
        // signer key).
        vault_fingerprint: expected_vault_id,
        // Validated just above; `sign_tx` seals every LAN session to this.
        transport_pubkey,
        fallback_addr: None,
    };

    run.check()?;
    let transaction_id = uuid::Uuid::new_v4().to_string();
    let transaction = run.authorized(|| {
        PairingTransaction::prepare(dir, transaction_id.clone(), paired)
            .map_err(|e| PairingError::InternalError(format!("stage pairing: {}", e)))
    })?;
    let result = async {
        use local_v1::pairing_step::Phase;
        send_step(&mut writer, accept_step(&transaction_id), run).await?;
        recv_step(&mut reader, &transaction_id, Phase::Prepared, run).await?;
        run.authorized(|| {
            transaction
                .write_candidate()
                .map_err(|e| PairingError::InternalError(format!("persist pairing: {}", e)))
        })?;
        send_step(&mut writer, step(&transaction_id, Phase::Commit), run).await?;
        recv_step(&mut reader, &transaction_id, Phase::Committed, run).await?;
        // Both candidates are durable, but neither peer has been authorized
        // to expose the new binding. Cancel can still win this final lock.
        run.decide(|| {
            transaction
                .decide()
                .map_err(|e| PairingError::InternalError(format!("complete pairing: {}", e)))
        })?;
        send_step(&mut writer, step(&transaction_id, Phase::Finish), run).await?;
        recv_step(&mut reader, &transaction_id, Phase::Finished, run).await?;
        run.check()?;
        transaction.finish().map_err(|e| {
            PairingError::InternalError(format!("record acknowledged completion: {}", e))
        })
    }
    .await;
    if let Err(error) = &result {
        // Cleanup consults the durable state. A decided transaction stays
        // hidden without another write, for EVERY error category or task drop.
        transaction
            .rollback()
            .map_err(|e| PairingError::InternalError(format!("pairing cleanup pending: {}", e)))?;
        let _ = writer
            .send(&LocalEnvelope {
                payload: Some(local_v1::local_envelope::Payload::Error(
                    local_v1::ErrorEnvelope {
                        code: super::PAIR_AGAIN_CODE.into(),
                        message: format!("Pairing incomplete: {}. Pair again.", error),
                        session_id: String::new(),
                    },
                )),
            })
            .await;
    }
    // Once staged, a failed handshake requires a fresh explicit offer.
    result.map_err(|error| {
        PairingError::InternalError(format!("Pairing incomplete: {}. Pair again.", error))
    })
}

fn validate_reported_tls_pin(
    phone_pin: &[u8; 32],
    complete: &local_v1::PairingComplete,
) -> Result<String, PairingError> {
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
    Ok(expected_pin_hex)
}

/// The phone's first frame on a pairing dial, read as its `PairingComplete`.
///
/// `Unpaired` is a retriable [`PairingError::NetworkError`], not a failure.
/// A phone that removed this desktop keeps trusting its cert for a few
/// connections so it can say so (Keychain's `RevokedDesktop`), and this
/// desktop dials as soon as the QR is on screen, so before the scan that is
/// what answers. Once the QR is scanned the phone routes this desktop to its
/// pairing handler, which always writes `PairingComplete` first, so a redial
/// is all it takes. Nothing has been staged at this point.
fn pairing_complete_from(
    envelope: LocalEnvelope,
) -> Result<local_v1::PairingComplete, PairingError> {
    match envelope.payload {
        Some(local_v1::local_envelope::Payload::PairingComplete(c)) => Ok(c),
        Some(local_v1::local_envelope::Payload::Unpaired(_)) => Err(PairingError::NetworkError(
            "phone still lists this desktop as removed; redialing until the QR is scanned".into(),
        )),
        Some(local_v1::local_envelope::Payload::Error(e)) => {
            Err(PairingError::InternalError(format!(
                "The phone refused the connection ({}: {}).",
                e.code, e.message
            )))
        }
        _ => Err(PairingError::InternalError(
            "expected PairingComplete envelope".into(),
        )),
    }
}

// `prost::Message` is used implicitly via the generated proto types'
// methods (encode/decode/encoded_len). Keep the import to avoid a
// future refactor accidentally dropping it.
#[allow(dead_code)]
fn _force_prost_import(env: &LocalEnvelope) -> usize {
    env.encoded_len()
}

/// Longest desktop name sent to the phone, in characters.
const MAX_DESKTOP_NAME_CHARS: usize = 64;

/// What the phone lists this pairing under: this computer's name, as the
/// Connect device list shows it. The QR's `svc` names the phone itself, so
/// the phone can't take the desktop's name from there.
pub(crate) fn desktop_display_name() -> String {
    clean_desktop_name(&crate::utils::device::device_label())
}

fn clean_desktop_name(label: &str) -> String {
    // macOS host names end in the mDNS `.local` suffix, noise on the phone.
    let name: String = label
        .replacen(".local (", " (", 1)
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_DESKTOP_NAME_CHARS)
        .collect();
    name.trim().to_string()
}

fn step(id: &str, phase: local_v1::pairing_step::Phase) -> local_v1::PairingStep {
    local_v1::PairingStep {
        transaction_id: id.into(),
        phase: phase as i32,
        desktop_name: String::new(),
    }
}

/// ACCEPT also names this desktop; see [`desktop_display_name`].
fn accept_step(id: &str) -> local_v1::PairingStep {
    local_v1::PairingStep {
        desktop_name: desktop_display_name(),
        ..step(id, local_v1::pairing_step::Phase::Accept)
    }
}

async fn send_step(
    writer: &mut super::transport::PairedWriter,
    step: local_v1::PairingStep,
    run: &PairingRun,
) -> Result<(), PairingError> {
    run.check()?;
    let envelope = LocalEnvelope {
        payload: Some(local_v1::local_envelope::Payload::PairingStep(step)),
    };
    run.wait(async {
        tokio::time::timeout(Duration::from_secs(10), writer.send(&envelope))
            .await
            .map_err(|_| PairingError::NetworkError("pairing send timeout; pair again".into()))?
            .map_err(|e| PairingError::NetworkError(e.to_string()))
    })
    .await
}

async fn recv_step(
    reader: &mut super::transport::PairedReader,
    id: &str,
    phase: local_v1::pairing_step::Phase,
    run: &PairingRun,
) -> Result<(), PairingError> {
    let env = run
        .wait(async {
            tokio::time::timeout(Duration::from_secs(10), reader.recv())
                .await
                .map_err(|_| {
                    PairingError::NetworkError("pairing reply timeout; pair again".into())
                })?
                .map_err(|e| PairingError::NetworkError(e.to_string()))
        })
        .await?;
    match env.payload {
        Some(local_v1::local_envelope::Payload::PairingStep(step)) if step.transaction_id==id && step.phase==phase as i32=>Ok(()),
        _=>Err(PairingError::InternalError("Peer refused durable pairing or sent an unexpected frame. Update both apps and pair again.".into()))
    }
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

    fn envelope(payload: local_v1::local_envelope::Payload) -> LocalEnvelope {
        LocalEnvelope {
            payload: Some(payload),
        }
    }

    fn unpaired() -> LocalEnvelope {
        envelope(local_v1::local_envelope::Payload::Unpaired(
            local_v1::Unpaired {
                reason: "removed on phone".into(),
            },
        ))
    }

    #[test]
    fn pairing_complete_is_the_first_frame_we_want() {
        let complete = local_v1::PairingComplete {
            device_name: "phone".into(),
            completion_protocol: 1,
            ..Default::default()
        };
        let got = pairing_complete_from(envelope(
            local_v1::local_envelope::Payload::PairingComplete(complete.clone()),
        ))
        .expect("PairingComplete passes through");
        assert_eq!(got, complete);
    }

    /// Regression: a phone that removed this desktop answers the pre-scan
    /// dial with `Unpaired`. Treating that as fatal tore the QR down before
    /// it could be scanned ("expected PairingComplete envelope").
    #[test]
    fn unpaired_first_frame_is_redialed() {
        let err = pairing_complete_from(unpaired()).expect_err("not a pairing");
        assert!(is_dial_retriable(&err), "got {:?}", err);
    }

    #[test]
    fn phone_error_first_frame_is_terminal_and_says_what_the_phone_said() {
        let err = pairing_complete_from(envelope(local_v1::local_envelope::Payload::Error(
            local_v1::ErrorEnvelope {
                code: "internal".into(),
                message: "pairing store lookup failed".into(),
                session_id: String::new(),
            },
        )))
        .expect_err("not a pairing");
        assert!(!is_dial_retriable(&err));
        let text = err.to_string();
        assert!(text.contains("internal"), "{}", text);
        assert!(text.contains("pairing store lookup failed"), "{}", text);
    }

    #[test]
    fn any_other_first_frame_is_terminal() {
        for env in [
            LocalEnvelope { payload: None },
            envelope(local_v1::local_envelope::Payload::PairingStep(
                Default::default(),
            )),
        ] {
            let err = pairing_complete_from(env).expect_err("not a pairing");
            assert!(!is_dial_retriable(&err), "got {:?}", err);
        }
    }

    /// The same regression end to end over a real loopback TLS session: a
    /// "phone" that answers with `Unpaired` must leave the dial retriable.
    #[tokio::test]
    async fn dial_answered_with_unpaired_is_retriable() {
        use crate::phone_signer::pairing::{generate_offer, OfferedKey};
        use prost::Message;
        use rcgen::{CertificateParams, KeyPair, PKCS_ECDSA_P256_SHA256};
        use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
        use std::sync::Arc;
        use tokio::io::AsyncWriteExt;

        let kp = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("keygen");
        let cert = CertificateParams::new(vec!["coincube-phone.local".to_string()])
            .expect("params")
            .self_signed(&kp)
            .expect("self-sign");
        let cfg = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .expect("versions")
        .with_no_client_auth()
        .with_single_cert(
            vec![cert.der().clone()],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(kp.serialize_der())),
        )
        .expect("server cert");
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(cfg));
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let phone = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.expect("accept");
            let mut tls = acceptor.accept(tcp).await.expect("tls accept");
            let body = unpaired().encode_to_vec();
            tls.write_all(&(body.len() as u32).to_be_bytes())
                .await
                .expect("len");
            tls.write_all(&body).await.expect("body");
            let _ = tls.shutdown().await;
        });

        let mut path = std::env::temp_dir();
        path.push(format!("coincube-pairing-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).expect("mkdir tempdir");
        let dir = CoincubeDirectory::new(path.clone());
        let identity = crate::phone_signer::identity::load_or_create(&dir).expect("identity");
        let vault = Fingerprint::from([1, 2, 3, 4]);
        let offer = generate_offer(
            vault,
            &identity,
            "keychain-test".into(),
            OfferedKey::default(),
        )
        .offer;

        let err = try_pair_once(
            &identity,
            &offer,
            &discovered("c5bf643c", &addr.to_string()),
            vault,
            &[vault],
            vault,
            &dir,
            &PairingRun::default(),
        )
        .await
        .expect_err("Unpaired must not pair");
        assert!(is_dial_retriable(&err), "got {:?}", err);

        phone.await.expect("phone task");
        let _ = std::fs::remove_dir_all(&path);
    }

    /// Only ACCEPT names the desktop; every other step leaves it empty.
    #[test]
    fn accept_step_names_this_desktop() {
        use local_v1::pairing_step::Phase;
        let accept = accept_step("tx");
        assert_eq!(accept.phase, Phase::Accept as i32);
        assert_eq!(accept.transaction_id, "tx");
        assert_eq!(accept.desktop_name, desktop_display_name());
        assert!(!accept.desktop_name.is_empty());
        for phase in [Phase::Commit, Phase::Finish] {
            assert!(step("tx", phase).desktop_name.is_empty());
        }
    }

    #[test]
    fn desktop_name_drops_the_mdns_suffix_and_stays_short() {
        assert_eq!(
            clean_desktop_name("Studio-Mac.local (macOS)"),
            "Studio-Mac (macOS)"
        );
        assert_eq!(clean_desktop_name("build-box (Linux)"), "build-box (Linux)");
        assert_eq!(clean_desktop_name("a\u{7}b\n (Linux)"), "ab (Linux)");
        assert_eq!(
            clean_desktop_name(&"x".repeat(200)).chars().count(),
            MAX_DESKTOP_NAME_CHARS
        );
    }
}
