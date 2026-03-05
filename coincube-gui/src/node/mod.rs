use coincubed::config::BitcoinBackend;

pub mod bitcoind;
pub mod electrum;
pub mod esplora;

#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub enum NodeType {
    Bitcoind,
    Electrum,
    Esplora,
}

impl From<&BitcoinBackend> for NodeType {
    fn from(bitcoin_backend: &BitcoinBackend) -> Self {
        match bitcoin_backend {
            BitcoinBackend::Bitcoind(_) => Self::Bitcoind,
            BitcoinBackend::Electrum(_) => Self::Electrum,
            BitcoinBackend::Esplora(_) => Self::Esplora,
        }
    }
}
