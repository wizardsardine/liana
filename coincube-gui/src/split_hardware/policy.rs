//! The device policy for a Split source (#568 Track B, slice B4b-2; owner
//! decision D7).
//!
//! A [`SplitSource`] holds the foreign wallet's receive descriptor, whose
//! keys end in `/0/*`, and optionally its change descriptor, whose keys end
//! in `/1/*`. A device registers and signs one policy for both branches, in
//! BIP389 form: every key origin-tagged and ending in `/<0;1>/*`, which is
//! what [`DevicePolicy::new`] validates. [`split_descriptor`] merges the two
//! branches into that form and [`split_policy`] turns it into the policy.
//! Without a change descriptor the change branch is each receive key's `/1`
//! step: the policy names the wallet, and the device signs only the inputs
//! the construction derived from the source, which the verified import
//! checks again.
//!
//! This module refuses, with its own reason, a key with no origin (no device
//! can be matched to it), a bare public key (the wallet is not ranged; owner
//! decision P7) and a branch step other than `/0` on the receive side or
//! `/1` on the change side. Everything else is refused by `DevicePolicy::new`
//! ([`PolicyError::Policy`]).
//!
//! Nothing here is reachable from the GUI (D1).

use std::fmt;

use coincube_core::{
    miniscript::{
        bitcoin::bip32::{ChildNumber, DerivationPath, Xpub},
        descriptor::{DerivPaths, DescriptorMultiXKey, DescriptorXKey, Wildcard},
        translate_hash_clone, Descriptor, DescriptorPublicKey, ForEachKey, TranslateErr,
        TranslatePk, Translator,
    },
    public_wallet_source::SplitSource,
    wallet_descriptor::ImportedWalletDescriptor,
};

