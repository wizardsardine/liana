//! The Cube's encryption keypair and the xpub-envelope codec (Track A of
//! `plans/PLAN-connect-blinding.md`, PR D1).
//!
//! ## Why
//!
//! `models.Key.XPub` used to be a plaintext column on Connect. An xpub is a
//! permanent watch key — a single DB snapshot exposes a Vault's composition,
//! balances, and *every future address, forever*. Under Connect blinding a
//! Contact's Keychain encrypts its xpub **on the phone** to the inviting
//! owner's Cube-scoped encryption pubkey; Connect stores and routes only the
//! ciphertext, and the owner's desktop opens it in memory at Vault-build time.
//!
//! ## The key (master invariants I2 / I3)
//!
//! The keypair is **derived from the Cube's master seed**, not stored:
//!
//! ```text
//! m / 7000' / coin_type' / 0'      coin_type = 0' mainnet, 1' otherwise
//! ```
//!
//! - `7000'` is a dedicated hardened purpose reserved for Connect-blinding
//!   encryption. It is not a BIP-43 purpose any wallet uses (44/45/48/49/84/86/
//!   87) and it is **hardened**, so it can never collide with, or be reached
//!   from, any signing path or descriptor (I3 — key separation). Nothing under
//!   it is ever exported as an xpub or written into a descriptor. The number
//!   echoes the inheritance codec's reserved *non-hardened* child `7000`
//!   ([`crate::services::inheritance::ecies::ENCRYPTION_CHILD_INDEX`]) so the
//!   two reservations read as one convention.
//! - Seed-derived means **no new backup artifact**: the Cube Recovery Kit
//!   already escrows the seed, so any seed restore re-derives this key. It also
//!   means a duress wipe needs no special handling (master I6) — the key ceases
//!   to exist along with the seed.
//! - Derive on demand post-unlock, use, drop. The private scalar lives in a
//!   [`Zeroizing`] buffer and is never persisted, logged, or sent anywhere.
//!   Only the 33-byte public half leaves the machine (PR D2 registers it).
//! - Descriptor-only (watch-only) restores have no seed and therefore no key.
//!   That is accepted: they can't build a Vault either.
//!
//! ## The envelope — BINDING CONTRACT
//!
//! An xpub envelope **is** a `SPEC-ecies-v1` envelope. Same scheme id, same
//! label, same HKDF info — the *only* difference is a new `artifact_kind` byte
//! `0x03` in the AAD ([`ArtifactKind::Xpub`]):
//!
//! ```text
//! P   = the Cube's encryption pubkey (compressed SEC1, 33B)   ← registered
//! (e, E) = fresh ephemeral secp256k1 keypair, per envelope    ← sealer side
//! ikm = compressed_SEC1( e · P )      // 33B RAW point, NOT SHA256'd
//! K   = HKDF-SHA256( salt = 0x00*32, ikm,
//!                    info = "coincube-inheritance-ecies-v1\x00" ‖ E ‖ P, L=32 )
//! AAD = "coincube-inheritance-ecies-v1\x00" ‖ 0x03
//!        ‖ cube_id (u64 BE) ‖ key_id (u64 BE)
//! ct‖tag = AES-256-GCM-Seal( K, nonce = random 12B, aad = AAD, xpub )
//! ```
//!
//! The authoritative byte layout is `coincube-api`'s `crypto/ecies.go`
//! (`ECIESAAD` + `Seal`), pinned there to the same `SPEC-ecies-v1` §7.1
//! known-answer vector this module's `kat_*` tests pin. **This is load-bearing**:
//! the A5 one-shot (`cmd/blind_xpubs`) seals existing rows server-side with that
//! code and there is no decrypt counterpart on the server, so a divergent Rust
//! open would brick every migrated key with no way to tell until Vault-build
//! time. Do not "improve" the construction here.
//!
//! Consequences of using the shared construction rather than a private one:
//!
//! - **Domain separation is by `artifact_kind`, not by label.** `0x03` is what
//!   stops an xpub envelope authenticating as a descriptor or seed envelope,
//!   and vice-versa — the AAD differs, so the GCM tag fails.
//! - **`key_id` is bound.** The AAD carries both the Cube id and the
//!   `models.Key` id, so a breached server cannot re-target a stored envelope
//!   at another Cube *or* at another key row within the same Cube. (The v1
//!   plan-draft AAD bound only the Cube; the API contract is strictly
//!   stronger.)
//!
//! Still not bound, and checked after the open instead: the key's fingerprint
//! and derivation path, which are server-supplied plaintext metadata — see the
//! Vault-builder consumption path ([`super::key_resolve`], PR D3).

