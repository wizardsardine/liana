//! Hardware signing session for a foreign (Split) wallet PSBT (#568 Track B,
//! slice B4a; owner decisions D7 and P6).
//!
//! A connected device signs a PSBT that spends from a foreign wallet in one
//! of the Split shapes: singlesig `pkh`, `sh(wpkh)` or `wpkh` on the standard
//! BIP44/49/84 account paths, or `wsh(multi)` / `wsh(sortedmulti)`. For a
//! multisig policy the session first asks the device whether the policy is
//! registered and registers it when it is not (`is_wallet_registered` then
//! `register_wallet`). Coincube writes nothing to disk. What happens to the
//! registration depends on the device:
//!
//! - Ledger returns an HMAC instead of storing the policy. The HMAC stays in
//!   this session's memory, wrapped in [`Zeroizing`], and is dropped with the
//!   session, so the next session registers the policy again (P6).
//! - Jade, Coldcard and BitBox02 store the registration on the device
//!   itself. A later session finds it there
//!   ([`RegistrationOutcome::AlreadyRegistered`]) and does not ask again,
//!   until the user removes it in the device's settings.
//!
//! The device's output is not trusted. [`SigningSession::sign`] copies only
//! new ECDSA partial signatures, for keys this device owns according to the
//! caller's PSBT, onto a copy of that PSBT. It refuses any signature or input
//! field that is not `SIGHASH_ALL` (F1). The result is an
//! [`UnverifiedDeviceSignatures`]: the caller must still pass it through the
//! verified import and finalize path, which checks every signature against
//! the construction it built.
//!
//! Nothing here is reachable from the GUI (D1). The module works on generic
//! PSBT and descriptor types so the step-1 and step-2 flows can plug in.
//!
//! # Device classes
//!
//! async-hwi binds the wallet policy to some device handles when they are
//! opened, not when they sign, so a handle from the ephemeral Split device
//! list cannot always sign on its own:
//!
//! | Device | Singlesig | Multisig |
//! |---|---|---|
//! | Jade | listed handle | listed handle, after registration on the device |
//! | Coldcard | listed handle | handle bound to the policy name |
//! | BitBox02 | listed handle for `sh(wpkh)` and `wpkh`; `pkh` refused (bitbox-api has no P2PKH) | handle bound to the policy |
//! | Ledger | handle bound to the default policy (no HMAC) | handle bound to the policy and the session HMAC |
//! | Specter | listed handle | refused: the device cannot report registration |
//!
//! A bound handle comes from a caller-supplied [`PolicyBinder`], fixed when
//! the session opens. No production binder exists yet; the caller that adds
//! the GUI entry point provides one. Until then, [`NoBinder`] refuses the
//! bound rows with [`SignError::NeedsPolicyBinding`] at
//! [`SigningSession::open`], before the device is asked to register
//! anything, so the user never approves a registration that cannot be used.

use std::sync::Arc;

use async_hwi::{DeviceKind, HWI};
use coincube_core::miniscript::{
    bitcoin::{
        bip32::{ChildNumber, DerivationPath, Fingerprint},
        ecdsa,
        hashes::Hash,
        psbt::Input,
        sighash::EcdsaSighashType,
        NetworkKind, Psbt,
    },
    descriptor::{DescriptorPublicKey, ShInner, Wildcard, WshInner},
    miniscript::decode::Terminal,
    Descriptor, ForEachKey,
};
use coincube_core::wallet_descriptor::ImportedWalletDescriptor;
use zeroize::Zeroizing;

use crate::hw::HardwareWallet;

/// The device knows only Bitcoin, so it shows a BTCB2 spend as Bitcoin.
pub const DEVICE_SHOWS_BITCOIN_WARNING: &str = "Your hardware wallet will call this a Bitcoin transaction. It cannot tell BTCB2 from Bitcoin. Before you approve, check that the amounts and the address on the device match what Coincube shows.";

/// Step 2 spends BTCB2 coins, but the device screen still says Bitcoin.
pub const STEP2_DEVICE_SHOWS_BITCOIN_WARNING: &str = "This step moves your BTCB2 coins. Your hardware wallet will still show the amounts as Bitcoin. Approve only if the address matches your BTCB2 Vault address shown in Coincube.";