use super::sign::{DevicePolicy, SignError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyError {
    /// A key carries no origin (master fingerprint and derivation path), so
    /// no device can be matched to it.
    MissingOrigin,
    /// A key is a bare public key: the wallet is not ranged.
    NotRanged,
    /// A receive key does not end in `/0/*`, or a change key in `/1/*`.
    Branch,
    /// The merged descriptor is not a Split device policy.
    Policy(SignError),
}

impl fmt::Display for PolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingOrigin => f.write_str("A wallet key has no origin (master fingerprint and derivation path), so no hardware wallet can be matched to it. Export the descriptor with key origins."),
            Self::NotRanged => f.write_str("This wallet uses a fixed public key, not a ranged account key. A hardware wallet signs ranged wallets only."),
            Self::Branch => f.write_str("The receive descriptor's keys must end in /0/* and the change descriptor's keys in /1/*."),
            Self::Policy(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for PolicyError {}

const RECEIVE: u32 = 0;
const CHANGE: u32 = 1;

fn branch_step(index: u32) -> DerivationPath {
    DerivationPath::from(vec![ChildNumber::Normal { index }])
}

/// The extended key behind one branch's key, checked for an origin, a ranged
/// (unhardened wildcard) form and exactly the `branch` step.
fn branch_key(
    key: &DescriptorPublicKey,
    branch: u32,
) -> Result<&DescriptorXKey<Xpub>, PolicyError> {
    let xkey = match key {
        DescriptorPublicKey::XPub(xkey) => xkey,
        DescriptorPublicKey::Single(_) => return Err(PolicyError::NotRanged),
        // A source holds single-branch descriptors; a multipath key has no
        // one branch to check.
        DescriptorPublicKey::MultiXPub(_) => return Err(PolicyError::Branch),
    };
    if xkey.origin.is_none() {
        return Err(PolicyError::MissingOrigin);
    }
    if xkey.wildcard != Wildcard::Unhardened || xkey.derivation_path != branch_step(branch) {
        return Err(PolicyError::Branch);
    }
    Ok(xkey)
}

fn check_branch(
    descriptor: &Descriptor<DescriptorPublicKey>,
    branch: u32,
) -> Result<(), PolicyError> {
    let mut result = Ok(());
    descriptor.for_each_key(|key| match branch_key(key, branch) {
        Ok(_) => true,
        Err(error) => {
            result = Err(error);
            false
        }
    });
    result
}

/// Rewrites each receive key into its two-branch form.
struct Merge;

impl Translator<DescriptorPublicKey, DescriptorPublicKey, PolicyError> for Merge {
    fn pk(&mut self, pk: &DescriptorPublicKey) -> Result<DescriptorPublicKey, PolicyError> {
        let xkey = branch_key(pk, RECEIVE)?;
        let derivation_paths = DerivPaths::new(vec![branch_step(RECEIVE), branch_step(CHANGE)])
            .ok_or(PolicyError::Branch)?;
        Ok(DescriptorPublicKey::MultiXPub(DescriptorMultiXKey {
            origin: xkey.origin.clone(),
            xkey: xkey.xkey,
            derivation_paths,
            wildcard: Wildcard::Unhardened,
        }))
    }

    translate_hash_clone!(DescriptorPublicKey, DescriptorPublicKey, PolicyError);
}

/// The source's receive and change branches as one origin-tagged `<0;1>`
/// descriptor: the form a device registers and signs.
fn public_descriptor(
    external: &Descriptor<DescriptorPublicKey>,
    internal: Option<&Descriptor<DescriptorPublicKey>>,
) -> Result<Descriptor<DescriptorPublicKey>, PolicyError> {
    check_branch(external, RECEIVE)?;
    if let Some(internal) = internal {
        check_branch(internal, CHANGE)?;
    }
    external
        .translate_pk(&mut Merge)
        .map_err(|error| match error {
            TranslateErr::TranslatorErr(error) => error,
            TranslateErr::OuterError(_) => PolicyError::Policy(SignError::UnsupportedShape),
        })
}

pub fn split_descriptor(
    source: &SplitSource,
) -> Result<Descriptor<DescriptorPublicKey>, PolicyError> {
    public_descriptor(source.external(), source.internal())
}

/// The imported wallet's complete pair in the two-branch form expected by
/// hardware registration. The public account was already validated by core.
pub fn imported_descriptor(
    wallet: &ImportedWalletDescriptor,
) -> Result<Descriptor<DescriptorPublicKey>, PolicyError> {
    public_descriptor(
        wallet.receive_descriptor(),
        Some(wallet.change_descriptor()),
    )
}

/// The device policy for `source`: [`split_descriptor`] validated as one of
/// the five Split shapes by [`DevicePolicy::new`].
pub fn split_policy(source: &SplitSource) -> Result<DevicePolicy, PolicyError> {
    DevicePolicy::new(&split_descriptor(source)?).map_err(PolicyError::Policy)
}

pub fn imported_policy(wallet: &ImportedWalletDescriptor) -> Result<DevicePolicy, PolicyError> {
    DevicePolicy::new_imported(&imported_descriptor(wallet)?, wallet).map_err(PolicyError::Policy)
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use coincube_core::miniscript::bitcoin::{secp256k1::Secp256k1, PublicKey};

    use super::*;
    use crate::{
        services::{
            split_source::split_source,
            split_test_wallets::{self as fixture, Shape, SHAPES},
        },
        split_hardware::sign::{PolicyShape, SinglesigScript},
    };

    fn desc(text: &str) -> Descriptor<DescriptorPublicKey> {
        Descriptor::from_str(text).unwrap()
    }

    /// An account key of `fixture::master(seed)`: its origin text and xpub.
    fn account(seed: u8, path: &str) -> (String, Xpub) {
        let secp = Secp256k1::new();
        let master = fixture::master(seed);
        let path = DerivationPath::from_str(path).unwrap();
        let xpub = Xpub::from_priv(&secp, &master.derive_priv(&secp, &path).unwrap());
        let origin = format!(
            "[{}/{}]",
            master.fingerprint(&secp),
            path.to_string().trim_start_matches("m/")
        );
        (origin, xpub)
    }

    fn source(external: &str, internal: Option<&str>) -> SplitSource {
        SplitSource::new(desc(external), internal.map(desc)).unwrap()
    }

    fn expected_split_policy(shape: Shape) -> (PolicyShape, usize) {
        match shape {
            Shape::Wpkh => (PolicyShape::Singlesig(SinglesigScript::Wpkh), 1),
            Shape::ShWpkh => (PolicyShape::Singlesig(SinglesigScript::ShWpkh), 1),
            Shape::Pkh => (PolicyShape::Singlesig(SinglesigScript::Pkh), 1),
            Shape::WshSortedMulti => (
                PolicyShape::Multisig {
                    k: 2,
                    n: 3,
                    sorted: true,
                },
                3,
            ),
            Shape::WshMulti => (
                PolicyShape::Multisig {
                    k: 2,
                    n: 3,
                    sorted: false,
                },
                3,
            ),
        }
    }

    #[test]
    fn split_policy_merges_branches_and_refuses_missing_origins() {
        for shape in SHAPES {
            let wallet = fixture::wallet(shape);
            let paired = split_source(&wallet.external, Some(&wallet.internal)).unwrap();
            let receive_only = split_source(&wallet.external, None).unwrap();
            let (expected, keys) = expected_split_policy(shape);
            for src in [&paired, &receive_only] {
                let merged = split_descriptor(src).unwrap();
                // Every key is origin-tagged and carries exactly the two
                // branches, ranged.
                let mut counted = 0;
                merged.for_each_key(|key| {
                    counted += 1;
                    let DescriptorPublicKey::MultiXPub(key) = key else {
                        panic!("{:?}: {}", shape, key);
                    };
                    assert!(key.origin.is_some());
                    assert_eq!(
                        key.derivation_paths.paths().as_slice(),
                        [branch_step(0), branch_step(1)]
                    );
                    assert_eq!(key.wildcard, Wildcard::Unhardened);
                    true
                });
                assert_eq!(counted, keys, "{shape:?}");
                assert_eq!(merged.to_string().matches("/<0;1>/*").count(), keys);
                // Split back into branches, it is the source's receive branch
                // and the change branch the source gave or implied.
                let branches = merged.clone().into_single_descriptors().unwrap();
                assert_eq!(branches.len(), 2);
                assert_eq!(branches[0], *src.external(), "{shape:?}");
                let implied_change = desc(
                    &src.external()
                        .to_string()
                        .split('#')
                        .next()
                        .unwrap()
                        .replace("/0/*", "/1/*"),
                );
                assert_eq!(branches[1], implied_change, "{shape:?}");
                if let Some(internal) = src.internal() {
                    assert_eq!(branches[1], *internal, "{shape:?}");
                }
                let policy = split_policy(src).unwrap();
                assert_eq!(policy.shape(), expected, "{shape:?}");
                if expected.is_multisig() {
                    let stable_name = match (shape, src.internal().is_some()) {
                        (Shape::WshSortedMulti, true) => "Splitb969ea7b",
                        (Shape::WshSortedMulti, false) => "Splitcb7ead5a",
                        (Shape::WshMulti, _) => "Splite53d7bc7",
                        _ => unreachable!(),
                    };
                    assert_eq!(policy.name(), stable_name);
                } else {
                    assert_eq!(policy.name(), "");
                }
            }
        }

        let (origin, xpub) = account(1, "m/84'/0'/0'");
        let refuse = |external: &str, internal: Option<&str>| {
            let src = source(external, internal);
            let error = split_descriptor(&src).unwrap_err();
            assert_eq!(split_policy(&src).unwrap_err(), error);
            error
        };
        // No origin: nothing a device's master fingerprint can be matched to.
        assert_eq!(
            refuse(&format!("wpkh({xpub}/0/*)"), None),
            PolicyError::MissingOrigin
        );
        assert_eq!(
            refuse(
                &format!("wpkh({xpub}/0/*)"),
                Some(&format!("wpkh({xpub}/1/*)"))
            ),
            PolicyError::MissingOrigin
        );
        // One multisig key without its origin.
        let (o1, x1) = account(1, "m/48'/0'/0'/2'");
        let (o2, x2) = account(2, "m/48'/0'/0'/2'");
        let (o3, x3) = account(3, "m/48'/0'/0'/2'");
        assert_eq!(
            refuse(
                &format!("wsh(sortedmulti(2,{o1}{x1}/0/*,{x2}/0/*,{o3}{x3}/0/*))"),
                None
            ),
            PolicyError::MissingOrigin
        );
        // A bare key is not ranged (P7).
        let single = PublicKey::new(xpub.public_key);
        assert_eq!(
            refuse(&format!("wpkh({single})"), None),
            PolicyError::NotRanged
        );
        // Branch steps: the receive branch at /1, an extra step, and a change
        // descriptor on the receive branch.
        assert_eq!(
            refuse(&format!("wpkh({origin}{xpub}/1/*)"), None),
            PolicyError::Branch
        );
        assert_eq!(
            refuse(&format!("wpkh({origin}{xpub}/0/0/*)"), None),
            PolicyError::Branch
        );
        // A fixed child key (no wildcard) is one address, not the ranged
        // branch a device policy would describe (P7; review F1).
        assert_eq!(
            refuse(&format!("wpkh({origin}{xpub}/0)"), None),
            PolicyError::Branch
        );
        assert_eq!(
            refuse(
                &format!("wsh(sortedmulti(2,{o1}{x1}/0/*,{o2}{x2}/0,{o3}{x3}/0/*))"),
                None
            ),
            PolicyError::Branch
        );
        assert_eq!(
            refuse(
                &format!("wpkh({origin}{xpub}/0/*)"),
                Some(&format!("wpkh({origin}{xpub}/0/*)"))
            ),
            PolicyError::Branch
        );
        // The merged form still has to be a Split device policy: a singlesig
        // key off its script's standard account path merges, and is then
        // B4a's refusal.
        let (o44, x44) = account(1, "m/44'/0'/0'");
        let off_path = source(&format!("wpkh({o44}{x44}/0/*)"), None);
        assert_eq!(
            split_descriptor(&off_path)
                .unwrap()
                .to_string()
                .matches("/<0;1>/*")
                .count(),
            1
        );
        assert_eq!(
            split_policy(&off_path).unwrap_err(),
            PolicyError::Policy(SignError::NonStandardSinglesigPath)
        );
    }
}
