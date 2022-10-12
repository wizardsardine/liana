use miniscript::{
    bitcoin::{self, hashes::hash160, hashes::Hash, secp256k1, util::bip32},
    descriptor::{self, DescriptorTrait},
    miniscript::{
        decode::Terminal,
        iter::PkPkh,
        limits::{SEQUENCE_LOCKTIME_DISABLE_FLAG, SEQUENCE_LOCKTIME_TYPE_FLAG},
        Miniscript,
    },
    policy::{Liftable, Semantic as SemanticPolicy},
    MiniscriptKey, ScriptContext, ToPublicKey, TranslatePk2,
};

use std::{collections::BTreeMap, error, fmt, io::Write, str, sync};

use serde::{Deserialize, Serialize};

// Flag applied to the nSequence and CSV value before comparing them.
//
// <https://github.com/bitcoin/bitcoin/blob/4a540683ec40393d6369da1a9e02e45614db936d/src/primitives/transaction.h#L87-L89>
pub const SEQUENCE_LOCKTIME_MASK: u32 = 0x00_00_ff_ff;

#[derive(Debug)]
pub enum DescCreationError {
    InsaneTimelock(u32),
    InvalidKey(descriptor::DescriptorPublicKey),
    Miniscript(miniscript::Error),
    IncompatibleDesc,
    DerivedKeyParsing,
}

impl std::fmt::Display for DescCreationError {
    fn fmt(&self, f: &mut fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::InsaneTimelock(tl) => write!(f, "Timelock value '{}' isn't safe to use", tl),
            Self::InvalidKey(key) => {
                write!(f, "Invalid key '{}'. Need a wildcard ('ranged') xpub", key)
            }
            Self::Miniscript(e) => write!(f, "Miniscript error: '{}'.", e),
            Self::IncompatibleDesc => write!(f, "Descriptor is not compatible."),
            Self::DerivedKeyParsing => write!(f, "Parsing derived key,"),
        }
    }
}

impl error::Error for DescCreationError {}

/// A public key used in derived descriptors
#[derive(Debug, Eq, PartialEq, Clone, Ord, PartialOrd, Hash)]
pub struct DerivedPublicKey {
    /// Fingerprint of the master xpub and the derivation index used. We don't use a path
    /// since we never derive at more than one depth.
    pub origin: (bip32::Fingerprint, bip32::ChildNumber),
    /// The actual key
    pub key: bitcoin::PublicKey,
}

impl fmt::Display for DerivedPublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (fingerprint, deriv_index) = &self.origin;

        write!(f, "[")?;
        for byte in fingerprint.as_bytes().iter() {
            write!(f, "{:02x}", byte)?;
        }
        write!(f, "/{}", deriv_index)?;
        write!(f, "]{}", self.key)
    }
}

impl str::FromStr for DerivedPublicKey {
    type Err = DescCreationError;

    fn from_str(s: &str) -> Result<DerivedPublicKey, Self::Err> {
        // The key is always of the form:
        // [ fingerprint / index ]<key>

        // 1 + 8 + 1 + 1 + 1 + 66 minimum
        if s.len() < 78 {
            return Err(DescCreationError::DerivedKeyParsing);
        }

        // Non-ASCII?
        for ch in s.as_bytes() {
            if *ch < 20 || *ch > 127 {
                return Err(DescCreationError::DerivedKeyParsing);
            }
        }

        if s.chars().next().expect("Size checked above") != '[' {
            return Err(DescCreationError::DerivedKeyParsing);
        }

        let mut parts = s[1..].split(']');
        let fg_deriv = parts.next().ok_or(DescCreationError::DerivedKeyParsing)?;
        let key_str = parts.next().ok_or(DescCreationError::DerivedKeyParsing)?;

        if fg_deriv.len() < 10 {
            return Err(DescCreationError::DerivedKeyParsing);
        }
        let fingerprint = bip32::Fingerprint::from_str(&fg_deriv[..8])
            .map_err(|_| DescCreationError::DerivedKeyParsing)?;
        let deriv_index = bip32::ChildNumber::from_str(&fg_deriv[9..])
            .map_err(|_| DescCreationError::DerivedKeyParsing)?;
        if deriv_index.is_hardened() {
            return Err(DescCreationError::DerivedKeyParsing);
        }

        let key = bitcoin::PublicKey::from_str(key_str)
            .map_err(|_| DescCreationError::DerivedKeyParsing)?;

        Ok(DerivedPublicKey {
            key,
            origin: (fingerprint, deriv_index),
        })
    }
}

