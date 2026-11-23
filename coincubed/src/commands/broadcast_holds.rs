//! Coins spent by a transaction this daemon broadcast, held out of automatic
//! coin selection until the wallet database records that they are being spent.
//!
//! The database learns of a spend from the poller, which reads it back from the
//! Bitcoin backend. Between the network accepting a transaction and the poller
//! recording it, the database still lists the transaction's inputs as unspent,
//! and a spend created with automatic coin selection could pick one of them and
//! conflict with the transaction just sent. How long that lasts depends on the
//! backend: about one poll on bitcoind and Electrum, and on Esplora as long as
//! the server takes to show the transaction to a poll that walks the wallet.
//!
//! A hold covers that gap and nothing more. The poller stays the only writer of
//! coin state, so a hold changes neither what the database stores nor what
//! `listcoins` reports, and it is not kept across a restart.

use std::{
    collections::{HashMap, HashSet},
    sync::Mutex,
    time::{Duration, Instant},
};

use miniscript::bitcoin::{OutPoint, Transaction, Txid};

use crate::database::Coin;

#[derive(Debug, Clone, Copy)]
struct Hold {
    /// The transaction we broadcast that spends the coin.
    spender: Txid,
    since: Instant,
}

/// The coins currently held, by outpoint.
#[derive(Debug, Default)]
pub(crate) struct BroadcastHolds {
    held: Mutex<HashMap<OutPoint, Hold>>,
}

impl BroadcastHolds {
    /// Hold the inputs of a transaction the network has just accepted.
    ///
    /// The network accepting `tx` means any transaction we broadcast earlier
    /// that shares an input with it has been replaced. That one's holds are
    /// dropped, since the inputs `tx` does not spend are free again.
    pub(crate) fn hold_inputs_of(&self, tx: &Transaction, now: Instant) {
        let spender = tx.compute_txid();
        let mut held = self.held.lock().expect("no panic while holding the lock");
        let replaced: HashSet<Txid> = tx
            .input
            .iter()
            .filter_map(|txin| held.get(&txin.previous_output))
            .map(|hold| hold.spender)
            .filter(|earlier| *earlier != spender)
            .collect();
        held.retain(|_, hold| !replaced.contains(&hold.spender));
        for txin in &tx.input {
            held.insert(
                txin.previous_output,
                Hold {
                    spender,
                    since: now,
                },
            );
        }
    }

    /// The coins automatic coin selection must leave alone.
    ///
    /// `wallet_coins` returns the wallet's coins among the given outpoints,
    /// whatever their status. A hold ends, for good, once the wallet records a
    /// spender for the coin or no longer has the coin: from then on the poller
    /// keeps the coin's state, including freeing it again if the spend is
    /// dropped or replaced. A hold also ends after `max_age`, so a transaction
    /// the backend never shows cannot keep a coin out of selection forever.
    pub(crate) fn held(
        &self,
        now: Instant,
        max_age: Duration,
        wallet_coins: impl FnOnce(&[OutPoint]) -> HashMap<OutPoint, Coin>,
    ) -> HashSet<OutPoint> {
        let mut held = self.held.lock().expect("no panic while holding the lock");
        if held.is_empty() {
            return HashSet::new();
        }
        let outpoints: Vec<OutPoint> = held.keys().copied().collect();
        let coins = wallet_coins(&outpoints);
        held.retain(|outpoint, hold| {
            if !coins.get(outpoint).is_some_and(|coin| !coin.is_spent()) {
                return false;
            }
            if now.saturating_duration_since(hold.since) >= max_age {
                log::warn!(
                    "The wallet still has no record of transaction '{}' spending coin '{}' \
                     {} seconds after its broadcast. Coin selection may use the coin again.",
                    hold.spender,
                    outpoint,
                    max_age.as_secs(),
                );
                return false;
            }
            true
        });
        held.keys().copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use miniscript::bitcoin::{
        absolute, hashes::Hash, transaction, Amount, ScriptBuf, Sequence, TxIn, Witness,
    };

    const MAX_AGE: Duration = Duration::from_secs(600);

    fn outpoint(byte: u8) -> OutPoint {
        OutPoint {
            txid: Txid::from_byte_array([byte; 32]),
            vout: 0,
        }
    }

    /// A transaction spending `inputs`. `tag` tells two spends of the same
    /// inputs apart.
    fn spend_of(inputs: &[OutPoint], tag: u32) -> Transaction {
        Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::from_consensus(tag),
            input: inputs
                .iter()
                .map(|previous_output| TxIn {
                    previous_output: *previous_output,
                    script_sig: ScriptBuf::new(),
                    sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                    witness: Witness::new(),
                })
                .collect(),
            output: Vec::new(),
        }
    }

    fn coin(outpoint: OutPoint, spend_txid: Option<Txid>) -> Coin {
        Coin {
            outpoint,
            is_immature: false,
            block_info: None,
            amount: Amount::from_sat(10_000),
            derivation_index: 0.into(),
            is_change: false,
            spend_txid,
            spend_block: None,
            is_from_self: false,
        }
    }

