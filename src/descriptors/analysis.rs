use miniscript::{bitcoin::util::bip32, descriptor, policy::Semantic as SemanticPolicy};

use std::{
    collections::{HashMap, HashSet},
    convert::TryFrom,
};

use crate::descriptors::LianaDescError;

/// Whether a Miniscript policy node represents a key check (or several of them).
pub fn is_single_key_or_multisig(policy: &SemanticPolicy<descriptor::DescriptorPublicKey>) -> bool {
    match policy {
        SemanticPolicy::Key(..) => true,
        SemanticPolicy::Threshold(_, subs) => {
            subs.iter().all(|sub| matches!(sub, SemanticPolicy::Key(_)))
        }
        _ => false,
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
}

/// Information about the descriptor: how many keys are present in each path, what's the timelock
/// of the recovery path, what's the threshold if there are multiple keys, etc..
#[derive(Debug, Eq, PartialEq, Clone, Ord, PartialOrd, Hash)]
pub struct LianaDescInfo {
    pub(super) primary_path: PathInfo,
    pub(super) recovery_path: (u16, PathInfo),
}

impl LianaDescInfo {
    pub(super) fn new(primary_path: PathInfo, recovery_path: (u16, PathInfo)) -> LianaDescInfo {
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
