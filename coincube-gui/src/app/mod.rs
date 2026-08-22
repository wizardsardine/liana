pub mod breez_liquid;
pub mod breez_spark;
pub mod cache;
pub mod config;
pub mod error;
pub mod features;
pub mod menu;
pub mod message;
pub mod seed_source;
pub mod session;
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
    LiquidSend, LiquidSettings, LiquidSwap, LiquidTransactions, PsbtsPanel, State, VaultOverview,
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
    liquid_swap: LiquidSwap,
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

    /// Path to this cube's local swap-history log (the SDK doesn't mark
    /// swaps, so we keep our own record). Lives alongside the cube's other
    /// per-network state.
    fn swaps_path(
        datadir: &CoincubeDirectory,
        network: bitcoin::Network,
        cube_id: &str,
    ) -> std::path::PathBuf {
        datadir
            .network_directory(network)
            .path()
            .join(format!("liquid-swaps-{cube_id}.json"))
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
        let swaps_path = Self::swaps_path(datadir, network, &cube_id);
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
            liquid_overview: LiquidOverview::new(liquid_backend.clone(), swaps_path.clone()),
            liquid_send: LiquidSend::new(liquid_backend.clone()),
            liquid_swap: LiquidSwap::new(liquid_backend.clone(), swaps_path.clone()),
            liquid_receive: LiquidReceive::new(liquid_backend.clone()),
            liquid_transactions: LiquidTransactions::new(
                liquid_backend.clone(),
                swaps_path.clone(),
            ),
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
                // `new_without_vault`: this Cube has no Vault wallet yet.
                false,
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
                        network,
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
        let swaps_path = Self::swaps_path(&data_dir, cache.network, &cube_id);
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
            liquid_overview: LiquidOverview::new(liquid_backend.clone(), swaps_path.clone()),
            liquid_send: LiquidSend::new(liquid_backend.clone()),
            liquid_swap: LiquidSwap::new(liquid_backend.clone(), swaps_path.clone()),
            liquid_receive: LiquidReceive::new(liquid_backend.clone()),
            liquid_transactions: LiquidTransactions::new(
                liquid_backend.clone(),
                swaps_path.clone(),
            ),
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
                // `new` (vault constructor): this Cube has a Vault wallet.
                true,
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
                        cache.network,
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
                crate::app::menu::LiquidSubMenu::Swap => Some(&self.liquid_swap),
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
                crate::app::menu::LiquidSubMenu::Swap => Some(&mut self.liquid_swap),
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
                crate::app::menu::LiquidSubMenu::Swap => Some(Message::View(
                    view::Message::LiquidSwap(view::LiquidSwapMessage::RefreshRequested),
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
    /// Guards the active-node net-stats poll (connections/upload/onion) so ticks
    /// don't stack concurrent RPCs.
    node_net_stats_probe_in_progress: bool,
    /// True while an off-thread daemon backend switch ([`Self::spawn_daemon_switch`])
    /// is in flight. The config isn't updated until the switch completes, so
    /// without this guard the next sync probe would keep re-firing the switch
    /// every tick — spawning concurrent daemon starts that race to load the same
    /// watchonly wallet and corrupt it. Cleared by `Message::DaemonRestarted`.
    daemon_switch_in_progress: bool,
    /// Set when an auto-promotion to the pending local node fails and the
    /// previous daemon is recovered. The recovered daemon still carries
    /// `auto_switch_to_pending = true` + `pending_bitcoind`, so without this the
    /// periodic sync probe would re-fire the same failing switch every poll,
    /// churning the daemon. Suppresses auto-switch until the next *successful*
    /// switch (a fresh adopt / manual switch re-arms it by clearing this).
    auto_switch_suppressed: bool,
    /// Global "payment received" celebration overlay — shown for incoming
    /// Liquid payments (e.g. LNURL) regardless of which panel is active.
    show_received_celebration: bool,
    /// One-time "turn on recovery alerts?" consent prompt overlay
    /// (PLAN-recovery-alerts-cleanup PR 3). Set once per session when a Vault
    /// with keyholders has no monitoring and the prompt hasn't been answered for
    /// this Cube; cleared on accept/decline. The durable "answered" record lives
    /// in `CubeSettings::recovery_alerts_prompt_answered`.
    show_recovery_alerts_prompt: bool,
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
    /// both the REST and gRPC paths. `None` when no live or persisted
    /// Connect session is available.
    /// Stored on the App so PR B's `resolve_signers` /
    /// `create_signing_session` call sites can construct a
    /// `GrpcSessionClient` without re-plumbing.
    #[allow(dead_code)]
    connect_auth: Option<
        Arc<tokio::sync::RwLock<crate::services::connect::client::auth::AccessTokenResponse>>,
    >,
    /// Email of the currently authenticated Connect account. Used to
    /// scope cache writes (device_id, last_seen_event_seq). `None` when no
    /// live or persisted Connect session is available.
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

/// Persist a completed duress enrollment locally (Phases 2 & 8). Writes the
/// duress PIN hash onto **every** Cube in `network` — a duress wipe takes all
/// Cubes on the device, so the duress PIN must trip from any Cube's unlock —
/// and stores this device's ChaCha20-encrypted duress code in
/// `DuressLocalState` (data-dir root, outside the wiped tree). Shared by the
/// App, Home, and Launcher surfaces, all of which can host the Connect Duress
/// panel. Hashing happens inside this async task so the UI thread never blocks
/// on argon2.
/// Every per-network directory under the datadir root that holds a
/// `settings.json` (i.e. has Cubes). A duress enrollment verifies + writes the
/// duress PIN across all of these, matching the all-networks scope of the wipe.
///
/// # Why this returns a `Result`
///
/// It used to swallow every I/O error — `if let Ok(entries)`, plus `.flatten()`
/// on the entries — and hand back an empty list. Every caller reads an empty
/// list as "this device has no Cubes", so a directory that could not be read
/// became the claim that the user owns nothing:
///
/// - enrollment refuses with "Couldn't find any Cubes on this device", which is
///   false and unactionable for someone looking at their Cubes on screen;
/// - **disable reports success having disarmed nothing.** That is the dangerous
///   one. `clear_duress_enrollment` iterates these directories to overwrite
///   each armed marker; over an empty list it clears no markers, then resets the
///   local state and returns `Ok`. The user is told duress is off while every
///   Cube still holds a live wipe trigger.
///
/// An unreadable datadir is not evidence of anything. Say so and let each
/// caller refuse.
fn duress_enroll_network_dirs(
    root: &std::path::Path,
) -> Result<Vec<crate::dir::NetworkDirectory>, String> {
    let describe =
        |e: std::io::Error| format!("Couldn't read your data directory to find your Cubes ({e}).");
    let mut dirs = Vec::new();
    for entry in std::fs::read_dir(root).map_err(describe)? {
        let p = entry.map_err(describe)?.path();
        if p.is_dir() && p.join(crate::app::settings::SETTINGS_FILE_NAME).is_file() {
            dirs.push(crate::dir::NetworkDirectory::new(p));
        }
    }
    Ok(dirs)
}

/// User-facing message when the candidate duress PIN equals a Cube's real
/// unlock PIN. A shared constant so the pre-flight wizard check and
/// `persist_duress_enrollment` can't drift apart.
pub(crate) const DURESS_PIN_COLLIDES_MSG: &str =
    "That duress PIN is already the unlock PIN of one of your Cubes. Choose a \
     PIN you don't use to unlock any Cube.";

/// User-facing message when there are no Cubes on the device to arm.
pub(crate) const DURESS_NO_CUBES_MSG: &str =
    "Couldn't find any Cubes on this device to protect with duress mode.";

/// Verify the candidate duress PIN does not collide with any Cube's real unlock
/// PIN across every network under `root`. `Err` on a collision, when there are
/// no Cubes to arm, or when settings can't be read.
///
/// The same duress PIN is armed on every Cube, and at unlock a Cube checks its
/// real PIN first and the duress marker second — so a duress PIN equal to any
/// Cube's unlock PIN either can't trip duress on that Cube or trips a wipe on a
/// *different* one. Each Cube can have its own PIN, hence the per-Cube check.
///
/// # How the check works now
///
/// It used to compare against each Cube's stored `security_pin_hash`. There is
/// no such hash any more, so the check does the exact thing instead: it
/// **trial-decrypts each Cube's seed file with the candidate duress PIN**. If
/// one opens, that PIN is that Cube's real unlock PIN.
///
/// This is strictly more correct than the hash compare it replaces, which could
/// only see Cubes whose hash happened to be recorded and would silently pass a
/// Cube whose settings had drifted. It is also slower — one ~831 ms Argon2 pass
/// per Cube — which is why this is **blocking** and callers must run it on a
/// blocking pool. Enrollment is a wizard step behind a spinner; paying a second
/// or two once, to be certain, is the right trade.
///
/// Run this BEFORE any server enrollment: Connect tiers enroll server-side
/// first, so checking the (deterministic, user-entered) collision only in the
/// later local persist would let a bad PIN enroll on the server and then fail
/// locally — leaving the account server-enrolled with no Cube armed.
pub(crate) fn duress_pin_collision_check_blocking(
    root: &std::path::Path,
    duress_pin: &str,
) -> Result<(), String> {
    use crate::services::unlock::{self, PinOutcome};

    let network_dirs = duress_enroll_network_dirs(root)?;
    if network_dirs.is_empty() {
        return Err(DURESS_NO_CUBES_MSG.to_string());
    }
    let mut total_cubes = 0usize;
    for network_dir in &network_dirs {
        let settings = crate::app::settings::Settings::from_file(network_dir)
            .map_err(|e| format!("Couldn't read your Cube settings to verify your PIN: {e}"))?;
        for cube in &settings.cubes {
            total_cubes += 1;
            let loc = unlock::CubeLocation::new(root, cube);
            match unlock::unlock_blocking(&loc, duress_pin) {
                Ok(PinOutcome::Unlock(_)) => return Err(DURESS_PIN_COLLIDES_MSG.to_string()),
                // Already this Cube's duress PIN (a re-enrolment with the same
                // value), or simply wrong. Neither is a collision with a real
                // unlock PIN.
                Ok(PinOutcome::Duress) | Ok(PinOutcome::Wrong) => {}
                // A Cube with no local seed can't collide. A keystore problem
                // means we can't be sure — and "can't be sure" must not become
                // "probably fine" on a path whose failure mode is an
                // unintended wipe.
                Err(unlock::UnlockError::NoPinConfigured) => {}
                Err(e) => {
                    return Err(format!(
                        "Couldn't check the duress PIN against Cube '{}': {e}",
                        cube.name
                    ))
                }
            }
        }
    }
    if total_cubes == 0 {
        return Err(DURESS_NO_CUBES_MSG.to_string());
    }
    Ok(())
}

/// Best-effort restore of the first `count` networks' settings to their
/// pre-enrollment snapshot, undoing the duress PIN hashes written in step 1.
/// Used when a later step (another network, or the local-state save) fails, so
/// the device never ends up with Cubes armed but the matching enrollment state
/// missing.
struct ArmedMarker {
    root: std::path::PathBuf,
    cube_id: String,
    cube_name: String,
    network: bitcoin::Network,
    /// The marker's file name. Random, so rollback cannot recompute it — it
    /// has to be carried from the write that minted it.
    file_name: String,
    /// Whether the write landed on a slot this Cube already had, rather than
    /// minting a new one. On a device that was **already enrolled** that slot
    /// held the previous duress PIN's marker, and overwriting it destroyed it —
    /// a fact rollback cannot undo, since it has no way to reconstruct a marker
    /// for a PIN it was never given. See [`prior_pin_deactivated`].
    reused_slot: bool,
}

/// Remove the markers an aborted enrollment already wrote, returning the names
/// of any Cube it could **not** disarm.
///
/// The return value is the point. A marker left behind is a live wipe trigger
/// on a Cube whose owner believes enrollment failed and nothing happened —
/// entering that PIN later destroys the device with no enrollment record to
/// explain it. Logging that to `<datadir>/logs/` and telling the user "no
/// changes were kept" would be a false statement about the one thing they most
/// need to be true.
#[must_use = "a marker that could not be removed leaves a live duress wipe trigger"]
fn rollback_duress_markers(armed: &[ArmedMarker]) -> Vec<String> {
    let mut still_armed = Vec::new();
    for m in armed {
        // Overwrite with a decoy rather than delete. Since unit 6b every Cube
        // carries a second slot from creation, so removing the file would take
        // this Cube from two blobs to one — undoing the arming *and* marking
        // the Cube as the one where something went wrong, which is precisely
        // the shape the decoy exists to prevent. A decoy opens for no PIN, so
        // it disarms just as completely as a delete.
        //
        // A keystore that can't be reached is not "this Cube has no secret". A
        // v3 Cube's decoy written without its secret lands at the wrong wire
        // version, which singles the Cube out — and doing it while reporting
        // the Cube as disarmed would be the false statement this function
        // exists to avoid. Leave the marker and say so instead.
        let secret = match crate::services::unlock::device_secret::load_optional(&m.cube_id) {
            Ok(s) => s,
            Err(e) => {
                log::error!(
                    "duress: rollback of marker for cube {} ({}) skipped, keystore unreachable: {e}",
                    m.cube_name,
                    m.cube_id
                );
                still_armed.push(m.cube_name.clone());
                continue;
            }
        };
        if let Err(e) = crate::services::unlock::marker::write_decoy(
            &m.root,
            m.network,
            &m.cube_id,
            &m.file_name,
            secret.as_ref(),
        ) {
            log::error!(
                "duress: rollback of marker for cube {} ({}) failed: {e}",
                m.cube_name,
                m.cube_id
            );
            still_armed.push(m.cube_name.clone());
        }
    }
    // The markers are back to decoys, so the orphan breadcrumb has nothing
    // left to point at. Best-effort: a stale `arming` flag only costs a
    // redundant disarm on the next launch, whereas failing here would replace
    // an accurate rollback message with a state-write error.
    if still_armed.is_empty() {
        if let Some(m) = armed.first() {
            if let Ok(mut st) = crate::services::duress::DuressLocalState::load(&m.root) {
                st.arming = false;
                let _ = st.save(&m.root);
            }
        }
    }
    still_armed
}

/// The Cubes whose *previous* duress PIN this attempt already destroyed.
///
/// Re-enrolment writes the new marker over the slot the Cube already had, so by
/// the time a later step fails the old marker is gone. Rollback restores a
/// decoy, not the old marker — it never saw the old PIN — so on those Cubes the
/// previously enrolled duress PIN is now inert. "No changes were kept" would be
/// a false statement about exactly the property the user relies on.
///
/// Gated on `was_enrolled`: since unit 6b every Cube carries a slot from
/// creation, so a reused slot on a device that was never enrolled held a decoy
/// and overwriting it cost nothing. Without that gate this would warn about a
/// lost duress PIN that never existed.
///
/// Within an enrolled device it can still over-report — a Cube created *after*
/// the last enrolment also has a decoy in its slot, and telling those two apart
/// would take the old PIN, which we don't have. Over-reporting is the safe
/// direction: the remedy (re-enroll) is the same and costs nothing.
fn prior_pin_deactivated(was_enrolled: bool, armed: &[ArmedMarker]) -> Vec<String> {
    if !was_enrolled {
        return Vec::new();
    }
    armed
        .iter()
        .filter(|m| m.reused_slot)
        .map(|m| m.cube_name.clone())
        .collect()
}

/// Finish an enrollment-failure message, telling the truth about what was left
/// behind.
pub(crate) fn describe_rollback(
    base: String,
    still_armed: Vec<String>,
    prior_pin_lost: Vec<String>,
) -> String {
    let mut msg = if still_armed.is_empty() {
        // Only honest when the previous enrolment also survived.
        if prior_pin_lost.is_empty() {
            return format!("{base} No changes were kept.");
        }
        base
    } else {
        format!(
            "{base} WARNING: the duress PIN could not be removed from {}. \
             Entering that PIN on {} will still erase this device. Turn duress mode \
             off and on again, or contact support before using that PIN.",
            still_armed.join(", "),
            if still_armed.len() == 1 { "it" } else { "them" },
        )
    };
    if !prior_pin_lost.is_empty() {
        msg.push_str(&format!(
            " Your previous duress PIN no longer works on {} and must be set up \
             again — turn duress mode on to re-enroll.",
            prior_pin_lost.join(", "),
        ));
    }
    msg
}

pub(crate) async fn persist_duress_enrollment(
    datadir: CoincubeDirectory,
    duress_pin: zeroize::Zeroizing<String>,
    duress_code: zeroize::Zeroizing<String>,
    account_id: Option<String>,
) -> Result<(), String> {
    // A duress wipe takes every Cube on EVERY network under the datadir, so the
    // duress PIN must trip from any of them — set it (and verify against it) on
    // all per-network settings, not just the active one.
    let root = datadir.path().to_path_buf();
    let network_dirs = duress_enroll_network_dirs(&root)?;
    // No per-network settings.json found — there are no Cubes to arm (or the
    // data directory couldn't be read). Fail loud instead of marking duress
    // "enabled" with no duress PIN written anywhere.
    if network_dirs.is_empty() {
        return Err(DURESS_NO_CUBES_MSG.to_string());
    }

    // 0. Guard against a duress PIN that collides with a real unlock PIN, and
    //    against having no Cubes to arm, BEFORE writing anything. This mirrors
    //    the wizard's pre-flight check (which runs before any server enroll);
    //    re-running it here is the authoritative guard against an on-disk Cube
    //    set that changed since the pre-flight.
    duress_pin_collision_check_blocking(&root, &duress_pin)?;

    // Snapshot the pre-write state of every network so a later step can roll
    // back. (The collision / no-Cubes guards above already validated the set.)
    let mut prior_settings: Vec<crate::app::settings::Settings> =
        Vec::with_capacity(network_dirs.len());
    for network_dir in &network_dirs {
        // We enumerated this dir because it HAS a settings.json. If it now
        // can't be read (corrupt/parse/IO, or a race), fail loud rather than
        // arm an unverified duress PIN anyway.
        let settings = crate::app::settings::Settings::from_file(network_dir)
            .map_err(|e| format!("Couldn't read your Cube settings to verify your PIN: {e}"))?;
        prior_settings.push(settings);
    }

    // 1. A duress marker → every Cube on every network.
    //
    //    The marker replaces the old per-Cube `duress_pin_hash`. That hash was
    //    Argon2id at m=19 MiB (27 ms a guess) sitting next to a seed file at
    //    m=256 MiB (831 ms a guess), so an attacker with the datadir cracked
    //    the duress PIN in seconds and learned that duress was armed at all.
    //    The marker is sealed by the same codec at the same parameters, so
    //    there is no cheap oracle and no tell (invariant I2). See
    //    `services::unlock::marker`.
    //
    //    Sequential writes have no cross-file transaction, so a later failure
    //    rolls back the markers already written — the device must never end up
    //    with some Cubes armed and others not.
    //
    //    Before the first write, drop an `arming` breadcrumb in
    //    `DuressLocalState`. A crash between here and the state write in step 2
    //    leaves live markers with no enrollment to explain them, and since
    //    unit 6b no scan of the datadir can detect that — every Cube carries a
    //    second slot, and a marker is indistinguishable from a decoy on
    //    purpose. The breadcrumb is what `reconcile_duress_*` reads instead.
    //
    //    The pre-existing `enrolled` flag comes back from that same read: it is
    //    what turns "this Cube's slot was overwritten" into "a live duress PIN
    //    was overwritten", which a rollback message has to admit to.
    let was_enrolled = match (|| -> Result<bool, String> {
        let mut st = crate::services::duress::DuressLocalState::load(&root)
            .map_err(|e| format!("Couldn't read existing duress state: {e}"))?;
        let was_enrolled = st.enrolled;
        st.arming = true;
        st.save(&root).map_err(|e| e.to_string())?;
        Ok(was_enrolled)
    })() {
        Ok(v) => v,
        Err(e) => return Err(format!("{e} No changes were kept.")),
    };

    let mut armed_cubes: Vec<ArmedMarker> = Vec::new();
    let mut arm_failure: Option<String> = None;

    'arming: for (i, network_dir) in network_dirs.iter().enumerate() {
        for cube in &prior_settings[i].cubes {
            let secret = match crate::services::unlock::device_secret::load_optional(&cube.id) {
                Ok(s) => s,
                Err(e) => {
                    arm_failure = Some(format!(
                        "Couldn't reach your system keychain to arm duress on Cube '{}': {e}",
                        cube.name
                    ));
                    break 'arming;
                }
            };
            // Reuse the name already recorded for this Cube when re-enrolling,
            // so a second enrolment replaces the marker in place instead of
            // leaving the first one orphaned and unfindable. Otherwise mint a
            // fresh unpredictable name stamped with this Cube's *seed file's*
            // timestamp, so the two files agree (X7).
            let reused_slot = cube.duress_slot_file.is_some();
            let file_name = cube.duress_slot_file.clone().unwrap_or_else(|| {
                crate::services::unlock::marker::new_file_name(
                    crate::services::unlock::marker::seed_timestamp(
                        &root,
                        cube.network,
                        cube.master_signer_fingerprint,
                        cube.created_at,
                    ),
                )
            });
            // The marker must use the *same* wire version and key material as
            // this Cube's seed file, or the pair stops being indistinguishable.
            if let Err(e) = crate::services::unlock::marker::write(
                &root,
                cube.network,
                &cube.id,
                &file_name,
                &duress_pin,
                secret.as_ref(),
            ) {
                arm_failure = Some(format!(
                    "Couldn't arm the duress PIN on Cube '{}' ({e}).",
                    cube.name
                ));
                break 'arming;
            }
            armed_cubes.push(ArmedMarker {
                root: root.clone(),
                cube_id: cube.id.clone(),
                cube_name: cube.name.clone(),
                network: cube.network,
                file_name,
                reused_slot,
            });
        }
        let _ = network_dir;
    }

    if let Some(msg) = arm_failure {
        let lost = prior_pin_deactivated(was_enrolled, &armed_cubes);
        let still_armed = rollback_duress_markers(&armed_cubes);
        return Err(describe_rollback(msg, still_armed, lost));
    }

    // Settings files were present but held no Cubes — no marker was written
    // anywhere. Abort rather than mark duress enabled with nothing that can
    // trip a wipe.
    if armed_cubes.is_empty() {
        return Err(
            "Couldn't find any Cubes on this device to protect with duress mode.".to_string(),
        );
    }

    // 1b. Record each marker's file name.
    //
    //     The name is random, so this is the **only** way to find the file
    //     again — a marker whose name was never written down is an
    //     unreferenced blob that no PIN can reach, and the Cube would report
    //     duress as unarmed while the file sat there. Persist before declaring
    //     success, and roll the markers back if it fails, exactly as step 2
    //     does.
    for (i, network_dir) in network_dirs.iter().enumerate() {
        let names: Vec<(String, String)> = armed_cubes
            .iter()
            .filter(|m| prior_settings[i].cubes.iter().any(|c| c.id == m.cube_id))
            .map(|m| (m.cube_id.clone(), m.file_name.clone()))
            .collect();
        if names.is_empty() {
            continue;
        }
        let written = crate::app::settings::update_settings_file(network_dir, |mut s| {
            for cube in s.cubes.iter_mut() {
                if let Some((_, name)) = names.iter().find(|(id, _)| *id == cube.id) {
                    cube.duress_slot_file = Some(name.clone());
                }
            }
            Some(s)
        })
        .await;
        if let Err(e) = written {
            let lost = prior_pin_deactivated(was_enrolled, &armed_cubes);
            let still_armed = rollback_duress_markers(&armed_cubes);
            return Err(describe_rollback(
                format!("Couldn't save the duress settings for your Cubes ({e})."),
                still_armed,
                lost,
            ));
        }
    }

    // 2. Encrypted device code + account id → DuressLocalState. The encrypted
    //    code is skipped when empty (the server-enroll path re-enters with an
    //    empty code once it's already stored). The account id is set
    //    unconditionally — including to `None` for a sovereign enrollment — so
    //    re-enrolling sovereign clears any previously stored Connect account id
    //    and local activation can't trigger-with-code against a stale account.
    //    `load` is resilient (missing → default); only `save` and the
    //    encryption are fail-loud. If this fails AFTER step 1 armed the PIN
    //    hashes, roll those back too — otherwise Cubes stay armed (and would
    //    wipe on the duress PIN) while the matching enrollment state is missing.
    let local: Result<(), String> = (|| {
        // load() already maps a missing file to Ok(default); a real parse/IO
        // error must NOT be papered over with a default that save() then writes
        // back, clobbering valid state. Propagate it (rolling back step 1).
        let mut st = crate::services::duress::DuressLocalState::load(&root)
            .map_err(|e| format!("Couldn't read existing duress state: {e}"))?;
        st.enrolled = true;
        // Enrollment is now fully recorded, so the orphan breadcrumb has done
        // its job. Cleared in the same write that sets `enrolled`, so the two
        // can never disagree.
        st.arming = false;
        st.account_id = account_id;
        if !duress_code.is_empty() {
            let key = crate::services::duress::cipher::DeviceKey::load_or_create(&root)
                .map_err(|e| e.to_string())?;
            st.duress_code = Some(key.encrypt(&duress_code)?);
        }
        st.save(&root).map_err(|e| e.to_string())
    })();
    if let Err(e) = local {
        let lost = prior_pin_deactivated(was_enrolled, &armed_cubes);
        let still_armed = rollback_duress_markers(&armed_cubes);
        return Err(describe_rollback(e, still_armed, lost));
    }
    Ok(())
}

/// User-facing message when the step-up PIN doesn't match any Cube's real unlock
/// PIN. The duress PIN never satisfies `verify_pin` (distinct hash, collision
/// forbidden at enroll), so entering it lands here too — without revealing that
/// it was the duress PIN.
pub(crate) const DURESS_STEP_UP_BAD_PIN_MSG: &str =
    "That PIN doesn't match any of your Cubes' unlock PINs.";

/// User-facing message when no Cube on this device has a PIN-protected seed, so
/// there is no local secret to check the step-up against. Fails closed: this is
/// precisely the device profile an attacker who has only the Connect account
/// would arrive on — a fresh install, or one holding Cubes restored nowhere —
/// and letting the disable through there would hand them the whole control.
pub(crate) const DURESS_STEP_UP_NO_PIN_MSG: &str =
    "No Cube on this device is protected by an unlock PIN, so there's no way to \
     confirm it's you. Turn duress mode off from a device where you unlock a \
     Cube with its PIN.";

/// How this device can prove it's the owner asking, for the duress-disable
/// step-up.
///
/// The step-up needs a secret the Connect account alone cannot supply, so it has
/// to come from something local. Which local thing depends on how the Cubes here
/// were made, and that is not knowable without touching the filesystem — hence
/// [`duress_step_up_method_blocking`], run once when the dialog opens so the
/// dialog can ask for the right thing instead of guessing.
// `pub`, not `pub(crate)`: it rides in `DuressMessage`, which is public, and a
// private type inside a public variant is a `private_interfaces` warning — an
// error under CI's `-D warnings`.
#[derive(Debug, Clone)]
pub enum DuressStepUpMethod {
    /// At least one Cube here has a PIN-protected seed. Re-enter its unlock PIN.
    Pin,
    /// No PIN-protected Cube, but a passkey Cube is here. A fresh WebAuthn
    /// assertion is the equivalent proof: it needs this machine plus the user's
    /// biometric/Apple ID, neither of which a stolen Connect session carries.
    /// Boxed because `CubeSettings` is comparatively large and this rides in a
    /// message iced clones freely.
    Passkey(Box<crate::app::settings::CubeSettings>),
    /// Neither. Nothing here can anchor a step-up, so the disable is refused and
    /// the user is pointed at a device that can.
    Unavailable,
}

/// Classify what this device can offer as a disable step-up factor.
///
/// PIN wins when both are present: it is the path that already existed, it costs
/// no system prompt, and a user with a PIN Cube here is expecting to be asked
/// for a PIN. The passkey branch exists for the device profile that has no PIN
/// at all — which, since passkey is the default creation method on macOS, is an
/// ordinary user rather than an edge case.
///
/// **Blocking** — reads settings and stats seed files for every Cube. Cheap next
/// to [`verify_regular_cube_pin_blocking`] (no Argon2 pass), but still I/O, so
/// callers run it off the UI thread.
pub(crate) fn duress_step_up_method_blocking(
    root: &std::path::Path,
) -> Result<DuressStepUpMethod, String> {
    use crate::services::unlock;

    let network_dirs = duress_enroll_network_dirs(root)?;
    if network_dirs.is_empty() {
        return Err(DURESS_NO_CUBES_MSG.to_string());
    }
    let mut passkey_cube: Option<crate::app::settings::CubeSettings> = None;
    for network_dir in &network_dirs {
        let settings = crate::app::settings::Settings::from_file(network_dir)
            .map_err(|e| format!("Couldn't read your Cube settings to verify your PIN: {e}"))?;
        for cube in &settings.cubes {
            let loc = unlock::CubeLocation::new(root, cube);
            if unlock::pin_requirement(&loc) == unlock::PinRequirement::Required {
                return Ok(DuressStepUpMethod::Pin);
            }
            // First one wins. Any passkey Cube proves the same two things
            // (this machine, this Apple ID), so there is no better choice to
            // make — the same reason the PIN path accepts any Cube's PIN.
            if passkey_cube.is_none() && cube.is_passkey_cube() {
                passkey_cube = Some(cube.clone());
            }
        }
    }
    Ok(match passkey_cube {
        Some(cube) => DuressStepUpMethod::Passkey(Box::new(cube)),
        None => DuressStepUpMethod::Unavailable,
    })
}

/// Step-up re-auth for the duress *disable* flow: verify `pin` is the REAL
/// unlock PIN of at least one Cube.
///
/// Verification is a trial decryption of that Cube's seed file, so entering the
/// **duress** PIN here is rejected — it opens the marker, never the seed, and
/// `PinOutcome::Duress` is not a match. That is exactly the plan's "do not
/// accept the duress PIN at step-up", and it now holds by construction rather
/// than by the two hashes happening to differ.
///
/// **Blocking** — one Argon2id pass per Cube until a match. Callers must run it
/// on a blocking pool.
///
/// # A passkey-only device never reaches this
///
/// A passkey Cube has no seed file, so `master_seed_path` finds nothing and
/// `pin_requirement` reports `NoLocalSeed` — it is skipped here, and a device
/// holding only passkey Cubes could never satisfy a PIN check whatever was
/// typed. Since duress enrollment does not require a PIN Cube
/// (`duress_pin_collision_check_blocking` skips `NoPinConfigured` Cubes and
/// proceeds), that would have been a one-way door: enrolled, and unable to
/// disable from the only device you own.
///
/// Such a device is therefore routed to the passkey step-up instead of this
/// function — see [`DuressStepUpMethod`], chosen once when the dialog opens.
/// [`DURESS_STEP_UP_NO_PIN_MSG`] is now reserved for a device with neither
/// factor, where refusing really is the only honest answer.
///
/// `Ok` only when a Cube's regular PIN matches. `Err` on an empty PIN, a
/// mismatch, no Cubes, no PIN-protected Cube on this device, or when settings
/// can't be read.
pub(crate) fn verify_regular_cube_pin_blocking(
    root: &std::path::Path,
    pin: &str,
) -> Result<(), String> {
    use crate::services::unlock::{self, PinOutcome};

    if pin.is_empty() {
        return Err("Enter your Cube unlock PIN to continue.".to_string());
    }
    let network_dirs = duress_enroll_network_dirs(root)?;
    if network_dirs.is_empty() {
        return Err(DURESS_NO_CUBES_MSG.to_string());
    }
    let mut any_pin = false;
    for network_dir in &network_dirs {
        let settings = crate::app::settings::Settings::from_file(network_dir)
            .map_err(|e| format!("Couldn't read your Cube settings to verify your PIN: {e}"))?;
        for cube in &settings.cubes {
            let loc = unlock::CubeLocation::new(root, cube);
            if unlock::pin_requirement(&loc) != unlock::PinRequirement::Required {
                // No PIN-protected seed on this device — this Cube can't
                // anchor the step-up.
                continue;
            }
            any_pin = true;
            match unlock::unlock_blocking(&loc, pin) {
                Ok(PinOutcome::Unlock(_)) => return Ok(()),
                // The duress PIN must not satisfy a step-up, and must not
                // reveal that it was recognised either.
                Ok(PinOutcome::Duress) | Ok(PinOutcome::Wrong) => {}
                Err(unlock::UnlockError::NoPinConfigured) => {}
                // A keystore failure is not a wrong PIN (I7); surface it as
                // itself rather than letting it read as a bad entry.
                Err(e) => return Err(e.to_string()),
            }
        }
    }
    if !any_pin {
        // No PIN-protected Cube on this device — there is no local secret to
        // check against, so the step-up cannot be satisfied. Fail closed: an
        // "Ok" here would let ANY non-empty string disarm duress on every
        // device, which is exactly what a Connect-account-only attacker (fresh
        // install, no seed restored) would be holding. Distinct from a wrong
        // PIN, because the user can't fix it by typing a different one.
        return Err(DURESS_STEP_UP_NO_PIN_MSG.to_string());
    }
    Err(DURESS_STEP_UP_BAD_PIN_MSG.to_string())
}

/// Local disarm — the inverse of [`persist_duress_enrollment`]. Turns duress
/// off ON THIS DEVICE without wiping anything: clears the per-Cube duress PIN
/// hash on every Cube across every network, resets `DuressLocalState` to the
/// un-enrolled baseline (zeroizing the encrypted device code), and empties the
/// durable activation queue. Cube funds and data are left untouched.
///
/// The per-Cube hashes are cleared UNCONDITIONALLY — never gated on
/// `DuressLocalState.enrolled`. A hard crash between persist's Cube-arming step
/// and its state-write step can leave Cubes armed while the local state still
/// reads "not enrolled"; trusting that flag here would let a disable report
/// success while the duress PIN could still trip a wipe. Setting each hash to
/// `None` is idempotent, so always clearing is cheap and safe.
///
/// Ordering mirrors persist in reverse: clear the Cube PIN hashes FIRST (so the
/// duress PIN can no longer trip a wipe), THEN drop the local state. A failure
/// midway leaves a consistent "still enrolled" view that a retry completes —
/// every step is idempotent.
pub(crate) async fn clear_duress_enrollment(datadir: CoincubeDirectory) -> Result<(), String> {
    let root = datadir.path().to_path_buf();

    // 1. Clear the duress PIN hash on every Cube on every network — ALWAYS,
    //    whatever DuressLocalState records. Setting it to None is idempotent;
    //    stop at the first failure so a retry re-clears the rest.
    let network_dirs = duress_enroll_network_dirs(&root)?;
    for network_dir in &network_dirs {
        let settings = crate::app::settings::Settings::from_file(network_dir)
            .map_err(|e| format!("Couldn't read your Cube settings to disarm duress: {e}"))?;
        for cube in &settings.cubes {
            // Overwrite the slot with a decoy — never delete it. Deleting
            // would take the Cube from two blobs to one, which is both a
            // regression of the 6b shape and a durable record that duress was
            // once armed here. A decoy opens for no PIN, so the wipe trigger
            // is just as dead.
            //
            // A Cube with no recorded slot has nothing to overwrite. That is a
            // pre-6b Cube awaiting backfill, not a failure — skip it.
            let Some(slot) = cube.duress_slot_file.as_deref() else {
                continue;
            };
            let secret =
                crate::services::unlock::device_secret::load_optional(&cube.id).map_err(|e| {
                    format!(
                        "Couldn't reach your system keychain to clear duress on Cube '{}' ({e}).",
                        cube.name
                    )
                })?;
            crate::services::unlock::marker::write_decoy(
                &root,
                cube.network,
                &cube.id,
                slot,
                secret.as_ref(),
            )
            .map_err(|e| {
                format!(
                    "Couldn't clear the duress PIN on Cube '{}' ({e}).",
                    cube.name
                )
            })?;
        }

        // The recorded names are deliberately **kept**. They name the slot,
        // not a marker, and the slot outlives any particular enrolment — see
        // `CubeSettings::duress_slot_file`. Clearing them here would strand
        // the decoy just written (nothing could find it again) and would
        // reintroduce exactly the tell 6b removes: a settings field that is
        // populated only on Cubes where duress happens to be armed.
        let _ = network_dir;
    }

    // 2. Reset DuressLocalState to the un-enrolled baseline (zeroizes the
    //    encrypted device code). `load` maps a missing file to default, so a
    //    never-enrolled device just rewrites a default; a real parse/IO error is
    //    surfaced rather than papered over. Only now is the device truly disarmed.
    let mut st = crate::services::duress::DuressLocalState::load(&root)
        .map_err(|e| format!("Couldn't read existing duress state: {e}"))?;
    st.disarm();
    st.save(&root)
        .map_err(|e| format!("Couldn't update local duress state: {e}"))?;

    // 3. Empty the durable activation queue — no pending activation should
    //    survive an un-enroll. Best-effort: a stale entry would only retry an
    //    already-disabled activation, so log rather than fail the disarm.
    if let Err(e) = crate::services::duress::queue::DuressQueue::new(&root).clear() {
        log::warn!("duress: failed to clear activation queue on disarm: {e}");
    }
    Ok(())
}

/// Whether any Cube on any network still has a duress PIN hash armed. Read-only.
/// Used to detect the *orphaned* state — Cubes armed while `DuressLocalState`
/// reads "not enrolled" — so a reconcile doesn't skip a device that a crash left
/// half-armed. A settings read error is surfaced rather than papered over (a
/// false "nothing armed" could leave a live wipe trigger in place).
fn any_cube_duress_armed(root: &std::path::Path) -> Result<bool, String> {
    // Reads the `arming` breadcrumb, not the datadir.
    //
    // This used to scan every Cube for a duress marker. Since unit 6b that
    // scan cannot work and must not be attempted: every Cube carries a second
    // slot from creation, and a marker is byte-indistinguishable from a decoy
    // by design, so "a marker exists" is either always true (if it means "the
    // slot exists") or unanswerable (if it means "the slot is real"). A scan
    // that appeared to work would be reading decoys and reporting every device
    // as armed.
    //
    // `DuressLocalState::arming` records the same fact directly: it is written
    // before enrollment arms the first Cube and cleared in the same write that
    // records `enrolled`. So `arming && !enrolled` is exactly the orphan the
    // scan was looking for, and it also catches the partially-armed crash the
    // scan never could.
    let st = crate::services::duress::DuressLocalState::load(root)
        .map_err(|e| format!("Couldn't read existing duress state: {e}"))?;
    Ok(st.arming && !st.enrolled)
}

/// Reconcile a possibly remote/offline duress *disable* against this device's
/// local state, returning whether a disarm actually ran. Disarms when EITHER:
///
/// * this device holds a Connect enrollment (`account_id` set) for `account_id`
///   — the account the server now reports as no longer enrolled; OR
/// * the device is *orphaned* — Cubes are still armed while `DuressLocalState`
///   reads "not enrolled". A hard crash between persist's Cube-arming step and
///   its state-write step leaves exactly this state. A properly-armed enrollment
///   (sovereign included) reads `enrolled == true`, so "armed while not enrolled"
///   uniquely flags an unfinished enrollment whose wipe trigger is still live —
///   we fail toward disarmed rather than leave it able to wipe after a disable.
///
/// A fully sovereign enrollment (`enrolled == true`, `account_id == None`) is
/// matched by neither branch and is therefore never disarmed by a server "not
/// enrolled" — sovereign duress is local-only and the server has no say over it.
/// The only sovereign state this can touch is an *orphaned* (crashed, unfinished)
/// one, which never reached the "enabled" confirmation; clearing its stale wipe
/// trigger is the safe, recoverable (re-enrollable) outcome.
pub(crate) async fn reconcile_duress_disarm(
    datadir: CoincubeDirectory,
    account_id: String,
) -> Result<bool, String> {
    let root = datadir.path().to_path_buf();
    let st = crate::services::duress::DuressLocalState::load(&root)
        .map_err(|e| format!("Couldn't read existing duress state: {e}"))?;

    let connect_enrollment_matches =
        st.enrolled && st.account_id.as_deref() == Some(account_id.as_str());
    // Only pay for the settings scan when the cheap flag check didn't already
    // decide it, and only when the local state claims "not enrolled" (a properly
    // enrolled device is handled by the flag check above).
    let orphaned_armed =
        !connect_enrollment_matches && !st.enrolled && any_cube_duress_armed(&root)?;

    if !(connect_enrollment_matches || orphaned_armed) {
        return Ok(false);
    }
    clear_duress_enrollment(datadir).await?;
    Ok(true)
}

/// Launch-time orphan check for an account the server still reports as ENROLLED.
/// Disarms ONLY when this device is *orphaned* — Cubes armed while
/// `DuressLocalState` never recorded the enrollment, the state a hard crash
/// between persist's Cube-arming and state-write steps leaves behind. The lost
/// device code can't be recovered, so the enrollment can't be completed; the
/// armed Cubes would otherwise stay a live wipe trigger that the panel reports
/// as inert ("not armed on this device"). Disarming makes the device honestly
/// match that copy and re-enrollable.
///
/// A healthy device (local state present → `enrolled == true`) or one with no
/// armed Cubes is a no-op, so a normally-enrolled device is NEVER disarmed here.
/// Returns whether a disarm ran.
pub(crate) async fn reconcile_duress_orphan(datadir: CoincubeDirectory) -> Result<bool, String> {
    let root = datadir.path().to_path_buf();
    let st = crate::services::duress::DuressLocalState::load(&root)
        .map_err(|e| format!("Couldn't read existing duress state: {e}"))?;
    // Local state present → a healthy, fully-recorded enrollment. Leave it.
    if st.enrolled {
        return Ok(false);
    }
    // Local state missing but Cubes armed → orphan. Disarm.
    if !any_cube_duress_armed(&root)? {
        return Ok(false);
    }
    clear_duress_enrollment(datadir).await?;
    Ok(true)
}

/// Poll the local bitcoind's IBD progress via its JSON-RPC interface.
/// Returns `(verificationprogress, initialblockdownload, subversion)` or an error
/// string. The subversion is `None` when the node would not say what it is.
async fn check_bitcoind_sync_progress(
    cfg: coincubed::config::BitcoindConfig,
) -> Result<(f64, bool, Option<String>), String> {
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

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| format!("bitcoind RPC client build failed: {}", e))?;
    let resp: serde_json::Value = client
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
    // Which build is actually syncing, so the progress copy can name it instead of
    // assuming Core. Read from the node's own subversion — the same source every
    // other flavour decision uses — and best-effort: an unreadable answer costs a
    // less specific sentence, not the progress report the user is waiting on.
    let subversion = subversion(&client, &url, &user, &pass).await;
    Ok((progress, ibd, subversion))
}

/// A node's `getnetworkinfo.subversion`, or `None` if it cannot be read.
async fn subversion(client: &reqwest::Client, url: &str, user: &str, pass: &str) -> Option<String> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "getnetworkinfo",
        "params": [],
        "id": 1
    });
    let resp: serde_json::Value = client
        .post(url)
        .basic_auth(user, Some(pass))
        .json(&body)
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    resp["result"]["subversion"].as_str().map(str::to_string)
}

