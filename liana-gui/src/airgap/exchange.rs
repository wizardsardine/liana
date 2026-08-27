use std::convert::TryFrom;

use bwk_qr::{
    bbqr,
    protocol::{
        self, request, response, Message, MessageType, Request, RequestId, SignResponseKind,
    },
    Config, Encoder, Image,
};
use liana::{
    descriptors::LianaDescriptor,
    miniscript::{
        bitcoin::{
            bip32::{ChildNumber, DerivationPath, Fingerprint, Xpub},
            psbt::Psbt,
            Address, Network,
        },
        descriptor::{DescriptorPublicKey, DescriptorXKey, Wildcard},
    },
};

use async_hwi::utils::extract_keys_and_template;

use crate::{
    airgap::{
        device::{Capabilities, FirmwareVersion, Registration},
        merge, Error,
    },
    utils::{check_key_network, derivation_path},
};

/// Accounts Liana asks for in one Get Xpubs exchange, so the user can pick one
/// without a second round trip.
pub const ACCOUNTS: u32 = 10;

/// The QR settings every exchange uses: the sparsest codes the transport
/// offers, so the widest range of cameras can read them. A denser code would
/// need fewer frames, but a signer that cannot read it has no way to say so.
///
/// A part is pinned to the version, so it has to fit the hex-doubled payload
/// plus the eight header characters at error correction Low.
fn config() -> Config {
    Config {
        max_qr_version: 11,
        bbqr_part_bytes: 200,
        // The frames come from our own camera, so this only has to be wide
        // enough for the largest mode one may negotiate. Leave it at the
        // default and a camera that only offers 1080p is rejected frame after
        // frame with nothing on screen to explain why.
        max_image_pixels: 1920 * 1080,
        ..Config::default()
    }
}

/// What a signer needs replayed with every request. A signer that stores the
/// descriptor takes the alias alone; a stateless one needs the body and the
/// proof it issued at registration.
#[derive(Debug, Clone)]
pub struct Wallet {
    pub alias: String,
    pub descriptor: LianaDescriptor,
    pub registration: Registration,
}

impl Wallet {
    /// The descriptor as a BIP-388 wallet policy: the keys once each, and a
    /// template referring to them by position.
    ///
    /// A Liana descriptor names the same key in several spending paths, so
    /// sending the keys once rather than once per mention takes a third off a
    /// multisig wallet. It also gives the signer the shape it displays for
    /// approval, instead of a descriptor it would have to take apart itself.
    fn body(&self) -> Result<request::DescriptorBody, Error> {
        let (policy, keys) = extract_keys_and_template::<String>(&self.descriptor.to_string())
            .map_err(|_| Error::UnsupportedDescriptor)?;
        Ok(request::DescriptorBody::Bip388 { keys, policy })
    }

    /// The body is resent unless the signer told us it stored the descriptor.
    fn replayed_body(&self) -> Result<Option<request::DescriptorBody>, Error> {
        match self.registration.stored {
            Some(true) => Ok(None),
            _ => self.body().map(Some),
        }
    }

    fn proof(&self) -> Option<Vec<u8>> {
        self.registration.proof.clone()
    }
}

/// What Liana asks an air-gapped signer, together with what it needs to check
/// the answer against.
#[derive(Debug, Clone)]
pub enum Ask {
    Xpubs {
        network: Network,
    },
    Register {
        wallet: Wallet,
    },
    VerifyAddress {
        wallet: Wallet,
        address: Address,
        change: bool,
        index: ChildNumber,
    },
    Sign {
        wallet: Wallet,
        psbt: Psbt,
    },
}

impl Ask {
    fn message_type(&self) -> MessageType {
        match self {
            Self::Xpubs { .. } => MessageType::GetXpubs,
            Self::Register { .. } => MessageType::RegisterDescriptor,
            Self::VerifyAddress { .. } => MessageType::AddressVerification,
            Self::Sign { .. } => MessageType::Signing,
        }
    }