/// Step 1 carries a zero-value data output that some devices show oddly.
pub const STEP1_DATA_OUTPUT_NOTE: &str = "This transaction includes a zero-value data output that separates your coins. Your device may show it as a data or unknown output with no amount.";

/// Shown before a Ledger asks the user to register a multisig policy: its
/// registration token lives only in this session's memory.
pub const REGISTRATION_NOTICE_SESSION: &str = "Your hardware wallet will ask you to register this multisig wallet. Coincube keeps the registration only while this window is open, so you will be asked again next time.";

/// Shown before a device that stores registrations (Jade, Coldcard,
/// BitBox02) asks the user to register a multisig policy.
pub const REGISTRATION_NOTICE_ON_DEVICE: &str = "Your hardware wallet will ask you to register this multisig wallet. The device keeps the registration, so it will not ask again next time. You can remove it later in the device's settings.";

/// The registration notice that matches how `kind` keeps a registration.
pub fn registration_notice(kind: DeviceKind) -> &'static str {
    match kind {
        DeviceKind::Ledger | DeviceKind::LedgerSimulator => REGISTRATION_NOTICE_SESSION,
        _ => REGISTRATION_NOTICE_ON_DEVICE,
    }
}

/// Device-visible name prefix for a registered Split policy.
const POLICY_NAME_PREFIX: &str = "Split";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SinglesigScript {
    Pkh,
    ShWpkh,
    Wpkh,
}

