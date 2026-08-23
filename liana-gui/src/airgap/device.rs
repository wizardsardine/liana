use async_hwi::utils::{extract_keys_and_template, merge_signatures};
use bip377::{
    backend::{rust_bitcoin::RustBitcoin, Backend},
    ll::{
        self,
        response::{Capabilities, Capability, FirmwareVersion},
        MessageType, RequestId, SignResponseKind,
    },
    request::{self, KeyInfo, KeyOrigin, Request},
    response::{self, SignatureEntry, Signed},
};
use liana::{
    descriptors::LianaDescriptor,
    miniscript::{
        bitcoin::{
            bip32::{ChildNumber, DerivationPath, Fingerprint},
            psbt::Psbt,
            Address, Network, NetworkKind,
        },
        descriptor::{DescriptorPublicKey, DescriptorXKey, Wildcard},
    },
};
use liana_ui::component::modal::LIANA_ACCOUNTS;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{airgap::error::Error, utils::derivation_path};

type SignerAddress = <RustBitcoin as Backend>::Address;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AirgappedDevice {
    pub fingerprint: Fingerprint,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    pub vendor: String,
    pub model: String,
    pub version: FirmwareVersion,
    pub capabilities: Capabilities,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub registrations: Vec<Registration>,
}

impl AirgappedDevice {
    pub fn new(signer: &Signer, alias: Option<String>) -> Self {
        Self {
            fingerprint: signer.fingerprint,
            alias,
            vendor: signer.vendor.clone(),
            model: signer.model.clone(),
            version: signer.version.clone(),
            capabilities: signer.capabilities,
            registrations: Vec::new(),
        }
    }

    pub fn registration(&self, descriptor_checksum: &str) -> Option<&Registration> {
        self.registrations
            .iter()
            .find(|registration| registration.is_for_descriptor(descriptor_checksum))
    }

    pub fn record(&mut self, registration: Registration) {
        if registration.descriptor_checksum.is_none() {
            return;
        }
        self.registrations
            .retain(|known| known.descriptor_checksum != registration.descriptor_checksum);
        self.registrations.push(registration);
    }

    pub fn merge(&mut self, other: AirgappedDevice) {
        for registration in other.registrations {
            self.record(registration);
        }
    }
}

pub fn save_device(devices: &mut Vec<AirgappedDevice>, device: AirgappedDevice) {
    match devices
        .iter_mut()
        .find(|known| known.fingerprint == device.fingerprint)
    {
        Some(known) => known.merge(device),
        None => devices.push(device),
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registration {
    #[serde(default)]
    pub descriptor_checksum: Option<String>,
    #[serde(default)]
    pub stored: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_proof",
        deserialize_with = "deserialize_proof"
    )]
    pub proof: Option<Vec<u8>>,
}

fn serialize_proof<S: Serializer>(
    proof: &Option<Vec<u8>>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    proof.as_deref().map(hex::encode).serialize(serializer)
}

fn deserialize_proof<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Vec<u8>>, D::Error> {
    Option::<String>::deserialize(deserializer)?
        .map(|proof| hex::decode(proof).map_err(serde::de::Error::custom))
        .transpose()
}

impl From<ll::response::Registration> for Registration {
    fn from(registration: ll::response::Registration) -> Self {
        Self {
            descriptor_checksum: None,
            stored: registration.stored,
            proof: registration.proof,
        }
    }
}

impl Registration {
    pub fn is_for_descriptor(&self, descriptor_checksum: &str) -> bool {
        self.descriptor_checksum.as_deref() == Some(descriptor_checksum)
    }
}

#[derive(Debug, Clone)]
pub struct RequestBuilder {
    pub alias: String,
    pub descriptor: LianaDescriptor,
    pub registration: Registration,
    pub capabilities: Capabilities,
}

impl RequestBuilder {
    fn descriptor_for_registration(&self) -> Result<request::DescriptorBody<RustBitcoin>, Error> {
        if self.capabilities.supports(Capability::Bip388WalletPolicies) {
            if let Some((policy, keys)) = self.bip388_policy() {
                return Ok(request::DescriptorBody::Bip388 { keys, policy });
            }
        }
        if self.capabilities.supports(Capability::Bip380Descriptors) {
            return Ok(request::DescriptorBody::Bip380(
                self.descriptor.descriptor().clone(),
            ));
        }
        Err(Error::UnsupportedDescriptor)
    }

