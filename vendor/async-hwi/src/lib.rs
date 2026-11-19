pub mod bip389;
#[cfg(feature = "bitbox")]
pub mod bitbox;
#[cfg(feature = "coldcard")]
pub mod coldcard;
#[cfg(feature = "jade")]
pub mod jade;
#[cfg(feature = "ledger")]
pub mod ledger;
#[cfg(feature = "service")]
pub mod service;
#[cfg(feature = "specter")]
pub mod specter;
pub mod utils;

use async_trait::async_trait;
use bitcoin::{
    bip32::{ChildNumber, DerivationPath, Fingerprint, Xpub},
    psbt::Psbt,
};

use std::{cmp::Ordering, fmt::Debug, str::FromStr};

const RECV_INDEX: ChildNumber = ChildNumber::Normal { index: 0 };
const CHANGE_INDEX: ChildNumber = ChildNumber::Normal { index: 1 };

#[derive(Debug, Clone)]
pub enum Error {
    ParsingPolicy(bip389::ParseError),
    MissingPolicy,
    UnsupportedVersion,
    UnsupportedInput,
    InvalidParameter(&'static str, String),
    UnimplementedMethod,
    DeviceDisconnected,
    DeviceNotFound,
    DeviceDidNotSign,
    Device(String),
    Unexpected(&'static str),
    UserRefused,
    NetworkMismatch,
    Bip86ChangeIndex,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Error::ParsingPolicy(e) => write!(f, "{e}"),
            Error::MissingPolicy => write!(f, "Missing policy"),
            Error::UnsupportedVersion => write!(f, "Unsupported version"),
            Error::UnsupportedInput => write!(f, "Unsupported input"),
            Error::UnimplementedMethod => write!(f, "Unimplemented method"),
            Error::DeviceDisconnected => write!(f, "Device disconnected"),
            Error::DeviceNotFound => write!(f, "Device not found"),
            Error::DeviceDidNotSign => write!(f, "Device did not sign"),
            Error::Device(e) => write!(f, "{e}"),
            Error::InvalidParameter(param, e) => write!(f, "Invalid parameter {param}: {e}"),
            Error::Unexpected(e) => write!(f, "{e}"),
            Error::UserRefused => write!(f, "User refused operation"),
            Error::NetworkMismatch => write!(f, "Device network is different"),
            Error::Bip86ChangeIndex => {
                write!(f, "Ledger devices only accept 0 or 1 as`change` index value for BIP86 derivation path")
            }
        }
    }
}

impl From<bip389::ParseError> for Error {
    fn from(value: bip389::ParseError) -> Self {
        Error::ParsingPolicy(value)
    }
}

impl std::error::Error for Error {}

/// A wallet registration found on a device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalletRegistration {
    /// The name the device keeps it under. `None` for a registration the
    /// device does not address by name.
    pub name: Option<String>,
}

/// HWI is the common Hardware Wallet Interface.
#[async_trait]
pub trait HWI: Debug {
    /// Return the device kind
    fn device_kind(&self) -> DeviceKind;
    /// Application version or OS version.
    async fn get_version(&self) -> Result<Version, Error>;
    /// Get master fingerprint.
    async fn get_master_fingerprint(&self) -> Result<Fingerprint, Error>;
    /// Get the xpub with the given derivation path.
    async fn get_extended_pubkey(&self, path: &DerivationPath) -> Result<Xpub, Error>;
    /// Register a new wallet policy.
    async fn register_wallet(&self, name: &str, policy: &str) -> Result<Option<[u8; 32]>, Error>;
    /// Returns true if the wallet is registered on the device.
    async fn is_wallet_registered(&self, name: &str, policy: &str) -> Result<bool, Error>;
    /// Looks for a registration of `policy` on the device under any name,
    /// including one another wallet app made (Specter, Sparrow, ...).
    ///
    /// The default only knows the registration under `name`, which is all a
    /// device can report when it keeps no registry (Ledger: the session
    /// HMAC) or looks registrations up by content anyway (BitBox02). Devices
    /// that store named registrations (Jade, Coldcard) search all of them, so
    /// a wallet registered elsewhere is not registered a second time.
    async fn find_wallet_registration(
        &self,
        name: &str,
        policy: &str,
    ) -> Result<Option<WalletRegistration>, Error> {
        Ok(self
            .is_wallet_registered(name, policy)
            .await?
            .then(|| WalletRegistration {
                name: Some(name.to_string()),
            }))
    }
    /// Display address on the device screen.
    async fn display_address(&self, script: &AddressScript) -> Result<(), Error>;
    /// Sign a partially signed bitcoin transaction (PSBT).
    async fn sign_tx(&self, tx: &mut Psbt) -> Result<(), Error>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddressScript {
    /// Must be a bip86 path.
    P2TR(DerivationPath),
    /// Miniscript requires the policy be loaded into the device.
    Miniscript { index: u32, change: bool },
}

#[derive(PartialEq, Eq, Debug, Clone, Default)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
    pub prerelease: Option<String>,
}

