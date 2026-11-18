//! Pure device display policy for chain catalogs. This does not grant runtime
//! support, Connect access, authentication, or permission to perform operations.

use crate::app::settings::preferences::DevicePreferences;
use crate::chain::ChainId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetworkVisibility {
    pub developer_mode: bool,
    pub bitcoin_blake2b_beta: bool,
}

impl From<DevicePreferences> for NetworkVisibility {
    fn from(preferences: DevicePreferences) -> Self {
        Self {
            developer_mode: preferences.developer_mode,
            bitcoin_blake2b_beta: preferences.bitcoin_blake2b_beta,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkFilter {
    All,
    Live,
    Test,
    Network(ChainId),
}

impl NetworkVisibility {
    pub fn allows(self, chain: ChainId) -> bool {
        match chain {
            ChainId::Bitcoin => true,
            ChainId::Testnet | ChainId::Testnet4 | ChainId::Signet | ChainId::Regtest => {
                self.developer_mode
            }
            ChainId::BitcoinBlake2b => self.bitcoin_blake2b_beta,
            ChainId::BitcoinBlake2bTestnet4 => self.developer_mode && self.bitcoin_blake2b_beta,
        }
    }

    /// Stable `ChainId` order, including both Blake2b identities when allowed.
    pub fn visible_networks(self) -> Vec<ChainId> {
        ChainId::ALL
            .iter()
            .copied()
            .filter(|&chain| self.allows(chain))
            .collect()
    }

    /// The All value remains valid even when its visual chip is hidden.
    pub fn show_all_chip(self) -> bool {
        self.visible_networks().len() > 1
    }

    /// Both Live and Test chips are offered only in Developer mode.
    pub fn show_live_test_chips(self) -> bool {
        self.developer_mode
    }

    pub fn normalize_filter(self, filter: NetworkFilter) -> NetworkFilter {
        match filter {
            NetworkFilter::All => NetworkFilter::All,
            NetworkFilter::Live | NetworkFilter::Test if !self.developer_mode => NetworkFilter::All,
            NetworkFilter::Network(chain) if !self.allows(chain) => NetworkFilter::All,
            other => other,
        }
    }

    /// Match the normalized UI filter while intersecting with device policy;
    /// a stale Test filter can never reveal a hidden Blake2b test row.
    pub fn matches(self, filter: NetworkFilter, chain: ChainId) -> bool {
        if !self.allows(chain) {
            return false;
        }
        match self.normalize_filter(filter) {
            NetworkFilter::All => true,
            NetworkFilter::Live => matches!(chain, ChainId::Bitcoin | ChainId::BitcoinBlake2b),
            NetworkFilter::Test => !matches!(chain, ChainId::Bitcoin | ChainId::BitcoinBlake2b),
            NetworkFilter::Network(selected) => selected == chain,
        }
    }

    /// Use only for a pending creation/catalog selection. Never substitute
    /// Bitcoin for the actual chain identity of an already loaded App.
    pub fn normalize_selection(self, chain: ChainId) -> ChainId {
        if self.allows(chain) {
            chain
        } else {
            ChainId::Bitcoin
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_visibility_matrix_and_stable_order() {
        for (developer_mode, bitcoin_blake2b_beta, expected) in [
            (
                false,
                false,
                [true, false, false, false, false, false, false],
            ),
            (false, true, [true, false, false, false, false, true, false]),
            (true, false, [true, true, true, true, true, false, false]),
            (true, true, [true, true, true, true, true, true, true]),
        ] {
            let policy = NetworkVisibility {
                developer_mode,
                bitcoin_blake2b_beta,
            };
            for (chain, allowed) in ChainId::ALL.iter().copied().zip(expected.iter().copied()) {
                assert_eq!(policy.allows(chain), allowed, "{policy:?} {chain:?}");
            }
            let visible = policy.visible_networks();
            assert_eq!(
                visible,
                ChainId::ALL
                    .iter()
                    .copied()
                    .filter(|&c| policy.allows(c))
                    .collect::<Vec<_>>()
            );
            assert_eq!(
                visible.len(),
                expected.iter().filter(|&&allowed| allowed).count()
            );
            assert_eq!(policy.show_all_chip(), visible.len() > 1);
            assert_eq!(policy.show_live_test_chips(), developer_mode);
        }
    }

    #[test]
    fn filters_normalize_idempotently_and_never_leak_hidden_chains() {
        for (developer_mode, bitcoin_blake2b_beta) in
            [(false, false), (false, true), (true, false), (true, true)]
        {
            let policy = NetworkVisibility {
                developer_mode,
                bitcoin_blake2b_beta,
            };
            let filters = [NetworkFilter::All, NetworkFilter::Live, NetworkFilter::Test]
                .iter()
                .copied()
                .chain(ChainId::ALL.iter().copied().map(NetworkFilter::Network));
            for filter in filters {
                let normalized = policy.normalize_filter(filter);
                assert_eq!(policy.normalize_filter(normalized), normalized);
                for chain in ChainId::ALL {
                    if !policy.allows(chain) {
                        assert!(
                            !policy.matches(filter, chain),
                            "{:?} {:?} {:?}",
                            policy,
                            filter,
                            chain
                        );
                    }
                }
            }
            if !developer_mode {
                assert_eq!(
                    policy.normalize_filter(NetworkFilter::Live),
                    NetworkFilter::All
                );
                assert_eq!(
                    policy.normalize_filter(NetworkFilter::Test),
                    NetworkFilter::All
                );
            } else {
                assert!(policy.matches(NetworkFilter::Live, ChainId::Bitcoin));
                assert!(!policy.matches(NetworkFilter::Test, ChainId::Bitcoin));
                assert!(policy.matches(NetworkFilter::Test, ChainId::Testnet));
            }
            assert_eq!(
                policy.normalize_selection(ChainId::BitcoinBlake2bTestnet4),
                if developer_mode && bitcoin_blake2b_beta {
                    ChainId::BitcoinBlake2bTestnet4
                } else {
                    ChainId::Bitcoin
                }
            );
        }
    }

    #[test]
    fn flag_changes_do_not_mutate_the_source_catalog() {
        let catalog = ChainId::ALL.to_vec();
        let off = NetworkVisibility {
            developer_mode: false,
            bitcoin_blake2b_beta: false,
        };
        let on = NetworkVisibility {
            developer_mode: true,
            bitcoin_blake2b_beta: true,
        };
        assert_eq!(off.visible_networks(), vec![ChainId::Bitcoin]);
        assert_eq!(on.visible_networks(), catalog);
        assert_eq!(off.visible_networks(), vec![ChainId::Bitcoin]);
        assert_eq!(catalog, ChainId::ALL);
    }
}