impl MiniscriptKey for DerivedPublicKey {
    // This allows us to be able to derive keys and key source even for PkH s
    type Hash = Self;

    fn is_uncompressed(&self) -> bool {
        self.key.is_uncompressed()
    }

    fn to_pubkeyhash(&self) -> Self::Hash {
        self.clone()
    }
}

impl ToPublicKey for DerivedPublicKey {
    fn to_public_key(&self) -> bitcoin::PublicKey {
        self.key
    }

    fn hash_to_hash160(derived_key: &Self) -> hash160::Hash {
        let mut engine = hash160::Hash::engine();
        engine
            .write_all(&derived_key.key.key.serialize())
            .expect("engines don't error");
        hash160::Hash::from_engine(engine)
    }
}

// We require the locktime to:
//  - not be disabled
//  - be in number of blocks
//  - be 'clean' / minimal, ie all bits without consensus meaning should be 0
fn csv_check(csv: u32) -> Result<(), DescCreationError> {
    if (csv & SEQUENCE_LOCKTIME_DISABLE_FLAG) == 0
        && (csv & SEQUENCE_LOCKTIME_TYPE_FLAG) == 0
        && (csv & SEQUENCE_LOCKTIME_MASK) == csv
    {
        Ok(())
    } else {
        Err(DescCreationError::InsaneTimelock(csv))
    }
}

fn is_unhardened_deriv(key: &descriptor::DescriptorPublicKey) -> bool {
    match *key {
        descriptor::DescriptorPublicKey::SinglePub(..) => false,
        descriptor::DescriptorPublicKey::XPub(ref xpub) => {
            xpub.wildcard == descriptor::Wildcard::Unhardened
        }
    }
}

/// A Miniscript descriptor with a main, unencombered, branch (the main owner of the coins)
/// and a timelocked branch (the heir).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InheritanceDescriptor(descriptor::Descriptor<descriptor::DescriptorPublicKey>);

/// Derived (containing only raw Bitcoin public keys) version of the inheritance descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DerivedInheritanceDescriptor(descriptor::Descriptor<DerivedPublicKey>);

impl fmt::Display for InheritanceDescriptor {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl str::FromStr for InheritanceDescriptor {
    type Err = DescCreationError;

    fn from_str(s: &str) -> Result<InheritanceDescriptor, Self::Err> {
        let wsh_desc = descriptor::Wsh::<descriptor::DescriptorPublicKey>::from_str(s)
            .map_err(DescCreationError::Miniscript)?;
        let ms = match wsh_desc.as_inner() {
            descriptor::WshInner::Ms(ms) => ms,
            _ => return Err(DescCreationError::IncompatibleDesc),
        };
        let invalid_key = ms.iter_pk_pkh().find_map(|pk_pkh| {
            let pk = match pk_pkh {
                PkPkh::PlainPubkey(pk) => pk,
                PkPkh::HashedPubkey(pk) => pk,
            };
            if is_unhardened_deriv(&pk) {
                None
            } else {
                Some(pk)
            }
        });
        if let Some(key) = invalid_key {
            return Err(DescCreationError::InvalidKey(key));
        }

        // Semantic of the Miniscript must be either the owner now, or the heir after
        // a timelock.
        let policy = ms
            .lift()
            .expect("Lifting can't fail on a Miniscript")
            .normalized();
        let subs = match policy {
            SemanticPolicy::Threshold(1, subs) => Some(subs),
            _ => None,
        }
        .ok_or(DescCreationError::IncompatibleDesc)?;
        if subs.len() != 2 {
            return Err(DescCreationError::IncompatibleDesc);
        }

        // Owner branch
        subs.iter()
            .find(|s| matches!(s, SemanticPolicy::KeyHash(_)))
            .ok_or(DescCreationError::IncompatibleDesc)?;

        // Heir branch
        let heir_subs = subs
            .iter()
            .find_map(|s| match s {
                SemanticPolicy::Threshold(2, subs) => Some(subs),
                _ => None,
            })
            .ok_or(DescCreationError::IncompatibleDesc)?;
        if heir_subs.len() != 2 {
            return Err(DescCreationError::IncompatibleDesc);
        }
        // Must be timelocked
        let csv = heir_subs
            .iter()
            .find_map(|s| match s {
                SemanticPolicy::Older(csv) => Some(csv),
                _ => None,
            })
            .ok_or(DescCreationError::IncompatibleDesc)?;
        csv_check(*csv)?;
        // And key locked
        heir_subs
            .iter()
            .find(|s| matches!(s, SemanticPolicy::KeyHash(_)))
            .ok_or(DescCreationError::IncompatibleDesc)?;

        Ok(InheritanceDescriptor(descriptor::Descriptor::Wsh(wsh_desc)))
    }
}

impl InheritanceDescriptor {
    pub fn new(
        owner_key: descriptor::DescriptorPublicKey,
        heir_key: descriptor::DescriptorPublicKey,
        timelock: u32,
    ) -> Result<InheritanceDescriptor, DescCreationError> {
        csv_check(timelock)?;

        if let Some(key) = vec![&owner_key, &heir_key]
            .iter()
            .find(|k| !is_unhardened_deriv(k))
        {
            return Err(DescCreationError::InvalidKey((**key).clone()));
        }

        let owner_pk = Miniscript::from_ast(Terminal::Check(sync::Arc::from(
            Miniscript::from_ast(Terminal::PkK(owner_key)).expect("TODO"),
        )))
        .expect("Well typed");

        let heir_pkh = Miniscript::from_ast(Terminal::Check(sync::Arc::from(
            Miniscript::from_ast(Terminal::PkH(heir_key)).expect("TODO"),
        )))
        .expect("Well typed");

        let heir_timelock = Terminal::Older(timelock);
        let heir_branch = Miniscript::from_ast(Terminal::AndV(
            Miniscript::from_ast(Terminal::Verify(heir_pkh.into()))
                .expect("Well typed")
                .into(),
            Miniscript::from_ast(heir_timelock)
                .expect("Well typed")
                .into(),
        ))
        .expect("Well typed");

        let tl_miniscript =
            Miniscript::from_ast(Terminal::OrD(owner_pk.into(), heir_branch.into()))
                .expect("Well typed");
        miniscript::Segwitv0::check_local_validity(&tl_miniscript)
            .expect("Miniscript must be sane");

        Ok(InheritanceDescriptor(descriptor::Descriptor::Wsh(
            descriptor::Wsh::new(tl_miniscript).expect("Must pass sanity checks"),
        )))
    }

