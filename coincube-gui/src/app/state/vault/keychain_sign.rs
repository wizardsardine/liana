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
    /// Set by `cancel_all` when the user cancels while this row's
    /// `CreateSigningSession` RPC is still in flight (empty
    /// `session_id`). `on_session_created` consults it so the
    /// just-created session is cancelled immediately rather than
    /// outliving the cancelled flow. Cleared on retry.
    pub cancel_requested: bool,
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

/// Sub-messages routed through `Message::KeychainSign`. Kept in this
/// module so they can evolve alongside the modal without churning
/// `app::message::Message`.
#[derive(Debug, Clone)]
pub enum KeychainSignMessage {
    /// Result of the initial parallel fetch:
    /// `(connect_vault_response, cube_keys, viewer_user)`.
    Classified(Result<ClassifiedSigners, String>),
    /// Result of `ResolveSigners(vault_id)`.
    SignersResolved(Result<ResolveSignersResponse, String>),
    /// Result of a single `CreateSigningSession` call, keyed by the
    /// fingerprint of the signer being addressed.
    SessionCreated(Fingerprint, Result<SigningSession, String>),
    /// Result of a `GetSigningSession` fetch after a SIGNATURE_SUBMITTED
    /// event, used to pull down the signed PSBT for merge.
    SessionFetched(String, Result<GetSigningSessionResponse, String>),
    /// Result of a `cancel_signing_session` call. Carries the session_id
    /// so the modal can mark the right row Cancelled.
    SessionCancelled(String, Result<(), String>),
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
    async fn make_session_client(&self) -> Result<GrpcSessionClient, String> {
        let channel = crate::services::connect::grpc::create_channel(&self.grpc_url)
            .await
            .map_err(|e| format!("gRPC channel: {}", e))?;
        let access_token = self.tokens.read().await.access_token.clone();
        Ok(GrpcSessionClient::new(
            channel,
            AuthInterceptor::new(&access_token),
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
                    .map_err(|e| format!("Failed to fetch vault: {}", e))?;
                let cube_keys: Vec<CubeKeyRaw> = client
                    .get_cube_keys(&cube_uuid)
                    .await
                    .map_err(|e| format!("Failed to fetch cube keys: {}", e))?;
                let user: User = client
                    .get_user()
                    .await
                    .map_err(|e| format!("Failed to identify viewer: {}", e))?;
                let self_user_id: u64 = user.id.into();
                let index: KeychainSignerIndex =
                    build_keychain_index(&vault.members, &cube_keys, self_user_id);
                let required =
                    classify_signers(&psbt, &wallet.main_descriptor, &index, &wallet.keys_aliases)
                        .map_err(|e| e.to_string())?;
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
        self.classified = Some(classified);
        if !has_keychain {
            self.phase = Phase::AllDone;
            self.error = Some(
                "No Keychain signers are required for this transaction. \
                 Use the Sign button to sign locally."
                    .to_string(),
            );
            return Task::none();
        }
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
                    .map_err(|e| format!("gRPC channel: {}", e))?;
                let access_token = tokens.read().await.access_token.clone();
                let mut client =
                    GrpcSessionClient::new(channel, AuthInterceptor::new(&access_token));
                client
                    .resolve_signers(vault_id)
                    .await
                    .map_err(|s| format!("{}", s))
            },
            |r| Message::KeychainSign(KeychainSignMessage::SignersResolved(r)),
        )
    }

