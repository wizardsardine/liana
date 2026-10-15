use std::time::{Duration, SystemTime, SystemTimeError, UNIX_EPOCH};

use liana::miniscript::bitcoin::{
    bip32::{ChildNumber, DerivationPath},
    Network,
};

pub mod serde;
pub mod subscription;

#[cfg(test)]
pub mod sandbox;

#[cfg(test)]
pub mod mock;

/// Returns the current time as a [`Duration`] since the UNIX epoch.
pub fn now() -> Duration {
    now_fallible().expect("cannot fail")
}

/// Faliible version of [`now`].
pub fn now_fallible() -> Result<Duration, SystemTimeError> {
    SystemTime::now().duration_since(UNIX_EPOCH)
}

pub fn default_derivation_path(network: Network) -> DerivationPath {
    derivation_path(network, ChildNumber::Hardened { index: 0 })
}

pub fn coin_type(network: Network) -> u32 {
    if network == Network::Bitcoin {
        0
    } else {
        1
    }
}

pub fn derivation_path(network: Network, account: ChildNumber) -> DerivationPath {
    assert!(account.is_hardened());
    vec![
        ChildNumber::Hardened { index: 48 },
        ChildNumber::Hardened {
            index: coin_type(network),
        },
        account,
        ChildNumber::Hardened { index: 2 },
    ]
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_derivation_path() {
        assert_eq!(
            default_derivation_path(Network::Bitcoin).to_string(),
            "48'/0'/0'/2'"
        );
        assert_eq!(
            default_derivation_path(Network::Testnet).to_string(),
            "48'/1'/0'/2'"
        );
        assert_eq!(
            default_derivation_path(Network::Testnet4).to_string(),
            "48'/1'/0'/2'"
        );
        assert_eq!(
            default_derivation_path(Network::Signet).to_string(),
            "48'/1'/0'/2'"
        );
        assert_eq!(
            default_derivation_path(Network::Regtest).to_string(),
            "48'/1'/0'/2'"
        );
    }

    #[test]
    fn test_derivation_path() {
        assert_eq!(
            derivation_path(Network::Bitcoin, ChildNumber::Hardened { index: 0 }).to_string(),
            "48'/0'/0'/2'"
        );
        assert_eq!(
            derivation_path(Network::Regtest, ChildNumber::Hardened { index: 0 }).to_string(),
            "48'/1'/0'/2'"
        );
        assert_eq!(
            derivation_path(Network::Bitcoin, ChildNumber::Hardened { index: 1 }).to_string(),
            "48'/0'/1'/2'"
        );
        assert_eq!(
            derivation_path(Network::Regtest, ChildNumber::Hardened { index: 1 }).to_string(),
            "48'/1'/1'/2'"
        );
    }

    #[test]
    #[should_panic]
    fn unhardened_derivation_path() {
        derivation_path(Network::Bitcoin, ChildNumber::Normal { index: 0 }).to_string();
    }
}