#[cfg(feature = "regex")]
pub fn parse_version(s: &str) -> Result<Version, Error> {
    // Regex from https://semver.org/ with patch group marked as optional
    let re = regex::Regex::new(r"^(0|[1-9]\d*)\.(0|[1-9]\d*)(?:\.(0|[1-9]\d*))?(?:-((?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*)(?:\.(?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*))*))?(?:\+([0-9a-zA-Z-]+(?:\.[0-9a-zA-Z-]+)*))?$").unwrap();
    if let Some(captures) = re.captures(
        s.trim_start_matches('v')
            // Coldcard Q does not follow semver format
            .trim_end_matches("QX")
            // Coldcard mk4 does not follow semver format
            .trim_end_matches('X'),
    ) {
        let major = if let Some(s) = captures.get(1) {
            u32::from_str(s.as_str()).map_err(|_| Error::UnsupportedVersion)?
        } else {
            0
        };
        let minor = if let Some(s) = captures.get(2) {
            u32::from_str(s.as_str()).map_err(|_| Error::UnsupportedVersion)?
        } else {
            0
        };
        let patch = if let Some(s) = captures.get(3) {
            u32::from_str(s.as_str()).map_err(|_| Error::UnsupportedVersion)?
        } else {
            0
        };
        Ok(Version {
            major,
            minor,
            patch,
            prerelease: captures.get(4).map(|s| s.as_str().to_string()),
        })
    } else {
        Err(Error::UnsupportedVersion)
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        match self.major.cmp(&other.major) {
            Ordering::Equal => match self.minor.cmp(&other.minor) {
                Ordering::Equal => match self.patch.cmp(&other.patch) {
                    Ordering::Equal => {
                        match (&self.prerelease, &other.prerelease) {
                            // Cannot compare versions at this point.
                            (Some(_), Some(_)) => None,
                            (Some(_), None) => Some(Ordering::Greater),
                            (None, Some(_)) => Some(Ordering::Less),
                            (None, None) => Some(Ordering::Equal),
                        }
                    }
                    other => Some(other),
                },
                other => Some(other),
            },
            other => Some(other),
        }
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        if let Some(prerelease) = &self.prerelease {
            write!(
                f,
                "{}.{}.{}-{}",
                self.major, self.minor, self.patch, prerelease
            )
        } else {
            write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
        }
    }
}

/// DeviceType is the result of the following process:
/// If it is talking like a Duck© hardware wallet it is a Duck© hardware wallet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeviceKind {
    BitBox02,
    Coldcard,
    Specter,
    SpecterSimulator,
    Ledger,
    LedgerSimulator,
    Jade,
}

impl std::fmt::Display for DeviceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            DeviceKind::BitBox02 => write!(f, "bitbox02"),
            DeviceKind::Coldcard => write!(f, "coldcard"),
            DeviceKind::Specter => write!(f, "specter"),
            DeviceKind::SpecterSimulator => write!(f, "specter-simulator"),
            DeviceKind::Ledger => write!(f, "ledger"),
            DeviceKind::LedgerSimulator => write!(f, "ledger-simulator"),
            DeviceKind::Jade => write!(f, "jade"),
        }
    }
}

impl std::str::FromStr for DeviceKind {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "bitbox02" => Ok(DeviceKind::BitBox02),
            "specter" => Ok(DeviceKind::Specter),
            "specter-simulator" => Ok(DeviceKind::SpecterSimulator),
            "ledger" => Ok(DeviceKind::Ledger),
            "ledger-simulator" => Ok(DeviceKind::LedgerSimulator),
            "jade" => Ok(DeviceKind::Jade),
            _ => Err(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "regex")]
    #[test]
    fn test_parse_version() {
        let test_cases = [
            (
                "v2.1.0",
                Version {
                    major: 2,
                    minor: 1,
                    patch: 0,
                    prerelease: None,
                },
            ),
            (
                "v1.0",
                Version {
                    major: 1,
                    minor: 0,
                    patch: 0,
                    prerelease: None,
                },
            ),
            (
                "3.0-rc2",
                Version {
                    major: 3,
                    minor: 0,
                    patch: 0,
                    prerelease: Some("rc2".to_string()),
                },
            ),
            (
                "0.1.0-ALPHA",
                Version {
                    major: 0,
                    minor: 1,
                    patch: 0,
                    prerelease: Some("ALPHA".to_string()),
                },
            ),
            (
                "6.2.1X",
                Version {
                    major: 6,
                    minor: 2,
                    patch: 1,
                    prerelease: None,
                },
            ),
            (
                "6.3.3QX",
                Version {
                    major: 6,
                    minor: 3,
                    patch: 3,
                    prerelease: None,
                },
            ),
        ];
        for (s, v) in test_cases {
            assert_eq!(v, parse_version(s).unwrap());
        }
    }

    #[cfg(feature = "regex")]
    #[test]
    fn test_partial_ord_version() {
        let test_cases = [
            ("v2.1.0", "v3.1.0"),
            ("v0.0.1", "v0.1"),
            ("v0.1", "v1.0.1"),
            ("v2.0.1", "v2.1.0"),
            ("v2.1.1", "v3.0-rc1"),
            ("v3.0-rc1", "v3.0.1"),
            ("v3.0", "v3.0-rc1"),
        ];
        for (l, r) in test_cases {
            let v1 = parse_version(l).unwrap();
            let v2 = parse_version(r).unwrap();
            assert!(v1 < v2);
        }

        // We cannot compare prerelease of the same version.
        let v1 = parse_version("v2.0-rc1weirdstuff").unwrap();
        let v2 = parse_version("v2.0-rc1weirderstuff").unwrap();
        assert!(v1.partial_cmp(&v2).is_none());
    }
}
