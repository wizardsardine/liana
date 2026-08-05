//! The one place a Connect-served key becomes a usable xpub
//! (`PLAN-connect-blinding.md` PR D3).
//!
//! Under Connect blinding a key arrives from the API in one of two shapes:
//!
//! - **blinded** — an [`XpubEnvelope`] sealed by the key owner's Keychain to
//!   this Cube's encryption pubkey. Only the owner's desktop can open it.
//! - **plaintext** — the legacy `xpub` column, still served during the
//!   dual-write window and for keys enrolled before blinding shipped.
//!
//! Everything downstream — the Vault-builder key picker, the descriptor
//! assembly, `classify_signers`, heir-escrow sealing — works on the *decrypted*
//! xpub and is unchanged by blinding. So the whole difference lives here, in
//! one function, and callers get a plain `Xpub` or a typed failure.
//!
//! ## Validation moved client-side
//!
//! The server used to run `crypto.ValidateXPub` at enrolment. It can't validate
//! ciphertext, so those checks move here and run **after** the open:
//!
//! 1. the plaintext parses as an extended public key;
//! 2. its network matches the Cube's;
//! 3. its BIP-32 depth matches the `derivationPath` the row advertises.
//!
//! (3) is a consistency check, not an identity binding. Note what it can*not*
//! do: `models.Key.Fingerprint` is the **origin master** fingerprint (that's
//! how `signers.rs` joins it against descriptor path-info), while an `Xpub`
//! only knows its own and its parent's — so the row's fingerprint cannot be
//! re-derived from the decrypted key and compared. Depth-vs-path is what's
//! actually verifiable here, and it catches a wrong-shaped key (an account xpub
//! served where a master was expected, or vice-versa).
//!
//! Row identity itself is bound **cryptographically**, not by these checks: the
//! envelope's AAD carries `cube_id` *and* `key_id`, so a breached server that
//! moves envelope A onto row B — even within one Cube, where both are readable
//! by this owner — breaks the GCM tag. That is why every resolve needs the key
//! id and why there is no "best effort" path without one.
//!
//! A failure here is **not** a crash and **not** a silent fallback to some
//! other key: it becomes a `envelope_invalid` report to the API (which pushes a
//! re-enrol prompt to the Contact's Keychain) and a "key needs re-enrolment"
//! state in the builder.

use coincube_core::miniscript::bitcoin::bip32::{DerivationPath, Xpub};
use coincube_core::miniscript::bitcoin::Network;
use std::str::FromStr;

use super::cube_enc_key::{CubeEncryptionKey, XpubEnvelope};
use crate::services::coincube::{CubeKeyRaw, VaultMemberKeySummary};

/// Why a Connect-served key couldn't be turned into a usable xpub.
///
/// Every variant except [`Self::Locked`] means *this key* is unusable and the
/// owner should be pushed to have it re-enrolled; `Locked` is a local, fixable
/// condition and must not be reported to the server as the key's fault.
#[derive(Debug, Clone)]
pub enum KeyResolveError {
    /// The row carries neither an envelope nor a plaintext xpub. Either the key
    /// was never enrolled properly, or an envelope-mode API served a row this
    /// client's Cube can't read.
    Missing,
    /// The Cube's encryption key isn't available on this device, so a blinded
    /// key can't be opened. Happens on a watch-only / descriptor-only restore
    /// (no seed → no key). **Local condition** — never reported as
    /// `envelope_invalid`.
    Locked,
    /// The envelope failed to open, or was structurally malformed. Carries the
    /// rendered `EciesError` rather than the error itself: this type has to
    /// be `Clone` to ride an iced `Message`, and the detail is only ever
    /// logged. Wrong key, tampered ciphertext, and a re-targeted AAD are one
    /// indistinguishable case by design.
    Envelope(String),
    /// The decrypted (or plaintext) value isn't a valid extended public key.
    NotAnXpub,
    /// The key is for a different Bitcoin network than this Cube.
    WrongNetwork { expected: Network, found: Network },
    /// The decrypted xpub's BIP-32 depth doesn't match the `derivationPath`
    /// the row advertises — a wrong-shaped key (e.g. a master xpub served where
    /// an account xpub was declared).
    DepthMismatch {
        path: String,
        declared_depth: usize,
        actual_depth: u8,
    },
}

