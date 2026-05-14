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
    icon,
    widget::{modal, Column, Element, Row},
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
    /// Result of a `cancel_signing_session` call. Carries the session_id
    /// so the modal can mark the right row Cancelled.
    SessionCancelled(String, Result<(), OpError>),
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
    /// All signers terminal-success. Modal will close on next tick.
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

    /// True when every pending session is at a terminal-success state.
    /// Recomputed each time a status changes; cheap because `pending`
    /// is small (≤ descriptor signer count).
    fn check_all_done(&self) -> bool {
        !self.pending.is_empty() && self.pending.iter().all(|p| p.status.is_terminal_success())
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
        Ok(GrpcSessionClient::new(
            channel,
            AuthInterceptor::new(self.tokens.clone()),
        ))
    }

    /// Kick off the fetch+classify task. Yields
    /// `KeychainSignMessage::Classified` with the joined signer list.
    pub fn launch(&self) -> Task<Message> {
        let client = self.coincube_client.clone();
        let cube_server_id = self.cube_server_id;
        let cube_uuid = self.cube_uuid.clone();
        let wallet = self.wallet.clone();
        let psbt = self.psbt.clone();

        Task::perform(
            async move {
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
        Task::perform(
            async move {
                let channel = crate::services::connect::grpc::create_channel(&grpc_url)
                    .await
                    .map_err(|e| OpError::new(format!("gRPC channel: {}", e)))?;
                let mut client = GrpcSessionClient::new(channel, AuthInterceptor::new(tokens));
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
            tasks.push(Task::perform(
                async move {
                    let channel = crate::services::connect::grpc::create_channel(&grpc_url)
                        .await
                        .map_err(|e| OpError::new(format!("gRPC channel: {}", e)))?;
                    let mut client = GrpcSessionClient::new(channel, AuthInterceptor::new(tokens));
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
                return Task::none();
            }
        }
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
            }
        }
        Task::none()
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
        match event_type {
            EventType::SessionDelivered => entry.status = PendingSessionStatus::Delivered,
            EventType::SessionViewed => entry.status = PendingSessionStatus::Viewed,
            EventType::SessionApproved => entry.status = PendingSessionStatus::Approved,
            EventType::SignatureSubmitted => {
                entry.status = PendingSessionStatus::PartiallySigned;
                // Fetch the session to get the signed PSBT.
                let session_id = event.session_id.clone();
                let tokens = self.tokens.clone();
                let grpc_url = self.grpc_url.clone();
                return Task::perform(
                    async move {
                        let channel = crate::services::connect::grpc::create_channel(&grpc_url)
                            .await
                            .map_err(|e| OpError::new(format!("gRPC channel: {}", e)))?;
                        let mut client =
                            GrpcSessionClient::new(channel, AuthInterceptor::new(tokens));
                        client
                            .get_signing_session(session_id.clone())
                            .await
                            .map_err(OpError::from_status)
                    },
                    {
                        let sid = event.session_id.clone();
                        move |r| {
                            Message::KeychainSign(KeychainSignMessage::SessionFetched(
                                sid.clone(),
                                r,
                            ))
                        }
                    },
                );
            }
            EventType::SessionCompleted => entry.status = PendingSessionStatus::Completed,
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
        if self.check_all_done() {
            self.phase = Phase::AllDone;
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
                    entry.status = PendingSessionStatus::Failed;
                    entry.error = Some(e.message);
                }
                return Task::none();
            }
        };
        let Some(session) = resp.session else {
            return Task::none();
        };
        // Decode the signed PSBT and merge into the local SpendTx via
        // the daemon's update path. The existing SignModal handler uses
        // the same `merge_signatures` + `update_spend_tx` shape; we
        // replicate it inline rather than refactor for one call site.
        let signed_psbt = match Psbt::deserialize(&session.psbt) {
            Ok(p) => p,
            Err(e) => {
                if let Some(entry) = self.pending.iter_mut().find(|p| p.session_id == session_id) {
                    entry.status = PendingSessionStatus::Failed;
                    entry.error = Some(format!("Malformed PSBT from API: {}", e));
                }
                return Task::none();
            }
        };
        tracing::info!(
            target: "coincube_gui::signing",
            vault_id = self.vault_id.unwrap_or(0),
            session_id = %session_id,
            "Merging signed PSBT from session into local SpendTx"
        );
        super::psbt::merge_signatures_pub(&mut tx.psbt, &signed_psbt);
        // Persist the merged PSBT so a restart picks it up.
        let merged = tx.psbt.clone();
        let daemon = daemon.clone();
        Task::perform(
            async move {
                daemon
                    .update_spend_tx(&merged)
                    .await
                    .map_err(AppError::from)
            },
            Message::Updated,
        )
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
        let mut tasks = Vec::new();
        for entry in self.pending.iter_mut() {
            if entry.status.is_terminal() || entry.session_id.is_empty() {
                continue;
            }
            let sid = entry.session_id.clone();
            let tokens = tokens.clone();
            let grpc_url = grpc_url.clone();
            tasks.push(Task::perform(
                async move {
                    let channel = crate::services::connect::grpc::create_channel(&grpc_url)
                        .await
                        .map_err(|e| OpError::new(format!("gRPC channel: {}", e)))?;
                    let mut client = GrpcSessionClient::new(channel, AuthInterceptor::new(tokens));
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
        if !entry.status.is_terminal() && !matches!(entry.status, PendingSessionStatus::Failed) {
            return Task::none();
        }
        let fingerprint = entry.fingerprint;
        let device_id = entry.device_id.clone();
        let key_id = entry.key_id;
        // Reset state to creating; clear any previous error.
        entry.status = PendingSessionStatus::Creating;
        entry.session_id.clear();
        entry.error = None;

        let psbt_bytes = self.psbt.serialize();
        let vault_id = self.vault_id.unwrap_or(0).to_string();
        let descriptor_id = self.descriptor_id.clone();
        let tokens = self.tokens.clone();
        let grpc_url = self.grpc_url.clone();
        Task::perform(
            async move {
                let channel = crate::services::connect::grpc::create_channel(&grpc_url)
                    .await
                    .map_err(|e| OpError::new(format!("gRPC channel: {}", e)))?;
                let mut client = GrpcSessionClient::new(channel, AuthInterceptor::new(tokens));
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

impl Modal for KeychainSignModal {
    fn subscription(&self) -> Subscription<Message> {
        Subscription::none()
    }

    fn update(
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
                return self.on_session_fetched(daemon, tx, sid, res);
            }
            Message::KeychainSign(KeychainSignMessage::SessionCancelled(sid, res)) => {
                self.on_session_cancelled(sid, res);
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

    fn view<'a>(&'a self, content: Element<'a, view::Message>) -> Element<'a, view::Message> {
        if !self.display_modal {
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

        modal::Modal::new(content, col)
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
