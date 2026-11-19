/// See https://github.com/Blockstream/Jade/blob/master/docs/index.rst
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use super::{JadeError, TransportError};

#[derive(Debug, Serialize, Deserialize)]
pub struct Request<'a, T: Serialize> {
    pub id: &'a str,
    pub method: &'a str,
    pub params: Option<T>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EmptyRequest;

#[derive(Debug, Serialize, Deserialize)]
pub struct Response<T> {
    pub id: String,
    pub seqlen: Option<u32>,
    pub seqnum: Option<u32>,
    pub result: Option<T>,
    pub error: Option<Error>,
}

impl<T> Response<T> {
    pub fn into_result(self) -> Result<T, JadeError> {
        if let Some(e) = self.error {
            return Err(JadeError::Rpc(e));
        }

        self.result
            .ok_or_else(|| TransportError::NoErrorOrResult.into())
    }
}

#[derive(Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum ErrorCode {
    InvalidRequest = -32600,
    UnknownMethod = -32601,
    BadParameters = -32602,
    InternalError = -32603,
    UserCancelled = -32000,
    ProtocolError = -32001,
    HwLocked = -32002,
    NetworkMismatch = -32003,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Error {
    pub code: i32,
    pub message: Option<String>,
    pub data: Option<Vec<u8>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetXpubParams<'a> {
    pub network: &'a str,
    pub path: Vec<u32>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AuthUserParams<'a> {
    pub network: &'a str,
    pub epoch: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AuthUserResponse {
    Authenticated(bool),
    PinServerRequired { http_request: PinServerRequest },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PinServerRequest {
    pub params: PinServerRequestParams,
    #[serde(alias = "on-reply")]
    pub onreply: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PinServerRequestParams {
    pub urls: PinServerUrls,
    pub method: String,
    pub accept: String,
    pub data: PinParams,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PinServerUrls {
    Array(Vec<String>),
    Object { url: String, onion: String },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PinParams {
    pub data: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetInfoResponse {
    #[serde(alias = "JADE_VERSION")]
    pub jade_version: String,
    #[serde(alias = "JADE_STATE")]
    pub jade_state: JadeState,
    #[serde(alias = "JADE_NETWORKS")]
    pub jade_networks: JadeNetworks,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum JadeState {
    #[serde(alias = "UNINIT")]
    Uninit,
    #[serde(alias = "UNSAVED")]
    Unsaved,
    #[serde(alias = "LOCKED")]
    Locked,
    #[serde(alias = "READY")]
    Ready,
    #[serde(alias = "TEMP")]
    Temp,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum JadeNetworks {
    #[serde(alias = "MAIN")]
    Main,
    #[serde(alias = "TEST")]
    Test,
    #[serde(alias = "ALL")]
    All,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DescriptorInfoResponse {
    pub descriptor_len: u32,
    pub num_datavalues: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetRegisteredDescriptorParams<'a> {
    pub descriptor_name: &'a str,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetRegisteredDescriptorResponse {
    pub descriptor_name: String,
    pub descriptor: String,
    pub datavalues: BTreeMap<String, String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetRegisteredMultisigParams<'a> {
    pub multisig_name: &'a str,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RegisterDescriptorParams<'a> {
    pub network: &'a str,
    pub descriptor_name: &'a str,
    pub descriptor: String,
    pub datavalues: BTreeMap<String, String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DescriptorAddressParams<'a> {
    pub network: &'a str,
    pub branch: u32,
    pub pointer: u32,
    pub descriptor_name: &'a str,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SignPsbtParams<'a> {
    pub network: &'a str,
    #[serde(with = "serde_bytes")]
    pub psbt: Vec<u8>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetExtendedDataParams<'a> {
    pub origid: &'a str,
    pub orig: &'a str,
    pub seqnum: u32,
    pub seqlen: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ResponseBytes {
    pub id: String,
    pub seqlen: Option<u32>,
    pub seqnum: Option<u32>,
    #[serde(with = "serde_bytes")]
    pub result: Option<Vec<u8>>,
    pub error: Option<Error>,
}