    fn bip388_policy(&self) -> Option<(String, Vec<KeyInfo<RustBitcoin>>)> {
        let (policy, keys) =
            extract_keys_and_template::<DescriptorPublicKey>(&self.descriptor.to_string()).ok()?;
        let keys = keys
            .into_iter()
            .map(|key| match key {
                DescriptorPublicKey::XPub(key) => Some(KeyInfo {
                    origin: key
                        .origin
                        .map(|(fingerprint, path)| KeyOrigin { fingerprint, path }),
                    xpub: key.xkey,
                }),
                _ => None,
            })
            .collect::<Option<_>>()?;
        Some((policy, keys))
    }

    fn descriptor(&self) -> Result<Option<request::DescriptorBody<RustBitcoin>>, Error> {
        match self.registration.stored {
            Some(true) => Ok(None),
            _ => self.descriptor_for_registration().map(Some),
        }
    }

    fn proof(&self) -> Option<Vec<u8>> {
        self.registration.proof.clone()
    }

    pub fn register(&self) -> Result<request::Body<RustBitcoin>, Error> {
        Ok(request::Body::RegisterDescriptor(
            request::RegisterDescriptor {
                descriptor_alias: self.alias.clone(),
                descriptor: Some(self.descriptor_for_registration()?),
            },
        ))
    }

    pub fn verify_address(
        &self,
        address: &Address,
        index: ChildNumber,
    ) -> Result<request::Body<RustBitcoin>, Error> {
        Ok(request::Body::VerifyAddress(request::VerifyAddress {
            descriptor_alias: self.alias.clone(),
            derivation_path: DerivationPath::from(vec![ChildNumber::Normal { index: 0 }, index]),
            address: Some(SignerAddress::Bitcoin(address.as_unchecked().clone())),
            descriptor: self.descriptor()?,
            proof: self.proof(),
        }))
    }

    pub fn sign(&self, psbt: &Psbt) -> Result<request::Body<RustBitcoin>, Error> {
        Ok(request::Body::Sign(request::Sign {
            descriptors: vec![request::Descriptor {
                alias: self.alias.clone(),
                descriptor: self.descriptor()?,
                proof: self.proof(),
            }],
            psbt: psbt.clone(),
            want_kind: Some(SignResponseKind::Signatures),
            accept_trimmed: true,
        }))
    }
}

pub fn get_keys(network: Network) -> request::Body<RustBitcoin> {
    request::Body::GetKeys(request::GetKeys {
        keys: (0..LIANA_ACCOUNTS)
            .map(|index| derivation_path(network, ChildNumber::Hardened { index }))
            .collect(),
    })
}

fn process_response(
    request: &request::Body<RustBitcoin>,
    response: response::Body<RustBitcoin>,
) -> Result<Answer, Error> {
    match (request, response) {
        (request::Body::GetKeys(request), response::Body::Keys(keys)) => {
            Ok(Answer::Xpubs(Signer::from_keys(&request.keys, keys)?))
        }
        (request::Body::RegisterDescriptor(_), response::Body::Registration(registration)) => {
            if !registration.registered {
                return Err(Error::NotRegistered);
            }
            Ok(Answer::Registered(Registration::from(registration)))
        }
        (
            request::Body::VerifyAddress(request::VerifyAddress {
                address: Some(_), ..
            }),
            response::Body::Address(_),
        ) => Ok(Answer::AddressVerified),
        (request::Body::Sign(request), response::Body::Signed(signed)) => {
            Ok(Answer::Signed(merge(&request.psbt, signed)))
        }
        (request, response) => Err(Error::MessageTypeMismatch {
            expected: label(request.message_type()),
            received: label(response.message_type()),
        }),
    }
}

#[derive(Debug, Clone)]
pub enum Answer {
    Xpubs(Signer),
    Registered(Registration),
    AddressVerified,
    Signed(Psbt),
}

#[derive(Debug, Clone)]
pub struct Signer {
    pub fingerprint: Fingerprint,
    pub vendor: String,
    pub model: String,
    pub version: FirmwareVersion,
    pub capabilities: Capabilities,
    pub accounts: Vec<(ChildNumber, DescriptorPublicKey)>,
}