    /// A wallet holding exactly `coins`, as the lookup `held` asks for.
    fn wallet(coins: &[Coin]) -> impl FnOnce(&[OutPoint]) -> HashMap<OutPoint, Coin> + '_ {
        move |outpoints| {
            coins
                .iter()
                .filter(|coin| outpoints.contains(&coin.outpoint))
                .map(|coin| (coin.outpoint, *coin))
                .collect()
        }
    }

    fn set(outpoints: &[OutPoint]) -> HashSet<OutPoint> {
        outpoints.iter().copied().collect()
    }

    #[test]
    fn nothing_is_held_and_the_wallet_is_not_read_before_a_broadcast() {
        let holds = BroadcastHolds::default();
        let held = holds.held(Instant::now(), MAX_AGE, |_| {
            panic!("no hold, so no reason to read the wallet")
        });
        assert!(held.is_empty());
    }

    #[test]
    fn inputs_are_held_while_the_wallet_shows_them_unspent() {
        let (a, b) = (outpoint(1), outpoint(2));
        let holds = BroadcastHolds::default();
        let now = Instant::now();
        holds.hold_inputs_of(&spend_of(&[a, b], 0), now);
        let unspent = [coin(a, None), coin(b, None), coin(outpoint(3), None)];
        assert_eq!(holds.held(now, MAX_AGE, wallet(&unspent)), set(&[a, b]));
        // Still held on a later look, short of the maximum age.
        let later = now + MAX_AGE - Duration::from_secs(1);
        assert_eq!(holds.held(later, MAX_AGE, wallet(&unspent)), set(&[a, b]));
    }

    /// Once the poller has recorded a spender the coin's state is the
    /// poller's to keep. If it later frees the coin (the spend was dropped
    /// from the mempool), the hold must not come back.
    #[test]
    fn a_hold_ends_for_good_once_the_wallet_records_a_spender() {
        let (a, b) = (outpoint(1), outpoint(2));
        let holds = BroadcastHolds::default();
        let now = Instant::now();
        let tx = spend_of(&[a, b], 0);
        holds.hold_inputs_of(&tx, now);
        let recorded = [coin(a, Some(tx.compute_txid())), coin(b, None)];
        assert_eq!(holds.held(now, MAX_AGE, wallet(&recorded)), set(&[b]));
        let freed = [coin(a, None), coin(b, None)];
        assert_eq!(holds.held(now, MAX_AGE, wallet(&freed)), set(&[b]));
    }

    #[test]
    fn a_hold_ends_when_the_wallet_no_longer_has_the_coin() {
        let a = outpoint(1);
        let holds = BroadcastHolds::default();
        let now = Instant::now();
        holds.hold_inputs_of(&spend_of(&[a], 0), now);
        assert!(holds.held(now, MAX_AGE, wallet(&[])).is_empty());
        assert!(holds
            .held(now, MAX_AGE, wallet(&[coin(a, None)]))
            .is_empty());
    }

    #[test]
    fn a_hold_ends_at_the_maximum_age() {
        let a = outpoint(1);
        let holds = BroadcastHolds::default();
        let now = Instant::now();
        holds.hold_inputs_of(&spend_of(&[a], 0), now);
        let unspent = [coin(a, None)];
        assert!(holds
            .held(now + MAX_AGE, MAX_AGE, wallet(&unspent))
            .is_empty());
        assert!(holds.held(now, MAX_AGE, wallet(&unspent)).is_empty());
    }

    /// A replacement that spends only some of the first transaction's inputs
    /// (a cancel) frees the others at once.
    #[test]
    fn a_replacement_frees_the_inputs_it_does_not_spend() {
        let (a, b, c) = (outpoint(1), outpoint(2), outpoint(3));
        let holds = BroadcastHolds::default();
        let now = Instant::now();
        holds.hold_inputs_of(&spend_of(&[a, b], 0), now);
        holds.hold_inputs_of(&spend_of(&[c], 0), now);
        holds.hold_inputs_of(&spend_of(&[a], 1), now);
        let unspent = [coin(a, None), coin(b, None), coin(c, None)];
        assert_eq!(holds.held(now, MAX_AGE, wallet(&unspent)), set(&[a, c]));
    }

    #[test]
    fn broadcasting_the_same_transaction_again_restarts_its_hold() {
        let a = outpoint(1);
        let holds = BroadcastHolds::default();
        let now = Instant::now();
        let tx = spend_of(&[a], 0);
        holds.hold_inputs_of(&tx, now);
        let again = now + MAX_AGE - Duration::from_secs(1);
        holds.hold_inputs_of(&tx, again);
        let unspent = [coin(a, None)];
        assert_eq!(
            holds.held(now + MAX_AGE, MAX_AGE, wallet(&unspent)),
            set(&[a])
        );
    }
}
