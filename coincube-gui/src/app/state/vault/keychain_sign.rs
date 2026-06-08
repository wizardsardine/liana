//! `KeychainSignModal` — drives the multi-signer Keychain signing flow
//! from inside the PSBT panel.
//!
//! Lifecycle:
//! 1. `KeychainSignModal::launch` fires off the vault-member fetch +
//!    cube-key fetch + `ResolveSigners` RPC to map the descriptor's
//!    required-but-not-yet-signed fingerprints to live `SignerDevice`
//!    targets.
//! 2. Per Keychain signer, the modal calls `CreateSigningSession`. Each
//!    successful call appends a `PendingSession` to `pending`.
//! 3. As `SessionEvent`s arrive on the realtime stream (routed by
//!    `App::handle_connect_stream` → `PsbtsPanel::route_session_event`,
//!    PR B Task B.4), the modal advances per-signer status and, on
//!    `SIGNATURE_SUBMITTED`, fetches the signed PSBT, merges it into
//!    the local SpendTx via `Daemon::update_spend_tx`, and tries to
//!    mark the session COMPLETED.
//! 4. When every `PendingSession` reaches a terminal-success state, the
//!    modal closes itself and the existing BroadcastModal takes over.
//!
//! Encryption note: the design doc envisions per-session PSBT encryption
//! using each signer's device pubkey. Until `coincube-api` PR 3 lands,
//! PSBTs are sent plaintext to the API; we still get end-to-end
//! confidentiality from TLS but the API can technically inspect the
//! transaction. The Final-step PR description should call this out.

use std::collections::HashMap;
use std::sync::Arc;

use coincube_core::miniscript::bitcoin::{bip32::Fingerprint, psbt::Psbt};
use iced::{Subscription, Task};
use tokio::sync::RwLock;

use coincube_ui::{
    component::{
        button, modal as modal_const,
        text::{p1_bold, p1_regular},
    },
    icon, theme,
    widget::{modal, Column, Container, Element, Row},
};

use crate::{
    app::{
        error::Error as AppError,
        message::Message,
        state::vault::psbt::Modal,
        state::vault::signers::{
            build_keychain_index, classify_signers, KeychainSignerIndex, RequiredSigner,
        },
        view::{self, SpendTxMessage},
        wallet::Wallet,
    },
    daemon::{model::SpendTx, Daemon},
    services::{
        coincube::{CoincubeClient, ConnectVaultResponse, CubeKeyRaw, User},
        connect::{
            client::auth::AccessTokenResponse,
            grpc::{
                connect_v1::{
                    CreateSigningSessionRequest, GetSigningSessionResponse, ResolveSignersResponse,
                    SessionStatus as ProtoSessionStatus, SigningSession,
                },
                interceptor::AuthInterceptor,
                session::GrpcSessionClient,
            },
        },
    },
};

/// How often to poll `GetSigningSession` for pending signers as a
/// fallback when realtime `SessionEvent`s don't arrive (stream dropped,
/// superseded, vault-scope mismatch, …). Kept conservative — the realtime
/// stream is the primary channel; this just guarantees eventual delivery.
const SESSION_POLL_INTERVAL_SECS: u64 = 4;

/// Per-Keychain-signer state tracked while the user waits for them to
/// approve and sign on their phone.
#[derive(Debug, Clone)]
pub struct PendingSession {
    pub session_id: String,
    /// Backend `keys.id` — surfaced in error messages and used to
    /// resume a half-completed flow.
    pub key_id: u64,
    /// Descriptor master fingerprint — used to merge signatures back
    /// into the local PSBT via the existing `bip32_derivation` path.
    pub fingerprint: Fingerprint,
    /// `SignerDevice.id` returned by `ResolveSigners`. Echoed back to
    /// the API on `CreateSigningSession.targets[i].device_id`.
    pub device_id: String,
    /// Display label — `name (owner_email)` or `name (you)`.
    pub label: String,
    /// Latest known session status. Driven by `SessionEvent`s on the
    /// realtime stream; `Pending` between session-create and the first
    /// delivery confirmation.
    pub status: PendingSessionStatus,
    /// Most recent error message, populated on rejected / expired /
    /// transport failure. Cleared on retry.
    pub error: Option<String>,
    /// Set by `cancel_all` when the user cancels while this row's
    /// `CreateSigningSession` RPC is still in flight (empty
    /// `session_id`). `on_session_created` consults it so the
    /// just-created session is cancelled immediately rather than
    /// outliving the cancelled flow. Cleared on retry.
    pub cancel_requested: bool,
    /// True once this session's signed PSBT has been fetched, merged
    /// into the local `SpendTx`, and successfully persisted via
    /// `update_spend_tx` (the `Persisted { Ok }` step). The API-driven
    /// `Completed` status can race ahead of that async fetch+merge, so
    /// modal-close keys off this flag — not `status` — to guarantee the
    /// signature is captured before the modal goes away. Reset on retry.
    pub signed_psbt_persisted: bool,
    /// True while a `GetSigningSession` fetch is in flight for this row.
    /// `SIGNATURE_SUBMITTED` and `SESSION_COMPLETED` can both ask for the
    /// signed PSBT; this prevents duplicate fetch/persist races.
    pub signed_psbt_fetching: bool,
}

/// View-friendly mirror of the gRPC `SessionStatus` enum, plus a
/// pre-RPC `Creating` placeholder for the moment between user-action
/// and the create-session RPC completing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingSessionStatus {
    /// Waiting on `CreateSigningSession` to return.
    Creating,
    Pending,
    Delivered,
    Viewed,
    Approved,
    PartiallySigned,
    Completed,
    Rejected,
    Cancelled,
    Expired,
    Failed,
}

impl PendingSessionStatus {
    /// True when the session is "done" and shouldn't receive further
    /// state changes. The cancel-all flow only fires cancels for
    /// non-terminal sessions.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Rejected | Self::Cancelled | Self::Expired | Self::Failed
        )
    }

    pub fn is_terminal_success(&self) -> bool {
        matches!(self, Self::Completed)
    }

    pub fn from_proto(status: ProtoSessionStatus) -> Self {
        match status {
            ProtoSessionStatus::Pending => Self::Pending,
            ProtoSessionStatus::Delivered => Self::Delivered,
            ProtoSessionStatus::Viewed => Self::Viewed,
            ProtoSessionStatus::Approved => Self::Approved,
            ProtoSessionStatus::PartiallySigned => Self::PartiallySigned,
            ProtoSessionStatus::Completed => Self::Completed,
            ProtoSessionStatus::Rejected => Self::Rejected,
            ProtoSessionStatus::Cancelled => Self::Cancelled,
            ProtoSessionStatus::Expired => Self::Expired,
            ProtoSessionStatus::Failed => Self::Failed,
            ProtoSessionStatus::Unspecified => Self::Pending,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Creating => "Creating…",
            Self::Pending => "Waiting for signer…",
            Self::Delivered => "Delivered",
            Self::Viewed => "Viewed",
            Self::Approved => "Approved",
            Self::PartiallySigned => "Signing…",
            Self::Completed => "Signed",
            Self::Rejected => "Rejected",
            Self::Cancelled => "Cancelled",
            Self::Expired => "Expired",
            Self::Failed => "Failed",
        }
    }
}

/// Error returned by any of the modal's async operations. The `auth`
/// flag flips when the underlying transport surfaced an
/// `Unauthenticated` / `PermissionDenied` status — the modal treats
/// these as terminal "log in again" cases rather than retryable
/// transient errors. Everything else stays in `Other`.
#[derive(Debug, Clone)]
pub struct OpError {
    pub message: String,
    pub auth: bool,
}

impl OpError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            auth: false,
        }
    }

    pub fn from_status(status: tonic::Status) -> Self {
        let (message, auth) = friendly_grpc_error(status);
        Self { message, auth }
    }
}

