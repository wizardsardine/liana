use crate::{
    app::settings::{display::DisplayMode, unit::BitcoinDisplayUnit},
    daemon::{
        model::{Coin, ListCoinsResult},
        Daemon, DaemonError,
    },
    dir::CoincubeDirectory,
    services::fiat::{
        api::{GetPriceResult, PriceApi, PriceApiError},
        client::PriceClient,
        Currency, PriceSource,
    },
};
use coincube_core::miniscript::bitcoin::Network;
use coincubed::commands::CoinStatus;
use std::sync::Arc;
use std::time::Instant;

/// Live network participation stats of the active managed Bitcoin node, polled
/// from `getnetworkinfo` + `getnettotals`. Surfaced in Settings → Node so users
/// can see their node is actually connected and (for inbound-over-Tor) sharing.
#[derive(Debug, Clone, Default)]
pub struct NodeNetStats {
    /// Peers connected to us (inbound).
    pub connections_in: u64,
    /// Peers we connected to (outbound).
    pub connections_out: u64,
    /// Bytes uploaded so far — within the current 24h cap cycle when a cap is
    /// set, else total bytes sent this session.
    pub upload_used: u64,
    /// Daily upload cap in bytes (`maxuploadtarget`); `0` means unlimited.
    pub upload_target: u64,
    /// The v3 onion address bitcoind advertises, if reachable over Tor.
    pub onion_address: Option<String>,
    /// `getnetworkinfo.subversion` — which client and build is actually serving
    /// this backend. The one authority on that; configuration can disagree.
    pub subversion: Option<String>,
}

/// Where the running App is with a Connect-first Vault's optional managed node
/// (`daemon.toml`'s `pending_managed_node_install`). Settings → Node reads it so
/// the install is visible there instead of looking like no node was requested.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum BackgroundNodeInstall {
    /// No background install is running or has failed in this session.
    #[default]
    Idle,
    /// Downloading the node; the percentage once the size is known.
    Downloading(Option<f32>),
    /// Verifying, configuring and starting the node.
    Starting,
    /// The last attempt failed; the request stays on disk for a retry.
    Failed(String),
}

impl BackgroundNodeInstall {
    pub fn is_running(&self) -> bool {
        matches!(self, Self::Downloading(_) | Self::Starting)
    }
}

/// A Spark Stable Balance condition worth a banner. See
/// [`Cache::spark_notice`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SparkNotice {
    /// The bridge switched Stable Balance off because the SDK's
    /// auto-conversion kept failing (`Event::StableBalancePaused`).
    /// `reason` is the SDK's error text, e.g. "Pool has no liquidity".
    StableBalancePaused { reason: String },
    /// This wallet holds USDB but Stable Balance is off on this device —
    /// the state a Cube recovered on a new machine lands in, because the
    /// toggle lives in the SDK's local cache and doesn't travel with the
    /// seed. Shown once per device, until the user decides either way.
    StableBalanceOffWithHolding,
}

/// The user's names for this Cube's wallets, as the side panel shows them:
/// `Spark · ‹spark›` and `Vault · ‹vault›`. `None` shows the plain wallet
/// name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WalletNames {
    /// The Spark nickname,
    /// [`CubeSettings::spark_alias`](crate::app::settings::CubeSettings::spark_alias).
    pub spark: Option<String>,
    /// The Vault's alias,
    /// [`Wallet::alias`](crate::app::wallet::Wallet::alias).
    pub vault: Option<String>,
}

impl WalletNames {
    pub fn new(
        cube: &crate::app::settings::CubeSettings,
        vault: Option<&crate::app::wallet::Wallet>,
    ) -> Self {
        Self {
            spark: cube.spark_alias.clone(),
            vault: vault.and_then(|wallet| wallet.alias.clone()),
        }
    }
}

/// Identity of one `App` instance — one per Cube open (`#393`).
///
/// A task an `App` spawned can complete after the tab has replaced that `App`
/// (a Cube switch, a lock and unlock, a return from the installer), and the
/// tab hands its message to whichever `App` is current. Every reply that
/// writes into [`Cache::entangled`] therefore carries the generation it was
/// issued under, and [`Cache::accept_entanglement`] compares it with the
/// receiving instance's. Process-wide monotonic, so two opens of the same
/// Cube never share one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AppGeneration(u64);

impl AppGeneration {
    /// A fresh, never-handed-out generation.
    pub fn next() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}

/// Where an entanglement lookup was issued from: the `App` instance, and the
/// chain whose twin it asked about. Carried by every reply so the receiving
/// `App` can tell its own lookups from a predecessor's ([`Cache::accept_entanglement`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LookupOrigin {
    pub app: AppGeneration,
    pub chain: crate::chain::ChainId,
}

/// Progress of a name saved only on this device (the Spark nickname, the
/// Vault name), shown under its field.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NameSave {
    /// Not saved this session.
    #[default]
    Idle,
    /// Being written to this device's settings.
    Saving,
    /// Written to this device's settings.
    Saved,
    /// Could not be written.
    Failed,
}

/// Progress of the last Cube rename, shown under the Cube name field.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CubeNameSync {
    /// No rename this session.
    #[default]
    Idle,
    /// Writing the name to this device's Cube settings.
    Saving,
    /// Saved on this device; Connect has not answered yet.
    Syncing,
    /// Saved on this device and accepted by Connect.
    Synced,
    /// Saved on this device; this Cube has no Connect registration or the
    /// account is signed out, so there is nothing to sync now.
    LocalOnly,
    /// Saved on this device; Connect refused or was unreachable. The
    /// Launcher's catch-up retries it.
    SyncFailed,
    /// Could not be written on this device.
    SaveFailed,
}