use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use coincube_core::miniscript::bitcoin::bip32::{ChildNumber, DerivationPath};
use coincube_core::miniscript::bitcoin::secp256k1::{PublicKey, Secp256k1, SecretKey};
use coincube_core::miniscript::bitcoin::Network;
use coincube_core::signer::MasterSigner;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::services::inheritance::ecies::{
    aad_bytes, ecdh_ikm, hkdf_key, ArtifactKind, ECIES_LABEL, NONCE_LEN, PUBKEY_LEN, TAG_LEN,
};
use crate::services::inheritance::EciesError;

/// Wire identifier for the xpub envelope scheme — the **same** id the
/// heir-escrow envelopes use (`coincube-api`'s `models.EnvelopeScheme`), because
/// it is the same construction. The API pins this string on enrolment, so a
/// change here is a cross-repo wire break.
pub const SCHEME: &str = crate::services::inheritance::ecies::SCHEME;

/// Reserved **hardened** BIP-32 purpose for the Cube's Connect-blinding
/// encryption key. See the module docs for why it can't collide with a signing
/// path.
pub const ENC_PURPOSE: u32 = 7000;

/// The full derivation path of the Cube encryption key on `network`:
/// `m/7000'/coin_type'/0'`, with the BIP-44 coin type (`0'` mainnet, `1'`
/// everywhere else — testnet, signet, and regtest all share `1'`, as elsewhere
/// in the wallet).
///
/// Exposed so callers can log or display the path without duplicating the
/// constant; the derivation itself is [`CubeEncryptionKey::derive`].
pub fn cube_encryption_path(network: Network) -> DerivationPath {
    let coin_type = if network == Network::Bitcoin { 0 } else { 1 };
    // All three indices are well under 2^31, so hardening them cannot fail.
    let path: Vec<ChildNumber> = [ENC_PURPOSE, coin_type, 0]
        .iter()
        .map(|i| {
            ChildNumber::from_hardened_idx(*i).expect("index is below the hardened-derivation cap")
        })
        .collect();
    DerivationPath::from(path)
}

/// The Cube's Connect-blinding encryption keypair, derived from the master seed
/// (module docs). The private scalar is zeroized on drop and never persisted;
/// hold one only for as long as a decrypt batch needs it.
pub struct CubeEncryptionKey {
    secret: Zeroizing<[u8; 32]>,
    /// Compressed SEC1 (33-byte) public key — the value PR D2 registers with
    /// Connect and the Keychain seals to.
    public: [u8; PUBKEY_LEN],
}

impl CubeEncryptionKey {
    /// Derives the Cube's encryption keypair from an unlocked master signer.
    ///
    /// `signer` must be the Cube's **master seed** signer (the one behind
    /// `CubeSettings::master_signer_fingerprint`), not a Vault hot signer or
    /// the Liquid/Spark signer — a different seed derives a different key and
    /// every envelope sealed to the registered pubkey would fail to open.
    pub fn derive(signer: &MasterSigner, network: Network) -> Self {
        let secp = Secp256k1::new();
        let xpriv = signer.xpriv_at(&cube_encryption_path(network), &secp);
        let sk = xpriv.private_key;
        let pk = PublicKey::from_secret_key(&secp, &sk);
        let mut secret = Zeroizing::new([0u8; 32]);
        secret.copy_from_slice(&sk.secret_bytes());
        Self {
            secret,
            public: pk.serialize(),
        }
    }

    /// The compressed-SEC1 public key: what gets registered with Connect and
    /// attached to invites. Public material — safe to log.
    pub fn public_key(&self) -> [u8; PUBKEY_LEN] {
        self.public
    }

    /// The registered public key as lowercase hex (33 bytes → 66 chars), the
    /// form the API stores and serves.
    pub fn public_key_hex(&self) -> String {
        hex::encode(self.public)
    }