impl KeyResolveError {
    /// Whether this failure is the *key's* fault and should be reported to
    /// Connect as `envelope_invalid`, pushing a re-enrol prompt to its owner.
    ///
    /// [`Self::Locked`] is excluded deliberately: the envelope is probably
    /// fine, this device just can't read it. Reporting it would make a
    /// watch-only restore invalidate every key in the Vault.
    pub fn should_report_invalid(&self) -> bool {
        !matches!(self, Self::Locked)
    }

    /// The machine tag for `POST …/keys/{keyId}/envelope-invalid`.
    ///
    /// `coincube-api` accepts a closed set — the value is echoed into the audit
    /// trail and the keyholder's re-enrol email, so free text would be an
    /// injection surface. The split is "the ciphertext wouldn't open" vs "it
    /// opened but the plaintext was wrong", which is what tells the keyholder
    /// whether re-sealing the same key will actually help.
    pub fn report_reason(&self) -> &'static str {
        match self {
            // Opened fine; the plaintext failed the checks that moved
            // client-side when the server lost the ability to run them.
            Self::NotAnXpub | Self::WrongNetwork { .. } | Self::DepthMismatch { .. } => {
                "xpub_invalid"
            }
            // Everything else is a transport/ciphertext failure. `Locked` never
            // reaches the wire (see `should_report_invalid`); it maps here only
            // so this function is total.
            Self::Envelope(_) | Self::Missing | Self::Locked => "decrypt_failed",
        }
    }

    /// Short, user-facing explanation for the Vault builder.
    pub fn user_message(&self, key_name: &str) -> String {
        match self {
            Self::Locked => format!(
                "“{}” is encrypted to this Cube's key, which isn't available on this device. \
                 Restore this Cube from its seed to use Keychain keys.",
                key_name
            ),
            Self::WrongNetwork { .. } => {
                format!("“{}” is for a different Bitcoin network.", key_name)
            }
            _ => format!(
                "“{}” couldn't be read and needs re-sharing. Ask its owner to re-share the key \
                 from their Keychain app.",
                key_name
            ),
        }
    }
}

impl std::fmt::Display for KeyResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => write!(f, "key has neither an xpub envelope nor a plaintext xpub"),
            Self::Locked => write!(
                f,
                "this Cube's encryption key is not available on this device"
            ),
            Self::Envelope(e) => write!(f, "xpub envelope could not be opened: {}", e),
            Self::NotAnXpub => write!(f, "decrypted key is not a valid extended public key"),
            Self::WrongNetwork { expected, found } => {
                write!(f, "key is for {} but this Cube is on {}", found, expected)
            }
            Self::DepthMismatch {
                path,
                declared_depth,
                actual_depth,
            } => write!(
                f,
                "key depth mismatch: row declares {:?} ({} levels) but the decrypted key is at \
                 depth {}",
                path, declared_depth, actual_depth
            ),
        }
    }
}

impl std::error::Error for KeyResolveError {}

/// The blinding-agnostic view of a Connect key row: whichever of the two
/// shapes the API served, plus the metadata the checks compare against.
///
/// Implemented for both key DTOs so [`resolve_key_xpub`] has exactly one body.
pub trait ConnectKeyRow {
    fn envelope(&self) -> Option<&XpubEnvelope>;
    fn plaintext_xpub(&self) -> &str;
    /// The row's advertised BIP-32 derivation path (`models.Key
    /// .DerivationPath`). Both DTOs carry it; it is not secret, so it stays a
    /// plaintext column under blinding.
    fn declared_derivation_path(&self) -> &str;
    /// The `models.Key` id. Bound into the envelope AAD, so it is required to
    /// open one at all — not optional metadata.
    fn key_id(&self) -> u64;
}

