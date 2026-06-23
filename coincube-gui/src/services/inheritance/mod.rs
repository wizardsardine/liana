//! Inheritance heir-escrow — client-side per-keyholder ECIES (COIN-377, rev 3).
//!
//! Server-blind heir recovery: the owner's desktop seals the recovery
//! material to each designated keyholder's xpub; COINCUBE stores opaque
//! ciphertext and only gates its release; the heir's Keychain does the ECDH
//! to decrypt. See the decision record
//! `decisions/2026-06-22-inheritance-ecies-heir-escrow.md` and
//! `plans/PLAN-inheritance-recovery.md`.
//!
//! This module is UI-free and (for [`ecies`]) network-free. The Connect
//! client lives in `services::coincube`; the Keychain ECDH-decrypt call lives
//! in `services::connect::grpc`; the UI state machines live under `app::` and
//! `recover_vault`.

pub mod ecies;
pub mod error;
pub mod escrow;
pub mod heir;
pub mod owner;
pub mod wire;

pub use ecies::{
    open_with_shared_key, seal_to_xpub, ArtifactKind, Envelope, ENCRYPTION_CHILD_DERIVATION,
    HKDF_INFO, SCHEME,
};
pub use error::EciesError;
pub use escrow::{build_escrow_set, keyholders_from_vault, EscrowError, EscrowTier, KeyholderXpub};
pub use heir::{decrypt_envelopes, HeirDecryptError};
pub use owner::{disable_escrow, enroll_escrow, OwnerEscrowError};
pub use wire::{envelope_to_wire, wire_to_envelope};