/// Sub-messages routed through `Message::KeychainSign`. Kept in this
/// module so they can evolve alongside the modal without churning
/// `app::message::Message`.
#[derive(Debug, Clone)]
pub enum KeychainSignMessage {
    /// Result of the initial parallel fetch:
    /// `(connect_vault_response, cube_keys, viewer_user)`.
    Classified(Result<ClassifiedSigners, OpError>),
    /// Result of `ResolveSigners(vault_id)`.
    SignersResolved(Result<ResolveSignersResponse, OpError>),
    /// Result of a single `CreateSigningSession` call, keyed by the
    /// fingerprint of the signer being addressed.
    SessionCreated(Fingerprint, Result<SigningSession, OpError>),
    /// Result of a `GetSigningSession` fetch after a SIGNATURE_SUBMITTED
    /// event, used to pull down the signed PSBT for merge.
    SessionFetched(String, Result<GetSigningSessionResponse, OpError>),
    /// Periodic timer tick (see `KeychainSignModal::subscription`). Fires
    /// while sessions are still pending so we can poll their status as a
    /// fallback for realtime `SessionEvent`s the desktop never received
    /// (e.g. while the Connect gRPC stream is flapping/superseded).
    PollTick,
    /// Result of a *polled* `GetSigningSession` fetch. Same payload as
    /// `SessionFetched` but routed so the handler treats it leniently:
    /// a transient error or a not-yet-signed response must NOT fail the
    /// session — the next tick (or a realtime event) retries.
    SessionPolled(String, Result<GetSigningSessionResponse, OpError>),
    /// Result of a `cancel_signing_session` call. Carries the session_id
    /// so the modal can mark the right row Cancelled.
    SessionCancelled(String, Result<(), OpError>),
    /// Result of persisting the merged PSBT via `Daemon::update_spend_tx`
    /// after a signed PSBT was fetched and merged. Carries the
    /// `session_id` so a persistence failure marks the originating row;
    /// on success we re-emit `Message::Updated` so the PSBT panel's
    /// existing post-save flow (saved flag, sigs recompute, keychain
    /// modal close) still runs unchanged.
    Persisted {
        session_id: String,
        result: Result<(), String>,
    },
    /// One `SessionEvent` forwarded from the top-level
    /// `App::handle_connect_stream`. Routed unconditionally — modals
    /// that don't recognise the session_id are no-ops.
    StreamEvent(crate::services::connect::grpc::connect_v1::SessionEvent),
    /// Realtime-stream health change forwarded by
    /// `App::handle_connect_stream` for every non-`SessionEvent`
    /// variant. The modal surfaces a banner when the stream drops
    /// while pending sessions are in flight — without this signal the
    /// user would sit and watch a frozen "waiting…" indicator with
    /// no indication that the desktop has stopped receiving updates.
    StreamHealth(crate::app::ConnectionStatus),
}

/// Output of the initial fetch+classify step. Held by the modal so the
/// view layer can show the list of signers it's about to address before
/// `ResolveSigners` returns.
#[derive(Debug, Clone)]
pub struct ClassifiedSigners {
    pub vault: ConnectVaultResponse,
    pub required: Vec<RequiredSigner>,
    pub self_user_id: u64,
}

#[derive(Debug, Clone)]
enum Phase {
    /// Fetching members + cube keys + classifying.
    Loading,
    /// Got classification; resolving signer targets.
    Resolving,
    /// Sessions in flight or terminal.
    Sessions,
    /// Every signer reached terminal-success *and* its signed PSBT was
    /// merged + persisted (see `check_all_done`). Modal closes on the
    /// `Message::Updated(Ok)` re-emitted alongside this transition.
    AllDone,
}

pub struct KeychainSignModal {
    wallet: Arc<Wallet>,
    /// Connect REST client — separate from the gRPC clients so REST
    /// calls (vault members, cube keys) don't share lifecycle with the
    /// session gRPC connection.
    coincube_client: CoincubeClient,
    /// Shared with the REST `BackendClient` so token refreshes flow
    /// through automatically. Used to construct `GrpcSessionClient`
    /// instances on demand (`make_session_client`).
    tokens: Arc<RwLock<AccessTokenResponse>>,
    grpc_url: String,
    desktop_device_id: String,
    /// Vault ID on the API — sourced from `ConnectVaultResponse.id`.
    /// Populated after the classification fetch returns.
    vault_id: Option<u64>,
    /// Cube server ID — used to call `GET /connect/cubes/{id}/vault`.
    /// Read from `cache.current_cube_server_id`.
    cube_server_id: u64,
    /// Cube UUID — used to call `GET /connect/cubes/{uuid}/keys`.
    /// Read from `cache.cube_id`.
    cube_uuid: String,
    /// Wallet alias / descriptor identity. Used for the
    /// `descriptor_id` field on `CreateSigningSession` so the API can
    /// reject mismatched-descriptor sessions later.
    descriptor_id: String,
    /// PSBT snapshot at session-open time. Cloned per session so each
    /// signer gets the same starting point; merges happen via the
    /// daemon's `update_spend_tx` rather than by mutating this copy.
    psbt: Psbt,
    /// Result of the initial fetch+classify step. `None` until the
    /// `Classified` message lands.
    classified: Option<ClassifiedSigners>,
    /// Resolved targets from `ResolveSigners`. Empty until that RPC
    /// returns; populated with one entry per Keychain signer.
    pending: Vec<PendingSession>,
    /// Resolved-but-unaddressable signers (e.g. owner with no device
    /// registered). Surfaced as a banner so the user knows why they
    /// can't proceed.
    unresolved: Vec<String>,
    /// Top-of-modal error banner.
    error: Option<String>,
    /// Latest realtime-stream health relayed from the App. Drives the
    /// "Connection lost" banner shown while sessions are pending but
    /// the desktop can't see updates. Defaults to `Connected` — we
    /// only flip out of that state when we receive a real signal,
    /// avoiding a misleading "connection lost" toast at modal open.
    stream_health: crate::app::ConnectionStatus,
    phase: Phase,
    /// When set, the on-blur action confirms cancel-all. Off by default
    /// because clicking outside the modal would otherwise silently
    /// discard in-flight session state.
    display_modal: bool,
    /// Set when the user dismissed the modal while one or more
    /// `CreateSigningSession` RPCs were still in flight. The modal stays
    /// mounted (but hidden) so it can still receive `SessionCreated` and
    /// fire the deferred cancels; once every pending session reaches a
    /// terminal state it self-closes via the `Message::Updated(Ok)`
    /// path. Without this the modal would be dropped immediately and the
    /// just-created sessions would be orphaned server-side until TTL.
    dismissed: bool,
}

