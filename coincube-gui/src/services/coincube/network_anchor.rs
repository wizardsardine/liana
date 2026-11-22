//! Authenticated operator-trusted fork evidence. This is not a spend permission.
use super::{
    network_status::{NetworkObservation, NetworkStatusError},
    CoincubeClient, CoincubeError,
};
use crate::services::http::NotSuccessResponseInfo;
use coincube_core::{chain::ChainId, miniscript::bitcoin::BlockHash};
use coincubed::connect::{
    AdmissionError, ConnectAnchorAuthority, ConnectBackend, TrustedChainAnchor, MAX_ANCHOR_AGE,
};
use serde::Deserialize;
use std::{
    convert::TryFrom,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub(crate) const MAX_ANCHOR_BODY_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnchorState {
    Available,
    NotConfigured,
    ConfigurationError,
    RpcUnavailable,
    Malformed,
    ForkAbsent,
    ForkInactive,
    RdtsAbsent,
    RdtsUnsupported,
    WrongChain,
    Syncing,
    InconsistentSnapshot,
    ForkUnverified,
}

/// Startup preserves service state, HTTP classification and daemon admission
/// separately so the loader can offer the correct recovery action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnchorStartupError {
    State(AnchorState),
    Http(u16),
    Transport,
    InvalidResponse,
    Admission(AdmissionError),
}
impl From<AdmissionError> for AnchorStartupError {
    fn from(error: AdmissionError) -> Self {
        Self::Admission(error)
    }
}
impl From<NetworkStatusError> for AnchorStartupError {
    fn from(error: NetworkStatusError) -> Self {
        match error {
            NetworkStatusError::UnsupportedChain => Self::Admission(AdmissionError::WrongChain),
            NetworkStatusError::InvalidResponse => Self::InvalidResponse,
            NetworkStatusError::Request(CoincubeError::Unsuccessful(info)) => {
                Self::Http(info.status_code)
            }
            NetworkStatusError::Request(CoincubeError::Network(_)) => Self::Transport,
            NetworkStatusError::Request(_) => Self::InvalidResponse,
        }
    }
}
impl std::fmt::Display for AnchorStartupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(401 | 403) => {
                f.write_str("Sign in to Connect again to use Bitcoin Blake2b.")
            }
            Self::Http(404) => {
                f.write_str("Bitcoin Blake2b is not enabled for this account or API.")
            }
            Self::Http(429) => f.write_str("Connect is rate limited. Wait before retrying."),
            Self::Http(status) => write!(
                f,
                "Connect returned HTTP {}. Retry when it is available.",
                status
            ),
            Self::Transport => {
                f.write_str("Connect could not be reached. Check the connection and retry.")
            }
            Self::InvalidResponse => {
                f.write_str("Connect returned malformed or inconsistent BTCB2 anchor data.")
            }
            Self::Admission(error) => write!(f, "{}", error),
            Self::State(state) => f.write_str(match state {
                AnchorState::NotConfigured => {
                    "The BTCB2 node endpoint is not configured in Connect."
                }
                AnchorState::ConfigurationError => "Connect's BTCB2 node configuration is invalid.",
                AnchorState::RpcUnavailable => {
                    "Connect cannot reach its BTCB2 node. Retry when it is available."
                }
                AnchorState::Malformed => "The BTCB2 node returned malformed data to Connect.",
                AnchorState::ForkAbsent => "The configured node does not report the BTCB2 fork.",
                AnchorState::ForkInactive => "The BTCB2 fork is not active on the configured node.",
                AnchorState::RdtsAbsent => "The configured BTCB2 node has no RDTS deployment.",
                AnchorState::RdtsUnsupported => {
                    "The configured BTCB2 node has an unsupported RDTS deployment."
                }
                AnchorState::WrongChain => "Connect's node is on a different chain.",
                AnchorState::Syncing => {
                    "The BTCB2 node is still syncing. Retry after synchronization."
                }
                AnchorState::InconsistentSnapshot => {
                    "The BTCB2 tip changed during observation. Retry."
                }
                AnchorState::ForkUnverified => {
                    "Connect could not verify the BTCB2 post-fork header."
                }
                AnchorState::Available => "Connect returned an incomplete available anchor.",
            }),
        }
    }
}
impl std::error::Error for AnchorStartupError {}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct NetworkAnchor {
    pub tip_hash: BlockHash,
    pub tip_height: u64,
    pub tip_median_time_past: i64,
    /// Unix seconds on this desktop's clock once returned by `network_anchor`.
    pub observed_at: i64,
    pub observation: NetworkObservation,
}

