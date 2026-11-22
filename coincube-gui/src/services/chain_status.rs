//! Which chains a Vault coin exists on, for Coin control's Chains column and
//! the Split coins overlay (`decisions/2026-10-29-coincube-desktop-btcb2-split-
//! protection-in-coin-control.md`).
//!
//! A coin is reported as existing on this chain only on **positive** evidence:
//! a structural path to a post-fork coinbase whose two chains' coinbases
//! differ (the existing ancestry engine), or a confirmed split recorded in this
//! Cube. A split output descends from its split key, so its proof is the key's
//! path extended by one link and needs no other kind of evidence. "The twin
//! chain has no such transaction" ([`Entanglement::NotEntangled`]) is never
//! proof: anyone holding the funding transaction can still broadcast it there,
//! so it shows as *checking*, not *verified*.
//!
//! Everything here is display and candidate selection. Signing never trusts a
//! cached status; the split builder re-qualifies its key before any signature.

pub mod store;

use std::{
    collections::{HashMap, HashSet},
    time::{Duration, SystemTime},
};

use coincube_core::miniscript::bitcoin::{OutPoint, Txid};

use crate::{chain::ChainId, services::entangled::Entanglement};

/// Confirmations a split needs before its coins count as split on both chains.
pub const SPLIT_CONFIRMATIONS: u32 = coincube_core::claim::MIN_CONFIRMATIONS as u32;

/// What the Chains column says about one coin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainStatus {
    /// The funding transaction exists on both chains and nothing has separated
    /// the two copies yet.
    BothChains,
    /// Spent by this Cube's split, which has `confirmations` of
    /// [`SPLIT_CONFIRMATIONS`].
    Splitting { confirmations: u32 },
    /// An output of this Cube's signature split, which is past
    /// [`SPLIT_CONFIRMATIONS`]. Not proof: the Vault could still confirm a
    /// legacy-signed copy of the split on Bitcoin. (The twin Cube counts the
    /// split's inputs as separated once their confirmed witness is checked;
    /// see `split_coins::twin_confirmed`.)
    SplitUnconfirmed,
    /// Proven to exist on this chain only.
    OnlyHere,
    /// The twin chain has no such transaction, which proves nothing; an
    /// ancestry check may still verify it.
    NotSeen,
    /// The ancestry check ran and found no qualifying path.
    CouldntVerify,
    /// No answer yet.
    NotChecked,
}

/// How a [`ChainStatus`] is coloured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusTone {
    Warn,
    Good,
    Muted,
}

/// The facts behind one coin's status.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CoinEvidence {
    /// The twin-chain lookup of the funding transaction; `None` before any.
    pub entanglement: Option<Entanglement>,
    /// The ancestry check of this exact coin.
    pub proof: Option<ProofOutcome>,
    /// Confirmations of the split of this Cube that spends this coin.
    pub splitting: Option<u32>,
    /// Spent by this Cube's signature split past [`SPLIT_CONFIRMATIONS`].
    pub signature_split: bool,
    /// A split-key split on the twin Cube spent this coin's twin copy, and
    /// Connect showed it [`SPLIT_CONFIRMATIONS`] deep on the twin chain.
    pub separated: bool,
}

/// The display-relevant part of an ancestry check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProofOutcome {
    Verified,
    NoQualifyingAncestry,
}

/// Classify one coin. A coin seen on both chains stays [`ChainStatus::BothChains`]
/// even if an ancestry path also verified, since the two facts contradict each
/// other and the safe reading wins.
pub fn classify(evidence: CoinEvidence) -> ChainStatus {
    if let Some(confirmations) = evidence.splitting {
        return ChainStatus::Splitting {
            confirmations: confirmations.min(SPLIT_CONFIRMATIONS),
        };
    }
    if evidence.separated {
        return ChainStatus::OnlyHere;
    }
    if evidence.entanglement == Some(Entanglement::Entangled) {
        return ChainStatus::BothChains;
    }
    // After the twin-chain lookup: a signature split replayed onto the twin
    // chain shows up there, and the safe reading wins over its intent.
    if evidence.signature_split {
        return ChainStatus::SplitUnconfirmed;
    }
    match evidence.proof {
        Some(ProofOutcome::Verified) => ChainStatus::OnlyHere,
        Some(ProofOutcome::NoQualifyingAncestry) => ChainStatus::CouldntVerify,
        None => match evidence.entanglement {
            Some(Entanglement::NotEntangled) => ChainStatus::NotSeen,
            _ => ChainStatus::NotChecked,
        },
    }
}