    /// Opens an xpub envelope sealed to this Cube.
    ///
    /// `cube_id` is the **server** Cube id (`models.ConnectCube.ID`) and
    /// `key_id` the `models.Key` id the envelope is enrolled against. Both are
    /// rebuilt into the AAD alongside [`ArtifactKind::Xpub`], so an envelope
    /// re-targeted at another Cube *or* another key row fails closed rather
    /// than returning a mis-attributed xpub.
    ///
    /// The plaintext lands in a [`Zeroizing`] buffer wiped on drop. A wrong key,
    /// a tampered ciphertext, and a tampered/re-targeted AAD are all
    /// indistinguishable [`EciesError::BadKeyOrCorrupt`] — deliberately, so the
    /// error surface is not a decryption oracle.
    pub fn open(
        &self,
        env: &XpubEnvelope,
        cube_id: u64,
        key_id: u64,
    ) -> Result<Zeroizing<Vec<u8>>, EciesError> {
        if env.scheme != SCHEME {
            return Err(EciesError::UnsupportedScheme(env.scheme.clone()));
        }
        let eph_pub_bytes = hex_field(&env.ephemeral_pubkey, "ephemeral pubkey")?;
        let nonce = hex_field(&env.nonce, "nonce")?;
        let ciphertext = hex_field(&env.ciphertext, "ciphertext")?;

        if eph_pub_bytes.len() != PUBKEY_LEN {
            return Err(EciesError::MalformedEnvelope("ephemeral pubkey length"));
        }
        if nonce.len() != NONCE_LEN {
            return Err(EciesError::MalformedEnvelope("nonce length"));
        }
        if ciphertext.len() < TAG_LEN {
            return Err(EciesError::MalformedEnvelope("ciphertext shorter than tag"));
        }

        let eph_pub = PublicKey::from_slice(&eph_pub_bytes)
            .map_err(|_| EciesError::MalformedEnvelope("ephemeral pubkey"))?;
        let d = SecretKey::from_slice(self.secret.as_ref())
            .map_err(|_| EciesError::MalformedEnvelope("cube encryption secret"))?;

        // ikm = d · E (== e · P); HKDF info = label ‖ E ‖ P (SPEC §1, verbatim).
        let ikm = ecdh_ikm(&eph_pub, &d);
        let key = hkdf_key(ECIES_LABEL, ikm.as_ref(), &eph_pub_bytes, &self.public);

        let cipher = Aes256Gcm::new_from_slice(key.as_ref()).map_err(EciesError::Cipher)?;
        let pt = cipher
            .decrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &ciphertext,
                    aad: &aad_bytes(ArtifactKind::Xpub, cube_id, key_id),
                },
            )
            .map_err(|_| EciesError::BadKeyOrCorrupt)?;
        Ok(Zeroizing::new(pt))
    }
}

impl std::fmt::Debug for CubeEncryptionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CubeEncryptionKey")
            .field("public", &hex::encode(self.public))
            .field("secret", &"<redacted>")
            .finish()
    }
}

/// Hex-decodes a wire field, collapsing any malformed input to a fail-closed
/// [`EciesError::MalformedEnvelope`] naming the field (non-sensitive).
fn hex_field(s: &str, field: &'static str) -> Result<Vec<u8>, EciesError> {
    hex::decode(s).map_err(|_| EciesError::MalformedEnvelope(field))
}

/// The only recipient class in v1: the owner of the Cube the key is enrolled
/// onto. Mirrors `coincube-api`'s `models.XPubEnvelopeRecipientCubeOwner`; the
/// column exists so a later multi-reader fan-out is an insert rather than a
/// schema change.
pub const RECIPIENT_CUBE_OWNER: &str = "cube-owner";