    /// Step 3: ResolveSigners returned. For each `target`, fire a
    /// `CreateSigningSession`. Each returns its own
    /// `KeychainSignMessage::SessionCreated` message.
    fn on_signers_resolved(&mut self, resp: ResolveSignersResponse) -> Task<Message> {
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
                cancel_requested: false,
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
                        .map_err(|e| format!("gRPC channel: {}", e))?;
                    let access_token = tokens.read().await.access_token.clone();
                    let mut client =
                        GrpcSessionClient::new(channel, AuthInterceptor::new(&access_token));
                    client
                        .create_signing_session(req)
                        .await
                        .map_err(|s| format!("{}", s))
                },
                move |r| Message::KeychainSign(KeychainSignMessage::SessionCreated(fingerprint, r)),
            ));
        }

        Task::batch(tasks)
    }

    fn on_session_created(
        &mut self,
        fingerprint: Fingerprint,
        result: Result<SigningSession, String>,
    ) -> Task<Message> {
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
                    entry.session_id = session.session_id.clone();
                    entry.status =
                        PendingSessionStatus::from_proto(session_status_from_i32(session.status));
                    entry.error = None;
                    if entry.cancel_requested && !entry.session_id.is_empty() {
                        Some(entry.session_id.clone())
                    } else {
                        None
                    }
                }
                Err(e) => {
                    entry.status = PendingSessionStatus::Failed;
                    entry.error = Some(e);
                    None
                }
            }
        };
        let Some(sid) = cancel_sid else {
            return Task::none();
        };
        let tokens = self.tokens.clone();
        let grpc_url = self.grpc_url.clone();
        let rpc_sid = sid.clone();
        Task::perform(
            async move {
                let channel = crate::services::connect::grpc::create_channel(&grpc_url)
                    .await
                    .map_err(|e| format!("gRPC channel: {}", e))?;
                let access_token = tokens.read().await.access_token.clone();
                let mut client =
                    GrpcSessionClient::new(channel, AuthInterceptor::new(&access_token));
                client
                    .cancel_signing_session(rpc_sid, "user_cancelled".to_string())
                    .await
                    .map_err(|s| format!("{}", s))
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
        let Some(entry) = self
            .pending
            .iter_mut()
            .find(|p| p.session_id == event.session_id)
        else {
            return Task::none();
        };
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
                            .map_err(|e| format!("gRPC channel: {}", e))?;
                        let access_token = tokens.read().await.access_token.clone();
                        let mut client =
                            GrpcSessionClient::new(channel, AuthInterceptor::new(&access_token));
                        client
                            .get_signing_session(session_id.clone())
                            .await
                            .map_err(|s| format!("{}", s))
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
        result: Result<GetSigningSessionResponse, String>,
    ) -> Task<Message> {
        let resp = match result {
            Ok(r) => r,
            Err(e) => {
                if let Some(entry) = self.pending.iter_mut().find(|p| p.session_id == session_id) {
                    entry.status = PendingSessionStatus::Failed;
                    entry.error = Some(e);
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

    fn on_session_cancelled(&mut self, session_id: String, result: Result<(), String>) {
        let Some(entry) = self.pending.iter_mut().find(|p| p.session_id == session_id) else {
            return;
        };
        match result {
            Ok(()) => entry.status = PendingSessionStatus::Cancelled,
            Err(e) => entry.error = Some(format!("Cancel failed: {}", e)),
        }
    }

    /// Cancel every non-terminal session. Decision (per Phase 3 plan):
    /// discard partial signatures rather than offer "keep what we have".
    /// Simpler UX; matches the original `KEY_ALREADY_USED_IN_VAULT`
    /// rollback semantics.
    pub fn cancel_all(&mut self) -> Task<Message> {
        let tokens = self.tokens.clone();
        let grpc_url = self.grpc_url.clone();
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
            tasks.push(Task::perform(
                async move {
                    let channel = crate::services::connect::grpc::create_channel(&grpc_url)
                        .await
                        .map_err(|e| format!("gRPC channel: {}", e))?;
                    let access_token = tokens.read().await.access_token.clone();
                    let mut client =
                        GrpcSessionClient::new(channel, AuthInterceptor::new(&access_token));
                    client
                        .cancel_signing_session(sid.clone(), "user_cancelled".to_string())
                        .await
                        .map_err(|s| format!("{}", s))
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
        // Reset state to creating; clear any previous error and stale
        // cancel intent (an explicit retry overrides a prior
        // cancel-all so the new session isn't auto-cancelled).
        entry.status = PendingSessionStatus::Creating;
        entry.session_id.clear();
        entry.error = None;
        entry.cancel_requested = false;

        let psbt_bytes = self.psbt.serialize();
        let vault_id = self.vault_id.unwrap_or(0).to_string();
        let descriptor_id = self.descriptor_id.clone();
        let tokens = self.tokens.clone();
        let grpc_url = self.grpc_url.clone();
        Task::perform(
            async move {
                let channel = crate::services::connect::grpc::create_channel(&grpc_url)
                    .await
                    .map_err(|e| format!("gRPC channel: {}", e))?;
                let access_token = tokens.read().await.access_token.clone();
                let mut client =
                    GrpcSessionClient::new(channel, AuthInterceptor::new(&access_token));
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
                    .map_err(|s| format!("{}", s))
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
                    self.error = Some(e);
                    self.phase = Phase::AllDone;
                }
            },
            Message::KeychainSign(KeychainSignMessage::SignersResolved(res)) => match res {
                Ok(r) => return self.on_signers_resolved(r),
                Err(e) => {
                    self.error = Some(format!("ResolveSigners failed: {}", e));
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
            Message::KeychainSign(KeychainSignMessage::Persisted { session_id, result }) => {
                match result {
                    Ok(()) => {
                        // Re-emit the message the panel expects so its
                        // existing post-save flow (saved flag, sigs
                        // recompute, keychain modal close) runs exactly
                        // as before this message carried session identity.
                        return Task::done(Message::Updated(Ok(())));
                    }
                    Err(e) => {
                        if let Some(entry) =
                            self.pending.iter_mut().find(|p| p.session_id == session_id)
                        {
                            entry.status = PendingSessionStatus::Failed;
                            entry.error = Some(format!("Failed to persist signed PSBT: {}", e));
                        }
                    }
                }
            }
            Message::KeychainSign(KeychainSignMessage::StreamEvent(event)) => {
                return self.on_session_event(event);
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
        if let Some(err) = &self.error {
            col = col.push(p1_regular(format!("Error: {}", err)));
        }
        for u in &self.unresolved {
            col = col.push(p1_regular(format!(
                "Cannot sign with {} — owner has no registered device",
                u,
            )));
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