impl ConnectKeyRow for CubeKeyRaw {
    fn envelope(&self) -> Option<&XpubEnvelope> {
        self.xpub_envelope.as_ref()
    }
    fn plaintext_xpub(&self) -> &str {
        &self.xpub
    }
    fn declared_derivation_path(&self) -> &str {
        &self.derivation_path
    }
    fn key_id(&self) -> u64 {
        self.id
    }
}

impl ConnectKeyRow for VaultMemberKeySummary {
    fn envelope(&self) -> Option<&XpubEnvelope> {
        self.xpub_envelope.as_ref()
    }
    fn plaintext_xpub(&self) -> &str {
        &self.xpub
    }
    fn declared_derivation_path(&self) -> &str {
        &self.derivation_path
    }
    fn key_id(&self) -> u64 {
        self.id
    }
}

/// Turns a Connect-served key row into a validated [`Xpub`].
///
/// Prefers the envelope whenever one is present — an enrolled-and-blinded key
/// must never silently fall back to a stale plaintext column (master I5: no
/// plaintext downgrade). `cube_enc_key` is this Cube's seed-derived encryption
/// key; `cube_id` is the **server** Cube id, rebuilt into the envelope AAD.
///
/// The plaintext branch runs the *same* post-decrypt validation, so a bad
/// legacy row fails identically to a bad envelope and both reach the same
/// re-enrol surface.
pub fn resolve_key_xpub<R: ConnectKeyRow + ?Sized>(
    row: &R,
    cube_enc_key: Option<&CubeEncryptionKey>,
    cube_id: u64,
    network: Network,
) -> Result<Xpub, KeyResolveError> {
    let xpub_str = match row.envelope() {
        Some(env) => {
            let key = cube_enc_key.ok_or(KeyResolveError::Locked)?;
            let plaintext = key
                .open(env, cube_id, row.key_id())
                .map_err(|e| KeyResolveError::Envelope(e.to_string()))?;
            // The plaintext is a base58 xpub string; it lives in the zeroizing
            // buffer until this scope ends.
            String::from_utf8(plaintext.to_vec()).map_err(|_| KeyResolveError::NotAnXpub)?
        }
        None => {
            if row.plaintext_xpub().is_empty() {
                return Err(KeyResolveError::Missing);
            }
            row.plaintext_xpub().to_string()
        }
    };

    let xpub = Xpub::from_str(&xpub_str).map_err(|_| KeyResolveError::NotAnXpub)?;
    validate_xpub(&xpub, row.declared_derivation_path(), network)?;
    Ok(xpub)
}

/// The checks `crypto.ValidateXPub` used to run server-side, plus the
/// depth-vs-path consistency check (module docs).
fn validate_xpub(
    xpub: &Xpub,
    declared_derivation_path: &str,
    network: Network,
) -> Result<(), KeyResolveError> {
    if !network_matches(xpub.network, network) {
        return Err(KeyResolveError::WrongNetwork {
            expected: network,
            found: network_of(xpub.network),
        });
    }
    // An unparseable path is the row's problem, not this key's shape — the
    // caller (`escrow.rs`) already fails closed on it separately, and the
    // builder would reject it when building the descriptor origin. Skip rather
    // than double-report.
    if let Ok(path) = DerivationPath::from_str(normalize_path(declared_derivation_path)) {
        let declared_depth = path.len();
        if usize::from(xpub.depth) != declared_depth {
            return Err(KeyResolveError::DepthMismatch {
                path: declared_derivation_path.to_string(),
                declared_depth,
                actual_depth: xpub.depth,
            });
        }
    }
    Ok(())
}

/// `DerivationPath::from_str` rejects the `m/` prefix; the API serves paths
/// both ways. Strip it so both forms parse (matching `escrow.rs`).
fn normalize_path(path: &str) -> &str {
    let trimmed = path.trim();
    trimmed
        .strip_prefix("m/")
        .or_else(|| trimmed.strip_prefix("M/"))
        .unwrap_or(trimmed)
}