/// The short name a chain goes by in the Chains column.
pub fn short_name(chain: ChainId) -> &'static str {
    if chain.is_blake2b() {
        "Blake2b"
    } else {
        "Bitcoin"
    }
}

impl ChainStatus {
    /// The Chains column label on a Cube whose Vault is on `here`.
    pub fn label(self, here: ChainId) -> String {
        let twin = crate::services::entangled::twin_chain(here).unwrap_or(here);
        match self {
            Self::BothChains => "Both chains".to_string(),
            Self::Splitting { confirmations } => {
                format!("Splitting · {} of {}", confirmations, SPLIT_CONFIRMATIONS)
            }
            Self::SplitUnconfirmed => "Split · unconfirmed".to_string(),
            Self::OnlyHere => format!("{} only · verified", short_name(here)),
            Self::NotSeen => format!("Not seen on {} · checking", short_name(twin)),
            Self::CouldntVerify => "Couldn't verify".to_string(),
            Self::NotChecked => "Not checked yet".to_string(),
        }
    }

    /// One sentence for the badge's tooltip.
    pub fn explanation(self, here: ChainId) -> String {
        let twin = crate::services::entangled::twin_chain(here).unwrap_or(here);
        match self {
            Self::BothChains => format!(
                "This coin also exists on {}. Until it is split, a transaction on one chain can be copied to the other.",
                chain_name(twin)
            ),
            Self::Splitting { .. } => format!(
                "Spent by a split. After {} confirmations it exists on each chain separately.",
                SPLIT_CONFIRMATIONS
            ),
            Self::SplitUnconfirmed => format!(
                "Spent by a split meant to use a replay-protected signature. The app can't check that signature yet, so this coin isn't verified as {} only.",
                short_name(here)
            ),
            Self::OnlyHere => format!(
                "It can no longer be copied to or from {}.",
                chain_name(twin)
            ),
            Self::NotSeen => format!(
                "Its transaction wasn't found on {}. That alone doesn't prove anything, so its history is being checked.",
                chain_name(twin)
            ),
            Self::CouldntVerify => format!(
                "Its history couldn't be proven to exist only on {}. It can't serve as a split key.",
                chain_name(here)
            ),
            Self::NotChecked => "The other chain hasn't been checked yet. It retries after sync.".to_string(),
        }
    }

    pub fn tone(self) -> StatusTone {
        match self {
            Self::BothChains | Self::Splitting { .. } | Self::SplitUnconfirmed => StatusTone::Warn,
            Self::OnlyHere => StatusTone::Good,
            Self::NotSeen | Self::CouldntVerify | Self::NotChecked => StatusTone::Muted,
        }
    }

    /// Whether the Split coins overlay offers this coin as one to split.
    pub fn needs_split(self) -> bool {
        matches!(self, Self::BothChains)
    }
}

fn chain_name(chain: ChainId) -> &'static str {
    if chain.is_blake2b() {
        "Bitcoin Blake2b"
    } else {
        "Bitcoin"
    }
}

