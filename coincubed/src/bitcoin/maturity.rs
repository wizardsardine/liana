//! Coinbase maturity per chain identity.
//!
//! Bitcoin requires a coinbase output to be buried under 100 blocks before it
//! can be spent. Bitcoin Blake2b (BTCB2) kept Bitcoin's encoding but not this
//! rule, so the answer depends on [`ChainId`], which `bitcoin::Network` cannot
//! tell apart.
//!
//! # Source
//!
//! Knots tag `v29.5.1.knots20260508`:
//! `src/kernel/chainparams.cpp` (the parameters below),
//! `src/consensus/params.h` (`CoinbaseMaturityBeforeRolling`,
//! `CoinbaseMaturityHistoricalActiveAt`, `CoinbaseMaturityLongActiveAt`) and
//! `src/consensus/tx_verify.cpp` (`CoinbaseMaturityPolicy`, `CheckTxInputs`).
//!
//! # Which rule: policy, not consensus
//!
//! A Knots node checks a coinbase spend twice, with different depths:
//!
//! - **Consensus** (`CheckTxInputs` with `Params`, run by `ConnectBlock`) takes
//!   the spend height into account. On BTCB2 mainnet it locks rewards mined at
//!   973440..=979918 until height 979920 and requires 180 × 144 = 25920 blocks
//!   for rewards from 979919 onwards; every other reward needs 100.
//! - **Mempool policy** (`CheckTxInputs` from `AcceptToMemoryPool`, and
//!   `CoinbaseMaturityPolicy` when the mempool is trimmed after a reorg) only
//!   looks at the reward's height: below `CoinbaseMaturityLongStartHeight` it
//!   requires `CoinbaseMaturityBeforeRolling()` (6480 on mainnet, 3395 on
//!   testnet4), from there `CoinbaseMaturityLong`.
//!
//! The wallet broadcasts through a node's mempool, so a coin is only
//! spendable once that node's policy accepts it. Policy is never shallower
//! than consensus on either BTCB2 chain (the tests check this around every
//! boundary), so following policy is also consensus-safe. This module
//! therefore mirrors `Consensus::CoinbaseMaturityPolicy`.
//!
//! # RDTS expiry (mainnet)
//!
//! On BTCB2 mainnet the long rule covers a reward only while the median time
//! past of its block's *parent* is below the RDTS expiry, 2027-09-01 00:00 UTC.
//! Rewards covered before expiry keep the long depth afterwards; rewards mined
//! after it need 100. [`coinbase_maturity`] takes that parent MTP when known.
//! When it is not known it assumes the reward is covered, which can only make
//! a coin wait longer than the node requires, never shorter. The poller does not
//! look the MTP up yet, so after expiry new mainnet rewards would be held for
//! the long depth rather than 100 blocks; that needs fixing before 2027-09-01.
//!
//! # Regtest
//!
//! A Knots regtest node can be started with `-coinbasematuritylong*` options
//! that lengthen its maturity. Regtest here is the Bitcoin-family identity,
//! so it gets 100; a test exercising BTCB2 maturity on regtest would need its
//! own identity or configuration.

use coincube_core::chain::ChainId;

/// Bitcoin's coinbase maturity (`COINBASE_MATURITY` in Core and Knots). Every
/// chain requires at least this, so backends may use it as a first filter.
pub const COINBASE_MATURITY: i32 = 100;

/// BTCB2 mainnet parameters (Knots `CMainParams`).
pub mod btcb2_mainnet {
    /// `RdtsExpiryTime`: September 1st, 2027 00:00 UTC.
    pub const RDTS_EXPIRY_TIME: i64 = 1_819_756_800;
    /// `CoinbaseMaturityHistoricalStartHeight`.
    pub const HISTORICAL_START_HEIGHT: i32 = 973_440;
    /// `CoinbaseMaturityHistoricalReleaseHeight`.
    pub const HISTORICAL_RELEASE_HEIGHT: i32 = 979_920;
    /// `CoinbaseMaturityLongStartHeight` (also `...EnforceHeight`).
    pub const LONG_START_HEIGHT: i32 = 979_919;
    /// `CoinbaseMaturityLong`: about 180 days.
    pub const LONG_MATURITY: i32 = 180 * 144;
    /// `CoinbaseMaturityBeforeRolling()`: the historical window's length.
    pub const BEFORE_ROLLING_MATURITY: i32 = HISTORICAL_RELEASE_HEIGHT - HISTORICAL_START_HEIGHT;
}