#[derive(Debug, Clone)]
pub struct Cache {
    /// The `App` instance this cache belongs to. See [`AppGeneration`].
    pub app_generation: AppGeneration,
    pub unswept_notice: Option<super::unswept_notice::Notice>,
    pub datadir_path: CoincubeDirectory,
    /// IBD progress (0.0–1.0) of the pending or active local Bitcoind,
    /// polled directly via RPC, independently of the wallet backend.
    pub node_bitcoind_sync_progress: Option<f64>,
    /// Verified blocks and known headers reported by the same local RPC probe.
    pub node_bitcoind_sync_heights: Option<(u64, u64)>,
    /// Whether the pending or active local Bitcoind is currently in initial block download.
    /// `None` until a local node answers.
    pub node_bitcoind_ibd: Option<bool>,
    /// `getnetworkinfo.subversion` of the pending or active local Bitcoind, read on the same
    /// poll that reports progress. `None` until the first poll, or when the node
    /// would not say what it is — the syncing copy then names no build rather
    /// than guessing one. Kept raw so both the flavour and the version can be
    /// derived from the one source.
    pub node_bitcoind_subversion: Option<String>,
    /// Pruning of the pending or active local Bitcoind, read on the same poll.
    /// `None` until a node answers, or when it would not say.
    pub node_bitcoind_pruning: Option<super::local_switch::NodePruning>,
    /// How far back the Vault's history reaches, read on that poll only while
    /// a synced, pruned pending node is a switch target. `None` otherwise, or
    /// when the daemon could not say. Lets the Node settings explain a refused
    /// switch (see [`super::local_switch`]).
    pub local_switch_history: Option<super::local_switch::VaultHistory>,
    /// Mirror of `App::daemon_switch_in_progress` so the stateless Node
    /// settings view can reflect an in-flight backend switch (disable the
    /// switch buttons and show a "switching…" status) instead of offering a
    /// button that just errors with "a switch is already in progress".
    pub daemon_switch_in_progress: bool,
    /// Latest UpdateTip/blockheaders line from the pending internal bitcoind's
    /// debug.log.  `None` until the first line is received.
    pub node_bitcoind_last_log: Option<String>,
    /// Live network stats of the *active* managed node (connection counts,
    /// upload used vs. target, onion address), polled from its RPC on tick.
    /// `None` until the first poll, or when the backend isn't a local node.
    pub node_net_stats: Option<NodeNetStats>,
    /// The Connect-first managed-node install run by the App, if any.
    pub background_node_install: BackgroundNodeInstall,
    pub node_history: super::node_history::Status,
    pub node_history_switch_requested: bool,
    /// Keep the transfer checklist through the backend handoff and its success.
    /// Presentation only: this never authorizes recovery or an automatic switch.
    pub local_node_transfer_visible: bool,
    pub network: Network,
    /// The `last_poll_timestamp` when starting the application.
    pub last_poll_at_startup: Option<u32>,
    pub daemon_cache: DaemonCache,
    pub fiat_price: Option<FiatPrice>,
    /// Authoritative Cube chain for pricing and storage; `network` only encodes addresses.
    pub fiat_chain: crate::chain::ChainId,
    pub btcb2_price_request: Option<FiatPriceRequest>,
    /// Bitcoin display unit preference (BTC or Sats)
    pub bitcoin_unit: BitcoinDisplayUnit,
    /// Global fiat-native vs. bitcoin-native display preference. Drives
    /// whether wallet headers lead with the fiat or bitcoin amount across
    /// the app. Mirrored from `Settings::display_mode` (top-level, not
    /// per-cube) and re-read on `SettingsSaved`.
    pub display_mode: DisplayMode,
    /// Whether the Connect user is authenticated (Dashboard step reached)
    pub connect_authenticated: bool,
    /// Display-only account status shared across tabs. Never authorizes Cube operations.
    pub sidebar_connect_signed_in: bool,
    /// Whether a Connect account session exists for this install — a
    /// launch-time restore from `connect.json`, or a live in-app sign-in —
    /// *independent of* whether the Connect panel has advanced to
    /// its `Dashboard` UI step yet. Distinct from `connect_authenticated`,
    /// which only tracks that panel step and therefore lags a session that
    /// was restored at launch (or never visited). Surfaces that must treat a
    /// valid-but-not-yet-active session as signed-in — e.g. the
    /// keychain-unavailable modal choosing between "Sign in to Connect" and
    /// "Sign with Connect" — read this, OR'd with `connect_authenticated`.
    /// Cleared on logout.
    pub has_connect_session: bool,
    /// The account panel can restore its own keyring session in this Cube.
    pub restorable_connect_session: bool,
    /// Whether this Cube's Bitcoin Vault is loaded: a Vault or an imported
    /// Vault. Inheritance-only screens use [`Self::has_inheritance_vault`].
    pub has_vault: bool,
    /// This Cube owns a pending or installed public Bitcoin wallet import.
    pub has_imported_bitcoin: bool,
    /// Keys from recovery words stored in this Cube for its imported wallet,
    /// once read. Vault Settings shows them.
    pub imported_stored_keys: Option<usize>,
    /// The imported wallet's server and history scan, for Vault Settings →
    /// Node.
    pub imported_node: Option<crate::app::state::imported_bitcoin::ImportedNodeSummary>,
    /// The loaded Vault records which of its keys came from the COINCUBE
    /// Keychain, and none did. A Keychain key can only enter a Vault when it
    /// is created, so no phone can ever pair for it and Settings hides the
    /// Keychain tab. `false` while unknown, so the tab shows.
    pub vault_lacks_keychain_key: bool,
    /// Display name of the current Cube
    pub cube_name: String,
    /// How far the last rename on Settings › Display & appearance got:
    /// written on this device, then accepted by Connect.
    pub cube_name_sync: CubeNameSync,
    /// How far the last Spark nickname save got (Settings › Spark).
    pub spark_nickname_save: NameSave,
    /// The wallet names the side panel shows after "Spark" and "Vault".
    /// Synced from the Cube settings and the loaded Vault by the App.
    pub wallet_names: WalletNames,
    /// The Connect account that owns this Cube
    /// ([`crate::app::settings::CubeSettings::connect_owner`]). Lets the Vault
    /// signing picker tell a user signed in under a different account to switch
    /// accounts. `None` when unknown (Cube registered before owners were
    /// recorded, or never registered).
    pub cube_connect_owner: Option<crate::app::settings::ConnectOwner>,
    /// Connect refused to register this Cube because a different account
    /// owns it (`CUBE_OWNED_BY_ANOTHER_ACCOUNT`). Authoritative even when
    /// [`Self::cube_connect_owner`] is unknown. Mirrored from the Connect
    /// cube panel every tick.
    pub cube_owned_by_other_account: bool,
    /// Whether the user has completed the master seed backup flow for this
    /// Cube. Drives the soft "not backed up" warning banners on the Vault
    /// home screens. Mirrors `CubeSettings::backed_up`.
    pub current_cube_backed_up: bool,
    /// A Spark Stable Balance condition the user should know about,
    /// rendered as a banner at the top of every page by the `dashboard`
    /// wrapper. `None` when there is nothing to say or the user dismissed
    /// it for the session. Set by the App from bridge events and from
    /// the post-sync reconcile; cleared by dismissal or by the user
    /// changing the toggle.
    pub spark_notice: Option<SparkNotice>,
    /// Whether the P2P panel is available (requires a valid mnemonic)
    pub has_p2p: bool,
    /// Resolved P2P test-coordinator gate for [`Self::network`]: a
    /// test-network Mostro coordinator is configured, the network has a usable
    /// Lightning rail for escrow (effectively Regtest), *and* a Spark backend
    /// is actually connected (escrow is paid over Spark). Drives the P2P
    /// nav-rail gate via [`crate::app::features::p2p`]. Mirrored from the P2P
    /// panel (`P2PPanel::has_test_coordinator`), since the stateless sidebar
    /// view can't reach the panel.
    pub p2p_test_coordinator: bool,
    /// Server-controlled Marketplace availability, mirrored from the Connect
    /// account panel's `/connect/features` response (see
    /// `ConnectAccountPanel::marketplace_server_flags`). Drives whether the
    /// Marketplace nav + its routes are shown at all, on top of the per-network
    /// gate. Fail-closed: defaults to [`MarketplaceServerFlags::OFF`] until
    /// features load, so a launch build hides the untested money feature while
    /// the API stance is unknown.
    pub marketplace_flags: crate::app::features::MarketplaceServerFlags,
    /// Whether this Cube can start a Bitcoin Blake2b **claim** — mirrored
    /// from the Connect account alongside [`Self::marketplace_flags`].
    ///
    /// The account half only. The rest of the visibility matrix (this Cube is
    /// on Bitcoin mainnet, it has a Vault, and no claim target already reuses
    /// its descriptor) is answered where it is known; see
    /// [`crate::app::features::claim_blake2b`]. Fails closed until
    /// `/connect/features` answers, like every other server-controlled gate.
    pub btcb2_server_enabled: bool,
    /// Whether a Bitcoin Blake2b Cube on this device already reuses this
    /// Cube's Vault descriptor — i.e. this Cube has already been claimed.
    ///
    /// Resolved once at cube-open by [`crate::app::claim_target_exists`]: it
    /// is a fact about the disk, not about the session, and a Connect outage must not make an existing
    /// claim target look absent.
    pub btcb2_already_claimed: bool,
    /// Other-chain transaction presence for either side of the fork,
    /// keyed by the deposit's txid: whether the same transaction exists on
    /// the twin Bitcoin chain, and when that was resolved. Only *resolved*
    /// answers are stored — a deposit absent from the map has not been
    /// checked (or the check failed) and is retried after the next sync; a
    /// negative older than `NEGATIVE_ANSWER_TTL` is re-queried; a positive is
    /// terminal. In-memory only; absence is not replay-protection proof. Every
    /// lookup reply enters through [`Self::accept_entanglement`], every write
    /// through [`Self::record_entanglement`].
    pub entangled: std::collections::HashMap<
        coincube_core::miniscript::bitcoin::Txid,
        crate::services::entangled::CachedEntanglement,
    >,
    /// Ancestry check results for Vault coins, keyed by outpoint: what the
    /// Chains column calls *verified* or *couldn't verify*. Display only;
    /// loaded from and saved to the Vault's data directory
    /// ([`crate::services::chain_status::store`]). Every batch enters through
    /// [`Self::chain_proofs_landed`].
    pub chain_proofs: std::collections::HashMap<
        coincube_core::miniscript::bitcoin::OutPoint,
        crate::services::chain_status::ProofEntry,
    >,
    /// This Vault's split records ([`crate::services::split_coins::journal`]).
    pub split_records: Vec<crate::services::split_coins::SplitRecord>,
    /// Outputs of this Vault's splits still short of their confirmations.
    pub splitting: std::collections::HashMap<coincube_core::miniscript::bitcoin::OutPoint, u32>,
    /// Outputs of this Vault's signature splits past their confirmations,
    /// shown as split but unconfirmed.
    pub split_unconfirmed: std::collections::HashSet<coincube_core::miniscript::bitcoin::OutPoint>,
    /// Coins whose twin copy a split-key split on the twin Cube spent, as
    /// Connect last showed it deep on the twin chain, each with the instant
    /// that check stops counting. Read through
    /// [`crate::services::split_coins::is_separated`], so an expired check
    /// stops counting even if no refresh pass lands.
    pub separated:
        std::collections::HashMap<coincube_core::miniscript::bitcoin::OutPoint, std::time::Instant>,
    /// Current theme mode (dark/light) — used for theme-aware widget rendering
    pub theme_mode: coincube_ui::theme::palette::ThemeMode,
    /// The effective reduce-motion preference: the Connect light holds steady
    /// instead of pulsing.
    pub reduce_motion: bool,
    /// The pulsing Connect light's dim phase, flipped by a slow tick only
    /// while it pulses.
    pub connect_light_dim: bool,
    /// Progress of giving this Cube a Lightning address automatically (a
    /// random name such as `orangezebra@coincube.io`), mirrored from the
    /// Connect Cube panel for Receive.
    pub lightning_address_setup: LightningAddressSetup,
    /// Presentation input for the shared top-bar balance eye. `None` until the
    /// GUI preferences owner publishes a resolved or unresolved privacy state.
    pub balance_privacy: Option<crate::app::state::preferences::BalancePrivacyChrome>,
    /// Transient shell disclosures. These belong to this open Cube and are
    /// deliberately not persisted with wallet or device settings.
    pub cube_switcher_open: bool,
    /// The Connect control's panel is open.
    pub connect_panel_open: bool,
    pub cube_lock_confirmation: bool,
    /// BTC price in USD, always fetched regardless of the user's selected fiat
    /// currency. Used for converting USDt (which is pegged to USD) into sats.
    pub btc_usd_price: Option<f64>,
    pub(crate) btc_usd_requested_at: Option<Instant>,
    /// Whether to show direction badges (receive/spend arrows) on transaction rows.
    pub show_direction_badges: bool,
    /// Cached Lightning Address for display in the sidebar across all panels
    pub lightning_address: Option<String>,
    /// Cached avatar image handle for display in the sidebar across all panels
    /// Avatar regenerations the Connect account has left, when known.
    pub avatar_regenerations_remaining: Option<i32>,
    pub avatar_handle: Option<iced::widget::image::Handle>,
    /// Id of the current Cube.
    pub cube_id: String,
    /// Connect's numeric id for the active Cube (mirror of
    /// `ConnectCubePanel::server_cube_id`). `None` when the user isn't
    /// signed in to Connect or the cube hasn't been registered yet.
    /// Recovery-kit calls need this because the backend identifies
    /// cubes by numeric id, not by the local UUID carried in
    /// `cube_id` above.
    pub current_cube_server_id: Option<u64>,
    /// The active Cube's Connect-blinding encryption keypair
    /// (`SPEC-cube-xpub-envelope-v1` §3), derived from the master seed the
    /// Cube was unlocked with and held for the life of the session.
    ///
    /// Every surface that reads a Connect-served key — the Vault builder, the
    /// heir-escrow recipient list, the signer join — needs it to open xpub
    /// envelopes, and none of them has a PIN in hand. Deriving it once here
    /// from the already-in-memory master signer avoids re-prompting and adds no
    /// new secret lifetime: the seed it comes from is resident anyway (it backs
    /// the Spark signer). The private scalar is zeroized when the Arc
    /// drops and is never written to disk.
    ///
    /// `None` on a Cube with no on-disk seed (watch-only / descriptor-only
    /// restores, passkey Cubes). Blinded keys then resolve to
    /// `KeyResolveError::Locked` rather than being reported as invalid.
    pub cube_encryption_key: Option<Arc<crate::services::connect::crypto::CubeEncryptionKey>>,
    /// This device's Connect signing-rail transport key (`PLAN-connect-blinding`
    /// PR D4) — the key signers seal their partial signatures to, and the only
    /// thing that can open them.
    ///
    /// Loaded (minted on first use) from the network directory at launch,
    /// alongside the device identity it is registered with. Unlike
    /// [`Self::cube_encryption_key`] it is **not** seed-derived and does not
    /// need an unlocked Cube: the rail comes up at login.
    ///
    /// `None` only if the sidecar couldn't be read or written, in which case
    /// end-to-end signing is unavailable and sessions fail closed rather than
    /// downgrading to a plaintext rail.
    pub connect_transport_key: Option<Arc<crate::services::connect::crypto::DeviceTransportKey>>,
    /// SHA-256 hex fingerprint of the *live* descriptor blob — i.e.
    /// what `descriptor_blob_from_wallet(...)` currently produces
    /// given the loaded wallet. `None` when there's no wallet.
    /// Recomputed by `App` whenever the wallet changes.
    pub current_descriptor_fingerprint: Option<String>,
    /// SHA-256 hex fingerprint of the descriptor blob that was last
    /// successfully uploaded to Connect for this Cube. Mirrors
    /// `CubeSettings::recovery_kit_last_backed_up_descriptor_fingerprint`.
    /// The Settings card compares this to `current_descriptor_fingerprint`
    /// to surface the "your descriptor changed since your last backup"
    /// drift banner (W12). `None` when no descriptor has ever been
    /// backed up, or when the kit has been removed.
    pub recovery_kit_last_backed_up_descriptor_fingerprint: Option<String>,
    /// Keychain (phone) analogue of the field above: mirrors
    /// `CubeSettings::recovery_kit_last_backed_up_keychain_descriptor_fingerprint`.
    /// The Settings card compares it to `current_descriptor_fingerprint` to
    /// surface the phone copy's independent drift verdict (per-method drift,
    /// PR 3). `None` when never phone-sealed or after the kit is removed.
    pub recovery_kit_last_backed_up_keychain_descriptor_fingerprint: Option<String>,
    /// Mirrors `CubeSettings::recovery_kit_password_backed_up` — whether a
    /// password Recovery Kit is known to exist on Connect for this Cube.
    /// Unlike the two fingerprints above this is a *presence* signal, not a
    /// drift one, so it stays true for a seed-only kit on a Vault-less Cube.
    /// Held here so the Recovery-Kit status load can reconcile the persisted
    /// flag against the server without re-reading settings.json on every load.
    pub recovery_kit_password_backed_up: bool,
    /// Connect gRPC base URL, populated once `Message::ConnectStreamReady`
    /// fires after login. `None` until then or for local-daemon installs
    /// — the Keychain "Sign via Keychain" button stays disabled while
    /// this is `None`.
    pub connect_grpc_url: Option<String>,
    /// Shared `Arc<RwLock<AccessTokenResponse>>` for the Connect session,
    /// mirrored into Cache so deep panels (PsbtState et al.) can spin up
    /// a `GrpcSessionClient` on demand without re-plumbing the auth
    /// handle. `None` when no live or persisted Connect session is available.
    pub connect_tokens: Option<
        Arc<tokio::sync::RwLock<crate::services::connect::client::auth::AccessTokenResponse>>,
    >,
    /// Health state of the Connect realtime gRPC stream. Mirrored from
    /// `App::handle_connect_stream` so the sidebar can render a small
    /// status dot without re-plumbing through view function parameters.
    /// `Inactive` for local-daemon installs and any session before the
    /// first `Message::ConnectStreamReady` has fired.
    pub connect_stream_status: crate::app::ConnectionStatus,
    /// Set when Connect, not the user, signed this computer out. Cleared by
    /// the next sign-in or by Not now.
    pub connect_signed_out_by: Option<crate::app::SignedOutBy>,
    /// The Vault's alert while its recent polls fail (C8), recomputed by the
    /// App after every update.
    pub vault_alert: Option<crate::app::VaultAlert>,
    /// The Vault reads its blockchain data through COINCUBE | Connect.
    pub vault_via_connect: bool,
    /// `SignerDevice.id` returned by the API's `RegisterDevice` RPC.
    /// Surfaced on the Settings → About page so the user can confirm
    /// this desktop is registered, and to drive the "Re-register"
    /// affordance. Mirrored from the Connect cache on
    /// `Message::ConnectStreamReady`. `None` when the desktop hasn't
    /// registered yet or for local-daemon installs.
    pub connect_device_id: Option<String>,
    /// Email of the authenticated Connect account. Surfaced alongside
    /// the device id for at-a-glance "which account is this device
    /// registered to" troubleshooting. `None` when no live or persisted
    /// Connect session is available.
    pub connect_email: Option<String>,
}

