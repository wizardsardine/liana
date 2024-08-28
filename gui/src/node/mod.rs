pub mod bitcoind;

#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub enum NodeType {
    Bitcoind,
}
