pub mod breez_liquid;
pub mod breez_spark;
pub mod cache;
pub mod config;
pub mod error;
pub mod menu;
pub mod message;
pub mod settings;
pub mod state;
pub mod view;
pub mod wallet;
pub mod wallets;

use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

use iced::{clipboard, time, Subscription, Task};
use tokio::runtime::Handle;
use tracing::{error, info, warn};

pub use coincube_core::miniscript::bitcoin;
use coincube_ui::{component::network_banner, theme as ui_theme, widget::Element};
pub use coincubed::{
    commands::CoinStatus,
    config::{BitcoindRpcAuth, Config as DaemonConfig},
};

pub use config::Config;
pub use message::Message;

use state::{
    CoinsPanel, ConnectPanel, CreateSpendPanel, GlobalHome, LiquidOverview, LiquidReceive,
    LiquidSend, LiquidSettings, LiquidTransactions, PsbtsPanel, State, VaultOverview,
    VaultReceivePanel, VaultTransactionsPanel,
};
use wallet::{sync_status, SyncStatus};

use crate::{
    app::{
        breez_liquid::BreezClient,
        cache::{Cache, DaemonCache},
        error::Error,
        menu::{MarketplaceSubMenu, Menu},
        message::FiatMessage,
        settings::WalletId,
        wallet::Wallet,
        wallets::LiquidBackend,
    },
    daemon::{embedded::EmbeddedDaemon, Daemon, DaemonBackend, DaemonError},
    dir::CoincubeDirectory,
    node::{
        bitcoind::{internal_bitcoind_datadir, internal_bitcoind_debug_log_path, Bitcoind},
        NodeType,
    },
    utils::truncate_middle,
};

use self::state::settings::SettingsState as GeneralSettingsState;
use self::state::vault::settings::SettingsState as VaultSettingsState;

struct Panels {
    current: Menu,
    // Always available panels
    global_home: GlobalHome,
    liquid_overview: LiquidOverview,
    liquid_send: LiquidSend,
    liquid_receive: LiquidReceive,
    liquid_transactions: LiquidTransactions,
    liquid_settings: LiquidSettings,
    /// Spark wallet Overview — Phase 3 placeholder. Always present so
    /// `current()` / `current_mut()` have a target; internally the
    /// panel checks whether the [`SparkBackend`] is wired and shows an
    /// "unavailable" stub when it isn't.
    spark_overview: state::SparkOverview,
    /// Phase 4c ships real Send and Receive panels backed by the
    /// bridge's new write-path RPCs (`prepare_send_payment`,
    /// `send_payment`, `receive_payment`). LNURL-pay, Lightning Address
    /// management, and the on-chain `claim_deposit` lifecycle are the
    /// Phase 4d follow-ups.
    spark_send: state::SparkSend,
    spark_receive: state::SparkReceive,
    /// Phase 4b ships real Transactions + Settings panels — they use
    /// `list_payments` / `get_info` which the bridge already exposes, so
    /// they ship ahead of the write-path flows.
    spark_transactions: state::SparkTransactions,
    spark_settings: state::SparkSettings,
    global_settings: GeneralSettingsState,
    // Vault-only panels - None when no vault exists
    vault_overview: Option<VaultOverview>,
    coins: Option<CoinsPanel>,
    transactions: Option<VaultTransactionsPanel>,
    psbts: Option<PsbtsPanel>,
    recovery: Option<CreateSpendPanel>,
    receive: Option<VaultReceivePanel>,
    create_spend: Option<CreateSpendPanel>,
    vault_settings: Option<VaultSettingsState>,
    // remaining panels
    buy_sell: Option<crate::app::view::buysell::BuySellPanel>,
    connect: ConnectPanel,
    p2p: Option<crate::app::view::p2p::P2PPanel>,
}

impl Panels {
    /// Read the cube's fiat currency preference from the settings file.
    fn default_fiat_currency(
        datadir: &CoincubeDirectory,
        network: bitcoin::Network,
        cube_id: &str,
    ) -> Option<String> {
        let network_dir = datadir.network_directory(network);
        settings::Settings::from_file(&network_dir)
            .ok()
            .and_then(|s| {
                s.cubes
                    .iter()
                    .find(|c| c.id == cube_id)
                    .and_then(|c| c.fiat_price.as_ref())
                    .map(|fp| fp.currency.to_string())
            })
    }

    /// Read the cube's persisted `balance_masked` eye-icon preference.
    fn initial_balance_masked(
        datadir: &CoincubeDirectory,
        network: bitcoin::Network,
        cube_id: &str,
    ) -> bool {
        let network_dir = datadir.network_directory(network);
        settings::Settings::from_file(&network_dir)
            .ok()
            .and_then(|s| {
                s.cubes
                    .iter()
                    .find(|c| c.id == cube_id)
                    .map(|c| c.balance_masked)
            })
            .unwrap_or(false)
    }