/// Re-expresses a Connect `observed_at` on this desktop's clock, so freshness
/// checks measure the evidence's age and not the skew between the two clocks.
/// `sent_at` is this desktop's time when the request was sent, so the whole
/// round trip (bounded by the request timeout) also counts toward the age.
/// Without `served_at` (an older API) the server's stamp is kept as is.
pub(crate) fn local_observed_at(observed_at: i64, served_at: Option<i64>, sent_at: i64) -> i64 {
    match served_at {
        Some(served_at) => sent_at.saturating_sub(served_at.saturating_sub(observed_at)),
        None => observed_at,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkAnchorStatus {
    pub network: ChainId,
    pub state: AnchorState,
    pub anchor: Option<NetworkAnchor>,
}

#[derive(Deserialize)]
struct Envelope {
    success: bool,
    data: WireStatus,
    error: Option<ErrorBody>,
}
#[derive(Deserialize)]
struct WireStatus {
    network: String,
    state: AnchorState,
    anchor: Option<NetworkAnchor>,
    served_at: Option<i64>,
}
#[derive(Deserialize)]
struct ErrorBody {
    code: String,
}

impl NetworkAnchor {
    fn consistent(&self) -> bool {
        self.tip_median_time_past >= 0
            && self.observed_at >= 0
            && self.observation.tip_height == self.tip_height
            && self
                .observation
                .fork
                .as_ref()
                .is_some_and(|fork| fork.active && self.tip_height >= fork.height)
    }

    fn trusted(
        &self,
        chain: ChainId,
        now: SystemTime,
    ) -> Result<TrustedChainAnchor, AdmissionError> {
        if !chain.is_blake2b() || !self.consistent() {
            return Err(AdmissionError::WrongChain);
        }
        let observed_at = UNIX_EPOCH
            .checked_add(Duration::from_secs(
                u64::try_from(self.observed_at).map_err(|_| AdmissionError::Stale)?,
            ))
            .ok_or(AdmissionError::Stale)?;
        if now
            .duration_since(observed_at)
            .map_err(|_| AdmissionError::Stale)?
            > MAX_ANCHOR_AGE
        {
            return Err(AdmissionError::Stale);
        }
        Ok(TrustedChainAnchor {
            chain,
            height: u32::try_from(self.tip_height).map_err(|_| AdmissionError::Unavailable)?,
            hash: self.tip_hash,
            median_time_past: u32::try_from(self.tip_median_time_past)
                .map_err(|_| AdmissionError::Unavailable)?,
            observed_at,
        })
    }
}

impl CoincubeClient {
    /// A successful authenticated endpoint response attests that Connect checked
    /// an active post-fork version-2 header in a coherent, bracketed RPC snapshot.
    /// The daemon must still match this hash against its exact selected indexer.
    pub async fn network_anchor(
        &self,
        chain: ChainId,
    ) -> Result<NetworkAnchorStatus, NetworkStatusError> {
        if !chain.is_blake2b() {
            return Err(NetworkStatusError::UnsupportedChain);
        }
        // Anchor observations cannot redirect authentication or allocate an
        // unbounded response body. Other CoincubeClient routes are unchanged.
        let client = self.anchor_transport()?;
        let sent_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|d| i64::try_from(d.as_secs()).ok())
            .unwrap_or(-1);
        let mut response = client
            .get(format!(
                "{}/api/v1/connect/networks/{}/anchor",
                self.base_url,
                chain.api_str()
            ))
            .send()
            .await
            .map_err(CoincubeError::from)?;
        let http = response.status();
        if response
            .content_length()
            .is_some_and(|length| length > MAX_ANCHOR_BODY_BYTES as u64)
        {
            return Err(NetworkStatusError::InvalidResponse);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(CoincubeError::from)? {
            if chunk.len() > MAX_ANCHOR_BODY_BYTES.saturating_sub(bytes.len()) {
                return Err(NetworkStatusError::InvalidResponse);
            }
            bytes.extend_from_slice(&chunk);
        }
        if http != reqwest::StatusCode::OK && http != reqwest::StatusCode::SERVICE_UNAVAILABLE {
            return Err(NetworkStatusError::Request(CoincubeError::Unsuccessful(
                NotSuccessResponseInfo {
                    status_code: http.as_u16(),
                    text: String::from_utf8_lossy(&bytes).into_owned(),
                },
            )));
        }
        let envelope: Envelope =
            serde_json::from_slice(&bytes).map_err(|_| NetworkStatusError::InvalidResponse)?;
        let available = envelope.data.state == AnchorState::Available;
        let valid_payload = if available {
            envelope
                .data
                .anchor
                .as_ref()
                .is_some_and(NetworkAnchor::consistent)
                && envelope.error.is_none()
        } else {
            envelope.data.anchor.is_none()
                && envelope
                    .error
                    .as_ref()
                    .is_some_and(|e| e.code == "SERVICE_UNAVAILABLE")
        };
        if envelope.data.network != chain.api_str()
            || envelope.success != available
            || http.is_success() != available
            || !valid_payload
        {
            return Err(NetworkStatusError::InvalidResponse);
        }
        let mut anchor = envelope.data.anchor;
        if let Some(anchor) = &mut anchor {
            anchor.observed_at =
                local_observed_at(anchor.observed_at, envelope.data.served_at, sent_at);
        }
        Ok(NetworkAnchorStatus {
            network: chain,
            state: envelope.data.state,
            anchor,
        })
    }

    /// Whether an authenticated start on `chain` would be admitted now: the
    /// anchor is available and trusted, as [`Self::authenticated_backend`]
    /// requires. Checked before a change that only such a start can reopen.
    pub async fn anchor_admits_start(&self, chain: ChainId) -> Result<(), String> {
        let status = self
            .network_anchor(chain)
            .await
            .map_err(|_| "Connect's Bitcoin Blake2b status could not be read".to_string())?;
        if status.state != AnchorState::Available {
            return Err("Connect's Bitcoin Blake2b backend is not available".into());
        }
        status
            .anchor
            .ok_or_else(|| "Connect returned no Bitcoin Blake2b anchor".to_string())?
            .trusted(chain, SystemTime::now())
            .map(|_| ())
            .map_err(|_| "Connect's Bitcoin Blake2b anchor is not trusted".into())
    }

    /// Creates one immutable chain/account/provider context. Caller must invalidate
    /// the returned session on logout or account/provider changes, and restart the
    /// daemon with a newly admitted context. Nothing here is serialized.
    pub async fn authenticated_backend(
        &self,
        chain: ChainId,
        selected_endpoint: &str,
    ) -> Result<(ConnectBackend, Arc<ConnectAnchorSession>), AnchorStartupError> {
        if !chain.is_blake2b() {
            return Err(AdmissionError::WrongChain.into());
        }
        let token = self
            .token()
            .filter(|t| !t.trim().is_empty())
            .ok_or(AdmissionError::MissingAuth)?;
        let endpoint = format!(
            "{}/api/v1/esplora/{}",
            self.base_url.trim_end_matches('/'),
            crate::installer::connect_esplora_path(chain)
        );
        if selected_endpoint != endpoint {
            return Err(AdmissionError::InvalidBackend.into());
        }
        let initial = self
            .network_anchor(chain)
            .await
            .map_err(AnchorStartupError::from)?;
        if initial.state != AnchorState::Available {
            return Err(AnchorStartupError::State(initial.state));
        }
        let initial = initial
            .anchor
            .ok_or(AnchorStartupError::InvalidResponse)?
            .trusted(chain, SystemTime::now())?;
        let session = Arc::new(ConnectAnchorSession {
            snapshot: Mutex::new(SessionSnapshot {
                active: true,
                anchor: Some(initial),
            }),
            refresh: Mutex::new(None),
        });
        let weak = Arc::downgrade(&session);
        let client = self.clone();
        let task = tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(30)).await;
                let result = client
                    .network_anchor(chain)
                    .await
                    .ok()
                    .and_then(|s| s.anchor)
                    .and_then(|anchor| anchor.trusted(chain, SystemTime::now()).ok());
                let Some(session) = weak.upgrade() else {
                    break;
                };
                if !session.apply_refresh(result) {
                    break;
                }
            }
        });
        *session
            .refresh
            .lock()
            .map_err(|_| AdmissionError::Unavailable)? = Some(task.abort_handle());
        let backend = ConnectBackend::new(chain, endpoint, token.to_string(), session.clone())?;
        Ok((backend, session))
    }
}

