use crate::{
    app::settings::unit::BitcoinDisplayUnit,
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
    /// Current theme mode (dark/light) — used for theme-aware widget rendering
    pub theme_mode: coincube_ui::theme::palette::ThemeMode,
    /// BTC price in USD, always fetched regardless of the user's selected fiat
    /// currency. Used for converting USDt (which is pegged to USD) into sats.
    pub btc_usd_price: Option<f64>,
    /// Whether to show direction badges (receive/spend arrows) on transaction rows.
    pub show_direction_badges: bool,
    /// Cached Lightning Address for display in the sidebar across all panels
    pub lightning_address: Option<String>,
    /// Id of the current Cube — needed by Spark Settings so the
    /// `update_settings_file` closure can find the right cube when
    /// persisting the `default_lightning_backend` picker change.
    pub cube_id: String,
    /// Current preference for which backend fulfills incoming
    /// Lightning Address invoices. Mirrored from
    /// `CubeSettings::default_lightning_backend` so panels can read
    /// it without going through the disk layer; the authoritative
    /// copy lives on `App::cube_settings` and is re-read on
    /// `Message::SettingsSaved`.
    pub default_lightning_backend: crate::app::wallets::WalletKind,
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
            connect_authenticated: false,
            has_vault: false,
            cube_name: String::new(),
            current_cube_backed_up: false,
            backup_warning_dismissed: false,
            current_cube_is_passkey: false,
            has_p2p: false,
            theme_mode: coincube_ui::theme::palette::ThemeMode::default(),
            btc_usd_price: None,
            show_direction_badges: true,
            lightning_address: None,
            cube_id: String::new(),
            default_lightning_backend: crate::app::wallets::WalletKind::default(),
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