    pub fn as_inner(&self) -> &descriptor::Descriptor<descriptor::DescriptorPublicKey> {
        &self.0
    }

    /// Derive this descriptor at a given index.
    pub fn derive(
        &self,
        index: bip32::ChildNumber,
        secp: &secp256k1::Secp256k1<impl secp256k1::Verification>,
    ) -> DerivedInheritanceDescriptor {
        assert!(index.is_normal());
        let desc = self
            .0
            .derive(index.into())
            .translate_pk2(|xpk| {
                xpk.derive_public_key(secp).map(|key| {
                    // FIXME: rust-miniscript will panic if we call
                    // xpk.master_fingerprint() on a key without origin
                    let origin = match xpk {
                        descriptor::DescriptorPublicKey::XPub(..) => {
                            (xpk.master_fingerprint(), index)
                        }
                        _ => unreachable!("All keys are always xpubs"),
                    };

                    DerivedPublicKey { key, origin }
                })
            })
            .expect("All pubkeys are derived, no wildcard.");
        DerivedInheritanceDescriptor(desc)
    }

    /// Get the value (in blocks) of the relative timelock for the heir's spending path.
    pub fn timelock_value(&self) -> u32 {
        let wsh_desc = match &self.0 {
            descriptor::Descriptor::Wsh(desc) => desc,
            _ => unreachable!(),
        };
        let ms = match wsh_desc.as_inner() {
            descriptor::WshInner::Ms(ms) => ms,
            _ => unreachable!(),
        };

        let policy = ms
            .lift()
            .expect("Lifting can't fail on a Miniscript")
            .normalized();
        let subs = match policy {
            SemanticPolicy::Threshold(1, subs) => subs,
            _ => unreachable!(),
        };
        let heir_subs = subs
            .iter()
            .find_map(|s| match s {
                SemanticPolicy::Threshold(2, subs) => Some(subs),
                _ => None,
            })
            .expect("Always present");
        let csv = heir_subs
            .iter()
            .find_map(|s| match s {
                SemanticPolicy::Older(csv) => Some(csv),
                _ => None,
            })
            .expect("Always present");

        *csv
    }
}

/// Map of a raw public key to the xpub used to derive it and its derivation path
pub type Bip32Deriv = BTreeMap<bitcoin::PublicKey, (bip32::Fingerprint, bip32::DerivationPath)>;

impl DerivedInheritanceDescriptor {
    pub fn address(&self, network: bitcoin::Network) -> bitcoin::Address {
        self.0
            .address(network)
            .expect("A P2WSH always has an address")
    }

    pub fn script_pubkey(&self) -> bitcoin::Script {
        self.0.script_pubkey()
    }