/// only used for tests.
impl std::default::Default for Cache {
    fn default() -> Self {
        Self {
            app_generation: AppGeneration::next(),
            unswept_notice: None,
            connect_transport_key: None,
            cube_encryption_key: None,
            datadir_path: CoincubeDirectory::new(std::path::PathBuf::new()),
            node_bitcoind_sync_progress: None,
            node_bitcoind_sync_heights: None,
            node_bitcoind_ibd: None,
            node_bitcoind_subversion: None,
            node_bitcoind_pruning: None,
            local_switch_history: None,
            daemon_switch_in_progress: false,
            node_bitcoind_last_log: None,
            node_net_stats: None,
            node_history: super::node_history::Status::default(),
            node_history_switch_requested: false,
            local_node_transfer_visible: false,
            background_node_install: Default::default(),
            network: Network::Bitcoin,
            last_poll_at_startup: None,
            daemon_cache: DaemonCache::default(),
            fiat_price: None,
            fiat_chain: crate::chain::ChainId::Bitcoin,
            btcb2_price_request: None,
            bitcoin_unit: BitcoinDisplayUnit::default(),
            display_mode: DisplayMode::default(),
            connect_authenticated: false,
            sidebar_connect_signed_in: false,
            has_connect_session: false,
            restorable_connect_session: false,
            has_vault: false,
            has_imported_bitcoin: false,
            imported_stored_keys: None,
            imported_node: None,
            vault_lacks_keychain_key: false,
            cube_name: String::new(),
            cube_name_sync: CubeNameSync::Idle,
            spark_nickname_save: NameSave::Idle,
            wallet_names: WalletNames::default(),
            cube_connect_owner: None,
            cube_owned_by_other_account: false,
            current_cube_backed_up: false,
            spark_notice: None,
            has_p2p: false,
            p2p_test_coordinator: false,
            marketplace_flags: crate::app::features::MarketplaceServerFlags::OFF,
            btcb2_server_enabled: false,
            btcb2_already_claimed: false,
            entangled: std::collections::HashMap::new(),
            chain_proofs: std::collections::HashMap::new(),
            split_records: Vec::new(),
            splitting: std::collections::HashMap::new(),
            split_unconfirmed: std::collections::HashSet::new(),
            separated: std::collections::HashMap::new(),
            theme_mode: coincube_ui::theme::palette::ThemeMode::default(),
            reduce_motion: false,
            connect_light_dim: false,
            lightning_address_setup: LightningAddressSetup::Idle,
            balance_privacy: None,
            cube_switcher_open: false,
            connect_panel_open: false,
            cube_lock_confirmation: false,
            btc_usd_price: None,
            btc_usd_requested_at: None,
            show_direction_badges: true,
            lightning_address: None,
            avatar_handle: None,
            avatar_regenerations_remaining: None,
            cube_id: String::new(),
            current_cube_server_id: None,
            current_descriptor_fingerprint: None,
            recovery_kit_last_backed_up_descriptor_fingerprint: None,
            recovery_kit_last_backed_up_keychain_descriptor_fingerprint: None,
            recovery_kit_password_backed_up: false,
            connect_grpc_url: None,
            connect_tokens: None,
            connect_stream_status: crate::app::ConnectionStatus::default(),
            connect_signed_out_by: None,
            vault_alert: None,
            vault_via_connect: false,
            connect_device_id: None,
            connect_email: None,
        }
    }
}

