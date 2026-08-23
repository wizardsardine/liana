use std::sync::Arc;

use bip377::{
    backend::rust_bitcoin,
    ll::{decode, encode, response},
    response::Mismatch,
};
use liana::miniscript::bitcoin::{
    bip32::{self, Fingerprint},
    psbt,
};

use crate::airgap::{camera::CameraFailure, device::label};

#[derive(Debug, Clone, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Transport(#[from] bwk_qr::error::Error),
    #[error("Could not encode the request: {0}")]
    Encode(#[from] encode::Error),
    #[error("Could not read the response: {0}")]
    Decode(#[from] decode::Error),
    #[error("The device sent a request, not a response")]
    NotAResponse,
    #[error(
        "This is the device's answer to an earlier request. Let it read the code above, \
         then scan its new answer"
    )]
    RequestIdMismatch,
    #[error("Expected a {expected} response, received a {received} one")]
    MessageTypeMismatch {
        expected: &'static str,
        received: &'static str,
    },
    #[error("The device refused: {}", refusal(.0, .1))]
    SignerRefused(response::Error, String),
    #[error("The device returned an unusable key: {0}")]
    UnknownXpub(bip32::Error),
    #[error("The device reported the wallet as not registered")]
    NotRegistered,
    #[error("This wallet's descriptor cannot be sent to this device")]
    UnsupportedDescriptor,
    #[error("The device is set to another network. Switch it and scan again")]
    WrongNetwork,
    #[error("{}", mismatch(.0))]
    Mismatch(Mismatch),
    #[error("The device answered with an unreadable payment URI: {0}")]
    InvalidAddressUri(Arc<rust_bitcoin::Error>),
    #[error("The device returned an unreadable PSBT: {0}")]
    InvalidPsbt(Arc<psbt::Error>),
    #[error("The device answered with unreadable data: {0}")]
    Unreadable(Arc<rust_bitcoin::Error>),
    #[error(transparent)]
    Camera(CameraFailure),
    #[error("The device did not return a signed transaction")]
    NotSigned,
    #[error("The transaction came back without any new signature")]
    NoNewSignature,
    #[error("The signing device {0} is not saved")]
    NotSaved(Fingerprint),
}

fn refusal<'a>(error: &response::Error, message: &'a str) -> &'a str {
    if !message.is_empty() {
        return message;
    }
    match error {
        response::Error::UserDeclined => "the request was declined on the device",
        response::Error::UnsupportedVersion => "the protocol version is not supported",
        response::Error::MalformedRequest => "the request was malformed",
        response::Error::UnknownDescriptorAlias => "this wallet is not registered on the device",
        response::Error::DescriptorRegistrationFailed => "registering the wallet failed",
        response::Error::UnsupportedDescriptorForm => "the descriptor form is not supported",
        response::Error::InvalidProof => "the proof of registration was rejected",
        response::Error::AddressMismatch => "the address did not match",
        response::Error::NothingToSign => "the device holds no key for any input",
        response::Error::InvalidPsbt => "the transaction could not be read",
        response::Error::InternalError => "the device reported an internal error",
        response::Error::MusigSession => "the signing session is missing or does not match",
        response::Error::AlreadyRegistered => "a wallet is already registered under this name",
        response::Error::UnsupportedOperation => "the device does not support this operation",
        response::Error::NetworkMismatch => "the device is set to another network",
        response::Error::Vendor | response::Error::Unknown(_) => "an unspecified error",
    }
}

fn mismatch(mismatch: &Mismatch) -> String {
    match mismatch {
        Mismatch::KeyCount { expected, received } => {
            format!("The device returned {received} keys, but Liana asked for {expected}")
        }
        Mismatch::Alias { expected, received } => {
            format!("The device registered the wallet as {received}, but Liana sent {expected}")
        }
        Mismatch::Address { expected, received } => {
            format!("The device verified {received}, but Liana expects {expected}")
        }
        Mismatch::InputIndex { index, inputs } => {
            format!("The device signed input {index}, but the transaction has {inputs} inputs")
        }
        other => other.to_string(),
    }
}

impl From<rust_bitcoin::Error> for Error {
    fn from(value: rust_bitcoin::Error) -> Self {
        match value {
            rust_bitcoin::Error::Encode(e) => Self::Encode(e),
            rust_bitcoin::Error::Decode(e) => Self::Decode(e),
            rust_bitcoin::Error::NotAResponse => Self::NotAResponse,
            rust_bitcoin::Error::Mismatch(Mismatch::RequestId { .. }) => Self::RequestIdMismatch,
            rust_bitcoin::Error::Mismatch(Mismatch::MessageType { expected, received }) => {
                Self::MessageTypeMismatch {
                    expected: label(expected),
                    received: label(received),
                }
            }
            rust_bitcoin::Error::Mismatch(mismatch) => Self::Mismatch(mismatch),
            e @ rust_bitcoin::Error::AddressUri(_) => Self::InvalidAddressUri(Arc::new(e)),
            rust_bitcoin::Error::ExtendedKey(e) => Self::UnknownXpub(e),
            rust_bitcoin::Error::Psbt(e) => Self::InvalidPsbt(Arc::new(e)),
            e => Self::Unreadable(Arc::new(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use bip377::backend::rust_bitcoin;

    use crate::airgap::error::Error;

    #[test]
    fn an_unreadable_answer_names_its_error() {
        let error = Error::from(rust_bitcoin::Error::UncompressedPublicKey);
        assert!(matches!(error, Error::Unreadable(_)));
        assert_eq!(
            error.to_string(),
            "The device answered with unreadable data: public key is not compressed"
        );
    }
}
