use miniscript::{
    bitcoin::{
        self,
        blockdata::transaction::Sequence,
        hashes::{hash160, ripemd160, sha256},
        secp256k1,
        util::{
            bip32,
            psbt::{Input as PsbtIn, Psbt},
        },
    },
    descriptor, hash256,
    miniscript::{decode::Terminal, Miniscript},
    policy::{Liftable, Semantic as SemanticPolicy},
    translate_hash_clone, ForEachKey, MiniscriptKey, ScriptContext, ToPublicKey, TranslatePk,
    Translator,
};

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    convert::TryFrom,
    error, fmt, str, sync,
};

use serde::{Deserialize, Serialize};

const WITNESS_FACTOR: usize = 4;

// Convert a size in weight units to a size in virtual bytes, rounding up.
fn wu_to_vb(vb: usize) -> usize {
    (vb + WITNESS_FACTOR - 1)
        .checked_div(WITNESS_FACTOR)
        .expect("Non 0")
}

#[derive(Debug)]
pub enum LianaDescError {
    InsaneTimelock(u32),
    InvalidKey(Box<descriptor::DescriptorPublicKey>),
    DuplicateKey(Box<descriptor::DescriptorPublicKey>),
    Miniscript(miniscript::Error),
    IncompatibleDesc,
    DerivedKeyParsing,
    InvalidMultiThresh(usize),
    InvalidMultiKeys(usize),
    /// Different number of PSBT vs tx inputs, etc..
    InsanePsbt,
    /// Not all inputs' sequence the same, not all inputs signed with the same key, ..
    InconsistentPsbt,
}

impl std::fmt::Display for LianaDescError {
    fn fmt(&self, f: &mut fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::InsaneTimelock(tl) => {
                write!(f, "Timelock value '{}' isn't valid or safe to use", tl)
            }
            Self::InvalidKey(key) => {
                write!(
                    f,
                    "Invalid key '{}'. Need a wildcard ('ranged') xpub with an origin and a multipath for (and only for) deriving change addresses. That is, an xpub of the form '[aaff0099]xpub.../<0;1>/*'.",
                    key
                    )
            }
            Self::DuplicateKey(key) => {
                write!(f, "Duplicate key '{}'.", key)
            }
            Self::Miniscript(e) => write!(f, "Miniscript error: '{}'.", e),
            Self::IncompatibleDesc => write!(f, "Descriptor is not compatible."),
            Self::DerivedKeyParsing => write!(f, "Parsing derived key,"),
            Self::InvalidMultiThresh(thresh) => write!(f, "Invalid threshold value '{}'. The threshold must be > to 0 and <= to the number of keys.", thresh),
            Self::InvalidMultiKeys(n_keys) => write!(f, "Invalid number of keys '{}'. Between 2 and 20 keys must be given to use multiple keys in a specific path.", n_keys),
            Self::InsanePsbt => write!(f, "Analyzed PSBT is empty or malformed."),
            Self::InconsistentPsbt => write!(f, "Analyzed PSBT is inconsistent across inputs.")
        }
    }
}

impl error::Error for LianaDescError {}

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
    type Err = LianaDescError;

    fn from_str(s: &str) -> Result<DerivedPublicKey, Self::Err> {
        // The key is always of the form:
        // [ fingerprint / index ]<key>

        // 1 + 8 + 1 + 1 + 1 + 66 minimum
        if s.len() < 78 {
            return Err(LianaDescError::DerivedKeyParsing);
        }

        // Non-ASCII?
        for ch in s.as_bytes() {
            if *ch < 20 || *ch > 127 {
                return Err(LianaDescError::DerivedKeyParsing);
            }
        }

        if s.chars().next().expect("Size checked above") != '[' {
            return Err(LianaDescError::DerivedKeyParsing);
        }

        let mut parts = s[1..].split(']');
        let fg_deriv = parts.next().ok_or(LianaDescError::DerivedKeyParsing)?;
        let key_str = parts.next().ok_or(LianaDescError::DerivedKeyParsing)?;

        if fg_deriv.len() < 10 {
            return Err(LianaDescError::DerivedKeyParsing);
        }
        let fingerprint = bip32::Fingerprint::from_str(&fg_deriv[..8])
            .map_err(|_| LianaDescError::DerivedKeyParsing)?;
        let deriv_path = bip32::DerivationPath::from_str(&fg_deriv[9..])
            .map_err(|_| LianaDescError::DerivedKeyParsing)?;
        if deriv_path.into_iter().any(bip32::ChildNumber::is_hardened) {
            return Err(LianaDescError::DerivedKeyParsing);
        }

        let key =
            bitcoin::PublicKey::from_str(key_str).map_err(|_| LianaDescError::DerivedKeyParsing)?;

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
fn csv_check(csv_value: u32) -> Result<u16, LianaDescError> {
    if csv_value > 0 {
        u16::try_from(csv_value).map_err(|_| LianaDescError::InsaneTimelock(csv_value))
    } else {
        Err(LianaDescError::InsaneTimelock(csv_value))
    }
}

// We require the descriptor key to:
//  - Be deriveable (to contain a wildcard)
//  - Be multipath (to contain a step in the derivation path with multiple indexes)
//  - The multipath step to only contain two indexes, 0 and 1.
//  - Be 'signable' by an external signer (to contain an origin)
fn is_valid_desc_key(key: &descriptor::DescriptorPublicKey) -> bool {
    match *key {
        descriptor::DescriptorPublicKey::Single(..) | descriptor::DescriptorPublicKey::XPub(..) => {
            false
        }
        descriptor::DescriptorPublicKey::MultiXPub(ref xpub) => {
            let der_paths = xpub.derivation_paths.paths();
            // Rust-miniscript enforces BIP389 which states that all paths must have the same len.
            let len = der_paths.get(0).expect("Cannot be empty").len();
            // Technically the xpub could be for the master xpub and not have an origin. But it's
            // no unlikely (and easily fixable) while users shooting themselves in the foot by
            // forgetting to provide the origin is so likely that it's worth ruling out xpubs
            // without origin entirely.
            xpub.origin.is_some()
                && xpub.wildcard == descriptor::Wildcard::Unhardened
                && der_paths.len() == 2
                && der_paths[0][len - 1] == 0.into()
                && der_paths[1][len - 1] == 1.into()
        }
    }
}

// Get the fingerprint for the key in a multipath descriptors.
// Returns None if the given key isn't a multixpub.
fn key_fingerprint(key: &descriptor::DescriptorPublicKey) -> Option<bip32::Fingerprint> {
    match key {
        descriptor::DescriptorPublicKey::MultiXPub(ref xpub) => Some(
            xpub.origin
                .as_ref()
                .map(|o| o.0)
                .unwrap_or_else(|| xpub.xkey.fingerprint()),
        ),
        _ => None,
    }
}

/// The keys in one of the two spending paths of a Liana descriptor.
/// May either be a single key, or between 2 and 20 keys along with a threshold (between two and
/// the number of keys).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LianaDescKeys {
    thresh: Option<usize>,
    keys: Vec<descriptor::DescriptorPublicKey>,
}

impl LianaDescKeys {
    pub fn from_single(key: descriptor::DescriptorPublicKey) -> LianaDescKeys {
        LianaDescKeys {
            thresh: None,
            keys: vec![key],
        }
    }

    pub fn from_multi(
        thresh: usize,
        keys: Vec<descriptor::DescriptorPublicKey>,
    ) -> Result<LianaDescKeys, LianaDescError> {
        if keys.len() < 2 || keys.len() > 20 {
            return Err(LianaDescError::InvalidMultiKeys(keys.len()));
        }
        if thresh == 0 || thresh > keys.len() {
            return Err(LianaDescError::InvalidMultiThresh(thresh));
        }
        Ok(LianaDescKeys {
            thresh: Some(thresh),
            keys,
        })
    }

    pub fn keys(&self) -> &Vec<descriptor::DescriptorPublicKey> {
        &self.keys
    }