/// Testnet, signet and regtest share one BIP-32 version prefix, so an `Xpub`
/// can only distinguish "mainnet" from "some test network". Compare at that
/// granularity rather than rejecting a perfectly good regtest key for
/// presenting itself as testnet.
fn network_matches(
    xpub_network: coincube_core::miniscript::bitcoin::NetworkKind,
    cube: Network,
) -> bool {
    use coincube_core::miniscript::bitcoin::NetworkKind;
    match xpub_network {
        NetworkKind::Main => cube == Network::Bitcoin,
        NetworkKind::Test => cube != Network::Bitcoin,
    }
}

fn network_of(kind: coincube_core::miniscript::bitcoin::NetworkKind) -> Network {
    use coincube_core::miniscript::bitcoin::NetworkKind;
    match kind {
        NetworkKind::Main => Network::Bitcoin,
        NetworkKind::Test => Network::Testnet,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use coincube_core::signer::MasterSigner;

    const MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
    /// `m/48'/1'/0'/2'` of the vector mnemonic — a real testnet account xpub.
    const XPUB: &str = "tpubDFH9dgzveyD8zTbPUFuLrGmCydNvxehyNdUXKJAQN8x4aZ4j6UZqGfnqFrD4NqyaTVGKbvEW54tsvPTK2UoSbCC1PJY8iCNiwTL3RWZEheQ";
    /// The §8 KAT envelope: the xpub above, sealed to the Cube key derived from
    /// the same mnemonic, bound to cube 42 / key 7.
    const KAT_E: &str = "032c0b7cf95324a07d05398b240174dc0c2be444d96b159aa6c7f7b1e668680991";
    const KAT_NONCE: &str = "0000000000000000cafebabe";
    const KAT_CT: &str = "fc13b1b9639e00e163b3664b62f516ad49d7f19c5383a758706ca813fa8e236cf14a4189aa61ee94801d31cb26a14a999eb5ea2c90a53bc704c5b262ff2b4cf984e97d7c92d13069b829b972c501190db9eaba00b8df84a25c78125e602cff3b037c7db65974b063084596a64667d5f92d647067c3c5453237d7e9e3573a57";
    const KAT_CUBE_ID: u64 = 42;
    /// Must match the fixture row's `id` — the AAD binds it.
    const KAT_KEY_ID: u64 = 7;

    fn cube_key() -> CubeEncryptionKey {
        let signer = MasterSigner::from_str(Network::Testnet, MNEMONIC).unwrap();
        CubeEncryptionKey::derive(&signer, Network::Testnet)
    }

    fn envelope() -> XpubEnvelope {
        XpubEnvelope {
            scheme: super::super::cube_enc_key::SCHEME.to_string(),
            recipient: super::super::cube_enc_key::RECIPIENT_CUBE_OWNER.to_string(),
            ephemeral_pubkey: KAT_E.to_string(),
            nonce: KAT_NONCE.to_string(),
            ciphertext: KAT_CT.to_string(),
        }
    }

    /// The row's declared derivation path — `m/48'/1'/0'/2'`, 4 levels, which
    /// is the depth of [`XPUB`].
    const PATH: &str = "m/48'/1'/0'/2'";

    fn row(envelope: Option<XpubEnvelope>, xpub: &str, derivation_path: &str) -> CubeKeyRaw {
        CubeKeyRaw {
            id: KAT_KEY_ID,
            name: "Kenji's phone".to_string(),
            xpub: xpub.to_string(),
            xpub_envelope: envelope,
            fingerprint: "73c5da0a".to_string(),
            derivation_path: derivation_path.to_string(),
            network: "testnet".to_string(),
            status: "active".to_string(),
            primary_owner_id: 0,
            keychain_id: None,
            curve: String::new(),
            taproot: false,
            cube_id: 0,
            created_at: String::new(),
            updated_at: String::new(),
            owner_user_id: 0,
            owner_email: String::new(),
            is_own_key: false,
            used_by_vault: false,
            recovery_role: String::new(),
        }
    }

    #[test]
    fn opens_a_blinded_key_and_validates_it() {
        let resolved = resolve_key_xpub(
            &row(Some(envelope()), "", PATH),
            Some(&cube_key()),
            KAT_CUBE_ID,
            Network::Testnet,
        )
        .unwrap();
        assert_eq!(resolved.to_string(), XPUB);
    }

    #[test]
    fn accepts_a_legacy_plaintext_row() {
        // Dual-write window / pre-blinding keys keep working unchanged.
        let resolved = resolve_key_xpub(
            &row(None, XPUB, PATH),
            Some(&cube_key()),
            KAT_CUBE_ID,
            Network::Testnet,
        )
        .unwrap();
        assert_eq!(resolved.to_string(), XPUB);
    }

    #[test]
    fn envelope_wins_over_a_stale_plaintext_column() {
        // Master I5, no plaintext downgrade: with both present we must open the
        // envelope, not trust whatever the plaintext column still says. Both
        // shapes here carry the same xpub, so the way to tell which branch ran
        // is to make the *envelope* branch the one that fails: a wrong cube id
        // breaks the AAD, and only the envelope branch consults it.
        let err = resolve_key_xpub(
            &row(Some(envelope()), XPUB, PATH),
            Some(&cube_key()),
            KAT_CUBE_ID + 1,
            Network::Testnet,
        )
        .unwrap_err();
        assert!(matches!(err, KeyResolveError::Envelope(_)));
    }

    #[test]
    fn the_key_id_is_taken_from_the_row_and_bound_into_the_aad() {
        // The API binds `key_id` in the AAD, so the row a resolve is called for
        // is cryptographically part of the open. A row claiming a different id
        // cannot open an envelope sealed for key 7 — this is what stops a
        // breached server swapping envelopes between two keys of one Cube,
        // both of which this owner can otherwise read.
        let mut wrong_row = row(Some(envelope()), "", PATH);
        wrong_row.id = KAT_KEY_ID + 1;
        let err = resolve_key_xpub(&wrong_row, Some(&cube_key()), KAT_CUBE_ID, Network::Testnet)
            .unwrap_err();
        assert!(matches!(err, KeyResolveError::Envelope(_)));
        assert!(err.should_report_invalid());
        assert_eq!(err.report_reason(), "decrypt_failed");
    }

    #[test]
    fn report_reasons_match_the_api_closed_set() {
        // The API rejects anything outside {decrypt_failed, xpub_invalid}, and
        // the split has to be meaningful: "re-sealing will help" vs "the key
        // itself is wrong".
        let ciphertext_failure = resolve_key_xpub(
            &row(Some(envelope()), "", PATH),
            Some(&cube_key()),
            KAT_CUBE_ID + 1,
            Network::Testnet,
        )
        .unwrap_err();
        assert_eq!(ciphertext_failure.report_reason(), "decrypt_failed");

        let plaintext_failure = resolve_key_xpub(
            &row(None, XPUB, PATH),
            Some(&cube_key()),
            KAT_CUBE_ID,
            Network::Bitcoin,
        )
        .unwrap_err();
        assert_eq!(plaintext_failure.report_reason(), "xpub_invalid");

        let garbage = resolve_key_xpub(
            &row(None, "not an xpub", PATH),
            Some(&cube_key()),
            KAT_CUBE_ID,
            Network::Testnet,
        )
        .unwrap_err();
        assert_eq!(garbage.report_reason(), "xpub_invalid");
    }

    #[test]
    fn depth_must_match_the_declared_derivation_path() {
        // A 4-level account xpub served against a 3-level path — the row and
        // the key disagree about what this key is.
        let err = resolve_key_xpub(
            &row(Some(envelope()), "", "m/48'/1'/0'"),
            Some(&cube_key()),
            KAT_CUBE_ID,
            Network::Testnet,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            KeyResolveError::DepthMismatch {
                declared_depth: 3,
                actual_depth: 4,
                ..
            }
        ));
        assert!(err.should_report_invalid());
    }

    #[test]
    fn bare_and_m_prefixed_paths_both_parse() {
        for path in ["m/48'/1'/0'/2'", "48'/1'/0'/2'", "48h/1h/0h/2h"] {
            assert!(
                resolve_key_xpub(&row(None, XPUB, path), None, KAT_CUBE_ID, Network::Testnet)
                    .is_ok(),
                "path {} should resolve",
                path
            );
        }
    }

    #[test]
    fn an_unparseable_path_does_not_block_a_good_key() {
        // The depth check is a consistency check, not a gate on the metadata:
        // a path we can't parse is reported by the caller that actually needs
        // it, not conflated with "this key is bad".
        assert!(resolve_key_xpub(
            &row(None, XPUB, "not a path"),
            None,
            KAT_CUBE_ID,
            Network::Testnet
        )
        .is_ok());
    }

    #[test]
    fn a_blinded_key_without_the_cube_key_is_locked_not_invalid() {
        let err = resolve_key_xpub(
            &row(Some(envelope()), "", PATH),
            None,
            KAT_CUBE_ID,
            Network::Testnet,
        )
        .unwrap_err();
        assert!(matches!(err, KeyResolveError::Locked));
        // Crucially: a watch-only restore must not flag everyone's keys as bad.
        assert!(!err.should_report_invalid());
    }

    #[test]
    fn a_retargeted_envelope_fails_and_is_reportable() {
        let err = resolve_key_xpub(
            &row(Some(envelope()), "", PATH),
            Some(&cube_key()),
            KAT_CUBE_ID + 1, // server re-pointed the envelope at another Cube
            Network::Testnet,
        )
        .unwrap_err();
        assert!(matches!(err, KeyResolveError::Envelope(_)));
        assert!(err.should_report_invalid());
    }

    #[test]
    fn wrong_network_is_rejected() {
        let err = resolve_key_xpub(
            &row(None, XPUB, PATH),
            Some(&cube_key()),
            KAT_CUBE_ID,
            Network::Bitcoin,
        )
        .unwrap_err();
        assert!(matches!(err, KeyResolveError::WrongNetwork { .. }));
    }

    #[test]
    fn regtest_and_signet_accept_testnet_version_bytes() {
        // One version prefix covers all three test networks — a regtest Cube
        // must not reject a perfectly good tpub.
        for net in [Network::Testnet, Network::Signet, Network::Regtest] {
            assert!(resolve_key_xpub(&row(None, XPUB, PATH), None, KAT_CUBE_ID, net,).is_ok());
        }
    }

    #[test]
    fn a_row_with_neither_shape_is_missing() {
        let err = resolve_key_xpub(
            &row(None, "", PATH),
            Some(&cube_key()),
            KAT_CUBE_ID,
            Network::Testnet,
        )
        .unwrap_err();
        assert!(matches!(err, KeyResolveError::Missing));
    }

    #[test]
    fn garbage_plaintext_is_not_an_xpub() {
        let err = resolve_key_xpub(
            &row(None, "definitely not an xpub", PATH),
            Some(&cube_key()),
            KAT_CUBE_ID,
            Network::Testnet,
        )
        .unwrap_err();
        assert!(matches!(err, KeyResolveError::NotAnXpub));
    }

    #[test]
    fn vault_member_summary_resolves_without_a_fingerprint_check() {
        let summary = VaultMemberKeySummary {
            id: KAT_KEY_ID,
            name: "Kenji's phone".to_string(),
            xpub: String::new(),
            xpub_envelope: Some(envelope()),
            derivation_path: "m/48'/1'/0'/2'".to_string(),
        };
        let resolved =
            resolve_key_xpub(&summary, Some(&cube_key()), KAT_CUBE_ID, Network::Testnet).unwrap();
        assert_eq!(resolved.to_string(), XPUB);
    }
}
