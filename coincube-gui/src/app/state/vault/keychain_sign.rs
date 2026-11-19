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
//! PSBTs and descriptors travel only in request-bound ECIES envelopes.
//! BTCB2 uses standard PSBT signature records on that wire and validated
//! internal records for local storage. Import accepts older internal-form
//! returns too, but rejects conflicting namespaces without a parse fallback.

use std::collections::HashMap;
use std::sync::Arc;

use coincube_core::miniscript::bitcoin::{bip32::Fingerprint, psbt::Psbt};
use iced::{Subscription, Task};
use tokio::sync::RwLock;

use coincube_ui::localized_format;
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
            build_keychain_index, classify_signers, resolved_keychain_replay_protection,
            KeychainSignerIndex, RequiredSigner,
        },
        view::{self, SpendTxMessage},
        wallet::Wallet,
    },
    daemon::{model::SpendTx, Daemon},
    services::{
        coincube::{
            vault_reconcile::reconcile_vault_members, CoincubeClient, ConnectVaultResponse,
            CubeKeyRaw, User,
        },
        connect::{
            client::auth::AccessTokenResponse,
            grpc::{
                connect_v1::{
                    CreateSigningSessionRequest, GetSigningSessionResponse, ResolveSignersResponse,
                    SessionStatus as ProtoSessionStatus, SignerTarget, SigningSession,
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

// The authenticated wallet chain selects the wire codec, never remote bytes.
fn signing_wire_psbt(chain: crate::chain::ChainId, psbt: Psbt) -> Result<Vec<u8>, String> {
    if chain.is_blake2b() {
        let internal =
            coincube_core::psbt_unified::UnifiedPsbt::from_psbt(psbt).map_err(|e| e.to_string())?;
        coincube_core::psbt_unified::export_standard(&internal).map_err(|e| e.to_string())
    } else {
        Ok(psbt.serialize())
    }
}

fn returned_wire_psbt(chain: crate::chain::ChainId, bytes: &[u8]) -> Result<Psbt, String> {
    if chain.is_blake2b() {
        // This strict importer accepts standard 0x21 and legacy internal records;
        // it rejects duplicate/conflicting namespaces. Never retry a failed parse.
        coincube_core::psbt_unified::import_standard(bytes)
            .map(|internal| internal.psbt().clone())
            .map_err(|e| e.to_string())
    } else {
        Psbt::deserialize(bytes).map_err(|e| e.to_string())
    }
}

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
    /// The target device's registered ECIES transport public key (33-byte
    /// compressed secp256k1), from `ResolveSigners`. **Empty means that
    /// keyholder's app predates end-to-end signing** — the session fails
    /// closed rather than falling back to a plaintext rail (master I5).
    pub transport_pubkey: Vec<u8>,
    /// Authenticated device capabilities from ResolveSigners.
    pub capabilities: Vec<String>,
    /// The client-generated `request_id` of this row's live session. Bound into
    /// the payload AAD in both directions, so it's needed again to open the
    /// signature envelope that comes back. Empty until a session is created.
    pub request_id: String,
    /// Display label — `name (owner_email)` or `name (you)`.
    pub label: String,
    /// Latest known session status. Driven by `SessionEvent`s on the
    /// realtime stream; `Pending` between session-create and the first
    /// delivery confirmation.
    pub status: PendingSessionStatus,
    /// Most recent error message, populated on rejected / expired /
    /// transport failure. Cleared on retry.
    pub error: Option<String>,
    /// Set by `cancel_all` for every non-terminal row the user cancels.
    /// For a row whose `CreateSigningSession` RPC is still in flight
    /// (empty `session_id`), `on_session_created` consults it so the
    /// just-created session is cancelled immediately rather than
    /// outliving the cancelled flow. For a row with a live session it
    /// also guards the poll/fetch path: a signature fetched in the window
    /// before the cancel RPC's reply lands is dropped instead of
    /// persisted, matching the flow's "discard partial signatures"
    /// contract. Cleared on retry.
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
    /// True once this session's signed PSBT has been merged into the local
    /// `SpendTx` in memory, until that merge is durably persisted
    /// (`signed_psbt_persisted`). While set-but-not-persisted the picker must
    /// not close on threshold: the daemon lacks the signature it would
    /// broadcast, and a persist failure needs the row to stay visible so it
    /// can be marked Failed and retried. Reset on retry.
    pub signed_psbt_merged: bool,
    /// True while this session's `update_spend_tx` persist callback is
    /// in flight — set when the persist is dispatched, cleared when it
    /// returns (Ok *or* Err). Distinct from `signed_psbt_merged`: a failed
    /// persist stays "merged but unsaved" (blocking threshold teardown) yet is
    /// no longer "in flight", so manual dismissal — which only waits for active
    /// persistence — can proceed instead of hanging on the failed row.
    pub signed_psbt_persisting: bool,
}

impl PendingSession {
    /// True once this session's signed PSBT has been merged into the local
    /// `SpendTx` (in memory) or durably persisted. The retry gates stop
    /// fetching a session only once its signature is captured — reaching
    /// `Completed` status alone is not enough, since the fetch+merge can lag
    /// (or miss) the `SESSION_COMPLETED` event.
    fn signature_captured(&self) -> bool {
        self.signed_psbt_merged || self.signed_psbt_persisted
    }
}

/// View-friendly mirror of the gRPC `SessionStatus` enum, plus a
/// pre-RPC `Creating` placeholder for the moment between user-action
/// and the create-session RPC completing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingSessionStatus {
    /// Resolved as a keychain signer but no session requested yet. The
    /// unified picker renders `Idle` rows as clickable "request signature"
    /// entries; a session is only created once the user clicks the row (or
    /// "Request from everyone"). Distinct from `Creating`, which means a
    /// `CreateSigningSession` RPC is already in flight.
    Idle,
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

    /// True only for the *give-up* terminal states — the session failed or was
    /// abandoned and its signature will never arrive. Distinct from
    /// `is_terminal()`, which also counts `Completed`: a `Completed` session
    /// whose signed PSBT hasn't been fetched+merged yet must keep being
    /// retried, so fetch/poll retry gates key off this, not `is_terminal()`.
    pub fn is_give_up(&self) -> bool {
        matches!(
            self,
            Self::Rejected | Self::Cancelled | Self::Expired | Self::Failed
        )
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

    /// True for a signer that has been resolved but not yet requested. The
    /// unified picker renders these as clickable rows rather than status text.
    pub fn is_idle(&self) -> bool {
        matches!(self, Self::Idle)
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Idle => "",
            Self::Creating => "Requesting…",
            Self::Pending => "Requested…",
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
    /// The shared Connect session tokens, so token refreshes flow
    /// through automatically. Used to construct `GrpcSessionClient`
    /// instances on demand (`make_session_client`).
    tokens: Arc<RwLock<AccessTokenResponse>>,
    grpc_url: String,
    desktop_device_id: String,
    /// This device's ECIES transport key (`PLAN-connect-blinding` PR D4).
    /// Signers seal their partial signatures to its public half; only this
    /// holds the private half. `None` if the sidecar couldn't be read — E2E
    /// signing is then unavailable and sessions fail closed rather than
    /// downgrading.
    transport_key: Option<Arc<crate::services::connect::crypto::DeviceTransportKey>>,
    /// Vault ID on the API — sourced from `ConnectVaultResponse.id`.
    /// Populated after the classification fetch returns.
    vault_id: Option<u64>,
    /// Cube server ID — used to call `GET /connect/cubes/{id}/vault`.
    /// Read from `cache.current_cube_server_id`.
    cube_server_id: u64,
    /// Cube UUID — used to call `GET /connect/cubes/{uuid}/keys`.
    /// Read from `cache.cube_id`.
    cube_uuid: String,
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

fn check_session_network(
    chain: crate::chain::ChainId,
    network: &str,
    session_exists: bool,
) -> Result<(), &'static str> {
    if network == chain.api_str() || (network.is_empty() && !chain.is_blake2b()) {
        return Ok(());
    }
    if network.is_empty() {
        return Err(if session_exists {
            "Connect needs updating before COINCUBE Mobile can sign on Bitcoin Blake2b. The request was cancelled."
        } else {
            "Connect needs updating before COINCUBE Mobile can sign on Bitcoin Blake2b. Nothing was sent to the signer."
        });
    }
    Err(if session_exists {
        "Connect reports this request on a different network than this Cube. The request was cancelled. Reopen the Cube; if this repeats, contact https://coincube.io/support."
    } else {
        "Connect reports this Vault on a different network than this Cube. Nothing was sent. Reopen the Cube; if this repeats, contact https://coincube.io/support."
    })
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
        psbt: Psbt,
        transport_key: Option<Arc<crate::services::connect::crypto::DeviceTransportKey>>,
    ) -> Self {
        Self {
            wallet,
            coincube_client,
            tokens,
            grpc_url,
            desktop_device_id,
            transport_key,
            vault_id: None,
            cube_server_id,
            cube_uuid,
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

    /// Start from the stream's current health. Events only report changes,
    /// so a modal opened after a rejection would otherwise read `Connected`.
    pub fn with_stream_health(mut self, status: crate::app::ConnectionStatus) -> Self {
        self.stream_health = status;
        self
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

    /// True once `ResolveSigners` has returned (or the flow finished): the
    /// `pending` list is now the authoritative set of keychain rows. Before
    /// this, the unified picker falls back to descriptor-derived placeholder
    /// rows so keychain signers are still visible while resolving.
    pub fn is_resolved(&self) -> bool {
        matches!(self.phase, Phase::Sessions | Phase::AllDone)
    }

    pub fn is_done(&self) -> bool {
        matches!(self.phase, Phase::AllDone)
    }

    /// Banner text describing a degraded realtime stream while keychain
    /// sessions are still in flight. `None` when the stream is healthy or no
    /// session is pending. Surfaced by the unified picker (the standalone
    /// keychain modal used to render this itself).
    pub fn stream_health_banner(&self) -> Option<String> {
        let has_pending_nonterminal = self.pending.iter().any(|p| !p.status.is_terminal());
        if !has_pending_nonterminal {
            return None;
        }
        // Built while the picker renders, so the templates follow the
        // window's language.
        match self.stream_health {
            crate::app::ConnectionStatus::Connecting
            | crate::app::ConnectionStatus::Reconnecting
            | crate::app::ConnectionStatus::Renewing => Some(coincube_ui::localized_format!(
                "Reconnecting to Connect. Requests already sent stay on Connect; signatures \
                 reach COINCUBE Desktop once live updates are back.",
                "Reconectando con Connect. Las solicitudes ya enviadas siguen en Connect; las \
                 firmas llegarán a COINCUBE Desktop cuando vuelvan las actualizaciones en vivo."
            )),
            crate::app::ConnectionStatus::Offline => Some(coincube_ui::localized_format!(
                "No internet connection. Requests already sent stay on Connect; signatures \
                 reach COINCUBE Desktop once you’re back online.",
                "Sin conexión a internet. Las solicitudes ya enviadas siguen en Connect; las \
                 firmas llegarán a COINCUBE Desktop cuando vuelvas a estar en línea."
            )),
            crate::app::ConnectionStatus::RenewFailed => Some(coincube_ui::localized_format!(
                "{}. Requests already sent stay on Connect; signatures reach COINCUBE Desktop once it \
                 reconnects. Try again from the Connect control in the top bar.",
                "{}. Las solicitudes ya enviadas siguen en Connect; las firmas llegarán a \
                 COINCUBE Desktop cuando se vuelva a conectar. Inténtalo de nuevo desde el control de \
                 Connect en la barra superior.",
                coincube_ui::locale::copy(crate::app::RENEW_FAILED_TITLE),
            )),
            crate::app::ConnectionStatus::Inactive | crate::app::ConnectionStatus::Connected => {
                None
            }
        }
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

    /// True while any pending session has its signed PSBT merged into the
    /// local `SpendTx` but not yet durably persisted — persist in flight *or*
    /// a persist that failed. Threshold-based modal teardown must wait for
    /// these: closing on the in-memory merge alone would drop the modal (and
    /// the daemon would lack a signature it needs to broadcast) before a
    /// persist failure could mark the row Failed for retry.
    pub fn has_persistence_pending(&self) -> bool {
        self.pending
            .iter()
            .any(|p| p.signed_psbt_merged && !p.signed_psbt_persisted)
    }

    /// True while any pending session has an async signed-PSBT capture still in
    /// flight — either a `GetSigningSession` fetch (`signed_psbt_fetching`) or an
    /// `update_spend_tx` persist (`signed_psbt_persisting`) that was dispatched
    /// and hasn't returned. Manual dismissal keeps the hidden modal mounted while
    /// this holds so the fetch can merge and the persist callback can land (and
    /// mark the row Failed on error) instead of being dropped. A `Completed`
    /// session whose fetch is still running looks status-drained, so keying
    /// dismissal off status alone would silently lose its signature. Unlike
    /// `has_persistence_pending`, an op that already *failed* is no longer in
    /// flight, so dismissal doesn't hang on it.
    pub fn has_capture_in_flight(&self) -> bool {
        self.pending
            .iter()
            .any(|p| p.signed_psbt_fetching || p.signed_psbt_persisting)
    }

    /// True while any pending session is non-terminal. After
    /// `cancel_all()`, such entries still depend on the modal staying
    /// mounted: empty-`session_id` rows are cancelled later by
    /// `on_session_created`, and direct-cancel rows only flip to
    /// `Cancelled` once their `SessionCancelled` reply lands. Dropping
    /// the modal before they drain orphans the sessions server-side.
    pub fn has_undrained_sessions(&self) -> bool {
        // Idle rows never started a session, so there is nothing to drain for
        // them — only rows with an actual in-flight/awaiting-cancel session
        // keep the dismissed modal mounted.
        self.pending
            .iter()
            .any(|p| !p.status.is_terminal() && !p.status.is_idle())
    }

    /// Mark the modal dismissed-but-mounted. The view hides immediately
    /// (see `view`), but the struct lives on to drive the deferred
    /// cancels until `close_if_dismissed_and_drained` tears it down.
    pub fn mark_dismissed(&mut self) {
        self.dismissed = true;
    }

    /// Whether [`Self::mark_dismissed`] ran: the modal is hidden and its
    /// sessions were cancelled, unlike a picker merely hidden while a local
    /// signer works.
    pub fn is_dismissed(&self) -> bool {
        self.dismissed
    }

    /// When the modal was dismissed mid-flight, close it once every
    /// pending session has reached a terminal state — reusing the
    /// existing `Phase::AllDone` + `Message::Updated(Ok)` close path
    /// that the panel already drives `self.modal = None` from.
    fn close_if_dismissed_and_drained(&mut self) -> Task<Message> {
        // A `Completed` row can still have an in-flight fetch or persist callback
        // (it is terminal-by-status but not yet captured) — don't tear down until
        // that async op returns, so the fetch can merge and a `Persisted(Err)`
        // can still mark the row Failed.
        let capturing = self.has_capture_in_flight();
        if self.dismissed
            && !capturing
            && !self.pending.is_empty()
            && self
                .pending
                .iter()
                .all(|p| p.status.is_terminal() || p.status.is_idle())
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
        let Some(descriptor) = self.wallet.inheritance_descriptor().cloned() else {
            return Task::done(Message::KeychainSign(KeychainSignMessage::Classified(Err(
                OpError::new("Imported wallets do not use Mobile signing."),
            ))));
        };
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
                // COIN-373: self-heal a vault left memberless (or partially
                // populated) by a Vault Builder sync whose `add_vault_member`
                // fan-out failed and was swallowed into a warning. Attach any
                // descriptor signer that is a registered cube key — owned by
                // this user or by a keyholder contact — but isn't yet a vault
                // member, then continue with the refreshed member list. Without
                // this, such a vault permanently reports "no Keychain signers
                // required" with no in-app recovery.
                let vault = reconcile_vault_members(
                    &client,
                    cube_server_id,
                    vault,
                    &cube_keys,
                    &descriptor,
                    self_user_id,
                )
                .await
                .vault;
                let index: KeychainSignerIndex =
                    build_keychain_index(&vault.members, &cube_keys, self_user_id);
                let required = classify_signers(
                    wallet.chain,
                    &psbt,
                    &descriptor,
                    &index,
                    &wallet.keys_aliases,
                    &super::signers::ReplayCapabilities::from_wallet(&wallet),
                )
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
        // Summarize the classified signers (fingerprint + Local/Keychain) so
        // a "no Keychain signers required" outcome is diagnosable: it tells us
        // whether the wrong spend path was chosen (e.g. primary instead of
        // recovery) or whether the right key was selected but didn't join to a
        // cube key and so fell back to Local.
        let required_summary = classified
            .required
            .iter()
            .map(|r| {
                format!(
                    "{}:{}",
                    r.fingerprint(),
                    if r.is_keychain() { "keychain" } else { "local" }
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        self.classified = Some(classified);
        if !has_keychain {
            tracing::info!(
                target: "coincube_gui::signing",
                vault_id = vault_id,
                phase = "classified",
                required = %required_summary,
                "No Mobile signers required for this transaction"
            );
            // No keychain rows to show. In the unified picker this isn't an
            // error — the user simply signs with the local signers listed
            // alongside — so we leave `error` unset (the standalone modal used
            // to surface a "use the Sign button" message here).
            self.phase = Phase::AllDone;
            return Task::none();
        }
        tracing::info!(
            target: "coincube_gui::signing",
            vault_id = vault_id,
            phase = "classified",
            keychain_signers = keychain_count,
            required = %required_summary,
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
        if let Err(message) = check_session_network(self.wallet.chain, &resp.network, false) {
            self.pending.clear();
            self.error = Some(message.to_string());
            self.phase = Phase::AllDone;
            return Task::none();
        }
        let classified = match self.classified.as_mut() {
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
            .map(|u| {
                let label = if u.reason == "signer_app_outdated" {
                    classified
                        .required
                        .iter()
                        .find_map(|r| match r {
                            RequiredSigner::Keychain {
                                fingerprint, name, ..
                            } if fingerprint.to_string() == u.key_fingerprint => {
                                Some(name.as_str())
                            }
                            _ => None,
                        })
                        .unwrap_or(&u.key_fingerprint)
                } else {
                    &u.key_fingerprint
                };
                format!("{} ({})", label, u.reason)
            })
            .collect();

        for target in &resp.targets {
            if let Some(RequiredSigner::Keychain {
                replay_protection, ..
            }) = classified.required.iter_mut().find(|required| {
                required.is_keychain()
                    && required.fingerprint().to_string() == target.key_fingerprint
            }) {
                *replay_protection =
                    resolved_keychain_replay_protection(self.wallet.chain, &target.capabilities);
            }
        }

        // For each resolved target, pair it with the matching
        // classified signer so we can populate the label / fingerprint
        // on the pending row. Targets carry `key_fingerprint` so the
        // join is straightforward.
        //
        // The rows start `Idle`: unlike the original fan-out flow we do NOT
        // create a `SigningSession` here. Sessions are created on demand when
        // the user clicks a row in the unified picker (`request_signer`) or
        // presses "Request from everyone" (`request_from_everyone`).
        //
        // Owned clone of the classification rows so the per-target
        // dispatch below can borrow without lifetime gymnastics.
        let by_fp: HashMap<String, RequiredSigner> = classified
            .required
            .iter()
            .filter(|r| r.is_keychain())
            .cloned()
            .map(|r| (r.fingerprint().to_string(), r))
            .collect();

        self.append_resolved_signer_rows(&resp.targets, &by_fp);

        Task::none()
    }

    fn append_resolved_signer_rows(
        &mut self,
        targets: &[SignerTarget],
        by_fp: &HashMap<String, RequiredSigner>,
    ) {
        for target in targets {
            let Some(matched) = by_fp.get(&target.key_fingerprint) else {
                tracing::warn!(
                    "ResolveSigners returned target for fingerprint {} that wasn't classified \
                     as Mobile — skipping",
                    target.key_fingerprint,
                );
                continue;
            };
            let (fingerprint, key_id, label, replay_protection) = match matched {
                RequiredSigner::Keychain {
                    fingerprint,
                    key_id,
                    name,
                    owner_email,
                    replay_protection,
                    ..
                } => {
                    let suffix = owner_email
                        .as_deref()
                        .map(|e| format!(" ({})", e))
                        .unwrap_or_else(|| " (you)".to_string());
                    (
                        *fingerprint,
                        *key_id,
                        format!("{}{}", name, suffix),
                        *replay_protection,
                    )
                }
                _ => unreachable!(),
            };
            let outdated = self.wallet.chain.is_blake2b() && !replay_protection.is_capable();
            let error = outdated.then(|| {
                format!(
                    "{}'s COINCUBE Mobile needs updating before it can sign on Bitcoin Blake2b.",
                    label
                )
            });
            self.pending.push(PendingSession {
                session_id: String::new(), // populated by SessionCreated
                key_id,
                fingerprint,
                device_id: target.device_id.clone(),
                transport_pubkey: target.transport_pubkey.clone(),
                capabilities: target.capabilities.clone(),
                request_id: String::new(),
                label,
                status: if outdated {
                    PendingSessionStatus::Failed
                } else {
                    PendingSessionStatus::Idle
                },
                error,
                cancel_requested: false,
                signed_psbt_persisted: false,
                signed_psbt_fetching: false,
                signed_psbt_merged: false,
                signed_psbt_persisting: false,
            });
        }
    }

    /// Build the `CreateSigningSession` task for one already-populated pending
    /// row, resetting its state to `Creating`. Shared by the on-demand request
    /// paths (`request_signer`, `request_from_everyone`) and the failed-session
    /// `retry_signer`. The caller is responsible for having gated on the row's
    /// current status; this unconditionally (re)starts a session for it.
    fn create_session_for(&mut self, index: usize) -> Task<Message> {
        let Some(entry) = self.pending.get_mut(index) else {
            return Task::none();
        };
        let Some(descriptor) = self.wallet.inheritance_descriptor() else {
            entry.status = PendingSessionStatus::Failed;
            entry.error = Some("Imported wallets do not use Mobile signing.".into());
            return Task::none();
        };
        if self.wallet.chain.is_blake2b()
            && !resolved_keychain_replay_protection(self.wallet.chain, &entry.capabilities)
                .is_capable()
        {
            entry.status = PendingSessionStatus::Failed;
            entry.error = Some(format!(
                "{}'s COINCUBE Mobile needs updating before it can sign on Bitcoin Blake2b.",
                entry.label
            ));
            return Task::none();
        }
        let fingerprint = entry.fingerprint;
        let device_id = entry.device_id.clone();
        let key_id = entry.key_id;
        let transport_pubkey = entry.transport_pubkey.clone();

        // ── End-to-end preconditions (PLAN-connect-blinding PR D4) ────────
        //
        // Every payload on this rail is an envelope. Two ways that can be
        // impossible, and both fail the row closed — never downgrade a session
        // to the plaintext rail (master I5). Checked before any PSBT work so
        // the user gets the actionable message rather than a preparation error.
        //
        //   * the signer registered no transport key (their app predates E2E);
        //   * this desktop has no transport key of its own, so the signature
        //     couldn't be sealed back to us anyway.
        if self.transport_key.is_none() {
            return Self::fail_session_creation(&mut self.pending, index, |_| {
                "This device can't set up encrypted signing yet. Restart COINCUBE Desktop and try \
                 again."
                    .to_string()
            });
        }
        if transport_pubkey.is_empty() {
            return Self::fail_session_creation(&mut self.pending, index, |entry| {
                format!(
                    "{} needs to update COINCUBE Mobile before they can sign — their app \
                     doesn't support encrypted signing requests yet.",
                    entry.label
                )
            });
        }
        // Reset state to creating; clear any previous error and stale
        // cancel intent (an explicit request overrides a prior cancel-all
        // so the new session isn't auto-cancelled). A retried/re-requested
        // session produces a fresh signature that must be fetched + persisted
        // again before this row counts as done.
        entry.status = PendingSessionStatus::Creating;
        entry.session_id.clear();
        entry.error = None;
        entry.cancel_requested = false;
        entry.signed_psbt_persisted = false;
        entry.signed_psbt_fetching = false;
        entry.signed_psbt_merged = false;
        entry.signed_psbt_persisting = false;

        // Prune the PSBT's BIP32 derivations to the active spending path before
        // sending it to the remote signer. The Keychain signs a single key at a
        // time; if the PSBT advertises this signer's fingerprint on more than
        // one spending path (e.g. both the primary `multi(...)` key at `/0;1/*`
        // and the recovery `pkh(...)` key at `/2;3/*`), the phone can sign the
        // wrong-path key, which then cannot satisfy the path this transaction
        // actually spends. Pruning leaves only the keys for the current path.
        // We prune a *clone* and merge the returned signatures back into the
        // full `tx.psbt`, so the stored PSBT keeps all its derivations — this
        // mirrors the hardware-wallet (BitBox02) handling in `sign_psbt`.
        let psbt_to_sign = match descriptor.prune_bip32_derivs_last_avail(self.psbt.clone()) {
            Ok(psbt) => psbt,
            Err(e) => {
                // Never fall back to sending the unpruned PSBT: the remote
                // signer could then sign a key from the wrong spending path,
                // producing a signature that can't satisfy this spend. Abort the
                // session (mark it Failed for retry) instead.
                tracing::warn!(
                    target: "coincube_gui::signing",
                    error = %e,
                    "Could not prune PSBT to the spending path; aborting signing session"
                );
                return Self::fail_session_creation(&mut self.pending, index, |_| {
                    format!("Could not prepare PSBT for signing: {e}")
                });
            }
        };
        let psbt_bytes = match signing_wire_psbt(self.wallet.chain, psbt_to_sign) {
            Ok(bytes) => bytes,
            Err(e) => {
                return Self::fail_session_creation(&mut self.pending, index, |_| {
                    format!("Could not prepare PSBT for signing: {e}")
                });
            }
        };

        // Seal the PSBT and descriptor to this signer's transport key so Connect
        // relays ciphertext it cannot read. The `request_id` minted here binds
        // both directions' envelopes to this session (see `transport.rs`), so
        // it is recorded on the row for the signature-open later.
        let request_id = uuid_v4();
        let (psbt_envelope, descriptor_envelope) = match seal_signing_payloads(
            &transport_pubkey,
            &request_id,
            &psbt_bytes,
            &descriptor.to_string(),
        ) {
            Ok(sealed) => sealed,
            Err(e) => {
                tracing::warn!(
                    target: "coincube_gui::signing",
                    error = %e,
                    "Could not encrypt the signing payloads to the signer's transport key"
                );
                return Self::fail_session_creation(&mut self.pending, index, |entry| {
                    format!(
                        "Couldn't encrypt the request for {}: their registered signing key looks \
                         invalid. Ask them to reopen COINCUBE Mobile.",
                        entry.label
                    )
                });
            }
        };
        if let Some(entry) = self.pending.get_mut(index) {
            entry.request_id = request_id.clone();
        }
        let envelope_device_id = device_id.clone();
        let network = self.wallet.chain.api_str().to_string();
        let capabilities = self.pending[index].capabilities.clone();

        let vault_id = self.vault_id.unwrap_or(0).to_string();
        let descriptor_id = crate::app::wallet::descriptor_id_fingerprint(descriptor).to_string();
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
                    network,
                    request_id,
                    vault_id,
                    descriptor_id,
                    // Empty under ECIES_V1 — the real PSBT rides
                    // `psbt_envelopes`, sealed per target.
                    psbt: Vec::new(),
                    payload_scheme:
                        crate::services::connect::grpc::connect_v1::PayloadScheme::EciesV1 as i32,
                    psbt_envelopes: vec![
                        crate::services::connect::grpc::connect_v1::PayloadEnvelope {
                            device_id: envelope_device_id.clone(),
                            ephemeral_pubkey: psbt_envelope.ephemeral_pubkey,
                            nonce: psbt_envelope.nonce,
                            ciphertext: psbt_envelope.ciphertext,
                        },
                    ],
                    descriptor_envelopes: vec![
                        crate::services::connect::grpc::connect_v1::PayloadEnvelope {
                            device_id: envelope_device_id,
                            ephemeral_pubkey: descriptor_envelope.ephemeral_pubkey,
                            nonce: descriptor_envelope.nonce,
                            ciphertext: descriptor_envelope.ciphertext,
                        },
                    ],
                    targets: vec![crate::services::connect::grpc::connect_v1::SignerTarget {
                        device_id,
                        key_fingerprint: fingerprint_as_str(&fingerprint),
                        key_id: key_id.to_string(),
                        // Echoed back so the server can pair target ↔ envelope
                        // without re-reading the device row.
                        transport_pubkey,
                        capabilities,
                    }],
                    note: String::new(),
                    ttl: Some(prost_types::Duration {
                        seconds: 24 * 60 * 60,
                        nanos: 0,
                    }),
                    require_user_presence: false,
                    // Owner-branch signing; the heir recovery sweep sets this true.
                    is_recovery_spend: false,
                };
                client
                    .create_signing_session(req)
                    .await
                    .map_err(OpError::from_status)
            },
            move |r| Message::KeychainSign(KeychainSignMessage::SessionCreated(fingerprint, r)),
        )
    }

    fn fail_session_creation(
        pending: &mut [PendingSession],
        index: usize,
        error: impl FnOnce(&PendingSession) -> String,
    ) -> Task<Message> {
        if let Some(entry) = pending.get_mut(index) {
            entry.status = PendingSessionStatus::Failed;
            entry.error = Some(error(entry));
        }
        Task::none()
    }

    /// Request a signature from one specific idle keychain signer (the user
    /// clicked its row in the unified picker). No-op unless the row is `Idle`.
    pub fn request_signer(&mut self, fingerprint: Fingerprint) -> Task<Message> {
        let Some(index) = self
            .pending
            .iter()
            .position(|p| p.fingerprint == fingerprint && p.status.is_idle())
        else {
            return Task::none();
        };
        self.create_session_for(index)
    }

    /// Request a signature from every idle keychain signer at once — the
    /// explicit "Request from everyone" affordance. Preserves the original
    /// fan-out behaviour but triggered by the user rather than on launch.
    pub fn request_from_everyone(&mut self) -> Task<Message> {
        let idle: Vec<usize> = self
            .pending
            .iter()
            .enumerate()
            .filter(|(_, p)| p.status.is_idle())
            .map(|(i, _)| i)
            .collect();
        let tasks: Vec<Task<Message>> = idle
            .into_iter()
            .map(|i| self.create_session_for(i))
            .collect();
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
                self.on_session_creation_auth_failure(fingerprint, e);
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

                    // The signer seals its signature to whichever key the server
                    // reports as ours (`SigningSession.creator_transport_pubkey`,
                    // added for exactly this — a signer has no other way to learn
                    // it). If that is not the key we hold, the signature comes
                    // back unopenable, and the only symptom is a baffling
                    // "couldn't decrypt the returned signature" *after* the
                    // keyholder has already approved on their phone.
                    //
                    // Reachable without any malice: `DeviceTransportKey::
                    // load_or_create` silently mints a replacement when the
                    // sidecar is unreadable, while device registration
                    // short-circuits once a device_id exists — so the server can
                    // legitimately still hold the old pubkey. Catch it here,
                    // before anyone is asked to approve anything.
                    let mismatched = self
                        .transport_key
                        .as_ref()
                        .map(|k| k.public_key())
                        .is_some_and(|ours| {
                            !session.creator_transport_pubkey.is_empty()
                                && session.creator_transport_pubkey != ours
                        });
                    if let Err(message) =
                        check_session_network(self.wallet.chain, &session.network, true)
                    {
                        entry.status = PendingSessionStatus::Failed;
                        entry.error = Some(message.to_string());
                        entry.cancel_requested = true;
                        Some((session_id, "network_mismatch"))
                    } else if mismatched {
                        tracing::error!(
                            target: "coincube_gui::signing",
                            session_id = %session_id,
                            "Connect reports a different transport key for this device than the \
                             one held locally; the returned signature would be unopenable"
                        );
                        entry.status = PendingSessionStatus::Failed;
                        entry.error = Some(
                            "This computer's encrypted-signing key doesn't match the one Connect \
                             has on file. Sign out of Connect and sign back in to re-register it, \
                             then try again."
                                .to_string(),
                        );
                        // Cancel it rather than leaving a session pending for a
                        // keyholder whose approval could not be used anyway.
                        Some((session_id, "creator_transport_key_mismatch"))
                    } else {
                        entry.status = PendingSessionStatus::from_proto(session_status_from_i32(
                            session.status,
                        ));
                        entry.error = None;
                        tracing::info!(
                            target: "coincube_gui::signing",
                            vault_id = self.vault_id.unwrap_or(0),
                            session_id = %session_id,
                            fingerprint = %fingerprint,
                            "Signing session created"
                        );
                        if entry.cancel_requested && !entry.session_id.is_empty() {
                            Some((entry.session_id.clone(), "user_cancelled"))
                        } else {
                            None
                        }
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
        // The reason rides into the server's event log, so it names which of
        // the two cancel paths fired — "the user hit Cancel all" and "we created
        // a session nobody could complete" are very different things to read
        // back six months later.
        let Some((sid, reason)) = cancel_sid else {
            return Task::none();
        };
        self.cancel_session_task(sid, reason)
    }

    fn on_session_creation_auth_failure(&mut self, fingerprint: Fingerprint, e: &OpError) {
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
    }

    fn cancel_session_task(&self, sid: String, reason: &'static str) -> Task<Message> {
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
                    .cancel_signing_session(rpc_sid, reason.to_string())
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
                if !entry.signed_psbt_fetching
                    && !entry.signed_psbt_merged
                    && !entry.signed_psbt_persisted
                {
                    entry.signed_psbt_fetching = true;
                    fetch_session_id = Some(event.session_id.clone());
                }
            }
            EventType::SessionCompleted => {
                entry.status = PendingSessionStatus::Completed;
                if !entry.signed_psbt_fetching
                    && !entry.signed_psbt_merged
                    && !entry.signed_psbt_persisted
                {
                    entry.signed_psbt_fetching = true;
                    fetch_session_id = Some(event.session_id.clone());
                }
                // If the fetch is skipped here (already fetching/merged/
                // persisted) and the in-flight fetch never lands the signature,
                // the poll fallback re-fetches this `Completed` row until it is
                // captured.
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
        }
        // Any status change reconciles the panel: the recompute/close
        // authority lives in `PsbtState`, so emit a UI-only `Reconcile` on
        // every event — even when this one started no fetch (e.g. it raced a
        // poll that already holds the fetch) and not all sessions are done yet.
        // Without this a merged signature can sit uncounted (badge stuck below
        // threshold, modal never closing) until some unrelated message happens
        // to trigger a reconcile. `Reconcile` (not `Updated(Ok)`) because no
        // save occurred here — persist success is signalled by `Persisted`.
        Task::done(Message::Reconcile)
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
        // If the user cancelled this session, drop whatever a poll/fetch
        // returned — even a submitted signature. The cancel-all flow
        // discards partial signatures, so a result that raced in before
        // the cancel RPC's reply must not be merged/persisted. Clear the
        // in-flight flag so nothing else waits on this fetch.
        if let Some(entry) = self.pending.iter_mut().find(|p| p.session_id == session_id) {
            if entry.cancel_requested || matches!(entry.status, PendingSessionStatus::Cancelled) {
                entry.signed_psbt_fetching = false;
                return Task::none();
            }
        }
        let resp = match result {
            Ok(r) => r,
            Err(e) => {
                self.on_session_fetch_error(&session_id, e, is_poll);
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
        if let Err(message) = check_session_network(self.wallet.chain, &session.network, true) {
            if let Some(entry) = self.pending.iter_mut().find(|p| p.session_id == session_id) {
                entry.signed_psbt_fetching = false;
                entry.cancel_requested = true;
                entry.status = PendingSessionStatus::Failed;
                entry.error = Some(message.to_string());
            }
            return self.cancel_session_task(session_id, "network_mismatch");
        }
        if !self.record_fetched_session_status(&session_id, &session, is_poll) {
            return Task::none();
        }
        // Decode the submitted signed PSBT(s) and merge into the local SpendTx via
        // the daemon's update path. The existing SignModal handler uses
        // the same `merge_signatures` + `update_spend_tx` shape; we
        // replicate it inline rather than refactor for one call site.
        // Under ECIES_V1 the signature comes back sealed to this desktop's
        // transport key (PR D4); the plaintext `signed_psbt` field is empty.
        // Open it here, then merge exactly as before — the merge, the daemon
        // update, and everything downstream are unchanged by blinding.
        let request_id = self
            .pending
            .iter()
            .find(|p| p.session_id == session_id)
            .map(|p| p.request_id.clone())
            .unwrap_or_default();
        let transport_key = self.transport_key.clone();
        let open_signature = |sig: &crate::services::connect::grpc::connect_v1::SubmittedSignature| -> Result<Vec<u8>, String> {
            let Some(env) = sig.signature_envelope.as_ref() else {
                // No plaintext fallback. This branch used to return
                // `sig.signed_psbt` verbatim, which meant a session this
                // desktop created as ECIES_V1 could still complete in
                // plaintext if the peer or server returned no envelope —
                // decided by envelope *shape*, never by `payload_scheme`. That
                // is a silent downgrade, and `merge_signatures` below performs
                // no validation and overwrites `tap_key_sig`, so the merged
                // result would be trusted unconditionally.
                //
                // Connect now refuses to advance a plaintext session at all
                // (`SubmitPartialSignature` returns `FailedPrecondition`), so
                // an envelope-less signature on a live session no longer has a
                // legitimate origin. Fail closed.
                return Err(
                    "This phone returned an unencrypted signature for an encrypted \
                     request. Update COINCUBE Mobile, then start the signature again."
                        .to_string(),
                );
            };
            let key = transport_key
                .as_ref()
                .ok_or_else(|| "no transport key on this device".to_string())?;
            key.open(
                &env.ephemeral_pubkey,
                &env.nonce,
                &env.ciphertext,
                &request_id,
            )
            .map(|pt| pt.to_vec())
            .map_err(|e| e.to_string())
        };

        // Every returned blob is merged into a clone of the local PSBT — the
        // one this desktop already holds and authenticated — never into a
        // remote blob, and never into `tx.psbt` mid-loop. On Bitcoin Blake2b
        // the merge verifies each merged candidate against the prevouts and
        // witness scripts the local copy carries, so a sparse return (a
        // signer answering with signatures only) verifies against local
        // metadata, and a refusal on any blob — wrong transaction, conflict,
        // a signature that does not verify — leaves `tx.psbt` untouched with
        // nothing scheduled for persistence. On the Bitcoin family this is
        // the historical copy applied in submission order, which resolves
        // to the same result as the former blob-seeded accumulator (last
        // write wins on a key either way).
        let mut signed_psbt = tx.psbt.clone();
        for sig in session.submitted_signatures {
            if !self.merge_fetched_signature(&session_id, &mut signed_psbt, open_signature(&sig)) {
                return Task::none();
            }
        }
        tracing::info!(
            target: "coincube_gui::signing",
            vault_id = self.vault_id.unwrap_or(0),
            session_id = %session_id,
            "Merging signed PSBT from session into local SpendTx"
        );
        tx.psbt = signed_psbt;
        // Mark the row merged-but-not-yet-persisted (blocks threshold close
        // until saved or retried) and persist-in-flight (keeps a dismissed
        // modal mounted until the callback below returns).
        if let Some(entry) = self.pending.iter_mut().find(|p| p.session_id == session_id) {
            entry.signed_psbt_merged = true;
            entry.signed_psbt_persisting = true;
        }
        // Persist the merged PSBT so a restart picks it up. Carry the
        // session_id through so a persistence failure marks the right
        // row instead of being silently swallowed by the panel's
        // generic `Message::Updated(Err)` path.
        let merged = tx.psbt.clone();
        let daemon = daemon.clone();
        let persist = Task::perform(
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
        );
        // The signature is already merged into `tx.psbt`; reconcile the panel's
        // count/badge/close decision now rather than waiting on the persist
        // round-trip (`Persisted { Ok }` re-emits `Updated(Ok)` on real save
        // success). `Reconcile` is UI-only — it must not mark the tx saved,
        // since this persist may still fail — but it must not strand a merged
        // signature uncounted either.
        Task::batch([persist, Task::done(Message::Reconcile)])
    }

    fn on_session_fetch_error(&mut self, session_id: &str, e: OpError, is_poll: bool) {
        if let Some(entry) = self.pending.iter_mut().find(|p| p.session_id == session_id) {
            // Always clear the in-flight flag — otherwise the poll/
            // event fetch guards keep skipping this row and it can
            // never be retried. (Auth errors are no exception: a stale
            // token clears on the next attempt via the interceptor, and
            // one row's failure must not force-close the whole modal.)
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
    }

    fn record_fetched_session_status(
        &mut self,
        session_id: &str,
        session: &SigningSession,
        is_poll: bool,
    ) -> bool {
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
            return false;
        }
        true
    }

    fn merge_fetched_signature(
        &mut self,
        session_id: &str,
        signed_psbt: &mut Psbt,
        opened_signature: Result<Vec<u8>, String>,
    ) -> bool {
        let bytes = match opened_signature {
            Ok(b) => b,
            Err(e) => {
                if let Some(entry) = self.pending.iter_mut().find(|p| p.session_id == session_id) {
                    entry.status = PendingSessionStatus::Failed;
                    entry.error = Some(format!("Couldn't decrypt the returned signature: {}", e));
                }
                return false;
            }
        };
        match returned_wire_psbt(self.wallet.chain, &bytes) {
            Ok(psbt) => {
                if let Err(e) =
                    super::psbt::merge_signatures_pub(self.wallet.chain, signed_psbt, &psbt)
                {
                    // Nothing merged, nothing persisted: the row fails
                    // with the adapter's reason instead of the daemon
                    // refusing later.
                    if let Some(entry) =
                        self.pending.iter_mut().find(|p| p.session_id == session_id)
                    {
                        entry.status = PendingSessionStatus::Failed;
                        entry.error = Some(
                            super::replay::unsafe_legacy_policy_copy(&e)
                                .map(str::to_owned)
                                .unwrap_or_else(|| {
                                    format!("Couldn't merge the returned signatures: {}", e)
                                }),
                        );
                    }
                    return false;
                }
            }
            Err(e) => {
                if let Some(entry) = self.pending.iter_mut().find(|p| p.session_id == session_id) {
                    entry.status = PendingSessionStatus::Failed;
                    entry.error = Some(format!("Malformed signed PSBT from API: {}", e));
                }
                return false;
            }
        }
        true
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
            // Skip sessions that gave up (rejected/cancelled/expired/failed),
            // were cancelled by the user (cancel RPC may still be in flight, so
            // status isn't `Cancelled` yet), not yet created, already being
            // fetched (event path or a previous tick), already merged (persist
            // may still be in flight), or already captured. NOTE: `Completed`
            // is deliberately NOT a skip — a `Completed` session whose signed
            // PSBT hasn't been fetched+merged yet must keep being polled until
            // its signature actually lands (`is_give_up`, not `is_terminal`).
            if entry.status.is_give_up()
                || entry.cancel_requested
                || entry.session_id.is_empty()
                || entry.signed_psbt_fetching
                || entry.signature_captured()
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
            "User cancelled Mobile signing flow"
        );
        let tokens = self.tokens.clone();
        let grpc_url = self.grpc_url.clone();
        let desktop_device_id = self.desktop_device_id.clone();
        let mut tasks = Vec::new();
        for entry in self.pending.iter_mut() {
            if entry.status.is_terminal() {
                continue;
            }
            // Flag the cancellation before spawning the cancel RPC so a
            // poll already in flight (or a tick that fires before the
            // `SessionCancelled` reply lands) drops its fetched signature
            // instead of persisting work the user just cancelled.
            entry.cancel_requested = true;
            if entry.session_id.is_empty() {
                // CreateSigningSession is still in flight — there's no
                // session_id to cancel yet. The `cancel_requested` flag set
                // above lets `on_session_created` cancel it the moment it
                // lands, instead of letting it outlive this cancel-all.
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
        let Some(entry) = self.pending.get(index) else {
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
        self.create_session_for(index)
    }
}

impl KeychainSignModal {
    fn on_signature_persisted(
        &mut self,
        session_id: String,
        result: Result<(), String>,
    ) -> Task<Message> {
        match result {
            Ok(()) => {
                // The signed PSBT for this session is now merged
                // and durably saved — mark the row so
                // `check_all_done` can count it. This (not the
                // API `Completed` event) is the authoritative
                // "this signature is captured" signal.
                if let Some(entry) = self.pending.iter_mut().find(|p| p.session_id == session_id) {
                    entry.signed_psbt_persisted = true;
                    entry.signed_psbt_persisting = false;
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
                if let Some(entry) = self.pending.iter_mut().find(|p| p.session_id == session_id) {
                    entry.status = PendingSessionStatus::Failed;
                    entry.signed_psbt_fetching = false;
                    // Persist callback returned — no longer in flight.
                    // `signed_psbt_merged` stays set so threshold close
                    // remains blocked (row is Failed, awaiting retry),
                    // but dismissal need not wait on it any longer.
                    entry.signed_psbt_persisting = false;
                    entry.error = Some(format!("Failed to persist signed PSBT: {}", e));
                }
            }
        }
        Task::none()
    }

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
                return self.on_signature_persisted(session_id, result);
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
            Message::View(view::Message::Spend(SpendTxMessage::SelectKeychainSigner(fp))) => {
                return self.request_signer(fp);
            }
            Message::View(view::Message::Spend(SpendTxMessage::RequestFromEveryone)) => {
                return self.request_from_everyone();
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
        // vault-scope mismatch, …), or that announce `SESSION_COMPLETED` before
        // the signed PSBT was actually fetched+merged. Active while any session
        // hasn't given up (rejected/cancelled/expired/failed) and its signature
        // isn't captured yet — `Completed` alone does NOT stop it, otherwise a
        // completed-but-unfetched signature would be stranded with no retry.
        // Once captured we wait on the local persist, not the remote, so a
        // merged row stops polling. It self-stops once everyone has signed.
        let needs_poll = matches!(self.phase, Phase::Sessions)
            && self.pending.iter().any(|p| {
                !p.status.is_give_up() && !p.signature_captured() && !p.session_id.is_empty()
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

        col = col.push(p1_bold("Sign via Mobile"));

        // Stream-health banner: surfaced only while sessions are
        // pending and the realtime stream is unhealthy. We don't
        // pre-cancel anything on disconnect because sessions live
        // server-side; reconnecting catches up via `last_seen_seq`.
        if let Some(banner) = self.stream_health_banner() {
            col = col.push(p1_regular(banner));
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

        // Unresolved (resolved-but-unaddressable) signers. Friendlier
        // copy than the raw API reason string; the mapping lives in
        // `friendly_unresolved_signer` so it is testable against the
        // exact wire tokens.
        for u in &self.unresolved {
            col = col.push(p1_regular(friendly_unresolved_signer(u)));
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
                    col = append_pending_signer_row(col, i, p);
                }
            }
            Phase::AllDone => {
                col = col.push(p1_regular("All Mobile signers have completed. Closing…"));
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

fn append_pending_signer_row<'a>(
    mut col: Column<'a, view::Message>,
    i: usize,
    p: &PendingSession,
) -> Column<'a, view::Message> {
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
            button::secondary(Some(icon::reload_icon()), "Retry")
                .on_press(view::Message::Spend(SpendTxMessage::RetryKeychainSigner(i))),
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
    col
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
    if matches!(
        status.code(),
        tonic::Code::InvalidArgument | tonic::Code::FailedPrecondition
    ) {
        let message = match status.message().split(':').next().unwrap_or_default() {
            "NETWORK_INVALID" if status.code() == tonic::Code::InvalidArgument => Some("Connect doesn't recognise this Vault's network. Update COINCUBE."),
            "NETWORK_INVALID" => Some("This Vault's Connect record has an unrecognised network. Contact support at https://coincube.io/support."),
            "NETWORK_MISMATCH" => Some("This Vault's network doesn't match its Connect Cube. Reopen the Cube; if this repeats, contact support at https://coincube.io/support."),
            "CHAIN_IDENTITY_REQUIRED" => Some("Update COINCUBE to keep signing with COINCUBE Mobile."),
            "TARGET_KEY_NOT_ON_VAULT" => Some("One of the keys selected isn't part of this Vault. Refresh the signers and try again."),
            "SIGNER_APP_OUTDATED" => Some("This signer's COINCUBE Mobile needs updating before it can sign on Bitcoin Blake2b."),
            "NETWORK_DISABLED" => Some("Bitcoin Blake2b isn't enabled for this account."),
            _ => None,
        };
        if let Some(message) = message {
            return (message.to_string(), false);
        }
    }
    if status.code() == tonic::Code::NotFound && status.message() == "vault not found" {
        return (
            "This Vault isn't registered to your account.".to_string(),
            false,
        );
    }
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
        // The server refuses to create an end-to-end session when the CREATING
        // device — this desktop — has no registered transport pubkey, because
        // signers would have nothing to seal the signature back to
        // (`PLAN-connect-blinding` 4a). The raw message is accurate but reads
        // as an instruction to a developer, and the remedy is a thing the user
        // has to do, so name it: re-registration happens on sign-in.
        //
        // Reachable two ways: this desktop registered before transport keys
        // existed, or its device row was revoked. Both are fixed the same way.
        tonic::Code::FailedPrecondition => (
            "This computer isn't set up for encrypted signing yet. Sign out of \
             Connect and sign back in to re-register it, then try again."
                .to_string(),
            false,
        ),
        _ => (status.message().to_string(), false),
    }
}

/// The reason token in an unresolved-signer entry as stored by
/// `on_signers_resolved` (`"<fingerprint> (<reason>)"`), or `None` when the
/// entry is not in that shape.
fn unresolved_signer_reason(entry: &str) -> Option<&str> {
    let (_, rest) = entry.rsplit_once(" (")?;
    rest.strip_suffix(')')
}

/// User-facing copy for one unresolved signer — the **only** place that
/// turns an unresolved entry into words. Every surface that shows
/// `unresolved()` (this modal's view, the unified picker's
/// `SignModal::keychain_notices`) calls it; a second hand-written
/// diagnosis is how the false "no registered device" claim survived on
/// the picker after it was fixed here.
///
/// The arms match the **exact** `UnresolvedSigner.reason` tokens Connect emits —
/// the Go constants in `coincube-api`
/// `services/connect/signing/grpc/session_server.go` are the source of truth,
/// not the comment on the proto field (which has been wrong in both directions:
/// it listed `owner_unknown`, which no server path emits, and omitted
/// `transport_key_stale`, which the server has emitted all along). A substring
/// or paraphrase match here would pass its test and never fire in production.
///
/// The fallthrough carries **no diagnosis**: an unfamiliar reason reaches the
/// user verbatim, and nothing else is claimed about it. Before this fix the
/// fallthrough appended "owner has no registered device" to every unrecognised
/// token — for `transport_key_stale` that told a signer who owns a registered
/// device that they had none, while the real fix was one screen away.
pub(crate) fn friendly_unresolved_signer(entry: &str) -> String {
    match unresolved_signer_reason(entry) {
        Some("signer_app_outdated") => localized_format!(
            "{}'s COINCUBE Mobile needs updating before it can sign on Bitcoin Blake2b.",
            "El COINCUBE Mobile de {} necesita actualizarse antes de poder firmar en Bitcoin Blake2b.",
            entry.trim_end_matches(" (signer_app_outdated)"),
        ),
        Some("no_device_registered") => localized_format!(
            "{} hasn't set up COINCUBE Mobile yet. Ask them to install it \
             and sign in, then retry.",
            "{} aún no configuró COINCUBE Mobile. Pídele que la instale e \
             inicie sesión, y vuelve a intentarlo.",
            entry,
        ),
        Some("all_devices_revoked") => localized_format!(
            "{} has revoked every device on their account. They need to \
             register a new device before this transaction can be signed.",
            "{} revocó todos los dispositivos de su cuenta. Necesita registrar \
             un dispositivo nuevo antes de poder firmar esta transacción.",
            entry,
        ),
        // The target device's transport key is past its confirmation TTL.
        // Opening Keychain re-registers the device; nothing about the
        // account's registration state is wrong, so say nothing about it.
        Some("transport_key_stale") => localized_format!(
            "{} — this signer's COINCUBE Mobile hasn't checked in with Connect \
             recently. Ask them to open COINCUBE Mobile on their phone, then retry.",
            "{} — el COINCUBE Mobile de este firmante no se ha comunicado con Connect \
             recientemente. Pídele que abra COINCUBE Mobile en su teléfono y vuelve a intentarlo.",
            entry,
        ),
        // The key is bound to a device Connect can no longer target (revoked,
        // deleted, or owned by another account). Connect does not guess a
        // replacement; Keychain re-claims the key on launch on the handset
        // that holds it (keychain-app#145).
        Some("enrolled_device_unavailable") => localized_format!(
            "{} is bound to a phone that can no longer be reached. Ask them \
             to open COINCUBE Mobile on the phone that holds this key, then retry.",
            "{} está vinculado a un teléfono al que ya no se puede llegar. Pídele \
             que abra COINCUBE Mobile en el teléfono que tiene esta clave y vuelve a intentarlo.",
            entry,
        ),
        // Kept for compatibility, but no server code path emits this token
        // today; the Go constants above are the source of truth. Do not read
        // "three matching arms" as "the list is complete" — that is how the
        // stale-transport-key case was missed.
        Some("owner_unknown") => localized_format!(
            "{} — this signer's owner isn't known to the backend. \
             Contact support if this persists.",
            "{} — el servidor no conoce al propietario de este firmante. \
             Contacta con soporte si esto continúa.",
            entry,
        ),
        _ => localized_format!(
            "Cannot sign with {}: COINCUBE doesn't recognise this reason. Update \
             COINCUBE, or contact support at coincube.io/support.",
            "No se puede firmar con {}: COINCUBE no reconoce este motivo. Actualiza \
             COINCUBE o contacta con soporte en coincube.io/support.",
            entry,
        ),
    }
}

/// Keep the full descriptor available for the phone's local xpub membership
/// check, while exposing only ciphertext to Connect. Each seal uses fresh
/// randomness and the same request-bound AAD as the PSBT.
fn seal_signing_payloads(
    transport_pubkey: &[u8],
    request_id: &str,
    psbt: &[u8],
    descriptor: &str,
) -> Result<
    (
        crate::services::connect::crypto::SealedPayload,
        crate::services::connect::crypto::SealedPayload,
    ),
    crate::services::inheritance::EciesError,
> {
    use crate::services::connect::crypto::seal_to_device;
    Ok((
        seal_to_device(transport_pubkey, request_id, psbt)?,
        seal_to_device(transport_pubkey, request_id, descriptor.as_bytes())?,
    ))
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

#[cfg(test)]
impl KeychainSignModal {
    /// A flow already resolved to one Connect signer per `(fingerprint,
    /// status)` — the state the unified picker reads its rows from. For the
    /// picker's tests in `psbt.rs`, which can't reach this module's fixtures.
    pub(super) fn resolved_for_test(
        wallet: Arc<Wallet>,
        signers: &[(Fingerprint, PendingSessionStatus)],
    ) -> Self {
        use crate::app::state::vault::test_support::{empty_psbt, tokens};
        let mut modal = Self::new(
            wallet,
            CoincubeClient::new(),
            tokens(),
            "https://grpc.example.test".to_string(),
            "desktop-device".to_string(),
            42,
            "cube-local".to_string(),
            empty_psbt(),
            None,
        );
        modal.pending = signers
            .iter()
            .enumerate()
            .map(|(i, (fingerprint, status))| PendingSession {
                session_id: format!("session-{i}"),
                key_id: i as u64 + 1,
                fingerprint: *fingerprint,
                device_id: format!("device-{i}"),
                transport_pubkey: Vec::new(),
                capabilities: Vec::new(),
                request_id: format!("req-{i}"),
                label: format!("Signer {i}"),
                status: *status,
                error: None,
                cancel_requested: false,
                signed_psbt_persisted: false,
                signed_psbt_fetching: false,
                signed_psbt_merged: false,
                signed_psbt_persisting: false,
            })
            .collect();
        modal.phase = Phase::Sessions;
        modal
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::app::state::vault::test_support::{empty_psbt, tokens};
    use coincube_core::descriptors::CoincubeDescriptor;
    use std::str::FromStr;

    #[test]
    fn chain_identity_is_exact_with_only_the_bitcoin_legacy_window() {
        use crate::chain::ChainId;
        for chain in [
            ChainId::Bitcoin,
            ChainId::Testnet,
            ChainId::Testnet4,
            ChainId::Signet,
            ChainId::Regtest,
            ChainId::BitcoinBlake2b,
            ChainId::BitcoinBlake2bTestnet4,
        ] {
            for exists in [false, true] {
                assert!(check_session_network(chain, chain.api_str(), exists).is_ok());
                assert_eq!(
                    check_session_network(chain, "", exists).is_ok(),
                    !chain.is_blake2b()
                );
                for wrong in ["bitcoin", "future-chain", " mainnet"] {
                    assert!(check_session_network(chain, wrong, exists).is_err());
                }
            }
        }
    }

    #[test]
    fn resolve_mismatch_and_identityless_btcb2_clear_targets_before_sealing() {
        use crate::chain::ChainId;
        for chain in [
            ChainId::Bitcoin,
            ChainId::BitcoinBlake2b,
            ChainId::BitcoinBlake2bTestnet4,
        ] {
            for network in ["future-chain", "bitcoin", "testnet"] {
                let mut m = modal();
                m.wallet = Arc::new(
                    Wallet::new(CoincubeDescriptor::from_str(RECOVERY_DESC).unwrap())
                        .with_chain(chain),
                );
                m.pending.push(pending(PendingSessionStatus::Idle));
                let task = m.on_signers_resolved(ResolveSignersResponse {
                    network: network.into(),
                    ..Default::default()
                });
                assert!(iced_runtime::task::into_stream(task).is_none());
                assert!(m.pending.is_empty());
                assert!(m.error.unwrap().contains("Nothing was sent"));
            }
        }
        let mut m = modal();
        m.wallet = Arc::new(
            Wallet::new(CoincubeDescriptor::from_str(RECOVERY_DESC).unwrap())
                .with_chain(ChainId::BitcoinBlake2b),
        );
        let task = m.on_signers_resolved(ResolveSignersResponse::default());
        assert!(iced_runtime::task::into_stream(task).is_none());
        assert!(m.error.unwrap().contains("Nothing was sent"));
    }

    #[test]
    fn resolved_capability_matrix_controls_btcb2_eligibility_and_classification() {
        use crate::chain::ChainId;
        use crate::services::connect::grpc::connect_v1::SignerTarget;
        let cases = [
            ("both", vec!["chain-identity-v1", "btcb2-unified-v1"], true),
            ("identity-only", vec!["chain-identity-v1"], false),
            ("unified-only", vec!["btcb2-unified-v1"], false),
            ("neither", vec![], false),
            (
                "malformed-equivalent",
                vec![" chain-identity-v1", "BTCB2-UNIFIED-V1"],
                false,
            ),
        ];
        for chain in [ChainId::BitcoinBlake2b, ChainId::BitcoinBlake2bTestnet4] {
            for (name, capabilities, allowed) in &cases {
                let mut m = modal();
                m.wallet = Arc::new(
                    Wallet::new(CoincubeDescriptor::from_str(RECOVERY_DESC).unwrap())
                        .with_chain(chain),
                );
                m.classified = Some(classified_signers_fixture());
                let task = m.on_signers_resolved(ResolveSignersResponse {
                    network: chain.api_str().into(),
                    unresolved: vec![],
                    targets: vec![SignerTarget {
                        key_fingerprint: "f5acc2fd".into(),
                        key_id: "7".into(),
                        capabilities: capabilities.iter().map(|value| (*value).into()).collect(),
                        transport_pubkey: TEST_TARGET_TRANSPORT_PUBKEY.to_vec(),
                        device_id: "phone".into(),
                    }],
                });
                assert!(iced_runtime::task::into_stream(task).is_none());
                assert!(m.error.is_none());
                assert_eq!(m.pending.len(), 1);
                assert_eq!(
                    m.pending[0].status.is_idle(),
                    *allowed,
                    "{:?}/{}",
                    chain,
                    name,
                );
                assert_eq!(
                    m.pending[0].error.is_none(),
                    *allowed,
                    "{:?}/{}",
                    chain,
                    name,
                );
                assert_eq!(
                    m.classified.as_ref().unwrap().required[0].replay_protection(),
                    if *allowed {
                        crate::app::state::vault::signers::ReplayProtection::Capable
                    } else {
                        crate::app::state::vault::signers::ReplayProtection::Legacy
                    },
                    "{:?}/{}",
                    chain,
                    name,
                );
                assert_eq!(
                    m.classified.as_ref().unwrap().required[1].replay_protection(),
                    crate::app::state::vault::signers::ReplayProtection::Legacy,
                    "only the exact resolved target may become capable",
                );
            }
        }
    }

    #[test]
    fn create_rechecks_every_btcb2_capability_before_psbt_preparation() {
        use crate::chain::ChainId;
        for chain in [ChainId::BitcoinBlake2b, ChainId::BitcoinBlake2bTestnet4] {
            for (name, capabilities) in [
                ("identity-only", vec!["chain-identity-v1"]),
                ("unified-only", vec!["btcb2-unified-v1"]),
                ("neither", vec![]),
                (
                    "malformed-equivalent",
                    vec!["chain-identity-v1 ", "BTcb2-unified-v1"],
                ),
            ] {
                let mut m = modal();
                m.wallet = Arc::new(
                    Wallet::new(CoincubeDescriptor::from_str(RECOVERY_DESC).unwrap())
                        .with_chain(chain),
                );
                let mut entry = pending(PendingSessionStatus::Idle);
                entry.capabilities = capabilities.into_iter().map(str::to_string).collect();
                entry.request_id.clear();
                entry.session_id.clear();
                m.pending.push(entry);
                let task = m.create_session_for(0);
                assert!(
                    iced_runtime::task::into_stream(task).is_none(),
                    "{:?}/{}",
                    chain,
                    name,
                );
                assert!(m.pending[0].request_id.is_empty(), "{:?}/{}", chain, name,);
                assert_eq!(
                    m.pending[0].status,
                    PendingSessionStatus::Failed,
                    "{:?}/{}",
                    chain,
                    name,
                );
                assert!(
                    m.pending[0]
                        .error
                        .as_ref()
                        .unwrap()
                        .contains("needs updating"),
                    "{:?}/{}: {:?}",
                    chain,
                    name,
                    m.pending[0].error,
                );
            }
        }
    }

    #[test]
    fn bitcoin_family_ignores_resolved_capability_matrix() {
        use crate::chain::ChainId;
        use crate::services::connect::grpc::connect_v1::SignerTarget;
        for chain in [
            ChainId::Bitcoin,
            ChainId::Testnet,
            ChainId::Testnet4,
            ChainId::Signet,
            ChainId::Regtest,
        ] {
            for (name, capabilities) in [
                ("both", vec!["chain-identity-v1", "btcb2-unified-v1"]),
                ("identity-only", vec!["chain-identity-v1"]),
                ("unified-only", vec!["btcb2-unified-v1"]),
                ("neither", vec![]),
                (
                    "malformed-equivalent",
                    vec!["chain-identity-v1 ", "BTcb2-unified-v1"],
                ),
            ] {
                let mut m = modal();
                m.wallet = Arc::new(
                    Wallet::new(CoincubeDescriptor::from_str(RECOVERY_DESC).unwrap())
                        .with_chain(chain),
                );
                m.classified = Some(classified_signers_fixture());
                let task = m.on_signers_resolved(ResolveSignersResponse {
                    network: chain.api_str().into(),
                    unresolved: vec![],
                    targets: vec![SignerTarget {
                        key_fingerprint: "f5acc2fd".into(),
                        key_id: "7".into(),
                        capabilities: capabilities.into_iter().map(str::to_string).collect(),
                        transport_pubkey: TEST_TARGET_TRANSPORT_PUBKEY.to_vec(),
                        device_id: "phone".into(),
                    }],
                });
                assert!(
                    iced_runtime::task::into_stream(task).is_none(),
                    "{:?}/{}",
                    chain,
                    name,
                );
                assert!(m.error.is_none(), "{:?}/{}", chain, name);
                assert!(m.pending[0].status.is_idle(), "{:?}/{}", chain, name);
                assert_eq!(
                    m.classified.as_ref().unwrap().required[0].replay_protection(),
                    crate::app::state::vault::signers::ReplayProtection::Legacy,
                    "{:?}/{}",
                    chain,
                    name,
                );
            }
        }
    }

    #[test]
    fn wrong_created_session_identity_cancels_and_blocks_later_merge() {
        use crate::chain::ChainId;
        for chain in [ChainId::Bitcoin, ChainId::BitcoinBlake2b] {
            for network in ["future-chain", "bitcoin-blake2b-testnet4", ""] {
                let mut m = modal();
                m.wallet = Arc::new(
                    Wallet::new(CoincubeDescriptor::from_str(RECOVERY_DESC).unwrap())
                        .with_chain(chain),
                );
                m.pending.push(pending(PendingSessionStatus::Creating));
                let mut session = created_session();
                session.network = network.into();
                let task = m.on_session_created(m.pending[0].fingerprint, Ok(session));
                let refused = !network.is_empty() || chain.is_blake2b();
                assert_eq!(m.pending[0].cancel_requested, refused);
                assert_eq!(iced_runtime::task::into_stream(task).is_some(), refused);
                if refused {
                    assert_eq!(m.pending[0].status, PendingSessionStatus::Failed);
                }
            }
        }
    }

    #[test]
    fn identity_refusals_do_not_misdiagnose_the_transport_key() {
        for token in [
            "NETWORK_INVALID",
            "NETWORK_MISMATCH",
            "CHAIN_IDENTITY_REQUIRED",
            "TARGET_KEY_NOT_ON_VAULT",
            "SIGNER_APP_OUTDATED",
            "NETWORK_DISABLED",
        ] {
            let (message, auth) = friendly_grpc_error(tonic::Status::failed_precondition(format!(
                "{}: server detail",
                token
            )));
            assert!(!auth);
            assert!(!message.contains("encrypted signing"), "{}", token);
            assert!(!message.contains("server detail"));
        }
    }

    // Primary signer `f5acc2fd`; recovery signer `8a64f2a9` behind a CSV.
    // Same fixture used by the `signers` classification tests.
    const RECOVERY_DESC: &str = "wsh(or_d(pk([f5acc2fd]tpubD6NzVbkrYhZ4YgUx2ZLNt2rLYAMTdYysCRzKoLu2BeSHKvzqPaBDvf17GeBPnExUVPkuBpx4kniP964e2MxyzzazcXLptxLXModSVCVEV1T/<0;1>/*),and_v(v:pkh([8a64f2a9]tpubD6NzVbkrYhZ4WmzFjvQrp7sDa4ECUxTi9oby8K4FZkd3XCBtEdKwUiQyYJaxiJo5y42gyDWEczrFpozEjeLxMPxjf2WtkfcbpUdfvNnozWF/<0;1>/*),older(10))))#d72le4dr";

    fn classified_signers_fixture() -> ClassifiedSigners {
        use crate::services::coincube::VaultStatus;
        ClassifiedSigners {
            vault: ConnectVaultResponse {
                id: 1,
                cube_id: 42,
                timelock_days: 90,
                timelock_expires_at: String::new(),
                last_reset_at: String::new(),
                status: VaultStatus::Active,
                members: vec![],
                fingerprint: String::new(),
                created_at: String::new(),
                updated_at: String::new(),
            },
            required: vec![
                RequiredSigner::Keychain {
                    fingerprint: Fingerprint::from_str("f5acc2fd").unwrap(),
                    key_id: 7,
                    owner_user_id: 7,
                    name: "Fixture".into(),
                    owner_email: None,
                    contact_id: None,
                    replay_protection: crate::app::state::vault::signers::ReplayProtection::Legacy,
                },
                RequiredSigner::Keychain {
                    fingerprint: Fingerprint::from_str("8a64f2a9").unwrap(),
                    key_id: 8,
                    owner_user_id: 8,
                    name: "Unresolved fixture".into(),
                    owner_email: None,
                    contact_id: None,
                    replay_protection: crate::app::state::vault::signers::ReplayProtection::Legacy,
                },
            ],
            self_user_id: 7,
        }
    }

    /// A Keychain flow with one session in flight on the server, for tests
    /// outside this module.
    pub(crate) fn modal_with_live_session() -> KeychainSignModal {
        let mut modal = modal();
        modal.pending.push(pending(PendingSessionStatus::Creating));
        modal
    }

    /// Whether every session of `modal` has had its cancellation requested.
    pub(crate) fn all_cancel_requested(modal: &KeychainSignModal) -> bool {
        modal.pending.iter().all(|p| p.cancel_requested)
    }

    fn modal() -> KeychainSignModal {
        let wallet = Arc::new(Wallet::new(
            CoincubeDescriptor::from_str(RECOVERY_DESC).unwrap(),
        ));
        KeychainSignModal::new(
            wallet,
            CoincubeClient::new(),
            tokens(),
            "https://grpc.example.test".to_string(),
            "desktop-device".to_string(),
            42,
            "cube-local".to_string(),
            empty_psbt(),
            Some(Arc::new(test_transport_key())),
        )
    }

    // ── Unresolved-signer copy ──────────────────────────────────────────
    //
    // The wire tokens below are spelled exactly as `coincube-api` emits them
    // (`services/connect/signing/grpc/session_server.go`). A paraphrase would
    // pass here while the arm never fires in production.

    // The two wordings the desktop used to append to every unrecognised
    // reason: the modal said "owner has no registered device", the unified
    // picker said "this signer has no registered device". The shared suffix
    // catches both and any future rephrasing of the same claim.
    const FALSE_DIAGNOSIS: &str = "no registered device";

    fn entry(reason: &str) -> String {
        // Same shape `on_signers_resolved` stores: "<fingerprint> (<reason>)".
        format!("f5acc2fd ({})", reason)
    }

    /// The regression: an unrecognised reason must not carry a diagnosis. Before
    /// the fix the fallthrough told the user the owner had no registered device
    /// for every token it did not know — including `transport_key_stale`, which
    /// the server emits and the desktop never matched.
    #[test]
    fn an_unrecognised_unresolved_reason_carries_no_diagnosis() {
        for reason in ["some_future_reason", ""] {
            let copy = friendly_unresolved_signer(&entry(reason));
            assert!(
                !copy.contains(FALSE_DIAGNOSIS),
                "{:?} rendered the false diagnosis: {}",
                reason,
                copy
            );
            // Verbatim, and pointed at something the user can do.
            assert!(copy.contains(&entry(reason)), "{}", copy);
            assert!(copy.contains("doesn't recognise this reason"), "{}", copy);
            assert!(copy.contains("coincube.io/support"), "{}", copy);
        }
        // An entry that is not even in the stored shape gets the same treatment.
        let copy = friendly_unresolved_signer("garbage");
        assert!(!copy.contains(FALSE_DIAGNOSIS), "{}", copy);
        assert!(copy.contains("garbage"), "{}", copy);
    }

    #[test]
    fn an_outdated_signer_has_the_known_update_diagnosis() {
        let copy = friendly_unresolved_signer("Alice (signer_app_outdated)");
        assert_eq!(
            copy,
            "Alice's COINCUBE Mobile needs updating before it can sign on Bitcoin Blake2b."
        );
        assert!(!copy.contains(FALSE_DIAGNOSIS));
    }

    #[test]
    fn a_stale_transport_key_tells_the_signer_to_open_keychain() {
        let copy = friendly_unresolved_signer(&entry("transport_key_stale"));
        assert!(
            copy.contains("open COINCUBE Mobile on their phone"),
            "{}",
            copy
        );
        assert!(!copy.contains(FALSE_DIAGNOSIS), "{}", copy);
        assert!(!copy.contains("registered"), "{}", copy);
    }

    #[test]
    fn an_unavailable_enrolled_device_points_at_the_phone_holding_the_key() {
        let copy = friendly_unresolved_signer(&entry("enrolled_device_unavailable"));
        assert!(copy.contains("can no longer be reached"), "{}", copy);
        assert!(copy.contains("phone that holds this key"), "{}", copy);
        assert!(!copy.contains(FALSE_DIAGNOSIS), "{}", copy);
    }

    /// The pre-existing arms keep their copy, matched on the exact token.
    #[test]
    fn the_existing_unresolved_arms_still_match_their_exact_tokens() {
        let no_device = friendly_unresolved_signer(&entry("no_device_registered"));
        assert!(
            no_device.contains("hasn't set up COINCUBE Mobile yet"),
            "{}",
            no_device
        );
        let revoked = friendly_unresolved_signer(&entry("all_devices_revoked"));
        assert!(revoked.contains("revoked every device"), "{}", revoked);
        // No server path emits this today; the arm is kept, not relied on.
        let unknown_owner = friendly_unresolved_signer(&entry("owner_unknown"));
        assert!(
            unknown_owner.contains("owner isn't known"),
            "{}",
            unknown_owner
        );
    }

    /// Exact-token matching: a token that merely *contains* a known token is
    /// unrecognised, so a renamed or extended wire value cannot be mistaken for
    /// the old one and given the old advice.
    #[test]
    fn unresolved_reason_matching_is_exact_not_substring() {
        let copy = friendly_unresolved_signer(&entry("no_device_registered_v2"));
        assert!(
            !copy.contains("hasn't set up COINCUBE Mobile yet"),
            "{}",
            copy
        );
        assert!(copy.contains("doesn't recognise this reason"), "{}", copy);
        assert_eq!(
            unresolved_signer_reason("abcd1234 (transport_key_stale)"),
            Some("transport_key_stale")
        );
        assert_eq!(
            unresolved_signer_reason("abcd1234 (a (nested) reason)"),
            Some("nested) reason")
        );
        assert_eq!(unresolved_signer_reason("no parens here"), None);
    }

    /// The unified picker (`SignModal::keychain_notices`) shows the same
    /// unresolved entries as this modal's own view. Driven through the real
    /// producer — `ResolveSigners` returning `unresolved` rows, stored by
    /// `on_signers_resolved` — and asserted as *equality* with
    /// `friendly_unresolved_signer`, so the two surfaces cannot drift: the
    /// picker used to hard-code "this signer has no registered device" for
    /// every reason, and a string-list assertion here would not have caught
    /// it.
    #[test]
    fn the_unified_picker_renders_unresolved_signers_through_the_shared_copy() {
        use crate::dir::CoincubeDirectory;
        use crate::services::coincube::VaultStatus;
        use crate::services::connect::grpc::connect_v1::UnresolvedSigner;
        use coincube_core::miniscript::bitcoin::Network;
        use std::collections::HashSet;
        use std::path::PathBuf;

        let mut modal = modal();
        // `on_signers_resolved` refuses to run before classification; an
        // empty required list is enough — no targets, only unresolved rows.
        modal.classified = Some(ClassifiedSigners {
            vault: ConnectVaultResponse {
                id: 1,
                cube_id: 42,
                timelock_days: 90,
                timelock_expires_at: "2027-01-01T00:00:00Z".to_string(),
                last_reset_at: "2026-01-01T00:00:00Z".to_string(),
                status: VaultStatus::Active,
                members: vec![],
                fingerprint: String::new(),
                created_at: "2026-01-01T00:00:00Z".to_string(),
                updated_at: "2026-01-01T00:00:00Z".to_string(),
            },
            required: vec![],
            self_user_id: 7,
        });
        let unresolved = |fp: &str, reason: &str| UnresolvedSigner {
            key_id: format!("key-{}", fp),
            key_fingerprint: fp.to_string(),
            owner_user_id: "user-9".to_string(),
            reason: reason.to_string(),
        };
        let _ = modal.on_signers_resolved(ResolveSignersResponse {
            network: String::new(),
            targets: vec![],
            unresolved: vec![
                unresolved("f5acc2fd", "transport_key_stale"),
                unresolved("8a64f2a9", "enrolled_device_unavailable"),
                unresolved("0badcafe", "some_future_reason"),
            ],
        });
        // The stored shape the copy function parses.
        assert_eq!(
            modal.unresolved(),
            [
                "f5acc2fd (transport_key_stale)".to_string(),
                "8a64f2a9 (enrolled_device_unavailable)".to_string(),
                "0badcafe (some_future_reason)".to_string(),
            ]
        );
        let expected: Vec<String> = modal
            .unresolved()
            .iter()
            .map(|u| friendly_unresolved_signer(u))
            .collect();

        let picker = super::super::psbt::SignModal::new(
            HashSet::new(),
            modal.wallet.clone(),
            CoincubeDirectory::new(PathBuf::new()),
            Network::Signet,
            false,
            None,
            Some(modal),
            true,
        );
        let notices = picker.keychain_notices();
        assert_eq!(notices, expected);
        assert_eq!(notices.len(), 3);
        for notice in &notices {
            assert!(!notice.contains(FALSE_DIAGNOSIS), "{}", notice);
        }
        assert!(
            notices[0].contains("open COINCUBE Mobile on their phone"),
            "{}",
            notices[0]
        );
        assert!(
            notices[1].contains("phone that holds this key"),
            "{}",
            notices[1]
        );
        assert!(
            notices[2].contains("doesn't recognise this reason"),
            "{}",
            notices[2]
        );
    }

    /// Verifies that `seal_signing_payloads` produces distinct ephemeral keys
    /// for PSBT and descriptor envelopes, both decrypt correctly with the
    /// request-bound AAD, and reject tampering or AAD mismatch.
    #[test]
    fn signing_descriptor_is_sealed_intact_and_bound_to_request() {
        let target = test_transport_key();
        let request_id = "descriptor-sealing-test";
        let (psbt, descriptor) =
            seal_signing_payloads(&target.public_key(), request_id, b"psbt", RECOVERY_DESC)
                .unwrap();
        assert_ne!(psbt.ephemeral_pubkey, descriptor.ephemeral_pubkey);
        assert_eq!(
            target
                .open(
                    &psbt.ephemeral_pubkey,
                    &psbt.nonce,
                    &psbt.ciphertext,
                    request_id
                )
                .unwrap()
                .as_slice(),
            b"psbt"
        );
        assert_eq!(
            target
                .open(
                    &descriptor.ephemeral_pubkey,
                    &descriptor.nonce,
                    &descriptor.ciphertext,
                    request_id
                )
                .unwrap()
                .as_slice(),
            RECOVERY_DESC.as_bytes()
        );
        assert!(target
            .open(
                &descriptor.ephemeral_pubkey,
                &descriptor.nonce,
                &descriptor.ciphertext,
                "another-request"
            )
            .is_err());
        let mut tampered = descriptor.ciphertext.clone();
        tampered[0] ^= 1;
        assert!(target
            .open(
                &descriptor.ephemeral_pubkey,
                &descriptor.nonce,
                &tampered,
                request_id
            )
            .is_err());
        assert!(seal_signing_payloads(&[], request_id, b"psbt", RECOVERY_DESC).is_err());
    }

    /// A throwaway transport keypair for the modal under test — the real one
    /// comes from the device sidecar via `Cache::connect_transport_key`.
    fn test_transport_key() -> crate::services::connect::crypto::DeviceTransportKey {
        let dir = crate::dir::NetworkDirectory::new(std::env::temp_dir().join(format!(
            "coincube-keychain-sign-transport-{}",
            std::process::id()
        )));
        std::fs::create_dir_all(dir.path()).unwrap();
        crate::services::connect::crypto::DeviceTransportKey::load_or_create(&dir).unwrap()
    }

    /// A valid compressed secp256k1 point standing in for a signer device's
    /// registered transport key (the generator point).
    const TEST_TARGET_TRANSPORT_PUBKEY: [u8; 33] = [
        0x02, 0x79, 0xBE, 0x66, 0x7E, 0xF9, 0xDC, 0xBB, 0xAC, 0x55, 0xA0, 0x62, 0x95, 0xCE, 0x87,
        0x0B, 0x07, 0x02, 0x9B, 0xFC, 0xDB, 0x2D, 0xCE, 0x28, 0xD9, 0x59, 0xF2, 0x81, 0x5B, 0x16,
        0xF8, 0x17, 0x98,
    ];

    /// A `CreateSigningSession` response with nothing set but the id — each
    /// test fills in only the field it is about.
    fn created_session() -> SigningSession {
        SigningSession {
            network: String::new(),
            session_id: "session-created".to_string(),
            request_id: "req-1".to_string(),
            user_id: String::new(),
            vault_id: String::new(),
            descriptor_id: String::new(),
            psbt: Vec::new(),
            tx_summary: None,
            policy_summary: None,
            targets: Vec::new(),
            status: crate::services::connect::grpc::connect_v1::SessionStatus::Pending as i32,
            created_at: None,
            expires_at: None,
            created_by_device_id: String::new(),
            note: String::new(),
            submitted_signatures: Vec::new(),
            is_recovery_spend: false,
            payload_scheme: crate::services::connect::grpc::connect_v1::PayloadScheme::EciesV1
                as i32,
            psbt_envelopes: Vec::new(),
            descriptor_envelopes: Vec::new(),
            creator_transport_pubkey: Vec::new(),
        }
    }

    fn pending(status: PendingSessionStatus) -> PendingSession {
        PendingSession {
            capabilities: Vec::new(),
            session_id: "session-1".to_string(),
            request_id: "req-1".to_string(),
            transport_pubkey: TEST_TARGET_TRANSPORT_PUBKEY.to_vec(),
            key_id: 7,
            fingerprint: Fingerprint::from_str("f5acc2fd").unwrap(),
            device_id: "phone-1".to_string(),
            label: "Phone (you)".to_string(),
            status,
            error: None,
            cancel_requested: false,
            signed_psbt_persisted: false,
            signed_psbt_fetching: false,
            signed_psbt_merged: false,
            signed_psbt_persisting: false,
        }
    }

    // ── End-to-end signing rail (PLAN-connect-blinding PR D4) ────────
    //
    // The rail must be all-or-nothing: a session either travels sealed to the
    // signer's transport key, or it doesn't happen. These pin the two refusal
    // paths and the sealed-payload round-trip.

    #[test]
    fn a_signer_without_a_transport_key_fails_closed_with_an_update_prompt() {
        // Master I5, no downgrade: an older Keychain that registered no
        // transport key must not silently drag the session back onto the
        // plaintext rail.
        let mut m = modal();
        let mut row = pending(PendingSessionStatus::Idle);
        row.transport_pubkey = Vec::new();
        m.pending = vec![row];

        let _ = m.create_session_for(0);

        assert_eq!(m.pending[0].status, PendingSessionStatus::Failed);
        let err = m.pending[0].error.as_deref().unwrap();
        assert!(
            err.contains("update COINCUBE Mobile"),
            "expected an update prompt, got: {}",
            err
        );
    }

    #[test]
    fn a_desktop_without_a_transport_key_refuses_rather_than_downgrading() {
        // The mirror case: we couldn't mint our own key, so a signature could
        // never be sealed back to us. Refuse instead of falling back.
        let mut m = modal();
        m.transport_key = None;
        m.pending = vec![pending(PendingSessionStatus::Idle)];

        let _ = m.create_session_for(0);

        assert_eq!(m.pending[0].status, PendingSessionStatus::Failed);
        assert!(m.pending[0]
            .error
            .as_deref()
            .unwrap()
            .contains("encrypted signing"));
    }

    // (A target whose transport key is the right length but not a point on the
    // curve is rejected at the seal, which sits after the PSBT prune and so
    // isn't reachable with a fixture PSBT. That rejection is pinned at the
    // codec level by `crypto::transport`'s `malformed_inputs_are_rejected_by_name`.)

    /// `SigningSession.creator_transport_pubkey` is what a signer seals its
    /// signature to. If Connect's copy has drifted from ours the signature
    /// comes back unopenable — and the only symptom without this check is a
    /// confusing decrypt failure *after* the keyholder already approved.
    #[test]
    fn a_creator_transport_key_mismatch_fails_before_anyone_approves() {
        let mut m = modal();
        m.pending = vec![pending(PendingSessionStatus::Creating)];
        let fp = m.pending[0].fingerprint;

        let mut session = created_session();
        // Any valid-but-different point: the server has a stale registration.
        session.creator_transport_pubkey = TEST_TARGET_TRANSPORT_PUBKEY.to_vec();

        let _ = m.on_session_created(fp, Ok(session));

        assert_eq!(m.pending[0].status, PendingSessionStatus::Failed);
        assert!(m.pending[0]
            .error
            .as_deref()
            .unwrap()
            .contains("re-register"));
    }

    #[test]
    fn a_matching_creator_transport_key_proceeds() {
        let mut m = modal();
        m.pending = vec![pending(PendingSessionStatus::Creating)];
        let fp = m.pending[0].fingerprint;

        let mut session = created_session();
        session.creator_transport_pubkey = m.transport_key.as_ref().unwrap().public_key().to_vec();

        let _ = m.on_session_created(fp, Ok(session));

        assert!(m.pending[0].error.is_none(), "{:?}", m.pending[0].error);
        assert_eq!(m.pending[0].session_id, "session-created");
    }

    #[test]
    fn an_empty_creator_transport_key_is_not_treated_as_a_mismatch() {
        // Empty means the server did not report one — an older build, or a
        // PLAINTEXT session. That is the "old server" case, not drift, and
        // must not block a session the server was willing to create.
        let mut m = modal();
        m.pending = vec![pending(PendingSessionStatus::Creating)];
        let fp = m.pending[0].fingerprint;

        let _ = m.on_session_created(fp, Ok(created_session()));

        assert!(m.pending[0].error.is_none(), "{:?}", m.pending[0].error);
    }

    /// The server refuses to create an ECIES session when THIS desktop has no
    /// registered transport key, and the remedy is something the user has to
    /// do. The raw gRPC text names the field, not the fix.
    #[test]
    fn failed_precondition_becomes_a_re_register_instruction() {
        let err = OpError::from_status(tonic::Status::failed_precondition(
            "this device has no registered transport_pubkey; re-register the device before \
             creating an end-to-end encrypted session",
        ));
        assert!(
            err.message.contains("Sign out of Connect and sign back in"),
            "the remedy must be named, got: {}",
            err.message
        );
        // Not an auth failure: it must not tear down the modal with the
        // "session expired" banner.
        assert!(!err.auth);
    }

    #[test]
    fn the_signature_envelope_round_trips_through_the_desktop_transport_key() {
        // What the rail actually does end to end: the signer seals the signed
        // PSBT to our transport pubkey, bound to the session's request_id, and
        // only this desktop can open it.
        use crate::services::connect::crypto::{seal_to_device, DeviceTransportKey};

        let key = test_transport_key();
        let signed_psbt = b"cHNidP8-pretend-signed-psbt".to_vec();
        let request_id = "req-e2e-1";

        let sealed = seal_to_device(&key.public_key(), request_id, &signed_psbt).unwrap();
        let opened = key
            .open(
                &sealed.ephemeral_pubkey,
                &sealed.nonce,
                &sealed.ciphertext,
                request_id,
            )
            .unwrap();
        assert_eq!(opened.as_slice(), signed_psbt.as_slice());

        // A different device — i.e. the server, or another signer — cannot.
        let other = DeviceTransportKey::load_or_create(&crate::dir::NetworkDirectory::new({
            let d = std::env::temp_dir().join(format!("coincube-ks-other-{}", std::process::id()));
            std::fs::create_dir_all(&d).unwrap();
            d
        }))
        .unwrap();
        assert!(other
            .open(
                &sealed.ephemeral_pubkey,
                &sealed.nonce,
                &sealed.ciphertext,
                request_id
            )
            .is_err());
    }

    #[test]
    fn pending_session_status_labels_and_terminal_flags_are_stable() {
        let cases = [
            (PendingSessionStatus::Idle, "", false, false),
            (PendingSessionStatus::Creating, "Requesting…", false, false),
            (PendingSessionStatus::Pending, "Requested…", false, false),
            (PendingSessionStatus::Delivered, "Delivered", false, false),
            (PendingSessionStatus::Viewed, "Viewed", false, false),
            (PendingSessionStatus::Approved, "Approved", false, false),
            (
                PendingSessionStatus::PartiallySigned,
                "Signing…",
                false,
                false,
            ),
            (PendingSessionStatus::Completed, "Signed", true, true),
            (PendingSessionStatus::Rejected, "Rejected", true, false),
            (PendingSessionStatus::Cancelled, "Cancelled", true, false),
            (PendingSessionStatus::Expired, "Expired", true, false),
            (PendingSessionStatus::Failed, "Failed", true, false),
        ];

        for (status, label, terminal, success) in cases {
            assert_eq!(status.label(), label);
            assert_eq!(status.is_terminal(), terminal);
            assert_eq!(status.is_terminal_success(), success);
        }
        assert!(PendingSessionStatus::Idle.is_idle());
        assert!(!PendingSessionStatus::Pending.is_idle());
    }

    #[test]
    fn modal_phase_helpers_track_loading_resolution_and_done_states() {
        let mut modal = modal();
        assert!(modal.is_loading());
        assert!(!modal.is_resolved());
        assert!(!modal.is_done());

        modal.phase = Phase::Resolving;
        assert!(modal.is_loading());
        assert!(!modal.is_resolved());

        modal.phase = Phase::Sessions;
        assert!(!modal.is_loading());
        assert!(modal.is_resolved());
        assert!(!modal.is_done());

        modal.phase = Phase::AllDone;
        assert!(modal.is_resolved());
        assert!(modal.is_done());
    }

    #[test]
    fn stream_health_banner_only_shows_for_degraded_pending_sessions() {
        let mut modal = modal();
        assert!(modal.stream_health_banner().is_none());

        modal.pending.push(pending(PendingSessionStatus::Pending));
        assert!(modal.stream_health_banner().is_none());

        modal.stream_health = crate::app::ConnectionStatus::Reconnecting;
        assert!(modal
            .stream_health_banner()
            .is_some_and(|b| b.starts_with("Reconnecting to Connect")));

        modal.stream_health = crate::app::ConnectionStatus::Offline;
        assert!(modal
            .stream_health_banner()
            .is_some_and(|b| b.contains("back online")));

        modal.stream_health = crate::app::ConnectionStatus::RenewFailed;
        assert!(modal
            .stream_health_banner()
            .is_some_and(|b| b.contains("Try again")));

        modal.pending[0].status = PendingSessionStatus::Completed;
        assert!(modal.stream_health_banner().is_none());
    }

    #[test]
    fn done_and_dismissed_drain_helpers_ignore_idle_but_wait_for_active_sessions() {
        let mut modal = modal();
        assert!(!modal.check_all_done());
        assert!(!modal.has_undrained_sessions());

        modal.pending.push(pending(PendingSessionStatus::Idle));
        assert!(!modal.check_all_done());
        assert!(!modal.has_undrained_sessions());

        modal.pending[0].status = PendingSessionStatus::Creating;
        assert!(modal.has_undrained_sessions());
        modal.mark_dismissed();
        let _ = modal.close_if_dismissed_and_drained();
        assert!(!matches!(modal.phase, Phase::AllDone));

        modal.pending[0].status = PendingSessionStatus::Completed;
        assert!(!modal.has_undrained_sessions());
        assert!(!modal.check_all_done());

        modal.pending[0].signed_psbt_persisted = true;
        assert!(modal.check_all_done());

        modal.pending[0].status = PendingSessionStatus::Cancelled;
        modal.pending[0].signed_psbt_persisted = false;
        let _ = modal.close_if_dismissed_and_drained();
        assert!(matches!(modal.phase, Phase::AllDone));
    }

    #[test]
    fn persistence_pending_tracks_merged_but_unsaved_rows() {
        let mut modal = modal();
        modal.pending.push(pending(PendingSessionStatus::Completed));

        // Terminal-success alone is not persistence-pending: nothing merged.
        assert!(!modal.has_persistence_pending());

        // Merged but not yet persisted — threshold close must wait.
        modal.pending[0].signed_psbt_merged = true;
        assert!(modal.has_persistence_pending());

        // A persist that *failed* still leaves the sig unsaved: keep blocking
        // so the row can be marked Failed rather than closing the modal.
        modal.pending[0].status = PendingSessionStatus::Failed;
        assert!(modal.has_persistence_pending());

        // Once durably persisted, no longer pending.
        modal.pending[0].status = PendingSessionStatus::Completed;
        modal.pending[0].signed_psbt_persisted = true;
        assert!(!modal.has_persistence_pending());
    }

    #[test]
    fn dismissal_waits_for_in_flight_persist_then_closes_on_persisted_err() {
        // Cancel during persistence, followed by Persisted(Err): the dismissed
        // modal must stay mounted until the persist callback returns, so the
        // failure can mark the row Failed instead of landing on a dropped modal.
        let mut modal = modal();
        modal.pending.push(pending(PendingSessionStatus::Completed));
        // Signature merged; `update_spend_tx` dispatched, callback not back yet
        // (the state `on_session_fetched` leaves behind).
        modal.pending[0].signed_psbt_merged = true;
        modal.pending[0].signed_psbt_persisting = true;

        // Both gates hold while the persist is in flight.
        assert!(modal.has_capture_in_flight());
        assert!(modal.has_persistence_pending());

        // User cancels/dismisses. `close_if_dismissed_and_drained` (the update
        // choke point) must NOT tear down while the callback is outstanding,
        // even though the row's status (Completed) is terminal.
        modal.mark_dismissed();
        let _ = modal.close_if_dismissed_and_drained();
        assert!(
            !matches!(modal.phase, Phase::AllDone),
            "dismissed modal must wait for the in-flight persist callback"
        );

        // Persisted(Err) lands: row Failed, no longer in flight, but still
        // merged-but-unsaved (so threshold teardown would stay blocked).
        modal.pending[0].status = PendingSessionStatus::Failed;
        modal.pending[0].signed_psbt_persisting = false;
        assert!(!modal.has_capture_in_flight());
        assert!(
            modal.has_persistence_pending(),
            "a failed persist still blocks threshold teardown until retry"
        );

        // With the callback resolved, the dismissed modal can finally drain.
        let _ = modal.close_if_dismissed_and_drained();
        assert!(matches!(modal.phase, Phase::AllDone));
    }

    #[test]
    fn dismissal_waits_for_in_flight_fetch_of_a_completed_session() {
        // A SessionCompleted event sets status=Completed AND dispatches the
        // GetSigningSession fetch (signed_psbt_fetching=true) *before* the merge.
        // Dismissing in that window must NOT drop the modal — otherwise the
        // returning fetch lands on nothing and the signature is lost even though
        // the signer already signed.
        let mut modal = modal();
        modal.pending.push(pending(PendingSessionStatus::Completed));
        modal.pending[0].signed_psbt_fetching = true; // fetch dispatched, not merged yet

        assert!(modal.has_capture_in_flight());

        modal.mark_dismissed();
        let _ = modal.close_if_dismissed_and_drained();
        assert!(
            !matches!(modal.phase, Phase::AllDone),
            "dismissed modal must wait for the in-flight signed-PSBT fetch of a Completed session"
        );

        // Fetch resolves (flag cleared); with nothing else in flight it drains.
        modal.pending[0].signed_psbt_fetching = false;
        assert!(!modal.has_capture_in_flight());
        let _ = modal.close_if_dismissed_and_drained();
        assert!(matches!(modal.phase, Phase::AllDone));
    }

    #[test]
    fn poll_does_not_refetch_a_merged_but_unpersisted_row() {
        // A non-terminal row whose signature is already merged (persist still
        // in flight) must not be re-fetched — doing so would run a second
        // GetSigningSession + update_spend_tx for the same session. The network
        // work in `poll_pending_sessions` is lazy, so calling it here only
        // exercises the synchronous skip/mark logic.
        let mut modal = modal();
        modal
            .pending
            .push(pending(PendingSessionStatus::PartiallySigned));

        // Baseline: an un-merged, non-terminal row *is* polled (marks fetching).
        let _ = modal.poll_pending_sessions();
        assert!(modal.pending[0].signed_psbt_fetching);

        // Merged-but-unpersisted: reset the fetch flag and confirm poll skips it.
        modal.pending[0].signed_psbt_fetching = false;
        modal.pending[0].signed_psbt_merged = true;
        let _ = modal.poll_pending_sessions();
        assert!(
            !modal.pending[0].signed_psbt_fetching,
            "merged row must not be re-fetched by a poll tick"
        );
    }

    #[test]
    fn poll_retries_a_completed_but_uncaptured_session() {
        // The core regression: a session that reached `Completed` (e.g. the
        // stream event landed before the signed PSBT was fetched) but whose
        // signature is NOT captured must keep being polled — `Completed` is
        // terminal, but retries key off `is_give_up`/`signature_captured`, not
        // `is_terminal`. Without this the signature is stranded forever.
        let mut modal = modal();
        modal.pending.push(pending(PendingSessionStatus::Completed));
        // session_id is set by `pending()`, no capture flags, not fetching.
        assert!(!modal.pending[0].signature_captured());

        let _ = modal.poll_pending_sessions();
        assert!(
            modal.pending[0].signed_psbt_fetching,
            "a Completed-but-uncaptured session must be re-fetched by the poll"
        );
    }

    #[test]
    fn poll_skips_captured_and_given_up_sessions() {
        // Captured (merged OR persisted) and give-up terminals must NOT poll.
        for (status, merged, persisted) in [
            (PendingSessionStatus::Completed, true, false), // merged
            (PendingSessionStatus::Completed, false, true), // persisted
            (PendingSessionStatus::Failed, false, false),
            (PendingSessionStatus::Rejected, false, false),
            (PendingSessionStatus::Cancelled, false, false),
            (PendingSessionStatus::Expired, false, false),
        ] {
            let mut modal = modal();
            modal.pending.push(pending(status));
            modal.pending[0].signed_psbt_merged = merged;
            modal.pending[0].signed_psbt_persisted = persisted;

            let _ = modal.poll_pending_sessions();
            assert!(
                !modal.pending[0].signed_psbt_fetching,
                "captured/given-up session ({:?}, merged={}, persisted={}) must not poll",
                status, merged, persisted
            );
        }
    }

    #[test]
    fn proto_status_mapping_handles_known_and_unknown_values() {
        use ProtoSessionStatus::*;

        let cases = [
            (Pending, PendingSessionStatus::Pending),
            (Delivered, PendingSessionStatus::Delivered),
            (Viewed, PendingSessionStatus::Viewed),
            (Approved, PendingSessionStatus::Approved),
            (PartiallySigned, PendingSessionStatus::PartiallySigned),
            (Completed, PendingSessionStatus::Completed),
            (Rejected, PendingSessionStatus::Rejected),
            (Cancelled, PendingSessionStatus::Cancelled),
            (Expired, PendingSessionStatus::Expired),
            (Failed, PendingSessionStatus::Failed),
            (Unspecified, PendingSessionStatus::Pending),
        ];

        for (proto, status) in cases {
            assert_eq!(PendingSessionStatus::from_proto(proto), status);
        }

        assert_eq!(session_status_from_i32(1), Pending);
        assert_eq!(session_status_from_i32(6), Completed);
        assert_eq!(session_status_from_i32(10), Failed);
        assert_eq!(session_status_from_i32(999), Unspecified);
    }

    #[test]
    fn event_type_mapping_handles_known_and_unknown_values() {
        use crate::services::connect::grpc::connect_v1::EventType;

        assert_eq!(event_type_from_i32(1), EventType::SessionCreated);
        assert_eq!(event_type_from_i32(6), EventType::SignatureSubmitted);
        assert_eq!(event_type_from_i32(12), EventType::DeviceOffline);
        assert_eq!(event_type_from_i32(-1), EventType::Unspecified);
    }

    #[test]
    fn auth_failure_detectors_classify_rest_and_grpc_errors() {
        assert!(is_rest_auth_failure("HTTP 401 Unauthorized"));
        assert!(is_rest_auth_failure("jwt expired"));
        assert!(is_rest_auth_failure("invalid token"));
        assert!(is_rest_auth_failure("403 forbidden"));
        assert!(!is_rest_auth_failure("temporary network failure"));

        let unauth = OpError::from_status(tonic::Status::unauthenticated("JWT expired"));
        assert!(unauth.auth);
        assert_eq!(
            unauth.message,
            "Your Connect session has expired. Please sign in again."
        );

        let denied = friendly_grpc_error(tonic::Status::permission_denied("nope"));
        assert!(denied.1);
        assert!(denied.0.contains("don't have permission"));

        let unavailable = friendly_grpc_error(tonic::Status::unavailable("offline"));
        assert!(!unavailable.1);
        assert!(unavailable.0.contains("temporarily unreachable"));

        let timed_out = friendly_grpc_error(tonic::Status::deadline_exceeded("slow"));
        assert!(!timed_out.1);
        assert!(timed_out.0.contains("timed out"));

        let other = friendly_grpc_error(tonic::Status::internal("boom"));
        assert_eq!(other, ("boom".to_string(), false));
    }

    #[test]
    fn generated_ids_and_fingerprint_strings_have_expected_wire_shape() {
        let id = uuid_v4();
        assert_eq!(id.len(), 36);
        assert_eq!(id.as_bytes()[14], b'4');
        assert!(matches!(id.as_bytes()[19], b'8' | b'9' | b'a' | b'b'));
        assert_eq!(id.matches('-').count(), 4);

        let fp = Fingerprint::from_str("f5acc2fd").unwrap();
        assert_eq!(fingerprint_as_str(&fp), "f5acc2fd");
        assert_eq!(OpError::new("plain").message, "plain");
        assert!(!OpError::new("plain").auth);
    }

    // ── API rail: every returned blob merges into the local PSBT ──────
    //
    // `on_session_fetched` seeds its accumulator from `tx.psbt` — the PSBT
    // this desktop already holds and authenticated — and merges every
    // returned blob into that clone. These drive the handler end to end with
    // **two** sealed returns: a one-return session never reaches the in-loop
    // merge, so it cannot tell a remote-seeded accumulator from a local one.
    mod api_rail_merge {
        use super::*;
        use crate::app::state::vault::test_support::unified::{fixture, legacy, unified, Fixture};
        use crate::chain::ChainId;
        use crate::services::connect::crypto::{seal_to_device, DeviceTransportKey};
        use crate::services::connect::grpc::connect_v1::{
            GetSigningSessionResponse, PayloadEnvelope, SessionStatus, SigningSession,
            SubmittedSignature,
        };
        use crate::utils::mock::Daemon as MockDaemon;
        use coincube_core::miniscript::bitcoin::{secp256k1, Amount, Network};
        use coincube_core::psbt_unified::UnifiedPsbt;
        use coincube_core::unified_finalize::verify_all_signatures;
        use serde_json::json;

        const SESSION_ID: &str = "session-two-blobs";
        const REQUEST_ID: &str = "req-two-blobs";

        /// A modal on `chain` for the fixture's Vault, holding `psbt` locally,
        /// with one live row whose fetch is in flight — the state a
        /// `SIGNATURE_SUBMITTED` / `SESSION_COMPLETED` fetch lands on.
        fn modal_on(
            f: &Fixture,
            chain: ChainId,
            psbt: &Psbt,
            key: &Arc<DeviceTransportKey>,
        ) -> KeychainSignModal {
            let wallet = Arc::new(Wallet::new(f.descriptor.clone()).with_chain(chain));
            let mut modal = KeychainSignModal::new(
                wallet,
                CoincubeClient::new(),
                tokens(),
                "https://grpc.example.test".to_string(),
                "desktop-device".to_string(),
                42,
                "cube-local".to_string(),
                psbt.clone(),
                Some(key.clone()),
            );
            let mut row = pending(PendingSessionStatus::PartiallySigned);
            row.session_id = SESSION_ID.to_string();
            row.request_id = REQUEST_ID.to_string();
            row.signed_psbt_fetching = true;
            modal.pending.push(row);
            modal
        }

        fn spend_tx(f: &Fixture, psbt: &Psbt) -> SpendTx {
            SpendTx::new(
                None,
                psbt.clone(),
                Vec::new(),
                &f.descriptor,
                &secp256k1::Secp256k1::new(),
                Network::Bitcoin,
            )
        }

        /// `psbt` sealed to this desktop's transport key under the session's
        /// request id, as a signer submits it (plaintext field empty).
        fn sealed(key: &DeviceTransportKey, psbt: &Psbt) -> SubmittedSignature {
            sealed_wire(key, &psbt.serialize())
        }

        fn sealed_wire(key: &DeviceTransportKey, bytes: &[u8]) -> SubmittedSignature {
            let s = seal_to_device(&key.public_key(), REQUEST_ID, bytes).unwrap();
            SubmittedSignature {
                device_id: "phone-1".to_string(),
                signed_psbt: Vec::new(),
                signed_key_ids: Vec::new(),
                created_at: None,
                signature_envelope: Some(PayloadEnvelope {
                    device_id: "desktop-device".to_string(),
                    ephemeral_pubkey: s.ephemeral_pubkey,
                    nonce: s.nonce,
                    ciphertext: s.ciphertext,
                }),
            }
        }

        /// A completed session carrying `blobs` in submission order.
        fn fetched(
            chain: ChainId,
            blobs: Vec<SubmittedSignature>,
        ) -> Result<GetSigningSessionResponse, OpError> {
            let session = SigningSession {
                network: chain.api_str().to_string(),
                session_id: SESSION_ID.to_string(),
                request_id: REQUEST_ID.to_string(),
                status: SessionStatus::Completed as i32,
                submitted_signatures: blobs,
                ..created_session()
            };
            Ok(GetSigningSessionResponse {
                session: Some(session),
            })
        }

        /// A signer's return stripped to its signatures — no prevout, no
        /// witness script, no derivations. Whether a Keychain's return is this
        /// sparse is the Keychain app's behaviour, not this repository's; the
        /// rail must not depend on it either way.
        fn sparse(psbt: &Psbt) -> Psbt {
            let mut p = psbt.clone();
            for input in &mut p.inputs {
                input.non_witness_utxo = None;
                input.witness_utxo = None;
                input.witness_script = None;
                input.bip32_derivation.clear();
            }
            p
        }

        /// The fixture's transaction with one output amount changed: a
        /// different txid, so signatures over it are for another transaction.
        fn other_transaction(f: &Fixture) -> Psbt {
            let mut other = f.psbt.clone();
            other.unsigned_tx.output[0].value = Amount::from_sat(40_001);
            other
        }

        fn unified_records(psbt: &Psbt) -> usize {
            psbt.inputs[0].proprietary.len()
        }

        /// Drive a task to completion, collecting the messages it emits. An
        /// `updatespend` the mock daemon was not told to expect panics the
        /// mock's thread and this future, so a completed drive against the
        /// empty mock is the proof that nothing was scheduled for persistence.
        async fn drive(task: Task<Message>) -> Vec<Message> {
            use iced::futures::StreamExt;
            use iced_runtime::{task::into_stream, Action};
            let mut out = Vec::new();
            if let Some(mut stream) = into_stream(task) {
                while let Some(action) = stream.next().await {
                    if let Action::Output(message) = action {
                        out.push(message);
                    }
                }
            }
            out
        }

        fn refusing_daemon() -> Arc<dyn Daemon + Sync + Send> {
            Arc::new(crate::daemon::client::Coincubed::new(
                MockDaemon::new(vec![]).run(),
            ))
        }

        /// Asserts the row failed with `needle`, `tx.psbt` is byte-identical
        /// to `before`, and neither merge nor persistence was recorded.
        fn assert_refused(modal: &KeychainSignModal, tx: &SpendTx, before: &[u8], needle: &str) {
            let row = &modal.pending[0];
            assert!(matches!(row.status, PendingSessionStatus::Failed));
            let error = row.error.as_deref().unwrap_or_default();
            assert!(error.contains(needle), "{}", error);
            assert_eq!(tx.psbt.serialize(), before, "local PSBT must be untouched");
            assert!(!row.signed_psbt_merged);
            assert!(!row.signed_psbt_persisting);
            assert!(!row.signed_psbt_fetching);
        }

        #[test]
        fn fetched_wrong_or_missing_btcb2_identity_cancels_without_opening_or_merging() {
            let f = fixture();
            let key = Arc::new(test_transport_key());
            for network in ["", "mainnet", "future-chain"] {
                let mut modal = modal_on(&f, ChainId::BitcoinBlake2b, &f.psbt, &key);
                let mut tx = spend_tx(&f, &f.psbt);
                let before = tx.psbt.serialize();
                let mut response = fetched(ChainId::BitcoinBlake2b, vec![]).unwrap();
                response.session.as_mut().unwrap().network = network.into();
                let task = modal.on_session_fetched(
                    refusing_daemon(),
                    &mut tx,
                    SESSION_ID.into(),
                    Ok(response),
                    true,
                );
                assert!(
                    iced_runtime::task::into_stream(task).is_some(),
                    "defensive cancellation must be queued"
                );
                assert!(modal.pending[0].cancel_requested);
                assert_refused(&modal, &tx, &before, "cancelled");
            }
        }

        #[test]
        fn outgoing_codec_is_standard_for_btcb2_and_byte_preserving_for_bitcoin() {
            let f = fixture();
            let signed = unified(&f.psbt, &f.signers[0]);
            for chain in [ChainId::BitcoinBlake2b, ChainId::BitcoinBlake2bTestnet4] {
                let wire = signing_wire_psbt(chain, signed.clone()).unwrap();
                assert_ne!(wire, signed.serialize());
                assert!(Psbt::deserialize(&wire).is_err());
                let imported = returned_wire_psbt(chain, &wire).unwrap();
                assert_eq!(imported.serialize(), signed.serialize());
            }
            let bitcoin = legacy(&f.psbt, &f.signers[0]);
            assert_eq!(
                signing_wire_psbt(ChainId::Bitcoin, bitcoin.clone()).unwrap(),
                bitcoin.serialize()
            );
            assert_eq!(
                returned_wire_psbt(ChainId::Bitcoin, &bitcoin.serialize())
                    .unwrap()
                    .serialize(),
                bitcoin.serialize()
            );
            let wire = signing_wire_psbt(ChainId::BitcoinBlake2b, signed).unwrap();
            assert!(returned_wire_psbt(ChainId::Bitcoin, &wire).is_err());
        }

        fn standard_refusal_wire(
            f: &Fixture,
            valid: &Psbt,
            valid_wire: &[u8],
            case: &str,
        ) -> (Vec<u8>, &'static str) {
            match case {
                "invalid-signature" | "conflict" => {
                    let signer = if case == "conflict" {
                        &f.signers[0]
                    } else {
                        &f.signers[1]
                    };
                    let mut invalid = sparse(&f.psbt);
                    invalid.inputs[0].proprietary = unified(&other_transaction(f), signer).inputs
                        [0]
                    .proprietary
                    .clone();
                    (
                        signing_wire_psbt(ChainId::BitcoinBlake2b, invalid).unwrap(),
                        if case == "conflict" {
                            "conflicting signature"
                        } else {
                            "cryptographically invalid"
                        },
                    )
                }
                "ambiguous-namespace" => {
                    let mut ambiguous = legacy(&f.psbt, &f.signers[0]);
                    ambiguous.inputs[0].proprietary = valid.inputs[0].proprietary.clone();
                    // Deliberately bypass the strict exporter to model hostile bytes.
                    assert!(signing_wire_psbt(ChainId::BitcoinBlake2b, ambiguous.clone()).is_err());
                    (ambiguous.serialize(), "Malformed signed PSBT")
                }
                "trailing-bytes" => {
                    let mut malformed = valid_wire.to_vec();
                    malformed.push(0);
                    (malformed, "Malformed signed PSBT")
                }
                _ => (valid_wire.to_vec(), "cancelled"),
            }
        }

        #[tokio::test]
        async fn standard_returns_refuse_invalid_conflicting_and_wrong_chain_inputs_atomically() {
            let f = fixture();
            let key = Arc::new(test_transport_key());
            let valid = unified(&f.psbt, &f.signers[0]);
            let valid_wire = signing_wire_psbt(ChainId::BitcoinBlake2b, valid.clone()).unwrap();
            for case in [
                "invalid-signature",
                "conflict",
                "ambiguous-namespace",
                "wrong-chain",
                "trailing-bytes",
            ] {
                let local = if case == "conflict" {
                    valid.clone()
                } else {
                    f.psbt.clone()
                };
                let mut modal = modal_on(&f, ChainId::BitcoinBlake2b, &local, &key);
                let mut tx = spend_tx(&f, &local);
                let before = tx.psbt.serialize();
                let (wire, needle) = standard_refusal_wire(&f, &valid, &valid_wire, case);
                let mut response = fetched(
                    ChainId::BitcoinBlake2b,
                    vec![sealed_wire(&key, &valid_wire), sealed_wire(&key, &wire)],
                )
                .unwrap();
                if case == "wrong-chain" {
                    response.session.as_mut().unwrap().network = "mainnet".into();
                }
                let task = modal.on_session_fetched(
                    refusing_daemon(),
                    &mut tx,
                    SESSION_ID.into(),
                    Ok(response),
                    false,
                );
                assert_refused(&modal, &tx, &before, needle);
                // Wrong-chain cancellation is deliberately queued; no merge or save.
                if case != "wrong-chain" {
                    assert!(drive(task).await.is_empty(), "{}", case);
                }
            }
        }

        #[tokio::test]
        async fn standard_unified_encrypted_return_imports_and_persists_internal_records() {
            let f = fixture();
            let key = Arc::new(test_transport_key());
            let signed = unified(&f.psbt, &f.signers[0]);
            let wire = coincube_core::psbt_unified::export_standard(
                &UnifiedPsbt::from_psbt(signed.clone()).unwrap(),
            )
            .unwrap();
            assert!(Psbt::deserialize(&wire).is_err());
            let mut expected = f.psbt.clone();
            crate::app::state::vault::psbt::merge_signatures_pub(
                ChainId::BitcoinBlake2b,
                &mut expected,
                &signed,
            )
            .unwrap();
            let mut modal = modal_on(&f, ChainId::BitcoinBlake2b, &f.psbt, &key);
            let mut tx = spend_tx(&f, &f.psbt);
            let daemon: Arc<dyn Daemon + Sync + Send> =
                Arc::new(crate::daemon::client::Coincubed::new(
                    MockDaemon::new(vec![(
                        Some(json!({"method":"updatespend","params":vec![expected.to_string()]})),
                        Ok(json!({})),
                    )])
                    .run(),
                ));
            let task = modal.on_session_fetched(
                daemon,
                &mut tx,
                SESSION_ID.into(),
                fetched(ChainId::BitcoinBlake2b, vec![sealed_wire(&key, &wire)]),
                false,
            );
            assert!(
                modal.pending[0].signed_psbt_merged,
                "{:?}",
                modal.pending[0].error
            );
            assert_eq!(tx.psbt.serialize(), expected.serialize());
            assert_eq!(unified_records(&tx.psbt), 1);
            verify_all_signatures(
                &UnifiedPsbt::from_psbt(tx.psbt.clone()).unwrap(),
                &secp256k1::Secp256k1::verification_only(),
            )
            .unwrap();
            let messages = drive(task).await;
            assert!(messages.iter().any(|m| matches!(
                m,
                Message::KeychainSign(KeychainSignMessage::Persisted { result: Ok(()), .. })
            )));
        }

        #[tokio::test]
        async fn two_sparse_encrypted_returns_merge_against_local_metadata() {
            let f = fixture();
            let key = Arc::new(test_transport_key());
            let first = sparse(&unified(&f.psbt, &f.signers[0]));
            let second = sparse(&unified(&f.psbt, &f.signers[1]));
            // Neither return can be verified on its own: the prevouts and
            // witness script live only in the local PSBT. An accumulator
            // seeded from the first return would refuse the second.
            for blob in [&first, &second] {
                assert!(verify_all_signatures(
                    &UnifiedPsbt::from_psbt(blob.clone()).unwrap(),
                    &secp256k1::Secp256k1::verification_only(),
                )
                .is_err());
            }
            let mut modal = modal_on(&f, ChainId::BitcoinBlake2b, &f.psbt, &key);
            let mut tx = spend_tx(&f, &f.psbt);
            let mut expected = f.psbt.clone();
            crate::app::state::vault::psbt::merge_signatures_pub(
                ChainId::BitcoinBlake2b,
                &mut expected,
                &first,
            )
            .unwrap();
            crate::app::state::vault::psbt::merge_signatures_pub(
                ChainId::BitcoinBlake2b,
                &mut expected,
                &second,
            )
            .unwrap();
            // The persisted row is the merged local PSBT, prevouts and all.
            let daemon: Arc<dyn Daemon + Sync + Send> =
                Arc::new(crate::daemon::client::Coincubed::new(
                    MockDaemon::new(vec![(
                        Some(json!({
                            "method": "updatespend",
                            "params": vec![expected.to_string()],
                        })),
                        Ok(json!({})),
                    )])
                    .run(),
                ));

            let task = modal.on_session_fetched(
                daemon,
                &mut tx,
                SESSION_ID.to_string(),
                fetched(
                    modal.wallet.chain,
                    vec![sealed(&key, &first), sealed(&key, &second)],
                ),
                false,
            );

            assert_eq!(tx.psbt.serialize(), expected.serialize());
            assert_eq!(unified_records(&tx.psbt), 2);
            assert!(tx.psbt.inputs[0].non_witness_utxo.is_some());
            verify_all_signatures(
                &UnifiedPsbt::from_psbt(tx.psbt.clone()).unwrap(),
                &secp256k1::Secp256k1::verification_only(),
            )
            .unwrap();
            let row = &modal.pending[0];
            assert!(matches!(row.status, PendingSessionStatus::Completed));
            assert!(row.signed_psbt_merged);
            assert!(row.signed_psbt_persisting);
            let messages = drive(task).await;
            assert!(messages.iter().any(|m| matches!(
                m,
                Message::KeychainSign(KeychainSignMessage::Persisted { session_id, result: Ok(()) })
                    if session_id == SESSION_ID
            )));
            assert!(messages.iter().any(|m| matches!(m, Message::Reconcile)));
        }

        #[tokio::test]
        async fn a_wrong_transaction_first_return_refuses_without_touching_the_local_psbt() {
            let f = fixture();
            let key = Arc::new(test_transport_key());
            let wrong = unified(&other_transaction(&f), &f.signers[0]);
            let valid = sparse(&unified(&f.psbt, &f.signers[1]));
            let mut modal = modal_on(&f, ChainId::BitcoinBlake2b, &f.psbt, &key);
            let mut tx = spend_tx(&f, &f.psbt);
            let before = tx.psbt.serialize();

            let task = modal.on_session_fetched(
                refusing_daemon(),
                &mut tx,
                SESSION_ID.to_string(),
                fetched(
                    modal.wallet.chain,
                    vec![sealed(&key, &wrong), sealed(&key, &valid)],
                ),
                false,
            );

            assert_refused(&modal, &tx, &before, "different transactions");
            assert!(
                drive(task).await.is_empty(),
                "nothing scheduled for persistence"
            );
        }

        #[tokio::test]
        async fn a_valid_first_and_invalid_later_return_refuses_atomically() {
            let f = fixture();
            let key = Arc::new(test_transport_key());
            let valid = sparse(&unified(&f.psbt, &f.signers[0]));
            // The second return is for this transaction but its signature was
            // produced over another one: representation is fine, the
            // signature does not verify.
            let mut invalid = sparse(&f.psbt);
            invalid.inputs[0].proprietary = unified(&other_transaction(&f), &f.signers[1]).inputs
                [0]
            .proprietary
            .clone();
            let mut modal = modal_on(&f, ChainId::BitcoinBlake2b, &f.psbt, &key);
            let mut tx = spend_tx(&f, &f.psbt);
            let before = tx.psbt.serialize();

            let task = modal.on_session_fetched(
                refusing_daemon(),
                &mut tx,
                SESSION_ID.to_string(),
                fetched(
                    modal.wallet.chain,
                    vec![sealed(&key, &valid), sealed(&key, &invalid)],
                ),
                false,
            );

            // Refused for the signature, not for missing prevouts: the
            // candidate was verified against the local metadata.
            assert_refused(&modal, &tx, &before, "cryptographically invalid");
            assert_eq!(
                unified_records(&tx.psbt),
                0,
                "the valid first return must not leak"
            );
            assert!(
                drive(task).await.is_empty(),
                "nothing scheduled for persistence"
            );
        }

        #[tokio::test]
        async fn a_pre_existing_local_signature_is_kept_and_a_conflict_is_refused() {
            // The local PSBT already carries a signature (a hot key, an
            // earlier session). Two returns: one adds a second key, the other
            // conflicts with the local one. Nothing of either lands.
            let f = fixture();
            let key = Arc::new(test_transport_key());
            let local = unified(&f.psbt, &f.signers[0]);
            let adds = sparse(&unified(&f.psbt, &f.signers[1]));
            let mut conflicting = sparse(&f.psbt);
            conflicting.inputs[0].proprietary = unified(&other_transaction(&f), &f.signers[0])
                .inputs[0]
                .proprietary
                .clone();
            let mut modal = modal_on(&f, ChainId::BitcoinBlake2b, &local, &key);
            let mut tx = spend_tx(&f, &local);
            let before = tx.psbt.serialize();

            let task = modal.on_session_fetched(
                refusing_daemon(),
                &mut tx,
                SESSION_ID.to_string(),
                fetched(
                    modal.wallet.chain,
                    vec![sealed(&key, &adds), sealed(&key, &conflicting)],
                ),
                false,
            );

            assert_refused(&modal, &tx, &before, "conflicting signature");
            assert_eq!(unified_records(&tx.psbt), 1);
            assert!(
                drive(task).await.is_empty(),
                "nothing scheduled for persistence"
            );
        }

        #[tokio::test]
        async fn bitcoin_two_returns_copy_in_submission_order_unchanged() {
            // The Bitcoin family keeps the historical copy. Pinned against the
            // former blob-seeded composition (first return as the base, later
            // returns copied over it, the result copied into the local PSBT):
            // last write wins on a key, so the two orders agree byte for byte.
            let f = fixture();
            let key = Arc::new(test_transport_key());
            let local = legacy(&f.psbt, &f.signers[2]);
            let first = legacy(&f.psbt, &f.signers[0]);
            let mut second = legacy(&f.psbt, &f.signers[1]);
            // The second return also carries a different signature for the
            // first return's key, so the copy order is observable.
            let (first_key, _) = first.inputs[0].partial_sigs.iter().next().unwrap();
            let replacement = legacy(&other_transaction(&f), &f.signers[0]).inputs[0]
                .partial_sigs
                .values()
                .next()
                .copied()
                .unwrap();
            second.inputs[0]
                .partial_sigs
                .insert(*first_key, replacement);
            let former = {
                let mut accumulator = first.clone();
                crate::app::state::vault::psbt::merge_signatures_pub(
                    ChainId::Bitcoin,
                    &mut accumulator,
                    &second,
                )
                .unwrap();
                let mut destination = local.clone();
                crate::app::state::vault::psbt::merge_signatures_pub(
                    ChainId::Bitcoin,
                    &mut destination,
                    &accumulator,
                )
                .unwrap();
                destination
            };
            // Local signatures kept, one key from each return (the first's
            // key overwritten by the second's copy of it).
            assert_eq!(
                former.inputs[0].partial_sigs.len(),
                local.inputs[0].partial_sigs.len() + 2
            );
            assert_eq!(former.inputs[0].partial_sigs[first_key], replacement);
            let mut modal = modal_on(&f, ChainId::Bitcoin, &local, &key);
            let mut tx = spend_tx(&f, &local);
            let daemon: Arc<dyn Daemon + Sync + Send> =
                Arc::new(crate::daemon::client::Coincubed::new(
                    MockDaemon::new(vec![(
                        Some(json!({
                            "method": "updatespend",
                            "params": vec![former.to_string()],
                        })),
                        Ok(json!({})),
                    )])
                    .run(),
                ));

            let task = modal.on_session_fetched(
                daemon,
                &mut tx,
                SESSION_ID.to_string(),
                fetched(
                    modal.wallet.chain,
                    vec![sealed(&key, &first), sealed(&key, &second)],
                ),
                false,
            );

            assert_eq!(tx.psbt.serialize(), former.serialize());
            assert!(modal.pending[0].signed_psbt_merged);
            assert!(modal.pending[0].signed_psbt_persisting);
            let messages = drive(task).await;
            assert!(messages.iter().any(|m| matches!(
                m,
                Message::KeychainSign(KeychainSignMessage::Persisted { result: Ok(()), .. })
            )));
        }
    }
}
