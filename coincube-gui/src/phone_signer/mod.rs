//! Local LAN signer integration: lets a paired Keychain phone show up
//! in the same hardware-wallet list as Jade/Ledger by implementing
//! [`async_hwi::HWI`] against a TLS-over-TCP transport on the LAN.
//!
//! The Connect-mediated signing path in
//! `app/state/vault/keychain_sign.rs` is **not** touched by this
//! module; both paths coexist.
//!
//! See `plans/PLAN-local-signer-lan-desktop.md` for the full design
//! and the companion phone-side plan it links to.

pub mod errors;
pub mod identity;
pub mod mdns;
pub mod pairing;
pub mod pairing_listener;
pub mod pairing_store;
pub mod protocol;
pub mod tls;
pub mod transport;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use coincube_core::miniscript::bitcoin::{
    bip32::{DerivationPath, Fingerprint, Xpub},
    psbt::Psbt,
};
use tokio::sync::Mutex;

use async_hwi::{AddressScript, DeviceKind, Error as HwiError, Version, HWI};

use crate::phone_signer::pairing_store::PairedPhone;
use crate::phone_signer::protocol::{
    cancel_envelope, present_session_envelope, Correlator, SignResponse,
};
use crate::phone_signer::transport::{PairedTransport, PairedWriter};

/// How long [`PhoneSigner::sign_tx`] waits for the phone to send a
/// `PartialSignature` after `PresentSession`. Sized for a slow user
/// (review on device, then confirm) plus a healthy fudge.
const SIGN_RESPONSE_TIMEOUT: Duration = Duration::from_secs(300);

/// Hardware-wallet adapter for a paired phone reachable over the LAN.
///
/// Inside `hw.rs` we wrap this in `Arc<dyn HWI + Send + Sync>` and put
/// it in `HardwareWallet::Supported`, so the PSBT panel hits it via
/// the same `hw.sign_tx(&mut psbt)` call path as Jade/Ledger.
pub struct PhoneSigner {
    /// Write half of the TLS transport. `Mutex` because `sign_tx`
    /// takes `&self` but the framed write cursor is stateful and
    /// needs serialised access. The matching read half lives inside
    /// the [`Correlator`] reader task, so writes never contend with
    /// `recv().await`.
    pub(crate) writer: Arc<Mutex<PairedWriter>>,

    /// Demultiplexer that pulls envelopes off the wire and routes
    /// each `PartialSignature` by `session_id` to the right
    /// `sign_tx` invocation. Owns the read half of the transport.
    pub(crate) correlator: Arc<Correlator>,

    /// Master fingerprint reported by the phone at pair time. We
    /// cache it so `get_master_fingerprint` doesn't have to round-trip
    /// for every refresh tick.
    pub(crate) fingerprint: Fingerprint,

    /// Optional cached app version reported during the last handshake.
    pub(crate) version: Option<Version>,

    /// The persisted record so we can surface a name and reuse the
    /// identity pubkey on re-dial. Also carries
    /// [`PairedPhone::transport_pubkey`], the ECIES key every LAN session is
    /// sealed to.
    pub(crate) paired_phone: PairedPhone,

    /// The vault's canonical descriptor — exactly
    /// `CoincubeDescriptor::to_string()`, checksum included.
    ///
    /// Sealed per session into `descriptor_envelopes` so the phone can verify
    /// its own xpub membership in what it decrypts rather than trusting the
    /// desktop's word. Held here because `HWI::sign_tx` takes only a PSBT, and
    /// `register_wallet` (the trait's one descriptor-shaped hook) is a no-op
    /// for this signer.
    pub(crate) descriptor: String,

    /// `descriptor_id_fingerprint` of [`Self::descriptor`], 8 lowercase hex.
    ///
    /// Derived in [`PhoneSigner::new`] from that exact field rather than passed
    /// in beside it, so the id and the descriptor it names cannot disagree.
    /// That makes the LAN rail's `descriptor_id` the same digest of the same
    /// string as the Cubes list by construction rather than by coincidence —
    /// both reduce to `app::wallet::descriptor_id_fingerprint_of_bytes`.
    pub(crate) descriptor_id: String,

