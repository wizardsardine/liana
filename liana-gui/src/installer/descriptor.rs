use async_hwi::{DeviceKind, Version};
use liana::miniscript::{
    bitcoin::bip32::{ChildNumber, Fingerprint},
    descriptor::DescriptorPublicKey,
};

use crate::{
    app::settings::ProviderKey, hw::is_compatible_with_tapminiscript, services::keys::api::KeyKind,
};

/// Whether to enable cosigner keys on all paths (excluding safety net paths).
const ENABLE_COSIGNER_KEYS: bool = false;

/// The source of a descriptor public key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeySource {
    /// A hardware signing device with the given kind and version.
    Device(DeviceKind, Option<Version>),
    /// A hot signer on the user's computer.
    HotSigner,
    /// A manually inserted xpub.
    Manual,
    /// A token for a key with the given kind.
    Token(KeyKind, ProviderKey),
}

impl KeySource {
    pub fn device_kind(&self) -> Option<&DeviceKind> {
        if let KeySource::Device(ref device_kind, _) = self {
            Some(device_kind)
        } else {
            None
        }
    }

    pub fn device_version(&self) -> Option<&Version> {
        if let KeySource::Device(_, ref version) = self {
            version.as_ref()
        } else {
            None
        }
    }

    pub fn is_compatible_taproot(&self) -> bool {
        if let KeySource::Device(ref device_kind, ref version) = self {
            is_compatible_with_tapminiscript(device_kind, version.as_ref())
        } else {
            true
        }
    }

    pub fn is_manual(&self) -> bool {
        matches!(self, KeySource::Manual)
    }

    pub fn is_token(&self) -> bool {
        matches!(self, KeySource::Token(_, _))
    }

    pub fn kind(&self) -> KeySourceKind {
        match self {
            Self::Device(_, _) => KeySourceKind::Device,
            Self::HotSigner => KeySourceKind::HotSigner,
            Self::Manual => KeySourceKind::Manual,
            Self::Token(kind, _) => KeySourceKind::Token(*kind),
        }
    }

    pub fn token(&self) -> Option<&String> {
        if let KeySource::Token(_, ProviderKey { token, .. }) = self {
            Some(token)
        } else {
            None
        }
    }

    pub fn provider_key(&self) -> Option<ProviderKey> {
        if let KeySource::Token(_, provider_key) = self {
            Some(provider_key.clone())
        } else {
            None
        }
    }

    pub fn provider_key_kind(&self) -> Option<KeyKind> {
        if let KeySource::Token(key_kind, _) = self {
            Some(*key_kind)
        } else {
            None
        }
    }
}

/// The kind of `KeySource`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Copy)]
pub enum KeySourceKind {
    /// A hardware signing device.
    Device,
    /// A hot signer.
    HotSigner,
    /// A manually inserted xpub.
    Manual,
    /// A token for a key with the given kind.
    Token(KeyKind),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key {
    pub source: KeySource,
    pub name: String,
    pub fingerprint: Fingerprint,
    pub key: DescriptorPublicKey,
    pub account: Option<ChildNumber>,
}

pub struct Path {
    pub keys: Vec<Option<Key>>,
    pub threshold: usize,
    pub sequence: PathSequence,
    pub warning: Option<PathWarning>,
}

impl Path {
    pub fn new(kind: PathKind) -> Self {
        let sequence = match kind {
            PathKind::Primary => PathSequence::Primary,
            PathKind::Recovery => PathSequence::Recovery(52_596), // displays "1y" in GUI
            PathKind::SafetyNet => PathSequence::SafetyNet,
        };
        Self {
            keys: vec![None],
            threshold: 1,
            sequence,
            warning: None,
        }
    }

    pub fn new_primary_path() -> Self {
        Self::new(PathKind::Primary)
    }

    pub fn new_recovery_path() -> Self {
        Self::new(PathKind::Recovery)
    }

    pub fn new_safety_net_path() -> Self {
        Self::new(PathKind::SafetyNet)
    }

    pub fn with_n_keys(mut self, n: usize) -> Self {
        self.keys = Vec::new();
        for _i in 0..n {
            self.keys.push(None);
        }
        self
    }

    pub fn with_threshold(mut self, t: usize) -> Self {
        self.threshold = if t > self.keys.len() {
            self.keys.len()
        } else {
            t
        };
        self
    }

    pub fn kind(&self) -> PathKind {
        self.sequence.path_kind()
    }

    pub fn valid(&self) -> bool {
        !self.keys.is_empty() && !self.keys.iter().any(|k| k.is_none()) && self.warning.is_none()
    }
}

/// The kind of spending path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKind {
    Primary,
    Recovery,
    SafetyNet,
}

impl PathKind {
    /// Whether a key with the given `KeySourceKind` can be chosen for this `PathKind`.
    pub fn can_choose_key_source_kind(&self, source_kind: &KeySourceKind) -> bool {
        match (self, source_kind) {
            // Safety net path only allows safety net keys.
            (Self::SafetyNet, KeySourceKind::Token(KeyKind::SafetyNet)) => true,
            (Self::SafetyNet, _) => false,
            // Safety net keys cannot be used in any other path kind.
            (_, KeySourceKind::Token(KeyKind::SafetyNet)) => false,
            // Enable/disable cosigner keys.
            (_, KeySourceKind::Token(KeyKind::Cosigner)) => ENABLE_COSIGNER_KEYS,
            _ => true,
        }
    }
}

/// The sequence of a spending path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathSequence {
    Primary,
    Recovery(u16), // this excludes zero, but we don't enforce it here.
    SafetyNet,
}

impl PathSequence {
    pub fn as_u16(&self) -> u16 {
        match self {
            Self::Primary => 0,
            Self::Recovery(s) => *s,
            Self::SafetyNet => u16::MAX,
        }
    }

    pub fn path_kind(&self) -> PathKind {
        match self {
            Self::Primary => PathKind::Primary,
            Self::Recovery(_) => PathKind::Recovery,
            Self::SafetyNet => PathKind::SafetyNet,
        }
    }
}

/// A path warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathWarning {
    DuplicateSequence,
    OnlyCosignerKeys,
    KeySourceKindDisallowed,
}

impl PathWarning {
    pub fn message(&self) -> &'static str {
        match self {
            Self::DuplicateSequence => {
                "No two recovery options may become available at the very same date."
            }
            Self::OnlyCosignerKeys => "A path cannot contain only cosigner keys.",
            Self::KeySourceKindDisallowed => {
                "Path contains a key that is disallowed for this kind of path."
            }
        }
    }
}