/// The xpub envelope as it rides the Connect REST API — the shape the Keychain
/// `POST /keys` uploads and every key/member listing serves in place of the
/// retired plaintext `xpub` field. Mirrors `coincube-api`'s
/// `xpubenvelope.Wire`.
///
/// Byte fields are **lowercase hex**, matching the inheritance envelope wire
/// convention (`SPEC-ecies-v1.md` §5) so `coincube-api` can `hex.DecodeString`
/// both families with one code path. There is deliberately no `artifactKind`
/// field (there is exactly one artifact here — the kind is fixed at `0x03` in
/// the AAD) and no `derivation` (the recipient key is the owner's own, derived
/// from their seed, not a BIP-32 child the reader must locate).
///
/// `Debug` is manual: the ciphertext is encrypted, but there is no reason to
/// dump key material shaped bytes into any `{:?}` site.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct XpubEnvelope {
    pub scheme: String,
    /// Which reader this envelope is for — always [`RECIPIENT_CUBE_OWNER`] in
    /// v1. Response-only on the API side, so `#[serde(default)]` keeps a
    /// request-shaped payload (or an older server) parsing.
    #[serde(default)]
    pub recipient: String,
    /// The server's record of **which `key_id` the sealer bound into the AAD**:
    /// `true` = this row's real id, `false` = `0` (see the module docs and
    /// `SPEC-cube-xpub-envelope-v1` §4a).
    ///
    /// A **hint, not proof** — it is what the writer claimed, and the server
    /// holds no key to verify it. Readers use it to try the likelier binding
    /// first and must still handle both. `#[serde(default)]` (⇒ `false`) keeps
    /// a server that predates the column parsing; a wrong value costs one extra
    /// AES-GCM open, never a wrong answer.
    #[serde(default)]
    pub aad_key_id_bound: bool,
    /// Compressed (33-byte) ephemeral public key, hex.
    pub ephemeral_pubkey: String,
    /// 12-byte GCM nonce, hex.
    pub nonce: String,
    /// `ciphertext || GCM tag`, hex.
    pub ciphertext: String,
}

impl std::fmt::Debug for XpubEnvelope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("XpubEnvelope")
            .field("scheme", &self.scheme)
            .field("recipient", &self.recipient)
            .field("aad_key_id_bound", &self.aad_key_id_bound)
            .field("ephemeral_pubkey", &self.ephemeral_pubkey)
            .field("nonce_hex_len", &self.nonce.len())
            .field("ciphertext_hex_len", &self.ciphertext.len())
            .finish()
    }
}

