use miniscript::{
    bitcoin::{
        self,
        blockdata::transaction::Sequence,
        hashes::{hash160, ripemd160, sha256},
        secp256k1,
        util::bip32,
    },
    descriptor, hash256,
    miniscript::{decode::Terminal, Miniscript},
    policy::{Liftable, Semantic as SemanticPolicy},
    translate_hash_clone, ForEachKey, MiniscriptKey, ScriptContext, ToPublicKey, TranslatePk,
    Translator,
};

use std::{collections::BTreeMap, convert::TryFrom, error, fmt, str, sync};

use serde::{Deserialize, Serialize};

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
    pub origin: (bip32::Fingerprint, bip32::DerivationPath),
    /// The actual key
    pub key: bitcoin::PublicKey,
}

impl fmt::Display for DerivedPublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (fingerprint, deriv_path) = &self.origin;

        write!(f, "[")?;
        for byte in fingerprint.as_bytes().iter() {
            write!(f, "{:02x}", byte)?;
        }
        write!(f, "/{}", deriv_path)?;
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
        let deriv_path = bip32::DerivationPath::from_str(&fg_deriv[9..])
            .map_err(|_| DescCreationError::DerivedKeyParsing)?;
        if deriv_path.into_iter().any(bip32::ChildNumber::is_hardened) {
            return Err(DescCreationError::DerivedKeyParsing);
        }

        let key = bitcoin::PublicKey::from_str(key_str)
            .map_err(|_| DescCreationError::DerivedKeyParsing)?;

        Ok(DerivedPublicKey {
            key,
            origin: (fingerprint, deriv_path),
        })
    }
}

impl MiniscriptKey for DerivedPublicKey {
    type Sha256 = sha256::Hash;
    type Hash256 = hash256::Hash;
    type Ripemd160 = ripemd160::Hash;
    type Hash160 = hash160::Hash;

    fn is_uncompressed(&self) -> bool {
        self.key.is_uncompressed()
    }

    fn is_x_only_key(&self) -> bool {
        false
    }

    fn num_der_paths(&self) -> usize {
        0
    }
}

impl ToPublicKey for DerivedPublicKey {
    fn to_public_key(&self) -> bitcoin::PublicKey {
        self.key
    }

    fn to_sha256(hash: &sha256::Hash) -> sha256::Hash {
        *hash
    }

    fn to_hash256(hash: &hash256::Hash) -> hash256::Hash {
        *hash
    }

    fn to_ripemd160(hash: &ripemd160::Hash) -> ripemd160::Hash {
        *hash
    }

    fn to_hash160(hash: &hash160::Hash) -> hash160::Hash {
        *hash
    }
}

// We require the locktime to:
//  - not be disabled
//  - be in number of blocks
//  - be 'clean' / minimal, ie all bits without consensus meaning should be 0
//
// All this is achieved simply through asking for a 16-bit integer, since all the
// above are signaled in leftmost bits.
fn csv_check(csv_value: u32) -> Result<(), DescCreationError> {
    u16::try_from(csv_value)
        .map(|_| ())
        .map_err(|_| DescCreationError::InsaneTimelock(csv_value))
}