impl Cache {
    /// A Vault with recovery paths (inheritance, recovery alerts), not an
    /// imported Vault.
    pub fn has_inheritance_vault(&self) -> bool {
        self.has_vault && !self.has_imported_bitcoin
    }

    /// Passive amounts are visible only after the GUI has published an
    /// accepted installation-wide Show receipt. Missing or unresolved state
    /// always masks, including the first frame of a newly opened Cube.
    pub fn balance_amounts_masked(&self) -> bool {
        self.balance_privacy
            .is_none_or(|chrome| chrome.privacy.masks_amounts())
    }

    /// Retain the Cube identity across storage and recovery boundaries.
    pub fn chain(&self) -> crate::chain::ChainId {
        self.fiat_chain
    }

    /// The cached entanglement answer for a deposit, [`Entanglement::Unknown`]
    /// when none has been resolved yet. A stale negative still reads as
    /// *not entangled* here — the refresh is the sync task's job and the
    /// spend screen's re-check, not this accessor's.
    pub fn entanglement_of(
        &self,
        txid: &coincube_core::miniscript::bitcoin::Txid,
    ) -> crate::services::entangled::Entanglement {
        self.entangled
            .get(txid)
            .map(|cached| cached.answer)
            .unwrap_or(crate::services::entangled::Entanglement::Unknown)
    }

    /// Record a lookup answer observed at `observed_at`. `Unknown` is never
    /// stored. `Entangled` is terminal: it replaces any negative whatever the
    /// instants say, and is never overwritten. A negative is monotonic in its
    /// observation instant: it replaces an existing negative only if it was
    /// observed strictly later, so an older observation that lands later —
    /// the first answer of a slow batch, a reply from a screen since closed —
    /// cannot re-stamp a newer one and extend its `NEGATIVE_ANSWER_TTL`
    /// (`#395`). Returns whether the cache changed.
    pub fn record_entanglement(
        &mut self,
        txid: coincube_core::miniscript::bitcoin::Txid,
        answer: crate::services::entangled::Entanglement,
        observed_at: std::time::Instant,
    ) -> bool {
        use crate::services::entangled::{CachedEntanglement, Entanglement};
        if !answer.is_resolved() {
            return false;
        }
        if let Some(existing) = self.entangled.get(&txid) {
            if matches!(existing.answer, Entanglement::Entangled) {
                return false;
            }
            if !matches!(answer, Entanglement::Entangled) && observed_at <= existing.resolved_at {
                return false;
            }
        }
        self.entangled.insert(
            txid,
            CachedEntanglement {
                answer,
                resolved_at: observed_at,
            },
        );
        true
    }

    /// Apply a lookup reply to this Cube's cache — the one entry point for
    /// every reply, whichever message carried it (`#393`).
    ///
    /// `origin` is where the lookup was issued; `chain` is this Cube's chain
    /// (`None` without a Vault). A reply from this `App` instance is applied
    /// in full. A reply from another instance — a task that outlived a Cube
    /// switch — is that Cube's, not this one's: its negatives are dropped
    /// (this instance asks about its own deposits after its next sync, and a
    /// stale negative would only pre-empt that), while a terminal positive
    /// for the same chain is still recorded, because a transaction on the
    /// twin chain is there regardless of which Cube asked. A different chain
    /// says nothing about this Cube's deposits and is dropped whole, which
    /// also keeps a Bitcoin-family Cube's cache empty. Every write goes
    /// through [`Self::record_entanglement`], so nothing here can re-stamp a
    /// newer negative. Returns whether the cache changed.
    pub fn accept_entanglement(
        &mut self,
        origin: LookupOrigin,
        chain: Option<crate::chain::ChainId>,
        answers: &[crate::services::entangled::LookupAnswer],
    ) -> bool {
        use crate::services::entangled::Entanglement;
        let own = self.issued_here(origin);
        let same_chain = Some(origin.chain) == chain;
        answers.iter().fold(false, |changed, reply| {
            let admitted = own || (same_chain && matches!(reply.answer, Entanglement::Entangled));
            (admitted && self.record_entanglement(reply.txid, reply.answer, reply.observed_at))
                || changed
        })
    }

    /// Whether a lookup was issued by the `App` instance this cache belongs
    /// to — as opposed to a predecessor whose task outlived a Cube switch.
    pub fn issued_here(&self, origin: LookupOrigin) -> bool {
        origin.app == self.app_generation
    }

    /// A sync-driven lookup batch landed (`Message::EntangledLookups`).
    /// `in_flight` is the receiving `App`'s single-flight claim set: a batch
    /// issued here releases its whole claim first — by claim, not by reply,
    /// so a txid that answered `Unknown` (never cached) is asked again next
    /// sync instead of staying in flight forever. A batch from a predecessor
    /// instance claimed on *that* instance and releases nothing here, where
    /// the same txids may be legitimately in flight for this instance's own
    /// batch. Its answers then go through [`Self::accept_entanglement`].
    /// Returns whether the cache changed.
    pub fn entangled_batch_landed(
        &mut self,
        in_flight: &mut std::collections::HashSet<coincube_core::miniscript::bitcoin::Txid>,
        chain: Option<crate::chain::ChainId>,
        origin: LookupOrigin,
        claimed: &[coincube_core::miniscript::bitcoin::Txid],
        answers: &[crate::services::entangled::LookupAnswer],
    ) -> bool {
        if self.issued_here(origin) {
            for txid in claimed {
                in_flight.remove(txid);
            }
        }
        self.accept_entanglement(origin, chain, answers)
    }

    /// The facts the Chains column has about one Vault coin.
    pub fn chain_evidence(
        &self,
        outpoint: &coincube_core::miniscript::bitcoin::OutPoint,
    ) -> crate::services::chain_status::CoinEvidence {
        self.chain_evidence_at(outpoint, std::time::Instant::now())
    }

    /// [`Self::chain_evidence`] as of `now`.
    pub fn chain_evidence_at(
        &self,
        outpoint: &coincube_core::miniscript::bitcoin::OutPoint,
        now: std::time::Instant,
    ) -> crate::services::chain_status::CoinEvidence {
        crate::services::chain_status::CoinEvidence {
            entanglement: self
                .entangled
                .get(&outpoint.txid)
                .map(|cached| cached.answer),
            proof: self
                .chain_proofs
                .get(outpoint)
                .and_then(|entry| entry.outcome()),
            splitting: self.splitting.get(outpoint).copied(),
            signature_split: self.split_unconfirmed.contains(outpoint),
            separated: crate::services::split_coins::is_separated(&self.separated, outpoint, now),
        }
    }

    /// The Chains column status of one Vault coin.
    pub fn chain_status(
        &self,
        outpoint: &coincube_core::miniscript::bitcoin::OutPoint,
    ) -> crate::services::chain_status::ChainStatus {
        crate::services::chain_status::classify(self.chain_evidence(outpoint))
    }

