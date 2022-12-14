use crate::bitcoin::{d::BlockStats, BlockChainTip};

use miniscript::bitcoin;

/// Truncate the sync progress, rounding it up if it gets above 0.999. Note this also caps the
/// progress to 1.0, as bitcoind could temporarily return value >1.0 in getblockchaininfo's
/// "verificationprogress" field.
/// Bitcoind uses a guess for the value of verificationprogress. It will eventually get to
/// be 1, and we want to be less conservative.
pub fn roundup_progress(progress: f64) -> f64 {
    let precision = 10u64.pow(5) as f64;
    let progress_rounded = (progress * precision + 1.0) as u64;

    if progress_rounded >= precision as u64 {
        1.0
    } else {
        (progress * precision) as u64 as f64 / precision
    }
}

// As a standalone function to unit test it.
/// Get the last block of the chain before the given date by performing a binary search.
pub fn block_before_date<Fh, Fs>(
    target_timestamp: u32,
    chain_tip: BlockChainTip,
    mut get_hash: Fh,
    mut get_stats: Fs,
) -> Option<BlockChainTip>
where
    Fh: FnMut(i32) -> Option<bitcoin::BlockHash>,
    Fs: FnMut(bitcoin::BlockHash) -> BlockStats,
{
    log::debug!("Looking for the first block before {}", target_timestamp);

    let mut start_height = 0;
    let mut end_height = chain_tip.height;

    let genesis_stats = get_stats(get_hash(0).expect("Genesis hash"));
    let tip_stats = get_stats(chain_tip.hash);
    if !(genesis_stats.time..tip_stats.time).contains(&target_timestamp) {
        return None;
    }

    while start_height < end_height {
        log::debug!("Start: {}, end: {}", start_height, end_height,);
        let delta = end_height.checked_sub(start_height).unwrap();
        let current_height = start_height + delta.checked_div(2).unwrap();
        // We want the last block with a timestamp below, not the first with a higher one.
        let next_height = current_height.checked_add(1).unwrap();
        let next_stats = get_stats(get_hash(next_height)?);
        log::debug!("Current next block: {:?}", next_stats);

        if target_timestamp > next_stats.time {
            start_height = next_height;
        } else {
            assert!(current_height < end_height);
            end_height = current_height;
        }
    }

    // TODO: the timestamps in the chain are not strictly ordered. There could technically be a
    // timestamp above the target a bit down this height. I think we would be safe by scanning the
    // last 12 blocks and checking their timestamp is below the target. Would we?
    log::debug!("Result height: {}", start_height);
    Some(BlockChainTip {
        height: start_height,
        hash: get_hash(start_height)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    // The expected number of seconds in average between two blocks.
    const EXPECTED_BLOCK_INTERVAL_SECS: u32 = 600;

    // Inefficient dummy implementation of BitcoinD's self.get_block_hash
    fn get_hash(chain: &[(BlockChainTip, BlockStats)], height: i32) -> Option<bitcoin::BlockHash> {
        chain
            .iter()
            .find(|(tip, _)| tip.height == height)
            .map(|(tip, _)| tip.hash)
    }

    // Inefficient dummy implementation of BitcoinD's self.get_block_stats
    fn get_stats(chain: &[(BlockChainTip, BlockStats)], hash: bitcoin::BlockHash) -> BlockStats {
        chain
            .iter()
            .find(|(tip, _)| tip.hash == hash)
            .unwrap()
            .1
            .clone()
    }

    macro_rules! bh {
        ($h_str: literal) => {
            bitcoin::BlockHash::from_str($h_str).unwrap()
        };
    }

    // Create a dummy BlockStats struct with the given time
    fn create_stats(time: u32) -> BlockStats {
        BlockStats {
            height: 0,
            confirmations: 0,
            previous_blockhash: Some(bh!(
                "000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f"
            )),
            blockhash: bh!("000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f"),
            time,
            median_time_past: 0,
        }
    }

    #[test]
    fn blk_before_time() {
        // A timestamp after the tip's
        let dummy_chain = [
            (
                BlockChainTip {
                    height: 0,
                    hash: bh!("000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f"),
                },
                create_stats(1231006505),
            ),
            (
                BlockChainTip {
                    height: 761683,
                    hash: bh!("0000000000000000000560bb21fbab991fe5f7d9a949eb424f9be3c34a55a54f"),
                },
                create_stats(1667558116),
            ),
        ];
        assert!(block_before_date(
            dummy_chain[0].1.time + 1,
            dummy_chain[0].0,
            |h| get_hash(&dummy_chain, h),
            |h| get_stats(&dummy_chain, h),
        )
        .is_none());

        // A timestamp before the genesis
        let dummy_chain = [
            (
                BlockChainTip {
                    height: 0,
                    hash: bh!("000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f"),
                },
                create_stats(1231006505),
            ),
            (
                BlockChainTip {
                    height: 761683,
                    hash: bh!("0000000000000000000560bb21fbab991fe5f7d9a949eb424f9be3c34a55a54f"),
                },
                create_stats(1667558116),
            ),
        ];
        assert!(block_before_date(
            dummy_chain[0].1.time - 1,
            dummy_chain[0].0,
            |h| get_hash(&dummy_chain, h),
            |h| get_stats(&dummy_chain, h),
        )
        .is_none());

        // Simulate and detail a full binary search through a dummy chain.
        let target_timestamp = 1531006505;
        let dummy_chain = [
            // Genesis: will be queried at step 0 (0, 761_683)
            (
                BlockChainTip {
                    height: 0,
                    hash: bh!("000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f"),
                },
                create_stats(1231006505),
            ),
            // Step 1 (0, 761_683): timestamp too low.
            (
                BlockChainTip {
                    height: 380_842,
                    hash: bh!("000000000000000005c0655db17fde80f67ff0502a62b7250ed2685619d19ba3"),
                },
                create_stats(target_timestamp - EXPECTED_BLOCK_INTERVAL_SECS * 10),
            ),
            // Step 4 (380_842, 476_052): timestamp too low.
            (
                BlockChainTip {
                    height: 428_448,
                    hash: bh!("000000000000000005c0655db17fde80f67ff0502a62b7250ed2685619d19ba4"),
                },
                create_stats(target_timestamp - EXPECTED_BLOCK_INTERVAL_SECS * 9),
            ),
            // Step 6 (428_448, 452_250): timestamp too low.
            (
                BlockChainTip {
                    height: 440_350,
                    hash: bh!("000000000000000005c0655db17fde80f67ff0502a62b7250ed2685619d19ba5"),
                },
                create_stats(target_timestamp - EXPECTED_BLOCK_INTERVAL_SECS * 8),
            ),
            // Step 7 (440_350, 452_250): timestamp too low.
            (
                BlockChainTip {
                    height: 446_301,
                    hash: bh!("000000000000000005c0655db17fde80f67ff0502a62b7250ed2685619d19ba6"),
                },
                create_stats(target_timestamp - EXPECTED_BLOCK_INTERVAL_SECS * 7),
            ),
            // Step 8 (446_301, 452_250): timestamp too low.
            (
                BlockChainTip {
                    height: 449_276,
                    hash: bh!("000000000000000005c0655db17fde80f67ff0502a62b7250ed2685619d19ba7"),
                },
                create_stats(target_timestamp - EXPECTED_BLOCK_INTERVAL_SECS * 6),
            ),
            // Step 9 (449_276, 452_250): timestamp too low.
            (
                BlockChainTip {
                    height: 450_764,
                    hash: bh!("000000000000000005c0655db17fde80f67ff0502a62b7250ed2685619d19ba8"),
                },
                create_stats(target_timestamp - EXPECTED_BLOCK_INTERVAL_SECS * 5),
            ),
            // Step 10 (450_764, 452_250): timestamp too low.
            (
                BlockChainTip {
                    height: 451_508,
                    hash: bh!("000000000000000005c0655db17fde80f67ff0502a62b7250ed2685619d19ba9"),
                },
                create_stats(target_timestamp - EXPECTED_BLOCK_INTERVAL_SECS * 4),
            ),
            // Step 11 (451_508, 452_250): timestamp too low (and equal to the previous one).
            (
                BlockChainTip {
                    height: 451_880,
                    hash: bh!("000000000000000005c0655db17fde80f67ff0502a62b7250ed2685619d19bab"),
                },
                create_stats(target_timestamp - EXPECTED_BLOCK_INTERVAL_SECS * 4),
            ),
            // Step 12 (451_880, 452_250): timestamp too low.
            (
                BlockChainTip {
                    height: 452_066,
                    hash: bh!("000000000000000005c0655db17fde80f67ff0502a62b7250ed2685619d19bac"),
                },
                create_stats(target_timestamp - EXPECTED_BLOCK_INTERVAL_SECS * 3),
            ),
            // Step 13 (452_066, 452_250): timestamp too low.
            (
                BlockChainTip {
                    height: 452_159,
                    hash: bh!("000000000000000005c0655db17fde80f67ff0502a62b7250ed2685619d19bad"),
                },
                create_stats(target_timestamp - EXPECTED_BLOCK_INTERVAL_SECS * 2),
            ),
            // Step 14 (452_159, 452_250): timestamp too low.
            (
                BlockChainTip {
                    height: 452_205,
                    hash: bh!("000000000000000005c0655db17fde80f67ff0502a62b7250ed2685619d19bae"),
                },
                create_stats(target_timestamp - EXPECTED_BLOCK_INTERVAL_SECS),
            ),
            // Step 15 (452_205, 452_250): timestamp too low.
            (
                BlockChainTip {
                    height: 452_228,
                    hash: bh!("000000000000000005c0655db17fde80f67ff0502a62b7250ed2685619d19baf"),
                },
                create_stats(target_timestamp - EXPECTED_BLOCK_INTERVAL_SECS),
            ),
            // Step 17 (452_228, 452_239): timestamp too low.
            (
                BlockChainTip {
                    height: 452_234,
                    hash: bh!("000000000000000005c0655db17fde80f67ff0502a62b7250ed2685619d19bb0"),
                },
                create_stats(target_timestamp - EXPECTED_BLOCK_INTERVAL_SECS / 2),
            ),
            // Step 18 (452_234, 452_239): timestamp too low.
            (
                BlockChainTip {
                    height: 452_237,
                    hash: bh!("000000000000000005c0655db17fde80f67ff0502a62b7250ed2685619d19bb1"),
                },
                create_stats(target_timestamp - EXPECTED_BLOCK_INTERVAL_SECS / 4),
            ),
            // Step 21 (452_237, 452_238): timestamp too low.
            (
                BlockChainTip {
                    height: 452_238,
                    hash: bh!("000000000000000005c0655db17fde80f67ff0502a62b7250ed2685619d19bb2"),
                },
                create_stats(target_timestamp - 1),
            ),
            // Step 20 (452_237, 452_239): timestamp too high (first block at this timestamp).
            (
                BlockChainTip {
                    height: 452_239,
                    hash: bh!("000000000000000005c0655db17fde80f67ff0502a62b7250ed2685619d19bb3"),
                },
                create_stats(target_timestamp),
            ),
            // Step 16 (452_228, 452_250): timestamp too high (timestamp don't necessarily
            // increase per block height).
            (
                BlockChainTip {
                    height: 452_240,
                    hash: bh!("000000000000000005c0655db17fde80f67ff0502a62b7250ed2685619d19bb4"),
                },
                create_stats(target_timestamp + 1),
            ),
            // Step 5 (428_448, 476_052): timestamp too high (again equal! That's possible).
            (
                BlockChainTip {
                    height: 452_251,
                    hash: bh!("000000000000000005c0655db17fde80f67ff0502a62b7250ed2685619d19bb5"),
                },
                create_stats(target_timestamp),
            ),
            // Step 3 (380_842, 571_262): timestamp too high (because equal).
            (
                BlockChainTip {
                    height: 476_053,
                    hash: bh!("000000000000000005c0655db17fde80f67ff0502a62b7250ed2685619d19bb6"),
                },
                create_stats(target_timestamp),
            ),
            // Step 2 (380_842, 761_683): timestamp too high.
            (
                BlockChainTip {
                    height: 571_263,
                    hash: bh!("000000000000000005c0655db17fde80f67ff0502a62b7250ed2685619d19bb7"),
                },
                create_stats(target_timestamp + EXPECTED_BLOCK_INTERVAL_SECS),
            ),
            // Tip: will be queried at step 0
            (
                BlockChainTip {
                    height: 761_683,
                    hash: bh!("0000000000000000000560bb21fbab991fe5f7d9a949eb424f9be3c34a55a54f"),
                },
                create_stats(1667558116),
            ),
        ];
        assert_eq!(
            block_before_date(
                target_timestamp,
                dummy_chain[dummy_chain.len() - 1].0,
                |h| get_hash(&dummy_chain, h),
                |h| get_stats(&dummy_chain, h),
            )
            .unwrap(),
            // Step 21 above
            BlockChainTip {
                height: 452_238,
                hash: bh!("000000000000000005c0655db17fde80f67ff0502a62b7250ed2685619d19bb2"),
            }
        );
    }

    #[test]
    fn bitcoind_roundup_progress() {
        assert_eq!(roundup_progress(0.6), 0.6);
        assert_eq!(roundup_progress(0.67891), 0.67891);
        assert_eq!(roundup_progress(0.98), 0.98);
        assert_eq!(roundup_progress(0.998), 0.998);
        assert_eq!(roundup_progress(0.9997), 0.9997);
        assert_eq!(roundup_progress(0.9476), 0.9476);
        assert_eq!(roundup_progress(0.99998), 0.99998);
        assert_eq!(roundup_progress(0.999998), 1.0);
    }
}
