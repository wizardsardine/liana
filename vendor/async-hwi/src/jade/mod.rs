pub mod api;
pub mod pinserver;

use std::{
    collections::BTreeMap,
    fmt::Debug,
    str::FromStr,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{de::DeserializeOwned, Serialize};

use bitcoin::{
    bip32::{DerivationPath, Fingerprint, Xpub},
    psbt::Psbt,
    Network,
};

use serialport::{available_ports, SerialPort, SerialPortType};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    sync::Mutex,
};
use tokio_serial::SerialPortBuilderExt;

pub use tokio_serial::SerialStream;

use crate::{parse_version, utils};

use super::{AddressScript, DeviceKind, Error as HWIError, WalletRegistration, HWI};
use async_trait::async_trait;

pub const JADE_NETWORK_MAINNET: &str = "mainnet";
pub const JADE_NETWORK_TESTNET: &str = "testnet";

#[derive(Debug)]
pub struct Jade<T> {
    transport: T,
    network: &'static str,
    kind: DeviceKind,
    descriptor_name: Option<String>,
}

impl<T: Transport + Sync + Send> Jade<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            network: JADE_NETWORK_MAINNET,
            kind: DeviceKind::Jade,
            descriptor_name: None,
        }
    }

    pub fn with_network(mut self, network: Network) -> Self {
        if network == Network::Bitcoin {
            self.network = JADE_NETWORK_MAINNET;
        } else {
            self.network = JADE_NETWORK_TESTNET;
        }
        self
    }

    pub fn with_wallet(mut self, descriptor_name: String) -> Self {
        self.descriptor_name = Some(descriptor_name);
        self
    }

    pub async fn ping(&self) -> Result<(), JadeError> {
        let _res: u64 = self
            .transport
            .request("ping", Option::<api::EmptyRequest>::None)
            .await?
            .into_result()?;
        Ok(())
    }

    pub async fn get_info(&self) -> Result<api::GetInfoResponse, HWIError> {
        let info: api::GetInfoResponse = self
            .transport
            .request("get_version_info", Option::<api::EmptyRequest>::None)
            .await?
            .into_result()?;
        Ok(info)
    }

    pub async fn get_registered_descriptors(
        &self,
    ) -> Result<BTreeMap<String, api::DescriptorInfoResponse>, HWIError> {
        let descriptors: BTreeMap<String, api::DescriptorInfoResponse> = self
            .transport
            .request(
                "get_registered_descriptors",
                Option::<api::EmptyRequest>::None,
            )
            .await?
            .into_result()?;
        Ok(descriptors)
    }

    pub async fn get_registered_descriptor(
        &self,
        name: &str,
    ) -> Result<api::GetRegisteredDescriptorResponse, HWIError> {
        let registered: api::GetRegisteredDescriptorResponse = self
            .transport
            .request(
                "get_registered_descriptor",
                Some(api::GetRegisteredDescriptorParams {
                    descriptor_name: name,
                }),
            )
            .await?
            .into_result()?;
        Ok(registered)
    }

    /// The name of a registered multisig wallet (the registry Specter and
    /// other HWI-based apps write to, apart from registered descriptors) with
    /// exactly `policy`'s shape and keys. Firmware without the multisig query
    /// RPCs, or any entry it cannot read, counts as not found.
    pub async fn find_registered_multisig(&self, policy: &utils::MultisigPolicy) -> Option<String> {
        let summaries: serde_cbor::Value = self
            .transport
            .request(
                "get_registered_multisigs",
                Option::<api::EmptyRequest>::None,
            )
            .await
            .ok()?
            .into_result()
            .ok()?;
        let serde_cbor::Value::Map(summaries) = summaries else {
            return None;
        };
        for (name, summary) in summaries {
            let serde_cbor::Value::Text(name) = name else {
                continue;
            };
            if !multisig_summary_matches(&summary, policy) {
                continue;
            }
            let detail: Option<serde_cbor::Value> = match self
                .transport
                .request(
                    "get_registered_multisig",
                    Some(api::GetRegisteredMultisigParams {
                        multisig_name: &name,
                    }),
                )
                .await
            {
                Ok(response) => response.into_result().ok(),
                Err(_) => None,
            };
            if detail.is_some_and(|detail| multisig_detail_matches(&detail, policy)) {
                return Some(name);
            }
        }
        None
    }

    pub async fn auth(&self) -> Result<(), JadeError> {
        let res: api::AuthUserResponse = self
            .transport
            .request(
                "auth_user",
                Some(api::AuthUserParams {
                    network: self.network,
                    epoch: SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .ok()
                        .map(|t| t.as_secs())
                        .unwrap_or(0),
                }),
            )
            .await?
            .into_result()?;

        if let api::AuthUserResponse::PinServerRequired { http_request } = res {
            let client = pinserver::PinServerClient::new();
            let pin_params: api::PinParams = client.request(http_request.params).await?;
            let handshake_completed: bool = self
                .transport
                .request("pin", Some(pin_params))
                .await?
                .into_result()?;
            if !handshake_completed {
                return Err(JadeError::HandShakeRefused);
            }
        }
        Ok(())
    }
}

