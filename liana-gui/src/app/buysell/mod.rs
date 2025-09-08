use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServiceProvider {
    AlchemyPay,
    Banxa,
    BinanceConnect,
    #[cfg(feature = "dev-onramp")]
    BinanceP2P,
    #[cfg(feature = "dev-meld")]
    BlockchainDotCom,
    BtcDirect,
    CoinbasePay,
    #[cfg(feature = "dev-onramp")]
    Coinify,
    #[cfg(feature = "dev-onramp")]
    Dfx,
    #[cfg(feature = "dev-onramp")]
    Fonbnk,
    #[cfg(feature = "dev-onramp")]
    GateConnect,
    #[cfg(feature = "dev-onramp")]
    GateFi,
    Guardarian,
    Koywe,
    #[cfg(feature = "dev-onramp")]
    LocalRamp,
    #[cfg(feature = "dev-meld")]
    Mesh,
    #[cfg(feature = "dev-meld")]
    Meso,
    #[cfg(feature = "dev-onramp")]
    Moonpay,
    #[cfg(feature = "dev-onramp")]
    Neocrypto,
    #[cfg(feature = "dev-onramp")]
    Onmeta,
    OnrampMoney,
    #[cfg(feature = "dev-meld")]
    Paybis,
    #[cfg(feature = "dev-onramp")]
    Revolut,
    #[cfg(feature = "dev-meld")]
    Transak,
}

impl ServiceProvider {
    pub fn as_str(&self) -> &'static str {
        match self {
            // universal providers
            ServiceProvider::AlchemyPay => "ALCHEMYPAY",
            ServiceProvider::Banxa => "BANXA",
            ServiceProvider::BinanceConnect => "BINANCECONNECT",
            ServiceProvider::BtcDirect => "BTCDIRECT",
            ServiceProvider::CoinbasePay => "COINBASEPAY",
            ServiceProvider::Guardarian => "GUARDARIAN",
            ServiceProvider::Koywe => "KOYWE",
            ServiceProvider::OnrampMoney => "ONRAMPMONEY",
            #[cfg(feature = "dev-meld")]
            meld_provider => match meld_provider {
                // meld exclusive providers
                ServiceProvider::BlockchainDotCom => "BLOCKCHAINDOTCOM",
                ServiceProvider::Mesh => "MESH",
                ServiceProvider::Meso => "MESO",
                ServiceProvider::Paybis => "PAYBIS",
                ServiceProvider::Transak => "TRANSAK",
                _ => unreachable!(),
            },

            #[cfg(feature = "dev-onramp")]
            onramper_provider => match onramper_provider {
                // onramper exclusive providers
                ServiceProvider::BinanceP2P => "BINANCEP2P",
                ServiceProvider::Coinify => "COINIFY",
                ServiceProvider::Dfx => "DFX",
                ServiceProvider::Fonbnk => "FONBNK",
                ServiceProvider::GateConnect => "GATECONNECT",
                ServiceProvider::GateFi => "GATEFI",
                ServiceProvider::LocalRamp => "LOCALRAMP",
                ServiceProvider::Moonpay => "MOONPAY",
                ServiceProvider::Neocrypto => "NEOCRYPTO",
                ServiceProvider::Onmeta => "ONMETA",
                ServiceProvider::Revolut => "REVOLUT",
                _ => unreachable!(),
            },
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            // universal providers
            ServiceProvider::AlchemyPay => "Alchemy Pay",
            ServiceProvider::Banxa => "Banxa",
            ServiceProvider::BinanceConnect => "Binance Connect",
            ServiceProvider::BtcDirect => "BTC Direct",
            ServiceProvider::CoinbasePay => "Coinbase Pay",
            ServiceProvider::Guardarian => "Guardarian",
                &ServiceProvider::OnrampMoney => "Onramp Money",

            ServiceProvider::Koywe => "Koywe",
            #[cfg(feature = "dev-meld")]
            meld_provider => match meld_provider {
                // meld exclusive providers
                ServiceProvider::BlockchainDotCom => "Blockchain.com",
                ServiceProvider::Mesh => "Mesh",
                ServiceProvider::Meso => "Meso",
                ServiceProvider::OnrampMoney => "Onramp Money",
                ServiceProvider::Paybis => "Paybis",
                ServiceProvider::Transak => "Transak",
                _ => unreachable!(),
            },
            #[cfg(feature = "dev-onramp")]
            onramper_provider => match onramper_provider {
                // onramper exclusive providers
                ServiceProvider::BinanceP2P => "Binance P2P",
                ServiceProvider::Coinify => "Coinify",
                ServiceProvider::Dfx => "DFX",
                ServiceProvider::Fonbnk => "Fonbnk",
                ServiceProvider::GateConnect => "GateConnect",
                ServiceProvider::GateFi => "Unlimit",
                ServiceProvider::LocalRamp => "LocalRamp",
                ServiceProvider::Moonpay => "MoonPay",
                ServiceProvider::Neocrypto => "Neocrypto",
                ServiceProvider::Onmeta => "Onmeta",
                ServiceProvider::Revolut => "Revolut",
                _ => unreachable!(),
            },
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

#[cfg(feature = "dev-onramp")]
pub mod onramper;