    /// Vault coins that can serve as a split key: unspent, with a verified
    /// ancestry path, and not spent by a pending split. A coin shown as
    /// separated by the twin Cube's split is not one: its twin copy is gone
    /// but it still descends from a both-chain coin. The split builder
    /// re-qualifies the chosen key before anything is built.
    pub fn split_keys(
        &self,
    ) -> Vec<(
        &Coin,
        &coincube_core::claim_ancestry::retained::RetainedPath,
    )> {
        let pending: std::collections::HashSet<_> = self
            .split_records
            .iter()
            .filter(|record| record.confirmations == 0)
            .flat_map(|record| record.inputs.iter().chain(record.key.iter()).copied())
            .collect();
        let mut keys: Vec<_> = self
            .coins()
            .iter()
            .filter(|coin| {
                coin.spend_info.is_none() && !coin.is_immature && coin.block_height.is_some()
            })
            .filter(|coin| !pending.contains(&coin.outpoint))
            .filter_map(|coin| match self.chain_proofs.get(&coin.outpoint) {
                Some(crate::services::chain_status::ProofEntry::Verified(path))
                    if self.entanglement_of(&coin.outpoint.txid)
                        != crate::services::entangled::Entanglement::Entangled =>
                {
                    Some((coin, path))
                }
                _ => None,
            })
            .collect();
        // The smallest key wastes the least when it is spent with the coins.
        keys.sort_by_key(|(coin, _)| (coin.amount, coin.outpoint));
        keys
    }

    /// The proof to re-qualify for `key` as a split key of the spend `spend`:
    /// the path a check verified in this session, else the one the split's
    /// journal record was built with (a restart leaves only a
    /// [`crate::services::chain_status::ProofEntry::Saved`] copy). Neither
    /// counts by itself; the PSBT screen re-qualifies it against both chains.
    pub fn split_key_path(
        &self,
        spend: coincube_core::miniscript::bitcoin::Txid,
        key: coincube_core::miniscript::bitcoin::OutPoint,
    ) -> Option<coincube_core::claim_ancestry::retained::RetainedPath> {
        use coincube_core::claim_ancestry::retained::RetainedPath;
        if let Some(crate::services::chain_status::ProofEntry::Verified(path)) =
            self.chain_proofs.get(&key)
        {
            return Some(path.clone());
        }
        self.split_records
            .iter()
            .find(|record| record.txid == spend && record.key == Some(key))
            .and_then(|record| hex::decode(record.key_path.as_ref()?).ok())
            .and_then(|bytes| RetainedPath::decode(key, &bytes).ok())
    }

    /// Whether [`Self::split_key_path`] has a proof to offer, without
    /// copying or decoding it (the PSBT screen asks on every render).
    pub fn has_split_key_path(
        &self,
        spend: coincube_core::miniscript::bitcoin::Txid,
        key: coincube_core::miniscript::bitcoin::OutPoint,
    ) -> bool {
        matches!(
            self.chain_proofs.get(&key),
            Some(crate::services::chain_status::ProofEntry::Verified(_))
        ) || self.split_records.iter().any(|record| {
            record.txid == spend && record.key == Some(key) && record.key_path.is_some()
        })
    }

    /// The split key of `spend` if this Cube's journal records it as a
    /// split-key split.
    pub fn recorded_split_key(
        &self,
        spend: coincube_core::miniscript::bitcoin::Txid,
    ) -> Option<coincube_core::miniscript::bitcoin::OutPoint> {
        self.split_records
            .iter()
            .find(|record| record.txid == spend)
            .and_then(|record| record.key)
    }

    /// An ancestry check batch landed. Like [`Self::entangled_batch_landed`],
    /// a batch issued by this `App` instance releases its whole claim first.
    /// Unlike entanglement, a result is about one Vault's coin, so a batch
    /// from a predecessor instance is dropped whole. A verified path is never
    /// replaced by a later miss or failure. Returns whether the cache changed.
    pub fn chain_proofs_landed(
        &mut self,
        in_flight: &mut std::collections::HashSet<coincube_core::miniscript::bitcoin::OutPoint>,
        origin: LookupOrigin,
        claimed: &[coincube_core::miniscript::bitcoin::OutPoint],
        results: Vec<(
            coincube_core::miniscript::bitcoin::OutPoint,
            crate::services::chain_status::ProofEntry,
        )>,
    ) -> bool {
        use crate::services::chain_status::ProofEntry;
        if !self.issued_here(origin) {
            return false;
        }
        for outpoint in claimed {
            in_flight.remove(outpoint);
        }
        let mut changed = false;
        for (outpoint, entry) in results {
            let previous = self.chain_proofs.get(&outpoint);
            if matches!(previous, Some(ProofEntry::Verified(_))) {
                continue;
            }
            // A check that couldn't finish never drops a saved path.
            let entry = ProofEntry::after_check(previous, entry);
            self.chain_proofs.insert(outpoint, entry);
            changed = true;
        }
        changed
    }

    pub fn blockheight(&self) -> i32 {
        self.daemon_cache.blockheight
    }

    pub fn coins(&self) -> &[Coin] {
        &self.daemon_cache.coins
    }

    pub fn rescan_progress(&self) -> Option<f64> {
        self.daemon_cache.rescan_progress
    }

    pub fn sync_progress(&self) -> f64 {
        self.daemon_cache.sync_progress
    }

    pub fn last_poll_timestamp(&self) -> Option<u32> {
        self.daemon_cache.last_poll_timestamp
    }

    pub fn history_sync(&self) -> &coincubed::commands::HistorySync {
        &self.daemon_cache.history_sync
    }
}

#[cfg(test)]
mod balance_privacy_tests {
    use super::Cache;
    use crate::app::state::preferences::{BalancePrivacy, BalancePrivacyChrome};

    #[test]
    fn passive_amounts_fail_closed_until_an_accepted_show_is_published() {
        let mut cache = Cache::default();
        assert!(cache.balance_amounts_masked());
        for privacy in [BalancePrivacy::Unresolved, BalancePrivacy::Hidden] {
            cache.balance_privacy = Some(BalancePrivacyChrome {
                privacy,
                busy: false,
                interactive: false,
            });
            assert!(cache.balance_amounts_masked());
        }
        cache.balance_privacy = Some(BalancePrivacyChrome {
            privacy: BalancePrivacy::Shown,
            busy: false,
            interactive: false,
        });
        assert!(!cache.balance_amounts_masked());
    }
}

/// The cache for dynamic daemon data.
#[derive(Debug, Clone)]
pub struct DaemonCache {
    pub blockheight: i32,
    pub coins: Vec<Coin>,
    pub rescan_progress: Option<f64>,
    pub sync_progress: f64,
    /// The most recent `last_poll_timestamp`.
    pub last_poll_timestamp: Option<u32>,
    /// The daemon's latest report of its full scans and failed polls.
    pub history_sync: coincubed::commands::HistorySync,
    pub last_tick: std::time::Instant,
}

/// only used for tests.
impl std::default::Default for DaemonCache {
    fn default() -> Self {
        Self {
            blockheight: 0,
            coins: Vec::new(),
            rescan_progress: None,
            sync_progress: 1.0,
            last_poll_timestamp: None,
            history_sync: Default::default(),
            last_tick: Instant::now(),
        }
    }
}

/// Get the coins that should be cached.
pub async fn coins_to_cache(
    daemon: Arc<dyn Daemon + Sync + Send>,
) -> Result<ListCoinsResult, DaemonError> {
    daemon
        .list_coins(&[CoinStatus::Unconfirmed, CoinStatus::Confirmed], &[])
        .await
}

/// Represents a fiat price fetched from the API together with the request that was used to fetch it.
#[derive(Debug, Clone)]
pub struct FiatPrice {
    pub res: Result<GetPriceResult, PriceApiError>, // also store error in case we want to display it to user
    pub request: FiatPriceRequest,
    /// BTCB2's earliest market or conversion expiry; absent for Bitcoin prices.
    pub btcb2_valid_until: Option<u64>,
}

impl FiatPrice {
    pub fn source(&self) -> PriceSource {
        self.request.source
    }

    pub fn currency(&self) -> Currency {
        self.request.currency
    }

    pub fn requested_at(&self) -> Instant {
        self.request.instant
    }
}

/// Represents a fiat price request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FiatPriceRequest {
    pub source: PriceSource,
    pub currency: Currency,
    pub instant: Instant,
    /// None is an existing globally shared Bitcoin price; BTCB2 is App-scoped.
    pub origin: Option<LookupOrigin>,
}

impl FiatPriceRequest {
    pub fn new(source: PriceSource, currency: Currency) -> Self {
        Self {
            source,
            currency,
            instant: Instant::now(),
            origin: None,
        }
    }

    pub fn for_btcb2(currency: Currency, origin: LookupOrigin) -> Self {
        Self {
            source: PriceSource::Coincube,
            currency,
            instant: Instant::now(),
            origin: Some(origin),
        }
    }