impl KeychainSignModal {
    /// Construct a new modal *without* launching the orchestration —
    /// call `launch()` next. Split because the caller pattern in
    /// `PsbtState::update` first builds the modal, then stashes it on
    /// `self.modal`, then dispatches the kickoff Task.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        wallet: Arc<Wallet>,
        coincube_client: CoincubeClient,
        tokens: Arc<RwLock<AccessTokenResponse>>,
        grpc_url: String,
        desktop_device_id: String,
        cube_server_id: u64,
        cube_uuid: String,
        descriptor_id: String,
        psbt: Psbt,
    ) -> Self {
        Self {
            wallet,
            coincube_client,
            tokens,
            grpc_url,
            desktop_device_id,
            vault_id: None,
            cube_server_id,
            cube_uuid,
            descriptor_id,
            psbt,
            classified: None,
            pending: Vec::new(),
            unresolved: Vec::new(),
            error: None,
            stream_health: crate::app::ConnectionStatus::Connected,
            phase: Phase::Loading,
            display_modal: true,
            dismissed: false,
        }
    }

    pub fn pending(&self) -> &[PendingSession] {
        &self.pending
    }

    pub fn unresolved(&self) -> &[String] {
        &self.unresolved
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn classified(&self) -> Option<&ClassifiedSigners> {
        self.classified.as_ref()
    }

    pub fn is_loading(&self) -> bool {
        matches!(self.phase, Phase::Loading | Phase::Resolving)
    }

    pub fn is_done(&self) -> bool {
        matches!(self.phase, Phase::AllDone)
    }

    /// True when every pending session has both reached a
    /// terminal-success state *and* had its signed PSBT merged and
    /// persisted locally. The `signed_psbt_persisted` half is essential:
    /// the API `Completed` event can arrive before the async
    /// `GetSigningSession` fetch+merge resolves, so gating on status
    /// alone would close the modal and silently drop a signature.
    /// Recomputed each time a status changes; cheap because `pending`
    /// is small (≤ descriptor signer count).
    fn check_all_done(&self) -> bool {
        !self.pending.is_empty()
            && self
                .pending
                .iter()
                .all(|p| p.status.is_terminal_success() && p.signed_psbt_persisted)
    }

    /// True while any pending session is non-terminal. After
    /// `cancel_all()`, such entries still depend on the modal staying
    /// mounted: empty-`session_id` rows are cancelled later by
    /// `on_session_created`, and direct-cancel rows only flip to
    /// `Cancelled` once their `SessionCancelled` reply lands. Dropping
    /// the modal before they drain orphans the sessions server-side.
    pub fn has_undrained_sessions(&self) -> bool {
        self.pending.iter().any(|p| !p.status.is_terminal())
    }

    /// Mark the modal dismissed-but-mounted. The view hides immediately
    /// (see `view`), but the struct lives on to drive the deferred
    /// cancels until `close_if_dismissed_and_drained` tears it down.
    pub fn mark_dismissed(&mut self) {
        self.dismissed = true;
    }

    /// When the modal was dismissed mid-flight, close it once every
    /// pending session has reached a terminal state — reusing the
    /// existing `Phase::AllDone` + `Message::Updated(Ok)` close path
    /// that the panel already drives `self.modal = None` from.
    fn close_if_dismissed_and_drained(&mut self) -> Task<Message> {
        if self.dismissed
            && !self.pending.is_empty()
            && self.pending.iter().all(|p| p.status.is_terminal())
        {
            self.phase = Phase::AllDone;
            return Task::done(Message::Updated(Ok(())));
        }
        Task::none()
    }

    /// Construct a fresh `GrpcSessionClient`. The shared `Arc<RwLock>` of
    /// tokens flows through `AuthInterceptor`, so every call picks up
    /// the latest access_token without re-plumbing on refresh. Held as
    /// a method for future refactor convenience even though every
    /// current call site inlines the same shape (we keep the closures
    /// `Send + 'static` by not capturing `&self`).
    #[allow(dead_code)]
    async fn make_session_client(&self) -> Result<GrpcSessionClient, OpError> {
        let channel = crate::services::connect::grpc::create_channel(&self.grpc_url)
            .await
            .map_err(|e| OpError::new(format!("gRPC channel: {}", e)))?;
        let access_token = self.tokens.read().await.access_token.clone();
        Ok(GrpcSessionClient::new(
            channel,
            AuthInterceptor::with_device_id(&access_token, self.desktop_device_id.clone()),
        ))
    }

    /// Kick off the fetch+classify task. Yields
    /// `KeychainSignMessage::Classified` with the joined signer list.
    pub fn launch(&self) -> Task<Message> {
        let mut client = self.coincube_client.clone();
        let tokens = self.tokens.clone();
        let cube_server_id = self.cube_server_id;
        let cube_uuid = self.cube_uuid.clone();
        let wallet = self.wallet.clone();
        let psbt = self.psbt.clone();

        Task::perform(
            async move {
                // Bake the current access token into the REST client
                // here — inside the async context — so the synchronous
                // `update` path never needs a blocking lock read.
                let access_token = tokens.read().await.access_token.clone();
                client.set_token(&access_token);
                let vault: ConnectVaultResponse = client
                    .get_connect_vault(cube_server_id)
                    .await
                    .map_err(|e| {
                        // CoincubeError formats include the underlying
                        // HTTP status — we surface 401 / 403 as auth
                        // failures so the modal can route to a "sign
                        // in again" path rather than offering retry.
                        let msg = e.to_string();
                        let auth = is_rest_auth_failure(&msg);
                        OpError {
                            message: format!("Failed to fetch vault: {}", msg),
                            auth,
                        }
                    })?;
                let cube_keys: Vec<CubeKeyRaw> =
                    client.get_cube_keys(&cube_uuid).await.map_err(|e| {
                        let msg = e.to_string();
                        let auth = is_rest_auth_failure(&msg);
                        OpError {
                            message: format!("Failed to fetch cube keys: {}", msg),
                            auth,
                        }
                    })?;
                let user: User = client.get_user().await.map_err(|e| {
                    let msg = e.to_string();
                    let auth = is_rest_auth_failure(&msg);
                    OpError {
                        message: format!("Failed to identify viewer: {}", msg),
                        auth,
                    }
                })?;
                let self_user_id: u64 = user.id.into();
                let index: KeychainSignerIndex =
                    build_keychain_index(&vault.members, &cube_keys, self_user_id);
                let required =
                    classify_signers(&psbt, &wallet.main_descriptor, &index, &wallet.keys_aliases)
                        .map_err(|e| OpError::new(e.to_string()))?;
                Ok(ClassifiedSigners {
                    vault,
                    required,
                    self_user_id,
                })
            },
            |r| Message::KeychainSign(KeychainSignMessage::Classified(r)),
        )
    }

    /// Step 2: once classification is in, call `ResolveSigners` for the
    /// vault. Returns `Task::none()` when no Keychain signers remain to
    /// address — the caller surfaces a "use Sign Locally" hint in that
    /// case.
    fn on_classified(&mut self, classified: ClassifiedSigners) -> Task<Message> {
        let has_keychain = classified.required.iter().any(|r| r.is_keychain());
        let vault_id = classified.vault.id;
        let keychain_count = classified
            .required
            .iter()
            .filter(|r| r.is_keychain())
            .count();
        self.classified = Some(classified);
        if !has_keychain {
            tracing::info!(
                target: "coincube_gui::signing",
                vault_id = vault_id,
                phase = "classified",
                "No Keychain signers required for this transaction"
            );
            self.phase = Phase::AllDone;
            self.error = Some(
                "No Keychain signers are required for this transaction. \
                 Use the Sign button to sign locally."
                    .to_string(),
            );
            return Task::none();
        }
        tracing::info!(
            target: "coincube_gui::signing",
            vault_id = vault_id,
            phase = "classified",
            keychain_signers = keychain_count,
            "Classification complete, resolving signer devices"
        );
        // Stash vault_id now that we have it.
        if let Some(c) = self.classified.as_ref() {
            self.vault_id = Some(c.vault.id);
        }
        self.phase = Phase::Resolving;

        let vault_id = self.vault_id.unwrap_or(0).to_string();
        let tokens = self.tokens.clone();
        let grpc_url = self.grpc_url.clone();
        let desktop_device_id = self.desktop_device_id.clone();
        Task::perform(
            async move {
                let channel = crate::services::connect::grpc::create_channel(&grpc_url)
                    .await
                    .map_err(|e| OpError::new(format!("gRPC channel: {}", e)))?;
                let access_token = tokens.read().await.access_token.clone();
                let mut client = GrpcSessionClient::new(
                    channel,
                    AuthInterceptor::with_device_id(&access_token, desktop_device_id),
                );
                client
                    .resolve_signers(vault_id)
                    .await
                    .map_err(OpError::from_status)
            },
            |r| Message::KeychainSign(KeychainSignMessage::SignersResolved(r)),
        )
    }

    /// Step 3: ResolveSigners returned. For each `target`, fire a
    /// `CreateSigningSession`. Each returns its own
    /// `KeychainSignMessage::SessionCreated` message.
    fn on_signers_resolved(&mut self, resp: ResolveSignersResponse) -> Task<Message> {
        tracing::info!(
            target: "coincube_gui::signing",
            vault_id = self.vault_id.unwrap_or(0),
            phase = "resolved",
            targets = resp.targets.len(),
            unresolved = resp.unresolved.len(),
            "ResolveSigners returned"
        );
        self.phase = Phase::Sessions;
        let classified = match self.classified.as_ref() {
            Some(c) => c,
            None => {
                self.error = Some(
                    "Internal error: signers resolved before classification finished".to_string(),
                );
                return Task::none();
            }
        };

        // Capture unresolved → friendly banner. Per-target reason is
        // already in proto; we just stringify here.
        self.unresolved = resp
            .unresolved
            .iter()
            .map(|u| format!("{} ({})", u.key_fingerprint, u.reason))
            .collect();

        // For each resolved target, pair it with the matching
        // classified signer so we can populate the label / fingerprint
        // on the pending row. Targets carry `key_fingerprint` so the
        // join is straightforward.
        let mut tasks = Vec::new();
        let psbt_bytes = self.psbt.serialize();
        let vault_id = resp
            .targets
            .first()
            .map(|_| self.vault_id.unwrap_or(0).to_string())
            .unwrap_or_default();
        let descriptor_id = self.descriptor_id.clone();
        let tokens = self.tokens.clone();
        let grpc_url = self.grpc_url.clone();
        let desktop_device_id = self.desktop_device_id.clone();

        // Owned clone of the classification rows so the per-target
        // dispatch below can borrow without lifetime gymnastics.
        let by_fp: HashMap<String, RequiredSigner> = classified
            .required
            .iter()
            .filter(|r| r.is_keychain())
            .cloned()
            .map(|r| (r.fingerprint().to_string(), r))
            .collect();

        for target in &resp.targets {
            let Some(matched) = by_fp.get(&target.key_fingerprint) else {
                tracing::warn!(
                    "ResolveSigners returned target for fingerprint {} that wasn't classified \
                     as Keychain — skipping",
                    target.key_fingerprint,
                );
                continue;
            };
            let (fingerprint, key_id, label) = match matched {
                RequiredSigner::Keychain {
                    fingerprint,
                    key_id,
                    name,
                    owner_email,
                    ..
                } => {
                    let suffix = owner_email
                        .as_deref()
                        .map(|e| format!(" ({})", e))
                        .unwrap_or_else(|| " (you)".to_string());
                    (*fingerprint, *key_id, format!("{}{}", name, suffix))
                }
                _ => unreachable!(),
            };
            self.pending.push(PendingSession {
                session_id: String::new(), // populated by SessionCreated
                key_id,
                fingerprint,
                device_id: target.device_id.clone(),
                label,
                status: PendingSessionStatus::Creating,
                error: None,
                cancel_requested: false,
                signed_psbt_persisted: false,
                signed_psbt_fetching: false,
            });

            let req = CreateSigningSessionRequest {
                request_id: uuid_v4(),
                vault_id: vault_id.clone(),
                descriptor_id: descriptor_id.clone(),
                psbt: psbt_bytes.clone(),
                targets: vec![target.clone()],
                note: String::new(),
                ttl: Some(prost_types::Duration {
                    seconds: 24 * 60 * 60,
                    nanos: 0,
                }),
                require_user_presence: false,
            };
            let tokens = tokens.clone();
            let grpc_url = grpc_url.clone();
            let desktop_device_id = desktop_device_id.clone();
            tasks.push(Task::perform(
                async move {
                    let channel = crate::services::connect::grpc::create_channel(&grpc_url)
                        .await
                        .map_err(|e| OpError::new(format!("gRPC channel: {}", e)))?;
                    let access_token = tokens.read().await.access_token.clone();
                    let mut client = GrpcSessionClient::new(
                        channel,
                        AuthInterceptor::with_device_id(&access_token, desktop_device_id),
                    );
                    client
                        .create_signing_session(req)
                        .await
                        .map_err(OpError::from_status)
                },
                move |r| Message::KeychainSign(KeychainSignMessage::SessionCreated(fingerprint, r)),
            ));
        }

        Task::batch(tasks)
    }

    fn on_session_created(
        &mut self,
        fingerprint: Fingerprint,
        result: Result<SigningSession, OpError>,
    ) -> Task<Message> {
        // Auth failures should close the modal — retrying won't help
        // until the user signs back in. We surface a single top-level
        // banner rather than mark just this entry as Failed.
        if let Err(e) = &result {
            if e.auth {
                self.error = Some(e.message.clone());
                self.phase = Phase::AllDone;
                // Mark this signer's row terminal so the drain logic
                // (`has_undrained_sessions` / `close_if_dismissed_and_drained`)
                // doesn't treat it as still in flight — otherwise a
                // dismissed modal would stay mounted forever. The
                // single top-level banner above is the user-facing
                // error; we deliberately leave the per-row message unset.
                if let Some(entry) = self
                    .pending
                    .iter_mut()
                    .find(|p| p.fingerprint == fingerprint)
                {
                    entry.status = PendingSessionStatus::Failed;
                }
                return Task::none();
            }
        }
        // Decide inside a scoped borrow whether the just-created
        // session needs an immediate cancel (user hit "Cancel all"
        // while this RPC was in flight), then build the Task after the
        // `&mut self.pending` borrow ends.
        let cancel_sid = {
            let Some(entry) = self
                .pending
                .iter_mut()
                .find(|p| p.fingerprint == fingerprint)
            else {
                return Task::none();
            };
            match result {
                Ok(session) => {
                    let session_id = session.session_id.clone();
                    entry.session_id = session_id.clone();
                    entry.status =
                        PendingSessionStatus::from_proto(session_status_from_i32(session.status));
                    entry.error = None;
                    tracing::info!(
                        target: "coincube_gui::signing",
                        vault_id = self.vault_id.unwrap_or(0),
                        session_id = %session_id,
                        fingerprint = %fingerprint,
                        "Signing session created"
                    );
                    if entry.cancel_requested && !entry.session_id.is_empty() {
                        Some(entry.session_id.clone())
                    } else {
                        None
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        target: "coincube_gui::signing",
                        vault_id = self.vault_id.unwrap_or(0),
                        fingerprint = %fingerprint,
                        auth_failure = e.auth,
                        "CreateSigningSession failed: {}",
                        e.message
                    );
                    entry.status = PendingSessionStatus::Failed;
                    entry.error = Some(e.message);
                    None
                }
            }
        };
        let Some(sid) = cancel_sid else {
            return Task::none();
        };
        let tokens = self.tokens.clone();
        let grpc_url = self.grpc_url.clone();
        let desktop_device_id = self.desktop_device_id.clone();
        let rpc_sid = sid.clone();
        Task::perform(
            async move {
                let channel = crate::services::connect::grpc::create_channel(&grpc_url)
                    .await
                    .map_err(|e| OpError::new(format!("gRPC channel: {}", e)))?;
                let access_token = tokens.read().await.access_token.clone();
                let mut client = GrpcSessionClient::new(
                    channel,
                    AuthInterceptor::with_device_id(&access_token, desktop_device_id),
                );
                client
                    .cancel_signing_session(rpc_sid, "user_cancelled".to_string())
                    .await
                    .map_err(OpError::from_status)
            },
            move |r| Message::KeychainSign(KeychainSignMessage::SessionCancelled(sid.clone(), r)),
        )
    }

    /// Route a top-level `SessionEvent` to its matching `PendingSession`.
    /// Returns a follow-up Task: for `SIGNATURE_SUBMITTED` we fetch the
    /// signed PSBT so we can merge it; other events just bump status.
    pub fn on_session_event(
        &mut self,
        event: crate::services::connect::grpc::connect_v1::SessionEvent,
    ) -> Task<Message> {
        let vault_id_for_log = self.vault_id.unwrap_or(0);
        let Some(entry) = self
            .pending
            .iter_mut()
            .find(|p| p.session_id == event.session_id)
        else {
            tracing::debug!(
                target: "coincube_gui::signing",
                vault_id = vault_id_for_log,
                session_id = %event.session_id,
                event_seq = event.event_seq,
                "SessionEvent for unknown session — modal isn't tracking it, dropping"
            );
            return Task::none();
        };
        tracing::info!(
            target: "coincube_gui::signing",
            vault_id = vault_id_for_log,
            session_id = %entry.session_id,
            event_seq = event.event_seq,
            event_type = event.event_type,
            "SessionEvent received"
        );
        use crate::services::connect::grpc::connect_v1::EventType;
        let event_type = event_type_from_i32(event.event_type);
        let mut fetch_session_id = None;
        match event_type {
            EventType::SessionDelivered => entry.status = PendingSessionStatus::Delivered,
            EventType::SessionViewed => entry.status = PendingSessionStatus::Viewed,
            EventType::SessionApproved => entry.status = PendingSessionStatus::Approved,
            EventType::SignatureSubmitted => {
                entry.status = PendingSessionStatus::PartiallySigned;
                if !entry.signed_psbt_fetching && !entry.signed_psbt_persisted {
                    entry.signed_psbt_fetching = true;
                    fetch_session_id = Some(event.session_id.clone());
                }
            }
            EventType::SessionCompleted => {
                entry.status = PendingSessionStatus::Completed;
                if !entry.signed_psbt_fetching && !entry.signed_psbt_persisted {
                    entry.signed_psbt_fetching = true;
                    fetch_session_id = Some(event.session_id.clone());
                }
            }
            EventType::SessionRejected => {
                entry.status = PendingSessionStatus::Rejected;
                entry.error = Some(event.message.clone());
            }
            EventType::SessionCancelled => entry.status = PendingSessionStatus::Cancelled,
            EventType::SessionExpired => entry.status = PendingSessionStatus::Expired,
            EventType::Error => {
                entry.status = PendingSessionStatus::Failed;
                entry.error = Some(event.message.clone());
            }
            _ => {}
        }
        if let Some(session_id) = fetch_session_id {
            let tokens = self.tokens.clone();
            let grpc_url = self.grpc_url.clone();
            let desktop_device_id = self.desktop_device_id.clone();
            return Task::perform(
                async move {
                    let channel = crate::services::connect::grpc::create_channel(&grpc_url)
                        .await
                        .map_err(|e| OpError::new(format!("gRPC channel: {}", e)))?;
                    let access_token = tokens.read().await.access_token.clone();
                    let mut client = GrpcSessionClient::new(
                        channel,
                        AuthInterceptor::with_device_id(&access_token, desktop_device_id),
                    );
                    client
                        .get_signing_session(session_id.clone())
                        .await
                        .map_err(OpError::from_status)
                },
                {
                    let sid = event.session_id.clone();
                    move |r| {
                        Message::KeychainSign(KeychainSignMessage::SessionFetched(sid.clone(), r))
                    }
                },
            );
        }
        if self.check_all_done() {
            // Reachable here only when this event is the *last* missing
            // piece and every signature was already persisted (i.e. the
            // `Completed` events trail their merges). In the opposite
            // ordering — `Completed` racing ahead of the in-flight
            // fetch+merge — `check_all_done` is still false here and the
            // `Persisted { Ok }` arm performs this transition instead.
            self.phase = Phase::AllDone;
            // `SessionCompleted` produces no follow-up message on its
            // own, and the panel's `Message::Updated(Ok)` arm is the
            // sole place that closes this modal — so without this
            // re-emit the modal would stay stuck on "Closing…".
            return Task::done(Message::Updated(Ok(())));
        }
        Task::none()
    }

    /// Merge the signed PSBT returned by `GetSigningSession` into the
    /// local SpendTx. Run via `Daemon::update_spend_tx` so the existing
    /// signature-merge path applies — same code as the local-signer
    /// flow uses.
    fn on_session_fetched(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        tx: &mut SpendTx,
        session_id: String,
        result: Result<GetSigningSessionResponse, OpError>,
        is_poll: bool,
    ) -> Task<Message> {
        let resp = match result {
            Ok(r) => r,
            Err(e) => {
                if e.auth {
                    self.error = Some(e.message);
                    self.phase = Phase::AllDone;
                    return Task::none();
                }
                if let Some(entry) = self.pending.iter_mut().find(|p| p.session_id == session_id) {
                    entry.signed_psbt_fetching = false;
                    // A transient failure on a background *poll* must not
                    // fail the session — the realtime event or the next
                    // tick can still deliver the signature. Only the
                    // event-driven fetch (where a signature was just
                    // announced) treats a fetch error as terminal.
                    if !is_poll {
                        entry.status = PendingSessionStatus::Failed;
                        entry.error = Some(e.message);
                    }
                }
                return Task::none();
            }
        };
        let Some(session) = resp.session else {
            if let Some(entry) = self.pending.iter_mut().find(|p| p.session_id == session_id) {
                entry.signed_psbt_fetching = false;
                entry.status = PendingSessionStatus::Failed;
                entry.error = Some("API response missing signing session".to_string());
            }
            return Task::none();
        };
        // Reflect the authoritative status from the fetch response.
        // Without this the row can stay stuck at `PartiallySigned` if
        // the separate `SESSION_COMPLETED` stream event races, drops, or
        // never arrives — leaving the modal unable to close even though
        // the signature was fetched, merged, and persisted.
        if let Some(entry) = self.pending.iter_mut().find(|p| p.session_id == session_id) {
            entry.signed_psbt_fetching = false;
            entry.status =
                PendingSessionStatus::from_proto(session_status_from_i32(session.status));
            entry.error = None;
        }
        if session.submitted_signatures.is_empty() {
            if let Some(entry) = self.pending.iter_mut().find(|p| p.session_id == session_id) {
                // A *poll* that arrives before the signer has submitted is
                // normal — keep the freshly-updated status and wait for the
                // next tick/event. Only treat "no signature" as a failure
                // for the event-driven path, or when the API itself reports
                // the session terminally succeeded yet returned nothing.
                if !is_poll || entry.status.is_terminal_success() {
                    entry.status = PendingSessionStatus::Failed;
                    entry.error = Some(
                        "API session completed without returning a submitted signed PSBT"
                            .to_string(),
                    );
                }
            }
            return Task::none();
        }
        // Decode the submitted signed PSBT(s) and merge into the local SpendTx via
        // the daemon's update path. The existing SignModal handler uses
        // the same `merge_signatures` + `update_spend_tx` shape; we
        // replicate it inline rather than refactor for one call site.
        let mut submitted = session.submitted_signatures.into_iter();
        let first = submitted.next().expect("checked non-empty above");
        let mut signed_psbt = match Psbt::deserialize(&first.signed_psbt) {
            Ok(p) => p,
            Err(e) => {
                if let Some(entry) = self.pending.iter_mut().find(|p| p.session_id == session_id) {
                    entry.status = PendingSessionStatus::Failed;
                    entry.error = Some(format!("Malformed signed PSBT from API: {}", e));
                }
                return Task::none();
            }
        };
        for sig in submitted {
            match Psbt::deserialize(&sig.signed_psbt) {
                Ok(psbt) => super::psbt::merge_signatures_pub(&mut signed_psbt, &psbt),
                Err(e) => {
                    if let Some(entry) =
                        self.pending.iter_mut().find(|p| p.session_id == session_id)
                    {
                        entry.status = PendingSessionStatus::Failed;
                        entry.error = Some(format!("Malformed signed PSBT from API: {}", e));
                    }
                    return Task::none();
                }
            }
        }
        tracing::info!(
            target: "coincube_gui::signing",
            vault_id = self.vault_id.unwrap_or(0),
            session_id = %session_id,
            "Merging signed PSBT from session into local SpendTx"
        );
        super::psbt::merge_signatures_pub(&mut tx.psbt, &signed_psbt);
        // Persist the merged PSBT so a restart picks it up. Carry the
        // session_id through so a persistence failure marks the right
        // row instead of being silently swallowed by the panel's
        // generic `Message::Updated(Err)` path.
        let merged = tx.psbt.clone();
        let daemon = daemon.clone();
        Task::perform(
            async move {
                daemon
                    .update_spend_tx(&merged)
                    .await
                    .map_err(|e| AppError::from(e).to_string())
            },
            move |result| {
                Message::KeychainSign(KeychainSignMessage::Persisted {
                    session_id: session_id.clone(),
                    result,
                })
            },
        )
    }

    /// Poll `GetSigningSession` for every live, identified pending signer.
    /// Fallback for missed realtime `SessionEvent`s: a successful poll that
    /// finds a submitted signature flows through the same fetch+merge+persist
    /// path as the event-driven case (via `on_session_fetched(.., is_poll =
    /// true)`), so a signature the desktop never heard about still lands.
    fn poll_pending_sessions(&mut self) -> Task<Message> {
        let tokens = self.tokens.clone();
        let grpc_url = self.grpc_url.clone();
        let desktop_device_id = self.desktop_device_id.clone();
        let mut tasks = Vec::new();
        for entry in self.pending.iter_mut() {
            // Skip sessions that are finished, not yet created, already being
            // fetched (event path or a previous tick), or already captured.
            if entry.status.is_terminal()
                || entry.session_id.is_empty()
                || entry.signed_psbt_fetching
                || entry.signed_psbt_persisted
            {
                continue;
            }
            entry.signed_psbt_fetching = true;
            let session_id = entry.session_id.clone();
            let sid_for_msg = session_id.clone();
            let tokens = tokens.clone();
            let grpc_url = grpc_url.clone();
            let desktop_device_id = desktop_device_id.clone();
            tasks.push(Task::perform(
                async move {
                    let channel = crate::services::connect::grpc::create_channel(&grpc_url)
                        .await
                        .map_err(|e| OpError::new(format!("gRPC channel: {}", e)))?;
                    let access_token = tokens.read().await.access_token.clone();
                    let mut client = GrpcSessionClient::new(
                        channel,
                        AuthInterceptor::with_device_id(&access_token, desktop_device_id),
                    );
                    client
                        .get_signing_session(session_id.clone())
                        .await
                        .map_err(OpError::from_status)
                },
                move |r| {
                    Message::KeychainSign(KeychainSignMessage::SessionPolled(
                        sid_for_msg.clone(),
                        r,
                    ))
                },
            ));
        }
        Task::batch(tasks)
    }

    fn on_session_cancelled(&mut self, session_id: String, result: Result<(), OpError>) {
        let Some(entry) = self.pending.iter_mut().find(|p| p.session_id == session_id) else {
            return;
        };
        match result {
            Ok(()) => entry.status = PendingSessionStatus::Cancelled,
            // Cancel-failed leaves the session in its previous state —
            // best-effort, since the session is being discarded
            // server-side too when its TTL elapses.
            Err(e) => entry.error = Some(format!("Cancel failed: {}", e.message)),
        }
    }

    /// Cancel every non-terminal session. Decision (per Phase 3 plan):
    /// discard partial signatures rather than offer "keep what we have".
    /// Simpler UX; matches the original `KEY_ALREADY_USED_IN_VAULT`
    /// rollback semantics.
    pub fn cancel_all(&mut self) -> Task<Message> {
        let non_terminal = self
            .pending
            .iter()
            .filter(|p| !p.status.is_terminal())
            .count();
        tracing::info!(
            target: "coincube_gui::signing",
            vault_id = self.vault_id.unwrap_or(0),
            cancelling = non_terminal,
            "User cancelled Keychain signing flow"
        );
        let tokens = self.tokens.clone();
        let grpc_url = self.grpc_url.clone();
        let desktop_device_id = self.desktop_device_id.clone();
        let mut tasks = Vec::new();
        for entry in self.pending.iter_mut() {
            if entry.status.is_terminal() {
                continue;
            }
            if entry.session_id.is_empty() {
                // CreateSigningSession is still in flight — there's no
                // session_id to cancel yet. Flag the intent so
                // `on_session_created` cancels it the moment it lands,
                // instead of letting it outlive this cancel-all.
                entry.cancel_requested = true;
                continue;
            }
            let sid = entry.session_id.clone();
            let tokens = tokens.clone();
            let grpc_url = grpc_url.clone();
            let desktop_device_id = desktop_device_id.clone();
            tasks.push(Task::perform(
                async move {
                    let channel = crate::services::connect::grpc::create_channel(&grpc_url)
                        .await
                        .map_err(|e| OpError::new(format!("gRPC channel: {}", e)))?;
                    let access_token = tokens.read().await.access_token.clone();
                    let mut client = GrpcSessionClient::new(
                        channel,
                        AuthInterceptor::with_device_id(&access_token, desktop_device_id),
                    );
                    client
                        .cancel_signing_session(sid.clone(), "user_cancelled".to_string())
                        .await
                        .map_err(OpError::from_status)
                },
                {
                    let sid = entry.session_id.clone();
                    move |r| {
                        Message::KeychainSign(KeychainSignMessage::SessionCancelled(sid.clone(), r))
                    }
                },
            ));
        }
        Task::batch(tasks)
    }

    /// Retry one signer whose session expired / was rejected. Creates a
    /// fresh `SigningSession` against the same `target_device_id` /
    /// `target_key_id`. The user must have manually addressed the
    /// underlying problem on the signer's device.
    pub fn retry_signer(&mut self, index: usize) -> Task<Message> {
        let Some(entry) = self.pending.get_mut(index) else {
            return Task::none();
        };
        // Only the recoverable failure states are retryable. Anything
        // else — in-flight sessions, a successful `Completed`, or a
        // user-initiated `Cancelled` — must not spawn a duplicate
        // signing session. (Matches the UI's Retry-button gating.)
        if !matches!(
            entry.status,
            PendingSessionStatus::Rejected
                | PendingSessionStatus::Expired
                | PendingSessionStatus::Failed
        ) {
            return Task::none();
        }
        let fingerprint = entry.fingerprint;
        let device_id = entry.device_id.clone();
        let key_id = entry.key_id;
        // Reset state to creating; clear any previous error and stale
        // cancel intent (an explicit retry overrides a prior
        // cancel-all so the new session isn't auto-cancelled).
        entry.status = PendingSessionStatus::Creating;
        entry.session_id.clear();
        entry.error = None;
        entry.cancel_requested = false;
        // The retried session produces a fresh signature that must be
        // fetched + persisted again before this row counts as done.
        entry.signed_psbt_persisted = false;
        entry.signed_psbt_fetching = false;

        let psbt_bytes = self.psbt.serialize();
        let vault_id = self.vault_id.unwrap_or(0).to_string();
        let descriptor_id = self.descriptor_id.clone();
        let tokens = self.tokens.clone();
        let grpc_url = self.grpc_url.clone();
        let desktop_device_id = self.desktop_device_id.clone();
        Task::perform(
            async move {
                let channel = crate::services::connect::grpc::create_channel(&grpc_url)
                    .await
                    .map_err(|e| OpError::new(format!("gRPC channel: {}", e)))?;
                let access_token = tokens.read().await.access_token.clone();
                let mut client = GrpcSessionClient::new(
                    channel,
                    AuthInterceptor::with_device_id(&access_token, desktop_device_id),
                );
                let req = CreateSigningSessionRequest {
                    request_id: uuid_v4(),
                    vault_id,
                    descriptor_id,
                    psbt: psbt_bytes,
                    targets: vec![crate::services::connect::grpc::connect_v1::SignerTarget {
                        device_id,
                        key_fingerprint: fingerprint_as_str(&fingerprint),
                        key_id: key_id.to_string(),
                    }],
                    note: String::new(),
                    ttl: Some(prost_types::Duration {
                        seconds: 24 * 60 * 60,
                        nanos: 0,
                    }),
                    require_user_presence: false,
                };
                client
                    .create_signing_session(req)
                    .await
                    .map_err(OpError::from_status)
            },
            move |r| Message::KeychainSign(KeychainSignMessage::SessionCreated(fingerprint, r)),
        )
    }
}