#[async_trait]
impl<T: Transport + Sync + Send> HWI for Jade<T> {
    fn device_kind(&self) -> DeviceKind {
        self.kind
    }

    async fn get_version(&self) -> Result<super::Version, HWIError> {
        let info = self.get_info().await?;
        parse_version(&info.jade_version)
    }

    async fn get_master_fingerprint(&self) -> Result<Fingerprint, HWIError> {
        let xpub = self.get_extended_pubkey(&DerivationPath::master()).await?;
        Ok(xpub.fingerprint())
    }

    async fn get_extended_pubkey(&self, path: &DerivationPath) -> Result<Xpub, HWIError> {
        let s: String = self
            .transport
            .request(
                "get_xpub",
                Some(api::GetXpubParams {
                    network: self.network,
                    path: path.to_u32_vec(),
                }),
            )
            .await?
            .into_result()?;
        let xpub = Xpub::from_str(&s).map_err(|e| HWIError::Device(e.to_string()))?;
        Ok(xpub)
    }

    async fn display_address(&self, script: &AddressScript) -> Result<(), HWIError> {
        match (self.descriptor_name.as_ref(), script) {
            (Some(descriptor_name), AddressScript::Miniscript { index, change }) => {
                let _address: String = self
                    .transport
                    .request(
                        "get_receive_address",
                        Some(api::DescriptorAddressParams {
                            network: self.network,
                            branch: u32::from(*change),
                            pointer: *index,
                            descriptor_name,
                        }),
                    )
                    .await?
                    .into_result()?;
                Ok(())
            }
            _ => Err(HWIError::UnimplementedMethod),
        }
    }

    async fn register_wallet(
        &self,
        name: &str,
        policy: &str,
    ) -> Result<Option<[u8; 32]>, HWIError> {
        let (descriptor_template, keys) = utils::extract_keys_and_template::<String>(policy)?;
        let registered: bool = self
            .transport
            .request(
                "register_descriptor",
                Some(api::RegisterDescriptorParams {
                    network: self.network,
                    descriptor_name: name,
                    descriptor: descriptor_template,
                    datavalues: keys
                        .into_iter()
                        .enumerate()
                        .map(|(i, key)| (format!("@{i}"), key))
                        .collect(),
                }),
            )
            .await?
            .into_result()?;
        if !registered {
            Err(HWIError::UserRefused)
        } else {
            Ok(None)
        }
    }

    async fn is_wallet_registered(&self, name: &str, policy: &str) -> Result<bool, HWIError> {
        let registered_descriptors = self.get_registered_descriptors().await?;
        if !registered_descriptors.contains_key(name) {
            return Ok(false);
        }

        let registered = self.get_registered_descriptor(name).await?;

        let (descriptor_template, keys) = utils::extract_keys_and_template::<String>(policy)?;
        let datavalues: BTreeMap<String, String> = keys
            .into_iter()
            .enumerate()
            .map(|(i, key)| (format!("@{i}"), key))
            .collect();

        Ok(registered.descriptor_name == name
            && registered.descriptor == descriptor_template
            && registered.datavalues == datavalues)
    }

