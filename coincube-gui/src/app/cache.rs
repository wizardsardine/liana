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

#[derive(Debug, Clone)]
pub struct Cache {
    pub datadir_path: CoincubeDirectory,
    /// IBD progress (0.0–1.0) of the pending local Bitcoind, polled via its
    /// RPC.  `None` when no local node is pending.
    pub node_bitcoind_sync_progress: Option<f64>,
    /// Whether the pending local Bitcoind is currently in initial block download.
    /// `None` when no local node is pending.
    pub node_bitcoind_ibd: Option<bool>,
    /// Latest UpdateTip/blockheaders line from the pending internal bitcoind's
    /// debug.log.  `None` until the first line is received.
    pub node_bitcoind_last_log: Option<String>,
    pub network: Network,
    /// The `last_poll_timestamp` when starting the application.
    pub last_poll_at_startup: Option<u32>,
    pub daemon_cache: DaemonCache,
    pub fiat_price: Option<FiatPrice>,
    /// Bitcoin display unit preference (BTC or Sats)
    pub bitcoin_unit: BitcoinDisplayUnit,
    /// Global fiat-native vs. bitcoin-native display preference. Drives
    /// whether wallet headers lead with the fiat or bitcoin amount across
    /// the app. Mirrored from `Settings::display_mode` (top-level, not
    /// per-cube) and re-read on `SettingsSaved`.
    pub display_mode: DisplayMode,
    /// Whether the Connect user is authenticated (Dashboard step reached)
    pub connect_authenticated: bool,
    /// Whether this cube has a vault wallet configured
    pub has_vault: bool,
    /// Display name of the current Cube
    pub cube_name: String,
    /// Whether the user has completed the master seed backup flow for this
    /// Cube. Drives the soft "not backed up" warning banners on the Vault
    /// and Liquid home screens. Mirrors `CubeSettings::backed_up`.
    pub current_cube_backed_up: bool,
    /// Session-scoped dismissal of the "not backed up" banner. Defaults
    /// to `false` for every new `Cache`, so each Tab (one Cube per Tab)
    /// starts with the banner visible, and it's also reset if
    /// `current_cube_backed_up` ever transitions back to `false`. Set to
    /// `true` only by the user clicking the dismiss button. Cleared on
    /// app restart — the reminder keeps surfacing until the user
    /// actually backs up.
    pub backup_warning_dismissed: bool,
    /// Whether the current Cube uses a passkey-derived master key (no PIN,
    /// no stored encrypted mnemonic). Used to hide the seed-backup UI.
    pub current_cube_is_passkey: bool,
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
    /// Current theme mode (dark/light) — used for theme-aware widget rendering
    pub theme_mode: coincube_ui::theme::palette::ThemeMode,
    /// BTC price in USD, always fetched regardless of the user's selected fiat
    /// currency. Used for converting USDt (which is pegged to USD) into sats.
    pub btc_usd_price: Option<f64>,
    /// Whether to show direction badges (receive/spend arrows) on transaction rows.
    pub show_direction_badges: bool,
    /// Cached Lightning Address for display in the sidebar across all panels
    pub lightning_address: Option<String>,
    /// Cached avatar image handle for display in the sidebar across all panels
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
    /// Connect gRPC base URL, populated once `Message::ConnectStreamReady`
    /// fires after login. `None` until then or for local-daemon installs
    /// — the Keychain "Sign via Keychain" button stays disabled while
    /// this is `None`.
    pub connect_grpc_url: Option<String>,
    /// Shared `Arc<RwLock<AccessTokenResponse>>` from the remote backend,
    /// mirrored into Cache so deep panels (PsbtState et al.) can spin up
    /// a `GrpcSessionClient` on demand without re-plumbing the auth
    /// handle. `None` for local-daemon installs.
    pub connect_tokens: Option<
        Arc<tokio::sync::RwLock<crate::services::connect::client::auth::AccessTokenResponse>>,
    >,
    /// Health state of the Connect realtime gRPC stream. Mirrored from
    /// `App::handle_connect_stream` so the sidebar can render a small
    /// status dot without re-plumbing through view function parameters.
    /// `Inactive` for local-daemon installs and any session before the
    /// first `Message::ConnectStreamReady` has fired.
    pub connect_stream_status: crate::app::ConnectionStatus,
    /// `SignerDevice.id` returned by the API's `RegisterDevice` RPC.
    /// Surfaced on the Settings → About page so the user can confirm
    /// this desktop is registered, and to drive the "Re-register"
    /// affordance. Mirrored from the Connect cache on
    /// `Message::ConnectStreamReady`. `None` when the desktop hasn't
    /// registered yet or for local-daemon installs.
    pub connect_device_id: Option<String>,
    /// Email of the authenticated Connect account. Surfaced alongside
    /// the device id for at-a-glance "which account is this device
    /// registered to" troubleshooting. `None` for local-daemon
    /// installs.
    pub connect_email: Option<String>,
}

/// only used for tests.
impl std::default::Default for Cache {
    fn default() -> Self {
        Self {
            datadir_path: CoincubeDirectory::new(std::path::PathBuf::new()),
            node_bitcoind_sync_progress: None,
            node_bitcoind_ibd: None,
            node_bitcoind_last_log: None,
            network: Network::Bitcoin,
            last_poll_at_startup: None,
            daemon_cache: DaemonCache::default(),
            fiat_price: None,
            bitcoin_unit: BitcoinDisplayUnit::default(),
            display_mode: DisplayMode::default(),
            connect_authenticated: false,
            has_vault: false,
            cube_name: String::new(),
            current_cube_backed_up: false,
            backup_warning_dismissed: false,
            current_cube_is_passkey: false,
            has_p2p: false,
            p2p_test_coordinator: false,
            theme_mode: coincube_ui::theme::palette::ThemeMode::default(),
            btc_usd_price: None,
            show_direction_badges: true,
            lightning_address: None,
            avatar_handle: None,
            cube_id: String::new(),
            current_cube_server_id: None,
            current_descriptor_fingerprint: None,
            recovery_kit_last_backed_up_descriptor_fingerprint: None,
            connect_grpc_url: None,
            connect_tokens: None,
            connect_stream_status: crate::app::ConnectionStatus::default(),
            connect_device_id: None,
            connect_email: None,
        }
    }
}

impl Cache {
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
}

impl FiatPriceRequest {
    pub fn new(source: PriceSource, currency: Currency) -> Self {
        Self {
            source,
            currency,
            instant: Instant::now(),
        }
    }

    /// Sends the request using the default client for the given source.
    pub async fn send_default(self) -> FiatPrice {
        let client = PriceClient::default_from_source(self.source);
        FiatPrice {
            res: client.get_price(self.currency).await,
            request: self,
        }
    }
}