    fn body(&self) -> Result<request::Body, Error> {
        Ok(match self {
            Self::Xpubs { network } => request::Body::GetXpubs(request::GetXpubs {
                derivation_paths: accounts(*network).map(|(_, path)| (&path).into()).collect(),
            }),
            Self::Register { wallet } => {
                request::Body::RegisterDescriptor(request::RegisterDescriptor {
                    descriptor_alias: wallet.alias.clone(),
                    descriptor: Some(wallet.body()?),
                })
            }
            Self::VerifyAddress {
                wallet,
                address,
                change,
                index,
            } => request::Body::VerifyAddress(request::VerifyAddress {
                descriptor_alias: wallet.alias.clone(),
                derivation_path: (&address_path(*change, *index)).into(),
                address: Some(address.to_string()),
                descriptor: wallet.replayed_body()?,
                proof: wallet.proof(),
            }),
            Self::Sign { wallet, psbt } => request::Body::Sign(request::Sign {
                descriptors: vec![request::Descriptor {
                    alias: wallet.alias.clone(),
                    body: wallet.body()?,
                    proof: wallet.proof(),
                }],
                psbt: psbt.serialize(),
                // Signatures alone are all Liana needs, and they keep the
                // answer small enough for far fewer frames. A signer is free to
                // return the whole PSBT instead, which is handled either way.
                want_kind: Some(SignResponseKind::Signatures),
            }),
        })
    }

    fn read(&self, body: response::Body) -> Result<Answer, Error> {
        match (self, body) {
            (Self::Xpubs { network }, response::Body::Xpubs(xpubs)) => {
                Ok(Answer::Xpubs(Signer::read(*network, xpubs)?))
            }
            (Self::Register { .. }, response::Body::Registration(registration)) => {
                Ok(Answer::Registered(Registration::read(registration)))
            }
            (Self::VerifyAddress { address, .. }, response::Body::AddressUri(uri)) => {
                check_address(address, uri.uri.as_deref())?;
                Ok(Answer::AddressVerified)
            }
            (Self::Sign { psbt, .. }, response::Body::Signed(signed)) => {
                Ok(Answer::Signed(merge::apply(psbt, signed)?))
            }
            (ask, body) => Err(Error::MessageTypeMismatch {
                expected: label(ask.message_type()),
                received: label(body.message_type()),
            }),
        }
    }
}

/// What a signer answered, once checked against the request.
#[derive(Debug, Clone)]
pub enum Answer {
    Xpubs(Signer),
    Registered(Registration),
    AddressVerified,
    Signed(Psbt),
}

/// The account keys a signer returned, with the device it reported itself as.
#[derive(Debug, Clone)]
pub struct Signer {
    pub fingerprint: Fingerprint,
    pub model: String,
    pub version: FirmwareVersion,
    pub capabilities: Capabilities,
    pub accounts: Vec<(ChildNumber, DescriptorPublicKey)>,
}

impl Signer {
    fn read(network: Network, xpubs: response::Xpubs) -> Result<Self, Error> {
        let fingerprint = (&xpubs.fingerprint).into();
        let accounts = accounts(network)
            .zip(&xpubs.xpubs)
            .map(|((account, path), xpub)| {
                let xkey =
                    Xpub::try_from(xpub).map_err(|error| Error::UnknownXpub(error.to_string()))?;
                let key = DescriptorPublicKey::XPub(DescriptorXKey {
                    origin: Some((fingerprint, path)),
                    derivation_path: DerivationPath::master(),
                    wildcard: Wildcard::None,
                    xkey,
                });
                // A signer left on the wrong network answers with keys that look
                // fine but belong to another chain, so refuse the whole answer
                // rather than build a descriptor that can never be spent.
                if !check_key_network(&key, network) {
                    return Err(Error::WrongNetwork);
                }
                Ok((account, key))
            })
            .collect::<Result<Vec<_>, Error>>()?;
        if accounts.is_empty() {
            return Err(Error::UnknownXpub("no account key was returned".to_owned()));
        }
        Ok(Self {
            fingerprint,
            model: xpubs.model,
            version: FirmwareVersion::read(&xpubs.version),
            capabilities: Capabilities(xpubs.capabilities.0),
            accounts,
        })
    }
}

/// One request/response round trip: the frames Liana shows, and the decoder
/// reading the signer's answer back.
pub struct Exchange {
    ask: Ask,
    request: Request,
    frames: Vec<Image>,
}