    /// Searches every registered descriptor (`name` first), then the
    /// multisig registry. Jade signs a PSBT against whichever registration
    /// matches its inputs, so any match can sign without registering again.
    /// Errors reading the list or `name` itself propagate, as the plain
    /// name check's do; an entry under another name that cannot be read is
    /// skipped, so it never hides a later match.
    async fn find_wallet_registration(
        &self,
        name: &str,
        policy: &str,
    ) -> Result<Option<WalletRegistration>, HWIError> {
        let (descriptor_template, keys) = utils::extract_keys_and_template::<String>(policy)?;
        let descriptors = self.get_registered_descriptors().await?;
        let names = descriptors
            .keys()
            .filter(|n| n.as_str() == name)
            .chain(descriptors.keys().filter(|n| n.as_str() != name));
        for registered_name in names {
            let registered = match self.get_registered_descriptor(registered_name).await {
                Ok(registered) => registered,
                Err(e) if registered_name == name => return Err(e),
                Err(_) => continue,
            };
            let same_keys = registered.datavalues.len() == keys.len()
                && keys.iter().enumerate().all(|(i, key)| {
                    registered
                        .datavalues
                        .get(&format!("@{i}"))
                        .is_some_and(|value| utils::same_descriptor(value, key))
                });
            if same_keys && utils::same_descriptor(&registered.descriptor, &descriptor_template) {
                return Ok(Some(WalletRegistration {
                    name: Some(registered_name.clone()),
                }));
            }
        }
        if let Some(multisig) = utils::MultisigPolicy::parse(policy) {
            if let Some(name) = self.find_registered_multisig(&multisig).await {
                return Ok(Some(WalletRegistration { name: Some(name) }));
            }
        }
        Ok(None)
    }

    async fn sign_tx(&self, psbt: &mut Psbt) -> Result<(), HWIError> {
        let first: api::Response<serde_bytes::ByteBuf> = self
            .transport
            .request(
                "sign_psbt",
                Some(api::SignPsbtParams {
                    network: self.network,
                    psbt: Psbt::serialize(psbt),
                }),
            )
            .await?;

        if let Some(e) = first.error {
            return Err(JadeError::Rpc(e).into());
        }

        let mut psbt_bytes = first
            .result
            .ok_or(JadeError::Transport(TransportError::NoErrorOrResult))?;

        if let (Some(mut seqlen), Some(mut seqnum)) = (first.seqlen, first.seqnum) {
            if seqlen > 1 {
                while seqnum < seqlen {
                    let mut res: api::Response<serde_bytes::ByteBuf> = self
                        .transport
                        .request(
                            "get_extended_data",
                            Some(api::GetExtendedDataParams {
                                origid: &first.id,
                                orig: "sign_psbt",
                                seqnum: seqnum + 1,
                                seqlen,
                            }),
                        )
                        .await?;

                    if let Some(e) = res.error {
                        return Err(JadeError::Rpc(e).into());
                    }

                    if let Some(bytes) = res.result.as_mut() {
                        psbt_bytes.append(bytes);
                    } else {
                        return Err(JadeError::Transport(TransportError::NoErrorOrResult).into());
                    }

                    if let (Some(len), Some(num)) = (res.seqlen, res.seqnum) {
                        seqlen = len;
                        seqnum = num;
                    } else {
                        return Err(JadeError::Transport(TransportError::NoErrorOrResult).into());
                    }
                }
            }
        }

        let signed_psbt =
            Psbt::deserialize(&psbt_bytes).map_err(|e| HWIError::Device(e.to_string()))?;
        utils::merge_signatures(psbt, &signed_psbt);

        Ok(())
    }
}

fn cbor_field<'a>(value: &'a serde_cbor::Value, key: &str) -> Option<&'a serde_cbor::Value> {
    match value {
        serde_cbor::Value::Map(map) => map.get(&serde_cbor::Value::Text(key.to_string())),
        _ => None,
    }
}

fn cbor_u64(value: &serde_cbor::Value) -> Option<u64> {
    match value {
        serde_cbor::Value::Integer(i) => std::convert::TryFrom::try_from(*i).ok(),
        _ => None,
    }
}

