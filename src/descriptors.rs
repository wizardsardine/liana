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

const WITNESS_FACTOR: usize = 4;

// Convert a size in weight units to a size in virtual bytes, rounding up.
fn wu_to_vb(vb: usize) -> usize {
    (vb + WITNESS_FACTOR - 1)
        .checked_div(WITNESS_FACTOR)
        .expect("Non 0")
}

#[derive(Debug)]
pub enum DescCreationError {
    InsaneTimelock(u32),
    InvalidKey(Box<descriptor::DescriptorPublicKey>),
    Miniscript(miniscript::Error),
    IncompatibleDesc,
    DerivedKeyParsing,
}

impl std::fmt::Display for DescCreationError {
    fn fmt(&self, f: &mut fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::InsaneTimelock(tl) => write!(f, "Timelock value '{}' isn't safe to use", tl),
            Self::InvalidKey(key) => {
                write!(
                    f,
                    "Invalid key '{}'. Need a wildcard ('ranged') xpub with a multipath for (and only for) deriving change addresses. That is, an xpub of the form 'xpub.../<0;1>/*'.",
                    key
                    )
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

// We require the descriptor key to:
//  - Be deriveable (to contain a wildcard)
//  - Be multipath (to contain a step in the derivation path with multiple indexes)
//  - The multipath step to only contain two indexes, 0 and 1.
fn is_valid_desc_key(key: &descriptor::DescriptorPublicKey) -> bool {
    match *key {
        descriptor::DescriptorPublicKey::Single(..) | descriptor::DescriptorPublicKey::XPub(..) => {
            false
        }
        descriptor::DescriptorPublicKey::MultiXPub(ref xpub) => {
            // Rust-miniscript enforces BIP389 which states that all paths must have the same len.
            let len = xpub.derivation_paths.get(0).expect("Cannot be empty").len();
            xpub.wildcard == descriptor::Wildcard::Unhardened
                && xpub.derivation_paths.len() == 2
                && xpub.derivation_paths[0][len - 1] == 0.into()
                && xpub.derivation_paths[1][len - 1] == 1.into()
        }
    }
}

/// An [InheritanceDescriptor] that contains multipath keys for (and only for) the receive keychain
/// and the change keychain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultipathDescriptor {
    multi_desc: descriptor::Descriptor<descriptor::DescriptorPublicKey>,
    receive_desc: InheritanceDescriptor,
    change_desc: InheritanceDescriptor,
}

/// A Miniscript descriptor with a main, unencombered, branch (the main owner of the coins)
/// and a timelocked branch (the heir). All keys in this descriptor are singlepath.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InheritanceDescriptor(descriptor::Descriptor<descriptor::DescriptorPublicKey>);

/// Derived (containing only raw Bitcoin public keys) version of the inheritance descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DerivedInheritanceDescriptor(descriptor::Descriptor<DerivedPublicKey>);

impl fmt::Display for MultipathDescriptor {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.multi_desc)
    }
}

impl str::FromStr for MultipathDescriptor {
    type Err = DescCreationError;