    #[allow(clippy::too_many_arguments)]
    fn new_without_vault(
        breez_client: Arc<BreezClient>,
        spark_backend: Option<Arc<crate::app::wallets::SparkBackend>>,
        wallet: Option<Arc<Wallet>>,
        datadir: &CoincubeDirectory,
        network: bitcoin::Network,
        cube_id: String,
        cube_name: String,
        cube_network: String,
    ) -> Panels {
        // NO VAULT - All vault panels are None, but Liquid panels always work
        // The UI layer prevents navigation to vault panels when has_vault=false

        let default_fiat_currency = Self::default_fiat_currency(datadir, network, &cube_id);
        let liquid_backend = Arc::new(LiquidBackend::new(breez_client.clone()));
        let initial_balance_masked = Self::initial_balance_masked(datadir, network, &cube_id);

        Self {
            current: Menu::Cube(crate::app::menu::CubeSubMenu::Overview),
            // Liquid panels always available (use LiquidBackend, not Vault wallet)
            global_home: if let Some(w) = &wallet {
                GlobalHome::new(
                    w.clone(),
                    liquid_backend.clone(),
                    spark_backend.clone(),
                    datadir.clone(),
                    network,
                    cube_id.clone(),
                    initial_balance_masked,
                )
            } else {
                GlobalHome::new_without_wallet(
                    liquid_backend.clone(),
                    spark_backend.clone(),
                    datadir.clone(),
                    network,
                    cube_id.clone(),
                    initial_balance_masked,
                )
            },
            liquid_overview: LiquidOverview::new(liquid_backend.clone()),
            liquid_send: LiquidSend::new(liquid_backend.clone()),
            liquid_receive: LiquidReceive::new(liquid_backend.clone()),
            liquid_transactions: LiquidTransactions::new(liquid_backend.clone()),
            liquid_settings: LiquidSettings::new(liquid_backend.clone()),
            spark_overview: state::SparkOverview::new(spark_backend.clone()),
            spark_send: state::SparkSend::new(spark_backend.clone()),
            spark_receive: state::SparkReceive::new(spark_backend.clone()),
            spark_transactions: state::SparkTransactions::new(spark_backend.clone()),
            spark_settings: state::SparkSettings::new(spark_backend.clone()),
            global_settings: {
                let network_dir = datadir.network_directory(network);
                let settings_file = settings::Settings::from_file(&network_dir).ok();
                let (price_setting, unit_setting) = settings_file
                    .as_ref()
                    .and_then(|s| s.cubes.iter().find(|c| c.id == cube_id))
                    .map(|c| {
                        (
                            c.fiat_price.clone().unwrap_or_default(),
                            c.unit_setting.clone(),
                        )
                    })
                    .unwrap_or_default();
                GeneralSettingsState::new(cube_id.clone(), price_setting, unit_setting)
            },
            // All vault panels are None - no vault exists
            vault_overview: None,
            coins: None,
            transactions: None,
            psbts: None,
            recovery: None,
            receive: None,
            create_spend: None,
            vault_settings: None,
            // remaining panels
            buy_sell: None,
            connect: ConnectPanel::new(
                spark_backend.as_ref().map(|b| b.client().clone()),
                cube_id.clone(),
                cube_name,
                cube_network,
            ),
            p2p: match breez_client
                .liquid_signer()
                .map(|s| s.lock().expect("signer lock").mnemonic_str())
            {
                Some(mnemonic) if !mnemonic.is_empty() => {
                    Some(crate::app::view::p2p::P2PPanel::new(
                        None,
                        spark_backend.clone(),
                        mnemonic,
                        default_fiat_currency,
                    ))
                }
                _ => {
                    log::warn!("P2P panel disabled: no mnemonic available from liquid signer");
                    None
                }
            },
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn new(
        breez_client: Arc<BreezClient>,
        spark_backend: Option<Arc<crate::app::wallets::SparkBackend>>,
        cache: &Cache,
        wallet: Arc<Wallet>,
        data_dir: CoincubeDirectory,
        daemon_backend: DaemonBackend,
        internal_bitcoind: Option<&Bitcoind>,
        config: Arc<Config>,
        restored_from_backup: bool,
        cube_id: String,
        cube_name: String,
        cube_network: String,
    ) -> Panels {
        let show_rescan_warning = restored_from_backup
            && daemon_backend.is_coincubed()
            && daemon_backend
                .node_type()
                .map(|nt| nt == NodeType::Bitcoind)
                // We don't know the node type for external coincubed so assume it's bitcoind.
                .unwrap_or(true);

        let default_fiat_currency = Self::default_fiat_currency(&data_dir, cache.network, &cube_id);
        let liquid_backend = Arc::new(LiquidBackend::new(breez_client.clone()));
        let initial_balance_masked =
            Self::initial_balance_masked(&data_dir, cache.network, &cube_id);

        Self {
            current: Menu::Cube(crate::app::menu::CubeSubMenu::Overview),
            global_home: GlobalHome::new(
                wallet.clone(),
                liquid_backend.clone(),
                spark_backend.clone(),
                data_dir.clone(),
                cache.network,
                cube_id.clone(),
                initial_balance_masked,
            ),
            vault_overview: Some(VaultOverview::new(
                wallet.clone(),
                cache.coins(),
                sync_status(
                    daemon_backend.clone(),
                    cache.blockheight(),
                    cache.sync_progress(),
                    cache.last_poll_timestamp(),
                    cache.last_poll_at_startup,
                ),
                cache.blockheight(),
                show_rescan_warning,
            )),
            liquid_overview: LiquidOverview::new(liquid_backend.clone()),
            liquid_send: LiquidSend::new(liquid_backend.clone()),
            liquid_receive: LiquidReceive::new(liquid_backend.clone()),
            liquid_transactions: LiquidTransactions::new(liquid_backend.clone()),
            liquid_settings: LiquidSettings::new(liquid_backend.clone()),
            spark_overview: state::SparkOverview::new(spark_backend.clone()),
            spark_send: state::SparkSend::new(spark_backend.clone()),
            spark_receive: state::SparkReceive::new(spark_backend.clone()),
            spark_transactions: state::SparkTransactions::new(spark_backend.clone()),
            spark_settings: state::SparkSettings::new(spark_backend.clone()),
            global_settings: {
                let network_dir = data_dir.network_directory(cache.network);
                let settings_file = settings::Settings::from_file(&network_dir).ok();
                let (price_setting, unit_setting) = settings_file
                    .as_ref()
                    .and_then(|s| s.cubes.iter().find(|c| c.id == cube_id))
                    .map(|c| {
                        (
                            c.fiat_price.clone().unwrap_or_default(),
                            c.unit_setting.clone(),
                        )
                    })
                    .unwrap_or_default();
                GeneralSettingsState::new(cube_id.clone(), price_setting, unit_setting)
            },
            coins: Some(CoinsPanel::new(
                cache.coins(),
                wallet.main_descriptor.first_timelock_value(),
            )),
            transactions: Some(VaultTransactionsPanel::new(wallet.clone())),
            psbts: Some(PsbtsPanel::new(wallet.clone())),
            recovery: Some(new_recovery_panel(
                wallet.clone(),
                cache,
                sync_status(
                    daemon_backend.clone(),
                    cache.blockheight(),
                    cache.sync_progress(),
                    cache.last_poll_timestamp(),
                    cache.last_poll_at_startup,
                ),
            )),
            receive: Some(VaultReceivePanel::new(data_dir.clone(), wallet.clone())),
            create_spend: Some({
                let (balance, unconfirmed_balance, _, _) = state::coins_summary(
                    cache.coins(),
                    cache.blockheight().max(0) as u32,
                    wallet.main_descriptor.first_timelock_value(),
                );
                CreateSpendPanel::new(
                    wallet.clone(),
                    cache.coins(),
                    cache.blockheight().max(0) as u32,
                    cache.network,
                    balance,
                    unconfirmed_balance,
                    sync_status(
                        daemon_backend.clone(),
                        cache.blockheight(),
                        cache.sync_progress(),
                        cache.last_poll_timestamp(),
                        cache.last_poll_at_startup,
                    ),
                    cache.bitcoin_unit,
                )
            }),
            vault_settings: Some(VaultSettingsState::new(
                data_dir.clone(),
                wallet.clone(),
                daemon_backend,
                internal_bitcoind.is_some(),
                config.clone(),
            )),
            connect: ConnectPanel::new(
                spark_backend.as_ref().map(|b| b.client().clone()),
                cube_id.clone(),
                cube_name,
                cube_network,
            ),
            buy_sell: Some(crate::app::view::buysell::BuySellPanel::new(
                cache.network,
                wallet.clone(),
                breez_client.clone(),
            )),
            p2p: match breez_client
                .liquid_signer()
                .map(|s| s.lock().expect("signer lock").mnemonic_str())
            {
                Some(mnemonic) if !mnemonic.is_empty() => {
                    Some(crate::app::view::p2p::P2PPanel::new(
                        Some(wallet),
                        spark_backend.clone(),
                        mnemonic,
                        default_fiat_currency,
                    ))
                }
                _ => {
                    log::warn!("P2P panel disabled: no mnemonic available from liquid signer");
                    None
                }
            },
        }
    }

    /// Rebuilds all vault-specific panels when a vault wallet is added to an app that didn't have one.
    /// This is called when transitioning from no-vault to has-vault state.
    #[allow(clippy::too_many_arguments)]
    fn build_vault_panels(
        &mut self,
        wallet: Arc<Wallet>,
        cache: &Cache,
        daemon_backend: DaemonBackend,
        data_dir: CoincubeDirectory,
        internal_bitcoind: Option<&Bitcoind>,
        config: Arc<Config>,
        breez_client: Arc<BreezClient>,
    ) {
        self.vault_overview = Some(VaultOverview::new(
            wallet.clone(),
            cache.coins(),
            sync_status(
                daemon_backend.clone(),
                cache.blockheight(),
                cache.sync_progress(),
                cache.last_poll_timestamp(),
                cache.last_poll_at_startup,
            ),
            cache.blockheight(),
            false, // show_rescan_warning: false when adding vault dynamically
        ));
        self.coins = Some(CoinsPanel::new(
            cache.coins(),
            wallet.main_descriptor.first_timelock_value(),
        ));
        self.transactions = Some(VaultTransactionsPanel::new(wallet.clone()));
        self.psbts = Some(PsbtsPanel::new(wallet.clone()));
        self.recovery = Some(new_recovery_panel(
            wallet.clone(),
            cache,
            sync_status(
                daemon_backend.clone(),
                cache.blockheight(),
                cache.sync_progress(),
                cache.last_poll_timestamp(),
                cache.last_poll_at_startup,
            ),
        ));
        self.receive = Some(VaultReceivePanel::new(data_dir.clone(), wallet.clone()));
        self.create_spend = Some({
            let (balance, unconfirmed_balance, _, _) = state::coins_summary(
                cache.coins(),
                cache.blockheight() as u32,
                wallet.main_descriptor.first_timelock_value(),
            );
            CreateSpendPanel::new(
                wallet.clone(),
                cache.coins(),
                cache.blockheight() as u32,
                cache.network,
                balance,
                unconfirmed_balance,
                sync_status(
                    daemon_backend.clone(),
                    cache.blockheight(),
                    cache.sync_progress(),
                    cache.last_poll_timestamp(),
                    cache.last_poll_at_startup,
                ),
                cache.bitcoin_unit,
            )
        });
        self.vault_settings = Some(VaultSettingsState::new(
            data_dir.clone(),
            wallet.clone(),
            daemon_backend,
            internal_bitcoind.is_some(),
            config.clone(),
        ));

        self.buy_sell = Some(crate::app::view::buysell::BuySellPanel::new(
            cache.network,
            wallet,
            breez_client,
        ));
    }

    fn current(&self) -> Option<&dyn State> {
        match &self.current {
            Menu::Cube(crate::app::menu::CubeSubMenu::Overview) => Some(&self.global_home),
            Menu::Cube(crate::app::menu::CubeSubMenu::Settings(_)) => {
                Some(&self.global_settings as &dyn State)
            }
            Menu::Liquid(submenu) => match submenu {
                crate::app::menu::LiquidSubMenu::Overview => Some(&self.liquid_overview),
                crate::app::menu::LiquidSubMenu::Send => Some(&self.liquid_send),
                crate::app::menu::LiquidSubMenu::Receive => Some(&self.liquid_receive),
                crate::app::menu::LiquidSubMenu::Transactions(_) => Some(&self.liquid_transactions),
                crate::app::menu::LiquidSubMenu::Settings(_) => Some(&self.liquid_settings),
            },
            // Phase 4c ships all five real Spark panels. Send/Receive
            // use the bridge write-path RPCs added in this phase;
            // Overview/Transactions/Settings are unchanged from 4b.
            Menu::Spark(submenu) => match submenu {
                crate::app::menu::SparkSubMenu::Overview => {
                    Some(&self.spark_overview as &dyn State)
                }
                crate::app::menu::SparkSubMenu::Send => Some(&self.spark_send as &dyn State),
                crate::app::menu::SparkSubMenu::Receive => Some(&self.spark_receive as &dyn State),
                crate::app::menu::SparkSubMenu::Transactions(_) => {
                    Some(&self.spark_transactions as &dyn State)
                }
                crate::app::menu::SparkSubMenu::Settings(_) => {
                    Some(&self.spark_settings as &dyn State)
                }
            },
            Menu::Vault(submenu) => match submenu {
                crate::app::menu::VaultSubMenu::Overview => {
                    self.vault_overview.as_ref().map(|v| v as &dyn State)
                }
                crate::app::menu::VaultSubMenu::Send => {
                    self.create_spend.as_ref().map(|v| v as &dyn State)
                }
                crate::app::menu::VaultSubMenu::Receive => {
                    self.receive.as_ref().map(|v| v as &dyn State)
                }
                crate::app::menu::VaultSubMenu::Coins(_) => {
                    self.coins.as_ref().map(|v| v as &dyn State)
                }
                crate::app::menu::VaultSubMenu::Transactions(_) => {
                    self.transactions.as_ref().map(|v| v as &dyn State)
                }
                crate::app::menu::VaultSubMenu::PSBTs(_) => {
                    self.psbts.as_ref().map(|v| v as &dyn State)
                }
                crate::app::menu::VaultSubMenu::Recovery => {
                    self.recovery.as_ref().map(|v| v as &dyn State)
                }
                crate::app::menu::VaultSubMenu::Settings(_) => {
                    self.vault_settings.as_ref().map(|v| v as &dyn State)
                }
            },
            Menu::Marketplace(MarketplaceSubMenu::BuySell) => {
                self.buy_sell.as_ref().map(|v| v as &dyn State)
            }
            Menu::Marketplace(MarketplaceSubMenu::P2P(_)) => {
                self.p2p.as_ref().map(|v| v as &dyn State)
            }
        }
    }

    fn current_mut(&mut self) -> Option<&mut dyn State> {
        match &self.current {
            Menu::Cube(crate::app::menu::CubeSubMenu::Overview) => Some(&mut self.global_home),
            Menu::Cube(crate::app::menu::CubeSubMenu::Settings(_)) => {
                Some(&mut self.global_settings as &mut dyn State)
            }
            Menu::Liquid(submenu) => match submenu {
                crate::app::menu::LiquidSubMenu::Overview => Some(&mut self.liquid_overview),
                crate::app::menu::LiquidSubMenu::Send => Some(&mut self.liquid_send),
                crate::app::menu::LiquidSubMenu::Receive => Some(&mut self.liquid_receive),
                crate::app::menu::LiquidSubMenu::Transactions(_) => {
                    Some(&mut self.liquid_transactions)
                }
                crate::app::menu::LiquidSubMenu::Settings(_) => Some(&mut self.liquid_settings),
            },
            Menu::Spark(submenu) => match submenu {
                crate::app::menu::SparkSubMenu::Overview => {
                    Some(&mut self.spark_overview as &mut dyn State)
                }
                crate::app::menu::SparkSubMenu::Send => {
                    Some(&mut self.spark_send as &mut dyn State)
                }
                crate::app::menu::SparkSubMenu::Receive => {
                    Some(&mut self.spark_receive as &mut dyn State)
                }
                crate::app::menu::SparkSubMenu::Transactions(_) => {
                    Some(&mut self.spark_transactions as &mut dyn State)
                }
                crate::app::menu::SparkSubMenu::Settings(_) => {
                    Some(&mut self.spark_settings as &mut dyn State)
                }
            },
            Menu::Vault(submenu) => match submenu {
                crate::app::menu::VaultSubMenu::Overview => {
                    self.vault_overview.as_mut().map(|v| v as &mut dyn State)
                }
                crate::app::menu::VaultSubMenu::Send => {
                    self.create_spend.as_mut().map(|v| v as &mut dyn State)
                }
                crate::app::menu::VaultSubMenu::Receive => {
                    self.receive.as_mut().map(|v| v as &mut dyn State)
                }
                crate::app::menu::VaultSubMenu::Coins(_) => {
                    self.coins.as_mut().map(|v| v as &mut dyn State)
                }
                crate::app::menu::VaultSubMenu::Transactions(_) => {
                    self.transactions.as_mut().map(|v| v as &mut dyn State)
                }
                crate::app::menu::VaultSubMenu::PSBTs(_) => {
                    self.psbts.as_mut().map(|v| v as &mut dyn State)
                }
                crate::app::menu::VaultSubMenu::Recovery => {
                    self.recovery.as_mut().map(|v| v as &mut dyn State)
                }
                crate::app::menu::VaultSubMenu::Settings(_) => {
                    self.vault_settings.as_mut().map(|v| v as &mut dyn State)
                }
            },
            Menu::Marketplace(MarketplaceSubMenu::BuySell) => {
                self.buy_sell.as_mut().map(|v| v as &mut dyn State)
            }
            Menu::Marketplace(MarketplaceSubMenu::P2P(_)) => {
                self.p2p.as_mut().map(|v| v as &mut dyn State)
            }
        }
    }

    /// Returns the refresh message for the currently visible liquid-related panel, if any.
    /// Used to avoid refreshing all liquid panels when only one is visible.
    /// When `exclude_home` is true, skips the Home panel (useful when the caller
    /// already sends a separate RefreshLiquidBalance message).
    fn active_liquid_refresh(&self, exclude_home: bool) -> Option<Message> {
        match &self.current {
            Menu::Cube(crate::app::menu::CubeSubMenu::Overview) if !exclude_home => Some(
                Message::View(view::Message::Home(view::HomeMessage::RefreshLiquidBalance)),
            ),
            Menu::Liquid(sub) => match sub {
                crate::app::menu::LiquidSubMenu::Overview => Some(Message::View(
                    view::Message::LiquidOverview(view::LiquidOverviewMessage::RefreshRequested),
                )),
                crate::app::menu::LiquidSubMenu::Send => Some(Message::View(
                    view::Message::LiquidSend(view::LiquidSendMessage::RefreshRequested),
                )),
                crate::app::menu::LiquidSubMenu::Receive => Some(Message::View(
                    view::Message::LiquidReceive(view::LiquidReceiveMessage::RefreshRequested),
                )),
                // Route to a dedicated `BackgroundRefresh` rather than
                // the generic `Reload` — `Reload` would call the
                // panel's `reload()` which clears `selected_payment`,
                // `selected_refundable`, the refund modal and form
                // state. SDK events fire frequently (Synced,
                // PaymentSucceeded, etc.), so a Reload arm would kick
                // the user out of any drill-down they're in.
                // `BackgroundRefresh` is gated to only fire when the
                // panel is idle and uses `fetch_page(0)` to replace
                // payments atomically without disturbing state.
                crate::app::menu::LiquidSubMenu::Transactions(_) => {
                    Some(Message::View(view::Message::LiquidTransactions(
                        view::LiquidTransactionsMessage::BackgroundRefresh,
                    )))
                }
                _ => None,
            },
            _ => None,
        }
    }
}

/// Interval between bitcoind sync progress polls (in seconds).
const BITCOIND_SYNC_POLL_INTERVAL: Duration = Duration::from_secs(10);

pub struct App {
    cache: Cache,
    wallet: Option<Arc<Wallet>>,
    breez_client: Arc<BreezClient>,
    /// Wallet registry — owns the concrete wallet backends and exposes
    /// routing hooks. Holds a [`LiquidBackend`] and an optional
    /// [`SparkBackend`] (present when the cube has a Spark signer and
    /// the bridge subprocess came up). The LNURL subscription hand-off
    /// reads [`WalletRegistry::route_lightning_address`] so incoming
    /// Lightning Address invoices route through Spark when available
    /// and fall back to Liquid otherwise.
    wallet_registry: crate::app::wallets::WalletRegistry,
    daemon: Option<Arc<dyn Daemon + Sync + Send>>,
    internal_bitcoind: Option<Bitcoind>,
    cube_settings: settings::CubeSettings,
    config: Arc<Config>,
    datadir: CoincubeDirectory,
    panels: Panels,
    errors: Vec<(usize, std::time::Instant, log::Level, String)>,
    current_error_id: usize,
    /// True while a check_bitcoind_sync_progress probe is in flight; prevents
    /// multiple concurrent RPC calls from piling up across subscription ticks.
    bitcoind_sync_probe_in_progress: bool,
    /// Global "payment received" celebration overlay — shown for incoming
    /// Liquid payments (e.g. LNURL) regardless of which panel is active.
    show_received_celebration: bool,
    received_celebration_amount: String,
    received_celebration_context: String,
    received_celebration_quote: coincube_ui::component::quote_display::Quote,
    received_celebration_image: iced::widget::image::Handle,
    /// tx_ids of recent incoming payments we've already toasted for in
    /// PaymentWaitingConfirmation. Breez fires this event multiple times for
    /// the same swap; bounded FIFO so concurrent incoming swaps don't evict
    /// each other and re-toast.
    toasted_incoming_waiting_tx_ids: VecDeque<String>,
    /// Debounces event-driven `list_refundables()` polls. Breez fires `Synced`
    /// and payment events frequently; without a debounce window the GUI would
    /// hammer the SDK several times a minute. 30s is short enough that a
    /// freshly-refundable swap surfaces without user action but long enough to
    /// avoid noisy churn.
    last_refundables_fetch: Option<std::time::Instant>,
    /// True while a `refresh_refundables_task()` poll is awaiting its result.
    /// Prevents duplicate concurrent SDK calls when several BreezEvents arrive
    /// in quick succession. Cleared in the `RefundablesLoaded` handler.
    refundables_fetch_in_flight: bool,
    /// Set when the user clicked "Switch to COINCUBE | Connect" on Vault →
    /// Settings → Node while not signed in to Connect. We routed them to the
    /// Connect tab to sign in; on the next auth transition (false → true) we
    /// jump back to Vault Settings → Node and re-fire the switch.
    pending_switch_to_connect_after_login: bool,
    /// Shared `Arc<RwLock<AccessTokenResponse>>` from the remote backend,
    /// reused by the gRPC interceptor so token refreshes are observed by
    /// both the REST and gRPC paths. `None` for local-daemon installs.
    /// Stored on the App so PR B's `resolve_signers` /
    /// `create_signing_session` call sites can construct a
    /// `GrpcSessionClient` without re-plumbing.
    #[allow(dead_code)]
    connect_auth: Option<
        Arc<tokio::sync::RwLock<crate::services::connect::client::auth::AccessTokenResponse>>,
    >,
    /// Email of the currently authenticated Connect account. Used to
    /// scope cache writes (device_id, last_seen_event_seq). `None` for
    /// local-daemon installs.
    connect_email: Option<String>,
    /// Live `ConnectStreamConfig` once it has been assembled from
    /// `ServiceConfig` + cache state. `None` until the bootstrap task
    /// fires `Message::ConnectStreamReady`, or permanently `None` if the
    /// service config returned no `grpc_url`.
    connect_stream_config: Option<crate::services::connect::grpc::stream::ConnectStreamConfig>,
}

/// Health of the Connect realtime stream as observed from the desktop.
///
/// Transitions are driven by `ConnectStreamMessage` events arriving on
/// the gRPC subscription. The `Inactive` variant is distinct from
/// `Disconnected` because we want to render *nothing* (rather than a
/// red dot) when the user has no Connect identity yet — a fresh-install
/// desktop on a local-daemon cube isn't "broken", it's just not using
/// Connect.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ConnectionStatus {
    /// No stream has been bootstrapped yet (no `Message::ConnectStreamReady`).
    /// Render an empty slot, not a status dot.
    #[default]
    Inactive,
    /// Stream subscription is mounted but no `Connected` has arrived
    /// yet, or a `Disconnected` has fired and the next reconnect is
    /// pending. Render amber.
    Connecting,
    /// `ConnectStreamMessage::Connected` was the last terminal event.
    /// Render green.
    Connected,
    /// `ConnectStreamMessage::Error` carried a non-recoverable signal,
    /// or the stream surfaced a transport failure. The string is the
    /// most recent error suitable for a tooltip. Render red.
    Error(String),
}

impl ConnectionStatus {
    /// True for any state that the nav should surface (i.e. anything
    /// non-`Inactive`). Keeps the empty-slot rendering at the call site
    /// clean.
    pub fn is_visible(&self) -> bool {
        !matches!(self, Self::Inactive)
    }

    /// Short user-facing tooltip text describing the current state.
    /// Kept here so the nav view doesn't have to spell out the variants.
    pub fn tooltip(&self) -> String {
        match self {
            Self::Inactive => "Connect inactive".to_string(),
            Self::Connecting => "Connecting to Coincube Connect…".to_string(),
            Self::Connected => "Connected".to_string(),
            Self::Error(e) => format!("Connection error: {}", e),
        }
    }
}

/// Returns true when a `DaemonError` indicates the daemon process is no longer
/// reachable (transport / stopped), as opposed to a transient RPC application
/// error that does not warrant a backend switch.
fn is_daemon_unreachable(e: &Error) -> bool {
    matches!(
        e,
        Error::Daemon(
            DaemonError::DaemonStopped | DaemonError::NoAnswer | DaemonError::RpcSocket(..)
        )
    )
}

/// Poll the local bitcoind's IBD progress via its JSON-RPC interface.
/// Returns `(verificationprogress, initialblockdownload)` or an error string.
async fn check_bitcoind_sync_progress(
    cfg: coincubed::config::BitcoindConfig,
) -> Result<(f64, bool), String> {
    use coincubed::config::BitcoindRpcAuth;

    let (user, pass) = match &cfg.rpc_auth {
        BitcoindRpcAuth::CookieFile(path) => {
            let cookie = tokio::fs::read_to_string(path)
                .await
                .map_err(|e| format!("Cannot read bitcoind cookie: {}", e))?;
            let trimmed = cookie.trim();
            let sep = trimmed
                .find(':')
                .ok_or_else(|| "Invalid cookie file format".to_string())?;
            (trimmed[..sep].to_string(), trimmed[sep + 1..].to_string())
        }
        BitcoindRpcAuth::UserPass(u, p) => (u.clone(), p.clone()),
    };

    let url = format!("http://{}/", cfg.addr);
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "getblockchaininfo",
        "params": [],
        "id": 1
    });

    let resp: serde_json::Value = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| format!("bitcoind RPC client build failed: {}", e))?
        .post(&url)
        .basic_auth(&user, Some(&pass))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("bitcoind RPC request failed: {}", e))?
        .json()
        .await
        .map_err(|e| format!("bitcoind RPC response parse failed: {}", e))?;

    let result = &resp["result"];
    let progress = result["verificationprogress"]
        .as_f64()
        .ok_or_else(|| "Missing verificationprogress in bitcoind response".to_string())?;
    let ibd = result["initialblockdownload"]
        .as_bool()
        .ok_or_else(|| "Missing initialblockdownload in bitcoind response".to_string())?;
    Ok((progress, ibd))
}

