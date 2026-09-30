use miniscript::bitcoin::{Amount, OutPoint, Transaction};

/// Amount of a foreign input until we fetch it, see
/// https://github.com/wizardsardine/liana/issues/2336.
pub const UNKNOWN_AMOUNT: Amount = Amount::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Coin {
    pub outpoint: OutPoint,
    pub amount: Amount,
}

/// How a transaction is shown, along with its payments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransactionKind {
    /// Our outputs receiving the payments.
    Incoming(Vec<OutPoint>),
    SendToSelf,
    /// The external outputs paid.
    Outgoing(Vec<OutPoint>),
    /// Our outputs receiving the payments.
    PayjoinReceive(Vec<OutPoint>),
    /// The external outputs paid.
    PayjoinSend(Vec<OutPoint>),
}

impl TransactionKind {
    pub fn incoming_payments(&self) -> &[OutPoint] {
        match self {
            TransactionKind::Incoming(outpoints) | TransactionKind::PayjoinReceive(outpoints) => {
                outpoints
            }
            TransactionKind::SendToSelf
            | TransactionKind::Outgoing(_)
            | TransactionKind::PayjoinSend(_) => &[],
        }
    }

    pub fn outgoing_payments(&self) -> &[OutPoint] {
        match self {
            TransactionKind::Outgoing(outpoints) | TransactionKind::PayjoinSend(outpoints) => {
                outpoints
            }
            TransactionKind::SendToSelf
            | TransactionKind::Incoming(_)
            | TransactionKind::PayjoinReceive(_) => &[],
        }
    }

    pub fn is_incoming(&self) -> bool {
        matches!(
            self,
            TransactionKind::Incoming(_) | TransactionKind::PayjoinReceive(_)
        )
    }

    pub fn is_outgoing(&self) -> bool {
        matches!(
            self,
            TransactionKind::Outgoing(_) | TransactionKind::PayjoinSend(_)
        )
    }

    pub fn is_send_to_self(&self) -> bool {
        matches!(self, TransactionKind::SendToSelf)
    }

    pub fn single_payment(&self) -> Option<OutPoint> {
        match (self.incoming_payments(), self.outgoing_payments()) {
            ([outpoint], []) | ([], [outpoint]) => Some(*outpoint),
            _ => None,
        }
    }

    pub fn is_batch(&self) -> bool {
        self.outgoing_payments().len() > 1
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaymentKind {
    Outgoing,
    Incoming,
    /// A payment to self, which could be either from a self-transfer
    /// or a change output from an outgoing transaction.
    SendToSelf,
}

/// A transaction split between our coins and the external ones.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalletTransaction {
    pub owned_inputs: Vec<Coin>,
    /// Amounts are `UNKNOWN_AMOUNT`. Empty if no input is ours.
    pub external_inputs: Vec<Coin>,
    pub owned_outputs: Vec<Coin>,
    /// Empty if no input is ours.
    pub external_outputs: Vec<Coin>,
}

impl WalletTransaction {
    /// `owned_inputs` are the coins of ours `tx` spends, `owned_outputs` the outputs paying us.
    pub fn new(tx: &Transaction, owned_inputs: &[Coin], owned_outputs: &[OutPoint]) -> Self {
        let txid = tx.compute_txid();
        let (outputs, external_outputs): (Vec<Coin>, Vec<Coin>) = tx
            .output
            .iter()
            .enumerate()
            .map(|(vout, output)| Coin {
                outpoint: OutPoint::new(txid, vout as u32),
                amount: output.value,
            })
            .partition(|coin| owned_outputs.contains(&coin.outpoint));
        let (inputs, external_inputs): (Vec<Coin>, Vec<Coin>) = tx
            .input
            .iter()
            .map(|input| {
                owned_inputs
                    .iter()
                    .find(|coin| coin.outpoint == input.previous_output)
                    .copied()
                    .unwrap_or(Coin {
                        outpoint: input.previous_output,
                        amount: UNKNOWN_AMOUNT,
                    })
            })
            .partition(|coin| owned_inputs.contains(coin));
        if inputs.is_empty() {
            return Self {
                owned_inputs: inputs,
                external_inputs: Vec::new(),
                owned_outputs: outputs,
                external_outputs: Vec::new(),
            };
        }
        Self {
            owned_inputs: inputs,
            external_inputs,
            owned_outputs: outputs,
            external_outputs,
        }
    }