struct SessionSnapshot {
    active: bool,
    anchor: Option<TrustedChainAnchor>,
}

/// No network I/O in the daemon's synchronous authority method. Refresh failures
/// immediately clear evidence; an old success never survives a failed refresh.
pub struct ConnectAnchorSession {
    snapshot: Mutex<SessionSnapshot>,
    refresh: Mutex<Option<tokio::task::AbortHandle>>,
}
impl ConnectAnchorSession {
    fn apply_refresh(&self, anchor: Option<TrustedChainAnchor>) -> bool {
        let Ok(mut snapshot) = self.snapshot.lock() else {
            return false;
        };
        if !snapshot.active {
            return false;
        }
        snapshot.anchor = anchor;
        true
    }
    pub fn invalidate(&self) {
        if let Ok(mut refresh) = self.refresh.lock() {
            if let Some(task) = refresh.take() {
                task.abort();
            }
        }
        if let Ok(mut snapshot) = self.snapshot.lock() {
            snapshot.active = false;
            snapshot.anchor = None;
        }
    }
}
impl ConnectAnchorAuthority for ConnectAnchorSession {
    fn fresh_anchor(&self) -> Result<TrustedChainAnchor, AdmissionError> {
        let snapshot = self
            .snapshot
            .lock()
            .map_err(|_| AdmissionError::Unavailable)?;
        let anchor = snapshot
            .anchor
            .as_ref()
            .ok_or(AdmissionError::Unavailable)?;
        if SystemTime::now()
            .duration_since(anchor.observed_at)
            .map_err(|_| AdmissionError::Stale)?
            > MAX_ANCHOR_AGE
        {
            return Err(AdmissionError::Stale);
        }
        Ok(anchor.clone())
    }
}
impl Drop for ConnectAnchorSession {
    fn drop(&mut self) {
        self.invalidate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::coincube::network_status::RdtsStatus;
    use httpmock::prelude::*;
    use serde_json::{json, Value};
    #[tokio::test]
    async fn oversized_anchor_body_is_rejected() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.path("/api/v1/connect/networks/bitcoin-blake2b/anchor");
            then.status(200).body("x".repeat(65 * 1024));
        });
        assert!(matches!(
            client(&server)
                .network_anchor(ChainId::BitcoinBlake2b)
                .await,
            Err(NetworkStatusError::InvalidResponse)
        ));
    }
    #[tokio::test]
    async fn anchor_and_ordinary_requests_share_frozen_identity_across_token_changes() {
        let server = MockServer::start();
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "X-Device-Fingerprint",
            "synthetic-frozen-fingerprint".parse().unwrap(),
        );
        headers.insert("X-Device-Name", "synthetic-device".parse().unwrap());
        let mut client = CoincubeClient::for_test_with_identity(server.base_url(), headers);
        for (index, token) in [Some("first-token"), Some("replacement-token"), None]
            .iter()
            .copied()
            .enumerate()
        {
            match token {
                Some(token) => client.set_token(token),
                None => client.clear_token(),
            }
            let mut anchor = server.mock(|when, then| {
                let when = when
                    .path("/api/v1/connect/networks/bitcoin-blake2b/anchor")
                    .header("X-Device-Fingerprint", "synthetic-frozen-fingerprint")
                    .header("X-Device-Name", "synthetic-device");
                if let Some(token) = token {
                    when.header("authorization", format!("Bearer {}", token));
                } else {
                    when.matches(|request| {
                        request.headers.as_ref().is_none_or(|headers| {
                            headers
                                .iter()
                                .all(|(name, _)| !name.eq_ignore_ascii_case("authorization"))
                        })
                    });
                }
                then.status(200).json_body(body(ChainId::BitcoinBlake2b));
            });
            let probe = server.mock(|when, then| {
                let when = when
                    .path(format!("/identity-check/{}", index))
                    .header("X-Device-Fingerprint", "synthetic-frozen-fingerprint")
                    .header("X-Device-Name", "synthetic-device");
                if let Some(token) = token {
                    when.header("authorization", format!("Bearer {}", token));
                } else {
                    when.matches(|request| {
                        request.headers.as_ref().is_none_or(|headers| {
                            headers
                                .iter()
                                .all(|(name, _)| !name.eq_ignore_ascii_case("authorization"))
                        })
                    });
                }
                then.status(200);
            });
            assert_eq!(
                client
                    .clone()
                    .network_anchor(ChainId::BitcoinBlake2b)
                    .await
                    .unwrap()
                    .state,
                AnchorState::Available
            );
            assert_eq!(
                client
                    .client
                    .get(format!("{}/identity-check/{}", server.base_url(), index))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                reqwest::StatusCode::OK
            );
            anchor.assert_hits(1);
            probe.assert_hits(1);
            anchor.delete();
        }
    }
    fn body(chain: ChainId) -> Value {
        json!({"success":true,"data":{"network":chain.api_str(),"state":"available","anchor":{
            "tip_hash":"11".repeat(32),"tip_height":973029,"tip_median_time_past":1800000000,
            "observed_at":SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
            "observation":{"tip_height":973029,"fork":{"height":972000,"active":true},
                "rdts":{"state":"flagday","flagday":{"height":972000,"expiry_time":1800010000_i64,"active":false}}}
        }}})
    }
    fn client(server: &MockServer) -> CoincubeClient {
        let mut client = CoincubeClient::new();
        client.base_url = server.base_url();
        client.set_token("synthetic-test-token");
        client
    }
    #[tokio::test]
    async fn served_at_measures_anchor_age_on_the_connect_clock() {
        // Connect's clock is an hour ahead of this desktop's.
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let mut response = body(ChainId::BitcoinBlake2b);
        response["data"]["anchor"]["observed_at"] = json!(now + 3600);
        response["data"]["served_at"] = json!(now + 3601);
        let server = MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method(GET);
                then.status(200).json_body(response);
            })
            .await;
        let anchor = client(&server)
            .network_anchor(ChainId::BitcoinBlake2b)
            .await
            .unwrap()
            .anchor
            .unwrap();
        assert!((now - 1..=now + 1).contains(&anchor.observed_at));
        assert!(anchor
            .trusted(ChainId::BitcoinBlake2b, SystemTime::now())
            .is_ok());
    }
    #[tokio::test]
    async fn a_slow_connect_clock_still_admits_a_just_served_anchor() {
        // Connect is two hours behind this desktop and served the anchor at
        // the same moment it observed it. Without translation the stamp is
        // outside the 90 s admission window. With it, the anchor is current.
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let served = now - 7_200;
        let mut response = body(ChainId::BitcoinBlake2b);
        response["data"]["anchor"]["observed_at"] = json!(served);
        response["data"]["served_at"] = json!(served);
        let server = MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method(GET);
                then.status(200).json_body(response);
            })
            .await;
        let anchor = client(&server)
            .network_anchor(ChainId::BitcoinBlake2b)
            .await
            .unwrap()
            .anchor
            .unwrap();
        assert!((now - 2..=now + 2).contains(&anchor.observed_at));
        assert!(anchor
            .trusted(ChainId::BitcoinBlake2b, SystemTime::now())
            .is_ok());
    }
    #[test]
    fn a_missing_served_at_keeps_the_server_stamp() {
        // An older API does not get a desktop stamp invented for it. A stamp
        // two hours behind the send time stays two hours behind.
        let sent_at = 10_000;
        let observed_at = sent_at - 7_200;
        assert_eq!(local_observed_at(observed_at, None, sent_at), observed_at);
        assert_eq!(
            local_observed_at(observed_at, Some(observed_at), sent_at),
            sent_at
        );
    }
    #[tokio::test]
    async fn both_chains_use_authenticated_dedicated_anchor_and_preserve_inactive_rdts() {
        for chain in [ChainId::BitcoinBlake2b, ChainId::BitcoinBlake2bTestnet4] {
            let server = MockServer::start_async().await;
            let mock = server
                .mock_async(|when, then| {
                    when.method(GET)
                        .path(format!(
                            "/api/v1/connect/networks/{}/anchor",
                            chain.api_str()
                        ))
                        .header("authorization", "Bearer synthetic-test-token");
                    then.status(200).json_body(body(chain));
                })
                .await;
            let status = client(&server).network_anchor(chain).await.unwrap();
            assert_eq!(status.network, chain);
            assert_eq!(status.state, AnchorState::Available);
            let anchor = status.anchor.unwrap();
            assert!(
                matches!(anchor.observation.rdts, RdtsStatus::Flagday { flagday } if !flagday.active)
            );
            mock.assert_async().await;
        }
    }
    #[tokio::test]
    async fn active_fork_anchor_admits_absent_or_unsupported_rdts() {
        for rdts in [json!({"state":"absent"}), json!({"state":"unsupported"})] {
            let server = MockServer::start_async().await;
            let mut response = body(ChainId::BitcoinBlake2b);
            response["data"]["anchor"]["observation"]["rdts"] = rdts;
            let mock = server
                .mock_async(|when, then| {
                    when.method(GET);
                    then.status(200).json_body(response);
                })
                .await;
            let status = client(&server)
                .network_anchor(ChainId::BitcoinBlake2b)
                .await
                .unwrap();
            let anchor = status.anchor.unwrap();
            assert!(matches!(
                anchor.observation.rdts,
                RdtsStatus::Absent | RdtsStatus::Unsupported
            ));
            assert!(anchor
                .trusted(ChainId::BitcoinBlake2b, SystemTime::now())
                .is_ok());
            mock.assert_hits_async(1).await;
        }
    }
    #[tokio::test]
    async fn unavailable_states_remain_distinct_and_never_contain_partial_evidence() {
        for state in [
            "not_configured",
            "configuration_error",
            "rpc_unavailable",
            "malformed",
            "fork_absent",
            "fork_inactive",
            "rdts_absent",
            "rdts_unsupported",
            "wrong_chain",
            "syncing",
            "inconsistent_snapshot",
            "fork_unverified",
        ] {
            let server = MockServer::start_async().await;
            server.mock_async(|when,then| {
                when.method(GET);
                then.status(503).json_body(json!({"success":false,"error":{"code":"SERVICE_UNAVAILABLE"},"data":{"network":"bitcoin-blake2b","state":state}}));
            }).await;
            let status = client(&server)
                .network_anchor(ChainId::BitcoinBlake2b)
                .await
                .unwrap();
            assert_eq!(
                status.state,
                serde_json::from_value::<AnchorState>(json!(state)).unwrap()
            );
            assert!(status.anchor.is_none());
        }
    }
    #[tokio::test]
    async fn malformed_identity_and_snapshot_matrix_refuses() {
        let chain = ChainId::BitcoinBlake2b;
        let mutations: Vec<(&str, Value)> = vec![
            ("/data/network", json!("bitcoin")),
            ("/data/network", json!("bitcoin_blake2b")),
            ("/data/state", json!("unknown")),
            ("/success", json!(false)),
            ("/data/anchor", Value::Null),
            ("/data/anchor/tip_hash", json!("bad")),
            ("/data/anchor/tip_median_time_past", json!(-1)),
            ("/data/anchor/observed_at", json!(-1)),
            ("/data/anchor/observation/tip_height", json!(973028)),
            ("/data/anchor/observation/fork", Value::Null),
            ("/data/anchor/observation/fork/active", json!(false)),
            ("/data/anchor/observation/fork/height", json!(973030)),
        ];
        for (path, value) in mutations {
            let server = MockServer::start_async().await;
            let mut malformed = body(chain);
            *malformed.pointer_mut(path).unwrap() = value;
            server
                .mock_async(|when, then| {
                    when.method(GET);
                    then.status(200).json_body(malformed);
                })
                .await;
            assert!(
                matches!(
                    client(&server).network_anchor(chain).await,
                    Err(NetworkStatusError::InvalidResponse)
                ),
                "{}",
                path
            );
        }
        let server = MockServer::start_async().await;
        let mut missing = body(chain);
        missing["data"]["anchor"]["observation"]
            .as_object_mut()
            .unwrap()
            .remove("fork");
        server
            .mock_async(|when, then| {
                when.method(GET);
                then.status(200).json_body(missing);
            })
            .await;
        assert!(matches!(
            client(&server).network_anchor(chain).await,
            Err(NetworkStatusError::InvalidResponse)
        ));
    }
    #[tokio::test]
    async fn auth_gate_errors_and_bitcoin_refusal_are_not_converted_to_anchor_states() {
        for code in [401, 404, 429] {
            let server = MockServer::start_async().await;
            server
                .mock_async(|when, then| {
                    when.method(GET);
                    then.status(code).body("unavailable");
                })
                .await;
            assert!(matches!(
                client(&server)
                    .network_anchor(ChainId::BitcoinBlake2b)
                    .await,
                Err(NetworkStatusError::Request(_))
            ));
        }
        let server = MockServer::start_async().await;
        let mock = server
            .mock_async(|when, then| {
                when.method(GET);
                then.status(500);
            })
            .await;
        assert!(matches!(
            client(&server).network_anchor(ChainId::Bitcoin).await,
            Err(NetworkStatusError::UnsupportedChain)
        ));
        mock.assert_hits_async(0).await;
    }
    #[tokio::test]
    async fn backend_admission_binds_auth_endpoint_and_cancels_on_drop() {
        let chain = ChainId::BitcoinBlake2b;
        let server = MockServer::start_async().await;
        let mock = server
            .mock_async(|when, then| {
                when.method(GET);
                then.status(200).json_body(body(chain));
            })
            .await;
        let client = client(&server);
        assert!(matches!(
            client
                .authenticated_backend(
                    chain,
                    &format!("{}/api/v1/esplora/bitcoin/mainnet", server.base_url())
                )
                .await,
            Err(AnchorStartupError::Admission(
                AdmissionError::InvalidBackend
            ))
        ));
        mock.assert_hits_async(0).await;
        let endpoint = format!(
            "{}/api/v1/esplora/bitcoin-blake2b/mainnet",
            server.base_url()
        );
        let mut unauthenticated = client.clone();
        unauthenticated.clear_token();
        assert!(matches!(
            unauthenticated
                .authenticated_backend(chain, &endpoint)
                .await,
            Err(AnchorStartupError::Admission(AdmissionError::MissingAuth))
        ));
        mock.assert_hits_async(0).await;
        let (backend, session) = client
            .authenticated_backend(chain, &endpoint)
            .await
            .unwrap();
        assert_eq!(session.fresh_anchor().unwrap().chain, chain);
        session.invalidate();
        assert!(matches!(
            session.fresh_anchor(),
            Err(AdmissionError::Unavailable)
        ));
        let weak = Arc::downgrade(&session);
        drop(backend);
        drop(session);
        assert!(
            weak.upgrade().is_none(),
            "refresh must not retain its authority"
        );
        mock.assert_hits_async(1).await;
    }
    #[tokio::test]
    async fn failed_refresh_clears_evidence_and_revocation_rejects_a_late_success() {
        let chain = ChainId::BitcoinBlake2b;
        let anchor: NetworkAnchor =
            serde_json::from_value(body(chain)["data"]["anchor"].clone()).unwrap();
        let trusted = anchor.trusted(chain, SystemTime::now()).unwrap();
        let session = ConnectAnchorSession {
            snapshot: Mutex::new(SessionSnapshot {
                active: true,
                anchor: Some(trusted.clone()),
            }),
            refresh: Mutex::new(None),
        };
        assert!(session.fresh_anchor().is_ok());
        assert!(session.apply_refresh(None));
        assert_eq!(session.fresh_anchor(), Err(AdmissionError::Unavailable));
        assert!(session.apply_refresh(Some(trusted.clone())));
        session.invalidate();
        assert!(!session.apply_refresh(Some(trusted)));
        assert_eq!(session.fresh_anchor(), Err(AdmissionError::Unavailable));
    }

    #[test]
    fn expired_future_and_unrepresentable_anchor_fields_refuse_daemon_admission() {
        let chain = ChainId::BitcoinBlake2b;
        let mut anchor: NetworkAnchor =
            serde_json::from_value(body(chain)["data"]["anchor"].clone()).unwrap();
        let observed = UNIX_EPOCH + Duration::from_secs(anchor.observed_at as u64);
        assert_eq!(
            anchor.trusted(chain, observed + MAX_ANCHOR_AGE + Duration::from_secs(1)),
            Err(AdmissionError::Stale)
        );
        assert_eq!(
            anchor.trusted(chain, observed - Duration::from_secs(1)),
            Err(AdmissionError::Stale)
        );
        anchor.tip_height = u64::from(u32::MAX) + 1;
        anchor.observation.tip_height = anchor.tip_height;
        assert_eq!(
            anchor.trusted(chain, observed),
            Err(AdmissionError::Unavailable)
        );
    }
}