/// BTCB2 testnet4 parameters (Knots `CTestNet4Params`). There is no
/// historical window and the long rule is not tied to RDTS expiry.
pub mod btcb2_testnet4 {
    /// `CoinbaseMaturityLongStartHeight`.
    pub const LONG_START_HEIGHT: i32 = 151_406;
    /// `CoinbaseMaturityLongEnforceHeight`. Only consensus uses it (policy does
    /// not), so outside the tests it is documentation.
    #[cfg_attr(not(test), allow(dead_code))]
    pub const LONG_ENFORCE_HEIGHT: i32 = 151_550;
    /// `CoinbaseMaturityLongReleaseHeight`.
    pub const LONG_RELEASE_HEIGHT: i32 = 154_801;
    /// `CoinbaseMaturityLong`, which is also `CoinbaseMaturityBeforeRolling()`
    /// because testnet4 has no historical window.
    pub const LONG_MATURITY: i32 = LONG_RELEASE_HEIGHT - LONG_START_HEIGHT;
}

/// How many blocks deep a coinbase output mined at `coin_height` must be
/// before a node on `chain` relays a transaction spending it.
///
/// `mining_parent_mtp` is the median time past of the parent of the block that
/// mined the reward, if known. Only BTCB2 mainnet uses it; `None` is treated as
/// before RDTS expiry (the conservative answer).
pub fn coinbase_maturity(chain: ChainId, coin_height: i32, mining_parent_mtp: Option<i64>) -> i32 {
    match chain {
        ChainId::Bitcoin
        | ChainId::Testnet
        | ChainId::Testnet4
        | ChainId::Signet
        | ChainId::Regtest => COINBASE_MATURITY,
        ChainId::BitcoinBlake2b => {
            use btcb2_mainnet::*;
            if mining_parent_mtp.is_some_and(|mtp| mtp >= RDTS_EXPIRY_TIME) {
                COINBASE_MATURITY
            } else if coin_height < LONG_START_HEIGHT {
                BEFORE_ROLLING_MATURITY
            } else {
                LONG_MATURITY
            }
        }
        // `CoinbaseMaturityLongUntilRdts` is false and there is no historical
        // window, so both branches of `CoinbaseMaturityPolicy` give the same
        // depth for every reward.
        ChainId::BitcoinBlake2bTestnet4 => btcb2_testnet4::LONG_MATURITY,
    }
}

