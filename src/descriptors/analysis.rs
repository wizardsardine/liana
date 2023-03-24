use miniscript::{
    bitcoin::{util::bip32, Sequence},
    descriptor,
    policy::{compiler, Concrete as ConcretePolicy, Liftable, Semantic as SemanticPolicy},
    ScriptContext,
};

use std::{
    collections::{HashMap, HashSet},
    convert::TryFrom,
    error, fmt,
};

#[derive(Debug)]
pub enum LianaPolicyError {
    InsaneTimelock(u32),
    InvalidKey(Box<descriptor::DescriptorPublicKey>),
    DuplicateKey(Box<descriptor::DescriptorPublicKey>),
    InvalidMultiThresh(usize),
    InvalidMultiKeys(usize),
    IncompatibleDesc,
    /// The spending policy is not a valid Miniscript policy: it may for instance be malleable, or
    /// overflow some limit.
    InvalidPolicy(compiler::CompilerError),
}

impl std::fmt::Display for LianaPolicyError {
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
            Self::InvalidMultiThresh(thresh) => write!(f, "Invalid multisig threshold value '{}'. The threshold must be > to 0 and <= to the number of keys.", thresh),
            Self::InvalidMultiKeys(n_keys) => write!(f, "Invalid number of keys '{}'. Between 2 and 20 keys must be given to use multiple keys in a specific path.", n_keys),
            Self::DuplicateKey(key) => {
                write!(f, "Duplicate key '{}'.", key)
            }
            Self::IncompatibleDesc => write!(
                f,
                "Descriptor is not compatible with a Liana spending policy."
            ),
            Self::InvalidPolicy(e) => write!(f, "Invalid Miniscript policy: {}", e),
        }
    }
}

impl error::Error for LianaPolicyError {}