/// Jade's name for a multisig script variant.
fn jade_variant(wrapper: utils::MultisigWrapper) -> &'static str {
    match wrapper {
        utils::MultisigWrapper::Wsh => "wsh(multi(k))",
        utils::MultisigWrapper::ShWsh => "sh(wsh(multi(k)))",
        utils::MultisigWrapper::Sh => "sh(multi(k))",
    }
}

/// Whether a `get_registered_multisigs` entry (or the config inside a
/// `get_registered_multisig` reply) can be `policy`. Fields the firmware
/// does not report are not held against it; the keys decide.
fn multisig_summary_matches(summary: &serde_cbor::Value, policy: &utils::MultisigPolicy) -> bool {
    let field_is = |key: &str, expected: &serde_cbor::Value| {
        cbor_field(summary, key).is_none_or(|value| value == expected)
    };
    let number_is = |key: &str, expected: u64| {
        cbor_field(summary, key).is_none_or(|value| cbor_u64(value) == Some(expected))
    };
    field_is(
        "variant",
        &serde_cbor::Value::Text(jade_variant(policy.wrapper).to_string()),
    ) && field_is("sorted", &serde_cbor::Value::Bool(policy.sorted))
        && number_is("threshold", u64::from(policy.threshold))
        && number_is("num_signers", policy.keys.len() as u64)
}

/// Walks a `get_registered_multisig` reply: every signer xpub must be one of
/// `policy`'s keys and every key present, no signer may carry an extra
/// derivation `path` (the registry then derives off the standard branches),
/// and the config's threshold, sortedness and variant must agree.
fn multisig_detail_matches(detail: &serde_cbor::Value, policy: &utils::MultisigPolicy) -> bool {
    fn walk(
        value: &serde_cbor::Value,
        policy: &utils::MultisigPolicy,
        xpubs: &mut Vec<Xpub>,
        ok: &mut bool,
    ) {
        match value {
            serde_cbor::Value::Text(text) => {
                if let Ok(xpub) = Xpub::from_str(text) {
                    xpubs.push(xpub);
                }
            }
            serde_cbor::Value::Array(items) => {
                for item in items {
                    walk(item, policy, xpubs, ok);
                }
            }
            serde_cbor::Value::Map(map) => {
                if cbor_field(value, "threshold").is_some()
                    && !multisig_summary_matches(value, policy)
                {
                    *ok = false;
                }
                if let Some(serde_cbor::Value::Array(path)) = cbor_field(value, "path") {
                    if !path.is_empty() {
                        *ok = false;
                    }
                }
                for item in map.values() {
                    walk(item, policy, xpubs, ok);
                }
            }
            _ => {}
        }
    }
    let (mut xpubs, mut ok) = (Vec::new(), true);
    walk(detail, policy, &mut xpubs, &mut ok);
    ok && policy.has_keys(&xpubs)
}

impl<T: 'static + Transport + Sync + Send> From<Jade<T>> for Box<dyn HWI + Send> {
    fn from(s: Jade<T>) -> Box<dyn HWI + Send> {
        Box::new(s)
    }
}

async fn exchange<S, D>(
    transport: &mut SerialStream,
    method: &str,
    params: Option<S>,
) -> Result<api::Response<D>, JadeError>
where
    S: Serialize + Unpin,
    D: DeserializeOwned + Unpin,
{
    let (reader, mut writer) = tokio::io::split(transport);

    let id = std::process::id();
    let req = serde_cbor::to_vec(&api::Request {
        id: &id.to_string(),
        method,
        params,
    })
    .map_err(TransportError::from)?;

    writer.write_all(&req).await.map_err(TransportError::from)?;

    let response = read_stream(reader).await?;

    if response.id != id.to_string() {
        return Err(TransportError::NonceMismatch.into());
    }

    Ok(response)
}