impl KeychainSignModal {
    fn dispatch(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        message: Message,
        tx: &mut SpendTx,
    ) -> Task<Message> {
        match message {
            Message::KeychainSign(KeychainSignMessage::Classified(res)) => match res {
                Ok(c) => return self.on_classified(c),
                Err(e) => {
                    self.error = Some(e.message);
                    self.phase = Phase::AllDone;
                }
            },
            Message::KeychainSign(KeychainSignMessage::SignersResolved(res)) => match res {
                Ok(r) => return self.on_signers_resolved(r),
                Err(e) => {
                    self.error = if e.auth {
                        Some(e.message)
                    } else {
                        Some(format!("ResolveSigners failed: {}", e.message))
                    };
                    self.phase = Phase::AllDone;
                }
            },
            Message::KeychainSign(KeychainSignMessage::SessionCreated(fp, res)) => {
                return self.on_session_created(fp, res);
            }
            Message::KeychainSign(KeychainSignMessage::SessionFetched(sid, res)) => {
                return self.on_session_fetched(daemon, tx, sid, res, false);
            }
            Message::KeychainSign(KeychainSignMessage::SessionPolled(sid, res)) => {
                return self.on_session_fetched(daemon, tx, sid, res, true);
            }
            Message::KeychainSign(KeychainSignMessage::PollTick) => {
                return self.poll_pending_sessions();
            }
            Message::KeychainSign(KeychainSignMessage::SessionCancelled(sid, res)) => {
                self.on_session_cancelled(sid, res);
            }
            Message::KeychainSign(KeychainSignMessage::Persisted { session_id, result }) => {
                match result {
                    Ok(()) => {
                        // The signed PSBT for this session is now merged
                        // and durably saved — mark the row so
                        // `check_all_done` can count it. This (not the
                        // API `Completed` event) is the authoritative
                        // "this signature is captured" signal.
                        if let Some(entry) =
                            self.pending.iter_mut().find(|p| p.session_id == session_id)
                        {
                            entry.signed_psbt_persisted = true;
                        }
                        if self.check_all_done() {
                            self.phase = Phase::AllDone;
                        }
                        // Re-emit the message the panel expects so its
                        // existing post-save flow (saved flag, sigs
                        // recompute, keychain modal close) runs exactly
                        // as before this message carried session identity.
                        // When this was the last outstanding persist the
                        // phase is now `AllDone`, so the panel closes the
                        // modal against a fully-merged PSBT.
                        return Task::done(Message::Updated(Ok(())));
                    }
                    Err(e) => {
                        if let Some(entry) =
                            self.pending.iter_mut().find(|p| p.session_id == session_id)
                        {
                            entry.status = PendingSessionStatus::Failed;
                            entry.signed_psbt_fetching = false;
                            entry.error = Some(format!("Failed to persist signed PSBT: {}", e));
                        }
                    }
                }
            }
            Message::KeychainSign(KeychainSignMessage::StreamEvent(event)) => {
                return self.on_session_event(event);
            }
            Message::KeychainSign(KeychainSignMessage::StreamHealth(status)) => {
                self.stream_health = status;
            }
            Message::View(view::Message::Spend(SpendTxMessage::CancelKeychainSign)) => {
                return self.cancel_all();
            }
            Message::View(view::Message::Spend(SpendTxMessage::RetryKeychainSigner(idx))) => {
                return self.retry_signer(idx);
            }
            _ => {}
        }
        Task::none()
    }
}

