//! Coins spent by a transaction this daemon broadcast, held out of automatic
//! coin selection until the poller has recorded that transaction.
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
    sync::{Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};

use miniscript::bitcoin::{OutPoint, Transaction, Txid};

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
    /// The map has no invariant a panic could break halfway, so a poisoned
    /// lock is still good to use. Refusing it would fail every later broadcast
    /// after its transaction was sent.
    fn lock(&self) -> MutexGuard<'_, HashMap<OutPoint, Hold>> {
        self.held.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Hold the inputs of a transaction the network has just accepted.
    ///
    /// The network accepting `tx` means any transaction we broadcast earlier
    /// that shares an input with it has been replaced. That one's holds are
    /// dropped, since the inputs `tx` does not spend are free again.
    pub(crate) fn hold_inputs_of(&self, tx: &Transaction, now: Instant) {
        let spender = tx.compute_txid();
        let mut held = self.lock();
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
    /// `saved_txids` returns the transactions the wallet database has stored.
    /// The poller stores a spending transaction when it first records the
    /// spend, and keeps it. So a hold ends, for good, once its transaction is
    /// among them: from then on the poller keeps the coin's state, including
    /// freeing it again if the spend is dropped or replaced. What the database
    /// says of the coin itself does not end a hold: a coin still marked as
    /// spent by the transaction this one replaced is not yet recorded as spent
    /// by this one.
    ///
    /// A hold also ends after `max_age`, so a transaction the backend never
    /// shows cannot keep a coin out of selection forever.
    pub(crate) fn held(
        &self,
        now: Instant,
        max_age: Duration,
        saved_txids: impl FnOnce() -> HashSet<Txid>,
    ) -> HashSet<OutPoint> {
        let mut held = self.lock();
        if held.is_empty() {
            return HashSet::new();
        }
        let saved = saved_txids();
        held.retain(|outpoint, hold| {
            if saved.contains(&hold.spender) {
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
        absolute, hashes::Hash, transaction, ScriptBuf, Sequence, TxIn, Witness,
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

    /// A wallet database that has stored exactly `txs`.
    fn saved(txs: &[&Transaction]) -> impl FnOnce() -> HashSet<Txid> {
        let txids: HashSet<Txid> = txs.iter().map(|tx| tx.compute_txid()).collect();
        move || txids
    }

    fn set(outpoints: &[OutPoint]) -> HashSet<OutPoint> {
        outpoints.iter().copied().collect()
    }

    #[test]
    fn nothing_is_held_and_the_wallet_is_not_read_before_a_broadcast() {
        let holds = BroadcastHolds::default();
        let held = holds.held(Instant::now(), MAX_AGE, || {
            panic!("no hold, so no reason to read the wallet")
        });
        assert!(held.is_empty());
    }

    #[test]
    fn inputs_are_held_while_the_wallet_has_not_stored_the_transaction() {
        let (a, b) = (outpoint(1), outpoint(2));
        let holds = BroadcastHolds::default();
        let now = Instant::now();
        holds.hold_inputs_of(&spend_of(&[a, b], 0), now);
        let unrelated = spend_of(&[outpoint(3)], 0);
        assert_eq!(holds.held(now, MAX_AGE, saved(&[&unrelated])), set(&[a, b]));
        // Still held on a later look, short of the maximum age.
        let later = now + MAX_AGE - Duration::from_secs(1);
        assert_eq!(holds.held(later, MAX_AGE, saved(&[])), set(&[a, b]));
    }

    /// Once the poller has recorded the transaction the coins' state is the
    /// poller's to keep, and the holds do not come back.
    #[test]
    fn a_hold_ends_for_good_once_the_wallet_stores_the_transaction() {
        let (a, b, c) = (outpoint(1), outpoint(2), outpoint(3));
        let holds = BroadcastHolds::default();
        let now = Instant::now();
        let first = spend_of(&[a, b], 0);
        holds.hold_inputs_of(&first, now);
        holds.hold_inputs_of(&spend_of(&[c], 0), now);
        assert_eq!(holds.held(now, MAX_AGE, saved(&[&first])), set(&[c]));
        assert_eq!(holds.held(now, MAX_AGE, saved(&[])), set(&[c]));
    }

    #[test]
    fn a_hold_ends_at_the_maximum_age() {
        let a = outpoint(1);
        let holds = BroadcastHolds::default();
        let now = Instant::now();
        holds.hold_inputs_of(&spend_of(&[a], 0), now);
        assert!(holds.held(now + MAX_AGE, MAX_AGE, saved(&[])).is_empty());
        assert!(holds.held(now, MAX_AGE, saved(&[])).is_empty());
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
        assert_eq!(holds.held(now, MAX_AGE, saved(&[])), set(&[a, c]));
    }

    /// The wallet having stored the replaced transaction says nothing of the
    /// replacement: its inputs stay held until the replacement is stored.
    #[test]
    fn a_replacement_is_held_while_the_wallet_only_knows_what_it_replaced() {
        let (a, b) = (outpoint(1), outpoint(2));
        let holds = BroadcastHolds::default();
        let now = Instant::now();
        let first = spend_of(&[a, b], 0);
        let replacement = spend_of(&[a, b], 1);
        holds.hold_inputs_of(&first, now);
        assert!(holds.held(now, MAX_AGE, saved(&[&first])).is_empty());
        holds.hold_inputs_of(&replacement, now);
        assert_eq!(holds.held(now, MAX_AGE, saved(&[&first])), set(&[a, b]));
        assert!(holds
            .held(now, MAX_AGE, saved(&[&first, &replacement]))
            .is_empty());
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
        assert_eq!(holds.held(now + MAX_AGE, MAX_AGE, saved(&[])), set(&[a]));
    }

    #[test]
    fn a_panic_while_reading_the_wallet_does_not_disable_the_holds() {
        let a = outpoint(1);
        let holds = BroadcastHolds::default();
        let now = Instant::now();
        holds.hold_inputs_of(&spend_of(&[a], 0), now);
        let read = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            holds.held(now, MAX_AGE, || panic!("the database failed"))
        }));
        assert!(read.is_err());
        holds.hold_inputs_of(&spend_of(&[outpoint(2)], 0), now);
        assert_eq!(holds.held(now, MAX_AGE, saved(&[])), set(&[a, outpoint(2)]));
    }
}