    /// This desktop's Connect transport key. The phone seals its partial
    /// signature back to the public half, which rides the session as
    /// `creator_transport_pubkey`; the secret half opens it here.
    pub(crate) transport_key: Option<Arc<crate::services::connect::crypto::DeviceTransportKey>>,
}

impl std::fmt::Debug for PhoneSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PhoneSigner")
            .field("fingerprint", &self.fingerprint)
            .field("paired_phone", &self.paired_phone.name)
            .finish()
    }
}

impl PhoneSigner {
    /// `descriptor` is the vault's canonical descriptor — exactly
    /// `CoincubeDescriptor::to_string()`, checksum included. It is both what
    /// gets sealed into `descriptor_envelopes` and the preimage of the
    /// session's `descriptor_id`, which is why the id is **derived here rather
    /// than taken as a parameter**: the two are one value, and accepting them
    /// separately let a caller supply a pair that disagree. The phone
    /// recomputes the same digest over the bytes it decrypts and refuses to
    /// sign on a mismatch, so a disagreement is not a cosmetic bug.
    pub fn new(
        transport: PairedTransport,
        fingerprint: Fingerprint,
        version: Option<Version>,
        paired_phone: PairedPhone,
        descriptor: String,
        transport_key: Option<Arc<crate::services::connect::crypto::DeviceTransportKey>>,
    ) -> Self {
        let (reader, writer) = transport.split();
        let correlator = Arc::new(Correlator::spawn(reader));
        let descriptor_id =
            crate::app::wallet::descriptor_id_fingerprint_of_bytes(descriptor.as_bytes())
                .to_string();
        Self {
            writer: Arc::new(Mutex::new(writer)),
            correlator,
            fingerprint,
            version,
            paired_phone,
            descriptor,
            descriptor_id,
            transport_key,
        }
    }

    /// Human-readable name to surface in the signer list. Used by the
    /// view layer to flesh out `HardwareWallet::Supported { alias, .. }`.
    pub fn display_name(&self) -> &str {
        &self.paired_phone.name
    }

    /// `true` while the underlying TLS session is still pumping
    /// envelopes. `hw.rs` checks this before short-circuiting a
    /// re-dial — a phone whose TCP/TLS died but whose mDNS keeps
    /// advertising must be redialled, otherwise the consumer keeps a
    /// stale handle and every sign attempt fails with
    /// `Disconnected` until mDNS times out.
    pub fn is_alive(&self) -> bool {
        self.correlator.is_alive()
    }
}

#[async_trait]
impl HWI for PhoneSigner {
    fn device_kind(&self) -> DeviceKind {
        // async_hwi 0.0.29 has no `Phone` variant. Specter is the
        // closest generic stand-in. Human name surfaces via the
        // `alias` slot on `HardwareWallet::Supported`.
        DeviceKind::Specter
    }

    async fn get_version(&self) -> Result<Version, HwiError> {
        self.version.clone().ok_or(HwiError::UnimplementedMethod)
    }

    async fn get_master_fingerprint(&self) -> Result<Fingerprint, HwiError> {
        Ok(self.fingerprint)
    }

    async fn get_extended_pubkey(&self, _path: &DerivationPath) -> Result<Xpub, HwiError> {
        // Not required for the v1 signer flow: the phone's xpub is
        // captured at vault-build time via the existing Keychain QR
        // path. The LAN signer only needs to sign PSBTs.
        Err(HwiError::UnimplementedMethod)
    }

    async fn register_wallet(
        &self,
        _name: &str,
        _policy: &str,
    ) -> Result<Option<[u8; 32]>, HwiError> {
        Ok(None)
    }

    async fn is_wallet_registered(&self, _name: &str, _policy: &str) -> Result<bool, HwiError> {
        Ok(true)
    }

    async fn display_address(&self, _script: &AddressScript) -> Result<(), HwiError> {
        Err(HwiError::UnimplementedMethod)
    }