    pub fn witness_script(&self) -> bitcoin::Script {
        self.0.explicit_script()
    }

    pub fn bip32_derivations(&self) -> Bip32Deriv {
        let ms = match self.0 {
            descriptor::Descriptor::Wsh(ref wsh) => match wsh.as_inner() {
                descriptor::WshInner::Ms(ms) => ms,
                descriptor::WshInner::SortedMulti(_) => {
                    unreachable!("None of our descriptors is a sorted multi")
                }
            },
            _ => unreachable!("All our descriptors are always P2WSH"),
        };

        // For DerivedPublicKey, Pk::Hash == Self.
        ms.iter_pk_pkh()
            .map(|pkpkh| match pkpkh {
                PkPkh::PlainPubkey(pk) => pk,
                PkPkh::HashedPubkey(pkh) => pkh,
            })
            .map(|k| {
                (
                    k.key,
                    (k.origin.0, bip32::DerivationPath::from(&[k.origin.1][..])),
                )
            })
            .collect()
    }

    /// Get the maximum size in WU of a satisfaction for this descriptor.
    pub fn max_sat_weight(&self) -> usize {
        self.0
            .max_satisfaction_weight()
            .expect("Cannot fail for P2WSH")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::str::FromStr;

    #[test]
    fn inheritance_descriptor_creation() {
        let owner_key = descriptor::DescriptorPublicKey::from_str("xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/*").unwrap();
        let heir_key = descriptor::DescriptorPublicKey::from_str("xpub688Hn4wScQAAiYJLPg9yH27hUpfZAUnmJejRQBCiwfP5PEDzjWMNW1wChcninxr5gyavFqbbDjdV1aK5USJz8NDVjUy7FRQaaqqXHh5SbXe/*").unwrap();
        let timelock = 52560;
        assert_eq!(InheritanceDescriptor::new(owner_key.clone(), heir_key.clone(), timelock).unwrap().to_string(), "wsh(or_d(pk(xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/*),and_v(v:pkh(xpub688Hn4wScQAAiYJLPg9yH27hUpfZAUnmJejRQBCiwfP5PEDzjWMNW1wChcninxr5gyavFqbbDjdV1aK5USJz8NDVjUy7FRQaaqqXHh5SbXe/*),older(52560))))#eeyujkt7");

        // We prevent footguns with timelocks
        InheritanceDescriptor::new(owner_key.clone(), heir_key.clone(), 0x00_01_0f_00).unwrap_err();
        InheritanceDescriptor::new(owner_key.clone(), heir_key.clone(), (1 << 31) + 1).unwrap_err();
        InheritanceDescriptor::new(owner_key, heir_key, (1 << 22) + 1).unwrap_err();

        let owner_key = descriptor::DescriptorPublicKey::from_str("[aabb0011/10/4893]xpub661MyMwAqRbcFG59fiikD8UV762quhruT8K8bdjqy6N2o3LG7yohoCdLg1m2HAY1W6rfBrtauHkBhbfA4AQ3iazaJj5wVPhwgaRCHBW2DBg/*").unwrap();
        let heir_key = descriptor::DescriptorPublicKey::from_str("xpub661MyMwAqRbcFfxf71L4Dx4w5TmyNXrBicTEAM7vLzumxangwATWWgdJPb6xH1JHcJH9S3jNZx3fCnkkB1WyqrqGgavj1rehHcbythmruvZ/24/32/*").unwrap();
        let timelock = 57600;
        assert_eq!(InheritanceDescriptor::new(owner_key.clone(), heir_key, timelock).unwrap().to_string(), "wsh(or_d(pk([aabb0011/10/4893]xpub661MyMwAqRbcFG59fiikD8UV762quhruT8K8bdjqy6N2o3LG7yohoCdLg1m2HAY1W6rfBrtauHkBhbfA4AQ3iazaJj5wVPhwgaRCHBW2DBg/*),and_v(v:pkh(xpub661MyMwAqRbcFfxf71L4Dx4w5TmyNXrBicTEAM7vLzumxangwATWWgdJPb6xH1JHcJH9S3jNZx3fCnkkB1WyqrqGgavj1rehHcbythmruvZ/24/32/*),older(57600))))#8kamh6y8");

        // We can't pass a raw key, an xpub that is not deriveable, or only hardened derivable
        let heir_key = descriptor::DescriptorPublicKey::from_str("xpub661MyMwAqRbcFfxf71L4Dx4w5TmyNXrBicTEAM7vLzumxangwATWWgdJPb6xH1JHcJH9S3jNZx3fCnkkB1WyqrqGgavj1rehHcbythmruvZ/0/354").unwrap();
        InheritanceDescriptor::new(owner_key.clone(), heir_key, timelock).unwrap_err();
        let heir_key = descriptor::DescriptorPublicKey::from_str("xpub661MyMwAqRbcFfxf71L4Dx4w5TmyNXrBicTEAM7vLzumxangwATWWgdJPb6xH1JHcJH9S3jNZx3fCnkkB1WyqrqGgavj1rehHcbythmruvZ/0/*'").unwrap();
        InheritanceDescriptor::new(owner_key.clone(), heir_key, timelock).unwrap_err();
        let heir_key = descriptor::DescriptorPublicKey::from_str(
            "02e24913be26dbcfdf8e8e94870b28725cdae09b448b6c127767bf0154e3a3c8e5",
        )
        .unwrap();
        InheritanceDescriptor::new(owner_key, heir_key, timelock).unwrap_err();
    }

    #[test]
    fn inheritance_descriptor_derivation() {
        let secp = secp256k1::Secp256k1::verification_only();
        let desc = InheritanceDescriptor::from_str("wsh(andor(pk(tpubDEN9WSToTyy9ZQfaYqSKfmVqmq1VVLNtYfj3Vkqh67et57eJ5sTKZQBkHqSwPUsoSskJeaYnPttHe2VrkCsKA27kUaN9SDc5zhqeLzKa1rr/*),older(10000),pk(tpubD8LYfn6njiA2inCoxwM7EuN3cuLVcaHAwLYeups13dpevd3nHLRdK9NdQksWXrhLQVxcUZRpnp5CkJ1FhE61WRAsHxDNAkvGkoQkAeWDYjV/*)))#y5wcna2d").unwrap();
        let der_desc = desc.derive(11.into(), &secp);
        assert_eq!(
            "bc1qvjzcg25nsxmfccct0txjvljxjwn68htkrw57jqmjhfzvhyd2z4msc74w65",
            der_desc.address(bitcoin::Network::Bitcoin).to_string()
        );

        // Sanity check we can call the methods on the derived desc
        der_desc.script_pubkey();
        der_desc.witness_script();
        assert!(!der_desc.bip32_derivations().is_empty());
        assert!(!der_desc.max_sat_weight() > 0);
    }

    #[test]
    fn inheritance_descriptor_tl_value() {
        let desc = InheritanceDescriptor::from_str("wsh(andor(pk(tpubDEN9WSToTyy9ZQfaYqSKfmVqmq1VVLNtYfj3Vkqh67et57eJ5sTKZQBkHqSwPUsoSskJeaYnPttHe2VrkCsKA27kUaN9SDc5zhqeLzKa1rr/*),older(1),pk(tpubD8LYfn6njiA2inCoxwM7EuN3cuLVcaHAwLYeups13dpevd3nHLRdK9NdQksWXrhLQVxcUZRpnp5CkJ1FhE61WRAsHxDNAkvGkoQkAeWDYjV/*)))").unwrap();
        assert_eq!(desc.timelock_value(), 1);

        let desc = InheritanceDescriptor::from_str("wsh(andor(pk(tpubDEN9WSToTyy9ZQfaYqSKfmVqmq1VVLNtYfj3Vkqh67et57eJ5sTKZQBkHqSwPUsoSskJeaYnPttHe2VrkCsKA27kUaN9SDc5zhqeLzKa1rr/*),older(42000),pk(tpubD8LYfn6njiA2inCoxwM7EuN3cuLVcaHAwLYeups13dpevd3nHLRdK9NdQksWXrhLQVxcUZRpnp5CkJ1FhE61WRAsHxDNAkvGkoQkAeWDYjV/*)))").unwrap();
        assert_eq!(desc.timelock_value(), 42000);

        let desc = InheritanceDescriptor::from_str("wsh(andor(pk(tpubDEN9WSToTyy9ZQfaYqSKfmVqmq1VVLNtYfj3Vkqh67et57eJ5sTKZQBkHqSwPUsoSskJeaYnPttHe2VrkCsKA27kUaN9SDc5zhqeLzKa1rr/*),older(65535),pk(tpubD8LYfn6njiA2inCoxwM7EuN3cuLVcaHAwLYeups13dpevd3nHLRdK9NdQksWXrhLQVxcUZRpnp5CkJ1FhE61WRAsHxDNAkvGkoQkAeWDYjV/*)))").unwrap();
        assert_eq!(desc.timelock_value(), 0xffff);
    }

    // TODO: test error conditions of deserialization.
}