// Whether a Miniscript policy node represents a key check (or several of them).
fn is_single_key_or_multisig(policy: &SemanticPolicy<descriptor::DescriptorPublicKey>) -> bool {
    match policy {
        SemanticPolicy::Key(..) => true,
        SemanticPolicy::Threshold(_, subs) => {
            subs.iter().all(|sub| matches!(sub, SemanticPolicy::Key(_)))
        }
        _ => false,
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

// We require the locktime to:
//  - not be disabled
//  - be in number of blocks
//  - be 'clean' / minimal, ie all bits without consensus meaning should be 0
//
// All this is achieved simply through asking for a 16-bit integer, since all the
// above are signaled in leftmost bits.
fn csv_check(csv_value: u32) -> Result<u16, LianaPolicyError> {
    if csv_value > 0 {
        u16::try_from(csv_value).map_err(|_| LianaPolicyError::InsaneTimelock(csv_value))
    } else {
        Err(LianaPolicyError::InsaneTimelock(csv_value))
    }
}

// Get the origin of a key in a multipath descriptors.
// Returns None if the given key isn't a multixpub.
fn key_origin(
    key: &descriptor::DescriptorPublicKey,
) -> Option<&(bip32::Fingerprint, bip32::DerivationPath)> {
    match key {
        descriptor::DescriptorPublicKey::MultiXPub(ref xpub) => xpub.origin.as_ref(),
        _ => None,
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
    ) -> Result<PathInfo, LianaPolicyError> {
        match policy {
            SemanticPolicy::Key(key) => Ok(PathInfo::Single(key)),
            SemanticPolicy::Threshold(k, subs) => {
                let keys: Result<_, LianaPolicyError> = subs
                    .into_iter()
                    .map(|sub| match sub {
                        SemanticPolicy::Key(key) => Ok(key),
                        _ => Err(LianaPolicyError::IncompatibleDesc),
                    })
                    .collect();
                Ok(PathInfo::Multi(k, keys?))
            }
            _ => Err(LianaPolicyError::IncompatibleDesc),
        }
    }

    /// Get the information about the recovery spending path.
    /// Returns None if the policy does not describe the recovery spending path of a Liana
    /// descriptor (that is, a set of keys after a timelock).
    pub fn from_recovery_path(
        policy: SemanticPolicy<descriptor::DescriptorPublicKey>,
    ) -> Result<(u16, PathInfo), LianaPolicyError> {
        // The recovery spending path must always be a policy of type `thresh(2, older(x), thresh(n, key1,
        // key2, ..))`. In the special case n == 1, it is only `thresh(2, older(x), key)`. In the
        // special case n == len(keys) (i.e. it's an N-of-N multisig), it is normalized as
        // `thresh(n+1, older(x), key1, key2, ...)`.
        let (k, subs) = match policy {
            SemanticPolicy::Threshold(k, subs) => (k, subs),
            _ => return Err(LianaPolicyError::IncompatibleDesc),
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
                .ok_or(LianaPolicyError::IncompatibleDesc)??;
            let keys_sub = subs
                .into_iter()
                .find(is_single_key_or_multisig)
                .ok_or(LianaPolicyError::IncompatibleDesc)?;
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
                            return Err(LianaPolicyError::IncompatibleDesc);
                        }
                        tl_value = Some(csv_check(val.0)?);
                    }
                    _ => return Err(LianaPolicyError::IncompatibleDesc),
                }
            }
            assert!(keys.len() > 1); // At least 3 subs, only one of which may be older().
            Ok((
                tl_value.ok_or(LianaPolicyError::IncompatibleDesc)?,
                PathInfo::Multi(k - 1, keys),
            ))
        } else {
            // If there is less than 2 subs, there can't be both a timelock and keys. If the
            // threshold is not equal to the number of subs, the timelock can't be mandatory.
            Err(LianaPolicyError::IncompatibleDesc)
        }
    }

    /// Add another available key to this `PathInfo`. Note this doesn't change the threshold.
    pub fn with_added_key(mut self, key: descriptor::DescriptorPublicKey) -> Self {
        match self {
            Self::Single(curr_key) => Self::Multi(1, vec![curr_key, key]),
            Self::Multi(_, ref mut keys) => {
                keys.push(key);
                self
            }
        }
    }

    /// Get the required number of keys for spending through this path, and the set of keys
    /// that can be used to provide a signature for this path.
    pub fn thresh_origins(&self) -> (usize, HashSet<(bip32::Fingerprint, bip32::DerivationPath)>) {
        match self {
            PathInfo::Single(key) => {
                let mut origins = HashSet::with_capacity(1);
                origins.insert(
                    key_origin(key)
                        .expect("Must be a multixpub with an origin.")
                        .clone(),
                );
                (1, origins)
            }
            PathInfo::Multi(k, keys) => (
                *k,
                keys.iter()
                    .map(|key| {
                        key_origin(key)
                            .expect("Must be a multixpub with an origin.")
                            .clone()
                    })
                    .collect(),
            ),
        }
    }

    /// Get the spend information for this descriptor based from the list of all pubkeys that
    /// signed the transaction.
    pub fn spend_info<'a>(
        &self,
        all_pubkeys_signed: impl Iterator<Item = &'a (bip32::Fingerprint, bip32::DerivationPath)>,
    ) -> PathSpendInfo {
        let mut signed_pubkeys = HashMap::new();
        let mut sigs_count = 0;
        let (threshold, origins) = self.thresh_origins();

        // For all existing signatures, pick those that are from one of our pubkeys.
        for (fg, der_path) in all_pubkeys_signed {
            // For all xpubs in the descriptor, we derive at /0/* or /1/*, so the xpub's origin's
            // derivation path is the key's one without the last two derivation indexes.
            if der_path.len() < 2 {
                continue;
            }
            let parent_der_path: bip32::DerivationPath = der_path[..der_path.len() - 2].into();
            let parent_origin = (*fg, parent_der_path);

            // Now if the origin of this key without the two final derivation indexes is part of
            // the set of our keys, count it as a signature for it. Note it won't mixup keys
            // between spending paths, since we can't have duplicate xpubs in the descriptor and
            // the (fingerprint, der_path) tuple is a UID for an xpub.
            if origins.contains(&parent_origin) {
                sigs_count += 1;
                if let Some(count) = signed_pubkeys.get_mut(&parent_origin) {
                    *count += 1;
                } else {
                    signed_pubkeys.insert(parent_origin, 1);
                }
            }
        }

        PathSpendInfo {
            threshold,
            sigs_count,
            signed_pubkeys,
        }
    }

    // TODO: avoid using a vec...
    /// Get the keys contained in this spending path.
    pub fn keys(&self) -> Vec<descriptor::DescriptorPublicKey> {
        match self {
            PathInfo::Single(ref key) => vec![key.clone()],
            PathInfo::Multi(_, keys) => keys.clone(),
        }
    }

    /// Get a Miniscript Policy for this path.
    pub fn into_ms_policy(self) -> ConcretePolicy<descriptor::DescriptorPublicKey> {
        match self {
            PathInfo::Single(key) => ConcretePolicy::Key(key),
            PathInfo::Multi(thresh, keys) => ConcretePolicy::Threshold(
                thresh,
                keys.into_iter().map(ConcretePolicy::Key).collect(),
            ),
        }
    }
}

