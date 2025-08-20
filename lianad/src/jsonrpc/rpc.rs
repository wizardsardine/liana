use crate::commands;

use std::{error, fmt};

use serde::{self, Deserialize, Deserializer, Serialize, Serializer};

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(untagged)]
pub enum Params {
    Array(Vec<serde_json::Value>),
    Map(serde_json::Map<String, serde_json::Value>),
}

impl Params {
    /// Get the parameter supposed to be at a given index / of a given name.
    pub fn get<Q>(&self, index: usize, name: &Q) -> Option<&serde_json::Value>
    where
        String: std::borrow::Borrow<Q>,
        Q: ?Sized + Ord + Eq + std::hash::Hash,
    {
        match self {
            Params::Array(vec) => vec.get(index),
            Params::Map(map) => map.get(name),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(untagged)]
pub enum ReqId {
    Num(u64),
    Str(String),
}

/// A JSONRPC2 request. See https://www.jsonrpc.org/specification#request_object.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    /// Version. Must be "2.0".
    pub jsonrpc: String,
    /// Command name.
    pub method: String,
    /// Command parameters.
    pub params: Option<Params>,
    /// Request identifier.
    pub id: ReqId,
}

/// A failure to broadcast a transaction to the P2P network.
const BROADCAST_ERROR: i64 = 1_000;
const REPLAY_ERROR: i64 = 1_001;

/// JSONRPC2 error codes. See https://www.jsonrpc.org/specification#error_object.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum ErrorCode {
    /// The method does not exist / is not available.
    MethodNotFound,
    /// Invalid method parameter(s).
    InvalidParams,
    /// Internal error while handling the command.
    InternalError,
    /// Reserved for implementation-defined server-errors.
    ServerError(i64),
}

impl From<&ErrorCode> for i64 {
    fn from(code: &ErrorCode) -> i64 {
        match code {
            ErrorCode::MethodNotFound => -32601,
            ErrorCode::InvalidParams => -32602,
            ErrorCode::InternalError => -32603,
            ErrorCode::ServerError(code) => *code,
        }
    }
}

impl From<i64> for ErrorCode {
    fn from(code: i64) -> ErrorCode {
        match code {
            -32601 => ErrorCode::MethodNotFound,
            -32602 => ErrorCode::InvalidParams,
            -32603 => ErrorCode::InternalError,
            code => ErrorCode::ServerError(code),
        }
    }
}

impl<'a> Deserialize<'a> for ErrorCode {
    fn deserialize<D>(deserializer: D) -> Result<ErrorCode, D::Error>
    where
        D: Deserializer<'a>,
    {
        let code: i64 = Deserialize::deserialize(deserializer)?;
        Ok(code.into())
    }
}

impl Serialize for ErrorCode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_i64(self.into())
    }
}

/// JSONRPC2 error response. See https://www.jsonrpc.org/specification#error_object.
#[derive(Debug, PartialEq, Eq, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Error {
    pub code: ErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl Error {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Error {
        Error {
            message: message.into(),
            code,
            data: None,
        }
    }

    pub fn method_not_found() -> Error {
        Error::new(ErrorCode::MethodNotFound, "Method not found")
    }

    pub fn invalid_params(message: impl Into<String>) -> Error {
        Error::new(
            ErrorCode::InvalidParams,
            format!("Invalid params: {}", message.into()),
        )
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let code: i64 = (&self.code).into();
        write!(f, "{}: {}", code, self.message)
    }
}

impl error::Error for Error {}

impl From<commands::CommandError> for Error {
    fn from(e: commands::CommandError) -> Error {
        match e {
            commands::CommandError::NoOutpointForSelfSend
            | commands::CommandError::UnknownOutpoint(..)
            | commands::CommandError::InvalidFeerate(..)
            | commands::CommandError::AlreadySpent(..)
            | commands::CommandError::ImmatureCoinbase(..)
            | commands::CommandError::Address(..)
            | commands::CommandError::SpendCreation(..)
            | commands::CommandError::InsufficientFunds(..)
            | commands::CommandError::UnknownSpend(..)
            | commands::CommandError::SpendFinalization(..)
            | commands::CommandError::InsaneRescanTimestamp(..)
            | commands::CommandError::AlreadyRescanning
            | commands::CommandError::InvalidDerivationIndex
            | commands::CommandError::RbfError(..)
            | commands::CommandError::EmptyFilterList
            | commands::CommandError::RecoveryNotAvailable
            | commands::CommandError::OutpointNotRecoverable(..)
            | commands::CommandError::FailedToFetchOhttpKeys(..) => {
                Error::new(ErrorCode::InvalidParams, e.to_string())
            }
            commands::CommandError::RescanTrigger(..) => {
                Error::new(ErrorCode::InternalError, e.to_string())
            }
            commands::CommandError::TxBroadcast(_) => {
                Error::new(ErrorCode::ServerError(BROADCAST_ERROR), e.to_string())
            }
            commands::CommandError::FailedToPostOriginalPayjoinProposal(_) => {
                Error::new(ErrorCode::ServerError(BROADCAST_ERROR), e.to_string())
            }
            commands::CommandError::ReplayError(_) => {
                Error::new(ErrorCode::ServerError(REPLAY_ERROR), e.to_string())
            }
        }
    }
}

/// JSONRPC2 response. See https://www.jsonrpc.org/specification#response_object.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    /// Version. Must be "2.0".
    jsonrpc: String,
    /// Required on success. Must not exist on error.
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    /// Required on error. Must not exist on success.
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<Error>,
    /// Request identifier.
    id: ReqId,
}

impl Response {
    fn new(id: ReqId, result: Option<serde_json::Value>, error: Option<Error>) -> Response {
        Response {
            jsonrpc: "2.0".to_string(),
            result,
            error,
            id,
        }
    }

    pub fn success(id: ReqId, result: serde_json::Value) -> Response {
        Response::new(id, Some(result), None)
    }

    pub fn error(id: ReqId, error: Error) -> Response {
        Response::new(id, None, Some(error))
    }
}