    pub async fn send_connect(
        self,
        client: crate::services::coincube::CoincubeClient,
    ) -> FiatPrice {
        use crate::services::fiat::btcb2;
        let (res, btcb2_valid_until) = if self
            .origin
            .is_some_and(|o| o.chain == crate::chain::ChainId::BitcoinBlake2b)
        {
            match client.btcb2_quote(self.currency).await {
                Ok(quote) => {
                    let res = btcb2::unix_now()
                        .ok_or_else(|| PriceApiError::CannotParseData("Clock unavailable".into()))
                        .and_then(|now| quote.usable_price(self.currency, now));
                    let valid_until = res.as_ref().ok().and_then(|_| quote.valid_until());
                    (res, valid_until)
                }
                Err(error) => (Err(btcb2::request_error(error)), None),
            }
        } else {
            (
                Err(PriceApiError::CannotParseData(
                    "BTCB2 pricing is unavailable for this chain".into(),
                )),
                None,
            )
        };
        FiatPrice {
            res,
            request: self,
            btcb2_valid_until,
        }
    }

    /// Sends the request using the default client for the given source.
    pub async fn send_default(self) -> FiatPrice {
        if self.origin.is_some() {
            return FiatPrice {
                res: Err(PriceApiError::CannotParseData(
                    "Scoped prices require Connect".into(),
                )),
                request: self,
                btcb2_valid_until: None,
            };
        }
        let client = PriceClient::default_from_source(self.source);
        FiatPrice {
            res: client.get_price(self.currency).await,
            request: self,
            btcb2_valid_until: None,
        }
    }
}

#[cfg(test)]
mod entangled_cache_tests {
    use super::*;
    use crate::chain::ChainId;
    use crate::services::entangled::{Entanglement, LookupAnswer, NEGATIVE_ANSWER_TTL};
    use coincube_core::miniscript::bitcoin::Txid;
    use std::str::FromStr;
    use std::time::Duration;

    fn txid(b: u8) -> Txid {
        Txid::from_str(&format!("{:0>64}", b)).unwrap()
    }

    fn observed(txid: Txid, answer: Entanglement, observed_at: Instant) -> LookupAnswer {
        LookupAnswer {
            txid,
            answer,
            observed_at,
        }
    }

    #[test]
    fn positives_are_terminal_negatives_are_replaceable_unknown_is_never_stored() {
        let txid = txid(9);
        let now = Instant::now();
        let mut cache = Cache::default();
        assert_eq!(cache.entanglement_of(&txid), Entanglement::Unknown);

        assert!(!cache.record_entanglement(txid, Entanglement::Unknown, now));
        assert!(cache.entangled.is_empty());

        assert!(cache.record_entanglement(txid, Entanglement::NotEntangled, now));
        assert_eq!(cache.entanglement_of(&txid), Entanglement::NotEntangled);
        assert_eq!(cache.entangled[&txid].resolved_at, now);

        // A later positive replaces the negative…
        let later = now + Duration::from_secs(10);
        assert!(cache.record_entanglement(txid, Entanglement::Entangled, later));
        assert_eq!(cache.entanglement_of(&txid), Entanglement::Entangled);
        // …and is terminal: neither a negative nor Unknown moves it.
        assert!(!cache.record_entanglement(txid, Entanglement::NotEntangled, later));
        assert!(!cache.record_entanglement(txid, Entanglement::Unknown, later));
        assert_eq!(cache.entanglement_of(&txid), Entanglement::Entangled);
        assert_eq!(cache.entangled[&txid].resolved_at, later);
    }

    /// `#395`: negatives are ordered by observation, not by arrival. A stale
    /// `NotEntangled` landing after a newer one leaves the newer instant in
    /// place — so the next refresh is due when the *newer* observation ages
    /// out, not `NEGATIVE_ANSWER_TTL` after the stale one was processed. A
    /// positive replaces a negative whatever the instants say.
    #[test]
    fn a_stale_negative_cannot_re_stamp_a_newer_one() {
        let txid = txid(9);
        let older = Instant::now();
        let newer = older + Duration::from_secs(30);
        let mut cache = Cache::default();

        assert!(cache.record_entanglement(txid, Entanglement::NotEntangled, newer));
        assert!(
            !cache.record_entanglement(txid, Entanglement::NotEntangled, older),
            "an older observation does not replace a newer one"
        );
        assert_eq!(cache.entangled[&txid].resolved_at, newer);
        assert!(
            !cache.record_entanglement(txid, Entanglement::NotEntangled, newer),
            "the same observation again is not a change"
        );
        // The refresh is due relative to the newer observation, not the
        // stale one's arrival.
        let refresh_due = newer + NEGATIVE_ANSWER_TTL;
        assert!(cache.entangled[&txid].needs_refresh(refresh_due));
        assert!(!cache.entangled[&txid].needs_refresh(refresh_due - Duration::from_secs(1)));

        // A strictly newer negative does move it.
        let newest = newer + Duration::from_secs(1);
        assert!(cache.record_entanglement(txid, Entanglement::NotEntangled, newest));
        assert_eq!(cache.entangled[&txid].resolved_at, newest);

        // An older positive still wins: Entangled is terminal truth.
        assert!(cache.record_entanglement(txid, Entanglement::Entangled, older));
        assert_eq!(cache.entanglement_of(&txid), Entanglement::Entangled);
        assert_eq!(cache.entangled[&txid].resolved_at, older);
    }

    /// A Chains-column ancestry batch: one from another `App` instance is
    /// dropped whole and releases nothing; this instance's own batch releases
    /// its claim and records its results.
    #[test]
    fn chain_proof_batches_are_scoped_to_their_app_instance() {
        use crate::services::chain_status::{ChainStatus, ProofEntry};
        use coincube_core::miniscript::bitcoin::OutPoint;
        let a = Cache::default();
        let mut b = Cache::default();
        let outpoint = OutPoint::new(txid(4), 0);
        let mut in_flight: std::collections::HashSet<OutPoint> = [outpoint].into();
        let checked = ProofEntry::NoQualifyingAncestry {
            checked_at: std::time::SystemTime::now(),
        };
        let from = |cache: &Cache| LookupOrigin {
            app: cache.app_generation,
            chain: ChainId::Bitcoin,
        };
        assert!(!b.chain_proofs_landed(
            &mut in_flight,
            from(&a),
            &[outpoint],
            vec![(outpoint, checked.clone())],
        ));
        assert!(in_flight.contains(&outpoint));
        assert!(b.chain_proofs.is_empty());

        b.record_entanglement(outpoint.txid, Entanglement::NotEntangled, Instant::now());
        assert_eq!(b.chain_status(&outpoint), ChainStatus::NotSeen);
        assert!(b.chain_proofs_landed(
            &mut in_flight,
            from(&b),
            &[outpoint],
            vec![(outpoint, checked)],
        ));
        assert!(in_flight.is_empty());
        assert_eq!(b.chain_status(&outpoint), ChainStatus::CouldntVerify);
    }