async fn read_stream<D: DeserializeOwned, S: AsyncRead + Unpin>(
    mut stream: S,
) -> Result<api::Response<D>, TransportError> {
    let mut buf = Vec::<u8>::new();
    let mut chunk = [0; 1024];
    let n = stream.read(&mut chunk).await?;
    buf.extend_from_slice(&chunk[..n]);
    if let Ok(response) = serde_cbor::from_slice(&buf) {
        return Ok(response);
    }
    loop {
        tokio::select! {
            res = stream.read(&mut chunk) => {
                let n = res?;
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
                if let Ok(response) = serde_cbor::from_slice(&buf) {
                    return Ok(response);
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {
                break;
            }
        }
    }
    match serde_cbor::from_slice(&buf) {
        Ok(response) => Ok(response),
        Err(_) => Err(TransportError::NoErrorOrResult),
    }
}

#[async_trait]
pub trait Transport: Debug {
    async fn request<S: Serialize + Send + Unpin, D: DeserializeOwned + Unpin + Send>(
        &self,
        method: &str,
        params: Option<S>,
    ) -> Result<api::Response<D>, JadeError>;
}

impl Jade<SerialTransport> {
    pub async fn enumerate() -> Result<Vec<Self>, JadeError> {
        let mut res = Vec::new();
        for port_name in SerialTransport::enumerate_potential_ports()? {
            let jade = Jade::<SerialTransport>::new(SerialTransport::new(port_name)?);
            jade.ping().await?;
            res.push(jade);
        }
        Ok(res)
    }
}

#[derive(Debug)]
pub struct SerialTransport {
    pub stream: Arc<Mutex<SerialStream>>,
}

pub const JADE_DEVICE_IDS: [(u16, u16); 6] = [
    (0x10c4, 0xea60),
    (0x1a86, 0x55d4),
    (0x0403, 0x6001),
    (0x1a86, 0x7523),
    (0x303a, 0x4001),
    (0x303a, 0x1001),
];

impl SerialTransport {
    pub fn new(port_name: String) -> Result<Self, TransportError> {
        let mut transport = tokio_serial::new(port_name, DEFAULT_JADE_BAUD_RATE)
            .open_native_async()
            .map_err(TransportError::from)?;
        // Ensure RTS and DTR are not set (as this can cause the hw to reboot)
        // according to https://github.com/Blockstream/Jade/blob/master/jadepy/jade_serial.py#L56
        transport
            .write_request_to_send(false)
            .map_err(TransportError::from)?;
        transport
            .write_data_terminal_ready(false)
            .map_err(TransportError::from)?;
        Ok(Self {
            stream: Arc::new(Mutex::new(transport)),
        })
    }
    pub fn enumerate_potential_ports() -> Result<Vec<String>, JadeError> {
        match available_ports() {
            Ok(ports) => Ok(ports
                .into_iter()
                .filter_map(|p| match p.port_type {
                    SerialPortType::UsbPort(info) => {
                        if JADE_DEVICE_IDS.contains(&(info.vid, info.pid)) {
                            Some(p.port_name)
                        } else {
                            None
                        }
                    }
                    _ => None,
                })
                .collect()),
            Err(e) => Err(JadeError::Transport(e.into())),
        }
    }
}

const DEFAULT_JADE_BAUD_RATE: u32 = 115200;

#[async_trait]
impl Transport for SerialTransport {
    async fn request<S: Serialize + Send + Unpin, D: DeserializeOwned + Unpin + Send>(
        &self,
        method: &str,
        params: Option<S>,
    ) -> Result<api::Response<D>, JadeError> {
        let mut stream = self.stream.lock().await;
        exchange(&mut stream, method, params).await
    }
}

#[derive(Debug)]
pub enum TransportError {
    Serialize(serde_cbor::Error),
    NoErrorOrResult,
    NonceMismatch,
    Io(std::io::Error),
    Serial(serialport::Error),
}

impl From<serde_cbor::Error> for TransportError {
    fn from(e: serde_cbor::Error) -> Self {
        Self::Serialize(e)
    }
}

impl From<std::io::Error> for TransportError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serialport::Error> for TransportError {
    fn from(e: serialport::Error) -> Self {
        Self::Serial(e)
    }
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Serialize(e) => write!(f, "{e}"),
            Self::NoErrorOrResult => write!(f, "No Error or Result"),
            Self::NonceMismatch => write!(f, "Nonce mismatched"),
            Self::Io(e) => write!(f, "{e}"),
            Self::Serial(e) => write!(f, "{e}"),
        }
    }
}

#[derive(Debug)]
pub enum JadeError {
    Transport(TransportError),
    Rpc(api::Error),
    PinServer(pinserver::Error),
    HandShakeRefused,
}