    pub fn is_incoming(&self) -> bool {
        self.kind().is_incoming()
    }

    pub fn is_outgoing(&self) -> bool {
        self.kind().is_outgoing()
    }

    pub fn kind(&self) -> TransactionKind {
        let owned_inputs = total(&self.owned_inputs);
        let owned_outputs = total(&self.owned_outputs);
        match (
            self.owned_inputs.is_empty(),
            self.external_inputs.is_empty(),
        ) {
            (true, _) => TransactionKind::Incoming(outpoints(&self.owned_outputs)),
            (false, false) if owned_outputs > owned_inputs => {
                TransactionKind::PayjoinReceive(outpoints(&self.owned_outputs))
            }
            (false, false) => TransactionKind::PayjoinSend(outpoints(&self.external_outputs)),
            (false, true) if total(&self.external_outputs) == Amount::ZERO => {
                TransactionKind::SendToSelf
            }
            (false, true) => TransactionKind::Outgoing(outpoints(&self.external_outputs)),
        }
    }

    /// Received amount if incoming, sent amount if outgoing, zero for a self-transfer. For a
    /// payjoin, what our coins gained or lost, fee included.
    pub fn amount(&self) -> Amount {
        let owned_inputs = total(&self.owned_inputs);
        let owned_outputs = total(&self.owned_outputs);
        match self.kind() {
            TransactionKind::Incoming(_) => owned_outputs,
            TransactionKind::SendToSelf => Amount::ZERO,
            TransactionKind::Outgoing(_) => total(&self.external_outputs),
            TransactionKind::PayjoinReceive(_) => owned_outputs - owned_inputs,
            TransactionKind::PayjoinSend(_) => owned_inputs - owned_outputs,
        }
    }

    /// `None` if no input is ours, as we do not pay it, or if the amount of an input is unknown.
    pub fn fee(&self) -> Option<Amount> {
        let unknown_input = self
            .external_inputs
            .iter()
            .any(|coin| coin.amount == UNKNOWN_AMOUNT);
        if self.owned_inputs.is_empty() || unknown_input {
            return None;
        }
        (total(&self.owned_inputs) + total(&self.external_inputs))
            .checked_sub(total(&self.owned_outputs) + total(&self.external_outputs))
    }

