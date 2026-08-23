use bwk_qr::protocol::{decode, encode, response};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Transport(String),
    Encode(encode::Error),
    Decode(decode::Error),
    /// A request came back where a response was expected.
    NotAResponse,
    /// The request id did not echo. Almost always the answer to an earlier
    /// exchange, still on the signer's screen.
    RequestIdMismatch,
    MessageTypeMismatch {
        expected: &'static str,
        received: &'static str,
    },
    /// The signer answered with an error body.
    SignerRefused(response::Error, String),
    UnknownXpub(String),
    /// The descriptor cannot be expressed as a wallet policy.
    UnsupportedDescriptor,
    /// The signer answered with keys for another network.
    WrongNetwork,
    /// The signer verified an address other than the one Liana derived.
    AddressMismatch {
        expected: String,
        received: String,
    },
    InvalidPsbt(String),
    Camera(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Transport(e) => write!(f, "{e}"),
            Self::Encode(e) => write!(f, "Could not encode the request: {e}"),
            Self::Decode(e) => write!(f, "Could not read the response: {e}"),
            Self::NotAResponse => write!(f, "The device sent a request, not a response"),
            Self::RequestIdMismatch => write!(
                f,
                "This is the device's answer to an earlier request. Let it read the code above, \
                 then scan its new answer"
            ),
            Self::MessageTypeMismatch { expected, received } => {
                write!(
                    f,
                    "Expected a {expected} response, received a {received} one"
                )
            }
            Self::SignerRefused(error, message) if message.is_empty() => {
                write!(f, "The device refused: {}", refusal(*error))
            }
            Self::SignerRefused(_, message) => write!(f, "The device refused: {message}"),
            Self::UnknownXpub(e) => write!(f, "The device returned an unusable key: {e}"),
            Self::UnsupportedDescriptor => {
                write!(
                    f,
                    "This wallet's descriptor cannot be sent to an air-gapped signer"
                )
            }
            Self::WrongNetwork => write!(
                f,
                "The device is set to another network. Switch it and scan again"
            ),
            Self::AddressMismatch { expected, received } => write!(
                f,
                "The device verified {received}, but Liana expects {expected}"
            ),
            Self::InvalidPsbt(e) => write!(f, "{e}"),
            Self::Camera(e) => write!(f, "{e}"),
        }
    }
}

fn refusal(error: response::Error) -> &'static str {
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
        response::Error::Vendor | response::Error::Unknown(_) => "an unspecified error",
    }
}

impl From<bwk_qr::Error> for Error {
    fn from(value: bwk_qr::Error) -> Self {
        Self::Transport(value.to_string())
    }
}

impl From<encode::Error> for Error {
    fn from(value: encode::Error) -> Self {
        Self::Encode(value)
    }
}

impl From<decode::Error> for Error {
    fn from(value: decode::Error) -> Self {
        Self::Decode(value)
    }
}