/// Hashable wrapper around `ConnectStreamConfig` so it can be used as
/// the identity key for `iced::Subscription::run_with`. We hash only the
/// fields that should force a fresh subscription: `device_id` and
/// `grpc_url`. The shared `Arc<RwLock<tokens>>` is intentionally excluded
/// — a token refresh must not tear down the stream.
///
/// `last_seen_seq` is also deliberately excluded: it advances on every
/// received event, and hashing it here would cause Iced to tear down and
/// recreate the subscription after each event. The new stream would then
/// be `Aborted: superseded` by the hub (because the old one is still in
/// the connections map for ~1s) and we'd flap in a tight loop. The
/// stream's own loop already tracks the latest seq locally and uses it
/// for the next reconnect's ClientHello.
struct ConnectStreamSubKey {
    cfg: crate::services::connect::grpc::stream::ConnectStreamConfig,
}

impl std::hash::Hash for ConnectStreamSubKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        "connect-stream".hash(state);
        self.cfg.device_id.hash(state);
        self.cfg.grpc_url.hash(state);
    }
}

/// Wrap a `ConnectionStatus` change into a Task that fires
/// `Message::KeychainSign(StreamHealth(..))`. The standard update path
/// then routes it to the open `KeychainSignModal` (if any) so it can
/// surface a "connection lost" banner while sessions are pending.
fn stream_health_dispatch(status: ConnectionStatus) -> Task<Message> {
    Task::done(Message::KeychainSign(
        crate::app::state::vault::keychain_sign::KeychainSignMessage::StreamHealth(status),
    ))
}

fn make_connect_stream(
    key: &ConnectStreamSubKey,
) -> impl iced::futures::Stream<Item = crate::services::connect::grpc::ConnectStreamMessage> + 'static
{
    crate::services::connect::grpc::stream::connect_stream(&key.cfg)
}

/// Background task that assembles a `ConnectStreamConfig` from
/// `ServiceConfig` + the on-disk Connect cache. Runs once at App startup
/// when a remote backend is in play. Yields `Message::ConnectStreamReady`
/// with `None` if the API config lacks a `grpc_url` (gRPC not enabled
/// for this environment), or with `Some(cfg)` otherwise. The handler
/// stashes the config and the next `subscription()` tick wires the
/// stream.
fn connect_stream_ready_task(
    network: coincube_core::miniscript::bitcoin::Network,
    datadir: CoincubeDirectory,
    tokens: Arc<tokio::sync::RwLock<crate::services::connect::client::auth::AccessTokenResponse>>,
    email: String,
    cube_uuid: Option<String>,
) -> Task<Message> {
    use crate::services::connect::client::cache::Account;
    use crate::services::connect::client::get_service_config;
    use crate::services::connect::grpc::stream::ConnectStreamConfig;

    Task::perform(
        async move {
            let service_config = match get_service_config(network).await {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(
                        "Connect stream bootstrap: failed to fetch ServiceConfig: {}",
                        e,
                    );
                    return None;
                }
            };
            let Some(grpc_url) = service_config.grpc_url else {
                tracing::info!("Connect stream bootstrap: ServiceConfig has no grpc_url");
                return None;
            };
            let network_dir = datadir.network_directory(network);
            let cache_account = Account::from_cache(&network_dir, &email).ok().flatten();
            let Some(device_id) = cache_account.as_ref().and_then(|a| a.device_id.clone()) else {
                tracing::info!(
                    "Connect stream bootstrap: no device_id in cache for {} — \
                     skipping stream until next launch",
                    email,
                );
                return None;
            };
            let last_seen_seq = cache_account
                .and_then(|a| a.last_seen_event_seq)
                .unwrap_or(0);

            // Look up the cube's vault id so the server can scope this
            // session's `SessionEvent` stream to just this cube. If
            // the lookup fails (no vault yet, transient error) we fall
            // back to an empty list — the server defaults to "all
            // events for this user", which is functionally fine but
            // slightly noisier. The fetch needs an authenticated
            // CoincubeClient; we build one against the access_token
            // we just read from the shared `Arc<RwLock>`.
            let vault_ids = if let Some(cube_uuid) = cube_uuid.as_ref() {
                let access_token = tokens.read().await.access_token.clone();
                let mut client = crate::services::coincube::CoincubeClient::new();
                client.set_token(&access_token);
                match client.list_cubes().await {
                    Ok(cubes) => cubes
                        .iter()
                        .find(|c| c.uuid == *cube_uuid)
                        .and_then(|c| c.vault.as_ref())
                        .map(|v| vec![v.id.to_string()])
                        .unwrap_or_default(),
                    Err(e) => {
                        tracing::warn!(
                            "Connect stream bootstrap: failed to fetch cubes for vault \
                             scoping: {} — subscribing to all events for this user",
                            e,
                        );
                        Vec::new()
                    }
                }
            } else {
                Vec::new()
            };

            Some(ConnectStreamConfig {
                grpc_url,
                tokens,
                device_id,
                user_agent: format!("coincube-gui/{}", env!("CARGO_PKG_VERSION")),
                vault_ids,
                last_seen_seq,
            })
        },
        Message::ConnectStreamReady,
    )
}

