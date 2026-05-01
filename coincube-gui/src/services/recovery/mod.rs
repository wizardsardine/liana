//! Cube Recovery Kit — on-device envelope codec and plaintext blob
//! types that back up a Cube (seed + wallet descriptor) to the Connect
//! account.
//!
//! This module is deliberately UI-free and network-free. The Connect
//! client sits in `services::coincube`, and the UI state machine /
//! views live in `app::`.
//!
//! See `plans/PLAN-cube-recovery-kit-desktop.md` for the end-to-end
//! feature plan; this module is W6 (envelope codec + plaintext types).

pub mod envelope;
pub mod error;
pub mod password;
pub mod plaintext;
pub mod restore;

pub use envelope::{decrypt, encrypt, KdfParams, ENVELOPE_VERSION, KDF_ID_ARGON2ID_V1};
pub use error::RecoveryError;
pub use password::{score as score_password, PasswordStrength, MIN_PASSWORD_LEN};
pub use plaintext::{
    DescriptorBlob, DescriptorBlobCube, DescriptorBlobSigner, DescriptorBlobVault, SeedBlob,
    SeedBlobCube, SeedBlobMnemonic, BLOB_VERSION,
};
pub use restore::{
    decrypt_descriptor_blob, decrypt_seed_blob, fetch_and_decrypt_descriptor,
    fetch_and_decrypt_for_install, fetch_and_decrypt_kit, DecryptedKit, RestoreError,
};