    async fn sign_tx(&self, psbt: &mut Psbt) -> Result<(), HwiError> {
        use crate::services::connect::grpc::connect_v1 as cv1;

        let session_id = uuid::Uuid::new_v4().to_string();
        let request_id = uuid::Uuid::new_v4().to_string();
        let psbt_bytes = psbt.serialize();

        // LAN is ECIES_V1, the same as the Connect rail. Both halves of the
        // key exchange must be present or we refuse the session outright:
        // there is deliberately no `descriptor_id`-only path "just for LAN",
        // because the LAN peer is exactly the untrusted party such a fallback
        // would empower. A missing key is a re-pair prompt, not a downgrade.
        let transport_key = self.transport_key.as_ref().ok_or_else(|| {
            HwiError::Device(
                "This computer has no Connect transport key, so the signing request \
                 couldn't be encrypted. Restart COINCUBE and try again."
                    .to_string(),
            )
        })?;
        // Same predicate the pairing gate uses. A stored key can still be
        // unusable here: a row written before that gate existed (empty), or one
        // persisted by an older build that accepted an uncompressed point. Both
        // must produce the actionable re-pair message rather than falling
        // through to a generic "could not encrypt" from `seal_to_device`.
        if !crate::services::connect::crypto::is_sealable_transport_pubkey(
            &self.paired_phone.transport_pubkey,
        ) {
            return Err(HwiError::Device(
                "This Keychain's encryption key is missing or unusable. Pair it again \
                 to sign over the local network."
                    .to_string(),
            ));
        }

        // One envelope each, independent ephemeral key and nonce per call
        // (`seal_to_device` mints both), AAD = label || utf8(request_id).
        let seal = |plaintext: &[u8]| {
            crate::services::connect::crypto::transport::seal_to_device(
                &self.paired_phone.transport_pubkey,
                &request_id,
                plaintext,
            )
        };
        let psbt_envelope = seal(&psbt_bytes).map_err(|e| {
            HwiError::Device(format!(
                "could not encrypt the transaction for this Keychain: {}",
                e
            ))
        })?;
        let descriptor_envelope = seal(self.descriptor.as_bytes()).map_err(|e| {
            HwiError::Device(format!(
                "could not encrypt the descriptor for this Keychain: {}",
                e
            ))
        })?;
        let to_proto = |sealed: crate::services::connect::crypto::transport::SealedPayload| {
            cv1::PayloadEnvelope {
                // LAN has no Connect device registry, so the phone matches its
                // envelope by being the sole target rather than by device id.
                device_id: String::new(),
                ephemeral_pubkey: sealed.ephemeral_pubkey,
                nonce: sealed.nonce,
                ciphertext: sealed.ciphertext,
            }
        };

        let session = cv1::SigningSession {
            session_id: session_id.clone(),
            request_id: request_id.clone(),
            user_id: String::new(),
            vault_id: String::new(),
            // The opaque vault fingerprint, never the descriptor. Same digest
            // of the same string as the Cubes list and the Connect rail.
            descriptor_id: self.descriptor_id.clone(),
            // Empty under ECIES_V1 — the real PSBT rides `psbt_envelopes`.
            psbt: Vec::new(),
            tx_summary: None,
            policy_summary: None,
            targets: vec![cv1::SignerTarget {
                device_id: String::new(),
                key_fingerprint: self.fingerprint.to_string(),
                key_id: String::new(),
                // Echoed back so the phone can confirm the session was sealed
                // to the key it reported at pairing.
                transport_pubkey: self.paired_phone.transport_pubkey.clone(),
            }],
            payload_scheme: cv1::PayloadScheme::EciesV1 as i32,
            psbt_envelopes: vec![to_proto(psbt_envelope)],
            descriptor_envelopes: vec![to_proto(descriptor_envelope)],
            // The key the phone seals its partial signature back to.
            creator_transport_pubkey: transport_key.public_key().to_vec(),
            status: cv1::SessionStatus::Pending as i32,
            created_at: None,
            expires_at: None,
            created_by_device_id: String::new(),
            note: String::new(),
            submitted_signatures: Vec::new(),
            is_recovery_spend: false,
        };

        let envelope = present_session_envelope(session);
        let rx = self.correlator.register(session_id.clone()).await;
        {
            let mut t = self.writer.lock().await;
            if let Err(e) = t.send(&envelope).await {
                self.correlator.cancel(&session_id).await;
                return Err(e);
            }
        }

        let response = match tokio::time::timeout(SIGN_RESPONSE_TIMEOUT, rx).await {
            Ok(Ok(resp)) => resp,
            Ok(Err(_)) => {
                self.correlator.cancel(&session_id).await;
                return Err(HwiError::Device("reader dropped".into()));
            }
            Err(_) => {
                // Timeout — best-effort tell the phone to drop it.
                let cancel = cancel_envelope(&session_id, "desktop timeout");
                let mut t = self.writer.lock().await;
                let _ = t.send(&cancel).await;
                self.correlator.cancel(&session_id).await;
                return Err(HwiError::Device("sign_tx timeout".into()));
            }
        };

        let partial = match response {
            SignResponse::Partial(p) => p,
            SignResponse::Error(msg) => {
                return Err(map_phone_error(msg));
            }
            SignResponse::Disconnected => {
                return Err(HwiError::DeviceDisconnected);
            }
        };

        // The session went out as ECIES_V1, so the signature must come back
        // sealed. A phone that answers with a plaintext `signed_psbt` is either
        // too old to have decrypted the descriptor it was sent, or is trying to
        // walk the session back to plaintext — refuse either way rather than
        // merging signatures we can't attribute to the request we sealed.
        let Some(env) = partial.signature_envelope.as_ref() else {
            return Err(HwiError::Device(
                "This Keychain returned an unencrypted signature for an encrypted \
                 request. Update the Keychain app, then try again."
                    .to_string(),
            ));
        };
        // Opening with this session's `request_id` in the AAD is what stops a
        // signature captured from another session being merged into this one.
        let signed_bytes = transport_key
            .open(
                &env.ephemeral_pubkey,
                &env.nonce,
                &env.ciphertext,
                &request_id,
            )
            .map_err(|e| {
                HwiError::Device(format!("could not decrypt the signed transaction: {}", e))
            })?;
        let signed: Psbt = Psbt::deserialize(&signed_bytes)
            .map_err(|e| HwiError::Device(format!("decode signed psbt: {}", e)))?;

        merge_signatures(psbt, &signed);
        Ok(())
    }
}

