use bip377::ll::response;
use liana::miniscript::{bitcoin::bip32::Fingerprint, descriptor::DescriptorPublicKey};
use serde::{Deserialize, Serialize};

use crate::airgap::exchange::Signer;

/// A persisted air-gapped signer. Only public account material is stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AirgappedSignerConfig {
    pub fingerprint: Fingerprint,
    pub alias: Option<String>,
    pub account: DescriptorPublicKey,
    pub model: String,
    pub version: FirmwareVersion,
    pub capabilities: Capabilities,
    #[serde(default)]
    pub registration: Registration,
}

impl AirgappedSignerConfig {
    pub fn new(signer: &Signer, account: DescriptorPublicKey, alias: Option<String>) -> Self {
        Self {
            fingerprint: signer.fingerprint,
            alias,
            account,
            model: signer.model.clone(),
            version: signer.version,
            capabilities: signer.capabilities,
            registration: Registration::default(),
        }
    }

    /// A registration only covers the descriptor it was made against.
    pub fn invalidate_registration(&mut self, descriptor_checksum: &str) {
        if !self.registration.covers(descriptor_checksum) {
            self.registration = Registration::default();
        }
    }
}

/// What a signer reported about a wallet registration. `stored` false or absent
/// means the descriptor has to travel with every later request.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registration {
    pub descriptor_checksum: Option<String>,
    pub stored: Option<bool>,
    pub proof: Option<Vec<u8>>,
}

impl Registration {
    pub fn read(registration: response::Registration) -> Self {
        Self {
            descriptor_checksum: None,
            stored: registration.stored,
            proof: registration.proof,
        }
    }

    pub fn covers(&self, descriptor_checksum: &str) -> bool {
        self.descriptor_checksum.as_deref() == Some(descriptor_checksum)
    }
}

/// The signer's firmware version, as it reported it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FirmwareVersion {
    pub major: u16,
    pub minor: u16,
    pub patch: u32,
    pub prerelease: Option<Prerelease>,
}

impl FirmwareVersion {
    pub fn read(version: &response::FirmwareVersion) -> Self {
        Self {
            major: version.major,
            minor: version.minor,
            patch: version.patch.into(),
            prerelease: Prerelease::read(version.flag),
        }
    }
}

impl std::fmt::Display for FirmwareVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let Self {
            major,
            minor,
            patch,
            prerelease,
        } = self;
        write!(f, "{major}.{minor}.{patch}")?;
        match prerelease {
            Some(prerelease) => write!(f, "-{prerelease}"),
            None => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Prerelease {
    Alpha(u8),
    Beta(u8),
    ReleaseCandidate(u8),
    /// A channel this version of Liana does not know about.
    Unknown,
}

impl Prerelease {
    fn read(flag: response::ReleaseFlag) -> Option<Self> {
        match flag {
            response::ReleaseFlag::Stable => None,
            response::ReleaseFlag::Alpha(number) => Some(Self::Alpha(number)),
            response::ReleaseFlag::Beta(number) => Some(Self::Beta(number)),
            response::ReleaseFlag::ReleaseCandidate(number) => Some(Self::ReleaseCandidate(number)),
            response::ReleaseFlag::Unknown { .. } => Some(Self::Unknown),
        }
    }
}

impl std::fmt::Display for Prerelease {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Alpha(number) => write!(f, "alpha.{number}"),
            Self::Beta(number) => write!(f, "beta.{number}"),
            Self::ReleaseCandidate(number) => write!(f, "rc.{number}"),
            Self::Unknown => f.write_str("prerelease"),
        }
    }
}

/// The capability bitfield a signer advertises. Unknown bits are kept as sent so
/// a newer signer is not misread as supporting less than it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities(pub u32);

impl Capabilities {
    pub fn supports(self, capability: response::Capability) -> bool {
        response::Capabilities(self.0).supports(capability)
    }
}
