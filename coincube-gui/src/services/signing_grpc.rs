use std::{convert::TryFrom, str::FromStr, time::Duration};

use coincube_core::miniscript::bitcoin::{bip32::Fingerprint, psbt::Psbt};
use prost_types::Duration as ProstDuration;
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{
    metadata::MetadataValue,
    transport::{Channel, Endpoint},
    Request,
};
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::app::state::connect::{CONNECT_KEYRING_SERVICE, CONNECT_KEYRING_USER};

pub mod connect_v1 {
    tonic::include_proto!("connect.v1");
}

use connect_v1::{
    realtime_service_client::RealtimeServiceClient, session_service_client::SessionServiceClient,
    stream_envelope, ClientAck, ClientHello, CreateSigningSessionRequest, DevicePlatform,
    GetSigningSessionRequest, SignerTarget, StreamEnvelope,
};

const DEFAULT_TTL_SECS: u64 = 600;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SigningGrpcConfig {
    pub grpc_url: String,
    pub desktop_device_id: String,
    pub target_device_id: String,
    pub target_key_id: String,
    pub target_fingerprint: Fingerprint,
    pub ttl: Duration,
}

impl SigningGrpcConfig {
    pub fn from_env() -> Result<Self, SigningGrpcError> {
        let grpc_url = required_env("COINCUBE_GRPC_URL")?;
        let desktop_device_id = required_env("COINCUBE_DESKTOP_DEVICE_ID")?;
        let target_device_id = required_env("COINCUBE_KEYCHAIN_TARGET_DEVICE_ID")?;
        let target_key_id = required_env("COINCUBE_KEYCHAIN_TARGET_KEY_ID")?;
        let target_fingerprint = Fingerprint::from_str(&required_env(
            "COINCUBE_KEYCHAIN_TARGET_FINGERPRINT",
        )?)
        .map_err(|e| SigningGrpcError::Config(format!("invalid target fingerprint: {e}")))?;
        let ttl_secs = optional_env("COINCUBE_GRPC_SIGNING_TTL_SECS")
            .map(|raw| {
                raw.parse::<u64>()
                    .map_err(|e| SigningGrpcError::Config(format!("invalid signing TTL: {e}")))
            })
            .transpose()?
            .unwrap_or(DEFAULT_TTL_SECS);

        Ok(Self {
            grpc_url,
            desktop_device_id,
            target_device_id,
            target_key_id,
            target_fingerprint,
            ttl: Duration::from_secs(ttl_secs),
        })
    }
}

#[derive(Debug)]
pub enum SigningGrpcError {
    Config(String),
    Auth(String),
    Keyring(String),
    Transport(String),
    Status(String),
    Psbt(String),
}

impl std::fmt::Display for SigningGrpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Config(e) => write!(f, "Keychain signing is not configured: {e}"),
            Self::Auth(e) => write!(f, "Connect authentication failed: {e}"),
            Self::Keyring(e) => write!(f, "Could not load Connect session: {e}"),
            Self::Transport(e) => write!(f, "gRPC signing transport failed: {e}"),
            Self::Status(e) => write!(f, "Signing session failed: {e}"),
            Self::Psbt(e) => write!(f, "Could not parse signed PSBT: {e}"),
        }
    }
}

impl std::error::Error for SigningGrpcError {}

#[derive(Debug, Clone)]
pub struct SigningGrpcRequest {
    pub config: SigningGrpcConfig,
    pub cube_id: String,
    pub vault_id: String,
    pub descriptor_id: String,
    pub psbt: Psbt,
    pub note: String,
}