impl Signer {
    fn from_keys(
        paths: &[DerivationPath],
        keys: response::Keys<RustBitcoin>,
    ) -> Result<Self, Error> {
        let fingerprint = keys.fingerprint;
        let accounts = paths
            .iter()
            .zip(keys.keys)
            .map(|(path, xkey)| {
                let network = if path[1] == (ChildNumber::Hardened { index: 0 }) {
                    NetworkKind::Main
                } else {
                    NetworkKind::Test
                };
                if xkey.network != network {
                    return Err(Error::WrongNetwork);
                }
                let account = path[2];
                let key = DescriptorPublicKey::XPub(DescriptorXKey {
                    origin: Some((fingerprint, path.clone())),
                    derivation_path: DerivationPath::master(),
                    wildcard: Wildcard::None,
                    xkey,
                });
                Ok((account, key))
            })
            .collect::<Result<Vec<_>, Error>>()?;
        Ok(Self {
            fingerprint,
            vendor: keys.vendor,
            model: keys.model,
            version: keys.version,
            capabilities: keys.capabilities,
            accounts,
        })
    }
}

pub struct Exchange {
    request: Request<RustBitcoin>,
}

impl Exchange {
    pub fn new(request: request::Body<RustBitcoin>, id: RequestId) -> Self {
        Self {
            request: Request { id, body: request },
        }
    }

    pub fn payload(&self) -> Result<Vec<u8>, Error> {
        let bytes = RustBitcoin::encode_request(&self.request)?;
        log::debug!(
            "{} request {}, {} bytes",
            label(self.request.body.message_type()),
            hex::encode(self.request.id.0),
            bytes.len(),
        );
        log::trace!("request payload {}", hex::encode(&bytes));
        Ok(bytes)
    }

    pub fn read(&self, bytes: &[u8]) -> Result<Answer, Error> {
        log::debug!("scanned a response of {} bytes", bytes.len());
        log::trace!("response payload {}", hex::encode(bytes));
        self.check(bytes).inspect_err(|error| {
            log::warn!("rejected the message just scanned: {error}");
        })
    }

    fn check(&self, bytes: &[u8]) -> Result<Answer, Error> {
        let response = RustBitcoin::decode_response(bytes)?;
        response::validate(&self.request, &response)?;
        if let response::Body::Error(error) = response.body {
            return Err(Error::SignerRefused(error.error, error.message));
        }
        log::debug!(
            "read the {} response to {}",
            label(response.body.message_type()),
            hex::encode(response.id.0),
        );
        process_response(&self.request.body, response.body)
    }
}

pub fn label(message_type: MessageType) -> &'static str {
    match message_type {
        MessageType::GetKeys => "key export",
        MessageType::RegisterDescriptor => "wallet registration",
        MessageType::AddressVerification => "address verification",
        MessageType::Signing => "signing",
        MessageType::GetInfo => "device info",
        MessageType::NoncePreparation => "nonce preparation",
        MessageType::UnregisterDescriptor => "wallet unregistration",
        MessageType::UnreadableRequest => "unreadable request",
    }
}

fn merge(canonical: &Psbt, signed: Signed<RustBitcoin>) -> Psbt {
    let mut merged = canonical.clone();
    match signed {
        Signed::Psbt(returned) => merge_signatures(&mut merged, &returned),
        Signed::Signatures(entries) => {
            for entry in entries {
                merge_signature_entry(&mut merged, entry);
            }
        }
    }
    merged
}