fn is_unhardened_deriv(key: &descriptor::DescriptorPublicKey) -> bool {
    match *key {
        descriptor::DescriptorPublicKey::Single(..)
        | descriptor::DescriptorPublicKey::MultiXPub(..) => false,
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
        let invalid_key = ms.iter_pk().find_map(|pk| {
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
            .find(|s| matches!(s, SemanticPolicy::Key(_)))
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
        let csv_value = heir_subs
            .iter()
            .find_map(|s| match s {
                SemanticPolicy::Older(csv) => Some(csv),
                _ => None,
            })
            .ok_or(DescCreationError::IncompatibleDesc)?;
        csv_check(csv_value.to_consensus_u32())?;
        // And key locked
        heir_subs
            .iter()
            .find(|s| matches!(s, SemanticPolicy::Key(_)))
            .ok_or(DescCreationError::IncompatibleDesc)?;

        Ok(InheritanceDescriptor(descriptor::Descriptor::Wsh(wsh_desc)))
    }
}

impl InheritanceDescriptor {
    pub fn new(
        owner_key: descriptor::DescriptorPublicKey,
        heir_key: descriptor::DescriptorPublicKey,
        timelock: u16,
    ) -> Result<InheritanceDescriptor, DescCreationError> {
        // We require the locktime to:
        //  - not be disabled
        //  - be in number of blocks
        //  - be 'clean' / minimal, ie all bits without consensus meaning should be 0
        //
        // All this is achieved through asking for a 16-bit integer.
        let timelock = Sequence::from_height(timelock);

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

    /// Whether all xpubs contained in this descriptor are for the passed expected network.
    pub fn all_xpubs_net_is(&self, expected_net: bitcoin::Network) -> bool {
        self.0.for_each_key(|xpub| {
            if let descriptor::DescriptorPublicKey::XPub(xpub) = xpub {
                xpub.xkey.network == expected_net
            } else {
                false
            }
        })
    }

    /// Derive this descriptor at a given index for a receiving address.
    ///
    /// # Panics
    /// - If the given index is hardened.
    pub fn derive_receive(
        &self,
        index: bip32::ChildNumber,
        secp: &secp256k1::Secp256k1<impl secp256k1::Verification>,
    ) -> DerivedInheritanceDescriptor {
        assert!(index.is_normal());

        // Unfortunately we can't just use `self.0.at_derivation_index().derived_descriptor()`
        // since it would return a raw public key, but we need the origin too.
        // TODO: upstream our DerivedPublicKey stuff to rust-miniscript.
        //
        // So we roll our own translation.
        struct Derivator<'a, C: secp256k1::Verification>(u32, &'a secp256k1::Secp256k1<C>);
        impl<'a, C: secp256k1::Verification>
            Translator<
                descriptor::DescriptorPublicKey,
                DerivedPublicKey,
                descriptor::ConversionError,
            > for Derivator<'a, C>
        {
            fn pk(
                &mut self,
                pk: &descriptor::DescriptorPublicKey,
            ) -> Result<DerivedPublicKey, descriptor::ConversionError> {
                let definite_key = pk
                    .clone()
                    .at_derivation_index(self.0)
                    .expect("We disallow multipath keys.");
                let origin = (
                    definite_key.master_fingerprint(),
                    definite_key
                        .full_derivation_path()
                        .expect("We disallow multipath keys."),
                );
                let key = definite_key.derive_public_key(self.1)?;
                Ok(DerivedPublicKey { origin, key })
            }
            translate_hash_clone!(
                descriptor::DescriptorPublicKey,
                DerivedPublicKey,
                descriptor::ConversionError
            );
        }

        let desc = self
            .0
            .translate_pk(&mut Derivator(index.into(), secp))
            .expect("May only fail on hardened derivation indexes, but we ruled out this case.");
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

        assert!(csv.is_height_locked());
        csv.to_consensus_u32()
    }
}

/// Map of a raw public key to the xpub used to derive it and its derivation path
pub type Bip32Deriv = BTreeMap<secp256k1::PublicKey, (bip32::Fingerprint, bip32::DerivationPath)>;

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
        self.0.explicit_script().expect("Not a Taproot descriptor")
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
        ms.iter_pk()
            .map(|k| (k.key.inner, (k.origin.0, k.origin.1)))
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
        assert_eq!(InheritanceDescriptor::new(owner_key, heir_key, timelock).unwrap().to_string(), "wsh(or_d(pk(xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/*),and_v(v:pkh(xpub688Hn4wScQAAiYJLPg9yH27hUpfZAUnmJejRQBCiwfP5PEDzjWMNW1wChcninxr5gyavFqbbDjdV1aK5USJz8NDVjUy7FRQaaqqXHh5SbXe/*),older(52560))))#eeyujkt7");

        // We prevent footguns with timelocks by requiring a u16. Note how the following wouldn't
        // compile:
        //InheritanceDescriptor::new(owner_key.clone(), heir_key.clone(), 0x00_01_0f_00).unwrap_err();
        //InheritanceDescriptor::new(owner_key.clone(), heir_key.clone(), (1 << 31) + 1).unwrap_err();
        //InheritanceDescriptor::new(owner_key, heir_key, (1 << 22) + 1).unwrap_err();

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
        let der_desc = desc.derive_receive(11.into(), &secp);
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
