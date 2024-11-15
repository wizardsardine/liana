// Rust JSON-RPC Library
// Written by
//     Andrew Poelstra <apoelstra@wpsoftware.net>
//     Wladimir J. van der Laan <laanwj@gmail.com>
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
//! Client support
//!
//! Support for connecting to JSONRPC servers over UNIX socets, sending requests,
//! and parsing responses
//!

#[cfg(not(windows))]
use std::os::unix::net::UnixStream;

use std::fmt::Debug;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::{error, fmt, io};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Deserializer;

use tracing::debug;

/// A handle to a remote JSONRPC server
#[derive(Debug, Clone)]
pub struct JsonRPCClient {
    sockpath: PathBuf,
    timeout: Option<Duration>,
}

impl super::Client for JsonRPCClient {
    type Error = Error;
    fn request<S: Serialize + Debug, D: DeserializeOwned + Debug>(
        &self,
        method: &str,
        params: Option<S>,
    ) -> Result<D, Self::Error> {
        self.send_request(method, params)
            .and_then(|res| res.into_result())
    }
}

impl JsonRPCClient {
    /// Creates a new client
    pub fn new<P: AsRef<Path>>(sockpath: P) -> JsonRPCClient {
        JsonRPCClient {
            sockpath: sockpath.as_ref().to_path_buf(),
            timeout: None,
        }
    }

    /// Set an optional timeout for requests
    #[allow(dead_code)]
    pub fn set_timeout(&mut self, timeout: Option<Duration>) {
        self.timeout = timeout;
    }

    #[cfg(windows)]
    pub fn send_request<S: Serialize + Debug, D: DeserializeOwned + Debug>(
        &self,
        method: &str,
        params: Option<S>,
    ) -> Result<Response<D>, Error> {
        Err(Error::NotSupported)
    }

    /// Sends a request to a client
    #[cfg(not(windows))]
    pub fn send_request<S: Serialize + Debug, D: DeserializeOwned + Debug>(
        &self,
        method: &str,
        params: Option<S>,
    ) -> Result<Response<D>, Error> {
        // Setup connection
        let mut stream = UnixStream::connect(&self.sockpath)?;
        stream.set_read_timeout(self.timeout)?;
        stream.set_write_timeout(self.timeout)?;

        let request = Request {
            method,
            params,
            id: std::process::id(),
            jsonrpc: "2.0",
        };

        debug!("Sending to lianad: {:#?}", request);

        stream.write_all(&[serde_json::to_string(&request).unwrap().as_bytes(), b"\n"].concat())?;

        let response: Response<D> = Deserializer::from_reader(&mut stream)
            .into_iter()
            .next()
            .map_or(Err(Error::NoErrorOrResult), |res| Ok(res?))?;
        if response
            .jsonrpc
            .as_ref()
            .map_or(false, |version| version != "2.0")
        {
            return Err(Error::VersionMismatch);
        }

        if response.id != request.id {
            return Err(Error::NonceMismatch);
        }

        debug!("Received from lianad: {:#?}", response);

        Ok(response)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
/// A JSONRPC request object
pub struct Request<'f, T: Serialize> {
    /// The name of the RPC call
    pub method: &'f str,
    /// Parameters to the RPC call
    pub params: Option<T>,
    /// Identifier for this Request, which should appear in the response
    pub id: u32,
    /// jsonrpc field, MUST be "2.0"
    pub jsonrpc: &'f str,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
/// A JSONRPC response object
pub struct Response<T> {
    /// A result if there is one, or null
    pub result: Option<T>,
    /// An error if there is one, or null
    pub error: Option<RpcError>,
    /// Identifier for this Request, which should match that of the request
    pub id: u32,
    /// jsonrpc field, MUST be "2.0"
    pub jsonrpc: Option<String>,
}

impl<T> Response<T> {
    /// Extract the result from a response, consuming the response
    pub fn into_result(self) -> Result<T, Error> {
        if let Some(e) = self.error {
            return Err(Error::Rpc(e));
        }

        self.result.ok_or(Error::NoErrorOrResult)
    }

    /// Returns whether or not the `result` field is empty
    #[allow(dead_code)]
    pub fn is_none(&self) -> bool {
        self.result.is_none()
    }
}

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
    /// unix socket communication is not supported.
    NotSupported,
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
            Error::NotSupported => write!(f, "unix socket communication is not supported"),
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

impl From<Error> for super::DaemonError {
    fn from(e: Error) -> super::DaemonError {
        match e {
            Error::Io(e) => super::DaemonError::RpcSocket(Some(e.kind()), format!("io: {:?}", e)),
            Error::Json(e) => super::DaemonError::RpcSocket(None, format!("json decode: {}", e)),
            Error::NonceMismatch => {
                super::DaemonError::RpcSocket(None, format!("transport: {}", e))
            }
            Error::VersionMismatch => {
                super::DaemonError::RpcSocket(None, format!("transport: {}", e))
            }
            Error::NoErrorOrResult => super::DaemonError::NoAnswer,
            Error::NotSupported => super::DaemonError::ClientNotSupported,
            Error::Rpc(e) => super::DaemonError::Rpc(e.code, e.message),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
/// A JSONRPC error object
pub struct RpcError {
    /// The integer identifier of the error
    pub code: i32,
    /// A string describing the error
    pub message: String,
    /// Additional data specific to the error
    pub data: Option<serde_json::Value>,
}