impl From<TransportError> for JadeError {
    fn from(e: TransportError) -> Self {
        Self::Transport(e)
    }
}

impl From<pinserver::Error> for JadeError {
    fn from(e: pinserver::Error) -> Self {
        Self::PinServer(e)
    }
}

impl std::fmt::Display for JadeError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Transport(e) => write!(f, "{e}"),
            Self::Rpc(e) => write!(f, "{e:?}"),
            Self::PinServer(e) => write!(f, "{e:?}"),
            Self::HandShakeRefused => write!(f, "Handshake with pinserver refused"),
        }
    }
}

impl From<JadeError> for HWIError {
    fn from(e: JadeError) -> HWIError {
        match e {
            JadeError::Transport(e) => HWIError::Device(e.to_string()),
            JadeError::Rpc(e) => {
                if e.code == api::ErrorCode::UserCancelled as i32 {
                    HWIError::UserRefused
                } else if e.code == api::ErrorCode::NetworkMismatch as i32 {
                    HWIError::NetworkMismatch
                } else {
                    HWIError::Device(format!("{e:?}"))
                }
            }
            JadeError::PinServer(e) => HWIError::Device(format!("{e:?}")),
            JadeError::HandShakeRefused => {
                HWIError::Device("Handshake with pinserver refused".to_string())
            }
        }
    }
}

#[cfg(test)]
mod registration_tests {
    use super::*;
    use serde_cbor::Value;

    const KEYS: [&str; 3] = [
        "[b0822927/48'/1'/0'/2']tpubDEvZxV86Br8Knbm9tWcr5Hvmg5cYTYsg92vinqH6Bie6U8ix8CsoN9W11NQygdqVwmHUJpsHXxNsi5gXn36g4xNfLWkMqPuFhRZAmMQ7jjQ",
        "[7fc39c07/48'/1'/0'/2']tpubDEvjgXtrUuH3Qtkapny9aE8gN847xiXsf9MDM5XueGf9nrvStqAuBSva3ajGyTvtp8Ti55FvVXsgYSXuS1tQkBeopFuodx2hRUDmQbvKxbZ",
        "[1a1ffd98/48'/1'/0'/2']tpubDFZqzTvGijYb13BC73CkS1er8DrP5YdzMhziN3kWCKUFaW51Yj6ggvf99YpdrkTJy4RT85mxQMHXDiFAKRxzf6BykQgT4pRRBNPshSJJcKo",
    ];

    fn text(s: &str) -> Value {
        Value::Text(s.to_string())
    }

    fn map(entries: Vec<(&str, Value)>) -> Value {
        Value::Map(entries.into_iter().map(|(k, v)| (text(k), v)).collect())
    }

    fn policy() -> utils::MultisigPolicy {
        utils::MultisigPolicy::parse(&format!(
            "wsh(sortedmulti(2,{}/<0;1>/*,{}/<0;1>/*,{}/<0;1>/*))",
            KEYS[0], KEYS[1], KEYS[2]
        ))
        .unwrap()
    }

    fn summary(variant: &str, sorted: bool, threshold: i128, signers: i128) -> Value {
        map(vec![
            ("variant", text(variant)),
            ("sorted", Value::Bool(sorted)),
            ("threshold", Value::Integer(threshold)),
            ("num_signers", Value::Integer(signers)),
        ])
    }

    fn detail(xpubs: &[&str], path: Vec<Value>) -> Value {
        let signers = xpubs
            .iter()
            .map(|key| {
                let xpub = key.rsplit_once(']').map_or(*key, |(_, x)| x);
                map(vec![
                    ("fingerprint", Value::Bytes(vec![0, 0, 0, 0])),
                    ("derivation", Value::Array(vec![])),
                    ("xpub", text(xpub)),
                    ("path", Value::Array(path.clone())),
                ])
            })
            .collect();
        map(vec![
            ("multisig_name", text("Specter")),
            (
                "descriptor",
                map(vec![
                    ("variant", text("wsh(multi(k))")),
                    ("sorted", Value::Bool(true)),
                    ("threshold", Value::Integer(2)),
                    ("signers", Value::Array(signers)),
                ]),
            ),
        ])
    }

