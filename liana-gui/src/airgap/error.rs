use std::fmt;

/// A protocol or bounded-decoder failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Cancelled,
    TimedOut,
    Empty,
    FragmentTooLarge {
        actual: usize,
        maximum: usize,
    },
    TooManyFragments {
        actual: u32,
        maximum: u32,
    },
    PayloadTooLarge {
        actual: usize,
        maximum: usize,
    },
    WrongUrType {
        expected: &'static str,
        actual: String,
    },
    MixedSession,
    Incomplete,
    InvalidUr(String),
    InvalidCbor(String),
    InvalidJson(String),
    JsonTooDeep {
        maximum: usize,
    },
    InvalidNetwork,
    InvalidPolicy(String),
    InvalidChecksum,
    InvalidFingerprint,
    InvalidAccount(String),
    InvalidPsbt(String),
    WrongResponseType,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => write!(f, "operation cancelled"),
            Self::TimedOut => write!(f, "QR scan session timed out"),
            Self::Empty => write!(f, "payload is empty"),
            Self::FragmentTooLarge { actual, maximum } => {
                write!(f, "QR fragment is {actual} bytes; the maximum is {maximum}")
            }
            Self::TooManyFragments { actual, maximum } => write!(
                f,
                "QR declares {actual} fragments; the maximum is {maximum}"
            ),
            Self::PayloadTooLarge { actual, maximum } => write!(
                f,
                "decoded payload is {actual} bytes; the maximum is {maximum}"
            ),
            Self::WrongUrType { expected, actual } => {
                write!(f, "expected UR type {expected}, received {actual}")
            }
            Self::MixedSession => write!(f, "QR fragment belongs to another scan session"),
            Self::Incomplete => write!(f, "QR sequence is incomplete"),
            Self::InvalidUr(e) => write!(f, "invalid UR: {e}"),
            Self::InvalidCbor(e) => write!(f, "invalid UR CBOR: {e}"),
            Self::InvalidJson(e) => write!(f, "invalid protocol JSON: {e}"),
            Self::JsonTooDeep { maximum } => {
                write!(f, "protocol JSON exceeds the nesting limit of {maximum}")
            }
            Self::InvalidNetwork => write!(f, "unsupported Bitcoin network"),
            Self::InvalidPolicy(e) => write!(f, "invalid wallet policy: {e}"),
            Self::InvalidChecksum => write!(f, "invalid descriptor checksum"),
            Self::InvalidFingerprint => write!(f, "invalid master fingerprint"),
            Self::InvalidAccount(e) => write!(f, "invalid air-gapped signer account: {e}"),
            Self::InvalidPsbt(e) => write!(f, "invalid PSBT: {e}"),
            Self::WrongResponseType => write!(f, "response is not valid for the active operation"),
        }
    }
}

impl std::error::Error for Error {}