/// **Keychain side, mirrored here for tests and fixture generation only.**
///
/// Production desktop never seals an xpub envelope — the Contact's phone does
/// (`keychain-app` PR K1), and `coincube-api`'s one-shot migration does for
/// legacy rows. This exists so the round-trip and known-answer tests below can
/// exercise the full path without a phone, exactly as the inheritance codec's
/// `keychain_shared_key` does.
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn seal_to_cube_pubkey(
    recipient_pub: &PublicKey,
    eph_sk: &SecretKey,
    nonce: &[u8; NONCE_LEN],
    cube_id: u64,
    key_id: u64,
    plaintext: &[u8],
) -> Result<XpubEnvelope, EciesError> {
    let secp = Secp256k1::new();
    let eph_pk = PublicKey::from_secret_key(&secp, eph_sk);
    let ikm = ecdh_ikm(recipient_pub, eph_sk);
    let key = hkdf_key(
        ECIES_LABEL,
        ikm.as_ref(),
        &eph_pk.serialize(),
        &recipient_pub.serialize(),
    );
    let cipher = Aes256Gcm::new_from_slice(key.as_ref()).map_err(EciesError::Cipher)?;
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: plaintext,
                aad: &aad_bytes(ArtifactKind::Xpub, cube_id, key_id),
            },
        )
        .map_err(|_| EciesError::Seal)?;
    Ok(XpubEnvelope {
        scheme: SCHEME.to_string(),
        recipient: RECIPIENT_CUBE_OWNER.to_string(),
        aad_key_id_bound: key_id != 0,
        ephemeral_pubkey: hex::encode(eph_pk.serialize()),
        nonce: hex::encode(nonce),
        ciphertext: hex::encode(ciphertext),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use coincube_core::miniscript::bitcoin::bip32::Xpub;
    use std::str::FromStr;

    /// The throwaway BIP-39 vector mnemonic. Testnet, never real funds.
    const KAT_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    fn kat_signer() -> MasterSigner {
        MasterSigner::from_str(Network::Testnet, KAT_MNEMONIC).unwrap()
    }

    // ─── Known-answer vectors ────────────────────────────────────────────────
    //
    // Two layers, both cross-repo:
    //
    //  1. The SPEC-ecies-v1 §7.1 AAD, pinned byte-for-byte. `coincube-api`'s
    //     `TestECIESAADMatchesSpec` pins the same string against `ECIESAAD`, so
    //     these two assertions are the contract between the Go seal and this
    //     Rust open. (§7.1's seal itself is pinned in `inheritance::ecies`.)
    //  2. An xpub envelope (`artifact_kind 0x03`) sealed to the seed-derived
    //     Cube key, generated by the independent Python oracle in
    //     `plans/cube_xpub_kat_reference.py`.

    /// SPEC-ecies-v1 §7.1 `aad` — descriptor kind, cube 42, key 7. Identical to
    /// the fixture `coincube-api/crypto/ecies_test.go` pins.
    const KAT_SPEC_71_AAD: &str = "636f696e637562652d696e6865726974616e63652d65636965732d76310001000000000000002a0000000000000007";
    /// The same ids under the xpub kind — one byte different, which is the
    /// whole of the domain separation.
    const KAT_XPUB_AAD: &str = "636f696e637562652d696e6865726974616e63652d65636965732d76310003000000000000002a0000000000000007";

    const KAT_ENC_PATH: &str = "m/7000'/1'/0'";
    const KAT_D: &str = "6ca01bcb3df94648b0b3884863ee8748706d54a637f25dcbe2b8ff980da170e4";
    const KAT_P: &str = "0217af5fa4d4b084db9cde6a882ebc99a8e18532371fa1cf267ab66878d103d33b";
    const KAT_E: &str = "032c0b7cf95324a07d05398b240174dc0c2be444d96b159aa6c7f7b1e668680991";
    const KAT_NONCE: &str = "0000000000000000cafebabe";
    const KAT_AES_KEY: &str = "005e62e78a579622db37467321cfbff3774fbacb812bdd5c3f351ed931f0b4e5";
    const KAT_CUBE_ID: u64 = 42;
    const KAT_KEY_ID: u64 = 7;
    /// The sealed plaintext: a real BIP-48 testnet account xpub
    /// (`m/48'/1'/0'/2'` of the same throwaway mnemonic) — exactly the shape a
    /// Keychain seals at enrolment.
    const KAT_XPUB: &str = "tpubDFH9dgzveyD8zTbPUFuLrGmCydNvxehyNdUXKJAQN8x4aZ4j6UZqGfnqFrD4NqyaTVGKbvEW54tsvPTK2UoSbCC1PJY8iCNiwTL3RWZEheQ";
    const KAT_CT: &str = "fc13b1b9639e00e163b3664b62f516ad49d7f19c5383a758706ca813fa8e236cf14a4189aa61ee94801d31cb26a14a999eb5ea2c90a53bc704c5b262ff2b4cf984e97d7c92d13069b829b972c501190db9eaba00b8df84a25c78125e602cff3b037c7db65974b063084596a64667d5f92d647067c3c5453237d7e9e3573a57";

    fn kat_envelope() -> XpubEnvelope {
        XpubEnvelope {
            scheme: SCHEME.to_string(),
            recipient: RECIPIENT_CUBE_OWNER.to_string(),
            aad_key_id_bound: true,
            ephemeral_pubkey: KAT_E.to_string(),
            nonce: KAT_NONCE.to_string(),
            ciphertext: KAT_CT.to_string(),
        }
    }

    #[test]
    fn encryption_path_is_hardened_purpose_7000_per_network() {
        assert_eq!(
            cube_encryption_path(Network::Bitcoin).to_string(),
            "7000'/0'/0'"
        );
        // Testnet, signet and regtest all share BIP-44 coin type 1'.
        for net in [Network::Testnet, Network::Signet, Network::Regtest] {
            assert_eq!(cube_encryption_path(net).to_string(), "7000'/1'/0'");
        }
        // Every step is hardened — an xpub can never walk into this subtree,
        // so it cannot be reached from any exported descriptor key (I3).
        assert!(cube_encryption_path(Network::Bitcoin)
            .into_iter()
            .all(|c| c.is_hardened()));
    }

    #[test]
    fn kat_derivation_matches_vector() {
        let key = CubeEncryptionKey::derive(&kat_signer(), Network::Testnet);
        assert_eq!(key.public_key_hex(), KAT_P);
        assert_eq!(hex::encode(*key.secret), KAT_D);
        assert_eq!(
            format!("m/{}", cube_encryption_path(Network::Testnet)),
            KAT_ENC_PATH
        );
    }

    /// The cross-repo AAD pin. If this drifts, `cmd/blind_xpubs` writes
    /// envelopes this client can never open — and nothing else would catch it
    /// until a user tried to build a Vault.
    #[test]
    fn kat_aad_layout_matches_the_api_contract() {
        // §7.1 (descriptor kind): identical to coincube-api's ECIESAAD fixture.
        assert_eq!(
            hex::encode(aad_bytes(ArtifactKind::Descriptor, 42, 7)),
            KAT_SPEC_71_AAD
        );
        // The xpub extension differs in exactly one byte, at the kind position.
        assert_eq!(
            hex::encode(aad_bytes(ArtifactKind::Xpub, KAT_CUBE_ID, KAT_KEY_ID)),
            KAT_XPUB_AAD
        );
        let spec = hex::decode(KAT_SPEC_71_AAD).unwrap();
        let xpub = hex::decode(KAT_XPUB_AAD).unwrap();
        assert_eq!(spec.len(), xpub.len());
        assert_eq!(
            spec.iter().zip(&xpub).filter(|(a, b)| a != b).count(),
            1,
            "descriptor and xpub AADs must differ only in the artifact_kind byte"
        );
    }

    #[test]
    fn kat_seal_matches_vector() {
        // The sealer's half (Keychain, or the API's A5 one-shot), with the
        // vector's fixed ephemeral scalar and nonce.
        let p = PublicKey::from_slice(&hex::decode(KAT_P).unwrap()).unwrap();
        let eph_sk = SecretKey::from_slice(&[0x44u8; 32]).unwrap();
        let mut nonce = [0u8; NONCE_LEN];
        nonce.copy_from_slice(&hex::decode(KAT_NONCE).unwrap());

        let env = seal_to_cube_pubkey(
            &p,
            &eph_sk,
            &nonce,
            KAT_CUBE_ID,
            KAT_KEY_ID,
            KAT_XPUB.as_bytes(),
        )
        .unwrap();
        assert_eq!(env.ephemeral_pubkey, KAT_E);
        assert_eq!(env.ciphertext, KAT_CT);
        assert_eq!(env.scheme, SCHEME);
        // The same scheme id the heir-escrow envelopes use — one construction.
        assert_eq!(env.scheme, crate::services::inheritance::SCHEME);
    }

    #[test]
    fn kat_open_matches_vector() {
        // Desktop half: the seed-derived key opens the vector ciphertext.
        let key = CubeEncryptionKey::derive(&kat_signer(), Network::Testnet);
        let pt = key.open(&kat_envelope(), KAT_CUBE_ID, KAT_KEY_ID).unwrap();
        assert_eq!(std::str::from_utf8(&pt).unwrap(), KAT_XPUB);
        // And the plaintext really is a parseable xpub.
        assert!(Xpub::from_str(std::str::from_utf8(&pt).unwrap()).is_ok());
    }

    #[test]
    fn kat_derived_key_matches_vector() {
        // The HKDF output itself — pinned so a label or info-ordering change is
        // caught here rather than as an opaque tag failure.
        let d = SecretKey::from_slice(&hex::decode(KAT_D).unwrap()).unwrap();
        let e = PublicKey::from_slice(&hex::decode(KAT_E).unwrap()).unwrap();
        let ikm = ecdh_ikm(&e, &d);
        let k = hkdf_key(
            ECIES_LABEL,
            ikm.as_ref(),
            &hex::decode(KAT_E).unwrap(),
            &hex::decode(KAT_P).unwrap(),
        );
        assert_eq!(hex::encode(*k), KAT_AES_KEY);
    }

    #[test]
    fn roundtrips_with_a_random_ephemeral() {
        let key = CubeEncryptionKey::derive(&kat_signer(), Network::Testnet);
        let p = PublicKey::from_slice(&key.public_key()).unwrap();
        let eph = crate::services::inheritance::ecies::random_secret_key();
        let env =
            seal_to_cube_pubkey(&p, &eph, &[0x07; NONCE_LEN], 9, 3, KAT_XPUB.as_bytes()).unwrap();
        let pt = key.open(&env, 9, 3).unwrap();
        assert_eq!(std::str::from_utf8(&pt).unwrap(), KAT_XPUB);
    }

    #[test]
    fn a_different_seed_cannot_open() {
        let mine = CubeEncryptionKey::derive(&kat_signer(), Network::Testnet);
        let theirs = CubeEncryptionKey::derive(
            &MasterSigner::generate(Network::Testnet).unwrap(),
            Network::Testnet,
        );
        let p = PublicKey::from_slice(&mine.public_key()).unwrap();
        let eph = crate::services::inheritance::ecies::random_secret_key();
        let env = seal_to_cube_pubkey(&p, &eph, &[0x07; NONCE_LEN], 9, 3, b"secret xpub").unwrap();

        assert!(mine.open(&env, 9, 3).is_ok());
        assert!(matches!(
            theirs.open(&env, 9, 3).unwrap_err(),
            EciesError::BadKeyOrCorrupt
        ));
    }

    #[test]
    fn wrong_network_derives_a_different_key() {
        // A regtest Cube and a mainnet Cube on the same seed must not share the
        // encryption key — otherwise one network's envelopes open on the other.
        let signer = kat_signer();
        let testnet = CubeEncryptionKey::derive(&signer, Network::Testnet);
        let mainnet = CubeEncryptionKey::derive(&signer, Network::Bitcoin);
        assert_ne!(testnet.public_key(), mainnet.public_key());
    }

    #[test]
    fn retargeting_the_cube_id_breaks_the_tag() {
        // The breach scenario the AAD binding exists for: a server moving a
        // stored envelope onto another Cube's key listing.
        let key = CubeEncryptionKey::derive(&kat_signer(), Network::Testnet);
        assert!(matches!(
            key.open(&kat_envelope(), KAT_CUBE_ID + 1, KAT_KEY_ID)
                .unwrap_err(),
            EciesError::BadKeyOrCorrupt
        ));
    }

    #[test]
    fn retargeting_the_key_id_breaks_the_tag() {
        // The sharper case the API contract added over the plan draft: moving
        // an envelope onto a *different key row of the same Cube*. Both rows
        // are readable by this owner, so only the key_id binding catches it —
        // without it the Vault would be built pairing one key's xpub with
        // another key's origin metadata.
        let key = CubeEncryptionKey::derive(&kat_signer(), Network::Testnet);
        assert!(matches!(
            key.open(&kat_envelope(), KAT_CUBE_ID, KAT_KEY_ID + 1)
                .unwrap_err(),
            EciesError::BadKeyOrCorrupt
        ));
    }

    #[test]
    fn tampered_ciphertext_fails_closed() {
        let key = CubeEncryptionKey::derive(&kat_signer(), Network::Testnet);
        let mut ct = hex::decode(KAT_CT).unwrap();
        ct[0] ^= 0x01;
        let mut env = kat_envelope();
        env.ciphertext = hex::encode(ct);
        assert!(matches!(
            key.open(&env, KAT_CUBE_ID, KAT_KEY_ID).unwrap_err(),
            EciesError::BadKeyOrCorrupt
        ));
    }

    #[test]
    fn an_xpub_envelope_cannot_be_opened_as_a_descriptor_or_seed() {
        // Domain separation is by artifact_kind now, not by label: opening the
        // §8 xpub ciphertext under a descriptor/seed AAD must fail. This is the
        // mirror of coincube-api's TestECIESAADDomainSeparatesXPub.
        let key = CubeEncryptionKey::derive(&kat_signer(), Network::Testnet);
        let mut aes_key = [0u8; 32];
        aes_key.copy_from_slice(&hex::decode(KAT_AES_KEY).unwrap());

        for kind in [ArtifactKind::Descriptor, ArtifactKind::Seed] {
            let env = crate::services::inheritance::Envelope {
                artifact_kind: kind,
                scheme: SCHEME.to_string(),
                ephemeral_pubkey: hex::decode(KAT_E).unwrap(),
                ciphertext: hex::decode(KAT_CT).unwrap(),
                nonce: hex::decode(KAT_NONCE).unwrap(),
                derivation: String::new(),
            };
            assert!(
                matches!(
                    crate::services::inheritance::open_with_shared_key(
                        &aes_key,
                        &env,
                        KAT_CUBE_ID,
                        KAT_KEY_ID
                    ),
                    Err(EciesError::BadKeyOrCorrupt)
                ),
                "{:?} must not open an xpub envelope",
                kind
            );
        }
        // …while the xpub path, with the same key material, does open it.
        assert!(key.open(&kat_envelope(), KAT_CUBE_ID, KAT_KEY_ID).is_ok());
    }

    #[test]
    fn unsupported_scheme_is_rejected_before_any_crypto() {
        let key = CubeEncryptionKey::derive(&kat_signer(), Network::Testnet);
        let mut env = kat_envelope();
        env.scheme = "ecies-v2-some-future-thing".to_string();
        assert!(matches!(
            key.open(&env, KAT_CUBE_ID, KAT_KEY_ID).unwrap_err(),
            EciesError::UnsupportedScheme(_)
        ));
    }

    #[test]
    fn malformed_fields_are_rejected_by_name() {
        let key = CubeEncryptionKey::derive(&kat_signer(), Network::Testnet);

        let mut bad_hex = kat_envelope();
        bad_hex.ciphertext = "not hex zz".to_string();
        assert!(matches!(
            key.open(&bad_hex, KAT_CUBE_ID, KAT_KEY_ID).unwrap_err(),
            EciesError::MalformedEnvelope("ciphertext")
        ));

        let mut short_nonce = kat_envelope();
        short_nonce.nonce = "00112233".to_string();
        assert!(matches!(
            key.open(&short_nonce, KAT_CUBE_ID, KAT_KEY_ID).unwrap_err(),
            EciesError::MalformedEnvelope("nonce length")
        ));

        let mut short_eph = kat_envelope();
        short_eph.ephemeral_pubkey = "0217af5f".to_string();
        assert!(matches!(
            key.open(&short_eph, KAT_CUBE_ID, KAT_KEY_ID).unwrap_err(),
            EciesError::MalformedEnvelope("ephemeral pubkey length")
        ));

        let mut stub_ct = kat_envelope();
        stub_ct.ciphertext = "00112233".to_string();
        assert!(matches!(
            key.open(&stub_ct, KAT_CUBE_ID, KAT_KEY_ID).unwrap_err(),
            EciesError::MalformedEnvelope("ciphertext shorter than tag")
        ));
    }

    #[test]
    fn wire_shape_matches_the_api_dto() {
        // The JSON `coincube-api`'s xpubenvelope.Wire serves must deserialise
        // as-is — camelCase keys, hex values, plus the response-only
        // `recipient` field.
        let json = format!(
            r#"{{"scheme":"{}","recipient":"cube-owner","ephemeralPubkey":"{}","nonce":"{}","ciphertext":"{}"}}"#,
            SCHEME, KAT_E, KAT_NONCE, KAT_CT
        );
        let env: XpubEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(env.recipient, RECIPIENT_CUBE_OWNER);
        let key = CubeEncryptionKey::derive(&kat_signer(), Network::Testnet);
        assert_eq!(
            std::str::from_utf8(&key.open(&env, KAT_CUBE_ID, KAT_KEY_ID).unwrap()).unwrap(),
            KAT_XPUB
        );
        // …and re-serialises to the same keys.
        assert!(serde_json::to_string(&env)
            .unwrap()
            .contains("ephemeralPubkey"));
    }

    #[test]
    fn a_wire_payload_without_recipient_still_parses() {
        // `recipient` is response-only on the API side, so anything shaped like
        // the request body (or an older server) must not fail to deserialise.
        let json = format!(
            r#"{{"scheme":"{}","ephemeralPubkey":"{}","nonce":"{}","ciphertext":"{}"}}"#,
            SCHEME, KAT_E, KAT_NONCE, KAT_CT
        );
        let env: XpubEnvelope = serde_json::from_str(&json).unwrap();
        assert!(env.recipient.is_empty());
        let key = CubeEncryptionKey::derive(&kat_signer(), Network::Testnet);
        assert!(key.open(&env, KAT_CUBE_ID, KAT_KEY_ID).is_ok());
    }

    #[test]
    fn debug_never_prints_the_secret() {
        let key = CubeEncryptionKey::derive(&kat_signer(), Network::Testnet);
        let rendered = format!("{:?}", key);
        assert!(rendered.contains("<redacted>"));
        assert!(!rendered.contains(KAT_D));
    }
}