/// Poll the active managed node's network participation stats — connection
/// counts, upload used vs. its daily cap, and the advertised onion address —
/// with two cheap RPCs (`getnetworkinfo` + `getnettotals`). Surfaced in the
/// Node settings so users can see their node is connected and (over Tor)
/// sharing.
async fn check_bitcoind_net_stats(
    cfg: coincubed::config::BitcoindConfig,
) -> Result<cache::NodeNetStats, String> {
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
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| format!("bitcoind RPC client build failed: {}", e))?;
    let call = |method: &'static str| {
        let (client, url, user, pass) = (client.clone(), url.clone(), user.clone(), pass.clone());
        async move {
            let resp: serde_json::Value = client
                .post(&url)
                .basic_auth(&user, Some(&pass))
                .json(&serde_json::json!({
                    "jsonrpc": "2.0", "method": method, "params": [], "id": 1
                }))
                .send()
                .await
                .map_err(|e| format!("{method} request failed: {e}"))?
                .json()
                .await
                .map_err(|e| format!("{method} parse failed: {e}"))?;
            // A JSON-RPC error comes back as HTTP 200 with a non-null `error`;
            // surface it instead of indexing a null `result` into all-zeros
            // stats (which the caller would then cache as if it were real).
            if !resp["error"].is_null() {
                return Err(format!("{method} error: {}", resp["error"]));
            }
            Ok(resp["result"].clone())
        }
    };

    let ni = call("getnetworkinfo").await?;
    let nt = call("getnettotals").await?;

    let upload_target = nt["uploadtarget"]["target"].as_u64().unwrap_or(0);
    let upload_used = if upload_target > 0 {
        upload_target.saturating_sub(
            nt["uploadtarget"]["bytes_left_in_cycle"]
                .as_u64()
                .unwrap_or(0),
        )
    } else {
        nt["totalbytessent"].as_u64().unwrap_or(0)
    };
    let onion_address = ni["localaddresses"].as_array().and_then(|addrs| {
        addrs.iter().find_map(|a| {
            a["address"]
                .as_str()
                .filter(|s| s.ends_with(".onion"))
                .map(|s| format!("{s}:{}", a["port"].as_u64().unwrap_or(8333)))
        })
    });

    Ok(cache::NodeNetStats {
        connections_in: ni["connections_in"].as_u64().unwrap_or(0),
        connections_out: ni["connections_out"].as_u64().unwrap_or(0),
        upload_used,
        upload_target,
        onion_address,
        subversion: ni["subversion"].as_str().map(str::to_string),
    })
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
/// Derives the active Cube's Connect-blinding encryption key
/// (`SPEC-cube-xpub-envelope-v1` §3) from the master signer the Cube was
/// unlocked with, for [`Cache::cube_encryption_key`].
///
/// The signer reached through the Breez client is the Cube's **master seed**
/// signer — all wallets (Vault, Liquid, Spark) derive from it, and it's the
/// same fingerprint recorded in `CubeSettings::master_signer_fingerprint`, so
/// the key derived here is the one whose public half was registered with
/// Connect at unlock.
///
/// Returns `None` when there's no on-disk seed to derive from (watch-only /
/// descriptor-only restores, passkey Cubes): blinded keys then surface as
/// `KeyResolveError::Locked` rather than being wrongly reported invalid.
fn derive_cube_encryption_key(
    breez_client: &BreezClient,
    network: coincube_core::miniscript::bitcoin::Network,
) -> Option<Arc<crate::services::connect::crypto::CubeEncryptionKey>> {
    let signer = breez_client.liquid_signer()?;
    let guard = signer.lock().ok()?;
    Some(Arc::new(
        crate::services::connect::crypto::CubeEncryptionKey::derive(&guard, network),
    ))
}