    fn from_str(s: &str) -> Result<MultipathDescriptor, Self::Err> {
        let wsh_desc = descriptor::Wsh::<descriptor::DescriptorPublicKey>::from_str(s)
            .map_err(DescCreationError::Miniscript)?;
        let ms = match wsh_desc.as_inner() {
            descriptor::WshInner::Ms(ms) => ms,
            _ => return Err(DescCreationError::IncompatibleDesc),
        };
        let invalid_key = ms.iter_pk().find_map(|pk| {
            if is_valid_desc_key(&pk) {
                None
            } else {
                Some(pk)
            }
        });
        if let Some(key) = invalid_key {
            return Err(DescCreationError::InvalidKey(key.into()));
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
        let multi_desc = descriptor::Descriptor::Wsh(wsh_desc);

        // Compute the receive and change "sub" descriptors right away. According to our pubkey
        // check above, there must be only two of those, 0 and 1.
        // We use /0/* for receiving and /1/* for change.
        // FIXME: don't rely on into_single_descs()'s ordering.
        let mut singlepath_descs = multi_desc
            .clone()
            .into_single_descriptors()
            .expect("Can't error, all paths have the same length")
            .into_iter();
        assert_eq!(singlepath_descs.len(), 2);
        let receive_desc = InheritanceDescriptor(singlepath_descs.next().expect("First of 2"));
        let change_desc = InheritanceDescriptor(singlepath_descs.next().expect("Second of 2"));

        Ok(MultipathDescriptor {
            multi_desc,
            receive_desc,
            change_desc,
        })
    }
}

impl fmt::Display for InheritanceDescriptor {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl PartialEq<descriptor::Descriptor<descriptor::DescriptorPublicKey>> for InheritanceDescriptor {
    fn eq(&self, other: &descriptor::Descriptor<descriptor::DescriptorPublicKey>) -> bool {
        self.0.eq(other)
    }
}

impl MultipathDescriptor {
    pub fn new(
        owner_key: descriptor::DescriptorPublicKey,
        heir_key: descriptor::DescriptorPublicKey,
        timelock: u16,
    ) -> Result<MultipathDescriptor, DescCreationError> {
        // We require the locktime to:
        //  - not be disabled
        //  - be in number of blocks
        //  - be 'clean' / minimal, ie all bits without consensus meaning should be 0
        //
        // All this is achieved through asking for a 16-bit integer.
        let timelock = Sequence::from_height(timelock);

        if let Some(key) = vec![&owner_key, &heir_key]
            .iter()
            .find(|k| !is_valid_desc_key(k))
        {
            return Err(DescCreationError::InvalidKey((**key).clone().into()));
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
        let multi_desc = descriptor::Descriptor::Wsh(
            descriptor::Wsh::new(tl_miniscript).expect("Must pass sanity checks"),
        );

        // Compute the receive and change "sub" descriptors right away. According to our pubkey
        // check above, there must be only two of those, 0 and 1.
        // We use /0/* for receiving and /1/* for change.
        // FIXME: don't rely on into_single_descs()'s ordering.
        let mut singlepath_descs = multi_desc
            .clone()
            .into_single_descriptors()
            .expect("Can't error, all paths have the same length")
            .into_iter();
        assert_eq!(singlepath_descs.len(), 2);
        let receive_desc = InheritanceDescriptor(singlepath_descs.next().expect("First of 2"));
        let change_desc = InheritanceDescriptor(singlepath_descs.next().expect("Second of 2"));

        Ok(MultipathDescriptor {
            multi_desc,
            receive_desc,
            change_desc,
        })
    }

    /// Whether all xpubs contained in this descriptor are for the passed expected network.
    pub fn all_xpubs_net_is(&self, expected_net: bitcoin::Network) -> bool {
        self.multi_desc.for_each_key(|xpub| {
            if let descriptor::DescriptorPublicKey::MultiXPub(xpub) = xpub {
                xpub.xkey.network == expected_net
            } else {
                false
            }
        })
    }

    /// Get the descriptor for receiving addresses.
    pub fn receive_descriptor(&self) -> &InheritanceDescriptor {
        &self.receive_desc
    }

    /// Get the descriptor for change addresses.
    pub fn change_descriptor(&self) -> &InheritanceDescriptor {
        &self.change_desc
    }

    /// Get the value (in blocks) of the relative timelock for the heir's spending path.
    pub fn timelock_value(&self) -> u32 {
        let wsh_desc = match &self.multi_desc {
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

impl InheritanceDescriptor {
    /// Derive this descriptor at a given index for a receiving address.
    ///
    /// # Panics
    /// - If the given index is hardened.
    pub fn derive(
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

        DerivedInheritanceDescriptor(
            self.0
                .translate_pk(&mut Derivator(index.into(), secp))
                .expect(
                    "May only fail on hardened derivation indexes, but we ruled out this case.",
                ),
        )
    }

    /// Get the maximum size in WU of a satisfaction for this descriptor.
    pub fn max_sat_weight(&self) -> usize {
        self.0
            .max_satisfaction_weight()
            .expect("Cannot fail for P2WSH")
    }

    /// Get the maximum size in virtual bytes of the whole input in a transaction spending
    /// a coin with this Script.
    pub fn spender_input_size(&self) -> usize {
        // txid + vout + nSequence + empty scriptSig + witness
        32 + 4 + 4 + 1 + wu_to_vb(self.max_sat_weight())
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
        let owner_key = descriptor::DescriptorPublicKey::from_str("xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*").unwrap();
        let heir_key = descriptor::DescriptorPublicKey::from_str("xpub688Hn4wScQAAiYJLPg9yH27hUpfZAUnmJejRQBCiwfP5PEDzjWMNW1wChcninxr5gyavFqbbDjdV1aK5USJz8NDVjUy7FRQaaqqXHh5SbXe/<0;1>/*").unwrap();
        let timelock = 52560;
        assert_eq!(MultipathDescriptor::new(owner_key, heir_key, timelock).unwrap().to_string(), "wsh(or_d(pk(xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*),and_v(v:pkh(xpub688Hn4wScQAAiYJLPg9yH27hUpfZAUnmJejRQBCiwfP5PEDzjWMNW1wChcninxr5gyavFqbbDjdV1aK5USJz8NDVjUy7FRQaaqqXHh5SbXe/<0;1>/*),older(52560))))#8n2ydpkt");

        // We prevent footguns with timelocks by requiring a u16. Note how the following wouldn't
        // compile:
        //MultipathDescriptor::new(owner_key.clone(), heir_key.clone(), 0x00_01_0f_00).unwrap_err();
        //MultipathDescriptor::new(owner_key.clone(), heir_key.clone(), (1 << 31) + 1).unwrap_err();
        //MultipathDescriptor::new(owner_key, heir_key, (1 << 22) + 1).unwrap_err();

        let owner_key = descriptor::DescriptorPublicKey::from_str("[aabb0011/10/4893]xpub661MyMwAqRbcFG59fiikD8UV762quhruT8K8bdjqy6N2o3LG7yohoCdLg1m2HAY1W6rfBrtauHkBhbfA4AQ3iazaJj5wVPhwgaRCHBW2DBg/<0;1>/*").unwrap();
        let heir_key = descriptor::DescriptorPublicKey::from_str("xpub661MyMwAqRbcFfxf71L4Dx4w5TmyNXrBicTEAM7vLzumxangwATWWgdJPb6xH1JHcJH9S3jNZx3fCnkkB1WyqrqGgavj1rehHcbythmruvZ/24/32/<0;1>/*").unwrap();
        let timelock = 57600;
        assert_eq!(MultipathDescriptor::new(owner_key.clone(), heir_key, timelock).unwrap().to_string(), "wsh(or_d(pk([aabb0011/10/4893]xpub661MyMwAqRbcFG59fiikD8UV762quhruT8K8bdjqy6N2o3LG7yohoCdLg1m2HAY1W6rfBrtauHkBhbfA4AQ3iazaJj5wVPhwgaRCHBW2DBg/<0;1>/*),and_v(v:pkh(xpub661MyMwAqRbcFfxf71L4Dx4w5TmyNXrBicTEAM7vLzumxangwATWWgdJPb6xH1JHcJH9S3jNZx3fCnkkB1WyqrqGgavj1rehHcbythmruvZ/24/32/<0;1>/*),older(57600))))#l6dlpc2l");

        // We can't pass a raw key, an xpub that is not deriveable, only hardened derivable,
        // without both the change and receive derivation paths, or with more than 2 different
        // derivation paths.
        let heir_key = descriptor::DescriptorPublicKey::from_str("xpub661MyMwAqRbcFfxf71L4Dx4w5TmyNXrBicTEAM7vLzumxangwATWWgdJPb6xH1JHcJH9S3jNZx3fCnkkB1WyqrqGgavj1rehHcbythmruvZ/0/<0;1>/354").unwrap();
        MultipathDescriptor::new(owner_key.clone(), heir_key, timelock).unwrap_err();
        let heir_key = descriptor::DescriptorPublicKey::from_str("xpub661MyMwAqRbcFfxf71L4Dx4w5TmyNXrBicTEAM7vLzumxangwATWWgdJPb6xH1JHcJH9S3jNZx3fCnkkB1WyqrqGgavj1rehHcbythmruvZ/0/<0;1>/*'").unwrap();
        MultipathDescriptor::new(owner_key.clone(), heir_key, timelock).unwrap_err();
        let heir_key = descriptor::DescriptorPublicKey::from_str(
            "02e24913be26dbcfdf8e8e94870b28725cdae09b448b6c127767bf0154e3a3c8e5",
        )
        .unwrap();
        MultipathDescriptor::new(owner_key.clone(), heir_key, timelock).unwrap_err();
        let heir_key = descriptor::DescriptorPublicKey::from_str("xpub661MyMwAqRbcFfxf71L4Dx4w5TmyNXrBicTEAM7vLzumxangwATWWgdJPb6xH1JHcJH9S3jNZx3fCnkkB1WyqrqGgavj1rehHcbythmruvZ/0/*'").unwrap();
        MultipathDescriptor::new(owner_key.clone(), heir_key, timelock).unwrap_err();
        let heir_key = descriptor::DescriptorPublicKey::from_str("xpub661MyMwAqRbcFfxf71L4Dx4w5TmyNXrBicTEAM7vLzumxangwATWWgdJPb6xH1JHcJH9S3jNZx3fCnkkB1WyqrqGgavj1rehHcbythmruvZ/<0;1;2>/*'").unwrap();
        MultipathDescriptor::new(owner_key, heir_key, timelock).unwrap_err();
    }

    #[test]
    fn inheritance_descriptor_derivation() {
        let secp = secp256k1::Secp256k1::verification_only();
        let desc = MultipathDescriptor::from_str("wsh(andor(pk(tpubDEN9WSToTyy9ZQfaYqSKfmVqmq1VVLNtYfj3Vkqh67et57eJ5sTKZQBkHqSwPUsoSskJeaYnPttHe2VrkCsKA27kUaN9SDc5zhqeLzKa1rr/<0;1>/*),older(10000),pk(tpubD8LYfn6njiA2inCoxwM7EuN3cuLVcaHAwLYeups13dpevd3nHLRdK9NdQksWXrhLQVxcUZRpnp5CkJ1FhE61WRAsHxDNAkvGkoQkAeWDYjV/<0;1>/*)))#5f6qd0d9").unwrap();
        let der_desc = desc.receive_descriptor().derive(11.into(), &secp);
        assert_eq!(
            "bc1q26gtczlz03u6juf5cxppapk4sr4fyz53s3g4zs2cgactcahqv6yqc2t8e6",
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
        let desc = MultipathDescriptor::from_str("wsh(andor(pk(tpubDEN9WSToTyy9ZQfaYqSKfmVqmq1VVLNtYfj3Vkqh67et57eJ5sTKZQBkHqSwPUsoSskJeaYnPttHe2VrkCsKA27kUaN9SDc5zhqeLzKa1rr/<0;1>/*),older(1),pk(tpubD8LYfn6njiA2inCoxwM7EuN3cuLVcaHAwLYeups13dpevd3nHLRdK9NdQksWXrhLQVxcUZRpnp5CkJ1FhE61WRAsHxDNAkvGkoQkAeWDYjV/<0;1>/*)))").unwrap();
        assert_eq!(desc.timelock_value(), 1);

        let desc = MultipathDescriptor::from_str("wsh(andor(pk(tpubDEN9WSToTyy9ZQfaYqSKfmVqmq1VVLNtYfj3Vkqh67et57eJ5sTKZQBkHqSwPUsoSskJeaYnPttHe2VrkCsKA27kUaN9SDc5zhqeLzKa1rr/<0;1>/*),older(42000),pk(tpubD8LYfn6njiA2inCoxwM7EuN3cuLVcaHAwLYeups13dpevd3nHLRdK9NdQksWXrhLQVxcUZRpnp5CkJ1FhE61WRAsHxDNAkvGkoQkAeWDYjV/<0;1>/*)))").unwrap();
        assert_eq!(desc.timelock_value(), 42000);

        let desc = MultipathDescriptor::from_str("wsh(andor(pk(tpubDEN9WSToTyy9ZQfaYqSKfmVqmq1VVLNtYfj3Vkqh67et57eJ5sTKZQBkHqSwPUsoSskJeaYnPttHe2VrkCsKA27kUaN9SDc5zhqeLzKa1rr/<0;1>/*),older(65535),pk(tpubD8LYfn6njiA2inCoxwM7EuN3cuLVcaHAwLYeups13dpevd3nHLRdK9NdQksWXrhLQVxcUZRpnp5CkJ1FhE61WRAsHxDNAkvGkoQkAeWDYjV/<0;1>/*)))").unwrap();
        assert_eq!(desc.timelock_value(), 0xffff);
    }

    #[test]
    fn inheritance_descriptor_sat_size() {
        let secp = secp256k1::Secp256k1::verification_only();
        let desc = MultipathDescriptor::from_str("wsh(or_d(pk([92162c45]tpubD6NzVbkrYhZ4WzTf9SsD6h7AH7oQEippXK2KP8qvhMMqFoNeN5YFVi7vRyeRSDGtgd2bPyMxUNmHui8t5yCgszxPPxMafu1VVzDpg9aruYW/<0;1>/*),and_v(v:pkh(tpubD6NzVbkrYhZ4Wdgu2yfdmrce5g4fiH1ZLmKhewsnNKupbi4sxjH1ZVAorkBLWSkhsjhg8kiq8C4BrBjMy3SjAKDyDdbuvUa1ToAHbiR98js/<0;1>/*),older(2))))#uact7s3g").unwrap();
        let receive_desc = desc.receive_descriptor();
        let change_desc = desc.change_descriptor();

        // Receive and change are the same descriptor.
        assert_eq!(receive_desc.max_sat_weight(), change_desc.max_sat_weight());
        assert_eq!(
            receive_desc.spender_input_size(),
            change_desc.spender_input_size()
        );

        // Derived or not the expected maximum satisfaction size should be the same for
        // the same descriptor.
        assert_eq!(
            receive_desc.derive(999.into(), &secp).max_sat_weight(),
            change_desc.derive(999.into(), &secp).max_sat_weight()
        );

        // Maximum input size is (txid + vout + scriptsig + nSequence + max_sat).
        // Where max_sat is:
        // - Push the witness stack size
        // - Push the script
        // - Push an empty vector for using the recovery path
        // - Push the recovery key
        // - Push a signature for the recovery key
        // NOTE: The specific value is asserted because this was tested against a regtest
        // transaction.
        let stack = vec![vec![0; 68], vec![0; 0], vec![0; 33], vec![0; 72]];
        let witness_size = bitcoin::VarInt(stack.len() as u64).len()
            + stack
                .iter()
                .map(|item| bitcoin::VarInt(stack.len() as u64).len() + item.len())
                .sum::<usize>();
        assert_eq!(
            receive_desc.spender_input_size(),
            32 + 4 + 1 + 4 + wu_to_vb(witness_size),
        );
    }

    // TODO: test error conditions of deserialization.
}
