//! Client-side crypto for Connect blinding (`PLAN-connect-blinding.md`).
//!
//! Connect coordinates the Vault but must never be able to *read* it. The
//! pieces here are what make that true at the storage layer rather than by
//! policy:
//!
//! - [`cube_enc_key`] — the Cube's seed-derived encryption keypair and the
//!   xpub-envelope codec a Contact's Keychain seals to it (Track A). The
//!   desktop is the only holder of the private half, and it re-derives it from
//!   the master seed on demand rather than storing it.
//! - [`key_resolve`] — the single point where a Connect-served key row becomes
//!   a usable `Xpub`: open the envelope (or accept a legacy plaintext column),
//!   then run the validation that used to happen server-side.
//! - [`transport`] — the signing rail's per-device transport keypair and the
//!   envelope codec that makes PSBTs and signatures end-to-end (Track B).
//!
//! The curve / KDF / AEAD core is **not** reimplemented here: it lives in
//! [`crate::services::inheritance::ecies`] (the heir-escrow codec, canonical in
//! `plans/SPEC-ecies-v1.md` §1) and is shared through `pub(crate)` primitives.
//! Each new use adds a **domain label**, never a second implementation.

pub mod cube_enc_key;
pub mod key_resolve;
pub mod transport;

pub use cube_enc_key::{
    cube_encryption_path, CubeEncryptionKey, XpubEnvelope, ENC_PURPOSE, RECIPIENT_CUBE_OWNER,
    SCHEME as XPUB_ENVELOPE_SCHEME,
};
pub use key_resolve::{resolve_key_xpub, ConnectKeyRow, KeyResolveError};
pub use transport::{seal_to_device, DeviceTransportKey, SealedPayload};