    /// `None` for an output that is not a payment of ours.
    pub fn payment_kind(&self, outpoint: &OutPoint) -> Option<PaymentKind> {
        let owned = self
            .owned_outputs
            .iter()
            .any(|coin| coin.outpoint == *outpoint);
        #[allow(unused_parens)]
        match self.kind() {
            TransactionKind::SendToSelf => Some(PaymentKind::SendToSelf),
            (TransactionKind::Outgoing(_) | TransactionKind::PayjoinSend(_)) if owned => {
                Some(PaymentKind::SendToSelf)
            }
            TransactionKind::Outgoing(_) | TransactionKind::PayjoinSend(_) => {
                Some(PaymentKind::Outgoing)
            }
            (TransactionKind::Incoming(_) | TransactionKind::PayjoinReceive(_)) if owned => {
                Some(PaymentKind::Incoming)
            }
            TransactionKind::Incoming(_) | TransactionKind::PayjoinReceive(_) => None,
        }
    }
}

fn total(coins: &[Coin]) -> Amount {
    coins.iter().map(|coin| coin.amount).sum()
}

fn outpoints(coins: &[Coin]) -> Vec<OutPoint> {
    coins.iter().map(|coin| coin.outpoint).collect()
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    use crate::miniscript::bitcoin::{
        absolute, transaction::Version, Address, Amount, Network, OutPoint, ScriptBuf, Sequence,
        Transaction, TxIn, TxOut, Txid, Witness,
    };

    const OUTPUT_AMOUNT: Amount = Amount::from_sat(10_000);

    fn address(index: u8) -> Address {
        Address::p2wsh(&ScriptBuf::from_bytes(vec![index]), Network::Bitcoin)
    }

    fn foreign_outpoint(index: u8) -> OutPoint {
        let txid = Txid::from_str(&format!("{index:0>64x}")).unwrap();
        OutPoint::new(txid, 0)
    }

    /// A transaction spending `inputs` to one output of `OUTPUT_AMOUNT` per address index.
    fn transaction(inputs: &[OutPoint], outputs: &[u8]) -> Transaction {
        Transaction {
            version: Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: inputs
                .iter()
                .map(|outpoint| TxIn {
                    previous_output: *outpoint,
                    script_sig: ScriptBuf::new(),
                    sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                    witness: Witness::new(),
                })
                .collect(),
            output: outputs
                .iter()
                .map(|index| TxOut {
                    value: OUTPUT_AMOUNT,
                    script_pubkey: address(*index).script_pubkey(),
                })
                .collect(),
        }
    }

    /// `tx` where the coins at `owned_inputs`, of `input_amount` sats each, and the outputs at
    /// `owned_vouts` are ours.
    fn wallet_tx(
        tx: &Transaction,
        owned_inputs: &[OutPoint],
        input_amount: u64,
        owned_vouts: &[u32],
    ) -> WalletTransaction {
        let txid = tx.compute_txid();
        let owned_inputs: Vec<Coin> = owned_inputs
            .iter()
            .map(|outpoint| Coin {
                outpoint: *outpoint,
                amount: Amount::from_sat(input_amount),
            })
            .collect();
        let owned_outputs: Vec<OutPoint> = owned_vouts
            .iter()
            .map(|vout| OutPoint::new(txid, *vout))
            .collect();
        WalletTransaction::new(tx, &owned_inputs, &owned_outputs)
    }

    #[test]
    fn incoming_single() {
        // Paying us on address 1, and a stranger on address 9.
        let tx = transaction(&[foreign_outpoint(1)], &[1, 9]);
        let wallet_tx = wallet_tx(&tx, &[], 0, &[0]);
        let ours = OutPoint::new(tx.compute_txid(), 0);
        assert_eq!(wallet_tx.kind(), TransactionKind::Incoming(vec![ours]));
        assert_eq!(wallet_tx.kind().single_payment(), Some(ours));
        assert!(wallet_tx.external_inputs.is_empty());
        assert!(wallet_tx.external_outputs.is_empty());
        assert_eq!(wallet_tx.amount(), Amount::from_sat(10_000));
        assert_eq!(wallet_tx.fee(), None);
    }

    #[test]
    fn incoming_several_payments_is_not_a_batch() {
        let tx = transaction(&[foreign_outpoint(1)], &[1, 2]);
        let txid = tx.compute_txid();
        let wallet_tx = wallet_tx(&tx, &[], 0, &[0, 1]);
        assert_eq!(
            wallet_tx.kind(),
            TransactionKind::Incoming(vec![OutPoint::new(txid, 0), OutPoint::new(txid, 1)])
        );
        assert!(!wallet_tx.kind().is_batch());
        assert_eq!(wallet_tx.kind().single_payment(), None);
        assert_eq!(wallet_tx.amount(), Amount::from_sat(20_000));
    }

    #[test]
    fn send_to_self() {
        let ours = foreign_outpoint(1);
        let tx = transaction(&[ours], &[4]);
        let wallet_tx = wallet_tx(&tx, &[ours], 11_000, &[0]);
        assert_eq!(wallet_tx.kind(), TransactionKind::SendToSelf);
        assert_eq!(wallet_tx.amount(), Amount::ZERO);
        assert_eq!(wallet_tx.fee(), Some(Amount::from_sat(1_000)));
    }

    #[test]
    fn outgoing_single() {
        // Paying a stranger on address 9, with change on address 3.
        let ours = foreign_outpoint(1);
        let tx = transaction(&[ours], &[9, 3]);
        let wallet_tx = wallet_tx(&tx, &[ours], 21_000, &[1]);
        assert_eq!(
            wallet_tx.kind(),
            TransactionKind::Outgoing(vec![OutPoint::new(tx.compute_txid(), 0)])
        );
        assert_eq!(wallet_tx.amount(), Amount::from_sat(10_000));
        assert_eq!(wallet_tx.fee(), Some(Amount::from_sat(1_000)));
    }

    #[test]
    fn outgoing_batch() {
        let ours = foreign_outpoint(1);
        let tx = transaction(&[ours], &[8, 9]);
        let txid = tx.compute_txid();
        let wallet_tx = wallet_tx(&tx, &[ours], 21_000, &[]);
        assert_eq!(
            wallet_tx.kind(),
            TransactionKind::Outgoing(vec![OutPoint::new(txid, 0), OutPoint::new(txid, 1)])
        );
        assert!(wallet_tx.kind().is_batch());
        assert_eq!(wallet_tx.amount(), Amount::from_sat(20_000));
    }

    #[test]
    fn payjoin_receive() {
        // Our 5_000 sats coin comes back with the payment on address 1, the sender gets their
        // change on address 9.
        let ours = foreign_outpoint(1);
        let tx = transaction(&[ours, foreign_outpoint(2)], &[1, 9]);
        let wallet_tx = wallet_tx(&tx, &[ours], 5_000, &[0]);
        assert_eq!(
            wallet_tx.kind(),
            TransactionKind::PayjoinReceive(vec![OutPoint::new(tx.compute_txid(), 0)])
        );
        assert_eq!(
            wallet_tx.external_inputs,
            vec![Coin {
                outpoint: foreign_outpoint(2),
                amount: UNKNOWN_AMOUNT,
            }]
        );
        assert_eq!(wallet_tx.amount(), Amount::from_sat(5_000));
        assert_eq!(wallet_tx.fee(), None);
    }

    #[test]
    fn payjoin_send() {
        // Paying the receiver on address 9, with change on address 3.
        let ours = foreign_outpoint(1);
        let tx = transaction(&[ours, foreign_outpoint(2)], &[9, 3]);
        let wallet_tx = wallet_tx(&tx, &[ours], 15_000, &[1]);
        assert_eq!(
            wallet_tx.kind(),
            TransactionKind::PayjoinSend(vec![OutPoint::new(tx.compute_txid(), 0)])
        );
        assert_eq!(wallet_tx.amount(), Amount::from_sat(5_000));
        assert_eq!(wallet_tx.fee(), None);
    }

    #[test]
    fn payment_kinds() {
        let tx = transaction(&[foreign_outpoint(1)], &[1, 9]);
        let txid = tx.compute_txid();
        let incoming = wallet_tx(&tx, &[], 0, &[0]);
        assert_eq!(
            incoming.payment_kind(&OutPoint::new(txid, 0)),
            Some(PaymentKind::Incoming)
        );
        assert_eq!(incoming.payment_kind(&OutPoint::new(txid, 1)), None);

        let ours = foreign_outpoint(1);
        let tx = transaction(&[ours], &[9, 3]);
        let txid = tx.compute_txid();
        let outgoing = wallet_tx(&tx, &[ours], 21_000, &[1]);
        assert_eq!(
            outgoing.payment_kind(&OutPoint::new(txid, 0)),
            Some(PaymentKind::Outgoing)
        );
        assert_eq!(
            outgoing.payment_kind(&OutPoint::new(txid, 1)),
            Some(PaymentKind::SendToSelf)
        );

        let tx = transaction(&[ours], &[4]);
        let to_self = wallet_tx(&tx, &[ours], 11_000, &[0]);
        assert_eq!(
            to_self.payment_kind(&OutPoint::new(tx.compute_txid(), 0)),
            Some(PaymentKind::SendToSelf)
        );

        let tx = transaction(&[ours, foreign_outpoint(2)], &[1, 9]);
        let txid = tx.compute_txid();
        let receive = wallet_tx(&tx, &[ours], 5_000, &[0]);
        assert_eq!(
            receive.payment_kind(&OutPoint::new(txid, 0)),
            Some(PaymentKind::Incoming)
        );
        assert_eq!(receive.payment_kind(&OutPoint::new(txid, 1)), None);

        let tx = transaction(&[ours, foreign_outpoint(2)], &[9, 3]);
        let txid = tx.compute_txid();
        let send = wallet_tx(&tx, &[ours], 15_000, &[1]);
        assert_eq!(
            send.payment_kind(&OutPoint::new(txid, 0)),
            Some(PaymentKind::Outgoing)
        );
        assert_eq!(
            send.payment_kind(&OutPoint::new(txid, 1)),
            Some(PaymentKind::SendToSelf)
        );
    }
}
