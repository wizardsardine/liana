// Rust JSON-RPC Library
// Written in 2015 by
//     Andrew Poelstra <apoelstra@wpsoftware.net>
//
// To the extent possible under law, the author(s) have dedicated all
// copyright and related and neighboring rights to this software to
// the public domain worldwide. This software is distributed without
// any warranty.
//
// You should have received a copy of the CC0 Public Domain Dedication
// along with this software.
// If not, see <http://creativecommons.org/publicdomain/zero/1.0/>.
//

//! Error handling
//!
//! Some useful methods for creating Error objects
//!

use std::io;
use std::{error, fmt};

use serde::{Deserialize, Serialize};

#[allow(dead_code)]
#[derive(Debug)]
#[allow(non_camel_case_types)]
pub enum RpcErrorCode {
    // Standard errors defined by JSON-RPC 2.0 standard
    /// Invalid request
    JSONRPC2_INVALID_REQUEST = -32600,
    /// Method not found
    JSONRPC2_METHOD_NOT_FOUND = -32601,
    /// Invalid parameters
    JSONRPC2_INVALID_PARAMS = -32602,
}

/// A library error
#[derive(Debug)]
pub enum Error {
    /// Json error
    Json(serde_json::Error),
    /// IO Error
    Io(io::Error),
    /// Error response
    Rpc(RpcError),
    /// Response has neither error nor result
    NoErrorOrResult,
    /// Response to a request did not have the expected nonce
    NonceMismatch,
    /// Response to a request had a jsonrpc field other than "2.0"
    VersionMismatch,
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Error {
        Error::Json(e)
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Error {
        Error::Io(e)
    }
}

impl From<RpcError> for Error {
    fn from(e: RpcError) -> Error {
        Error::Rpc(e)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Error::Json(ref e) => write!(f, "JSON decode error: {}", e),
            Error::Io(ref e) => write!(f, "IO error response: {}", e),
            Error::Rpc(ref r) => write!(f, "RPC error response: {:?}", r),
            Error::NoErrorOrResult => write!(f, "Malformed RPC response"),
            Error::NonceMismatch => write!(f, "Nonce of response did not match nonce of request"),
            Error::VersionMismatch => write!(f, "`jsonrpc` field set to non-\"2.0\""),
        }
    }
}

impl error::Error for Error {
    fn cause(&self) -> Option<&dyn error::Error> {
        match *self {
            Error::Json(ref e) => Some(e),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
/// A JSONRPC error object
pub struct RpcError {
    /// The integer identifier of the error
    pub code: i32,
    /// A string describing the error
    pub message: String,
    /// Additional data specific to the error
    pub data: Option<serde_json::Value>,
}