    pub fn into_miniscript(
        mut self,
        as_hash: bool,
    ) -> Miniscript<descriptor::DescriptorPublicKey, miniscript::Segwitv0> {
        if let Some(thresh) = self.thresh {
            assert!(self.keys.len() >= 2 && self.keys.len() <= 20);
            Miniscript::from_ast(Terminal::Multi(thresh, self.keys))
                .expect("multi is a valid Miniscript")
        } else {
            assert_eq!(self.keys.len(), 1);
            let key = self.keys.pop().expect("Length was just asserted");
            Miniscript::from_ast(Terminal::Check(sync::Arc::from(
                Miniscript::from_ast(if as_hash {
                    Terminal::PkH(key)
                } else {
                    Terminal::PkK(key)
                })
                .expect("pk_k is a valid Miniscript"),
            )))
            .expect("Well typed")
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

fn is_single_key_or_multisig(policy: &SemanticPolicy<descriptor::DescriptorPublicKey>) -> bool {
    match policy {
        SemanticPolicy::Key(..) => true,
        SemanticPolicy::Threshold(_, subs) => {
            subs.iter().all(|sub| matches!(sub, SemanticPolicy::Key(_)))
        }
        _ => false,
    }
}

impl str::FromStr for MultipathDescriptor {
    type Err = LianaDescError;

    fn from_str(s: &str) -> Result<MultipathDescriptor, Self::Err> {
        let wsh_desc = descriptor::Wsh::<descriptor::DescriptorPublicKey>::from_str(s)
            .map_err(LianaDescError::Miniscript)?;
        let ms = match wsh_desc.as_inner() {
            descriptor::WshInner::Ms(ms) => ms,
            _ => return Err(LianaDescError::IncompatibleDesc),
        };
        let invalid_key = ms.iter_pk().find_map(|pk| {
            if is_valid_desc_key(&pk) {
                None
            } else {
                Some(pk)
            }
        });
        if let Some(key) = invalid_key {
            return Err(LianaDescError::InvalidKey(key.into()));
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
        .ok_or(LianaDescError::IncompatibleDesc)?;
        if subs.len() != 2 {
            return Err(LianaDescError::IncompatibleDesc);
        }

        // Must always contain a non-timelocked primary spending path and a timelocked recovery
        // path. The PathInfo constructors perform the checks that each path is well formed.
        for sub in subs {
            if is_single_key_or_multisig(&sub) {
                PathInfo::from_primary_path(sub)?;
            } else {
                PathInfo::from_recovery_path(sub)?;
            }
        }

        // All good, construct the multipath descriptor.
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

/// Information about a single spending path in the descriptor.
#[derive(Debug, Eq, PartialEq, Clone, Ord, PartialOrd, Hash)]
pub enum PathInfo {
    Single(descriptor::DescriptorPublicKey),
    Multi(usize, Vec<descriptor::DescriptorPublicKey>),
}

impl PathInfo {
    /// Get the information about the primary spending path.
    /// Returns None if the policy does not describe the primary spending path of a Liana
    /// descriptor (that is, a set of keys).
    pub fn from_primary_path(
        policy: SemanticPolicy<descriptor::DescriptorPublicKey>,
    ) -> Result<PathInfo, LianaDescError> {
        match policy {
            SemanticPolicy::Key(key) => Ok(PathInfo::Single(key)),
            SemanticPolicy::Threshold(k, subs) => {
                let keys: Result<_, LianaDescError> = subs
                    .into_iter()
                    .map(|sub| match sub {
                        SemanticPolicy::Key(key) => Ok(key),
                        _ => Err(LianaDescError::IncompatibleDesc),
                    })
                    .collect();
                Ok(PathInfo::Multi(k, keys?))
            }
            _ => Err(LianaDescError::IncompatibleDesc),
        }
    }

    /// Get the information about the recovery spending path.
    /// Returns None if the policy does not describe the recovery spending path of a Liana
    /// descriptor (that is, a set of keys after a timelock).
    pub fn from_recovery_path(
        policy: SemanticPolicy<descriptor::DescriptorPublicKey>,
    ) -> Result<(u16, PathInfo), LianaDescError> {
        // The recovery spending path must always be a policy of type `thresh(2, older(x), thresh(n, key1,
        // key2, ..))`. In the special case n == 1, it is only `thresh(2, older(x), key)`. In the
        // special case n == len(keys) (i.e. it's an N-of-N multisig), it is normalized as
        // `thresh(n+1, older(x), key1, key2, ...)`.
        let (k, subs) = match policy {
            SemanticPolicy::Threshold(k, subs) => (k, subs),
            _ => return Err(LianaDescError::IncompatibleDesc),
        };
        if k == 2 && subs.len() == 2 {
            // The general case (as well as the n == 1 case). The sub that is not the timelock is
            // of the same form as a primary path.
            let tl_value = subs
                .iter()
                .find_map(|s| match s {
                    SemanticPolicy::Older(val) => Some(csv_check(val.0)),
                    _ => None,
                })
                .ok_or(LianaDescError::IncompatibleDesc)??;
            let keys_sub = subs
                .into_iter()
                .find(is_single_key_or_multisig)
                .ok_or(LianaDescError::IncompatibleDesc)?;
            PathInfo::from_primary_path(keys_sub).map(|info| (tl_value, info))
        } else if k == subs.len() && subs.len() > 2 {
            // The N-of-N case. All subs but the threshold must be keys (if one had been thresh()
            // of keys it would have been normalized).
            let mut tl_value = None;
            let mut keys = Vec::with_capacity(subs.len());
            for sub in subs {
                match sub {
                    SemanticPolicy::Key(key) => keys.push(key),
                    SemanticPolicy::Older(val) => {
                        if tl_value.is_some() {
                            return Err(LianaDescError::IncompatibleDesc);
                        }
                        tl_value = Some(csv_check(val.0)?);
                    }
                    _ => return Err(LianaDescError::IncompatibleDesc),
                }
            }
            assert!(keys.len() > 1); // At least 3 subs, only one of which may be older().
            Ok((
                tl_value.ok_or(LianaDescError::IncompatibleDesc)?,
                PathInfo::Multi(k - 1, keys),
            ))
        } else {
            // If there is less than 2 subs, there can't be both a timelock and keys. If the
            // threshold is not equal to the number of subs, the timelock can't be mandatory.
            Err(LianaDescError::IncompatibleDesc)
        }
    }

    /// Get the required number of keys for spending through this path, and the set of keys
    /// that can be used to provide a signature for this path.
    pub fn thresh_fingerprints(&self) -> (usize, HashSet<bip32::Fingerprint>) {
        match self {
            PathInfo::Single(key) => {
                let mut fingerprints = HashSet::with_capacity(1);
                fingerprints.insert(key_fingerprint(key).expect("Must be a multixpub."));
                (1, fingerprints)
            }
            PathInfo::Multi(k, keys) => (
                *k,
                keys.iter()
                    .map(|key| key_fingerprint(key).expect("Must be a multixpub."))
                    .collect(),
            ),
        }
    }

    /// Get the spend information for this descriptor based from the list of all pubkeys that
    /// signed the transaction.
    pub fn spend_info(
        &self,
        all_pubkeys_signed: impl Iterator<Item = bip32::Fingerprint>,
    ) -> PathSpendInfo {
        let mut signed_pubkeys = HashMap::new();
        let mut sigs_count = 0;
        let (threshold, fingerprints) = self.thresh_fingerprints();

        // For all existing signatures, pick those that are from one of our pubkeys.
        for fingerprint in all_pubkeys_signed {
            if fingerprints.contains(&fingerprint) {
                sigs_count += 1;
                if let Some(count) = signed_pubkeys.get_mut(&fingerprint) {
                    *count += 1;
                } else {
                    signed_pubkeys.insert(fingerprint, 1);
                }
            }
        }

        PathSpendInfo {
            threshold,
            sigs_count,
            signed_pubkeys,
        }
    }
}

/// Information about the descriptor: how many keys are present in each path, what's the timelock
/// of the recovery path, what's the threshold if there are multiple keys, etc..
#[derive(Debug, Eq, PartialEq, Clone, Ord, PartialOrd, Hash)]
pub struct LianaDescInfo {
    primary_path: PathInfo,
    recovery_path: (u16, PathInfo),
}

impl LianaDescInfo {
    fn new(primary_path: PathInfo, recovery_path: (u16, PathInfo)) -> LianaDescInfo {
        LianaDescInfo {
            primary_path,
            recovery_path,
        }
    }

    pub fn primary_path(&self) -> &PathInfo {
        &self.primary_path
    }

    /// Timelock and path info for the recovery path.
    pub fn recovery_path(&self) -> (u16, &PathInfo) {
        (self.recovery_path.0, &self.recovery_path.1)
    }
}

/// Partial spend information for a specific spending path within a descriptor.
#[derive(Debug, Eq, PartialEq, Clone)]
pub struct PathSpendInfo {
    /// The required number of signatures to provide to spend through this path.
    pub threshold: usize,
    /// The number of signatures provided.
    pub sigs_count: usize,
    /// The keys for which a signature was provided and the number (always >=1) of
    /// signatures provided for this key.
    pub signed_pubkeys: HashMap<bip32::Fingerprint, usize>,
}

/// Information about a partial spend of Liana coins
#[derive(Debug, Eq, PartialEq, Clone)]
pub struct PartialSpendInfo {
    /// Number of signatures present for the primary path
    primary_path: PathSpendInfo,
    /// Number of signatures present for the recovery path, only present if the path is available
    /// in the first place.
    recovery_path: Option<PathSpendInfo>,
}

impl PartialSpendInfo {
    /// Get the number of signatures present for the primary path
    pub fn primary_path(&self) -> &PathSpendInfo {
        &self.primary_path
    }

    /// Get the number of signatures present for the recovery path. Only present if the path is
    /// available in the first place.
    pub fn recovery_path(&self) -> &Option<PathSpendInfo> {
        &self.recovery_path
    }
}

impl MultipathDescriptor {
    pub fn new(
        owner_keys: LianaDescKeys,
        heir_keys: LianaDescKeys,
        timelock: u16,
    ) -> Result<MultipathDescriptor, LianaDescError> {
        // We require the locktime to:
        //  - not be disabled
        //  - be in number of blocks
        //  - be 'clean' / minimal, ie all bits without consensus meaning should be 0
        //  - be positive (Miniscript requires it not to be 0)
        //
        // All this is achieved through asking for a 16-bit integer.
        if timelock == 0 {
            return Err(LianaDescError::InsaneTimelock(timelock as u32));
        }
        let timelock = Sequence::from_height(timelock);

        // Check all keys are valid according to our standard (this checks all are multipath keys).
        let all_keys = owner_keys.keys().iter().chain(heir_keys.keys().iter());
        if let Some(key) = all_keys.clone().find(|k| !is_valid_desc_key(k)) {
            return Err(LianaDescError::InvalidKey((*key).clone().into()));
        }

        // Check for key duplicates. They are invalid in (nonmalleable) miniscripts.
        let mut key_set = HashSet::new();
        for key in all_keys {
            let xpub = match key {
                descriptor::DescriptorPublicKey::MultiXPub(ref multi_xpub) => multi_xpub.xkey,
                _ => unreachable!("Just checked it was a multixpub above"),
            };
            if key_set.contains(&xpub) {
                return Err(LianaDescError::DuplicateKey(key.clone().into()));
            }
            key_set.insert(xpub);
        }
        assert!(!key_set.is_empty());

        // Create the timelocked spending path. If there is a single key we make it a pk_h() in
        // order to save on the script size (since we assume the timelocked recovery path will
        // seldom be used).
        let heir_timelock = Terminal::Older(timelock);
        let heir_branch = Miniscript::from_ast(Terminal::AndV(
            Miniscript::from_ast(Terminal::Verify(heir_keys.into_miniscript(true).into()))
                .expect("Well typed")
                .into(),
            Miniscript::from_ast(heir_timelock)
                .expect("Well typed")
                .into(),
        ))
        .expect("Well typed");

        // Combine the timelocked spending path with the simple "primary" path. For the primary key
        // we don't use a pkh since it's the one that will likely always be used.
        let tl_miniscript = Miniscript::from_ast(Terminal::OrD(
            owner_keys.into_miniscript(false).into(),
            heir_branch.into(),
        ))
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

    /// Parse information about this descriptor
    pub fn info(&self) -> LianaDescInfo {
        // Get the Miniscript
        let wsh_desc = match &self.multi_desc {
            descriptor::Descriptor::Wsh(desc) => desc,
            _ => unreachable!(),
        };
        let ms = match wsh_desc.as_inner() {
            descriptor::WshInner::Ms(ms) => ms,
            _ => unreachable!(),
        };

        // Lift the semantic policy from the Miniscript
        let policy = ms
            .lift()
            .expect("Lifting can't fail on a Miniscript")
            .normalized();
        let subs = match policy {
            SemanticPolicy::Threshold(1, subs) => subs,
            _ => unreachable!("The policy is always 'one of the primary or the recovery path'"),
        };
        // For now we only ever allow a single recovery path.
        assert_eq!(subs.len(), 2);

        // Fetch the two spending paths' semantic policies. The primary path is identified as the
        // only one that isn't timelocked.
        let (prim_path_sub, reco_path_sub) =
            subs.into_iter()
                .fold((None, None), |(mut prim_sub, mut reco_sub), sub| {
                    if is_single_key_or_multisig(&sub) {
                        prim_sub = Some(sub);
                    } else {
                        reco_sub = Some(sub);
                    }
                    (prim_sub, reco_sub)
                });
        let (prim_path_sub, reco_path_sub) = (
            prim_path_sub.expect("Must be present"),
            reco_path_sub.expect("Must be present"),
        );

        // Now parse information about each spending path.
        let primary_path = PathInfo::from_primary_path(prim_path_sub)
            .expect("Must always be a set of keys without timelock");
        let reco_path = PathInfo::from_recovery_path(reco_path_sub)
            .expect("The recovery path policy must always be a timelock along with a set of keys.");

        LianaDescInfo::new(primary_path, reco_path)
    }

    /// Get the value (in blocks) of the relative timelock for the heir's spending path.
    pub fn timelock_value(&self) -> u32 {
        // TODO: make it return a u16
        self.info().recovery_path.0 as u32
    }

    /// Get the maximum size in WU of a satisfaction for this descriptor.
    pub fn max_sat_weight(&self) -> usize {
        self.multi_desc
            .max_satisfaction_weight()
            .expect("Cannot fail for P2WSH")
    }

    /// Get the maximum size in vbytes (rounded up) of a satisfaction for this descriptor.
    pub fn max_sat_vbytes(&self) -> usize {
        self.multi_desc
            .max_satisfaction_weight()
            .expect("Cannot fail for P2WSH")
            .checked_add(WITNESS_FACTOR - 1)
            .unwrap()
            .checked_div(WITNESS_FACTOR)
            .unwrap()
    }

    /// Get the maximum size in virtual bytes of the whole input in a transaction spending
    /// a coin with this Script.
    pub fn spender_input_size(&self) -> usize {
        // txid + vout + nSequence + empty scriptSig + witness
        32 + 4 + 4 + 1 + wu_to_vb(self.max_sat_weight())
    }

    /// Get some information about a PSBT input spending Liana coins.
    /// This analysis assumes that:
    /// - The PSBT input actually spend a Liana coin for this descriptor. Otherwise the analysis will be off.
    /// - The signatures contained in the PSBT input are valid for this script.
    pub fn partial_spend_info_txin(
        &self,
        psbt_in: &PsbtIn,
        txin: &bitcoin::TxIn,
    ) -> PartialSpendInfo {
        // Get the identifier of all the keys that signed this transaction.
        let pubkeys_signed = psbt_in
            .partial_sigs
            .iter()
            .filter_map(|(pk, _)| psbt_in.bip32_derivation.get(&pk.inner).map(|(fg, _)| *fg));

        // Determine the structure of the descriptor. Then compute the spend info for the primary
        // and recovery paths. Only provide the spend info for the recovery path if it is available
        // (ie if the nSequence is >= to the chosen CSV value).
        let desc_info = self.info();
        let primary_path = desc_info.primary_path.spend_info(pubkeys_signed.clone());
        let recovery_path = if txin.sequence.is_height_locked()
            && txin.sequence.0 >= desc_info.recovery_path.0 as u32
        {
            Some(desc_info.recovery_path.1.spend_info(pubkeys_signed))
        } else {
            None
        };

        PartialSpendInfo {
            primary_path,
            recovery_path,
        }
    }

    // TODO: decide whether we should check the signatures too. To be useful it should check pubkeys
    // correspond to those in the script. And we could be checking the witness scripts are all for
    // our descriptor too..
    /// Get some information about a PSBT spending Liana coins.
    /// This analysis assumes that:
    /// - The PSBT only contains input that spends Liana coins. Otherwise the analysis will be off.
    /// - The PSBT is consistent across inputs (the sequence is the same across inputs, the
    /// signatures are either absent or present for all inputs, ..)
    /// - The provided signatures are valid for this script.
    pub fn partial_spend_info(&self, psbt: &Psbt) -> Result<PartialSpendInfo, LianaDescError> {
        // Check the PSBT isn't empty or malformed.
        if psbt.inputs.len() != psbt.unsigned_tx.input.len()
            || psbt.outputs.len() != psbt.unsigned_tx.output.len()
            || psbt.inputs.is_empty()
            || psbt.outputs.is_empty()
        {
            return Err(LianaDescError::InsanePsbt);
        }

        // We are doing this analysis at a transaction level. We assume that if an input
        // is set to use the recovery path, all are. If one input is signed with a key, all
        // must be.
        // This gets the information needed to analyze the number of signatures from the
        // first input, and checks that this info matches on all inputs.
        let (mut psbt_ins, mut txins) = (psbt.inputs.iter(), psbt.unsigned_tx.input.iter());
        let (first_psbt_in, first_txin) = (
            psbt_ins
                .next()
                .expect("We checked at least one is present."),
            txins.next().expect("We checked at least one is present."),
        );
        let spend_info = self.partial_spend_info_txin(first_psbt_in, first_txin);
        for (psbt_in, txin) in psbt_ins.zip(txins) {
            // TODO: maybe it's better to not error if one of the input has more, or different
            // signatures? Instead of erroring we could ignore the superfluous data?
            if txin.sequence != first_txin.sequence
                || spend_info != self.partial_spend_info_txin(psbt_in, txin)
            {
                return Err(LianaDescError::InconsistentPsbt);
            }
        }

        Ok(spend_info)
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
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::str::FromStr;

    #[test]
    fn descriptor_creation() {
        let owner_key = LianaDescKeys::from_single(descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*").unwrap());
        let heir_key = LianaDescKeys::from_single(descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub688Hn4wScQAAiYJLPg9yH27hUpfZAUnmJejRQBCiwfP5PEDzjWMNW1wChcninxr5gyavFqbbDjdV1aK5USJz8NDVjUy7FRQaaqqXHh5SbXe/<0;1>/*").unwrap());
        let timelock = 52560;
        assert_eq!(MultipathDescriptor::new(owner_key.clone(), heir_key.clone(), timelock).unwrap().to_string(), "wsh(or_d(pk([abcdef01]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*),and_v(v:pkh([abcdef01]xpub688Hn4wScQAAiYJLPg9yH27hUpfZAUnmJejRQBCiwfP5PEDzjWMNW1wChcninxr5gyavFqbbDjdV1aK5USJz8NDVjUy7FRQaaqqXHh5SbXe/<0;1>/*),older(52560))))#g7vk9r5l");

        // A decaying multisig after 6 months. Note we can't duplicate the keys, so different ones
        // are used. In practice they would both be controlled by the same entity.
        let primary_keys = LianaDescKeys::from_multi(
            3,
            vec![
                descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*").unwrap(),
                descriptor::DescriptorPublicKey::from_str("[aabb0011/10/4893]xpub6Bw79HbNSeS2xXw1sngPE3ehnk1U3iSPCgLYzC9LpN8m9nDuaKLZvkg8QXxL5pDmEmQtYscmUD8B9MkAAZbh6vxPzNXMaLfGQ9Sb3z85qhR/<0;1>/*").unwrap(),
                descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub67zuTXF9Ln4731avKTBSawoVVNRuMfmRvkL7kLUaLBRqma9ZqdHBJg9qx8cPUm3oNQMiXT4TmGovXNoQPuwg17RFcVJ8YrnbcooN7pxVJqC/<0;1>/*").unwrap()
            ]
        )
        .unwrap();
        let recovery_keys = LianaDescKeys::from_multi(
            2,
            vec![
                descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub69cP4Y7S9TWcbSNxmk6CEDBsoaqr3ZEdjHuZcHxEFFKGh569RsJNr2V27XGhsbH9FXgWUEmKXRN7c5wQfq2VPjt31xP9VsYnVUyU8HcVevm/<0;1>/*").unwrap(),
                descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub6AA2N8RALRYgLD6jT1iXYCEDkndTeZndMtWPbtNX6sY5dPiLtf2T88ahdxrGXMUPoNadgR86sFhBXWQVgifPzDYbY9ZtwK4gqzx4y5Da1DW/<0;1>/*").unwrap(),
                descriptor::DescriptorPublicKey::from_str("[aabb0011/10/4893]xpub6AyxexvxizZJffF153evmfqHcE9MV88fCNCAtP3jQjXJHwrAKri71Tq9jWUkPxj9pja4u6AkCPHY7atgxzSEa2HtDwJfrRWKK4fsfQg4o77/<0;1>/*").unwrap(),
            ],
        )
        .unwrap();
        assert_eq!(MultipathDescriptor::new(primary_keys, recovery_keys, 26352).unwrap().to_string(), "wsh(or_d(multi(3,[abcdef01]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*,[aabb0011/10/4893]xpub6Bw79HbNSeS2xXw1sngPE3ehnk1U3iSPCgLYzC9LpN8m9nDuaKLZvkg8QXxL5pDmEmQtYscmUD8B9MkAAZbh6vxPzNXMaLfGQ9Sb3z85qhR/<0;1>/*,[abcdef01]xpub67zuTXF9Ln4731avKTBSawoVVNRuMfmRvkL7kLUaLBRqma9ZqdHBJg9qx8cPUm3oNQMiXT4TmGovXNoQPuwg17RFcVJ8YrnbcooN7pxVJqC/<0;1>/*),and_v(v:multi(2,[abcdef01]xpub69cP4Y7S9TWcbSNxmk6CEDBsoaqr3ZEdjHuZcHxEFFKGh569RsJNr2V27XGhsbH9FXgWUEmKXRN7c5wQfq2VPjt31xP9VsYnVUyU8HcVevm/<0;1>/*,[abcdef01]xpub6AA2N8RALRYgLD6jT1iXYCEDkndTeZndMtWPbtNX6sY5dPiLtf2T88ahdxrGXMUPoNadgR86sFhBXWQVgifPzDYbY9ZtwK4gqzx4y5Da1DW/<0;1>/*,[aabb0011/10/4893]xpub6AyxexvxizZJffF153evmfqHcE9MV88fCNCAtP3jQjXJHwrAKri71Tq9jWUkPxj9pja4u6AkCPHY7atgxzSEa2HtDwJfrRWKK4fsfQg4o77/<0;1>/*),older(26352))))#s0zsa6uc");

        // We prevent footguns with timelocks by requiring a u16. Note how the following wouldn't
        // compile:
        //MultipathDescriptor::new(owner_key.clone(), heir_key.clone(), 0x00_01_0f_00).unwrap_err();
        //MultipathDescriptor::new(owner_key.clone(), heir_key.clone(), (1 << 31) + 1).unwrap_err();
        //MultipathDescriptor::new(owner_key, heir_key, (1 << 22) + 1).unwrap_err();

        // You can't use a null timelock in Miniscript.
        MultipathDescriptor::new(owner_key, heir_key, 0).unwrap_err();

        let owner_key = LianaDescKeys::from_single(descriptor::DescriptorPublicKey::from_str("[aabb0011/10/4893]xpub661MyMwAqRbcFG59fiikD8UV762quhruT8K8bdjqy6N2o3LG7yohoCdLg1m2HAY1W6rfBrtauHkBhbfA4AQ3iazaJj5wVPhwgaRCHBW2DBg/<0;1>/*").unwrap());
        let heir_key = LianaDescKeys::from_single(descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub661MyMwAqRbcFfxf71L4Dx4w5TmyNXrBicTEAM7vLzumxangwATWWgdJPb6xH1JHcJH9S3jNZx3fCnkkB1WyqrqGgavj1rehHcbythmruvZ/24/32/<0;1>/*").unwrap());
        let timelock = 57600;
        assert_eq!(MultipathDescriptor::new(owner_key.clone(), heir_key, timelock).unwrap().to_string(), "wsh(or_d(pk([aabb0011/10/4893]xpub661MyMwAqRbcFG59fiikD8UV762quhruT8K8bdjqy6N2o3LG7yohoCdLg1m2HAY1W6rfBrtauHkBhbfA4AQ3iazaJj5wVPhwgaRCHBW2DBg/<0;1>/*),and_v(v:pkh([abcdef01]xpub661MyMwAqRbcFfxf71L4Dx4w5TmyNXrBicTEAM7vLzumxangwATWWgdJPb6xH1JHcJH9S3jNZx3fCnkkB1WyqrqGgavj1rehHcbythmruvZ/24/32/<0;1>/*),older(57600))))#ak4cm093");

        // We can't pass a raw key, an xpub that is not deriveable, only hardened derivable,
        // without both the change and receive derivation paths, or with more than 2 different
        // derivation paths.
        let heir_key = LianaDescKeys::from_single(descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub661MyMwAqRbcFfxf71L4Dx4w5TmyNXrBicTEAM7vLzumxangwATWWgdJPb6xH1JHcJH9S3jNZx3fCnkkB1WyqrqGgavj1rehHcbythmruvZ/0/<0;1>/354").unwrap());
        MultipathDescriptor::new(owner_key.clone(), heir_key, timelock).unwrap_err();
        let heir_key = LianaDescKeys::from_single(descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub661MyMwAqRbcFfxf71L4Dx4w5TmyNXrBicTEAM7vLzumxangwATWWgdJPb6xH1JHcJH9S3jNZx3fCnkkB1WyqrqGgavj1rehHcbythmruvZ/0/<0;1>/*'").unwrap());
        MultipathDescriptor::new(owner_key.clone(), heir_key, timelock).unwrap_err();
        let heir_key = LianaDescKeys::from_single(
            descriptor::DescriptorPublicKey::from_str(
                "[abcdef01]02e24913be26dbcfdf8e8e94870b28725cdae09b448b6c127767bf0154e3a3c8e5",
            )
            .unwrap(),
        );
        MultipathDescriptor::new(owner_key.clone(), heir_key, timelock).unwrap_err();
        let heir_key = LianaDescKeys::from_single(descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub661MyMwAqRbcFfxf71L4Dx4w5TmyNXrBicTEAM7vLzumxangwATWWgdJPb6xH1JHcJH9S3jNZx3fCnkkB1WyqrqGgavj1rehHcbythmruvZ/0/*'").unwrap());
        MultipathDescriptor::new(owner_key.clone(), heir_key, timelock).unwrap_err();
        let heir_key = LianaDescKeys::from_single(descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub661MyMwAqRbcFfxf71L4Dx4w5TmyNXrBicTEAM7vLzumxangwATWWgdJPb6xH1JHcJH9S3jNZx3fCnkkB1WyqrqGgavj1rehHcbythmruvZ/<0;1;2>/*'").unwrap());
        MultipathDescriptor::new(owner_key, heir_key, timelock).unwrap_err();

        // And it's checked even in a multisig. For instance:
        let primary_keys = LianaDescKeys::from_multi(
            1,
            vec![
                descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*").unwrap(),
                descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub661MyMwAqRbcFfxf71L4Dx4w5TmyNXrBicTEAM7vLzumxangwATWWgdJPb6xH1JHcJH9S3jNZx3fCnkkB1WyqrqGgavj1rehHcbythmruvZ/0/<0;1>/354").unwrap(),
            ]
        )
        .unwrap();
        let recovery_keys = LianaDescKeys::from_multi(
            1,
            vec![
                descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub69cP4Y7S9TWcbSNxmk6CEDBsoaqr3ZEdjHuZcHxEFFKGh569RsJNr2V27XGhsbH9FXgWUEmKXRN7c5wQfq2VPjt31xP9VsYnVUyU8HcVevm/<0;1>/*").unwrap(),
                descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub6AA2N8RALRYgLD6jT1iXYCEDkndTeZndMtWPbtNX6sY5dPiLtf2T88ahdxrGXMUPoNadgR86sFhBXWQVgifPzDYbY9ZtwK4gqzx4y5Da1DW/<0;1>/*").unwrap(),
            ],
        )
        .unwrap();
        MultipathDescriptor::new(primary_keys, recovery_keys, 26352).unwrap_err();

        // You can't pass duplicate keys, even if they are encoded differently.
        let owner_key = LianaDescKeys::from_single(descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*").unwrap());
        let heir_key = LianaDescKeys::from_single(descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*").unwrap());
        MultipathDescriptor::new(owner_key, heir_key, timelock).unwrap_err();
        let owner_key = LianaDescKeys::from_single(descriptor::DescriptorPublicKey::from_str("[00aabb44]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*").unwrap());
        let heir_key = LianaDescKeys::from_single(descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*").unwrap());
        MultipathDescriptor::new(owner_key, heir_key, timelock).unwrap_err();
        let owner_key = LianaDescKeys::from_single(descriptor::DescriptorPublicKey::from_str("[00aabb44]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*").unwrap());
        let heir_key = LianaDescKeys::from_single(descriptor::DescriptorPublicKey::from_str("[11223344/2/98]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*").unwrap());
        MultipathDescriptor::new(owner_key, heir_key, timelock).unwrap_err();

        // You can't pass duplicate keys, even across multisigs.
        let primary_keys = LianaDescKeys::from_multi(
            3,
            vec![
                descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*").unwrap(),
                descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub6Bw79HbNSeS2xXw1sngPE3ehnk1U3iSPCgLYzC9LpN8m9nDuaKLZvkg8QXxL5pDmEmQtYscmUD8B9MkAAZbh6vxPzNXMaLfGQ9Sb3z85qhR/<0;1>/*").unwrap(),
                descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub67zuTXF9Ln4731avKTBSawoVVNRuMfmRvkL7kLUaLBRqma9ZqdHBJg9qx8cPUm3oNQMiXT4TmGovXNoQPuwg17RFcVJ8YrnbcooN7pxVJqC/<0;1>/*").unwrap()
            ]
        )
        .unwrap();
        let recovery_keys = LianaDescKeys::from_multi(
            2,
            vec![
                descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub69cP4Y7S9TWcbSNxmk6CEDBsoaqr3ZEdjHuZcHxEFFKGh569RsJNr2V27XGhsbH9FXgWUEmKXRN7c5wQfq2VPjt31xP9VsYnVUyU8HcVevm/<0;1>/*").unwrap(),
                descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub6AA2N8RALRYgLD6jT1iXYCEDkndTeZndMtWPbtNX6sY5dPiLtf2T88ahdxrGXMUPoNadgR86sFhBXWQVgifPzDYbY9ZtwK4gqzx4y5Da1DW/<0;1>/*").unwrap(),
                descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub6Bw79HbNSeS2xXw1sngPE3ehnk1U3iSPCgLYzC9LpN8m9nDuaKLZvkg8QXxL5pDmEmQtYscmUD8B9MkAAZbh6vxPzNXMaLfGQ9Sb3z85qhR/<0;1>/*").unwrap(),
            ],
        )
        .unwrap();
        MultipathDescriptor::new(primary_keys, recovery_keys, 26352).unwrap_err();

        // No origin in one of the keys
        let owner_key = LianaDescKeys::from_single(descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*").unwrap());
        let heir_key = LianaDescKeys::from_single(descriptor::DescriptorPublicKey::from_str("xpub688Hn4wScQAAiYJLPg9yH27hUpfZAUnmJejRQBCiwfP5PEDzjWMNW1wChcninxr5gyavFqbbDjdV1aK5USJz8NDVjUy7FRQaaqqXHh5SbXe/<0;1>/*").unwrap());
        let timelock = 52560;
        MultipathDescriptor::new(owner_key, heir_key, timelock).unwrap_err();
    }

    #[test]
    fn inheritance_descriptor_derivation() {
        let secp = secp256k1::Secp256k1::verification_only();
        let desc = MultipathDescriptor::from_str("wsh(andor(pk([abcdef01]tpubDEN9WSToTyy9ZQfaYqSKfmVqmq1VVLNtYfj3Vkqh67et57eJ5sTKZQBkHqSwPUsoSskJeaYnPttHe2VrkCsKA27kUaN9SDc5zhqeLzKa1rr/<0;1>/*),older(10000),pk([abcdef01]tpubD8LYfn6njiA2inCoxwM7EuN3cuLVcaHAwLYeups13dpevd3nHLRdK9NdQksWXrhLQVxcUZRpnp5CkJ1FhE61WRAsHxDNAkvGkoQkAeWDYjV/<0;1>/*)))#2qj59a9y").unwrap();
        let der_desc = desc.receive_descriptor().derive(11.into(), &secp);
        assert_eq!(
            "bc1q26gtczlz03u6juf5cxppapk4sr4fyz53s3g4zs2cgactcahqv6yqc2t8e6",
            der_desc.address(bitcoin::Network::Bitcoin).to_string()
        );

        // Sanity check we can call the methods on the derived desc
        der_desc.script_pubkey();
        der_desc.witness_script();
        assert!(!der_desc.bip32_derivations().is_empty());
    }

    #[test]
    fn inheritance_descriptor_tl_value() {
        let desc = MultipathDescriptor::from_str("wsh(andor(pk([abcdef01]tpubDEN9WSToTyy9ZQfaYqSKfmVqmq1VVLNtYfj3Vkqh67et57eJ5sTKZQBkHqSwPUsoSskJeaYnPttHe2VrkCsKA27kUaN9SDc5zhqeLzKa1rr/<0;1>/*),older(1),pk([abcdef01]tpubD8LYfn6njiA2inCoxwM7EuN3cuLVcaHAwLYeups13dpevd3nHLRdK9NdQksWXrhLQVxcUZRpnp5CkJ1FhE61WRAsHxDNAkvGkoQkAeWDYjV/<0;1>/*)))").unwrap();
        assert_eq!(desc.timelock_value(), 1);

        let desc = MultipathDescriptor::from_str("wsh(andor(pk([abcdef01]tpubDEN9WSToTyy9ZQfaYqSKfmVqmq1VVLNtYfj3Vkqh67et57eJ5sTKZQBkHqSwPUsoSskJeaYnPttHe2VrkCsKA27kUaN9SDc5zhqeLzKa1rr/<0;1>/*),older(42000),pk([abcdef01]tpubD8LYfn6njiA2inCoxwM7EuN3cuLVcaHAwLYeups13dpevd3nHLRdK9NdQksWXrhLQVxcUZRpnp5CkJ1FhE61WRAsHxDNAkvGkoQkAeWDYjV/<0;1>/*)))").unwrap();
        assert_eq!(desc.timelock_value(), 42000);

        let desc = MultipathDescriptor::from_str("wsh(andor(pk([abcdef01]tpubDEN9WSToTyy9ZQfaYqSKfmVqmq1VVLNtYfj3Vkqh67et57eJ5sTKZQBkHqSwPUsoSskJeaYnPttHe2VrkCsKA27kUaN9SDc5zhqeLzKa1rr/<0;1>/*),older(65535),pk([abcdef01]tpubD8LYfn6njiA2inCoxwM7EuN3cuLVcaHAwLYeups13dpevd3nHLRdK9NdQksWXrhLQVxcUZRpnp5CkJ1FhE61WRAsHxDNAkvGkoQkAeWDYjV/<0;1>/*)))").unwrap();
        assert_eq!(desc.timelock_value(), 0xffff);
    }

    #[test]
    fn inheritance_descriptor_sat_size() {
        let desc = MultipathDescriptor::from_str("wsh(or_d(pk([92162c45]tpubD6NzVbkrYhZ4WzTf9SsD6h7AH7oQEippXK2KP8qvhMMqFoNeN5YFVi7vRyeRSDGtgd2bPyMxUNmHui8t5yCgszxPPxMafu1VVzDpg9aruYW/<0;1>/*),and_v(v:pkh([abcdef01]tpubD6NzVbkrYhZ4Wdgu2yfdmrce5g4fiH1ZLmKhewsnNKupbi4sxjH1ZVAorkBLWSkhsjhg8kiq8C4BrBjMy3SjAKDyDdbuvUa1ToAHbiR98js/<0;1>/*),older(2))))#ravw7jw5").unwrap();
        assert_eq!(desc.max_sat_vbytes(), (1 + 69 + 1 + 34 + 73 + 3) / 4); // See the stack details below.

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
            desc.spender_input_size(),
            32 + 4 + 1 + 4 + wu_to_vb(witness_size),
        );
    }

    #[test]
    fn liana_desc_keys() {
        let desc_key_a = descriptor::DescriptorPublicKey::from_str("[aabbccdd]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*").unwrap();
        let desc_key_b = descriptor::DescriptorPublicKey::from_str("[aabbccdd]xpub688Hn4wScQAAiYJLPg9yH27hUpfZAUnmJejRQBCiwfP5PEDzjWMNW1wChcninxr5gyavFqbbDjdV1aK5USJz8NDVjUy7FRQaaqqXHh5SbXe/<0;1>/*").unwrap();
        LianaDescKeys::from_single(desc_key_a.clone());

        LianaDescKeys::from_multi(1, vec![desc_key_a.clone()]).unwrap_err();
        LianaDescKeys::from_multi(2, vec![desc_key_a.clone()]).unwrap_err();
        LianaDescKeys::from_multi(1, vec![desc_key_a.clone(), desc_key_b.clone()]).unwrap();
        LianaDescKeys::from_multi(0, vec![desc_key_a.clone(), desc_key_b.clone()]).unwrap_err();
        LianaDescKeys::from_multi(2, vec![desc_key_a.clone(), desc_key_b.clone()]).unwrap();
        LianaDescKeys::from_multi(3, vec![desc_key_a.clone(), desc_key_b]).unwrap_err();
        LianaDescKeys::from_multi(3, (0..20).map(|_| desc_key_a.clone()).collect()).unwrap();
        LianaDescKeys::from_multi(20, (0..20).map(|_| desc_key_a.clone()).collect()).unwrap();
        LianaDescKeys::from_multi(20, (0..21).map(|_| desc_key_a.clone()).collect()).unwrap_err();
    }

    fn roundtrip(desc_str: &str) {
        let desc = MultipathDescriptor::from_str(desc_str).unwrap();
        assert_eq!(desc.to_string(), desc_str);
    }

    #[test]
    fn roundtrip_descriptor() {
        // A descriptor with single keys in both primary and recovery paths
        roundtrip("wsh(or_d(pk([aabbccdd]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*),and_v(v:pkh([aabbccdd]xpub688Hn4wScQAAiYJLPg9yH27hUpfZAUnmJejRQBCiwfP5PEDzjWMNW1wChcninxr5gyavFqbbDjdV1aK5USJz8NDVjUy7FRQaaqqXHh5SbXe/<0;1>/*),older(52560))))#7437yjrs");
        // One with a multisig in both paths
        roundtrip("wsh(or_d(multi(3,[aabbccdd]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*,[aabb0011/10/4893]xpub6Bw79HbNSeS2xXw1sngPE3ehnk1U3iSPCgLYzC9LpN8m9nDuaKLZvkg8QXxL5pDmEmQtYscmUD8B9MkAAZbh6vxPzNXMaLfGQ9Sb3z85qhR/<0;1>/*,[aabbccdd]xpub67zuTXF9Ln4731avKTBSawoVVNRuMfmRvkL7kLUaLBRqma9ZqdHBJg9qx8cPUm3oNQMiXT4TmGovXNoQPuwg17RFcVJ8YrnbcooN7pxVJqC/<0;1>/*),and_v(v:multi(2,[aabbccdd]xpub69cP4Y7S9TWcbSNxmk6CEDBsoaqr3ZEdjHuZcHxEFFKGh569RsJNr2V27XGhsbH9FXgWUEmKXRN7c5wQfq2VPjt31xP9VsYnVUyU8HcVevm/<0;1>/*,[aabbccdd]xpub6AA2N8RALRYgLD6jT1iXYCEDkndTeZndMtWPbtNX6sY5dPiLtf2T88ahdxrGXMUPoNadgR86sFhBXWQVgifPzDYbY9ZtwK4gqzx4y5Da1DW/<0;1>/*,[aabb0011/10/4893]xpub6AyxexvxizZJffF153evmfqHcE9MV88fCNCAtP3jQjXJHwrAKri71Tq9jWUkPxj9pja4u6AkCPHY7atgxzSEa2HtDwJfrRWKK4fsfQg4o77/<0;1>/*),older(26352))))#ypwt7h7e");
        // A single key as primary path, a multisig as recovery
        roundtrip("wsh(or_d(pk([aabbccdd]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*),and_v(v:multi(2,[aabbccdd]xpub69cP4Y7S9TWcbSNxmk6CEDBsoaqr3ZEdjHuZcHxEFFKGh569RsJNr2V27XGhsbH9FXgWUEmKXRN7c5wQfq2VPjt31xP9VsYnVUyU8HcVevm/<0;1>/*,[aabbccdd]xpub6AA2N8RALRYgLD6jT1iXYCEDkndTeZndMtWPbtNX6sY5dPiLtf2T88ahdxrGXMUPoNadgR86sFhBXWQVgifPzDYbY9ZtwK4gqzx4y5Da1DW/<0;1>/*,[aabb0011/10/4893]xpub6AyxexvxizZJffF153evmfqHcE9MV88fCNCAtP3jQjXJHwrAKri71Tq9jWUkPxj9pja4u6AkCPHY7atgxzSEa2HtDwJfrRWKK4fsfQg4o77/<0;1>/*),older(26352))))#7du8x4v7");
        // The other way around
        roundtrip("wsh(or_d(multi(3,[aabbccdd]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*,[aabb0011/10/4893]xpub6Bw79HbNSeS2xXw1sngPE3ehnk1U3iSPCgLYzC9LpN8m9nDuaKLZvkg8QXxL5pDmEmQtYscmUD8B9MkAAZbh6vxPzNXMaLfGQ9Sb3z85qhR/<0;1>/*,[aabbccdd]xpub67zuTXF9Ln4731avKTBSawoVVNRuMfmRvkL7kLUaLBRqma9ZqdHBJg9qx8cPUm3oNQMiXT4TmGovXNoQPuwg17RFcVJ8YrnbcooN7pxVJqC/<0;1>/*),and_v(v:pk([aabbccdd]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*),older(26352))))#y9l4ldvr");
    }

    fn psbt_from_str(psbt_str: &str) -> Psbt {
        bitcoin::consensus::deserialize(&base64::decode(psbt_str).unwrap()).unwrap()
    }

    #[test]
    fn partial_spend_info() {
        // A simple descriptor with 1 keys as primary path and 1 recovery key.
        let desc = MultipathDescriptor::from_str("wsh(or_d(pk([f5acc2fd]tpubD6NzVbkrYhZ4YgUx2ZLNt2rLYAMTdYysCRzKoLu2BeSHKvzqPaBDvf17GeBPnExUVPkuBpx4kniP964e2MxyzzazcXLptxLXModSVCVEV1T/<0;1>/*),and_v(v:pkh([8a64f2a9]tpubD6NzVbkrYhZ4WmzFjvQrp7sDa4ECUxTi9oby8K4FZkd3XCBtEdKwUiQyYJaxiJo5y42gyDWEczrFpozEjeLxMPxjf2WtkfcbpUdfvNnozWF/<0;1>/*),older(10))))#d72le4dr").unwrap();
        let desc_info = desc.info();
        let prim_key_fg = bip32::Fingerprint::from_str("f5acc2fd").unwrap();
        let recov_key_fg = bip32::Fingerprint::from_str("8a64f2a9").unwrap();

        // A PSBT with a single input and output, no signature. nSequence is not set to use the
        // recovery path.
        let mut unsigned_single_psbt: Psbt = psbt_from_str("cHNidP8BAHECAAAAAUSHuliRtuCX1S6JxRuDRqDCKkWfKmWL5sV9ukZ/wzvfAAAAAAD9////AogTAAAAAAAAFgAUIxe7UY6LJ6y5mFBoWTOoVispDmdwFwAAAAAAABYAFKqO83TK+t/KdpAt21z2HGC7/Z2FAAAAAAABASsQJwAAAAAAACIAIIIySQjGCTeyx/rKUQx8qobjhJeNCiVCliBJPdyRX6XKAQVBIQI2cqWpc9UAW2gZt2WkKjvi8KoMCui00pRlL6wG32uKDKxzZHapFNYASzIYkEdH9bJz6nnqUG3uBB8kiK1asmgiBgI2cqWpc9UAW2gZt2WkKjvi8KoMCui00pRlL6wG32uKDAz1rML9AAAAAG8AAAAiBgMLcbOxsfLe6+3r1UcjQo77HY0As8OKE4l37yj0/qhIyQyKZPKpAAAAAG8AAAAAAAA=");
        let info = desc.partial_spend_info(&unsigned_single_psbt).unwrap();
        assert_eq!(info.primary_path.threshold, 1);
        assert_eq!(info.primary_path.sigs_count, 0);
        assert!(info.primary_path.signed_pubkeys.is_empty());
        assert!(info.recovery_path.is_none());

        // If we set the sequence too low we still won't have the recovery path info.
        unsigned_single_psbt.unsigned_tx.input[0].sequence =
            Sequence::from_height(desc_info.recovery_path.0 - 1);
        let info = desc.partial_spend_info(&unsigned_single_psbt).unwrap();
        assert!(info.recovery_path.is_none());

        // Now if we set the sequence at the right value we'll have it.
        unsigned_single_psbt.unsigned_tx.input[0].sequence =
            Sequence::from_height(desc_info.recovery_path.0);
        let info = desc.partial_spend_info(&unsigned_single_psbt).unwrap();
        assert!(info.recovery_path.is_some());

        // Even if it's a bit too high (as long as it's still a block height and activated)
        unsigned_single_psbt.unsigned_tx.input[0].sequence =
            Sequence::from_height(desc_info.recovery_path.0 + 42);
        let info = desc.partial_spend_info(&unsigned_single_psbt).unwrap();
        let recov_info = info.recovery_path.unwrap();
        assert_eq!(recov_info.threshold, 1);
        assert_eq!(recov_info.sigs_count, 0);
        assert!(recov_info.signed_pubkeys.is_empty());

        // The same PSBT but with an (invalid) signature for the primary key.
        let mut signed_single_psbt = psbt_from_str("cHNidP8BAHECAAAAAUSHuliRtuCX1S6JxRuDRqDCKkWfKmWL5sV9ukZ/wzvfAAAAAAD9////AogTAAAAAAAAFgAUIxe7UY6LJ6y5mFBoWTOoVispDmdwFwAAAAAAABYAFKqO83TK+t/KdpAt21z2HGC7/Z2FAAAAAAABASsQJwAAAAAAACIAIIIySQjGCTeyx/rKUQx8qobjhJeNCiVCliBJPdyRX6XKIgICNnKlqXPVAFtoGbdlpCo74vCqDArotNKUZS+sBt9rigxIMEUCIQCYZusUL8bdi2PnjWao4bIDDgMQ9Dj2Lcup3/VmkGbYJAIgX/wF5HsqugC5JzvU2cGOmUWtHr2Pg0N4912qogYgDH4BAQVBIQI2cqWpc9UAW2gZt2WkKjvi8KoMCui00pRlL6wG32uKDKxzZHapFNYASzIYkEdH9bJz6nnqUG3uBB8kiK1asmgiBgI2cqWpc9UAW2gZt2WkKjvi8KoMCui00pRlL6wG32uKDAz1rML9AAAAAG8AAAAiBgMLcbOxsfLe6+3r1UcjQo77HY0As8OKE4l37yj0/qhIyQyKZPKpAAAAAG8AAAAAAAA=");
        let info = desc.partial_spend_info(&signed_single_psbt).unwrap();
        assert_eq!(signed_single_psbt.inputs[0].partial_sigs.len(), 1);
        assert_eq!(info.primary_path.threshold, 1);
        assert_eq!(info.primary_path.sigs_count, 1);
        assert!(
            info.primary_path.signed_pubkeys.len() == 1
                && info.primary_path.signed_pubkeys.contains_key(&prim_key_fg)
        );
        assert!(info.recovery_path.is_none());

        // Now enable the recovery path and add a signature for the recovery key.
        signed_single_psbt.unsigned_tx.input[0].sequence =
            Sequence::from_height(desc_info.recovery_path.0);
        let recov_pubkey = bitcoin::PublicKey {
            compressed: true,
            inner: *signed_single_psbt.inputs[0]
                .bip32_derivation
                .iter()
                .find(|(_, (fg, _))| fg == &recov_key_fg)
                .unwrap()
                .0,
        };
        let prim_key = *signed_single_psbt.inputs[0]
            .partial_sigs
            .iter()
            .next()
            .unwrap()
            .0;
        let sig = signed_single_psbt.inputs[0]
            .partial_sigs
            .remove(&prim_key)
            .unwrap();
        signed_single_psbt.inputs[0]
            .partial_sigs
            .insert(recov_pubkey, sig);
        let info = desc.partial_spend_info(&signed_single_psbt).unwrap();
        assert_eq!(signed_single_psbt.inputs[0].partial_sigs.len(), 1);
        assert_eq!(info.primary_path.threshold, 1);
        assert_eq!(info.primary_path.sigs_count, 0);
        assert!(info.primary_path.signed_pubkeys.is_empty());
        let recov_info = info.recovery_path.unwrap();
        assert_eq!(recov_info.threshold, 1);
        assert_eq!(recov_info.sigs_count, 1);
        assert!(
            recov_info.signed_pubkeys.len() == 1
                && recov_info.signed_pubkeys.contains_key(&recov_key_fg)
        );

        // A PSBT with multiple inputs, all signed for the primary path.
        let psbt: Psbt = psbt_from_str("cHNidP8BAP0fAQIAAAAGAGo6V8K5MtKcQ8vRFedf5oJiOREiH4JJcEniyRv2800BAAAAAP3///9e3dVLjWKPAGwDeuUOmKFzOYEP5Ipu4LWdOPA+lITrRgAAAAAA/f///7cl9oeu9ssBXKnkWMCUnlgZPXhb+qQO2+OPeLEsbdGkAQAAAAD9////idkxRErbs34vsHUZ7QCYaiVaAFDV9gxNvvtwQLozwHsAAAAAAP3///9EakyJhd2PjwYh1I7zT2cmcTFI5g1nBd3srLeL7wKEewIAAAAA/f///7BcaP77nMaA2NjT/hyI6zueB/2jU/jK4oxmSqMaFkAzAQAAAAD9////AUAfAAAAAAAAFgAUqo7zdMr638p2kC3bXPYcYLv9nYUAAAAAAAEA/X4BAgAAAAABApEoe5xCmSi8hNTtIFwsy46aj3hlcLrtFrug39v5wy+EAQAAAGpHMEQCIDeI8JTWCTyX6opCCJBhWc4FytH8g6fxDaH+Wa/QqUoMAiAgbITpz8TBhwxhv/W4xEXzehZpOjOTjKnPw36GIy6SHAEhA6QnYCHUbU045FVh6ZwRwYTVineqRrB9tbqagxjaaBKh/v///+v1seDE9gGsZiWwewQs3TKuh0KSBIHiEtG8ABbz2DpAAQAAAAD+////Aqhaex4AAAAAFgAUkcVOEjVMct0jyCzhZN6zBT+lvTQvIAAAAAAAACIAIKKDUd/GWjAnwU99llS9TAK2dK80/nSRNLjmrhj0odUEAAJHMEQCICSn+boh4ItAa3/b4gRUpdfblKdcWtMLKZrgSEFFrC+zAiBtXCx/Dq0NutLSu1qmzFF1lpwSCB3w3MAxp5W90z7b/QEhA51S2ERUi0bg+l+bnJMJeAfDknaetMTagfQR9+AOrVKlxdMkAAEBKy8gAAAAAAAAIgAgooNR38ZaMCfBT32WVL1MArZ0rzT+dJE0uOauGPSh1QQiAgN+zbSfdr8oJBtlKomnQTHynF2b/UhovAwf0eS8awRSqUgwRQIhAJhm6xQvxt2LY+eNZqjhsgMOAxD0OPYty6nf9WaQZtgkAiBf/AXkeyq6ALknO9TZwY6ZRa0evY+DQ3j3XaqiBiAMfgEBBUEhA37NtJ92vygkG2UqiadBMfKcXZv9SGi8DB/R5LxrBFKprHNkdqkUxttmGj2sqzzaxSaacJTnJPDCbY6IrVqyaCIGAv9qeBDEB+5kvM/sZ8jQ7QApfZcDrqtq5OAe2gQ1V+pmDIpk8qkAAAAA0AAAACIGA37NtJ92vygkG2UqiadBMfKcXZv9SGi8DB/R5LxrBFKpDPWswv0AAAAA0AAAAAABAOoCAAAAAAEB0OPoVJs9ihvnAwjO16k/wGJuEus1IEE1Yo2KBjC2NSEAAAAAAP7///8C6AMAAAAAAAAiACBfeUS9jQv6O1a96Aw/mPV6gHxHl3mfj+f0frfAs2sMpP1QGgAAAAAAFgAUDS4UAIpdm1RlFYmg0OoCxW0yBT4CRzBEAiAPvbNlnhiUxLNshxN83AuK/lGWwlpXOvmcqoxsMLzIKwIgWwATJuYPf9buLe9z5SnXVnPVL0q6UZaWE5mjCvEl1RUBIQI54LFZmq9Lw0pxKpEGeqI74NnIfQmLMDcv5ySplUS1/wDMJAABASvoAwAAAAAAACIAIF95RL2NC/o7Vr3oDD+Y9XqAfEeXeZ+P5/R+t8CzawykIgICYn4eZbb6KGoxB1PEv/XPiujZFDhfoi/rJPtfHPVML2lHMEQCIDOHEqKdBozXIPLVgtBj3eWC1MeIxcKYDADe4zw0DbcMAiAq4+dbkTNCAjyCxJi0TKz5DWrPulxrqOdjMRHWngXHsQEBBUEhAmJ+HmW2+ihqMQdTxL/1z4ro2RQ4X6Iv6yT7Xxz1TC9prHNkdqkUzc/gCLoe6rQw63CGXhIR3YRz1qCIrVqyaCIGAmJ+HmW2+ihqMQdTxL/1z4ro2RQ4X6Iv6yT7Xxz1TC9pDPWswv0AAAAAqgAAACIGA8JCTIzdSoTJhiKN1pn+NnlkyuKOndiTgH2NIX+yNsYqDIpk8qkAAAAAqgAAAAABAOoCAAAAAAEBRGpMiYXdj48GIdSO809nJnExSOYNZwXd7Ky3i+8ChHsAAAAAAP7///8COMMQAAAAAAAWABQ5rnyuG5T8iuhqfaGAmpzlybo3t+gDAAAAAAAAIgAg7Kz3CX1RBjIvbK9LBYztmi7F1XIxQpX6mtCUkflvvl8CRzBEAiBaYx4sOHckEZwDnSrbb1ivc6seX4Puasm1PBGnBWgSTQIgCeUiXvd90ajI3F4/BHifLUI4fVIgVQFCqLTbbeXQD5oBIQOmGm+gTRx1slzF+wn8NhZoR1xfSYgoKX6bpRSVRjLcEXrOJAABASvoAwAAAAAAACIAIOys9wl9UQYyL2yvSwWM7ZouxdVyMUKV+prQlJH5b75fIgID0X2UJhC5+2jgJqUrihxZxDZHK7jgPFlrUYzoSHQTmP9HMEQCIEM4K8lVACvE2oSMZHDJiOeD81qsYgAvgpRgcSYgKc3AAiAQjdDr2COBea69W+2iVbnODuH3QwacgShW3dS4yeggJAEBBUEhA9F9lCYQufto4CalK4ocWcQ2Ryu44DxZa1GM6Eh0E5j/rHNkdqkU0DTexcgOQQ+BFjgS031OTxcWiH2IrVqyaCIGA9F9lCYQufto4CalK4ocWcQ2Ryu44DxZa1GM6Eh0E5j/DPWswv0AAAAAvwAAACIGA/xg4Uvem3JHVPpyTLP5JWiUH/yk3Y/uUI6JkZasCmHhDIpk8qkAAAAAvwAAAAABAOoCAAAAAAEBmG+mPq0O6QSWEMctsMjvv5LzWHGoT8wsA9Oa05kxIxsBAAAAAP7///8C6AMAAAAAAAAiACDUvIILFr0OxybADV3fB7ms7+ufnFZgicHR0nbI+LFCw1UoGwAAAAAAFgAUC+1ZjCC1lmMcvJ/4JkevqoZF4igCRzBEAiA3d8o96CNgNWHUkaINWHTvAUinjUINvXq0KBeWcsSWuwIgKfzRNWFR2LDbnB/fMBsBY/ylVXcSYwLs8YC+kmko1zIBIQOpEfsLv0htuertA1sgzCwGvHB0vE4zFO69wWEoHClKmAfMJAABASvoAwAAAAAAACIAINS8ggsWvQ7HJsANXd8Huazv65+cVmCJwdHSdsj4sULDIgID96jZc0sCi0IIXf2CpfE7tY+9LRmMsOdSTTHelFxfCwJHMEQCIHlaiMMznx8Cag8Y3X2gXi9Qtg0ZuyHEC6DsOzipSGOKAiAV2eC+S3Mbq6ig5QtRvTBsq5M3hCBdEJQlOrLVhWWt6AEBBUEhA/eo2XNLAotCCF39gqXxO7WPvS0ZjLDnUk0x3pRcXwsCrHNkdqkUyJ+Cbx7vYVY665yjJnMNODyYrAuIrVqyaCIGAt8UyDXk+mW3Y6IZNIBuDJHkdOaZi/UEShkN5L3GiHR5DIpk8qkAAAAAuAAAACIGA/eo2XNLAotCCF39gqXxO7WPvS0ZjLDnUk0x3pRcXwsCDPWswv0AAAAAuAAAAAABAP0JAQIAAAAAAQG7Zoy4I3J9x+OybAlIhxVKcYRuPFrkDFJfxMiC3kIqIAEAAAAA/v///wO5xxAAAAAAABYAFHgBzs9wJNVk6YwR81IMKmckTmC56AMAAAAAAAAWABTQ/LmJix5JoHBOr8LcgEChXHdLROgDAAAAAAAAIgAg7Kz3CX1RBjIvbK9LBYztmi7F1XIxQpX6mtCUkflvvl8CRzBEAiA+sIKnWVE3SmngjUgJdu1K2teW6eqeolfGe0d11b+irAIgL20zSabXaFRNM8dqVlcFsfNJ0exukzvxEOKl/OcF8VsBIQJrUspHq45AMSwbm24//2a9JM8XHFWbOKpyV+gNCtW71nrOJAABASvoAwAAAAAAACIAIOys9wl9UQYyL2yvSwWM7ZouxdVyMUKV+prQlJH5b75fIgID0X2UJhC5+2jgJqUrihxZxDZHK7jgPFlrUYzoSHQTmP9IMEUCIQCmDhJ9fyhlQwPruoOUemDuldtRu3ZkiTM3DA0OhkguSQIgYerNaYdP43DcqI5tnnL3n4jEeMHFCs+TBkOd6hDnqAkBAQVBIQPRfZQmELn7aOAmpSuKHFnENkcruOA8WWtRjOhIdBOY/6xzZHapFNA03sXIDkEPgRY4EtN9Tk8XFoh9iK1asmgiBgPRfZQmELn7aOAmpSuKHFnENkcruOA8WWtRjOhIdBOY/wz1rML9AAAAAL8AAAAiBgP8YOFL3ptyR1T6ckyz+SVolB/8pN2P7lCOiZGWrAph4QyKZPKpAAAAAL8AAAAAAQDqAgAAAAABAT6/vc6qBRzhQyjVtkC25NS2BvGyl2XjjEsw3e8vAesjAAAAAAD+////AgPBAO4HAAAAFgAUEwiWd/qI1ergMUw0F1+qLys5G/foAwAAAAAAACIAIOOPEiwmp2ZXR7ciyrveITXw0tn6zbQUA1Eikd9QlHRhAkcwRAIgJMZdO5A5u2UIMrAOgrR4NcxfNgZI6OfY7GKlZP0O8yUCIDFujbBRnamLEbf0887qidnXo6UgQA9IwTx6Zomd4RvJASEDoNmR2/XcqSyCWrE1tjGJ1oLWlKt4zsFekK9oyB4Hl0HF0yQAAQEr6AMAAAAAAAAiACDjjxIsJqdmV0e3Isq73iE18NLZ+s20FANRIpHfUJR0YSICAo3uyJxKHR9Z8fwvU7cywQCnZyPvtMl3nv54wPW1GSGqSDBFAiEAlLY98zqEL/xTUvm9ZKy5kBa4UWfr4Ryu6BmSZjseXPQCIGy7efKbZLQSDq8RhgNNjl1384gWFTN7nPwWV//SGriyAQEFQSECje7InEodH1nx/C9TtzLBAKdnI++0yXee/njA9bUZIaqsc2R2qRQhPRlaLsh/M/K/9fvbjxF/M20cNoitWrJoIgYCF7Rj5jFhe5L6VDzP5m2BeaG0mA9e7+6fMeWkWxLwpbAMimTyqQAAAADNAAAAIgYCje7InEodH1nx/C9TtzLBAKdnI++0yXee/njA9bUZIaoM9azC/QAAAADNAAAAAAA=");
        let info = desc.partial_spend_info(&psbt).unwrap();
        assert!(psbt
            .inputs
            .iter()
            .all(|psbt_in| psbt_in.partial_sigs.len() == 1));
        assert_eq!(info.primary_path.threshold, 1);
        assert_eq!(info.primary_path.sigs_count, 1);
        assert!(
            info.primary_path.signed_pubkeys.len() == 1
                && info.primary_path.signed_pubkeys.contains_key(&prim_key_fg)
        );
        assert!(info.recovery_path.is_none());

        // Enable the recovery path, it should show no recovery sig.
        let mut rec_psbt = psbt.clone();
        for txin in rec_psbt.unsigned_tx.input.iter_mut() {
            txin.sequence = Sequence::from_height(desc_info.recovery_path.0);
        }
        let info = desc.partial_spend_info(&rec_psbt).unwrap();
        assert!(rec_psbt
            .inputs
            .iter()
            .all(|psbt_in| psbt_in.partial_sigs.len() == 1));
        assert_eq!(info.primary_path.threshold, 1);
        assert_eq!(info.primary_path.sigs_count, 1);
        assert!(
            info.primary_path.signed_pubkeys.len() == 1
                && info.primary_path.signed_pubkeys.contains_key(&prim_key_fg)
        );
        let recov_info = info.recovery_path.unwrap();
        assert_eq!(recov_info.threshold, 1);
        assert_eq!(recov_info.sigs_count, 0);
        assert!(recov_info.signed_pubkeys.is_empty());

        // If the sequence of one of the input is different from the other ones, it'll return
        // an error since the analysis is on the whole transaction.
        let mut inconsistent_psbt = psbt.clone();
        inconsistent_psbt.unsigned_tx.input[0].sequence =
            Sequence::from_height(desc_info.recovery_path.0 + 1);
        assert!(desc
            .partial_spend_info(&inconsistent_psbt)
            .unwrap_err()
            .to_string()
            .contains("Analyzed PSBT is inconsistent across inputs."));

        // Same if all inputs don't have the same number of signatures.
        let mut inconsistent_psbt = psbt.clone();
        inconsistent_psbt.inputs[0].partial_sigs.clear();
        assert!(desc
            .partial_spend_info(&inconsistent_psbt)
            .unwrap_err()
            .to_string()
            .contains("Analyzed PSBT is inconsistent across inputs."));

        // If we analyze a descriptor with a multisig we'll get the right threshold.
        let desc = MultipathDescriptor::from_str("wsh(or_d(multi(2,[f5acc2fd]tpubD6NzVbkrYhZ4YgUx2ZLNt2rLYAMTdYysCRzKoLu2BeSHKvzqPaBDvf17GeBPnExUVPkuBpx4kniP964e2MxyzzazcXLptxLXModSVCVEV1T/<0;1>/*,[00112233]xpub6FC8vmQGGfSuQGfKG5L73fZ7WjXit8TzfJYDKwTtHkhrbAhU5Kma41oenVq6aMnpgULJRXpQuxnVysyfdpRhVgD6vYe7XLbFDhmvYmDrAVq/<0;1>/*,[aabbccdd]xpub68XtbpvDM19d39wEKdvadHkZ4FGKf4tnryKzAacttp8BLX3uHj7eK8shRnFBhZ2UL83S9dwXe42Qm6eG6BkR1jy8XwUSNBcHKtET7j4V5FB/<0;1>/*),and_v(v:pkh([8a64f2a9]tpubD6NzVbkrYhZ4WmzFjvQrp7sDa4ECUxTi9oby8K4FZkd3XCBtEdKwUiQyYJaxiJo5y42gyDWEczrFpozEjeLxMPxjf2WtkfcbpUdfvNnozWF/<0;1>/*),older(10))))#2kgxuax5").unwrap();
        let info = desc.partial_spend_info(&psbt).unwrap();
        assert!(psbt
            .inputs
            .iter()
            .all(|psbt_in| psbt_in.partial_sigs.len() == 1));
        assert_eq!(info.primary_path.threshold, 2);
        assert_eq!(info.primary_path.sigs_count, 1);
        assert!(
            info.primary_path.signed_pubkeys.len() == 1
                && info.primary_path.signed_pubkeys.contains_key(&prim_key_fg)
        );
        assert!(info.recovery_path.is_none());

        let desc = MultipathDescriptor::from_str("wsh(or_d(multi(2,[636adf3f/48'/1'/0'/2']tpubDEE9FvWbG4kg4gxDNrALgrWLiHwNMXNs8hk6nXNPw4VHKot16xd2251vwi2M6nsyQTkak5FJNHVHkCcuzmvpSbWHdumX3DxpDm89iTfSBaL/<0;1>/*,[ffd63c8d/48'/1'/0'/2']tpubDExA3EC3iAsPxPhFn4j6gMiVup6V2eH3qKyk69RcTc9TTNRfFYVPad8bJD5FCHVQxyBT4izKsvr7Btd2R4xmQ1hZkvsqGBaeE82J71uTK4N/<0;1>/*),and_v(v:multi(2,[636adf3f/48'/1'/1'/2']tpubDDvF2khuoBBj8vcSjQfa7iKaxsQZE7YjJ7cJL8A8eaneadMPKbHSpoSr4JD1F5LUvWD82HCxdtSppGfrMUmiNbFxrA2EHEVLnrdCFNFe75D/<0;1>/*,[ffd63c8d/48'/1'/1'/2']tpubDFMs44FD4kFt3M7Z317cFh5tdKEGN8tyQRY6Q5gcSha4NtxZfGmTVRMbsD1bWN469LstXU4aVSARDxrvxFCUjHeegfEY2cLSazMBkNCmDPD/<0;1>/*),older(2))))#xcf6jr2r").unwrap();
        let info = desc.info();
        assert_eq!(info.primary_path, PathInfo::Multi(
            2,
            vec![
                descriptor::DescriptorPublicKey::from_str("[636adf3f/48'/1'/0'/2']tpubDEE9FvWbG4kg4gxDNrALgrWLiHwNMXNs8hk6nXNPw4VHKot16xd2251vwi2M6nsyQTkak5FJNHVHkCcuzmvpSbWHdumX3DxpDm89iTfSBaL/<0;1>/*").unwrap(),
                descriptor::DescriptorPublicKey::from_str("[ffd63c8d/48'/1'/0'/2']tpubDExA3EC3iAsPxPhFn4j6gMiVup6V2eH3qKyk69RcTc9TTNRfFYVPad8bJD5FCHVQxyBT4izKsvr7Btd2R4xmQ1hZkvsqGBaeE82J71uTK4N/<0;1>/*").unwrap(),
            ],
        ));
        assert_eq!(info.recovery_path, (2, PathInfo::Multi(
            2,
            vec![
                descriptor::DescriptorPublicKey::from_str("[636adf3f/48'/1'/1'/2']tpubDDvF2khuoBBj8vcSjQfa7iKaxsQZE7YjJ7cJL8A8eaneadMPKbHSpoSr4JD1F5LUvWD82HCxdtSppGfrMUmiNbFxrA2EHEVLnrdCFNFe75D/<0;1>/*").unwrap(),
                descriptor::DescriptorPublicKey::from_str("[ffd63c8d/48'/1'/1'/2']tpubDFMs44FD4kFt3M7Z317cFh5tdKEGN8tyQRY6Q5gcSha4NtxZfGmTVRMbsD1bWN469LstXU4aVSARDxrvxFCUjHeegfEY2cLSazMBkNCmDPD/<0;1>/*").unwrap(),
            ],
        )));
        // TODO: fix the partial spend info..
    }

    // TODO: test error conditions of deserialization.
}
