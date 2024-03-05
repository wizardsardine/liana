use liana::miniscript::bitcoin::{OutPoint, Txid};
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Menu {
    Home,
    Receive,
    PSBTs,
    Transactions,
    TransactionPreSelected(Txid),
    Settings,
    Coins,
    CreateSpendTx,
    Recovery,
    RefreshCoins(Vec<OutPoint>),
    PsbtPreSelected(Txid),
}