/// Loads (minting on first use) this device's Connect signing-rail transport
/// key for [`Cache::connect_transport_key`].
///
/// A failure here is non-fatal and logged: the desktop simply can't run
/// end-to-end signing sessions this launch, and `KeychainSignModal` fails those
/// rows closed rather than downgrading to a plaintext rail.
fn load_connect_transport_key(
    network_dir: &crate::dir::NetworkDirectory,
) -> Option<Arc<crate::services::connect::crypto::DeviceTransportKey>> {
    match crate::services::connect::crypto::DeviceTransportKey::load_or_create(network_dir) {
        Ok(k) => Some(Arc::new(k)),
        Err(e) => {
            tracing::warn!("Could not load this device's Connect transport key: {e}");
            None
        }
    }
}

fn connect_stream_ready_task(
    network: coincube_core::miniscript::bitcoin::Network,
    datadir: CoincubeDirectory,
    tokens: Arc<tokio::sync::RwLock<crate::services::connect::client::auth::AccessTokenResponse>>,
    email: String,
    cube_uuid: Option<String>,
) -> Task<Message> {
    use crate::services::connect::client::cache::Account;
    use crate::services::connect::client::resolve_connect_grpc_url;
    use crate::services::connect::grpc::stream::ConnectStreamConfig;

    Task::perform(
        async move {
            let Some(grpc_url) = resolve_connect_grpc_url().await else {
                tracing::info!("Connect stream bootstrap: no Connect gRPC URL available");
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

/// The unix time this Vault still owes a rescan from, if any.
///
/// Read from `settings.json` rather than carried in memory: the rescan it drives
/// can take hours on mainnet, and a quit or a crash part-way through has to
/// leave the Vault still owing one. `App::new` clears the field only once the
/// daemon has accepted a rescan, so an interrupted one is offered again on the
/// next launch.
fn pending_rescan(
    data_dir: &CoincubeDirectory,
    network: bitcoin::Network,
    wallet: &Wallet,
) -> Option<settings::PendingRescan> {
    settings::WalletSettings::from_file(&data_dir.network_directory(network), |s| {
        s.descriptor_checksum == wallet.descriptor_checksum
    })
    .ok()
    .flatten()
    .and_then(|s| s.pending_rescan)
}

/// `settings` with this Vault's pending-rescan marker cleared, and everything
/// else untouched.
///
/// Returns `Settings`, not `Option<Settings>`, on purpose. The updater passed to
/// [`settings::update_settings_file`] deletes `settings.json` outright when it
/// returns `None` — so an updater that looks a record up with `?` and finds
/// nothing does not "skip the write", it **wipes every Cube's configuration**.
/// A checksum that matches no wallet is an ordinary miss (a Vault removed while
/// the rescan was starting, a concurrent rewrite), and it must leave the file
/// exactly as it was. Making the miss unrepresentable is what keeps that true.
fn cleared_pending_rescan(
    mut settings: settings::Settings,
    descriptor_checksum: &str,
) -> settings::Settings {
    if let Some(wallet) = settings
        .wallets
        .iter_mut()
        .find(|w| w.descriptor_checksum == descriptor_checksum)
    {
        wallet.pending_rescan = None;
    }
    settings
}

/// Ask the daemon for the rescan a restored Vault owes, and clear the debt once
/// it has taken it.
///
/// Fire-and-forget by design. The daemon owns the scan from here — progress
/// shows in Settings > Node, which is also where the user can start one by hand
/// — so nothing downstream waits on this and a failure must not block the app
/// from opening. The field is cleared only on success, so a daemon that refuses
/// (already rescanning, or a timestamp it rejects) leaves the Vault owing one
/// and it is tried again next launch.
fn start_pending_rescan(
    daemon: Arc<dyn Daemon + Sync + Send>,
    network_dir: crate::dir::NetworkDirectory,
    descriptor_checksum: String,
    timestamp: u32,
) -> Task<Message> {
    Task::perform(
        async move {
            if let Err(e) = daemon.start_rescan(timestamp).await {
                tracing::error!(
                    "Restored Vault needs a rescan from unix time {} but the daemon refused: {}. \
                     It can be started by hand in Settings > Node.",
                    timestamp,
                    e
                );
                return;
            }
            tracing::info!(
                "Started the rescan this restored Vault owed, from unix time {}.",
                timestamp
            );
            if let Err(e) = settings::update_settings_file(&network_dir, |settings| {
                Some(cleared_pending_rescan(settings, &descriptor_checksum))
            })
            .await
            {
                // The scan is running; we just failed to record that it was
                // asked for. Next launch asks again, which is wasteful but not
                // wrong — `start_rescan` refuses while one is in flight.
                tracing::warn!("Could not clear the pending rescan marker: {}", e);
            }
        },
        |_| Message::Tick,
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
        let mut cache = cache;
        // Connect blinding (PR D3): derive the Cube's encryption key once from
        // the master signer the unlock already loaded, so every surface that
        // opens a Connect-served key can do so without re-prompting for a PIN.
        cache.cube_encryption_key = derive_cube_encryption_key(&breez_client, cache.network);
        // …and load (or mint) this device's signing-rail transport key. Not
        // seed-derived and not Cube-scoped — it comes up with the rail at login
        // (PR D4).
        cache.connect_transport_key =
            load_connect_transport_key(&data_dir.network_directory(cache.network));
        let cache = cache;
        let config_arc = Arc::new(config);
        let liquid_backend = Arc::new(LiquidBackend::new(breez_client.clone()));
        let wallet_registry = crate::app::wallets::WalletRegistry::with_spark(
            liquid_backend.clone(),
            spark_backend.clone(),
        );

        // A Vault restored from a Recovery Kit lands its descriptors in a
        // watchonly wallet that has never scanned them, so it owes a rescan from
        // the kit's recorded birthday. Read off disk rather than threaded
        // through the loader messages: the rescan has to survive a quit or a
        // crash part-way through, which an in-memory flag would not.
        let pending_rescan = pending_rescan(&data_dir, cache.network, &wallet);
        let mut panels = Panels::new(
            breez_client.clone(),
            spark_backend.clone(),
            &cache,
            wallet.clone(),
            data_dir.clone(),
            daemon.backend(),
            internal_bitcoind.as_ref(),
            config_arc.clone(),
            restored_from_backup || pending_rescan.is_some(),
            cube_settings.id.clone(),
            cube_settings.name.clone(),
            settings::network_to_api_string(cache.network),
        );
        // Connect blinding (PR D2): hand the panel the seed-derived encryption
        // pubkey persisted at unlock, so the registration wave can publish it.
        panels
            .connect
            .set_cube_encryption_pubkey(cube_settings.connect_encryption_pubkey.clone());
        // The Vault's identity, for the assertion wave that fires once the
        // server cube id resolves (PLAN-vault-identity-unification D3/D4).
        // Seeded from settings here; the backfill below overwrites it on a Cube
        // whose settings predate the field.
        panels
            .connect
            .set_vault_fingerprint(cube_settings.vault_fingerprint.clone());
        let mut tasks = vec![];
        // Only a known date can be started unattended. `DateUnknown` still
        // raises the prompt above — the user supplies the date in Settings >
        // Node, because a guessed one would present as a scan that found
        // nothing.
        if let Some(timestamp) = pending_rescan.and_then(|p| p.timestamp()) {
            tasks.push(start_pending_rescan(
                daemon.clone(),
                data_dir.network_directory(cache.network),
                wallet.descriptor_checksum.clone(),
                timestamp,
            ));
        }
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
        // A managed node that came up stranded on the stalled BIP-110 fork was
        // repaired during startup, long before any of this existed to say so. The
        // sidecar carried the fact across; collect it here, where there is finally
        // a UI to show it in. Self-clearing, so it appears exactly once.
        if crate::node::revalidate::ManagedNodeState::take_repair_notice(&data_dir) {
            tasks.push(Task::done(Message::View(view::Message::ShowToast(
                log::Level::Info,
                crate::node::revalidate::CHAIN_REPAIRED_NOTICE.to_string(),
            ))));
        }
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
        // Liquid sunset gate, local half. `load_breez_client` has already run its
        // keep-or-discard policy (grandfathered wallet / server grant / a scan
        // that found funds), leaving `storage.sql` on disk only for a wallet
        // worth surfacing. Probe that on-disk state rather than the live
        // connection: on a network without a Liquid backend (Testnet, regtest
        // without Esplora) the SDK is returned *disconnected* even when a funded
        // wallet exists on disk, so keying the gate off `is_connected()` there
        // would hide the rail entirely instead of showing it network-gated. The
        // probe is also what the discard's "failed delete → spurious nav entry"
        // fallback assumes. The server half is mirrored in separately later.
        cache_with_vault.liquid_gate.local_state_exists =
            crate::app::breez_liquid::local_state_exists(data_dir.path(), cache_with_vault.network);
        cache_with_vault.connect_tokens = connect_auth_arc.clone();
        cache_with_vault.connect_email = connect_email.clone();
        // A restored on-disk session (or a threaded remote-backend one) means
        // the user is signed in to Connect even though the Connect panel won't
        // reach its `Dashboard` step — and so won't flip `connect_authenticated`
        // — until its keyring session check completes. Mirror it now so the
        // keychain-unavailable modal offers "Sign with Connect" rather than a
        // "Sign in to Connect" that would no-op.
        cache_with_vault.has_connect_session = connect_auth_arc.is_some();
        cache_with_vault.p2p_test_coordinator = panels
            .p2p
            .as_ref()
            .is_some_and(|p| p.has_test_coordinator());
        let mut app = Self {
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
            node_net_stats_probe_in_progress: false,
            daemon_switch_in_progress: false,
            auto_switch_suppressed: false,
            show_received_celebration: false,
            show_recovery_alerts_prompt: false,
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
        };
        // A Cube opened for the first time on a build that persists the Vault's
        // identity: compute it now, while the descriptor is in scope, and
        // converge both the local settings and Connect
        // (PLAN-vault-identity-unification D4).
        let backfill = app.vault_fingerprint_backfill_task();
        (app, Task::batch([cmd, backfill]))
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
            cube_id: cube_settings.id.clone(),
            recovery_kit_last_backed_up_descriptor_fingerprint: cube_settings
                .recovery_kit_last_backed_up_descriptor_fingerprint
                .clone(),
            recovery_kit_last_backed_up_keychain_descriptor_fingerprint: cube_settings
                .recovery_kit_last_backed_up_keychain_descriptor_fingerprint
                .clone(),
            recovery_kit_password_backed_up: cube_settings.recovery_kit_password_backed_up,
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
        // See the sibling assignments in `App::new` (PRs D2/D3).
        panels
            .connect
            .set_cube_encryption_pubkey(cube_settings.connect_encryption_pubkey.clone());
        panels
            .connect
            .set_vault_fingerprint(cube_settings.vault_fingerprint.clone());
        let mut cache = cache;
        cache.cube_encryption_key = derive_cube_encryption_key(&breez_client, network);
        cache.connect_transport_key =
            load_connect_transport_key(&datadir.network_directory(network));
        cache.has_p2p = panels.p2p.is_some();
        // See the sibling assignment in `App::new`: probe the on-disk Liquid
        // state (not the live connection) for the "wallet exists on this machine"
        // half of the sunset gate — a backend-gated network returns the SDK
        // disconnected even for a funded on-disk wallet, which would wrongly
        // hide the rail.
        cache.liquid_gate.local_state_exists =
            crate::app::breez_liquid::local_state_exists(datadir.path(), network);
        cache.p2p_test_coordinator = panels
            .p2p
            .as_ref()
            .is_some_and(|p| p.has_test_coordinator());

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
                node_net_stats_probe_in_progress: false,
                daemon_switch_in_progress: false,
                auto_switch_suppressed: false,
                show_received_celebration: false,
                show_recovery_alerts_prompt: false,
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

    /// Fire-and-forget vault recovery heartbeat (Estate Notifications —
    /// PR 2). Returns a detached task that POSTs
    /// `{earliest_recovery_height, computed_at}` after a sync when this
    /// account has a Heartbeat- or Full-tier monitored vault and a live
    /// descriptor on hand. The heartbeat NEVER blocks or affects sync — its
    /// result is discarded via `RecoveryHeartbeatSent`. Returns
    /// `Task::none()` whenever no heartbeat applies (not authenticated,
    /// monitoring off, vault id not yet resolved, or no live wallet).
    fn recovery_heartbeat_task(&self) -> Task<Message> {
        use crate::services::coincube::{VaultHeartbeatRequest, VaultMonitoringLevel};
        if !self.panels.connect.account.is_authenticated() {
            return Task::none();
        }
        let ra = &self.panels.global_settings.recovery_alerts;
        // The heartbeat must fire after every vault sync while authenticated
        // (Estate Notifications plan P2) — it can't depend on the user having
        // opened Settings, which is otherwise the only thing that hydrates the
        // monitoring config. If we're entitled and a Connect cube is resolved
        // but the config hasn't been fetched from any path yet, kick a
        // one-shot `LoadStatus` now (the same hydrator the settings card uses);
        // the next sync's heartbeat then sees the resolved level/vault_id. The
        // `loaded_once`/`loading` flags — shared with the settings card and
        // reset on logout — keep this to a single fetch, and gating on
        // cube + entitlement avoids prematurely marking an un-loadable config
        // as loaded before those prerequisites arrive.
        if !ra.loaded_once
            && !ra.loading
            && self.panels.connect.account.is_recovery_alerts_entitled()
            && self.panels.connect.cube.server_cube_id.is_some()
        {
            return Task::done(Message::View(view::Message::Settings(
                view::SettingsMessage::RecoveryAlerts(view::RecoveryAlertsMessage::LoadStatus),
            )));
        }
        // Defense in depth: never POST heartbeats for an account that isn't
        // Estate-entitled. The cached monitoring level/vault_id can outlive a
        // plan downgrade or Connect account switch — `ra.status` only reloads
        // on settings-open or logout — so re-check the *live* entitlement here,
        // the same gate the mutating monitoring APIs apply.
        if !self.panels.connect.account.is_recovery_alerts_entitled()
            || matches!(ra.level(), VaultMonitoringLevel::Off)
            // The heartbeat is cube-scoped, but this still gates on a
            // resolved vault id: it's the "does this cube actually have a
            // Vault" signal, distinct from `server_cube_id` (every Connect
            // cube has one; not every cube has a Vault).
            || ra.vault_id.is_none()
        {
            return Task::none();
        }
        let (Some(cube_id), Some(wallet), Some(client)) = (
            self.panels.connect.cube.server_cube_id,
            self.wallet.as_ref(),
            self.authenticated_coincube_client(),
        ) else {
            return Task::none();
        };
        let tip = self.cache.blockheight();
        if tip <= 0 {
            // Not synced enough to compute a meaningful recovery height yet.
            return Task::none();
        }
        // earliest_recovery_height = the absolute height at which this vault's
        // EARLIEST recovery branch opens. Under Liana CSV semantics a coin's
        // recovery path opens `timelock` blocks after the coin confirmed
        // (`remaining_sequence`), so the binding constraint is the OLDEST
        // confirmed coin — `min(block_height) + timelock` — NOT `tip +
        // timelock`. The latter (the open height of a coin confirming right
        // now) recedes one block per block as the tip advances, so under the
        // server's monotonic "newest report wins" rule the recovery height
        // would forever outrun the chain and the keyholder alert would fire
        // late or never. Mirror `coins_summary`'s coin filter: confirmed,
        // owned, unspent. With no such coins there are no funds at recovery
        // risk yet, so fall back to the tip-based estimate — harmless and
        // still monotonic.
        let timelock = wallet.main_descriptor.first_timelock_value() as i64;
        let oldest_confirmed = self
            .cache
            .coins()
            .iter()
            .filter(|c| c.spend_info.is_none() && crate::daemon::model::coin_is_owned(c))
            .filter_map(|c| c.block_height)
            .min();
        let earliest = match oldest_confirmed {
            Some(h) => h as i64 + timelock,
            None => tip as i64 + timelock,
        };
        let earliest = earliest.max(0) as u32;
        // The API's sweep keys the reported height to a chain tip, so tell it
        // which chain in the Esplora-proxy id form it expects. It only
        // distinguishes mainnet from testnet (and rejects any other value), so
        // every non-mainnet network reports as `bitcoin-testnet`. Omitting this
        // would let the server default to mainnet and mis-key a testnet vault.
        let network = match self.cache.network {
            bitcoin::Network::Bitcoin => "bitcoin-mainnet",
            _ => "bitcoin-testnet",
        };
        let req = VaultHeartbeatRequest {
            earliest_recovery_height: earliest,
            computed_at: chrono::Utc::now(),
            network: network.to_string(),
        };
        Task::perform(
            async move {
                client
                    .post_vault_heartbeat(cube_id, req)
                    .await
                    .map_err(|e| e.to_string())
            },
            Message::RecoveryHeartbeatSent,
        )
    }

    /// Show the one-time recovery-alerts consent prompt (PR 3) when this device
    /// holds the Vault and that Vault has keyholders, a Connect session, and no
    /// monitoring yet — and the prompt hasn't already been answered for this
    /// Cube. A walletless instance (`App::new_without_wallet`) can still have a
    /// Connect cube with keyholders, so gate on the local wallet or it would
    /// consent on behalf of a Vault it doesn't hold. Idempotent within a
    /// session (the overlay flag guards re-entry); the durable answer lives in
    /// `CubeSettings::recovery_alerts_prompt_answered`.
    ///
    /// Only ever called off the back of a monitoring-status load, because that
    /// load is what resolves the recipient list this gates on. Calling it any
    /// earlier reads an empty list and silently declines to prompt — which is
    /// exactly how the prompt used to be unreachable in every build.
    fn maybe_show_recovery_alerts_prompt(&mut self) {
        if should_show_recovery_alerts_prompt(
            self.show_recovery_alerts_prompt,
            self.cube_settings.recovery_alerts_prompt_answered,
            self.panels.connect.account.is_authenticated(),
            self.panels.connect.cube.server_cube_id.is_some(),
            self.panels.connect.account.is_recovery_alerts_entitled(),
            !self
                .panels
                .global_settings
                .recovery_alerts
                .recipients
                .notified
                .is_empty(),
            self.panels.global_settings.recovery_alerts.alerts_on(),
            self.wallet.is_some(),
        ) {
            self.show_recovery_alerts_prompt = true;
        }
    }

    /// Backfill this Cube's Vault identity
    /// (`plans/PLAN-vault-identity-unification.md` D4).
    ///
    /// Every Vault that predates the scheme has no fingerprint on either side,
    /// and the desktop is the only party that can supply one — the server holds
    /// no plaintext descriptor and by design never can. Wallet load is where
    /// the descriptor *is* in scope, so that is where the identity is computed,
    /// persisted to [`settings::CubeSettings::vault_fingerprint`], and asserted
    /// to Connect.
    ///
    /// Both halves run on every wallet load, including the mid-session one
    /// where a Vault is built inside an already-registered Cube — the Connect
    /// panel's own assertion triggers have both already passed by then, so
    /// leaving the PATCH to them would strand the new Vault's identity until
    /// the next launch.
    ///
    /// One-shot per Cube and idempotent: the persist is skipped when settings
    /// already carry the right value, and the PATCH self-latches. It also
    /// self-heals a fingerprint that drifted — a descriptor change (key
    /// rotation, membership change) mints a new identity, which is correct for
    /// a binding and is exactly why the *name* lives on the Cube instead.
    ///
    /// Until a Cube is opened here, both this list and Keychain show
    /// "Vault configured" with no id. That is the deliberate choice over
    /// showing a wrong or leaky one.
    ///
    /// Best-effort: a failed write costs one more launch before the id appears,
    /// and never blocks or affects anything the user is doing.
    fn vault_fingerprint_backfill_task(&mut self) -> Task<Message> {
        let Some(wallet) = self.wallet.as_ref() else {
            return Task::none();
        };
        let fingerprint = wallet.id_fingerprint().to_string();
        // Seed *and* fire the assertion wave regardless of whether the local
        // settings already agree: the server half converges independently of
        // the disk half, and a Cube can have been persisted here but never
        // PATCHed. Firing it here rather than leaving it to the Connect panel
        // is what covers a Vault built **mid-session** — the panel's own
        // triggers (`CubeRegistered`, `ensure_cube_registered`) have both
        // already passed by then on a registered Cube, so the identity would
        // otherwise wait for a relaunch. A no-op until there is a client and a
        // server cube id, and self-latching once sent.
        self.panels
            .connect
            .set_vault_fingerprint(Some(fingerprint.clone()));
        let assert_task = self.panels.connect.assert_vault_fingerprint();
        if !self.cube_settings.adopt_vault_fingerprint(&fingerprint) {
            return assert_task;
        }
        let network_dir = self
            .cache
            .datadir_path
            .network_directory(self.cache.network);
        let cube_id = self.cube_settings.id.clone();
        let persist_task = Task::perform(
            async move {
                settings::update_settings_file(&network_dir, |mut s| {
                    if let Some(cube) = s.cubes.iter_mut().find(|c| c.id == cube_id) {
                        cube.vault_fingerprint = Some(fingerprint);
                    }
                    Some(s)
                })
                .await
                .map_err(|e| e.to_string())
            },
            |res: Result<(), String>| match res {
                Ok(()) => Message::SettingsSaved,
                Err(e) => {
                    // Deliberately not an error toast: the Cube is fully usable
                    // without the persisted id, and it is recomputed at the
                    // next open.
                    log::warn!("failed to persist vault fingerprint: {e}");
                    Message::SettingsSaved
                }
            },
        );
        Task::batch([persist_task, assert_task])
    }

    /// Persist this Cube's "answered the consent prompt" flag to the settings
    /// file (PR 3). The in-memory `cube_settings` copy is set by the caller; this
    /// just makes it durable so the prompt never re-fires across restarts.
    fn persist_recovery_alerts_answered(&self) -> Task<Message> {
        let network_dir = self
            .cache
            .datadir_path
            .network_directory(self.cache.network);
        let cube_id = self.cube_settings.id.clone();
        Task::perform(
            async move {
                settings::update_settings_file(&network_dir, |mut s| {
                    if let Some(cube) = s.cubes.iter_mut().find(|c| c.id == cube_id) {
                        cube.recovery_alerts_prompt_answered = true;
                    }
                    Some(s)
                })
                .await
                .map_err(|e| format!("Failed to save recovery-alerts answer: {}", e))
            },
            |res: Result<(), String>| match res {
                Ok(()) => Message::SettingsSaved,
                Err(e) => Message::View(view::Message::ShowError(e)),
            },
        )
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

    /// Write the account's latest `liquidEnabled` grant to this cube's settings
    /// so the next cube open can act on it (see `CubeSettings::liquid_granted`
    /// for why a persisted copy is needed at all).
    ///
    /// Best-effort: a failed write only costs the user a relaunch before a new
    /// grant takes effect, and it can never hide an existing Liquid wallet —
    /// that is decided by whether the wallet is on disk, not by this flag.
    fn persist_liquid_grant(&self, granted: bool) -> Task<Message> {
        let network_dir = self
            .cache
            .datadir_path
            .network_directory(self.cache.network);
        let cube_id = self.cache.cube_id.clone();
        Task::perform(
            async move {
                settings::update_settings_file(&network_dir, move |mut current| {
                    if let Some(cube) = current.cubes.iter_mut().find(|c| c.id == cube_id) {
                        cube.liquid_granted = Some(granted);
                    }
                    Some(current)
                })
                .await
            },
            |res| {
                if let Err(e) = res {
                    log::warn!("Failed to persist the Liquid grant: {}", e);
                }
                Message::Tick
            },
        )
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
                    menu::CubeSettingsOption::Recovery => {
                        Some(view::SettingsMessage::RecoverySection)
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
                    let daemon = self.daemon.clone();
                    if let Some(panel) = self.panels.current_mut() {
                        return panel.update(
                            daemon,
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
                                            menu::SettingsOption::LocalSigning => {
                                                view::SettingsMessage::LocalSigningSection
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
                        menu::VaultSubMenu::Recovery
                            if self
                                .panels
                                .recovery
                                .as_ref()
                                .is_none_or(|p| !p.keep_state()) =>
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

        // Keep the cross-network shift-status poll alive off the Spark Receive
        // panel too: a swap settling while the user is elsewhere must still be
        // observed promptly, or GlobalHome can't register the arrival before the
        // deposit is auto-claimed and the flow hangs on "Bitcoin arriving". The
        // poll stops itself once the shift is terminal, so this is idle outside
        // the pre-settle window.
        if !matches!(
            self.panels.current,
            Menu::Spark(crate::app::menu::SparkSubMenu::Receive)
        ) {
            subscriptions.push(self.panels.spark_receive.sideshift_poll_subscription());
        }

        // Stream the internal bitcoind's debug.log for UpdateTip lines.
        //
        // Prefer a pending node (one being set up or switched to), but fall back to
        // the active managed node: a chain repair after a Core↔Knots swap reconnects
        // blocks on a node that is already live, with nothing pending, and until now
        // that progress was invisible.
        if let Some(pending_cfg) = self.daemon.as_ref().and_then(|d| d.config()).and_then(|c| {
            c.pending_bitcoind.clone().or_else(|| {
                match c.bitcoin_backend.as_ref() {
                    Some(coincubed::config::BitcoinBackend::Bitcoind(cfg)) => Some(cfg.clone()),
                    // Electrum/Esplora backends have no debug.log to tail.
                    _ => None,
                }
            })
        }) {
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
            // Stop the managed Tor daemon (if inbound-over-Tor was running)
            // alongside the node it serves.
            crate::node::tor::stop_managed_tor();
        }
    }

    pub fn on_tick(&mut self) -> Task<Message> {
        // Skip tick processing if no vault is configured
        if self.daemon.is_none() {
            tracing::debug!("Skipping tick - no vault configured");
            return Task::none();
        }
        // Skip while a backend switch is in flight: `self.daemon` still points at
        // the daemon the off-thread switch is stopping, so polling it here would
        // hit a stopped poller/RPC. The next tick after `DaemonRestarted` swaps in
        // the new daemon resumes normally.
        if self.daemon_switch_in_progress {
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

            // Poll the active managed node's network participation stats
            // (connections / upload / onion) for the Node settings, on the same
            // cadence. Only when the backend is a local Bitcoind node.
            if !self.node_net_stats_probe_in_progress {
                if let Some(coincubed::config::BitcoinBackend::Bitcoind(cfg)) = self
                    .daemon
                    .as_ref()
                    .and_then(|d| d.config())
                    .and_then(|c| c.bitcoin_backend.clone())
                {
                    self.node_net_stats_probe_in_progress = true;
                    tasks.push(Task::perform(
                        check_bitcoind_net_stats(cfg),
                        Message::BitcoindNetStats,
                    ));
                }
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
            M::DuressActivated { unlock_at, source } => {
                // Phase 7b: remote duress activation. Persist active state to
                // DuressLocalState (at the data-dir root) WITHOUT wiping —
                // remote activation can be accidental, so local Cube data is
                // left intact. The UI lock is sequenced AFTER the persist (it's
                // the task's completion message, not a parallel batch) so the
                // cryptic screen never appears before `active` is durable; if
                // the app exits right after seeing the lock, the relaunch
                // reconcile still routes back into duress.
                log::warn!(
                    "[CONNECT GRPC] Duress activated remotely (source={})",
                    source
                );
                let root = self.datadir.path().to_path_buf();
                Task::perform(
                    async move {
                        // load() already maps a missing file to Ok(default); a
                        // real parse/IO error must NOT be papered over with a
                        // default that save() then writes back, clobbering valid
                        // state (enrolled / account_id / encrypted code). Skip
                        // the persist on a real read error — the UI still locks
                        // below, and the DuressLockRemote backstop / session-check
                        // re-sync handle durability.
                        let mut st = match crate::services::duress::DuressLocalState::load(&root) {
                            Ok(st) => st,
                            Err(e) => {
                                log::warn!(
                                    "[CONNECT GRPC] reading duress state failed; \
                                     not overwriting: {e}"
                                );
                                return;
                            }
                        };
                        st.active = true;
                        st.unlock_at = unlock_at;
                        // Retry: a failed persist would let a relaunch reconcile
                        // (which keys off st.active) drop back to the normal Home
                        // flow with Cube data intact while the server is still in
                        // duress. The DuressLockRemote handler re-persists as a
                        // final backstop tied to the UI lock.
                        for attempt in 1..=3 {
                            match st.save(&root) {
                                Ok(()) => break,
                                Err(e) => log::warn!(
                                    "[CONNECT GRPC] persist remote duress state \
                                     attempt {attempt}/3 failed: {e}"
                                ),
                            }
                        }
                    },
                    |_| Message::View(view::Message::DuressLockRemote),
                )
            }
            M::DuressCleared => {
                log::info!("[CONNECT GRPC] Duress cleared remotely");
                let root = self.datadir.path().to_path_buf();
                Task::perform(
                    async move {
                        // As above: skip on a real read error so a transient
                        // failure can't clobber valid state with a default. The
                        // cryptic screen's own server poll re-syncs the cleared
                        // state on the next check.
                        let mut st = match crate::services::duress::DuressLocalState::load(&root) {
                            Ok(st) => st,
                            Err(e) => {
                                log::warn!(
                                    "[CONNECT GRPC] reading duress state failed; \
                                     not overwriting: {e}"
                                );
                                return;
                            }
                        };
                        st.active = false;
                        st.unlock_at = None;
                        if let Err(e) = st.save(&root) {
                            log::warn!("[CONNECT GRPC] Failed to clear remote duress state: {e}");
                        }
                    },
                    |_| Message::CacheUpdated,
                )
            }
            M::DuressDisabled { account_id } => {
                // Issue 2: duress disabled account-wide on another device. Disarm
                // THIS device locally (clear Cube PIN hashes + local state +
                // queue) — no UI lock, no wipe. `reconcile_duress_disarm` only
                // touches a matching Connect enrollment, so a sovereign local
                // enrollment (or an unrelated account) is left untouched. Refresh
                // the Duress panel afterwards if the user is sitting in it.
                log::info!("[CONNECT GRPC] Duress disabled remotely");
                let datadir = self.datadir.clone();
                let gen = self.panels.connect.account.session_generation();
                Task::perform(
                    async move { reconcile_duress_disarm(datadir, account_id).await },
                    move |res| {
                        Message::View(view::Message::ConnectAccount(
                            view::ConnectAccountMessage::Duress(
                                view::DuressMessage::DisarmComplete(res, gen),
                            ),
                        ))
                    },
                )
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

        // Keep the Connect auth mirror fresh every tick (not just on
        // ConnectAccount/ConnectCube messages) so surfaces that read it —
        // e.g. the keychain-unavailable modal deciding between "Sign in to
        // Connect" and "Sign with Connect" — never see a stale value after
        // a launch-time session restore.
        self.cache.connect_authenticated = self.panels.connect.account.is_authenticated();

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
                self.cache.has_connect_session = true;

                let network = self.cache.network;
                let datadir = self.cache.datadir_path.clone();
                // Use the authoritative Cube settings id. The cache mirror can
                // still be empty during local-vault startup.
                let cube_uuid = if self.cube_settings.id.is_empty() {
                    None
                } else {
                    Some(self.cube_settings.id.clone())
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
                            cache.active_email = Some(email_for_task.clone());
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
                        if let Some(grpc_url) =
                            crate::services::connect::client::resolve_connect_grpc_url().await
                        {
                            // Same helper as every other registration path.
                            // `ensure_device_registered` short-circuits on a
                            // cached device_id, so whichever path registers
                            // first owns this machine's name permanently —
                            // all four call sites must derive it identically.
                            let device_name = crate::utils::device::device_label();
                            ensure_device_registered_best_effort(
                                &grpc_url,
                                tokens_arc.clone(),
                                &network_dir,
                                &email_for_task,
                                device_name,
                                env!("CARGO_PKG_VERSION").to_string(),
                                std::env::consts::OS.to_string(),
                            )
                            .await;
                        } else {
                            tracing::warn!(
                                "InAppConnectLoginCompleted: no Connect gRPC URL available"
                            );
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
                self.connect_auth = Some(tokens.clone());
                self.connect_email = Some(email.clone());
                self.cache.connect_tokens = Some(tokens.clone());
                self.cache.connect_email = Some(email.clone());
                self.cache.has_connect_session = true;
                return connect_stream_ready_task(network, datadir, tokens, email, cube_uuid);
            }
            Message::EnsureConnectReady => {
                // The user is signed in to Connect but the signing stream
                // isn't ready. Run the same device-registration + stream
                // bootstrap the in-app login flow does, sourcing tokens
                // from `connect.json`. The async block decides the next
                // message itself so we can give accurate feedback: some
                // networks (e.g. testnet4) have no `grpc_url` in their
                // ServiceConfig, meaning Connect/Keychain relay signing is
                // unavailable there no matter how often the user retries —
                // we say so and point them at local Wi-Fi pairing instead.
                let network = self.cache.network;
                let datadir = self.cache.datadir_path.clone();
                let expected_email = self
                    .connect_email
                    .clone()
                    .or_else(|| self.cache.connect_email.clone());
                let cube_uuid = if self.cube_settings.id.is_empty() {
                    None
                } else {
                    Some(self.cube_settings.id.clone())
                };
                // The device/stream bootstrap below populates grpc_url /
                // tokens / device_id but NOT the cube's server-side id. When
                // that id is the missing Keychain prerequisite, register it
                // here too — otherwise "Sign with Connect" can never resolve
                // it. A live panel session registers through its authenticated
                // client; a restored connect.json (or remote) session whose
                // panel hasn't reached Dashboard has no panel client, so the
                // panel path would no-op — register directly from the restored
                // tokens (the same source the device bootstrap uses) and feed
                // the result back through the normal `CubeRegistered` path so
                // `current_cube_server_id` still populates.
                let cube_registration = if self.cache.current_cube_server_id.is_some() {
                    Task::none()
                } else if self.panels.connect.account.is_authenticated() {
                    self.panels.connect.ensure_cube_registered()
                } else {
                    let datadir = datadir.clone();
                    let net_str = settings::network_to_api_string(network);
                    let cube_name = self.cube_settings.name.clone();
                    // Report this Cube's Vault presence so other devices can
                    // evaluate the duress vault gate (PLAN-duress-vault-gate
                    // PR 3). Monotonic upgrade-only: report `true` only when
                    // this device holds the Vault, else omit (never clobber a
                    // `true` reported by another device). Captured before the
                    // async move.
                    let cube_has_vault =
                        self.cube_settings.vault_wallet_id.is_some().then_some(true);
                    let cube_uuid = cube_uuid.clone();
                    let registration_email = expected_email.clone();
                    Task::perform(
                        async move {
                            use crate::services::coincube::{CoincubeClient, RegisterCubeRequest};
                            use crate::services::connect::client::cache::Account;
                            let uuid = cube_uuid.ok_or_else(|| "no cube uuid".to_string())?;
                            let email = registration_email.ok_or_else(|| {
                                "EnsureConnectReady: no active Connect email to register the cube"
                                    .to_string()
                            })?;
                            let account =
                                Account::from_cache(&datadir.network_directory(network), &email)
                                    .ok()
                                    .flatten()
                                    .ok_or_else(|| {
                                        "EnsureConnectReady: no cached Connect session to \
                                         register the cube"
                                            .to_string()
                                    })?;
                            let mut client = CoincubeClient::new();
                            client.set_token(&account.tokens.access_token);
                            client
                                .register_cube(RegisterCubeRequest {
                                    uuid,
                                    name: cube_name,
                                    network: net_str,
                                    has_vault: cube_has_vault,
                                }) // upgrade-only Option<bool>
                                .await
                                .map_err(|e| e.to_string())
                        },
                        |r| {
                            Message::View(view::Message::ConnectCube(
                                view::ConnectCubeMessage::CubeRegistered(r),
                            ))
                        },
                    )
                };
                let bootstrap = Task::perform(
                    async move {
                        use crate::services::connect::client::cache::Account;
                        use crate::services::connect::grpc::bootstrap::ensure_device_registered_best_effort;

                        let info_toast = |msg: &str| {
                            Message::View(view::Message::ShowToast(
                                log::Level::Info,
                                msg.to_string(),
                            ))
                        };

                        let network_dir = datadir.network_directory(network);
                        let Some(expected_email) = expected_email else {
                            tracing::warn!(
                                "EnsureConnectReady: no active Connect email to match connect.json"
                            );
                            return info_toast(
                                "Couldn't find a Connect session. Sign in to Connect, or pair \
                                 a phone over Wi-Fi to sign locally.",
                            );
                        };
                        let Some(account) = Account::from_cache(&network_dir, &expected_email)
                            .ok()
                            .flatten()
                        else {
                            tracing::warn!(
                                "EnsureConnectReady: no cached Connect account for active email"
                            );
                            return info_toast(
                                "Couldn't find this Connect session. Sign in to Connect, or pair \
                                 a phone over Wi-Fi to sign locally.",
                            );
                        };
                        let email = account.email.clone();
                        let tokens_arc =
                            std::sync::Arc::new(tokio::sync::RwLock::new(account.tokens));

                        let Some(grpc_url) =
                            crate::services::connect::client::resolve_connect_grpc_url().await
                        else {
                            // No gRPC endpoint for this network — Connect
                            // relay signing can't work here. Don't pretend
                            // a retry will help.
                            tracing::info!(
                                "EnsureConnectReady: no Connect gRPC URL available for {network}"
                            );
                            return info_toast(
                                "Keychain signing over Connect isn't available on this network. \
                                 Pair a phone over Wi-Fi to sign locally instead.",
                            );
                        };

                        // Same helper as every other registration path — see
                        // the note on the in-app login path above.
                        let device_name = crate::utils::device::device_label();
                        let Some(_device_id) = ensure_device_registered_best_effort(
                            &grpc_url,
                            tokens_arc.clone(),
                            &network_dir,
                            &email,
                            device_name,
                            env!("CARGO_PKG_VERSION").to_string(),
                            std::env::consts::OS.to_string(),
                        )
                        .await
                        else {
                            return info_toast(
                                "Couldn't register this device with Connect. Try again, or pair a \
                                 phone over Wi-Fi to sign locally.",
                            );
                        };

                        // gRPC is available and the device is registered —
                        // bring the stream up so the cache fields populate,
                        // then the user can retry Sign via Keychain.
                        Message::TriggerConnectStreamReady {
                            network,
                            datadir,
                            tokens: tokens_arc,
                            email,
                            cube_uuid,
                        }
                    },
                    |m| m,
                );
                return Task::batch([cube_registration, bootstrap]);
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
            Message::BitcoindNetStats(res) => {
                self.node_net_stats_probe_in_progress = false;
                match res {
                    Ok(stats) => self.cache.node_net_stats = Some(stats),
                    // Transient (e.g. mid-restart) — keep the last good stats.
                    Err(e) => tracing::debug!("node net-stats poll failed: {e}"),
                }
            }
            Message::BitcoindSyncProgress(res) => {
                self.bitcoind_sync_probe_in_progress = false;
                match res {
                    Err(e) => tracing::warn!("Bitcoind sync check failed: {}", e),
                    Ok((progress, ibd, subversion)) => {
                        self.cache.node_bitcoind_sync_progress = Some(progress);
                        self.cache.node_bitcoind_ibd = Some(ibd);
                        // Keep the last good answer if this poll couldn't read it.
                        if subversion.is_some() {
                            self.cache.node_bitcoind_subversion = subversion;
                        }
                        // Auto-switch to the pending node once it's synced, but
                        // only if the user *adopted* it (`auto_switch_to_pending`).
                        // This fires even for a node that reused an existing
                        // chainstate and was therefore never observed in IBD —
                        // while never reverting a node merely parked by a
                        // switch-to-Connect or a Bitcoind-failure fallback (those
                        // clear the flag). Skip while a switch is already in
                        // flight, so we don't spawn overlapping switches every
                        // tick (which raced to load — and corrupted — the wallet).
                        if !ibd && !self.daemon_switch_in_progress && !self.auto_switch_suppressed {
                            let switch =
                                self.daemon.as_ref().and_then(|d| d.config()).and_then(|c| {
                                    // Promote unless the node was explicitly parked
                                    // (`Some(false)`). An absent flag (`None`) is a
                                    // legacy adopted node — the old build promoted it
                                    // on sync with no flag — so it must promote too.
                                    if c.auto_switch_to_pending == Some(false) {
                                        return None;
                                    }
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
                                    new_cfg.auto_switch_to_pending = Some(false);
                                    new_cfg.fallback_esplora = old_esplora;
                                    Some(new_cfg)
                                });
                            if let Some(new_cfg) = switch {
                                info!("Switching to local Bitcoind — node synced");
                                return self.spawn_daemon_switch(new_cfg);
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
                        // Same for the keychain (phone) drift slot (per-method
                        // drift, PR 3).
                        self.cache
                            .recovery_kit_last_backed_up_keychain_descriptor_fingerprint = cube
                            .recovery_kit_last_backed_up_keychain_descriptor_fingerprint
                            .clone();
                        self.cube_settings
                            .recovery_kit_last_backed_up_keychain_descriptor_fingerprint = cube
                            .recovery_kit_last_backed_up_keychain_descriptor_fingerprint
                            .clone();
                        // And the password kit's *presence* flag — the signal
                        // `has_recovery_kit()` reads for that method, which a
                        // Vault-less (seed-only) kit has no fingerprint to carry.
                        self.cache.recovery_kit_password_backed_up =
                            cube.recovery_kit_password_backed_up;
                        self.cube_settings.recovery_kit_password_backed_up =
                            cube.recovery_kit_password_backed_up;

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
            Message::UpdateDaemonCache(res) => {
                match res {
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
                        // Fire-and-forget recovery heartbeat after the sync's
                        // fresh tip lands (Estate Notifications — PR 2). Batched
                        // alongside the normal cache cascade so it never delays
                        // or blocks it.
                        let heartbeat = self.recovery_heartbeat_task();
                        return Task::batch([heartbeat, Task::done(Message::CacheUpdated)]);
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
                                            Some(coincubed::config::BitcoinBackend::Bitcoind(
                                                bc,
                                            )) => Some(bc.clone()),
                                            _ => None,
                                        };
                                        new_cfg.bitcoin_backend = Some(
                                            coincubed::config::BitcoinBackend::Esplora(fb.clone()),
                                        );
                                        new_cfg.pending_bitcoind = preserved_bitcoind;
                                        // Fell back to Connect after a Bitcoind
                                        // failure — park the node but don't
                                        // auto-revert to it on the next probe.
                                        new_cfg.auto_switch_to_pending = Some(false);
                                        new_cfg.fallback_esplora = None;
                                        new_cfg
                                    })
                                });
                            if let Some(new_cfg) = fallback {
                                if !self.daemon_switch_in_progress {
                                    info!("Switching to COINCUBE | Connect fallback after Bitcoind failure");
                                    return self.spawn_daemon_switch(new_cfg);
                                }
                            }
                        }
                    }
                }
            }
            Message::CompleteDuressEnrollment(payload) => {
                // Drop a completion that outlived its Connect session: a logout
                // or session reset bumps `session_generation`, and persisting
                // now would arm the duress PIN + DuressLocalState for an account
                // the user is no longer signed into.
                if payload.gen != self.panels.connect.account.session_generation() {
                    log::warn!("duress: ignoring enrollment completion from a stale session");
                    return Task::none();
                }
                // Phases 2 & 8: the Connect panel collected + validated the
                // credentials and (for Connect tiers) enrolled on the server;
                // persist the duress PIN + this device's encrypted code here,
                // where the Cube + datadir context lives. Shared with the Home
                // and Launcher surfaces, which also host the Connect Duress
                // panel (see `persist_duress_enrollment`).
                let crate::app::message::DuressEnrollmentPayload {
                    duress_pin,
                    duress_code,
                    account_id,
                    ..
                } = payload;
                let datadir = self.datadir.clone();
                return Task::perform(
                    persist_duress_enrollment(datadir, duress_pin, duress_code, account_id),
                    |res| res,
                )
                .then(|res| match res {
                    Ok(()) => Task::batch([
                        // Reflect the enabled state only now that the duress PIN
                        // is actually armed on every Cube.
                        Task::done(Message::View(view::Message::ConnectAccount(
                            view::ConnectAccountMessage::Duress(
                                view::DuressMessage::EnrollmentPersisted,
                            ),
                        ))),
                        Task::done(Message::CacheUpdated),
                    ]),
                    Err(e) => {
                        log::error!("duress: failed to persist enrollment: {e}");
                        Task::done(Message::View(view::Message::ShowError(format!(
                            "Couldn't finish enabling duress mode: {e}. Please try again."
                        ))))
                    }
                });
            }
            Message::RecoveryHeartbeatSent(res) => {
                // Fire-and-forget: the heartbeat must never affect app state
                // or sync. Log a transient failure at debug and move on (a
                // newer report always wins server-side, so a dropped one is
                // harmless).
                if let Err(e) = res {
                    log::debug!("[RECOVERY] heartbeat post failed (ignored): {e}");
                }
                return Task::none();
            }
            Message::CubeVaultReported => {
                // Fire-and-forget: the re-report task logs its own outcome; the
                // terminal message just closes the task. Nothing to do.
                return Task::none();
            }
            Message::View(view::Message::RecoveryAlertsConsent(accept)) => {
                // Answer the one-time consent prompt (PR 3). Dismiss the overlay
                // either way.
                self.show_recovery_alerts_prompt = false;
                if !accept {
                    // Decline: record durably and move on — never re-prompt.
                    self.cube_settings.recovery_alerts_prompt_answered = true;
                    return self.persist_recovery_alerts_answered();
                }
                // Accept: turn alerts on now (cube-scoped POST). Route the result
                // through the settings card's `ChangeResult`, which reflects
                // alerts-on on the card AND — on `Ok` — marks + persists the prompt
                // answered (the shared `is_ok_change` handler below). So the answer
                // is recorded only after the enable actually succeeds: a
                // post-dispatch API failure surfaces there and leaves the prompt
                // un-answered, so it re-fires later. `enable_alerts` is idempotent,
                // so a redundant accept is safe.
                //
                // If the Connect session dropped while the overlay was up (token
                // expiry / account switch) there's nothing to dispatch: leave the
                // prompt un-answered (it re-fires once Connect is back) and surface
                // a visible error rather than failing silently.
                let gen = self.panels.connect.account.session_generation();
                match (
                    self.authenticated_coincube_client(),
                    self.panels.connect.cube.server_cube_id,
                ) {
                    (Some(client), Some(cube_id)) => {
                        return Task::perform(
                            async move {
                                crate::services::inheritance::enable_alerts(&client, cube_id)
                                    .await
                                    .map_err(|e| e.to_string())
                            },
                            move |res| {
                                Message::View(view::Message::Settings(
                                    view::SettingsMessage::RecoveryAlerts(
                                        view::RecoveryAlertsMessage::ChangeResult(res, gen, None),
                                    ),
                                ))
                            },
                        );
                    }
                    _ => {
                        return Task::done(Message::View(view::Message::ShowError(
                            "Couldn't reach your Connect account — recovery alerts weren't turned \
                             on. We'll ask again."
                                .to_string(),
                        )));
                    }
                }
            }
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
                    let is_recovery_current = matches!(
                        current,
                        Menu::Vault(crate::app::menu::VaultSubMenu::Recovery)
                    );

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

                    // The recovery panel is a separate CreateSpendPanel and must
                    // be refreshed too, otherwise its sync status and balance stay
                    // frozen at the value captured when it was constructed.
                    if let Some(recovery) = self.panels.recovery.as_mut() {
                        commands.push(recovery.update(
                            Some(daemon.clone()),
                            &cache,
                            Message::UpdatePanelCache(is_recovery_current),
                        ));
                    }
                }

                return Task::batch(commands);
            }
            Message::LoadDaemonConfig(cfg) => {
                // Only switch if we have a vault (daemon and wallet exist). The
                // stop-old/start-new runs off the UI thread; the pending "syncing"
                // card is cleared in the `DaemonRestarted` success path.
                if self.daemon.is_some() && self.wallet.is_some() && !self.daemon_switch_in_progress
                {
                    return self.spawn_daemon_switch(*cfg);
                } else if self.daemon_switch_in_progress {
                    tracing::warn!("Ignoring backend switch — one is already in progress");
                    // Every LoadDaemonConfig originates in the node-settings panel,
                    // which has already flipped its local `processing` flags and is
                    // awaiting a DaemonConfigLoaded reply. Dropping the request
                    // silently would leave those flags to be cleared by an
                    // *unrelated* switch's later success — masquerading as this
                    // one and losing its config. Route an explicit error back so
                    // the panel resets and the user can retry once the in-flight
                    // switch finishes.
                    return self.update_dispatch(Message::DaemonConfigLoaded(Err(Error::Config(
                        "A backend switch is already in progress. If a local node is still \
                         scanning the blockchain this can take a while — please wait for it \
                         to finish, then try again."
                            .to_string(),
                    ))));
                } else {
                    tracing::warn!("Attempted to load daemon config without vault");
                }
            }
            Message::DaemonRestarted(outcome) => {
                self.daemon_switch_in_progress = false;
                self.cache.daemon_switch_in_progress = false;
                // Non-blocking toast emitted on top of the normal result (e.g. a
                // switch that succeeded but couldn't be persisted to disk).
                let mut extra = Task::none();
                let result = match outcome {
                    DaemonRestart::Started(daemon) => {
                        self.daemon = Some(daemon);
                        // A fresh successful switch (adopt / manual) re-arms
                        // auto-promotion that a prior failure had suppressed.
                        self.auto_switch_suppressed = false;
                        Ok(())
                    }
                    DaemonRestart::StartedNotPersisted(daemon) => {
                        // The switch succeeded (wallet is on the new backend); the
                        // only problem is the config wasn't saved. Report success
                        // but warn that it may not survive a restart.
                        self.daemon = Some(daemon);
                        self.auto_switch_suppressed = false;
                        extra = Task::done(Message::View(view::Message::ShowToast(
                            log::Level::Warn,
                            "Switched Bitcoin backend, but couldn't save the change to disk — \
                             it may revert if you restart Coincube."
                                .to_string(),
                        )));
                        Ok(())
                    }
                    DaemonRestart::Failed { error, recovered } => {
                        if let Some(daemon) = recovered {
                            self.daemon = Some(daemon);
                            // The recovered daemon still carries the armed
                            // `auto_switch_to_pending` + `pending_bitcoind`, so
                            // suppress auto-promotion to stop the same failing
                            // switch re-firing every poll. A later user-initiated
                            // switch re-arms it (see the Started arms).
                            self.auto_switch_suppressed = true;
                        } else {
                            // The old daemon was already stopped during the switch
                            // and recovery couldn't bring one back. Drop it rather
                            // than keep referencing a dead daemon that ticks and
                            // config loads would keep poking.
                            self.daemon = None;
                        }
                        error!("Daemon backend switch failed: {}", error);
                        Err(error)
                    }
                    DaemonRestart::Panicked(error) => {
                        // Unknown post-panic state. self.daemon still holds the
                        // pre-switch daemon (cloned, not taken), so leave it in
                        // place — a possibly-stopped backend the user can re-switch
                        // beats no backend on an open vault. Suppress auto-promotion
                        // so a still-armed config can't re-trigger the same panic
                        // every poll; a manual switch re-arms it.
                        self.auto_switch_suppressed = true;
                        error!("Daemon backend switch panicked; keeping previous daemon: {error}");
                        Err(error)
                    }
                };
                // A successful switch clears the pending local-node sync card.
                if result.is_ok() {
                    self.cache.node_bitcoind_sync_progress = None;
                    self.cache.node_bitcoind_ibd = None;
                    self.cache.node_bitcoind_subversion = None;
                    self.cache.node_bitcoind_last_log = None;
                }
                let cfg_task = self.update_dispatch(Message::DaemonConfigLoaded(result));
                return Task::batch([cfg_task, extra, Task::done(Message::CacheUpdated)]);
            }
            Message::WalletUpdated(Ok(wallet)) => {
                // Check if we're transitioning from no-vault to has-vault state
                let was_vaultless = !self.cache.has_vault;

                self.wallet = Some(wallet.clone());
                self.cache.has_vault = true;

                // A Vault attached mid-session never passes through `App::new`,
                // so run the same identity backfill here — otherwise a Vault
                // created in this session would render "Vault configured" with
                // no id until the next relaunch
                // (PLAN-vault-identity-unification D4).
                let vault_fp_task = self.vault_fingerprint_backfill_task();

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

                    // Duress vault gate (PLAN-duress-vault-gate PR 3): the Cube
                    // just gained a Vault. Re-report `has_vault` to the server so
                    // other devices' duress gate sees it without waiting for a
                    // re-registration, and — if duress is already enrolled —
                    // surface the extra Recovery-Kit nudge line (master decision
                    // 6): the freshly-created Vault's Wallet Descriptor isn't in
                    // the kit yet, so a duress wipe of it would be irreversible.
                    let report_task = self.panels.connect.report_vault_created();
                    let duress_nudge_task: Option<Task<Message>> =
                        if self.panels.connect.account.is_duress_enrolled() {
                            Some(Task::done(Message::View(view::Message::ShowToast(
                                log::Level::Info,
                                "Duress Mode is on — add this Vault's Wallet Descriptor to your \
                                 Recovery Kit so a duress wipe stays recoverable."
                                    .to_string(),
                            ))))
                        } else {
                            None
                        };
                    // Vault-setup-completion trigger (PR 3): the new Vault has a
                    // keyholder set and a Connect session — offer the one-time
                    // recovery-alerts consent prompt (alerts pre-selected on).
                    //
                    // Dispatched as a status load rather than a direct
                    // `maybe_show_recovery_alerts_prompt()` because the prompt
                    // gates on the recipient list, and that list is resolved by
                    // this very load. The `is_status_load` branch in the
                    // RecoveryAlerts dispatch offers the prompt once the load
                    // lands with the new Vault's keyholders in hand.
                    //
                    // Gated on an authenticated session with a resolved Connect
                    // cube, like the Recovery-Kit nudge above. Dispatching it
                    // without both doesn't merely waste a queue hop: the
                    // handler's early return still marks the monitoring config
                    // `loaded_once`, and that permanently disables the lazy
                    // hydrator in `recovery_heartbeat_task` (gated on
                    // `!loaded_once`) for the rest of the session — so a Vault
                    // built before signing in to Connect would neither
                    // heartbeat nor prompt until Settings was opened by hand.
                    let alerts_load_task: Option<Task<Message>> =
                        (self.panels.connect.account.is_authenticated()
                            && self.panels.connect.cube.server_cube_id.is_some())
                        .then(|| {
                            Task::done(Message::View(view::Message::Settings(
                                view::SettingsMessage::RecoveryAlerts(
                                    view::RecoveryAlertsMessage::LoadStatus,
                                ),
                            )))
                        });

                    // Fold the re-report, the optional duress nudge and the
                    // optional alerts load into a single extra task so the
                    // return arms below stay simple.
                    let report_task = Task::batch(
                        std::iter::once(report_task)
                            .chain(duress_nudge_task)
                            .chain(alerts_load_task),
                    );
                    // Forward to the current panel; batch the nudge and the
                    // has_vault re-report in when present.
                    if let (Some(daemon), Some(panel)) =
                        (self.daemon.clone(), self.panels.current_mut())
                    {
                        let panel_task = panel.update(
                            Some(daemon),
                            &self.cache,
                            Message::WalletUpdated(Ok(wallet)),
                        );
                        return match nudge_task {
                            Some(nudge) => {
                                Task::batch([panel_task, nudge, report_task, vault_fp_task])
                            }
                            None => Task::batch([panel_task, report_task, vault_fp_task]),
                        };
                    }
                    return match nudge_task {
                        Some(nudge) => Task::batch([nudge, report_task, vault_fp_task]),
                        None => Task::batch([report_task, vault_fp_task]),
                    };
                }

                // Forward the message to the current panel
                if let (Some(daemon), Some(panel)) =
                    (self.daemon.clone(), self.panels.current_mut())
                {
                    let panel_task = panel.update(
                        Some(daemon),
                        &self.cache,
                        Message::WalletUpdated(Ok(wallet)),
                    );
                    return Task::batch([panel_task, vault_fp_task]);
                }
                return vault_fp_task;
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
                // Mirror the server-controlled Marketplace flags so the nav
                // rails and route guard reflect the latest `/connect/features`
                // stance. Recomputed after every ConnectAccount/ConnectCube
                // message (FeaturesLoaded flows through here, and logout clears
                // `features` back to fail-closed OFF via the accessor).
                self.cache.marketplace_flags =
                    self.panels.connect.account.marketplace_server_flags();
                // Same mirror for the Liquid sunset grant. Only the server half
                // is refreshed here — `local_state_exists` reflects whether the
                // Liquid SDK actually connected at cube-open, and must survive a
                // logout: losing the session must never hide an existing wallet.
                let liquid_granted = self.panels.connect.account.liquid_server_enabled();
                let grant_changed = self.cache.liquid_gate.server_enabled != liquid_granted;
                self.cache.liquid_gate.server_enabled = liquid_granted;
                // Persist the grant so the *next* cube open can act on it. The
                // Liquid SDK connects at PIN entry, long before Connect signs
                // in, so without this a newly-granted account would have its
                // (empty) Liquid wallet discarded on every launch and could
                // never actually get one. See `CubeSettings::liquid_granted`.
                let persist_grant = if grant_changed {
                    self.persist_liquid_grant(liquid_granted)
                } else {
                    Task::none()
                };
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
                    // Keep the panel's server-P2P mirror current so its Mostro
                    // subscription gate drops the relay stream when the server
                    // has P2P off (the cache flag was just refreshed above).
                    p2p.sync_marketplace_flags_from_cache(&self.cache);
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
                    self.cache.has_connect_session = false;
                    self.cache.connect_stream_status = ConnectionStatus::Inactive;
                    // Drop the previous session's vault-monitoring config so
                    // it can't leak into the next account and so the
                    // heartbeat's lazy `loaded_once` re-hydration re-fetches
                    // for whoever signs in next (the card itself lives here on
                    // `global_settings`, not on the connect panel that handles
                    // the rest of the logout scrub).
                    self.panels.global_settings.recovery_alerts =
                        crate::app::state::settings::recovery_alerts::RecoveryAlerts::new();
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
                    return Task::batch([task, persist_grant, nav, switch]);
                }
                return Task::batch([task, persist_grant]);
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
            // Collapses the advisory detail panel for one device and persists
            // that choice. The badge on the row is unaffected — an advisory is
            // never dismissed away entirely.
            Message::View(view::Message::DismissHwAdvisory(fingerprint, advisory_id)) => {
                crate::hw_advisory::dismissals::dismiss(&fingerprint, advisory_id);
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
            // The cross-network Receive flow lives in the (always-resident)
            // `spark_receive` panel, so route its messages there directly rather
            // than to the current panel. This is what lets a swap arrival reach
            // the flow while the user is on another screen; on the Receive screen
            // itself `spark_receive` is the current panel anyway, so it's the
            // same target. The flow's status poll is kept alive off-screen too
            // (`sideshift_poll_subscription`), so its `PollStatus`/`StatusUpdated`
            // messages land here regardless of which panel is current.
            msg @ Message::View(view::Message::SparkSideshiftReceive(_)) => {
                return self
                    .panels
                    .spark_receive
                    .update(self.daemon.clone(), &self.cache, msg);
            }

            Message::ShowReceivedCelebration {
                context,
                amount_sat,
            } => {
                // Drive the same global overlay the Liquid `PaymentSucceeded`
                // handler uses, but for a Spark deposit that just claimed (the
                // Spark bridge has no distinct "payment received" event for a
                // claim — only `DepositsChanged` — so the celebration is fired
                // explicitly from the auto-claim watcher in `GlobalHome`).
                use coincube_ui::component::amount::DisplayAmount;
                self.received_celebration_amount = bitcoin::Amount::from_sat(amount_sat)
                    .to_formatted_string_with_unit(self.cache.bitcoin_unit);
                self.received_celebration_quote =
                    coincube_ui::component::quote_display::random_quote(&context);
                self.received_celebration_image =
                    coincube_ui::component::quote_display::image_handle_for_context(&context);
                self.received_celebration_context = context.clone();
                self.show_received_celebration = true;
                // A Spark swap's bitcoin just landed. Forward the arrival to the
                // cross-network Receive flow so it advances from "Bitcoin
                // arriving" to its arrived confirmation instead of sitting stale.
                // Dispatched (not called inline) so it reaches the resident
                // `spark_receive` panel even if the user is on another screen —
                // arrival lands ~30 min after settle, by which point they've
                // usually navigated away. The flow ignores it unless it's
                // actually showing "Bitcoin arriving", so a plain (non-swap)
                // Spark receive is unaffected.
                if context == "spark-receive" {
                    return Task::done(Message::View(view::Message::SparkSideshiftReceive(
                        view::SparkSideshiftReceiveMessage::Arrived { amount_sat },
                    )));
                }
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
                    let task = p2p.update(self.daemon.clone(), &self.cache, msg);
                    // A P2P message may have changed the Mostro config (e.g.
                    // adding/selecting a test coordinator in Settings), so
                    // refresh the rail gate flag the sidebar reads.
                    self.cache.p2p_test_coordinator = self
                        .panels
                        .p2p
                        .as_ref()
                        .is_some_and(|p| p.has_test_coordinator());
                    return task;
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

            // Vault Recovery Alerts dispatch (Estate Notifications — PR 2).
            // Like the Recovery Kit above, the handler needs the
            // authenticated client, the Connect cube id, the live wallet
            // descriptor, and the `recovery_alerts` entitlement — none plumbed
            // through `State::update`. The keyholder list is *not* injected:
            // it's derived from the vault the handler's own load fetches.
            Message::View(view::Message::Settings(view::SettingsMessage::RecoveryAlerts(msg))) => {
                let client = self.authenticated_coincube_client();
                let server_cube_id = self.panels.connect.cube.server_cube_id;
                let wallet = self.wallet.clone();
                let entitled = self.panels.connect.account.is_recovery_alerts_entitled();
                let escrow_entitled = self.panels.connect.account.is_inheritance_escrow_entitled();
                let session_generation = self.panels.connect.account.session_generation();
                let local_cube_id = self.cube_settings.id.clone();
                // A *successful, current-session* status load is the only message
                // that reveals an eligible Vault for the one-time prompt; a
                // *successful user change* means the owner has engaged with the
                // alerts decision, so the prompt must never re-fire for this Cube
                // (else turning alerts off would immediately re-nudge — the
                // residual nudge is the banner, not the modal). Classify before
                // `msg` is moved into `update`.
                //
                // Gate the load on `Ok` + a matching generation: a failed load
                // leaves the monitoring state unknown (or resets it to no-vault),
                // and `update` drops a stale-generation load without hydrating
                // anything (see its guard), so neither reveals a fresh eligible
                // state to prompt off.
                let is_status_load = matches!(
                    msg,
                    view::RecoveryAlertsMessage::StatusLoaded(Ok(_), gen)
                        if gen == session_generation
                );
                // Only an *accepted* change counts as engagement. `update` drops
                // a `ChangeResult` whose captured generation no longer matches the
                // active session (see its stale-result guard), so gate on the same
                // match here — otherwise a stale success that never touched state
                // would still permanently mark the prompt answered.
                let is_ok_change = matches!(
                    msg,
                    view::RecoveryAlertsMessage::ChangeResult(Ok(_), gen, _)
                        if gen == session_generation
                );
                let task = crate::app::state::settings::recovery_alerts::update(
                    &mut self.panels.global_settings.recovery_alerts,
                    msg,
                    client,
                    server_cube_id,
                    wallet,
                    entitled,
                    escrow_entitled,
                    session_generation,
                    &self.cache,
                    &local_cube_id,
                );
                // The owner engaged with the alerts decision via the card → mark
                // the prompt answered (durably) so the modal never fires for this
                // Cube again.
                let engaged = if is_ok_change && !self.cube_settings.recovery_alerts_prompt_answered
                {
                    self.cube_settings.recovery_alerts_prompt_answered = true;
                    self.persist_recovery_alerts_answered()
                } else {
                    Task::none()
                };
                // Existing-Vault trigger (PR 3): a just-resolved `StatusLoaded`
                // may have revealed an eligible (keyholders, monitoring off,
                // unanswered) Vault. Offer the one-time consent prompt now.
                if is_status_load {
                    self.maybe_show_recovery_alerts_prompt();
                }
                return Task::batch([task, engaged]);
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

    /// Switch the daemon to `cfg` OFF the UI thread.
    ///
    /// Stopping the old daemon blocks until its poller finishes the work in
    /// flight — which over Esplora can be a slow wallet scan — and starting the
    /// new one is synchronous, so doing this inline (the previous
    /// `load_daemon_config`) froze the whole app on every backend switch. The
    /// blocking work now runs on a `spawn_blocking` task; the new daemon arrives
    /// via [`Message::DaemonRestarted`], which swaps it in on the UI thread.
    pub fn spawn_daemon_switch(&mut self, cfg: DaemonConfig) -> Task<Message> {
        // Mark a switch in flight so subsequent sync probes / triggers don't
        // re-fire it before it completes (the config only changes on success).
        self.daemon_switch_in_progress = true;
        // Mirror into the cache so the Node settings view reflects it.
        self.cache.daemon_switch_in_progress = true;
        let old_daemon = self.daemon.clone();
        let network = cfg.bitcoin_config.network;
        let wallet_id = self.wallet.as_ref().expect("wallet should exist").id();
        let mut daemon_config_path = self
            .cache
            .datadir_path
            .network_directory(network)
            .coincubed_data_directory(&wallet_id)
            .path()
            .to_path_buf();
        daemon_config_path.push("daemon.toml");
        Task::perform(
            async move {
                match tokio::task::spawn_blocking(move || {
                    restart_daemon_blocking(old_daemon, cfg, daemon_config_path)
                })
                .await
                {
                    Ok(outcome) => outcome,
                    // The blocking switch panicked. Still deliver a DaemonRestarted
                    // message so its handler clears `daemon_switch_in_progress` —
                    // otherwise the guard sticks `true` and blocks every later
                    // LoadDaemonConfig / auto-switch indefinitely. Report it as
                    // `Panicked` (not `Failed`) so the handler KEEPS the App's
                    // existing daemon (still held, since `old_daemon` was cloned)
                    // rather than nulling it and leaving the vault backend-less.
                    Err(join_err) => {
                        error!("daemon restart task panicked: {join_err}");
                        DaemonRestart::Panicked(Error::Config(format!(
                            "daemon restart task panicked: {join_err}"
                        )))
                    }
                }
            },
            Message::DaemonRestarted,
        )
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
                "has arrived.",
                view::Message::DismissReceivedCelebration,
            );
            view::dashboard(&self.panels.current, &self.cache, celebration)
        } else if let Some(reason) = features::route_availability(
            &self.panels.current,
            self.cache.network,
            self.cache.p2p_test_coordinator,
            self.cache.marketplace_flags,
            self.cache.liquid_gate,
        )
        .reason()
        .map(str::to_string)
        {
            // The active route targets a feature that isn't available on
            // this network (a restored or deep-linked route onto an item
            // that renders greyed in the rail). Show the shared
            // "unavailable" placeholder rather than a live panel. Checked
            // before `connect_settings_content` so a gated route that also
            // happens to be a Connect-settings page (e.g. Spark → Settings →
            // Lightning Address on a network where Spark is unavailable)
            // shows the placeholder instead of the live Connect UI.
            view::dashboard(
                &self.panels.current,
                &self.cache,
                view::feature_unavailable_panel(reason),
            )
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
        let content: Element<'_, Message> = match self.errors.is_empty() {
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
        };

        // One-time recovery-alerts consent prompt overlays everything (PR 3).
        // `opaque` makes the full-screen overlay capture mouse presses so a
        // backdrop click can't fall through to (and actuate) the content layer
        // beneath it in the Stack — a proper modal focus trap.
        if self.show_recovery_alerts_prompt {
            iced::widget::Stack::new()
                .push(content)
                .push(iced::widget::opaque(
                    recovery_alerts_consent_overlay().map(Message::View),
                ))
                .into()
        } else {
            content
        }
    }

    pub fn datadir_path(&self) -> &CoincubeDirectory {
        &self.cache.datadir_path
    }
}

/// Pure gating rules for the one-time recovery-alerts consent prompt (PR 3),
/// separated from `App::maybe_show_recovery_alerts_prompt` so they're unit-
/// testable without a full `App`. The prompt shows exactly when: this device
/// actually holds the Vault (a walletless instance has nothing to monitor and
/// must never consent on its behalf), it isn't already up, it hasn't been
/// answered for this Cube, there's an authenticated Connect session with a
/// resolved cube, alerts are available on the plan (defense-in-depth; universal
/// after API PR 3), the Vault has ≥1 reachable recovery recipient (someone the
/// server can actually email), and this Vault isn't already monitored.
#[allow(clippy::too_many_arguments)]
fn should_show_recovery_alerts_prompt(
    already_showing: bool,
    answered: bool,
    authenticated: bool,
    has_server_cube: bool,
    entitled: bool,
    has_recipients: bool,
    alerts_on: bool,
    has_vault: bool,
) -> bool {
    has_vault
        && !already_showing
        && !answered
        && authenticated
        && has_server_cube
        && entitled
        && has_recipients
        && !alerts_on
}

/// The one-time recovery-alerts consent prompt (PR 3): a centered modal card
/// over a dimmed backdrop, offering to turn on recovery alerts (pre-selected)
/// or decline. Both buttons resolve `view::Message::RecoveryAlertsConsent`,
/// which persists the answer so the prompt never re-fires. Copy is the C4
/// disclosure plus one line of *why*; no escrow mention (that stays a
/// settings-page choice). Returns a `view::Message` element so its buttons are
/// `Clone` (the top-level `Message` isn't); the caller maps it to `Message::View`.
fn recovery_alerts_consent_overlay<'a>() -> Element<'a, view::Message> {
    use coincube_ui::component::text::*;
    use coincube_ui::component::{button, card};
    use coincube_ui::theme;
    use iced::widget::{Column, Container, Row, Space};
    use iced::Length;

    let body = Column::new()
        .spacing(14)
        .max_width(460)
        .push(text("Alert your keyholders?").size(20).bold())
        .push(
            text(
                "Turn on recovery alerts so COINCUBE can email your keyholders when this Vault's \
                 recovery window opens. Without this, your keyholders are never told when your \
                 recovery window opens.",
            )
            .size(14),
        )
        .push(
            text(
                "COINCUBE learns only the block height at which the window opens and that this \
                 desktop checked in — never your addresses or balances.",
            )
            .size(13)
            .style(theme::text::secondary),
        )
        .push(Space::new().height(Length::Fixed(6.0)))
        .push(
            Row::new()
                .spacing(10)
                .push(
                    button::primary(None, "Turn on recovery alerts")
                        .on_press(view::Message::RecoveryAlertsConsent(true)),
                )
                .push(
                    button::secondary(None, "Not now")
                        .on_press(view::Message::RecoveryAlertsConsent(false)),
                ),
        );

    Container::new(card::simple(body).max_width(520))
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(24)
        .style(theme::container::custom(iced::Color::from_rgba(
            0.0, 0.0, 0.0, 0.6,
        )))
        .into()
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

/// Outcome of an off-UI-thread daemon restart (see [`restart_daemon_blocking`]).
#[derive(Debug)]
pub enum DaemonRestart {
    /// The new daemon started; swap it in.
    Started(Arc<dyn Daemon + Sync + Send>),
    /// The new daemon started and should be installed (the switch SUCCEEDED),
    /// but persisting `daemon.toml` failed. Not a switch failure — the wallet is
    /// on the new backend — but the change may not survive a restart, so the app
    /// installs the daemon AND shows a non-blocking warning.
    StartedNotPersisted(Arc<dyn Daemon + Sync + Send>),
    /// The switch failed. `recovered` is the previous daemon brought back up (if
    /// any) so the app stays usable.
    Failed {
        error: Error,
        recovered: Option<Arc<dyn Daemon + Sync + Send>>,
    },
    /// The blocking restart task itself panicked, so its outcome (and whether the
    /// old daemon was stopped) is unknown. Distinct from `Failed` because the App
    /// still holds the pre-switch daemon (`spawn_daemon_switch` clones it rather
    /// than taking it): the handler keeps that reference so an open vault isn't
    /// left with no backend, instead of nulling it like the clean-failure path.
    Panicked(Error),
}

/// Stop `old_daemon`, start a new one for `cfg`, then persist `daemon.toml`.
///
/// This is the blocking half of a backend switch, factored out of the App so it
/// can run on a `spawn_blocking` task rather than the UI thread (see
/// [`App::spawn_daemon_switch`]). `daemon.stop()` can block until a poller
/// finishes a slow scan, and `EmbeddedDaemon::start` is synchronous, so running
/// this inline froze the app. On a start failure the previous daemon is brought
/// back so the app is never left without one.
fn restart_daemon_blocking(
    old_daemon: Option<Arc<dyn Daemon + Sync + Send>>,
    cfg: DaemonConfig,
    daemon_config_path: std::path::PathBuf,
) -> DaemonRestart {
    let recovery_cfg = old_daemon.as_ref().and_then(|d| d.config().cloned());

    if let Some(daemon) = &old_daemon {
        if let Err(e) = Handle::current().block_on(async { daemon.stop().await }) {
            // Couldn't stop the old daemon — keep it rather than leave none.
            return DaemonRestart::Failed {
                error: e.into(),
                recovered: old_daemon.clone(),
            };
        }
    }

    let daemon: Arc<dyn Daemon + Sync + Send> = match EmbeddedDaemon::start(cfg) {
        Ok(d) => Arc::new(d),
        Err(start_err) => {
            // New daemon failed to start. Bring the old one back so the app is
            // left usable rather than dead.
            let recovered = recovery_cfg.and_then(|old_cfg| match EmbeddedDaemon::start(old_cfg) {
                Ok(old_daemon) => {
                    warn!(
                        "New daemon failed to start; recovered previous daemon. Start error: {}",
                        start_err
                    );
                    Some(Arc::new(old_daemon) as Arc<dyn Daemon + Sync + Send>)
                }
                Err(recovery_err) => {
                    error!(
                        "New daemon failed to start and recovery also failed: start={} recovery={}",
                        start_err, recovery_err
                    );
                    None
                }
            });
            return DaemonRestart::Failed {
                error: start_err.into(),
                recovered,
            };
        }
    };

    // Persist the new backend. The switch ALREADY succeeded — the new daemon is
    // running and about to be installed — so a failed `daemon.toml` write is NOT
    // a switch failure: reporting one would show an error in Settings while the
    // wallet is actually on the new backend. Treat it as success and just log
    // the persistence problem loudly; its only consequence is that the change
    // may not survive a restart (the stale on-disk config would reload).
    let persisted = (|| -> Result<(), Error> {
        let content =
            toml::to_string(&daemon.config()).map_err(|e| Error::Config(e.to_string()))?;
        OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&daemon_config_path)
            .map_err(|e| Error::Config(e.to_string()))?
            .write_all(content.as_bytes())
            .map_err(|e| Error::Config(e.to_string()))
    })();

    if let Err(e) = persisted {
        error!(
            "Backend switch succeeded but persisting daemon.toml failed; the wallet is on the \
             new backend now, but the change may not survive a restart: {:?}",
            e
        );
        return DaemonRestart::StartedNotPersisted(daemon);
    }
    DaemonRestart::Started(daemon)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `update_settings_file` **deletes** `settings.json` when its updater
    /// returns `None`. So an updater that looks a record up with `?` does not
    /// quietly skip the write when it misses — it destroys every Cube's
    /// configuration on this network.
    ///
    /// This clearing runs from a background task after a rescan starts, so a
    /// miss is entirely reachable: the Vault could have been removed, or another
    /// writer could have rewritten the file, between the task starting and
    /// finishing. It must be a no-op, not a wipe.
    #[test]
    fn clearing_a_pending_rescan_for_an_unknown_vault_changes_nothing() {
        use crate::app::settings::{PendingRescan, WalletSettings};

        let wallet = |checksum: &str, pending| WalletSettings {
            name: format!("Coincube-{}", checksum),
            alias: None,
            descriptor_checksum: checksum.to_string(),
            pinned_at: None,
            keys: Vec::new(),
            hardware_wallets: Vec::new(),
            remote_backend_auth: None,
            start_internal_bitcoind: None,
            pending_rescan: pending,
        };

        let before = settings::Settings {
            wallets: vec![
                wallet("kt6ht0kt", Some(PendingRescan::DateUnknown)),
                wallet("rsyfz849", Some(PendingRescan::From(1_784_953_848))),
            ],
            ..Default::default()
        };

        // A checksum that matches nothing: every wallet survives, untouched.
        let after = cleared_pending_rescan(before.clone(), "nosuchck");
        assert_eq!(after.wallets.len(), 2, "no wallet may be dropped");
        assert_eq!(
            after.wallets[0].pending_rescan,
            Some(PendingRescan::DateUnknown)
        );
        assert_eq!(
            after.wallets[1].pending_rescan,
            Some(PendingRescan::From(1_784_953_848))
        );

        // A checksum that matches: only that wallet's marker is cleared.
        let after = cleared_pending_rescan(before, "rsyfz849");
        assert_eq!(after.wallets.len(), 2);
        assert_eq!(
            after.wallets[0].pending_rescan,
            Some(PendingRescan::DateUnknown),
            "the other Vault's rescan is not ours to clear"
        );
        assert_eq!(after.wallets[1].pending_rescan, None);
    }

    use std::{
        fs,
        io::ErrorKind,
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    use coincube_core::miniscript::bitcoin::Network;

    use crate::{
        app::settings::{CubeSettings, Settings, SETTINGS_FILE_NAME},
        services::duress::DuressLocalState,
    };

    // Baseline: every gating precondition satisfied → prompt shows.
    fn prompt_args_all_go() -> (bool, bool, bool, bool, bool, bool, bool, bool) {
        // (already_showing, answered, authenticated, has_server_cube, entitled,
        //  has_recipients, alerts_on, has_vault)
        (false, false, true, true, true, true, false, true)
    }

    #[test]
    fn recovery_alerts_prompt_shows_when_all_conditions_met() {
        let (a, b, c, d, e, f, g, h) = prompt_args_all_go();
        assert!(should_show_recovery_alerts_prompt(a, b, c, d, e, f, g, h));
    }

    #[test]
    fn recovery_alerts_prompt_suppressed_by_each_gate() {
        // Already showing → don't stack a second prompt.
        assert!(!should_show_recovery_alerts_prompt(
            true, false, true, true, true, true, false, true
        ));
        // Already answered → durable, never re-prompt.
        assert!(!should_show_recovery_alerts_prompt(
            false, true, true, true, true, true, false, true
        ));
        // No Connect session.
        assert!(!should_show_recovery_alerts_prompt(
            false, false, false, true, true, true, false, true
        ));
        // No resolved Connect cube.
        assert!(!should_show_recovery_alerts_prompt(
            false, false, true, false, true, true, false, true
        ));
        // Not entitled (alerts unavailable on plan).
        assert!(!should_show_recovery_alerts_prompt(
            false, false, true, true, false, true, false, true
        ));
        // No reachable recipients → nobody to alert.
        assert!(!should_show_recovery_alerts_prompt(
            false, false, true, true, true, false, false, true
        ));
        // Already monitored → nothing to prompt for.
        assert!(!should_show_recovery_alerts_prompt(
            false, false, true, true, true, true, true, true
        ));
        // No local Vault (walletless instance) → nothing to consent for, even
        // with a Connect cube and keyholders present.
        assert!(!should_show_recovery_alerts_prompt(
            false, false, true, true, true, true, false, false
        ));
    }

    #[test]
    fn recovery_alerts_prompt_answered_flag_defaults_false_and_round_trips() {
        // Old settings.json without the field parses as "not answered".
        let cube: CubeSettings = serde_json::from_value(serde_json::json!({
            "id": "cube-1",
            "name": "Vault",
            "network": "bitcoin",
            "created_at": 0
        }))
        .unwrap();
        assert!(!cube.recovery_alerts_prompt_answered);

        // Once answered it survives a serialize/deserialize round-trip, so a
        // decline persists across restarts (never re-prompt).
        let mut answered = cube;
        answered.recovery_alerts_prompt_answered = true;
        let json = serde_json::to_string(&answered).unwrap();
        let back: CubeSettings = serde_json::from_str(&json).unwrap();
        assert!(back.recovery_alerts_prompt_answered);
    }

    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new(label: &str) -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock should be after unix epoch")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "coincube-app-{label}-{}-{unique}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("create test root");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// What this test's datadir actually contains, right now.
    ///
    /// Attached to the failure messages of the tests that write seed files.
    /// Both of them fail on the macOS CI runners and pass everywhere else, and
    /// both fail in the same way: a directory the test just created is not
    /// there when the next syscall runs (`store_encrypted` gets `ENOENT` from
    /// `open`; `duress_enroll_network_dirs` finds nothing on a root it read
    /// successfully moments earlier). A bare `unwrap` on a machine nobody can
    /// log into leaves no way to tell a vanished directory from one that was
    /// never created, so the assertions carry the tree with them.
    ///
    /// Remove once those runs are understood.
    fn describe_tree(root: &Path) -> String {
        fn walk(dir: &Path, depth: usize, out: &mut String) {
            let entries = match fs::read_dir(dir) {
                Ok(e) => e,
                Err(e) => {
                    out.push_str(&format!(
                        "{:indent$}<unreadable: {e}>\n",
                        "",
                        indent = depth * 2
                    ));
                    return;
                }
            };
            let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
            paths.sort();
            for p in paths {
                let name = p.file_name().unwrap_or_default().to_string_lossy();
                out.push_str(&format!("{:indent$}{name}\n", "", indent = depth * 2));
                if p.is_dir() {
                    walk(&p, depth + 1, out);
                }
            }
        }

        let mut out = format!(
            "TMPDIR={:?}\nroot={:?} exists={} is_dir={}\n",
            std::env::temp_dir(),
            root,
            root.exists(),
            root.is_dir(),
        );
        // The other tests' roots. `TMPDIR=runner.temp` ruled out the macOS
        // purge theory — the roots vanish on a volume with 96 GiB free — so the
        // question is whether whatever removes them is aiming at this one or
        // sweeping the whole temp directory. If the siblings are gone too it is
        // a sweep, and only the slow tests (two Argon2 passes at 256 MiB) live
        // long enough to notice.
        if let Ok(entries) = fs::read_dir(std::env::temp_dir()) {
            let mut siblings: Vec<String> = entries
                .flatten()
                .filter_map(|e| e.file_name().to_str().map(str::to_owned))
                .filter(|n| n.starts_with("coincube-"))
                .collect();
            siblings.sort();
            out.push_str(&format!(
                "sibling coincube-* dirs in TMPDIR ({}): {:?}\n",
                siblings.len(),
                siblings
            ));
        }
        walk(root, 1, &mut out);
        out
    }

    fn write_settings_dir(root: &Path, name: &str, settings: Settings) -> PathBuf {
        let dir = root.join(name);
        fs::create_dir_all(&dir).expect("create network dir");
        let bytes = serde_json::to_vec_pretty(&settings).expect("serialize settings");
        fs::write(dir.join(SETTINGS_FILE_NAME), bytes).expect("write settings");
        dir
    }

    fn cube(id: &str, name: &str, network: Network) -> CubeSettings {
        CubeSettings::new_with_raw_id(id.to_string(), name.to_string(), network)
    }

    #[test]
    fn connection_status_visibility_and_tooltips_are_stable() {
        assert!(!ConnectionStatus::Inactive.is_visible());
        assert_eq!(ConnectionStatus::Inactive.tooltip(), "Connect inactive");

        assert!(ConnectionStatus::Connecting.is_visible());
        assert!(ConnectionStatus::Connecting
            .tooltip()
            .starts_with("Connecting to Coincube Connect"));

        assert!(ConnectionStatus::Connected.is_visible());
        assert_eq!(ConnectionStatus::Connected.tooltip(), "Connected");

        let err = ConnectionStatus::Error("socket closed".to_string());
        assert!(err.is_visible());
        assert_eq!(err.tooltip(), "Connection error: socket closed");
    }

    #[test]
    fn daemon_unreachable_detection_is_limited_to_transport_failures() {
        assert!(is_daemon_unreachable(&Error::Daemon(
            DaemonError::DaemonStopped
        )));
        assert!(is_daemon_unreachable(&Error::Daemon(DaemonError::NoAnswer)));
        assert!(is_daemon_unreachable(&Error::Daemon(
            DaemonError::RpcSocket(Some(ErrorKind::ConnectionRefused), "refused".to_string())
        )));

        assert!(!is_daemon_unreachable(&Error::Daemon(DaemonError::Rpc(
            -1,
            "application error".to_string()
        ))));
        assert!(!is_daemon_unreachable(&Error::Daemon(DaemonError::Http(
            Some(500),
            "server error".to_string()
        ))));
        assert!(!is_daemon_unreachable(&Error::Unexpected(
            "not a daemon error".to_string()
        )));
    }

    #[test]
    fn duress_enroll_network_dirs_only_includes_settings_directories() {
        let root = TempRoot::new("duress-dirs");
        write_settings_dir(root.path(), "bitcoin", Settings::default());
        write_settings_dir(root.path(), "regtest", Settings::default());

        fs::create_dir_all(root.path().join("testnet")).expect("create ignored network dir");
        fs::write(root.path().join("settings.json"), "{}").expect("write root-level file");
        fs::write(root.path().join("loose-file"), "").expect("write ignored file");

        let mut names = duress_enroll_network_dirs(root.path())
            .expect("readable datadir")
            .into_iter()
            .map(|dir| {
                dir.path()
                    .file_name()
                    .expect("network dir has name")
                    .to_string_lossy()
                    .into_owned()
            })
            .collect::<Vec<_>>();
        names.sort();

        assert_eq!(names, vec!["bitcoin".to_string(), "regtest".to_string()]);
    }

    /// An unreadable datadir is not "this device has no Cubes".
    ///
    /// The lookup used to swallow the I/O error and return an empty list, and
    /// every caller reads empty as "no Cubes". Enrollment then refused with a
    /// message that is false, and — the dangerous half — `clear_duress_enrollment`
    /// disarmed nothing, reset the local state, and reported success while every
    /// Cube kept a live wipe trigger.
    #[test]
    fn an_unreadable_datadir_is_an_error_not_an_empty_cube_list() {
        let root = TempRoot::new("duress-unreadable");
        let missing = root.path().join("not-a-directory");

        let err = duress_enroll_network_dirs(&missing).expect_err("a missing datadir must error");
        assert!(
            err.contains("Couldn't read your data directory"),
            "the message must name the real problem: {}",
            err
        );

        // Every duress entry point refuses rather than reporting no Cubes.
        for (label, got) in [
            (
                "collision check",
                duress_pin_collision_check_blocking(&missing, "1234").unwrap_err(),
            ),
            (
                "step-up",
                verify_regular_cube_pin_blocking(&missing, "1234").unwrap_err(),
            ),
        ] {
            assert_ne!(
                got, DURESS_NO_CUBES_MSG,
                "{} still claims the device has no Cubes",
                label
            );
            assert_eq!(
                got, err,
                "{} should surface the read failure verbatim",
                label
            );
        }

        // The disable path is the one that must not succeed quietly: reporting
        // "duress is off" after clearing nothing is the false statement that
        // leaves a live wipe trigger behind.
        let cleared = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime")
            .block_on(clear_duress_enrollment(CoincubeDirectory::new(
                missing.clone(),
            )));
        assert_eq!(
            cleared.unwrap_err(),
            err,
            "disable reported success without reading a single Cube"
        );
    }

    /// Write an encrypted master seed for `cube`, so the trial-decrypt PIN
    /// checks below have something real to verify against.
    fn store_seed(root: &std::path::Path, cube: &crate::app::settings::CubeSettings, pin: &str) {
        use coincube_core::miniscript::bitcoin::secp256k1::Secp256k1;
        use coincube_core::signer::{MasterSigner, MASTER_SEED_LABEL};
        let secp = Secp256k1::signing_only();
        let signer = MasterSigner::generate(cube.network).unwrap();
        signer
            .store_encrypted(
                root,
                cube.network,
                &secp,
                Some((
                    format!("{}{}", MASTER_SEED_LABEL, cube.created_at),
                    cube.created_at,
                )),
                pin,
                &cube.id,
                None,
            )
            .unwrap_or_else(|e| {
                panic!(
                    "store_encrypted failed for cube {} on {}: {}\nmnemonics dir: {:?} exists={}\n{}",
                    cube.id,
                    cube.network,
                    e,
                    MasterSigner::mnemonics_folder(root, cube.network),
                    MasterSigner::mnemonics_folder(root, cube.network).exists(),
                    describe_tree(root),
                )
            });
    }

    #[test]
    fn duress_pin_collision_check_rejects_empty_and_real_cube_pin() {
        let root = TempRoot::new("duress-collision");
        assert_eq!(
            duress_pin_collision_check_blocking(root.path(), "1234").unwrap_err(),
            DURESS_NO_CUBES_MSG
        );

        let protected_cube = cube("cube-a", "Primary", Network::Bitcoin);
        let secondary_cube = cube("cube-b", "Secondary", Network::Regtest);
        // The collision check now decrypts these, so they have to exist.
        store_seed(root.path(), &protected_cube, "1234");
        store_seed(root.path(), &secondary_cube, "9999");

        write_settings_dir(
            root.path(),
            "bitcoin",
            Settings {
                cubes: vec![protected_cube],
                ..Settings::default()
            },
        );
        write_settings_dir(
            root.path(),
            "regtest",
            Settings {
                cubes: vec![secondary_cube],
                ..Settings::default()
            },
        );

        // Collides with cube-a's real unlock PIN...
        assert_eq!(
            duress_pin_collision_check_blocking(root.path(), "1234").unwrap_err(),
            DURESS_PIN_COLLIDES_MSG,
            "{}",
            describe_tree(root.path())
        );
        // ...and with cube-b's, even though it's on a different network. The
        // hash-based predecessor could only catch a Cube whose hash happened to
        // be recorded; this one opens the file.
        assert_eq!(
            duress_pin_collision_check_blocking(root.path(), "9999").unwrap_err(),
            DURESS_PIN_COLLIDES_MSG,
            "{}",
            describe_tree(root.path())
        );
        assert!(
            duress_pin_collision_check_blocking(root.path(), "5555").is_ok(),
            "{}",
            describe_tree(root.path())
        );
    }

    /// Arm duress on `cube` exactly as enrollment does: mint an unpredictable
    /// marker name, write the marker under it, and record it on the Cube.
    /// Recording is not optional — the name is random, so a marker whose name
    /// was not written down cannot be found again.
    fn arm_duress(root: &std::path::Path, cube: &mut CubeSettings, duress_pin: &str) {
        let name = crate::services::unlock::marker::new_file_name(
            crate::services::unlock::marker::seed_timestamp(
                root,
                cube.network,
                cube.master_signer_fingerprint,
                cube.created_at,
            ),
        );
        crate::services::unlock::marker::write(
            root,
            cube.network,
            &cube.id,
            &name,
            duress_pin,
            None,
        )
        .expect("arm duress");
        cube.duress_slot_file = Some(name);
    }

    #[test]
    fn verify_regular_cube_pin_accepts_real_pin_and_rejects_duress_pin() {
        let root = TempRoot::new("regular-pin");
        let mut cube = cube("cube-a", "Primary", Network::Bitcoin);
        store_seed(root.path(), &cube, "1234");
        arm_duress(root.path(), &mut cube, "8765");

        write_settings_dir(
            root.path(),
            "bitcoin",
            Settings {
                cubes: vec![cube],
                ..Settings::default()
            },
        );

        assert_eq!(
            verify_regular_cube_pin_blocking(root.path(), "").unwrap_err(),
            "Enter your Cube unlock PIN to continue."
        );
        assert!(
            verify_regular_cube_pin_blocking(root.path(), "1234").is_ok(),
            "{}",
            describe_tree(root.path())
        );
        // The duress PIN must not satisfy a step-up — and the rejection must
        // look identical to any other wrong PIN.
        assert_eq!(
            verify_regular_cube_pin_blocking(root.path(), "8765").unwrap_err(),
            DURESS_STEP_UP_BAD_PIN_MSG
        );
        assert_eq!(
            verify_regular_cube_pin_blocking(root.path(), "0000").unwrap_err(),
            DURESS_STEP_UP_BAD_PIN_MSG
        );
    }

    /// With no PIN-protected Cube on the device there is nothing to check the
    /// step-up against, so it must FAIL — never pass. Passing would let any
    /// non-empty string disarm duress everywhere, which is the one thing this
    /// step-up exists to prevent.
    #[test]
    fn verify_regular_cube_pin_refuses_when_no_cube_has_a_regular_pin() {
        let root = TempRoot::new("regular-pinless");
        write_settings_dir(
            root.path(),
            "bitcoin",
            Settings {
                cubes: vec![cube("cube-a", "Pinless", Network::Bitcoin)],
                ..Settings::default()
            },
        );

        assert_eq!(
            verify_regular_cube_pin_blocking(root.path(), "any pin").unwrap_err(),
            DURESS_STEP_UP_NO_PIN_MSG
        );
    }

    fn passkey_cube(id: &str, name: &str, network: Network) -> CubeSettings {
        cube(id, name, network).with_passkey(crate::app::settings::PasskeyMetadata {
            credential_id: "Y3JlZA==".to_string(),
            rp_id: "coincube.io".to_string(),
            created_at: 0,
            label: None,
        })
    }

    /// A device whose only Cube is a passkey Cube must be routed to the passkey
    /// step-up. Before this existed it landed on the PIN check, which such a
    /// device can never satisfy — so an enrolled user had no way to turn duress
    /// off at all.
    #[test]
    fn a_passkey_only_device_is_offered_the_passkey_step_up() {
        let root = TempRoot::new("stepup-passkey-only");
        write_settings_dir(
            root.path(),
            "bitcoin",
            Settings {
                cubes: vec![passkey_cube("cube-a", "Passkey Cube", Network::Bitcoin)],
                ..Settings::default()
            },
        );

        match duress_step_up_method_blocking(root.path()).expect("classified") {
            DuressStepUpMethod::Passkey(cube) => assert_eq!(cube.name, "Passkey Cube"),
            other => panic!(
                "a passkey-only device must offer the passkey step-up, got {:?}",
                other
            ),
        }
    }

    /// A PIN-protected Cube anywhere on the device wins, even alongside passkey
    /// Cubes: it is the path that already existed and costs no system prompt.
    #[test]
    fn a_pin_cube_takes_priority_over_a_passkey_cube() {
        let root = TempRoot::new("stepup-mixed");
        let mut pin_cube = cube("cube-pin", "PIN Cube", Network::Bitcoin);
        store_seed(root.path(), &pin_cube, "1234");
        pin_cube.duress_slot_file = None;

        write_settings_dir(
            root.path(),
            "bitcoin",
            Settings {
                cubes: vec![
                    passkey_cube("cube-pk", "Passkey Cube", Network::Bitcoin),
                    pin_cube,
                ],
                ..Settings::default()
            },
        );

        assert!(matches!(
            duress_step_up_method_blocking(root.path()).expect("classified"),
            DuressStepUpMethod::Pin
        ));
    }

    /// Neither factor: a Cube registered in Connect but never restored here has
    /// no seed to check a PIN against and no passkey to assert. Refusing is the
    /// only honest answer, and it is the one case
    /// [`DURESS_STEP_UP_NO_PIN_MSG`] is still for.
    #[test]
    fn a_device_with_neither_factor_can_offer_no_step_up() {
        let root = TempRoot::new("stepup-neither");
        write_settings_dir(
            root.path(),
            "bitcoin",
            Settings {
                cubes: vec![cube("cube-a", "Elsewhere", Network::Bitcoin)],
                ..Settings::default()
            },
        );

        assert!(matches!(
            duress_step_up_method_blocking(root.path()).expect("classified"),
            DuressStepUpMethod::Unavailable
        ));
    }

    /// A failed enrollment must not claim "no changes were kept" when a marker
    /// survived the rollback. That marker is a live wipe trigger on a Cube whose
    /// owner has been told nothing happened.
    #[test]
    fn a_failed_rollback_is_reported_not_swallowed() {
        let root = TempRoot::new("rollback-report");
        let mut cube = cube("cube-a", "Primary", Network::Bitcoin);
        arm_duress(root.path(), &mut cube, "8765");
        let marker_name = cube.duress_slot_file.clone().expect("recorded name");

        let armed = vec![ArmedMarker {
            root: root.path().to_path_buf(),
            cube_id: cube.id.clone(),
            cube_name: cube.name.clone(),
            network: Network::Bitcoin,
            file_name: marker_name.clone(),
            reused_slot: false,
        }];

        // Happy path: the marker comes off, and the message says so.
        let still_armed = rollback_duress_markers(&armed);
        assert!(still_armed.is_empty());
        // The slot survives as a decoy — rollback must not take this Cube
        // from two blobs to one, which would both undo 6b's shape and flag
        // the Cube where enrolment failed.
        assert!(
            crate::services::unlock::marker::exists(
                root.path(),
                Network::Bitcoin,
                Some(marker_name.as_str())
            ),
            "rollback deleted the slot instead of overwriting it with a decoy"
        );
        assert!(
            !crate::services::unlock::marker::verify(
                root.path(),
                Network::Bitcoin,
                &cube.id,
                Some(marker_name.as_str()),
                "8765",
                None,
            ),
            "the duress PIN still opens the slot after rollback — the wipe trigger is live"
        );
        let msg = describe_rollback("Couldn't arm.".to_string(), still_armed, Vec::new());
        assert!(msg.contains("No changes were kept"), "{}", msg);
        assert!(!msg.contains("WARNING"), "{}", msg);

        // Failure path: whatever the cause, the user is told which Cube is still
        // armed and what that PIN will now do.
        let msg = describe_rollback(
            "Couldn't arm.".to_string(),
            vec!["Primary".to_string()],
            Vec::new(),
        );
        assert!(msg.contains("Primary"), "the Cube must be named: {}", msg);
        assert!(msg.contains("erase this device"), "{}", msg);
        assert!(
            !msg.contains("No changes were kept"),
            "the message still claims nothing was kept: {}",
            msg
        );
    }

    /// A failed **re-**enrolment has already overwritten the old marker, so the
    /// previously enrolled duress PIN is dead and rollback cannot bring it back.
    /// Saying "no changes were kept" there would leave the owner believing a PIN
    /// still wipes their device when it does not.
    #[test]
    fn a_failed_reenrolment_admits_the_old_duress_pin_is_gone() {
        let marker = |name: &str, reused: bool| ArmedMarker {
            root: std::path::PathBuf::from("/nonexistent"),
            cube_id: format!("id-{name}"),
            cube_name: name.to_string(),
            network: Network::Bitcoin,
            file_name: "slot".to_string(),
            reused_slot: reused,
        };

        // Never enrolled: since 6b the reused slot held a decoy, so overwriting
        // it cost nothing. Warning here would invent a lost PIN.
        assert!(prior_pin_deactivated(false, &[marker("Primary", true)]).is_empty());
        // Enrolled, but this Cube's slot was minted by this attempt — its old
        // state is whatever a fresh rollback decoy is.
        assert!(prior_pin_deactivated(true, &[marker("Primary", false)]).is_empty());

        let lost = prior_pin_deactivated(true, &[marker("Primary", true)]);
        assert_eq!(lost, vec!["Primary".to_string()]);

        // Markers all rolled back cleanly — but "no changes were kept" is still
        // false, because the old duress PIN went with them.
        let msg = describe_rollback("Couldn't arm.".to_string(), Vec::new(), lost.clone());
        assert!(
            !msg.contains("No changes were kept"),
            "the old duress PIN is gone, so nothing-was-kept is a lie: {}",
            msg
        );
        assert!(msg.contains("Primary"), "the Cube must be named: {}", msg);
        assert!(
            msg.contains("previous duress PIN no longer works"),
            "{}",
            msg
        );

        // Both failures at once: the live trigger and the dead old PIN.
        let msg = describe_rollback("Couldn't arm.".to_string(), vec!["Other".to_string()], lost);
        assert!(msg.contains("erase this device"), "{}", msg);
        assert!(
            msg.contains("previous duress PIN no longer works"),
            "{}",
            msg
        );
    }

    /// Orphan detection reads the `arming` breadcrumb, not the datadir.
    ///
    /// It used to scan Cubes for a duress marker. Unit 6b makes that
    /// impossible on purpose — every Cube carries a second slot and a marker
    /// is indistinguishable from a decoy — so a surviving scan would report
    /// every device as armed. These cases pin the replacement.
    #[test]
    fn orphan_detection_reads_the_arming_breadcrumb_not_the_marker_files() {
        let root = TempRoot::new("duress-armed");
        let mut armed_cube = cube("cube-b", "Armed", Network::Regtest);
        arm_duress(root.path(), &mut armed_cube, "8765");
        write_settings_dir(
            root.path(),
            "regtest",
            Settings {
                cubes: vec![armed_cube],
                ..Settings::default()
            },
        );

        // A real marker on disk is NOT an orphan on its own — a healthy
        // enrolment looks exactly like this.
        assert!(
            !any_cube_duress_armed(root.path()).expect("no breadcrumb"),
            "a marker on disk must not by itself read as an orphan"
        );

        // Crash between arming and recording the enrolment: breadcrumb set,
        // `enrolled` never written. That is the orphan.
        DuressLocalState {
            arming: true,
            ..DuressLocalState::default()
        }
        .save(root.path())
        .expect("save arming breadcrumb");
        assert!(
            any_cube_duress_armed(root.path()).expect("orphan"),
            "arming-without-enrolled is the crash this must catch"
        );

        // A completed enrolment clears the breadcrumb and is never an orphan.
        DuressLocalState {
            arming: false,
            enrolled: true,
            ..DuressLocalState::default()
        }
        .save(root.path())
        .expect("save enrolled state");
        assert!(
            !any_cube_duress_armed(root.path()).expect("healthy"),
            "a fully-recorded enrolment must never be treated as an orphan"
        );
    }

    #[tokio::test]
    async fn clear_duress_enrollment_clears_cube_hashes_and_local_state() {
        let root = TempRoot::new("duress-clear");
        let mut armed_cube = cube("cube-a", "Armed", Network::Bitcoin);
        arm_duress(root.path(), &mut armed_cube, "8765");
        let network_dir = write_settings_dir(
            root.path(),
            "bitcoin",
            Settings {
                cubes: vec![armed_cube],
                ..Settings::default()
            },
        );
        DuressLocalState {
            enrolled: true,
            active: true,
            account_id: Some("acct-1".to_string()),
            duress_code: Some("ciphertext".to_string()),
            ..DuressLocalState::default()
        }
        .save(root.path())
        .expect("save local state");

        clear_duress_enrollment(CoincubeDirectory::new(root.path().to_path_buf()))
            .await
            .expect("clear duress");

        let settings =
            Settings::from_file(&crate::dir::NetworkDirectory::new(network_dir)).unwrap();
        // The slot is still there — disarming overwrites it with a decoy
        // rather than deleting it, so the Cube's on-disk shape is unchanged.
        // What must be gone is the duress PIN's ability to open it.
        for cube in &settings.cubes {
            assert!(
                cube.has_duress_slot(root.path()),
                "disarming deleted the second slot for Cube '{}' — the Cube now \
                 has one blob where every other Cube has two",
                cube.name
            );
            assert!(
                !crate::services::unlock::marker::verify(
                    root.path(),
                    cube.network,
                    &cube.id,
                    cube.duress_slot_file.as_deref(),
                    "8765",
                    None,
                ),
                "the duress PIN still opens Cube '{}' after a disarm",
                cube.name
            );
        }
        assert_eq!(
            DuressLocalState::load(root.path()).expect("load cleared state"),
            DuressLocalState::default()
        );
    }
}
