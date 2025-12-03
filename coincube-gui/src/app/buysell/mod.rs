use serde::{Deserialize, Serialize};
use std::fmt;

/// Service providers supported by Onramper
/// Note: We keep all provider variants for potential future use
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ServiceProvider {
    // Onramper-supported providers
    AlchemyPay,
    Banxa,
    BinanceConnect,
    #[default]
    BtcDirect,
    CoinbasePay,
    Guardarian,
    Koywe,
    OnrampMoney,

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
            ServiceProvider::Moonpay => "MOONPAY",
            ServiceProvider::Neocrypto => "NEOCRYPTO",
            ServiceProvider::Onmeta => "ONMETA",
            ServiceProvider::OnrampMoney => "ONRAMPMONEY",
            ServiceProvider::Revolut => "REVOLUT",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            ServiceProvider::AlchemyPay => "Alchemy Pay",
            ServiceProvider::Banxa => "Banxa",
            ServiceProvider::BinanceConnect => "Binance Connect",
            ServiceProvider::BinanceP2P => "Binance P2P",
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
            ServiceProvider::Moonpay => "MoonPay",
            ServiceProvider::Neocrypto => "Neocrypto",
            ServiceProvider::Onmeta => "Onmeta",
            ServiceProvider::OnrampMoney => "Onramp Money",
            ServiceProvider::Revolut => "Revolut",
        }
    }
}

impl fmt::Display for ServiceProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

pub mod onramper;