/// One coin's ancestry check result as the cache holds it.
#[derive(Debug, Clone)]
pub enum ProofEntry {
    /// A path to a qualifying coinbase found by a check in this session.
    Verified(coincube_core::claim_ancestry::retained::RetainedPath),
    /// A path loaded from `chain-proofs.json`. It decodes, but decoding only
    /// proves its links fit together, not that the chain still holds them
    /// (a reorg or a restart in between). Shown as checking and never used as
    /// a split key until a fresh check turns it into [`Self::Verified`].
    Saved(coincube_core::claim_ancestry::retained::RetainedPath),
    /// The bounded search found no qualifying path at `checked_at`.
    NoQualifyingAncestry { checked_at: SystemTime },
    /// The check could not finish at `failed_at`. Shown as not checked.
    Failed { failed_at: SystemTime },
    /// A [`Self::Saved`] path whose fresh check could not finish at
    /// `failed_at`. A check that can't finish (Connect down, a read timing
    /// out) says nothing about the path, so the path is kept, written back,
    /// and re-checked after [`FAILED_RETRY`] like any failed check. Shown
    /// as checking, never used as a split key.
    SavedRetry {
        path: coincube_core::claim_ancestry::retained::RetainedPath,
        failed_at: SystemTime,
    },
}

impl ProofEntry {
    pub fn outcome(&self) -> Option<ProofOutcome> {
        match self {
            Self::Verified(_) => Some(ProofOutcome::Verified),
            Self::NoQualifyingAncestry { .. } => Some(ProofOutcome::NoQualifyingAncestry),
            Self::Saved(_) | Self::Failed { .. } | Self::SavedRetry { .. } => None,
        }
    }

    /// The path this entry holds, verified or waiting for a fresh check.
    pub fn path(&self) -> Option<&coincube_core::claim_ancestry::retained::RetainedPath> {
        match self {
            Self::Verified(path) | Self::Saved(path) | Self::SavedRetry { path, .. } => Some(path),
            Self::NoQualifyingAncestry { .. } | Self::Failed { .. } => None,
        }
    }

    /// Fold a fresh check's `result` into what was held before. A check
    /// that could not finish keeps a held path (as [`Self::SavedRetry`]);
    /// every other result is the answer.
    pub fn after_check(previous: Option<&Self>, result: Self) -> Self {
        match (previous.and_then(Self::path), result) {
            (Some(path), Self::Failed { failed_at }) => Self::SavedRetry {
                path: path.clone(),
                failed_at,
            },
            (_, result) => result,
        }
    }

    fn retry_due(&self, now: SystemTime) -> bool {
        let (at, after) = match self {
            Self::Verified(_) => return false,
            Self::Saved(_) => return true,
            Self::NoQualifyingAncestry { checked_at } => (*checked_at, NO_PATH_RETRY),
            Self::Failed { failed_at } | Self::SavedRetry { failed_at, .. } => {
                (*failed_at, FAILED_RETRY)
            }
        };
        now.duration_since(at).map_or(true, |age| age >= after)
    }
}

/// A coin with no qualifying path may be re-checked after this long: a deeper
/// confirmation history does not change the answer, so it is rare.
pub const NO_PATH_RETRY: Duration = Duration::from_secs(24 * 60 * 60);
/// A check that failed to finish is retried sooner.
pub const FAILED_RETRY: Duration = Duration::from_secs(30 * 60);
/// At most this many ancestry searches per pass; each can take up to 90 s.
pub const MAX_CHECKS_PER_PASS: usize = 2;

/// Which coins to run the ancestry check on now. Only coins whose funding
/// transaction the twin chain definitely lacks are candidates: a coin seen on
/// both chains cannot have a qualifying path, and an unanswered lookup is
/// asked first. Sorted, deduplicated and capped at [`MAX_CHECKS_PER_PASS`].
pub fn proof_candidates(
    coins: impl IntoIterator<Item = OutPoint>,
    entanglement: impl Fn(&Txid) -> Entanglement,
    proofs: &HashMap<OutPoint, ProofEntry>,
    in_flight: &HashSet<OutPoint>,
    now: SystemTime,
) -> Vec<OutPoint> {
    let mut candidates: Vec<OutPoint> = coins
        .into_iter()
        .filter(|outpoint| !in_flight.contains(outpoint))
        .filter(|outpoint| entanglement(&outpoint.txid) == Entanglement::NotEntangled)
        .filter(|outpoint| {
            proofs
                .get(outpoint)
                .is_none_or(|entry| entry.retry_due(now))
        })
        .collect();
    candidates.sort();
    candidates.dedup();
    candidates.truncate(MAX_CHECKS_PER_PASS);
    candidates
}