pub async fn sign_with_keychain(req: SigningGrpcRequest) -> Result<Psbt, SigningGrpcError> {
    let token = load_connect_token(Some(&req.cube_id))?;
    let auth = auth_metadata(&token)?;
    let x_device_id = MetadataValue::from_str(&req.config.desktop_device_id)
        .map_err(|e| SigningGrpcError::Auth(e.to_string()))?;

    let channel = Endpoint::from_shared(req.config.grpc_url.clone())
        .map_err(|e| SigningGrpcError::Config(e.to_string()))?
        .connect()
        .await
        .map_err(|e| SigningGrpcError::Transport(e.to_string()))?;

    let (stream_tx, stream_rx) = mpsc::channel::<StreamEnvelope>(16);
    stream_tx
        .send(StreamEnvelope {
            body: Some(stream_envelope::Body::ClientHello(ClientHello {
                device_id: req.config.desktop_device_id.clone(),
                platform: DevicePlatform::Desktop.into(),
                user_agent: "coincube-desktop".to_string(),
                subscribe_vault_ids: vec![req.vault_id.clone()],
                last_seen_event_seq: 0,
            })),
        })
        .await
        .map_err(|e| SigningGrpcError::Transport(e.to_string()))?;

    let mut stream_request = Request::new(ReceiverStream::new(stream_rx));
    stream_request
        .metadata_mut()
        .insert("authorization", auth.clone());
    let mut stream = RealtimeServiceClient::new(channel.clone())
        .connect(stream_request)
        .await
        .map_err(|e| SigningGrpcError::Transport(e.to_string()))?
        .into_inner();

    let session = create_session(channel.clone(), &req, auth.clone(), x_device_id.clone()).await?;
    let session_id = session.session_id;
    if session_id.is_empty() {
        return Err(SigningGrpcError::Status(
            "server returned an empty session id".to_string(),
        ));
    }

    loop {
        let Some(envelope) = stream
            .message()
            .await
            .map_err(|e| SigningGrpcError::Transport(e.to_string()))?
        else {
            return Err(SigningGrpcError::Transport(
                "realtime stream closed before signing completed".to_string(),
            ));
        };

        let Some(stream_envelope::Body::SessionEvent(event)) = envelope.body else {
            continue;
        };

        if event.event_seq > 0 {
            let _ = stream_tx
                .send(StreamEnvelope {
                    body: Some(stream_envelope::Body::ClientAck(ClientAck {
                        event_seq: event.event_seq,
                    })),
                })
                .await;
        }

        if event.session_id != session_id {
            continue;
        }

        match connect_v1::SessionStatus::try_from(event.status)
            .unwrap_or(connect_v1::SessionStatus::Unspecified)
        {
            connect_v1::SessionStatus::Completed => {
                return fetch_merged_signed_psbt(channel, &session_id, auth, x_device_id).await;
            }
            connect_v1::SessionStatus::Rejected
            | connect_v1::SessionStatus::Cancelled
            | connect_v1::SessionStatus::Expired
            | connect_v1::SessionStatus::Failed => {
                return Err(SigningGrpcError::Status(format!(
                    "session {session_id} ended with status {:?}",
                    connect_v1::SessionStatus::try_from(event.status)
                        .unwrap_or(connect_v1::SessionStatus::Unspecified)
                )));
            }
            _ => {}
        }
    }
}

async fn create_session(
    channel: Channel,
    req: &SigningGrpcRequest,
    auth: MetadataValue<tonic::metadata::Ascii>,
    x_device_id: MetadataValue<tonic::metadata::Ascii>,
) -> Result<connect_v1::SigningSession, SigningGrpcError> {
    let mut grpc = SessionServiceClient::new(channel);
    let mut request = Request::new(CreateSigningSessionRequest {
        request_id: Uuid::new_v4().to_string(),
        vault_id: req.vault_id.clone(),
        descriptor_id: req.descriptor_id.clone(),
        psbt: req.psbt.serialize(),
        targets: vec![SignerTarget {
            device_id: req.config.target_device_id.clone(),
            key_fingerprint: req.config.target_fingerprint.to_string(),
            key_id: req.config.target_key_id.clone(),
        }],
        note: req.note.clone(),
        ttl: Some(ProstDuration {
            seconds: req.config.ttl.as_secs() as i64,
            nanos: 0,
        }),
        require_user_presence: true,
    });
    request.metadata_mut().insert("authorization", auth);
    request.metadata_mut().insert("x-device-id", x_device_id);

    grpc.create_signing_session(request)
        .await
        .map_err(|e| SigningGrpcError::Status(e.to_string()))?
        .into_inner()
        .session
        .ok_or_else(|| SigningGrpcError::Status("server returned no session".to_string()))
}

async fn fetch_merged_signed_psbt(
    channel: Channel,
    session_id: &str,
    auth: MetadataValue<tonic::metadata::Ascii>,
    x_device_id: MetadataValue<tonic::metadata::Ascii>,
) -> Result<Psbt, SigningGrpcError> {
    let mut grpc = SessionServiceClient::new(channel);
    let mut request = Request::new(GetSigningSessionRequest {
        session_id: session_id.to_string(),
    });
    request.metadata_mut().insert("authorization", auth);
    request.metadata_mut().insert("x-device-id", x_device_id);

    let session = grpc
        .get_signing_session(request)
        .await
        .map_err(|e| SigningGrpcError::Status(e.to_string()))?
        .into_inner()
        .session
        .ok_or_else(|| SigningGrpcError::Status("server returned no session".to_string()))?;

    let mut signed = None;
    for sig in session.submitted_signatures {
        let psbt = Psbt::deserialize(&sig.signed_psbt)
            .map_err(|e| SigningGrpcError::Psbt(e.to_string()))?;
        if let Some(existing) = &mut signed {
            merge_psbt_signatures(existing, &psbt);
        } else {
            signed = Some(psbt);
        }
    }

    signed.ok_or_else(|| {
        SigningGrpcError::Status("completed session has no submitted signatures".to_string())
    })
}

