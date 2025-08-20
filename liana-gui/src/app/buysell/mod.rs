use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServiceProvider {
    AlchemyPay,
    Banxa,
    BinanceConnect,
    BlockchainDotCom,
    BtcDirect,
    CoinbasePay,
    Guardarian,
    Koywe,
    Mesh,
    Meso,
    OnrampMoney,
    Paybis,
    Transak,
}

impl ServiceProvider {
    pub fn as_str(&self) -> &'static str {
        match self {
            ServiceProvider::AlchemyPay => "ALCHEMYPAY",
            ServiceProvider::Banxa => "BANXA",
            ServiceProvider::BinanceConnect => "BINANCECONNECT",
            ServiceProvider::BlockchainDotCom => "BLOCKCHAINDOTCOM",
            ServiceProvider::BtcDirect => "BTCDIRECT",
            ServiceProvider::CoinbasePay => "COINBASEPAY",
            ServiceProvider::Guardarian => "GUARDARIAN",
            ServiceProvider::Koywe => "KOYWE",
            ServiceProvider::Mesh => "MESH",
            ServiceProvider::Meso => "MESO",
            ServiceProvider::OnrampMoney => "ONRAMPMONEY",
            ServiceProvider::Paybis => "PAYBIS",
            ServiceProvider::Transak => "TRANSAK",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            ServiceProvider::AlchemyPay => "Alchemy Pay",
            ServiceProvider::Banxa => "Banxa",
            ServiceProvider::BinanceConnect => "Binance Connect",
            ServiceProvider::BlockchainDotCom => "Blockchain.com",
            ServiceProvider::BtcDirect => "BTC Direct",
            ServiceProvider::CoinbasePay => "Coinbase Pay",
            ServiceProvider::Guardarian => "Guardarian",
            ServiceProvider::Koywe => "Koywe",
            ServiceProvider::Mesh => "Mesh",
            ServiceProvider::Meso => "Meso",
            ServiceProvider::OnrampMoney => "Onramp Money",
            ServiceProvider::Paybis => "Paybis",
            ServiceProvider::Transak => "Transak",
        }
    }
}

impl Default for ServiceProvider {
    fn default() -> Self {
        ServiceProvider::BtcDirect
    }
}

impl fmt::Display for ServiceProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

#[cfg(feature = "dev-meld")]
pub mod meld;