/// Whether the ancestry engine runs on `chain`: Bitcoin and Bitcoin
/// Blake2b mainnet, the pair Connect observes. There is no launch gate; the
/// App runs checks and builds splits whenever Bitcoin Blake2b is on for the
/// Cube (`App::blake2b_on`, on Robert's instruction of 2026-10-29).
pub fn checks_enabled(chain: ChainId) -> bool {
    matches!(chain, ChainId::Bitcoin | ChainId::BitcoinBlake2b)
}

/// Run the ancestry check on `outpoints` of a Vault on `chain`, one at a
/// time, through Connect. A search that cannot finish is [`ProofEntry::Failed`],
/// never a negative answer.
pub async fn check_ancestry(
    client: crate::services::coincube::CoincubeClient,
    chain: ChainId,
    outpoints: Vec<OutPoint>,
    generation: u64,
    live: tokio::sync::watch::Receiver<u64>,
) -> Vec<(OutPoint, ProofEntry)> {
    use crate::services::claim_observation::{http::HttpObservationSource, CollectionContext};

    let failed = |outpoints: &[OutPoint]| {
        outpoints
            .iter()
            .map(|outpoint| {
                (
                    *outpoint,
                    ProofEntry::Failed {
                        failed_at: SystemTime::now(),
                    },
                )
            })
            .collect::<Vec<_>>()
    };
    let source = match HttpObservationSource::new(
        client,
        ChainId::Bitcoin,
        ChainId::BitcoinBlake2b,
        CollectionContext {
            expected_generation: generation,
            generation: live,
        },
    ) {
        Ok(source) => source,
        Err(_) => return failed(&outpoints),
    };
    let policy = coincube_core::claim::Policy {
        max_observation_age_seconds: 60,
    };
    let mut results = Vec::with_capacity(outpoints.len());
    for outpoint in outpoints {
        let found = if chain.is_blake2b() {
            source.discover_fork_ancestry(outpoint, policy).await
        } else {
            source.discover_ancestry(outpoint, policy).await
        };
        let entry = match found {
            Ok(Some(discovered)) => match discovered.retained_path() {
                Ok(path) => ProofEntry::Verified(path),
                Err(_) => ProofEntry::Failed {
                    failed_at: SystemTime::now(),
                },
            },
            Ok(None) => ProofEntry::NoQualifyingAncestry {
                checked_at: SystemTime::now(),
            },
            Err(error) => {
                tracing::debug!(
                    "ancestry check for {} did not finish: {:?}",
                    outpoint,
                    error
                );
                ProofEntry::Failed {
                    failed_at: SystemTime::now(),
                }
            }
        };
        results.push((outpoint, entry));
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn outpoint(n: u8) -> OutPoint {
        OutPoint::from_str(&format!("{}:0", format!("{:02x}", n).repeat(32))).unwrap()
    }

    #[test]
    fn negative_lookup_is_never_verified() {
        let status = classify(CoinEvidence {
            entanglement: Some(Entanglement::NotEntangled),
            ..Default::default()
        });
        assert_eq!(status, ChainStatus::NotSeen);
        assert_eq!(
            status.label(ChainId::Bitcoin),
            "Not seen on Blake2b · checking"
        );
    }

    #[test]
    fn both_chains_beats_a_contradicting_path() {
        let status = classify(CoinEvidence {
            entanglement: Some(Entanglement::Entangled),
            proof: Some(ProofOutcome::Verified),
            ..Default::default()
        });
        assert_eq!(status, ChainStatus::BothChains);
        assert!(status.needs_split());
    }

    #[test]
    fn verified_path_is_only_here_on_either_chain() {
        let status = classify(CoinEvidence {
            entanglement: Some(Entanglement::NotEntangled),
            proof: Some(ProofOutcome::Verified),
            ..Default::default()
        });
        assert_eq!(status, ChainStatus::OnlyHere);
        assert_eq!(status.label(ChainId::Bitcoin), "Bitcoin only · verified");
        assert_eq!(
            status.label(ChainId::BitcoinBlake2b),
            "Blake2b only · verified"
        );
    }

    #[test]
    fn splitting_wins_and_caps_at_the_threshold() {
        let status = classify(CoinEvidence {
            entanglement: Some(Entanglement::Entangled),
            splitting: Some(9),
            ..Default::default()
        });
        assert_eq!(status, ChainStatus::Splitting { confirmations: 6 });
        assert_eq!(status.label(ChainId::Bitcoin), "Splitting · 6 of 6");
        assert!(!status.needs_split());
    }

    #[test]
    fn a_deep_signature_split_is_split_but_unconfirmed() {
        let status = classify(CoinEvidence {
            entanglement: Some(Entanglement::NotEntangled),
            signature_split: true,
            ..Default::default()
        });
        assert_eq!(status, ChainStatus::SplitUnconfirmed);
        assert_eq!(status.label(ChainId::BitcoinBlake2b), "Split · unconfirmed");
        assert_eq!(status.tone(), StatusTone::Warn);
        assert!(!status.needs_split());
    }

    #[test]
    fn a_replayed_signature_split_is_on_both_chains() {
        let status = classify(CoinEvidence {
            entanglement: Some(Entanglement::Entangled),
            signature_split: true,
            ..Default::default()
        });
        assert_eq!(status, ChainStatus::BothChains);
        assert!(status.needs_split());
    }

    #[test]
    fn recorded_twin_split_separates_a_both_chain_coin() {
        let status = classify(CoinEvidence {
            entanglement: Some(Entanglement::Entangled),
            separated: true,
            ..Default::default()
        });
        assert_eq!(status, ChainStatus::OnlyHere);
    }

    #[test]
    fn unanswered_and_failed_checks_are_not_checked() {
        assert_eq!(classify(CoinEvidence::default()), ChainStatus::NotChecked);
        assert_eq!(
            classify(CoinEvidence {
                entanglement: Some(Entanglement::Unknown),
                ..Default::default()
            }),
            ChainStatus::NotChecked
        );
        assert_eq!(
            classify(CoinEvidence {
                entanglement: Some(Entanglement::NotEntangled),
                proof: Some(ProofOutcome::NoQualifyingAncestry),
                ..Default::default()
            }),
            ChainStatus::CouldntVerify
        );
    }

    #[test]
    fn candidates_are_only_unseen_coins_due_a_check() {
        let now = SystemTime::now();
        let coins = [
            outpoint(1),
            outpoint(2),
            outpoint(3),
            outpoint(4),
            outpoint(5),
        ];
        let entangled = |txid: &Txid| match *txid {
            t if t == outpoint(1).txid => Entanglement::Entangled,
            t if t == outpoint(2).txid => Entanglement::Unknown,
            _ => Entanglement::NotEntangled,
        };
        let mut proofs = HashMap::new();
        proofs.insert(
            outpoint(3),
            ProofEntry::NoQualifyingAncestry { checked_at: now },
        );
        proofs.insert(
            outpoint(4),
            ProofEntry::Failed {
                failed_at: now - FAILED_RETRY,
            },
        );
        let in_flight = HashSet::new();
        assert_eq!(
            proof_candidates(coins, entangled, &proofs, &in_flight, now),
            vec![outpoint(4), outpoint(5)]
        );
        let in_flight: HashSet<OutPoint> = vec![outpoint(4)].into_iter().collect();
        assert_eq!(
            proof_candidates(coins, entangled, &proofs, &in_flight, now),
            vec![outpoint(5)]
        );
    }

    #[test]
    fn candidates_are_capped_per_pass() {
        let coins: Vec<_> = (1..=5).map(outpoint).collect();
        let picked = proof_candidates(
            coins,
            |_| Entanglement::NotEntangled,
            &HashMap::new(),
            &HashSet::new(),
            SystemTime::now(),
        );
        assert_eq!(picked.len(), MAX_CHECKS_PER_PASS);
    }
}
