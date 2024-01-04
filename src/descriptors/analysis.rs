use miniscript::{
    bitcoin::{bip32, Sequence},
    descriptor,
    policy::{compiler, Concrete as ConcretePolicy, Liftable, Semantic as SemanticPolicy},
    ScriptContext,
};

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    convert::TryFrom,
    error, fmt,
};

#[derive(Debug)]
pub enum LianaPolicyError {
    MissingRecoveryPath,
    InsaneTimelock(u32),
    InvalidKey(Box<descriptor::DescriptorPublicKey>),
    DuplicateKey(Box<descriptor::DescriptorPublicKey>),
    /// The same signer was used more than once in a single spending path.
    DuplicateOriginSamePath(Box<descriptor::DescriptorPublicKey>),
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
            Self::MissingRecoveryPath => write!(f, "A Liana policy requires at least one recovery path."),
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
            Self::DuplicateOriginSamePath(key) => {
                write!(f, "Key '{}' is derived from the same origin as another key present in the same spending path. It is not possible to use a signer more than once within a single spending path.", key)
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

struct DescKeyChecker {
    keys_set: HashSet<(bip32::Xpub, descriptor::DerivPaths)>,
}

impl DescKeyChecker {
    pub fn new() -> DescKeyChecker {
        DescKeyChecker {
            keys_set: HashSet::new(),
        }
    }

    /// We require the descriptor key to:
    ///  - Be deriveable (to contain a wildcard)
    ///  - Be multipath (to contain a step in the derivation path with multiple indexes)
    ///  - The multipath step to only contain two indexes. These can be any indexes, which is
    ///     useful for deriving multiple keys from the same xpub.
    ///  - Be 'signable' by an external signer (to contain an origin)
    ///
    /// This returns the origin fingerprint for this xpub, to make it possible for the caller to
    /// check the same signer is never used twice in the same spending path.
    pub fn check(
        &mut self,
        key: &descriptor::DescriptorPublicKey,
    ) -> Result<bip32::Fingerprint, LianaPolicyError> {
        if let descriptor::DescriptorPublicKey::MultiXPub(ref xpub) = *key {
            let key_identifier = (xpub.xkey, xpub.derivation_paths.clone());
            // First make sure it's not a duplicate and record seeing it.
            if self.keys_set.contains(&key_identifier) {
                return Err(LianaPolicyError::DuplicateKey(key.clone().into()));
            }
            self.keys_set.insert(key_identifier);
            // Then perform the contextless checks (origin, deriv paths, ..).
            // Technically the xpub could be for the master xpub and not have an origin. But it's
            // unlikely (and easily fixable) while users shooting themselves in the foot by
            // forgetting to provide the origin is so likely that it's worth ruling out xpubs
            // without origin entirely.
            if let Some(ref origin) = xpub.origin {
                let der_paths = xpub.derivation_paths.paths();
                let first_der_path = der_paths.first().expect("Cannot be empty");
                // We also rule out xpubs with hardened derivation steps (non-normalized xpubs).
                let valid = xpub.wildcard == descriptor::Wildcard::Unhardened
                    && der_paths.len() == 2
                    && first_der_path.into_iter().all(|step| step.is_normal());
                if valid {
                    return Ok(origin.0);
                }
            }
        }
        Err(LianaPolicyError::InvalidKey(key.clone().into()))
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

// Get the fingerprint and the full derivation paths (path from the master fingerprint in the
// origin, with the xpub derivation path appended) for a multipath xpub.
fn key_origins(
    key: &descriptor::DescriptorPublicKey,
) -> Option<(bip32::Fingerprint, HashSet<bip32::DerivationPath>)> {
    match key {
        descriptor::DescriptorPublicKey::MultiXPub(ref xpub) => {
            xpub.origin.as_ref().map(|(fg, orig_path)| {
                let mut der_paths = HashSet::with_capacity(xpub.derivation_paths.paths().len());
                for der_path in xpub.derivation_paths.paths() {
                    der_paths.insert(
                        orig_path
                            .into_iter()
                            .chain(der_path.into_iter())
                            .copied()
                            .collect(),
                    );
                }
                (*fg, der_paths)
            })
        }
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
    /// that can be used to provide a signature for this path. The set of keys is represented as a
    /// mapping from a master extended key fingerprint, to a set of derivation paths. This is
    /// because we are using multipath descriptors. The derivation paths included the xpub's
    /// derivation path appended to the origin's derivation path (without the wildcard step).
    pub fn thresh_origins(
        &self,
    ) -> (
        usize,
        HashMap<bip32::Fingerprint, HashSet<bip32::DerivationPath>>,
    ) {
        match self {
            PathInfo::Single(key) => {
                let mut all_origins = HashMap::with_capacity(1);
                let (fg, der_path) = key_origins(key).expect("Must be a multixpub with an origin.");
                all_origins.insert(fg, der_path);
                (1, all_origins)
            }
            PathInfo::Multi(k, keys) => {
                let mut all_origins: HashMap<_, HashSet<_>> = HashMap::with_capacity(keys.len());
                for key in keys {
                    let (fg, der_paths) =
                        key_origins(key).expect("Must be a multixpub with an origin.");
                    if let Some(existing_der_paths) = all_origins.get_mut(&fg) {
                        existing_der_paths.extend(der_paths)
                    } else {
                        all_origins.insert(fg, der_paths);
                    }
                }
                (*k, all_origins)
            }
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
            // All xpubs in the descriptor must be wildcard, and therefore have a derivation with
            // at least one step. (In practice there is at least two, for `/<0;1>/*`.)
            if der_path.is_empty() {
                continue;
            }

            // Now check if this signature is for a public key derived from the fingerprint of one
            // of our known master xpubs.
            if let Some(parent_der_paths) = origins.get(fg) {
                // If it is, make sure it's for one of the xpubs included in the descriptor. Remove
                // the wilcard step and check if it's in the set of the derivation paths.
                let der_path_wo_wc: bip32::DerivationPath = der_path[..der_path.len() - 1].into();
                if parent_der_paths.contains(&der_path_wo_wc) {
                    // If the origin of the key without the wilcard step is part of our keys, count
                    // it as a signature. Also record how many times this master extended key
                    // signed.
                    sigs_count += 1;
                    if let Some(count) = signed_pubkeys.get_mut(fg) {
                        *count += 1;
                    } else {
                        signed_pubkeys.insert(*fg, 1);
                    }
                }
            }
        }

        PathSpendInfo {
            threshold,
            sigs_count,
            signed_pubkeys,
        }
    }

    /// Get a Miniscript Policy for this path.
    pub fn into_ms_policy(self) -> ConcretePolicy<descriptor::DescriptorPublicKey> {
        match self {
            PathInfo::Single(key) => ConcretePolicy::Key(key),
            PathInfo::Multi(thresh, keys) => ConcretePolicy::Threshold(
                thresh,
                keys.into_iter()
                    .map(|key| ConcretePolicy::Key(key).into())
                    .collect(),
            ),
        }
    }
}

/// A Liana spending policy is one composed of at least two spending paths:
///     - A directly available path with any number of keys checks; or
///     - One or more recovery paths with any number of keys checks, behind increasing relative
///     timelocks. No two recovery paths may have the same timelock.
/// A Liana policy can be created from some settings (the primary and recovery keys, the
/// timelock(s)) and be used to derive a descriptor. It can also be inferred from a descriptor and
/// be used to retrieve the settings.
/// Do note however that the descriptor generation process is not deterministic, therefore you
/// **cannot roundtrip** a descriptor through a `LianaPolicy`.
#[derive(Debug, Eq, PartialEq, Clone)]
pub struct LianaPolicy {
    pub(super) primary_path: PathInfo,
    pub(super) recovery_paths: BTreeMap<u16, PathInfo>,
}

impl LianaPolicy {
    /// Create a new Liana policy from a given configuration.
    pub fn new(
        primary_path: PathInfo,
        recovery_paths: BTreeMap<u16, PathInfo>,
    ) -> Result<LianaPolicy, LianaPolicyError> {
        if recovery_paths.is_empty() {
            return Err(LianaPolicyError::MissingRecoveryPath);
        }

        // We require the locktime to:
        //  - not be disabled
        //  - be in number of blocks
        //  - be 'clean' / minimal, ie all bits without consensus meaning should be 0
        //  - be positive (Miniscript requires it not to be 0)
        //
        // All this is achieved through asking for a 16-bit integer.
        if recovery_paths.contains_key(&0) {
            return Err(LianaPolicyError::InsaneTimelock(0));
        }

        // Check all keys are valid according to our standard (this checks all are multipath keys).
        // Note while the Miniscript compiler does check for duplicate, it does so at the
        // "descriptor key expression" level. We don't want duplicate xpubs at all so we do it
        // ourselves here.
        let spending_paths = recovery_paths
            .values()
            .chain(std::iter::once(&primary_path));
        let mut key_checker = DescKeyChecker::new();
        for path in spending_paths {
            match path {
                PathInfo::Single(ref key) => {
                    let _ = key_checker.check(key)?;
                }
                PathInfo::Multi(_, ref keys) => {
                    // Record the origins of the keys for this spending path. If any two keys share
                    // the same origin, they are from the same signer. We restrict using a signer
                    // more than once within a single spending path as it can lead to surprising
                    // behaviour. For details see:
                    // https://github.com/wizardsardine/liana/pull/706#issuecomment-1744705808
                    let mut origin_fingerprints = HashSet::with_capacity(keys.len());
                    for key in keys {
                        let fg = key_checker.check(key)?;
                        if origin_fingerprints.contains(&fg) {
                            return Err(LianaPolicyError::DuplicateOriginSamePath(
                                key.clone().into(),
                            ));
                        }
                        origin_fingerprints.insert(fg);
                    }
                }
            }
        }

        // Make sure it is a valid Miniscript policy by (ab)using the compiler.
        let policy = LianaPolicy {
            primary_path,
            recovery_paths,
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

        // Lift a semantic policy out of this Miniscript and normalize it to make sure we compare
        // apples to apples below.
        let ms = match wsh_desc.as_inner() {
            descriptor::WshInner::Ms(ms) => ms,
            _ => return Err(LianaPolicyError::IncompatibleDesc),
        };
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

        // Fetch all spending paths' semantic policies. The primary path is identified as the only
        // one that isn't timelocked.
        let (mut primary_path, mut recovery_paths) = (None::<PathInfo>, BTreeMap::new());
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
                // If it's not a simple (multi)key check, it must be (one of) the timelocked
                // recovery path(s).
                let (timelock, path_info) = PathInfo::from_recovery_path(sub)?;
                if recovery_paths.contains_key(&timelock) {
                    return Err(LianaPolicyError::IncompatibleDesc);
                }
                recovery_paths.insert(timelock, path_info);
            }
        }

        // Use the constructor for sanity checking the keys and the Miniscript policy. Note this
        // makes sure the recovery paths mapping isn't empty, too.
        let prim_path = primary_path.ok_or(LianaPolicyError::IncompatibleDesc)?;
        LianaPolicy::new(prim_path, recovery_paths)
    }

    pub fn primary_path(&self) -> &PathInfo {
        &self.primary_path
    }

    /// Timelocks and path info of the recovery paths. Note we guarantee this mapping is never
    /// empty, as there is always at least one recovery path.
    pub fn recovery_paths(&self) -> &BTreeMap<u16, PathInfo> {
        assert!(!self.recovery_paths.is_empty());
        &self.recovery_paths
    }

    fn into_miniscript(
        self,
    ) -> Result<
        miniscript::Miniscript<descriptor::DescriptorPublicKey, miniscript::Segwitv0>,
        LianaPolicyError,
    > {
        let LianaPolicy {
            primary_path,
            recovery_paths,
        } = self;

        // Start with the primary spending path. We'll then or() all the recovery paths to it.
        let primary_keys = primary_path.into_ms_policy();

        // Incrementally create the top-level policy using all recovery paths.
        assert!(!recovery_paths.is_empty());
        let tl_policy =
            recovery_paths
                .into_iter()
                .fold(primary_keys, |tl_policy, (timelock, path_info)| {
                    let timelock = ConcretePolicy::Older(Sequence::from_height(timelock));
                    let keys = path_info.into_ms_policy();
                    let recovery_branch = ConcretePolicy::And(vec![keys.into(), timelock.into()]);
                    // We assume the larger the timelock the less likely a branch would be used.
                    ConcretePolicy::Or(vec![(99, tl_policy.into()), (1, recovery_branch.into())])
                });

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
    pub signed_pubkeys: HashMap<bip32::Fingerprint, usize>,
}

/// Information about a partial spend of Liana coins
#[derive(Debug, Eq, PartialEq, Clone)]
pub struct PartialSpendInfo {
    /// Number of signatures present for the primary path
    pub(super) primary_path: PathSpendInfo,
    /// Number of signatures present for the recovery path, only present for the recovery paths
    /// that are available.
    pub(super) recovery_paths: BTreeMap<u16, PathSpendInfo>,
}

impl PartialSpendInfo {
    /// Get the number of signatures present for the primary path
    pub fn primary_path(&self) -> &PathSpendInfo {
        &self.primary_path
    }

    /// Get the number of signatures present for each recovery path. Only present for available
    /// paths.
    pub fn recovery_paths(&self) -> &BTreeMap<u16, PathSpendInfo> {
        &self.recovery_paths
    }
}