    /// Only a coin with a verified ancestry path is a split key; a coin the
    /// twin Cube's split separated shows as one-chain but is not a key.
    #[test]
    fn split_keys_need_a_verified_path_not_a_separated_status() {
        use crate::services::chain_status::{ChainStatus, ProofEntry};
        use coincube_core::{
            claim_ancestry::{retained::RetainedPath, search::OwnedLink},
            miniscript::bitcoin::{
                absolute, bip32::ChildNumber, consensus::serialize, script::Builder, transaction,
                Address, Amount, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Witness,
            },
        };
        use std::str::FromStr;
        let coinbase = Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::null(),
                script_sig: Builder::new().push_int(962_000).push_int(1).into_script(),
                sequence: Sequence::MAX,
                witness: Witness::new(),
            }],
            output: vec![TxOut {
                value: Amount::from_sat(30_000),
                script_pubkey: ScriptBuf::new(),
            }],
        };
        let key = OutPoint::new(coinbase.compute_txid(), 0);
        let separated = OutPoint::new(txid(5), 0);
        let coin = |outpoint| Coin {
            amount: Amount::from_sat(30_000),
            outpoint,
            address: Address::from_str("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4")
                .unwrap()
                .assume_checked(),
            block_height: Some(100),
            derivation_index: ChildNumber::from_normal_idx(0).unwrap(),
            spend_info: None,
            is_immature: false,
            is_change: false,
            is_from_self: false,
        };
        let mut cache = Cache::default();
        cache.daemon_cache.coins = vec![coin(key), coin(separated)];
        cache.chain_proofs.insert(
            key,
            ProofEntry::Verified(
                RetainedPath::new(
                    key,
                    vec![OwnedLink {
                        transaction: serialize(&coinbase),
                        parent_input: None,
                    }],
                )
                .unwrap(),
            ),
        );
        let until = std::time::Instant::now() + std::time::Duration::from_secs(600);
        cache.separated.insert(separated, until);
        assert_eq!(cache.chain_status(&separated), ChainStatus::OnlyHere);
        assert_ne!(
            crate::services::chain_status::classify(
                cache.chain_evidence_at(&separated, until + std::time::Duration::from_secs(1))
            ),
            ChainStatus::OnlyHere,
            "an expired twin check stops showing the coin as one-chain without a refresh"
        );
        let keys: Vec<_> = cache.split_keys().iter().map(|(c, _)| c.outpoint).collect();
        assert_eq!(keys, vec![key]);
    }

    /// `#393`: a reply issued by App instance A, arriving after the tab has
    /// moved to instance B, does not put A's negatives into B's cache; B's
    /// own reply still applies in full; a terminal positive for the same
    /// chain is not lost to the scoping; a different chain's reply is dropped
    /// whole, positives included.
    #[test]
    fn a_reply_from_another_app_instance_only_contributes_same_chain_positives() {
        let now = Instant::now();
        let a = Cache::default();
        let mut b = Cache::default();
        assert_ne!(a.app_generation, b.app_generation, "never reused");
        let chain = ChainId::BitcoinBlake2b;
        let from_a = LookupOrigin {
            app: a.app_generation,
            chain,
        };
        let from_b = LookupOrigin {
            app: b.app_generation,
            chain,
        };

        // A's negative and Unknown never reach B's cache; A's positive does.
        let changed = b.accept_entanglement(
            from_a,
            Some(chain),
            &[
                observed(txid(1), Entanglement::NotEntangled, now),
                observed(txid(2), Entanglement::Unknown, now),
                observed(txid(3), Entanglement::Entangled, now),
            ],
        );
        assert!(changed, "the positive is a change");
        assert_eq!(
            b.entanglement_of(&txid(1)),
            Entanglement::Unknown,
            "A's negative never reaches B's cache"
        );
        assert_eq!(b.entanglement_of(&txid(2)), Entanglement::Unknown);
        assert_eq!(
            b.entanglement_of(&txid(3)),
            Entanglement::Entangled,
            "A's terminal positive is not lost to the scoping"
        );
        assert_eq!(b.entangled.len(), 1);
        assert!(
            !b.accept_entanglement(
                from_a,
                Some(chain),
                &[observed(txid(1), Entanglement::NotEntangled, now)]
            ),
            "a stale instance's negative alone is no change"
        );

        // B's own reply applies in full.
        assert!(b.accept_entanglement(
            from_b,
            Some(chain),
            &[
                observed(txid(1), Entanglement::NotEntangled, now),
                observed(txid(2), Entanglement::Unknown, now),
            ],
        ));
        assert_eq!(b.entanglement_of(&txid(1)), Entanglement::NotEntangled);
        assert_eq!(b.entangled[&txid(1)].resolved_at, now);
        assert_eq!(b.entanglement_of(&txid(2)), Entanglement::Unknown);

        // A's stale negative, observed earlier, cannot re-stamp B's — nor,
        // observed later, replace it: it is not B's lookup at all.
        for at in [now - Duration::from_secs(5), now + Duration::from_secs(5)] {
            assert!(!b.accept_entanglement(
                from_a,
                Some(chain),
                &[observed(txid(1), Entanglement::NotEntangled, at)]
            ));
            assert_eq!(b.entangled[&txid(1)].resolved_at, now);
        }

        // Another chain's reply — a BTCB2 testnet Cube's lookup landing on a
        // mainnet Cube, or on a Bitcoin-family Cube — is dropped whole.
        let mut bitcoin_cube = Cache::default();
        for (cube_chain, reply_chain) in [
            (
                Some(ChainId::BitcoinBlake2b),
                ChainId::BitcoinBlake2bTestnet4,
            ),
            (Some(ChainId::Bitcoin), ChainId::BitcoinBlake2b),
            (None, ChainId::BitcoinBlake2b),
        ] {
            assert!(!bitcoin_cube.accept_entanglement(
                LookupOrigin {
                    app: a.app_generation,
                    chain: reply_chain,
                },
                cube_chain,
                &[
                    observed(txid(3), Entanglement::Entangled, now),
                    observed(txid(4), Entanglement::NotEntangled, now),
                ],
            ));
            assert!(bitcoin_cube.entangled.is_empty(), "{:?}", cube_chain);
        }
    }

    /// `#395`, the scenario as filed: a sync batch started before the spend
    /// screen's re-check lands after it. The batch's answer for the shared
    /// deposit was observed *before* the screen's, so it does not re-stamp
    /// the screen's `resolved_at`, and the next refresh is still due
    /// `NEGATIVE_ANSWER_TTL` after the screen's observation — not after the
    /// batch was processed.
    #[test]
    fn a_batch_observed_before_a_screen_negative_does_not_re_stamp_it() {
        use std::collections::HashSet;
        let chain = ChainId::BitcoinBlake2b;
        let mut cache = Cache::default();
        let origin = LookupOrigin {
            app: cache.app_generation,
            chain,
        };
        let mut in_flight: HashSet<Txid> = HashSet::from([txid(1), txid(2)]);

        // The batch asks about 1 first (observed at t0), then 2 (slow).
        let t0 = Instant::now();
        // Meanwhile the spend screen asks about 1 and gets its answer at t1.
        let t1 = t0 + Duration::from_secs(20);
        assert!(cache.accept_entanglement(
            origin,
            Some(chain),
            &[observed(txid(1), Entanglement::NotEntangled, t1)]
        ));
        // The batch lands at t2, carrying 1's answer observed at t0.
        let t2 = t1 + Duration::from_secs(40);
        let changed = cache.entangled_batch_landed(
            &mut in_flight,
            Some(chain),
            origin,
            &[txid(1), txid(2)],
            &[
                observed(txid(1), Entanglement::NotEntangled, t0),
                observed(txid(2), Entanglement::NotEntangled, t2),
            ],
        );
        assert!(changed, "2 is new");
        assert!(in_flight.is_empty());
        assert_eq!(
            cache.entangled[&txid(1)].resolved_at,
            t1,
            "the screen's newer observation stands"
        );
        assert_eq!(cache.entangled[&txid(2)].resolved_at, t2);
        assert!(
            cache.entangled[&txid(1)].needs_refresh(t1 + NEGATIVE_ANSWER_TTL),
            "refresh is due relative to the screen's observation"
        );
        assert!(!cache.entangled[&txid(1)]
            .needs_refresh(t1 + NEGATIVE_ANSWER_TTL - Duration::from_secs(1)));
    }

    /// `#393`, the sync-driven variant: a batch issued here releases its
    /// claim as a whole (`Unknown` included) and is applied; a predecessor
    /// instance's batch releases nothing on this instance — its `claimed`
    /// may name txids this instance has in flight for its own batch — and
    /// contributes only same-chain positives.
    #[test]
    fn a_batch_from_another_app_instance_releases_no_claim_here() {
        use std::collections::HashSet;
        let now = Instant::now();
        let chain = ChainId::BitcoinBlake2b;
        let a = Cache::default();
        let mut b = Cache::default();
        let from_a = LookupOrigin {
            app: a.app_generation,
            chain,
        };
        let from_b = LookupOrigin {
            app: b.app_generation,
            chain,
        };
        // B has its own batch in flight on 1 and 2.
        let mut in_flight: HashSet<Txid> = HashSet::from([txid(1), txid(2)]);

        // A's batch, claimed on A, lands on B: B's claim is untouched, A's
        // negative for 1 is dropped, A's positive for 3 is kept.
        let changed = b.entangled_batch_landed(
            &mut in_flight,
            Some(chain),
            from_a,
            &[txid(1), txid(2), txid(3)],
            &[
                observed(txid(1), Entanglement::NotEntangled, now),
                observed(txid(2), Entanglement::Unknown, now),
                observed(txid(3), Entanglement::Entangled, now),
            ],
        );
        assert!(changed);
        assert_eq!(
            in_flight,
            HashSet::from([txid(1), txid(2)]),
            "claim untouched"
        );
        assert_eq!(
            b.entanglement_of(&txid(1)),
            Entanglement::Unknown,
            "A's negative never reaches B's cache"
        );
        assert_eq!(
            b.entanglement_of(&txid(3)),
            Entanglement::Entangled,
            "A's terminal positive is kept"
        );

        // B's own batch: the whole claim is released, `Unknown` included,
        // and every resolved answer is recorded.
        let changed = b.entangled_batch_landed(
            &mut in_flight,
            Some(chain),
            from_b,
            &[txid(1), txid(2)],
            &[
                observed(txid(1), Entanglement::NotEntangled, now),
                observed(txid(2), Entanglement::Unknown, now),
            ],
        );
        assert!(changed);
        assert!(in_flight.is_empty(), "released by claim, not by reply");
        assert_eq!(b.entanglement_of(&txid(1)), Entanglement::NotEntangled);
        assert_eq!(b.entanglement_of(&txid(2)), Entanglement::Unknown);
    }
}

impl Cache {
    pub fn fiat_toggle_allowed(&self) -> bool {
        !self.fiat_chain.is_blake2b() || self.btcb2_price_usable()
    }

