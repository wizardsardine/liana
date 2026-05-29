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
    /// identity pubkey on re-dial.
    pub(crate) paired_phone: PairedPhone,
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
    pub fn new(
        transport: PairedTransport,
        fingerprint: Fingerprint,
        version: Option<Version>,
        paired_phone: PairedPhone,
    ) -> Self {
        let (reader, writer) = transport.split();
        let correlator = Arc::new(Correlator::spawn(reader));
        Self {
            writer: Arc::new(Mutex::new(writer)),
            correlator,
            fingerprint,
            version,
            paired_phone,
        }
    }

    /// Human-readable name to surface in the signer list. Used by the
    /// view layer to flesh out `HardwareWallet::Supported { alias, .. }`.
    pub fn display_name(&self) -> &str {
        &self.paired_phone.name
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

        let session = cv1::SigningSession {
            session_id: session_id.clone(),
            request_id,
            user_id: String::new(),
            vault_id: String::new(),
            descriptor_id: String::new(),
            psbt: psbt_bytes,
            tx_summary: None,
            policy_summary: None,
            targets: vec![cv1::SignerTarget {
                device_id: String::new(),
                key_fingerprint: self.fingerprint.to_string(),
                key_id: String::new(),
            }],
            status: cv1::SessionStatus::Pending as i32,
            created_at: None,
            expires_at: None,
            created_by_device_id: String::new(),
            note: String::new(),
            submitted_signatures: Vec::new(),
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

        let signed: Psbt = Psbt::deserialize(&partial.signed_psbt)
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