impl App {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        cache: Cache,
        wallet: Arc<Wallet>,
        breez_client: Arc<BreezClient>,
        spark_backend: Option<Arc<crate::app::wallets::SparkBackend>>,
        config: Config,
        daemon: Arc<dyn Daemon + Sync + Send>,
        data_dir: CoincubeDirectory,
        internal_bitcoind: Option<Bitcoind>,
        restored_from_backup: bool,
        cube_settings: settings::CubeSettings,
        connect_auth: Option<(
            Arc<tokio::sync::RwLock<crate::services::connect::client::auth::AccessTokenResponse>>,
            String,
        )>,
    ) -> (App, Task<Message>) {
        let config_arc = Arc::new(config);
        let liquid_backend = Arc::new(LiquidBackend::new(breez_client.clone()));
        let wallet_registry = crate::app::wallets::WalletRegistry::with_spark(
            liquid_backend.clone(),
            spark_backend.clone(),
        );

        let mut panels = Panels::new(
            breez_client.clone(),
            spark_backend.clone(),
            &cache,
            wallet.clone(),
            data_dir.clone(),
            daemon.backend(),
            internal_bitcoind.as_ref(),
            config_arc.clone(),
            restored_from_backup,
            cube_settings.id.clone(),
            cube_settings.name.clone(),
            settings::network_to_api_string(cache.network),
        );
        let mut tasks = vec![];
        if let Some(vault_overview) = panels.vault_overview.as_mut() {
            tasks.push(vault_overview.reload(Some(daemon.clone()), Some(wallet.clone())));
        } else {
            tracing::warn!("vault_overview not present in App::new despite vault being configured");
        }
        tasks.push(
            panels
                .global_home
                .reload(Some(daemon.clone()), Some(wallet.clone())),
        );
        tasks.push(panels.connect.ensure_session_check());
        let (connect_auth_arc, connect_email) = match connect_auth {
            Some((a, e)) => (Some(a), Some(e)),
            None => (None, None),
        };
        if let (Some(auth), Some(email)) = (connect_auth_arc.as_ref(), connect_email.as_deref()) {
            tasks.push(connect_stream_ready_task(
                cache.network,
                data_dir.clone(),
                auth.clone(),
                email.to_string(),
                Some(cube_settings.id.clone()),
            ));
        }
        let cmd = Task::batch(tasks);
        let mut cache_with_vault = cache;
        cache_with_vault.has_vault = true;
        cache_with_vault.has_p2p = panels.p2p.is_some();
        (
            Self {
                panels,
                cache: cache_with_vault,
                daemon: Some(daemon),
                wallet: Some(wallet),
                breez_client,
                wallet_registry,
                internal_bitcoind,
                cube_settings,
                config: config_arc,
                datadir: data_dir,
                errors: Vec::with_capacity(8),
                current_error_id: 256,
                bitcoind_sync_probe_in_progress: false,
                show_received_celebration: false,
                received_celebration_amount: String::new(),
                received_celebration_context: "transaction-received".to_string(),
                received_celebration_quote: coincube_ui::component::quote_display::random_quote(
                    "transaction-received",
                ),
                received_celebration_image:
                    coincube_ui::component::quote_display::image_handle_for_context(
                        "transaction-received",
                    ),
                toasted_incoming_waiting_tx_ids: VecDeque::with_capacity(16),
                last_refundables_fetch: None,
                refundables_fetch_in_flight: false,
                pending_switch_to_connect_after_login: false,
                connect_auth: connect_auth_arc,
                connect_email,
                connect_stream_config: None,
            },
            cmd,
        )
    }

    pub fn new_without_wallet(
        breez_client: Arc<BreezClient>,
        spark_backend: Option<Arc<crate::app::wallets::SparkBackend>>,
        config: Config,
        datadir: CoincubeDirectory,
        network: coincube_core::miniscript::bitcoin::Network,
        cube_settings: settings::CubeSettings,
    ) -> (App, Task<Message>) {
        let config_arc = Arc::new(config);
        let liquid_backend = Arc::new(LiquidBackend::new(breez_client.clone()));
        let wallet_registry = crate::app::wallets::WalletRegistry::with_spark(
            liquid_backend.clone(),
            spark_backend.clone(),
        );
        // Load bitcoin_unit and display_mode from settings if available
        let network_dir = datadir.network_directory(network);
        let settings_file = settings::Settings::from_file(&network_dir).ok();
        let bitcoin_unit = settings_file
            .as_ref()
            .and_then(|s| {
                s.cubes
                    .iter()
                    .find(|c| c.id == cube_settings.id)
                    .map(|c| c.unit_setting.display_unit)
            })
            .unwrap_or_default();
        let display_mode = settings_file
            .as_ref()
            .map(|s| s.display_mode)
            .unwrap_or_default();
        let cache = Cache {
            network,
            datadir_path: datadir.clone(),
            has_vault: false,
            bitcoin_unit,
            display_mode,
            cube_name: cube_settings.name.clone(),
            current_cube_backed_up: cube_settings.backed_up,
            current_cube_is_passkey: cube_settings.is_passkey_cube(),
            cube_id: cube_settings.id.clone(),
            recovery_kit_last_backed_up_descriptor_fingerprint: cube_settings
                .recovery_kit_last_backed_up_descriptor_fingerprint
                .clone(),
            ..Default::default()
        };

        let mut panels = Panels::new_without_vault(
            breez_client.clone(),
            spark_backend.clone(),
            None,
            &datadir,
            network,
            cube_settings.id.clone(),
            cube_settings.name.clone(),
            settings::network_to_api_string(network),
        );
        let mut cache = cache;
        cache.has_p2p = panels.p2p.is_some();

        let cmd = iced::Task::batch([
            panels.connect.ensure_session_check(),
            panels.global_home.reload(None, None),
        ]);

        (
            Self {
                panels,
                cache,
                daemon: None,
                wallet: None,
                breez_client,
                wallet_registry,
                internal_bitcoind: None,
                cube_settings,
                config: config_arc,
                datadir,
                errors: Vec::with_capacity(8),
                current_error_id: 256,
                bitcoind_sync_probe_in_progress: false,
                show_received_celebration: false,
                received_celebration_amount: String::new(),
                received_celebration_context: "transaction-received".to_string(),
                received_celebration_quote: coincube_ui::component::quote_display::random_quote(
                    "transaction-received",
                ),
                received_celebration_image:
                    coincube_ui::component::quote_display::image_handle_for_context(
                        "transaction-received",
                    ),
                toasted_incoming_waiting_tx_ids: VecDeque::with_capacity(16),
                last_refundables_fetch: None,
                refundables_fetch_in_flight: false,
                pending_switch_to_connect_after_login: false,
                connect_auth: None,
                connect_email: None,
                connect_stream_config: None,
            },
            cmd,
        )
    }

    pub fn wallet_id(&self) -> Option<WalletId> {
        self.wallet.as_ref().map(|w| w.id())
    }

    pub fn title(&self) -> &str {
        &self.cube_settings.name
    }

    pub fn cache(&self) -> &Cache {
        &self.cache
    }

    pub fn cache_mut(&mut self) -> &mut Cache {
        &mut self.cache
    }

    pub fn breez_client(&self) -> Arc<BreezClient> {
        self.breez_client.clone()
    }

    pub fn spark_backend(&self) -> Option<Arc<crate::app::wallets::SparkBackend>> {
        self.wallet_registry.spark().cloned()
    }

    /// Returns a clone of the authenticated coincube-api client (with JWT set),
    /// or `None` if the user has not logged in yet.
    pub fn authenticated_coincube_client(
        &self,
    ) -> Option<crate::services::coincube::CoincubeClient> {
        self.panels.connect.account.authenticated_client()
    }

    /// True when this tab's ConnectAccountPanel either already holds an
    /// authenticated session or can pull one out of the shared keyring
    /// entry. Lets the tab-level OpenConnectSignIn handler short-circuit
    /// the Home-tab handoff when the in-tab inline refresh is enough.
    pub fn can_restore_connect_session(&self) -> bool {
        self.panels.connect.account.is_authenticated()
            || self.panels.connect.account.has_stored_session()
    }

    pub fn wallet(&self) -> Option<&Wallet> {
        self.wallet.as_ref().map(|w| w.as_ref())
    }

    pub fn has_vault(&self) -> bool {
        self.wallet.is_some()
    }

    pub fn datadir(&self) -> &CoincubeDirectory {
        &self.datadir
    }

    pub fn cube_settings(&self) -> &settings::CubeSettings {
        &self.cube_settings
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    fn daemon_backend(&self) -> DaemonBackend {
        self.daemon
            .as_ref()
            .map(|d| d.backend())
            .unwrap_or(DaemonBackend::RemoteBackend)
    }

    fn set_current_panel(&mut self, menu: Menu) -> Task<Message> {
        if let Some(panel) = self.panels.current_mut() {
            panel.interrupt();
        }

        match &menu {
            // Cube → Settings → {General/About/Stats}: auto-dispatch the
            // matching sub-section so the inner SettingsState installs the
            // right child panel. The third rail visible alongside drives this
            // and highlights the active option.
            menu::Menu::Cube(menu::CubeSubMenu::Settings(option)) => {
                self.panels.current = menu.clone();
                let section_msg = match option {
                    menu::CubeSettingsOption::General => {
                        Some(view::SettingsMessage::GeneralSection)
                    }
                    menu::CubeSettingsOption::About => Some(view::SettingsMessage::AboutSection),
                    menu::CubeSettingsOption::Stats => {
                        Some(view::SettingsMessage::InstallStatsSection)
                    }
                    // Avatar / Members render from `ConnectCubePanel` via
                    // App::view; no section message is dispatched to the
                    // SettingsState. Side-effect loads (avatar fetch,
                    // members fetch) are kicked below.
                    menu::CubeSettingsOption::Avatar | menu::CubeSettingsOption::Members => None,
                };
                if let Some(section_msg) = section_msg {
                    // Fire even if daemon is None — the inner settings
                    // panels don't require daemon for construction; they
                    // just pass it through to their own reload().
                    if let Some(panel) = self.panels.current_mut() {
                        return panel.update(
                            self.daemon.clone(),
                            &self.cache,
                            Message::View(view::Message::Settings(section_msg)),
                        );
                    }
                    return Task::none();
                }
                // Avatar and Members: trigger the underlying load via
                // ConnectCubePanel, mirroring the per-Cube Connect arm.
                match option {
                    menu::CubeSettingsOption::Avatar => {
                        return iced::Task::done(Message::View(view::Message::ConnectCube(
                            view::ConnectCubeMessage::Avatar(view::AvatarMessage::Enter),
                        )));
                    }
                    menu::CubeSettingsOption::Members
                        if self.panels.connect.account.is_authenticated() =>
                    {
                        return iced::Task::done(Message::View(view::Message::ConnectCube(
                            view::ConnectCubeMessage::Members(
                                view::ConnectCubeMembersMessage::Enter,
                            ),
                        )));
                    }
                    _ => {}
                }
                return Task::none();
            }
            menu::Menu::Vault(submenu) => {
                // Only process vault menu if we have a wallet
                if let Some(wallet) = &self.wallet {
                    match submenu {
                        menu::VaultSubMenu::Transactions(Some(txid)) => {
                            if let Some(daemon) = &self.daemon {
                                if let Ok(Some(tx)) = Handle::current().block_on(async {
                                    daemon
                                        .get_history_txs(&[*txid])
                                        .await
                                        .map(|txs| txs.first().cloned())
                                }) {
                                    if let Some(transactions) = &mut self.panels.transactions {
                                        transactions.preselect(tx);
                                    }
                                    self.panels.current = menu;
                                    return Task::none();
                                }
                            }
                        }
                        menu::VaultSubMenu::PSBTs(Some(txid)) => {
                            if let Some(daemon) = &self.daemon {
                                if let Ok(Some(spend_tx)) = Handle::current().block_on(async {
                                    daemon
                                        .list_spend_transactions(Some(&[*txid]))
                                        .await
                                        .map(|txs| txs.first().cloned())
                                }) {
                                    if let Some(psbts) = &mut self.panels.psbts {
                                        psbts.preselect(spend_tx);
                                    }
                                    self.panels.current = menu;
                                    return Task::none();
                                }
                            }
                        }
                        menu::VaultSubMenu::Settings(Some(setting)) => {
                            if let Some(daemon) = &self.daemon {
                                self.panels.current = menu.clone();
                                if let Some(panel) = self.panels.current_mut() {
                                    return panel.update(
                                        Some(daemon.clone()),
                                        &self.cache,
                                        Message::View(view::Message::Settings(match setting {
                                            menu::SettingsOption::Node => {
                                                view::SettingsMessage::EditBitcoindSettings
                                            }
                                            menu::SettingsOption::Wallet => {
                                                view::SettingsMessage::EditWalletSettings
                                            }
                                            menu::SettingsOption::ImportExport => {
                                                view::SettingsMessage::ImportExportSection
                                            }
                                        })),
                                    );
                                }
                            }
                        }
                        menu::VaultSubMenu::Coins(Some(preselected)) => {
                            let (balance, unconfirmed_balance, _, _) = state::coins_summary(
                                self.cache.coins(),
                                self.cache.blockheight() as u32,
                                wallet.main_descriptor.first_timelock_value(),
                            );
                            self.panels.create_spend = Some(CreateSpendPanel::new_self_send(
                                wallet.clone(),
                                self.cache.coins(),
                                self.cache.blockheight() as u32,
                                preselected,
                                self.cache.network,
                                balance,
                                unconfirmed_balance,
                                sync_status(
                                    self.daemon_backend(),
                                    self.cache.blockheight(),
                                    self.cache.sync_progress(),
                                    self.cache.last_poll_timestamp(),
                                    self.cache.last_poll_at_startup,
                                ),
                                self.cache.bitcoin_unit,
                            ));
                        }
                        menu::VaultSubMenu::Send => {
                            // redo the process of spending only if user want to start a new one.
                            if self
                                .panels
                                .create_spend
                                .as_ref()
                                .is_none_or(|p| !p.keep_state())
                            {
                                self.panels.create_spend = Some({
                                    let (balance, unconfirmed_balance, _, _) = state::coins_summary(
                                        self.cache.coins(),
                                        self.cache.blockheight() as u32,
                                        wallet.main_descriptor.first_timelock_value(),
                                    );
                                    CreateSpendPanel::new(
                                        wallet.clone(),
                                        self.cache.coins(),
                                        self.cache.blockheight() as u32,
                                        self.cache.network,
                                        balance,
                                        unconfirmed_balance,
                                        sync_status(
                                            self.daemon_backend(),
                                            self.cache.blockheight(),
                                            self.cache.sync_progress(),
                                            self.cache.last_poll_timestamp(),
                                            self.cache.last_poll_at_startup,
                                        ),
                                        self.cache.bitcoin_unit,
                                    )
                                });
                            }
                        }
                        menu::VaultSubMenu::Recovery => {
                            if self
                                .panels
                                .recovery
                                .as_ref()
                                .is_none_or(|p| !p.keep_state())
                            {
                                self.panels.recovery = Some(new_recovery_panel(
                                    wallet.clone(),
                                    &self.cache,
                                    sync_status(
                                        self.daemon_backend(),
                                        self.cache.blockheight(),
                                        self.cache.sync_progress(),
                                        self.cache.last_poll_timestamp(),
                                        self.cache.last_poll_at_startup,
                                    ),
                                ));
                            }
                        }
                        _ => {}
                    }
                }
            }
            menu::Menu::Liquid(_submenu) => {
                // Liquid transaction preselection is handled via PreselectPayment message
                // since Payment objects are passed directly instead of fetching by ID
            }
            _ => {
                tracing::debug!(
                    "Menu variant {:?} has no special handling in set_current_panel",
                    menu
                );
            }
        }

        self.panels.current = menu.clone();

        // Call reload with optional daemon/wallet
        // Liquid panels don't need them (use BreezClient), Vault panels do
        if let Some(panel) = self.panels.current_mut() {
            panel.reload(self.daemon.clone(), self.wallet.clone())
        } else {
            Task::none()
        }
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let mut subscriptions = vec![];

        // Always subscribe to Breez events (handles fee acceptance globally)
        subscriptions.push(self.breez_client.subscription().map(Message::BreezEvent));

        // Subscribe to Spark bridge events when a Spark backend is
        // active. The backend is optional (cubes without a Spark signer
        // run with `wallet_registry.spark() == None`), so we only wire
        // the subscription when there's actually a bridge to listen to.
        // The subscription identity is keyed on the `Arc<SparkClient>`
        // pointer inside the backend, so reconnecting produces a fresh
        // subscription instead of stale wiring.
        if let Some(spark_backend) = self.wallet_registry.spark() {
            subscriptions.push(spark_backend.event_subscription().map(Message::SparkEvent));
        }

        // Only create tick subscription if we have a vault (daemon exists)
        if self.daemon.is_some() {
            subscriptions.push(
                time::every(Duration::from_secs(
                    match sync_status(
                        self.daemon_backend(),
                        self.cache.blockheight(),
                        self.cache.sync_progress(),
                        self.cache.last_poll_timestamp(),
                        self.cache.last_poll_at_startup,
                    ) {
                        SyncStatus::BlockchainSync(_) => 5, // Only applies to local backends
                        SyncStatus::WalletFullScan
                            if self.daemon_backend() == DaemonBackend::RemoteBackend =>
                        {
                            10
                        } // If remote backend, don't ping too often
                        SyncStatus::WalletFullScan | SyncStatus::LatestWalletSync => 3,
                        SyncStatus::Synced => {
                            if self.daemon_backend() == DaemonBackend::RemoteBackend {
                                // Remote backend has no rescan feature. For a synced wallet,
                                // cache refresh is only used to warn user about recovery availability.
                                120
                            } else {
                                // For the rescan feature, we refresh more often in order
                                // to give user an up-to-date view of the rescan progress.
                                10
                            }
                        }
                    },
                ))
                .map(|_| Message::Tick),
            );
        }

        // Poll pending local Bitcoind IBD progress on a fixed interval,
        // independent of the variable-rate tick subscription.
        if self
            .daemon
            .as_ref()
            .and_then(|d| d.config())
            .and_then(|c| c.pending_bitcoind.as_ref())
            .is_some()
        {
            subscriptions
                .push(time::every(BITCOIND_SYNC_POLL_INTERVAL).map(|_| Message::PollBitcoindSync));
        }

        // Current panel's subscription
        subscriptions.push(
            self.panels
                .current()
                .unwrap_or(&self.panels.global_home)
                .subscription(),
        );

        // Keep P2P subscription alive even when another panel is active,
        // so trade updates and DMs are not lost while navigating elsewhere.
        if !matches!(
            self.panels.current,
            Menu::Marketplace(MarketplaceSubMenu::P2P(_))
        ) {
            if let Some(p2p) = self.panels.p2p.as_ref() {
                subscriptions.push(p2p.subscription());
            }
        }

        // Stream the pending internal bitcoind's debug.log for UpdateTip lines.
        if let Some(pending_cfg) = self
            .daemon
            .as_ref()
            .and_then(|d| d.config())
            .and_then(|c| c.pending_bitcoind.clone())
        {
            let internal_datadir = internal_bitcoind_datadir(&self.cache.datadir_path);
            let is_internal = match &pending_cfg.rpc_auth {
                BitcoindRpcAuth::CookieFile(path) => path.starts_with(&internal_datadir),
                _ => false,
            };
            if is_internal {
                let log_path =
                    internal_bitcoind_debug_log_path(&self.cache.datadir_path, self.cache.network);
                subscriptions.push(
                    iced::Subscription::run_with(log_path, |p| {
                        crate::loader::get_bitcoind_log(p.clone())
                    })
                    .map(Message::PendingBitcoindLog),
                );
            }
        }

        // Connect realtime gRPC stream. Active once `Message::ConnectStreamReady`
        // has populated `connect_stream_config`. The subscription identity is
        // keyed on `(device_id, grpc_url, last_seen_seq)` so reconnecting after
        // any of those change produces a fresh stream instead of stale wiring.
        if let Some(cfg) = self.connect_stream_config.as_ref() {
            subscriptions.push(
                iced::Subscription::run_with(
                    ConnectStreamSubKey { cfg: cfg.clone() },
                    make_connect_stream,
                )
                .map(Message::ConnectStream),
            );
        }

        Subscription::batch(subscriptions)
    }

    pub fn stop(&mut self) {
        info!("Close requested");
        if self.daemon_backend().is_embedded() {
            if let Some(daemon) = &self.daemon {
                if let Err(e) = Handle::current().block_on(async { daemon.stop().await }) {
                    error!("{}", e);
                } else {
                    info!("Internal daemon stopped");
                }
            }
            if let Some(bitcoind) = self.internal_bitcoind.take() {
                bitcoind.stop();
            }
        }
    }

    pub fn on_tick(&mut self) -> Task<Message> {
        // Skip tick processing if no vault is configured
        if self.daemon.is_none() {
            tracing::debug!("Skipping tick - no vault configured");
            return Task::none();
        }

        let tick = std::time::Instant::now();
        let mut tasks = if let Some(daemon) = &self.daemon {
            if let Some(panel) = self.panels.current_mut() {
                vec![panel.update(Some(daemon.clone()), &self.cache, Message::Tick)]
            } else {
                vec![]
            }
        } else {
            vec![]
        };

        // Check if we need to update the daemon cache.
        let duration = Duration::from_secs(
            match sync_status(
                self.daemon_backend(),
                self.cache.blockheight(),
                self.cache.sync_progress(),
                self.cache.last_poll_timestamp(),
                self.cache.last_poll_at_startup,
            ) {
                SyncStatus::BlockchainSync(_) => 5, // Only applies to local backends
                SyncStatus::WalletFullScan
                    if self.daemon_backend() == DaemonBackend::RemoteBackend =>
                {
                    10
                } // If remote backend, don't ping too often
                SyncStatus::WalletFullScan | SyncStatus::LatestWalletSync => 3,
                SyncStatus::Synced => {
                    if self.daemon_backend() == DaemonBackend::RemoteBackend {
                        // Remote backend has no rescan feature. For a synced wallet,
                        // cache refresh is only used to warn user about recovery availability.
                        120
                    } else {
                        // For the rescan feature, we refresh more often in order
                        // to give user an up-to-date view of the rescan progress.
                        10
                    }
                }
            },
        );
        if self.cache.daemon_cache.last_tick + duration <= tick {
            // We have to update here the last_tick to prevent that during a burst of events
            // there is a race condition with the Task and too much tasks are triggered.
            self.cache.daemon_cache.last_tick = tick;

            if let Some(daemon) = &self.daemon {
                let daemon = daemon.clone();
                let datadir_path = self.cache.datadir_path.clone();
                let network = self.cache.network;
                tasks.push(Task::perform(
                    async move {
                        // we check every 10 second if the daemon poller is alive
                        // or if the access token is not expired.
                        daemon.is_alive(&datadir_path, network).await?;

                        let info = daemon.get_info().await?;
                        let coins = cache::coins_to_cache(daemon).await?;
                        Ok(DaemonCache {
                            blockheight: info.block_height,
                            coins: coins.coins,
                            rescan_progress: info.rescan_progress,
                            sync_progress: info.sync,
                            last_poll_timestamp: info.last_poll_timestamp,
                            last_tick: tick,
                        })
                    },
                    Message::UpdateDaemonCache,
                ));
            }
        }

        Task::batch(tasks)
    }

    /// Kick off a background `list_refundables()` poll, debounced so that
    /// SDK events (which can fire several times a second during sync) don't
    /// hammer the SDK. Result comes back as `Message::RefundablesPolled` —
    /// a variant distinct from `RefundablesLoaded` (which manual panel
    /// reloads produce) so that only poll responses touch the App's
    /// debounce and in-flight fields.
    ///
    /// The Transactions panel itself fetches refundables on every reload()
    /// too — this debounced helper covers the case where the user is sitting
    /// on a non-Transactions screen while a swap becomes refundable, so they
    /// still see it the next time they navigate or glance at the app.
    fn refresh_refundables_task(&mut self) -> Task<Message> {
        const DEBOUNCE: std::time::Duration = std::time::Duration::from_secs(30);
        // Skip if a previous fetch is still in flight — otherwise a burst of
        // BreezEvents would launch several concurrent `list_refundables()`
        // calls before any of them returned.
        if self.refundables_fetch_in_flight {
            return Task::none();
        }
        // Debounce against the timestamp of the last *successful* fetch. On
        // failure we leave `last_refundables_fetch` unchanged so the next
        // event can retry immediately instead of being suppressed for 30s.
        if let Some(prev) = self.last_refundables_fetch {
            if std::time::Instant::now().duration_since(prev) < DEBOUNCE {
                return Task::none();
            }
        }
        self.refundables_fetch_in_flight = true;
        let client = self.breez_client.clone();
        Task::perform(
            async move {
                client.list_refundables().await.map(|v| {
                    v.into_iter()
                        .map(crate::app::wallets::DomainRefundableSwap::from)
                        .collect()
                })
            },
            Message::RefundablesPolled,
        )
    }

    /// Top-level handler for `Message::ConnectStream`. PR A logs and
    /// persists the latest event seq; PR B's session-routing logic is
    /// folded in here when the per-modal dispatch lands.
    fn handle_connect_stream(
        &mut self,
        event: crate::services::connect::grpc::ConnectStreamMessage,
    ) -> Task<Message> {
        use crate::services::connect::grpc::ConnectStreamMessage as M;
        match event {
            M::Connected => {
                log::info!("[CONNECT GRPC] Stream connected");
                self.cache.connect_stream_status = ConnectionStatus::Connected;
                stream_health_dispatch(ConnectionStatus::Connected)
            }
            M::Disconnected(reason) => {
                log::warn!("[CONNECT GRPC] Stream disconnected: {}", reason);
                self.cache.connect_stream_status = ConnectionStatus::Connecting;
                stream_health_dispatch(ConnectionStatus::Connecting)
            }
            M::Error(err) => {
                log::warn!("[CONNECT GRPC] Stream error: {}", err);
                let status = ConnectionStatus::Error(err);
                self.cache.connect_stream_status = status.clone();
                stream_health_dispatch(status)
            }
            M::SessionEvent(session_event) => {
                log::info!(
                    "[CONNECT GRPC] SessionEvent seq={} type={:?} session={}",
                    session_event.event_seq,
                    session_event.event_type,
                    session_event.session_id,
                );
                // Persist the latest seq so a restart resumes from the
                // right cursor. Best-effort — log and continue on error.
                let seq = session_event.event_seq;
                let persist_task = if let Some(email) = self.connect_email.clone() {
                    let network_dir = self.datadir.network_directory(self.cache.network);
                    Task::perform(
                        async move {
                            if let Err(e) =
                                crate::services::connect::client::cache::set_last_seen_event_seq_for_email(
                                    &network_dir,
                                    &email,
                                    seq,
                                )
                                .await
                            {
                                log::warn!(
                                    "[CONNECT GRPC] Failed to persist last_seen_event_seq={}: {}",
                                    seq,
                                    e,
                                );
                            }
                        },
                        |_| Message::CacheUpdated,
                    )
                } else {
                    Task::none()
                };
                // Fan the event out via Message::KeychainSign(StreamEvent).
                // It travels through the standard update path and is
                // delegated to the active PSBT modal (if any) by
                // `PsbtState`'s catchall arm — modals that don't
                // recognise the session_id are no-ops.
                let dispatch_task = Task::done(Message::KeychainSign(
                    crate::app::state::vault::keychain_sign::KeychainSignMessage::StreamEvent(
                        session_event,
                    ),
                ));
                Task::batch([persist_task, dispatch_task])
            }
        }
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        let task = self.update_dispatch(message);
        // Sync *after* dispatch: if this update just mutated
        // `self.panels.connect.cube.server_cube_id` (e.g. a
        // `CubeRegistered(Ok)` result) or loaded a wallet, the cache
        // must reflect that by the time the next view render runs.
        // A pre-dispatch sync would miss those same-call mutations
        // and leave view layers one full message cycle behind.
        self.sync_panel_derived_cache_fields();
        task
    }

    /// Mirrors panel-owned state into `Cache` for cheap read access
    /// by view layers and `State::reload()` callbacks that don't
    /// reach into the panel hierarchy. Runs after every `update`
    /// dispatch so same-tick mutations are observable by the next
    /// render.
    fn sync_panel_derived_cache_fields(&mut self) {
        // Authoritative server cube id lives on ConnectPanel; views
        // (Recovery-Kit card, future dashboards) read the Cache
        // mirror. `None` until `CubeRegistered(Ok)` populates the
        // panel's id.
        self.cache.current_cube_server_id = self.panels.connect.cube.server_cube_id;

        // W12 drift detection: SHA-256 over a JSON blob —
        // microseconds — so running it every tick is fine and
        // avoids a separate invalidation pathway tied to wallet
        // changes. When the wallet is absent (no Vault yet) the
        // fingerprint is `None`, which the card treats as "nothing
        // to drift against".
        self.cache.current_descriptor_fingerprint = self.wallet.as_ref().and_then(|w| {
            use crate::app::state::settings::recovery_kit as rk;
            // Canonical API string ("mainnet" for Bitcoin mainnet) —
            // the fingerprint inputs must agree byte-for-byte with
            // the string used at backup time (see `network_str` in
            // `state::settings::recovery_kit`). Any divergence here
            // would make every tick report a spurious drift.
            let network = settings::network_to_api_string(self.cache.network);
            rk::live_descriptor_fingerprint(w.as_ref(), &self.cube_settings.id, &network)
        });
    }

    fn update_dispatch(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::View(view::Message::DismissToast(id)) => {
                self.errors.retain(|(i, ..)| *i != id);
            }
            Message::View(view::Message::ShowError(msg)) => {
                // Redirect ShowError to ShowToast with Error level
                return self.update_dispatch(Message::View(view::Message::ShowToast(
                    log::Level::Error,
                    msg,
                )));
            }
            Message::View(view::Message::ShowSuccess(msg)) => {
                return self.update_dispatch(Message::View(view::Message::ShowToast(
                    log::Level::Info,
                    msg,
                )));
            }
            Message::View(view::Message::ShowToast(level, msg)) => {
                // Show toast with specified level
                self.errors
                    .push((self.current_error_id, std::time::Instant::now(), level, msg));
                self.current_error_id += 1;

                let id = self.current_error_id - 1;
                return Task::perform(
                    async move { tokio::time::sleep(Duration::from_secs(8)).await },
                    move |_| Message::View(view::Message::DismissToast(id)),
                );
            }
            Message::PendingBitcoindLog(log) => {
                if let Some(line) = log {
                    self.cache.node_bitcoind_last_log = Some(line);
                }
            }
            Message::ConnectStreamReady(cfg) => {
                match cfg {
                    Some(cfg) => {
                        tracing::info!(
                            "Connect stream ready (device_id={}, last_seen_seq={})",
                            cfg.device_id,
                            cfg.last_seen_seq,
                        );
                        // Mirror into Cache so deep panels (the open
                        // PSBT modal in particular) can spin up a
                        // GrpcSessionClient on demand.
                        self.cache.connect_grpc_url = Some(cfg.grpc_url.clone());
                        self.cache.connect_tokens = Some(cfg.tokens.clone());
                        self.cache.connect_device_id = Some(cfg.device_id.clone());
                        self.cache.connect_email = self.connect_email.clone();
                        self.connect_stream_config = Some(cfg);
                        // Subscription will mount on the next render
                        // tick — show `Connecting` until the first
                        // `ConnectStreamMessage::Connected` lands.
                        self.cache.connect_stream_status = ConnectionStatus::Connecting;
                    }
                    None => {
                        tracing::debug!(
                            "Connect stream not started: missing grpc_url or device_id",
                        );
                    }
                }
            }
            Message::ConnectStream(event) => {
                return self.handle_connect_stream(event);
            }
            Message::InAppConnectLoginCompleted {
                token,
                refresh_token,
                email,
            } => {
                // Bridge the in-app Connect login → realtime stream
                // bootstrap that the home path gets at App init.
                // Persists JWTs to `connect.json`, registers a signer
                // device via gRPC, and re-fires
                // `connect_stream_ready_task` to populate
                // `cache.connect_grpc_url` / `connect_tokens` /
                // `connect_device_id`. Without this hop "Sign via
                // Keychain" stays unreachable until a full app
                // restart. See `account.rs::post_login_tasks` and
                // PLAN comment near `mod.rs:2374`.
                self.connect_email = Some(email.clone());
                self.cache.connect_email = Some(email.clone());

                let network = self.cache.network;
                let datadir = self.cache.datadir_path.clone();
                // `cache.cube_id` is `String` (empty when no cube yet);
                // the stream task takes `Option<String>` so the realtime
                // subscription can scope events to that cube's vault.
                let cube_uuid = if self.cache.cube_id.is_empty() {
                    None
                } else {
                    Some(self.cache.cube_id.clone())
                };
                let email_for_task = email.clone();

                return Task::perform(
                    async move {
                        use crate::services::connect::client::auth::AccessTokenResponse;
                        use crate::services::connect::client::cache::ConnectCache;
                        use crate::services::connect::grpc::bootstrap::ensure_device_registered_best_effort;
                        use async_fd_lock::LockWrite;
                        use std::io::SeekFrom;
                        use tokio::fs::OpenOptions;
                        use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

                        // Coincube backend issues 30-day JWTs (see
                        // `CLAUDE.md`); we approximate expires_at as
                        // now + 30d so the AccessTokenResponse shape
                        // matches what the home path produces.
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs() as i64)
                            .unwrap_or(0);
                        let tokens = AccessTokenResponse {
                            access_token: token,
                            refresh_token,
                            expires_at: now + 30 * 24 * 60 * 60,
                        };

                        // Persist to `<network_dir>/connect.json` so
                        // the next `connect_stream_ready_task` invocation
                        // (this one, plus future app launches) can read
                        // the device_id back. We write the file
                        // directly instead of going through
                        // `update_connect_cache` because the latter
                        // requires an `AuthClient` we don't have here.
                        let network_dir = datadir.network_directory(network);
                        let mut path = network_dir.path().to_path_buf();
                        path.push("connect.json");
                        if let Some(parent) = path.parent() {
                            let _ = tokio::fs::create_dir_all(parent).await;
                        }

                        let write_result: Result<(), String> = async {
                            let file = OpenOptions::new()
                                .read(true)
                                .write(true)
                                .create(true)
                                .truncate(false)
                                .open(&path)
                                .await
                                .map_err(|e| format!("open connect.json: {e}"))?;
                            let mut guard = file
                                .lock_write()
                                .await
                                .map_err(|e| format!("lock connect.json: {e:?}"))?;
                            let mut buf = Vec::new();
                            guard
                                .read_to_end(&mut buf)
                                .await
                                .map_err(|e| format!("read connect.json: {e}"))?;
                            let mut cache: ConnectCache = if buf.is_empty() {
                                ConnectCache::default()
                            } else {
                                serde_json::from_slice(&buf)
                                    .map_err(|e| format!("parse connect.json: {e}"))?
                            };
                            if let Some(acct) = cache
                                .accounts
                                .iter_mut()
                                .find(|a| a.email == email_for_task)
                            {
                                acct.tokens = tokens.clone();
                            } else {
                                cache.accounts.push(
                                    crate::services::connect::client::cache::Account {
                                        email: email_for_task.clone(),
                                        tokens: tokens.clone(),
                                        device_id: None,
                                        last_seen_event_seq: None,
                                    },
                                );
                            }
                            let serialized = serde_json::to_vec_pretty(&cache)
                                .map_err(|e| format!("serialize: {e}"))?;
                            guard
                                .seek(SeekFrom::Start(0))
                                .await
                                .map_err(|e| format!("seek: {e}"))?;
                            guard
                                .write_all(&serialized)
                                .await
                                .map_err(|e| format!("write: {e}"))?;
                            guard
                                .inner_mut()
                                .set_len(serialized.len() as u64)
                                .await
                                .map_err(|e| format!("truncate: {e}"))?;
                            Ok(())
                        }
                        .await;

                        if let Err(e) = write_result {
                            tracing::warn!(
                                "InAppConnectLoginCompleted: failed to persist tokens: {e}"
                            );
                            return (network, datadir, None, email_for_task, cube_uuid);
                        }

                        // Fetch grpc_url and register the device.
                        let tokens_arc = std::sync::Arc::new(tokio::sync::RwLock::new(tokens));
                        let service_config =
                            match crate::services::connect::client::get_service_config(network)
                                .await
                            {
                                Ok(c) => c,
                                Err(e) => {
                                    tracing::warn!(
                                    "InAppConnectLoginCompleted: get_service_config failed: {e}"
                                );
                                    return (network, datadir, None, email_for_task, cube_uuid);
                                }
                            };
                        if let Some(grpc_url) = service_config.grpc_url.as_deref() {
                            let device_name = std::env::var("HOSTNAME")
                                .ok()
                                .filter(|s| !s.is_empty())
                                .unwrap_or_else(|| {
                                    format!("Coincube Desktop ({})", std::env::consts::OS)
                                });
                            ensure_device_registered_best_effort(
                                grpc_url,
                                tokens_arc.clone(),
                                &network_dir,
                                &email_for_task,
                                device_name,
                                env!("CARGO_PKG_VERSION").to_string(),
                                std::env::consts::OS.to_string(),
                            )
                            .await;
                        }

                        (
                            network,
                            datadir,
                            Some(tokens_arc),
                            email_for_task,
                            cube_uuid,
                        )
                    },
                    |(network, datadir, tokens_opt, email, cube_uuid)| {
                        // Chain the existing stream bootstrap so the
                        // cache fields populate without an app restart.
                        match tokens_opt {
                            Some(tokens) => Message::TriggerConnectStreamReady {
                                network,
                                datadir,
                                tokens,
                                email,
                                cube_uuid,
                            },
                            None => Message::ConnectStreamReady(None),
                        }
                    },
                );
            }
            Message::TriggerConnectStreamReady {
                network,
                datadir,
                tokens,
                email,
                cube_uuid,
            } => {
                return connect_stream_ready_task(network, datadir, tokens, email, cube_uuid);
            }
            Message::InstallStats(_) => {
                if let Some(panel) = self.panels.current_mut() {
                    return panel.update(self.daemon.clone(), &self.cache, message);
                }
            }
            Message::SetInternalBitcoind(bitcoind) => {
                self.internal_bitcoind = Some(bitcoind);
            }
            Message::PollBitcoindSync => {
                if !self.bitcoind_sync_probe_in_progress {
                    if let Some(pending_cfg) = self
                        .daemon
                        .as_ref()
                        .and_then(|d| d.config())
                        .and_then(|c| c.pending_bitcoind.clone())
                    {
                        self.bitcoind_sync_probe_in_progress = true;
                        return Task::perform(
                            check_bitcoind_sync_progress(pending_cfg),
                            Message::BitcoindSyncProgress,
                        );
                    }
                }
            }
            Message::BitcoindSyncProgress(res) => {
                self.bitcoind_sync_probe_in_progress = false;
                match res {
                    Err(e) => tracing::warn!("Bitcoind sync check failed: {}", e),
                    Ok((progress, ibd)) => {
                        let was_in_ibd = self.cache.node_bitcoind_ibd == Some(true);
                        self.cache.node_bitcoind_sync_progress = Some(progress);
                        self.cache.node_bitcoind_ibd = Some(ibd);
                        // Only auto-switch when we have observed the node transition
                        // OUT of IBD (was_in_ibd=true → ibd=false).  This prevents
                        // the immediate reversal that occurs when the
                        // SwitchToConnect flow saves an already-synced Bitcoind
                        // into pending_bitcoind: the first poll would otherwise
                        // see ibd=false and switch back.
                        if !ibd && was_in_ibd {
                            let switch =
                                self.daemon.as_ref().and_then(|d| d.config()).and_then(|c| {
                                    let pending = c.pending_bitcoind.clone()?;
                                    // Preserve the current Connect config as the new fallback.
                                    let old_esplora = match &c.bitcoin_backend {
                                        Some(coincubed::config::BitcoinBackend::Esplora(e)) => {
                                            Some(e.clone())
                                        }
                                        _ => None,
                                    };
                                    let mut new_cfg = c.clone();
                                    new_cfg.bitcoin_backend =
                                        Some(coincubed::config::BitcoinBackend::Bitcoind(pending));
                                    new_cfg.pending_bitcoind = None;
                                    new_cfg.fallback_esplora = old_esplora;
                                    Some(new_cfg)
                                });
                            if let Some(new_cfg) = switch {
                                let datadir = self.cache.datadir_path.clone();
                                match self.load_daemon_config(datadir, new_cfg) {
                                    Ok(()) => {
                                        info!("Switched to local Bitcoind — IBD complete");
                                        self.cache.node_bitcoind_sync_progress = None;
                                        self.cache.node_bitcoind_ibd = None;
                                        self.cache.node_bitcoind_last_log = None;
                                        let cfg_task = self
                                            .update_dispatch(Message::DaemonConfigLoaded(Ok(())));
                                        return Task::batch([
                                            cfg_task,
                                            Task::done(Message::CacheUpdated),
                                        ]);
                                    }
                                    Err(e) => error!("Failed to switch to Bitcoind: {}", e),
                                }
                            }
                        }
                    }
                }
            }
            Message::SettingsSaved => {
                // Settings saved - reload unit preference and fiat_price from cube settings
                let network_dir = self
                    .cache
                    .datadir_path
                    .network_directory(self.cache.network);
                if let Ok(settings) = settings::Settings::from_file(&network_dir) {
                    if let Some(cube) = settings
                        .cubes
                        .iter()
                        .find(|c| c.id == self.cube_settings.id)
                    {
                        self.cache.bitcoin_unit = cube.unit_setting.display_unit;
                        self.cube_settings.fiat_price = cube.fiat_price.clone();
                        // Keep the "backed up" banner state in sync with
                        // whatever was persisted — the backup flow saves
                        // cube.backed_up = true via this same path. If the
                        // backed-up state transitions back to false, also
                        // clear the session dismissal so the banner
                        // resurfaces for the new state.
                        if self.cache.current_cube_backed_up && !cube.backed_up {
                            self.cache.backup_warning_dismissed = false;
                        }
                        self.cache.current_cube_backed_up = cube.backed_up;
                        self.cache.current_cube_is_passkey = cube.is_passkey_cube();
                        self.cube_settings.backed_up = cube.backed_up;
                        // Mirror the drift fingerprint cache (W12). Refreshing
                        // on every SettingsSaved keeps the Recovery-Kit card
                        // in sync after a successful upload or remove.
                        self.cache
                            .recovery_kit_last_backed_up_descriptor_fingerprint = cube
                            .recovery_kit_last_backed_up_descriptor_fingerprint
                            .clone();
                        self.cube_settings
                            .recovery_kit_last_backed_up_descriptor_fingerprint = cube
                            .recovery_kit_last_backed_up_descriptor_fingerprint
                            .clone();

                        // Clear cached fiat display price if disabled.
                        // Note: btc_usd_price is NOT cleared — it's needed for
                        // USDt→sats conversion regardless of fiat display setting.
                        if !cube.fiat_price.as_ref().is_some_and(|p| p.is_enabled) {
                            self.cache.fiat_price = None;
                        }
                    }
                }

                // Reload global settings into cache
                {
                    use settings::global::GlobalSettings;
                    let global_path = GlobalSettings::path(&self.cache.datadir_path);
                    self.cache.show_direction_badges =
                        GlobalSettings::load_show_direction_badges(&global_path);
                }

                // Forward to state panels so they can reload their internal state
                if let Some(panel) = self.panels.current_mut() {
                    return Task::batch(vec![
                        panel.update(self.daemon.clone(), &self.cache, message),
                        Task::done(Message::CacheUpdated),
                    ]);
                }

                return Task::done(Message::CacheUpdated);
            }
            Message::Fiat(FiatMessage::GetPriceResult(fiat_price)) => {
                let mut updated = false;

                // Always extract BTC/USD price for USDt→sats conversion,
                // regardless of whether fiat display is enabled.
                if fiat_price.currency() == crate::services::fiat::Currency::USD {
                    if let Ok(price) = fiat_price.res.as_ref() {
                        self.cache.btc_usd_price = Some(price.value);
                        updated = true;
                    }
                }

                // Store user's selected currency price (only when fiat display is enabled).
                let is_relevant = self.cube_settings.fiat_price.as_ref().is_some_and(|sett| {
                    sett.is_enabled
                        && sett.source == fiat_price.source()
                        && sett.currency == fiat_price.currency()
                });

                if is_relevant
                    // make sure we only update if the price is newer than the cached one
                    && !self.cache.fiat_price.as_ref().is_some_and(|cached| {
                        cached.source() == fiat_price.source()
                            && cached.currency() == fiat_price.currency()
                            && cached.requested_at() >= fiat_price.requested_at()
                    })
                {
                    self.cache.fiat_price = Some(fiat_price);
                    updated = true;
                }

                if updated {
                    return Task::done(Message::CacheUpdated);
                }
            }
            Message::UpdateDaemonCache(res) => match res {
                Ok(mut daemon_cache) => {
                    // Apply optimistic-broadcast overrides before the
                    // cache is published: reconcile drops entries the
                    // daemon now reflects on its own, then any still-
                    // pending broadcasts get synthetic `spend_info` so
                    // `coins_summary` (Vault balance) and every other
                    // `cache.coins()` consumer treats the inputs as
                    // already spent.
                    if let Some(wallet) = &self.wallet {
                        wallet.reconcile_with_coins(&daemon_cache.coins);
                        wallet.apply_coin_overrides(&mut daemon_cache.coins);
                    }
                    self.cache.daemon_cache = daemon_cache;
                    return Task::done(Message::CacheUpdated);
                }
                Err(e) => {
                    tracing::error!("Failed to update daemon cache: {}", e);
                    // If the active Bitcoind daemon has failed and a Connect
                    // Esplora fallback is configured (set when IBD completed),
                    // restart using Connect — but only on transport/stopped
                    // errors, not transient RPC application-level responses.
                    if is_daemon_unreachable(&e) {
                        let fallback = self
                            .daemon
                            .as_ref()
                            .filter(|d| {
                                matches!(
                                    d.backend(),
                                    DaemonBackend::EmbeddedCoincubed(Some(NodeType::Bitcoind))
                                )
                            })
                            .and_then(|d| d.config())
                            .and_then(|c| {
                                c.fallback_esplora.as_ref().map(|fb| {
                                    let mut new_cfg = c.clone();
                                    // Demote the current Bitcoind to
                                    // `pending_bitcoind` so the syncing card
                                    // reappears and the user can retry once
                                    // the node is healthy. Without this the
                                    // fallback strands the user on Connect
                                    // with an empty pending slot, which
                                    // surfaces the "Set up local node" prompt
                                    // and forces a full re-install.
                                    let preserved_bitcoind = match &c.bitcoin_backend {
                                        Some(coincubed::config::BitcoinBackend::Bitcoind(bc)) => {
                                            Some(bc.clone())
                                        }
                                        _ => None,
                                    };
                                    new_cfg.bitcoin_backend = Some(
                                        coincubed::config::BitcoinBackend::Esplora(fb.clone()),
                                    );
                                    new_cfg.pending_bitcoind = preserved_bitcoind;
                                    new_cfg.fallback_esplora = None;
                                    new_cfg
                                })
                            });
                        if let Some(new_cfg) = fallback {
                            let datadir = self.cache.datadir_path.clone();
                            match self.load_daemon_config(datadir, new_cfg) {
                                Ok(()) => {
                                    info!("Switched to COINCUBE | Connect fallback after Bitcoind failure");
                                    let cfg_task =
                                        self.update_dispatch(Message::DaemonConfigLoaded(Ok(())));
                                    return Task::batch([
                                        cfg_task,
                                        Task::done(Message::CacheUpdated),
                                    ]);
                                }
                                Err(e) => error!("Failed to activate Connect fallback: {}", e),
                            }
                        }
                    }
                }
            },
            Message::CacheUpdated => {
                // Cube (Home) Settings lives on every cube, vault or not,
                // so its cache update must fire independently of the
                // vault-panel branch below. Vault Settings and Cube
                // Settings are distinct panels backed by separate state —
                // each panel's "am I current?" flag only matches the one
                // it actually owns.
                let is_global_settings_current = matches!(
                    &self.panels.current,
                    Menu::Cube(crate::app::menu::CubeSubMenu::Settings(_))
                );
                let mut commands = vec![self.panels.global_settings.update(
                    self.daemon.clone(),
                    &self.cache,
                    Message::UpdatePanelCache(is_global_settings_current),
                )];

                // Vault-specific panels only exist on cubes with a
                // configured vault.
                if let (Some(daemon), Some(vault_overview), Some(vault_settings)) = (
                    &self.daemon,
                    self.panels.vault_overview.as_mut(),
                    self.panels.vault_settings.as_mut(),
                ) {
                    let daemon = daemon.clone();
                    let current = &self.panels.current;
                    let cache = self.cache.clone();

                    let is_vault_settings_current = matches!(
                        current,
                        Menu::Vault(crate::app::menu::VaultSubMenu::Settings(_))
                    );
                    let is_spend_current =
                        matches!(current, Menu::Vault(crate::app::menu::VaultSubMenu::Send));

                    commands.push(vault_overview.update(
                        Some(daemon.clone()),
                        &cache,
                        Message::UpdatePanelCache(
                            current == &Menu::Vault(crate::app::menu::VaultSubMenu::Overview),
                        ),
                    ));
                    commands.push(vault_settings.update(
                        Some(daemon.clone()),
                        &cache,
                        Message::UpdatePanelCache(is_vault_settings_current),
                    ));

                    // Also update create_spend panel if it exists
                    if let Some(create_spend) = self.panels.create_spend.as_mut() {
                        commands.push(create_spend.update(
                            Some(daemon.clone()),
                            &cache,
                            Message::UpdatePanelCache(is_spend_current),
                        ));
                    }
                }

                return Task::batch(commands);
            }
            Message::LoadDaemonConfig(cfg) => {
                // Only load daemon config if we have a vault (daemon and wallet exist)
                if self.daemon.is_some() && self.wallet.is_some() {
                    // If pending_bitcoind is being cleared (e.g. manual SwitchToBitcoind),
                    // clear the associated sync progress fields so the vault overview
                    // stops showing a stale "syncing" card.
                    let pending_cleared = self
                        .daemon
                        .as_ref()
                        .and_then(|d| d.config())
                        .map(|c| c.pending_bitcoind.is_some())
                        .unwrap_or(false)
                        && cfg.pending_bitcoind.is_none();
                    if pending_cleared {
                        self.cache.node_bitcoind_sync_progress = None;
                        self.cache.node_bitcoind_ibd = None;
                        self.cache.node_bitcoind_last_log = None;
                    }
                    let res = self.load_daemon_config(self.cache.datadir_path.clone(), *cfg);
                    return self.update_dispatch(Message::DaemonConfigLoaded(res));
                } else {
                    tracing::warn!("Attempted to load daemon config without vault");
                }
            }
            Message::WalletUpdated(Ok(wallet)) => {
                // Check if we're transitioning from no-vault to has-vault state
                let was_vaultless = !self.cache.has_vault;

                self.wallet = Some(wallet.clone());
                self.cache.has_vault = true;

                // If we didn't have a vault before, rebuild all vault panels
                if was_vaultless {
                    if let Some(daemon) = &self.daemon {
                        self.panels.build_vault_panels(
                            wallet.clone(),
                            &self.cache,
                            daemon.backend(),
                            self.datadir.clone(),
                            self.internal_bitcoind.as_ref(),
                            self.config.clone(),
                            self.breez_client.clone(),
                        );
                    }

                    // W10 — nudge the user to back up the freshly-created
                    // Vault to their Connect Recovery Kit. Fires
                    // `LoadStatus` now; the `StatusLoaded` handler in
                    // `state::settings::recovery_kit` reads this flag
                    // and emits the toast only if the freshly-loaded
                    // status shows the descriptor isn't already
                    // backed up. Gating on the in-memory `status`
                    // here (pre-fetch) would misfire: on app startup
                    // and after Connect sign-out the cached value is
                    // `None` even for users whose kit is complete.
                    //
                    // Both the flag and the `LoadStatus` dispatch are
                    // gated on auth — unauthenticated users have no
                    // Connect account to fetch against, and dispatching
                    // the message anyway would just round-trip through
                    // `load_status`'s early-return. Skipping saves the
                    // message-queue hop and keeps the intent obvious.
                    let nudge_task: Option<Task<Message>> =
                        if self.panels.connect.account.is_authenticated() {
                            self.panels
                                .global_settings
                                .recovery_kit
                                .nudge_on_next_status_load = true;
                            Some(Task::done(Message::View(view::Message::Settings(
                                view::SettingsMessage::RecoveryKit(
                                    view::RecoveryKitMessage::LoadStatus,
                                ),
                            ))))
                        } else {
                            None
                        };
                    // Forward to the current panel; batch the nudge in
                    // only when we actually constructed one.
                    if let (Some(daemon), Some(panel)) =
                        (self.daemon.clone(), self.panels.current_mut())
                    {
                        let panel_task = panel.update(
                            Some(daemon),
                            &self.cache,
                            Message::WalletUpdated(Ok(wallet)),
                        );
                        return match nudge_task {
                            Some(nudge) => Task::batch([panel_task, nudge]),
                            None => panel_task,
                        };
                    }
                    return nudge_task.unwrap_or_else(Task::none);
                }

                // Forward the message to the current panel
                if let (Some(daemon), Some(panel)) =
                    (self.daemon.clone(), self.panels.current_mut())
                {
                    return panel.update(
                        Some(daemon),
                        &self.cache,
                        Message::WalletUpdated(Ok(wallet)),
                    );
                }
            }
            Message::View(view::Message::Menu(menu)) => {
                // Always honor the navigation even when the current
                // panel has no instance (e.g. the user landed on an
                // orphan route like Marketplace(BuySell) with no vault).
                // Otherwise rail clicks get silently dropped and the
                // user is trapped on whichever screen is rendering.
                //
                // We deliberately do not touch
                // `pending_switch_to_connect_after_login` here. Sign-in
                // happens on the Home tab now, so the user is expected
                // to switch tabs (and possibly poke around this one)
                // while it's in flight — the flag is consumed on the
                // auth-success edge a few branches below, or on logout.
                let close_task = self
                    .panels
                    .current_mut()
                    .map(|p| p.close())
                    .unwrap_or_else(Task::none);
                return Task::batch([close_task, self.set_current_panel(menu)]);
            }
            msg @ Message::View(view::Message::ConnectAccount(_))
            | msg @ Message::View(view::Message::ConnectCube(_)) => {
                let was_authenticated = self.cache.connect_authenticated;
                let task = self
                    .panels
                    .connect
                    .update(self.daemon.clone(), &self.cache, msg);
                self.cache.connect_authenticated = self.panels.connect.account.is_authenticated();
                // Sync lightning address to cache for sidebar display
                self.cache.lightning_address = self
                    .panels
                    .connect
                    .cube
                    .lightning_address
                    .as_ref()
                    .and_then(|la| {
                        la.lightning_address.as_ref().map(|addr| {
                            if addr.contains('@') {
                                addr.clone()
                            } else {
                                format!("{}{}", addr, "@coincube.io")
                            }
                        })
                    });
                if let Some(p2p) = self.panels.p2p.as_mut() {
                    p2p.sync_lightning_address_from_cache(&self.cache);
                }
                // Sync avatar handle to cache for sidebar display across all panels.
                // Only update when Some to avoid blinking during in-flight image loads.
                // Clear on logout when auth state transitions from true to false.
                if let Some(handle) = self.panels.connect.cube.get_active_avatar_handle() {
                    self.cache.avatar_handle = Some(handle);
                } else if was_authenticated && !self.cache.connect_authenticated {
                    // Logout occurred - clear the avatar
                    self.cache.avatar_handle = None;
                }
                // Connect logout: tear down the realtime stream. The
                // subscription is keyed on `connect_stream_config`, so
                // clearing it (plus the cache mirrors) drops the gRPC
                // stream on the next `subscription()` tick — Iced's
                // model is declarative, there is no task handle to
                // cancel. NOTE: a subsequent in-place relogin does not
                // yet rebuild the stream (would need the token Arc +
                // email re-plumbed from the account panel); the stream
                // currently only re-establishes on app restart.
                if was_authenticated && !self.cache.connect_authenticated {
                    self.connect_stream_config = None;
                    self.connect_email = None;
                    self.cache.connect_grpc_url = None;
                    self.cache.connect_tokens = None;
                    self.cache.connect_device_id = None;
                    self.cache.connect_email = None;
                    self.cache.connect_stream_status = ConnectionStatus::Inactive;
                    // Logout breaks the "Switch to Connect" trip the
                    // user started; firing the auto-return on a fresh
                    // unrelated login later would be surprising.
                    self.pending_switch_to_connect_after_login = false;
                }
                // Auto-return for the "Switch to Connect" flow. When the user
                // clicked it without an active session, we routed them to the
                // Connect tab and set this flag. Now that they've signed in,
                // jump back to Vault → Settings → Node and re-fire the switch
                // — which will fast-path through the new session's JWT.
                if !was_authenticated
                    && self.cache.connect_authenticated
                    && self.pending_switch_to_connect_after_login
                {
                    self.pending_switch_to_connect_after_login = false;
                    let nav = self.set_current_panel(menu::Menu::Vault(
                        menu::VaultSubMenu::Settings(Some(menu::SettingsOption::Node)),
                    ));
                    let switch = Task::done(Message::View(view::Message::Settings(
                        view::SettingsMessage::NodeSettings(
                            view::NodeSettingsMessage::SwitchToConnect,
                        ),
                    )));
                    return Task::batch([task, nav, switch]);
                }
                return task;
            }
            Message::View(view::Message::DismissReceivedCelebration) => {
                self.show_received_celebration = false;
                // Panels that render their own celebration overlay
                // (e.g. the Vault overview) keep a separate
                // `show_received_celebration` flag and reuse this same
                // global dismiss message. Clearing only the app-level
                // flag here would leave the panel stuck on the
                // celebration screen, so forward the dismiss to the
                // active panel as well — mirrors the generic
                // message-forwarding catch-all below.
                if let (Some(daemon), Some(panel)) =
                    (self.daemon.clone(), self.panels.current_mut())
                {
                    return panel.update(
                        Some(daemon),
                        &self.cache,
                        Message::View(view::Message::DismissReceivedCelebration),
                    );
                } else if let Some(panel) = self.panels.current_mut() {
                    return panel.update(
                        None,
                        &self.cache,
                        Message::View(view::Message::DismissReceivedCelebration),
                    );
                }
            }
            Message::View(view::Message::DismissBackupWarning) => {
                self.cache.backup_warning_dismissed = true;
            }
            Message::View(view::Message::FlipDisplayMode) => {
                let new_mode = self.cache.display_mode.flipped();
                self.cache.display_mode = new_mode;
                let network_dir = self.datadir.network_directory(self.cache.network);
                return Task::perform(
                    async move {
                        settings::update_settings_file(&network_dir, move |mut current| {
                            current.display_mode = new_mode;
                            Some(current)
                        })
                        .await
                    },
                    |res| {
                        if let Err(e) = res {
                            tracing::warn!("Failed to persist display_mode: {}", e);
                        }
                        Message::Tick
                    },
                );
            }
            Message::View(view::Message::OpenUrl(url)) => {
                if let Err(e) = open::that_detached(&url) {
                    tracing::error!("Error opening '{}': {}", url, e);
                }
            }
            Message::View(view::Message::Clipboard(text)) => return clipboard::write(text),
            msg @ Message::View(view::Message::Home(_)) => {
                return self
                    .panels
                    .global_home
                    .update(self.daemon.clone(), &self.cache, msg);
            }

            Message::SparkEvent(client_event) => {
                use coincube_spark_protocol::Event as SparkEvent;
                let crate::app::breez_spark::SparkClientEvent(event) = client_event;
                log::info!("App received Spark event: {:?}", event);

                let mut tasks: Vec<Task<Message>> = Vec::new();

                // Refresh Spark Overview on every event — balance
                // moves on any payment state change, and `Synced`
                // ticks are the SDK's "you're up to date, re-read
                // state" signal. Deposits being claimed counts as a
                // balance change too.
                tasks.push(self.panels.spark_overview.reload(None, None));

                // Also refresh the Home (Cube → Overview) Spark card.
                // `global_home.reload` only runs on navigation, so on
                // cold start the first `get_info` may return the SDK's
                // persisted pre-sync value (e.g. zero before this
                // session's incoming payments landed). The Home state
                // gates `spark_balance_loaded` on observing at least
                // one `Synced` from the bridge — `SparkSyncedObserved`
                // (sent only for `SparkEvent::Synced`) flips that gate,
                // and the `RefreshSparkBalance` dispatched alongside
                // re-fetches whatever the SDK can now report.
                // A periodic poll in `GlobalHome::subscription` is the
                // safety net for the case where `Synced` fires before
                // iced subscribes (tokio broadcast doesn't replay).
                if matches!(event, SparkEvent::Synced) {
                    tasks.push(Task::done(Message::View(view::Message::Home(
                        view::HomeMessage::SparkSyncedObserved,
                    ))));
                }
                tasks.push(Task::done(Message::View(view::Message::Home(
                    view::HomeMessage::RefreshSparkBalance,
                ))));

                // Payment-related events reload the Transactions list
                // so newly surfaced rows appear without the user
                // manually navigating / pressing refresh. `Synced`
                // and `DepositsChanged` alone don't imply new
                // payment-list rows.
                if matches!(
                    event,
                    SparkEvent::PaymentSucceeded { .. }
                        | SparkEvent::PaymentPending { .. }
                        | SparkEvent::PaymentFailed { .. }
                ) {
                    tasks.push(self.panels.spark_transactions.reload(None, None));
                }

                match event {
                    SparkEvent::PaymentSucceeded {
                        amount_sat, bolt11, ..
                    } => {
                        // Phase 4f: forward the BOLT11 field so the
                        // Receive panel can correlate against its
                        // currently displayed invoice.
                        tasks.push(Task::done(Message::View(view::Message::SparkReceive(
                            view::SparkReceiveMessage::PaymentReceived { amount_sat, bolt11 },
                        ))));
                    }
                    SparkEvent::DepositsChanged => {
                        // Phase 4f: refresh the Receive panel's
                        // pending deposits card. The panel handles
                        // the actual `list_unclaimed_deposits` RPC
                        // dispatch.
                        tasks.push(Task::done(Message::View(view::Message::SparkReceive(
                            view::SparkReceiveMessage::DepositsChanged,
                        ))));
                        // Transfer-redesign follow-up: the Home state tracks a
                        // `pending_spark_incoming` indicator for transfer-initiated
                        // deposits (VaultToSpark / LiquidToSpark). Forward the
                        // event so Home can reconcile its own view (auto-claim a
                        // matured deposit, or clear the indicator once claimed).
                        tasks.push(Task::done(Message::View(view::Message::Home(
                            view::HomeMessage::SparkDepositsChanged,
                        ))));
                    }
                    SparkEvent::LightningAddressChanged { info } => {
                        // Phase 4g: forward to ConnectCube so it can
                        // refresh its view and auto-re-register if
                        // the SDK state went Some → None unexpectedly.
                        tasks.push(Task::done(Message::View(view::Message::ConnectCube(
                            view::ConnectCubeMessage::SparkLightningAddressChanged(info),
                        ))));
                    }
                    _ => {}
                }

                return Task::batch(tasks);
            }

            Message::BreezEvent(event) => {
                use breez_sdk_liquid::prelude::{PaymentDetails, PaymentType, SdkEvent};
                log::info!("App received Breez Event: {:?}", event);

                let swap_id_for_bitcoin_send = |details: &breez_sdk_liquid::prelude::Payment| {
                    if matches!(details.payment_type, PaymentType::Send) {
                        match &details.details {
                            PaymentDetails::Bitcoin { swap_id, .. } => Some(swap_id.clone()),
                            _ => None,
                        }
                    } else {
                        None
                    }
                };

                match event {
                    SdkEvent::PaymentWaitingFeeAcceptance { details } => {
                        log::info!("Payment waiting for fee acceptance: {:?}", details);
                        let client = self.breez_client.clone();

                        return Task::perform(
                            async move {
                                if let PaymentDetails::Bitcoin { swap_id, .. } = details.details {
                                    match client.fetch_payment_proposed_fees(&swap_id).await {
                                        Ok(fees_response) => {
                                            log::info!(
                                                "Accepting fees for swap {}: payer_amount={}, fees={}",
                                                swap_id,
                                                fees_response.payer_amount_sat,
                                                fees_response.fees_sat
                                            );
                                            if let Err(e) = client
                                                .accept_payment_proposed_fees(fees_response)
                                                .await
                                            {
                                                log::error!("Failed to accept payment fees: {}", e);
                                                Err(format!("Failed to accept payment fees: {}", e))
                                            } else {
                                                log::info!(
                                                    "Successfully accepted fees for swap {}",
                                                    swap_id
                                                );
                                                Ok(())
                                            }
                                        }
                                        Err(e) => {
                                            log::error!("Failed to fetch proposed fees: {}", e);
                                            Err(format!("Failed to fetch proposed fees: {}", e))
                                        }
                                    }
                                } else {
                                    Ok(())
                                }
                            },
                            |result| {
                                if let Err(err) = result {
                                    log::error!("Fee acceptance failed: {}", err);
                                }
                                // Trigger a cache update to refresh balance displays
                                Message::Tick
                            },
                        );
                    }
                    SdkEvent::PaymentPending { details } => {
                        let home_task = swap_id_for_bitcoin_send(&details).map(|swap_id| {
                            Task::done(Message::View(view::Message::Home(
                                view::HomeMessage::LiquidToVaultPending(Some(swap_id)),
                            )))
                        });

                        // Refresh only the active liquid panel + home balance.
                        // Inactive panels refresh when navigated to via reload().
                        let mut tasks = vec![
                            Task::done(Message::View(view::Message::Home(
                                view::HomeMessage::RefreshLiquidBalance,
                            ))),
                            home_task.unwrap_or_else(Task::none),
                        ];
                        if let Some(msg) = self.panels.active_liquid_refresh(true) {
                            tasks.push(Task::done(msg));
                        }
                        return Task::batch(tasks);
                    }
                    SdkEvent::PaymentSucceeded { details } => {
                        // Show global celebration for incoming payments
                        if matches!(details.payment_type, PaymentType::Receive) {
                            use coincube_ui::component::amount::DisplayAmount;
                            let usdt_id =
                                crate::app::breez_liquid::assets::usdt_asset_id(self.cache.network);
                            // Mirror the check in state/liquid/receive.rs: a
                            // payment is considered USDt only when it's a
                            // Liquid asset with the matching asset_id AND
                            // `asset_info` is populated so we can format the
                            // minor-unit amount.
                            let usdt_amount_minor: Option<u64> = match &details.details {
                                PaymentDetails::Liquid {
                                    asset_id,
                                    asset_info,
                                    ..
                                } if usdt_id.is_some_and(|id| id == asset_id) => {
                                    asset_info.as_ref().map(|info| {
                                        crate::app::breez_liquid::assets::usdt_amount_to_minor(
                                            info.amount,
                                        )
                                    })
                                }
                                _ => None,
                            };
                            let context = if usdt_amount_minor.is_some() {
                                "note-receive"
                            } else {
                                match &details.details {
                                    PaymentDetails::Lightning { .. } => "lightning-receive",
                                    PaymentDetails::Bitcoin { .. } => "bitcoin-receive",
                                    _ => "liquid-receive",
                                }
                            };
                            self.received_celebration_amount = if let Some(minor) =
                                usdt_amount_minor
                            {
                                format!(
                                    "{} USDt",
                                    crate::app::breez_liquid::assets::format_usdt_display(minor)
                                )
                            } else {
                                bitcoin::Amount::from_sat(details.amount_sat)
                                    .to_formatted_string_with_unit(self.cache.bitcoin_unit)
                            };
                            self.received_celebration_context = context.to_string();
                            self.received_celebration_quote =
                                coincube_ui::component::quote_display::random_quote(context);
                            self.received_celebration_image =
                                coincube_ui::component::quote_display::image_handle_for_context(
                                    context,
                                );
                            self.show_received_celebration = true;
                        }

                        let home_task = swap_id_for_bitcoin_send(&details).map(|swap_id| {
                            Task::done(Message::View(view::Message::Home(
                                view::HomeMessage::LiquidToVaultSucceeded(Some(swap_id)),
                            )))
                        });

                        let mut tasks = vec![
                            Task::done(Message::View(view::Message::Home(
                                view::HomeMessage::RefreshLiquidBalance,
                            ))),
                            home_task.unwrap_or_else(Task::none),
                        ];
                        // Transfer-redesign follow-up: a peg-in (BTC on-chain →
                        // L-BTC) completing is the event we need to clear the
                        // Liquid card's pending-receive indicator after a
                        // VaultToLiquid or SparkToLiquid transfer. Only counts
                        // when the incoming payment is the Bitcoin swap leg.
                        if matches!(details.payment_type, PaymentType::Receive)
                            && matches!(details.details, PaymentDetails::Bitcoin { .. })
                        {
                            tasks.push(Task::done(Message::View(view::Message::Home(
                                view::HomeMessage::LiquidPeginCompleted {
                                    amount_sat: details.amount_sat,
                                },
                            ))));
                        }
                        if let Some(msg) = self.panels.active_liquid_refresh(true) {
                            tasks.push(Task::done(msg));
                        }
                        return Task::batch(tasks);
                    }
                    SdkEvent::PaymentFailed { details } => {
                        let home_task = swap_id_for_bitcoin_send(&details).map(|swap_id| {
                            Task::done(Message::View(view::Message::Home(
                                view::HomeMessage::LiquidToVaultFailed(Some(swap_id)),
                            )))
                        });

                        let mut tasks = vec![
                            Task::done(Message::View(view::Message::Home(
                                view::HomeMessage::RefreshLiquidBalance,
                            ))),
                            home_task.unwrap_or_else(Task::none),
                        ];
                        if let Some(msg) = self.panels.active_liquid_refresh(true) {
                            tasks.push(Task::done(msg));
                        }
                        // A failed BTC→L-BTC swap may have become refundable — let the
                        // transactions panel know so the user sees the Refund CTA.
                        tasks.push(self.refresh_refundables_task());
                        return Task::batch(tasks);
                    }
                    SdkEvent::PaymentRefundable { details } => {
                        log::info!(
                            target: "breez_swap",
                            "SdkEvent::PaymentRefundable tx_id={:?}",
                            details.tx_id.as_deref().map(|t| truncate_middle(t, 6, 6))
                        );
                        let mut tasks = Vec::new();
                        if let Some(msg) = self.panels.active_liquid_refresh(true) {
                            tasks.push(Task::done(msg));
                        }
                        tasks.push(self.refresh_refundables_task());
                        return Task::batch(tasks);
                    }
                    SdkEvent::PaymentRefundPending { details } => {
                        log::info!(
                            target: "breez_swap",
                            "SdkEvent::PaymentRefundPending tx_id={:?}",
                            details.tx_id.as_deref().map(|t| truncate_middle(t, 6, 6))
                        );
                        let mut tasks = Vec::new();
                        if let Some(msg) = self.panels.active_liquid_refresh(true) {
                            tasks.push(Task::done(msg));
                        }
                        tasks.push(self.refresh_refundables_task());
                        return Task::batch(tasks);
                    }
                    SdkEvent::PaymentRefunded { details } => {
                        log::info!(
                            target: "breez_swap",
                            "SdkEvent::PaymentRefunded tx_id={:?}",
                            details.tx_id.as_deref().map(|t| truncate_middle(t, 6, 6))
                        );
                        let mut tasks = vec![Task::done(Message::View(view::Message::Home(
                            view::HomeMessage::RefreshLiquidBalance,
                        )))];
                        if let Some(msg) = self.panels.active_liquid_refresh(true) {
                            tasks.push(Task::done(msg));
                        }
                        tasks.push(self.refresh_refundables_task());
                        return Task::batch(tasks);
                    }
                    SdkEvent::PaymentWaitingConfirmation { details } => {
                        let home_task = swap_id_for_bitcoin_send(&details).map(|swap_id| {
                            Task::done(Message::View(view::Message::Home(
                                view::HomeMessage::LiquidToVaultWaitingConfirmation(Some(swap_id)),
                            )))
                        });

                        let mut tasks = vec![
                            Task::done(Message::View(view::Message::Home(
                                view::HomeMessage::RefreshLiquidBalance,
                            ))),
                            home_task.unwrap_or_else(Task::none),
                        ];
                        if let Some(msg) = self.panels.active_liquid_refresh(true) {
                            tasks.push(Task::done(msg));
                        }

                        // Notify the user that an incoming Lightning payment is
                        // mid-swap to L-BTC. The swap can take a couple of minutes,
                        // so without this toast the wait between PaymentWaitingConfirmation
                        // and PaymentSucceeded looks like nothing is happening.
                        // Breez fires this event multiple times for the same swap, so
                        // dedupe by tx_id to avoid stacking duplicate toasts.
                        if matches!(details.payment_type, PaymentType::Receive)
                            && details.tx_id.as_ref().is_some_and(|id| {
                                !self.toasted_incoming_waiting_tx_ids.contains(id)
                            })
                        {
                            let tx_id = details.tx_id.clone().unwrap();
                            if self.toasted_incoming_waiting_tx_ids.len() == 16 {
                                self.toasted_incoming_waiting_tx_ids.pop_front();
                            }
                            self.toasted_incoming_waiting_tx_ids.push_back(tx_id);
                            use coincube_ui::component::amount::DisplayAmount;
                            let amount = bitcoin::Amount::from_sat(details.amount_sat)
                                .to_formatted_string_with_unit(self.cache.bitcoin_unit);
                            tasks.push(Task::done(Message::View(view::Message::ShowToast(
                                log::Level::Info,
                                format!(
                                    "Incoming payment of {} — swapping to L-BTC, awaiting confirmation",
                                    amount
                                ),
                            ))));
                        }

                        return Task::batch(tasks);
                    }
                    SdkEvent::Synced => {
                        // SDK completed an internal sync — refresh only the
                        // active liquid panel to avoid redundant info() calls.
                        // Inactive panels refresh when navigated to via reload().
                        let mut tasks = Vec::new();
                        if let Some(msg) = self.panels.active_liquid_refresh(false) {
                            tasks.push(Task::done(msg));
                        }
                        // Debounced refundables poll — picks up older expired
                        // swaps that didn't emit an explicit refundable event
                        // while the app was offline. Always enqueued, so this
                        // arm unconditionally returns.
                        tasks.push(self.refresh_refundables_task());
                        return Task::batch(tasks);
                    }
                    _ => {
                        // Other events - just log
                        log::debug!("Unhandled Breez event: {:?}", event);
                    }
                }
            }

            // Route P2P messages directly to the P2P panel regardless of active menu,
            // so real-time trade updates are processed even when viewing other panels.
            msg @ Message::View(view::Message::P2P(_)) => {
                if let Some(p2p) = self.panels.p2p.as_mut() {
                    return p2p.update(self.daemon.clone(), &self.cache, msg);
                }
            }

            // Intercept the mnemonic backup completion so the "not backed up"
            // warning banners on the Vault/Liquid home screens disappear
            // immediately. Route the message directly to the global settings
            // panel (rather than `current_mut()`) so the backup flow still
            // transitions to Completed and scrubs `backup_mnemonic` even if
            // the user navigated away from Settings before the async write
            // resolved.
            msg @ Message::View(view::Message::Settings(
                view::SettingsMessage::BackupMasterSeedUpdated,
            )) => {
                self.cache.current_cube_backed_up = true;
                self.cube_settings.backed_up = true;
                return self
                    .panels
                    .global_settings
                    .update(self.daemon.clone(), &self.cache, msg);
            }

            // Vault → Settings → Node "Switch to COINCUBE | Connect". The
            // canonical Connect session lives in `panels.connect.account`; we
            // either reuse its JWT for an immediate switch, or send the user
            // to the Connect tab to sign in and auto-return on success.
            Message::View(view::Message::Settings(view::SettingsMessage::NodeSettings(
                view::NodeSettingsMessage::SwitchToConnect,
            ))) => {
                let existing_jwt = self
                    .panels
                    .connect
                    .account
                    .authenticated_client()
                    .and_then(|c| c.token().map(str::to_owned));
                if let Some(jwt) = existing_jwt {
                    let routed = Message::View(view::Message::Settings(
                        view::SettingsMessage::NodeSettings(
                            view::NodeSettingsMessage::SwitchToConnectFastPath(
                                view::ConnectJwt::new(jwt),
                            ),
                        ),
                    ));
                    if let (Some(daemon), Some(panel)) =
                        (self.daemon.clone(), self.panels.current_mut())
                    {
                        return panel.update(Some(daemon), &self.cache, routed);
                    }
                } else {
                    self.pending_switch_to_connect_after_login = true;
                    // No active Connect session on this Cube; bubble up
                    // through the tab/pane so the Home tab takes focus
                    // and the user can sign in there.
                    return iced::Task::done(Message::View(view::Message::OpenConnectSignIn));
                }
            }

            // Cube Recovery Kit dispatch. Handled at App level because
            // the handler needs the authenticated CoincubeClient, the
            // Connect numeric cube id, and the live Wallet — none of
            // which are plumbed through `State::update`. Mirrors the
            // `cube_members::update(state, msg, client, cube_id)`
            // pattern at `state/connect/cube_members.rs:79`.
            Message::View(view::Message::Settings(view::SettingsMessage::RecoveryKit(msg))) => {
                let seed_source = if self.cube_settings.is_passkey_cube() {
                    crate::app::state::settings::recovery_kit::SeedSource::Passkey
                } else {
                    crate::app::state::settings::recovery_kit::SeedSource::Mnemonic
                };
                let client = self.authenticated_coincube_client();
                let server_cube_id = self.panels.connect.cube.server_cube_id;
                let wallet = self.wallet.clone();
                let local_cube_id = self.cube_settings.id.clone();
                return crate::app::state::settings::recovery_kit::update(
                    &mut self.panels.global_settings.recovery_kit,
                    msg,
                    &self.cache,
                    &local_cube_id,
                    seed_source,
                    client,
                    server_cube_id,
                    wallet,
                );
            }

            // Route refundables updates directly to LiquidTransactions so that
            // event-driven `list_refundables()` polls (fired from `BreezEvent`
            // handlers above) land on the correct panel even when the user is
            // sitting on a different screen. Otherwise the result would be
            // dropped into whatever panel happens to be current.
            Message::RefundablesPolled(result) => {
                // Poll response: clear the in-flight guard regardless of
                // outcome, but only advance the debounce timestamp on
                // success so a failed poll doesn't suppress retries for 30s.
                // We intentionally *don't* touch these fields for a manual
                // reload response — see the `RefundablesLoaded` arm below.
                self.refundables_fetch_in_flight = false;
                match result {
                    Ok(refundables) => {
                        self.last_refundables_fetch = Some(std::time::Instant::now());
                        // Forward the payload to LiquidTransactions through
                        // the panel's regular handler. The panel's
                        // reconciliation logic is origin-agnostic, so a poll
                        // result is converted to a `RefundablesLoaded` for
                        // it.
                        return self.panels.liquid_transactions.update(
                            self.daemon.clone(),
                            &self.cache,
                            Message::RefundablesLoaded(Ok(refundables)),
                        );
                    }
                    Err(e) => {
                        // Swallow: this is a background debounce poll the
                        // user didn't initiate. Surfacing it as a global
                        // ShowError toast — which is what
                        // `RefundablesLoaded(Err)` in LiquidTransactions
                        // does — would interrupt whichever panel the user
                        // is currently viewing with an error they have no
                        // context for. Log locally and let the next poll
                        // (or a manual reload) retry.
                        log::warn!(
                            target: "breez_swap",
                            "background refundables poll failed: {}",
                            e
                        );
                    }
                }
            }
            msg @ Message::RefundablesLoaded(_) | msg @ Message::RefundCompleted { .. } => {
                return self.panels.liquid_transactions.update(
                    self.daemon.clone(),
                    &self.cache,
                    msg,
                );
            }
            msg => {
                if let (Some(daemon), Some(panel)) =
                    (self.daemon.clone(), self.panels.current_mut())
                {
                    return panel.update(Some(daemon), &self.cache, msg);
                } else if let Some(panel) = self.panels.current_mut() {
                    return panel.update(None, &self.cache, msg);
                }
            }
        }

        Task::none()
    }

    pub fn load_daemon_config(
        &mut self,
        datadir_path: CoincubeDirectory,
        cfg: DaemonConfig,
    ) -> Result<(), Error> {
        // Keep a copy of the running config so we can recover if the new
        // daemon fails to start and the user would otherwise be stuck with
        // no daemon at all.
        let recovery_cfg = self.daemon.as_ref().and_then(|d| d.config().cloned());

        if let Some(daemon) = &self.daemon {
            Handle::current().block_on(async { daemon.stop().await })?;
        }
        let network = cfg.bitcoin_config.network;
        let daemon = match EmbeddedDaemon::start(cfg) {
            Ok(d) => d,
            Err(start_err) => {
                // New daemon failed to start.  Try to bring the old one back
                // so the app is left in a usable state rather than dead.
                if let Some(old_cfg) = recovery_cfg {
                    match EmbeddedDaemon::start(old_cfg) {
                        Ok(old_daemon) => {
                            self.daemon = Some(Arc::new(old_daemon));
                            warn!(
                                "New daemon failed to start; recovered previous daemon. \
                                 Start error: {}",
                                start_err
                            );
                        }
                        Err(recovery_err) => {
                            error!(
                                "New daemon failed to start and recovery also failed: \
                                 start={} recovery={}",
                                start_err, recovery_err
                            );
                        }
                    }
                }
                return Err(start_err.into());
            }
        };
        self.daemon = Some(Arc::new(daemon));
        let mut daemon_config_path = datadir_path
            .network_directory(network)
            .coincubed_data_directory(&self.wallet.as_ref().expect("wallet should exist").id())
            .path()
            .to_path_buf();
        daemon_config_path.push("daemon.toml");

        let content = toml::to_string(&self.daemon.as_ref().expect("daemon should exist").config())
            .map_err(|e| Error::Config(e.to_string()))?;

        OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(daemon_config_path)
            .map_err(|e| Error::Config(e.to_string()))?
            .write_all(content.as_bytes())
            .map_err(|e| {
                warn!("failed to write to file: {:?}", e);
                Error::Config(e.to_string())
            })
    }

    /// Render content for a settings sub-page that needs both its
    /// owning panel and the ConnectCubePanel (Spark → Settings →
    /// Lightning Address, Cube → Settings → Avatar / Members). Returns
    /// `None` for routes the generic panel dispatch can handle.
    ///
    /// Auth and LN-address preconditions render an inline prompt in
    /// place of the feature UI; the user signs in or claims an
    /// address, then the page re-renders with the real form.
    fn connect_settings_content(&self) -> Option<Element<'_, view::Message>> {
        use crate::app::view::connect::sign_in_prompt;
        let authenticated = self.panels.connect.account.is_authenticated();
        let has_ln_address = self
            .panels
            .connect
            .cube
            .lightning_address
            .as_ref()
            .and_then(|la| la.lightning_address.as_ref())
            .is_some();
        match &self.panels.current {
            Menu::Spark(menu::SparkSubMenu::Settings(Some(
                menu::SparkSettingsOption::LightningAddress,
            ))) => Some(if authenticated {
                view::spark::settings::lightning_address::lightning_address_ux(
                    &self.panels.connect.cube,
                )
                .map(view::Message::ConnectCube)
            } else {
                sign_in_prompt::sign_in_prompt("claim a Lightning Address")
            }),
            Menu::Cube(menu::CubeSubMenu::Settings(menu::CubeSettingsOption::Avatar)) => {
                Some(if !authenticated {
                    sign_in_prompt::sign_in_prompt("set up an Avatar")
                } else if !has_ln_address {
                    sign_in_prompt::claim_ln_address_prompt()
                } else {
                    view::connect::avatar_ux(&self.panels.connect.cube)
                        .map(view::Message::ConnectCube)
                })
            }
            Menu::Cube(menu::CubeSubMenu::Settings(menu::CubeSettingsOption::Members)) => {
                Some(if authenticated {
                    view::connect::cube_members::cube_members_ux(&self.panels.connect.cube)
                        .map(view::Message::ConnectCube)
                } else {
                    sign_in_prompt::sign_in_prompt("manage Cube Members")
                })
            }
            _ => None,
        }
    }

    pub fn view(&self) -> Element<'_, Message> {
        let view = if self.show_received_celebration {
            // Global celebration overlay takes precedence over the normal panel view
            let celebration = coincube_ui::component::received_celebration_page(
                &self.received_celebration_context,
                &self.received_celebration_amount,
                &self.received_celebration_quote,
                &self.received_celebration_image,
                view::Message::DismissReceivedCelebration,
            );
            view::dashboard(&self.panels.current, &self.cache, celebration)
        } else if let Some(content) = self.connect_settings_content() {
            // Connect-dependent settings sub-pages (Spark → Settings →
            // Lightning Address, Cube → Settings → Avatar / Members)
            // need both the relevant panel state and the
            // ConnectCubePanel. The State trait's `view` only sees the
            // active panel + Cache, so the dispatch lives here — App
            // owns every panel.
            view::dashboard(&self.panels.current, &self.cache, content)
        } else {
            self.panels
                .current()
                .unwrap_or(&self.panels.global_home)
                .view(&self.panels.current, &self.cache)
        };

        let content = if self.cache.network != bitcoin::Network::Bitcoin {
            iced::widget::column![network_banner(self.cache.network), view.map(Message::View)]
                .into()
        } else {
            view.map(Message::View)
        };

        // Overlay toast at bottom if present
        match self.errors.is_empty() {
            true => content,
            false => {
                // Errors are already in chronological order (Vec is append-only)
                let error_snapshot: Vec<_> = self.errors.iter().collect();

                let theme = ui_theme::Theme::default();
                iced::widget::Stack::new()
                    .push(content)
                    .push(
                        view::toast_overlay(
                            error_snapshot
                                .iter()
                                .map(|(id, _, level, msg)| (*id, *level, msg.as_str())),
                            &theme,
                        )
                        .map(Message::View),
                    )
                    .into()
            }
        }
    }

    pub fn datadir_path(&self) -> &CoincubeDirectory {
        &self.cache.datadir_path
    }
}

fn new_recovery_panel(
    wallet: Arc<Wallet>,
    cache: &Cache,
    sync_status: SyncStatus,
) -> CreateSpendPanel {
    let (balance, unconfirmed_balance, _, _) = state::coins_summary(
        cache.coins(),
        cache.blockheight() as u32,
        wallet.main_descriptor.first_timelock_value(),
    );
    CreateSpendPanel::new_recovery(
        wallet,
        cache.coins(),
        cache.blockheight() as u32,
        cache.network,
        balance,
        unconfirmed_balance,
        sync_status,
        cache.bitcoin_unit,
    )
}
