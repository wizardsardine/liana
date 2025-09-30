use serde::{Deserialize, Serialize};
use std::fmt;

/// Service providers supported by Meld and/or Onramper
/// Note: Meld and Onramper have different provider support, so we keep all variants
/// and use them based on which aggregator (Meld/Onramper) is selected at runtime
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ServiceProvider {
    // Universal providers (supported by both)
    AlchemyPay,
    Banxa,
    BinanceConnect,
    #[default]
    BtcDirect,
    CoinbasePay,
    Guardarian,
    Koywe,
    OnrampMoney,

    // Meld-specific providers
    BlockchainDotCom,
    Mesh,
    Meso,
    Paybis,
    Transak,

    // Onramper-specific providers
    BinanceP2P,
    Coinify,
    Dfx,
    Fonbnk,
    GateConnect,
    GateFi,
    LocalRamp,
    Moonpay,
    Neocrypto,
    Onmeta,
    Revolut,
}

impl ServiceProvider {
    pub fn as_str(&self) -> &'static str {
        match self {
            ServiceProvider::AlchemyPay => "ALCHEMYPAY",
            ServiceProvider::Banxa => "BANXA",
            ServiceProvider::BinanceConnect => "BINANCECONNECT",
            ServiceProvider::BinanceP2P => "BINANCEP2P",
            ServiceProvider::BlockchainDotCom => "BLOCKCHAINDOTCOM",
            ServiceProvider::BtcDirect => "BTCDIRECT",
            ServiceProvider::CoinbasePay => "COINBASEPAY",
            ServiceProvider::Coinify => "COINIFY",
            ServiceProvider::Dfx => "DFX",
            ServiceProvider::Fonbnk => "FONBNK",
            ServiceProvider::GateConnect => "GATECONNECT",
            ServiceProvider::GateFi => "GATEFI",
            ServiceProvider::Guardarian => "GUARDARIAN",
            ServiceProvider::Koywe => "KOYWE",
            ServiceProvider::LocalRamp => "LOCALRAMP",
            ServiceProvider::Mesh => "MESH",
            ServiceProvider::Meso => "MESO",
            ServiceProvider::Moonpay => "MOONPAY",
            ServiceProvider::Neocrypto => "NEOCRYPTO",
            ServiceProvider::Onmeta => "ONMETA",
            ServiceProvider::OnrampMoney => "ONRAMPMONEY",
            ServiceProvider::Paybis => "PAYBIS",
            ServiceProvider::Revolut => "REVOLUT",
            ServiceProvider::Transak => "TRANSAK",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            ServiceProvider::AlchemyPay => "Alchemy Pay",
            ServiceProvider::Banxa => "Banxa",
            ServiceProvider::BinanceConnect => "Binance Connect",
            ServiceProvider::BinanceP2P => "Binance P2P",
            ServiceProvider::BlockchainDotCom => "Blockchain.com",
            ServiceProvider::BtcDirect => "BTC Direct",
            ServiceProvider::CoinbasePay => "Coinbase Pay",
            ServiceProvider::Coinify => "Coinify",
            ServiceProvider::Dfx => "DFX",
            ServiceProvider::Fonbnk => "Fonbnk",
            ServiceProvider::GateConnect => "GateConnect",
            ServiceProvider::GateFi => "Unlimit",
            ServiceProvider::Guardarian => "Guardarian",
            ServiceProvider::Koywe => "Koywe",
            ServiceProvider::LocalRamp => "LocalRamp",
            ServiceProvider::Mesh => "Mesh",
            ServiceProvider::Meso => "Meso",
            ServiceProvider::Moonpay => "MoonPay",
            ServiceProvider::Neocrypto => "Neocrypto",
            ServiceProvider::Onmeta => "Onmeta",
            ServiceProvider::OnrampMoney => "Onramp Money",
            ServiceProvider::Paybis => "Paybis",
            ServiceProvider::Revolut => "Revolut",
            ServiceProvider::Transak => "Transak",
        }
    }
}

impl fmt::Display for ServiceProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

pub mod meld;
pub mod onramper;