pub fn load_connect_token(cube_id: Option<&str>) -> Result<Zeroizing<String>, SigningGrpcError> {
    #[derive(Deserialize)]
    struct StoredSession {
        token: String,
    }

    let mut keys = Vec::new();
    if let Some(cube_id) = cube_id {
        if !cube_id.is_empty() {
            keys.push(format!("cube_{cube_id}"));
        }
    }
    keys.push(CONNECT_KEYRING_USER.to_string());

    for key in keys {
        let Ok(entry) = keyring::Entry::new(CONNECT_KEYRING_SERVICE, &key) else {
            continue;
        };
        let Ok(bytes) = entry.get_secret() else {
            continue;
        };
        let session = serde_json::from_slice::<StoredSession>(&bytes)
            .map_err(|e| SigningGrpcError::Keyring(e.to_string()))?;
        if !session.token.is_empty() {
            return Ok(Zeroizing::new(session.token));
        }
    }

    Err(SigningGrpcError::Keyring(
        "no Connect session found for this cube".to_string(),
    ))
}

fn merge_psbt_signatures(psbt: &mut Psbt, signed_psbt: &Psbt) {
    for i in 0..signed_psbt.inputs.len() {
        let Some(psbtin) = psbt.inputs.get_mut(i) else {
            continue;
        };
        let Some(signed_psbtin) = signed_psbt.inputs.get(i) else {
            continue;
        };
        psbtin
            .partial_sigs
            .extend(&mut signed_psbtin.partial_sigs.iter());
        psbtin
            .tap_script_sigs
            .extend(&mut signed_psbtin.tap_script_sigs.iter());
        if let Some(sig) = signed_psbtin.tap_key_sig {
            psbtin.tap_key_sig = Some(sig);
        }
    }
}

fn auth_metadata(
    token: &Zeroizing<String>,
) -> Result<MetadataValue<tonic::metadata::Ascii>, SigningGrpcError> {
    MetadataValue::from_str(&format!("Bearer {}", token.as_str()))
        .map_err(|e| SigningGrpcError::Auth(e.to_string()))
}

fn required_env(name: &str) -> Result<String, SigningGrpcError> {
    optional_env(name).ok_or_else(|| SigningGrpcError::Config(format!("{name} is required")))
}

fn optional_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    #[test]
    fn config_from_env_requires_grpc_url() {
        let _guard = env_lock().lock().unwrap();
        clear_env();
        std::env::set_var("COINCUBE_DESKTOP_DEVICE_ID", "1");
        std::env::set_var("COINCUBE_KEYCHAIN_TARGET_DEVICE_ID", "2");
        std::env::set_var("COINCUBE_KEYCHAIN_TARGET_KEY_ID", "3");
        std::env::set_var("COINCUBE_KEYCHAIN_TARGET_FINGERPRINT", "f714c228");

        let err = SigningGrpcConfig::from_env().unwrap_err().to_string();
        assert!(err.contains("COINCUBE_GRPC_URL"));
        clear_env();
    }

    #[test]
    fn config_from_env_uses_default_ttl() {
        let _guard = env_lock().lock().unwrap();
        clear_env();
        std::env::set_var("COINCUBE_GRPC_URL", "http://127.0.0.1:50051");
        std::env::set_var("COINCUBE_DESKTOP_DEVICE_ID", "1");
        std::env::set_var("COINCUBE_KEYCHAIN_TARGET_DEVICE_ID", "2");
        std::env::set_var("COINCUBE_KEYCHAIN_TARGET_KEY_ID", "3");
        std::env::set_var("COINCUBE_KEYCHAIN_TARGET_FINGERPRINT", "f714c228");

        let config = SigningGrpcConfig::from_env().unwrap();
        assert_eq!(config.ttl, Duration::from_secs(DEFAULT_TTL_SECS));
        assert_eq!(config.target_fingerprint.to_string(), "f714c228");
        clear_env();
    }

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn clear_env() {
        for key in [
            "COINCUBE_GRPC_URL",
            "COINCUBE_DESKTOP_DEVICE_ID",
            "COINCUBE_KEYCHAIN_TARGET_DEVICE_ID",
            "COINCUBE_KEYCHAIN_TARGET_KEY_ID",
            "COINCUBE_KEYCHAIN_TARGET_FINGERPRINT",
            "COINCUBE_GRPC_SIGNING_TTL_SECS",
        ] {
            std::env::remove_var(key);
        }
    }
}