impl Modal for KeychainSignModal {
    fn subscription(&self) -> Subscription<Message> {
        // Poll pending signing sessions as a fallback for realtime
        // `SessionEvent`s that never arrive (gRPC stream flapping/superseded,
        // vault-scope mismatch, …). Active only while we have at least one
        // non-terminal session whose signature hasn't yet been captured —
        // so it self-stops once everyone has signed (or the flow ends).
        let needs_poll = matches!(self.phase, Phase::Sessions)
            && self.pending.iter().any(|p| {
                !p.status.is_terminal() && !p.signed_psbt_persisted && !p.session_id.is_empty()
            });
        if needs_poll {
            iced::time::every(std::time::Duration::from_secs(SESSION_POLL_INTERVAL_SECS))
                .map(|_| Message::KeychainSign(KeychainSignMessage::PollTick))
        } else {
            Subscription::none()
        }
    }

    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        message: Message,
        tx: &mut SpendTx,
    ) -> Task<Message> {
        // Single choke point: whichever arm handled the message, if the
        // modal was dismissed mid-flight and every pending session is
        // now terminal, tear it down here. Centralised so no individual
        // arm (Persisted / SessionFetched errors, stream events, …) can
        // leak a hidden dismissed modal by forgetting to re-check.
        let task = self.dispatch(daemon, message, tx);
        Task::batch([task, self.close_if_dismissed_and_drained()])
    }

    fn view<'a>(&'a self, content: Element<'a, view::Message>) -> Element<'a, view::Message> {
        if !self.display_modal || self.dismissed {
            return content;
        }
        let mut col = Column::new()
            .spacing(modal_const::V_SPACING)
            .padding(15)
            .width(iced::Length::Fixed(modal_const::MODAL_WIDTH as f32));

        col = col.push(p1_bold("Sign via Keychain"));

        // Stream-health banner: surfaced only while sessions are
        // pending and the realtime stream is unhealthy. We don't
        // pre-cancel anything on disconnect because sessions live
        // server-side; reconnecting catches up via `last_seen_seq`.
        let has_pending_nonterminal = self.pending.iter().any(|p| !p.status.is_terminal());
        if has_pending_nonterminal {
            match &self.stream_health {
                crate::app::ConnectionStatus::Connecting => {
                    col = col.push(p1_regular(
                        "Connection lost — reconnecting. Your signing requests are \
                         still active server-side; updates will catch up once the \
                         connection comes back.",
                    ));
                }
                crate::app::ConnectionStatus::Error(e) => {
                    col = col.push(p1_regular(format!(
                        "Connection error ({}). Your signing requests are still \
                         active server-side; reconnect to see updates.",
                        e,
                    )));
                }
                _ => {}
            }
        }

        if let Some(err) = &self.error {
            // Top-level errors come from `ResolveSigners` /
            // `CreateSigningSession` / Connect-not-ready paths. The
            // Cancel-all button below handles the user-out for these
            // states; we don't render a separate Retry-all because
            // the right action depends on the error (re-open modal
            // for transient issues, re-login for auth failures).
            col = col.push(p1_regular(format!("Couldn't start signing: {}", err)));
        }

        // Unresolved (resolved-but-unaddressable) signers. The "owner
        // has no registered device" case is the most common — the
        // contact hasn't installed the Keychain app yet. Friendlier
        // copy than the raw API reason string.
        for u in &self.unresolved {
            // The format from `on_signers_resolved` is `"<fingerprint>
            // (<reason>)"`. We surface the friendlier message but
            // keep the original suffix so an unfamiliar reason still
            // reaches the user verbatim (forward-compat with new API
            // reason codes).
            let friendly = if u.contains("no_device_registered") {
                format!(
                    "{} hasn't set up the Keychain app yet. Ask them to install it \
                     and sign in, then retry.",
                    u,
                )
            } else if u.contains("all_devices_revoked") {
                format!(
                    "{} has revoked every device on their account. They need to \
                     register a new device before this transaction can be signed.",
                    u,
                )
            } else if u.contains("owner_unknown") {
                format!(
                    "{} — this signer's owner isn't known to the backend. \
                     Contact support if this persists.",
                    u,
                )
            } else {
                format!("Cannot sign with {} — owner has no registered device", u)
            };
            col = col.push(p1_regular(friendly));
        }
        match self.phase {
            Phase::Loading => col = col.push(p1_regular("Loading vault members…")),
            Phase::Resolving => col = col.push(p1_regular("Looking up signer devices…")),
            Phase::Sessions => {
                col = col.push(p1_regular(format!(
                    "Waiting on {} signer(s)…",
                    self.pending.len(),
                )));
                for (i, p) in self.pending.iter().enumerate() {
                    let mut row = Row::new()
                        .spacing(modal_const::V_SPACING)
                        .push(p1_regular(p.label.clone()))
                        .push(iced::widget::Space::new().width(iced::Length::Fill))
                        .push(p1_regular(p.status.label()));
                    if matches!(
                        p.status,
                        PendingSessionStatus::Rejected
                            | PendingSessionStatus::Expired
                            | PendingSessionStatus::Failed
                    ) {
                        row = row.push(
                            button::secondary(Some(icon::reload_icon()), "Retry").on_press(
                                view::Message::Spend(SpendTxMessage::RetryKeychainSigner(i)),
                            ),
                        );
                    }
                    col = col.push(row);
                    // Per-row error / explanation. We choose the copy
                    // based on the status so the user sees actionable
                    // text rather than the raw `entry.error` string
                    // (which is occasionally a tonic Status). The raw
                    // error is still surfaced below as a fallback /
                    // forward-compat when an unknown error shape
                    // arrives.
                    let row_hint: Option<String> = match p.status {
                        PendingSessionStatus::Rejected => Some(format!(
                            "  {} declined the request. Tap Retry to ask again, or \
                             Cancel all to abandon.",
                            p.label,
                        )),
                        PendingSessionStatus::Expired => Some(format!(
                            "  {} didn't respond within 24h. Tap Retry to send a \
                             fresh request.",
                            p.label,
                        )),
                        PendingSessionStatus::Failed => Some(format!(
                            "  Couldn't reach {}'s device. Tap Retry to try again.",
                            p.label,
                        )),
                        _ => None,
                    };
                    if let Some(hint) = row_hint {
                        col = col.push(p1_regular(hint));
                    }
                    if let Some(err) = &p.error {
                        col = col.push(p1_regular(format!("  {}", err)));
                    }
                }
            }
            Phase::AllDone => {
                col = col.push(p1_regular("All Keychain signers have completed. Closing…"));
            }
        }

        // Footer with Cancel All (always available).
        col = col.push(iced::widget::Space::new().height(iced::Length::Fixed(10.0)));
        col = col.push(
            button::secondary(None, "Cancel all")
                .on_press(view::Message::Spend(SpendTxMessage::CancelKeychainSign)),
        );

        // Wrap the column in a card-styled Container so it has its own
        // backdrop on top of the modal's dim layer. Without this the text
        // renders directly over the 80%-black dim and is invisible on
        // themes whose default text colour is dark; the only thing the
        // user sees is the "Cancel all" button (which has its own style).
        let modal_card = Container::new(col).style(theme::card::simple);

        modal::Modal::new(content, modal_card)
            .on_blur(Some(view::Message::Spend(SpendTxMessage::Cancel)))
            .into()
    }
}