    /// Answers each `(method, name parameter)` from `replies`; anything
    /// else fails as a transport error.
    #[derive(Debug)]
    struct FakeTransport {
        replies: Vec<((&'static str, Option<&'static str>), Value)>,
    }

    #[async_trait]
    impl Transport for FakeTransport {
        async fn request<S: Serialize + Send + Unpin, D: DeserializeOwned + Unpin + Send>(
            &self,
            method: &str,
            params: Option<S>,
        ) -> Result<api::Response<D>, JadeError> {
            let params = params.map(|p| serde_cbor::value::to_value(p).unwrap());
            let name = match &params {
                Some(Value::Map(m)) => m.values().find_map(|v| match v {
                    Value::Text(t) => Some(t.clone()),
                    _ => None,
                }),
                _ => None,
            };
            let reply = self
                .replies
                .iter()
                .find(|((m, n), _)| *m == method && n.map(str::to_string) == name)
                .map(|(_, reply)| reply.clone())
                .ok_or(TransportError::NoErrorOrResult)?;
            Ok(api::Response {
                id: "0".to_string(),
                seqlen: None,
                seqnum: None,
                result: Some(
                    serde_cbor::value::from_value(reply)
                        .map_err(|_| TransportError::NoErrorOrResult)?,
                ),
                error: None,
            })
        }
    }

    fn descriptor_info() -> Value {
        map(vec![
            ("descriptor_len", Value::Integer(1)),
            ("num_datavalues", Value::Integer(1)),
        ])
    }

    fn sortedmulti() -> String {
        format!(
            "wsh(sortedmulti(2,{}/<0;1>/*,{}/<0;1>/*,{}/<0;1>/*))",
            KEYS[0], KEYS[1], KEYS[2]
        )
    }

    #[tokio::test]
    async fn an_unreadable_descriptor_under_another_name_does_not_hide_the_multisig_registry() {
        let jade = Jade::new(FakeTransport {
            replies: vec![
                (
                    ("get_registered_descriptors", None),
                    map(vec![("Broken", descriptor_info())]),
                ),
                (
                    ("get_registered_multisigs", None),
                    map(vec![("Specter", summary("wsh(multi(k))", true, 2, 3))]),
                ),
                (
                    ("get_registered_multisig", Some("Specter")),
                    detail(&KEYS, vec![]),
                ),
            ],
        });
        assert_eq!(
            jade.find_wallet_registration("Import0000", &sortedmulti())
                .await
                .unwrap(),
            Some(WalletRegistration {
                name: Some("Specter".to_string())
            })
        );
    }

    #[tokio::test]
    async fn an_unreadable_descriptor_under_our_name_is_an_error() {
        let jade = Jade::new(FakeTransport {
            replies: vec![(
                ("get_registered_descriptors", None),
                map(vec![("Import0000", descriptor_info())]),
            )],
        });
        assert!(jade
            .find_wallet_registration("Import0000", &sortedmulti())
            .await
            .is_err());
    }

    #[test]
    fn summary_must_agree_on_every_reported_field() {
        let p = policy();
        assert!(multisig_summary_matches(
            &summary("wsh(multi(k))", true, 2, 3),
            &p
        ));
        assert!(multisig_summary_matches(&map(vec![]), &p));
        assert!(!multisig_summary_matches(
            &summary("sh(wsh(multi(k)))", true, 2, 3),
            &p
        ));
        assert!(!multisig_summary_matches(
            &summary("wsh(multi(k))", false, 2, 3),
            &p
        ));
        assert!(!multisig_summary_matches(
            &summary("wsh(multi(k))", true, 1, 3),
            &p
        ));
        assert!(!multisig_summary_matches(
            &summary("wsh(multi(k))", true, 2, 4),
            &p
        ));
    }

    #[test]
    fn detail_must_hold_exactly_the_policy_keys_on_standard_branches() {
        let p = policy();
        assert!(multisig_detail_matches(&detail(&KEYS, vec![]), &p));
        assert!(!multisig_detail_matches(&detail(&KEYS[..2], vec![]), &p));
        assert!(!multisig_detail_matches(
            &detail(&KEYS, vec![Value::Integer(5)]),
            &p
        ));
    }
}