    pub fn btcb2_price_usable(&self) -> bool {
        self.fiat_chain == crate::chain::ChainId::BitcoinBlake2b
            && self.fiat_price.as_ref().is_some_and(|price| {
                price.requested_at().elapsed().as_secs()
                    <= crate::services::fiat::btcb2::MAX_QUOTE_AGE
                    && price.source() == PriceSource::Coincube
                    && price.request.origin
                        == Some(LookupOrigin {
                            app: self.app_generation,
                            chain: self.fiat_chain,
                        })
                    && price.res.as_ref().is_ok_and(|result| {
                        result.value.is_finite()
                            && result.value > 0.0
                            && result
                                .updated_at
                                .zip(crate::services::fiat::btcb2::unix_now())
                                .is_some_and(|(at, now)| {
                                    crate::services::fiat::btcb2::timestamp_fresh(at, now)
                                        && price.btcb2_valid_until.is_some_and(|until| now <= until)
                                })
                    })
            })
    }

    pub fn clear_btcb2_fiat(&mut self) -> bool {
        let changed = self.fiat_price.is_some()
            || self.btc_usd_price.is_some()
            || self.display_mode != DisplayMode::BitcoinNative;
        self.fiat_price = None;
        self.btc_usd_price = None;
        self.btc_usd_requested_at = None;
        self.display_mode = DisplayMode::BitcoinNative;
        changed
    }

    pub fn accept_btcb2_price(&mut self, price: FiatPrice) -> bool {
        if !self.fiat_chain.is_blake2b()
            || price.request.origin
                != Some(LookupOrigin {
                    app: self.app_generation,
                    chain: self.fiat_chain,
                })
            || self.btcb2_price_request != Some(price.request)
        {
            return false;
        }
        self.btc_usd_price = None;
        self.btc_usd_requested_at = None;
        self.fiat_price = Some(price);
        if !self.btcb2_price_usable() {
            self.clear_btcb2_fiat();
        }
        true
    }
}

#[cfg(test)]
mod btcb2_fiat_tests {
    use super::*;
    use crate::chain::ChainId;

    fn cache() -> Cache {
        Cache {
            fiat_chain: ChainId::BitcoinBlake2b,
            ..Default::default()
        }
    }

    fn answer(cache: &mut Cache, at: u64) -> FiatPrice {
        let request = FiatPriceRequest::for_btcb2(
            Currency::USD,
            LookupOrigin {
                app: cache.app_generation,
                chain: cache.fiat_chain,
            },
        );
        cache.btcb2_price_request = Some(request);
        FiatPrice {
            request,
            res: Ok(GetPriceResult {
                value: 102.0,
                updated_at: Some(at),
            }),
            btcb2_valid_until: at.checked_add(crate::services::fiat::btcb2::MAX_QUOTE_AGE),
        }
    }

    #[test]
    fn scoped_quotes_never_cross_app_generation_or_chain() {
        let mut current = cache();
        let mut previous = cache();
        let now = crate::services::fiat::btcb2::unix_now().unwrap();
        let own = answer(&mut current, now);
        let old = answer(&mut previous, now);
        assert!(!current.accept_btcb2_price(old));
        let mut wrong_chain = own.clone();
        wrong_chain.request.origin.as_mut().unwrap().chain = ChainId::BitcoinBlake2bTestnet4;
        assert!(!current.accept_btcb2_price(wrong_chain));
        current.btc_usd_price = Some(99_999.0);
        assert!(current.accept_btcb2_price(own));
        assert!(current.fiat_toggle_allowed());
        assert_eq!(current.btc_usd_price, None);
    }

    #[test]
    fn unavailable_or_expired_quotes_remove_fiat_and_disable_toggle() {
        let mut cache = cache();
        let now = crate::services::fiat::btcb2::unix_now().unwrap();
        let own = answer(&mut cache, now);
        assert!(cache.accept_btcb2_price(own));
        cache.display_mode = DisplayMode::FiatNative;
        let mut failed = answer(&mut cache, now);
        failed.res = Err(PriceApiError::RequestFailed("unavailable".into()));
        assert!(cache.accept_btcb2_price(failed));
        assert!(cache.fiat_price.is_none());
        assert!(!cache.fiat_toggle_allowed());
        assert_eq!(cache.display_mode, DisplayMode::BitcoinNative);
        let mut market_expired = answer(&mut cache, now);
        market_expired.btcb2_valid_until = now.checked_sub(1);
        assert!(cache.accept_btcb2_price(market_expired));
        assert!(cache.fiat_price.is_none());
        assert!(!cache.fiat_toggle_allowed());
        let expired = answer(
            &mut cache,
            now - crate::services::fiat::btcb2::MAX_QUOTE_AGE - 1,
        );
        assert!(cache.accept_btcb2_price(expired));
        assert!(cache.fiat_price.is_none());
        assert!(!cache.fiat_toggle_allowed());
    }

    #[test]
    fn newer_request_invalidates_an_older_currency_result() {
        let mut cache = cache();
        let now = crate::services::fiat::btcb2::unix_now().unwrap();
        let old = answer(&mut cache, now);
        cache.btcb2_price_request.as_mut().unwrap().currency = Currency::EUR;
        assert!(!cache.accept_btcb2_price(old));
        assert!(cache.fiat_price.is_none());
        assert!(
            Cache::default().fiat_toggle_allowed(),
            "Bitcoin behavior unchanged"
        );
        let old_same_currency = answer(&mut cache, now);
        cache.btcb2_price_request.as_mut().unwrap().instant += std::time::Duration::from_secs(1);
        assert!(!cache.accept_btcb2_price(old_same_currency));
    }

    #[tokio::test]
    async fn btcb2_cannot_use_default_bitcoin_transport_or_testnet_mainnet_quote() {
        let current = cache();
        let request = FiatPriceRequest::for_btcb2(
            Currency::USD,
            LookupOrigin {
                app: current.app_generation,
                chain: ChainId::BitcoinBlake2b,
            },
        );
        assert!(request.send_default().await.res.is_err());
        let testnet = FiatPriceRequest::for_btcb2(
            Currency::USD,
            LookupOrigin {
                app: current.app_generation,
                chain: ChainId::BitcoinBlake2bTestnet4,
            },
        );
        let client = crate::services::coincube::CoincubeClient::for_test("http://127.0.0.1:1");
        assert!(testnet.send_connect(client).await.res.is_err());
    }

    /// **The only live pricing path today.** Both approved exchanges refused
    /// read-only access on 2026-09-19 (`docs/BTCB2_PRICING.md`), so no adapter
    /// exists and the Connect route answers 503 on an empty cache. A BTCB2
    /// Cube must render in `BitcoinNative` and keep the toggle inert — and
    /// must never fall back to a *Bitcoin* quote, which is a different coin.
    #[tokio::test]
    async fn a_503_from_the_unbuilt_price_route_leaves_the_cube_in_bitcoin_native() {
        use httpmock::prelude::*;
        let server = MockServer::start_async().await;
        let route = server
            .mock_async(|when, then| {
                when.method(GET).path_contains("price");
                then.status(503).json_body(serde_json::json!({
                    "success": false,
                    "message": "BTCB2 pricing is unavailable"
                }));
            })
            .await;

        let mut cache = cache();
        // A stale mainnet quote in hand is the dangerous starting state: it is
        // the thing a fork Cube must not silently render.
        cache.btc_usd_price = Some(99_999.0);
        cache.display_mode = DisplayMode::FiatNative;

        let request = FiatPriceRequest::for_btcb2(
            Currency::USD,
            LookupOrigin {
                app: cache.app_generation,
                chain: cache.fiat_chain,
            },
        );
        cache.btcb2_price_request = Some(request);
        let client = crate::services::coincube::CoincubeClient::for_test(server.base_url());
        let answer = request.send_connect(client).await;
        assert!(answer.res.is_err(), "503 is not a price");
        route.assert_async().await;

        assert!(cache.accept_btcb2_price(answer));
        assert_eq!(cache.display_mode, DisplayMode::BitcoinNative);
        assert!(cache.fiat_price.is_none());
        assert_eq!(cache.btc_usd_price, None, "never a Bitcoin quote on BTCB2");
        assert!(
            !cache.fiat_toggle_allowed(),
            "FlipDisplayMode must be inert while the feed has nothing to show"
        );
        // And the handler's contract on top of that predicate: a flip attempt
        // clears rather than flips (`App::update`, `FlipDisplayMode`).
        assert!(cache.clear_btcb2_fiat() || cache.display_mode == DisplayMode::BitcoinNative);
        assert_eq!(cache.display_mode, DisplayMode::BitcoinNative);
    }
}

/// Where automatic Lightning address assignment stands for this Cube.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum LightningAddressSetup {
    /// Not started (or the Cube has an address).
    #[default]
    Idle,
    /// Picking a free random name and claiming it.
    Assigning,
    /// The last attempt failed; the message says why.
    Failed(String),
}