// ───── small helpers ──────────────────────────────────────────────────

/// Best-effort detector for REST-side auth failures. `CoincubeError`'s
/// `Display` impl includes the HTTP status; we look for the standard
/// `401` / `403` markers and the Coincube-API auth error codes that
/// the desktop has historically used. False negatives just route to
/// the generic "Other" path — the user still sees the original
/// message, they just don't get the "session expired" closed-modal
/// path.
fn is_rest_auth_failure(msg: &str) -> bool {
    let lower = msg.to_ascii_lowercase();
    lower.contains("401")
        || lower.contains("403")
        || lower.contains("unauthorized")
        || lower.contains("forbidden")
        || lower.contains("expired token")
        || lower.contains("invalid token")
        || lower.contains("token expired")
        || lower.contains("jwt expired")
}

/// Convert a `tonic::Status` into a user-friendly message string.
///
/// The default `Display` impl prints `Status { code: Unauthenticated,
/// message: "JWT expired", ... }` which is unhelpful as a modal banner.
/// We hand-pick the codes that matter for the signing flow and fall
/// back to the original message for everything else. Returns
/// `(friendly_text, is_auth_failure)`; callers branch on the bool to
/// decide whether to surface a "Please sign in again." path that
/// closes the modal rather than just dismissing the error.
fn friendly_grpc_error(status: tonic::Status) -> (String, bool) {
    match status.code() {
        tonic::Code::Unauthenticated => (
            "Your Connect session has expired. Please sign in again.".to_string(),
            true,
        ),
        tonic::Code::PermissionDenied => (
            "You don't have permission to sign for this vault. \
             Sign in with the account that owns the vault."
                .to_string(),
            true,
        ),
        tonic::Code::Unavailable => (
            "Coincube Connect is temporarily unreachable. Check your \
             network and try again."
                .to_string(),
            false,
        ),
        tonic::Code::DeadlineExceeded => (
            "Request timed out. The signing service may be slow — try again.".to_string(),
            false,
        ),
        _ => (status.message().to_string(), false),
    }
}