impl SinglesigScript {
    fn purpose(self) -> u32 {
        match self {
            Self::Pkh => 44,
            Self::ShWpkh => 49,
            Self::Wpkh => 84,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyShape {
    Singlesig(SinglesigScript),
    /// `wsh(multi(k, ...))` or, when `sorted`, `wsh(sortedmulti(k, ...))`.
    Multisig {
        k: usize,
        n: usize,
        sorted: bool,
    },
}

impl PolicyShape {
    pub fn is_multisig(self) -> bool {
        matches!(self, Self::Multisig { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignError {
    /// Not one of the five Split shapes.
    UnsupportedShape,
    /// A key is not an origin-tagged mainnet xpub ending in `/<0;1>/*`.
    KeyForm,
    /// A singlesig key is not on the standard account path for its script.
    NonStandardSinglesigPath,
    /// The same key appears twice in a multisig policy.
    DuplicateKey,
    /// The device's fingerprint is not one of the policy's keys.
    DeviceNotInPolicy,
    /// The device reported another fingerprint than the listed one.
    Fingerprint,
    /// The listed device is locked or unsupported.
    DeviceUnavailable,
    /// This device class cannot sign this shape: Specter multisig (it
    /// cannot report registration) or BitBox02 `pkh` (no P2PKH support).
    UnsupportedDevice(DeviceKind),
    /// This device class signs only through a policy-bound handle, and the
    /// caller supplied no binder for it.
    NeedsPolicyBinding(DeviceKind),
    /// Signing a multisig policy before it was registered in this session,
    /// or the device no longer reports it as registered.
    PolicyNotRegistered,
    /// The user declined the registration on the device.
    RegistrationRefused,
    /// The user declined to sign on the device.
    SigningRefused,
    /// The PSBT asks for, or the device returned, a non-ALL sighash.
    NonAllSighash {
        input: usize,
    },
    /// The device changed the transaction or the input list.
    TransactionChanged,
    /// The device returned a signature for a key it does not own in this
    /// PSBT, replaced an existing signature, or added a Taproot signature.
    UnexpectedSignature {
        input: usize,
    },
    /// The device returned no new signature.
    DeviceDidNotSign,
    Device(String),
}

impl std::fmt::Display for SignError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedShape => write!(f, "Only pkh, sh(wpkh), wpkh, wsh(multi) and wsh(sortedmulti) wallets can be signed with a hardware wallet in Split."),
            Self::KeyForm => write!(f, "Each wallet key must be a mainnet xpub with its origin, ending in /<0;1>/*."),
            Self::NonStandardSinglesigPath => write!(f, "A single-key wallet must use the standard account path for its address type."),
            Self::DuplicateKey => write!(f, "The multisig wallet lists the same key twice."),
            Self::DeviceNotInPolicy => write!(f, "This hardware wallet holds none of this wallet's keys."),
            Self::Fingerprint => write!(f, "The device reported a different wallet fingerprint. Reconnect it and try again."),
            Self::DeviceUnavailable => write!(f, "Unlock the hardware wallet and open its Bitcoin app."),
            Self::UnsupportedDevice(kind) => write!(f, "{kind} cannot sign this type of wallet for Split."),
            Self::NeedsPolicyBinding(kind) => write!(f, "{kind} cannot sign this wallet from Split yet."),
            Self::PolicyNotRegistered => write!(f, "Register the multisig wallet on the device before signing."),
            Self::RegistrationRefused => write!(f, "The multisig wallet was not registered on the device."),
            Self::SigningRefused => write!(f, "The transaction was not signed on the device."),
            Self::NonAllSighash { input } => write!(f, "Input {input} uses a signature type other than SIGHASH_ALL. Nothing was accepted."),
            Self::TransactionChanged => write!(f, "The device returned a different transaction. Nothing was accepted."),
            Self::UnexpectedSignature { input } => write!(f, "The device returned an unexpected signature for input {input}. Nothing was accepted."),
            Self::DeviceDidNotSign => write!(f, "The device did not sign any input."),
            Self::Device(e) => write!(f, "Device error: {e}"),
        }
    }
}

impl std::error::Error for SignError {}

fn device_error(e: async_hwi::Error) -> SignError {
    match e {
        async_hwi::Error::DeviceDidNotSign => SignError::DeviceDidNotSign,
        e => SignError::Device(e.to_string()),
    }
}

/// A foreign wallet policy in the form a device registers and signs.
#[derive(Clone)]
pub struct DevicePolicy {
    shape: PolicyShape,
    /// Descriptor string sent to the device, with checksum.
    descriptor: String,
    /// Device-visible name. Empty for a singlesig default policy, which
    /// Ledger requires for its unregistered standard wallets.
    name: String,
    fingerprints: Vec<Fingerprint>,
}

impl std::fmt::Debug for DevicePolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DevicePolicy")
            .field("shape", &self.shape)
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl DevicePolicy {
    /// Validates a multipath (`/<0;1>/*`) public descriptor as a Split shape.
    pub fn new(descriptor: &Descriptor<DescriptorPublicKey>) -> Result<Self, SignError> {
        let (shape, keys): (PolicyShape, Vec<&DescriptorPublicKey>) = match descriptor {
            Descriptor::Pkh(pkh) => (
                PolicyShape::Singlesig(SinglesigScript::Pkh),
                vec![pkh.as_inner()],
            ),
            Descriptor::Wpkh(wpkh) => (
                PolicyShape::Singlesig(SinglesigScript::Wpkh),
                vec![wpkh.as_inner()],
            ),
            Descriptor::Sh(sh) => match sh.as_inner() {
                ShInner::Wpkh(wpkh) => (
                    PolicyShape::Singlesig(SinglesigScript::ShWpkh),
                    vec![wpkh.as_inner()],
                ),
                _ => return Err(SignError::UnsupportedShape),
            },
            Descriptor::Wsh(wsh) => match wsh.as_inner() {
                WshInner::SortedMulti(smv) => (
                    PolicyShape::Multisig {
                        k: smv.k(),
                        n: smv.pks().len(),
                        sorted: true,
                    },
                    smv.pks().iter().collect(),
                ),
                WshInner::Ms(ms) => match &ms.node {
                    Terminal::Multi(thresh) => (
                        PolicyShape::Multisig {
                            k: thresh.k(),
                            n: thresh.n(),
                            sorted: false,
                        },
                        thresh.data().iter().collect(),
                    ),
                    _ => return Err(SignError::UnsupportedShape),
                },
            },
            _ => return Err(SignError::UnsupportedShape),
        };
        // Every key the descriptor carries must be one of the shape's keys.
        let mut total = 0;
        descriptor.for_each_key(|_| {
            total += 1;
            true
        });
        if total != keys.len() {
            return Err(SignError::UnsupportedShape);
        }

        let mut fingerprints = Vec::with_capacity(keys.len());
        for key in &keys {
            fingerprints.push(check_key(key, shape)?);
        }
        let mut distinct = keys.iter().map(|k| k.to_string()).collect::<Vec<_>>();
        distinct.sort();
        distinct.dedup();
        if distinct.len() != keys.len() {
            return Err(SignError::DuplicateKey);
        }

        let descriptor = descriptor.to_string();
        let name = if shape.is_multisig() {
            policy_name(&descriptor)
        } else {
            String::new()
        };
        Ok(Self {
            shape,
            descriptor,
            name,
            fingerprints,
        })
    }

    /// Imported multisig registration uses its chain-bound full wallet
    /// identity, keeping every existing Split device name unchanged.
    pub(super) fn new_imported(
        descriptor: &Descriptor<DescriptorPublicKey>,
        wallet: &ImportedWalletDescriptor,
    ) -> Result<Self, SignError> {
        let mut policy = Self::new(descriptor)?;
        if policy.shape.is_multisig() {
            let identity = wallet
                .identity(wallet.chain())
                .map_err(|_| SignError::KeyForm)?;
            policy.name = format!("Import{}", &identity.to_string()[..8]);
        }
        Ok(policy)
    }

    pub fn shape(&self) -> PolicyShape {
        self.shape
    }

    /// The name the device shows for a registered multisig policy.
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn descriptor(&self) -> &str {
        &self.descriptor
    }

    pub fn contains(&self, fingerprint: Fingerprint) -> bool {
        self.fingerprints.contains(&fingerprint)
    }
}

/// `Split` plus eight hex digits of the descriptor hash: short enough for
/// Jade and Coldcard name limits, stable for one descriptor.
fn policy_name(descriptor: &str) -> String {
    let digest =
        coincube_core::miniscript::bitcoin::hashes::sha256::Hash::hash(descriptor.as_bytes());
    format!(
        "{POLICY_NAME_PREFIX}{}",
        &hex::encode(digest.to_byte_array())[..8]
    )
}

fn check_key(key: &DescriptorPublicKey, shape: PolicyShape) -> Result<Fingerprint, SignError> {
    let DescriptorPublicKey::MultiXPub(xkey) = key else {
        return Err(SignError::KeyForm);
    };
    let Some((fingerprint, origin)) = &xkey.origin else {
        return Err(SignError::KeyForm);
    };
    let branches = [
        DerivationPath::from(vec![ChildNumber::Normal { index: 0 }]),
        DerivationPath::from(vec![ChildNumber::Normal { index: 1 }]),
    ];
    if xkey.xkey.network != NetworkKind::Main
        || xkey.wildcard != Wildcard::Unhardened
        || xkey.derivation_paths.paths().as_slice() != branches
        || *fingerprint == Fingerprint::default()
    {
        return Err(SignError::KeyForm);
    }
    if let PolicyShape::Singlesig(script) = shape {
        let path: Vec<ChildNumber> = origin.into_iter().copied().collect();
        let standard = matches!(
            path.as_slice(),
            [ChildNumber::Hardened { index: purpose }, ChildNumber::Hardened { index: 0 }, ChildNumber::Hardened { .. }]
                if *purpose == script.purpose()
        );
        if !standard || xkey.xkey.depth != 3 {
            return Err(SignError::NonStandardSinglesigPath);
        }
    }
    Ok(*fingerprint)
}

/// Which handle signs for a device class and shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigningHandle {
    /// The handle from the device list signs directly.
    Listed,
    /// A handle bound to the policy (and, for Ledger multisig, the session
    /// HMAC) must be opened through a [`PolicyBinder`].
    Bound,
}

pub fn signing_handle(kind: DeviceKind, shape: PolicyShape) -> Result<SigningHandle, SignError> {
    let multisig = shape.is_multisig();
    Ok(match kind {
        DeviceKind::Ledger | DeviceKind::LedgerSimulator => SigningHandle::Bound,
        DeviceKind::Jade => SigningHandle::Listed,
        // BitBox02 multisig must stay on the bound route: with no forced
        // policy, bitbox-api 0.9.0 `script_config_from_utxo` reaches `todo!()`
        // for a P2WSH input. `split_hw_sign_handle_table` pins this.
        DeviceKind::BitBox02 | DeviceKind::Coldcard if multisig => SigningHandle::Bound,
        // bitbox-api 0.9.0 infers only p2wpkh, p2wpkh-p2sh and p2tr; a P2PKH
        // input ends in `UnknownOutputType`, so refuse it before the device.
        DeviceKind::BitBox02 if shape == PolicyShape::Singlesig(SinglesigScript::Pkh) => {
            return Err(SignError::UnsupportedDevice(kind))
        }
        DeviceKind::BitBox02 | DeviceKind::Coldcard => SigningHandle::Listed,
        // Specter's `is_wallet_registered` is unimplemented, so registration
        // could never be confirmed.
        DeviceKind::Specter | DeviceKind::SpecterSimulator if multisig => {
            return Err(SignError::UnsupportedDevice(kind))
        }
        DeviceKind::Specter | DeviceKind::SpecterSimulator => SigningHandle::Listed,
    })
}

/// What a binder needs to open a policy-bound handle for one device.
pub struct BindRequest<'a> {
    pub kind: DeviceKind,
    pub fingerprint: Fingerprint,
    pub name: &'a str,
    pub policy: &'a str,
    /// The registration token from this session, if the device returned one.
    /// The binder must not store it beyond the handle it returns.
    pub hmac: Option<&'a [u8; 32]>,
}

/// Opens a device handle bound to a policy, for devices whose async-hwi
/// handle carries the wallet from construction (Ledger `with_wallet`,
/// Coldcard `with_wallet_name`, BitBox02 `policy`).
#[async_trait::async_trait]
pub trait PolicyBinder: Send + Sync {
    /// Whether this binder can open a bound handle for `kind`. A session
    /// that needs a bound handle refuses at open when this is false.
    fn can_bind(&self, kind: DeviceKind) -> bool;

    async fn bind(
        &self,
        request: BindRequest<'_>,
    ) -> Result<Arc<dyn HWI + Send + Sync>, async_hwi::Error>;
}

/// The binder for callers that have none: every bound case is refused.
pub struct NoBinder;

#[async_trait::async_trait]
impl PolicyBinder for NoBinder {
    fn can_bind(&self, _: DeviceKind) -> bool {
        false
    }

    async fn bind(
        &self,
        _: BindRequest<'_>,
    ) -> Result<Arc<dyn HWI + Send + Sync>, async_hwi::Error> {
        Err(async_hwi::Error::UnimplementedMethod)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationOutcome {
    /// Singlesig: devices sign the standard account paths without one.
    NotNeeded,
    /// The device already reported the policy as registered.
    AlreadyRegistered,
    /// The user approved the registration on the device just now.
    Registered,
}

/// In-memory registration for this session only.
struct Registration {
    hmac: Option<Zeroizing<[u8; 32]>>,
}

/// One device, one policy, for one Split signing attempt. Dropping it drops
/// (and zeroizes) any registration token.
pub struct SigningSession {
    device: Arc<dyn HWI + Send + Sync>,
    kind: DeviceKind,
    fingerprint: Fingerprint,
    policy: DevicePolicy,
    handle: SigningHandle,
    binder: Arc<dyn PolicyBinder>,
    registration: Option<Registration>,
}

impl std::fmt::Debug for SigningSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SigningSession")
            .field("kind", &self.kind)
            .field("fingerprint", &self.fingerprint)
            .field("policy", &self.policy)
            .field("registered", &self.registration.is_some())
            .finish_non_exhaustive()
    }
}

impl SigningSession {
    /// Opens a session on a listed device, rechecking its fingerprint.
    pub async fn from_listed(
        hw: &HardwareWallet,
        policy: DevicePolicy,
        binder: Arc<dyn PolicyBinder>,
    ) -> Result<Self, SignError> {
        match hw {
            HardwareWallet::Supported {
                device,
                fingerprint,
                ..
            } => Self::open(device.clone(), *fingerprint, policy, binder).await,
            _ => Err(SignError::DeviceUnavailable),
        }
    }

    /// Refuses before any device prompt when the device class cannot sign
    /// this shape, or needs a bound handle that `binder` cannot open.
    pub async fn open(
        device: Arc<dyn HWI + Send + Sync>,
        listed: Fingerprint,
        policy: DevicePolicy,
        binder: Arc<dyn PolicyBinder>,
    ) -> Result<Self, SignError> {
        let kind = device.device_kind();
        let handle = signing_handle(kind, policy.shape)?;
        if handle == SigningHandle::Bound && !binder.can_bind(kind) {
            return Err(SignError::NeedsPolicyBinding(kind));
        }
        if !policy.contains(listed) {
            return Err(SignError::DeviceNotInPolicy);
        }
        let reported = device
            .get_master_fingerprint()
            .await
            .map_err(device_error)?;
        if reported != listed {
            return Err(SignError::Fingerprint);
        }
        Ok(Self {
            device,
            kind,
            fingerprint: listed,
            policy,
            handle,
            binder,
            registration: None,
        })
    }

    pub fn policy(&self) -> &DevicePolicy {
        &self.policy
    }

    pub fn handle(&self) -> SigningHandle {
        self.handle
    }

    /// Multisig: asks the device whether the policy is registered, under
    /// any name, and registers it (the user approves on the device) when it
    /// is not. The token the device returns, if any, is held only in this
    /// session.
    pub async fn register(&mut self) -> Result<RegistrationOutcome, SignError> {
        if !self.policy.shape.is_multisig() {
            return Ok(RegistrationOutcome::NotNeeded);
        }
        let (name, policy) = (&self.policy.name, &self.policy.descriptor);
        // Any registration of this policy counts, whatever app made it and
        // whatever it is called: registering it again only adds a duplicate
        // the user has to approve and later clean up.
        let registered = match self.device.find_wallet_registration(name, policy).await {
            Ok(found) => found.is_some(),
            Err(async_hwi::Error::UnimplementedMethod) => {
                return Err(SignError::UnsupportedDevice(self.kind))
            }
            Err(e) => return Err(device_error(e)),
        };
        if registered {
            self.registration = Some(Registration { hmac: None });
            return Ok(RegistrationOutcome::AlreadyRegistered);
        }
        let hmac = match self.device.register_wallet(name, policy).await {
            Ok(hmac) => hmac.map(Zeroizing::new),
            Err(async_hwi::Error::UserRefused) => return Err(SignError::RegistrationRefused),
            Err(async_hwi::Error::UnimplementedMethod) => {
                return Err(SignError::UnsupportedDevice(self.kind))
            }
            Err(e) => return Err(device_error(e)),
        };
        // A device that stores the registration itself must now report it.
        // A token-returning device (Ledger) cannot, until a handle is bound
        // with the token; `sign` checks that bound handle instead.
        if hmac.is_none()
            && self
                .device
                .find_wallet_registration(name, policy)
                .await
                .map_err(device_error)?
                .is_none()
        {
            return Err(SignError::PolicyNotRegistered);
        }
        self.registration = Some(Registration { hmac });
        Ok(RegistrationOutcome::Registered)
    }

    /// Asks the device to sign a copy of `psbt` and returns only the new
    /// ALL signatures for this device's keys, on an otherwise unchanged copy.
    pub async fn sign(&self, psbt: &Psbt) -> Result<UnverifiedDeviceSignatures, SignError> {
        let multisig = self.policy.shape.is_multisig();
        if multisig && self.registration.is_none() {
            return Err(SignError::PolicyNotRegistered);
        }
        for (i, input) in psbt.inputs.iter().enumerate() {
            validate_declared_sighash(input, i)?;
        }

        let device = match self.handle {
            SigningHandle::Listed => self.device.clone(),
            SigningHandle::Bound => {
                let hmac = self.registration.as_ref().and_then(|r| r.hmac.as_deref());
                let bound = self
                    .binder
                    .bind(BindRequest {
                        kind: self.kind,
                        fingerprint: self.fingerprint,
                        name: &self.policy.name,
                        policy: &self.policy.descriptor,
                        hmac,
                    })
                    .await
                    .map_err(|e| match e {
                        async_hwi::Error::UnimplementedMethod => {
                            SignError::NeedsPolicyBinding(self.kind)
                        }
                        e => device_error(e),
                    })?;
                if bound.get_master_fingerprint().await.map_err(device_error)? != self.fingerprint {
                    return Err(SignError::Fingerprint);
                }
                bound
            }
        };
        // Checked on the handle that signs: a Coldcard handle also remembers
        // a registration found under another name and signs against it.
        if multisig
            && device
                .find_wallet_registration(&self.policy.name, &self.policy.descriptor)
                .await
                .map_err(device_error)?
                .is_none()
        {
            return Err(SignError::PolicyNotRegistered);
        }

        let mut signed = psbt.clone();
        match device.sign_tx(&mut signed).await {
            Ok(()) => {}
            Err(async_hwi::Error::UserRefused) => return Err(SignError::SigningRefused),
            Err(e) => return Err(device_error(e)),
        }
        accept_device_signatures(psbt, &signed, self.fingerprint).map(UnverifiedDeviceSignatures)
    }
}

/// Copies onto `original` only the device's new partial signatures that are
/// `SIGHASH_ALL` and belong to a key with this device's fingerprint in the
/// original input's BIP32 derivations. Everything else the device returned
/// is dropped; anything suspicious refuses the whole result.
pub fn accept_device_signatures(
    original: &Psbt,
    device: &Psbt,
    fingerprint: Fingerprint,
) -> Result<Psbt, SignError> {
    if device.unsigned_tx != original.unsigned_tx || device.inputs.len() != original.inputs.len() {
        return Err(SignError::TransactionChanged);
    }
    let mut accepted = original.clone();
    let mut added = 0;
    for (i, (before, after)) in original.inputs.iter().zip(&device.inputs).enumerate() {
        accept_input_signatures(
            before,
            after,
            &mut accepted.inputs[i],
            i,
            fingerprint,
            &mut added,
        )?;
    }
    if added == 0 {
        return Err(SignError::DeviceDidNotSign);
    }
    Ok(accepted)
}

fn validate_declared_sighash(input: &Input, i: usize) -> Result<(), SignError> {
    if let Some(sighash) = input.sighash_type {
        if sighash.ecdsa_hash_ty() != Ok(EcdsaSighashType::All) {
            return Err(SignError::NonAllSighash { input: i });
        }
    }
    Ok(())
}

fn accept_input_signatures(
    before: &Input,
    after: &Input,
    accepted: &mut Input,
    i: usize,
    fingerprint: Fingerprint,
    added: &mut i32,
) -> Result<(), SignError> {
    validate_declared_sighash(after, i)?;
    if after.tap_key_sig.is_some() || !after.tap_script_sigs.is_empty() {
        return Err(SignError::UnexpectedSignature { input: i });
    }
    for (key, sig) in &after.partial_sigs {
        if let Some(existing) = before.partial_sigs.get(key) {
            if existing != sig {
                return Err(SignError::UnexpectedSignature { input: i });
            }
            continue;
        }
        let owned = before
            .bip32_derivation
            .get(&key.inner)
            .is_some_and(|(fp, _)| *fp == fingerprint);
        if !owned {
            return Err(SignError::UnexpectedSignature { input: i });
        }
        if !is_all(sig) {
            return Err(SignError::NonAllSighash { input: i });
        }
        accepted.partial_sigs.insert(*key, *sig);
        *added += 1;
    }
    Ok(())
}

fn is_all(sig: &ecdsa::Signature) -> bool {
    sig.sighash_type == EcdsaSighashType::All
}

/// Device signatures that passed the sighash and ownership screen but have
/// not been verified. Pass the PSBT to the verified import and finalize path.
#[derive(Debug)]
pub struct UnverifiedDeviceSignatures(Psbt);

impl UnverifiedDeviceSignatures {
    pub fn into_psbt(self) -> Psbt {
        self.0
    }
}

#[cfg(test)]
mod tests;