fn merge_signature_entry(psbt: &mut Psbt, entry: SignatureEntry<RustBitcoin>) {
    let Some(input) = entry
        .input_index()
        .and_then(|index| psbt.inputs.get_mut(index as usize))
    else {
        return;
    };
    match entry {
        SignatureEntry::Ecdsa {
            public_key,
            signature,
            ..
        } => {
            input.partial_sigs.insert(public_key, signature);
        }
        SignatureEntry::TapKey { signature, .. } => {
            input.tap_key_sig = Some(signature);
        }
        SignatureEntry::TapScript {
            xonly_public_key,
            tap_leaf_hash,
            signature,
            ..
        } => {
            input
                .tap_script_sigs
                .insert((xonly_public_key, tap_leaf_hash), signature);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use bip377::{
        ll::response::{Capabilities, FirmwareVersion, ReleaseFlag},
        response::{SignatureEntry, Signed},
    };
    use liana::miniscript::bitcoin::{
        base64::{prelude::BASE64_STANDARD, Engine},
        bip32::Fingerprint,
        psbt::Psbt,
        taproot,
    };

    use crate::airgap::device::{merge, save_device, AirgappedDevice, Registration};

    fn device(registration: Registration) -> AirgappedDevice {
        let mut device = AirgappedDevice {
            fingerprint: Fingerprint::from_str("9f141cf0").unwrap(),
            alias: Some("Cold signer".to_owned()),
            vendor: "Vendor".to_owned(),
            model: "Model".to_owned(),
            version: FirmwareVersion {
                major: 1,
                minor: 2,
                patch: 3,
                flag: ReleaseFlag::Stable,
            },
            capabilities: Capabilities(7),
            registrations: Vec::new(),
        };
        device.record(registration);
        device
    }

    #[test]
    fn a_device_keeps_a_registration_per_descriptor() {
        let mut device = device(Registration {
            descriptor_checksum: Some("u768v50p".to_owned()),
            stored: Some(false),
            proof: Some(vec![7; 32]),
        });
        device.record(Registration {
            descriptor_checksum: Some("another".to_owned()),
            stored: Some(true),
            proof: None,
        });

        let json = serde_json::to_value(&device).unwrap();
        assert_eq!(json["registrations"][0]["proof"], "07".repeat(32));
        let read: AirgappedDevice = serde_json::from_value(json).unwrap();
        assert_eq!(read, device);

        let wallet = read.registration("u768v50p").unwrap();
        assert_eq!(wallet.proof, Some(vec![7; 32]));
        assert_eq!(wallet.stored, Some(false));
        let other = read.registration("another").unwrap();
        assert_eq!(other.stored, Some(true));
        assert_eq!(other.proof, None);
        assert_eq!(read.registration("unknown"), None);
    }

    #[test]
    fn saving_a_known_device_adds_its_registration() {
        let mut devices = vec![device(Registration {
            descriptor_checksum: Some("u768v50p".to_owned()),
            stored: Some(true),
            proof: None,
        })];
        save_device(
            &mut devices,
            device(Registration {
                descriptor_checksum: Some("another".to_owned()),
                stored: Some(false),
                proof: Some(vec![1; 32]),
            }),
        );
        save_device(&mut devices, device(Registration::default()));

        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].registrations.len(), 2);
        assert!(devices[0].registration("u768v50p").is_some());
    }

    #[test]
    fn merging_a_device_keeps_the_known_fields() {
        let mut known = device(Registration {
            descriptor_checksum: Some("u768v50p".to_owned()),
            stored: Some(true),
            proof: None,
        });
        let kept = known.clone();
        let mut incoming = device(Registration {
            descriptor_checksum: Some("another".to_owned()),
            stored: Some(false),
            proof: Some(vec![1; 32]),
        });
        incoming.alias = Some("From a backup".to_owned());
        incoming.vendor = String::new();
        incoming.model = String::new();
        incoming.version = FirmwareVersion {
            major: 0,
            minor: 9,
            patch: 0,
            flag: ReleaseFlag::Stable,
        };
        incoming.capabilities = Capabilities(0);

        known.merge(incoming);

        assert_eq!(known.alias, kept.alias);
        assert_eq!(known.vendor, kept.vendor);
        assert_eq!(known.model, kept.model);
        assert_eq!(known.version, kept.version);
        assert_eq!(known.capabilities, kept.capabilities);
        assert!(known.registration("u768v50p").is_some());
        assert_eq!(
            known.registration("another").unwrap().proof,
            Some(vec![1; 32])
        );
    }

    fn psbt(base64: &str) -> Psbt {
        Psbt::deserialize(&BASE64_STANDARD.decode(base64.trim()).unwrap()).unwrap()
    }

    fn unsigned() -> Psbt {
        psbt(include_str!(
            "../../test_assets/airgap/taproot-unsigned.psbt.base64"
        ))
    }

    fn returned_psbt() -> Psbt {
        psbt(include_str!(
            "../../test_assets/airgap/taproot-key-path-signed.psbt.base64"
        ))
    }

    fn returned_signature() -> taproot::Signature {
        returned_psbt().inputs[0]
            .tap_key_sig
            .expect("the fixture carries a key-path signature")
    }

    #[test]
    fn a_key_path_signature_merges() {
        let merged = merge(
            &unsigned(),
            Signed::Signatures(vec![SignatureEntry::TapKey {
                input_index: 0,
                signature: returned_signature(),
            }]),
        );
        assert_eq!(merged.inputs[0].tap_key_sig, Some(returned_signature()));
    }

    #[test]
    fn both_answer_forms_agree() {
        let from_signatures = merge(
            &unsigned(),
            Signed::Signatures(vec![SignatureEntry::TapKey {
                input_index: 0,
                signature: returned_signature(),
            }]),
        );
        let from_psbt = merge(&unsigned(), Signed::Psbt(returned_psbt()));
        assert_eq!(from_signatures, from_psbt);
    }
}