/// Whether a coinbase output mined at `coin_height` can be spent in the block
/// after `tip_height`, i.e. whether a spend broadcast now is accepted.
///
/// Knots compares `spend_height - coin_height` against the maturity, with
/// `spend_height = tip_height + 1` for the mempool. That difference is the
/// coin's confirmation count at `tip_height`.
pub fn coinbase_is_mature(
    chain: ChainId,
    coin_height: i32,
    tip_height: i32,
    mining_parent_mtp: Option<i64>,
) -> bool {
    let confirmations = i64::from(tip_height) - i64::from(coin_height) + 1;
    confirmations >= i64::from(coinbase_maturity(chain, coin_height, mining_parent_mtp))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A test-only port of Knots' *consensus* check (`CheckTxInputs` with
    /// `Params` in `tx_verify.cpp`) for a reward before RDTS expiry: whether a
    /// coinbase mined at `coin_height` may be spent in a block at
    /// `spend_height`. Used to show that the policy depth above never admits
    /// a spend consensus would reject.
    fn consensus_allows(chain: ChainId, coin_height: i32, spend_height: i32) -> bool {
        // Before `start` the default `COINBASE_MATURITY` applies.
        let check = |long: i32, start: i32| {
            let required = if coin_height >= start {
                long
            } else {
                COINBASE_MATURITY
            };
            spend_height - coin_height >= required
        };
        match chain {
            ChainId::BitcoinBlake2b => {
                use btcb2_mainnet::*;
                let historical_active =
                    (HISTORICAL_START_HEIGHT..HISTORICAL_RELEASE_HEIGHT).contains(&spend_height);
                if historical_active && !check(BEFORE_ROLLING_MATURITY, HISTORICAL_START_HEIGHT) {
                    return false;
                }
                // `CoinbaseMaturityLongUntilRdts`: active from the enforce
                // height, with no release height.
                let start = if spend_height >= LONG_START_HEIGHT {
                    LONG_START_HEIGHT
                } else {
                    i32::MAX
                };
                check(LONG_MATURITY, start)
            }
            ChainId::BitcoinBlake2bTestnet4 => {
                use btcb2_testnet4::*;
                let start = if (LONG_ENFORCE_HEIGHT..LONG_RELEASE_HEIGHT).contains(&spend_height) {
                    LONG_START_HEIGHT
                } else {
                    i32::MAX
                };
                check(LONG_MATURITY, start)
            }
            _ => check(COINBASE_MATURITY, i32::MAX),
        }
    }

    /// The smallest tip height at which [`coinbase_is_mature`] says yes.
    fn first_mature_tip(chain: ChainId, coin_height: i32) -> i32 {
        coin_height + coinbase_maturity(chain, coin_height, None) - 1
    }

    #[test]
    fn knots_parameters() {
        assert_eq!(btcb2_mainnet::LONG_MATURITY, 25_920);
        assert_eq!(btcb2_mainnet::BEFORE_ROLLING_MATURITY, 6_480);
        assert_eq!(btcb2_testnet4::LONG_MATURITY, 3_395);
    }

    #[test]
    fn bitcoin_family_is_unchanged() {
        for chain in ChainId::ALL.iter().copied().filter(|c| !c.is_blake2b()) {
            for height in [0, 1, 151_406, 973_439, 973_440, 979_919, 2_000_000] {
                assert_eq!(coinbase_maturity(chain, height, None), 100, "{}", chain);
                assert_eq!(coinbase_maturity(chain, height, Some(i64::MAX)), 100);
            }
            assert!(!coinbase_is_mature(chain, 1_000, 1_098, None));
            assert!(coinbase_is_mature(chain, 1_000, 1_099, None));
        }
    }

    #[test]
    fn btcb2_mainnet_boundaries() {
        let chain = ChainId::BitcoinBlake2b;
        let m = |h| coinbase_maturity(chain, h, None);
        // Before the historical window the node's policy still asks for the
        // pre-rolling depth (consensus asks for only 100).
        assert_eq!(m(961_640), 6_480);
        assert_eq!(m(973_439), 6_480);
        // The historical window.
        assert_eq!(m(973_440), 6_480);
        assert_eq!(m(979_918), 6_480);
        // The rolling rule starts at 979919, one block before the historical
        // window is released.
        assert_eq!(m(979_919), 25_920);
        assert_eq!(m(979_920), 25_920);
        assert_eq!(m(2_000_000), 25_920);

        // Spendability one block either side of the threshold.
        assert!(!coinbase_is_mature(chain, 973_440, 979_918, None));
        assert!(coinbase_is_mature(chain, 973_440, 979_919, None));
        assert!(!coinbase_is_mature(chain, 979_919, 1_005_837, None));
        assert!(coinbase_is_mature(chain, 979_919, 1_005_838, None));
        // What the fixed rule got wrong: 100 confirmations is not enough.
        assert!(!coinbase_is_mature(chain, 980_000, 980_099, None));
    }

    #[test]
    fn btcb2_mainnet_rdts_expiry() {
        use btcb2_mainnet::RDTS_EXPIRY_TIME;
        let chain = ChainId::BitcoinBlake2b;
        // Covered while the mining block's parent MTP is below expiry...
        assert_eq!(
            coinbase_maturity(chain, 1_030_000, Some(RDTS_EXPIRY_TIME - 1)),
            25_920
        );
        // ...and not from the first block whose parent MTP reached it.
        assert_eq!(
            coinbase_maturity(chain, 1_030_000, Some(RDTS_EXPIRY_TIME)),
            100
        );
        // A covered reward keeps the long depth: it is judged by its own
        // block's parent MTP, whatever the time at the tip.
        assert!(!coinbase_is_mature(
            chain,
            1_030_000,
            1_030_200,
            Some(RDTS_EXPIRY_TIME - 1)
        ));
        // Unknown MTP is treated as covered.
        assert_eq!(coinbase_maturity(chain, 1_030_000, None), 25_920);
    }

    #[test]
    fn btcb2_testnet4_boundaries() {
        let chain = ChainId::BitcoinBlake2bTestnet4;
        // The node's policy uses the long depth for every reward, inside the
        // window or not, and ignores RDTS.
        for height in [
            150_308, 151_405, 151_406, 151_549, 151_550, 154_800, 154_801, 200_000,
        ] {
            assert_eq!(coinbase_maturity(chain, height, None), 3_395, "{}", height);
            assert_eq!(coinbase_maturity(chain, height, Some(i64::MAX)), 3_395);
        }
        assert!(!coinbase_is_mature(chain, 151_406, 154_799, None));
        assert!(coinbase_is_mature(chain, 151_406, 154_800, None));
    }

    /// Policy never admits a spend consensus rejects: for every reward height
    /// around each boundary, the first tip at which we call the coin mature
    /// is one where consensus accepts the spend in the next block, and it
    /// stays accepted afterwards.
    #[test]
    fn policy_is_never_shallower_than_consensus() {
        let boundaries = [
            (
                ChainId::BitcoinBlake2b,
                btcb2_mainnet::HISTORICAL_START_HEIGHT,
            ),
            (ChainId::BitcoinBlake2b, btcb2_mainnet::LONG_START_HEIGHT),
            (
                ChainId::BitcoinBlake2b,
                btcb2_mainnet::HISTORICAL_RELEASE_HEIGHT,
            ),
            (
                ChainId::BitcoinBlake2b,
                btcb2_mainnet::HISTORICAL_RELEASE_HEIGHT - btcb2_mainnet::BEFORE_ROLLING_MATURITY,
            ),
            (
                ChainId::BitcoinBlake2bTestnet4,
                btcb2_testnet4::LONG_START_HEIGHT,
            ),
            (
                ChainId::BitcoinBlake2bTestnet4,
                btcb2_testnet4::LONG_ENFORCE_HEIGHT,
            ),
            (
                ChainId::BitcoinBlake2bTestnet4,
                btcb2_testnet4::LONG_RELEASE_HEIGHT,
            ),
            (
                ChainId::BitcoinBlake2bTestnet4,
                btcb2_testnet4::LONG_ENFORCE_HEIGHT - btcb2_testnet4::LONG_MATURITY,
            ),
            (ChainId::Bitcoin, 840_000),
        ];
        for (chain, boundary) in boundaries {
            for coin_height in boundary - 120..boundary + 120 {
                let first = first_mature_tip(chain, coin_height);
                assert!(!coinbase_is_mature(chain, coin_height, first - 1, None));
                assert!(coinbase_is_mature(chain, coin_height, first, None));
                for tip in first..first + 200 {
                    assert!(
                        consensus_allows(chain, coin_height, tip + 1),
                        "{}: reward at {} called mature at tip {}",
                        chain,
                        coin_height,
                        tip
                    );
                }
            }
        }
    }

    /// The consensus port reproduces the windows Knots describes.
    #[test]
    fn consensus_port_matches_knots_windows() {
        let main = ChainId::BitcoinBlake2b;
        // A historical reward is locked until the window is released...
        assert!(!consensus_allows(main, 973_440, 979_919));
        assert!(consensus_allows(main, 973_440, 979_920));
        // ...and needs 100 blocks once it is.
        assert!(!consensus_allows(main, 979_900, 979_999));
        assert!(consensus_allows(main, 979_900, 980_000));
        // Rolling rewards need 25920.
        assert!(!consensus_allows(main, 979_919, 979_919 + 25_919));
        assert!(consensus_allows(main, 979_919, 979_919 + 25_920));
        // Before the window consensus asks for only 100.
        assert!(consensus_allows(main, 970_000, 970_100));

        let t4 = ChainId::BitcoinBlake2bTestnet4;
        // Window rewards are locked from enforcement until release.
        assert!(!consensus_allows(t4, 151_406, 151_550));
        assert!(!consensus_allows(t4, 151_406, 154_800));
        assert!(consensus_allows(t4, 151_406, 154_801));
    }
}