/// Translate a phone-reported error string into a friendlier
/// [`HwiError`]. Today the only specific case is `replay_refused:`,
/// emitted by the phone's persistent replay guard when the desktop
/// asks it to sign a `session_id` the phone has already finalised
/// (possibly via the Connect transport). The PSBT panel already
/// distinguishes `HwiError::Device` from `DeviceNotFound`, so we
/// keep it as a Device error and just rewrite the message.
fn map_phone_error(msg: String) -> HwiError {
    if msg.starts_with("replay_refused:") {
        HwiError::Device(format!(
            "Session already signed by this phone — refusing replay. ({})",
            msg,
        ))
    } else {
        HwiError::Device(msg)
    }
}

/// Merge `partial_sigs`, `tap_key_sig`, and `tap_script_sigs` from
/// `signed` into `target`. Mirrors the post-`sign_tx` merge logic in
/// `app::state::vault::psbt::sign_psbt`, so the phone signer behaves
/// like a hardware wallet that signs one path at a time.
fn merge_signatures(target: &mut Psbt, signed: &Psbt) {
    for (i, target_in) in target.inputs.iter_mut().enumerate() {
        if let Some(signed_in) = signed.inputs.get(i) {
            for (pk, sig) in &signed_in.partial_sigs {
                target_in.partial_sigs.insert(*pk, *sig);
            }
            if let Some(tap_key_sig) = signed_in.tap_key_sig {
                target_in.tap_key_sig = Some(tap_key_sig);
            }
            for (k, v) in &signed_in.tap_script_sigs {
                target_in.tap_script_sigs.insert(*k, *v);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_refused_error_gets_friendly_message() {
        let err = map_phone_error("replay_refused: session abc already signed".into());
        let msg = match err {
            HwiError::Device(m) => m,
            other => panic!("expected Device, got {:?}", other),
        };
        assert!(
            msg.starts_with("Session already signed by this phone"),
            "got: {}",
            msg,
        );
        assert!(msg.contains("replay_refused:"), "got: {}", msg);
    }

    #[test]
    fn non_replay_error_passes_through_unchanged() {
        let err = map_phone_error("USER_DECLINED: tap reject".into());
        let msg = match err {
            HwiError::Device(m) => m,
            other => panic!("expected Device, got {:?}", other),
        };
        assert_eq!(msg, "USER_DECLINED: tap reject");
    }
}