/// A Liana spending policy. Can be created from some settings (the primary and recovery keys, the
/// timelock(s)) and be used to derive a descriptor. It can also be inferred from a descriptor and
/// be used to retrieve the settings.
/// Do note however that the descriptor generation process is not deterministic, therefore you
/// **cannot roundtrip** a descriptor through a `LianaPolicy`.
#[derive(Debug, Eq, PartialEq, Clone, Ord, PartialOrd, Hash)]
pub struct LianaPolicy {
    pub(super) primary_path: PathInfo,
    pub(super) recovery_path: (u16, PathInfo),
}

impl LianaPolicy {
    /// Create a new Liana policy from a given configuration.
    pub fn new(
        primary_path: PathInfo,
        recovery_path: PathInfo,
        recovery_timelock: u16,
    ) -> Result<LianaPolicy, LianaPolicyError> {
        // We require the locktime to:
        //  - not be disabled
        //  - be in number of blocks
        //  - be 'clean' / minimal, ie all bits without consensus meaning should be 0
        //  - be positive (Miniscript requires it not to be 0)
        //
        // All this is achieved through asking for a 16-bit integer.
        if recovery_timelock == 0 {
            return Err(LianaPolicyError::InsaneTimelock(recovery_timelock as u32));
        }

        // Check all keys are valid according to our standard (this checks all are multipath keys).
        let (prim_keys, rec_keys) = (primary_path.keys(), recovery_path.keys());
        let all_keys = prim_keys.iter().chain(rec_keys.iter());
        if let Some(key) = all_keys.clone().find(|k| !is_valid_desc_key(k)) {
            return Err(LianaPolicyError::InvalidKey((*key).clone().into()));
        }

        // Check for key duplicates. They are invalid in (nonmalleable) miniscripts. This is
        // checked by the Miniscript policy compiler too but not at the raw xpub level.
        let mut key_set = HashSet::new();
        for key in all_keys {
            let xpub = match key {
                descriptor::DescriptorPublicKey::MultiXPub(ref multi_xpub) => multi_xpub.xkey,
                _ => unreachable!("Just checked it was a multixpub above"),
            };
            if key_set.contains(&xpub) {
                return Err(LianaPolicyError::DuplicateKey(key.clone().into()));
            }
            key_set.insert(xpub);
        }
        assert!(!key_set.is_empty());

        // Make sure it is a valid Miniscript policy by (ab)using the compiler.
        let policy = LianaPolicy {
            primary_path,
            recovery_path: (recovery_timelock, recovery_path),
        };
        policy.clone().into_miniscript()?;
        Ok(policy)
    }

