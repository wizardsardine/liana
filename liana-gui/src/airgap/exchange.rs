use bip377::{
    backend::{Active, Backend},
    ll::{self, request, response, Message, MessageType, Request, RequestId, SignResponseKind},
};
use bwk_qr::{config::QrSize, encoder::Encoder, image::Image};
use liana::{
    descriptors::LianaDescriptor,
    miniscript::{
        bitcoin::{
            bip32::{ChildNumber, DerivationPath, Fingerprint},
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
        error::Error,
        merge,
    },
    utils::{check_key_network, derivation_path},
};

/// Accounts Liana asks for in one Get Keys exchange, so the user can pick one
/// without a second round trip.
pub const ACCOUNTS: u32 = 10;

/// What a signer needs replayed with every request. A signer that stores the
/// descriptor takes the alias alone; a stateless one needs the body and the
/// proof it issued at registration.
#[derive(Debug, Clone)]
pub struct Wallet {
    pub alias: String,
    pub descriptor: LianaDescriptor,
    pub registration: Registration,
    pub capabilities: Capabilities,
}

impl Wallet {
    /// The descriptor in a form the signer takes, a BIP-388 wallet policy when
    /// it can, a BIP-380 descriptor otherwise.
    ///
    /// A Liana descriptor names the same key in several spending paths, so a
    /// policy, sending the keys once rather than once per mention, takes a third
    /// off a multisig wallet. It also gives the signer the shape it displays for
    /// approval, instead of a descriptor it would have to take apart itself.
    fn body(&self) -> Result<request::DescriptorBody, Error> {
        let descriptor = self.descriptor.to_string();
        if self
            .capabilities
            .supports(response::Capability::Bip388WalletPolicies)
        {
            if let Ok((policy, keys)) = extract_keys_and_template::<String>(&descriptor) {
                return Ok(request::DescriptorBody::Bip388 { keys, policy });
            }
        }
        if self
            .capabilities
            .supports(response::Capability::Bip380Descriptors)
        {
            return Ok(request::DescriptorBody::Bip380(descriptor));
        }
        Err(Error::UnsupportedDescriptor)
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
            Self::Xpubs { .. } => MessageType::GetKeys,
            Self::Register { .. } => MessageType::RegisterDescriptor,
            Self::VerifyAddress { .. } => MessageType::AddressVerification,
            Self::Sign { .. } => MessageType::Signing,
        }
    }

    fn body(&self) -> Result<request::Body, Error> {
        Ok(match self {
            Self::Xpubs { network } => request::Body::GetKeys(request::GetKeys {
                keys: accounts(*network)
                    .map(|(_, path)| Active::to_ll_path(&path))
                    .collect(),
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
                derivation_path: Active::to_ll_path(&address_path(*change, *index)),
                address: Some(address.to_string()),
                descriptor: wallet.replayed_body()?,
                proof: wallet.proof(),
            }),
            Self::Sign { wallet, psbt } => request::Body::Sign(request::Sign {
                descriptors: vec![request::Descriptor {
                    alias: wallet.alias.clone(),
                    descriptor: wallet.replayed_body()?,
                    proof: wallet.proof(),
                }],
                psbt: psbt.serialize(),
                // Signatures alone are all Liana needs, and they keep the
                // answer small enough for far fewer frames. A signer is free to
                // return the whole PSBT instead, which is handled either way.
                want_kind: Some(SignResponseKind::Signatures),
                accept_trimmed: false,
            }),
        })
    }

    fn read(&self, body: response::Body) -> Result<Answer, Error> {
        match (self, body) {
            (Self::Xpubs { network }, response::Body::Keys(keys)) => {
                Ok(Answer::Xpubs(Signer::read(*network, keys)?))
            }
            (Self::Register { wallet }, response::Body::Registration(registration)) => {
                if !registration.registered {
                    return Err(Error::NotRegistered);
                }
                if registration.descriptor_alias != wallet.alias {
                    return Err(Error::AliasMismatch {
                        expected: wallet.alias.clone(),
                        received: registration.descriptor_alias,
                    });
                }
                Ok(Answer::Registered(Registration::read(registration)))
            }
            (Self::VerifyAddress { address, .. }, response::Body::AddressUri(uri)) => {
                check_address(address, &uri.uri)?;
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
    pub vendor: String,
    pub model: String,
    pub version: FirmwareVersion,
    pub capabilities: Capabilities,
    pub accounts: Vec<(ChildNumber, DescriptorPublicKey)>,
}

impl Signer {
    fn read(network: Network, keys: response::Keys) -> Result<Self, Error> {
        if keys.keys.len() != ACCOUNTS as usize {
            return Err(Error::UnknownXpub(format!(
                "{ACCOUNTS} account keys were asked for, {} were returned",
                keys.keys.len()
            )));
        }
        let fingerprint = Active::from_ll_fingerprint(&keys.fingerprint);
        let accounts = accounts(network)
            .zip(&keys.keys)
            .map(|((account, path), xpub)| {
                let xkey = Active::from_ll_xpub(xpub)
                    .map_err(|error| Error::UnknownXpub(error.to_string()))?;
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
        Ok(Self {
            fingerprint,
            vendor: keys.vendor,
            model: keys.model,
            version: FirmwareVersion::read(&keys.version),
            capabilities: Capabilities(keys.capabilities.0),
            accounts,
        })
    }
}

/// One request/response round trip: the frames Liana shows, and the decoder
/// reading the signer's answer back.
pub struct Exchange {
    ask: Ask,
    request: Request,
    bytes: Vec<u8>,
    frame_size: QrSize,
    frames: Vec<Image>,
}

impl Exchange {
    /// Every frame is drawn at `frame_size` and carries as many payload bytes as
    /// that version holds.
    pub fn new(ask: Ask, id: RequestId, frame_size: QrSize) -> Result<Self, Error> {
        let request = Request {
            id,
            body: ask.body()?,
        };
        let bytes = ll::encode_request(&request)?;
        log::debug!(
            "Air-gap: showing a {} request {}, {} bytes",
            label(request.body.message_type()),
            hex::encode(request.id.0),
            bytes.len(),
        );
        let mut exchange = Self {
            frames: Vec::new(),
            frame_size,
            bytes,
            ask,
            request,
        };
        exchange.reframe(frame_size)?;
        Ok(exchange)
    }

    /// Splits the same request, under the same id, into frames of another size.
    pub fn reframe(&mut self, frame_size: QrSize) -> Result<(), Error> {
        self.frames = Encoder::new(frame_size).encode_bbqr(&self.bytes)?;
        self.frame_size = frame_size;
        log::debug!(
            "Air-gap: request {} over {} QR frame(s) of version {}",
            hex::encode(self.request.id.0),
            self.frames.len(),
            frame_size.version(),
        );
        log_payload("request", &self.bytes, frame_size);
        Ok(())
    }

    pub fn frames(&self) -> &[Image] {
        &self.frames
    }

    /// Checks a message the scanner reassembled against the request that is on
    /// screen: it must be the response to this very request, and of its type.
    pub fn read(&self, bytes: &[u8]) -> Result<Answer, Error> {
        log::debug!("Air-gap: scanned a response of {} bytes", bytes.len());
        log_payload("response", bytes, self.frame_size);
        self.check(bytes).inspect_err(|error| {
            log::warn!("Air-gap: rejected the message just scanned: {error}");
        })
    }

    fn check(&self, bytes: &[u8]) -> Result<Answer, Error> {
        let response = match ll::decode(bytes)? {
            Message::Response(response) => response,
            Message::Request(_) => return Err(Error::NotAResponse),
        };
        // A signer that cannot read the request answers with the reserved id
        // instead of echoing ours.
        if response.id != self.request.id
            && response.body.message_type() != MessageType::UnreadableRequest
        {
            return Err(Error::RequestIdMismatch);
        }
        if let response::Body::Error(error) = response.body {
            // A refusal names the request it answers, or that it could not read one.
            if error.message_type != self.ask.message_type()
                && error.message_type != MessageType::UnreadableRequest
            {
                return Err(Error::MessageTypeMismatch {
                    expected: label(self.ask.message_type()),
                    received: label(error.message_type),
                });
            }
            return Err(Error::SignerRefused(error.error, error.message));
        }
        log::debug!(
            "Air-gap: read the {} response to {}",
            label(response.body.message_type()),
            hex::encode(response.id.0),
        );
        self.ask.read(response.body)
    }
}

/// The BIP48 account paths of one Get Keys request, in the order the signer
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

/// The signer answers with a BIP-321 payment URI, which must name the very
/// address Liana derived and asked about.
fn check_address(expected: &Address, uri: &str) -> Result<(), Error> {
    let malformed = || Error::InvalidAddressUri(uri.to_owned());
    let (scheme, rest) = uri.split_once(':').ok_or_else(malformed)?;
    if !scheme.eq_ignore_ascii_case("bitcoin") {
        return Err(malformed());
    }
    let (received, query) = match rest.split_once('?') {
        Some((address, query)) => (address, Some(query)),
        None => (rest, None),
    };
    if let Some(query) = query {
        let well_formed = query
            .split('&')
            .all(|pair| pair.split_once('=').is_some_and(|(key, _)| !key.is_empty()));
        if !well_formed {
            return Err(malformed());
        }
    }
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
/// They carry descriptors, extended public keys and PSBTs, so they are only
/// written at trace level, out of the logs users attach to bug reports.
fn log_payload(kind: &str, bytes: &[u8], frame_size: QrSize) {
    log::trace!("Air-gap: {kind} payload {}", hex::encode(bytes));
    match bwk_bbqr::split(bytes, frame_size.bbqr_part_bytes()) {
        Ok(split) => {
            let total = split.parts.len();
            for (index, part) in split.parts.iter().enumerate() {
                log::trace!("Air-gap: {kind} frame {}/{total} {part}", index + 1);
            }
        }
        Err(error) => log::warn!("Air-gap: could not frame the {kind} payload: {error}"),
    }
}

fn label(message_type: MessageType) -> &'static str {
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