fn uuid_v4() -> String {
    // Avoid adding a `uuid` crate dep for a single call: format eight
    // bytes from `rand` as `xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx` per
    // RFC 4122 v4. Falls back to a timestamp-derived id if rng is
    // unavailable (extremely unlikely on desktop).
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40; // version 4
    bytes[8] = (bytes[8] & 0x3f) | 0x80; // variant 1
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5],
        bytes[6], bytes[7],
        bytes[8], bytes[9],
        bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
    )
}

fn fingerprint_as_str(fp: &Fingerprint) -> String {
    fp.to_string()
}

/// Manual i32 → SessionStatus mapping. The generated `prost::Enumeration`
/// derive emits a `TryFrom<i32>` impl, but the conversion is small and a
/// direct match avoids the trait dance plus lets the `_` fallback log
/// once for any new variant the API adds before the desktop ships an
/// update.
fn session_status_from_i32(v: i32) -> ProtoSessionStatus {
    match v {
        1 => ProtoSessionStatus::Pending,
        2 => ProtoSessionStatus::Delivered,
        3 => ProtoSessionStatus::Viewed,
        4 => ProtoSessionStatus::Approved,
        5 => ProtoSessionStatus::PartiallySigned,
        6 => ProtoSessionStatus::Completed,
        7 => ProtoSessionStatus::Rejected,
        8 => ProtoSessionStatus::Cancelled,
        9 => ProtoSessionStatus::Expired,
        10 => ProtoSessionStatus::Failed,
        _ => ProtoSessionStatus::Unspecified,
    }
}

fn event_type_from_i32(v: i32) -> crate::services::connect::grpc::connect_v1::EventType {
    use crate::services::connect::grpc::connect_v1::EventType;
    match v {
        1 => EventType::SessionCreated,
        2 => EventType::SessionDelivered,
        3 => EventType::SessionViewed,
        4 => EventType::SessionApproved,
        5 => EventType::SessionRejected,
        6 => EventType::SignatureSubmitted,
        7 => EventType::SessionCompleted,
        8 => EventType::SessionCancelled,
        9 => EventType::SessionExpired,
        10 => EventType::Error,
        11 => EventType::DeviceOnline,
        12 => EventType::DeviceOffline,
        _ => EventType::Unspecified,
    }
}