    /// Create a Liana policy from a descriptor. This will check the descriptor is correctly formed
    /// (P2WSH, multipath, ..) and has a valid Liana semantic.
    pub fn from_multipath_descriptor(
        desc: &descriptor::Descriptor<descriptor::DescriptorPublicKey>,
    ) -> Result<LianaPolicy, LianaPolicyError> {
        // For now we only allow P2WSH descriptors.
        let wsh_desc = match &desc {
            descriptor::Descriptor::Wsh(desc) => desc,
            _ => return Err(LianaPolicyError::IncompatibleDesc),
        };

        // Get the Miniscript from the descriptor and make sure it only contains valid multipath
        // descriptor keys.
        let ms = match wsh_desc.as_inner() {
            descriptor::WshInner::Ms(ms) => ms,
            _ => return Err(LianaPolicyError::IncompatibleDesc),
        };
        let invalid_key = ms.iter_pk().find_map(|pk| {
            if is_valid_desc_key(&pk) {
                None
            } else {
                Some(pk)
            }
        });
        if let Some(key) = invalid_key {
            return Err(LianaPolicyError::InvalidKey(key.into()));
        }

        // Now lift a semantic policy out of this Miniscript and normalize it to make sure we
        // compare apples to apples below.
        let policy = ms
            .lift()
            .expect("Lifting can't fail on a Miniscript")
            .normalized();

        // The policy must always be "1 of N spending paths" with at least an always-available
        // primary path with at least one key, and at least one timelocked recovery path with at
        // least one key.
        let subs = match policy {
            SemanticPolicy::Threshold(1, subs) => Some(subs),
            _ => None,
        }
        .ok_or(LianaPolicyError::IncompatibleDesc)?;

        // Fetch the two spending paths' semantic policies. The primary path is identified as the
        // only one that isn't timelocked.
        let (mut primary_path, mut recovery_path) = (None::<PathInfo>, None);
        for sub in subs {
            // This is a (multi)key check. It must be the primary path.
            if is_single_key_or_multisig(&sub) {
                // We only support a single primary path. But it may be that the primary path is a
                // 1-of-N multisig. In this case the policy is normalized from `thresh(1, thresh(1,
                // pk(A), pk(B)), thresh(2, older(42), pk(C)))` to `thresh(1, pk(A), pk(B),
                // thresh(2, older(42), pk(C)))`.
                if let Some(prim_path) = primary_path {
                    if let SemanticPolicy::Key(key) = sub {
                        primary_path = Some(prim_path.with_added_key(key));
                    } else {
                        return Err(LianaPolicyError::IncompatibleDesc);
                    }
                } else {
                    primary_path = Some(PathInfo::from_primary_path(sub)?);
                }
            } else {
                // If it's not a simple (multi)key check, it must be the timelocked recovery path.
                // For now, we only support a single recovery path.
                if recovery_path.is_some() {
                    return Err(LianaPolicyError::IncompatibleDesc);
                }
                recovery_path = Some(PathInfo::from_recovery_path(sub)?);
            }
        }

        // Use the constructor for the sanity checks (especially around the Miniscript policy).
        let prim_path = primary_path.ok_or(LianaPolicyError::IncompatibleDesc)?;
        let (timelock, reco_path) = recovery_path.ok_or(LianaPolicyError::IncompatibleDesc)?;
        LianaPolicy::new(prim_path, reco_path, timelock)
    }

    pub fn primary_path(&self) -> &PathInfo {
        &self.primary_path
    }

    /// Timelock and path info for the recovery path.
    pub fn recovery_path(&self) -> (u16, &PathInfo) {
        (self.recovery_path.0, &self.recovery_path.1)
    }

    fn into_miniscript(
        self,
    ) -> Result<
        miniscript::Miniscript<descriptor::DescriptorPublicKey, miniscript::Segwitv0>,
        LianaPolicyError,
    > {
        let LianaPolicy {
            primary_path,
            recovery_path: (timelock, recovery_path),
        } = self;

        // Create the timelocked recovery spending path.
        let recovery_timelock = ConcretePolicy::Older(Sequence::from_height(timelock));
        let recovery_keys = recovery_path.into_ms_policy();
        let recovery_branch = ConcretePolicy::And(vec![recovery_keys, recovery_timelock]);

        // Create the primary spending path and combine both, assuming the recovery path will
        // seldom be used.
        let primary_keys = primary_path.into_ms_policy();
        let tl_policy = ConcretePolicy::Or(vec![(99, primary_keys), (1, recovery_branch)]);

        tl_policy
            .compile::<miniscript::Segwitv0>()
            .map_err(LianaPolicyError::InvalidPolicy)
    }

    /// Create a descriptor from this spending policy with multipath key expressions. Note this
    /// involves a Miniscript policy compilation: this function is **not deterministic**. If you
    /// are inferring a `LianaPolicy` from a descriptor, generating a descriptor from this
    /// `LianaPolicy` may not yield the same descriptor.
    pub fn into_multipath_descriptor(
        self,
    ) -> descriptor::Descriptor<descriptor::DescriptorPublicKey> {
        let ms = self
            .into_miniscript()
            .expect("This is always checked when creating a LianaPolicy.");
        miniscript::Segwitv0::check_local_validity(&ms).expect("Miniscript must be sane");
        descriptor::Descriptor::Wsh(descriptor::Wsh::new(ms).expect("Must pass sanity checks"))
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
    pub signed_pubkeys: HashMap<(bip32::Fingerprint, bip32::DerivationPath), usize>,
}

/// Information about a partial spend of Liana coins
#[derive(Debug, Eq, PartialEq, Clone)]
pub struct PartialSpendInfo {
    /// Number of signatures present for the primary path
    pub(super) primary_path: PathSpendInfo,
    /// Number of signatures present for the recovery path, only present if the path is available
    /// in the first place.
    pub(super) recovery_path: Option<PathSpendInfo>,
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