impl Exchange {
    pub fn new(ask: Ask, id: RequestId) -> Result<Self, Error> {
        let request = Request {
            id,
            body: ask.body()?,
        };
        let bytes = protocol::encode_request(&request)?;
        let frames = Encoder::new(config())?.encode_bytes(&bytes)?;
        log::info!(
            "Air-gap: showing a {} request {}, {} bytes over {} QR frame(s)",
            label(request.body.message_type()),
            hex::encode(request.id.0),
            bytes.len(),
            frames.len(),
        );
        log_payload("request", &bytes);
        Ok(Self {
            frames,
            ask,
            request,
        })
    }

    pub fn frames(&self) -> &[Image] {
        &self.frames
    }

    /// How the scanner must be configured to read the answer to this request.
    pub fn scan_config(&self) -> Config {
        config()
    }

    /// Checks a message the scanner reassembled against the request that is on
    /// screen: it must be the response to this very request, and of its type.
    pub fn read(&self, bytes: &[u8]) -> Result<Answer, Error> {
        log_payload("response", bytes);
        self.check(bytes).inspect_err(|error| {
            log::warn!("Air-gap: rejected the message just scanned: {error}");
        })
    }

    fn check(&self, bytes: &[u8]) -> Result<Answer, Error> {
        let response = match protocol::decode(bytes)? {
            Message::Response(response) => response,
            Message::Request(_) => return Err(Error::NotAResponse),
        };
        if response.id != self.request.id {
            return Err(Error::RequestIdMismatch);
        }
        if let response::Body::Error(error) = response.body {
            return Err(Error::SignerRefused(error.error, error.message));
        }
        log::info!(
            "Air-gap: read the {} response to {}",
            label(response.body.message_type()),
            hex::encode(response.id.0),
        );
        self.ask.read(response.body)
    }
}

/// The BIP48 account paths of one Get Xpubs request, in the order the signer
/// must answer them.
fn accounts(network: Network) -> impl Iterator<Item = (ChildNumber, DerivationPath)> {
    (0..ACCOUNTS).map(move |index| {
        let account = ChildNumber::Hardened { index };
        (account, derivation_path(network, account))
    })
}

/// The path under the descriptor an address is derived at.
fn address_path(change: bool, index: ChildNumber) -> DerivationPath {
    vec![
        ChildNumber::Normal {
            index: change.into(),
        },
        index,
    ]
    .into()
}

/// The signer answers with a BIP-21 URI, which must name the very address Liana
/// derived and asked about.
fn check_address(expected: &Address, uri: Option<&str>) -> Result<(), Error> {
    let uri = uri.ok_or_else(|| Error::AddressMismatch {
        expected: expected.to_string(),
        received: "nothing".to_owned(),
    })?;
    let received = uri
        .strip_prefix("bitcoin:")
        .unwrap_or(uri)
        .split(['?', '&'])
        .next()
        .unwrap_or_default();
    if !received.eq_ignore_ascii_case(&expected.to_string()) {
        return Err(Error::AddressMismatch {
            expected: expected.to_string(),
            received: received.to_owned(),
        });
    }
    Ok(())
}

/// The whole message, and the frames it is carried in, so a failing exchange can
/// be replayed from the log alone. Both are decodable by hand: the payload
/// against the protocol spec, the frames as the signer's camera sees them.
///
/// This writes descriptors, extended public keys and PSBTs to the log file at
/// the default level, so that a failing exchange is always recoverable from a
/// log the user already has, without asking them to reproduce it.
fn log_payload(kind: &str, bytes: &[u8]) {
    log::info!("Air-gap: {kind} payload {}", hex::encode(bytes));
    match bbqr::split(bytes, config().bbqr_part_bytes) {
        Ok(split) => {
            let total = split.parts.len();
            for (index, part) in split.parts.iter().enumerate() {
                log::info!("Air-gap: {kind} frame {}/{total} {part}", index + 1);
            }
        }
        Err(error) => log::warn!("Air-gap: could not frame the {kind} payload: {error}"),
    }
}

fn label(message_type: MessageType) -> &'static str {
    match message_type {
        MessageType::GetXpubs => "key export",
        MessageType::RegisterDescriptor => "wallet registration",
        MessageType::AddressVerification => "address verification",
        MessageType::Signing => "signing",
    }
}
