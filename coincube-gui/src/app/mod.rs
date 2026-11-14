mod activity_routing;
mod connect_auth;
mod connect_stream;
mod cube_name;
mod recovery_settings;

pub mod breez_liquid;
pub mod breez_spark;
pub mod cache;
pub mod config;
pub mod connect_session;
pub mod error;
pub mod features;
mod imported_kit_metadata;
pub mod local_mcp;
pub mod local_switch;
pub mod menu;
pub mod message;
pub mod node_history;
pub mod seed_source;
pub mod session;
pub mod settings;
pub mod state;
// Admission contract for the later cross-Cube Transfer UI. No payment route uses it yet.
#[allow(dead_code)]
mod transfer_destination;
// Private B-side verification and read-only Vault backend attachment, used by
// the Overview Transfer's "Another Cube…" route (Vault to Vault).
mod transfer_destination_loader;
mod unswept_notice;
pub mod view;
pub mod wallet;
pub mod wallets;

use std::collections::{HashSet, VecDeque};
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

use iced::{clipboard, time, Subscription, Task};
use tokio::runtime::Handle;
use tracing::{error, info, warn};

pub use coincube_core::miniscript::bitcoin;
use coincube_ui::{component::network_banner, widget::Element};
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

enum ToastContent {
    Text(String),
    IncomingLiquidWaiting { amount_sat: u64 },
}

impl ToastContent {
    fn display(&self, unit: settings::unit::BitcoinDisplayUnit, masked: bool) -> String {
        match self {
            Self::Text(message) => message.clone(),
            Self::IncomingLiquidWaiting { amount_sat } => {
                let amount = if masked {
                    "••••••".to_owned()
                } else {
                    use coincube_ui::component::amount::DisplayAmount;
                    bitcoin::Amount::from_sat(*amount_sat).to_formatted_string_with_unit(unit)
                };
                coincube_ui::localized_format!(
                    "Incoming payment of {} — swapping to L-BTC, awaiting confirmation",
                    "Pago entrante de {} — intercambiando a L-BTC, esperando confirmación",
                    amount
                )
            }
        }
    }
}

struct Panels {
    current: Menu,
    activity: state::activity::Activity,
    imported_bitcoin: Option<Box<state::imported_bitcoin::ImportedBitcoinPanel>>,
    // Always available panels
    global_home: GlobalHome,
    liquid_overview: Option<LiquidOverview>,
    liquid_send: Option<LiquidSend>,
    liquid_swap: Option<LiquidSwap>,
    liquid_receive: Option<LiquidReceive>,
    liquid_transactions: Option<LiquidTransactions>,
    liquid_settings: Option<LiquidSettings>,
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
    /// Swap BTC ⇄ USDB (S3).
    spark_swap: state::spark::swap::SparkSwap,
    /// Phase 4b ships real Transactions + Settings panels — they use
    /// `list_payments` / `get_info` which the bridge already exposes, so
    /// they ship ahead of the write-path flows.
    spark_transactions: state::SparkTransactions,
    spark_settings: state::SparkSettings,
    global_settings: GeneralSettingsState,
    // Vault-only panels - None when no vault exists
    vault_overview: Option<VaultOverview>,
    coins: Option<CoinsPanel>,
    split_hub: Option<state::vault::split_hub::SplitHubPanel>,
    transactions: Option<VaultTransactionsPanel>,
    psbts: Option<PsbtsPanel>,
    recovery: Option<CreateSpendPanel>,
    receive: Option<VaultReceivePanel>,
    create_spend: Option<CreateSpendPanel>,
    vault_settings: Option<VaultSettingsState>,
    /// The generation every Connect-session read (the unswept-coins notice,
    /// the Chains column's ancestry checks) checks. Advanced by
    /// [`App::revoke_claim`] when the session those reads run under is
    /// replaced; in-flight reads subscribe to it and cancel.
    session_generation: tokio::sync::watch::Sender<u64>,
    // remaining panels
    buy_sell: Option<crate::app::view::buysell::BuySellPanel>,
    connect: ConnectPanel,
    p2p: Option<crate::app::view::p2p::P2PPanel>,
}

impl Panels {
    /// Hand the Liquid gate to the panels that load Liquid data from a
    /// `reload`, which gets no cache: Activity and the Cube home. Called
    /// wherever the App sets [`Cache::liquid_gate`].
    fn show_liquid(&mut self, gate: crate::app::features::LiquidGate) {
        let shown = gate.show();
        self.activity.set_liquid_shown(shown);
        self.global_home.set_liquid_shown(shown);
    }

    /// Read the cube's fiat currency preference from the settings file.
    fn default_fiat_currency(
        datadir: &CoincubeDirectory,
        network: impl Into<crate::chain::ChainId>,
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
        network: impl Into<crate::chain::ChainId>,
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
        network: impl Into<crate::chain::ChainId>,
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
        breez_client: Option<Arc<BreezClient>>,
        spark_backend: Option<Arc<crate::app::wallets::SparkBackend>>,
        wallet: Option<Arc<Wallet>>,
        datadir: &CoincubeDirectory,
        network: bitcoin::Network,
        chain: crate::chain::ChainId,
        cube_id: String,
        cube_name: String,
        cube_network: String,
        bitcoin_import: Option<Box<settings::BitcoinImport>>,
    ) -> Panels {
        // NO VAULT - All vault panels are None. Liquid panels work whenever
        // the Cube has a Breez client; a Bitcoin Blake2b Cube has none.
        // The UI layer prevents navigation to vault panels when has_vault=false

        let default_fiat_currency = Self::default_fiat_currency(datadir, chain, &cube_id);
        let liquid_backend = breez_client
            .as_ref()
            .map(|client| Arc::new(LiquidBackend::new(client.clone())));
        let swaps_path = Self::swaps_path(datadir, chain, &cube_id);
        let initial_balance_masked = Self::initial_balance_masked(datadir, chain, &cube_id);

        Self {
            current: Menu::Cube(crate::app::menu::CubeSubMenu::Overview),
            activity: state::activity::Activity::new(liquid_backend.clone(), spark_backend.clone()),
            imported_bitcoin: bitcoin_import.map(|import| {
                let mut panel = state::imported_bitcoin::ImportedBitcoinPanel::new(
                    datadir.network_directory(chain),
                    cube_id.clone(),
                    *import,
                );
                panel.set_hardware_root(datadir.clone());
                Box::new(panel)
            }),
            // Liquid panels always available (use LiquidBackend, not Vault wallet)
            global_home: if let Some(w) = &wallet {
                GlobalHome::new(
                    w.clone(),
                    liquid_backend.clone(),
                    spark_backend.clone(),
                    datadir.clone(),
                    chain,
                    cube_id.clone(),
                    initial_balance_masked,
                )
            } else {
                GlobalHome::new_without_wallet(
                    liquid_backend.clone(),
                    spark_backend.clone(),
                    datadir.clone(),
                    chain,
                    cube_id.clone(),
                    initial_balance_masked,
                )
            },
            liquid_overview: liquid_backend
                .clone()
                .map(|backend| LiquidOverview::new(backend, swaps_path.clone())),
            liquid_send: liquid_backend.clone().map(LiquidSend::new),
            liquid_swap: liquid_backend
                .clone()
                .map(|backend| LiquidSwap::new(backend, swaps_path.clone())),
            liquid_receive: liquid_backend.clone().map(LiquidReceive::new),
            liquid_transactions: liquid_backend
                .clone()
                .map(|backend| LiquidTransactions::new(backend, swaps_path.clone())),
            liquid_settings: liquid_backend.clone().map(LiquidSettings::new),
            spark_overview: state::SparkOverview::new(spark_backend.clone()),
            spark_send: state::SparkSend::new(spark_backend.clone()),
            spark_receive: state::SparkReceive::new(spark_backend.clone()),
            spark_swap: state::spark::swap::SparkSwap::new(spark_backend.clone()),
            spark_transactions: state::SparkTransactions::new(spark_backend.clone()),
            spark_settings: state::SparkSettings::new(spark_backend.clone()),
            global_settings: GeneralSettingsState::from_directory(
                cube_id.clone(),
                &datadir.network_directory(chain),
            ),
            // All vault panels are None - no vault exists
            vault_overview: None,
            coins: None,
            split_hub: None,
            transactions: None,
            psbts: None,
            recovery: None,
            receive: None,
            create_spend: None,
            vault_settings: None,
            session_generation: tokio::sync::watch::channel(1).0,
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
                .as_ref()
                .and_then(|client| client.liquid_signer())
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
        breez_client: Option<Arc<BreezClient>>,
        spark_backend: Option<Arc<crate::app::wallets::SparkBackend>>,
        cache: &Cache,
        wallet: Arc<Wallet>,
        data_dir: CoincubeDirectory,
        daemon_backend: DaemonBackend,
        internal_bitcoind: Option<&Bitcoind>,
        config: Arc<Config>,
        cube_id: String,
        cube_name: String,
        cube_network: String,
    ) -> Panels {
        let needs_rescan_date =
            needs_rescan_date(&data_dir, cache.chain(), &wallet, &daemon_backend);

        let default_fiat_currency = Self::default_fiat_currency(&data_dir, cache.chain(), &cube_id);
        let liquid_backend = breez_client
            .as_ref()
            .map(|client| Arc::new(LiquidBackend::new(client.clone())));
        let swaps_path = Self::swaps_path(&data_dir, cache.chain(), &cube_id);
        let initial_balance_masked =
            Self::initial_balance_masked(&data_dir, cache.chain(), &cube_id);

        Self {
            current: Menu::Cube(crate::app::menu::CubeSubMenu::Overview),
            activity: state::activity::Activity::new(liquid_backend.clone(), spark_backend.clone()),
            imported_bitcoin: None,
            global_home: GlobalHome::new(
                wallet.clone(),
                liquid_backend.clone(),
                spark_backend.clone(),
                data_dir.clone(),
                cache.chain(),
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
                    cache.history_sync(),
                ),
                cache.blockheight(),
                needs_rescan_date,
            )),
            liquid_overview: liquid_backend
                .as_ref()
                .map(|backend| LiquidOverview::new(backend.clone(), swaps_path.clone())),
            liquid_send: liquid_backend
                .as_ref()
                .map(|backend| LiquidSend::new(backend.clone())),
            liquid_swap: liquid_backend
                .as_ref()
                .map(|backend| LiquidSwap::new(backend.clone(), swaps_path.clone())),
            liquid_receive: liquid_backend
                .as_ref()
                .map(|backend| LiquidReceive::new(backend.clone())),
            liquid_transactions: liquid_backend
                .as_ref()
                .map(|backend| LiquidTransactions::new(backend.clone(), swaps_path.clone())),
            liquid_settings: liquid_backend
                .as_ref()
                .map(|backend| LiquidSettings::new(backend.clone())),
            spark_overview: state::SparkOverview::new(spark_backend.clone()),
            spark_send: state::SparkSend::new(spark_backend.clone()),
            spark_receive: state::SparkReceive::new(spark_backend.clone()),
            spark_swap: state::spark::swap::SparkSwap::new(spark_backend.clone()),
            spark_transactions: state::SparkTransactions::new(spark_backend.clone()),
            spark_settings: state::SparkSettings::new(spark_backend.clone()),
            global_settings: GeneralSettingsState::from_directory(
                cube_id.clone(),
                &data_dir.network_directory(cache.fiat_chain),
            ),
            coins: Some(
                CoinsPanel::new(cache.coins(), wallet.recovery_timelock(), wallet.chain)
                    .with_wallet(wallet.clone()),
            ),
            split_hub: Some(state::vault::split_hub::SplitHubPanel::new(wallet.clone())),
            transactions: Some(VaultTransactionsPanel::new(wallet.clone())),
            psbts: wallet
                .supports_inheritance()
                .then(|| PsbtsPanel::new(wallet.clone())),
            recovery: wallet.supports_inheritance().then(|| {
                new_recovery_panel(
                    wallet.clone(),
                    cache,
                    sync_status(
                        daemon_backend.clone(),
                        cache.blockheight(),
                        cache.sync_progress(),
                        cache.last_poll_timestamp(),
                        cache.last_poll_at_startup,
                        cache.history_sync(),
                    ),
                )
            }),
            receive: Some(VaultReceivePanel::new(data_dir.clone(), wallet.clone())),
            create_spend: wallet.supports_inheritance().then(|| {
                let (balance, unconfirmed_balance, _, _) = state::coins_summary(
                    cache.coins(),
                    cache.blockheight().max(0) as u32,
                    wallet.recovery_timelock(),
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
                        cache.history_sync(),
                    ),
                    cache.bitcoin_unit,
                )
            }),
            vault_settings: wallet.supports_inheritance().then(|| {
                VaultSettingsState::new(
                    data_dir.clone(),
                    wallet.clone(),
                    internal_bitcoind.is_some(),
                    config.clone(),
                )
                .with_managed_owner(
                    internal_bitcoind
                        .and_then(|node| node.owner_lock_path().map(std::path::PathBuf::from)),
                )
            }),
            session_generation: tokio::sync::watch::channel(1).0,
            connect: ConnectPanel::new(
                spark_backend.as_ref().map(|b| b.client().clone()),
                cube_id.clone(),
                cube_name,
                cube_network,
                // Connect's legacy hasVault contract requires an inheritance policy.
                wallet.supports_inheritance(),
            ),
            buy_sell: breez_client.as_ref().map(|client| {
                crate::app::view::buysell::BuySellPanel::new(
                    cache.network,
                    wallet.clone(),
                    client.clone(),
                )
            }),
            p2p: match breez_client
                .as_ref()
                .and_then(|client| client.liquid_signer())
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
        breez_client: Option<Arc<BreezClient>>,
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
                cache.history_sync(),
            ),
            cache.blockheight(),
            needs_rescan_date(&data_dir, cache.chain(), &wallet, &daemon_backend),
        ));
        self.coins = Some(
            CoinsPanel::new(cache.coins(), wallet.recovery_timelock(), wallet.chain)
                .with_wallet(wallet.clone()),
        );
        self.split_hub = Some(state::vault::split_hub::SplitHubPanel::new(wallet.clone()));
        self.transactions = Some(VaultTransactionsPanel::new(wallet.clone()));
        self.psbts = wallet
            .supports_inheritance()
            .then(|| PsbtsPanel::new(wallet.clone()));
        self.recovery = wallet.supports_inheritance().then(|| {
            new_recovery_panel(
                wallet.clone(),
                cache,
                sync_status(
                    daemon_backend.clone(),
                    cache.blockheight(),
                    cache.sync_progress(),
                    cache.last_poll_timestamp(),
                    cache.last_poll_at_startup,
                    cache.history_sync(),
                ),
            )
        });
        self.receive = Some(VaultReceivePanel::new(data_dir.clone(), wallet.clone()));
        self.create_spend = wallet.supports_inheritance().then(|| {
            let (balance, unconfirmed_balance, _, _) = state::coins_summary(
                cache.coins(),
                cache.blockheight() as u32,
                wallet.recovery_timelock(),
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
                    cache.history_sync(),
                ),
                cache.bitcoin_unit,
            )
        });
        self.vault_settings = wallet.supports_inheritance().then(|| {
            VaultSettingsState::new(
                data_dir.clone(),
                wallet.clone(),
                internal_bitcoind.is_some(),
                config.clone(),
            )
            .with_managed_owner(
                internal_bitcoind
                    .and_then(|node| node.owner_lock_path().map(std::path::PathBuf::from)),
            )
        });

        self.buy_sell = breez_client.map(|client| {
            crate::app::view::buysell::BuySellPanel::new(cache.network, wallet, client)
        });
    }

    fn current(&self) -> Option<&dyn State> {
        if let Some(panel) = self
            .imported_bitcoin
            .as_ref()
            .filter(|panel| panel.handles_route(&self.current))
        {
            return Some(panel.as_ref());
        }
        match &self.current {
            Menu::Cube(crate::app::menu::CubeSubMenu::Overview) => Some(&self.global_home),
            Menu::Cube(crate::app::menu::CubeSubMenu::Activity) => Some(&self.activity),
            Menu::Cube(crate::app::menu::CubeSubMenu::Settings(_)) => {
                Some(&self.global_settings as &dyn State)
            }
            Menu::Liquid(submenu) => match submenu {
                crate::app::menu::LiquidSubMenu::Overview => self
                    .liquid_overview
                    .as_ref()
                    .map(|panel| panel as &dyn State),
                crate::app::menu::LiquidSubMenu::Send => {
                    self.liquid_send.as_ref().map(|panel| panel as &dyn State)
                }
                crate::app::menu::LiquidSubMenu::Swap => {
                    self.liquid_swap.as_ref().map(|panel| panel as &dyn State)
                }
                crate::app::menu::LiquidSubMenu::Receive => self
                    .liquid_receive
                    .as_ref()
                    .map(|panel| panel as &dyn State),
                crate::app::menu::LiquidSubMenu::Transactions(_) => self
                    .liquid_transactions
                    .as_ref()
                    .map(|panel| panel as &dyn State),
                crate::app::menu::LiquidSubMenu::Settings(_) => self
                    .liquid_settings
                    .as_ref()
                    .map(|panel| panel as &dyn State),
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
                crate::app::menu::SparkSubMenu::Swap => Some(&self.spark_swap as &dyn State),
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
                crate::app::menu::VaultSubMenu::SplitCoins => {
                    self.split_hub.as_ref().map(|v| v as &dyn State)
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
        if self
            .imported_bitcoin
            .as_ref()
            .is_some_and(|panel| panel.handles_route(&self.current))
        {
            return self
                .imported_bitcoin
                .as_mut()
                .map(|panel| panel.as_mut() as &mut dyn State);
        }
        match &self.current {
            Menu::Cube(crate::app::menu::CubeSubMenu::Overview) => Some(&mut self.global_home),
            Menu::Cube(crate::app::menu::CubeSubMenu::Activity) => Some(&mut self.activity),
            Menu::Cube(crate::app::menu::CubeSubMenu::Settings(_)) => {
                Some(&mut self.global_settings as &mut dyn State)
            }
            Menu::Liquid(submenu) => match submenu {
                crate::app::menu::LiquidSubMenu::Overview => self
                    .liquid_overview
                    .as_mut()
                    .map(|panel| panel as &mut dyn State),
                crate::app::menu::LiquidSubMenu::Send => self
                    .liquid_send
                    .as_mut()
                    .map(|panel| panel as &mut dyn State),
                crate::app::menu::LiquidSubMenu::Swap => self
                    .liquid_swap
                    .as_mut()
                    .map(|panel| panel as &mut dyn State),
                crate::app::menu::LiquidSubMenu::Receive => self
                    .liquid_receive
                    .as_mut()
                    .map(|panel| panel as &mut dyn State),
                crate::app::menu::LiquidSubMenu::Transactions(_) => self
                    .liquid_transactions
                    .as_mut()
                    .map(|panel| panel as &mut dyn State),
                crate::app::menu::LiquidSubMenu::Settings(_) => self
                    .liquid_settings
                    .as_mut()
                    .map(|panel| panel as &mut dyn State),
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
                crate::app::menu::SparkSubMenu::Swap => {
                    Some(&mut self.spark_swap as &mut dyn State)
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
                crate::app::menu::VaultSubMenu::SplitCoins => {
                    self.split_hub.as_mut().map(|v| v as &mut dyn State)
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
/// How long a toast stays once it is on screen, unless dismissed sooner.
const TOAST_LIFETIME: Duration = Duration::from_secs(8);
/// Most toasts kept waiting out of view behind the shown ones; older ones in
/// a burst are dropped unseen.
const TOAST_MAX_WAITING: usize = 3;

pub struct App {
    // Pairing records grow independently of the tab's by-value state. Keep them
    // on the heap so debug GUI transitions fit the Windows test-thread stack.
    local_mcp: Box<local_mcp::Settings>,
    cache: Cache,
    wallet: Option<Arc<Wallet>>,
    imported_kit_metadata: imported_kit_metadata::ImportedKitMetadata,
    breez_client: Option<Arc<BreezClient>>,
    fork_connect_client: Option<crate::services::coincube::CoincubeClient>,
    local_feature_valid: bool,
    local_feature_checked_at: Option<std::time::Instant>,
    /// The fork feature check in flight, if any. Its answer is applied only
    /// while this matches: a replaced App's check keeps running, and its
    /// answer reaches whichever App now holds the tab.
    local_feature_check: Option<u64>,
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
    /// Private destination capability for a future cross-Cube Transfer. The
    /// source App remains the sole public session and owns its teardown.
    transfer_destination: Option<Box<transfer_destination::TransferDestinationOwner>>,
    /// Boxed so that `App` — and therefore `gui::tab::State`, whose size is
    /// set by this variant — stays small. `Panels` holds every panel's state
    /// inline (~30 KiB); carried by value it made each `self.state = ...`
    /// transition in `Tab::update_inner` materialise a ~32 KiB temporary, and
    /// unoptimised builds give every temporary its own stack slot, so the
    /// function's frame alone approached the 2 MiB test-thread stack.
    panels: Box<Panels>,
    /// Toasts, oldest first: id, expiry deadline, level, content. The
    /// deadline is set only while the toast is on screen (see
    /// `arm_toast_timers`).
    errors: Vec<(usize, Option<std::time::Instant>, log::Level, ToastContent)>,
    current_error_id: usize,
    /// True while a check_bitcoind_sync_progress probe is in flight; prevents
    /// multiple concurrent RPC calls from piling up across subscription ticks.
    bitcoind_sync_probe_in_progress: bool,
    /// Guards the active-node net-stats poll (connections/upload/onion) so ticks
    /// don't stack concurrent RPCs.
    node_net_stats_probe_in_progress: bool,
    node_history_in_progress: bool,
    node_history_pending: Option<node_history::Action>,
    node_history_epoch: u64,
    node_history_request_epoch: u64,
    /// Set at the global Connect auth boundary (`Tab::invalidate_fork_session`
    /// → [`Self::on_global_auth_change`]): the session this tab's Connect
    /// panel still shows is not one a claim may work under. Lifted only by
    /// a completion (`SetSession` or `SessionLoaded`) of an authentication
    /// operation this tab's own Connect panel began *after* the hold — the
    /// completion carries the panel's authentication epoch from when the
    /// operation was spawned, compared with `claim_hold_epoch`. A cached
    /// account callback, re-entering the panel, a Refresh, a sibling tab's
    /// sign-in, or a completion of an operation begun before the hold cannot
    /// lift it. Claim-scoped: the Connect panel itself is not synchronised
    /// across tabs here.
    claim_session_invalidated: bool,
    /// The account panel's authentication epoch as advanced by the most
    /// recent hold; a completion lifts the hold only if its epoch is at
    /// least this.
    claim_hold_epoch: u64,
    /// True while an off-thread daemon backend switch ([`Self::spawn_daemon_switch`])
    /// is in flight. The config isn't updated until the switch completes, so
    /// without this guard the next sync probe would keep re-firing the switch
    /// every tick — spawning concurrent daemon starts that race to load the same
    /// watchonly wallet and corrupt it. Cleared by `Message::DaemonRestarted`.
    daemon_switch_in_progress: bool,
    /// Token of the background managed-node install this App started last.
    /// Results and progress carrying another token belong to an earlier
    /// attempt — possibly from an App this one replaced — and must not
    /// change the status shown in Settings → Node.
    background_node_attempt: Option<u64>,
    /// Whether a completed full scan has retired this Vault's rescan obligation
    /// this session: set only once the settings write succeeded. See
    /// [`full_scan_settles_rescan_obligation`].
    rescan_obligation_retired_by_scan: bool,
    /// A settings write retiring the obligation is under way, so a refresh does
    /// not start a second one. Cleared when it reports back; a failed write
    /// leaves the obligation to be retried on a later refresh.
    rescan_obligation_retire_in_flight: bool,
    /// Set when an auto-promotion to the pending local node fails and the
    /// previous daemon is recovered. The recovered daemon still carries
    /// `auto_switch_to_pending = true` + `pending_bitcoind`, so without this the
    /// periodic sync probe would re-fire the same failing switch every poll,
    /// churning the daemon. Suppresses auto-switch until the next *successful*
    /// switch (a fresh adopt / manual switch re-arms it by clearing this).
    auto_switch_suppressed: bool,
    /// Why the last probe of a synced pending node did not promote it, if it
    /// did not. Kept only so each reason is logged once rather than every probe.
    local_switch_hold: Option<local_switch::SwitchHold>,
    /// Deposit txids a sync-driven entangled lookup batch has claimed and not
    /// yet answered (`#276` I13). Single-flight: two syncs inside one
    /// in-flight window would otherwise queue the same txids twice. Released
    /// as a whole set when the batch's `EntangledLookups` lands — including
    /// txids that answered `Unknown`, which are not written to the cache and
    /// would otherwise stay claimed forever.
    entangled_in_flight: HashSet<bitcoin::Txid>,
    /// Coins a Chains-column ancestry check batch has claimed and not yet
    /// answered. Released as a whole set when its `ChainProofs` lands.
    chain_proofs_in_flight: HashSet<bitcoin::OutPoint>,
    /// The last split refresh pass issued, and the last one applied.
    split_refresh_issued: u64,
    split_refresh_applied: u64,
    /// The last fresh Connect check of each twin split.
    twin_checks: std::collections::HashMap<bitcoin::Txid, crate::services::split_coins::TwinCheck>,
    /// The twin Cube's split journal as the last applied pass read it, so a
    /// late pass's twin checks can still be applied to it.
    twin_records: Vec<crate::services::split_coins::SplitRecord>,
    /// Whether this Vault's saved ancestry results have been read into the
    /// cache yet ([`crate::services::chain_status::store::load`]).
    chain_proofs_loaded: bool,
    unswept_in_flight: Option<u64>,
    unswept_session: Option<crate::app::connect_session::ConnectSession>,
    /// The Connect session the last account update was accepted under. A
    /// later update under another one revokes every Connect-session read.
    seen_session: Option<crate::app::connect_session::ConnectSession>,
    /// Global "payment received" celebration overlay — shown for incoming
    /// Liquid payments (e.g. LNURL) regardless of which panel is active.
    show_received_celebration: bool,
    /// One-time "turn on recovery alerts?" consent prompt overlay
    /// (PLAN-recovery-alerts-cleanup PR 3). Set once per session when a Vault
    /// with keyholders has no monitoring and the prompt hasn't been answered for
    /// this Cube; cleared on accept/decline. The durable "answered" record lives
    /// in `CubeSettings::recovery_alerts_prompt_answered`.
    show_recovery_alerts_prompt: bool,
    /// Set once the session's Stable Balance reconcile has been kicked off
    /// (on the first Spark `Synced`), so later sync ticks don't repeat it.
    /// See [`reconcile_stable_balance`] for what the reconcile decides.
    spark_stable_balance_reconciled: bool,
    /// The Spark nickname being written now (`Some(None)`: back to plain
    /// "Spark"). One write runs at a time, so a later request can't land
    /// before an earlier one.
    spark_nickname_in_flight: Option<Option<String>>,
    /// The latest nickname requested while a write was running, written once
    /// that write lands.
    spark_nickname_queued: Option<Option<String>>,
    received_celebration_amount: String,
    received_celebration_context: String,
    /// The receipt's "Into …" line for the payment shown, untranslated.
    received_celebration_wallet: &'static str,
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
    /// Shared `Arc<RwLock<AccessTokenResponse>>` for the Connect session,
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
    /// Automatic renewal of this computer's Connect registration.
    connect_renewal: Renewal,
    /// Part of the stream subscription's identity: bumped to restart it
    /// without an id change.
    connect_stream_restart: u64,
    /// Cache state before this App panel sent its one pending OTP verification.
    in_app_otp_cache_snapshot: Option<crate::services::connect::client::cache::ConnectCache>,
    /// Network cache observed before a keyring restore starts its refresh RPC.
    in_app_restore_cache_snapshot: Option<RestoreAuthSnapshot>,
    /// Keep stream bootstrap from replacing the shared Arc during a refresh CAS.
    in_app_refresh_commit: Option<(
        u64,
        Option<
            Arc<tokio::sync::RwLock<crate::services::connect::client::auth::AccessTokenResponse>>,
        >,
    )>,
    deferred_connect_stream_triggers: Vec<Message>,
}

struct RestoreAuthSnapshot {
    cache: crate::services::connect::client::cache::ConnectCache,
    email: Option<String>,
    auth: Option<(
        Arc<tokio::sync::RwLock<crate::services::connect::client::auth::AccessTokenResponse>>,
        crate::services::connect::client::auth::AccessTokenResponse,
    )>,
}

/// Health of this computer's Connect live updates, as the header control,
/// its panel and Cube › Settings › Connections show it.
///
/// Transitions are driven by `ConnectStreamMessage` events arriving on the
/// gRPC subscription and by the App's automatic renewal. `Inactive` is
/// distinct from the others because a Cube that doesn't use Connect isn't
/// "broken": nothing is shown for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConnectionStatus {
    /// No stream has been bootstrapped yet (no `Message::ConnectStreamReady`).
    #[default]
    Inactive,
    /// The stream is starting and hasn't connected since sign-in or launch.
    Connecting,
    /// Live updates are on.
    Connected,
    /// No usable network. Tenshu reconnects on its own.
    Offline,
    /// The network works but Connect doesn't answer, or the stream dropped.
    /// Tenshu keeps trying.
    Reconnecting,
    /// Connect stopped recognizing this computer's id and Tenshu is renewing
    /// the registration on its own.
    Renewing,
    /// Renewal failed twice, or a server without reasons refused the id. The
    /// user presses Try again.
    RenewFailed,
}

/// What everything Tenshu sends to Connect waits for while live updates
/// aren't on.
pub const CONNECT_WAITS_FOR: &str = "Keychain signing, Recovery Kits and Cube backups to Connect";

/// The status light on the Connect control's account glyph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectLight {
    /// Steady green: signed in, live updates on.
    On,
    /// Pulsing green: connecting for the first time.
    Connecting,
    /// Pulsing amber: something is wrong and Tenshu is fixing it.
    Reconnecting,
    /// Grey ring: no internet connection.
    Offline,
    /// Red: the user has something to do.
    NeedsYou,
}

impl ConnectionStatus {
    /// True for any state the header should surface.
    pub fn is_visible(&self) -> bool {
        !matches!(self, Self::Inactive)
    }

    /// The word after "Live updates on this computer".
    pub fn word(&self) -> &'static str {
        match self {
            Self::Inactive | Self::Connecting => "Connecting…",
            Self::Connected => "On",
            Self::Offline | Self::RenewFailed => "Offline",
            Self::Reconnecting | Self::Renewing => "Reconnecting…",
        }
    }

    /// The one line under the status word. `RenewFailed` says its piece in a
    /// notice instead ([`RENEW_FAILED_TITLE`]).
    pub fn line(&self) -> Option<String> {
        match self {
            Self::Inactive | Self::Connecting => {
                Some(coincube_ui::locale::copy("Starting live updates from Connect.").to_owned())
            }
            Self::Connected => Some(
                coincube_ui::locale::copy(
                    "Keychain signing requests and signatures arrive as they happen.",
                )
                .to_owned(),
            ),
            Self::Offline => Some(coincube_ui::localized_format!(
                "No internet connection. Tenshu reconnects when you’re back online. \
                 {CONNECT_WAITS_FOR} wait until then.",
                "Sin conexión a internet. Tenshu se vuelve a conectar cuando vuelvas a estar \
                 en línea. La firma con Keychain, los Kits de recuperación y las copias de \
                 Cubes en Connect esperan hasta entonces."
            )),
            Self::Reconnecting => Some(coincube_ui::localized_format!(
                "Tenshu can’t reach Connect right now and keeps trying. \
                 {CONNECT_WAITS_FOR} wait until it’s back.",
                "Tenshu no puede conectarse con Connect en este momento y lo sigue intentando. \
                 La firma con Keychain, los Kits de recuperación y las copias de Cubes en \
                 Connect esperan hasta que vuelva."
            )),
            Self::Renewing => Some(
                coincube_ui::locale::copy(
                    "Renewing this computer’s link to Connect. Nothing to do.",
                )
                .to_owned(),
            ),
            Self::RenewFailed => None,
        }
    }

    /// The light on the account glyph, `None` while there is no stream.
    pub fn light(&self) -> Option<ConnectLight> {
        match self {
            Self::Inactive => None,
            Self::Connecting => Some(ConnectLight::Connecting),
            Self::Connected => Some(ConnectLight::On),
            Self::Offline => Some(ConnectLight::Offline),
            Self::Reconnecting | Self::Renewing => Some(ConnectLight::Reconnecting),
            Self::RenewFailed => Some(ConnectLight::NeedsYou),
        }
    }

    /// Whether uploads to Connect (Recovery Kits, Cube backups) may run.
    pub fn allows_uploads(&self) -> bool {
        matches!(self, Self::Connected)
    }
}

/// The notice shown when automatic renewal didn't work.
pub const RENEW_FAILED_TITLE: &str = "Couldn’t reconnect this computer";

pub fn renew_failed_detail() -> String {
    coincube_ui::localized_format!(
        "Connect stopped recognizing this computer, and renewing its link automatically \
         didn’t work. {CONNECT_WAITS_FOR} wait until it does.",
        "Connect dejó de reconocer esta computadora y no se pudo renovar su vínculo \
         automáticamente. La firma con Keychain, los Kits de recuperación y las copias de \
         Cubes en Connect esperan hasta que se renueve."
    )
}

/// Why Connect, not the user, signed this computer out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignedOutBy {
    /// The computer was removed from the Connect account.
    ComputerRemoved,
    /// The session ended and couldn't be refreshed.
    SessionEnded,
}

impl SignedOutBy {
    pub fn title(&self) -> &'static str {
        coincube_ui::locale::copy(match self {
            Self::ComputerRemoved => "This computer was removed from your Connect account",
            Self::SessionEnded => "Your Connect session on this computer ended",
        })
    }

    pub fn detail(&self) -> String {
        coincube_ui::localized_format!(
            "Sign in to use {CONNECT_WAITS_FOR} again.",
            "Inicia sesión para volver a usar la firma con Keychain, los Kits de recuperación \
             y las copias de Cubes en Connect."
        )
    }
}

/// Why a Vault that stopped updating isn't, in the Vault's own words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultStall {
    /// No internet connection (the Vault reads through Connect).
    Offline,
    /// Connect can't be reached right now.
    Reconnecting,
    /// Signed out of Connect, or any other failure.
    NotUpdating,
    /// Connect stopped recognizing this computer and automatic renewal
    /// failed: retries have stopped until the user presses Try again.
    NeedsReconnect,
}

/// A Vault whose recent polls failed (C8). Set by the Vault's own sync
/// result, whatever the provider; the Connect state only picks the words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultAlert {
    pub stall: VaultStall,
    /// The Vault reads its blockchain data through COINCUBE | Connect.
    pub via_connect: bool,
    /// Signed out of Connect while the Vault depends on it: the notice
    /// offers Sign in.
    pub needs_sign_in: bool,
    /// Local time ("HH:MM") of the last successful sync, when known.
    pub last_synced: Option<String>,
    /// The block the Vault last synced to.
    pub block: i32,
}

impl VaultAlert {
    /// The Vault's status word in place of Synced.
    pub fn word(&self) -> &'static str {
        coincube_ui::locale::copy(match self.stall {
            VaultStall::Offline => "Offline",
            VaultStall::Reconnecting => "Reconnecting…",
            VaultStall::NotUpdating | VaultStall::NeedsReconnect => "Not updating",
        })
    }

    /// The notice's title on the Vault page.
    pub fn title(&self) -> &'static str {
        coincube_ui::locale::copy(match self.stall {
            VaultStall::Offline => "No internet connection",
            VaultStall::Reconnecting => "Can’t reach COINCUBE | Connect",
            VaultStall::NotUpdating if self.needs_sign_in => {
                "Sign in to Connect to update this Vault"
            }
            VaultStall::NotUpdating => "This Vault isn’t updating",
            VaultStall::NeedsReconnect => crate::app::RENEW_FAILED_TITLE,
        })
    }

    fn figures_age(&self) -> String {
        match &self.last_synced {
            Some(at) => coincube_ui::localized_format!(
                "Balance and activity are from {at}.",
                "El saldo y la actividad son de las {at}."
            ),
            None => {
                coincube_ui::locale::copy("Balance and activity may be out of date.").to_owned()
            }
        }
    }

    /// The notice's line on the Vault page.
    pub fn detail(&self) -> String {
        let from = self.figures_age();
        match self.stall {
            VaultStall::Offline => coincube_ui::localized_format!(
                "{from} They update when you’re back online.",
                "{from} Se actualizan cuando vuelvas a estar en línea."
            ),
            VaultStall::Reconnecting => coincube_ui::localized_format!(
                "{from} Tenshu keeps trying and updates them when Connect is back.",
                "{from} Tenshu lo sigue intentando y los actualiza cuando Connect vuelva."
            ),
            VaultStall::NotUpdating if self.needs_sign_in => coincube_ui::localized_format!(
                "{from} This Vault gets blockchain data through Connect, which needs you signed in.",
                "{from} Este Vault obtiene los datos de la cadena de bloques a través de Connect, \
                 que requiere que hayas iniciado sesión."
            ),
            VaultStall::NotUpdating => coincube_ui::localized_format!(
                "{from} Tenshu keeps trying.",
                "{from} Tenshu lo sigue intentando."
            ),
            VaultStall::NeedsReconnect => coincube_ui::localized_format!(
                "{from} Press Try again in the Connect control to update this Vault.",
                "{from} Pulsa Reintentar en el control de Connect para actualizar este Vault."
            ),
        }
    }

    /// The line under "Vault · Not updating" in the Connect panel.
    pub fn panel_line(&self) -> String {
        let shown = match &self.last_synced {
            Some(at) => coincube_ui::localized_format!(
                " The balance shown is from {at}.",
                " El saldo que se muestra es de las {at}."
            ),
            None => String::new(),
        };
        if self.needs_sign_in {
            coincube_ui::localized_format!(
                "It gets blockchain data through COINCUBE | Connect, which needs you signed in.{shown}",
                "Obtiene los datos de la cadena de bloques a través de COINCUBE | Connect, que \
                 requiere que hayas iniciado sesión.{shown}"
            )
        } else {
            coincube_ui::localized_format!(
                "It gets blockchain data through COINCUBE | Connect.{shown}",
                "Obtiene los datos de la cadena de bloques a través de COINCUBE | Connect.{shown}"
            )
        }
    }
}

/// The Vault's alert, if its recent polls failed.
pub fn vault_alert(
    sync: &SyncStatus,
    via_connect: bool,
    connect: ConnectionStatus,
    signed_in: bool,
    last_poll: Option<u32>,
    block: i32,
) -> Option<VaultAlert> {
    if !matches!(sync, SyncStatus::SyncFailing { .. }) {
        return None;
    }
    let needs_sign_in = via_connect && !signed_in;
    let stall = if !via_connect || needs_sign_in {
        VaultStall::NotUpdating
    } else {
        match connect {
            ConnectionStatus::Offline => VaultStall::Offline,
            ConnectionStatus::Connecting
            | ConnectionStatus::Reconnecting
            | ConnectionStatus::Renewing => VaultStall::Reconnecting,
            ConnectionStatus::RenewFailed => VaultStall::NeedsReconnect,
            _ => VaultStall::NotUpdating,
        }
    };
    let last_synced = last_poll
        .and_then(|t| chrono::DateTime::from_timestamp(t as i64, 0))
        .map(|t| t.with_timezone(&chrono::Local).format("%H:%M").to_string());
    Some(VaultAlert {
        stall,
        via_connect,
        needs_sign_in,
        last_synced,
        block,
    })
}

/// Automatic renewal of this computer's Connect registration (C1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Renewal {
    /// Attempts made in the current cycle.
    pub attempts: u8,
    /// A `RegisterDevice` request is in flight.
    pub in_flight: bool,
}

/// Automatic attempts before showing Couldn't reconnect.
pub const RENEWAL_ATTEMPTS: u8 = 2;
/// The wait between automatic attempts.
pub const RENEWAL_RETRY: std::time::Duration = std::time::Duration::from_secs(30);
/// Half a pulse of the Connect light: bright, then dim.
const CONNECT_LIGHT_PULSE: std::time::Duration = std::time::Duration::from_millis(800);

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

/// One chain directory under the datadir, with the Cubes it holds — after the
/// whole set has passed [`duress_cube_inventory`]'s identity and runtime
/// checks.
#[derive(Debug)]
pub(crate) struct DuressCubeDir {
    pub dir: crate::dir::NetworkDirectory,
    /// The chain the directory belongs to (from its name). Every record in
    /// `settings` agrees with it, and this build can run it.
    pub chain: crate::chain::ChainId,
    pub settings: crate::app::settings::Settings,
}

/// User-facing message when a duress operation is refused because this device
/// holds a Cube on a chain this build only knows the identity of. Arming or
/// disarming would write into that Cube's seed folder — storage this version
/// must leave untouched — so the whole operation is refused before any Cube is
/// changed.
pub(crate) const DURESS_DORMANT_CHAIN_MSG: &str =
    "Duress mode can't be changed on this device while it holds a Bitcoin Blake2b Cube, \
     which this version of Tenshu can't open. Nothing was changed. Update Tenshu to a \
     version with Bitcoin Blake2b support, or move that Cube's folder off this device.";

/// Enumerate and validate every Cube a duress operation would touch, **before
/// anything is written** — the whole-operation preflight for
/// [`persist_duress_enrollment`], [`clear_duress_enrollment`] and the wizard's
/// [`duress_pin_collision_check_blocking`].
///
/// Duress arms and disarms *every* Cube under the datadir, and each of those
/// writes goes into the Cube's chain-keyed seed folder. Two things must hold
/// for every record first:
///
/// 1. **Directory identity.** A chain directory is named after its chain
///    (`ChainId::dir_name`), and each record in its `settings.json` must name
///    that same chain. A directory whose name is no chain this build knows, or
///    a record claiming another chain than the folder it sits in (a copied or
///    hand-edited file), is refused: its seed folder, marker path and identity
///    would otherwise disagree with each other.
/// 2. **Runtime support.** Both Bitcoin Blake2b identities are dormant in this
///    build ([`crate::chain::RuntimeSupport`]): their settings are kept intact
///    and nothing is written under them. A single such Cube refuses the whole
///    operation — a device half-armed or half-disarmed is worse than one that
///    says why it can't change.
///
/// Errors are returned before the caller has mutated anything, so a refused
/// operation leaves every marker, every settings file and the local duress
/// state exactly as they were.
pub(crate) fn duress_cube_inventory(root: &std::path::Path) -> Result<Vec<DuressCubeDir>, String> {
    use crate::chain::{ChainId, ChainIdExt, RuntimeSupport};
    let mut out = Vec::new();
    for dir in duress_enroll_network_dirs(root)? {
        let name = dir
            .path()
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let Some(chain) = ChainId::from_dir_name(&name) else {
            return Err(format!(
                "The '{name}' folder in your data directory holds Cube settings but isn't a \
                 chain this version of Tenshu knows, so duress mode can't be changed safely. \
                 Nothing was changed."
            ));
        };
        let settings = crate::app::settings::Settings::from_file(&dir)
            .map_err(|e| format!("Couldn't read your Cube settings to change duress mode: {e}"))?;
        for cube in &settings.cubes {
            if cube.network != chain {
                return Err(format!(
                    "Cube '{}' says it lives on {}, but its settings were found in the {} \
                     folder. Duress mode can't be changed until the two agree. Nothing was \
                     changed.",
                    cube.name,
                    cube.network.label(),
                    chain.label(),
                ));
            }
            if let RuntimeSupport::Dormant { .. } = cube.network.runtime_support() {
                return Err(DURESS_DORMANT_CHAIN_MSG.to_string());
            }
        }
        out.push(DuressCubeDir {
            dir,
            chain,
            settings,
        });
    }
    Ok(out)
}

/// The import capability guard is separate from inventory: disabling existing
/// Duress Mode must remain possible for an interrupted pending import.
fn ensure_import_duress_compatibility(inventory: &[DuressCubeDir]) -> Result<(), String> {
    if inventory
        .iter()
        .flat_map(|entry| &entry.settings.cubes)
        .any(|cube| !cube.supports_duress())
    {
        return Err("Duress Mode cannot be enabled while this device holds a pending or imported Bitcoin wallet. Imported wallet recovery is not supported by Duress Mode yet.".into());
    }
    Ok(())
}

pub(crate) fn check_import_duress_compatibility() -> Result<(), String> {
    let directory = CoincubeDirectory::active().map_err(|e| e.to_string())?;
    ensure_import_duress_compatibility(&duress_cube_inventory(directory.path())?)
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
/// or two once, to be certain, is the right trade. Each trial now shares that
/// Cube's unlock delay; enrollment cannot become an alternate PIN oracle.
///
/// Run this BEFORE any server enrollment: Connect tiers enroll server-side
/// first, so checking the (deterministic, user-entered) collision only in the
/// later local persist would let a bad PIN enroll on the server and then fail
/// locally — leaving the account server-enrolled with no Cube armed.
pub(crate) fn duress_pin_collision_check_blocking(
    root: &std::path::Path,
    duress_pin: &str,
) -> Result<(), String> {
    use crate::services::unlock::{self, throttle::PinAttempt, PinOutcome};

    // Identity and runtime preflight over every Cube first: a dormant-chain
    // Cube must refuse here, before the wizard enrolls anything server-side,
    // with copy that says why — not as an opaque unlock error below.
    let inventory = duress_cube_inventory(root)?;
    if inventory.is_empty() {
        return Err(DURESS_NO_CUBES_MSG.to_string());
    }
    ensure_import_duress_compatibility(&inventory)?;
    let mut total_cubes = 0usize;
    for entry in &inventory {
        for cube in &entry.settings.cubes {
            total_cubes += 1;
            let loc = unlock::CubeLocation::new(root, cube);
            match unlock::throttle::classify_regular_pin(root, &loc, duress_pin) {
                Ok(PinAttempt::Classified(PinOutcome::Unlock(_))) => {
                    return Err(DURESS_PIN_COLLIDES_MSG.to_string())
                }
                // Already this Cube's duress PIN (a re-enrolment with the same
                // value), or simply wrong. Neither is a collision with a real
                // unlock PIN.
                Ok(PinAttempt::Classified(PinOutcome::Duress | PinOutcome::Wrong)) => {}
                Ok(PinAttempt::Locked(remaining)) => {
                    return Err(unlock::throttle::lockout_message(remaining))
                }
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
    network: crate::chain::ChainId,
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
    // Whole-operation preflight: every directory's identity and every Cube's
    // chain are validated before a single byte is written (see
    // `duress_cube_inventory`). No per-network settings.json found — there are
    // no Cubes to arm (or the data directory couldn't be read). Fail loud
    // instead of marking duress "enabled" with no duress PIN written anywhere.
    let inventory = duress_cube_inventory(&root)?;
    ensure_import_duress_compatibility(&inventory)?;
    if inventory.is_empty() {
        return Err(DURESS_NO_CUBES_MSG.to_string());
    }

    // 0. Guard against a duress PIN that collides with a real unlock PIN, and
    //    against having no Cubes to arm, BEFORE writing anything. This mirrors
    //    the wizard's pre-flight check (which runs before any server enroll);
    //    re-running it here is the authoritative guard against an on-disk Cube
    //    set that changed since the pre-flight.
    duress_pin_collision_check_blocking(&root, &duress_pin)?;

    // The pre-write state of every network, so a later step can roll back —
    // read once by the preflight, which also validated the set.
    let network_dirs: Vec<crate::dir::NetworkDirectory> =
        inventory.iter().map(|e| e.dir.clone()).collect();
    let prior_settings: Vec<crate::app::settings::Settings> =
        inventory.into_iter().map(|e| e.settings).collect();

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
        // A pending "migration turned duress off" notice is false from here
        // on: the user has just turned it back on.
        st.migration_disarm_notice = false;
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
/// can't be read. Each candidate is charged to every PIN Cube it actually
/// tests; a locked Cube is skipped so another Cube's valid PIN can still pass.
pub(crate) fn verify_regular_cube_pin_blocking(
    root: &std::path::Path,
    pin: &str,
) -> Result<(), String> {
    use crate::services::unlock::{self, throttle::PinAttempt, PinOutcome};

    if pin.is_empty() {
        return Err("Enter your Cube unlock PIN to continue.".to_string());
    }
    let network_dirs = duress_enroll_network_dirs(root)?;
    if network_dirs.is_empty() {
        return Err(DURESS_NO_CUBES_MSG.to_string());
    }
    let mut any_pin = false;
    let mut shortest_lockout: Option<std::time::Duration> = None;
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
            match unlock::throttle::classify_regular_pin(root, &loc, pin) {
                Ok(PinAttempt::Classified(PinOutcome::Unlock(_))) => return Ok(()),
                // The duress PIN must not satisfy a step-up, and must not
                // reveal that it was recognised either.
                Ok(PinAttempt::Classified(PinOutcome::Duress | PinOutcome::Wrong)) => {}
                Ok(PinAttempt::Locked(remaining)) => {
                    shortest_lockout =
                        Some(shortest_lockout.map_or(remaining, |current| current.min(remaining)));
                }
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
    if let Some(remaining) = shortest_lockout {
        return Err(unlock::throttle::lockout_message(remaining));
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

    // 0. Whole-operation preflight (`duress_cube_inventory`): every directory
    //    must be a chain this build knows, every record must name the chain of
    //    the folder it sits in, and every chain must be one this build can
    //    run. Refused *before* step 1 so a device with a dormant-chain Cube is
    //    left exactly as it was — no decoy written into that Cube's folder, no
    //    local state reset — rather than half-disarmed.
    let inventory = duress_cube_inventory(&root)?;

    // 1. Clear the duress PIN hash on every Cube on every network — ALWAYS,
    //    whatever DuressLocalState records. Setting it to None is idempotent;
    //    stop at the first failure so a retry re-clears the rest.
    for entry in &inventory {
        let network_dir = &entry.dir;
        for cube in &entry.settings.cubes {
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
            // Keyed on the *directory's* chain — which the preflight proved
            // equal to the record's own — so the decoy lands in the folder the
            // Cube was actually found in.
            crate::services::unlock::marker::write_decoy(
                &root,
                entry.chain,
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

/// Chain progress from the local node, independent of Connect or wallet scans.
#[derive(Debug, Clone)]
pub struct LocalNodeSync {
    progress: f64,
    ibd: bool,
    blocks: u64,
    headers: u64,
    subversion: Option<String>,
    /// `None` when the node did not say (see [`local_switch::NodePruning::from_blockchain_info`]).
    pruning: Option<local_switch::NodePruning>,
}

/// The Vault scan a backend switch would discard, if one is running: the
/// wallet catching up, its first address scan through an Esplora/Electrum
/// backend, or a pending rescan.
///
/// A running full scan, including one started mid-session (a user rescan, or
/// the forced full scan after a `CannotConnect`), is read from the daemon's
/// lock-free `history_sync` report: `sync_status` returns `WalletFullScan`
/// while one runs. An embedded Esplora/Electrum backend with no completed poll
/// this session also counts as scanning whatever the height, which covers a
/// daemon too old to send that report.
pub(crate) fn running_vault_scan(
    daemon_backend: DaemonBackend,
    cache: &Cache,
) -> Option<local_switch::RunningScan> {
    let scans_addresses = matches!(
        daemon_backend,
        DaemonBackend::EmbeddedCoincubed(Some(NodeType::Esplora))
            | DaemonBackend::EmbeddedCoincubed(Some(NodeType::Electrum))
    );
    let status = sync_status(
        daemon_backend,
        cache.blockheight(),
        cache.sync_progress(),
        cache.last_poll_timestamp(),
        cache.last_poll_at_startup,
        cache.history_sync(),
    );
    local_switch::running_scan(
        status.wallet_is_syncing()
            || local_switch::first_address_scan_pending(
                scans_addresses,
                cache.last_poll_timestamp(),
                cache.last_poll_at_startup,
            ),
        cache.rescan_progress().is_some(),
    )
}

/// Log why the auto-switch to the local node is held, once per change.
fn log_local_switch_hold(hold: Option<local_switch::SwitchHold>) {
    use local_switch::{RunningScan, SwitchHold};
    match hold {
        None => {}
        Some(SwitchHold::Scanning(RunningScan::WalletSync)) => info!(
            "Local node is synced; waiting for the Vault to finish scanning before switching \
             to it, so the scan isn't thrown away."
        ),
        Some(SwitchHold::Scanning(RunningScan::Rescan)) => info!(
            "Local node is synced; waiting for the Vault's rescan to finish before switching \
             to it, so the rescan isn't thrown away."
        ),
        Some(SwitchHold::Unknown) => info!(
            "Local node is synced; not switching until its pruning and this Vault's history \
             can be read."
        ),
        Some(SwitchHold::Pruned(why)) => warn!("Not switching to the local node: {why}"),
    }
}

/// Prefer the pending node; keep observing the active local node after promotion.
fn local_node_sync_config(cfg: &DaemonConfig) -> Option<&coincubed::config::BitcoindConfig> {
    cfg.pending_bitcoind
        .as_ref()
        .or(match &cfg.bitcoin_backend {
            Some(coincubed::config::BitcoinBackend::Bitcoind(local)) => Some(local),
            _ => None,
        })
}

/// Poll the local bitcoind directly, including while the old wallet daemon stops.
async fn check_bitcoind_sync_progress(
    cfg: coincubed::config::BitcoindConfig,
) -> Result<LocalNodeSync, String> {
    let (user, pass) = local_switch::rpc_credentials(&cfg).await?;

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
    let blocks = result["blocks"]
        .as_u64()
        .ok_or_else(|| "Missing blocks in bitcoind response".to_string())?;
    let headers = result["headers"]
        .as_u64()
        .ok_or_else(|| "Missing headers in bitcoind response".to_string())?;
    // Best-effort like `subversion`: an unreadable answer only means a switch
    // to this node waits, not that its progress goes unreported.
    let pruning = local_switch::NodePruning::from_blockchain_info(result);
    Ok(LocalNodeSync {
        progress,
        ibd,
        blocks,
        headers,
        subversion,
        pruning,
    })
}

/// What the Vault needs read alongside a pending-node probe, to judge whether
/// that node could serve it (see [`local_switch`]).
struct VaultHistoryProbe {
    daemon: Arc<dyn Daemon + Sync + Send>,
    data_dir: CoincubeDirectory,
    chain: crate::chain::ChainId,
    wallet: Arc<Wallet>,
}

impl VaultHistoryProbe {
    /// How far back this Vault's history reaches, or `None` if the daemon
    /// could not say.
    ///
    /// Asked of the daemon over all four coin statuses rather than read from
    /// the cache, which holds only unspent coins — the same reason
    /// [`settle_rescan_obligation`] does: a Vault whose history is all spent
    /// would otherwise look like one with none.
    async fn read(
        self,
        node: &coincubed::config::BitcoindConfig,
        pruning: local_switch::NodePruning,
    ) -> Option<local_switch::VaultHistory> {
        use coincubed::commands::CoinStatus;
        let coins = match self
            .daemon
            .list_coins(
                &[
                    CoinStatus::Unconfirmed,
                    CoinStatus::Confirmed,
                    CoinStatus::Spending,
                    CoinStatus::Spent,
                ],
                &[],
            )
            .await
        {
            Ok(res) => res.coins,
            Err(e) => {
                tracing::debug!("Could not list the Vault's coins for the local-node check: {e}");
                return None;
            }
        };
        let mut rescan_owed = pending_rescan(&self.data_dir, self.chain, &self.wallet).is_some();
        let config = self.daemon.config()?;
        let data_dir = config.data_directory()?;
        let wallet_path = coincubed::bitcoind_watchonly_wallet_path(&data_dir);
        if !self.chain.is_blake2b() {
            let path = crate::node::history::recovery_path(
                &crate::node::retention::directory(&self.data_dir, self.chain.bitcoin_network()),
                &wallet_path,
            );
            match crate::node::history::read_json::<crate::node::history::Recovery>(&path) {
                Ok(Some(job))
                    if !matches!(
                        job.phase,
                        crate::node::history::Phase::Complete
                            | crate::node::history::Phase::Cancelled
                    ) =>
                {
                    return None
                }
                Err(_) => return None,
                Ok(Some(job))
                    if job.phase == crate::node::history::Phase::Complete && job.start == 0 =>
                {
                    let binding = crate::node::retention::managed_binding(
                        &self.data_dir,
                        self.chain.bitcoin_network(),
                        self.chain,
                        node,
                        &wallet_path,
                        &self.wallet.inheritance_descriptor()?.to_string(),
                    )
                    .ok()?;
                    job.validate(&binding).ok()?;
                    let rpc = crate::node::history::NodeRpc::new(node, &wallet_path, self.chain)
                        .await
                        .ok()?;
                    rpc.check_chain(self.chain.bitcoin_network()).await.ok()?;
                    if rpc.block_hash(job.target).await.ok()? != job.target_hash {
                        return None;
                    }
                    if rpc
                        .call(true, "getwalletinfo", serde_json::json!([]))
                        .await
                        .ok()?["scanning"]
                        .as_bool()
                        != Some(false)
                    {
                        return None;
                    }
                    // A full Core scan permits handoff. Its durable records are
                    // replayed by local daemon startup before retiring the marker.
                    rescan_owed = false;
                }
                _ => {}
            }
        }
        let history = local_switch::VaultHistory::from_coin_heights(
            coins.iter().map(|c| c.block_height),
            rescan_owed,
        );
        if self.chain.is_blake2b()
            || !matches!(history, local_switch::VaultHistory::From(_))
            || local_switch::pruned_node_serves(pruning, history).is_ok()
        {
            return Some(history);
        }
        // Funding AND spending transactions, spent coins included. Checking a
        // single recent coin would miss a partially scanned local wallet.
        let txids: std::collections::BTreeSet<_> = coins
            .iter()
            .flat_map(|coin| {
                coin.block_height
                    .map(|_| coin.outpoint.txid)
                    .into_iter()
                    .chain(
                        coin.spend_info
                            .as_ref()
                            .filter(|spend| spend.height.is_some())
                            .map(|spend| spend.txid),
                    )
            })
            .collect();
        match local_switch::local_wallet_tracks(
            node,
            &wallet_path,
            self.chain,
            &txids.into_iter().collect::<Vec<_>>(),
        )
        .await
        {
            Ok(true) => {
                let rpc = crate::node::history::NodeRpc::new(node, &wallet_path, self.chain)
                    .await
                    .ok()?;
                rpc.check_chain(self.chain.bitcoin_network()).await.ok()?;
                match crate::node::history::reconcile_coins(
                    &rpc,
                    &coins,
                    crate::node::history::outspend_source(config),
                )
                .await
                {
                    Ok(None) => Some(local_switch::VaultHistory::TrackedLocally),
                    Ok(Some(_)) => Some(history),
                    Err(_) => None,
                }
            }
            Ok(false) => Some(history),
            Err(e) => {
                tracing::debug!("Could not check the local wallet's history: {e}");
                None
            }
        }
    }
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
    /// Bumped to restart a stream whose id didn't change, such as after a
    /// renewal that returned the same id while the stream sleeps on a
    /// refusal re-check.
    restart: u64,
}

impl std::hash::Hash for ConnectStreamSubKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        "connect-stream".hash(state);
        self.cfg.device_id.hash(state);
        self.cfg.grpc_url.hash(state);
        self.restart.hash(state);
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

/// Background task that assembles a `ConnectStreamConfig` from the
/// resolved gRPC URL + the on-disk Connect cache. Runs once at App startup
/// when a Connect session is available. Yields `Message::ConnectStreamReady`
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

/// Registration must keep the App's bearer Arc when it represents this exact
/// cached session: a same-session refresh can rotate it during the gRPC RPC.
async fn connect_registration_auth(
    shared: Option<
        Arc<tokio::sync::RwLock<crate::services::connect::client::auth::AccessTokenResponse>>,
    >,
    cached: crate::services::connect::client::auth::AccessTokenResponse,
    network_dir: &crate::dir::NetworkDirectory,
    email: &str,
) -> Arc<tokio::sync::RwLock<crate::services::connect::client::auth::AccessTokenResponse>> {
    if let Some(shared) = shared {
        let current = {
            let current = shared.read().await;
            current.clone()
        };
        let same_session = (current.access_token == cached.access_token
            && current.refresh_token == cached.refresh_token)
            || crate::services::connect::client::cache::ConnectCache::from_file(network_dir)
                .ok()
                .and_then(|cache| cache.active_account().cloned())
                .is_some_and(|account| {
                    account.email == email
                        && account.tokens.access_token == current.access_token
                        && account.tokens.refresh_token == current.refresh_token
                });
        if same_session {
            return shared;
        }
    }
    Arc::new(tokio::sync::RwLock::new(cached))
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

    let completion_email = email.clone();
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
        move |cfg| Message::ConnectStreamReady(cfg.map(|cfg| (cfg, completion_email))),
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
    network: impl Into<crate::chain::ChainId>,
    wallet: &Wallet,
) -> Option<settings::PendingRescan> {
    settings::WalletSettings::from_file(&data_dir.network_directory(network), |s| {
        s.descriptor_checksum == wallet.descriptor_checksum
    })
    .ok()
    .flatten()
    .and_then(|s| s.pending_rescan)
}

/// Whether this Vault needs the user to supply a date before its rescan can run.
///
/// Only [`settings::PendingRescan::DateUnknown`] does. Every other restore
/// records a birthday, and [`settle_rescan_obligation`] scans from it unattended
/// at launch — nothing to prompt about. An older Recovery Kit records nothing
/// that dates the wallet, so its obligation sits there doing nothing until a
/// date arrives, and the only place that can come from is the user.
///
/// Gated on a local bitcoind as well, since that is the only backend that can be
/// told to rescan at all.
fn needs_rescan_date(
    data_dir: &CoincubeDirectory,
    network: impl Into<crate::chain::ChainId>,
    wallet: &Wallet,
    daemon_backend: &DaemonBackend,
) -> bool {
    matches!(
        pending_rescan(data_dir, network, wallet),
        Some(settings::PendingRescan::DateUnknown)
    ) && daemon_backend.is_coincubed()
        && daemon_backend
            .node_type()
            .map(|nt| nt == NodeType::Bitcoind)
            // We don't know the node type for external coincubed so assume it's bitcoind.
            .unwrap_or(true)
}

/// Whether the daemon has just finished the full scan a restored Vault was owed.
///
/// On Electrum and Esplora a full scan looks up every address the Vault has used,
/// from the start of the chain: once one has completed and reached the database,
/// the history is in, whatever it contains. That retires the obligation even for a
/// Vault that is genuinely empty, which [`settle_rescan_obligation`]'s
/// "a confirmed coin is proof" rule never could — leaving the marker behind to
/// misreport the Vault as unscanned the moment it is moved to a local node.
///
/// Acted on once per session, from the first refresh that shows a completed scan,
/// so the settings file is not rewritten on every refresh. That includes a scan
/// that had already completed by the app's first snapshot: the daemon's poller
/// starts before the loader takes it, and a fast scan can win that race. A local
/// `bitcoind` is excluded: it rescans from a date, and only coins prove that date
/// was early enough.
fn full_scan_settles_rescan_obligation(
    backend: &DaemonBackend,
    already_retired: bool,
    history: &coincubed::commands::HistorySync,
) -> bool {
    matches!(
        backend,
        DaemonBackend::EmbeddedCoincubed(Some(NodeType::Electrum | NodeType::Esplora))
    ) && !already_retired
        && history.full_scan_completed_at.is_some()
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

/// Settle the rescan obligation this Vault carries: retire it if the seeding
/// scan demonstrably worked, otherwise try again.
///
/// # Why the GUI seeds at all, when the daemon has a heal
///
/// They cover different halves and neither is sufficient alone. A fresh restore
/// begins with an **empty database** — the datadir is new — and
/// `coincubed::heal_scan_window` derives how far back to scan from the coins it
/// holds, so on an empty database it has nothing to reason from and correctly
/// does nothing. The Recovery Kit's birthday is the only thing that knows where
/// this wallet's history starts, and this is what carries it to the node.
/// Without it the wallet is scanned from "now", finds nothing, the database
/// stays empty, and the heal never gets anything to work from.
///
/// Conversely this alone is not enough: the scan can be discarded by anything
/// that re-creates the watchonly wallet afterwards (a backend switch does, at
/// `timestamp: "now"`), or cut short by a shutdown. That is what the heal
/// repairs, and it can, because by then this scan has put the history in the
/// database for it to derive a date from.
///
/// So: this seeds the history, the daemon keeps it.
///
/// # Why this asks the daemon instead of reading the cache
///
/// Twice over: for the coins, and for whether a scan is already running.
///
/// Proof that the history was found is "we have a confirmed coin", and the
/// app's cache cannot answer that. [`crate::app::cache::coins_to_cache`] lists
/// only `Unconfirmed | Confirmed` — the **unspent** coins — because that is what
/// every balance and coin-selection surface wants. A restored Vault whose
/// history is entirely spent has none of those, and would look exactly like a
/// Vault that was never scanned: the obligation would never retire and every
/// launch would start another full-chain rescan from the birthday, for ever.
///
/// So the question is put to the daemon over all four statuses, spent included.
/// One extra RPC, paid only by a Vault that still carries an obligation.
///
/// # Why proof at all
///
/// The obligation used to be retired the moment `start_rescan` returned `Ok`,
/// which proves nothing: bitcoind stamps the descriptors before it scans, and
/// keeps the stamp through an aborting shutdown. A scan interrupted before
/// recording any coin then left an empty database — precisely the one state
/// `coincubed::heal_scan_window` cannot derive a date from — with the obligation
/// already discarded. That Vault never recovered.
fn settle_rescan_obligation(
    daemon: Arc<dyn Daemon + Sync + Send>,
    network_dir: crate::dir::NetworkDirectory,
    descriptor_checksum: String,
    timestamp: Option<u32>,
) -> Task<Message> {
    Task::perform(
        async move {
            use coincubed::commands::CoinStatus;

            let seeded = match daemon
                .list_coins(
                    &[
                        CoinStatus::Unconfirmed,
                        CoinStatus::Confirmed,
                        CoinStatus::Spending,
                        CoinStatus::Spent,
                    ],
                    &[],
                )
                .await
            {
                Ok(res) => res.coins.iter().any(|c| c.block_height.is_some()),
                // Not an answer. Leave the obligation alone and try again next
                // start rather than guessing in either direction.
                Err(e) => {
                    tracing::warn!(
                        "Could not check whether this restored Vault's history is present: {}. \
                         The rescan obligation stays recorded.",
                        e
                    );
                    return;
                }
            };

            if seeded {
                // History is here. Everything that can go wrong from now on — a
                // wallet re-created, a later scan cut short — is visible to the
                // daemon, which re-checks on every start and repairs it.
                if let Err(e) = settings::update_settings_file(&network_dir, |settings| {
                    Some(cleared_pending_rescan(settings, &descriptor_checksum))
                })
                .await
                {
                    // Harmless: re-evaluated on every start, and the next one
                    // sees the same coins and retires it.
                    tracing::warn!("Could not retire the pending rescan marker: {}", e);
                }
                return;
            }

            // Not seeded. `None` here is `DateUnknown`: no date to retry from,
            // so the obligation stays and the prompt asks the user for one.
            let Some(timestamp) = timestamp else {
                return;
            };

            // Nothing already scanning — a second `importdescriptors` on a live
            // scan restarts it from the top, so a slow scan would never finish
            // if every launch kicked it over.
            //
            // Asked of the daemon, not of `Cache::rescan_progress`: that field
            // is `None` at every `App::new`, because both construction paths
            // leave it at its default and only the periodic refresh ever fills
            // it in. Reading it here looked like a guard and was never once
            // true.
            match daemon.get_info().await {
                Ok(info) if info.rescan_progress.is_some() => {
                    tracing::info!(
                        "A rescan is already running; leaving the obligation recorded rather \
                         than restarting it."
                    );
                    return;
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(
                        "Could not check for a running rescan: {}. Not starting one; the \
                         obligation stays recorded for the next start.",
                        e
                    );
                    return;
                }
            }
            match daemon.start_rescan(timestamp).await {
                Ok(()) => tracing::info!(
                    "Started the rescan this restored Vault owed, from unix time {}. The \
                     obligation stays recorded until a confirmed coin shows it worked.",
                    timestamp
                ),
                Err(e) => tracing::error!(
                    "Restored Vault needs a rescan from unix time {} but the daemon refused: \
                     {}. It will be retried on the next start, or can be run by hand in \
                     Settings > Node.",
                    timestamp,
                    e
                ),
            }
        },
        |_| Message::Tick,
    )
}

/// Every Bitcoin Blake2b copy on this device: descriptor checksum → the
/// unique valid Bitcoin Blake2b Cube that reuses it, or `None` for an
/// ambiguous identity. The reader behind [`claim_target_exists`].
fn claim_targets(datadir: &CoincubeDirectory) -> std::collections::HashMap<String, Option<String>> {
    let fork_dir = datadir.network_directory(crate::chain::ChainId::BitcoinBlake2b);
    // No file, no targets — and no retry. `Settings::from_file` treats
    // `NotFound` as possibly-transient and sleeps between five attempts (at
    // least 300 ms) because the paths that call it are reading a file that is
    // *supposed* to exist. This one asks a question whose ordinary answer is
    // "there is nothing here yet": the fork settings file does not exist until
    // the first claim completes, so every caller before then would pay the
    // whole retry budget for a file whose absence is the answer. Home asks on
    // every message.
    if !fork_dir.path().join(settings::SETTINGS_FILE_NAME).is_file() {
        return std::collections::HashMap::new();
    }
    settings::Settings::from_file(&fork_dir)
        .map(|s| {
            // A **Cube**, not a wallet record. The install writes the wallet
            // before the exit seam writes the Cube, so an interruption between
            // the two leaves a checksum with nothing to open: keying on the
            // wallet would hide the claim entry (nothing to retry with) while
            // Home shows no Cube (nothing to open), stranding the user between
            // two screens that each think the other has it.
            // Presence still blocks another target installation, but ambiguous
            // descriptor or Cube identities must never select a Claim journal.
            let mut ids = std::collections::HashMap::new();
            for cube in &s.cubes {
                *ids.entry(&cube.id).or_insert(0usize) += 1;
            }
            let mut targets = std::collections::HashMap::new();
            for cube in &s.cubes {
                let Some(wallet) = &cube.vault_wallet_id else {
                    continue;
                };
                let selected = (cube.network == crate::chain::ChainId::BitcoinBlake2b
                    && !cube.id.is_empty()
                    && ids.get(&cube.id) == Some(&1))
                .then(|| cube.id.clone());
                targets
                    .entry(wallet.descriptor_checksum.clone())
                    .and_modify(|selected| *selected = None)
                    .or_insert(selected);
            }
            targets
        })
        .unwrap_or_default()
}

/// Whether a claim target already exists for this descriptor.
///
/// One definition, two consumers: the running app asks per Cube
/// ([`App::claim_source_cube`]) and Home asks for the whole set at refresh
/// ([`crate::home::Home::claim_availability`]). They were separate reads once
/// and the two drifted — the App was repaired to key on the Cube while Home
/// went on counting wallets — so they now share [`claim_target_checksums`].
pub(crate) fn claim_target_exists(datadir: &CoincubeDirectory, descriptor_checksum: &str) -> bool {
    claim_targets(datadir).contains_key(descriptor_checksum)
}

impl App {
    #[cfg(test)]
    pub(crate) fn current_menu_for_test(&self) -> &Menu {
        &self.panels.current
    }

    #[cfg(test)]
    pub(crate) fn connect_user_id_for_test(&self) -> Option<u32> {
        self.panels
            .connect
            .account
            .user
            .as_ref()
            .map(|user| user.id)
    }

    /// Identifies a harmless shell dismissal for the GUI's sole Escape
    /// dispatcher. This does not handle the key or mutate the App.
    pub fn shell_escape_intent(&self) -> Option<view::Message> {
        view::nav::shell::escape_intent(view::nav::shell::ShellState {
            switcher_open: self.cache.cube_switcher_open,
            connect_open: self.cache.connect_panel_open,
            lock_confirmation: self.cache.cube_lock_confirmation,
            backup_attention: false,
        })
    }

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
        cube_settings: settings::CubeSettings,
        connect_auth: Option<(
            Arc<tokio::sync::RwLock<crate::services::connect::client::auth::AccessTokenResponse>>,
            String,
        )>,
    ) -> Result<(App, Task<Message>), Error> {
        if cube_settings.network.is_blake2b() {
            return Err(Error::Daemon(DaemonError::ConnectAnchor(
                coincubed::connect::AdmissionError::MissingAuth.into(),
            )));
        }
        let (mut app, task) = Self::new_inner(
            cache,
            wallet,
            Some(breez_client),
            spark_backend,
            config,
            daemon,
            data_dir,
            internal_bitcoind,
            cube_settings,
            connect_auth,
        );
        let provision = app.background_managed_node_task(true);
        Ok((app, Task::batch([task, provision])))
    }

    /// Build an admitted fork Vault without constructing Liquid or Spark clients.
    #[allow(clippy::too_many_arguments)]
    pub fn new_for_chain(
        cache: Cache,
        wallet: Arc<Wallet>,
        cube_encryption_key: Option<Arc<crate::services::connect::crypto::CubeEncryptionKey>>,
        client: crate::services::coincube::CoincubeClient,
        config: Config,
        daemon: Arc<dyn Daemon + Sync + Send>,
        data_dir: CoincubeDirectory,
        cube_settings: settings::CubeSettings,
    ) -> Result<(App, Task<Message>), Error> {
        Self::new_for_chain_with_node(
            cache,
            wallet,
            cube_encryption_key,
            client,
            config,
            daemon,
            data_dir,
            cube_settings,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_for_chain_with_node(
        cache: Cache,
        wallet: Arc<Wallet>,
        cube_encryption_key: Option<Arc<crate::services::connect::crypto::CubeEncryptionKey>>,
        client: crate::services::coincube::CoincubeClient,
        config: Config,
        daemon: Arc<dyn Daemon + Sync + Send>,
        data_dir: CoincubeDirectory,
        cube_settings: settings::CubeSettings,
        internal_bitcoind: Option<Bitcoind>,
    ) -> Result<(App, Task<Message>), Error> {
        Self::build_fork(
            cache,
            wallet,
            cube_encryption_key,
            Some(client),
            config,
            daemon,
            data_dir,
            cube_settings,
            internal_bitcoind,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_local_fork(
        cache: Cache,
        wallet: Arc<Wallet>,
        cube_encryption_key: Option<Arc<crate::services::connect::crypto::CubeEncryptionKey>>,
        config: Config,
        daemon: Arc<dyn Daemon + Sync + Send>,
        data_dir: CoincubeDirectory,
        cube_settings: settings::CubeSettings,
        internal_bitcoind: Option<Bitcoind>,
    ) -> Result<(App, Task<Message>), Error> {
        Self::build_fork(
            cache,
            wallet,
            cube_encryption_key,
            None,
            config,
            daemon,
            data_dir,
            cube_settings,
            internal_bitcoind,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn build_fork(
        mut cache: Cache,
        wallet: Arc<Wallet>,
        cube_encryption_key: Option<Arc<crate::services::connect::crypto::CubeEncryptionKey>>,
        client: Option<crate::services::coincube::CoincubeClient>,
        config: Config,
        daemon: Arc<dyn Daemon + Sync + Send>,
        data_dir: CoincubeDirectory,
        cube_settings: settings::CubeSettings,
        internal_bitcoind: Option<Bitcoind>,
    ) -> Result<(App, Task<Message>), Error> {
        let chain = cube_settings.network;
        let refuse = |error: Error| -> Result<(App, Task<Message>), Error> { Err(error) };
        if !chain.is_blake2b() || wallet.chain != chain || cache.chain() != chain {
            return refuse(Error::Daemon(DaemonError::ConnectAnchor(
                coincubed::connect::AdmissionError::WrongChain.into(),
            )));
        }
        let local_backend = daemon
            .config()
            .is_some_and(|cfg| crate::chain::is_managed_local_fork(cfg, &data_dir));
        if local_backend
            && (cube_encryption_key.is_none()
                || !daemon
                    .config()
                    .and_then(|cfg| cfg.data_directory())
                    .is_some_and(|found| {
                        crate::chain::same_path(
                            found.path(),
                            data_dir
                                .network_directory(chain)
                                .coincubed_data_directory(&wallet.id())
                                .path(),
                        )
                    }))
        {
            return refuse(Error::Daemon(DaemonError::ConnectAnchor(
                coincubed::connect::AdmissionError::InvalidBackend.into(),
            )));
        }
        if !local_backend
            && client
                .as_ref()
                .and_then(|c| c.token())
                .is_none_or(|token| token.trim().is_empty())
        {
            return refuse(Error::Daemon(DaemonError::ConnectAnchor(
                coincubed::connect::AdmissionError::MissingAuth.into(),
            )));
        }
        let Some(backend_config) = daemon.config() else {
            return refuse(Error::Daemon(DaemonError::ConnectAnchor(
                coincubed::connect::AdmissionError::InvalidBackend.into(),
            )));
        };
        let endpoint = format!(
            "{}/api/v1/esplora/{}",
            client
                .as_ref()
                .map(|c| c.base_url.trim_end_matches('/'))
                .unwrap_or(""),
            crate::installer::connect_esplora_path(chain)
        );
        let managed_dir = crate::node::bitcoind::internal_bitcoind_datadir_for(
            &data_dir,
            crate::node::bitcoind::NodeChainFamily::BitcoinBlake2b,
        );
        let managed = |node: &coincubed::config::BitcoindConfig| {
            node.addr.ip().is_loopback()
                && matches!(&node.rpc_auth, coincubed::config::BitcoindRpcAuth::CookieFile(cookie) if crate::chain::same_path(cookie, &crate::node::bitcoind::internal_bitcoind_cookie_path(&managed_dir, &chain.bitcoin_network())))
        };
        let correct_backend = matches!(&backend_config.bitcoin_backend,
            Some(coincubed::config::BitcoinBackend::Esplora(esplora))
                if client.is_some() && esplora.addr == endpoint && esplora.fallback_addr.is_none()
                    && esplora.secondary_fallback_addr.is_none())
            || matches!(&backend_config.bitcoin_backend, Some(coincubed::config::BitcoinBackend::Bitcoind(_)) if local_backend);
        if backend_config.bitcoin_config.chain != chain
            || !daemon.backend().is_embedded()
            || !correct_backend
            || internal_bitcoind
                .as_ref()
                .is_some_and(|node| !managed(&node.config))
            || backend_config
                .pending_bitcoind
                .as_ref()
                .is_some_and(|node| !managed(node))
        {
            return refuse(Error::Daemon(DaemonError::ConnectAnchor(
                coincubed::connect::AdmissionError::InvalidBackend.into(),
            )));
        }
        cache.cube_encryption_key = cube_encryption_key;
        let (mut app, task) = Self::new_inner(
            cache,
            wallet,
            None,
            None,
            config,
            daemon,
            data_dir,
            internal_bitcoind,
            cube_settings,
            None,
        );
        if let Some(client) = client {
            app.panels.connect.install_admitted_client(client.clone());
            app.cache.has_connect_session = true;
            app.fork_connect_client = Some(client);
        }
        let provision = app.background_managed_node_task(true);
        Ok((app, Task::batch([task, provision])))
    }

    /// Start the managed-node install a Connect-first Vault asked for, unless
    /// one is already running. `announce` adds the opening toast; a retry from
    /// Settings → Node is already on the page that shows its progress.
    fn background_managed_node_task(&mut self, announce: bool) -> Task<Message> {
        if self.cache.background_node_install.is_running() {
            return Task::none();
        }
        let Some(cfg) = self.daemon.as_ref().and_then(|daemon| daemon.config()) else {
            return Task::none();
        };
        let Some(flavor_name) = cfg.pending_managed_node_install.clone() else {
            return Task::none();
        };
        if cfg.pending_bitcoind.is_some()
            || !matches!(
                cfg.bitcoin_backend,
                Some(coincubed::config::BitcoinBackend::Esplora(_))
            )
        {
            return Task::none();
        }
        let flavor = match flavor_name.as_str() {
            "Core" => crate::node::bitcoind::NodeFlavor::Core,
            "Knots" => crate::node::bitcoind::NodeFlavor::Knots,
            "KnotsBlake2b" => crate::node::bitcoind::NodeFlavor::KnotsBlake2b,
            _ => return Task::none(),
        };
        use iced::task::Sipper as _;
        // Unique across App instances: a replaced App's task keeps running,
        // and its messages reach whichever App now holds the tab.
        static NEXT_ATTEMPT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let attempt = NEXT_ATTEMPT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.background_node_attempt = Some(attempt);
        self.cache.background_node_install = cache::BackgroundNodeInstall::Downloading(None);
        let root = self.datadir.clone();
        let chain = self.cache.chain();
        let client = self.fork_connect_client.clone();
        let cube = self.cube_settings.id.clone();
        let progress_cube = cube.clone();
        let provision = Task::sip(
            iced::task::sipper(async move |progress| {
                if chain.is_blake2b() {
                    crate::chain::require_local_feature(chain, client, &root).await?;
                }
                crate::installer::step::node::bitcoind::provision_managed_node_in_background(
                    root, chain, flavor,
                )
                .run(progress)
                .await
            }),
            move |progress| Message::BackgroundManagedNodeProgress {
                cube: progress_cube.clone(),
                attempt,
                progress,
            },
            move |result| Message::BackgroundManagedNodeReady {
                cube,
                attempt,
                flavor: flavor_name,
                result,
            },
        );
        if !announce {
            return provision;
        }
        Task::batch([
            Task::done(Message::View(view::Message::ShowToast(
                log::Level::Info,
                "Your Vault is ready on Connect. The local node is being set up in the background."
                    .to_string(),
            ))),
            provision,
        ])
    }

    #[allow(clippy::too_many_arguments)]
    fn new_inner(
        cache: Cache,
        wallet: Arc<Wallet>,
        breez_client: Option<Arc<BreezClient>>,
        spark_backend: Option<Arc<crate::app::wallets::SparkBackend>>,
        config: Config,
        daemon: Arc<dyn Daemon + Sync + Send>,
        data_dir: CoincubeDirectory,
        internal_bitcoind: Option<Bitcoind>,
        cube_settings: settings::CubeSettings,
        connect_auth: Option<(
            Arc<tokio::sync::RwLock<crate::services::connect::client::auth::AccessTokenResponse>>,
            String,
        )>,
    ) -> (App, Task<Message>) {
        let mut cache = cache;
        let (breez_client, spark_backend) = if cube_settings.network.is_blake2b() {
            (None, None)
        } else {
            (breez_client, spark_backend)
        };
        cache.fiat_chain = cube_settings.network;
        cache.has_vault = wallet.supports_inheritance();
        cache.has_imported_bitcoin = wallet.is_imported();
        cache.vault_lacks_keychain_key = wallet.lacks_keychain_key();
        cache.wallet_names = cache::WalletNames::new(&cube_settings, Some(&wallet));
        if cache.fiat_chain.is_blake2b() {
            cache.clear_btcb2_fiat();
        }
        // Resolved once per cube-open, from the fork chain's settings file.
        // Only a Bitcoin Cube can be a claim source, so the question is not
        // asked on any other chain.
        cache.btcb2_already_claimed = wallet.supports_inheritance()
            && cube_settings.network == crate::chain::ChainId::Bitcoin
            && claim_target_exists(&data_dir, &wallet.descriptor_checksum);
        // Connect blinding (PR D3): derive the Cube's encryption key once from
        // the master signer the unlock already loaded, so every surface that
        // opens a Connect-served key can do so without re-prompting for a PIN.
        if let Some(client) = &breez_client {
            cache.cube_encryption_key = derive_cube_encryption_key(client, cache.network);
        }
        // …and load (or mint) this device's signing-rail transport key. Not
        // seed-derived and not Cube-scoped — it comes up with the rail at login
        // (PR D4).
        cache.connect_transport_key =
            load_connect_transport_key(&data_dir.network_directory(cache.chain()));
        let cache = cache;
        let config_arc = Arc::new(config);
        let wallet_registry = match &breez_client {
            Some(client) => crate::app::wallets::WalletRegistry::with_spark(
                Arc::new(LiquidBackend::new(client.clone())),
                spark_backend.clone(),
            ),
            None => crate::app::wallets::WalletRegistry::vault_only(),
        };

        // A Vault restored from a Recovery Kit lands its descriptors in a
        // watchonly wallet that has never scanned them, so it owes a rescan from
        // the kit's recorded birthday. Read off disk rather than threaded
        // through the loader messages: the rescan has to survive a quit or a
        // crash part-way through, which an in-memory flag would not.
        let pending_rescan = wallet
            .supports_inheritance()
            .then(|| pending_rescan(&data_dir, cache.chain(), &wallet))
            .flatten();
        let mut panels = Panels::new(
            breez_client.clone(),
            spark_backend.clone(),
            &cache,
            wallet.clone(),
            data_dir.clone(),
            daemon.backend(),
            internal_bitcoind.as_ref(),
            config_arc.clone(),
            cube_settings.id.clone(),
            cube_settings.name.clone(),
            cache.chain().api_str().to_string(),
        );
        // Connect blinding (PR D2): hand the panel the seed-derived encryption
        // pubkey persisted at unlock, so the registration wave can publish it.
        panels.connect.account.bitcoin_blake2b_opt_in =
            settings::global::GlobalSettings::load_bitcoin_blake2b_beta(
                &settings::global::GlobalSettings::path(&data_dir),
            );
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
        // This device's Spark Stable Balance decision rides the registration
        // request so the Cube's server record — the cross-device copy — has it.
        panels
            .connect
            .set_spark_stable_balance(cube_settings.spark_stable_balance);
        let mut tasks = vec![];
        if wallet.is_imported() {
            if let Some(import) = cube_settings.bitcoin_import.clone() {
                let mut panel = state::imported_bitcoin::ImportedBitcoinPanel::new(
                    data_dir.network_directory(cube_settings.network),
                    cube_settings.id.clone(),
                    *import,
                );
                panel.set_hardware_root(data_dir.clone());
                tasks.push(panel.bind_runtime(daemon.clone()));
                // Vault settings say whether recovery words are stored here.
                tasks.push(panel.load_key_inventory());
                panels.imported_bitcoin = Some(Box::new(panel));
                panels.global_home.imported_installed = true;
            }
        }
        let completed_recovery = daemon
            .config()
            .and_then(|cfg| {
                let node = local_node_sync_config(cfg)?;
                let wallet_path = coincubed::bitcoind_watchonly_wallet_path(&cfg.data_directory()?);
                let binding = crate::node::retention::managed_binding(
                    &data_dir,
                    cache.network,
                    cache.chain(),
                    node,
                    &wallet_path,
                    &wallet.inheritance_descriptor()?.to_string(),
                )
                .ok()?;
                let path = crate::node::history::recovery_path(
                    &crate::node::retention::directory(&data_dir, cache.network),
                    &wallet_path,
                );
                let job: crate::node::history::Recovery =
                    crate::node::history::read_json(&path).ok()??;
                job.validate(&binding).ok()?;
                Some(job.phase == crate::node::history::Phase::Complete && job.start == 0)
            })
            .unwrap_or(false);
        if let Some(pending) = pending_rescan.filter(|_| !completed_recovery) {
            tasks.push(settle_rescan_obligation(
                daemon.clone(),
                data_dir.network_directory(cache.chain()),
                wallet.descriptor_checksum.clone(),
                pending.timestamp(),
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
        // The Overview opens first; its "Recent activity" reads Activity.
        tasks.push(panels.activity.refresh_scope(
            view::activity::RecentScope::Cube,
            Some(daemon.clone()),
            Some(wallet.clone()),
        ));
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
        cache_with_vault.has_vault = wallet.supports_inheritance();
        cache_with_vault.has_imported_bitcoin = wallet.is_imported();
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
        cache_with_vault.liquid_gate.local_state_exists = !cache_with_vault.chain().is_blake2b()
            && crate::app::breez_liquid::local_state_exists(
                data_dir.path(),
                cache_with_vault.network,
            );
        panels.show_liquid(cache_with_vault.liquid_gate);
        cache_with_vault.connect_tokens = connect_auth_arc.clone();
        cache_with_vault.connect_email = connect_email.clone();
        // A restored on-disk session means
        // the user is signed in to Connect even though the Connect panel won't
        // reach its `Dashboard` step — and so won't flip `connect_authenticated`
        // — until its keyring session check completes. Mirror it now so the
        // keychain-unavailable modal offers "Sign with Connect" rather than a
        // "Sign in to Connect" that would no-op.
        cache_with_vault.has_connect_session = connect_auth_arc.is_some();
        cache_with_vault.restorable_connect_session = panels.connect.account.has_stored_session();
        cache_with_vault.p2p_test_coordinator = panels
            .p2p
            .as_ref()
            .is_some_and(|p| p.has_test_coordinator());
        let mut app = Self {
            transfer_destination: None,
            panels: Box::new(panels),
            cache: cache_with_vault,
            daemon: Some(daemon),
            wallet: Some(wallet),
            local_mcp: Default::default(),
            imported_kit_metadata: Default::default(),
            breez_client,
            fork_connect_client: None,
            local_feature_valid: true,
            local_feature_checked_at: None,
            local_feature_check: None,
            wallet_registry,
            internal_bitcoind,
            cube_settings,
            config: config_arc,
            datadir: data_dir,
            errors: Vec::with_capacity(8),
            current_error_id: 256,
            bitcoind_sync_probe_in_progress: false,
            node_net_stats_probe_in_progress: false,
            node_history_in_progress: false,
            node_history_pending: None,
            node_history_epoch: 0,
            node_history_request_epoch: 0,
            claim_session_invalidated: false,
            claim_hold_epoch: 0,
            daemon_switch_in_progress: false,
            background_node_attempt: None,
            rescan_obligation_retired_by_scan: false,
            rescan_obligation_retire_in_flight: false,
            auto_switch_suppressed: false,
            local_switch_hold: None,
            entangled_in_flight: HashSet::new(),
            chain_proofs_in_flight: HashSet::new(),
            split_refresh_issued: 0,
            split_refresh_applied: 0,
            twin_checks: std::collections::HashMap::new(),
            twin_records: Vec::new(),
            chain_proofs_loaded: false,
            unswept_in_flight: None,
            unswept_session: None,
            seen_session: None,
            show_received_celebration: false,
            show_recovery_alerts_prompt: false,
            spark_stable_balance_reconciled: false,
            spark_nickname_in_flight: None,
            spark_nickname_queued: None,
            received_celebration_amount: String::new(),
            received_celebration_context: "transaction-received".to_string(),
            received_celebration_wallet: "",
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
            connect_renewal: Renewal::default(),
            connect_stream_restart: 0,
            in_app_otp_cache_snapshot: None,
            in_app_restore_cache_snapshot: None,
            in_app_refresh_commit: None,
            deferred_connect_stream_triggers: Vec::new(),
        };
        app.imported_kit_metadata
            .reload(&app.datadir, &app.cube_settings);
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
    ) -> Result<(App, Task<Message>), Error> {
        if cube_settings.network.is_blake2b() {
            return Err(Error::Daemon(DaemonError::ConnectAnchor(
                coincubed::connect::AdmissionError::InvalidBackend.into(),
            )));
        }
        let cube_encryption_key = derive_cube_encryption_key(&breez_client, network);
        Ok(Self::without_wallet(
            Some(breez_client),
            spark_backend,
            config,
            datadir,
            network,
            cube_settings,
            cube_encryption_key,
        ))
    }

    /// A Bitcoin Blake2b Cube that has no Vault yet. It opens like a Bitcoin
    /// Cube without one, with Create Vault next, but never starts Liquid or
    /// Spark: none of their SDKs serve this chain. The encryption key comes
    /// from the master signer the unlock verified.
    pub fn new_fork_without_wallet(
        cube_encryption_key: Arc<crate::services::connect::crypto::CubeEncryptionKey>,
        client: Option<crate::services::coincube::CoincubeClient>,
        config: Config,
        datadir: CoincubeDirectory,
        cube_settings: settings::CubeSettings,
    ) -> Result<(App, Task<Message>), Error> {
        if !cube_settings.network.is_blake2b() || cube_settings.vault_wallet_id.is_some() {
            return Err(Error::Daemon(DaemonError::ConnectAnchor(
                coincubed::connect::AdmissionError::WrongChain.into(),
            )));
        }
        let network = cube_settings.network.bitcoin_network();
        let (mut app, task) = Self::without_wallet(
            None,
            None,
            config,
            datadir,
            network,
            cube_settings,
            Some(cube_encryption_key),
        );
        if let Some(client) = client.filter(|c| c.token().is_some()) {
            app.panels.connect.install_admitted_client(client.clone());
            app.cache.has_connect_session = true;
            app.fork_connect_client = Some(client);
        }
        // Check the fork's feature now rather than at the first tick: a
        // passkey unlock reaches here without the PIN screen's check, and a
        // revoked feature must close the Cube before it is used. Signed in,
        // the account's own flag is checked, as the PIN screen does.
        let feature = app.fork_feature_check();
        Ok((app, Task::batch([task, feature])))
    }

    fn without_wallet(
        breez_client: Option<Arc<BreezClient>>,
        spark_backend: Option<Arc<crate::app::wallets::SparkBackend>>,
        config: Config,
        datadir: CoincubeDirectory,
        network: coincube_core::miniscript::bitcoin::Network,
        cube_settings: settings::CubeSettings,
        cube_encryption_key: Option<Arc<crate::services::connect::crypto::CubeEncryptionKey>>,
    ) -> (App, Task<Message>) {
        let fork = cube_settings.network.is_blake2b();
        let (breez_client, spark_backend) = if fork {
            (None, None)
        } else {
            (breez_client, spark_backend)
        };
        let config_arc = Arc::new(config);
        let wallet_registry = match &breez_client {
            Some(client) => crate::app::wallets::WalletRegistry::with_spark(
                Arc::new(LiquidBackend::new(client.clone())),
                spark_backend.clone(),
            ),
            None => crate::app::wallets::WalletRegistry::vault_only(),
        };
        // Load bitcoin_unit and display_mode from settings if available
        let network_dir = datadir.network_directory(cube_settings.network);
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
            has_imported_bitcoin: cube_settings.bitcoin_import.is_some(),
            bitcoin_unit,
            fiat_chain: cube_settings.network,
            display_mode: if cube_settings.network.is_blake2b() {
                settings::display::DisplayMode::BitcoinNative
            } else {
                display_mode
            },
            cube_name: cube_settings.name.clone(),
            wallet_names: cache::WalletNames::new(&cube_settings, None),
            cube_connect_owner: cube_settings.connect_owner.clone(),
            cube_owned_by_other_account: false,
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
            cube_settings.network,
            cube_settings.id.clone(),
            cube_settings.name.clone(),
            cube_settings.api_network_string(),
            cube_settings.bitcoin_import.clone(),
        );
        panels.global_home.imported_installed = cube_settings
            .bitcoin_import
            .as_deref()
            .and_then(settings::BitcoinImport::active_attachment)
            .is_some();
        if cube_settings.bitcoin_import_pending() {
            panels.current = Menu::Vault(menu::VaultSubMenu::Overview);
        }
        // See the sibling assignments in `App::new` (PRs D2/D3).
        panels.connect.account.bitcoin_blake2b_opt_in =
            settings::global::GlobalSettings::load_bitcoin_blake2b_beta(
                &settings::global::GlobalSettings::path(&datadir),
            );
        panels
            .connect
            .set_cube_encryption_pubkey(cube_settings.connect_encryption_pubkey.clone());
        panels
            .connect
            .set_vault_fingerprint(cube_settings.vault_fingerprint.clone());
        panels
            .connect
            .set_spark_stable_balance(cube_settings.spark_stable_balance);
        let mut cache = cache;
        if fork {
            cache.clear_btcb2_fiat();
        }
        cache.cube_encryption_key = cube_encryption_key;
        cache.connect_transport_key =
            load_connect_transport_key(&datadir.network_directory(cube_settings.network));
        cache.has_p2p = panels.p2p.is_some();
        // See the sibling assignment in `App::new`: probe the on-disk Liquid
        // state (not the live connection) for the "wallet exists on this machine"
        // half of the sunset gate — a backend-gated network returns the SDK
        // disconnected even for a funded on-disk wallet, which would wrongly
        // hide the rail.
        cache.liquid_gate.local_state_exists =
            !fork && crate::app::breez_liquid::local_state_exists(datadir.path(), network);
        panels.show_liquid(cache.liquid_gate);
        cache.p2p_test_coordinator = panels
            .p2p
            .as_ref()
            .is_some_and(|p| p.has_test_coordinator());

        let cmd = iced::Task::batch([
            panels.connect.ensure_session_check(),
            panels.global_home.reload(None, None),
            // The Overview opens first; its "Recent activity" reads Activity.
            panels
                .activity
                .refresh_scope(view::activity::RecentScope::Cube, None, None),
            Task::done(Message::ImportedBitcoin(Box::new(
                state::imported_bitcoin::ImportEvent::Start,
            ))),
        ]);

        (
            Self {
                transfer_destination: None,
                panels: Box::new(panels),
                cache,
                daemon: None,
                wallet: None,
                local_mcp: Default::default(),
                imported_kit_metadata: Default::default(),
                breez_client,
                fork_connect_client: None,
                local_feature_valid: true,
                local_feature_checked_at: None,
                local_feature_check: None,
                wallet_registry,
                internal_bitcoind: None,
                cube_settings,
                config: config_arc,
                datadir,
                errors: Vec::with_capacity(8),
                current_error_id: 256,
                bitcoind_sync_probe_in_progress: false,
                node_net_stats_probe_in_progress: false,
                node_history_in_progress: false,
                node_history_pending: None,
                node_history_epoch: 0,
                node_history_request_epoch: 0,
                claim_session_invalidated: false,
                claim_hold_epoch: 0,
                daemon_switch_in_progress: false,
                background_node_attempt: None,
                rescan_obligation_retired_by_scan: false,
                rescan_obligation_retire_in_flight: false,
                auto_switch_suppressed: false,
                local_switch_hold: None,
                entangled_in_flight: HashSet::new(),
                chain_proofs_in_flight: HashSet::new(),
                split_refresh_issued: 0,
                split_refresh_applied: 0,
                twin_checks: std::collections::HashMap::new(),
                twin_records: Vec::new(),
                chain_proofs_loaded: false,
                unswept_in_flight: None,
                unswept_session: None,
                seen_session: None,
                show_received_celebration: false,
                show_recovery_alerts_prompt: false,
                spark_stable_balance_reconciled: false,
                spark_nickname_in_flight: None,
                spark_nickname_queued: None,
                received_celebration_amount: String::new(),
                received_celebration_context: "transaction-received".to_string(),
                received_celebration_wallet: "",
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
                connect_renewal: Renewal::default(),
                connect_stream_restart: 0,
                in_app_otp_cache_snapshot: None,
                in_app_restore_cache_snapshot: None,
                in_app_refresh_commit: None,
                deferred_connect_stream_triggers: Vec::new(),
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

    pub fn breez_client(&self) -> Option<Arc<BreezClient>> {
        self.breez_client.clone()
    }

    pub fn spark_backend(&self) -> Option<Arc<crate::app::wallets::SparkBackend>> {
        self.wallet_registry.spark().cloned()
    }

    /// A loaded Spark or Liquid balance is not zero. Deleting the Cube
    /// would leave those funds without it.
    pub fn holds_loaded_wallet_funds(&self) -> bool {
        self.panels.global_home.holds_loaded_funds()
    }

    #[cfg(test)]
    pub(crate) fn set_loaded_spark_balance_for_test(&mut self, sats: u64) {
        self.panels
            .global_home
            .set_loaded_spark_balance_for_test(sats);
    }

    /// Returns a clone of the authenticated coincube-api client (with JWT set),
    /// or `None` if the user has not logged in yet.
    pub fn authenticated_coincube_client(
        &self,
    ) -> Option<crate::services::coincube::CoincubeClient> {
        self.panels.connect.account.authenticated_client()
    }

    /// An account-panel completion for the same admitted fork session is not
    /// a global auth change. A new token or account still crosses that boundary.
    pub(crate) fn matches_admitted_connect_session(
        &self,
        login: &crate::services::coincube::LoginResponse,
    ) -> bool {
        self.requires_connect()
            && self.fork_connect_client.as_ref().is_some_and(|bound| {
                bound.token() == Some(login.token.as_str())
                    && self
                        .panels
                        .connect
                        .account
                        .user
                        .as_ref()
                        .is_none_or(|user| user.id == login.user.id)
            })
    }

    /// Refresh positive, session-bound recovery display evidence after sync.
    /// Ineligibility withdraws the current notice; a refresh keeps it until
    /// its callback replaces it, so each sync tick does not blank the line.
    /// Display stays gated by [`unswept_notice::Notice::visible`].
    fn unswept_notice_task(&mut self) -> Task<Message> {
        let previous = self.cache.unswept_notice.take();
        if self.cache.chain() != crate::chain::ChainId::Bitcoin || !self.cache.btcb2_server_enabled
        {
            return Task::none();
        }
        if self.unswept_in_flight.is_some() {
            self.cache.unswept_notice = previous;
            return Task::none();
        }
        let Some(wallet) = self.wallet.as_ref() else {
            return Task::none();
        };
        let Some(session) = self.claim_connect_session() else {
            return Task::none();
        };
        let client = session.client.clone();
        if !wallet.supports_inheritance() {
            return Task::none();
        }
        let outputs = unswept_notice::owned(&self.cache, &wallet.main_descriptor);
        if outputs.is_empty() {
            return Task::none();
        }
        let generation = *self.panels.session_generation.borrow();
        let live = self.panels.session_generation.subscribe();
        self.unswept_in_flight = Some(generation);
        self.unswept_session = Some(session);
        self.cache.unswept_notice = previous;
        let app = self.cache.app_generation;
        Task::perform(
            crate::services::foreign_scan::known::known_unspent(client, outputs, generation, live),
            move |result| Message::UnsweptNotice {
                app,
                generation,
                result,
            },
        )
    }

    /// Presence badges for either side of the fork, through Connect only.
    /// Bitcoin-side lookups require the BTCB2 opt-in. Missing transactions
    /// remain unverified; only the Claim ancestry check supplies proof.
    /// Runs independently of sync and signing.
    fn entangled_lookup_task(&mut self) -> Task<Message> {
        let Some(wallet) = self.wallet.as_ref() else {
            return Task::none();
        };
        if crate::services::entangled::twin_chain(wallet.chain).is_none()
            || (!wallet.chain.is_blake2b()
                && !self.panels.connect.account.bitcoin_blake2b_enabled())
        {
            return Task::none();
        }
        let Some(client) = self.authenticated_coincube_client() else {
            return Task::none();
        };
        let pending = crate::services::entangled::pending_lookups(
            self.cache.coins().iter().map(|coin| coin.outpoint.txid),
            &self.cache.entangled,
            &self.entangled_in_flight,
            std::time::Instant::now(),
        );
        if pending.is_empty() {
            return Task::none();
        }
        self.entangled_in_flight.extend(pending.iter().copied());
        let chain = wallet.chain;
        let origin = cache::LookupOrigin {
            app: self.cache.app_generation,
            chain,
        };
        let claimed = pending.clone();
        Task::perform(
            crate::services::entangled::lookup_all(client, chain, pending),
            move |answers| Message::EntangledLookups {
                origin,
                claimed: claimed.clone(),
                answers,
            },
        )
    }

    /// The Vault's data directory, where its Chains-column results are saved.
    fn vault_data_dir(&self) -> Option<std::path::PathBuf> {
        let wallet = self.wallet.as_ref()?;
        Some(
            self.cache
                .datadir_path
                .network_directory(wallet.chain)
                .coincubed_data_directory(&wallet.id())
                .path()
                .to_path_buf(),
        )
    }

    /// Ancestry checks for the Chains column: only coins whose funding
    /// transaction the twin chain lacks, a few per pass, whenever Bitcoin
    /// Blake2b is on ([`Self::blake2b_on`]). Saved results are read in on the
    /// first pass.
    fn chain_proof_task(&mut self) -> Task<Message> {
        let Some(wallet) = self.wallet.as_ref() else {
            return Task::none();
        };
        let chain = wallet.chain;
        if crate::services::entangled::twin_chain(chain).is_none() || !self.blake2b_on() {
            return Task::none();
        }
        if !self.chain_proofs_loaded {
            self.chain_proofs_loaded = true;
            if let Some(dir) = self.vault_data_dir() {
                let saved = crate::services::chain_status::store::load(&dir);
                for (outpoint, entry) in saved {
                    self.cache.chain_proofs.entry(outpoint).or_insert(entry);
                }
                self.cache.split_records = crate::services::split_coins::journal::load(&dir, chain);
            }
        }
        if !crate::services::chain_status::checks_enabled(chain) {
            return Task::none();
        }
        let Some(client) = self.authenticated_coincube_client() else {
            return Task::none();
        };
        let pending = crate::services::chain_status::proof_candidates(
            self.cache.coins().iter().map(|coin| coin.outpoint),
            |txid| self.cache.entanglement_of(txid),
            &self.cache.chain_proofs,
            &self.chain_proofs_in_flight,
            std::time::SystemTime::now(),
        );
        if pending.is_empty() {
            return Task::none();
        }
        self.chain_proofs_in_flight.extend(pending.iter().copied());
        let origin = cache::LookupOrigin {
            app: self.cache.app_generation,
            chain,
        };
        let generation = *self.panels.session_generation.borrow();
        let live = self.panels.session_generation.subscribe();
        let claimed = pending.clone();
        Task::perform(
            crate::services::chain_status::check_ancestry(client, chain, pending, generation, live),
            move |results| Message::ChainProofs {
                origin,
                claimed: claimed.clone(),
                results,
            },
        )
    }

    /// Refresh the Chains column's split facts: read the daemon's copies of
    /// the recorded split transactions, then derive *Splitting*, the split
    /// outputs' proofs and the coins a split separated.
    fn split_refresh_task(&mut self) -> Task<Message> {
        let (Some(wallet), Some(daemon)) = (self.wallet.as_ref(), self.daemon.clone()) else {
            return Task::none();
        };
        if !self.chain_proofs_loaded {
            return Task::none();
        }
        let origin = cache::LookupOrigin {
            app: self.cache.app_generation,
            chain: wallet.chain,
        };
        let txids: Vec<bitcoin::Txid> = self
            .cache
            .split_records
            .iter()
            .map(|record| record.txid)
            .collect();
        let chain = wallet.chain;
        let twin_records = crate::services::split_coins::journal::twin_data_dir(
            &self.cache.datadir_path,
            chain,
            &wallet.descriptor_checksum,
        )
        .zip(crate::services::entangled::twin_chain(chain))
        .map(|(dir, twin)| crate::services::split_coins::journal::load(&dir, twin))
        .unwrap_or_default();
        // The twin journal only nominates splits; Connect must show each one
        // deep on the twin chain now before its coins count as separated.
        let candidates =
            crate::services::split_coins::twin_candidates(&twin_records, &self.twin_checks);
        self.split_refresh_issued += 1;
        let seq = self.split_refresh_issued;
        let client = (!candidates.is_empty()
            && self.blake2b_on()
            && crate::services::chain_status::checks_enabled(chain))
        .then(|| self.authenticated_coincube_client())
        .flatten();
        let generation = *self.panels.session_generation.borrow();
        let live = self.panels.session_generation.subscribe();
        Task::perform(
            async move {
                // Stamped before any read, so a slow pass never stretches
                // how long its checks count.
                let twin_checked_at = std::time::Instant::now();
                let transactions = async {
                    if txids.is_empty() {
                        return std::collections::HashMap::new();
                    }
                    daemon
                        .list_txs(&txids)
                        .await
                        .map(|found| {
                            found
                                .transactions
                                .into_iter()
                                .map(|info| (info.tx.compute_txid(), info.tx))
                                .collect()
                        })
                        .unwrap_or_default()
                };
                // A nominated split this pass could not look up is answered
                // as not separated, never left on an older positive.
                let confirmed = async {
                    match client {
                        Some(client) => {
                            crate::services::split_coins::twin_confirmed(
                                client, chain, candidates, generation, live,
                            )
                            .await
                        }
                        None => candidates
                            .into_iter()
                            .map(|(txid, _)| (txid, None))
                            .collect(),
                    }
                };
                let (transactions, twin_checked) = tokio::join!(transactions, confirmed);
                (transactions, twin_records, twin_checked, twin_checked_at)
            },
            move |(transactions, twin_records, twin_checked, twin_checked_at)| {
                Message::SplitTransactions {
                    origin,
                    transactions,
                    seq,
                    twin_records,
                    twin_checked,
                    twin_checked_at,
                }
            },
        )
    }

    /// Record a split this Cube just built and stored with the daemon, so the
    /// Chains column follows it.
    pub(crate) fn record_split(
        &mut self,
        record: crate::services::split_coins::SplitRecord,
    ) -> Task<Message> {
        self.cache.split_records.push(record);
        self.save_split_records();
        self.split_refresh_task()
    }

    fn save_split_records(&self) {
        if let Some(dir) = self.vault_data_dir() {
            if let Err(e) =
                crate::services::split_coins::journal::save(&dir, &self.cache.split_records)
            {
                tracing::warn!("couldn't save split records: {}", e);
            }
        }
    }

    fn save_chain_proofs(&self) {
        if let Some(dir) = self.vault_data_dir() {
            let held: HashSet<bitcoin::OutPoint> = self
                .cache
                .coins()
                .iter()
                .map(|coin| coin.outpoint)
                .collect();
            if let Err(e) = crate::services::chain_status::store::save(
                &dir,
                &self.cache.chain_proofs,
                |outpoint| held.contains(outpoint),
            ) {
                tracing::warn!("couldn't save Chains-column results: {}", e);
            }
        }
    }

    /// The chain of this Cube's Vault, `None` without one — what a lookup
    /// reply's [`cache::LookupOrigin::chain`] is compared with.
    fn vault_chain(&self) -> Option<crate::chain::ChainId> {
        self.wallet.as_ref().map(|wallet| wallet.chain)
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
        // The existing recovery-alert contract only accepts Bitcoin-family
        // network IDs; never report a fork Vault under Bitcoin mainnet.
        if self.cache.chain().is_blake2b()
            || !self
                .wallet
                .as_ref()
                .is_some_and(|wallet| wallet.supports_inheritance())
        {
            return Task::none();
        }
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
        let Some(timelock) = wallet.recovery_timelock().map(i64::from) else {
            return Task::none();
        };
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
        // Cloned before the panels are borrowed mutably below. The reconcile
        // needs the descriptor itself, not just its fingerprint: it decides
        // which registered cube keys this Vault actually commits to.
        let Some(descriptor) = wallet.inheritance_descriptor().cloned() else {
            return Task::none();
        };
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
        // Same wave, same preconditions (registered Cube + loaded Vault): heal
        // any Vault member row the installer's fan-out failed to write, so a
        // Cube whose Keychain app reports "no vault" for one of its own keys
        // converges at open rather than waiting for someone to start a desktop
        // sign (COIN-373; `services::coincube::vault_reconcile`).
        self.panels.connect.set_vault_descriptor(Some(descriptor));
        let reconcile_task = self.panels.connect.reconcile_vault_members();
        if !self.cube_settings.adopt_vault_fingerprint(&fingerprint) {
            return Task::batch([assert_task, reconcile_task]);
        }
        let network_dir = self
            .cache
            .datadir_path
            .network_directory(self.cache.chain());
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
        Task::batch([persist_task, assert_task, reconcile_task])
    }

    /// Record the user's Stable Balance decision for this Cube — in memory,
    /// in the settings file, and on the Cube's Connect record so the owner's
    /// other desktops pick it up. See `CubeSettings::spark_stable_balance`
    /// for when this is called. Any Stable Balance banner is cleared: a
    /// decision supersedes the prompt.
    fn record_spark_stable_balance(&mut self, enabled: bool) -> Task<Message> {
        Task::batch([
            self.record_spark_stable_balance_locally(enabled),
            self.panels
                .connect
                .cube
                .report_spark_stable_balance(enabled),
        ])
    }

    /// The local half of [`Self::record_spark_stable_balance`]: memory,
    /// settings file, banner. Used on its own when the decision *came from*
    /// Connect, where pushing it back would be a pointless round trip.
    fn record_spark_stable_balance_locally(&mut self, enabled: bool) -> Task<Message> {
        self.cube_settings.spark_stable_balance = Some(enabled);
        self.cache.spark_notice = None;
        let network_dir = self
            .cache
            .datadir_path
            .network_directory(self.cache.chain());
        let cube_id = self.cube_settings.id.clone();
        Task::perform(
            async move {
                settings::update_settings_file(&network_dir, |mut s| {
                    if let Some(cube) = s.cubes.iter_mut().find(|c| c.id == cube_id) {
                        cube.spark_stable_balance = Some(enabled);
                    }
                    Some(s)
                })
                .await
                .map_err(|e| e.to_string())
            },
            |res: Result<(), String>| {
                if let Err(e) = res {
                    // Not a user-facing error: the SDK holds the live
                    // setting; this copy only matters at the next launch.
                    log::warn!("failed to persist Spark Stable Balance preference: {e}");
                }
                Message::SettingsSaved
            },
        )
    }

    /// One-shot read of the Spark SDK's Stable Balance state and USDB
    /// holding, for [`reconcile_stable_balance`]. Runs after the session's
    /// first `Synced` so the holding is current.
    fn spark_stable_balance_state_task(&self) -> Task<Message> {
        let Some(backend) = self.spark_backend() else {
            return Task::none();
        };
        Task::perform(
            async move {
                let settings = backend.get_user_settings().await?;
                let info = backend.get_info().await?;
                let holds_usdb = info.stable_balance.is_some_and(|sb| sb.balance > 0);
                Ok::<_, crate::app::breez_spark::SparkClientError>((
                    settings.stable_balance_active,
                    holds_usdb,
                ))
            },
            |res| match res {
                Ok(state) => Message::SparkStableBalanceState(Some(state)),
                Err(e) => {
                    log::warn!("skipping Spark Stable Balance reconcile: {e}");
                    Message::SparkStableBalanceState(None)
                }
            },
        )
    }

    /// This tab's Connect account panel's authentication epoch
    /// (`ConnectAccountPanel::auth_epoch`): the GUI reads it before it
    /// treats a `SetSession` completion from this tab as a sign-in.
    pub fn connect_auth_epoch(&self) -> u64 {
        self.panels.connect.account.auth_epoch()
    }

    /// Retained account state rendered by the GUI's account-wide Connect sheet.
    pub(crate) fn connect_sheet_account(&self) -> &state::connect::ConnectAccountPanel {
        &self.panels.connect.account
    }

    pub(crate) fn connect_sheet_account_mut(&mut self) -> &mut state::connect::ConnectAccountPanel {
        &mut self.panels.connect.account
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

    pub(crate) fn reload_global_settings(&mut self) -> Task<Message> {
        self.panels.connect.account.bitcoin_blake2b_opt_in =
            settings::global::GlobalSettings::load_bitcoin_blake2b_beta(
                &settings::global::GlobalSettings::path(&self.datadir),
            );
        self.cache.btcb2_server_enabled = self.panels.connect.account.bitcoin_blake2b_enabled();
        if !self.cache.btcb2_server_enabled {
            self.revoke_claim();
        }
        Task::none()
    }

    pub fn datadir(&self) -> &CoincubeDirectory {
        &self.datadir
    }

    /// Whether the Spark wallet has settled: it returned a synced balance,
    /// or this Cube has none. The opening waits on it.
    pub fn spark_settled(&self) -> bool {
        self.panels.global_home.spark_settled()
    }

    pub fn cube_settings(&self) -> &settings::CubeSettings {
        &self.cube_settings
    }

    /// The signed-out-by-Connect notice, as the GUI relays it to every tab.
    pub fn set_connect_signed_out_by(&mut self, reason: Option<SignedOutBy>) {
        self.cache.connect_signed_out_by = reason;
        self.panels.connect.account.signed_out_by = reason;
    }

    pub(crate) fn dismiss_shell_disclosures(&mut self) {
        self.cache.cube_switcher_open = false;
        self.cache.cube_lock_confirmation = false;
        self.cache.connect_panel_open = false;
    }

    #[cfg(test)]
    pub(crate) fn reviewer_show_seed_phrase_for_test(&mut self) {
        self.panels.current = Menu::Cube(crate::app::menu::CubeSubMenu::Settings(
            crate::app::menu::CubeSettingsOption::Recovery,
        ));
        self.panels
            .global_settings
            .reviewer_show_seed_phrase_for_test(&self.datadir);
    }

    /// The Connect session account-scoped reads (the unswept-coins notice)
    /// work under: the account's authenticated client and its id. `None`
    /// until the account is signed in and known, and while a sign-out hold
    /// is in force.
    fn claim_connect_session(&self) -> Option<crate::app::connect_session::ConnectSession> {
        if self.claim_session_invalidated || !self.panels.connect.account.bitcoin_blake2b_enabled()
        {
            return None;
        }
        let client = self.authenticated_coincube_client()?;
        let account = self.panels.connect.account.user.as_ref()?.id.to_string();
        Some(crate::app::connect_session::ConnectSession { client, account })
    }

    /// Drop every Connect-session read and advance the session generation.
    /// Called before the context those reads were started under is replaced:
    /// Connect sign-out or account change (from this tab or, through
    /// `Tab::invalidate_fork_session`, any other), a node backend switch,
    /// Cube lock, Cube/tab close. In-flight reads check the generation.
    pub fn revoke_claim(&mut self) {
        self.cache.unswept_notice = None;
        self.unswept_in_flight = None;
        self.unswept_session = None;
        self.panels
            .session_generation
            .send_modify(|generation| *generation = generation.wrapping_add(1));
    }

    /// Hold the session: the generation advances, the account panel's
    /// authentication epoch advances and is recorded as the hold's, and the
    /// account this tab's Connect panel still shows stops counting as a
    /// session until an operation begun here after this completes (see
    /// `claim_session_invalidated`). The Bitcoin wallet itself stays open.
    fn hold_claim(&mut self) {
        self.revoke_claim();
        self.claim_hold_epoch = self.panels.connect.account.invalidate_auth();
        self.claim_session_invalidated = true;
    }

    /// The global Connect auth boundary for a log-out in any tab: hold.
    pub fn invalidate_claim_session(&mut self) {
        self.hold_claim();
    }

    /// A retained client behind a global sign-out/account-change hold must not
    /// seed a new tab, even while the Bitcoin wallet itself remains open.
    pub(crate) fn shareable_connect_client(
        &self,
    ) -> Option<crate::services::coincube::CoincubeClient> {
        if self.claim_session_invalidated {
            None
        } else {
            self.authenticated_coincube_client()
        }
    }

    /// Only a completed, unheld account session can seed a cold PIN candidate.
    /// The pending PIN still rechecks the expected user before decrypting.
    pub(crate) fn shareable_connect_identity(
        &self,
    ) -> Option<(u32, crate::services::coincube::CoincubeClient)> {
        if !self.panels.connect.account.is_authenticated() {
            return None;
        }
        Some((
            self.panels.connect.account.user.as_ref()?.id,
            self.shareable_connect_client()?,
        ))
    }

    /// An ordinary Bitcoin Cube can reuse a verified sibling session at startup.
    /// Fork daemons keep their admitted credentials, and an explicit auth hold
    /// must not be lifted by discovering an old sibling client.
    pub(crate) fn can_adopt_shared_connect_session(&self) -> bool {
        !self.cache.chain().is_blake2b()
            && !self.claim_session_invalidated
            && matches!(
                self.panels.connect.account.step,
                state::connect::account::ConnectFlowStep::CheckingSession
                    | state::connect::account::ConnectFlowStep::Login { loading: false, .. }
            )
            && self.panels.connect.account.can_adopt_shared_session()
    }

    pub(crate) fn adopt_shared_connect_client(
        &mut self,
        client: crate::services::coincube::CoincubeClient,
    ) -> Task<Message> {
        self.panels.connect.account.invalidate_auth();
        self.panels.connect.account.install_shared_client(client);
        self.update(Message::View(view::Message::ConnectAccount(
            view::ConnectAccountMessage::Init,
        )))
    }

    /// The global Connect auth boundary for a Bitcoin App, as broadcast by
    /// `GUI::update` to every tab (`Tab::invalidate_fork_session`).
    /// - A log-out from any tab holds the claim (`invalidate_claim_session`).
    /// - A sign-in this tab originated is its own hook's business
    ///   (`claim_replaced`, and the epoch-correlated lift): nothing here.
    /// - A sibling sign-in never lifts an existing hold, and adds nothing to it.
    /// - A sibling sign-in or refresh of the *same* Connect user as this
    ///   tab's account leaves this tab's session standing: the claim is
    ///   revoked for a re-read (its coordinator was bound before that
    ///   sign-in) and re-binds under this tab's session on the next Refresh
    ///   or entry — no sign-out copy, no log-out needed.
    /// - A sibling sign-in of another account, or while this tab has none,
    ///   holds the claim, and says why.
    pub fn on_global_auth_change(&mut self, change: crate::gui::tab::AuthChange, originated: bool) {
        use crate::gui::tab::AuthChange;
        match change {
            AuthChange::LogOut => self.invalidate_claim_session(),
            AuthChange::SignIn { .. } if originated => {}
            AuthChange::SignIn { .. } if self.claim_session_invalidated => {}
            AuthChange::SignIn { user_id, .. } => {
                let mine = self.panels.connect.account.user.as_ref().map(|u| u.id);
                if mine == Some(user_id) {
                    self.revoke_claim();
                } else {
                    self.hold_claim();
                }
            }
        }
    }

    /// This Cube as a candidate Bitcoin Blake2b claim source — the single
    /// input to [`crate::app::features::claim_blake2b`], so the Vault rail and
    /// the message handler cannot drift apart.
    /// Whether Bitcoin Blake2b is on for this Cube: a Bitcoin Blake2b Cube,
    /// or any other Cube whose account has the Bitcoin Blake2b grant and
    /// opt-in. The same answer that shows the Chains column and Tools ›
    /// Split coins; it is what lets ancestry checks run and a split be built.
    pub(crate) fn blake2b_on(&self) -> bool {
        self.cache.chain().is_blake2b() || self.panels.connect.account.bitcoin_blake2b_enabled()
    }

    /// Re-read from the disk whether this Bitcoin Cube's Vault already has a
    /// Bitcoin Blake2b copy (`Cache::btcb2_already_claimed`).
    pub(crate) fn refresh_copy_exists(&mut self) {
        self.cache.btcb2_already_claimed = self.cube_settings.network
            == crate::chain::ChainId::Bitcoin
            && self.wallet.as_ref().is_some_and(|wallet| {
                wallet.supports_inheritance()
                    && claim_target_exists(&self.datadir, &wallet.descriptor_checksum)
            });
    }

    pub(crate) fn claim_source_cube(&self) -> crate::app::features::ClaimSourceCube {
        crate::app::features::ClaimSourceCube {
            chain: self.cube_settings.network,
            has_vault: self
                .wallet
                .as_ref()
                .is_some_and(|wallet| wallet.supports_inheritance()),
            server_enabled: self.cache.btcb2_server_enabled,
            already_claimed: self.cache.btcb2_already_claimed,
        }
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Clear this Vault's recorded rescan obligation. See
    /// [`full_scan_settles_rescan_obligation`] for when that is right.
    fn retire_rescan_obligation_task(&self) -> Option<Task<Message>> {
        let wallet = self.wallet.as_ref()?;
        let network_dir = self
            .cache
            .datadir_path
            .network_directory(self.cache.chain());
        let descriptor_checksum = wallet.descriptor_checksum.clone();
        Some(Task::perform(
            async move {
                settings::update_settings_file(&network_dir, |settings| {
                    Some(cleared_pending_rescan(settings, &descriptor_checksum))
                })
                .await
                .map_err(|e| e.to_string())
            },
            Message::RescanObligationRetired,
        ))
    }

    fn daemon_backend(&self) -> DaemonBackend {
        self.daemon
            .as_ref()
            .map(|d| d.backend())
            // No Vault daemon: claim nothing about local sync progress.
            .unwrap_or(DaemonBackend::ExternalCoincubed)
    }

    /// The Vault scan a backend switch would discard, if one is running.
    fn running_vault_scan(&self) -> Option<local_switch::RunningScan> {
        running_vault_scan(self.daemon_backend(), &self.cache)
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
            .network_directory(self.cache.chain());
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

    /// On a wallet page the pane belongs to a row it shows: once a load moves
    /// the selected row out of the five, nothing is selected, so it cannot
    /// reopen by itself when a later load brings the row back.
    fn settle_wallet_selection(&mut self) {
        let Some(wallet) = wallet_page(&self.panels.current) else {
            return;
        };
        let Some(selected) = self.panels.activity.selected_id.clone() else {
            return;
        };
        let shown = self
            .panels
            .activity
            .recent_entries(view::activity::RecentScope::Wallet(wallet))
            .into_iter()
            .any(|entry| entry.id == selected);
        if !shown {
            self.panels.activity.clear_selection();
        }
    }

    fn set_current_panel(&mut self, menu: Menu) -> Task<Message> {
        if menu == Menu::Vault(menu::VaultSubMenu::SplitCoins) {
            // A copy may have been created (or removed) since this Cube
            // opened, by this App's copy installer or another instance.
            self.refresh_copy_exists();
        }
        // A bare Vault Settings route has no distinct landing page. Resolve it
        // before imported-wallet dispatch and panel reload so the content and
        // selected in-content section both become Wallet & signers.
        let menu = match menu {
            Menu::Vault(menu::VaultSubMenu::Settings(None)) => Menu::Vault(
                menu::VaultSubMenu::Settings(Some(menu::SettingsOption::Wallet)),
            ),
            menu => menu,
        };
        if self.cache.has_imported_bitcoin {
            if matches!(
                &menu,
                Menu::Vault(
                    menu::VaultSubMenu::Recovery
                        | menu::VaultSubMenu::Settings(Some(menu::SettingsOption::LocalSigning))
                )
            ) {
                return Task::none();
            }
            if self
                .panels
                .imported_bitcoin
                .as_ref()
                .is_some_and(|panel| panel.handles_route(&menu))
            {
                // Leaving a panel for the imported wallet's route interrupts
                // it like any other route change (Transfer locks another Cube).
                if let Some(panel) = self.panels.current_mut() {
                    panel.interrupt();
                }
                self.panels.current = menu.clone();
                return self
                    .panels
                    .imported_bitcoin
                    .as_mut()
                    .map_or_else(Task::none, |panel| panel.enter_route(&menu));
            }
        }
        if self.cache.chain().is_blake2b()
            && matches!(
                menu,
                Menu::Liquid(_) | Menu::Spark(_) | Menu::Marketplace(_)
            )
        {
            return Task::none();
        }
        if let Some(panel) = self.panels.current_mut() {
            panel.interrupt();
        }
        if matches!(
            menu,
            Menu::Spark(menu::SparkSubMenu::Overview) | Menu::Vault(menu::VaultSubMenu::Overview)
        ) {
            // The page's Transfer needs to know whether another Cube can
            // receive: one catalog read per visit, not per frame.
            self.panels
                .global_home
                .refresh_other_cube_receivers(&self.cache);
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
                    menu::CubeSettingsOption::PaymentChecks => {
                        Some(view::SettingsMessage::PaymentChecksSection)
                    }
                    menu::CubeSettingsOption::DataExports => {
                        Some(view::SettingsMessage::DataExportsSection)
                    }
                    menu::CubeSettingsOption::Connections => {
                        Some(view::SettingsMessage::ConnectionsSection)
                    }
                    menu::CubeSettingsOption::About => Some(view::SettingsMessage::AboutSection),
                    menu::CubeSettingsOption::Stats => {
                        Some(view::SettingsMessage::InstallStatsSection)
                    }
                    // Avatar / Members render from `ConnectCubePanel` via
                    // App::view; no section message is dispatched to the
                    // SettingsState. Side-effect loads (avatar fetch,
                    // members fetch) are kicked below.
                    menu::CubeSettingsOption::AiAccess
                    | menu::CubeSettingsOption::Avatar
                    | menu::CubeSettingsOption::Members => None,
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
                        // Display & appearance stays underneath the flow.
                        let daemon = self.daemon.clone();
                        let display = self.panels.global_settings.update(
                            daemon,
                            &self.cache,
                            Message::View(view::Message::Settings(
                                view::SettingsMessage::GeneralSection,
                            )),
                        );
                        return Task::batch([
                            display,
                            iced::Task::done(Message::View(view::Message::ConnectCube(
                                view::ConnectCubeMessage::Avatar(view::AvatarMessage::Enter),
                            ))),
                        ]);
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
                        // Connections renders from the App (see
                        // `connect_settings_content`); no section to install.
                        menu::VaultSubMenu::Settings(Some(menu::SettingsOption::Connections)) => {
                            self.panels.current = menu.clone();
                            return Task::none();
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
                                            menu::SettingsOption::LocalSigning
                                            | menu::SettingsOption::Connections => {
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
                                wallet.recovery_timelock(),
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
                                    self.cache.history_sync(),
                                ),
                                self.cache.bitcoin_unit,
                            ));
                            // A refresh is a self-send spend, so show the
                            // panel we just built. Falling through would
                            // leave `current` on `Coins(..)`, which routes
                            // back to the coins list — the accordion would
                            // just collapse and nothing else would happen.
                            self.panels.current = Menu::Vault(menu::VaultSubMenu::Send);
                            if let Some(panel) = self.panels.current_mut() {
                                return panel.reload(self.daemon.clone(), self.wallet.clone());
                            }
                            return Task::none();
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
                                        wallet.recovery_timelock(),
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
                                            self.cache.history_sync(),
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
                                    self.cache.history_sync(),
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

        // A row chosen on a wallet page is that page's: neither another page
        // nor the next visit opens with it selected.
        if wallet_page(&self.panels.current).is_some() || wallet_page(&menu).is_some() {
            self.panels.activity.clear_selection();
        }
        self.panels.current = menu.clone();

        // A page with "Recent activity" re-reads its rows on entry: a failed
        // read then shows with its Retry even if the page's own reload fails.
        // Spark's page also follows each later page reload
        // (`SparkOverviewMessage::DataLoaded`), and the Overview follows
        // Spark's payment events and the Vault's coins (`overview_activity_*`).
        let recent = recent_scope(&menu).map_or_else(Task::none, |scope| {
            self.panels
                .activity
                .refresh_scope(scope, self.daemon.clone(), self.wallet.clone())
        });

        // Call reload with optional daemon/wallet
        // Liquid panels don't need them (use BreezClient), Vault panels do
        let reload = if let Some(panel) = self.panels.current_mut() {
            panel.reload(self.daemon.clone(), self.wallet.clone())
        } else {
            Task::none()
        };
        Task::batch([reload, recent])
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let mut subscriptions = vec![];

        // Always subscribe to Breez events (handles fee acceptance globally)
        if let Some(client) = &self.breez_client {
            subscriptions.push(client.subscription().map(Message::BreezEvent));
        }

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
                        self.cache.history_sync(),
                    ) {
                        SyncStatus::BlockchainSync(_) => 5, // Only applies to local backends
                        SyncStatus::WalletFullScan { .. } | SyncStatus::LatestWalletSync => 3,
                        // Polls are being retried; refresh often enough to notice recovery.
                        SyncStatus::SyncFailing { .. } => 5,
                        // For the rescan feature, we refresh often enough to give
                        // user an up-to-date view of the rescan progress.
                        SyncStatus::Synced => 10,
                    },
                ))
                .map(|_| Message::Tick),
            );
        }

        // Poll pending or active local Bitcoind progress on a fixed interval,
        // independent of the variable-rate tick subscription.
        if self
            .daemon
            .as_ref()
            .and_then(|d| d.config())
            .and_then(local_node_sync_config)
            .is_some()
        {
            subscriptions
                .push(time::every(BITCOIND_SYNC_POLL_INTERVAL).map(|_| Message::PollBitcoindSync));
            subscriptions.push(
                time::every(Duration::from_secs(5))
                    .map(|_| Message::NodeHistory(node_history::Action::Poll)),
            );
        }

        // Current panel's subscription
        let current_subscription = self
            .panels
            .current()
            .unwrap_or(&self.panels.global_home)
            .subscription();
        subscriptions.push(current_subscription);
        subscriptions.extend(self.panels.global_home.spark_load_poll());

        if let Some(imported) = self.panels.imported_bitcoin.as_ref() {
            if !imported.handles_route(&self.panels.current) {
                subscriptions.push(imported.subscription());
            }
        }

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
        // the active managed node, so sync progress on a node that is already live,
        // with nothing pending, is visible too.
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
        // keyed on `(device_id, grpc_url)`; the stream tracks its event cursor
        // without restarting the subscription for each received event.
        // A stored Connect session that couldn't be checked is checked again.
        if let Some(unverified) = self.panels.connect.account.unverified.as_ref() {
            subscriptions.push(unverified.recheck().map(|_| {
                Message::View(view::Message::ConnectAccount(
                    view::ConnectAccountMessage::RecheckSession,
                ))
            }));
        }
        // The Connect light pulses while connecting, reconnecting or
        // renewing, unless the user reduces motion.
        if !self.cache.reduce_motion
            && matches!(
                self.cache.connect_stream_status.light(),
                Some(ConnectLight::Connecting | ConnectLight::Reconnecting)
            )
        {
            subscriptions.push(time::every(CONNECT_LIGHT_PULSE).map(|_| Message::ConnectLightTick));
        }
        if let Some(cfg) = self.connect_stream_config.as_ref() {
            subscriptions.push(
                iced::Subscription::run_with(
                    ConnectStreamSubKey {
                        cfg: cfg.clone(),
                        restart: self.connect_stream_restart,
                    },
                    make_connect_stream,
                )
                .map(Message::ConnectStream),
            );
        }

        Subscription::batch(subscriptions)
    }

    pub fn stop(&mut self) {
        self.stop_with(false);
    }

    /// [`Self::stop`] for locking a Cube. Everything tied to the session ends
    /// here, but the embedded daemon, then any managed node, stops on its own
    /// thread: the daemon's poller first finishes its in-flight network
    /// request, which on a slow network kept the UI blocked for over a minute.
    /// Reopening the Cube waits for it (see [`crate::daemon::stopping`]).
    pub fn stop_in_background(&mut self) {
        self.stop_with(true);
    }

    fn stop_with(&mut self, in_background: bool) {
        info!("Close requested");
        if let Some(mut destination) = self.transfer_destination.take() {
            destination.revoke();
        }
        if let Some(imported) = &mut self.panels.imported_bitcoin {
            if in_background {
                imported.stop_in_background();
            } else {
                imported.stop();
            }
        }
        // Before the daemon that would carry a claim submission goes away.
        self.local_mcp.revoke();
        self.revoke_claim();
        if !self.daemon_backend().is_embedded() {
            return;
        }
        let bitcoind = self.internal_bitcoind.take();
        match self.daemon.clone() {
            Some(daemon) if in_background => {
                crate::daemon::stopping::stop_in_background(daemon, move || {
                    if let Some(bitcoind) = bitcoind {
                        bitcoind.stop();
                    }
                });
                return;
            }
            Some(daemon) => {
                if let Err(e) = Handle::current().block_on(async { daemon.stop().await }) {
                    error!("{}", e);
                } else {
                    info!("Internal daemon stopped");
                }
            }
            None => {}
        }
        if let Some(bitcoind) = bitcoind {
            bitcoind.stop();
        }
    }

    fn btcb2_price_task(&mut self) -> Task<Message> {
        use crate::services::fiat::btcb2::POLL_INTERVAL;
        let chain = self.cube_settings.network;
        if !chain.is_blake2b() {
            return Task::none();
        }
        let currency = self
            .cube_settings
            .fiat_price
            .as_ref()
            .filter(|setting| setting.is_enabled)
            .map(|setting| setting.currency);
        let client = self.authenticated_coincube_client();
        if chain != crate::chain::ChainId::BitcoinBlake2b || currency.is_none() || client.is_none()
        {
            let changed = self.cache.clear_btcb2_fiat();
            self.cache.btcb2_price_request = None;
            return if changed {
                Task::done(Message::CacheUpdated)
            } else {
                Task::none()
            };
        }
        let mut changed = false;
        if !self.cache.btcb2_price_usable() {
            changed |= self.cache.clear_btcb2_fiat();
        }
        let (Some(currency), Some(client)) = (currency, client) else {
            return Task::none();
        };
        if self.cache.btcb2_price_request.is_some_and(|request| {
            request.currency == currency && request.instant.elapsed() < POLL_INTERVAL
        }) {
            return if changed {
                Task::done(Message::CacheUpdated)
            } else {
                Task::none()
            };
        }
        // A settings change invalidates the displayed currency immediately.
        if self
            .cache
            .fiat_price
            .as_ref()
            .is_some_and(|price| price.currency() != currency)
        {
            changed |= self.cache.clear_btcb2_fiat();
        }
        let request = cache::FiatPriceRequest::for_btcb2(
            currency,
            cache::LookupOrigin {
                app: self.cache.app_generation,
                chain,
            },
        );
        self.cache.btcb2_price_request = Some(request);
        let request_task = Task::perform(request.send_connect(client), |price| {
            Message::Fiat(FiatMessage::GetPriceResult(price))
        });
        if changed {
            Task::batch([Task::done(Message::CacheUpdated), request_task])
        } else {
            request_task
        }
    }

    pub fn on_tick(&mut self) -> Task<Message> {
        let local_feature_task = if self.cache.chain().is_blake2b()
            && !self.requires_connect()
            && self.local_feature_check.is_none()
            && self
                .local_feature_checked_at
                .is_none_or(|at| at.elapsed() >= Duration::from_secs(60))
        {
            self.fork_feature_check()
        } else {
            Task::none()
        };
        let price_task = self.btcb2_price_task();
        // Skip tick processing if no vault is configured
        if self.daemon.is_none() {
            tracing::debug!("Skipping tick - no vault configured");
            return Task::batch([price_task, local_feature_task]);
        }
        // Skip while a backend switch is in flight: `self.daemon` still points at
        // the daemon the off-thread switch is stopping, so polling it here would
        // hit a stopped poller/RPC. The next tick after `DaemonRestarted` swaps in
        // the new daemon resumes normally.
        if self.daemon_switch_in_progress {
            return Task::batch([price_task, local_feature_task]);
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

        tasks.push(price_task);
        tasks.push(local_feature_task);

        // Imported policies use the same App daemon, with a generation-bound
        // snapshot that also verifies discovery coverage and the Cube attachment.
        if self.cache.has_imported_bitcoin {
            let client = self.authenticated_coincube_client();
            let epoch = self.panels.connect.account.session_generation();
            if let Some(panel) = self.panels.imported_bitcoin.as_mut() {
                tasks.push(panel.update_view(
                    state::imported_bitcoin::ImportMessage::Refresh,
                    client,
                    epoch,
                ));
            }
            return Task::batch(tasks);
        }

        // Check if we need to update the daemon cache.
        let duration = Duration::from_secs(
            match sync_status(
                self.daemon_backend(),
                self.cache.blockheight(),
                self.cache.sync_progress(),
                self.cache.last_poll_timestamp(),
                self.cache.last_poll_at_startup,
                self.cache.history_sync(),
            ) {
                SyncStatus::BlockchainSync(_) => 5, // Only applies to local backends
                SyncStatus::WalletFullScan { .. } | SyncStatus::LatestWalletSync => 3,
                // Polls are being retried; refresh often enough to notice recovery.
                SyncStatus::SyncFailing { .. } => 5,
                // For the rescan feature, we refresh often enough to give user an
                // up-to-date view of the rescan progress.
                SyncStatus::Synced => 10,
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
                            history_sync: info.history_sync,
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
        let Some(client) = self.breez_client.clone() else {
            return Task::none();
        };
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

    pub fn update(&mut self, message: Message) -> Task<Message> {
        let task = self.update_connect_chrome(message);
        self.refresh_vault_alert();
        task
    }

    /// Recomputes the Vault's alert from its sync result (C8).
    fn refresh_vault_alert(&mut self) {
        let alert = if self.cache.has_vault {
            let backend = self.daemon_backend();
            let via_connect =
                backend == DaemonBackend::EmbeddedCoincubed(Some(crate::node::NodeType::Esplora));
            self.cache.vault_via_connect = via_connect;
            let sync = sync_status(
                backend,
                self.cache.blockheight(),
                self.cache.sync_progress(),
                self.cache.last_poll_timestamp(),
                self.cache.last_poll_at_startup,
                self.cache.history_sync(),
            );
            vault_alert(
                &sync,
                via_connect,
                self.cache.connect_stream_status,
                self.cache.connect_authenticated,
                self.cache.last_poll_timestamp(),
                self.cache.blockheight(),
            )
        } else {
            None
        };
        if self.cache.vault_alert != alert {
            self.cache.vault_alert = alert;
        }
    }

    fn route(&mut self, message: Message) -> Task<Message> {
        self.refresh_mcp_pairings();
        if let Message::View(view::Message::LocalMcp(message)) = message {
            let focus_after = match &message {
                local_mcp::Message::RequestRevokePair(_) => Some(iced::advanced::widget::Id::from(
                    "tenshu.ai-access.revoke.keep",
                )),
                local_mcp::Message::CancelRevokePair => {
                    self.local_mcp.pending_revoke().map(|(_, id)| {
                        iced::advanced::widget::Id::from(format!(
                            "tenshu.ai-access.client.{id}.revoke"
                        ))
                    })
                }
                local_mcp::Message::RevokePair(id)
                    if self
                        .local_mcp
                        .pending_revoke()
                        .is_some_and(|(_, pending)| pending == id) =>
                {
                    // The Cube-wide switch is on both AI access views and
                    // stays focusable whenever access is available or on;
                    // "Pair a client" is hidden while the form is open and
                    // disabled after a storage failure.
                    Some(iced::advanced::widget::Id::from(
                        if self.mcp_proposals_supported()
                            || (self.mcp_supported() && self.local_mcp.switch_focusable(true))
                        {
                            "tenshu.ai-access.enable"
                        } else {
                            view::nav::shell::CUBE_SWITCHER_ID
                        },
                    ))
                }
                _ => None,
            };
            let requested_revoke = matches!(&message, local_mcp::Message::RequestRevokePair(_));
            // Controls that remove themselves hand focus to what replaces them.
            let focus_later = match &message {
                local_mcp::Message::Pair => Some(local_mcp::FocusAfter::Pair),
                local_mcp::Message::TogglePairing => Some(local_mcp::FocusAfter::TogglePairing),
                local_mcp::Message::TurnOnFromForm => Some(local_mcp::FocusAfter::Enable),
                _ => None,
            };
            #[cfg(feature = "mcp-transactions")]
            if let local_mcp::Message::ReviewTransaction {
                client_id,
                draft_id,
            } = &message
            {
                if self.mcp_supported()
                    && local_mcp::transactions::supported_chain(self.cube_settings.network)
                {
                    if let Some(wallet) = &self.wallet {
                        if let Some(service) = self.local_mcp.transaction_service(client_id, wallet)
                        {
                            let client_id = client_id.clone();
                            let draft_id = draft_id.clone();
                            let check_id = draft_id.clone();
                            let session_id = service.session_id();
                            return Task::perform(
                                async move { service.native_checked(&check_id).await },
                                move |native| {
                                    Message::View(view::Message::LocalMcp(
                                        local_mcp::Message::ReviewTransactionChecked {
                                            client_id,
                                            draft_id,
                                            session_id,
                                            native,
                                        },
                                    ))
                                },
                            );
                        }
                    }
                }
                return Task::done(Message::View(view::Message::ShowError(
                    "The AI pairing or Vault changed. Reopen AI access and review the current draft."
                        .into(),
                )));
            }
            #[cfg(feature = "mcp-transactions")]
            if let local_mcp::Message::ReviewTransactionChecked {
                client_id,
                draft_id,
                session_id,
                native,
            } = &message
            {
                let pairing_changed = "The AI pairing or Vault changed. Reopen AI access and review the current draft.";
                let unavailable = "Native review is temporarily unavailable. Try Review again.";
                let stale = "This draft is stale. Ask the paired client to prepare a new request, then review it in Tenshu.";
                let failure = if self.mcp_supported()
                    && local_mcp::transactions::supported_chain(self.cube_settings.network)
                {
                    if let Some(wallet) = &self.wallet {
                        if let Some(service) = self.local_mcp.transaction_service(client_id, wallet)
                        {
                            if service.session_id() == *session_id {
                                match service.review_status(draft_id) {
                                    local_mcp::transactions::ReviewStatus::Ready => {
                                        if let Some(native) = native
                                            .as_ref()
                                            .filter(|draft| draft.chain == wallet.chain)
                                        {
                                            if let Some(panel) = self.panels.current_mut() {
                                                panel.interrupt();
                                            }
                                            self.panels.create_spend =
                                                Some(CreateSpendPanel::from_mcp_prepared(
                                                    wallet.clone(),
                                                    native.clone(),
                                                    self.cache.blockheight(),
                                                ));
                                            self.panels.current =
                                                Menu::Vault(menu::VaultSubMenu::Send);
                                            return Task::none();
                                        }
                                        unavailable
                                    }
                                    local_mcp::transactions::ReviewStatus::Stale => stale,
                                    local_mcp::transactions::ReviewStatus::Unavailable => {
                                        unavailable
                                    }
                                }
                            } else {
                                pairing_changed
                            }
                        } else {
                            pairing_changed
                        }
                    } else {
                        pairing_changed
                    }
                } else {
                    pairing_changed
                };
                return Task::done(Message::View(view::Message::ShowError(failure.into())));
            }
            let supported = self.mcp_supported() || self.mcp_proposals_supported();
            let config = self.local_mcp.update(
                message,
                self.wallet.clone(),
                self.daemon.clone(),
                supported,
                &self.cube_settings.id,
            );
            self.refresh_mcp_pairings();
            let focus_after = focus_after
                .or_else(|| focus_later.and_then(|after| self.local_mcp.focus_after(after)));
            if let Some(id) = focus_after {
                if !requested_revoke || self.local_mcp.pending_revoke().is_some() {
                    return iced::widget::operation::focus(id);
                }
            }
            return match config {
                Some(config) => clipboard::write(config),
                None => Task::none(),
            };
        }

        // Known metadata changes must not leave a previously healthy Kit
        // fingerprint visible if their subsequent settings reload fails.
        if matches!(
            &message,
            Message::SettingsSaved | Message::LoadWallet(_) | Message::WalletUpdated(_)
        ) {
            self.imported_kit_metadata.invalidate();
        }
        if matches!(&message, Message::LoadWallet(_) | Message::WalletUpdated(_)) {
            self.local_mcp.revoke();
        }
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
        self.cache.cube_owned_by_other_account = self.panels.connect.cube.owned_by_other_account;

        // Keep the Connect auth mirror fresh every tick (not just on
        // ConnectAccount/ConnectCube messages) so surfaces that read it —
        // e.g. the keychain-unavailable modal deciding between "Sign in to
        // Connect" and "Sign with Connect" — never see a stale value after
        // a launch-time session restore.
        self.cache.connect_authenticated = self.panels.connect.account.is_authenticated();

        // The side panel's wallet names follow the Cube settings and the
        // loaded Vault, whichever message changed them (a Vault created or
        // renamed mid-session, an imported wallet adopted, a settings reload).
        self.cache.wallet_names =
            cache::WalletNames::new(&self.cube_settings, self.wallet.as_deref());
        // The Connect grant (one gate input) can change on any message.
        self.panels.show_liquid(self.cache.liquid_gate);

        // The Kit fingerprint must include the imported wallet's durable
        // address-allocation state. A Cube seed cannot stand in for its
        // descriptor or its external signers.
        self.cache.current_descriptor_fingerprint = if let Some(w) = self
            .wallet
            .as_ref()
            .filter(|wallet| wallet.supports_inheritance())
        {
            use crate::app::state::settings::recovery_kit as rk;
            // Canonical API string ("mainnet" for Bitcoin mainnet) —
            // the fingerprint inputs must agree byte-for-byte with
            // the string used at backup time (see `network_str` in
            // `state::settings::recovery_kit`). Any divergence here
            // would make every tick report a spurious drift.
            let network = self.cache.chain().api_str();
            rk::live_descriptor_fingerprint(w.as_ref(), &self.cube_settings.id, network)
        } else {
            self.imported_kit_metadata.fingerprint(
                &self.datadir,
                &self.cube_settings,
                self.cache.chain(),
                self.panels
                    .imported_bitcoin
                    .as_ref()
                    .and_then(|panel| panel.verified_address_state()),
            )
        };
    }

    /// Adopt an imported policy into the existing Vault wallet and App runtime.
    /// The import controller retains only a shared handle for its policy-specific
    /// signing and discovery work; the App owns the wallet and shutdown.
    fn adopt_imported_runtime(&mut self) -> Task<Message> {
        if self
            .cube_settings
            .bitcoin_import
            .as_deref()
            .and_then(settings::BitcoinImport::active_attachment)
            .is_none()
        {
            return Task::none();
        }
        let wallet = match Wallet::from_imported_cube(&self.cube_settings) {
            Ok(wallet) => Arc::new(wallet),
            Err(error) => return Task::done(Message::View(view::Message::ShowError(error))),
        };
        if self
            .wallet
            .as_ref()
            .is_some_and(|current| current.id() != wallet.id() || !current.is_imported())
        {
            return Task::done(Message::View(view::Message::ShowError(
                "The imported policy does not match this Cube's Vault.".into(),
            )));
        }
        let Some(panel) = self.panels.imported_bitcoin.as_mut() else {
            return Task::none();
        };
        let Some(daemon) = panel.handoff_runtime() else {
            // A wider scan stops and replaces the daemon. Do not show the old balance.
            self.panels.global_home.imported_balance = None;
            self.cache.daemon_cache.coins.clear();
            self.cache.daemon_cache.sync_progress = 0.0;
            if self
                .wallet
                .as_ref()
                .is_some_and(|wallet| wallet.is_imported())
            {
                self.daemon = None;
            }
            return Task::none();
        };
        let was_ready = self.panels.global_home.imported_balance.is_some();
        let before = self.cache.daemon_cache.clone();
        let ready = panel.sync_cache(&mut self.cache);
        if !ready {
            self.cache.daemon_cache.coins.clear();
            self.cache.daemon_cache.sync_progress = 0.0;
        }
        let first_load = self.wallet.is_none();
        let runtime_changed = self
            .daemon
            .as_ref()
            .is_none_or(|previous| !Arc::ptr_eq(previous, &daemon));
        self.daemon = Some(daemon.clone());
        self.cache.has_vault = false;
        self.cache.has_imported_bitcoin = true;
        self.cache.vault_lacks_keychain_key = false;
        self.panels.global_home.imported_installed = true;
        self.panels.global_home.imported_balance = panel.authoritative_balance();
        let mut tasks = Vec::new();
        if first_load {
            self.wallet = Some(wallet.clone());
            self.panels.build_vault_panels(
                wallet.clone(),
                &self.cache,
                daemon.backend(),
                self.datadir.clone(),
                None,
                self.config.clone(),
                self.breez_client.clone(),
            );
            tasks.push(
                self.panels
                    .global_home
                    .reload(Some(daemon.clone()), Some(wallet.clone())),
            );
        }
        if ready {
            tasks.push(Task::done(Message::CacheUpdated));
            // The imported runtime never sends `UpdateDaemonCache`: the
            // Overview's Vault rows follow it here, once its history is in
            // and whenever a refresh can have changed it.
            if matches!(self.panels.current, Menu::Cube(menu::CubeSubMenu::Overview))
                && (first_load
                    || !was_ready
                    || runtime_changed
                    || overview_activity_vault_changed(&before, &self.cache.daemon_cache))
            {
                tasks.push(self.panels.activity.refresh_wallet(
                    view::activity::WalletFilter::Vault,
                    Some(daemon.clone()),
                    Some(wallet.clone()),
                ));
            }
            if first_load || !was_ready || runtime_changed {
                if let Some(overview) = self.panels.vault_overview.as_mut() {
                    tasks.push(overview.reload(Some(daemon.clone()), Some(wallet.clone())));
                }
                if let Some(transactions) = self.panels.transactions.as_mut() {
                    tasks.push(transactions.reload(Some(daemon), Some(wallet)));
                }
            }
        }
        Task::batch(tasks)
    }

    fn update_dispatch(&mut self, message: Message) -> Task<Message> {
        // Legacy signer bootstrap persists Bitcoin-family connect.json and uses
        // unbound gRPC registration. A fork Cube must retain its admitted client;
        // neither direct requests nor late completions may enter that pipeline.
        if self.cache.chain().is_blake2b()
            && matches!(
                &message,
                Message::InAppConnectLoginCompleted { .. }
                    | Message::InAppConnectAuthReady { .. }
                    | Message::InAppConnectAuthPersisted { .. }
                    | Message::EnsureConnectReady
                    | Message::TriggerConnectStreamReady { .. }
                    | Message::ConnectStreamReady(_)
                    | Message::ConnectStream(_)
            )
        {
            return Task::done(Message::View(view::Message::ShowError(
                "Connect signer bootstrap is unavailable for Bitcoin Blake2b. Reopen this Cube through authenticated Connect startup; use its local Cube key for signing."
                    .into(),
            )));
        }
        match message {
            Message::ImportedBitcoin(event) => {
                if self.panels.imported_bitcoin.is_none() {
                    if let state::imported_bitcoin::ImportEvent::Opened(_, Ok(daemon)) = *event {
                        return Task::perform(
                            async move {
                                if let Err(error) = daemon.stop().await {
                                    log::error!(
                                        "Could not stop orphan imported Bitcoin daemon: {error}"
                                    );
                                }
                            },
                            |_| Message::Tick,
                        );
                    }
                    return Task::none();
                }
                if matches!(
                    event.as_ref(),
                    state::imported_bitcoin::ImportEvent::Installed(_, Ok(()))
                        | state::imported_bitcoin::ImportEvent::Resumed(_, Ok(Some(_)))
                        | state::imported_bitcoin::ImportEvent::Opened(_, Ok(_))
                ) {
                    self.imported_kit_metadata.invalidate();
                }
                let refresh_cube = matches!(
                    event.as_ref(),
                    state::imported_bitcoin::ImportEvent::Installed(_, Ok(()))
                        | state::imported_bitcoin::ImportEvent::Resumed(_, Ok(Some(_)))
                        | state::imported_bitcoin::ImportEvent::Refreshed(_, Ok(_))
                        | state::imported_bitcoin::ImportEvent::Opened(_, Ok(_))
                );
                let import_client = self.authenticated_coincube_client();
                let task = self
                    .panels
                    .imported_bitcoin
                    .as_mut()
                    .map_or_else(Task::none, |panel| {
                        panel.set_connect_client(import_client.as_ref());
                        panel.update_event(*event)
                    });
                if self
                    .panels
                    .imported_bitcoin
                    .as_ref()
                    .is_some_and(|panel| panel.abandoned())
                {
                    if let Ok(settings) = settings::Settings::from_file(
                        &self.datadir.network_directory(self.cube_settings.network),
                    ) {
                        if let Some(cube) = settings.cubes.into_iter().find(|cube| {
                            cube.id == self.cube_settings.id && cube.bitcoin_import.is_none()
                        }) {
                            self.cube_settings = cube;
                            self.cache.has_imported_bitcoin = false;
                            self.panels.global_home.imported_balance = None;
                            self.panels.global_home.imported_installed = false;
                            self.panels.imported_bitcoin = None;
                            return Task::batch([
                                task,
                                self.set_current_panel(Menu::Cube(
                                    crate::app::menu::CubeSubMenu::Overview,
                                )),
                            ]);
                        }
                    }
                    return task;
                }
                if refresh_cube {
                    if let Ok(settings) = settings::Settings::from_file(
                        &self.datadir.network_directory(self.cube_settings.network),
                    ) {
                        if let Some(cube) = settings
                            .cubes
                            .iter()
                            .find(|cube| cube.id == self.cube_settings.id)
                        {
                            self.imported_kit_metadata
                                .replace(&self.datadir, cube, &settings);
                            self.cache
                                .recovery_kit_last_backed_up_descriptor_fingerprint = cube
                                .recovery_kit_last_backed_up_descriptor_fingerprint
                                .clone();
                            self.cache
                                .recovery_kit_last_backed_up_keychain_descriptor_fingerprint = cube
                                .recovery_kit_last_backed_up_keychain_descriptor_fingerprint
                                .clone();
                            self.cube_settings = cube.clone();
                            self.panels.global_home.imported_installed = self
                                .cube_settings
                                .bitcoin_import
                                .as_deref()
                                .and_then(settings::BitcoinImport::active_attachment)
                                .is_some();
                        }
                    }
                }
                let runtime = self.adopt_imported_runtime();
                return Task::batch([task, runtime]);
            }
            Message::View(view::Message::ImportedBitcoin(message)) => {
                let client = self.authenticated_coincube_client();
                let connect_epoch = self.panels.connect.account.session_generation();
                let task = self
                    .panels
                    .imported_bitcoin
                    .as_mut()
                    .map_or_else(Task::none, |panel| {
                        panel.update_view(message, client, connect_epoch)
                    });
                let runtime = self.adopt_imported_runtime();
                return Task::batch([task, runtime]);
            }
            Message::View(view::Message::DismissToast(id)) => {
                self.errors.retain(|(i, ..)| *i != id);
                return self.arm_toast_timers();
            }
            Message::ExpireToast(id) => {
                let now = std::time::Instant::now();
                let due = self
                    .errors
                    .iter()
                    .any(|(i, deadline, ..)| *i == id && deadline.is_some_and(|d| d <= now));
                if due {
                    self.errors.retain(|(i, ..)| *i != id);
                    return self.arm_toast_timers();
                }
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
                    .push((self.current_error_id, None, level, ToastContent::Text(msg)));
                self.current_error_id += 1;
                return self.arm_toast_timers();
            }
            Message::View(view::Message::ShowIncomingLiquidWaitingToast(amount_sat)) => {
                self.errors.push((
                    self.current_error_id,
                    None,
                    log::Level::Info,
                    ToastContent::IncomingLiquidWaiting { amount_sat },
                ));
                self.current_error_id += 1;
                return self.arm_toast_timers();
            }
            Message::PendingBitcoindLog(log) => {
                if let Some(line) = log {
                    self.cache.node_bitcoind_last_log = Some(line);
                }
            }
            Message::ConnectStreamReady(cfg) => {
                match cfg {
                    Some((mut cfg, email)) => {
                        if self.connect_email.as_deref() != Some(email.as_str())
                            || !self
                                .connect_auth
                                .as_ref()
                                .is_some_and(|auth| Arc::ptr_eq(auth, &cfg.tokens))
                        {
                            return Task::none();
                        }
                        let current = match cfg.tokens.try_read() {
                            Ok(current) => current,
                            Err(_) => {
                                let tokens = cfg.tokens.clone();
                                let retry_cfg = cfg.clone();
                                return Task::perform(
                                    async move {
                                        drop(tokens.read().await);
                                        Message::ConnectStreamReady(Some((retry_cfg, email)))
                                    },
                                    |message| message,
                                );
                            }
                        };
                        let network_dir = self
                            .cache
                            .datadir_path
                            .network_directory(self.cache.network);
                        let cached_account =
                            crate::services::connect::client::cache::ConnectCache::from_file(
                                &network_dir,
                            )
                            .ok()
                            .and_then(|cache| cache.active_account().cloned())
                            .filter(|account| {
                                account.email == email
                                    && account.tokens.access_token == current.access_token
                                    && account.tokens.refresh_token == current.refresh_token
                            });
                        drop(current);
                        let Some(account) = cached_account else {
                            return Task::none();
                        };
                        let Some(device_id) = account.device_id.filter(|id| !id.trim().is_empty())
                        else {
                            return Task::none();
                        };
                        // About may have completed re-registration while the
                        // async vault lookup ran. Install its current id, not
                        // the one captured before that await.
                        cfg.device_id = device_id;
                        cfg.last_seen_seq = cfg
                            .last_seen_seq
                            .max(account.last_seen_event_seq.unwrap_or(0));
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
                        self.cache.connect_email = Some(email);
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
            Message::InAppConnectAuthReady {
                login,
                epoch,
                previous_refresh_token,
            } => {
                return self.connect_auth_ready(login, epoch, previous_refresh_token);
            }
            Message::InAppConnectAuthPersisted {
                login,
                epoch,
                previous_refresh_token,
                tokens,
            } => {
                return self.connect_auth_persisted(login, epoch, previous_refresh_token, tokens);
            }
            Message::InAppConnectLoginCompleted {
                token,
                refresh_token,
                email,
                epoch,
            } => {
                return self.connect_login_completed(token, refresh_token, email, epoch);
            }
            Message::TriggerConnectStreamReady {
                network,
                datadir,
                tokens,
                email,
                cube_uuid,
            } => {
                if self.in_app_refresh_commit.is_some() {
                    self.deferred_connect_stream_triggers.push(
                        Message::TriggerConnectStreamReady {
                            network,
                            datadir,
                            tokens,
                            email,
                            cube_uuid,
                        },
                    );
                    return Task::none();
                }
                let current = match tokens.try_read() {
                    Ok(current) => current,
                    Err(_) => {
                        let wait_tokens = tokens.clone();
                        let retry_tokens = tokens.clone();
                        return Task::perform(
                            async move {
                                drop(wait_tokens.read().await);
                                Message::TriggerConnectStreamReady {
                                    network,
                                    datadir,
                                    tokens: retry_tokens,
                                    email,
                                    cube_uuid,
                                }
                            },
                            |message| message,
                        );
                    }
                };
                let network_dir = datadir.network_directory(network);
                let owns_cached_session =
                    crate::services::connect::client::cache::ConnectCache::from_file(&network_dir)
                        .ok()
                        .and_then(|cache| cache.active_account().cloned())
                        .is_some_and(|account| {
                            account.email == email
                                && account.tokens.access_token == current.access_token
                                && account.tokens.refresh_token == current.refresh_token
                        });
                drop(current);
                if !owns_cached_session {
                    return Task::none();
                }
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
                let shared_auth = if self.connect_email == expected_email {
                    self.connect_auth.clone()
                } else {
                    None
                };
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
                } else if let Some(generation) =
                    self.panels.connect.cube.begin_external_registration()
                {
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
                    // Same for this device's Spark Stable Balance decision:
                    // `None` is omitted, so it never clobbers another device's.
                    let cube_spark_stable_balance = self.cube_settings.spark_stable_balance;
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
                                    spark_stable_balance: cube_spark_stable_balance,
                                }) // upgrade-only Option<bool>
                                .await
                                .map_err(|e| {
                                    crate::app::state::connect::cube::CubeRegistrationError::from(
                                        &e,
                                    )
                                })
                        },
                        move |r| {
                            Message::View(view::Message::ConnectCube(
                                view::ConnectCubeMessage::CubeRegistered {
                                    generation,
                                    result: r,
                                },
                            ))
                        },
                    )
                } else {
                    // A registration is already in flight — claimed either by
                    // this branch on an earlier press or by the panel. Skip the
                    // duplicate; the bootstrap below still runs, which is the
                    // part a second press is actually asking for.
                    Task::none()
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
                        let tokens_arc = connect_registration_auth(
                            shared_auth,
                            account.tokens,
                            &network_dir,
                            &email,
                        )
                        .await;

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
                if let Some(settings) = self.panels.vault_settings.as_mut() {
                    settings.set_managed_owner(
                        bitcoind.owner_lock_path().map(std::path::PathBuf::from),
                    );
                }
                self.internal_bitcoind = Some(bitcoind);
            }
            Message::BackgroundManagedNodeReady {
                cube,
                attempt,
                flavor,
                result,
            } => {
                // An earlier attempt's result may still adopt or release its
                // node, but only the current attempt owns the shown status.
                let current = self.background_node_attempt == Some(attempt);
                let Some(cfg) = self.daemon.as_ref().and_then(|daemon| daemon.config()) else {
                    return Task::none();
                };
                if cube != self.cube_settings.id {
                    if let Ok((_, node)) = result {
                        node.stop();
                    }
                    return Task::none();
                }
                if cfg.pending_managed_node_install.as_deref() != Some(flavor.as_str())
                    || !matches!(
                        cfg.bitcoin_backend,
                        Some(coincubed::config::BitcoinBackend::Esplora(_))
                    )
                {
                    if current {
                        self.cache.background_node_install = cache::BackgroundNodeInstall::Idle;
                    }
                    if let Ok((_, node)) = result {
                        node.stop();
                    }
                    return Task::none();
                }
                if self.daemon_switch_in_progress {
                    // The pending-node config must be applied after the other
                    // switch, or LoadDaemonConfig would discard this request.
                    return Task::perform(
                        async move {
                            tokio::time::sleep(Duration::from_secs(5)).await;
                            Message::BackgroundManagedNodeReady {
                                cube,
                                attempt,
                                flavor,
                                result,
                            }
                        },
                        |message| message,
                    );
                }
                match result {
                    Ok((node_config, node)) => {
                        // The pending node's sync card takes over from here.
                        if current {
                            self.cache.background_node_install = cache::BackgroundNodeInstall::Idle;
                        }
                        if let Some(settings) = self.panels.vault_settings.as_mut() {
                            settings.set_managed_owner(
                                node.owner_lock_path().map(std::path::PathBuf::from),
                            );
                        }
                        self.internal_bitcoind = Some(node);
                        let mut updated = cfg.clone();
                        updated.pending_bitcoind = Some(node_config);
                        updated.pending_managed_node_install = None;
                        updated.auto_switch_to_pending = Some(true);
                        return Task::done(Message::LoadDaemonConfig(Box::new(updated)));
                    }
                    Err(error) => {
                        tracing::warn!("Background managed node setup failed: {error}");
                        if !current {
                            // A later attempt is running; its own result decides.
                            return Task::none();
                        }
                        // Leave the request on disk so a retry, here or on
                        // reopening the Vault, can pick it up.
                        self.cache.background_node_install =
                            cache::BackgroundNodeInstall::Failed(error.clone());
                        return Task::done(Message::View(view::Message::ShowToast(
                            log::Level::Warn,
                            coincube_ui::localized_format!(
                                "Local node setup paused: {error}. Connect remains available; retry from Settings → Node.",
                                "La configuración del nodo local está en pausa: {error}. Connect sigue disponible; vuelve a intentarlo desde Ajustes → Nodo."
                            ),
                        )));
                    }
                }
            }
            Message::BackgroundManagedNodeProgress {
                cube,
                attempt,
                progress,
            } => {
                if cube == self.cube_settings.id
                    && self.background_node_attempt == Some(attempt)
                    && self.cache.background_node_install.is_running()
                {
                    self.cache.background_node_install = progress;
                }
            }
            Message::View(view::Message::Settings(view::SettingsMessage::NodeSettings(
                view::NodeSettingsMessage::RetryBackgroundNodeSetup,
            ))) => {
                if matches!(
                    self.cache.background_node_install,
                    cache::BackgroundNodeInstall::Failed(_)
                ) {
                    self.cache.background_node_install = cache::BackgroundNodeInstall::Idle;
                    return self.background_managed_node_task(false);
                }
            }
            Message::View(view::Message::Settings(view::SettingsMessage::NodeSettings(
                view::NodeSettingsMessage::SwitchToBitcoind,
            ))) => {
                let managed = self
                    .daemon
                    .as_ref()
                    .and_then(|d| d.config())
                    .and_then(local_node_sync_config)
                    .is_some_and(|cfg| {
                        crate::node::retention::is_managed_endpoint(
                            &self.cache.datadir_path,
                            self.cache.network,
                            self.cache.chain(),
                            cfg,
                        )
                    });
                let missing = self
                    .cache
                    .node_bitcoind_pruning
                    .zip(self.cache.local_switch_history)
                    .is_some_and(|(pruning, history)| {
                        local_switch::pruned_node_serves(pruning, history).is_err()
                    });
                if managed && missing && self.cache.node_bitcoind_ibd == Some(false) {
                    self.cache.node_history_switch_requested = true;
                    self.cache.local_node_transfer_visible = true;
                    self.auto_switch_suppressed = false;
                }
                if let Some(panel) = self.panels.vault_settings.as_mut() {
                    return panel.update(self.daemon.clone(), &self.cache, message);
                }
                return Task::none();
            }
            Message::View(view::Message::Settings(view::SettingsMessage::NodeSettings(
                view::NodeSettingsMessage::RecoveryDismiss
                | view::NodeSettingsMessage::RecoveryCancel,
            ))) => {
                // Returning to Connect withdraws promotion consent for this
                // session, including a previously enabled automatic switch.
                self.auto_switch_suppressed = true;
                self.cache.node_history_switch_requested = false;
                self.cache.local_node_transfer_visible = false;
                if let Some(panel) = self.panels.vault_settings.as_mut() {
                    return panel.update(self.daemon.clone(), &self.cache, message);
                }
                return Task::none();
            }
            Message::NodeHistory(action) => {
                if self.cache.chain().is_blake2b() {
                    return Task::none();
                }
                if self.node_history_in_progress {
                    if !matches!(action, node_history::Action::Poll) {
                        self.cache.node_history.cancel_queued =
                            matches!(action, node_history::Action::Cancel);
                        self.node_history_pending = Some(action);
                    }
                    return Task::none();
                }
                if self.daemon_switch_in_progress {
                    return Task::none();
                }
                let (Some(daemon), Some(wallet), Some(managed)) =
                    (&self.daemon, &self.wallet, &self.internal_bitcoind)
                else {
                    return Task::none();
                };
                let Some(config) = daemon.config().and_then(local_node_sync_config).cloned() else {
                    return Task::none();
                };
                if config != managed.config {
                    return Task::none();
                }
                self.node_history_request_epoch = self.node_history_request_epoch.wrapping_add(1);
                let request_epoch = self.node_history_request_epoch;
                self.node_history_in_progress = true;
                // The 5-second background Poll is not a user's batch. Marking
                // it busy made Node settings show recovery in progress and
                // hide the switch for the length of each Poll, so the page
                // flashed every 5 seconds. `node_history_in_progress` still
                // keeps a second worker from starting.
                if !matches!(action, node_history::Action::Poll) {
                    self.cache.node_history.busy = true;
                    self.cache.node_history.error = None;
                    self.node_history_epoch = self.node_history_epoch.wrapping_add(1);
                    self.cache.local_switch_history = None;
                }
                let app = self.cache.app_generation;
                let wallet_id = wallet.id();
                let context = node_history::Context {
                    datadir: self.cache.datadir_path.clone(),
                    network: self.cache.network,
                    chain: self.cache.chain(),
                    config: config.clone(),
                    managed_owner: managed.owner_lock_path().map(std::path::PathBuf::from),
                    daemon: daemon.clone(),
                    wallet: wallet.clone(),
                };
                return Task::perform(
                    async move {
                        let result = context.run(action).await.map(Box::new);
                        Message::NodeHistoryCompleted {
                            request_epoch,
                            app,
                            wallet: wallet_id,
                            config,
                            result,
                        }
                    },
                    |message| message,
                );
            }
            Message::NodeHistoryCompleted {
                request_epoch,
                app,
                wallet,
                config,
                result,
            } => {
                if request_epoch != self.node_history_request_epoch {
                    return Task::none();
                }
                // The current worker has finished even when its configuration
                // disappeared. An older worker must never release a newer one.
                self.node_history_in_progress = false;
                self.cache.node_history.busy = false;
                if app != self.cache.app_generation
                    || self.wallet.as_ref().map(|w| w.id()) != Some(wallet)
                    || self
                        .daemon
                        .as_ref()
                        .and_then(|d| d.config())
                        .and_then(local_node_sync_config)
                        != Some(&config)
                {
                    self.node_history_pending = None;
                    self.cache.node_history.cancel_queued = false;
                    return Task::none();
                }
                match result {
                    Ok(outcome) => {
                        self.cache.node_history = outcome.status;
                        if let Some(node) = outcome.node {
                            if let Some(settings) = self.panels.vault_settings.as_mut() {
                                settings.set_managed_owner(
                                    node.owner_lock_path().map(std::path::PathBuf::from),
                                );
                            }
                            self.internal_bitcoind = Some(node);
                        }
                    }
                    Err(error) => self.cache.node_history.error = Some(error),
                }
                let cancel_queued = matches!(
                    self.node_history_pending.as_ref(),
                    Some(node_history::Action::Cancel)
                );
                let queued = self.node_history_pending.take().map(Message::NodeHistory);
                self.cache.node_history.cancel_queued = cancel_queued;
                return Task::batch([
                    Task::done(Message::CacheUpdated),
                    Task::done(Message::PollBitcoindSync),
                    queued.map(Task::done).unwrap_or_else(Task::none),
                ]);
            }
            Message::PollBitcoindSync => {
                if !self.bitcoind_sync_probe_in_progress {
                    if let Some(pending_cfg) = self
                        .daemon
                        .as_ref()
                        .and_then(|d| d.config())
                        .and_then(local_node_sync_config)
                        .cloned()
                    {
                        self.bitcoind_sync_probe_in_progress = true;
                        let app = self.cache.app_generation;
                        let history_epoch = self.node_history_epoch;
                        let wallet = self.wallet.as_ref().map(|w| w.id());
                        // Only a *pending* node is a switch target, so only then
                        // is the Vault's history worth reading.
                        let history_probe = match (&self.daemon, &self.wallet) {
                            (Some(daemon), Some(wallet))
                                if daemon
                                    .config()
                                    .is_some_and(|c| c.pending_bitcoind.is_some()) =>
                            {
                                Some(VaultHistoryProbe {
                                    daemon: daemon.clone(),
                                    data_dir: self.cache.datadir_path.clone(),
                                    chain: self.cache.chain(),
                                    wallet: wallet.clone(),
                                })
                            }
                            _ => None,
                        };
                        return Task::perform(
                            async move {
                                let result =
                                    check_bitcoind_sync_progress(pending_cfg.clone()).await;
                                // Only a synced, pruned node needs it; an
                                // unpruned one serves any history.
                                let needs_history = result.as_ref().is_ok_and(|s| {
                                    !s.ibd
                                        && matches!(
                                            s.pruning,
                                            Some(local_switch::NodePruning::Pruned { .. })
                                        )
                                });
                                let vault_history = match history_probe {
                                    Some(probe) if needs_history => {
                                        let pruning = result
                                            .as_ref()
                                            .ok()
                                            .and_then(|s| s.pruning)
                                            .expect("needs_history requires known pruning");
                                        probe.read(&pending_cfg, pruning).await
                                    }
                                    _ => None,
                                };
                                Message::BitcoindSyncProgress {
                                    app,
                                    history_epoch,
                                    wallet,
                                    config: pending_cfg,
                                    result,
                                    vault_history,
                                }
                            },
                            |message| message,
                        );
                    }
                }
            }
            Message::UnsweptNotice {
                app,
                generation,
                result,
            } => {
                if app != self.cache.app_generation
                    || self.unswept_in_flight != Some(generation)
                    || generation != *self.panels.session_generation.borrow()
                {
                    return Task::none();
                }
                self.unswept_in_flight = None;
                let current_session = self.claim_connect_session();
                if !matches!((&self.unswept_session, &current_session), (Some(old), Some(new)) if crate::app::connect_session::same_session(old, new))
                {
                    self.cache.unswept_notice = None;
                    self.unswept_session = None;
                    return Task::none();
                }
                self.cache.unswept_notice = result.ok().flatten().and_then(|proof| {
                    let wallet = self.wallet.as_ref()?;
                    let notice = unswept_notice::Notice::new(proof, generation, wallet);
                    notice
                        .visible(&self.cache, std::time::Instant::now())
                        .then_some(notice)
                });
                return Task::done(Message::CacheUpdated);
            }
            Message::EntangledLookups {
                origin,
                claimed,
                answers,
            } => {
                // Claim release and cache write, scoped to the instance that
                // issued the batch (`#393`): see `Cache::entangled_batch_landed`.
                let chain = self.vault_chain();
                let changed = self.cache.entangled_batch_landed(
                    &mut self.entangled_in_flight,
                    chain,
                    origin,
                    &claimed,
                    &answers,
                );
                if !changed {
                    return Task::none();
                }
                // A new negative answer can make a coin an ancestry candidate.
                let chain_proofs = self.chain_proof_task();
                return Task::batch([chain_proofs, Task::done(Message::CacheUpdated)]);
            }
            Message::ChainProofs {
                origin,
                claimed,
                results,
            } => {
                let changed = self.cache.chain_proofs_landed(
                    &mut self.chain_proofs_in_flight,
                    origin,
                    &claimed,
                    results,
                );
                if !changed {
                    return Task::none();
                }
                self.save_chain_proofs();
                return Task::done(Message::CacheUpdated);
            }
            Message::SplitFetch { fee, address } => {
                let Some(wallet) = self.wallet.as_ref() else {
                    return Task::none();
                };
                let mut tasks = Vec::new();
                if fee {
                    let client = self.authenticated_coincube_client();
                    let chain = wallet.chain;
                    tasks.push(Task::perform(
                        crate::services::split_coins::fee_rate(client, chain),
                        Message::SplitFeeRate,
                    ));
                }
                if let (true, Some(daemon)) = (address, self.daemon.clone()) {
                    tasks.push(Task::perform(
                        async move {
                            daemon
                                .get_new_address()
                                .await
                                .map(|result| result.address.to_string())
                                .map_err(|e| format!("Couldn't get an address: {}", e))
                        },
                        Message::SplitAddress,
                    ));
                }
                return Task::batch(tasks);
            }
            Message::SplitRequest {
                method,
                coins,
                key,
                feerate_vb,
            } => {
                let failed = |e: &str| Task::done(Message::SplitPrepared(Err(e.to_string())));
                let (Some(wallet), Some(daemon)) = (self.wallet.clone(), self.daemon.clone())
                else {
                    return failed("This Vault isn't loaded.");
                };
                if !self.blake2b_on() {
                    return failed("Split coins needs Bitcoin Blake2b to be on for this account.");
                }
                let Some(client) = self.authenticated_coincube_client() else {
                    return failed("Sign in to COINCUBE Connect to split coins.");
                };
                let key = match key {
                    Some(outpoint) => match self
                        .cache
                        .split_keys()
                        .into_iter()
                        .find(|(coin, _)| coin.outpoint == outpoint)
                    {
                        Some((_, path)) => Some(crate::services::split_coins::KeyChoice {
                            outpoint,
                            path: path.clone(),
                        }),
                        None => return failed("The split key is no longer available."),
                    },
                    None => None,
                };
                let generation = *self.panels.session_generation.borrow();
                let live = self.panels.session_generation.subscribe();
                return Task::perform(
                    crate::services::split_coins::prepare(
                        daemon, wallet, method, coins, key, feerate_vb, client, generation, live,
                    ),
                    |result| {
                        Message::SplitPrepared(result.map(|prepared| Box::new(prepared.record)))
                    },
                );
            }
            Message::SplitKeyCheck {
                spend,
                key,
                generation,
            } => {
                let reply = move |result: Result<(), String>| Message::SplitKeyChecked {
                    spend,
                    key,
                    generation,
                    result,
                };
                let Some(wallet) = self.wallet.as_ref() else {
                    return Task::done(reply(Err("This Vault isn't loaded.".into())));
                };
                let chain = wallet.chain;
                let Some(path) = self.cache.split_key_path(spend, key) else {
                    return Task::done(reply(Err(
                        "The split key has no proof to check. Build the split again.".into(),
                    )));
                };
                let Some(client) = self.authenticated_coincube_client() else {
                    return Task::done(reply(Err(
                        "Sign in to COINCUBE Connect to check the split key.".into(),
                    )));
                };
                let session = *self.panels.session_generation.borrow();
                let live = self.panels.session_generation.subscribe();
                return Task::perform(
                    async move {
                        let key = crate::services::split_coins::KeyChoice {
                            outpoint: key,
                            path,
                        };
                        crate::services::split_coins::requalify(&client, chain, &key, session, live)
                            .await
                    },
                    reply,
                );
            }
            Message::SplitPrepared(result) => {
                let txid = result.as_ref().ok().map(|record| record.txid);
                let mut tasks = Vec::new();
                if let Ok(record) = &result {
                    tasks.push(self.record_split((**record).clone()));
                }
                // To the Coins panel whichever panel is showing: its overlay
                // stays locked until it hears the result.
                if let (Some(daemon), Some(panel)) =
                    (self.daemon.clone(), self.panels.coins.as_mut())
                {
                    tasks.push(panel.update(
                        Some(daemon),
                        &self.cache,
                        Message::SplitPrepared(result),
                    ));
                }
                if let Some(txid) = txid {
                    // Sign and broadcast through the Vault's normal PSBT screen.
                    tasks.push(Task::done(Message::View(view::Message::Menu(Menu::Vault(
                        menu::VaultSubMenu::PSBTs(Some(txid)),
                    )))));
                }
                return Task::batch(tasks);
            }
            Message::View(view::Message::CreateBlake2bCopy) => {
                // From the disk, not the cache (#503): a copy made earlier in
                // this session leaves the cached flag stale, and the hub would
                // otherwise start a second installer for the same Vault.
                let Some(wallet) = self.wallet.as_ref() else {
                    return Task::none();
                };
                if self.cube_settings.network != crate::chain::ChainId::Bitcoin {
                    return Task::none();
                }
                self.cache.btcb2_already_claimed =
                    claim_target_exists(&self.datadir, &wallet.descriptor_checksum);
                if self.cache.btcb2_already_claimed {
                    return Task::none();
                }
                return Task::done(Message::View(view::Message::StartClaimBlake2b));
            }
            Message::View(view::Message::SplitFirst) => {
                let menu = Menu::Vault(menu::VaultSubMenu::Coins(None));
                let load = self.set_current_panel(menu);
                return Task::batch([
                    load,
                    Task::done(Message::View(view::Message::CoinControl(
                        view::CoinControlMessage::OpenSplit,
                    ))),
                ]);
            }
            Message::SendSelectedCoins(outpoints) => {
                let Some(wallet) = self.wallet.clone() else {
                    return Task::none();
                };
                let (balance, unconfirmed_balance, _, _) = state::coins_summary(
                    self.cache.coins(),
                    self.cache.blockheight() as u32,
                    wallet.recovery_timelock(),
                );
                self.panels.create_spend = Some(CreateSpendPanel::new_with_coins(
                    wallet.clone(),
                    self.cache.coins(),
                    self.cache.blockheight() as u32,
                    &outpoints,
                    self.cache.network,
                    balance,
                    unconfirmed_balance,
                    sync_status(
                        self.daemon_backend(),
                        self.cache.blockheight(),
                        self.cache.sync_progress(),
                        self.cache.last_poll_timestamp(),
                        self.cache.last_poll_at_startup,
                        self.cache.history_sync(),
                    ),
                    self.cache.bitcoin_unit,
                ));
                self.panels.current = Menu::Vault(menu::VaultSubMenu::Send);
                if let Some(panel) = self.panels.current_mut() {
                    return panel.reload(self.daemon.clone(), self.wallet.clone());
                }
                return Task::none();
            }
            Message::SplitTransactions {
                origin,
                transactions,
                seq,
                twin_records,
                twin_checked,
                twin_checked_at,
            } => {
                let Some(wallet) = self.wallet.as_ref() else {
                    return Task::none();
                };
                if !self.cache.issued_here(origin) || origin.chain != wallet.chain {
                    return Task::none();
                }
                // Twin checks apply even from a pass that lands late: each
                // replaces only an older check of the same split.
                let checks_changed = crate::services::split_coins::record_twin_checks(
                    &mut self.twin_checks,
                    twin_checked,
                    twin_checked_at,
                );
                if !crate::services::split_coins::newer_pass(&mut self.split_refresh_applied, seq) {
                    if !checks_changed {
                        return Task::none();
                    }
                    self.cache.separated = crate::services::split_coins::twin_separated(
                        &self.twin_records,
                        &self.twin_checks,
                        std::time::Instant::now(),
                    );
                    return Task::done(Message::CacheUpdated);
                }
                self.twin_records = twin_records;
                let twin_records = &self.twin_records;
                self.twin_checks
                    .retain(|txid, _| twin_records.iter().any(|record| record.txid == *txid));
                let coins = self.cache.coins().to_vec();
                let tip = self.cache.blockheight();
                let derived = crate::services::split_coins::derive(
                    &mut self.cache.split_records,
                    &coins,
                    tip,
                    &transactions,
                    std::time::SystemTime::now(),
                );
                self.cache.splitting = derived.splitting;
                self.cache.split_unconfirmed = derived.unconfirmed;
                self.cache.separated = crate::services::split_coins::twin_separated(
                    &self.twin_records,
                    &self.twin_checks,
                    std::time::Instant::now(),
                );
                let proved = crate::services::split_coins::merge_proofs(
                    &mut self.cache.chain_proofs,
                    derived.proofs,
                );
                if derived.changed {
                    self.save_split_records();
                }
                if proved {
                    self.save_chain_proofs();
                }
                return Task::done(Message::CacheUpdated);
            }
            Message::EntangledRevalidated {
                origin,
                spend,
                generation,
                answers,
            } => {
                // The spend screen's own re-check of a replayable spend's
                // inputs. Every resolved answer is cached before the reply is
                // routed, stale-by-check-generation ones included: each
                // carries its own observation instant and the cache is
                // monotonic in it (`#395`), so a reply from a screen since
                // closed, or from before a signature was added, cannot
                // re-stamp a newer negative — and its `Entangled` is still
                // true. The check generation is the *panel's* filter, for the
                // claim it clears, not the cache's. A reply from a predecessor
                // `App` instance (`#393`) contributes at most same-chain
                // positives (`Cache::accept_entanglement`) and is not routed:
                // the screen that asked is gone with that instance.
                let changed = self
                    .cache
                    .accept_entanglement(origin, self.vault_chain(), &answers);
                if !self.cache.issued_here(origin) {
                    if changed {
                        return Task::done(Message::CacheUpdated);
                    }
                    return Task::none();
                }
                let routed = Message::EntangledRevalidated {
                    origin,
                    spend,
                    generation,
                    answers,
                };
                let forwarded = match (self.daemon.clone(), self.panels.current_mut()) {
                    (Some(daemon), Some(panel)) => panel.update(Some(daemon), &self.cache, routed),
                    (None, Some(panel)) => panel.update(None, &self.cache, routed),
                    (_, None) => Task::none(),
                };
                if changed {
                    return Task::batch([forwarded, Task::done(Message::CacheUpdated)]);
                }
                return forwarded;
            }
            Message::BitcoindNetStats(res) => {
                self.node_net_stats_probe_in_progress = false;
                match res {
                    Ok(stats) => self.cache.node_net_stats = Some(stats),
                    // Transient (e.g. mid-restart) — keep the last good stats.
                    Err(e) => tracing::debug!("node net-stats poll failed: {e}"),
                }
            }
            Message::BitcoindSyncProgress {
                app,
                history_epoch,
                wallet,
                config,
                result,
                vault_history,
            } => {
                // Wallet-history permission belongs to the App that asked.
                // A delayed result must not authorize another Vault using the
                // same managed node, or release its in-flight probe guard.
                if app != self.cache.app_generation
                    || wallet != self.wallet.as_ref().map(|w| w.id())
                {
                    return Task::none();
                }
                // A result from the previous node must not update or promote a new one.
                if self
                    .daemon
                    .as_ref()
                    .and_then(|d| d.config())
                    .and_then(local_node_sync_config)
                    != Some(&config)
                {
                    return Task::none();
                }
                self.bitcoind_sync_probe_in_progress = false;
                match result {
                    Err(e) => tracing::warn!("Bitcoind sync check failed: {}", e),
                    Ok(LocalNodeSync {
                        progress,
                        ibd,
                        blocks,
                        headers,
                        subversion,
                        pruning,
                    }) => {
                        self.cache.node_bitcoind_sync_progress = Some(progress);
                        self.cache.node_bitcoind_pruning = pruning;
                        self.cache.local_switch_history = if history_epoch
                            == self.node_history_epoch
                            && !self.node_history_in_progress
                        {
                            vault_history
                        } else {
                            None
                        };
                        self.cache.node_bitcoind_sync_heights = Some((blocks, headers));
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
                        if !ibd
                            && !self.daemon_switch_in_progress
                            && !self.auto_switch_suppressed
                            && (!self.cache.node_history_switch_requested
                                || self.cache.node_history.reconciled)
                            && !self.node_history_in_progress
                            && !self.cache.node_history.job.as_ref().is_some_and(|job| {
                                !matches!(
                                    job.phase,
                                    crate::node::history::Phase::Complete
                                        | crate::node::history::Phase::Cancelled
                                )
                            })
                        {
                            let switch =
                                self.daemon.as_ref().and_then(|d| d.config()).and_then(|c| {
                                    // Promote unless the node was explicitly parked
                                    // (`Some(false)`). An absent flag (`None`) is a
                                    // legacy adopted node — the old build promoted it
                                    // on sync with no flag — so it must promote too.
                                    if c.auto_switch_to_pending == Some(false)
                                        && !self.cache.node_history_switch_requested
                                    {
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
                                    if new_cfg.bitcoin_config.chain.is_blake2b() {
                                        new_cfg.bitcoin_config.poll_interval_secs =
                                            Duration::from_secs(
                                                coincubed::config::LOCAL_BACKEND_POLL_INTERVAL_SECS,
                                            );
                                    }
                                    Some(new_cfg)
                                });
                            if let Some(new_cfg) = switch {
                                // Not while the Vault is scanning (the switch
                                // would throw the scan away), and never to a
                                // local wallet missing history it cannot rescan.
                                // Deferred, not dropped: every probe re-asks.
                                let hold = local_switch::auto_switch_hold(
                                    self.running_vault_scan(),
                                    pruning,
                                    self.cache.local_switch_history,
                                );
                                if hold != self.local_switch_hold {
                                    log_local_switch_hold(hold);
                                }
                                self.local_switch_hold = hold;
                                if hold.is_none() {
                                    info!("Switching to local Bitcoind — node synced");
                                    return self.spawn_daemon_switch(new_cfg);
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
                    .network_directory(self.cube_settings.network);
                if let Ok(settings) = settings::Settings::from_file(&network_dir) {
                    self.imported_kit_metadata.replace(
                        &self.datadir,
                        &self.cube_settings,
                        &settings,
                    );
                    if let Some(cube) = settings
                        .cubes
                        .iter()
                        .find(|c| c.id == self.cube_settings.id)
                    {
                        self.cache.bitcoin_unit = cube.unit_setting.display_unit;
                        self.cube_settings.fiat_price = cube.fiat_price.clone();
                        if self.cube_settings.network.is_blake2b()
                            && self.cache.fiat_price.as_ref().is_some_and(|price| {
                                !cube.fiat_price.as_ref().is_some_and(|setting| {
                                    setting.is_enabled && setting.currency == price.currency()
                                })
                            })
                        {
                            self.cache.clear_btcb2_fiat();
                            self.cache.btcb2_price_request = None;
                        }
                        // The backup attention row follows persisted state.
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
                            if self.cube_settings.network.is_blake2b() {
                                self.cache.clear_btcb2_fiat();
                                self.cache.btcb2_price_request = None;
                            }
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
                if self.cube_settings.network.is_blake2b() {
                    let relevant = self
                        .cube_settings
                        .fiat_price
                        .as_ref()
                        .is_some_and(|setting| {
                            setting.is_enabled && setting.currency == fiat_price.currency()
                        });
                    if relevant && self.cache.accept_btcb2_price(fiat_price) {
                        return Task::done(Message::CacheUpdated);
                    }
                    return Task::none();
                }
                // A scoped BTCB2 quote must never seed Bitcoin/USD conversion.
                if fiat_price.request.origin.is_some() {
                    return Task::none();
                }
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
                if self.cache.has_imported_bitcoin {
                    return Task::none();
                }
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
                        let retire_obligation = if full_scan_settles_rescan_obligation(
                            &self.daemon_backend(),
                            self.rescan_obligation_retired_by_scan
                                || self.rescan_obligation_retire_in_flight,
                            &daemon_cache.history_sync,
                        ) {
                            let task = self.retire_rescan_obligation_task();
                            self.rescan_obligation_retire_in_flight = task.is_some();
                            task
                        } else {
                            None
                        };
                        let vault_changed = overview_activity_vault_changed(
                            &self.cache.daemon_cache,
                            &daemon_cache,
                        );
                        self.cache.daemon_cache = daemon_cache;
                        // The Vault page's rows follow its own reload
                        // (`Message::Payments`).
                        let recent = if vault_changed
                            && matches!(
                                self.panels.current,
                                Menu::Cube(menu::CubeSubMenu::Overview)
                            ) {
                            self.panels.activity.refresh_wallet(
                                view::activity::WalletFilter::Vault,
                                self.daemon.clone(),
                                self.wallet.clone(),
                            )
                        } else {
                            Task::none()
                        };
                        // Fire-and-forget recovery heartbeat after the sync's
                        // fresh tip lands (Estate Notifications — PR 2). Batched
                        // alongside the normal cache cascade so it never delays
                        // or blocks it.
                        let heartbeat = self.recovery_heartbeat_task();
                        // Same posture for the BTCB2 entangled-deposit lookups:
                        // `Task::none()` on every Bitcoin-family Cube.
                        let entangled = self.entangled_lookup_task();
                        let chain_proofs = self.chain_proof_task();
                        let splits = self.split_refresh_task();
                        let unswept = self.unswept_notice_task();
                        return Task::batch([
                            heartbeat,
                            entangled,
                            chain_proofs,
                            splits,
                            unswept,
                            retire_obligation.unwrap_or_else(Task::none),
                            recent,
                            Task::done(Message::CacheUpdated),
                        ]);
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
                    Err(e) => Task::done(Message::View(view::Message::ShowError(
                        crate::user_error::UserError::logged(
                            "Couldn't finish enabling duress mode",
                            "Try again. If it keeps failing, contact support and quote the reference below.",
                            crate::user_error::CC_API_BADRESP,
                            true,
                            e,
                        )
                        .toast(),
                    ))),
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
                return self.handle_recovery_alerts_consent(accept);
            }
            Message::RescanObligationRetired(result) => {
                self.rescan_obligation_retire_in_flight = false;
                match result {
                    Ok(()) => self.rescan_obligation_retired_by_scan = true,
                    // Left unretired: the next refresh showing the completed
                    // scan writes it again.
                    Err(e) => tracing::warn!(
                        "Could not retire the pending rescan marker: {}. Retrying on the next refresh.",
                        e
                    ),
                }
                return Task::none();
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
                if let (Some(daemon), Some(vault_overview)) =
                    (&self.daemon, self.panels.vault_overview.as_mut())
                {
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
                    if let Some(vault_settings) = self.panels.vault_settings.as_mut() {
                        commands.push(vault_settings.update(
                            Some(daemon.clone()),
                            &cache,
                            Message::UpdatePanelCache(is_vault_settings_current),
                        ));
                    }

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
                // A successful switch invalidates the previous node probe and
                // clears its cached permission before polling the replacement.
                if result.is_ok() {
                    self.bitcoind_sync_probe_in_progress = false;
                    self.node_history_request_epoch =
                        self.node_history_request_epoch.wrapping_add(1);
                    self.node_history_in_progress = false;
                    self.node_history_pending = None;
                    self.cache.node_history = node_history::Status::default();
                    self.cache.node_history_switch_requested = false;
                    self.cache.node_bitcoind_sync_progress = None;
                    self.cache.node_bitcoind_sync_heights = None;
                    self.cache.node_bitcoind_ibd = None;
                    self.cache.node_bitcoind_subversion = None;
                    self.cache.node_bitcoind_pruning = None;
                    self.cache.local_switch_history = None;
                    self.cache.node_bitcoind_last_log = None;
                    self.local_switch_hold = None;
                }
                let cfg_task = self.update_dispatch(Message::DaemonConfigLoaded(result));
                return Task::batch([
                    cfg_task,
                    extra,
                    Task::done(Message::CacheUpdated),
                    Task::done(Message::PollBitcoindSync),
                ]);
            }
            Message::WalletUpdated(Ok(wallet)) => {
                // Check if we're transitioning from no-vault to has-vault state
                let was_vaultless = self.wallet.is_none();

                if self.wallet.as_ref().map(|w| w.id()) != Some(wallet.id()) {
                    // A permission to switch is specific to the probed Vault.
                    self.cache.local_switch_history = None;
                    self.local_switch_hold = None;
                    self.bitcoind_sync_probe_in_progress = false;
                    self.node_history_request_epoch =
                        self.node_history_request_epoch.wrapping_add(1);
                    self.node_history_in_progress = false;
                    self.node_history_pending = None;
                    self.cache.node_history = node_history::Status::default();
                    self.cache.node_history_switch_requested = false;
                    self.cache.local_node_transfer_visible = false;
                }
                self.wallet = Some(wallet.clone());
                self.imported_kit_metadata
                    .reload(&self.datadir, &self.cube_settings);
                self.cache.has_vault = wallet.supports_inheritance();
                self.cache.has_imported_bitcoin = wallet.is_imported();
                self.cache.vault_lacks_keychain_key = wallet.lacks_keychain_key();

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
                    let report_task = if wallet.supports_inheritance() {
                        self.panels.connect.report_vault_created()
                    } else {
                        Task::none()
                    };
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
            Message::LocalForkFeatureChecked { attempt, enabled } => {
                if self.local_feature_check != Some(attempt) {
                    return Task::none();
                }
                self.local_feature_check = None;
                self.local_feature_checked_at = Some(std::time::Instant::now());
                self.local_feature_valid = enabled;
                return Task::none();
            }
            Message::View(view::Message::ToggleCubeSwitcher) => {
                self.cache.connect_panel_open = false;
                self.cache.cube_switcher_open = !self.cache.cube_switcher_open;
                self.cache.cube_lock_confirmation = false;
                return iced::widget::operation::focus(view::nav::shell::CUBE_SWITCHER_ID);
            }
            Message::View(view::Message::AskCubeLock) => {
                self.cache.cube_switcher_open = true;
                self.cache.cube_lock_confirmation = true;
                return iced::widget::operation::focus(view::nav::shell::CUBE_LOCK_CANCEL_ID);
            }
            Message::View(view::Message::CancelCubeLock) => {
                self.cache.cube_lock_confirmation = false;
                return iced::widget::operation::focus(view::nav::shell::CUBE_LOCK_ID);
            }
            Message::View(view::Message::FocusAction(id)) => {
                return iced::widget::operation::focus(id.clone());
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
                self.cache.cube_switcher_open = false;
                self.cache.cube_lock_confirmation = false;
                self.cache.connect_panel_open = false;
                let nickname_task = self.save_pending_spark_nickname(&menu);
                return Task::batch([close_task, nickname_task, self.set_current_panel(menu)]);
            }
            // Activity's own results and intents reach it from any route: the
            // Vault and Spark pages show its rows and its detail pane.
            msg @ (Message::ActivityVaultLoaded(..)
            | Message::ActivityPsbtLoaded(..)
            | Message::ActivitySparkLoaded(..)
            | Message::ActivitySparkRefreshed(..)
            | Message::ActivityLiquidLoaded(..)
            | Message::ActivityRefundablesLoaded(..)
            | Message::ActivityLabelSaved(..)) => {
                return self.route_activity_result(msg);
            }
            Message::View(view::Message::Activity(view::activity::ActivityMessage::ViewAll(
                wallet,
            ))) => {
                return self.route_activity_view_all(wallet);
            }
            Message::View(view::Message::Activity(view::activity::ActivityMessage::Open(id))) => {
                return self.route_activity_open(id);
            }
            msg @ Message::View(view::Message::Activity(_)) => {
                return self.route_activity_view(msg);
            }
            msg @ Message::View(view::Message::SparkOverview(
                view::SparkOverviewMessage::DataLoaded { .. },
            )) if matches!(
                self.panels.current,
                Menu::Spark(menu::SparkSubMenu::Overview)
            ) =>
            {
                return self.route_activity_spark_overview(msg);
            }
            msg @ Message::Payments(Ok(_))
                if matches!(
                    self.panels.current,
                    Menu::Vault(menu::VaultSubMenu::Overview)
                ) =>
            {
                return self.route_activity_vault_overview(msg);
            }
            Message::ActivityOpenDetail(target) => {
                return self.route_activity_detail(target);
            }
            msg @ Message::View(view::Message::ConnectAccount(_))
            | msg @ Message::View(view::Message::ConnectCube(_)) => {
                let mut committed_login = None;
                if let Message::View(view::Message::ConnectAccount(
                    view::ConnectAccountMessage::AuthCommitAccepted {
                        login,
                        epoch,
                        refreshed,
                    },
                )) = &msg
                {
                    let network_dir = self
                        .cache
                        .datadir_path
                        .network_directory(self.cache.network);
                    let still_current =
                        crate::services::connect::client::cache::ConnectCache::from_file(
                            &network_dir,
                        )
                        .ok()
                        .and_then(|cache| cache.active_account().cloned())
                        .is_some_and(|account| {
                            account.email == login.user.email
                                && account.tokens.access_token == login.token
                                && account.tokens.refresh_token == login.refresh_token
                        });
                    if !still_current {
                        return Task::done(Message::View(view::Message::ConnectAccount(
                            view::ConnectAccountMessage::AuthCommitRejected { epoch: *epoch },
                        )));
                    }
                    committed_login = Some((login.clone(), *refreshed));
                }
                if self.cache.chain().is_blake2b() && !self.requires_connect() {
                    self.panels.connect.account.use_local_backend();
                }
                let was_authenticated = self.cache.connect_authenticated;
                let previous_server_cube_id = self.panels.connect.cube.server_cube_id;
                let explicit_logout = matches!(
                    &msg,
                    Message::View(view::Message::ConnectAccount(
                        view::ConnectAccountMessage::LogOut
                    ))
                );
                // The two completions that establish a session in this tab —
                // a login or refresh result, and the user it loads — carry the
                // authentication epoch of the operation that produced them.
                // Nothing else (a features, plan, flags or activity load, a
                // re-entry) lifts a hold on the claim's session, and neither
                // does a completion of an operation begun before the hold
                // (see `claim_session_invalidated`).
                let session_epoch = match &msg {
                    Message::View(view::Message::ConnectAccount(
                        view::ConnectAccountMessage::SetSession(_, epoch)
                        | view::ConnectAccountMessage::AuthCommitAccepted { epoch, .. }
                        | view::ConnectAccountMessage::SessionLoaded { epoch, .. },
                    )) => Some(*epoch),
                    _ => None,
                };
                // Features and the post-login account check complete independently.
                // A pending account check is not a revocation of this admitted Cube.
                let current_features_denied = matches!(
                    &msg,
                    Message::View(view::Message::ConnectAccount(
                        view::ConnectAccountMessage::FeaturesLoaded(features, generation)
                    )) if *generation == self.panels.connect.account.session_generation()
                        && !crate::app::features::bitcoin_blake2b_enabled(
                            features.as_ref().and_then(|f| f.bitcoin_blake2b_enabled).unwrap_or(false),
                            self.panels.connect.account.bitcoin_blake2b_opt_in,
                        )
                );
                if matches!(
                    &msg,
                    Message::View(view::Message::ConnectAccount(
                        view::ConnectAccountMessage::RefreshSession { .. }
                    ))
                ) && !self.panels.connect.account.is_authenticated()
                {
                    use crate::services::connect::client::cache::{
                        ConnectCache, ConnectCacheError,
                    };
                    let network_dir = self
                        .cache
                        .datadir_path
                        .network_directory(self.cache.network);
                    let auth = self
                        .connect_auth
                        .as_ref()
                        .map(|arc| arc.try_read().map(|tokens| (arc.clone(), tokens.clone())));
                    self.in_app_restore_cache_snapshot =
                        match (ConnectCache::from_file(&network_dir), auth) {
                            (Ok(cache), Some(Ok(auth))) => Some(RestoreAuthSnapshot {
                                cache,
                                email: self.connect_email.clone(),
                                auth: Some(auth),
                            }),
                            (Ok(cache), None) => Some(RestoreAuthSnapshot {
                                cache,
                                email: self.connect_email.clone(),
                                auth: None,
                            }),
                            (Err(ConnectCacheError::NotFound), Some(Ok(auth))) => {
                                Some(RestoreAuthSnapshot {
                                    cache: ConnectCache::default(),
                                    email: self.connect_email.clone(),
                                    auth: Some(auth),
                                })
                            }
                            (Err(ConnectCacheError::NotFound), None) => Some(RestoreAuthSnapshot {
                                cache: ConnectCache::default(),
                                email: self.connect_email.clone(),
                                auth: None,
                            }),
                            (_, Some(Err(error))) => {
                                tracing::warn!(
                                    "Cannot capture Connect restore auth state: {error}"
                                );
                                None
                            }
                            (Err(error), _) => {
                                tracing::warn!(
                                    "Cannot capture Connect restore cache state: {error}"
                                );
                                None
                            }
                        };
                }
                if matches!(
                    &msg,
                    Message::View(view::Message::ConnectAccount(
                        view::ConnectAccountMessage::VerifyOtp
                    ))
                ) && self.panels.connect.account.can_begin_verify_otp()
                {
                    use crate::services::connect::client::cache::{
                        ConnectCache, ConnectCacheError,
                    };
                    let network_dir = self
                        .cache
                        .datadir_path
                        .network_directory(self.cache.network);
                    self.in_app_otp_cache_snapshot = match ConnectCache::from_file(&network_dir) {
                        Ok(cache) => Some(cache),
                        Err(ConnectCacheError::NotFound) => Some(ConnectCache::default()),
                        Err(error) => {
                            tracing::warn!("Cannot capture Connect OTP cache state: {error}");
                            None
                        }
                    };
                }
                let auth_revision_before = self.panels.connect.account.accepted_auth_revision();
                let task = self
                    .panels
                    .connect
                    .update(self.daemon.clone(), &self.cache, msg);
                let mut auth_bootstrap = Task::none();
                if let Some((login, refreshed)) = committed_login {
                    if self.panels.connect.account.accepted_auth_revision() != auth_revision_before
                    {
                        let needs_new_arc = self.connect_email.as_deref()
                            != Some(login.user.email.as_str())
                            || self.connect_auth.is_none();
                        if needs_new_arc {
                            self.connect_stream_config = None;
                            self.cache.connect_grpc_url = None;
                            self.cache.connect_device_id = None;
                            let tokens = std::sync::Arc::new(tokio::sync::RwLock::new(
                                crate::services::connect::client::auth::AccessTokenResponse {
                                    access_token: login.token.clone(),
                                    refresh_token: login.refresh_token.clone(),
                                    expires_at: chrono::Utc::now().timestamp() + 30 * 24 * 60 * 60,
                                },
                            ));
                            self.connect_auth = Some(tokens.clone());
                            self.cache.connect_tokens = Some(tokens);
                        }
                        self.connect_email = Some(login.user.email.clone());
                        self.cache.connect_email = Some(login.user.email);
                        self.cache.has_connect_session = true;
                        if refreshed && needs_new_arc {
                            auth_bootstrap = Task::done(Message::InAppConnectLoginCompleted {
                                token: login.token,
                                refresh_token: login.refresh_token,
                                email: self.connect_email.clone().expect("just installed"),
                                epoch: self.panels.connect.account.auth_epoch(),
                            });
                        }
                    }
                }
                self.cache.connect_authenticated = self.panels.connect.account.is_authenticated();
                self.cache.restorable_connect_session =
                    self.panels.connect.account.has_stored_session();
                let refresh_recovery_kit = if self.panels.connect.account.is_authenticated()
                    && self.panels.connect.cube.server_cube_id.is_some()
                    && (!was_authenticated || previous_server_cube_id.is_none())
                {
                    Task::done(Message::View(view::Message::Settings(
                        view::SettingsMessage::RecoveryKit(view::RecoveryKitMessage::LoadStatus),
                    )))
                } else {
                    Task::none()
                };
                if let Some(bound) = &self.fork_connect_client {
                    let current = self.panels.connect.account.authenticated_client();
                    let changed = current.as_ref().is_some_and(|current| {
                        current.base_url != bound.base_url || current.token() != bound.token()
                    });
                    if explicit_logout || changed || current.is_none() {
                        if self.requires_connect() {
                            self.invalidate_fork_session();
                        } else {
                            self.revoke_local_connect_services();
                        }
                    }
                }
                // Mirror the server-controlled Marketplace flags so the nav
                // rails and route guard reflect the latest `/connect/features`
                // stance. Recomputed after every ConnectAccount/ConnectCube
                // message (FeaturesLoaded flows through here, and logout clears
                // `features` back to fail-closed OFF via the accessor).
                self.cache.marketplace_flags =
                    self.panels.connect.account.marketplace_server_flags();
                // Same mirror for the fork account grant, so the Vault rail can
                // answer "can this Cube start a claim?" without reaching into
                // the Connect panel from the view layer.
                self.cache.btcb2_server_enabled =
                    self.panels.connect.account.bitcoin_blake2b_enabled();
                if self.requires_connect() && current_features_denied {
                    self.invalidate_fork_session();
                    return Task::none();
                }
                // Connect-session reads (the unswept notice, the Chains
                // column's proofs) work under the account's session. A
                // sign-out, or a different account or client identity, revokes every
                // Connect-session read here, synchronously, before anything
                // else acts on the change, and advances the generation.
                if session_epoch.is_some_and(|epoch| epoch >= self.claim_hold_epoch) {
                    self.claim_session_invalidated = false;
                }
                let claim_signed_in =
                    !explicit_logout && self.panels.connect.account.bitcoin_blake2b_enabled();
                let claim_session = if claim_signed_in {
                    self.claim_connect_session()
                } else {
                    None
                };
                let notice_replaced = self.unswept_session.as_ref().is_some_and(|old| {
                    !self.cache.btcb2_server_enabled
                        || claim_session
                            .as_ref()
                            .is_none_or(|new| !crate::app::connect_session::same_session(old, new))
                });
                // A first sign-in replaces nothing; another account,
                // endpoint or credential (or a sign-out) does.
                let previous_session =
                    std::mem::replace(&mut self.seen_session, claim_session.clone());
                let session_replaced = match (&previous_session, &claim_session) {
                    (Some(old), Some(new)) => !crate::app::connect_session::same_session(old, new),
                    (Some(_), None) => true,
                    (None, _) => false,
                };
                if !claim_signed_in || session_replaced || notice_replaced {
                    self.revoke_claim();
                }
                if self.cache.chain().is_blake2b() {
                    self.cache.marketplace_flags = Default::default();
                }
                // Same mirror for the Liquid sunset grant. Only the server half
                // is refreshed here — `local_state_exists` reflects whether the
                // Liquid SDK actually connected at cube-open, and must survive a
                // logout: losing the session must never hide an existing wallet.
                let liquid_granted = !self.cache.chain().is_blake2b()
                    && self.panels.connect.account.liquid_server_enabled();
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
                        crate::services::coincube::display_lightning_address(
                            la.lightning_address.as_deref(),
                        )
                    });
                self.cache.lightning_address_setup = self.panels.connect.cube.ln_auto.clone();
                self.panels.spark_receive.sync_lightning_address_qr(
                    self.cache
                        .connect_authenticated
                        .then_some(self.cache.lightning_address.as_deref())
                        .flatten(),
                );
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
                self.cache.avatar_regenerations_remaining = self
                    .panels
                    .connect
                    .cube
                    .avatar_data
                    .as_ref()
                    .map(|data| data.regenerations_remaining);
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
                if !was_authenticated && self.cache.connect_authenticated {
                    self.cache.connect_signed_out_by = None;
                }
                if was_authenticated && !self.cache.connect_authenticated {
                    self.connect_renewal = Renewal::default();
                    self.cache.connect_panel_open = false;
                    self.connect_stream_config = None;
                    self.connect_email = None;
                    self.cache.connect_grpc_url = None;
                    self.cache.connect_tokens = None;
                    self.cache.connect_device_id = None;
                    self.cache.connect_email = None;
                    self.cache.has_connect_session = false;
                    self.cache.lightning_address = None;
                    self.panels.spark_receive.sync_lightning_address_qr(None);
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
                    return Task::batch([
                        task,
                        auth_bootstrap,
                        refresh_recovery_kit,
                        persist_grant,
                        nav,
                        switch,
                    ]);
                }
                return Task::batch([task, auth_bootstrap, refresh_recovery_kit, persist_grant]);
            }
            Message::View(view::Message::ReceivedCelebrationActivity) => {
                return Task::done(Message::View(view::Message::DismissReceivedCelebration)).chain(
                    Task::done(Message::View(view::Message::Menu(Menu::Cube(
                        crate::app::menu::CubeSubMenu::Activity,
                    )))),
                );
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
            Message::View(view::Message::DismissSparkNotice) => {
                // Dismissing "USDB held but off" is an answer — keep it off
                // and don't ask again on this device. The paused banner is
                // just informational; closing it changes nothing.
                if self.cache.spark_notice == Some(cache::SparkNotice::StableBalanceOffWithHolding)
                {
                    return self.record_spark_stable_balance(false);
                }
                self.cache.spark_notice = None;
            }
            Message::CubeSparkStableBalanceReported => {}
            Message::SparkStableBalanceFromConnect(server) => {
                // Adopt the server's record only when this device has none:
                // the registration request already carried a local decision,
                // so a response that disagrees with one is a stale answer to
                // an earlier request (the user toggled while it was in
                // flight), not newer information. With no local decision the
                // server's is the only one there is — adopt it, then re-check
                // the SDK against it. `None` on both sides is the fresh-Cube
                // case; a local decision with no server record was pushed by
                // the registration itself.
                match (self.cube_settings.spark_stable_balance, server) {
                    (None, Some(enabled)) => {
                        log::info!(
                            "Spark Stable Balance decision from Connect: {}",
                            if enabled { "on" } else { "off" }
                        );
                        return Task::batch([
                            self.record_spark_stable_balance_locally(enabled),
                            self.spark_stable_balance_state_task(),
                        ]);
                    }
                    (Some(local), Some(remote)) if local != remote => {
                        log::debug!(
                            "Connect reports Spark Stable Balance {remote} but this device \
                             decided {local}; keeping the local decision"
                        );
                    }
                    _ => {}
                }
            }
            Message::SparkStableBalanceState(None) => {}
            Message::SparkStableBalanceState(Some((sdk_active, holds_usdb))) => {
                let preference = self.cube_settings.spark_stable_balance;
                match reconcile_stable_balance(preference, sdk_active, holds_usdb) {
                    StableBalanceReconcile::InSync => {}
                    StableBalanceReconcile::Apply(enabled) => {
                        let Some(backend) = self.spark_backend() else {
                            return Task::none();
                        };
                        log::info!(
                            "Spark Stable Balance is {} on the SDK but the Cube wants it {}; \
                             re-applying",
                            if sdk_active { "on" } else { "off" },
                            if enabled { "on" } else { "off" },
                        );
                        return Task::perform(
                            async move { backend.set_stable_balance(enabled).await },
                            move |res| {
                                Message::SparkStableBalanceApplied(
                                    res.map(|()| enabled).map_err(|e| e.to_string()),
                                )
                            },
                        );
                    }
                    StableBalanceReconcile::Adopt => {
                        return self.record_spark_stable_balance(true);
                    }
                    StableBalanceReconcile::PromptHolding => {
                        self.cache.spark_notice =
                            Some(cache::SparkNotice::StableBalanceOffWithHolding);
                    }
                }
            }
            Message::SparkStableBalanceApplied(Ok(enabled)) => {
                log::info!(
                    "Spark Stable Balance preference re-applied: {}",
                    if enabled { "on" } else { "off" }
                );
                // The Settings toggle and Overview badge read the SDK on
                // reload; refresh both so they don't show the stale state.
                return Task::batch([
                    self.panels.spark_settings.reload(None, None),
                    self.panels.spark_overview.reload(None, None),
                ]);
            }
            Message::SparkStableBalanceApplied(Err(e)) => {
                log::warn!("could not re-apply Spark Stable Balance preference: {e}");
            }

            // Swap's recent payments are priced with the cache, which
            // `deliver` doesn't have.
            Message::View(view::Message::SparkSwap(
                view::spark::SparkSwapMessage::PaymentsLoaded(payments),
            )) => {
                self.panels
                    .spark_swap
                    .set_recent_payments(&payments, &self.cache);
            }
            // Swap's answers reach its panel whichever page is showing, so a
            // swap that settles while the user is elsewhere keeps its result.
            Message::View(view::Message::SparkSwap(msg)) => {
                let on_screen = self.panels.current == Menu::Spark(menu::SparkSubMenu::Swap);
                return self.panels.spark_swap.deliver(msg, on_screen);
            }

            // The user changed the Stable Balance toggle and the bridge
            // accepted it: record the decision before the panel updates its
            // own state, then let it through. Routed to the settings panel
            // directly (not `current_mut()`) so navigating away mid-save
            // can't lose the reconcile.
            Message::View(view::Message::SparkSettings(
                view::SparkSettingsMessage::StableBalanceSaved(Ok(enabled)),
            )) => {
                self.panels.spark_overview.finish_stable_balance_save();
                let record = self.record_spark_stable_balance(enabled);
                let panel = self.panels.spark_settings.update(
                    self.daemon.clone(),
                    &self.cache,
                    Message::View(view::Message::SparkSettings(
                        view::SparkSettingsMessage::StableBalanceSaved(Ok(enabled)),
                    )),
                );
                let receive = self
                    .panels
                    .spark_receive
                    .stable_balance_settled(Some(enabled));
                return Task::batch([
                    record,
                    panel,
                    self.panels.spark_overview.reload(None, None),
                    receive,
                ]);
            }
            // Overview shares the same Settings save owner. Route both the
            // request and failed settlement there even when Overview is active.
            Message::View(view::Message::SparkSettings(
                view::SparkSettingsMessage::StableBalanceToggled(enabled),
            )) => {
                if !self.panels.spark_overview.try_start_stable_balance_save() {
                    return Task::none();
                }
                return self.panels.spark_settings.update(
                    self.daemon.clone(),
                    &self.cache,
                    Message::View(view::Message::SparkSettings(
                        view::SparkSettingsMessage::StableBalanceToggled(enabled),
                    )),
                );
            }
            // Spark's nickname, from Settings › Spark › Balances & conversion.
            // It lives on this device's Cube settings (Spark has no wallet
            // settings of its own) and isn't sent to Connect.
            Message::View(view::Message::SparkSettings(
                view::SparkSettingsMessage::NicknameSubmitted(input),
            )) => {
                if !state::spark::settings::nickname_fits(&input) {
                    return Task::none();
                }
                let nickname = state::spark::settings::saved_nickname(&input);
                // A write is running: this request waits for it, so the
                // latest request is the one that lands, even one going back
                // to the saved name.
                if self.spark_nickname_in_flight.is_some() {
                    self.spark_nickname_queued = Some(nickname);
                    return Task::none();
                }
                // So is one going back to the saved name while an earlier
                // App's write for this Cube is still running (Lock Cube, then
                // reopen): that write would land after the shortcut.
                if nickname == self.cube_settings.spark_alias
                    && !SparkNicknameWrite::pending(
                        &self.spark_nickname_network_dir(),
                        &self.cube_settings.id,
                    )
                {
                    return self.panels.spark_settings.update(
                        self.daemon.clone(),
                        &self.cache,
                        Message::View(view::Message::SparkSettings(
                            view::SparkSettingsMessage::NicknameSaved(nickname),
                        )),
                    );
                }
                return self.start_spark_nickname_write(nickname);
            }
            // Another Cube's save (the tab moved on while it was written)
            // changes nothing here: its file write already happened.
            Message::SparkNicknameSaved { cube_id, .. } if cube_id != self.cube_settings.id => {}
            Message::SparkNicknameSaved { result, .. } => {
                self.spark_nickname_in_flight = None;
                let mut tasks = Vec::new();
                match result {
                    Ok(nickname) => {
                        // The side panel's name follows:
                        // `sync_panel_derived_cache_fields` rebuilds
                        // `cache.wallet_names` from the Cube settings after
                        // this message, before the next view.
                        self.cube_settings.spark_alias = nickname.clone();
                        tasks.push(self.panels.spark_settings.update(
                            self.daemon.clone(),
                            &self.cache,
                            Message::View(view::Message::SparkSettings(
                                view::SparkSettingsMessage::NicknameSaved(nickname),
                            )),
                        ));
                    }
                    Err(e) => tasks.push(Task::done(Message::View(view::Message::ShowError(
                        coincube_ui::localized_format!(
                            "Couldn't save the Spark nickname: {}",
                            "No se pudo guardar el apodo de Spark: {}",
                            e
                        ),
                    )))),
                }
                // A request made while this write ran is written now, after
                // it, unless it is what is saved.
                if let Some(next) = self.spark_nickname_queued.take() {
                    if next == self.cube_settings.spark_alias {
                        tasks.push(self.panels.spark_settings.update(
                            self.daemon.clone(),
                            &self.cache,
                            Message::View(view::Message::SparkSettings(
                                view::SparkSettingsMessage::NicknameSaved(next),
                            )),
                        ));
                    } else {
                        tasks.push(self.start_spark_nickname_write(next));
                    }
                }
                return Task::batch(tasks);
            }
            Message::View(view::Message::SparkSettings(
                view::SparkSettingsMessage::StableBalanceSaved(Err(err)),
            )) => {
                self.panels.spark_overview.finish_stable_balance_save();
                let panel = self.panels.spark_settings.update(
                    self.daemon.clone(),
                    &self.cache,
                    Message::View(view::Message::SparkSettings(
                        view::SparkSettingsMessage::StableBalanceSaved(Err(err.clone())),
                    )),
                );
                return Task::batch([
                    panel,
                    self.panels.spark_overview.reload(None, None),
                    self.panels.spark_receive.stable_balance_settled(None),
                    Task::done(Message::View(view::Message::ShowError(err))),
                ]);
            }
            Message::View(view::Message::FlipDisplayMode) => {
                if !self.cache.fiat_toggle_allowed() {
                    self.cache.clear_btcb2_fiat();
                    return Task::done(Message::CacheUpdated);
                }
                let new_mode = self.cache.display_mode.flipped();
                self.cache.display_mode = new_mode;
                let network_dir = self.datadir.network_directory(self.cube_settings.network);
                return Task::perform(
                    async move {
                        settings::update_settings_file(&network_dir, move |mut current| {
                            current.display_mode = new_mode;
                            Some(current)
                        })
                        .await
                    },
                    |res| match res {
                        Ok(()) => Message::SettingsSaved,
                        Err(e) => Message::SettingsSaveFailed(e.into()),
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
                if let Err(e) = crate::browser::open_url(&url) {
                    tracing::error!("Error opening '{}': {}", url, e);
                }
            }
            Message::View(view::Message::Clipboard(text)) => return clipboard::write(text),
            Message::View(view::Message::CopyWithToast(text, notice)) => {
                return clipboard::write(text.clone()).chain(clipboard::read().map(
                    move |copied| {
                        let (level, message) = if copied.as_deref() == Some(text.as_str()) {
                            (log::Level::Info, notice.clone())
                        } else {
                            (
                                log::Level::Error,
                                "Could not copy to clipboard. Please try again.".to_string(),
                            )
                        };
                        Message::View(view::Message::ShowToast(level, message))
                    },
                ));
            }
            Message::View(view::Message::Home(view::HomeMessage::TransferFromWallet(wallet))) => {
                // A wallet page's Transfer: the flow lives on the Overview
                // route, so switch there (the side menu keeps the wallet
                // selected), then open it from this wallet. A press that
                // raced the button's state changing does nothing.
                if self
                    .panels
                    .global_home
                    .wallet_transfer_entry(&self.cache, wallet)
                    != view::global_home::WalletTransferEntry::Ready
                {
                    return Task::none();
                }
                let switch = self.update(Message::View(view::Message::Menu(Menu::Cube(
                    menu::CubeSubMenu::Overview,
                ))));
                let start = self.panels.global_home.start_transfer_from(
                    self.daemon.clone(),
                    &self.cache,
                    wallet,
                );
                return Task::batch([switch, start]);
            }
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
                self.received_celebration_amount = format!(
                    "{} {}",
                    bitcoin::Amount::from_sat(amount_sat)
                        .to_formatted_string_with_unit(self.cache.bitcoin_unit),
                    self.cache.bitcoin_unit.label()
                );
                self.received_celebration_quote =
                    coincube_ui::component::quote_display::random_quote(&context);
                self.received_celebration_image =
                    coincube_ui::component::quote_display::image_handle_for_context(&context);
                self.received_celebration_context = context.clone();
                self.received_celebration_wallet = "Into Spark";
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
                    // First sync of the session: check the SDK's Stable
                    // Balance toggle against the Cube's recorded decision
                    // (and its USDB holding). Once per session — the
                    // outcome is durable, and later ticks have nothing new.
                    if !self.spark_stable_balance_reconciled {
                        self.spark_stable_balance_reconciled = true;
                        tasks.push(self.spark_stable_balance_state_task());
                    }
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
                // `PaymentUpdated` is late metadata on a payment that is
                // already listed: refresh the page in view (and any open
                // detail pane) without sending the user back to page 0.
                if matches!(event, SparkEvent::PaymentUpdated { .. }) {
                    tasks.push(self.panels.spark_transactions.refresh_in_place());
                }
                // The Spark page's rows follow its own reload (`DataLoaded`).
                if matches!(self.panels.current, Menu::Cube(menu::CubeSubMenu::Overview))
                    && overview_activity_spark_event(&event)
                {
                    tasks.push(self.panels.activity.refresh_wallet(
                        view::activity::WalletFilter::Spark,
                        None,
                        None,
                    ));
                }

                match event {
                    SparkEvent::PaymentSucceeded {
                        amount_sat,
                        bolt11,
                        token,
                        ..
                    } => {
                        // Phase 4f: forward the BOLT11 field so the
                        // Receive panel can correlate against its
                        // currently displayed invoice. A token payment's
                        // amount is in token base units, so its token goes
                        // along to be read with it.
                        let is_token = token.is_some();
                        tasks.push(Task::done(Message::View(view::Message::SparkReceive(
                            view::SparkReceiveMessage::PaymentReceived {
                                amount_sat,
                                bolt11,
                                token,
                            },
                        ))));
                        // An auto-claim accepted before maturity settles
                        // through this event; Home needs the settled amount
                        // to complete its indicator and fire the splash. A
                        // token payment carries no sats.
                        if let Ok(amount_sat) =
                            <u64 as std::convert::TryFrom<i64>>::try_from(amount_sat)
                        {
                            if amount_sat > 0 && !is_token {
                                tasks.push(Task::done(Message::View(view::Message::Home(
                                    view::HomeMessage::SparkPaymentSucceeded { amount_sat },
                                ))));
                            }
                        }
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
                    SparkEvent::StableBalancePaused { reason, failures } => {
                        log::error!(
                            "Spark bridge paused Stable Balance after {failures} failed \
                             auto-conversions: {reason}"
                        );
                        // The bridge already flipped the SDK setting off;
                        // the Settings toggle and Overview badge re-read it
                        // on reload. Record "off" as the Cube's decision so
                        // the next launch's reconcile doesn't re-arm the
                        // loop, then show the banner — after the record,
                        // which clears any banner it finds.
                        tasks.push(self.record_spark_stable_balance(false));
                        self.cache.spark_notice =
                            Some(cache::SparkNotice::StableBalancePaused { reason });
                        tasks.push(self.panels.spark_settings.reload(None, None));
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
                        let Some(client) = self.breez_client.clone() else {
                            return Task::none();
                        };

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
                        // Show global celebration for incoming payments. Hidden
                        // Liquid celebrates nothing: the amount appears in no
                        // balance or row (Tenshu v175 master §3 decision 2).
                        if matches!(details.payment_type, PaymentType::Receive)
                            && self.cache.liquid_gate.show()
                        {
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
                                format!(
                                    "{} {}",
                                    bitcoin::Amount::from_sat(details.amount_sat)
                                        .to_formatted_string_with_unit(self.cache.bitcoin_unit),
                                    self.cache.bitcoin_unit.label()
                                )
                            };
                            self.received_celebration_context = context.to_string();
                            self.received_celebration_wallet = "Into Liquid";
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
                        // dedupe by tx_id to avoid stacking duplicate toasts. Hidden
                        // Liquid raises no toast, as with the celebration above.
                        if matches!(details.payment_type, PaymentType::Receive)
                            && self.cache.liquid_gate.show()
                            && details.tx_id.as_ref().is_some_and(|id| {
                                !self.toasted_incoming_waiting_tx_ids.contains(id)
                            })
                        {
                            let tx_id = details.tx_id.clone().unwrap();
                            if self.toasted_incoming_waiting_tx_ids.len() == 16 {
                                self.toasted_incoming_waiting_tx_ids.pop_front();
                            }
                            self.toasted_incoming_waiting_tx_ids.push_back(tx_id);
                            tasks.push(Task::done(Message::View(
                                view::Message::ShowIncomingLiquidWaitingToast(details.amount_sat),
                            )));
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
                return self.route_recovery_kit(msg);
            }

            // Inline Cube rename on Settings → Display & appearance. Mirrors
            // the Launcher: write this device's Cube settings first (marked
            // unsynced, so catch-up retries), then update Connect.
            Message::View(view::Message::Settings(view::SettingsMessage::CubeNameSubmitted(
                name,
            ))) => {
                return self.route_cube_name_submitted(name);
            }
            Message::CubeNameSaved(result) => {
                return self.route_cube_name_saved(result);
            }
            Message::CubeNameSynced { cube_id, result } => {
                return self.route_cube_name_synced(cube_id, result);
            }
            // Vault Recovery Alerts dispatch (Estate Notifications — PR 2).
            // Like the Recovery Kit above, the handler needs the
            // authenticated client, the Connect cube id, the live wallet
            // descriptor, and the `recovery_alerts` entitlement — none plumbed
            // through `State::update`. The keyholder list is *not* injected:
            // it's derived from the vault the handler's own load fetches.
            Message::View(view::Message::Settings(view::SettingsMessage::RecoveryAlerts(msg))) => {
                return self.route_recovery_alerts(msg);
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
                        let Some(panel) = self.panels.liquid_transactions.as_mut() else {
                            return Task::none();
                        };
                        return panel.update(
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
                let Some(panel) = self.panels.liquid_transactions.as_mut() else {
                    return Task::none();
                };
                return panel.update(self.daemon.clone(), &self.cache, msg);
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
    /// Called before logout, account replacement or an API-provider change.
    /// A new authenticated startup is required; an old authority is never reused.
    /// Local wallet authority comes from its admitted managed node, independently
    /// of Connect sessions. Account changes only withdraw Connect services.
    /// Start one check of this fork Cube's feature. A Vault-less Cube that
    /// is signed in checks its account's own flag, as its unlock did; every
    /// other fork Cube not served by Connect checks the global flag.
    fn fork_feature_check(&mut self) -> Task<Message> {
        // Unique across App instances, as for the background node install.
        static NEXT_ATTEMPT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let attempt = NEXT_ATTEMPT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.local_feature_check = Some(attempt);
        let root = self.datadir.clone();
        let chain = self.cache.chain();
        let client = self.fork_connect_client.clone();
        let account = self.daemon.is_none();
        Task::perform(
            async move {
                match client {
                    Some(client) if account => {
                        crate::chain::require_connect_feature(chain, &client, &root)
                            .await
                            .is_ok()
                    }
                    client => crate::chain::require_local_feature(chain, client, &root)
                        .await
                        .is_ok(),
                }
            },
            move |enabled| Message::LocalForkFeatureChecked { attempt, enabled },
        )
    }

    /// A fork Cube whose feature check has not answered yet: nothing that
    /// leaves the App (Create Vault) may start until it has.
    pub(crate) fn fork_feature_unconfirmed(&self) -> bool {
        self.cache.chain().is_blake2b()
            && !self.requires_connect()
            && self.local_feature_check.is_some()
            && self.local_feature_checked_at.is_none()
    }

    pub(crate) fn local_feature_revoked(&self) -> bool {
        self.cache.chain().is_blake2b() && !self.requires_connect() && !self.local_feature_valid
    }

    pub(crate) fn requires_connect(&self) -> bool {
        // A fork Cube with no Vault has no backend that a Connect session
        // serves: it stays open signed out, under the local feature check.
        self.cache.chain().is_blake2b()
            && self.daemon.as_ref().is_some_and(|d| {
                !d.config()
                    .is_some_and(|cfg| crate::chain::is_managed_local_fork(cfg, &self.datadir))
            })
    }

    pub(crate) fn on_local_auth_change(&mut self, originated: bool) {
        self.revoke_local_connect_services();
        if !originated {
            self.panels.connect.detach_local_session();
        }
    }

    pub(crate) fn revoke_local_connect_services(&mut self) {
        self.revoke_claim();
        self.fork_connect_client = None;
        self.panels.connect.account.use_local_backend();
        self.cache.has_connect_session = false;
        self.cache.lightning_address = None;
        self.panels.spark_receive.sync_lightning_address_qr(None);
        self.cache.restorable_connect_session = false;
        self.cache.connect_authenticated = false;
        self.cache.connect_tokens = None;
        self.cache.connect_email = None;
        self.cache.btcb2_server_enabled = false;
        self.connect_auth = None;
        self.connect_email = None;
        self.connect_stream_config = None;
        self.cache.clear_btcb2_fiat();
    }

    pub fn invalidate_fork_session(&mut self) {
        if self.cache.chain().is_blake2b() {
            self.revoke_claim();
            if let Some(daemon) = &self.daemon {
                daemon.invalidate_connect_session();
            }
            if let Some(node) = self.internal_bitcoind.take() {
                node.stop();
            }
            self.fork_connect_client = None;
            self.panels.connect.revoke_admitted_client();
            self.cache.has_connect_session = false;
            self.cache.lightning_address = None;
            self.panels.spark_receive.sync_lightning_address_qr(None);
            self.cache.restorable_connect_session = false;
            self.cache.connect_authenticated = false;
            self.cache.connect_tokens = None;
            self.cache.connect_email = None;
            self.connect_auth = None;
            self.connect_email = None;
            self.connect_stream_config = None;
            self.cache.cube_encryption_key = None;
            self.cache.clear_btcb2_fiat();
        }
    }

    pub fn spawn_daemon_switch(&mut self, cfg: DaemonConfig) -> Task<Message> {
        // A generic backend switch cannot carry fresh authenticated admission.
        // Reopen the fork Cube through its authenticated startup route instead.
        if cfg.bitcoin_config.chain != self.cache.chain()
            || (cfg.bitcoin_config.chain.is_blake2b()
                && self.fork_connect_client.is_none()
                && !crate::chain::is_managed_local_fork(&cfg, &self.datadir))
        {
            return Task::done(Message::View(view::Message::ShowError(
                "Reopen this Bitcoin Blake2b Cube to change its authenticated Connect backend."
                    .into(),
            )));
        }
        // Connect-session reads bound to the daemon being replaced are
        // revoked, synchronously, before the switch is dispatched.
        self.local_mcp.revoke();
        self.revoke_claim();
        // Mark a switch in flight so subsequent sync probes / triggers don't
        // re-fire it before it completes (the config only changes on success).
        self.daemon_switch_in_progress = true;
        // Mirror into the cache so the Node settings view reflects it.
        self.cache.daemon_switch_in_progress = true;
        match &cfg.bitcoin_backend {
            Some(coincubed::config::BitcoinBackend::Bitcoind(local))
                if self
                    .daemon
                    .as_ref()
                    .and_then(|d| d.config())
                    .is_some_and(|old| {
                        matches!(
                            old.bitcoin_backend,
                            Some(coincubed::config::BitcoinBackend::Esplora(_))
                        )
                    })
                    && crate::node::retention::is_managed_endpoint(
                        &self.cache.datadir_path,
                        self.cache.network,
                        self.cache.chain(),
                        local,
                    ) =>
            {
                self.cache.local_node_transfer_visible = true
            }
            Some(coincubed::config::BitcoinBackend::Esplora(_)) => {
                self.cache.local_node_transfer_visible = false;
            }
            _ => {}
        }
        let old_daemon = self.daemon.clone();
        let chain = cfg.bitcoin_config.chain;
        let connect_client = self.fork_connect_client.clone();
        let local_root = self.datadir.clone();
        let wallet_id = self.wallet.as_ref().expect("wallet should exist").id();
        let mut daemon_config_path = self
            .cache
            .datadir_path
            .network_directory(chain)
            .coincubed_data_directory(&wallet_id)
            .path()
            .to_path_buf();
        daemon_config_path.push("daemon.toml");
        Task::perform(
            async move {
                match tokio::task::spawn_blocking(move || {
                    restart_daemon_blocking_guarded(
                        old_daemon,
                        cfg,
                        daemon_config_path,
                        connect_client,
                        Some(local_root),
                    )
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

    /// Leaving Spark's Balances & conversion for `next` saves a nickname
    /// that was typed but not submitted, as Return does (the Cube name does
    /// the same on Display & appearance).
    fn save_pending_spark_nickname(&self, next: &Menu) -> Task<Message> {
        let on_balances = |route: &Menu| {
            matches!(
                route,
                Menu::Spark(menu::SparkSubMenu::Settings(
                    None | Some(menu::SparkSettingsOption::General)
                ))
            )
        };
        if !on_balances(&self.panels.current) || on_balances(next) {
            return Task::none();
        }
        match self
            .panels
            .spark_settings
            .pending_nickname(self.latest_spark_nickname().as_deref())
        {
            Some(input) => Task::done(Message::View(view::Message::SparkSettings(
                view::SparkSettingsMessage::NicknameSubmitted(input),
            ))),
            None => Task::none(),
        }
    }

    /// The Spark nickname as last requested: queued, else being written, else
    /// saved.
    fn latest_spark_nickname(&self) -> Option<String> {
        self.spark_nickname_queued
            .clone()
            .or_else(|| self.spark_nickname_in_flight.clone())
            .unwrap_or_else(|| self.cube_settings.spark_alias.clone())
    }

    fn spark_nickname_network_dir(&self) -> crate::dir::NetworkDirectory {
        self.datadir.network_directory(self.cube_settings.network)
    }

    /// Write `nickname` to this Cube's settings; [`Message::SparkNicknameSaved`]
    /// answers. Callers start one only while none is running, so this App's
    /// requests are coalesced; the write itself follows every earlier one
    /// for this Cube, from any App.
    fn start_spark_nickname_write(&mut self, nickname: Option<String>) -> Task<Message> {
        self.spark_nickname_in_flight = Some(nickname.clone());
        let cube_id = self.cube_settings.id.clone();
        let write =
            SparkNicknameWrite::new(self.spark_nickname_network_dir(), cube_id.clone(), nickname);
        Task::perform(write.run(), move |result| Message::SparkNicknameSaved {
            cube_id: cube_id.clone(),
            result,
        })
    }

    /// The Spark nickname write this App still owes when the tab replaces it
    /// (Lock Cube, Set up Vault): a name typed on Balances & conversion and
    /// not yet requested, else one queued behind a running write. A running
    /// write finishes on its own, and this one waits for it to end before
    /// writing, however the two tasks are scheduled (see
    /// [`SparkNicknameWrite::new`]). The tab starts this without waiting for
    /// it, so the field's "saved when you leave" holds without delaying the
    /// lock.
    pub fn spark_nickname_flush(&self) -> Option<SparkNicknameWrite> {
        let on_balances = matches!(
            self.panels.current,
            Menu::Spark(menu::SparkSubMenu::Settings(
                None | Some(menu::SparkSettingsOption::General)
            ))
        );
        let typed = on_balances
            .then(|| {
                self.panels
                    .spark_settings
                    .pending_nickname(self.latest_spark_nickname().as_deref())
            })
            .flatten()
            .map(|input| state::spark::settings::saved_nickname(&input));
        let nickname = typed.or_else(|| self.spark_nickname_queued.clone())?;
        Some(SparkNicknameWrite::new(
            self.spark_nickname_network_dir(),
            self.cube_settings.id.clone(),
            nickname,
        ))
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
            Menu::Cube(menu::CubeSubMenu::Settings(menu::CubeSettingsOption::AiAccess)) => {
                Some(self.local_mcp.view_for_cube(self.mcp_supported(), false))
            }
            Menu::Vault(menu::VaultSubMenu::Settings(Some(menu::SettingsOption::Connections))) => {
                Some(view::vault::settings::connections(&self.cache))
            }
            // Spark's Lightning address tab. Balances & conversion and
            // Connections are the Spark Settings panel's own view.
            Menu::Spark(menu::SparkSubMenu::Settings(Some(
                menu::SparkSettingsOption::LightningAddress,
            ))) => {
                // No heading repeats the tab name; the form heads itself.
                if authenticated {
                    Some(
                        view::spark::settings::lightning_address::settings_view(
                            &self.panels.connect.cube,
                        )
                        .map(view::Message::ConnectCube),
                    )
                } else {
                    Some(view::settings::page::group(
                        "Your Lightning address",
                        None,
                        sign_in_prompt::sign_in_prompt("claim a Lightning Address"),
                    ))
                }
            }
            // The avatar flow opens inline under Cube identity on Display &
            // appearance, rather than replacing the page.
            Menu::Cube(menu::CubeSubMenu::Settings(menu::CubeSettingsOption::Avatar)) => {
                let flow = if !authenticated {
                    sign_in_prompt::sign_in_prompt("set up an Avatar")
                } else if !has_ln_address {
                    sign_in_prompt::claim_ln_address_prompt()
                } else {
                    view::connect::avatar_ux(&self.panels.connect.cube)
                        .map(view::Message::ConnectCube)
                };
                Some(match self.panels.global_settings.general() {
                    Some(general) => general.display_content(&self.cache, Some(flow)),
                    None => flow,
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

    pub(crate) fn mcp_proposals_supported(&self) -> bool {
        cfg!(feature = "mcp-proposals")
            && self
                .cube_settings
                .network
                .dir_name()
                .parse::<coincube_mcp::Chain>()
                .is_ok()
            && self.wallet.is_none()
            && self.daemon.is_none()
            && self.cube_settings.vault_wallet_id.is_none()
            && self.cube_settings.bitcoin_import.is_none()
            && !self.daemon_switch_in_progress
            && session::generation_for(&self.cube_settings.id).is_some()
    }

    fn refresh_mcp_pairings(&mut self) {
        let supported = self.mcp_supported();
        let proposal_supported = self.mcp_proposals_supported();
        self.local_mcp.ensure_pairings(local_mcp::PairingContext {
            root: self.datadir.path(),
            chain: self.cube_settings.network,
            cube_id: &self.cube_settings.id,
            wallet: self.wallet.clone(),
            daemon: self.daemon.clone(),
            supported,
            proposal_supported,
            last_poll_at_startup: self.cache.last_poll_at_startup,
        });
    }

    #[cfg(feature = "mcp-proposals")]
    pub(crate) fn take_mcp_proposal(
        &mut self,
        paired_client: Option<&str>,
    ) -> Option<(local_mcp::Settings, crate::installer::proposal::Session)> {
        if !self.mcp_proposals_supported() {
            return None;
        }
        let proposal = match paired_client {
            Some(id) => self
                .local_mcp
                .paired_proposal_session(&self.cube_settings.id, id)?,
            None => self.local_mcp.proposal_session(&self.cube_settings.id)?,
        };
        Some((*std::mem::take(&mut self.local_mcp), proposal))
    }

    fn mcp_supported(&self) -> bool {
        self.wallet.as_ref().is_some_and(|wallet| {
            wallet.chain == self.cube_settings.network
                && wallet.main_descriptor.as_vault().is_some()
        }) && self
            .daemon
            .as_ref()
            .is_some_and(|daemon| daemon.backend().is_embedded())
            && !self.daemon_switch_in_progress
    }

    fn received_celebration_view(&self) -> Element<'_, view::Message> {
        // The underlying amount remains available for a later explicit Show,
        // but an arrival must never reveal it while privacy is hidden or
        // unresolved (including the first launch after migration).
        let displayed_amount = if self.cache.balance_amounts_masked() {
            "••••••"
        } else {
            &self.received_celebration_amount
        };
        use coincube_ui::component::payment_result::{payment_result, Outcome};
        let mut page = payment_result(
            Outcome::Received,
            displayed_amount,
            &self.received_celebration_context,
            &self.received_celebration_quote,
            &self.received_celebration_image,
            view::Message::DismissReceivedCelebration,
        )
        .voice(coincube_ui::worldview_copy::VoiceLine::PaymentReceived.get())
        .on_activity(view::Message::ReceivedCelebrationActivity);
        if !self.received_celebration_wallet.is_empty() {
            page = page.detail(coincube_ui::locale::copy(self.received_celebration_wallet));
        }
        page.into()
    }

    /// Starts the expiry clock of each toast that has just come on screen and
    /// stops the clock of each toast pushed out of view by
    /// `view::TOAST_MAX_VISIBLE`, so every toast gets its full time on screen
    /// in a burst of up to `TOAST_MAX_WAITING` more. A toast that comes back
    /// into view starts a fresh clock. Call after every change to `errors`; timers armed
    /// earlier are ignored by `Message::ExpireToast` once their deadline is
    /// cleared or replaced.
    fn arm_toast_timers(&mut self) -> Task<Message> {
        // A burst bigger than the backlog drops its oldest toasts, so stale
        // errors don't replay long after their cause and `errors` stays small.
        let excess = self
            .errors
            .len()
            .saturating_sub(view::TOAST_MAX_VISIBLE + TOAST_MAX_WAITING);
        self.errors.drain(..excess);
        let hidden = self.errors.len().saturating_sub(view::TOAST_MAX_VISIBLE);
        let now = std::time::Instant::now();
        let mut timers = Vec::new();
        for (index, (id, deadline, ..)) in self.errors.iter_mut().enumerate() {
            if index < hidden {
                *deadline = None;
            } else if deadline.is_none() {
                *deadline = Some(now + TOAST_LIFETIME);
                let id = *id;
                timers.push(Task::perform(
                    async move { tokio::time::sleep(TOAST_LIFETIME).await },
                    move |_| Message::ExpireToast(id),
                ));
            }
        }
        Task::batch(timers)
    }

    fn displayed_toasts(&self) -> impl Iterator<Item = (usize, log::Level, String)> + '_ {
        let masked = self.cache.balance_amounts_masked();
        let unit = self.cache.bitcoin_unit;
        self.errors
            .iter()
            .map(move |(id, _, level, content)| (*id, *level, content.display(unit, masked)))
    }

    pub fn view(&self) -> Element<'_, Message> {
        let journey = self
            .panels
            .imported_bitcoin
            .as_ref()
            .filter(|panel| panel.journey_open());
        let view = if let Some(panel) = journey {
            // Import or restore holds the window from the imported wallet's
            // Source step until the user leaves Done.
            panel.journey_view(&self.cache, self.cube_settings.passkey_metadata.is_some())
        } else if self.show_received_celebration {
            // Global celebration overlay takes precedence over the normal panel view.
            view::replacement_page(
                &self.panels.current,
                &self.cache,
                self.received_celebration_view(),
            )
        } else if matches!(
            self.panels.current,
            Menu::Cube(menu::CubeSubMenu::Settings(
                menu::CubeSettingsOption::AiAccess
            ))
        ) {
            view::dashboard(
                &self.panels.current,
                &self.cache,
                self.local_mcp
                    .view_for_cube(self.mcp_supported(), self.mcp_proposals_supported()),
            )
        } else if let Some(reason) = features::route_availability(
            &self.panels.current,
            self.cache.chain(),
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
            // shows the placeholder instead of the live Connect UI. The
            // route's Settings head and tabs would only lead to more of it.
            view::replacement_page(
                &self.panels.current,
                &self.cache,
                view::feature_unavailable_panel(reason),
            )
        } else if matches!(
            self.panels.current,
            Menu::Spark(menu::SparkSubMenu::Overview)
        ) {
            // The Spark page shows the Connect-owned address beside its
            // balance; claiming and changing it happen in Settings. No second
            // address or auth state lives in Overview.
            let lightning_address = state::spark::overview::lightning_address_for(
                self.panels.connect.account.is_authenticated(),
                &self.panels.connect.cube,
            );
            let (activity, cache) = (&self.panels.activity, &self.cache);
            let transfer = self
                .panels
                .global_home
                .wallet_transfer_entry(cache, view::global_home::WalletKind::Spark);
            let conversion = self.panels.spark_receive.conversion_summary();
            self.panels.spark_overview.view_with_lightning_share(
                &self.panels.current,
                &self.cache,
                lightning_address,
                transfer,
                move || {
                    let recent = view::activity::recent_activity(
                        activity,
                        cache,
                        view::activity::RecentScope::Wallet(view::activity::WalletFilter::Spark),
                        None,
                    );
                    // A cross-network receive still running (or arrived and not
                    // yet closed) isn't a payment yet, so Activity can't list it:
                    // say so above it, with the way back to its page.
                    match conversion {
                        Some((option, arrived)) => iced::widget::Column::new()
                            .spacing(16)
                            .push(view::spark::receive::conversion_notice(
                                option,
                                arrived,
                                view::Message::SparkOverview(
                                    view::SparkOverviewMessage::ViewConversion,
                                ),
                            ))
                            .push(recent)
                            .into(),
                        None => recent,
                    }
                },
                move || {
                    view::activity::wallet_activity_detail(
                        activity,
                        cache,
                        view::activity::WalletFilter::Spark,
                    )
                },
            )
        } else if let (Menu::Vault(menu::VaultSubMenu::Overview), Some(panel), false) = (
            &self.panels.current,
            self.panels.vault_overview.as_ref(),
            self.panels
                .imported_bitcoin
                .as_ref()
                .is_some_and(|panel| panel.handles_route(&self.panels.current)),
        ) {
            // The Vault page's "Recent activity" and its detail pane are
            // Activity's rows and pane, filtered to the Vault.
            let (activity, cache) = (&self.panels.activity, &self.cache);
            let transfer = self
                .panels
                .global_home
                .wallet_transfer_entry(cache, view::global_home::WalletKind::Vault);
            panel.view_with_recent_activity(
                &self.panels.current,
                cache,
                transfer,
                move |unknown_history| {
                    view::activity::recent_activity(
                        activity,
                        cache,
                        view::activity::RecentScope::Wallet(view::activity::WalletFilter::Vault),
                        unknown_history,
                    )
                },
                move || {
                    view::activity::wallet_activity_detail(
                        activity,
                        cache,
                        view::activity::WalletFilter::Vault,
                    )
                },
            )
        } else if matches!(self.panels.current, Menu::Cube(menu::CubeSubMenu::Overview)) {
            // The Overview's "Recent activity" is Activity's rows, Spark and
            // the Vault together; a row opens in Activity. Until the Vault's
            // history is known, an empty table says so, as the Vault page does.
            let (activity, cache) = (&self.panels.activity, &self.cache);
            let unknown_history = self
                .panels
                .vault_overview
                .as_ref()
                .and_then(|vault| vault.unknown_history_note());
            self.panels.global_home.view_with_recent_activity(
                &self.panels.current,
                cache,
                Some(view::activity::recent_activity(
                    activity,
                    cache,
                    view::activity::RecentScope::Cube,
                    unknown_history,
                )),
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

        let view: Element<'_, view::Message> = if self.cache.network != bitcoin::Network::Bitcoin {
            iced::widget::column![network_banner(self.cache.network), view].into()
        } else {
            view
        };
        let view = if matches!(
            self.panels.current,
            Menu::Cube(menu::CubeSubMenu::Settings(
                menu::CubeSettingsOption::AiAccess
            ))
        ) {
            // `optional` keeps the page at the same tree position whether or
            // not the revoke dialog is open, so opening and closing it doesn't
            // reset the page's scroll offset or other widget state.
            coincube_ui::widget::modal::Modal::optional(view, self.local_mcp.revoke_dialog())
                .on_blur(Some(view::Message::LocalMcp(
                    local_mcp::Message::CancelRevokePair,
                )))
                .into()
        } else {
            view
        };
        let content = view.map(Message::View);

        // Toasts sit in a layer above the content. The Stack is mounted even
        // when there are no toasts (the overlay is then empty and reports
        // `mouse::Interaction::None`, so input reaches the content) because
        // swapping the root between `content` and a Stack would make iced's
        // tree diff discard the content's widget state — scroll offsets and
        // text-input focus — when the first toast appears and the last goes.
        let content: Element<'_, Message> = iced::widget::Stack::new()
            .push(content)
            .push(view::toast_overlay(self.displayed_toasts()).map(Message::View))
            .into();

        // One-time recovery-alerts consent prompt overlays everything (PR 3).
        // `opaque` makes the full-screen overlay capture mouse presses so a
        // backdrop click can't fall through to (and actuate) the content layer
        // beneath it in the Stack — a proper modal focus trap. As with the
        // toast layer, the Stack stays mounted while the prompt is hidden (an
        // empty `Space` layer that never claims the pointer) so showing or
        // answering the prompt doesn't discard the content's widget state.
        let prompt: Element<'_, Message> = if self.show_recovery_alerts_prompt {
            iced::widget::opaque(recovery_alerts_consent_overlay().map(Message::View))
        } else {
            iced::widget::Space::new().into()
        };
        iced::widget::Stack::new().push(content).push(prompt).into()
    }

    pub fn datadir_path(&self) -> &CoincubeDirectory {
        &self.cache.datadir_path
    }
}

/// What to do about Spark Stable Balance once the SDK's state is known for
/// the session. See `CubeSettings::spark_stable_balance` for why the app
/// keeps its own copy of the toggle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StableBalanceReconcile {
    /// The SDK already matches what the user wanted.
    InSync,
    /// The SDK disagrees with the recorded preference: re-apply it. The
    /// SDK's copy is a local cache that can be lost (fresh install,
    /// recovered Cube), so the app's record wins.
    Apply(bool),
    /// Stable Balance is on but the app never recorded a decision — a
    /// Cube from before the preference existed. Adopt the SDK's state as
    /// the record.
    Adopt,
    /// Off, no decision recorded, and the wallet holds USDB: the
    /// recovered-on-a-new-machine case. Ask the user via the banner.
    PromptHolding,
}

/// Pure decision for the session's Stable Balance reconcile, separated
/// from the `App` handlers so it's unit-testable. `preference` is the
/// Cube's recorded decision, `sdk_active` the SDK's live toggle, and
/// `holds_usdb` whether the wallet's USDB balance is non-zero.
fn reconcile_stable_balance(
    preference: Option<bool>,
    sdk_active: bool,
    holds_usdb: bool,
) -> StableBalanceReconcile {
    match (preference, sdk_active, holds_usdb) {
        (Some(wanted), active, _) if wanted == active => StableBalanceReconcile::InSync,
        (Some(wanted), _, _) => StableBalanceReconcile::Apply(wanted),
        (None, true, _) => StableBalanceReconcile::Adopt,
        (None, false, true) => StableBalanceReconcile::PromptHolding,
        (None, false, false) => StableBalanceReconcile::InSync,
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
        .push(
            text(coincube_ui::locale::copy("Alert your keyholders?"))
                .size(20)
                .bold(),
        )
        .push(
            text(coincube_ui::locale::copy(
                "Turn on recovery alerts so COINCUBE can email your keyholders when this Vault's \
                 recovery window opens. Without this, your keyholders are never told when your \
                 recovery window opens.",
            ))
            .size(14),
        )
        .push(
            text(coincube_ui::locale::copy(
                "COINCUBE learns only the block height at which the window opens and that this \
                 desktop checked in — never your addresses or balances.",
            ))
            .size(13)
            .style(theme::text::secondary),
        )
        .push(Space::new().height(Length::Fixed(6.0)))
        .push(
            Row::new()
                .spacing(10)
                .push(
                    button::primary(None, coincube_ui::locale::copy("Turn on recovery alerts"))
                        .on_press(view::Message::RecoveryAlertsConsent(true)),
                )
                .push(
                    button::secondary(None, coincube_ui::locale::copy("Not now"))
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
        wallet.recovery_timelock(),
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
fn restart_daemon_blocking_guarded(
    old_daemon: Option<Arc<dyn Daemon + Sync + Send>>,
    cfg: DaemonConfig,
    daemon_config_path: std::path::PathBuf,
    connect_client: Option<crate::services::coincube::CoincubeClient>,
    local_root: Option<CoincubeDirectory>,
) -> DaemonRestart {
    let recovery_cfg = old_daemon.as_ref().and_then(|d| d.config().cloned());
    if recovery_cfg
        .as_ref()
        .is_some_and(|previous| previous.bitcoin_config.chain != cfg.bitcoin_config.chain)
        || (cfg.bitcoin_config.chain.is_blake2b() && connect_client.is_none()
            && recovery_cfg.as_ref().is_none_or(|previous|
                !matches!((&previous.bitcoin_backend, &cfg.bitcoin_backend),
                    (Some(coincubed::config::BitcoinBackend::Bitcoind(a)), Some(coincubed::config::BitcoinBackend::Bitcoind(b))) if a == b)
                || cfg.pending_bitcoind.is_some()))
    {
        return DaemonRestart::Failed {
            error: Error::Daemon(DaemonError::ConnectAnchor(
                coincubed::connect::AdmissionError::InvalidBackend.into(),
            )),
            recovered: old_daemon,
        };
    }
    if cfg.bitcoin_config.chain.is_blake2b()
        && matches!(
            cfg.bitcoin_backend,
            Some(coincubed::config::BitcoinBackend::Bitcoind(_))
        )
    {
        let admitted = local_root
            .as_ref()
            .filter(|root| crate::chain::is_managed_local_fork(&cfg, root));
        let check = match admitted {
            Some(root) => Handle::current().block_on(crate::chain::require_local_feature(
                cfg.bitcoin_config.chain,
                connect_client.clone(),
                root,
            )),
            None => Err("Reopen this Cube through its managed local node".into()),
        };
        if let Err(error) = check {
            return DaemonRestart::Failed {
                error: Error::Daemon(DaemonError::Unexpected(error)),
                recovered: old_daemon,
            };
        }
    }
    let start = |config: DaemonConfig| {
        if !config.bitcoin_config.chain.is_blake2b() {
            EmbeddedDaemon::start(config)
        } else if matches!(
            config.bitcoin_backend,
            Some(coincubed::config::BitcoinBackend::Bitcoind(_))
        ) {
            let root = local_root
                .as_ref()
                .ok_or_else(|| DaemonError::Unexpected("Managed local node required".into()))?;
            if !crate::chain::is_managed_local_fork(&config, root) {
                return Err(DaemonError::Unexpected(
                    "Managed local node required".into(),
                ));
            }
            Handle::current()
                .block_on(crate::chain::require_local_feature(
                    config.bitcoin_config.chain,
                    connect_client.clone(),
                    root,
                ))
                .map_err(DaemonError::Unexpected)?;
            EmbeddedDaemon::start_local_fork(config)
        } else {
            Handle::current().block_on(EmbeddedDaemon::start_authenticated(
                config,
                connect_client.clone().expect("fork session checked"),
            ))
        }
    };

    if let Some(daemon) = &old_daemon {
        if let Err(e) = Handle::current().block_on(async { daemon.stop().await }) {
            // Couldn't stop the old daemon — keep it rather than leave none.
            return DaemonRestart::Failed {
                error: e.into(),
                recovered: old_daemon.clone(),
            };
        }
    }

    let daemon: Arc<dyn Daemon + Sync + Send> = match start(cfg) {
        Ok(d) => Arc::new(d),
        Err(start_err) => {
            // New daemon failed to start. Bring the old one back so the app is
            // left usable rather than dead.
            let recovered = recovery_cfg.and_then(|old_cfg| match start(old_cfg) {
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
        let content = toml::to_string(&daemon.config().map(|config| config.for_persistence()))
            .map_err(|e| Error::Config(e.to_string()))?;
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

/// The wallet whose "Recent activity" a route shows, if any.
fn wallet_page(menu: &Menu) -> Option<view::activity::WalletFilter> {
    match menu {
        Menu::Vault(menu::VaultSubMenu::Overview) => Some(view::activity::WalletFilter::Vault),
        Menu::Spark(menu::SparkSubMenu::Overview) => Some(view::activity::WalletFilter::Spark),
        _ => None,
    }
}

/// One write of a Spark nickname (`None`: plain "Spark") to a Cube's settings.
pub struct SparkNicknameWrite {
    network_dir: crate::dir::NetworkDirectory,
    cube_id: String,
    nickname: Option<String>,
    /// The write this one must follow: it ends when that write's `running`
    /// closes.
    after: Option<tokio::sync::watch::Receiver<()>>,
    /// Closed when this write ends, or when it is dropped unrun.
    running: tokio::sync::watch::Sender<()>,
}

/// Each Cube's latest nickname write created in this process, keyed by its
/// settings file and Cube id. Process-wide because an App can be replaced
/// (Lock Cube, then reopen) while its write still runs.
type SparkNicknameChain =
    std::collections::HashMap<(std::path::PathBuf, String), tokio::sync::watch::Receiver<()>>;

fn spark_nickname_chain() -> std::sync::MutexGuard<'static, SparkNicknameChain> {
    static CHAIN: std::sync::OnceLock<std::sync::Mutex<SparkNicknameChain>> =
        std::sync::OnceLock::new();
    CHAIN
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

impl SparkNicknameWrite {
    /// A write that lands after every nickname write created before it for
    /// this Cube, from this App or any other: writes are created in the
    /// order they are requested, on the update thread.
    pub fn new(
        network_dir: crate::dir::NetworkDirectory,
        cube_id: String,
        nickname: Option<String>,
    ) -> Self {
        let (running, ended) = tokio::sync::watch::channel(());
        let mut chain = spark_nickname_chain();
        // Writes that have ended need no entry.
        chain.retain(|_, ended| ended.has_changed().is_ok());
        let after = chain.insert((network_dir.path().to_path_buf(), cube_id.clone()), ended);
        Self {
            network_dir,
            cube_id,
            nickname,
            after,
            running,
        }
    }

    /// Whether a nickname write for this Cube has been created and not ended.
    pub fn pending(network_dir: &crate::dir::NetworkDirectory, cube_id: &str) -> bool {
        spark_nickname_chain()
            .get(&(network_dir.path().to_path_buf(), cube_id.to_string()))
            .is_some_and(|ended| ended.has_changed().is_ok())
    }

    /// Writes after the write it follows has ended: the settings file's own
    /// lock doesn't order writers, and neither does the order their tasks
    /// are polled in.
    pub async fn run(self) -> Result<Option<String>, String> {
        let Self {
            network_dir,
            cube_id,
            nickname,
            after,
            running,
        } = self;
        if let Some(mut after) = after {
            // `changed` fails once the sender is gone: that write has ended.
            while after.changed().await.is_ok() {}
        }
        // Held until this write has landed.
        let _running = running;
        let saved = nickname.clone();
        settings::update_settings_file(&network_dir, move |mut s| {
            if let Some(c) = s.cubes.iter_mut().find(|c| c.id == cube_id) {
                c.spark_alias = saved;
            }
            Some(s)
        })
        .await
        .map(|_| nickname)
        .map_err(|e| e.to_string())
    }
}

/// The "Recent activity" a route shows, if any: the Overview's, or a wallet
/// page's.
fn recent_scope(menu: &Menu) -> Option<view::activity::RecentScope> {
    match menu {
        Menu::Cube(menu::CubeSubMenu::Overview) => Some(view::activity::RecentScope::Cube),
        _ => wallet_page(menu).map(view::activity::RecentScope::Wallet),
    }
}

/// Whether a Spark event can change Spark's newest payment rows: a payment's
/// state, a claimed deposit, or the first sync of the session's payments.
fn overview_activity_spark_event(event: &coincube_spark_protocol::Event) -> bool {
    use coincube_spark_protocol::Event as SparkEvent;
    matches!(
        event,
        SparkEvent::Synced
            | SparkEvent::DepositsChanged
            | SparkEvent::PaymentSucceeded { .. }
            | SparkEvent::PaymentPending { .. }
            | SparkEvent::PaymentFailed { .. }
            | SparkEvent::PaymentUpdated { .. }
    )
}

/// Whether a daemon poll can have changed the Vault's newest history rows:
/// a coin arrived, confirmed, was spent or its spend confirmed, a block
/// arrived, or a full scan or rescan finished. The cached coins hold only
/// unspent ones, so a spend with no change output leaves them as they were
/// when it confirms; the new block is the only sign of that. A scan can
/// likewise find only spent history at the same tip; its completion is the
/// only sign of that. The Overview re-reads the Vault's history then, rather
/// than on every poll.
fn overview_activity_vault_changed(before: &DaemonCache, after: &DaemonCache) -> bool {
    let state = |coins: &[crate::daemon::model::Coin]| {
        coins
            .iter()
            .map(|coin| {
                (
                    coin.outpoint,
                    coin.block_height,
                    coin.spend_info.map(|spend| (spend.txid, spend.height)),
                )
            })
            .collect::<HashSet<_>>()
    };
    before.blockheight != after.blockheight
        || before.history_sync.full_scan_completed_at != after.history_sync.full_scan_completed_at
        || (before.rescan_progress.is_some() && after.rescan_progress.is_none())
        || state(&before.coins) != state(&after.coins)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The Vault descriptor of the Split rebind tests.
    const SPLIT_DESC: &str = "wsh(or_d(pk([f5acc2fd]tpubD6NzVbkrYhZ4YgUx2ZLNt2rLYAMTdYysCRzKoLu2BeSHKvzqPaBDvf17GeBPnExUVPkuBpx4kniP964e2MxyzzazcXLptxLXModSVCVEV1T/<0;1>/*),and_v(v:pkh([8a64f2a9]tpubD6NzVbkrYhZ4WmzFjvQrp7sDa4ECUxTi9oby8K4FZkd3XCBtEdKwUiQyYJaxiJo5y42gyDWEczrFpozEjeLxMPxjf2WtkfcbpUdfvNnozWF/<0;1>/*),older(10))))#d72le4dr";

    /// Daemon backends for the Split rebind tests: Connect's BTCB2 Esplora
    /// at `https://connect.example/` (step 2's admitted route), or Electrum
    /// (not admitted).
    fn split_backend_daemon(
        root: &std::path::Path,
        admitted: bool,
    ) -> Arc<dyn Daemon + Sync + Send> {
        use coincubed::config::{BitcoinBackend, BitcoinConfig, ElectrumConfig, EsploraConfig};
        use std::str::FromStr;
        let backend = if admitted {
            BitcoinBackend::Esplora(EsploraConfig {
                addr: "https://connect.example/api/v1/esplora/bitcoin-blake2b/mainnet".into(),
                token: None,
                fallback_addr: None,
                fallback_token: None,
                secondary_fallback_addr: None,
                secondary_fallback_token: None,
            })
        } else {
            BitcoinBackend::Electrum(ElectrumConfig {
                addr: "ssl://electrum.example:50002".into(),
                validate_domain: true,
            })
        };
        Arc::new(EmbeddedDaemon::unstarted_for_test(
            coincubed::config::Config::new(
                BitcoinConfig::new(
                    crate::chain::ChainId::BitcoinBlake2b,
                    Duration::from_secs(30),
                ),
                Some(backend),
                log::LevelFilter::Off,
                coincube_core::descriptors::CoincubeDescriptor::from_str(SPLIT_DESC).unwrap(),
                coincubed::datadir::DataDirectory::new(root.to_path_buf()),
            ),
            None,
        ))
    }

    /// An App opened on a wallet-less Cube whose imported Vault policy is
    /// attached and syncing: its runtime daemon (unstarted) and attachment.
    fn imported_vault_app(
        root: &std::path::Path,
    ) -> (
        App,
        Arc<dyn Daemon + Send + Sync>,
        settings::ImportedWalletAttachment,
    ) {
        use coincube_core::{chain::ChainId, wallet_descriptor::ImportedWalletDescriptor};
        use coincubed::config::{BitcoinBackend, BitcoinConfig, ElectrumConfig};
        let datadir = CoincubeDirectory::new(root.to_path_buf());
        let descriptor = ImportedWalletDescriptor::parse(
            "wpkh([6cb116eb/84h/0h/0h]xpub6C6c6ShFaUMjohRwYGnrDr5kbrnUjBeGh5U8BPAjtZnZ2D7WpRidu2SUNpBVfmMaw8TpDA7quYNW4iNwGYaq9iAwp5fzHLCXJwTqe76WpmF/<0;1>/*)",
            None, ChainId::Bitcoin,
        ).unwrap();
        let attachment =
            settings::ImportedWalletAttachment::new(descriptor.clone(), ChainId::Bitcoin, 0, 20)
                .unwrap();
        let mut cube =
            settings::CubeSettings::new("Imported family Vault".into(), ChainId::Bitcoin);
        cube.bitcoin_import = Some(Box::new(settings::BitcoinImport {
            operation_id: uuid::Uuid::new_v4(),
            stage: settings::BitcoinImportStage::Syncing {
                wallet: attachment.clone(),
            },
            last_error: None,
        }));
        let wallet = Arc::new(Wallet::from_imported_cube(&cube).unwrap());
        let mut daemon_config = coincubed::config::Config::new(
            BitcoinConfig::new(ChainId::Bitcoin, Duration::from_secs(30)),
            Some(BitcoinBackend::Electrum(ElectrumConfig {
                addr: "tcp://127.0.0.1:1".into(),
                validate_domain: false,
            })),
            log::LevelFilter::Off,
            descriptor,
            datadir
                .network_directory(ChainId::Bitcoin)
                .coincubed_data_directory(&attachment.wallet_id),
        );
        daemon_config.import_birthday = Some(0);
        daemon_config.import_initial_lookahead = Some(20);
        let daemon: Arc<dyn Daemon + Send + Sync> =
            Arc::new(EmbeddedDaemon::unstarted_for_test(daemon_config, None));
        let (app, startup) = App::new_inner(
            Cache {
                network: bitcoin::Network::Bitcoin,
                fiat_chain: ChainId::Bitcoin,
                cube_id: cube.id.clone(),
                ..Cache::default()
            },
            wallet,
            None,
            None,
            Config::new(false),
            daemon.clone(),
            datadir,
            None,
            cube,
            None,
        );
        drop(startup);
        (app, daemon, attachment)
    }

    #[test]
    fn imported_policy_loads_as_the_existing_vault_wallet() {
        let root =
            std::env::temp_dir().join(format!("imported-vault-app-{}", uuid::Uuid::new_v4()));
        let (mut app, daemon, attachment) = imported_vault_app(&root);
        assert!(app.wallet.as_ref().unwrap().is_imported());
        assert!(Arc::ptr_eq(app.daemon.as_ref().unwrap(), &daemon));
        assert!(app.panels.vault_overview.is_some());
        assert!(app.panels.receive.is_some());
        assert!(app.panels.coins.is_some());
        assert!(app.panels.transactions.is_some());
        assert!(app.panels.recovery.is_none());
        assert!(!app.claim_source_cube().has_vault);
        assert!(!app.cache.has_vault); // Legacy inheritance/Connect capability.
        assert!(app.cache.has_imported_bitcoin);
        assert!(app.wallet.as_ref().unwrap().signer.is_none());
        assert_eq!(app.wallet.as_ref().unwrap().id(), attachment.wallet_id);
        assert!(Arc::ptr_eq(
            &app.panels
                .imported_bitcoin
                .as_ref()
                .unwrap()
                .runtime_daemon()
                .unwrap(),
            &daemon
        ));
        drop(app.set_current_panel(Menu::Vault(menu::VaultSubMenu::Overview)));
        assert_eq!(
            app.panels.current,
            Menu::Vault(menu::VaultSubMenu::Overview)
        );
        for route in [
            menu::VaultSubMenu::Recovery,
            menu::VaultSubMenu::Settings(Some(menu::SettingsOption::LocalSigning)),
        ] {
            drop(app.set_current_panel(Menu::Vault(route)));
            assert_eq!(
                app.panels.current,
                Menu::Vault(menu::VaultSubMenu::Overview)
            );
        }
        // A sibling-tab sign-out holds the account session even while this
        // App still has its old admitted client in memory.
        let mut client =
            crate::services::coincube::CoincubeClient::for_test("https://connect.example/");
        client.set_token("synthetic-imported-session");
        app.panels.connect.install_admitted_client(client);
        app.panels.connect.account.user = Some(crate::services::coincube::User {
            id: 7,
            email: "synthetic@example.invalid".into(),
            email_verified: Some(true),
        });
        app.panels.connect.account.step = state::connect::account::ConnectFlowStep::Dashboard;
        app.panels.connect.account.bitcoin_blake2b_opt_in = true;
        app.panels.connect.account.features = Some(
            serde_json::from_value(serde_json::json!({
                "plans": [], "bitcoinBlake2bEnabled": true,
            }))
            .unwrap(),
        );
        assert!(app.claim_connect_session().is_some());
        app.invalidate_claim_session();
        assert!(app.authenticated_coincube_client().is_some());
        assert!(app.panels.connect.account.bitcoin_blake2b_enabled());
        assert!(app.claim_connect_session().is_none());
    }

    /// An open Import or restore takes the whole window: none of the Cube's
    /// menu shows until the user leaves it.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_open_import_journey_replaces_the_cube_shell() {
        use crate::home::restore_frame::rendered_labels;
        let root =
            std::env::temp_dir().join(format!("imported-vault-journey-{}", uuid::Uuid::new_v4()));
        let (mut app, _daemon, _) = imported_vault_app(&root);
        let shell = rendered_labels(app.view()).await;
        assert!(
            !shell.iter().any(|text| text == "Import or restore"),
            "{:?}",
            shell
        );
        assert!(shell.iter().any(|text| text == "Activity"), "{:?}", shell);

        app.panels
            .imported_bitcoin
            .as_mut()
            .unwrap()
            .open_journey_for_tests();
        let journey = rendered_labels(app.view()).await;
        assert!(
            journey.iter().any(|text| text == "Import or restore"),
            "{:?}",
            journey
        );
        assert!(
            !journey.iter().any(|text| text == "Activity"),
            "{:?}",
            journey
        );

        drop(app.update(Message::View(view::Message::ImportedBitcoin(
            state::imported_bitcoin::ImportMessage::LeaveJourney,
        ))));
        let closed = rendered_labels(app.view()).await;
        assert!(
            !closed.iter().any(|text| text == "Import or restore"),
            "{:?}",
            closed
        );
    }

    /// The imported runtime never sends `UpdateDaemonCache`, so the Overview's
    /// Vault rows follow its adoption: once the scan completes, then on each
    /// refresh that moves a coin or a block, never while the scan runs, for
    /// an unchanged snapshot, or on another page.
    #[test]
    fn the_overview_follows_the_imported_vaults_history() {
        let _guard = crate::app::session::test_guard();
        let root =
            std::env::temp_dir().join(format!("imported-vault-overview-{}", uuid::Uuid::new_v4()));
        let (mut app, _daemon, _) = imported_vault_app(&root);
        app.panels.current = Menu::Cube(menu::CubeSubMenu::Overview);
        let settle = |app: &mut App| {
            let generation = app.panels.activity.generation_for_tests();
            drop(app.update(Message::ActivityVaultLoaded(
                generation,
                Ok((Vec::new(), false)),
            )));
            assert!(!app.panels.activity.vault_loading);
        };
        let scanned = |app: &mut App, block_height: i32| {
            app.panels
                .imported_bitcoin
                .as_mut()
                .unwrap()
                .complete_scan_for_tests(block_height, Vec::new());
            drop(app.adopt_imported_runtime());
        };
        settle(&mut app);

        drop(app.adopt_imported_runtime());
        assert!(
            !app.panels.activity.vault_loading,
            "the scan is still running"
        );

        scanned(&mut app, 100);
        assert!(app.panels.activity.vault_loading, "the history is in");
        settle(&mut app);

        scanned(&mut app, 100);
        assert!(!app.panels.activity.vault_loading, "nothing changed");

        scanned(&mut app, 101);
        assert!(app.panels.activity.vault_loading, "a block arrived");
        settle(&mut app);

        app.panels.current = Menu::Cube(menu::CubeSubMenu::Settings(
            menu::CubeSettingsOption::General,
        ));
        scanned(&mut app, 102);
        assert!(!app.panels.activity.vault_loading, "another page");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn fork_theme_toggle_reaches_global_handler_and_updates_all_tabs() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let _enter = rt.enter();
                use coincube_ui::theme::palette::ThemeMode;
                use iced::futures::StreamExt;
                let root =
                    std::env::temp_dir().join(format!("fork-theme-{}", uuid::Uuid::new_v4()));
                std::fs::create_dir_all(&root).unwrap();
                let mut gui = crate::gui::GUI::reviewer_claim_gui(
                    fork_app(&root, true),
                    fork_app(&root, true),
                );
                // The reviewer fixture starts unresolved. GUI::new loads an
                // accepted receipt before the first preference change.
                gui.reviewer_load_preferences();
                rt.block_on(async {
                    for expected in [ThemeMode::Light, ThemeMode::Dark] {
                        let mut pending = vec![gui.reviewer_theme_toggle(1)];
                        while let Some(task) = pending.pop() {
                            if let Some(mut stream) = iced_runtime::task::into_stream(task) {
                                while let Some(action) = stream.next().await {
                                    if let iced_runtime::Action::Output(message) = action {
                                        pending.push(gui.update(message));
                                    }
                                }
                            }
                        }
                        for index in 0..2 {
                            assert_eq!(gui.reviewer_app_mut(index).cache.theme_mode, expected);
                        }
                    }
                    drop(gui);
                    std::fs::remove_dir_all(root).unwrap();
                });
            })
            .unwrap()
            .join()
            .unwrap();
    }

    fn local_app(root: &std::path::Path) -> App {
        let directory = CoincubeDirectory::new(root.to_path_buf());
        let (cfg, wallet_settings) =
            crate::chain::managed_local_fixture(&directory, crate::chain::ChainId::BitcoinBlake2b);
        let wallet = Arc::new(
            Wallet::new(
                cfg.main_descriptor
                    .as_vault()
                    .expect("Vault test config")
                    .clone(),
            )
            .with_chain(crate::chain::ChainId::BitcoinBlake2b)
            .load_from_settings(wallet_settings)
            .unwrap(),
        );
        let signer =
            coincube_core::signer::MasterSigner::generate(bitcoin::Network::Bitcoin).unwrap();
        let cek = Arc::new(crate::services::connect::crypto::CubeEncryptionKey::derive(
            &signer,
            bitcoin::Network::Bitcoin,
        ));
        let (app, task) = App::new_local_fork(
            Cache {
                fiat_chain: crate::chain::ChainId::BitcoinBlake2b,
                ..Cache::default()
            },
            wallet,
            Some(cek),
            Config::new(false),
            Arc::new(EmbeddedDaemon::unstarted_for_test(cfg, None)),
            directory,
            settings::CubeSettings::new("Local".into(), crate::chain::ChainId::BitcoinBlake2b),
            None,
        )
        .unwrap();
        drop(task);
        assert!(!app.requires_connect());
        assert!(app.fork_connect_client.is_none());
        assert!(app.breez_client.is_none() && app.spark_backend().is_none());
        app
    }

    /// The Connect-first node install must be visible from start to failure
    /// and restartable from Settings → Node without reopening the Vault.
    #[test]
    fn background_node_install_status_tracks_progress_failure_and_retry() {
        let result = std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let _guard = crate::app::session::test_guard();
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let _enter = rt.enter();
                let root = std::env::temp_dir()
                    .join(format!("background-node-status-{}", uuid::Uuid::new_v4()));
                let mut app = local_app(&root);
                let mut cfg = app.daemon.as_ref().unwrap().config().unwrap().clone();
                cfg.bitcoin_backend = Some(coincubed::config::BitcoinBackend::Esplora(
                    crate::installer::connect_esplora_config(
                        crate::chain::ChainId::BitcoinBlake2b,
                        "synthetic-jwt",
                    ),
                ));
                cfg.pending_bitcoind = None;
                cfg.pending_managed_node_install = Some("KnotsBlake2b".to_string());
                app.daemon = Some(Arc::new(EmbeddedDaemon::unstarted_for_test(cfg, None)));
                app.cache.background_node_install = cache::BackgroundNodeInstall::Idle;
                let cube = app.cube_settings.id.clone();
                let ready = |attempt: u64, result: Result<_, String>, flavor: &str| {
                    Message::BackgroundManagedNodeReady {
                        cube: cube.clone(),
                        attempt,
                        flavor: flavor.to_string(),
                        result,
                    }
                };
                let progress =
                    |cube: &str, attempt: u64, progress| Message::BackgroundManagedNodeProgress {
                        cube: cube.to_string(),
                        attempt,
                        progress,
                    };
                let retry = || {
                    Message::View(view::Message::Settings(
                        view::SettingsMessage::NodeSettings(
                            view::NodeSettingsMessage::RetryBackgroundNodeSetup,
                        ),
                    ))
                };

                // A retry does nothing while no attempt has failed.
                let _ = app.update(retry());
                assert_eq!(
                    app.cache.background_node_install,
                    cache::BackgroundNodeInstall::Idle
                );

                let _ = app.background_managed_node_task(false);
                assert_eq!(
                    app.cache.background_node_install,
                    cache::BackgroundNodeInstall::Downloading(None)
                );
                let first = app.background_node_attempt.unwrap();
                let _ = app.update(progress(
                    &cube,
                    first,
                    cache::BackgroundNodeInstall::Downloading(Some(42.0)),
                ));
                assert_eq!(
                    app.cache.background_node_install,
                    cache::BackgroundNodeInstall::Downloading(Some(42.0))
                );
                let _ = app.update(progress(
                    "another-cube",
                    first,
                    cache::BackgroundNodeInstall::Starting,
                ));
                assert_eq!(
                    app.cache.background_node_install,
                    cache::BackgroundNodeInstall::Downloading(Some(42.0)),
                    "another Cube's progress is not this install's"
                );
                // A second start while one runs would race it for the node.
                let _ = app.background_managed_node_task(false);
                assert_eq!(app.background_node_attempt, Some(first));
                assert_eq!(
                    app.cache.background_node_install,
                    cache::BackgroundNodeInstall::Downloading(Some(42.0))
                );

                let _ = app.update(ready(
                    first,
                    Err("download refused".to_string()),
                    "KnotsBlake2b",
                ));
                assert_eq!(
                    app.cache.background_node_install,
                    cache::BackgroundNodeInstall::Failed("download refused".to_string())
                );
                let _ = app.update(progress(
                    &cube,
                    first,
                    cache::BackgroundNodeInstall::Starting,
                ));
                assert!(
                    matches!(
                        app.cache.background_node_install,
                        cache::BackgroundNodeInstall::Failed(_)
                    ),
                    "late progress cannot hide the failure"
                );

                let _ = app.update(retry());
                assert_eq!(
                    app.cache.background_node_install,
                    cache::BackgroundNodeInstall::Downloading(None),
                    "Retry restarts the install in this session"
                );
                let second = app.background_node_attempt.unwrap();
                assert_ne!(first, second);
                // The first attempt's late messages, or those of an App this
                // one replaced, cannot move the running attempt's status.
                let _ = app.update(progress(
                    &cube,
                    first,
                    cache::BackgroundNodeInstall::Downloading(Some(99.0)),
                ));
                let _ = app.update(ready(first, Err("old timeout".to_string()), "KnotsBlake2b"));
                assert_eq!(
                    app.cache.background_node_install,
                    cache::BackgroundNodeInstall::Downloading(None),
                    "a stale attempt neither advances nor fails the current one"
                );
                let _ = app.update(retry());
                assert_eq!(
                    app.background_node_attempt,
                    Some(second),
                    "no Retry while the current attempt runs"
                );

                let _ = app.update(ready(second, Err("stale".to_string()), "Knots"));
                assert_eq!(
                    app.cache.background_node_install,
                    cache::BackgroundNodeInstall::Idle,
                    "a result for a request no longer on disk ends the install"
                );
                drop(app);
                let _ = std::fs::remove_dir_all(root);
            })
            .unwrap()
            .join();
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }

    #[test]
    fn local_gui_logout_signin_and_stale_account_results_preserve_wallet() {
        let result = std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let _guard = crate::app::session::test_guard();
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let _enter = rt.enter();
                let root =
                    std::env::temp_dir().join(format!("local-gui-auth-{}", uuid::Uuid::new_v4()));
                let mut gui =
                    crate::gui::GUI::reviewer_claim_gui(local_app(&root), local_app(&root));
                let cek = gui
                    .reviewer_app_mut(0)
                    .cache
                    .cube_encryption_key
                    .clone()
                    .unwrap();
                let generation = gui
                    .reviewer_app_mut(1)
                    .panels
                    .connect
                    .account
                    .session_generation();
                state::connect::write_connect_secret(
                    state::connect::CONNECT_KEYRING_USER,
                    b"synthetic-old",
                )
                .unwrap();
                drop(gui.reviewer_account(1, view::ConnectAccountMessage::LogOut));
                assert!(
                    state::connect::read_connect_secret(state::connect::CONNECT_KEYRING_USER)
                        .is_none(),
                    "originating logout was dispatched"
                );
                for index in 0..2 {
                    let app = gui.reviewer_app_mut(index);
                    assert!(!app.requires_connect());
                    assert!(!app.panels.connect.account.requires_authenticated_reopen());
                    assert!(app.daemon.is_some() && app.cache.cube_encryption_key.is_some());
                    assert!(app.fork_connect_client.is_none());
                }
                assert!(Arc::ptr_eq(
                    &cek,
                    gui.reviewer_app_mut(0)
                        .cache
                        .cube_encryption_key
                        .as_ref()
                        .unwrap()
                ));
                let epoch = gui.reviewer_app_mut(0).connect_auth_epoch();
                drop(gui.reviewer_account(
                    1,
                    view::ConnectAccountMessage::SetSession(
                        crate::services::coincube::LoginResponse {
                            requires_2fa: false,
                            token: "synthetic-new".into(),
                            refresh_token: "synthetic-refresh".into(),
                            user: crate::services::coincube::User {
                                id: 77,
                                email: "fixture@example.invalid".into(),
                                email_verified: Some(true),
                            },
                        },
                        epoch,
                    ),
                ));
                let saved =
                    state::connect::read_connect_secret(state::connect::CONNECT_KEYRING_USER)
                        .expect("local sign-in was dispatched");
                assert!(String::from_utf8(saved).unwrap().contains("synthetic-new"));
                let granted = serde_json::from_value(
                    serde_json::json!({"plans":[],"bitcoin_blake2b_enabled":true}),
                )
                .unwrap();
                drop(gui.reviewer_account(
                    2,
                    view::ConnectAccountMessage::FeaturesLoaded(Some(granted), generation),
                ));
                assert!(gui
                    .reviewer_app_mut(1)
                    .panels
                    .connect
                    .account
                    .features
                    .is_none());
                assert!(!gui.reviewer_app_mut(1).cache.btcb2_server_enabled);
                assert!(gui.reviewer_app_mut(0).cache.cube_encryption_key.is_some());
                std::fs::remove_dir_all(root).unwrap();
            })
            .unwrap()
            .join();
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }

    #[tokio::test]
    async fn denied_local_restart_preserves_old_daemon_and_config() {
        let root =
            std::env::temp_dir().join(format!("local-restart-denied-{}", uuid::Uuid::new_v4()));
        let app = {
            let _guard = crate::app::session::test_guard();
            local_app(&root)
        };
        let cfg = app.daemon.as_ref().unwrap().config().unwrap().clone();
        let path = cfg.data_directory().unwrap().path().join("daemon.toml");
        let bytes = std::fs::read(&path).unwrap();
        let old = app.daemon.clone();
        let directory = app.datadir.clone();
        let result = tokio::task::spawn_blocking(move || {
            restart_daemon_blocking_guarded(old, cfg, path, None, Some(directory))
        })
        .await
        .unwrap();
        let DaemonRestart::Failed { recovered, error } = result else {
            panic!("restarted with beta disabled")
        };
        assert!(error.to_string().contains("Enable Bitcoin Blake2b"));
        assert!(Arc::ptr_eq(
            recovered.as_ref().unwrap(),
            app.daemon.as_ref().unwrap()
        ));
        let path = app
            .daemon
            .as_ref()
            .unwrap()
            .config()
            .unwrap()
            .data_directory()
            .unwrap()
            .path()
            .join("daemon.toml");
        assert_eq!(std::fs::read(path).unwrap(), bytes);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn late_connect_retry_does_not_restart_a_settled_connection() {
        let _guard = crate::app::session::test_guard();
        let root =
            std::env::temp_dir().join(format!("connect-late-retry-{}", uuid::Uuid::new_v4()));
        let mut app = local_app(&root);
        app.cache.fiat_chain = crate::chain::ChainId::Bitcoin;
        for status in [
            ConnectionStatus::Inactive,
            ConnectionStatus::Connecting,
            ConnectionStatus::Connected,
            ConnectionStatus::Offline,
            ConnectionStatus::Reconnecting,
            ConnectionStatus::RenewFailed,
        ] {
            app.cache.connect_stream_status = status;
            app.connect_renewal = Renewal {
                attempts: RENEWAL_ATTEMPTS,
                in_flight: false,
            };
            let restart = app.connect_stream_restart;
            assert!(app.update(Message::ConnectRenewalRetry).units() == 0);
            assert_eq!(app.cache.connect_stream_status, status);
            assert_eq!(app.connect_renewal.attempts, RENEWAL_ATTEMPTS);
            assert!(!app.connect_renewal.in_flight);
            assert_eq!(app.connect_stream_restart, restart);
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    /// The App's view of Connect refusals and renewal (C1, C3, C6): an
    /// unexplained refusal waits for Try again, an unknown device is renewed
    /// automatically (twice at most), and a renewal's result swaps the new id
    /// into the stream config so the keyed subscription restarts.
    #[test]
    fn refusals_and_renewal_drive_the_connect_status() {
        use crate::services::connect::grpc::trouble::Refusal;
        use crate::services::connect::grpc::ConnectStreamMessage as M;
        // Every result below was requested while the stream ran on id "1".
        let reregistered = |email: &str, device_id: &str| view::ReregisteredDevice {
            email: email.into(),
            replaces: "1".into(),
            device_id: device_id.into(),
        };
        let _guard = crate::app::session::test_guard();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _enter = rt.enter();
        let root =
            std::env::temp_dir().join(format!("connect-device-refused-{}", uuid::Uuid::new_v4()));
        let mut app = local_app(&root);
        // `local_app` is a Blake2b fork Cube, whose dispatch guard drops every
        // Connect stream message. The stream only runs on Bitcoin-family Cubes.
        app.cache.fiat_chain = crate::chain::ChainId::Bitcoin;
        app.connect_stream_config = Some(
            crate::services::connect::grpc::stream::ConnectStreamConfig {
                grpc_url: "https://grpc.example.invalid".into(),
                tokens: Arc::new(tokio::sync::RwLock::new(
                    crate::services::connect::client::auth::AccessTokenResponse {
                        access_token: "fixture".into(),
                        refresh_token: "fixture-refresh".into(),
                        expires_at: 1,
                    },
                )),
                device_id: "1".into(),
                user_agent: "test".into(),
                vault_ids: Vec::new(),
                last_seen_seq: 0,
            },
        );
        app.cache.connect_device_id = Some("1".into());
        app.cache.connect_stream_status = ConnectionStatus::Connecting;

        // A server without reasons: never renewed on its own.
        drop(app.update(Message::ConnectStream(M::Refused(Refusal::Unexplained))));
        assert_eq!(
            app.cache.connect_stream_status,
            ConnectionStatus::RenewFailed
        );
        assert_eq!(app.connect_renewal, Renewal::default());

        // Try again starts a renewal.
        app.cache.connect_grpc_url = Some("https://grpc.example.invalid".into());
        app.cache.connect_tokens = app
            .connect_stream_config
            .as_ref()
            .map(|cfg| cfg.tokens.clone());
        app.cache.connect_email = Some("refused@example.invalid".into());
        app.connect_email = Some("refused@example.invalid".into());
        assert!(
            app.update(Message::View(view::Message::Settings(
                view::SettingsMessage::ReregisterConnectDevice,
            )))
            .units()
                > 0,
            "Try again must start the registration"
        );
        assert_eq!(app.cache.connect_stream_status, ConnectionStatus::Renewing);
        assert!(app.connect_renewal.in_flight);

        // Try again's one attempt failed: Couldn't reconnect again.
        drop(app.update(Message::View(view::Message::Settings(
            view::SettingsMessage::ConnectDeviceReregistered(Err("offline".into())),
        ))));
        assert_eq!(
            app.cache.connect_stream_status,
            ConnectionStatus::RenewFailed
        );
        assert_eq!(app.connect_stream_config.as_ref().unwrap().device_id, "1");

        // An unknown device is renewed automatically; a failure retries
        // once after the wait, then gives up.
        app.connect_renewal = Renewal::default();
        drop(app.update(Message::ConnectStream(M::Refused(Refusal::DeviceUnknown))));
        assert_eq!(app.cache.connect_stream_status, ConnectionStatus::Renewing);
        assert_eq!(app.connect_renewal.attempts, 1);
        drop(app.update(Message::View(view::Message::Settings(
            view::SettingsMessage::ConnectDeviceReregistered(Err("offline".into())),
        ))));
        assert_eq!(app.cache.connect_stream_status, ConnectionStatus::Renewing);
        drop(app.update(Message::ConnectRenewalRetry));
        assert_eq!(app.connect_renewal.attempts, 2);
        drop(app.update(Message::View(view::Message::Settings(
            view::SettingsMessage::ConnectDeviceReregistered(Err("offline".into())),
        ))));
        assert_eq!(
            app.cache.connect_stream_status,
            ConnectionStatus::RenewFailed
        );

        // A result for another account (switched mid-flight) is ignored.
        drop(app.update(Message::View(view::Message::Settings(
            view::SettingsMessage::ConnectDeviceReregistered(Ok(reregistered(
                "other@example.invalid",
                "41",
            ))),
        ))));
        assert_eq!(app.connect_stream_config.as_ref().unwrap().device_id, "1");

        // The same id restarts the sleeping stream through its restart count.
        let restart = app.connect_stream_restart;
        drop(app.update(Message::View(view::Message::Settings(
            view::SettingsMessage::ConnectDeviceReregistered(Ok(reregistered(
                "refused@example.invalid",
                "1",
            ))),
        ))));
        assert_eq!(app.connect_stream_restart, restart + 1);
        assert_eq!(app.connect_stream_config.as_ref().unwrap().device_id, "1");
        assert_eq!(app.cache.connect_stream_status, ConnectionStatus::Renewing);

        // A new id restarts the stream under it; Connected ends the cycle.
        drop(app.update(Message::View(view::Message::Settings(
            view::SettingsMessage::ConnectDeviceReregistered(Ok(reregistered(
                "refused@example.invalid",
                "42",
            ))),
        ))));
        assert_eq!(app.connect_stream_config.as_ref().unwrap().device_id, "42");
        assert_eq!(app.cache.connect_device_id.as_deref(), Some("42"));
        assert_eq!(app.cache.connect_stream_status, ConnectionStatus::Renewing);
        drop(app.update(Message::ConnectStream(M::Connected)));
        assert_eq!(app.cache.connect_stream_status, ConnectionStatus::Connected);
        assert_eq!(app.connect_renewal, Renewal::default());

        // A late result from the earlier request (still replacing "1") must
        // not overwrite the registration the stream has moved on to.
        drop(app.update(Message::View(view::Message::Settings(
            view::SettingsMessage::ConnectDeviceReregistered(Ok(reregistered(
                "refused@example.invalid",
                "44",
            ))),
        ))));
        assert_eq!(app.connect_stream_config.as_ref().unwrap().device_id, "42");

        // A result landing after sign-out must not resurrect the device id.
        app.connect_stream_config = None;
        app.cache.connect_device_id = None;
        drop(app.update(Message::View(view::Message::Settings(
            view::SettingsMessage::ConnectDeviceReregistered(Ok(reregistered(
                "refused@example.invalid",
                "43",
            ))),
        ))));
        assert_eq!(app.cache.connect_device_id, None);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A removed computer is never renewed: Connect signed it out (C3).
    #[test]
    fn a_revoked_device_is_signed_out_not_renewed() {
        use crate::services::connect::grpc::trouble::Refusal;
        use crate::services::connect::grpc::ConnectStreamMessage as M;
        let _guard = crate::app::session::test_guard();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _enter = rt.enter();
        let root = std::env::temp_dir().join(format!("connect-revoked-{}", uuid::Uuid::new_v4()));
        let mut app = local_app(&root);
        app.cache.fiat_chain = crate::chain::ChainId::Bitcoin;
        app.cache.connect_stream_status = ConnectionStatus::Connected;
        assert!(
            app.update(Message::ConnectStream(M::Refused(Refusal::DeviceRevoked)))
                .units()
                > 0
        );
        assert_eq!(app.connect_renewal, Renewal::default());
        drop(app.update(Message::ConnectSignedOutBy(SignedOutBy::ComputerRemoved)));
        assert_eq!(
            app.cache.connect_signed_out_by,
            Some(SignedOutBy::ComputerRemoved)
        );
        drop(app.update(Message::View(view::Message::DismissConnectSignOut)));
        assert_eq!(app.cache.connect_signed_out_by, None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn connect_retry_keeps_failure_visible_until_connected() {
        use crate::services::connect::grpc::trouble::Trouble;
        use crate::services::connect::grpc::ConnectStreamMessage as M;

        let _guard = crate::app::session::test_guard();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _enter = rt.enter();
        let root = std::env::temp_dir().join(format!("connect-retry-{}", uuid::Uuid::new_v4()));
        let mut app = local_app(&root);
        app.cache.fiat_chain = crate::chain::ChainId::Bitcoin;
        for (failure, expected) in [
            (
                M::Trouble(Trouble::Unreachable),
                ConnectionStatus::Reconnecting,
            ),
            (M::Trouble(Trouble::Offline), ConnectionStatus::Offline),
            (
                M::Error("Connect session expired".into()),
                ConnectionStatus::Reconnecting,
            ),
        ] {
            drop(app.update(Message::ConnectStream(failure)));
            assert_eq!(app.cache.connect_stream_status, expected);
            drop(app.update(Message::ConnectStream(M::Disconnected("retrying".into()))));
            assert_eq!(app.cache.connect_stream_status, expected);
            drop(app.update(Message::ConnectStream(M::Connected)));
            assert_eq!(app.cache.connect_stream_status, ConnectionStatus::Connected);
        }
        // A live stream that drops reads as Reconnecting…, not Connecting….
        drop(app.update(Message::ConnectStream(M::Disconnected(
            "stream ended".into(),
        ))));
        assert_eq!(
            app.cache.connect_stream_status,
            ConnectionStatus::Reconnecting
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn local_global_revocation_closes_the_app_and_rejects_late_results() {
        let _guard = crate::app::session::test_guard();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _enter = rt.enter();
        let root = std::env::temp_dir().join(format!("local-revocation-{}", uuid::Uuid::new_v4()));
        let mut app = local_app(&root);
        drop(app.fork_feature_check());
        let attempt = app.local_feature_check.expect("a check in flight");
        let mut tab = crate::gui::tab::Tab::new(1, crate::gui::tab::State::App(app));
        drop(tab.update(crate::gui::tab::Message::Run(
            Message::LocalForkFeatureChecked {
                attempt,
                enabled: false,
            },
        )));
        assert!(matches!(tab.state, crate::gui::tab::State::Home(_)));
        drop(tab.update(crate::gui::tab::Message::Run(
            Message::LocalForkFeatureChecked {
                attempt,
                enabled: true,
            },
        )));
        assert!(matches!(tab.state, crate::gui::tab::State::Home(_)));
        std::fs::remove_dir_all(root).unwrap();
    }

    /// A Bitcoin Blake2b Cube App on an embedded fixture daemon, with the
    /// account grant on and, when `session`, an admitted Connect session.
    fn fork_app(root: &std::path::Path, session: bool) -> App {
        use std::str::FromStr;
        let chain = crate::chain::ChainId::BitcoinBlake2b;
        let descriptor =
            coincube_core::descriptors::CoincubeDescriptor::from_str(SPLIT_DESC).unwrap();
        let wallet = Arc::new(Wallet::new(descriptor).with_chain(chain));
        let datadir = CoincubeDirectory::new(root.to_path_buf());
        let (mut app, startup) = App::new_inner(
            Cache {
                fiat_chain: chain,
                network: chain.bitcoin_network(),
                ..Cache::default()
            },
            wallet,
            None,
            None,
            Config::new(false),
            split_backend_daemon(root, true),
            datadir,
            None,
            settings::CubeSettings::new("Fork".into(), chain),
            None,
        );
        drop(startup);
        app.panels.connect.account.bitcoin_blake2b_opt_in = true;
        app.panels.connect.account.features = Some(
            serde_json::from_value(serde_json::json!({
                "plans": [], "bitcoinBlake2bEnabled": true,
            }))
            .unwrap(),
        );
        if session {
            let mut client =
                crate::services::coincube::CoincubeClient::for_test("https://connect.example/");
            client.set_token("synthetic-test-token");
            app.panels.connect.install_admitted_client(client.clone());
            app.panels.connect.account.user = Some(crate::services::coincube::User {
                id: 7,
                email: "synthetic@example.invalid".into(),
                email_verified: Some(true),
            });
            app.panels.connect.account.step = state::connect::account::ConnectFlowStep::Dashboard;
            app.fork_connect_client = Some(client);
        }
        app
    }

    /// A Bitcoin Blake2b Cube without a Vault opens like a Bitcoin Cube
    /// without one, with no Liquid or Spark anywhere: no Breez client, no
    /// Spark backend, no Liquid or P2P panels, and no Spark on its Overview.
    #[test]
    fn a_fork_cube_without_a_vault_opens_without_liquid_or_spark() {
        let _guard = crate::app::session::test_guard();
        let root = std::env::temp_dir().join(format!("fork-no-vault-{}", uuid::Uuid::new_v4()));
        let chain = crate::chain::ChainId::BitcoinBlake2b;
        let signer =
            coincube_core::signer::MasterSigner::generate(chain.bitcoin_network()).unwrap();
        let cek = Arc::new(crate::services::connect::crypto::CubeEncryptionKey::derive(
            &signer,
            chain.bitcoin_network(),
        ));
        let (app, startup) = App::new_fork_without_wallet(
            cek,
            None,
            Config::new(false),
            CoincubeDirectory::new(root.clone()),
            settings::CubeSettings::new("Fork".into(), chain),
        )
        .expect("a Vault-less fork Cube opens");
        drop(startup);

        assert!(app.wallet.is_none());
        assert!(app.breez_client.is_none());
        assert!(app.spark_backend().is_none());
        assert!(app.panels.liquid_overview.is_none());
        assert!(app.panels.liquid_send.is_none());
        assert!(app.panels.p2p.is_none());
        assert!(app.cache.cube_encryption_key.is_some());
        assert_eq!(app.cache.chain(), chain);
        assert!(!app.cache.has_vault);

        let overview = crate::app::view::shared::landing::capture::inspect(
            "fork-no-vault-overview",
            app.view(),
            (1280, 800),
            "dark",
            coincube_ui::theme::Theme::dark(),
        );
        assert!(overview.contains("Create Vault"), "Create Vault comes next");
        assert!(!overview.contains("Spark"), "no Spark on a fork Cube");
        assert!(!overview.contains("Liquid"), "no Liquid on a fork Cube");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Through the Tab, as the unlock routes reach it: a signed-out,
    /// Vault-less fork Cube stays open, because nothing in it needs a Connect
    /// session. Its feature check runs at once, and a revoked feature closes it.
    #[test]
    fn a_signed_out_fork_cube_without_a_vault_stays_open_until_its_feature_is_revoked() {
        let _guard = crate::app::session::test_guard();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _enter = rt.enter();
        let root = std::env::temp_dir().join(format!("fork-no-vault-tab-{}", uuid::Uuid::new_v4()));
        let chain = crate::chain::ChainId::BitcoinBlake2b;
        let signer =
            coincube_core::signer::MasterSigner::generate(chain.bitcoin_network()).unwrap();
        let cek = Arc::new(crate::services::connect::crypto::CubeEncryptionKey::derive(
            &signer,
            chain.bitcoin_network(),
        ));
        let (app, startup) = App::new_fork_without_wallet(
            cek,
            None,
            Config::new(false),
            CoincubeDirectory::new(root.clone()),
            settings::CubeSettings::new("Fork".into(), chain),
        )
        .unwrap();
        drop(startup);
        assert!(
            !app.requires_connect(),
            "no Vault, nothing for Connect to serve"
        );
        let attempt = app
            .local_feature_check
            .expect("the feature check starts at once");

        assert!(app.fork_feature_unconfirmed());

        let mut tab = crate::gui::tab::Tab::new(1, crate::gui::tab::State::App(app));
        // Create Vault waits for the first answer: the installer would drop
        // a revocation that lands on it.
        drop(tab.update(crate::gui::tab::Message::Run(Message::View(
            view::Message::SetupVault,
        ))));
        assert!(
            matches!(tab.state, crate::gui::tab::State::App(_)),
            "no installer before the feature is confirmed"
        );
        drop(tab.update(crate::gui::tab::Message::Run(
            Message::LocalForkFeatureChecked {
                attempt,
                enabled: true,
            },
        )));
        let crate::gui::tab::State::App(app) = &mut tab.state else {
            panic!("a signed-out Vault-less fork Cube stays open");
        };
        assert!(!app.fork_feature_unconfirmed());
        // The next tick's check.
        drop(app.fork_feature_check());
        let attempt = app.local_feature_check.expect("a check in flight");
        drop(tab.update(crate::gui::tab::Message::Run(
            Message::LocalForkFeatureChecked {
                attempt,
                enabled: false,
            },
        )));
        assert!(
            matches!(tab.state, crate::gui::tab::State::Home(_)),
            "a revoked feature closes it"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A replaced App's feature check keeps running, and its answer reaches
    /// whichever App now holds the tab. That answer is not for this Cube: a
    /// `true` must not open Create Vault before the Cube's own check answers,
    /// and a `false` must not close it.
    #[test]
    fn another_apps_feature_answer_changes_nothing() {
        let _guard = crate::app::session::test_guard();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _enter = rt.enter();
        let root =
            std::env::temp_dir().join(format!("fork-stale-feature-{}", uuid::Uuid::new_v4()));
        let other_root = root.join("other");
        let mut other = local_app(&other_root);
        drop(other.fork_feature_check());
        let stale = other.local_feature_check.expect("a check in flight");
        drop(other);

        let chain = crate::chain::ChainId::BitcoinBlake2b;
        let signer =
            coincube_core::signer::MasterSigner::generate(chain.bitcoin_network()).unwrap();
        let cek = Arc::new(crate::services::connect::crypto::CubeEncryptionKey::derive(
            &signer,
            chain.bitcoin_network(),
        ));
        let (app, startup) = App::new_fork_without_wallet(
            cek,
            None,
            Config::new(false),
            CoincubeDirectory::new(root.clone()),
            settings::CubeSettings::new("Fork".into(), chain),
        )
        .unwrap();
        drop(startup);
        let attempt = app
            .local_feature_check
            .expect("the feature check starts at once");
        assert_ne!(attempt, stale, "each check has its own identity");

        let mut tab = crate::gui::tab::Tab::new(1, crate::gui::tab::State::App(app));
        for enabled in [true, false] {
            drop(tab.update(crate::gui::tab::Message::Run(
                Message::LocalForkFeatureChecked {
                    attempt: stale,
                    enabled,
                },
            )));
            let crate::gui::tab::State::App(app) = &tab.state else {
                panic!("another App's answer ({}) closed the Cube", enabled);
            };
            assert_eq!(app.local_feature_check, Some(attempt));
            assert!(app.local_feature_checked_at.is_none());
            assert!(app.local_feature_valid);
            assert!(
                app.fork_feature_unconfirmed(),
                "Create Vault still waits for this Cube's own check"
            );
        }

        // The Cube's own answer still applies.
        drop(tab.update(crate::gui::tab::Message::Run(
            Message::LocalForkFeatureChecked {
                attempt,
                enabled: true,
            },
        )));
        let crate::gui::tab::State::App(app) = &tab.state else {
            panic!("the Cube's own `true` keeps it open");
        };
        assert_eq!(app.local_feature_check, None);
        assert!(app.local_feature_checked_at.is_some());
        assert!(!app.fork_feature_unconfirmed());

        // A repeat of an answered check is no longer current either.
        drop(tab.update(crate::gui::tab::Message::Run(
            Message::LocalForkFeatureChecked {
                attempt,
                enabled: false,
            },
        )));
        assert!(matches!(tab.state, crate::gui::tab::State::App(_)));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The fork constructor refuses a Bitcoin Cube and a fork Cube that
    /// already has a Vault: both have their own way in.
    #[test]
    fn the_fork_no_vault_constructor_refuses_other_cubes() {
        let root =
            std::env::temp_dir().join(format!("fork-no-vault-refuse-{}", uuid::Uuid::new_v4()));
        let signer =
            coincube_core::signer::MasterSigner::generate(bitcoin::Network::Bitcoin).unwrap();
        let cek = Arc::new(crate::services::connect::crypto::CubeEncryptionKey::derive(
            &signer,
            bitcoin::Network::Bitcoin,
        ));
        let bitcoin = settings::CubeSettings::new("Bitcoin".into(), crate::chain::ChainId::Bitcoin);
        assert!(App::new_fork_without_wallet(
            cek.clone(),
            None,
            Config::new(false),
            CoincubeDirectory::new(root.clone()),
            bitcoin,
        )
        .is_err());
        let mut vaulted =
            settings::CubeSettings::new("Fork".into(), crate::chain::ChainId::BitcoinBlake2b);
        vaulted.vault_wallet_id = Some(settings::WalletId::new("abcd1234".into(), Some(1)));
        assert!(App::new_fork_without_wallet(
            cek,
            None,
            Config::new(false),
            CoincubeDirectory::new(root.clone()),
            vaulted,
        )
        .is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A Bitcoin Blake2b Cube App (`fork_app`) whose Vault is recorded on the Cube, signed in to an
    /// account whose `/connect/features` grant is `grant` (`None`: the field
    /// is absent from a loaded response).
    fn granted_fork_app(root: &std::path::Path, grant: Option<bool>) -> App {
        use std::str::FromStr;
        let mut app = fork_app(root, true);
        let descriptor =
            coincube_core::descriptors::CoincubeDescriptor::from_str(SPLIT_DESC).unwrap();
        let wallet = app.wallet.clone().unwrap();
        app.cube_settings = app
            .cube_settings
            .clone()
            .with_vault(settings::VaultIdentity::new(wallet.id(), Some(&descriptor)));
        app.panels.connect.account.step = state::connect::account::ConnectFlowStep::Dashboard;
        app.panels.connect.account.bitcoin_blake2b_opt_in = true;
        let features = match grant {
            Some(grant) => format!("{{\"plans\":[],\"bitcoin_blake2b_enabled\":{}}}", grant),
            None => "{\"plans\":[]}".to_string(),
        };
        app.panels.connect.account.features = Some(serde_json::from_str(&features).unwrap());
        assert!(app.panels.connect.account.is_authenticated());
        assert_eq!(
            app.panels.connect.account.bitcoin_blake2b_enabled(),
            grant == Some(true)
        );
        app
    }

    #[test]
    fn admitted_fork_session_does_not_invalidate_itself_on_same_session_completion() {
        let root = std::env::temp_dir().join(format!(
            "coincube-fork-same-session-{}",
            uuid::Uuid::new_v4()
        ));
        let app = fork_app(&root, true);
        let login = crate::services::coincube::LoginResponse {
            requires_2fa: false,
            token: "synthetic-test-token".into(),
            refresh_token: "synthetic-refresh".into(),
            user: crate::services::coincube::User {
                id: 7,
                email: "synthetic@example.invalid".into(),
                email_verified: Some(true),
            },
        };
        assert!(app.matches_admitted_connect_session(&login));
        let mut changed = login.clone();
        changed.token = "replacement".into();
        assert!(!app.matches_admitted_connect_session(&changed));
        changed = login;
        changed.user.id = 8;
        assert!(!app.matches_admitted_connect_session(&changed));
        drop(app);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn admitted_fork_survives_features_arriving_before_account_check() {
        let _guard = crate::app::session::test_guard();
        for features_first in [true, false] {
            let root = std::env::temp_dir().join(format!(
                "coincube-fork-account-order-{}",
                uuid::Uuid::new_v4()
            ));
            let mut app = fork_app(&root, true);
            app.panels.connect.account.features = None;
            app.panels.connect.account.step =
                state::connect::account::ConnectFlowStep::CheckingDuress {
                    status: state::connect::account::DuressGateStatus::Checking,
                };
            let generation = app.panels.connect.account.session_generation();
            let features = view::ConnectAccountMessage::FeaturesLoaded(
                Some(
                    serde_json::from_value(serde_json::json!({
                        "plans": [], "bitcoinBlake2bEnabled": true,
                    }))
                    .unwrap(),
                ),
                generation,
            );
            let checked = view::ConnectAccountMessage::DuressStateChecked(
                crate::services::coincube::DuressCheckOutcome::Ok(
                    crate::services::coincube::DuressState {
                        active: false,
                        unlock_at: None,
                        enrolled: false,
                        this_device_registered: false,
                    },
                ),
                generation,
                0,
            );
            let messages = if features_first {
                [features, checked]
            } else {
                [checked, features]
            };
            for (index, message) in IntoIterator::into_iter(messages).enumerate() {
                drop(app.update(Message::View(view::Message::ConnectAccount(message))));
                assert!(
                    app.authenticated_coincube_client().is_some(),
                    "an admitted Cube must survive either completion order"
                );
                assert_eq!(
                    app.cache.btcb2_server_enabled,
                    index == 1,
                    "claim access stays closed until both checks finish"
                );
            }
            drop(app);
            let _ = std::fs::remove_dir_all(root);
        }
    }

    #[test]
    fn global_beta_gate_closes_a_fork_when_the_current_server_grant_is_removed() {
        let root =
            std::env::temp_dir().join(format!("coincube-beta-revoke-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let mut app = granted_fork_app(&root, Some(true));
        assert!(app.authenticated_coincube_client().is_some());
        let generation = app.panels.connect.account.session_generation();
        let session_generation = *app.panels.session_generation.borrow();
        let denied =
            serde_json::from_value(serde_json::json!({"plans":[],"bitcoinBlake2bEnabled":false}))
                .unwrap();
        drop(app.update(Message::View(view::Message::ConnectAccount(
            view::ConnectAccountMessage::FeaturesLoaded(Some(denied), generation.wrapping_add(1)),
        ))));
        assert!(
            app.authenticated_coincube_client().is_some(),
            "a stale denial must be ignored"
        );
        let denied =
            serde_json::from_value(serde_json::json!({"plans":[],"bitcoinBlake2bEnabled":false}))
                .unwrap();
        drop(app.update(Message::View(view::Message::ConnectAccount(
            view::ConnectAccountMessage::FeaturesLoaded(Some(denied), generation),
        ))));
        assert!(app.authenticated_coincube_client().is_none());
        assert!(!app.cache.btcb2_server_enabled);
        assert_ne!(*app.panels.session_generation.borrow(), session_generation);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fork_panels_construct_without_any_sdk_client_or_marketplace_panel() {
        use std::str::FromStr;
        let descriptor = coincube_core::descriptors::CoincubeDescriptor::from_str(
            "wsh(or_d(pk([f5acc2fd]tpubD6NzVbkrYhZ4YgUx2ZLNt2rLYAMTdYysCRzKoLu2BeSHKvzqPaBDvf17GeBPnExUVPkuBpx4kniP964e2MxyzzazcXLptxLXModSVCVEV1T/<0;1>/*),and_v(v:pkh([8a64f2a9]tpubD6NzVbkrYhZ4WmzFjvQrp7sDa4ECUxTi9oby8K4FZkd3XCBtEdKwUiQyYJaxiJo5y42gyDWEczrFpozEjeLxMPxjf2WtkfcbpUdfvNnozWF/<0;1>/*),older(10))))#d72le4dr"
        ).unwrap();
        let chain = crate::chain::ChainId::BitcoinBlake2bTestnet4;
        let wallet = Arc::new(Wallet::new(descriptor).with_chain(chain));
        let root =
            std::env::temp_dir().join(format!("coincube-vault-only-{}", uuid::Uuid::new_v4()));
        let cache = Cache {
            fiat_chain: chain,
            network: chain.bitcoin_network(),
            ..Cache::default()
        };
        let mut panels = Panels::new(
            None,
            None,
            &cache,
            wallet,
            CoincubeDirectory::new(root.clone()),
            DaemonBackend::EmbeddedCoincubed(Some(crate::node::NodeType::Esplora)),
            None,
            Arc::new(Config::new(false)),
            "fixture".into(),
            "Fixture".into(),
            chain.api_str().into(),
        );
        assert!(panels.liquid_overview.is_none());
        assert!(panels.liquid_send.is_none());
        assert!(panels.liquid_swap.is_none());
        assert!(panels.liquid_receive.is_none());
        assert!(panels.liquid_transactions.is_none());
        assert!(panels.liquid_settings.is_none());
        assert!(panels.buy_sell.is_none() && panels.p2p.is_none());
        assert!(panels.vault_overview.is_some());
        panels.current = Menu::Liquid(crate::app::menu::LiquidSubMenu::Send);
        assert!(panels.current().is_none() && panels.current_mut().is_none());
        assert!(
            !root.exists(),
            "panel construction must not create a Bitcoin or fork datadir"
        );
    }

    #[tokio::test]
    async fn admitted_app_hands_exact_client_to_account_and_cube_consumers() {
        use httpmock::prelude::*;
        use iced::futures::StreamExt;
        use std::str::FromStr;
        for status in [200, 401, 503] {
            let server = MockServer::start_async().await;
            let user = server.mock_async(|when, then| {
            when.method(GET).path("/api/v1/user").header("authorization", "Bearer admitted-fixture");
            then.status(status).json_body(serde_json::json!({"id":7,"email":"fixture@example.invalid","email_verified":true}));
        }).await;
            let mut client = crate::services::coincube::CoincubeClient::new();
            client.base_url = server.base_url();
            client.set_token("admitted-fixture");
            let chain = crate::chain::ChainId::BitcoinBlake2bTestnet4;
            let desc = coincube_core::descriptors::CoincubeDescriptor::from_str(
            "wsh(or_d(pk([f5acc2fd]tpubD6NzVbkrYhZ4YgUx2ZLNt2rLYAMTdYysCRzKoLu2BeSHKvzqPaBDvf17GeBPnExUVPkuBpx4kniP964e2MxyzzazcXLptxLXModSVCVEV1T/<0;1>/*),and_v(v:pkh([8a64f2a9]tpubD6NzVbkrYhZ4WmzFjvQrp7sDa4ECUxTi9oby8K4FZkd3XCBtEdKwUiQyYJaxiJo5y42gyDWEczrFpozEjeLxMPxjf2WtkfcbpUdfvNnozWF/<0;1>/*),older(10))))#d72le4dr"
        ).unwrap();
            let root =
                std::env::temp_dir().join(format!("coincube-app-handoff-{}", uuid::Uuid::new_v4()));
            let endpoint = format!(
                "{}/api/v1/esplora/bitcoin-blake2b/testnet4",
                server.base_url()
            );
            let cfg: coincubed::config::Config = toml::from_str(&format!(
            "main_descriptor = '{}'\ndata_directory = '{}'\n[bitcoin_config]\nnetwork = '{}'\n[esplora_config]\naddr = '{}'\n", desc,root.display(),chain.api_str(),endpoint
        )).unwrap();
            // A GUI-only handoff fixture: no running daemon or live admission is claimed.
            server.mock_async(|when, then| {
            when.method(GET).path("/api/v1/connect/networks/bitcoin-blake2b-testnet4/anchor");
            then.status(200).json_body(serde_json::json!({"success":true,"data":{
                "network":"bitcoin-blake2b-testnet4","state":"available","anchor":{
                    "tip_hash":"11".repeat(32),"tip_height":973029,"tip_median_time_past":1800000000,
                    "observed_at":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(),
                    "observation":{"tip_height":973029,"fork":{"height":972000,"active":true},
                        "rdts":{"state":"flagday","flagday":{"height":972000,"expiry_time":1800010000_i64,"active":false}}}
                }}}));
        }).await;
            let (_, authority) = client
                .authenticated_backend(chain, &endpoint)
                .await
                .unwrap();
            use coincubed::connect::ConnectAnchorAuthority;
            assert!(authority.fresh_anchor().is_ok());
            let daemon = Arc::new(EmbeddedDaemon::unstarted_for_test(
                cfg,
                Some(authority.clone()),
            ));
            let cache = Cache {
                fiat_chain: chain,
                network: chain.bitcoin_network(),
                ..Cache::default()
            };
            let settings = settings::CubeSettings::new("Fixture".into(), chain);
            let (mut app, startup_tasks) = App::new_for_chain(
                cache,
                Arc::new(Wallet::new(desc).with_chain(chain)),
                None,
                client.clone(),
                Config::new(false),
                daemon,
                CoincubeDirectory::new(root.clone()),
                settings,
            )
            .unwrap();
            drop(startup_tasks);
            app.sync_panel_derived_cache_fields();
            let wallet = app.wallet.as_ref().unwrap();
            let fork_fingerprint = state::settings::recovery_kit::live_descriptor_fingerprint(
                wallet,
                &app.cube_settings.id,
                chain.api_str(),
            );
            assert!(fork_fingerprint.is_some());
            assert_eq!(app.cache.current_descriptor_fingerprint, fork_fingerprint);
            assert_ne!(
                app.cache.current_descriptor_fingerprint,
                state::settings::recovery_kit::live_descriptor_fingerprint(
                    wallet,
                    &app.cube_settings.id,
                    "testnet4",
                )
            );
            assert!(app.breez_client().is_none());
            assert!(app.spark_backend().is_none());
            assert!(app.wallet_registry.route_lightning_address().is_none());
            assert!(app.cache.has_connect_session);
            let cube_client = app.panels.connect.cube.client.as_ref().unwrap();
            assert_eq!(cube_client.base_url, server.base_url());
            assert_eq!(cube_client.token(), Some("admitted-fixture"));
            let handed = app.panels.connect.account.authenticated_client().unwrap();
            assert_eq!(handed.base_url, server.base_url());
            assert_eq!(handed.token(), Some("admitted-fixture"));
            let task = app
                .panels
                .connect
                .account
                .update_message(view::ConnectAccountMessage::Init);
            let mut stream = iced_runtime::task::into_stream(task).unwrap();
            while let Some(action) = stream.next().await {
                if let iced_runtime::Action::Output(Message::View(view::Message::ConnectAccount(
                    message,
                ))) = action
                {
                    let _ = app.update(Message::View(view::Message::ConnectAccount(message)));
                }
            }
            user.assert_hits_async(1).await;
            if status == 401 {
                assert!(app.fork_connect_client.is_none());
                assert!(app.panels.connect.account.authenticated_client().is_none());
                assert_eq!(
                    authority.fresh_anchor(),
                    Err(coincubed::connect::AdmissionError::Unavailable)
                );
            } else {
                let handed = app.panels.connect.account.authenticated_client().unwrap();
                assert_eq!(handed.base_url, client.base_url);
                assert_eq!(handed.token(), client.token());
                assert!(authority.fresh_anchor().is_ok());
                if status == 200 {
                    assert_eq!(app.panels.connect.account.user.as_ref().unwrap().id, 7);
                } else {
                    assert!(app.panels.connect.account.error.is_some());
                    let retry = app.update(Message::View(view::Message::ConnectAccount(
                        view::ConnectAccountMessage::Retry(view::RetryAction::Session),
                    )));
                    let mut stream = iced_runtime::task::into_stream(retry).unwrap();
                    while let Some(action) = stream.next().await {
                        if let iced_runtime::Action::Output(Message::View(
                            view::Message::ConnectAccount(message),
                        )) = action
                        {
                            let _ =
                                app.update(Message::View(view::Message::ConnectAccount(message)));
                        }
                    }
                    user.assert_hits_async(2).await;
                    assert_eq!(
                        app.panels
                            .connect
                            .account
                            .authenticated_client()
                            .unwrap()
                            .token(),
                        client.token()
                    );
                }
            }
            // Direct legacy bootstrap and a delayed Bitcoin-family completion
            // cannot mutate the admitted client, write connect.json, or mount gRPC.
            let bound_before = app.fork_connect_client.as_ref().map(|c| c.base_url.clone());
            let token_before = app.cache.connect_tokens.is_some();
            let legacy_tokens = Arc::new(tokio::sync::RwLock::new(
                crate::services::connect::client::auth::AccessTokenResponse {
                    access_token: "legacy-fixture".into(),
                    refresh_token: "legacy-refresh".into(),
                    expires_at: 1,
                },
            ));
            for message in [
                Message::InAppConnectLoginCompleted {
                    token: "legacy-fixture".into(),
                    refresh_token: "legacy-refresh".into(),
                    email: "other@example.invalid".into(),
                    epoch: 0,
                },
                Message::EnsureConnectReady,
                Message::TriggerConnectStreamReady {
                    network: chain.bitcoin_network(),
                    datadir: CoincubeDirectory::new(root.clone()),
                    tokens: legacy_tokens,
                    email: "other@example.invalid".into(),
                    cube_uuid: None,
                },
                Message::ConnectStreamReady(None),
            ] {
                let mut stream = iced_runtime::task::into_stream(app.update(message)).unwrap();
                assert!(matches!(
                    stream.next().await,
                    Some(iced_runtime::Action::Output(Message::View(view::Message::ShowError(text))))
                        if text.contains("Reopen this Cube")
                ));
                assert!(stream.next().await.is_none());
                assert_eq!(
                    app.fork_connect_client.as_ref().map(|c| c.base_url.clone()),
                    bound_before
                );
                assert_eq!(app.cache.connect_tokens.is_some(), token_before);
                assert!(app.connect_stream_config.is_none());
                assert!(!root.join("testnet4").join("connect.json").exists());
                assert!(!root
                    .join("bitcoin-blake2b-testnet4")
                    .join("connect.json")
                    .exists());
            }
            let replacement = crate::services::coincube::LoginResponse {
                requires_2fa: false,
                token: "replacement-fixture".into(),
                refresh_token: "replacement-refresh".into(),
                user: crate::services::coincube::User {
                    id: 8,
                    email: "replacement@example.invalid".into(),
                    email_verified: Some(true),
                },
            };
            let saved_before =
                state::connect::read_connect_secret(state::connect::CONNECT_KEYRING_USER);
            // Cover initial 401, explicit logout, and token replacement through
            // actual App dispatch. None may leave a login form on a dead backend.
            if status == 200 {
                drop(app.update(Message::View(view::Message::ConnectAccount(
                    view::ConnectAccountMessage::LogOut,
                ))));
            } else if status == 503 {
                drop(app.update(Message::View(view::Message::ConnectAccount(
                    view::ConnectAccountMessage::SetSession(replacement.clone(), 0),
                ))));
            }
            assert!(app.panels.connect.account.requires_authenticated_reopen());
            assert!(app.fork_connect_client.is_none());
            assert_eq!(
                state::connect::read_connect_secret(state::connect::CONNECT_KEYRING_USER),
                saved_before
            );
            for message in [
                view::ConnectAccountMessage::Init,
                view::ConnectAccountMessage::SubmitLogin,
                view::ConnectAccountMessage::VerifyOtp,
                view::ConnectAccountMessage::SetSession(replacement.clone(), 0),
                view::ConnectAccountMessage::SessionLoaded {
                    user: replacement.user.clone(),
                    plan: None,
                    epoch: 0,
                },
                view::ConnectAccountMessage::AdmittedUserLoaded {
                    user: Ok(replacement.user.clone()),
                    generation: app.panels.connect.account.session_generation(),
                    epoch: 0,
                },
                view::ConnectAccountMessage::Retry(view::RetryAction::Session),
            ] {
                let task = app.update(Message::View(view::Message::ConnectAccount(message)));
                // App batches the refused account task with grant persistence.
                // An empty batch can still have a stream; require no actions,
                // not a particular representation of an empty Task.
                if let Some(mut stream) = iced_runtime::task::into_stream(task) {
                    let action =
                        tokio::time::timeout(std::time::Duration::from_secs(1), stream.next())
                            .await
                            .expect("revoked account task must finish without actions");
                    assert!(action.is_none(), "revoked account task emitted an action");
                }
                assert!(app.panels.connect.account.requires_authenticated_reopen());
                assert!(app.panels.connect.account.authenticated_client().is_none());
                assert!(!app.panels.connect.account.is_authenticated());
                assert!(app.panels.connect.cube.client.is_none());
                assert_eq!(
                    authority.fresh_anchor(),
                    Err(coincubed::connect::AdmissionError::Unavailable)
                );
                assert_eq!(
                    state::connect::read_connect_secret(state::connect::CONNECT_KEYRING_USER),
                    saved_before
                );
            }
            assert!(!app.cache.has_connect_session);
            assert!(app.panels.connect.account.authenticated_client().is_none());
            assert!(app.panels.connect.cube.client.is_none());
            assert_eq!(
                authority.fresh_anchor(),
                Err(coincubed::connect::AdmissionError::Unavailable)
            );
            assert!(!root.join("bitcoin").exists());
            drop(app);
            if root.exists() {
                std::fs::remove_dir_all(root).unwrap();
            }
        }
    }

    /// The obligation is retired on *proof*, not on the daemon's acceptance.
    ///
    /// `start_rescan` returning `Ok` means bitcoind took the descriptor import;
    /// it stamps them before scanning and keeps the stamp through an aborting
    /// shutdown. So acceptance says nothing about whether history was found. A
    /// confirmed coin does — and until one exists the database is empty, which
    /// is the single state `coincubed::heal_scan_window` cannot derive a rescan
    /// date from, so discarding the obligation there strands the Vault.
    #[test]
    fn the_rescan_obligation_is_retired_by_a_confirmed_coin_not_by_acceptance() {
        fn coin(block_height: Option<i32>) -> crate::daemon::model::Coin {
            use coincube_core::miniscript::bitcoin as btc;
            use std::str::FromStr as _;
            crate::daemon::model::Coin {
                amount: btc::Amount::from_sat(10_000),
                outpoint: btc::OutPoint::null(),
                address: btc::Address::from_str(
                    "tb1qgpuzqmt7jgdk2t426e9uwfv68uyukh48nfq4qyl74rh0v8cn44rs4wnj68",
                )
                .expect("fixture address must parse")
                .assume_checked(),
                block_height,
                derivation_index: btc::bip32::ChildNumber::from_normal_idx(0).unwrap(),
                is_change: false,
                spend_info: None,
                is_immature: false,
                is_from_self: false,
            }
        }
        // This is the predicate `App::new` uses; it is the whole decision.
        let seeded =
            |coins: &[crate::daemon::model::Coin]| coins.iter().any(|c| c.block_height.is_some());

        assert!(!seeded(&[]), "an empty database is the stranding state");
        // The status set matters as much as the predicate: a Vault whose
        // history is entirely spent has no *unspent* coins, so asking the
        // cache — which lists only `Unconfirmed | Confirmed` — would call it
        // unseeded for ever and rescan the whole chain on every launch.
        // `settle_rescan_obligation` therefore asks the daemon across all four
        // statuses, and a spent coin still carries a `block_height`.
        assert!(
            seeded(&[coin(Some(145_604))]),
            "a coin that is confirmed — spent or not — is proof the scan reached its block"
        );
        assert!(
            !seeded(&[coin(None)]),
            "an unconfirmed coin is not evidence the history was scanned"
        );
        assert!(seeded(&[coin(Some(145_604))]));
        assert!(
            seeded(&[coin(None), coin(Some(145_604))]),
            "one confirmed coin among unconfirmed ones is still proof"
        );
    }

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
    fn a_completed_full_scan_retires_the_obligation_once_and_only_on_scanning_backends() {
        use coincubed::commands::HistorySync;
        let scanning = HistorySync {
            full_scan_in_progress: true,
            ..Default::default()
        };
        let completed = HistorySync {
            full_scan_completed_at: Some(1_791_329_974),
            ..Default::default()
        };
        let esplora = DaemonBackend::EmbeddedCoincubed(Some(NodeType::Esplora));
        let electrum = DaemonBackend::EmbeddedCoincubed(Some(NodeType::Electrum));
        assert!(full_scan_settles_rescan_obligation(
            &esplora, false, &completed
        ));
        assert!(full_scan_settles_rescan_obligation(
            &electrum, false, &completed
        ));
        // Once per session, not on every refresh after it.
        assert!(!full_scan_settles_rescan_obligation(
            &esplora, true, &completed
        ));
        // Still scanning: nothing proven yet.
        assert!(!full_scan_settles_rescan_obligation(
            &esplora, false, &scanning
        ));
        // A local node rescans from a date, which only coins can vouch for.
        for backend in [
            DaemonBackend::EmbeddedCoincubed(Some(NodeType::Bitcoind)),
            DaemonBackend::EmbeddedCoincubed(None),
            DaemonBackend::ExternalCoincubed,
        ] {
            assert!(!full_scan_settles_rescan_obligation(
                &backend, false, &completed
            ));
        }
    }

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
            keychain_keys_recorded: false,
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
    fn spark_stable_balance_preference_defaults_none_and_round_trips() {
        // Old settings.json without the field parses as "never decided".
        let cube: CubeSettings = serde_json::from_value(serde_json::json!({
            "id": "cube-1",
            "name": "Vault",
            "network": "bitcoin",
            "created_at": 0
        }))
        .unwrap();
        assert_eq!(cube.spark_stable_balance, None);
        // ...and is not written back while undecided.
        let json = serde_json::to_value(&cube).unwrap();
        assert!(json.get("spark_stable_balance").is_none());

        for decision in [true, false] {
            let mut decided = cube.clone();
            decided.spark_stable_balance = Some(decision);
            let json = serde_json::to_string(&decided).unwrap();
            let back: CubeSettings = serde_json::from_str(&json).unwrap();
            assert_eq!(back.spark_stable_balance, Some(decision));
        }
    }

    #[test]
    fn stable_balance_reconcile_reapplies_the_recorded_decision() {
        use StableBalanceReconcile::*;
        // Recorded decision wins over the SDK's local cache, whatever the
        // holding says.
        for holds_usdb in [false, true] {
            assert_eq!(
                reconcile_stable_balance(Some(true), true, holds_usdb),
                InSync
            );
            assert_eq!(
                reconcile_stable_balance(Some(false), false, holds_usdb),
                InSync
            );
            assert_eq!(
                reconcile_stable_balance(Some(true), false, holds_usdb),
                Apply(true)
            );
            assert_eq!(
                reconcile_stable_balance(Some(false), true, holds_usdb),
                Apply(false)
            );
        }
    }

    #[test]
    fn stable_balance_reconcile_without_a_decision_follows_the_evidence() {
        use StableBalanceReconcile::*;
        // SDK on, nothing recorded: a pre-preference Cube — adopt it.
        assert_eq!(reconcile_stable_balance(None, true, false), Adopt);
        assert_eq!(reconcile_stable_balance(None, true, true), Adopt);
        // SDK off and USDB held: the recovered-elsewhere case — ask.
        assert_eq!(reconcile_stable_balance(None, false, true), PromptHolding);
        // SDK off, no USDB: nothing to say.
        assert_eq!(reconcile_stable_balance(None, false, false), InSync);
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
    fn connection_status_words_and_lights_follow_the_design() {
        use ConnectionStatus as S;
        assert!(!S::Inactive.is_visible());
        assert_eq!(S::Inactive.light(), None);
        let expected = [
            (S::Connecting, "Connecting…", ConnectLight::Connecting),
            (S::Connected, "On", ConnectLight::On),
            (S::Offline, "Offline", ConnectLight::Offline),
            (S::Reconnecting, "Reconnecting…", ConnectLight::Reconnecting),
            (S::Renewing, "Reconnecting…", ConnectLight::Reconnecting),
            (S::RenewFailed, "Offline", ConnectLight::NeedsYou),
        ];
        for (status, word, light) in expected {
            assert!(status.is_visible());
            assert_eq!(status.word(), word, "{:?}", status);
            assert_eq!(status.light(), Some(light), "{:?}", status);
        }
        // Only the user's own trouble is red; uploads wait unless On.
        assert!(S::Connected.allows_uploads());
        for status in [
            S::Connecting,
            S::Offline,
            S::Reconnecting,
            S::Renewing,
            S::RenewFailed,
        ] {
            assert!(!status.allows_uploads(), "{:?}", status);
        }
        // No line says "registration", "stream" or "device".
        for status in [
            S::Connecting,
            S::Connected,
            S::Offline,
            S::Reconnecting,
            S::Renewing,
        ] {
            let line = status.line().unwrap();
            for jargon in ["registration", "stream", "device", "gRPC"] {
                assert!(!line.contains(jargon), "{:?}: {}", status, line);
            }
        }
        assert_eq!(S::RenewFailed.line(), None);
    }

    #[test]
    fn the_vault_alert_follows_its_own_sync_result() {
        let failing = SyncStatus::SyncFailing {
            message: "Esplora client error".into(),
            since: 0,
            retry_progress: None,
        };
        // Synced: no alert, whatever Connect says.
        assert_eq!(
            vault_alert(
                &SyncStatus::Synced,
                true,
                ConnectionStatus::Offline,
                true,
                None,
                1
            ),
            None
        );
        let alert = |connect, signed_in| {
            vault_alert(&failing, true, connect, signed_in, None, 974_600).unwrap()
        };
        assert_eq!(alert(ConnectionStatus::Offline, true).word(), "Offline");
        assert_eq!(
            alert(ConnectionStatus::Reconnecting, true).word(),
            "Reconnecting…"
        );
        let signed_out = alert(ConnectionStatus::Inactive, false);
        assert_eq!(signed_out.word(), "Not updating");
        assert!(signed_out.needs_sign_in);
        assert_eq!(
            signed_out.title(),
            "Sign in to Connect to update this Vault"
        );
        // After automatic renewal failed nothing retries: point at Try again.
        let failed = alert(ConnectionStatus::RenewFailed, true);
        assert_eq!(failed.stall, VaultStall::NeedsReconnect);
        assert_eq!(failed.word(), "Not updating");
        assert!(failed.detail().contains("Try again"));
        assert!(!failed.detail().contains("keeps trying"));
        // A Vault on its own node says Not updating, never Connect's words.
        let local = vault_alert(&failing, false, ConnectionStatus::Offline, true, None, 1).unwrap();
        assert_eq!(local.word(), "Not updating");
        assert!(!local.via_connect);
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
        let signer = MasterSigner::generate(cube.network.bitcoin_network()).unwrap();
        signer
            .store_encrypted(
                root,
                cube.network.bitcoin_network(),
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
                    MasterSigner::mnemonics_folder(root, cube.network.bitcoin_network()),
                    MasterSigner::mnemonics_folder(root, cube.network.bitcoin_network()).exists(),
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
        // Enrollment cannot become a fresh oracle after Cube A's ordinary
        // unlock counter has imposed a delay, even for its real PIN.
        let mut throttle = crate::services::unlock::throttle::ThrottleState::load(root.path());
        // Twelve failures reach the 300s cap. Five gave 4s, which the
        // seed-decrypting check below can outlast on a loaded machine.
        for _ in 0..12 {
            throttle.record_failure(root.path(), "cube-a");
        }
        assert!(duress_pin_collision_check_blocking(root.path(), "1234")
            .unwrap_err()
            .contains("Too many incorrect PINs"));
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
        let mut throttle = crate::services::unlock::throttle::ThrottleState::load(root.path());
        // Twelve failures reach the 300s cap. Five gave 4s, which the
        // seed-decrypting check below can outlast on a loaded machine.
        for _ in 0..12 {
            throttle.record_failure(root.path(), "cube-a");
        }
        assert!(verify_regular_cube_pin_blocking(root.path(), "1234")
            .unwrap_err()
            .contains("Too many incorrect PINs"));
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
            network: crate::chain::ChainId::Bitcoin,
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
            network: crate::chain::ChainId::Bitcoin,
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

#[cfg(test)]
mod duress_chain_identity_tests {
    //! Duress arm/disarm across a datadir that holds a Cube on a chain this
    //! build cannot run (BTCB2 plan PR 2; CodeRabbit finding on coincube#370).
    //!
    //! Both operations write into every Cube's chain-keyed seed folder. A
    //! dormant-chain Cube's folder is storage this version must leave
    //! untouched, so the *whole* operation is refused up front — no decoy or
    //! marker written anywhere, no local state reset — and an ordinary
    //! Bitcoin-family device keeps disarming exactly as before.

    use super::*;
    use crate::app::settings::{CubeSettings, Settings, SETTINGS_FILE_NAME};
    use crate::chain::{ChainId, ChainIdExt};
    use crate::dir::CoincubeDirectory;
    use crate::services::duress::DuressLocalState;
    use std::path::{Path, PathBuf};

    fn temp_root(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "coincube-duress-chain-{}-{}-{}",
            tag,
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_settings(root: &Path, dir_name: &str, cubes: Vec<CubeSettings>) {
        let dir = root.join(dir_name);
        std::fs::create_dir_all(&dir).unwrap();
        let settings = Settings {
            cubes,
            ..Settings::default()
        };
        std::fs::write(
            dir.join(SETTINGS_FILE_NAME),
            serde_json::to_vec_pretty(&settings).unwrap(),
        )
        .unwrap();
    }

    /// A Cube record on `chain`, armed exactly as enrollment arms one: a real
    /// marker for `duress_pin` under a recorded slot name, written into the
    /// chain's own seed folder.
    fn armed_cube(root: &Path, id: &str, chain: ChainId, duress_pin: &str) -> CubeSettings {
        let mut cube = CubeSettings::new_with_raw_id(id.to_string(), id.to_string(), chain);
        cube.created_at = 1_700_000_000;
        let slot = crate::services::unlock::marker::new_file_name(cube.created_at);
        crate::services::unlock::marker::write(root, chain, &cube.id, &slot, duress_pin, None)
            .expect("arm");
        cube.duress_slot_file = Some(slot);
        cube
    }

    /// Every file under `root` with its bytes — the "nothing changed" oracle.
    fn snapshot(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        fn walk(p: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
            if let Ok(rd) = std::fs::read_dir(p) {
                for e in rd.flatten() {
                    let path = e.path();
                    if path.is_dir() {
                        walk(&path, out);
                    } else {
                        out.push((path.clone(), std::fs::read(&path).unwrap_or_default()));
                    }
                }
            }
        }
        let mut out = Vec::new();
        walk(root, &mut out);
        out.sort();
        out
    }

    fn enrolled_state(root: &Path) {
        DuressLocalState {
            enrolled: true,
            active: false,
            account_id: Some("acct-1".to_string()),
            duress_code: Some("ciphertext".to_string()),
            ..DuressLocalState::default()
        }
        .save(root)
        .expect("save local state");
    }

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(f)
    }

    /// The reviewer's case: a Bitcoin Cube armed in `bitcoin/`, and a Cube on a
    /// dormant chain in its own folder, also holding a marker (as a build that
    /// can run that chain would have left it). Disarm must refuse as a whole:
    /// the Bitcoin marker still opens for the duress PIN, the BTCB2 folder is
    /// byte-identical, and the local state still says enrolled.
    #[test]
    fn disarm_is_refused_as_a_whole_when_a_dormant_chain_cube_is_present() {
        for chain in [ChainId::BitcoinBlake2b, ChainId::BitcoinBlake2bTestnet4] {
            let root = temp_root("clear");
            let bitcoin = armed_cube(&root, "btc", ChainId::Bitcoin, "8765");
            let fork = armed_cube(&root, "fork", chain, "8765");
            write_settings(&root, "bitcoin", vec![bitcoin.clone()]);
            write_settings(&root, chain.dir_name(), vec![fork.clone()]);
            enrolled_state(&root);
            let before = snapshot(&root);

            let err = block_on(clear_duress_enrollment(CoincubeDirectory::new(
                root.clone(),
            )))
            .expect_err("disarm must refuse");
            assert_eq!(err, DURESS_DORMANT_CHAIN_MSG, "{:?}", chain);

            assert_eq!(
                snapshot(&root),
                before,
                "{:?}: something was written",
                chain
            );
            // The Bitcoin marker is untouched: the duress PIN still opens it.
            assert!(crate::services::unlock::marker::verify(
                &root,
                ChainId::Bitcoin,
                &bitcoin.id,
                bitcoin.duress_slot_file.as_deref(),
                "8765",
                None,
            ));
            assert!(crate::services::unlock::marker::verify(
                &root,
                chain,
                &fork.id,
                fork.duress_slot_file.as_deref(),
                "8765",
                None,
            ));
            assert!(DuressLocalState::load(&root).unwrap().enrolled);
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    /// A record claiming a dormant chain but sitting in `bitcoin/settings.json`
    /// (a copied or edited file) is refused too — in either record order — and
    /// the reason names the folder disagreement, not the dormant runtime.
    #[test]
    fn a_dormant_record_in_the_bitcoin_folder_is_refused_in_either_order() {
        for chain in [ChainId::BitcoinBlake2b, ChainId::BitcoinBlake2bTestnet4] {
            for fork_first in [true, false] {
                let root = temp_root("mixed");
                let bitcoin = armed_cube(&root, "btc", ChainId::Bitcoin, "8765");
                let mut claimant = CubeSettings::new_with_raw_id(
                    "claimant".to_string(),
                    "claimant".to_string(),
                    chain,
                );
                claimant.duress_slot_file = Some("slot-from-nowhere".to_string());
                let cubes = if fork_first {
                    vec![claimant, bitcoin.clone()]
                } else {
                    vec![bitcoin.clone(), claimant]
                };
                write_settings(&root, "bitcoin", cubes);
                enrolled_state(&root);
                let before = snapshot(&root);

                let err = block_on(clear_duress_enrollment(CoincubeDirectory::new(
                    root.clone(),
                )))
                .expect_err("disarm must refuse");
                assert!(
                    err.contains(chain.label()) && err.contains("Bitcoin folder"),
                    "{}",
                    err
                );
                assert!(err.contains("Nothing was changed"), "{}", err);
                assert_eq!(
                    snapshot(&root),
                    before,
                    "{:?} fork_first={}",
                    chain,
                    fork_first
                );
                assert!(
                    !root.join(chain.dir_name()).exists(),
                    "a folder was created for the claim"
                );
                let _ = std::fs::remove_dir_all(&root);
            }
        }
    }

    /// Arming refuses at the same boundary, before the `arming` breadcrumb and
    /// before any marker: the wizard's pre-flight check and the persist step
    /// both say why, and the device is left exactly as it was.
    #[test]
    fn arming_is_refused_before_any_breadcrumb_or_marker_when_a_dormant_cube_is_present() {
        let root = temp_root("arm");
        let mut bitcoin =
            CubeSettings::new_with_raw_id("btc".to_string(), "btc".to_string(), ChainId::Bitcoin);
        bitcoin.created_at = 1_700_000_000;
        let mut fork = CubeSettings::new_with_raw_id(
            "fork".to_string(),
            "fork".to_string(),
            ChainId::BitcoinBlake2b,
        );
        fork.created_at = 1_700_000_000;
        write_settings(&root, "bitcoin", vec![bitcoin]);
        write_settings(&root, "bitcoin-blake2b", vec![fork]);
        let before = snapshot(&root);

        assert_eq!(
            duress_pin_collision_check_blocking(&root, "4321").unwrap_err(),
            DURESS_DORMANT_CHAIN_MSG
        );
        let err = block_on(persist_duress_enrollment(
            CoincubeDirectory::new(root.clone()),
            zeroize::Zeroizing::new("4321".to_string()),
            zeroize::Zeroizing::new("code".to_string()),
            None,
        ))
        .expect_err("arming must refuse");
        assert_eq!(err, DURESS_DORMANT_CHAIN_MSG);
        assert_eq!(snapshot(&root), before, "arming wrote something");
        // No breadcrumb, no state file at all.
        assert_eq!(
            DuressLocalState::load(&root).unwrap(),
            DuressLocalState::default()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A seed-file upgrade that turned duress off leaves a notice for the next
    /// unlock. Enrolling again before it is shown makes it false: it must go
    /// in the same write that records the enrolment.
    #[test]
    fn enrolling_again_clears_a_pending_duress_off_notice() {
        let root = temp_root("re-enrol");
        let mut bitcoin =
            CubeSettings::new_with_raw_id("btc".to_string(), "btc".to_string(), ChainId::Bitcoin);
        bitcoin.created_at = 1_700_000_000;
        write_settings(&root, "bitcoin", vec![bitcoin]);
        DuressLocalState {
            migration_disarm_notice: true,
            ..DuressLocalState::default()
        }
        .save(&root)
        .unwrap();

        block_on(persist_duress_enrollment(
            CoincubeDirectory::new(root.clone()),
            zeroize::Zeroizing::new("4321".to_string()),
            zeroize::Zeroizing::new("code".to_string()),
            None,
        ))
        .expect("enrolment succeeds");

        let st = DuressLocalState::load(&root).unwrap();
        assert!(st.enrolled);
        assert!(
            !st.migration_disarm_notice,
            "a re-enrolled device would be told its duress PIN no longer works"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The launch-time reconcilers reach `clear_duress_enrollment` through the
    /// same preflight: they surface the refusal and mutate nothing.
    #[test]
    fn the_reconcile_callers_refuse_without_side_effects() {
        let root = temp_root("reconcile");
        let bitcoin = armed_cube(&root, "btc", ChainId::Bitcoin, "8765");
        let fork = armed_cube(&root, "fork", ChainId::BitcoinBlake2b, "8765");
        write_settings(&root, "bitcoin", vec![bitcoin]);
        write_settings(&root, "bitcoin-blake2b", vec![fork]);
        // An orphan: armed while the local state never recorded the enrollment.
        DuressLocalState {
            arming: true,
            ..DuressLocalState::default()
        }
        .save(&root)
        .unwrap();
        let before = snapshot(&root);

        let err = block_on(reconcile_duress_orphan(CoincubeDirectory::new(
            root.clone(),
        )))
        .expect_err("orphan reconcile must refuse");
        assert_eq!(err, DURESS_DORMANT_CHAIN_MSG);
        assert_eq!(snapshot(&root), before);

        // A Connect enrollment the server no longer reports.
        DuressLocalState {
            enrolled: true,
            account_id: Some("acct-1".to_string()),
            ..DuressLocalState::default()
        }
        .save(&root)
        .unwrap();
        let before = snapshot(&root);
        let err = block_on(reconcile_duress_disarm(
            CoincubeDirectory::new(root.clone()),
            "acct-1".to_string(),
        ))
        .expect_err("disarm reconcile must refuse");
        assert_eq!(err, DURESS_DORMANT_CHAIN_MSG);
        assert_eq!(snapshot(&root), before);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Control: a device with only Bitcoin-family Cubes, across two chain
    /// folders, still disarms completely — decoys land in each Cube's own
    /// folder, the local state resets.
    #[test]
    fn an_ordinary_bitcoin_family_device_still_disarms() {
        let root = temp_root("control");
        let main = armed_cube(&root, "main", ChainId::Bitcoin, "8765");
        let sig = armed_cube(&root, "sig", ChainId::Signet, "8765");
        write_settings(&root, "bitcoin", vec![main.clone()]);
        write_settings(&root, "signet", vec![sig.clone()]);
        enrolled_state(&root);

        block_on(clear_duress_enrollment(CoincubeDirectory::new(
            root.clone(),
        )))
        .expect("ordinary disarm");

        for (cube, chain) in [(&main, ChainId::Bitcoin), (&sig, ChainId::Signet)] {
            assert!(cube.has_duress_slot(&root), "slot kept for {}", cube.id);
            assert!(
                !crate::services::unlock::marker::verify(
                    &root,
                    chain,
                    &cube.id,
                    cube.duress_slot_file.as_deref(),
                    "8765",
                    None,
                ),
                "duress PIN still opens {}",
                cube.id
            );
        }
        assert_eq!(
            DuressLocalState::load(&root).unwrap(),
            DuressLocalState::default()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The preflight itself: a directory that is no chain this build knows is
    /// refused; a clean Bitcoin-family inventory reports each folder's chain.
    #[test]
    fn the_inventory_rejects_unknown_folders_and_reports_chains() {
        let root = temp_root("inventory");
        write_settings(&root, "bitcoin", vec![]);
        write_settings(&root, "testnet4", vec![]);
        let mut inv = duress_cube_inventory(&root).unwrap();
        inv.sort_by_key(|e| e.chain.dir_name());
        assert_eq!(
            inv.iter().map(|e| e.chain).collect::<Vec<_>>(),
            vec![ChainId::Bitcoin, ChainId::Testnet4]
        );

        write_settings(&root, "not-a-chain", vec![]);
        let err = duress_cube_inventory(&root).unwrap_err();
        assert!(
            err.contains("not-a-chain") && err.contains("Nothing was changed"),
            "{}",
            err
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// App-level shell fixtures (a Bitcoin Cube with a Vault) and the tests that
/// drive them.
#[cfg(test)]
pub(crate) mod shell_tests {
    use super::*;
    use iced::futures::StreamExt;
    use std::str::FromStr;

    const DESC: &str = "wsh(or_d(pk([f5acc2fd]tpubD6NzVbkrYhZ4YgUx2ZLNt2rLYAMTdYysCRzKoLu2BeSHKvzqPaBDvf17GeBPnExUVPkuBpx4kniP964e2MxyzzazcXLptxLXModSVCVEV1T/<0;1>/*),and_v(v:pkh([8a64f2a9]tpubD6NzVbkrYhZ4WmzFjvQrp7sDa4ECUxTi9oby8K4FZkd3XCBtEdKwUiQyYJaxiJo5y42gyDWEczrFpozEjeLxMPxjf2WtkfcbpUdfvNnozWF/<0;1>/*),older(10))))#d72le4dr";

    /// A Bitcoin Cube with a Vault and an embedded (unstarted) daemon,
    /// with the account grant on so the claim entry is available.
    pub(crate) fn bitcoin_app(root: &std::path::Path) -> (App, Arc<Wallet>) {
        let descriptor = coincube_core::descriptors::CoincubeDescriptor::from_str(DESC).unwrap();
        let wallet = Arc::new(Wallet::new(descriptor.clone()));
        let cfg: coincubed::config::Config = toml::from_str(&format!(
                "main_descriptor = '{}'\ndata_directory = '{}'\n[bitcoin_config]\nnetwork = 'bitcoin'\n[esplora_config]\naddr = 'https://api.example.invalid/api/v1/esplora/bitcoin/mainnet'\n",
                descriptor,
                root.display()
            ))
            .unwrap();
        let daemon: Arc<dyn Daemon + Sync + Send> =
            Arc::new(EmbeddedDaemon::unstarted_for_test(cfg, None));
        let chain = crate::chain::ChainId::Bitcoin;
        let cache = Cache {
            fiat_chain: chain,
            network: chain.bitcoin_network(),
            btcb2_server_enabled: true,
            ..Cache::default()
        };
        let (mut app, startup) = App::new_inner(
            cache,
            wallet.clone(),
            None,
            None,
            Config::new(false),
            daemon,
            CoincubeDirectory::new(root.to_path_buf()),
            None,
            settings::CubeSettings::new("Fixture".into(), chain),
            None,
        );
        drop(startup);
        app.cache.btcb2_server_enabled = true;
        app.panels.connect.account.bitcoin_blake2b_opt_in = true;
        app.panels.connect.account.features = Some(
            serde_json::from_value(serde_json::json!({
                "plans": [], "bitcoinBlake2bEnabled": true,
            }))
            .unwrap(),
        );
        (app, wallet)
    }

    #[test]
    fn create_copy_starts_the_installer_only_while_no_copy_is_on_disk() {
        let root = std::env::temp_dir().join(format!("create-copy-{}", uuid::Uuid::new_v4()));
        let (mut app, wallet) = {
            let _guard = crate::app::session::test_guard();
            bitcoin_app(&root)
        };
        let starts = |task: Task<Message>| {
            use iced::futures::StreamExt;
            let Some(stream) = iced_runtime::task::into_stream(task) else {
                return false;
            };
            iced::futures::executor::block_on(stream.collect::<Vec<_>>())
                .into_iter()
                .any(|action| {
                    matches!(
                        action,
                        iced_runtime::Action::Output(Message::View(
                            view::Message::StartClaimBlake2b
                        ))
                    )
                })
        };
        assert!(!app.cache.btcb2_already_claimed);
        assert!(
            starts(app.update(Message::View(view::Message::CreateBlake2bCopy))),
            "no copy yet: the installer starts"
        );

        // A copy made since the Cube opened; the cached flag is stale.
        let fork_dir = app
            .datadir
            .network_directory(crate::chain::ChainId::BitcoinBlake2b);
        std::fs::create_dir_all(fork_dir.path()).unwrap();
        let mut copy = settings::CubeSettings::new_with_raw_id(
            uuid::Uuid::new_v4().to_string(),
            "Fixture · BTCB2".to_string(),
            crate::chain::ChainId::BitcoinBlake2b,
        );
        copy.vault_wallet_id = Some(crate::app::settings::WalletId::new(
            wallet.descriptor_checksum.clone(),
            Some(1),
        ));
        std::fs::write(
            fork_dir.path().join(settings::SETTINGS_FILE_NAME),
            serde_json::to_vec(&settings::Settings {
                cubes: vec![copy],
                ..Default::default()
            })
            .unwrap(),
        )
        .unwrap();
        assert!(!app.cache.btcb2_already_claimed, "stale by construction");
        assert!(
            !starts(app.update(Message::View(view::Message::CreateBlake2bCopy))),
            "a second installer must not start"
        );
        assert!(app.cache.btcb2_already_claimed, "corrected from the disk");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_split_result_reaches_the_coins_panel_from_another_panel() {
        let root = std::env::temp_dir().join(format!("split-result-{}", uuid::Uuid::new_v4()));
        let (mut app, _) = bitcoin_app(&root);
        app.panels
            .coins
            .as_mut()
            .unwrap()
            .start_building_split_for_test();
        drop(app.set_current_panel(Menu::Vault(menu::VaultSubMenu::Overview)));
        drop(app.update(Message::SplitPrepared(Err("Couldn't build".into()))));
        assert_eq!(
            app.panels.coins.as_ref().unwrap().split_progress_for_test(),
            Some((false, Some("Couldn't build".to_string()))),
            "unlocked, with the error"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn app_otp_waits_for_cache_commit_before_broadcasting_sign_in() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let _enter = rt.enter();
                let root = std::env::temp_dir()
                    .join(format!("app-otp-broadcast-{}", uuid::Uuid::new_v4()));
                let (mut first, _) = bitcoin_app(&root);
                let (mut second, _) = bitcoin_app(&root);
                first.cache.datadir_path = CoincubeDirectory::new(root.clone());
                second.cache.datadir_path = CoincubeDirectory::new(root.clone());
                first.panels.connect.account.step =
                    state::connect::account::ConnectFlowStep::OtpVerification {
                        email: "first@example.invalid".into(),
                        otp: "123456".into(),
                        sending: true,
                        is_signup: false,
                        cooldown: 0,
                    };
                second.panels.connect.account.user = Some(crate::services::coincube::User {
                    id: 99,
                    email: "second@example.invalid".into(),
                    email_verified: Some(true),
                });
                let mut gui = crate::gui::GUI::reviewer_claim_gui(first, second);
                let login = crate::services::coincube::LoginResponse {
                    requires_2fa: false,
                    token: "first-token".into(),
                    refresh_token: "first-refresh".into(),
                    user: crate::services::coincube::User {
                        id: 7,
                        email: "first@example.invalid".into(),
                        email_verified: Some(true),
                    },
                };
                let task = gui.reviewer_account(
                    1,
                    view::ConnectAccountMessage::OtpVerified(login.clone(), 0, 0),
                );
                assert!(!gui.reviewer_app_mut(1).claim_session_invalidated);
                assert_eq!(
                    gui.reviewer_app_mut(0)
                        .panels
                        .connect
                        .account
                        .client
                        .token(),
                    None
                );
                drop(task);
                // Another process supersedes the cache between persistence and
                // this App's adoption. The GUI must not broadcast the raw
                // AuthCommitAccepted completion to the sibling first.
                let network_dir = {
                    let app = gui.reviewer_app_mut(0);
                    app.cache.datadir_path.network_directory(app.cache.network)
                };
                std::fs::create_dir_all(network_dir.path()).unwrap();
                std::fs::write(
                    network_dir
                        .path()
                        .join(crate::services::connect::client::cache::CONNECT_CACHE_FILENAME),
                    serde_json::to_vec(&serde_json::json!({
                        "active_email": "first@example.invalid",
                        "accounts": [{
                            "email": "first@example.invalid",
                            "tokens": {
                                "access_token": "newer-token",
                                "refresh_token": "newer-refresh",
                                "expires_at": 1000
                            }
                        }]
                    }))
                    .unwrap(),
                )
                .unwrap();
                drop(gui.reviewer_account(
                    1,
                    view::ConnectAccountMessage::AuthCommitAccepted {
                        login,
                        epoch: 0,
                        refreshed: false,
                    },
                ));
                assert!(!gui.reviewer_app_mut(1).claim_session_invalidated);
                assert_eq!(
                    gui.reviewer_app_mut(0)
                        .panels
                        .connect
                        .account
                        .client
                        .token(),
                    None
                );
                let _ = std::fs::remove_dir_all(root);
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[tokio::test]
    async fn rejected_refresh_does_not_touch_stream_auth_or_register_device() {
        let root = std::env::temp_dir().join(format!("refresh-rejected-{}", uuid::Uuid::new_v4()));
        let (mut app, _) = bitcoin_app(&root);
        let original = crate::services::connect::client::auth::AccessTokenResponse {
            access_token: "current-access".into(),
            refresh_token: "current-refresh".into(),
            expires_at: 300,
        };
        let shared = Arc::new(tokio::sync::RwLock::new(original.clone()));
        app.connect_auth = Some(shared.clone());
        app.connect_email = Some("alice@example.com".into());
        app.cache.connect_tokens = Some(shared.clone());
        let mut login = home_login();
        login.user.email = "alice@example.com".into();
        let task = app.update(Message::InAppConnectAuthPersisted {
            login,
            epoch: app.panels.connect.account.auth_epoch(),
            previous_refresh_token: Some("stale-refresh".into()),
            tokens: None, // cache CAS rejected this completion
        });
        use iced::futures::StreamExt;
        let mut stream = iced_runtime::task::into_stream(task).unwrap();
        assert!(matches!(
            stream.next().await,
            Some(iced_runtime::Action::Output(Message::View(
                view::Message::ConnectAccount(
                    view::ConnectAccountMessage::AuthCommitRejected { .. }
                )
            )))
        ));
        assert!(stream.next().await.is_none());
        assert_eq!(shared.read().await.access_token, original.access_token);
        assert_eq!(shared.read().await.refresh_token, original.refresh_token);
        assert!(app.connect_stream_config.is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn superseded_cache_refuses_auth_adoption_after_persisted_completion() {
        let root =
            std::env::temp_dir().join(format!("auth-adoption-superseded-{}", uuid::Uuid::new_v4()));
        let (mut app, _) = bitcoin_app(&root);
        app.cache.datadir_path = CoincubeDirectory::new(root.clone());
        let mut login = home_login();
        login.user.email = "alice@example.com".into();
        let network_dir = app.cache.datadir_path.network_directory(app.cache.network);
        std::fs::create_dir_all(network_dir.path()).unwrap();
        std::fs::write(
            network_dir
                .path()
                .join(crate::services::connect::client::cache::CONNECT_CACHE_FILENAME),
            serde_json::to_vec(&serde_json::json!({
                "active_email": "alice@example.com",
                "accounts": [{
                    "email": "alice@example.com",
                    "tokens": {
                        "access_token": "newer-access",
                        "refresh_token": "newer-refresh",
                        "expires_at": 1000
                    },
                    "device_id": "newer-device",
                    "reregister_claim": "newer-claim"
                }]
            }))
            .unwrap(),
        )
        .unwrap();
        let epoch = app.panels.connect.account.auth_epoch();
        let task = app.update(Message::View(view::Message::ConnectAccount(
            view::ConnectAccountMessage::AuthCommitAccepted {
                login,
                epoch,
                refreshed: false,
            },
        )));
        let mut stream = iced_runtime::task::into_stream(task).unwrap();
        assert!(matches!(
            stream.next().await,
            Some(iced_runtime::Action::Output(Message::View(
                view::Message::ConnectAccount(
                    view::ConnectAccountMessage::AuthCommitRejected { .. }
                )
            )))
        ));
        assert!(stream.next().await.is_none());
        assert_eq!(app.panels.connect.account.client.token(), None);
        let cached =
            crate::services::connect::client::cache::ConnectCache::from_file(&network_dir).unwrap();
        assert_eq!(
            cached.active_account().unwrap().device_id.as_deref(),
            Some("newer-device")
        );
        assert!(serde_json::to_string(&cached)
            .unwrap()
            .contains("newer-claim"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn changed_in_memory_refresh_refuses_disk_commit() {
        let root =
            std::env::temp_dir().join(format!("auth-refresh-in-memory-{}", uuid::Uuid::new_v4()));
        let (mut app, _) = bitcoin_app(&root);
        app.cache.datadir_path = CoincubeDirectory::new(root.clone());
        let network_dir = app.cache.datadir_path.network_directory(app.cache.network);
        std::fs::create_dir_all(network_dir.path()).unwrap();
        std::fs::write(
            network_dir
                .path()
                .join(crate::services::connect::client::cache::CONNECT_CACHE_FILENAME),
            serde_json::to_vec(&serde_json::json!({
                "active_email": "alice@example.com",
                "accounts": [{
                    "email": "alice@example.com",
                    "tokens": {
                        "access_token": "old-access",
                        "refresh_token": "old-refresh",
                        "expires_at": 1000
                    },
                    "device_id": "old-device",
                    "reregister_claim": "live-claim"
                }]
            }))
            .unwrap(),
        )
        .unwrap();
        let current = Arc::new(tokio::sync::RwLock::new(
            crate::services::connect::client::auth::AccessTokenResponse {
                access_token: "newer-access".into(),
                refresh_token: "newer-refresh".into(),
                expires_at: 2000,
            },
        ));
        app.connect_auth = Some(current.clone());
        app.connect_email = Some("alice@example.com".into());
        let mut login = home_login();
        login.user.email = "alice@example.com".into();
        let epoch = app.panels.connect.account.auth_epoch();
        let task = app.update(Message::InAppConnectAuthReady {
            login,
            epoch,
            previous_refresh_token: Some("old-refresh".into()),
        });
        let mut stream = iced_runtime::task::into_stream(task).unwrap();
        assert!(matches!(
            stream.next().await,
            Some(iced_runtime::Action::Output(
                Message::InAppConnectAuthPersisted { tokens: None, .. }
            ))
        ));
        let cached =
            crate::services::connect::client::cache::ConnectCache::from_file(&network_dir).unwrap();
        assert_eq!(
            cached.active_account().unwrap().tokens.refresh_token,
            "old-refresh"
        );
        assert_eq!(
            cached.active_account().unwrap().device_id.as_deref(),
            Some("old-device")
        );
        assert!(serde_json::to_string(&cached)
            .unwrap()
            .contains("live-claim"));
        assert_eq!(current.read().await.refresh_token, "newer-refresh");
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn keyring_restore_accepts_unchanged_older_network_cache_and_rotates_shared_auth() {
        let root =
            std::env::temp_dir().join(format!("auth-keyring-drift-{}", uuid::Uuid::new_v4()));
        let (mut app, _) = bitcoin_app(&root);
        app.cache.datadir_path = CoincubeDirectory::new(root.clone());
        let network_dir = app.cache.datadir_path.network_directory(app.cache.network);
        std::fs::create_dir_all(network_dir.path()).unwrap();
        std::fs::write(
            network_dir.path().join(crate::services::connect::client::cache::CONNECT_CACHE_FILENAME),
            serde_json::to_vec(&serde_json::json!({
                "active_email": "alice@example.com",
                "accounts": [{
                    "email": "alice@example.com",
                    "tokens": {"access_token": "cache-access", "refresh_token": "cache-refresh", "expires_at": 1000},
                    "device_id": "device-a",
                    "reregister_claim": "live-claim"
                }]
            })).unwrap(),
        ).unwrap();
        let shared = Arc::new(tokio::sync::RwLock::new(
            crate::services::connect::client::auth::AccessTokenResponse {
                access_token: "cache-access".into(),
                refresh_token: "cache-refresh".into(),
                expires_at: 1000,
            },
        ));
        app.connect_auth = Some(shared.clone());
        app.connect_email = Some("alice@example.com".into());
        drop(app.update(Message::View(view::Message::ConnectAccount(
            view::ConnectAccountMessage::RefreshSession {
                refresh_token: "keyring-refresh".into(),
            },
        ))));
        assert!(app.in_app_restore_cache_snapshot.is_some());
        let mut login = home_login();
        login.user.email = "alice@example.com".into();
        let epoch = app.panels.connect.account.auth_epoch();
        let task = app.update(Message::InAppConnectAuthReady {
            login,
            epoch,
            previous_refresh_token: Some("keyring-refresh".into()),
        });
        let mut stream = iced_runtime::task::into_stream(task).unwrap();
        assert!(matches!(
            stream.next().await,
            Some(iced_runtime::Action::Output(
                Message::InAppConnectAuthPersisted {
                    tokens: Some(_),
                    ..
                }
            ))
        ));
        let cached =
            crate::services::connect::client::cache::ConnectCache::from_file(&network_dir).unwrap();
        assert_eq!(
            cached.active_account().unwrap().tokens.refresh_token,
            "home-refresh"
        );
        assert_eq!(
            cached.active_account().unwrap().device_id.as_deref(),
            Some("device-a")
        );
        assert_eq!(
            cached.active_account().unwrap().reregister_claim.as_deref(),
            Some("live-claim")
        );
        assert_eq!(shared.read().await.refresh_token, "home-refresh");
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn restore_adopts_sibling_rotated_cache_when_this_tabs_arc_was_already_stale() {
        let root =
            std::env::temp_dir().join(format!("auth-sibling-rotation-{}", uuid::Uuid::new_v4()));
        let (mut app, _) = bitcoin_app(&root);
        app.cache.datadir_path = CoincubeDirectory::new(root.clone());
        let network_dir = app.cache.datadir_path.network_directory(app.cache.network);
        std::fs::create_dir_all(network_dir.path()).unwrap();
        std::fs::write(
            network_dir.path().join(crate::services::connect::client::cache::CONNECT_CACHE_FILENAME),
            serde_json::to_vec(&serde_json::json!({
                "active_email": "alice@example.com",
                "accounts": [{
                    "email": "alice@example.com",
                    "tokens": {"access_token": "sibling-access", "refresh_token": "sibling-refresh", "expires_at": 2000},
                    "device_id": "device-a", "reregister_claim": "live-claim"
                }]
            })).unwrap(),
        ).unwrap();
        let stale = Arc::new(tokio::sync::RwLock::new(
            crate::services::connect::client::auth::AccessTokenResponse {
                access_token: "stale-access".into(),
                refresh_token: "stale-refresh".into(),
                expires_at: 1000,
            },
        ));
        app.connect_auth = Some(stale.clone());
        app.connect_email = Some("alice@example.com".into());
        drop(app.update(Message::View(view::Message::ConnectAccount(
            view::ConnectAccountMessage::RefreshSession {
                refresh_token: "sibling-refresh".into(),
            },
        ))));
        let mut login = home_login();
        login.user.email = "alice@example.com".into();
        let epoch = app.panels.connect.account.auth_epoch();
        let task = app.update(Message::InAppConnectAuthReady {
            login,
            epoch,
            previous_refresh_token: Some("sibling-refresh".into()),
        });
        let sibling = Arc::new(tokio::sync::RwLock::new(
            crate::services::connect::client::auth::AccessTokenResponse {
                access_token: "sibling-access".into(),
                refresh_token: "sibling-refresh".into(),
                expires_at: 2000,
            },
        ));
        let network = app.cache.network;
        let datadir = app.cache.datadir_path.clone();
        drop(app.update(Message::TriggerConnectStreamReady {
            network,
            datadir,
            tokens: sibling.clone(),
            email: "alice@example.com".into(),
            cube_uuid: None,
        }));
        assert!(Arc::ptr_eq(app.connect_auth.as_ref().unwrap(), &stale));
        let mut stream = iced_runtime::task::into_stream(task).unwrap();
        let Some(iced_runtime::Action::Output(
            persisted @ Message::InAppConnectAuthPersisted {
                tokens: Some(_), ..
            },
        )) = stream.next().await
        else {
            panic!("restore was not persisted")
        };
        let mut completion = iced_runtime::task::into_stream(app.update(persisted)).unwrap();
        let mut saw_deferred = false;
        while let Some(iced_runtime::Action::Output(message)) = completion.next().await {
            if let Message::TriggerConnectStreamReady { .. } = message {
                saw_deferred = true;
                drop(app.update(message));
            }
        }
        assert!(saw_deferred);
        assert!(Arc::ptr_eq(app.connect_auth.as_ref().unwrap(), &stale));
        let cached =
            crate::services::connect::client::cache::ConnectCache::from_file(&network_dir).unwrap();
        assert_eq!(
            cached.active_account().unwrap().tokens.refresh_token,
            "home-refresh"
        );
        assert_eq!(
            cached.active_account().unwrap().device_id.as_deref(),
            Some("device-a")
        );
        assert_eq!(
            cached.active_account().unwrap().reregister_claim.as_deref(),
            Some("live-claim")
        );
        assert_eq!(stale.read().await.refresh_token, "home-refresh");
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn restore_rejects_arc_rotation_after_snapshot_without_retiring_device() {
        let root = std::env::temp_dir().join(format!("auth-arc-rotation-{}", uuid::Uuid::new_v4()));
        let (mut app, _) = bitcoin_app(&root);
        app.cache.datadir_path = CoincubeDirectory::new(root.clone());
        let network_dir = app.cache.datadir_path.network_directory(app.cache.network);
        std::fs::create_dir_all(network_dir.path()).unwrap();
        std::fs::write(
            network_dir.path().join(crate::services::connect::client::cache::CONNECT_CACHE_FILENAME),
            serde_json::to_vec(&serde_json::json!({
                "active_email": "alice@example.com",
                "accounts": [{
                    "email": "alice@example.com",
                    "tokens": {"access_token": "cache-access", "refresh_token": "cache-refresh", "expires_at": 2000},
                    "device_id": "device-a", "reregister_claim": "live-claim"
                }]
            })).unwrap(),
        ).unwrap();
        let shared = Arc::new(tokio::sync::RwLock::new(
            crate::services::connect::client::auth::AccessTokenResponse {
                access_token: "stale-access".into(),
                refresh_token: "stale-refresh".into(),
                expires_at: 1000,
            },
        ));
        app.connect_auth = Some(shared.clone());
        app.connect_email = Some("alice@example.com".into());
        drop(app.update(Message::View(view::Message::ConnectAccount(
            view::ConnectAccountMessage::RefreshSession {
                refresh_token: "cache-refresh".into(),
            },
        ))));
        shared.write().await.refresh_token = "newer-refresh".into();
        let mut login = home_login();
        login.user.email = "alice@example.com".into();
        let epoch = app.panels.connect.account.auth_epoch();
        let task = app.update(Message::InAppConnectAuthReady {
            login,
            epoch,
            previous_refresh_token: Some("cache-refresh".into()),
        });
        let valid_stream_auth = Arc::new(tokio::sync::RwLock::new(
            crate::services::connect::client::auth::AccessTokenResponse {
                access_token: "cache-access".into(),
                refresh_token: "cache-refresh".into(),
                expires_at: 2000,
            },
        ));
        let network = app.cache.network;
        let datadir = app.cache.datadir_path.clone();
        drop(app.update(Message::TriggerConnectStreamReady {
            network,
            datadir,
            tokens: valid_stream_auth.clone(),
            email: "alice@example.com".into(),
            cube_uuid: None,
        }));
        assert!(Arc::ptr_eq(app.connect_auth.as_ref().unwrap(), &shared));
        let mut stream = iced_runtime::task::into_stream(task).unwrap();
        let Some(iced_runtime::Action::Output(
            persisted @ Message::InAppConnectAuthPersisted { tokens: None, .. },
        )) = stream.next().await
        else {
            panic!("superseded restore unexpectedly persisted")
        };
        let cached =
            crate::services::connect::client::cache::ConnectCache::from_file(&network_dir).unwrap();
        assert_eq!(
            cached.active_account().unwrap().tokens.refresh_token,
            "cache-refresh"
        );
        assert_eq!(
            cached.active_account().unwrap().device_id.as_deref(),
            Some("device-a")
        );
        assert_eq!(
            cached.active_account().unwrap().reregister_claim.as_deref(),
            Some("live-claim")
        );
        assert_eq!(shared.read().await.refresh_token, "newer-refresh");
        let mut completion = iced_runtime::task::into_stream(app.update(persisted)).unwrap();
        let mut saw_deferred = false;
        while let Some(iced_runtime::Action::Output(message)) = completion.next().await {
            if let Message::TriggerConnectStreamReady { .. } = message {
                saw_deferred = true;
                drop(app.update(message));
            }
        }
        assert!(saw_deferred);
        assert!(Arc::ptr_eq(
            app.connect_auth.as_ref().unwrap(),
            &valid_stream_auth
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn accepted_restore_on_new_network_queues_device_and_stream_bootstrap() {
        let root =
            std::env::temp_dir().join(format!("restore-new-network-{}", uuid::Uuid::new_v4()));
        let (mut app, _) = bitcoin_app(&root);
        app.cache.datadir_path = CoincubeDirectory::new(root.clone());
        let network_dir = app.cache.datadir_path.network_directory(app.cache.network);
        std::fs::create_dir_all(network_dir.path()).unwrap();
        let mut login = home_login();
        login.user.email = "alice@example.com".into();
        std::fs::write(
            network_dir.path().join(crate::services::connect::client::cache::CONNECT_CACHE_FILENAME),
            serde_json::to_vec(&serde_json::json!({
                "active_email": "alice@example.com",
                "accounts": [{"email": "alice@example.com", "tokens": {
                    "access_token": login.token.clone(), "refresh_token": login.refresh_token.clone(),
                    "expires_at": 1000
                }}]
            })).unwrap(),
        ).unwrap();
        assert!(app.connect_auth.is_none());
        let epoch = app.panels.connect.account.auth_epoch();
        drop(app.panels.connect.account.update_message(
            view::ConnectAccountMessage::RefreshedSession(
                login.clone(),
                epoch,
                "old-keyring-refresh".into(),
            ),
        ));
        let task = app.update(Message::View(view::Message::ConnectAccount(
            view::ConnectAccountMessage::AuthCommitAccepted {
                login: login.clone(),
                epoch,
                refreshed: true,
            },
        )));
        assert_eq!(app.connect_email.as_deref(), Some("alice@example.com"));
        assert!(app.connect_auth.is_some());
        let mut outputs = iced_runtime::task::into_stream(task).unwrap();
        let mut saw_bootstrap = false;
        for _ in 0..12 {
            let Ok(Some(action)) =
                tokio::time::timeout(std::time::Duration::from_secs(2), outputs.next()).await
            else {
                break;
            };
            if let iced_runtime::Action::Output(Message::InAppConnectLoginCompleted {
                token,
                refresh_token,
                email,
                epoch: completion_epoch,
            }) = action
            {
                assert_eq!(token, login.token);
                assert_eq!(refresh_token, login.refresh_token);
                assert_eq!(email, "alice@example.com");
                assert_eq!(completion_epoch, epoch);
                saw_bootstrap = true;
                break;
            }
        }
        assert!(saw_bootstrap);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn registration_uses_matching_shared_auth_through_refresh_rotation() {
        let root =
            std::env::temp_dir().join(format!("registration-shared-auth-{}", uuid::Uuid::new_v4()));
        let datadir = CoincubeDirectory::new(root.clone());
        let network_dir = datadir.network_directory(bitcoin::Network::Bitcoin);
        std::fs::create_dir_all(network_dir.path()).unwrap();
        let cached = crate::services::connect::client::auth::AccessTokenResponse {
            access_token: "old-access".into(),
            refresh_token: "old-refresh".into(),
            expires_at: 1000,
        };
        let shared = Arc::new(tokio::sync::RwLock::new(cached.clone()));
        let selected = connect_registration_auth(
            Some(shared.clone()),
            cached,
            &network_dir,
            "alice@example.com",
        )
        .await;
        assert!(Arc::ptr_eq(&selected, &shared));
        let rotated = crate::services::connect::client::auth::AccessTokenResponse {
            access_token: "new-access".into(),
            refresh_token: "new-refresh".into(),
            expires_at: 2000,
        };
        *shared.write().await = rotated.clone();
        std::fs::write(
            network_dir
                .path()
                .join(crate::services::connect::client::cache::CONNECT_CACHE_FILENAME),
            serde_json::to_vec(&serde_json::json!({
                "active_email": "alice@example.com",
                "accounts": [{"email": "alice@example.com", "tokens": rotated}]
            }))
            .unwrap(),
        )
        .unwrap();
        let current = selected.read().await;
        assert!(
            crate::services::connect::client::cache::set_device_id_for_email_if_token(
                &network_dir,
                "alice@example.com",
                "registered-device",
                &current.refresh_token
            )
            .await
            .unwrap()
        );
        drop(current);
        assert_eq!(
            crate::services::connect::client::cache::ConnectCache::from_file(&network_dir)
                .unwrap()
                .active_account()
                .unwrap()
                .device_id
                .as_deref(),
            Some("registered-device")
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn late_registration_bootstrap_cannot_replace_newer_stream_auth() {
        let root =
            std::env::temp_dir().join(format!("late-registration-auth-{}", uuid::Uuid::new_v4()));
        let (mut app, _) = bitcoin_app(&root);
        app.cache.datadir_path = CoincubeDirectory::new(root.clone());
        let network_dir = app.cache.datadir_path.network_directory(app.cache.network);
        std::fs::create_dir_all(network_dir.path()).unwrap();
        std::fs::write(
            network_dir
                .path()
                .join(crate::services::connect::client::cache::CONNECT_CACHE_FILENAME),
            serde_json::to_vec(&serde_json::json!({
                "active_email": "bob@example.com",
                "accounts": [{"email": "bob@example.com", "tokens": {
                    "access_token": "new-access", "refresh_token": "new-refresh", "expires_at": 2000
                }}]
            }))
            .unwrap(),
        )
        .unwrap();
        let newer = Arc::new(tokio::sync::RwLock::new(
            crate::services::connect::client::auth::AccessTokenResponse {
                access_token: "new-access".into(),
                refresh_token: "new-refresh".into(),
                expires_at: 2000,
            },
        ));
        app.connect_auth = Some(newer.clone());
        app.connect_email = Some("bob@example.com".into());
        let stale = Arc::new(tokio::sync::RwLock::new(
            crate::services::connect::client::auth::AccessTokenResponse {
                access_token: "old-access".into(),
                refresh_token: "old-refresh".into(),
                expires_at: 1000,
            },
        ));
        let network = app.cache.network;
        let datadir = app.cache.datadir_path.clone();
        let task = app.update(Message::TriggerConnectStreamReady {
            network,
            datadir,
            tokens: stale,
            email: "alice@example.com".into(),
            cube_uuid: None,
        });
        assert!(iced_runtime::task::into_stream(task).is_none());
        assert!(Arc::ptr_eq(app.connect_auth.as_ref().unwrap(), &newer));
        assert_eq!(app.connect_email.as_deref(), Some("bob@example.com"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn registration_bootstrap_waits_for_same_session_refresh_write_lock() {
        let root =
            std::env::temp_dir().join(format!("registration-lock-wait-{}", uuid::Uuid::new_v4()));
        let (mut app, _) = bitcoin_app(&root);
        app.cache.datadir_path = CoincubeDirectory::new(root.clone());
        let network_dir = app.cache.datadir_path.network_directory(app.cache.network);
        std::fs::create_dir_all(network_dir.path()).unwrap();
        std::fs::write(
            network_dir.path().join(crate::services::connect::client::cache::CONNECT_CACHE_FILENAME),
            serde_json::to_vec(&serde_json::json!({
                "active_email": "alice@example.com",
                "accounts": [{"email": "alice@example.com", "tokens": {
                    "access_token": "current-access", "refresh_token": "current-refresh", "expires_at": 2000
                }, "device_id": "device-a"}]
            })).unwrap(),
        ).unwrap();
        let shared = Arc::new(tokio::sync::RwLock::new(
            crate::services::connect::client::auth::AccessTokenResponse {
                access_token: "current-access".into(),
                refresh_token: "current-refresh".into(),
                expires_at: 2000,
            },
        ));
        app.connect_auth = Some(shared.clone());
        app.connect_email = Some("alice@example.com".into());
        let held = shared.write().await;
        let network = app.cache.network;
        let datadir = app.cache.datadir_path.clone();
        let task = app.update(Message::TriggerConnectStreamReady {
            network,
            datadir,
            tokens: shared.clone(),
            email: "alice@example.com".into(),
            cube_uuid: None,
        });
        let mut outputs = iced_runtime::task::into_stream(task).unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), outputs.next())
                .await
                .is_err()
        );
        drop(held);
        let output = tokio::time::timeout(std::time::Duration::from_secs(2), outputs.next())
            .await
            .unwrap()
            .unwrap();
        let iced_runtime::Action::Output(message @ Message::TriggerConnectStreamReady { .. }) =
            output
        else {
            panic!("refresh unlock did not retry stream bootstrap");
        };
        drop(app.update(message));
        assert!(Arc::ptr_eq(app.connect_auth.as_ref().unwrap(), &shared));
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn stale_async_stream_completion_cannot_install_over_new_login() {
        let root =
            std::env::temp_dir().join(format!("stale-stream-completion-{}", uuid::Uuid::new_v4()));
        let (mut app, _) = bitcoin_app(&root);
        app.cache.datadir_path = CoincubeDirectory::new(root.clone());
        let network_dir = app.cache.datadir_path.network_directory(app.cache.network);
        std::fs::create_dir_all(network_dir.path()).unwrap();
        std::fs::write(
            network_dir
                .path()
                .join(crate::services::connect::client::cache::CONNECT_CACHE_FILENAME),
            serde_json::to_vec(&serde_json::json!({
                "active_email": "bob@example.com",
                "accounts": [{"email": "bob@example.com", "tokens": {
                    "access_token": "bob-access", "refresh_token": "bob-refresh", "expires_at": 2000
                }, "device_id": "bob-device"}]
            }))
            .unwrap(),
        )
        .unwrap();
        let bob = Arc::new(tokio::sync::RwLock::new(
            crate::services::connect::client::auth::AccessTokenResponse {
                access_token: "bob-access".into(),
                refresh_token: "bob-refresh".into(),
                expires_at: 2000,
            },
        ));
        app.connect_auth = Some(bob.clone());
        app.connect_email = Some("bob@example.com".into());
        let stale = Arc::new(tokio::sync::RwLock::new(
            crate::services::connect::client::auth::AccessTokenResponse {
                access_token: "alice-access".into(),
                refresh_token: "alice-refresh".into(),
                expires_at: 1000,
            },
        ));
        let old_cfg = crate::services::connect::grpc::stream::ConnectStreamConfig {
            grpc_url: "https://grpc.example.invalid".into(),
            tokens: stale,
            device_id: "alice-device".into(),
            user_agent: "test".into(),
            vault_ids: vec![],
            last_seen_seq: 0,
        };
        drop(app.update(Message::ConnectStreamReady(Some((
            old_cfg,
            "alice@example.com".into(),
        )))));
        assert!(app.connect_stream_config.is_none());
        assert!(Arc::ptr_eq(app.connect_auth.as_ref().unwrap(), &bob));
        drop(app.update(Message::ConnectStreamReady(None)));
        assert_eq!(app.connect_email.as_deref(), Some("bob@example.com"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn async_stream_completion_uses_device_id_committed_during_vault_lookup() {
        let root =
            std::env::temp_dir().join(format!("stream-device-reregister-{}", uuid::Uuid::new_v4()));
        let (mut app, _) = bitcoin_app(&root);
        app.cache.datadir_path = CoincubeDirectory::new(root.clone());
        let network_dir = app.cache.datadir_path.network_directory(app.cache.network);
        std::fs::create_dir_all(network_dir.path()).unwrap();
        std::fs::write(
            network_dir.path().join(crate::services::connect::client::cache::CONNECT_CACHE_FILENAME),
            serde_json::to_vec(&serde_json::json!({
                "active_email": "alice@example.com",
                "accounts": [{"email": "alice@example.com", "tokens": {
                    "access_token": "alice-access", "refresh_token": "alice-refresh", "expires_at": 2000
                }, "device_id": "new-device", "last_seen_event_seq": 12}]
            })).unwrap(),
        ).unwrap();
        let shared = Arc::new(tokio::sync::RwLock::new(
            crate::services::connect::client::auth::AccessTokenResponse {
                access_token: "alice-access".into(),
                refresh_token: "alice-refresh".into(),
                expires_at: 2000,
            },
        ));
        app.connect_auth = Some(shared.clone());
        app.connect_email = Some("alice@example.com".into());
        let stale_cfg = crate::services::connect::grpc::stream::ConnectStreamConfig {
            grpc_url: "https://grpc.example.invalid".into(),
            tokens: shared,
            device_id: "old-device".into(),
            user_agent: "test".into(),
            vault_ids: vec![],
            last_seen_seq: 4,
        };
        drop(app.update(Message::ConnectStreamReady(Some((
            stale_cfg,
            "alice@example.com".into(),
        )))));
        let installed = app.connect_stream_config.as_ref().unwrap();
        assert_eq!(installed.device_id, "new-device");
        assert_eq!(installed.last_seen_seq, 12);
        assert_eq!(app.cache.connect_device_id.as_deref(), Some("new-device"));
        let _ = std::fs::remove_dir_all(root);
    }

    pub(crate) fn signed_in_connect_for_shell(app: &mut App) {
        app.panels.connect.account.user = Some(crate::services::coincube::User {
            id: 7,
            email: "shell@example.invalid".into(),
            email_verified: Some(true),
        });
        app.panels.connect.account.step = state::connect::ConnectFlowStep::Dashboard;
        app.cache.connect_authenticated = true;
    }

    pub(crate) fn bitcoin_app_with_id(root: &std::path::Path, id: &str) -> App {
        let (mut app, _) = bitcoin_app(root);
        app.cube_settings.id = id.to_owned();
        app.cache.cube_id = id.to_owned();
        app
    }

    pub(crate) fn fixture_daemon(app: &App) -> Arc<dyn Daemon + Sync + Send> {
        app.daemon.as_ref().unwrap().clone()
    }

    #[test]
    fn older_cube_reuses_sibling_session_and_verifies_before_showing_signed_in() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                use httpmock::prelude::*;
                use iced::futures::StreamExt;
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let _enter = rt.enter();
                let root =
                    std::env::temp_dir().join(format!("cube-session-{}", uuid::Uuid::new_v4()));
                let (first, _) = bitcoin_app(&root);
                let (second, _) = bitcoin_app(&root);
                let mut gui = crate::gui::GUI::reviewer_claim_gui(first, second);
                rt.block_on(async {
                    let server = MockServer::start_async().await;
                    let user = server
                        .mock_async(|when, then| {
                            when.method(GET)
                                .path("/api/v1/user")
                                .header("authorization", "Bearer sibling-session");
                            then.status(200).json_body(serde_json::json!({
                                "id":7,"email":"fixture@example.invalid","email_verified":true
                            }));
                        })
                        .await;
                    server
                        .mock_async(|when, then| {
                            when.method(GET).path("/api/v1/connect/features");
                            then.status(200).json_body(serde_json::json!({
                                "success":true,"data":{"plans":[]}
                            }));
                        })
                        .await;
                    server
                        .mock_async(|when, then| {
                            when.method(GET).path("/api/v1/connect/duress");
                            then.status(200).json_body(serde_json::json!({
                                "success":true,"data":{"active":false,"enrolled":false,
                                "thisDeviceRegistered":false}
                            }));
                        })
                        .await;
                    let mut client =
                        crate::services::coincube::CoincubeClient::for_test(server.base_url());
                    client.set_token("sibling-session");
                    let sibling = gui.reviewer_app_mut(0);
                    sibling.panels.connect.account.install_shared_client(client);
                    sibling.panels.connect.account.user = Some(crate::services::coincube::User {
                        id: 7,
                        email: "fixture@example.invalid".into(),
                        email_verified: Some(true),
                    });
                    sibling.panels.connect.account.step =
                        state::connect::account::ConnectFlowStep::Dashboard;
                    sibling.cache.connect_authenticated = true;
                    let task = gui.reviewer_account(2, view::ConnectAccountMessage::Init);
                    let older = gui.reviewer_app_mut(1);
                    assert!(
                        !older.cache.connect_authenticated,
                        "a bearer alone is not verified sign-in"
                    );
                    assert_eq!(
                        older.authenticated_coincube_client().unwrap().token(),
                        Some("sibling-session")
                    );
                    let mut pending = vec![task];
                    while let Some(task) = pending.pop() {
                        if let Some(mut stream) = iced_runtime::task::into_stream(task) {
                            while let Some(action) = stream.next().await {
                                if let iced_runtime::Action::Output(message) = action {
                                    if crate::gui::GUI::reviewer_is_account_message(&message) {
                                        pending.push(gui.update(message));
                                    }
                                }
                            }
                        }
                    }
                    user.assert_async().await;
                    let older = gui.reviewer_app_mut(1);
                    assert!(older.cache.connect_authenticated);
                    assert_eq!(older.panels.connect.account.user.as_ref().unwrap().id, 7);
                    older.invalidate_claim_session();
                    assert!(older.shareable_connect_client().is_none());
                    assert!(
                        !older.can_adopt_shared_connect_session(),
                        "logout/claim holds cannot reuse a sibling bearer"
                    );
                    drop(gui.reviewer_account(2, view::ConnectAccountMessage::PlanLoaded(None, 0)));
                    assert!(
                        gui.reviewer_app_mut(1).cache.sidebar_connect_signed_in,
                        "another authenticated tab still makes the app signed in"
                    );
                    gui.reviewer_app_mut(0).invalidate_claim_session();
                    drop(gui.reviewer_account(2, view::ConnectAccountMessage::PlanLoaded(None, 0)));
                    assert!(
                        !gui.reviewer_app_mut(1).cache.sidebar_connect_signed_in,
                        "invalidated retained clients must not keep the global indicator signed in"
                    );
                });
                drop(gui);
                let _ = std::fs::remove_dir_all(root);
            })
            .unwrap()
            .join()
            .unwrap();
    }

    /// Mock Connect API for a completed Home sign-in. Every bearer is accepted
    /// by `/user`; `refresh_status` drives the keyring-refresh outcome.
    async fn signin_server(refresh_status: u16) -> httpmock::MockServer {
        use httpmock::prelude::*;
        let server = MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method(POST).path("/api/v1/auth/token/refresh");
                then.status(refresh_status).json_body(serde_json::json!({
                    "requires_2fa":false,"token":"refreshed-session",
                    "refresh_token":"refreshed-refresh",
                    "user":{"id":7,"email":"fixture@example.invalid","email_verified":true}
                }));
            })
            .await;
        server
            .mock_async(|when, then| {
                when.method(GET).path("/api/v1/user");
                then.status(200).json_body(serde_json::json!({
                    "id":7,"email":"fixture@example.invalid","email_verified":true
                }));
            })
            .await;
        server
            .mock_async(|when, then| {
                when.method(GET).path("/api/v1/connect/features");
                then.status(200)
                    .json_body(serde_json::json!({"success":true,"data":{"plans":[]}}));
            })
            .await;
        server
            .mock_async(|when, then| {
                when.method(GET).path("/api/v1/connect/duress");
                then.status(200).json_body(serde_json::json!({
                    "success":true,"data":{"active":false,"enrolled":false,
                    "thisDeviceRegistered":false}
                }));
            })
            .await;
        server
    }

    /// Run every sign-in propagation message through the real GUI until the
    /// tabs settle. Returns the number of GUI updates it took.
    async fn settle_signin(gui: &mut crate::gui::GUI, task: Task<crate::gui::Message>) -> usize {
        use iced::futures::StreamExt;
        let mut pending = vec![task];
        let mut updates = 0;
        while let Some(task) = pending.pop() {
            if let Some(mut stream) = iced_runtime::task::into_stream(task) {
                while let Some(action) = stream.next().await {
                    if let iced_runtime::Action::Output(message) = action {
                        if crate::gui::GUI::reviewer_is_session_propagation(&message) {
                            updates += 1;
                            assert!(updates < 200, "sign-in propagation did not settle");
                            pending.push(gui.update(message));
                        }
                    }
                }
            }
        }
        updates
    }

    fn home_login() -> crate::services::coincube::LoginResponse {
        serde_json::from_value(serde_json::json!({
            "requires_2fa":false,"token":"home-session","refresh_token":"home-refresh",
            "user":{"id":7,"email":"fixture@example.invalid","email_verified":true}
        }))
        .unwrap()
    }

    fn seed_home_login_cache(root: &std::path::Path) {
        let dir =
            CoincubeDirectory::new(root.to_path_buf()).network_directory(bitcoin::Network::Bitcoin);
        std::fs::create_dir_all(dir.path()).unwrap();
        std::fs::write(
            dir.path()
                .join(crate::services::connect::client::cache::CONNECT_CACHE_FILENAME),
            serde_json::to_vec(&serde_json::json!({
                "active_email": "fixture@example.invalid",
                "accounts": [{
                    "email": "fixture@example.invalid",
                    "tokens": {
                        "access_token": "home-session",
                        "refresh_token": "home-refresh",
                        "expires_at": chrono::Utc::now().timestamp() + 3600
                    }
                }]
            }))
            .unwrap(),
        )
        .unwrap();
    }

    /// L11 (2026-10-21 walkthrough): a Bitcoin Cube already open and signed
    /// out, then a sign-in completed in a Home tab. The Cube must re-evaluate
    /// its account-backed settings instead of staying on "Sign in to Connect"
    /// while the footer, which summarises every tab, says "Signed in".
    #[test]
    fn open_signed_out_cube_signs_in_after_home_login() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let _enter = rt.enter();
                let root =
                    std::env::temp_dir().join(format!("cube-home-signin-{}", uuid::Uuid::new_v4()));
                rt.block_on(async {
                    let server = signin_server(200).await;
                    let (mut cube, _) = bitcoin_app(&root);
                    cube.cache.datadir_path = CoincubeDirectory::new(root.clone());
                    cube.panels.connect.account.client =
                        crate::services::coincube::CoincubeClient::for_test(server.base_url());
                    let (mut home, _) = crate::home::Home::new_for_chain(
                        CoincubeDirectory::new(root.clone()),
                        Some(crate::chain::ChainId::Bitcoin),
                    );
                    home.connect_account.client =
                        crate::services::coincube::CoincubeClient::for_test(server.base_url());
                    let mut gui = crate::gui::GUI::reviewer_app_home_gui(cube, home);
                    // The Cube opened with nothing saved: it settles on Login.
                    let task = gui.reviewer_account(1, view::ConnectAccountMessage::Init);
                    settle_signin(&mut gui, task).await;
                    assert!(!gui
                        .reviewer_app_mut(0)
                        .panels
                        .connect
                        .account
                        .is_authenticated());
                    // Home's OTP verification completes.
                    let epoch = gui.reviewer_home_mut(1).connect_account.auth_epoch();
                    seed_home_login_cache(&root);
                    let task = gui.reviewer_home_account(
                        2,
                        view::ConnectAccountMessage::SetSession(home_login(), epoch),
                    );
                    let updates = settle_signin(&mut gui, task).await;
                    assert!(updates <= 40, "sign-in ping-pong: {} updates", updates);
                    assert!(gui.reviewer_home_mut(1).connect_account.is_authenticated());
                    let cube = gui.reviewer_app_mut(0);
                    assert!(cube.cache.sidebar_connect_signed_in);
                    assert!(
                        cube.panels.connect.account.is_authenticated(),
                        "the open Cube must re-evaluate its Connect session after Home signs in"
                    );
                    assert!(cube.cache.connect_authenticated);
                });
                let _ = std::fs::remove_dir_all(root);
            })
            .unwrap()
            .join()
            .unwrap();
    }

    /// A transient refresh failure in the open Cube keeps the stored session,
    /// waiting to be checked again, instead of a silent "Sign in" prompt or a
    /// loop, and the Cube's own retry signs it in once the backend recovers.
    #[test]
    fn open_cube_shows_retryable_error_when_its_refresh_fails() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let _enter = rt.enter();
                let root = std::env::temp_dir()
                    .join(format!("cube-refresh-fail-{}", uuid::Uuid::new_v4()));
                rt.block_on(async {
                    let failing = signin_server(503).await;
                    let (mut cube, _) = bitcoin_app(&root);
                    cube.cache.datadir_path = CoincubeDirectory::new(root.clone());
                    cube.panels.connect.account.client =
                        crate::services::coincube::CoincubeClient::for_test(failing.base_url());
                    let (mut home, _) = crate::home::Home::new_for_chain(
                        CoincubeDirectory::new(root.clone()),
                        Some(crate::chain::ChainId::Bitcoin),
                    );
                    home.connect_account.client =
                        crate::services::coincube::CoincubeClient::for_test(failing.base_url());
                    let mut gui = crate::gui::GUI::reviewer_app_home_gui(cube, home);
                    let task = gui.reviewer_account(1, view::ConnectAccountMessage::Init);
                    settle_signin(&mut gui, task).await;
                    let epoch = gui.reviewer_home_mut(1).connect_account.auth_epoch();
                    seed_home_login_cache(&root);
                    let task = gui.reviewer_home_account(
                        2,
                        view::ConnectAccountMessage::SetSession(home_login(), epoch),
                    );
                    settle_signin(&mut gui, task).await;
                    assert!(gui.reviewer_home_mut(1).connect_account.is_authenticated());
                    let cube = gui.reviewer_app_mut(0);
                    assert!(!cube.panels.connect.account.is_authenticated());
                    assert!(!cube.panels.connect.account.is_checking_session());
                    assert!(
                        cube.panels.connect.account.unverified.is_some(),
                        "a failed check keeps the session waiting, not signed out"
                    );
                    // The backend recovers; the panel's "Try again" re-runs Init.
                    let healthy = signin_server(200).await;
                    gui.reviewer_app_mut(0).panels.connect.account.client =
                        crate::services::coincube::CoincubeClient::for_test(healthy.base_url());
                    let task = gui.reviewer_account(1, view::ConnectAccountMessage::Init);
                    settle_signin(&mut gui, task).await;
                    let cube = gui.reviewer_app_mut(0);
                    assert!(cube.panels.connect.account.is_authenticated());
                    assert!(cube.cache.connect_authenticated);
                    assert!(gui.reviewer_home_mut(1).connect_account.is_authenticated());
                });
                let _ = std::fs::remove_dir_all(root);
            })
            .unwrap()
            .join()
            .unwrap();
    }

    /// The same sequence with the Cube in another split pane: the sign-in
    /// rebroadcast must reach every pane, not only the one holding Home.
    #[test]
    fn signed_out_cube_in_another_pane_signs_in_after_home_login() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let _enter = rt.enter();
                let root =
                    std::env::temp_dir().join(format!("cube-pane-signin-{}", uuid::Uuid::new_v4()));
                rt.block_on(async {
                    let server = signin_server(200).await;
                    let (mut cube, _) = bitcoin_app(&root);
                    cube.cache.datadir_path = CoincubeDirectory::new(root.clone());
                    cube.panels.connect.account.client =
                        crate::services::coincube::CoincubeClient::for_test(server.base_url());
                    let (mut home, _) = crate::home::Home::new_for_chain(
                        CoincubeDirectory::new(root.clone()),
                        Some(crate::chain::ChainId::Bitcoin),
                    );
                    home.connect_account.client =
                        crate::services::coincube::CoincubeClient::for_test(server.base_url());
                    let (mut gui, cube_pane) =
                        crate::gui::GUI::reviewer_split_app_home_gui(cube, home);
                    let task =
                        gui.reviewer_account_in(cube_pane, 1, view::ConnectAccountMessage::Init);
                    settle_signin(&mut gui, task).await;
                    assert!(!gui
                        .reviewer_app_in(cube_pane)
                        .panels
                        .connect
                        .account
                        .is_authenticated());
                    let epoch = gui.reviewer_home_mut(0).connect_account.auth_epoch();
                    seed_home_login_cache(&root);
                    let task = gui.reviewer_home_account(
                        2,
                        view::ConnectAccountMessage::SetSession(home_login(), epoch),
                    );
                    settle_signin(&mut gui, task).await;
                    assert!(gui.reviewer_home_mut(0).connect_account.is_authenticated());
                    let cube = gui.reviewer_app_in(cube_pane);
                    assert!(
                        cube.panels.connect.account.is_authenticated(),
                        "a Cube in another pane must re-evaluate its session too: {:?} {:?}",
                        cube.panels.connect.account.step,
                        cube.panels.connect.account.error,
                    );
                    assert!(cube.cache.connect_authenticated);
                });
                let _ = std::fs::remove_dir_all(root);
            })
            .unwrap()
            .join()
            .unwrap();
    }

    /// The App acts on a completed full scan: the refresh that first reports it
    /// rewrites the settings file without the restored Vault's rescan marker, and a
    /// refresh while the scan is still running leaves the marker alone.
    #[test]
    fn a_completed_full_scan_retires_the_restored_vaults_rescan_marker() {
        use crate::app::settings::{PendingRescan, WalletSettings};
        use coincubed::commands::HistorySync;

        let root = std::env::temp_dir().join(format!("retire-marker-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let (mut app, wallet) = bitcoin_app(&root);
        // The fixture's cache has an empty data directory, which resolves relative
        // to the crate under `cargo test`: point it at the temp root so neither the
        // test nor the task it drives writes into the source tree.
        app.cache.datadir_path = CoincubeDirectory::new(root.clone());
        assert_eq!(
            app.daemon_backend(),
            DaemonBackend::EmbeddedCoincubed(Some(NodeType::Esplora))
        );
        let network_dir = app.cache.datadir_path.network_directory(app.cache.chain());
        assert!(network_dir.path().starts_with(&root));
        std::fs::create_dir_all(network_dir.path()).unwrap();
        let marked = settings::Settings {
            wallets: vec![WalletSettings {
                name: format!("Coincube-{}", wallet.descriptor_checksum),
                alias: None,
                descriptor_checksum: wallet.descriptor_checksum.clone(),
                pinned_at: None,
                keys: Vec::new(),
                hardware_wallets: Vec::new(),
                remote_backend_auth: None,
                start_internal_bitcoind: None,
                pending_rescan: Some(PendingRescan::DateUnknown),
                keychain_keys_recorded: false,
            }],
            ..Default::default()
        };
        std::fs::write(
            network_dir.path().join(settings::SETTINGS_FILE_NAME),
            serde_json::to_vec(&marked).unwrap(),
        )
        .unwrap();
        let marker = || {
            let read: settings::Settings = serde_json::from_slice(
                &std::fs::read(network_dir.path().join(settings::SETTINGS_FILE_NAME)).unwrap(),
            )
            .unwrap();
            read.wallets[0].pending_rescan
        };
        let refresh = |history_sync: HistorySync| {
            Message::UpdateDaemonCache(Ok(cache::DaemonCache {
                blockheight: 970_247,
                history_sync,
                ..Default::default()
            }))
        };

        settle(&mut app, |app| {
            app.update(refresh(HistorySync {
                full_scan_in_progress: true,
                ..Default::default()
            }))
        });
        assert_eq!(marker(), Some(PendingRescan::DateUnknown));

        settle(&mut app, |app| {
            app.update(refresh(HistorySync {
                full_scan_completed_at: Some(1_791_329_974),
                ..Default::default()
            }))
        });
        assert_eq!(marker(), None, "the completed scan retires the marker");
        std::fs::remove_dir_all(&root).ok();
    }

    /// A full scan can complete before the app's first daemon snapshot: the poller
    /// starts before the loader takes it. Completion already present at startup still
    /// retires the marker on the first refresh, and only once per session.
    #[test]
    fn a_full_scan_completed_before_startup_still_retires_the_marker_once() {
        use crate::app::settings::{PendingRescan, WalletSettings};
        use coincubed::commands::HistorySync;

        let root = std::env::temp_dir().join(format!("retire-early-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let (mut app, wallet) = bitcoin_app(&root);
        app.cache.datadir_path = CoincubeDirectory::new(root.clone());
        let completed = HistorySync {
            full_scan_completed_at: Some(1_791_329_974),
            ..Default::default()
        };
        // As seeded by the loader's first snapshot.
        app.cache.daemon_cache.history_sync = completed.clone();
        let network_dir = app.cache.datadir_path.network_directory(app.cache.chain());
        assert!(network_dir.path().starts_with(&root));
        std::fs::create_dir_all(network_dir.path()).unwrap();
        let settings_file = network_dir.path().join(settings::SETTINGS_FILE_NAME);
        let write_marker = || {
            let marked = settings::Settings {
                wallets: vec![WalletSettings {
                    name: format!("Coincube-{}", wallet.descriptor_checksum),
                    alias: None,
                    descriptor_checksum: wallet.descriptor_checksum.clone(),
                    pinned_at: None,
                    keys: Vec::new(),
                    hardware_wallets: Vec::new(),
                    remote_backend_auth: None,
                    start_internal_bitcoind: None,
                    pending_rescan: Some(PendingRescan::DateUnknown),
                    keychain_keys_recorded: false,
                }],
                ..Default::default()
            };
            std::fs::write(&settings_file, serde_json::to_vec(&marked).unwrap()).unwrap();
        };
        let marker = || {
            let read: settings::Settings =
                serde_json::from_slice(&std::fs::read(&settings_file).unwrap()).unwrap();
            read.wallets[0].pending_rescan
        };
        let refresh = |history_sync: HistorySync| {
            Message::UpdateDaemonCache(Ok(cache::DaemonCache {
                blockheight: 970_247,
                history_sync,
                ..Default::default()
            }))
        };

        write_marker();
        settle(&mut app, |app| app.update(refresh(completed.clone())));
        assert_eq!(marker(), None, "completion seen at startup retires it");

        // Once per session: a later refresh does not rewrite the file again.
        write_marker();
        settle(&mut app, |app| app.update(refresh(completed)));
        assert_eq!(marker(), Some(PendingRescan::DateUnknown));
        std::fs::remove_dir_all(&root).ok();
    }

    /// Run the task `update` returns to completion, and hand the App back the
    /// rescan-marker write's outcome, as the runtime would. Only for tasks that
    /// make no network call.
    fn settle(app: &mut App, update: impl FnOnce(&mut App) -> Task<Message>) {
        use iced::futures::StreamExt;
        let task = update(app);
        let Some(stream) = iced_runtime::task::into_stream(task) else {
            return;
        };
        let outputs: Vec<Message> = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(
                stream
                    .filter_map(|action| async move {
                        match action {
                            iced_runtime::Action::Output(message) => Some(message),
                            _ => None,
                        }
                    })
                    .collect(),
            );
        for message in outputs {
            if matches!(message, Message::RescanObligationRetired(_)) {
                let _ = app.update(message);
            }
        }
    }

    /// A settings write that fails leaves the obligation unretired, and the next
    /// refresh showing the completed scan writes it again; while a write is in
    /// flight, a refresh does not start another.
    #[test]
    fn a_failed_marker_write_is_retried_on_a_later_refresh() {
        use crate::app::settings::{PendingRescan, WalletSettings};
        use coincubed::commands::HistorySync;

        let root = std::env::temp_dir().join(format!("retire-retry-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let (mut app, wallet) = bitcoin_app(&root);
        app.cache.datadir_path = CoincubeDirectory::new(root.clone());
        let network_dir = app.cache.datadir_path.network_directory(app.cache.chain());
        assert!(network_dir.path().starts_with(&root));
        std::fs::create_dir_all(network_dir.path()).unwrap();
        let settings_file = network_dir.path().join(settings::SETTINGS_FILE_NAME);
        let completed = HistorySync {
            full_scan_completed_at: Some(1_791_329_974),
            ..Default::default()
        };
        let refresh = |history_sync: HistorySync| {
            Message::UpdateDaemonCache(Ok(cache::DaemonCache {
                blockheight: 970_247,
                history_sync,
                ..Default::default()
            }))
        };

        // A settings file that cannot be parsed: the write is refused.
        std::fs::write(&settings_file, b"{ not json").unwrap();
        settle(&mut app, |app| app.update(refresh(completed.clone())));
        assert!(!app.rescan_obligation_retired_by_scan, "the write failed");
        assert!(!app.rescan_obligation_retire_in_flight);

        // A refresh while a write is pending starts no second one.
        app.rescan_obligation_retire_in_flight = true;
        let task = app.update(refresh(completed.clone()));
        drop(task);
        assert!(app.rescan_obligation_retire_in_flight);
        app.rescan_obligation_retire_in_flight = false;

        // With the file readable again, the next refresh retires the marker.
        let marked = settings::Settings {
            wallets: vec![WalletSettings {
                name: format!("Coincube-{}", wallet.descriptor_checksum),
                alias: None,
                descriptor_checksum: wallet.descriptor_checksum.clone(),
                pinned_at: None,
                keys: Vec::new(),
                hardware_wallets: Vec::new(),
                remote_backend_auth: None,
                start_internal_bitcoind: None,
                pending_rescan: Some(PendingRescan::DateUnknown),
                keychain_keys_recorded: false,
            }],
            ..Default::default()
        };
        std::fs::write(&settings_file, serde_json::to_vec(&marked).unwrap()).unwrap();
        settle(&mut app, |app| app.update(refresh(completed)));
        let read: settings::Settings =
            serde_json::from_slice(&std::fs::read(&settings_file).unwrap()).unwrap();
        assert_eq!(read.wallets[0].pending_rescan, None);
        assert!(app.rescan_obligation_retired_by_scan);
        std::fs::remove_dir_all(&root).ok();
    }

    /// Put a claim target for `wallet` on the fork chain's settings file,
    /// as another App instance would.
    fn write_claim_target(root: &CoincubeDirectory, wallet: &Wallet) {
        let fork_dir = root.network_directory(crate::chain::ChainId::BitcoinBlake2b);
        std::fs::create_dir_all(fork_dir.path()).unwrap();
        let mut target = settings::CubeSettings::new_with_raw_id(
            uuid::Uuid::new_v4().to_string(),
            "Fixture · BTCB2".to_string(),
            crate::chain::ChainId::BitcoinBlake2b,
        );
        target.vault_wallet_id = Some(crate::app::settings::WalletId::new(
            wallet.descriptor_checksum.clone(),
            Some(1),
        ));
        let with_cube = settings::Settings {
            cubes: vec![target],
            ..Default::default()
        };
        std::fs::write(
            fork_dir.path().join(settings::SETTINGS_FILE_NAME),
            serde_json::to_vec(&with_cube).unwrap(),
        )
        .unwrap();
    }

    /// Every message a task produces, in order.
    async fn outputs(task: Task<Message>) -> Vec<Message> {
        let mut out = Vec::new();
        let Some(mut stream) = iced_runtime::task::into_stream(task) else {
            return out;
        };
        while let Some(action) = stream.next().await {
            if let iced_runtime::Action::Output(message) = action {
                out.push(message);
            }
        }
        out
    }

    /// Split coins has no launch gate (Robert, 2026-10-29): with Bitcoin
    /// Blake2b on for the account, a split request reaches the split builder
    /// (here refused for its own reason, a replay-protected signature on
    /// Bitcoin); with it off, the request is refused before anything runs.
    #[tokio::test]
    async fn split_coins_follows_blake2b_being_on_not_a_launch_gate() {
        let root_path = std::env::temp_dir().join(format!("split-on-{}", uuid::Uuid::new_v4()));
        let (mut app, _) = {
            let _guard = crate::app::session::test_guard();
            bitcoin_app(&root_path)
        };
        let mut client =
            crate::services::coincube::CoincubeClient::for_test("https://connect.example.invalid");
        client.set_token("fixture-token");
        app.panels.connect.account.client = client;
        app.panels.connect.account.user = Some(crate::services::coincube::User {
            id: 7,
            email: "7@example.invalid".into(),
            email_verified: Some(true),
        });
        app.panels.connect.account.step =
            crate::app::state::connect::account::ConnectFlowStep::Dashboard;
        assert!(crate::services::chain_status::checks_enabled(
            crate::chain::ChainId::Bitcoin
        ));
        let request = || Message::SplitRequest {
            method: crate::services::split_coins::Method::Signature,
            coins: vec![bitcoin::OutPoint::null()],
            key: None,
            feerate_vb: 2,
        };
        let refusal = |messages: Vec<Message>| {
            messages.into_iter().find_map(|message| match message {
                Message::SplitPrepared(Err(error)) => Some(error),
                _ => None,
            })
        };

        assert!(app.blake2b_on());
        let on = refusal(outputs(app.update(request())).await).expect("a refusal");
        assert!(on.contains("replay-protected signature"), "{}", on);

        app.panels.connect.account.bitcoin_blake2b_opt_in = false;
        assert!(!app.blake2b_on());
        let off = refusal(outputs(app.update(request())).await).expect("a refusal");
        assert!(off.contains("Bitcoin Blake2b"), "{}", off);
        let _ = std::fs::remove_dir_all(&root_path);
    }

    /// Whether a copy exists is read from the disk whenever Tools › Split
    /// coins opens, never only from the flag cached at Cube open: the App was
    /// built while no copy existed; a copy then appears on disk (this App's
    /// copy installer, or another instance); opening the hub corrects it.
    #[test]
    fn a_stale_copy_exists_cache_is_corrected_when_split_coins_opens() {
        let root_path = std::env::temp_dir().join(format!("copy-stale-{}", uuid::Uuid::new_v4()));
        let root = CoincubeDirectory::new(root_path.clone());
        let (mut app, wallet) = {
            let _guard = crate::app::session::test_guard();
            bitcoin_app(&root_path)
        };
        assert!(!app.cache.btcb2_already_claimed);
        let split_coins = Menu::Vault(menu::VaultSubMenu::SplitCoins);
        drop(app.set_current_panel(split_coins.clone()));
        assert!(!app.cache.btcb2_already_claimed, "nothing on disk yet");

        write_claim_target(&root, &wallet);
        assert!(!app.cache.btcb2_already_claimed, "stale by construction");
        drop(app.set_current_panel(split_coins));
        assert!(app.cache.btcb2_already_claimed, "corrected from the disk");
        assert!(app.claim_source_cube().already_claimed);
        let _ = std::fs::remove_dir_all(&root_path);
    }

    /// Gandalf's reviewer regression (#518 review, finding 2): the GUI
    /// broadcasts a Connect log-out or session replacement to every tab
    /// through `Tab::invalidate_fork_session`; a Bitcoin App holding a claim
    /// must have it revoked there, whether or not it originated the change.
    #[test]
    fn reviewer_global_auth_invalidation_must_revoke_bitcoin_claim() {
        let _guard = crate::app::session::test_guard();
        let root = std::env::temp_dir().join(format!("reviewer-auth-{}", uuid::Uuid::new_v4()));
        let (app, _) = bitcoin_app(&root);
        let generation = app.panels.session_generation.subscribe();
        let before = *generation.borrow();
        let mut tab = crate::gui::tab::Tab::new(1, crate::gui::tab::State::App(app));
        // GUI::update broadcasts this to every tab on a global LogOut.
        drop(tab.invalidate_fork_session(crate::gui::tab::AuthChange::LogOut, false));
        let after = *generation.borrow();
        assert!(
            matches!(&tab.state, crate::gui::tab::State::App(_)),
            "the Bitcoin App itself stays open"
        );
        let _ = std::fs::remove_dir_all(&root);
        assert_ne!(
            before, after,
            "global auth invalidation left the Bitcoin claim generation unchanged"
        );
    }

    /// Finding 2's Some→Some half through the App's own hook: a replacement
    /// session (another account) revokes the claim and advances the
    /// generation; a first sign-in, and the same session again, do not. The
    /// account panel is put in its signed-in state directly — the hook reads
    /// it, it does not drive it.
    #[test]
    fn a_replacement_session_through_the_hook_revokes_and_bumps() {
        let _guard = crate::app::session::test_guard();
        let root = std::env::temp_dir().join(format!("claim-replace-{}", uuid::Uuid::new_v4()));
        let (mut app, _) = bitcoin_app(&root);
        let generation = app.panels.session_generation.subscribe();
        let sign_in = |app: &mut App, id: u32| {
            let mut client = crate::services::coincube::CoincubeClient::for_test(
                "https://connect.example.invalid",
            );
            client.set_token("fixture-token");
            app.panels.connect.account.client = client;
            app.panels.connect.account.user = Some(crate::services::coincube::User {
                id,
                email: format!("{id}@example.invalid"),
                email_verified: Some(true),
            });
            app.panels.connect.account.step =
                crate::app::state::connect::account::ConnectFlowStep::Dashboard;
        };
        // Any account message runs the hook; this one changes nothing else.
        let poke = |app: &mut App| {
            drop(app.update(Message::View(view::Message::ConnectAccount(
                view::ConnectAccountMessage::PlanLoaded(None, 0),
            ))))
        };
        let account = |app: &App| app.claim_connect_session().map(|session| session.account);

        sign_in(&mut app, 7);
        let before = *generation.borrow();
        poke(&mut app);
        assert_eq!(
            *generation.borrow(),
            before,
            "a first sign-in replaces nothing"
        );
        assert_eq!(account(&app).as_deref(), Some("7"));
        poke(&mut app);
        assert_eq!(
            *generation.borrow(),
            before,
            "the same session again replaces nothing"
        );

        sign_in(&mut app, 8);
        poke(&mut app);
        assert_ne!(
            *generation.borrow(),
            before,
            "another account replaces the session: revoked and bumped"
        );
        assert_eq!(account(&app).as_deref(), Some("8"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Gandalf's reviewer regression (#518 review, finding 3): a backend
    /// switch replaces the daemon a reviewed claim is bound to, so the claim
    /// is revoked before the switch is dispatched (#509 item 7).
    #[test]
    fn reviewer_backend_switch_must_revoke_claim_before_dispatch() {
        let _guard = crate::app::session::test_guard();
        let root = std::env::temp_dir().join(format!("reviewer-provider-{}", uuid::Uuid::new_v4()));
        let (mut app, _) = bitcoin_app(&root);
        let generation = app.panels.session_generation.subscribe();
        let before = *generation.borrow();
        let mut cfg = app.daemon.as_ref().unwrap().config().unwrap().clone();
        if let Some(coincubed::config::BitcoinBackend::Esplora(selection)) =
            cfg.bitcoin_backend.as_mut()
        {
            selection.addr =
                "https://replacement.example.invalid/api/v1/esplora/bitcoin/mainnet".into();
        }
        // Do not poll: the revocation must precede dispatch of the switch.
        drop(app.spawn_daemon_switch(cfg));
        let after = *generation.borrow();
        assert!(app.daemon_switch_in_progress);
        let _ = std::fs::remove_dir_all(&root);
        assert_ne!(
            before, after,
            "backend switch left the claim generation unchanged"
        );
    }

    /// Control for the test above: the Blake2b refusal dispatches no switch
    /// and revokes nothing.
    #[tokio::test]
    async fn a_refused_blake2b_backend_switch_revokes_nothing() {
        let root = std::env::temp_dir().join(format!("claim-provider-{}", uuid::Uuid::new_v4()));
        let (mut app, _) = {
            let _guard = crate::app::session::test_guard();
            bitcoin_app(&root)
        };
        let generation = app.panels.session_generation.subscribe();
        let before = *generation.borrow();
        let mut cfg = app.daemon.as_ref().unwrap().config().unwrap().clone();
        cfg.bitcoin_config.chain = crate::chain::ChainId::BitcoinBlake2b;
        let produced = outputs(app.spawn_daemon_switch(cfg)).await;
        assert!(
            matches!(&produced[..], [Message::View(view::Message::ShowError(_))]),
            "{:?}",
            produced
        );
        assert_eq!(*generation.borrow(), before, "the refusal revokes nothing");
        assert!(!app.daemon_switch_in_progress);
        let _ = std::fs::remove_dir_all(&root);
    }
    #[tokio::test]
    async fn stale_connect_auth_completions_preserve_the_current_session() {
        let root =
            std::env::temp_dir().join(format!("connect-auth-routing-{}", uuid::Uuid::new_v4()));
        let (mut app, _) = bitcoin_app(&root);
        app.panels
            .connect
            .account
            .client
            .set_token("current-access");
        let tokens = crate::services::connect::client::auth::AccessTokenResponse {
            access_token: "current-access".into(),
            refresh_token: "current-refresh".into(),
            expires_at: 300,
        };
        let shared = Arc::new(tokio::sync::RwLock::new(tokens.clone()));
        app.connect_auth = Some(shared.clone());
        app.connect_email = Some("current@example.invalid".into());
        let epoch = app.panels.connect.account.auth_epoch();
        let messages = [
            Message::InAppConnectAuthReady {
                login: home_login(),
                epoch: epoch + 1,
                previous_refresh_token: Some("current-refresh".into()),
            },
            Message::InAppConnectAuthPersisted {
                login: home_login(),
                epoch: epoch + 1,
                previous_refresh_token: None,
                tokens: Some(tokens),
            },
            Message::InAppConnectLoginCompleted {
                token: "old-access".into(),
                refresh_token: "old-refresh".into(),
                email: "old@example.invalid".into(),
                epoch,
            },
        ];
        for message in messages {
            let task = app.update(message);
            if let Some(mut stream) = iced_runtime::task::into_stream(task) {
                use iced::futures::StreamExt;
                assert!(
                    stream.next().await.is_none(),
                    "stale completion started work"
                );
            }
            assert_eq!(
                app.connect_email.as_deref(),
                Some("current@example.invalid")
            );
            assert!(Arc::ptr_eq(app.connect_auth.as_ref().unwrap(), &shared));
            assert_eq!(shared.read().await.access_token, "current-access");
            assert_eq!(shared.read().await.refresh_token, "current-refresh");
            assert!(app.in_app_refresh_commit.is_none());
            assert!(app.connect_stream_config.is_none());
        }
        let _ = std::fs::remove_dir_all(root);
    }
}

#[cfg(test)]
mod received_celebration_privacy_tests {
    use crate::app::state::preferences::{BalancePrivacy, BalancePrivacyChrome};
    use crate::app::view::shared::landing::capture;

    #[test]
    fn mounted_app_arrival_masks_hidden_and_unresolved_amounts() {
        let _guard = crate::app::session::test_guard();
        let root =
            std::env::temp_dir().join(format!("celebration-privacy-{}", uuid::Uuid::new_v4()));
        let (mut app, _) = super::shell_tests::bitcoin_app(&root);
        app.show_received_celebration = true;
        app.received_celebration_amount = "12,345 SATS".into();

        for privacy in [BalancePrivacy::Hidden, BalancePrivacy::Unresolved] {
            app.cache.balance_privacy = Some(BalancePrivacyChrome {
                privacy,
                busy: false,
                interactive: false,
            });
            let mounted = capture::inspect(
                "received-celebration",
                app.received_celebration_view(),
                (1024, 768),
                "dark",
                coincube_ui::theme::Theme::dark(),
            );
            assert!(mounted.contains("••••••"));
            assert!(!mounted.contains("12,345"));
        }

        app.cache.balance_privacy = Some(BalancePrivacyChrome {
            privacy: BalancePrivacy::Shown,
            busy: false,
            interactive: true,
        });
        let shown = capture::inspect(
            "received-celebration",
            app.received_celebration_view(),
            (1024, 768),
            "dark",
            coincube_ui::theme::Theme::dark(),
        );
        // The receipt sets the unit beside the figure as its own text.
        assert!(shown.contains("+12,345"));
        assert!(shown.contains("SATS"));
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod toast_timer_tests {
    use std::time::{Duration, Instant};

    fn show(app: &mut super::App, text: &str) {
        drop(
            app.update(super::Message::View(super::view::Message::ShowToast(
                log::Level::Error,
                text.to_owned(),
            ))),
        );
    }

    /// (id, armed) for each queued toast, oldest first.
    fn clocks(app: &super::App) -> Vec<(usize, bool)> {
        app.errors
            .iter()
            .map(|(id, deadline, ..)| (*id, deadline.is_some()))
            .collect()
    }

    fn make_due(app: &mut super::App, id: usize) {
        let toast = app.errors.iter_mut().find(|(i, ..)| *i == id).unwrap();
        toast.1 = Some(Instant::now() - Duration::from_millis(1));
    }

    /// Only shown toasts run their clock, so a burst past the cap can't
    /// expire unseen: hidden toasts wait, then get their full time on screen.
    #[test]
    fn hidden_toasts_wait_for_their_turn_on_screen() {
        let _guard = crate::app::session::test_guard();
        let root = std::env::temp_dir().join(format!("toast-timers-{}", uuid::Uuid::new_v4()));
        let (mut app, _) = super::shell_tests::bitcoin_app(&root);
        app.errors.clear();
        let first = app.current_error_id;
        for n in 0..5 {
            show(&mut app, &format!("burst {n}"));
        }
        let id = |n: usize| first + n;
        assert_eq!(
            clocks(&app),
            vec![
                (id(0), false),
                (id(1), false),
                (id(2), true),
                (id(3), true),
                (id(4), true)
            ],
            "only the {} shown toasts are on the clock",
            super::view::TOAST_MAX_VISIBLE
        );

        // A hidden toast's timer (e.g. armed before it was pushed out of view)
        // and a shown toast's early or stale timer both leave it in place.
        drop(app.update(super::Message::ExpireToast(id(0))));
        drop(app.update(super::Message::ExpireToast(id(3))));
        assert_eq!(app.errors.len(), 5);

        // A shown toast whose time is up leaves, and the newest hidden one
        // comes on screen with a fresh clock.
        make_due(&mut app, id(2));
        drop(app.update(super::Message::ExpireToast(id(2))));
        assert_eq!(
            clocks(&app),
            vec![(id(0), false), (id(1), true), (id(3), true), (id(4), true)]
        );

        // Dismissing by hand brings the last waiting toast on screen.
        drop(
            app.update(super::Message::View(super::view::Message::DismissToast(
                id(4),
            ))),
        );
        assert_eq!(
            clocks(&app),
            vec![(id(0), true), (id(1), true), (id(3), true)]
        );

        // A toast pushed back out of view stops its clock again.
        show(&mut app, "late");
        assert_eq!(clocks(&app)[0], (id(0), false));

        // A burst past the backlog drops the oldest waiting toasts.
        for n in 0..10 {
            show(&mut app, &format!("flood {n}"));
        }
        assert_eq!(
            app.errors.len(),
            super::view::TOAST_MAX_VISIBLE + super::TOAST_MAX_WAITING
        );
        assert_eq!(
            app.errors.last().map(|(i, ..)| *i),
            Some(app.current_error_id - 1),
            "the newest toast is always kept"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod incoming_liquid_toast_privacy_tests {
    use crate::app::state::preferences::{BalancePrivacy, BalancePrivacyChrome};
    use crate::app::view::shared::landing::capture;

    #[test]
    fn visible_waiting_toast_tracks_hide_and_accepted_show() {
        let _guard = crate::app::session::test_guard();
        let root =
            std::env::temp_dir().join(format!("incoming-toast-privacy-{}", uuid::Uuid::new_v4()));
        let (mut app, _) = super::shell_tests::bitcoin_app(&root);
        assert_eq!(
            super::ToastContent::Text("Split review: 12,345 sats".into())
                .display(app.cache.bitcoin_unit, true),
            "Split review: 12,345 sats",
            "ordinary toast text must not be subject to broad string redaction"
        );
        drop(app.update(super::Message::View(
            super::view::Message::ShowIncomingLiquidWaitingToast(12_345),
        )));
        assert_eq!(app.errors.len(), 1);

        let mounted = |app: &super::App| {
            capture::inspect(
                "incoming-liquid-waiting-toast",
                super::view::toast_overlay(app.displayed_toasts()),
                (1024, 768),
                "dark",
                coincube_ui::theme::Theme::dark(),
            )
        };
        let set_privacy = |app: &mut super::App, privacy| {
            app.cache.balance_privacy = Some(BalancePrivacyChrome {
                privacy,
                busy: false,
                interactive: privacy == BalancePrivacy::Shown,
            });
        };

        // The same toast stays mounted throughout. Hide masks immediately,
        // including before its persistence completes; a failed or pending Show
        // still has no accepted Show receipt and cannot reveal the amount.
        set_privacy(&mut app, BalancePrivacy::Shown);
        let shown = mounted(&app);
        assert!(shown.contains("12,345"));
        set_privacy(&mut app, BalancePrivacy::HiddenLocally);
        let hidden = mounted(&app);
        assert!(hidden.contains("••••••"));
        assert!(!hidden.contains("12,345"));
        assert!(hidden.contains("swapping to L-BTC, awaiting confirmation"));
        set_privacy(&mut app, BalancePrivacy::Hidden);
        assert!(!mounted(&app).contains("12,345"));
        set_privacy(&mut app, BalancePrivacy::Unresolved);
        assert!(!mounted(&app).contains("12,345"));
        app.cache.balance_privacy = None;
        assert!(!mounted(&app).contains("12,345"));
        // The user has requested Show, but the write has not produced its
        // accepted receipt. The GUI still publishes Hidden while busy.
        app.cache.balance_privacy = Some(BalancePrivacyChrome {
            privacy: BalancePrivacy::Hidden,
            busy: true,
            interactive: false,
        });
        assert!(!mounted(&app).contains("12,345"));
        // The GUI publishes Shown only after accepting the explicit Show
        // receipt; the existing toast then reveals the original amount.
        set_privacy(&mut app, BalancePrivacy::Shown);
        assert!(mounted(&app).contains("12,345"));
        assert_eq!(app.errors.len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// #968 review finding 1: Tenshu v175 hides Liquid for everyone (master §3
/// decision 2), so a Liquid receive raises no waiting toast and no
/// celebration, even on a Cube with the grant and a Liquid wallet on disk.
/// [`super::features::LiquidGate::show`] can't be turned on, so each test was
/// checked against its guard removed (both failed); the Spark control shows
/// the celebration itself still works.
#[cfg(test)]
mod hidden_liquid_receive_tests {
    use breez_sdk_liquid::prelude::{Payment, PaymentDetails, PaymentState, PaymentType, SdkEvent};

    fn liquid_receive() -> Payment {
        Payment {
            destination: None,
            tx_id: Some("synthetic-liquid-receive".into()),
            unblinding_data: None,
            timestamp: 0,
            amount_sat: 12_345,
            fees_sat: 0,
            swapper_fees_sat: None,
            payment_type: PaymentType::Receive,
            status: PaymentState::Pending,
            details: PaymentDetails::Liquid {
                destination: "synthetic-destination".into(),
                description: String::new(),
                asset_id: String::new(),
                asset_info: None,
                lnurl_info: None,
                bip353_address: None,
                payer_note: None,
            },
        }
    }

    /// A mainnet App whose Liquid gate has both inputs set.
    fn app_with_liquid_inputs(root: &std::path::Path) -> super::App {
        let (mut app, _) = super::shell_tests::bitcoin_app(root);
        app.cache.liquid_gate = super::features::LiquidGate {
            server_enabled: true,
            local_state_exists: true,
        };
        app
    }

    /// Every message a task produces, in order.
    fn outputs(task: super::Task<super::Message>) -> Vec<super::Message> {
        use iced_runtime::futures::futures::StreamExt;
        let Some(stream) = iced_runtime::task::into_stream(task) else {
            return Vec::new();
        };
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(
                stream
                    .filter_map(|action| async move {
                        match action {
                            iced_runtime::Action::Output(message) => Some(message),
                            _ => None,
                        }
                    })
                    .collect(),
            )
    }

    #[test]
    fn hidden_liquid_receive_raises_no_waiting_toast() {
        let _guard = crate::app::session::test_guard();
        let root =
            std::env::temp_dir().join(format!("hidden-liquid-toast-{}", uuid::Uuid::new_v4()));
        let mut app = app_with_liquid_inputs(&root);
        let produced = outputs(app.update(super::Message::BreezEvent(
            SdkEvent::PaymentWaitingConfirmation {
                details: liquid_receive(),
            },
        )));
        let _ = std::fs::remove_dir_all(&root);
        assert!(!produced.iter().any(|message| matches!(
            message,
            super::Message::View(super::view::Message::ShowIncomingLiquidWaitingToast(_))
        )));
        assert!(app.toasted_incoming_waiting_tx_ids.is_empty());
    }

    #[test]
    fn hidden_liquid_receive_does_not_celebrate() {
        let _guard = crate::app::session::test_guard();
        let root = std::env::temp_dir().join(format!(
            "hidden-liquid-celebration-{}",
            uuid::Uuid::new_v4()
        ));
        let mut app = app_with_liquid_inputs(&root);
        drop(
            app.update(super::Message::BreezEvent(SdkEvent::PaymentSucceeded {
                details: liquid_receive(),
            })),
        );
        assert!(!app.show_received_celebration);

        // Control: a Spark receive on the same App still celebrates.
        drop(app.update(super::Message::ShowReceivedCelebration {
            context: "spark-receive".to_string(),
            amount_sat: 12_345,
        }));
        let _ = std::fs::remove_dir_all(&root);
        assert!(app.show_received_celebration);
    }
}

#[cfg(test)]
mod settings_navigation_integration_tests {
    use super::*;
    use crate::app::view::shared::landing::capture;
    use coincube_ui::theme as ui_theme;

    #[test]
    fn bare_vault_settings_route_mounts_the_selected_wallet_section() {
        let _guard = crate::app::session::test_guard();
        let root =
            std::env::temp_dir().join(format!("settings-default-route-{}", uuid::Uuid::new_v4()));
        let (mut app, _) = super::shell_tests::bitcoin_app(&root);

        drop(app.update(Message::View(view::Message::Menu(Menu::Vault(
            menu::VaultSubMenu::Settings(None),
        )))));
        assert_eq!(
            app.panels.current,
            Menu::Vault(menu::VaultSubMenu::Settings(Some(
                menu::SettingsOption::Wallet
            )))
        );
        let mounted = capture::inspect(
            "vault-settings-default-route",
            app.panels
                .current()
                .unwrap()
                .view(&app.panels.current, &app.cache),
            (1024, 768),
            "dark",
            ui_theme::Theme::dark(),
        );
        assert!(mounted.contains("Wallet & signers"));
        assert!(mounted.contains("Identity and policy"));
        assert!(mounted.contains("Signers"));
        assert!(!mounted.contains("Open Wallet & signers to review"));
        let _ = std::fs::remove_dir_all(root);
    }

    /// A full-page replacement on a Settings route (the received-payment
    /// celebration, the unavailable placeholder) shows neither the Settings
    /// head nor its tabs.
    #[test]
    fn replacement_pages_on_settings_routes_take_no_route_chrome() {
        let cache = Cache {
            has_vault: true,
            cube_name: "Everyday".into(),
            ..Cache::default()
        };
        let menu = Menu::Spark(menu::SparkSubMenu::Settings(Some(
            menu::SparkSettingsOption::Connections,
        )));
        let routed = capture::inspect(
            "settings-route-page",
            view::dashboard(&menu, &cache, iced::widget::text("Page body")),
            (1024, 768),
            "dark",
            ui_theme::Theme::dark(),
        );
        assert!(routed.contains("Page body"));
        assert!(routed.contains("Spark · Everyday · Bitcoin mainnet"));
        assert!(routed.contains("Balances & conversion"));
        assert!(routed.contains("Lightning address"));

        let replaced = capture::inspect(
            "settings-route-replacement",
            view::replacement_page(&menu, &cache, iced::widget::text("Page body")),
            (1024, 768),
            "dark",
            ui_theme::Theme::dark(),
        );
        assert!(replaced.contains("Page body"));
        assert!(!replaced.contains("Spark · Everyday · Bitcoin mainnet"));
        assert!(!replaced.contains("Balances & conversion"));
        assert!(!replaced.contains("Lightning address"));
    }

    /// Every message a task produces, in order.
    fn outputs(task: Task<Message>) -> Vec<Message> {
        use iced_runtime::futures::futures::StreamExt;
        let Some(stream) = iced_runtime::task::into_stream(task) else {
            return Vec::new();
        };
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(
                stream
                    .filter_map(|action| async move {
                        match action {
                            iced_runtime::Action::Output(message) => Some(message),
                            _ => None,
                        }
                    })
                    .collect(),
            )
    }

    fn saved_spark_alias(app: &App) -> Option<String> {
        let network_dir = app.datadir.network_directory(app.cube_settings.network);
        settings::Settings::from_file(&network_dir)
            .unwrap()
            .cubes
            .into_iter()
            .find(|cube| cube.id == app.cube_settings.id)
            .expect("the Cube is in its settings file")
            .spark_alias
    }

    fn nickname(message: view::SparkSettingsMessage) -> Message {
        Message::View(view::Message::SparkSettings(message))
    }

    /// The texts drawn inside the side panel's Spark row of the App's own
    /// view, so neither the Settings head line nor the cache can stand in.
    fn side_panel_spark_row(app: &App) -> Vec<String> {
        let page = capture::inspect_wide_content(
            "spark-nickname-side-panel",
            app.view(),
            (1440, 900),
            "dark",
            ui_theme::Theme::dark(),
            1440,
        );
        let (_, row) = page
            .focus
            .iter()
            .find(|(id, _)| *id == iced::advanced::widget::Id::new("tenshu.shell.nav.Spark"))
            .expect("the side panel has a Spark row");
        page.text
            .iter()
            .filter(|(_, bounds)| row.contains(bounds.center()))
            .map(|(value, _)| value.clone())
            .collect()
    }

    /// An App on Spark's Balances & conversion with its Cube in the settings
    /// file, as a saved Cube is.
    /// A second App over `old`'s Cube, as reopening it builds.
    fn reopened_nickname_app(root: &std::path::Path, old: &App) -> App {
        let mut app = nickname_app(root);
        app.cube_settings = old.cube_settings.clone();
        app.cache.cube_id = old.cube_settings.id.clone();
        app
    }

    fn nickname_app(root: &std::path::Path) -> App {
        let (mut app, _) = super::shell_tests::bitcoin_app(root);
        let network_dir = app.datadir.network_directory(app.cube_settings.network);
        let cube = app.cube_settings.clone();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(settings::update_settings_file(
                &network_dir,
                move |mut s| {
                    s.cubes.push(cube);
                    Some(s)
                },
            ))
            .unwrap();
        drop(
            app.set_current_panel(Menu::Spark(menu::SparkSubMenu::Settings(Some(
                menu::SparkSettingsOption::General,
            )))),
        );
        app
    }

    /// Submissions made while a nickname write runs wait for it, so the
    /// latest one is what is saved and shown: A then B, and a new name then
    /// back to the saved one.
    #[test]
    fn overlapping_spark_nickname_saves_land_in_request_order() {
        let _guard = crate::app::session::test_guard();
        let root =
            std::env::temp_dir().join(format!("spark-nickname-order-{}", uuid::Uuid::new_v4()));
        let mut app = nickname_app(&root);
        let submit = |app: &mut App, name: &str| {
            outputs(
                app.update(nickname(view::SparkSettingsMessage::NicknameSubmitted(
                    name.into(),
                ))),
            )
        };

        // A, then B while A is being written: B starts no write of its own.
        let a = submit(&mut app, "A");
        assert!(
            matches!(a.as_slice(), [Message::SparkNicknameSaved { result: Ok(Some(n)), .. }] if n == "A")
        );
        assert!(submit(&mut app, "B").is_empty(), "B waits for A");
        // A lands; B is written after it, and B is what stays.
        let b = outputs(app.update(a.into_iter().next().unwrap()));
        assert_eq!(app.cube_settings.spark_alias.as_deref(), Some("A"));
        let b: Vec<Message> = b
            .into_iter()
            .filter(|m| matches!(m, Message::SparkNicknameSaved { .. }))
            .collect();
        assert!(
            matches!(b.as_slice(), [Message::SparkNicknameSaved { result: Ok(Some(n)), .. }] if n == "B")
        );
        for message in b {
            drop(app.update(message));
        }
        assert_eq!(app.cube_settings.spark_alias.as_deref(), Some("B"));
        assert_eq!(saved_spark_alias(&app).as_deref(), Some("B"));
        assert_eq!(side_panel_spark_row(&app), ["Spark · B"]);

        // "New", then back to "B" before New lands: B is not skipped as
        // already saved, and is written after New.
        let new = submit(&mut app, "New");
        assert!(submit(&mut app, "B").is_empty(), "B waits for New");
        let back: Vec<Message> = outputs(app.update(new.into_iter().next().unwrap()))
            .into_iter()
            .filter(|m| matches!(m, Message::SparkNicknameSaved { .. }))
            .collect();
        assert_eq!(
            saved_spark_alias(&app).as_deref(),
            Some("B"),
            "written after New"
        );
        for message in back {
            drop(app.update(message));
        }
        assert_eq!(app.cube_settings.spark_alias.as_deref(), Some("B"));
        assert_eq!(side_panel_spark_row(&app), ["Spark · B"]);
        let _ = std::fs::remove_dir_all(root);
    }

    /// A write the tab starts as it replaces the App lands after the App's
    /// running write, however the two tasks are polled: here the teardown
    /// write runs first, before the running write's task has started. And if
    /// that task is dropped unrun, the teardown write doesn't wait forever.
    #[test]
    fn the_teardown_nickname_write_follows_the_running_one() {
        use iced_runtime::futures::futures::StreamExt;
        let _guard = crate::app::session::test_guard();
        let root =
            std::env::temp_dir().join(format!("spark-nickname-teardown-{}", uuid::Uuid::new_v4()));
        let mut app = nickname_app(&root);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let submit = |app: &mut App, name: &str| {
            app.update(nickname(view::SparkSettingsMessage::NicknameSubmitted(
                name.into(),
            )))
        };

        // A is submitted but its task hasn't run; B queues behind it.
        let a = submit(&mut app, "A");
        drop(submit(&mut app, "B"));
        let teardown = app.spark_nickname_flush().expect("B is owed");
        runtime.block_on(async {
            let teardown = tokio::spawn(teardown.run());
            // The teardown write is polled first, and waits.
            for _ in 0..10 {
                tokio::task::yield_now().await;
            }
            assert!(!teardown.is_finished(), "it waits for A");
            iced_runtime::task::into_stream(a)
                .unwrap()
                .for_each(|_| async {})
                .await;
            assert_eq!(teardown.await.unwrap(), Ok(Some("B".into())));
        });
        assert_eq!(saved_spark_alias(&app).as_deref(), Some("B"), "B after A");

        // A running write whose task is dropped unrun releases its follower.
        let mut app = nickname_app(&root.join("dropped"));
        let a = submit(&mut app, "A");
        drop(submit(&mut app, "B"));
        let teardown = app.spark_nickname_flush().expect("B is owed");
        drop(a);
        let written = runtime.block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(10), teardown.run()).await
        });
        assert_eq!(written, Ok(Ok(Some("B".into()))));
        let _ = std::fs::remove_dir_all(root);
    }

    /// A Cube locked while its nickname write runs and reopened at once: the
    /// new App's writes land after the old App's, however the tasks are
    /// polled, and going back to the name the new App read isn't skipped as
    /// already saved while the old write could still land over it.
    #[test]
    fn a_reopened_cubes_nickname_writes_follow_the_previous_apps() {
        use iced_runtime::futures::futures::StreamExt;
        let _guard = crate::app::session::test_guard();
        let root =
            std::env::temp_dir().join(format!("spark-nickname-reopen-{}", uuid::Uuid::new_v4()));
        let mut old = nickname_app(&root);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let submit = |app: &mut App, name: &str| {
            app.update(nickname(view::SparkSettingsMessage::NicknameSubmitted(
                name.into(),
            )))
        };

        // The old App's B is created but its task hasn't run; the reopened
        // App (a second App over the same settings file) submits C.
        let b = submit(&mut old, "B");
        let mut reopened = reopened_nickname_app(&root, &old);
        let c = submit(&mut reopened, "C");
        runtime.block_on(async {
            let c = tokio::spawn(async move {
                iced_runtime::task::into_stream(c)
                    .unwrap()
                    .collect::<Vec<_>>()
                    .await
            });
            for _ in 0..10 {
                tokio::task::yield_now().await;
            }
            assert!(!c.is_finished(), "C waits for B");
            iced_runtime::task::into_stream(b)
                .unwrap()
                .for_each(|_| async {})
                .await;
            c.await.unwrap();
        });
        assert_eq!(
            saved_spark_alias(&reopened).as_deref(),
            Some("C"),
            "C after B"
        );

        // B is running again in the old App; the reopened App read A before
        // B landed, and asks for A: it is written, after B.
        let root = root.join("back");
        let mut old = nickname_app(&root);
        let mut reopened = reopened_nickname_app(&root, &old);
        let b = submit(&mut old, "B");
        assert_eq!(reopened.cube_settings.spark_alias, None);
        let back = submit(&mut reopened, "");
        runtime.block_on(
            iced_runtime::task::into_stream(b)
                .unwrap()
                .for_each(|_| async {}),
        );
        let back = outputs(back);
        assert!(
            matches!(
                back.as_slice(),
                [Message::SparkNicknameSaved {
                    result: Ok(None),
                    ..
                }]
            ),
            "plain Spark is written, not skipped"
        );
        assert_eq!(saved_spark_alias(&reopened), None, "plain Spark after B");
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    /// What the App still owes when the tab replaces it: a typed name on
    /// Balances & conversion, else one queued behind a running write, and
    /// nothing once both are written or off that tab.
    #[test]
    fn the_spark_nickname_flush_is_the_typed_name_else_the_queued_one() {
        let _guard = crate::app::session::test_guard();
        let root =
            std::env::temp_dir().join(format!("spark-nickname-flush-{}", uuid::Uuid::new_v4()));
        let mut app = nickname_app(&root);
        assert!(app.spark_nickname_flush().is_none());

        drop(
            app.update(nickname(view::SparkSettingsMessage::NicknameEdited(
                "Daily".into(),
            ))),
        );
        let write = app.spark_nickname_flush().expect("the typed name");
        let written = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(write.run());
        assert_eq!(written, Ok(Some("Daily".into())));
        assert_eq!(saved_spark_alias(&app).as_deref(), Some("Daily"));

        // Off the tab, a typed name is the leave-tab save's, not the flush's.
        drop(
            app.set_current_panel(Menu::Spark(menu::SparkSubMenu::Settings(Some(
                menu::SparkSettingsOption::Connections,
            )))),
        );
        assert!(app.spark_nickname_flush().is_none());

        // A request queued behind a running write is owed too.
        let running = outputs(
            app.update(nickname(view::SparkSettingsMessage::NicknameSubmitted(
                "One".into(),
            ))),
        );
        drop(
            app.update(nickname(view::SparkSettingsMessage::NicknameSubmitted(
                "Two".into(),
            ))),
        );
        assert!(matches!(
            app.spark_nickname_flush().map(|w| w.nickname),
            Some(Some(n)) if n == "Two"
        ));
        drop(running);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_spark_nickname_left_unsubmitted_is_saved_and_names_the_side_panel() {
        let _guard = crate::app::session::test_guard();
        let root = std::env::temp_dir().join(format!("spark-nickname-{}", uuid::Uuid::new_v4()));
        let (mut app, _) = super::shell_tests::bitcoin_app(&root);
        let network_dir = app.datadir.network_directory(app.cube_settings.network);
        let cube = app.cube_settings.clone();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(settings::update_settings_file(
                &network_dir,
                move |mut s| {
                    s.cubes.push(cube);
                    Some(s)
                },
            ))
            .unwrap();
        let balances = Menu::Spark(menu::SparkSubMenu::Settings(Some(
            menu::SparkSettingsOption::General,
        )));
        let connections = Menu::Spark(menu::SparkSubMenu::Settings(Some(
            menu::SparkSettingsOption::Connections,
        )));
        drop(app.set_current_panel(balances.clone()));
        drop(
            app.update(nickname(view::SparkSettingsMessage::NicknameEdited(
                " Daily spending ".into(),
            ))),
        );

        // Staying on the tab (its bare route included) saves nothing yet.
        assert!(outputs(app.save_pending_spark_nickname(&balances)).is_empty());
        assert!(outputs(
            app.save_pending_spark_nickname(&Menu::Spark(menu::SparkSubMenu::Settings(None)))
        )
        .is_empty());

        // Leaving it for another tab submits the typed name, which the App
        // writes to this Cube's settings and shows in the side panel.
        let submitted: Vec<Message> =
            outputs(app.update(Message::View(view::Message::Menu(connections.clone()))))
                .into_iter()
                .filter(|message| {
                    matches!(
                        message,
                        Message::View(view::Message::SparkSettings(
                            view::SparkSettingsMessage::NicknameSubmitted(_)
                        ))
                    )
                })
                .collect();
        assert_eq!(app.panels.current, connections);
        assert!(matches!(
            submitted.as_slice(),
            [Message::View(view::Message::SparkSettings(
                view::SparkSettingsMessage::NicknameSubmitted(name)
            ))] if name == " Daily spending "
        ));
        let saved = outputs(app.update(submitted.into_iter().next().unwrap()));
        assert!(matches!(
            saved.as_slice(),
            [Message::SparkNicknameSaved { result: Ok(Some(name)), .. }] if name == "Daily spending"
        ));
        // Typed and written, but the App hasn't taken the result yet.
        assert_eq!(side_panel_spark_row(&app), ["Spark"]);
        for message in saved {
            drop(app.update(message));
        }
        // The next frame after the save names the wallet.
        assert_eq!(side_panel_spark_row(&app), ["Spark · Daily spending"]);
        assert_eq!(
            app.cube_settings.spark_alias.as_deref(),
            Some("Daily spending")
        );
        assert_eq!(saved_spark_alias(&app).as_deref(), Some("Daily spending"));
        // Saved, so leaving again has nothing to submit.
        drop(app.set_current_panel(balances.clone()));
        assert!(outputs(app.save_pending_spark_nickname(&connections)).is_empty());

        // An empty field goes back to plain "Spark".
        drop(
            app.update(nickname(view::SparkSettingsMessage::NicknameEdited(
                String::new(),
            ))),
        );
        let saved = outputs(
            app.update(nickname(view::SparkSettingsMessage::NicknameSubmitted(
                String::new(),
            ))),
        );
        for message in saved {
            drop(app.update(message));
        }
        assert_eq!(side_panel_spark_row(&app), ["Spark"]);
        assert_eq!(app.cube_settings.spark_alias, None);
        assert_eq!(saved_spark_alias(&app), None);

        // One too long to save is left in the field, unsaved.
        let long = "a".repeat(64);
        drop(
            app.update(nickname(view::SparkSettingsMessage::NicknameEdited(
                long.clone(),
            ))),
        );
        assert!(outputs(
            app.update(nickname(view::SparkSettingsMessage::NicknameSubmitted(
                long
            )))
        )
        .is_empty());
        assert!(outputs(app.save_pending_spark_nickname(&connections)).is_empty());
        assert_eq!(saved_spark_alias(&app), None);

        // A save for another Cube (the tab moved on while it was written)
        // changes nothing in this App.
        drop(app.update(Message::SparkNicknameSaved {
            cube_id: format!("{}-other", app.cube_settings.id),
            result: Ok(Some("Elsewhere".into())),
        }));
        assert_eq!(app.cube_settings.spark_alias, None);
        assert_eq!(side_panel_spark_row(&app), ["Spark"]);
        let _ = std::fs::remove_dir_all(root);
    }

    /// The Spark page's share panel leads to Settings › Spark › Lightning
    /// address with the change form open, and the claim button to the same
    /// tab with the form closed. Either way the panel is gone on return.
    #[test]
    fn spark_lightning_share_change_and_claim_open_the_address_settings() {
        let _guard = crate::app::session::test_guard();
        let root = std::env::temp_dir().join(format!("spark-ln-share-{}", uuid::Uuid::new_v4()));
        let (mut app, _) = super::shell_tests::bitcoin_app(&root);
        let overview = Menu::Spark(menu::SparkSubMenu::Overview);
        let address_settings = Menu::Spark(menu::SparkSubMenu::Settings(Some(
            menu::SparkSettingsOption::LightningAddress,
        )));
        let spark = |message| Message::View(view::Message::SparkOverview(message));
        drop(app.set_current_panel(overview.clone()));

        drop(
            app.update(spark(view::SparkOverviewMessage::ShareLightningAddress(
                "sats@coincube.io".into(),
            ))),
        );
        assert_eq!(
            app.panels.spark_overview.lightning_share_for_tests(),
            Some("sats@coincube.io")
        );
        let copied = outputs(
            app.update(spark(view::SparkOverviewMessage::CopyLightningAddress(
                "sats@coincube.io".into(),
            ))),
        );
        assert!(matches!(
            copied.as_slice(),
            [Message::View(view::Message::Clipboard(text))] if text == "sats@coincube.io"
        ));
        drop(app.update(spark(view::SparkOverviewMessage::CloseLightningShare)));
        assert_eq!(app.panels.spark_overview.lightning_share_for_tests(), None);

        drop(
            app.update(spark(view::SparkOverviewMessage::ShareLightningAddress(
                "sats@coincube.io".into(),
            ))),
        );
        let change = outputs(app.update(spark(view::SparkOverviewMessage::ChangeLightningAddress)));
        assert_eq!(app.panels.spark_overview.lightning_share_for_tests(), None);
        assert!(
            matches!(
                change.as_slice(),
                [
                    Message::View(view::Message::Menu(menu)),
                    Message::View(view::Message::ConnectCube(
                        view::ConnectCubeMessage::BeginEditLightningAddress
                    )),
                ] if *menu == address_settings
            ),
            "{:?}",
            change
        );
        for message in change {
            drop(app.update(message));
        }
        assert_eq!(app.panels.current, address_settings);
        assert!(app.panels.connect.cube.ln_editing);

        drop(app.update(Message::View(view::Message::Menu(overview))));
        app.panels.connect.cube.ln_editing = false;
        let claim = outputs(app.update(spark(view::SparkOverviewMessage::ClaimLightningAddress)));
        assert!(matches!(
            claim.as_slice(),
            [Message::View(view::Message::Menu(menu))] if *menu == address_settings
        ));
        for message in claim {
            drop(app.update(message));
        }
        assert_eq!(app.panels.current, address_settings);
        assert!(!app.panels.connect.cube.ln_editing);
        let _ = std::fs::remove_dir_all(root);
    }
}

#[cfg(test)]
mod local_node_sync_tests {
    use super::*;
    use coincubed::config::{BitcoinBackend, BitcoindConfig, BitcoindRpcAuth};

    #[test]
    fn local_node_wizard_survives_failure_and_success_until_dismissed() {
        let root = std::env::temp_dir().join(format!("switch-checklist-{}", uuid::Uuid::new_v4()));
        let _guard = crate::app::session::test_guard();
        let (mut app, _) = super::shell_tests::bitcoin_app(&root);
        let pending = BitcoindConfig {
            addr: "127.0.0.1:8332".parse().unwrap(),
            rpc_auth: BitcoindRpcAuth::CookieFile(
                crate::node::bitcoind::internal_bitcoind_cookie_path(
                    &crate::node::bitcoind::internal_bitcoind_datadir(&app.cache.datadir_path),
                    &app.cache.network,
                ),
            ),
        };
        let mut connect = app.daemon.as_ref().unwrap().config().unwrap().clone();
        connect.pending_bitcoind = Some(pending.clone());
        app.daemon = Some(Arc::new(EmbeddedDaemon::unstarted_for_test(
            connect.clone(),
            None,
        )));
        let mut local = connect.clone();
        local.bitcoin_backend = Some(BitcoinBackend::Bitcoind(pending));
        local.pending_bitcoind = None;

        drop(app.spawn_daemon_switch(local.clone()));
        assert!(app.cache.local_node_transfer_visible);
        assert!(app.cache.daemon_switch_in_progress);
        drop(app.update(Message::DaemonRestarted(DaemonRestart::Failed {
            error: Error::Config("fixture switch failure".into()),
            recovered: app.daemon.clone(),
        })));
        assert!(
            app.cache.local_node_transfer_visible,
            "retain retry after failure"
        );
        assert!(!app.cache.daemon_switch_in_progress);

        drop(app.spawn_daemon_switch(local.clone()));
        drop(
            app.update(Message::DaemonRestarted(DaemonRestart::Started(Arc::new(
                EmbeddedDaemon::unstarted_for_test(local, None),
            )))),
        );
        assert!(
            app.cache.local_node_transfer_visible,
            "keep completed steps visible"
        );
        assert!(
            !app.cache.node_history_switch_requested,
            "presentation is not switch consent"
        );
        assert!(!app.cache.daemon_switch_in_progress);
        drop(app.set_current_panel(Menu::Vault(menu::VaultSubMenu::Transactions(None))));
        drop(
            app.set_current_panel(Menu::Vault(menu::VaultSubMenu::Settings(Some(
                menu::SettingsOption::Node,
            )))),
        );
        assert!(
            app.cache.local_node_transfer_visible,
            "navigation preserves the result"
        );
        drop(app.update(Message::View(view::Message::Settings(
            view::SettingsMessage::NodeSettings(view::NodeSettingsMessage::RecoveryDismiss),
        ))));
        assert!(!app.cache.local_node_transfer_visible);

        app.cache.local_node_transfer_visible = true;
        drop(app.spawn_daemon_switch(connect));
        assert!(
            !app.cache.local_node_transfer_visible,
            "leaving the local node clears the result"
        );
        drop(app);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn manual_recovery_switch_survives_navigation_and_honors_cancellation() {
        use local_switch::{NodePruning, VaultHistory};
        let root = std::env::temp_dir().join(format!("guided-switch-{}", uuid::Uuid::new_v4()));
        let _guard = crate::app::session::test_guard();
        let (mut app, _) = super::shell_tests::bitcoin_app(&root);
        let pending = BitcoindConfig {
            addr: "127.0.0.1:8332".parse().unwrap(),
            rpc_auth: BitcoindRpcAuth::CookieFile(
                crate::node::bitcoind::internal_bitcoind_cookie_path(
                    &crate::node::bitcoind::internal_bitcoind_datadir(&app.cache.datadir_path),
                    &app.cache.network,
                ),
            ),
        };
        let mut cfg = app.daemon.as_ref().unwrap().config().unwrap().clone();
        cfg.pending_bitcoind = Some(pending.clone());
        cfg.auto_switch_to_pending = Some(false);
        app.daemon = Some(Arc::new(EmbeddedDaemon::unstarted_for_test(cfg, None)));
        app.cache.node_bitcoind_ibd = Some(false);
        app.cache.node_bitcoind_pruning = Some(NodePruning::Pruned {
            prune_height: 970_000,
        });
        app.cache.local_switch_history = Some(VaultHistory::From(946_000));
        app.cache.daemon_cache.blockheight = 970_500;
        app.cache.last_poll_at_startup = Some(100);
        app.cache.daemon_cache.last_poll_timestamp = Some(200);
        let node_page = Menu::Vault(menu::VaultSubMenu::Settings(Some(
            menu::SettingsOption::Node,
        )));
        drop(app.set_current_panel(node_page.clone()));
        drop(app.update(Message::View(view::Message::Settings(
            view::SettingsMessage::NodeSettings(view::NodeSettingsMessage::SwitchToBitcoind),
        ))));
        assert!(app.cache.node_history_switch_requested);
        drop(app.set_current_panel(Menu::Vault(menu::VaultSubMenu::Transactions(None))));
        drop(app.set_current_panel(node_page));
        assert!(
            app.cache.node_history_switch_requested,
            "page recreation lost consent"
        );
        let progress = |app: &App, history| Message::BitcoindSyncProgress {
            app: app.cache.app_generation,
            history_epoch: app.node_history_epoch,
            wallet: app.wallet.as_ref().map(|w| w.id()),
            config: pending.clone(),
            result: Ok(LocalNodeSync {
                progress: 1.0,
                ibd: false,
                blocks: 970_500,
                headers: 970_500,
                subversion: None,
                pruning: Some(NodePruning::Pruned {
                    prune_height: 970_000,
                }),
            }),
            vault_history: history,
        };
        drop(app.update(progress(&app, Some(VaultHistory::TrackedLocally))));
        assert!(!app.daemon_switch_in_progress, "wait for reconciliation");
        app.cache.node_history.reconciled = true;
        drop(app.update(progress(&app, None)));
        assert!(
            !app.daemon_switch_in_progress,
            "wait for fresh history permission"
        );
        drop(app.update(progress(&app, Some(VaultHistory::TrackedLocally))));
        assert!(
            app.daemon_switch_in_progress,
            "explicit parked-node switch was not completed; hold={:?}",
            app.local_switch_hold
        );
        app.daemon_switch_in_progress = false;
        app.cache.daemon_switch_in_progress = false;
        drop(app.update(Message::View(view::Message::Settings(
            view::SettingsMessage::NodeSettings(view::NodeSettingsMessage::RecoveryDismiss),
        ))));
        assert!(!app.cache.node_history_switch_requested);
        assert!(app.auto_switch_suppressed);
        drop(app.update(progress(&app, Some(VaultHistory::TrackedLocally))));
        assert!(
            !app.daemon_switch_in_progress,
            "dismissal must withdraw promotion consent"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cancel_during_history_batch_is_queued_and_acknowledged() {
        let root = std::env::temp_dir().join(format!("history-cancel-{}", uuid::Uuid::new_v4()));
        let _guard = crate::app::session::test_guard();
        let (mut app, _) = super::shell_tests::bitcoin_app(&root);
        app.node_history_in_progress = true;
        app.cache.node_history.busy = true;

        drop(app.update(Message::NodeHistory(node_history::Action::Cancel)));

        assert!(matches!(
            app.node_history_pending,
            Some(node_history::Action::Cancel)
        ));
        assert!(app.cache.node_history.cancel_queued);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn background_history_poll_does_not_mark_recovery_busy() {
        let root = std::env::temp_dir().join(format!("history-poll-{}", uuid::Uuid::new_v4()));
        let _guard = crate::app::session::test_guard();
        let (mut app, _) = super::shell_tests::bitcoin_app(&root);
        let config = node(8333);
        let mut daemon_config = app.daemon.as_ref().unwrap().config().unwrap().clone();
        daemon_config.pending_bitcoind = Some(config.clone());
        app.daemon = Some(Arc::new(EmbeddedDaemon::unstarted_for_test(
            daemon_config,
            None,
        )));
        app.internal_bitcoind = Some(Bitcoind::for_test(
            config,
            &app.cache.datadir_path,
            app.cache.network,
        ));
        let error = Some("Cannot reach local node for getblockhash".to_string());
        app.cache.node_history.error = error.clone();

        drop(app.update(Message::NodeHistory(node_history::Action::Poll)));
        assert!(app.node_history_in_progress, "the poll must still run");
        assert!(!app.cache.node_history.busy);
        assert_eq!(app.cache.node_history.error, error);

        // A user action still disables the controls and clears the error.
        app.node_history_in_progress = false;
        drop(app.update(Message::NodeHistory(node_history::Action::Pause)));
        assert!(app.cache.node_history.busy);
        assert!(app.cache.node_history.error.is_none());
        drop(app);
        assert!(crate::node::bitcoind::wait_for_managed_node_cleanup(
            std::time::Duration::from_secs(120)
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn current_history_completion_releases_busy_state_when_daemon_disappears() {
        let root =
            std::env::temp_dir().join(format!("history-completion-{}", uuid::Uuid::new_v4()));
        let _guard = crate::app::session::test_guard();
        let (mut app, _) = super::shell_tests::bitcoin_app(&root);
        let wallet = app.wallet.as_ref().unwrap().id();
        app.node_history_request_epoch = 2;
        app.node_history_in_progress = true;
        app.cache.node_history.busy = true;
        app.node_history_pending = Some(node_history::Action::Poll);
        app.daemon = None;
        drop(app.update(Message::NodeHistoryCompleted {
            request_epoch: 2,
            app: app.cache.app_generation,
            wallet,
            config: node(8333),
            result: Err("stale context".into()),
        }));
        assert!(!app.node_history_in_progress);
        assert!(!app.cache.node_history.busy);
        assert!(app.node_history_pending.is_none());
        assert!(app.cache.node_history.error.is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn history_node_replacement_refreshes_open_settings_owner() {
        let root =
            std::env::temp_dir().join(format!("history-settings-owner-{}", uuid::Uuid::new_v4()));
        let _guard = crate::app::session::test_guard();
        let (mut app, _) = super::shell_tests::bitcoin_app(&root);
        let config = node(8333);
        let mut daemon_config = app.daemon.as_ref().unwrap().config().unwrap().clone();
        daemon_config.pending_bitcoind = Some(config.clone());
        app.daemon = Some(Arc::new(EmbeddedDaemon::unstarted_for_test(
            daemon_config,
            None,
        )));
        let prior = Bitcoind::for_test(config.clone(), &app.cache.datadir_path, app.cache.network);
        let prior_path = prior.owner_lock_path().unwrap().to_path_buf();
        app.panels
            .vault_settings
            .as_mut()
            .unwrap()
            .set_managed_owner(Some(prior_path.clone()));
        app.internal_bitcoind = Some(prior);
        let replacement =
            Bitcoind::for_test(config.clone(), &app.cache.datadir_path, app.cache.network);
        let replacement_path = replacement.owner_lock_path().unwrap().to_path_buf();
        assert_ne!(prior_path, replacement_path);
        app.node_history_request_epoch = 1;
        let wallet = app.wallet.as_ref().unwrap().id();
        drop(app.update(Message::NodeHistoryCompleted {
            request_epoch: 1,
            app: app.cache.app_generation,
            wallet,
            config,
            result: Ok(Box::new(node_history::Outcome {
                status: node_history::Status::default(),
                node: Some(replacement),
            })),
        }));
        assert_eq!(
            app.panels
                .vault_settings
                .as_ref()
                .unwrap()
                .managed_owner_path(),
            Some(replacement_path)
        );
        drop(app);
        assert!(crate::node::bitcoind::wait_for_managed_node_cleanup(
            std::time::Duration::from_secs(120)
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn superseded_history_completion_cannot_release_a_new_worker() {
        let root = std::env::temp_dir().join(format!("history-epoch-{}", uuid::Uuid::new_v4()));
        let _guard = crate::app::session::test_guard();
        let (mut app, _) = super::shell_tests::bitcoin_app(&root);
        let wallet = app.wallet.as_ref().unwrap().id();
        app.node_history_request_epoch = 3;
        app.node_history_in_progress = true;
        app.cache.node_history.busy = true;
        app.node_history_pending = Some(node_history::Action::Poll);
        drop(app.update(Message::NodeHistoryCompleted {
            request_epoch: 2,
            app: app.cache.app_generation,
            wallet,
            config: node(8333),
            result: Err("obsolete worker".into()),
        }));
        assert!(app.node_history_in_progress);
        assert!(app.cache.node_history.busy);
        assert!(app.node_history_pending.is_some());
        assert!(app.cache.node_history.error.is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_probe_started_before_recovery_cannot_restore_history_permission() {
        let root = std::env::temp_dir().join(format!("recovery-probe-{}", uuid::Uuid::new_v4()));
        let _guard = crate::app::session::test_guard();
        let (mut app, _) = super::shell_tests::bitcoin_app(&root);
        let pending = node(8333);
        let mut config = app.daemon.as_ref().unwrap().config().unwrap().clone();
        config.pending_bitcoind = Some(pending.clone());
        config.auto_switch_to_pending = Some(true);
        app.daemon = Some(Arc::new(EmbeddedDaemon::unstarted_for_test(config, None)));
        app.cache.last_poll_at_startup = Some(100);
        app.cache.daemon_cache.last_poll_timestamp = Some(200);
        app.node_history_epoch = 1;
        drop(app.update(Message::BitcoindSyncProgress {
            app: app.cache.app_generation,
            history_epoch: 0,
            wallet: app.wallet.as_ref().map(|wallet| wallet.id()),
            config: pending,
            result: Ok(LocalNodeSync {
                progress: 1.0,
                ibd: false,
                blocks: 1000,
                headers: 1000,
                subversion: None,
                pruning: Some(local_switch::NodePruning::Pruned { prune_height: 900 }),
            }),
            vault_history: Some(local_switch::VaultHistory::TrackedLocally),
        }));
        assert_eq!(app.cache.local_switch_history, None);
        assert!(!app.daemon_switch_in_progress);
        let _ = std::fs::remove_dir_all(root);
    }

    /// A full scan started mid-session — a user rescan, or the forced full scan
    /// after a `CannotConnect` — after the Vault has already polled: the "no poll
    /// this session" rule no longer applies, so only the daemon's `history_sync`
    /// report shows the scan. The local-node switch must wait for it.
    #[test]
    fn a_full_scan_started_mid_session_counts_as_running() {
        use coincubed::commands::HistorySync;
        let esplora = DaemonBackend::EmbeddedCoincubed(Some(NodeType::Esplora));
        let mut cache = Cache {
            last_poll_at_startup: Some(1_000),
            ..Cache::default()
        };
        cache.daemon_cache.blockheight = 970_500;
        cache.daemon_cache.last_poll_timestamp = Some(2_000);
        assert_eq!(running_vault_scan(esplora.clone(), &cache), None);

        cache.daemon_cache.history_sync = HistorySync {
            full_scan_in_progress: true,
            addresses_checked: 10,
            addresses_expected: 402,
            ..Default::default()
        };
        assert_eq!(
            running_vault_scan(esplora, &cache),
            Some(local_switch::RunningScan::WalletSync)
        );
    }

    fn node(port: u16) -> BitcoindConfig {
        BitcoindConfig {
            addr: std::net::SocketAddr::from(([127, 0, 0, 1], port)),
            rpc_auth: BitcoindRpcAuth::UserPass("fixture".into(), "fixture".into()),
        }
    }

    #[tokio::test]
    async fn local_node_sync_reads_pruned_node_without_connect() {
        use httpmock::prelude::*;
        let server = MockServer::start_async().await;
        let chain = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/")
                    .json_body_partial(r#"{"method":"getblockchaininfo"}"#);
                then.status(200).json_body(serde_json::json!({"result": {
                    "verificationprogress": 0.975, "initialblockdownload": true,
                    "blocks": 890000, "headers": 900000, "pruned": true
                }}));
            })
            .await;
        let build = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/")
                    .json_body_partial(r#"{"method":"getnetworkinfo"}"#);
                then.status(200)
                    .json_body(serde_json::json!({"result":{"subversion":"/Satoshi:29.3.0/"}}));
            })
            .await;
        let observed = check_bitcoind_sync_progress(node(server.address().port()))
            .await
            .unwrap();
        assert_eq!(observed.progress, 0.975);
        assert!(observed.ibd);
        assert_eq!((observed.blocks, observed.headers), (890000, 900000));
        assert_eq!(observed.subversion.as_deref(), Some("/Satoshi:29.3.0/"));
        chain.assert_async().await;
        build.assert_async().await;
    }

    #[tokio::test]
    async fn local_node_sync_survives_switch_and_rejects_previous_node() {
        let root = std::env::temp_dir().join(format!("local-node-sync-{}", uuid::Uuid::new_v4()));
        let (mut app, _) = super::shell_tests::bitcoin_app(&root);
        let mut cfg = app.daemon.as_ref().unwrap().config().unwrap().clone();
        let active = node(8332);
        let pending = node(8333);
        cfg.bitcoin_backend = Some(BitcoinBackend::Bitcoind(active.clone()));
        assert_eq!(local_node_sync_config(&cfg), Some(&active));
        cfg.pending_bitcoind = Some(pending.clone());
        cfg.auto_switch_to_pending = Some(false);
        assert_eq!(local_node_sync_config(&cfg), Some(&pending));
        app.daemon = Some(Arc::new(EmbeddedDaemon::unstarted_for_test(
            cfg.clone(),
            None,
        )));
        app.daemon_switch_in_progress = true;
        app.cache.daemon_switch_in_progress = true;
        let observation = || LocalNodeSync {
            progress: 0.999,
            ibd: false,
            blocks: 900000,
            headers: 900000,
            subversion: None,
            pruning: Some(local_switch::NodePruning::Unpruned),
        };
        drop(app.update(Message::BitcoindSyncProgress {
            app: app.cache.app_generation,
            history_epoch: app.node_history_epoch,
            wallet: app.wallet.as_ref().map(|w| w.id()),
            config: pending.clone(),
            result: Ok(observation()),
            vault_history: None,
        }));
        assert_eq!(app.cache.node_bitcoind_sync_heights, Some((900000, 900000)));
        assert_eq!(app.cache.node_bitcoind_ibd, Some(false));
        // Promotion removes pending, but the same RPC remains the active observer.
        cfg.pending_bitcoind = None;
        cfg.bitcoin_backend = Some(BitcoinBackend::Bitcoind(pending.clone()));
        assert_eq!(local_node_sync_config(&cfg), Some(&pending));
        app.bitcoind_sync_probe_in_progress = true;
        drop(
            app.update(Message::DaemonRestarted(DaemonRestart::Started(Arc::new(
                EmbeddedDaemon::unstarted_for_test(cfg, None),
            )))),
        );
        assert!(
            !app.bitcoind_sync_probe_in_progress,
            "old probe blocks replacement"
        );
        // A new probe can start; a delayed old-node result cannot release it.
        app.bitcoind_sync_probe_in_progress = true;
        drop(app.update(Message::BitcoindSyncProgress {
            app: app.cache.app_generation,
            history_epoch: app.node_history_epoch,
            wallet: app.wallet.as_ref().map(|w| w.id()),
            config: active,
            result: Ok(observation()),
            vault_history: None,
        }));
        assert_eq!(app.cache.node_bitcoind_sync_heights, None);
        assert!(
            app.bitcoind_sync_probe_in_progress,
            "stale result released fresh probe"
        );
        drop(app.update(Message::BitcoindSyncProgress {
            app: app.cache.app_generation,
            history_epoch: app.node_history_epoch,
            wallet: app.wallet.as_ref().map(|w| w.id()),
            config: pending,
            result: Ok(observation()),
            vault_history: None,
        }));
        assert_eq!(app.cache.node_bitcoind_sync_heights, Some((900000, 900000)));
    }

    #[test]
    fn local_node_history_result_cannot_authorize_a_different_app_on_the_same_node() {
        let root =
            std::env::temp_dir().join(format!("stale-local-history-{}", uuid::Uuid::new_v4()));
        let _guard = crate::app::session::test_guard();
        let (previous, _) = super::shell_tests::bitcoin_app(&root);
        let (mut current, _) = super::shell_tests::bitcoin_app(&root);
        assert_ne!(previous.cache.app_generation, current.cache.app_generation);
        let mut cfg = current.daemon.as_ref().unwrap().config().unwrap().clone();
        let pending = node(8333);
        cfg.pending_bitcoind = Some(pending.clone());
        cfg.auto_switch_to_pending = Some(true);
        current.daemon = Some(Arc::new(EmbeddedDaemon::unstarted_for_test(cfg, None)));
        current.cache.last_poll_at_startup = Some(100);
        current.cache.daemon_cache.last_poll_timestamp = Some(200);
        current.bitcoind_sync_probe_in_progress = true;
        drop(current.update(Message::BitcoindSyncProgress {
            app: previous.cache.app_generation,
            history_epoch: previous.node_history_epoch,
            wallet: previous.wallet.as_ref().map(|w| w.id()),
            config: pending,
            result: Ok(LocalNodeSync {
                progress: 1.0,
                ibd: false,
                blocks: 970_500,
                headers: 970_500,
                subversion: None,
                pruning: Some(local_switch::NodePruning::Pruned {
                    prune_height: 969_938,
                }),
            }),
            vault_history: Some(local_switch::VaultHistory::TrackedLocally),
        }));
        assert!(!current.daemon_switch_in_progress);
        assert!(current.bitcoind_sync_probe_in_progress);
        assert_eq!(current.cache.local_switch_history, None);
        assert_eq!(current.cache.node_bitcoind_pruning, None);
        let _ = std::fs::remove_dir_all(root);
    }

    /// The automatic promotion of a synced pending node waits for a running
    /// Vault scan instead of discarding it, refuses a node pruned past the
    /// Vault's coins, and goes ahead once neither applies.
    #[test]
    fn auto_switch_defers_for_scans_and_refuses_pruned_history() {
        use local_switch::{NodePruning, PrunedHistory, RunningScan, SwitchHold, VaultHistory};

        let root = std::env::temp_dir().join(format!("local-switch-{}", uuid::Uuid::new_v4()));
        let _guard = crate::app::session::test_guard();
        let (mut app, _) = super::shell_tests::bitcoin_app(&root);
        let mut cfg = app.daemon.as_ref().unwrap().config().unwrap().clone();
        let pending = node(8333);
        cfg.pending_bitcoind = Some(pending.clone());
        cfg.auto_switch_to_pending = Some(true);
        app.daemon = Some(Arc::new(EmbeddedDaemon::unstarted_for_test(cfg, None)));
        let synced = |pruning| LocalNodeSync {
            progress: 1.0,
            ibd: false,
            blocks: 970_500,
            headers: 970_500,
            subversion: None,
            pruning: Some(pruning),
        };
        let pruned = NodePruning::Pruned {
            prune_height: 969_938,
        };

        // The Connect Vault has not finished its first poll of the session —
        // its startup scan is still running.
        app.cache.daemon_cache.blockheight = 970_500;
        app.cache.last_poll_at_startup = Some(100);
        app.cache.daemon_cache.last_poll_timestamp = Some(100);
        drop(app.update(Message::BitcoindSyncProgress {
            app: app.cache.app_generation,
            history_epoch: app.node_history_epoch,
            wallet: app.wallet.as_ref().map(|w| w.id()),
            config: pending.clone(),
            result: Ok(synced(NodePruning::Unpruned)),
            vault_history: None,
        }));
        assert!(!app.daemon_switch_in_progress, "switched mid-scan");
        assert_eq!(
            app.local_switch_hold,
            Some(SwitchHold::Scanning(RunningScan::WalletSync))
        );

        // Scan done, but the node is pruned past the Vault's earliest coin.
        app.cache.daemon_cache.last_poll_timestamp = Some(200);
        drop(app.update(Message::BitcoindSyncProgress {
            app: app.cache.app_generation,
            history_epoch: app.node_history_epoch,
            wallet: app.wallet.as_ref().map(|w| w.id()),
            config: pending.clone(),
            result: Ok(synced(pruned)),
            vault_history: Some(VaultHistory::From(958_601)),
        }));
        assert!(!app.daemon_switch_in_progress, "switched to a pruned node");
        assert_eq!(
            app.local_switch_hold,
            Some(SwitchHold::Pruned(PrunedHistory::CoinsBelowPrune {
                prune_height: 969_938,
                earliest: 958_601,
            }))
        );
        assert_eq!(app.cache.node_bitcoind_pruning, Some(pruned));
        assert_eq!(
            app.cache.local_switch_history,
            Some(VaultHistory::From(958_601))
        );

        // History not readable this probe: wait, don't guess.
        drop(app.update(Message::BitcoindSyncProgress {
            app: app.cache.app_generation,
            history_epoch: app.node_history_epoch,
            wallet: app.wallet.as_ref().map(|w| w.id()),
            config: pending.clone(),
            result: Ok(synced(pruned)),
            vault_history: None,
        }));
        assert!(!app.daemon_switch_in_progress);
        assert_eq!(app.local_switch_hold, Some(SwitchHold::Unknown));

        // Its existing wallet records the history even though the blocks are gone.
        drop(app.update(Message::BitcoindSyncProgress {
            app: app.cache.app_generation,
            history_epoch: app.node_history_epoch,
            wallet: app.wallet.as_ref().map(|w| w.id()),
            config: pending,
            result: Ok(synced(pruned)),
            vault_history: Some(VaultHistory::TrackedLocally),
        }));
        assert!(app.daemon_switch_in_progress, "deferred switch never fired");
        assert_eq!(app.local_switch_hold, None);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A Connect Vault's first-ever full scan: no tip in the database yet
    /// (`blockheight` 0, which `sync_status` reports as `Synced` for Esplora)
    /// and no poll completed this session. The promotion must still wait.
    #[test]
    fn auto_switch_waits_for_a_connect_vaults_first_scan() {
        use local_switch::{NodePruning, RunningScan, SwitchHold};

        let root = std::env::temp_dir().join(format!("local-switch-{}", uuid::Uuid::new_v4()));
        let _guard = crate::app::session::test_guard();
        let (mut app, _) = super::shell_tests::bitcoin_app(&root);
        let mut cfg = app.daemon.as_ref().unwrap().config().unwrap().clone();
        assert!(matches!(
            cfg.bitcoin_backend,
            Some(BitcoinBackend::Esplora(_))
        ));
        let pending = node(8333);
        cfg.pending_bitcoind = Some(pending.clone());
        cfg.auto_switch_to_pending = Some(true);
        app.daemon = Some(Arc::new(EmbeddedDaemon::unstarted_for_test(cfg, None)));
        app.cache.daemon_cache.blockheight = 0;
        app.cache.daemon_cache.last_poll_timestamp = None;
        app.cache.daemon_cache.rescan_progress = None;
        app.cache.last_poll_at_startup = None;
        drop(app.update(Message::BitcoindSyncProgress {
            app: app.cache.app_generation,
            history_epoch: app.node_history_epoch,
            wallet: app.wallet.as_ref().map(|w| w.id()),
            config: pending,
            result: Ok(LocalNodeSync {
                progress: 1.0,
                ibd: false,
                blocks: 970_500,
                headers: 970_500,
                subversion: None,
                pruning: Some(NodePruning::Unpruned),
            }),
            vault_history: None,
        }));
        assert!(!app.daemon_switch_in_progress, "switched mid first scan");
        assert_eq!(
            app.local_switch_hold,
            Some(SwitchHold::Scanning(RunningScan::WalletSync))
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod wallet_recent_activity_tests {
    use super::*;

    fn receipt(id: &str) -> coincube_spark_protocol::PaymentSummary {
        coincube_spark_protocol::PaymentSummary {
            id: id.into(),
            amount_sat: 1_500,
            fees_sat: 0,
            token_amount: None,
            token_decimals: None,
            token_ticker: None,
            timestamp: 1_760_000_000,
            status: "completed".into(),
            direction: "receive".into(),
            method: "lightning".into(),
            description: Some("Synthetic receipt".into()),
            details: Default::default(),
        }
    }

    /// A Vault payment to an outside address (`view::activity` tests' shape).
    fn vault_payment() -> crate::daemon::model::HistoryTransaction {
        use crate::daemon::model::Coin;
        use coincube_core::miniscript::bitcoin::{
            absolute, bip32, hashes::Hash, transaction, Address, Amount, Network, OutPoint,
            ScriptBuf, Transaction, TxIn, TxOut, Txid, WScriptHash,
        };
        let script = ScriptBuf::new_p2wsh(&WScriptHash::all_zeros());
        let previous_output = OutPoint {
            txid: Txid::all_zeros(),
            vout: 1,
        };
        let coin = Coin {
            amount: Amount::from_sat(5_000_000),
            outpoint: previous_output,
            address: Address::from_script(&script, Network::Regtest).unwrap(),
            block_height: Some(1),
            derivation_index: bip32::ChildNumber::from(0),
            spend_info: None,
            is_immature: false,
            is_change: false,
            is_from_self: false,
        };
        let tx = Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: vec![TxIn {
                previous_output,
                ..Default::default()
            }],
            output: vec![
                TxOut {
                    value: Amount::from_sat(1_000_000),
                    script_pubkey: script,
                },
                TxOut {
                    value: Amount::from_sat(3_999_000),
                    script_pubkey: ScriptBuf::new_p2wsh(&WScriptHash::from_byte_array([1; 32])),
                },
            ],
        };
        crate::daemon::model::HistoryTransaction::new(
            tx,
            Some(103),
            Some(1_760_000_000),
            vec![coin],
            vec![0],
            Network::Regtest,
        )
    }

    #[test]
    fn late_activity_results_do_not_navigate_or_replace_current_rows() {
        let _guard = crate::app::session::test_guard();
        let root =
            std::env::temp_dir().join(format!("activity-late-routing-{}", uuid::Uuid::new_v4()));
        let (mut app, _) = super::shell_tests::bitcoin_app(&root);
        let route = Menu::Cube(menu::CubeSubMenu::Settings(menu::CubeSettingsOption::About));
        app.panels.current = route.clone();
        let generation = app.panels.activity.generation_for_tests();
        drop(app.update(Message::ActivitySparkLoaded(
            generation,
            Ok((vec![receipt("current")], false)),
        )));
        let current_ids = app
            .panels
            .activity
            .wallet_entries(view::activity::WalletFilter::Spark)
            .iter()
            .map(|entry| entry.id.clone())
            .collect::<Vec<_>>();
        assert_eq!(current_ids.len(), 1);
        let stale = generation.wrapping_add(1);
        for message in [
            Message::ActivitySparkLoaded(stale, Ok((vec![receipt("stale")], false))),
            Message::ActivitySparkRefreshed(stale, Ok((Vec::new(), false))),
            Message::ActivityVaultLoaded(stale, Err("late vault failure".into())),
            Message::ActivityPsbtLoaded(stale, Err("late PSBT failure".into())),
            Message::ActivityLiquidLoaded(stale, Err("late Liquid failure".into())),
            Message::ActivityRefundablesLoaded(stale, Err("late refund failure".into())),
            Message::ActivityLabelSaved(u64::MAX, Err("late label failure".into())),
        ] {
            drop(app.update(message));
            assert_eq!(app.panels.current, route);
            assert_eq!(
                app.panels
                    .activity
                    .wallet_entries(view::activity::WalletFilter::Spark)
                    .iter()
                    .map(|entry| entry.id.clone())
                    .collect::<Vec<_>>(),
                current_ids
            );
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn activity_detail_without_a_vault_panel_keeps_the_current_route() {
        let _guard = crate::app::session::test_guard();
        let root =
            std::env::temp_dir().join(format!("activity-missing-panel-{}", uuid::Uuid::new_v4()));
        let (mut app, _) = super::shell_tests::bitcoin_app(&root);
        let route = Menu::Cube(menu::CubeSubMenu::Settings(menu::CubeSettingsOption::About));
        app.panels.current = route.clone();
        app.panels.transactions = None;
        let task = app.update(Message::ActivityOpenDetail(
            state::activity::ActivityTarget::Vault(vault_payment()),
        ));
        assert_eq!(task.units(), 0);
        assert_eq!(app.panels.current, route);
        let _ = std::fs::remove_dir_all(root);
    }

    /// The Vault page's own payment refresh re-reads Activity's Vault rows
    /// there and nowhere else; a request made while a read is in flight is
    /// served once that read lands; a wallet page's Retry keeps other rows.
    #[test]
    fn vault_page_refreshes_its_rows_without_dropping_a_request() {
        let _guard = crate::app::session::test_guard();
        let root = std::env::temp_dir().join(format!("recent-refresh-{}", uuid::Uuid::new_v4()));
        let (mut app, _) = super::shell_tests::bitcoin_app(&root);

        app.panels.current = Menu::Cube(menu::CubeSubMenu::Overview);
        // The Overview's own first read (`App::new`) lands first.
        assert!(
            app.panels.activity.vault_loading,
            "the Overview reads on open"
        );
        let generation = app.panels.activity.generation_for_tests();
        drop(app.update(Message::ActivityVaultLoaded(
            generation,
            Ok((Vec::new(), false)),
        )));
        drop(app.update(Message::Payments(Ok(Vec::new()))));
        assert!(!app.panels.activity.vault_loading, "off the Vault page");

        app.panels.current = Menu::Vault(menu::VaultSubMenu::Overview);
        drop(app.update(Message::Payments(Ok(Vec::new()))));
        assert!(app.panels.activity.vault_loading, "on the Vault page");

        // Asked again mid-read: no second read now, one when the first lands.
        let again = app.panels.activity.refresh_wallet(
            view::activity::WalletFilter::Vault,
            app.daemon.clone(),
            None,
        );
        assert_eq!(again.units(), 0);
        let generation = app.panels.activity.generation_for_tests();
        let followup = app.panels.activity.update(
            app.daemon.clone(),
            &app.cache,
            Message::ActivityVaultLoaded(generation, Ok((Vec::new(), false))),
        );
        assert!(followup.units() > 0, "the stale request is served");
        assert!(app.panels.activity.vault_loading);
        let settled = app.panels.activity.update(
            app.daemon.clone(),
            &app.cache,
            Message::ActivityVaultLoaded(generation, Ok((vec![vault_payment()], false))),
        );
        assert_eq!(settled.units(), 0, "served once");

        // Retry on the Vault page re-reads the Vault and keeps Spark's rows.
        drop(app.update(Message::ActivitySparkLoaded(
            generation,
            Ok((vec![receipt("recv-2")], false)),
        )));
        drop(app.update(Message::View(view::Message::Activity(
            view::activity::ActivityMessage::RetryWallet(view::activity::WalletFilter::Vault),
        ))));
        assert!(app.panels.activity.vault_loading);
        assert_eq!(app.panels.activity.generation_for_tests(), generation);
        assert_eq!(
            app.panels
                .activity
                .wallet_entries(view::activity::WalletFilter::Spark)
                .len(),
            1
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// An Overview row opens Activity unfiltered with that row selected, by
    /// the row's own id: a filter or a selection left from an earlier visit
    /// can't hide it or open another row, and the selection holds while
    /// Activity reloads.
    #[test]
    fn an_overview_row_opens_activity_unfiltered_on_that_row() {
        let _guard = crate::app::session::test_guard();
        let root = std::env::temp_dir().join(format!("recent-open-{}", uuid::Uuid::new_v4()));
        let (mut app, _) = super::shell_tests::bitcoin_app(&root);
        app.panels.current = Menu::Cube(menu::CubeSubMenu::Overview);
        let at = |n: u64, id: &str| coincube_spark_protocol::PaymentSummary {
            timestamp: 1_760_000_000 + n,
            description: Some(format!("Receipt {id}")),
            ..receipt(id)
        };
        let rows = || vec![at(3, "newest"), at(2, "chosen"), at(1, "older")];
        let generation = app.panels.activity.generation_for_tests();
        drop(app.update(Message::ActivitySparkLoaded(
            generation,
            Ok((rows(), false)),
        )));
        let entries = app
            .panels
            .activity
            .recent_entries(view::activity::RecentScope::Cube);
        let chosen = entries[1].id.clone();
        let stale = entries[0].id.clone();
        // Left from an earlier Activity visit.
        app.panels.activity.filters.wallet = view::activity::WalletFilter::Vault;
        app.panels.activity.filters.search = "rent".into();
        app.panels.activity.selected_id = Some(stale.clone());

        drop(app.update(Message::View(view::Message::Activity(
            view::activity::ActivityMessage::Open(chosen.clone()),
        ))));
        assert_eq!(app.panels.current, Menu::Cube(menu::CubeSubMenu::Activity));
        let filters = &app.panels.activity.filters;
        assert_eq!(filters.wallet, view::activity::WalletFilter::All);
        assert!(filters.search.is_empty() && filters.date_range.is_empty());
        assert_eq!(filters.direction, view::activity::DirectionFilter::All);
        assert_eq!(filters.status, view::activity::StatusFilter::All);
        assert_eq!(
            app.panels.activity.selected_id.as_deref(),
            Some(chosen.as_str())
        );
        // Activity reloads on entry; once its rows land, in another order,
        // the pane is the chosen row's.
        let generation = app.panels.activity.generation_for_tests();
        let mut reordered = rows();
        reordered.reverse();
        drop(app.update(Message::ActivitySparkLoaded(
            generation,
            Ok((reordered, false)),
        )));
        assert_eq!(
            app.panels.activity.selected_id.as_deref(),
            Some(chosen.as_str())
        );
        let page = view::shared::landing::capture::inspect_filled(
            "overview-open-in-activity",
            view::activity::render(&app.panels.activity, &app.cache),
            (1440, 900),
            "dark",
            coincube_ui::theme::Theme::dark(),
            (1192, 835),
        );
        assert!(page.has_focus(iced::advanced::widget::Id::from(
            view::activity::DETAIL_CLOSE_ID
        )));
        let title_count =
            |title: &str| page.text.iter().filter(|(value, _)| value == title).count();
        assert_eq!(title_count("Receipt chosen"), 2, "its row and its pane");
        assert_eq!(title_count("Receipt newest"), 1, "its row only");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A row the Overview opens keeps its detail pane when Activity's own
    /// read fails: the switch's reload clears the rows, so the pane shows the
    /// row as the Overview listed it, beside the list and in the compact
    /// layout alike.
    #[test]
    fn an_overview_row_keeps_its_pane_when_activitys_read_fails() {
        let _guard = crate::app::session::test_guard();
        let root = std::env::temp_dir().join(format!("recent-failed-{}", uuid::Uuid::new_v4()));
        let (mut app, _) = super::shell_tests::bitcoin_app(&root);
        app.panels.current = Menu::Cube(menu::CubeSubMenu::Overview);
        let at = |n: u64, id: &str| coincube_spark_protocol::PaymentSummary {
            timestamp: 1_760_000_000 + n,
            description: Some(format!("Receipt {id}")),
            ..receipt(id)
        };
        let generation = app.panels.activity.generation_for_tests();
        drop(app.update(Message::ActivitySparkLoaded(
            generation,
            Ok((vec![at(2, "chosen"), at(1, "older")], false)),
        )));
        let chosen = app
            .panels
            .activity
            .recent_entries(view::activity::RecentScope::Cube)
            .into_iter()
            .find(|entry| entry.title == "Receipt chosen")
            .unwrap()
            .id;
        drop(app.update(Message::View(view::Message::Activity(
            view::activity::ActivityMessage::Open(chosen.clone()),
        ))));
        let generation = app.panels.activity.generation_for_tests();
        drop(app.update(Message::ActivitySparkLoaded(
            generation,
            Err("bridge offline".into()),
        )));
        assert!(app.panels.activity.entries().is_empty(), "the read failed");
        for (outer, pane) in [((1440, 900), (1192, 835)), ((1024, 768), (816, 703))] {
            let page = view::shared::landing::capture::inspect_filled(
                "overview-open-failed-read",
                view::activity::render(&app.panels.activity, &app.cache),
                outer,
                "dark",
                coincube_ui::theme::Theme::dark(),
                pane,
            );
            let titles = page
                .text
                .iter()
                .filter(|(value, _)| value == "Receipt chosen")
                .count();
            assert_eq!(titles, 1, "its pane at {:?}", outer);
            assert!(page.has_focus(iced::advanced::widget::Id::from(
                view::activity::DETAIL_CLOSE_ID
            )));
        }
        // Once a read succeeds the fresh row takes over.
        drop(app.update(Message::View(view::Message::Activity(
            view::activity::ActivityMessage::Retry,
        ))));
        let generation = app.panels.activity.generation_for_tests();
        drop(app.update(Message::ActivitySparkLoaded(
            generation,
            Ok((vec![at(2, "chosen"), at(1, "older")], false)),
        )));
        assert_eq!(
            app.panels.activity.selected_id.as_deref(),
            Some(chosen.as_str())
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// While the Vault's history is unknown (its first full scan still
    /// running), the Overview's empty "Recent activity" says so, as the Vault
    /// page does, instead of "No transactions yet"; once known, it doesn't.
    #[test]
    fn the_overview_never_reads_an_unknown_vault_history_as_empty() {
        let _guard = crate::app::session::test_guard();
        let root = std::env::temp_dir().join(format!("recent-unknown-{}", uuid::Uuid::new_v4()));
        let (mut app, _) = super::shell_tests::bitcoin_app(&root);
        app.panels.current = Menu::Cube(menu::CubeSubMenu::Overview);
        let generation = app.panels.activity.generation_for_tests();
        drop(app.update(Message::ActivityVaultLoaded(
            generation,
            Ok((Vec::new(), false)),
        )));
        drop(app.update(Message::ActivitySparkLoaded(
            generation,
            Ok((Vec::new(), false)),
        )));
        let note = "Transactions appear once the Vault's history has loaded";
        let page = |app: &App| {
            crate::app::view::shared::landing::capture::inspect(
                "overview-recent-unknown",
                app.view(),
                (1440, 900),
                "dark",
                coincube_ui::theme::Theme::dark(),
            )
        };
        let wallet = app.wallet.clone().unwrap();
        for (sync, unknown) in [
            (
                SyncStatus::WalletFullScan {
                    progress: Some(0.4),
                },
                true,
            ),
            (SyncStatus::Synced, false),
        ] {
            app.panels.vault_overview =
                Some(VaultOverview::new(wallet.clone(), &[], sync, 0, false));
            let shown = page(&app);
            assert!(shown.contains("Recent activity"));
            assert_eq!(shown.contains(note), unknown);
            assert_eq!(shown.contains("No transactions yet"), !unknown);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The filters judge the copy of an opened row that the pane shows: a
    /// row opened while pending that completes during the reload shows no
    /// pane under the Pending filter, which hides its row.
    #[test]
    fn an_opened_rows_pane_follows_the_filters_on_its_fresh_copy() {
        let _guard = crate::app::session::test_guard();
        let root = std::env::temp_dir().join(format!("recent-filter-{}", uuid::Uuid::new_v4()));
        let (mut app, _) = super::shell_tests::bitcoin_app(&root);
        app.panels.current = Menu::Cube(menu::CubeSubMenu::Overview);
        let row = |status: &str| coincube_spark_protocol::PaymentSummary {
            timestamp: 1_760_000_000,
            status: status.into(),
            description: Some("Receipt chosen".into()),
            ..receipt("chosen")
        };
        let generation = app.panels.activity.generation_for_tests();
        drop(app.update(Message::ActivitySparkLoaded(
            generation,
            Ok((vec![row("pending")], false)),
        )));
        let chosen = app
            .panels
            .activity
            .recent_entries(view::activity::RecentScope::Cube)[0]
            .id
            .clone();
        drop(app.update(Message::View(view::Message::Activity(
            view::activity::ActivityMessage::Open(chosen),
        ))));
        let generation = app.panels.activity.generation_for_tests();
        drop(app.update(Message::ActivitySparkLoaded(
            generation,
            Ok((vec![row("completed")], false)),
        )));
        let titles = |app: &App| {
            view::shared::landing::capture::inspect_filled(
                "overview-open-filtered",
                view::activity::render(&app.panels.activity, &app.cache),
                (1440, 900),
                "dark",
                coincube_ui::theme::Theme::dark(),
                (1192, 835),
            )
            .text
            .iter()
            .filter(|(value, _)| value == "Receipt chosen")
            .count()
        };
        assert_eq!(titles(&app), 2, "its row and its pane");
        drop(app.update(Message::View(view::Message::Activity(
            view::activity::ActivityMessage::Status(view::activity::StatusFilter::Pending),
        ))));
        assert_eq!(titles(&app), 0, "neither under Pending");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A row the Overview opens gets Activity's detail pane even when it sits
    /// past the complete-history cutoff, which hides it from Activity's own
    /// list: here Spark's whole first page shares one timestamp and more pages
    /// remain. A row picked on Activity itself gets no such exception.
    #[test]
    fn an_overview_row_past_the_history_cutoff_still_opens_in_activity() {
        let _guard = crate::app::session::test_guard();
        let root = std::env::temp_dir().join(format!("recent-cutoff-{}", uuid::Uuid::new_v4()));
        let (mut app, _) = super::shell_tests::bitcoin_app(&root);
        app.panels.current = Menu::Cube(menu::CubeSubMenu::Overview);
        let at = |id: &str| coincube_spark_protocol::PaymentSummary {
            timestamp: 1_760_000_000,
            description: Some(format!("Receipt {id}")),
            ..receipt(id)
        };
        let rows = || vec![at("chosen"), at("other")];
        let load = |app: &mut App| {
            let generation = app.panels.activity.generation_for_tests();
            drop(app.update(Message::ActivitySparkLoaded(generation, Ok((rows(), true)))));
        };
        load(&mut app);
        let entries = app
            .panels
            .activity
            .recent_entries(view::activity::RecentScope::Cube);
        let id = |title: &str| {
            entries
                .iter()
                .find(|entry| entry.title == title)
                .unwrap()
                .id
                .clone()
        };
        let (chosen, other) = (id("Receipt chosen"), id("Receipt other"));

        drop(app.update(Message::View(view::Message::Activity(
            view::activity::ActivityMessage::Open(chosen.clone()),
        ))));
        load(&mut app);
        assert_eq!(app.panels.activity.complete_through(), Some(1_760_000_000));
        // Beside the list at 1440, under its place at the 1024 minimum.
        let count_at = |app: &App, title: &str, outer: (u32, u32), pane: (u32, u32)| {
            view::shared::landing::capture::inspect_filled(
                "overview-open-past-cutoff",
                view::activity::render(&app.panels.activity, &app.cache),
                outer,
                "dark",
                coincube_ui::theme::Theme::dark(),
                pane,
            )
            .text
            .iter()
            .filter(|(value, _)| value == title)
            .count()
        };
        let count = |app: &App, title: &str| count_at(app, title, (1440, 900), (1192, 835));
        assert_eq!(count(&app, "Receipt chosen"), 1, "its pane, not a row");
        assert_eq!(count(&app, "Receipt other"), 0);
        assert_eq!(
            count_at(&app, "Receipt chosen", (1024, 768), (816, 703)),
            1,
            "compact: its pane, at the top"
        );

        // Picked on Activity, a row past the cutoff owns no pane.
        drop(app.update(Message::View(view::Message::Activity(
            view::activity::ActivityMessage::Select(other),
        ))));
        assert_eq!(count(&app, "Receipt other"), 0);
        assert_eq!(count(&app, "Receipt chosen"), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The Overview reads Activity's Spark and Vault rows when it opens, when
    /// a Spark event can change Spark's payments, and when a poll moves the
    /// Vault's coins; never because it was drawn again, and never for a
    /// Spark or Vault change while another page is open.
    #[test]
    fn the_overview_reads_its_activity_on_entry_and_wallet_changes_only() {
        use coincube_spark_protocol::Event as SparkEvent;
        let _guard = crate::app::session::test_guard();
        let root = std::env::temp_dir().join(format!("recent-overview-{}", uuid::Uuid::new_v4()));
        let (mut app, _) = super::shell_tests::bitcoin_app(&root);
        let settle_vault = |app: &mut App| {
            let generation = app.panels.activity.generation_for_tests();
            drop(app.update(Message::ActivityVaultLoaded(
                generation,
                Ok((Vec::new(), false)),
            )));
            assert!(!app.panels.activity.vault_loading);
        };
        let spark_event = |app: &mut App, event: SparkEvent| {
            drop(app.update(Message::SparkEvent(
                crate::app::breez_spark::SparkClientEvent(event),
            )));
        };
        // `App::new` opens the Overview and reads both.
        assert_eq!(app.panels.current, Menu::Cube(menu::CubeSubMenu::Overview));
        assert!(app.panels.activity.vault_loading);
        let opened = app.panels.activity.spark_refresh_requests;
        assert!(opened > 0);
        settle_vault(&mut app);

        // Drawn again and again: nothing is read.
        drop(app.view());
        drop(app.view());
        assert!(!app.panels.activity.vault_loading);
        assert_eq!(app.panels.activity.spark_refresh_requests, opened);

        // Opened again from another page: both again.
        drop(
            app.set_current_panel(Menu::Cube(menu::CubeSubMenu::Settings(
                menu::CubeSettingsOption::General,
            ))),
        );
        let away = app.panels.activity.spark_refresh_requests;
        drop(app.set_current_panel(Menu::Cube(menu::CubeSubMenu::Overview)));
        assert_eq!(app.panels.activity.spark_refresh_requests, away + 1);
        assert!(app.panels.activity.vault_loading);
        settle_vault(&mut app);

        // Spark payment events re-read Spark; others don't.
        let before = app.panels.activity.spark_refresh_requests;
        spark_event(
            &mut app,
            SparkEvent::PaymentSucceeded {
                id: "pay-1".into(),
                amount_sat: 1_000,
                bolt11: None,
                token: None,
            },
        );
        assert_eq!(app.panels.activity.spark_refresh_requests, before + 1);
        spark_event(&mut app, SparkEvent::LightningAddressChanged { info: None });
        assert_eq!(app.panels.activity.spark_refresh_requests, before + 1);
        assert!(
            !app.panels.activity.vault_loading,
            "Spark events read Spark"
        );

        // A poll that moves the Vault's coins re-reads the Vault; an
        // unchanged poll doesn't.
        let mut poll = app.cache.daemon_cache.clone();
        drop(app.update(Message::UpdateDaemonCache(Ok(poll.clone()))));
        assert!(!app.panels.activity.vault_loading, "nothing moved");
        poll.coins
            .push(vault_payment().coins.into_values().next().unwrap());
        drop(app.update(Message::UpdateDaemonCache(Ok(poll.clone()))));
        assert!(app.panels.activity.vault_loading, "a coin arrived");
        settle_vault(&mut app);
        // A block with the coins unchanged re-reads too: the cache holds
        // unspent coins only, so a spend with no change output confirms
        // without moving them.
        poll.blockheight += 1;
        drop(app.update(Message::UpdateDaemonCache(Ok(poll.clone()))));
        assert!(app.panels.activity.vault_loading, "a block arrived");
        settle_vault(&mut app);
        // A scan can find only spent history at the same tip: its end re-reads.
        poll.rescan_progress = Some(0.5);
        drop(app.update(Message::UpdateDaemonCache(Ok(poll.clone()))));
        assert!(!app.panels.activity.vault_loading, "a rescan is running");
        poll.rescan_progress = None;
        drop(app.update(Message::UpdateDaemonCache(Ok(poll.clone()))));
        assert!(app.panels.activity.vault_loading, "the rescan finished");
        settle_vault(&mut app);
        poll.history_sync.full_scan_completed_at = Some(1_760_000_000);
        drop(app.update(Message::UpdateDaemonCache(Ok(poll.clone()))));
        assert!(app.panels.activity.vault_loading, "a full scan finished");
        settle_vault(&mut app);

        // On another page neither reads the Overview's rows.
        drop(
            app.set_current_panel(Menu::Cube(menu::CubeSubMenu::Settings(
                menu::CubeSettingsOption::General,
            ))),
        );
        let before = app.panels.activity.spark_refresh_requests;
        spark_event(
            &mut app,
            SparkEvent::PaymentPending {
                id: "pay-2".into(),
                amount_sat: 1_000,
            },
        );
        assert_eq!(app.panels.activity.spark_refresh_requests, before);
        poll.coins.clear();
        drop(app.update(Message::UpdateDaemonCache(Ok(poll))));
        assert!(!app.panels.activity.vault_loading);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A row picked on a wallet page stays selected only while that page
    /// shows it and only on that page; the Spark page's own reloads still
    /// reach it.
    #[test]
    fn a_wallet_page_selection_ends_with_its_row_or_the_page() {
        let _guard = crate::app::session::test_guard();
        let root = std::env::temp_dir().join(format!("recent-select-{}", uuid::Uuid::new_v4()));
        let (mut app, _) = super::shell_tests::bitcoin_app(&root);
        app.panels.current = Menu::Spark(menu::SparkSubMenu::Overview);
        let generation = app.panels.activity.generation_for_tests();
        let at = |n: u64, id: &str| coincube_spark_protocol::PaymentSummary {
            timestamp: 1_760_000_000 + n,
            ..receipt(id)
        };
        drop(app.update(Message::ActivitySparkLoaded(
            generation,
            Ok((vec![at(0, "old")], false)),
        )));
        let old = app
            .panels
            .activity
            .wallet_entries(view::activity::WalletFilter::Spark)[0]
            .id
            .clone();
        let select = |app: &mut App, id: &str| {
            drop(app.update(Message::View(view::Message::Activity(
                view::activity::ActivityMessage::Select(id.to_owned()),
            ))))
        };
        select(&mut app, &old);

        // Still one of the five: kept.
        drop(app.update(Message::ActivitySparkRefreshed(
            generation,
            Ok((vec![at(5, "n5"), at(0, "old")], false)),
        )));
        assert_eq!(
            app.panels.activity.selected_id.as_deref(),
            Some(old.as_str())
        );
        // Five newer rows push it off the page: nothing stays selected, so
        // it cannot reopen when a later read brings it back.
        let newer: Vec<_> = (1..=5)
            .map(|n| at(n, &format!("n{n}")))
            .chain([at(0, "old")])
            .collect();
        drop(app.update(Message::ActivitySparkRefreshed(
            generation,
            Ok((newer, false)),
        )));
        assert!(app.panels.activity.selected_id.is_none());
        drop(app.update(Message::ActivitySparkRefreshed(
            generation,
            Ok((vec![at(0, "old")], false)),
        )));
        assert!(app.panels.activity.selected_id.is_none());

        // Leaving the page by the sidebar drops its selection too.
        select(&mut app, &old);
        drop(app.set_current_panel(Menu::Cube(menu::CubeSubMenu::Activity)));
        assert!(app.panels.activity.selected_id.is_none());
        // The Activity page's own selection survives the Activity page.
        select(&mut app, &old);
        drop(app.set_current_panel(Menu::Cube(menu::CubeSubMenu::Activity)));
        assert_eq!(
            app.panels.activity.selected_id.as_deref(),
            Some(old.as_str())
        );

        // The Spark page's reload reaches its panel and re-reads Activity's
        // Spark rows; off that page it does neither for Activity.
        app.panels.current = Menu::Cube(menu::CubeSubMenu::Overview);
        drop(app.update(Message::View(view::Message::SparkOverview(
            view::SparkOverviewMessage::DataLoaded {
                balance: coincube_core::miniscript::bitcoin::Amount::from_sat(42),
                usdb_balance: None,
                recent_payments: Vec::new(),
            },
        ))));
        let before = app.panels.activity.spark_refresh_requests;
        app.panels.current = Menu::Spark(menu::SparkSubMenu::Overview);
        drop(app.update(Message::View(view::Message::SparkOverview(
            view::SparkOverviewMessage::DataLoaded {
                balance: coincube_core::miniscript::bitcoin::Amount::from_sat(42),
                usdb_balance: None,
                recent_payments: Vec::new(),
            },
        ))));
        assert!(app.panels.spark_overview.has_snapshot_for_tests());
        assert_eq!(app.panels.activity.spark_refresh_requests, before + 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A swap's answer reaches the Swap panel while another page shows, and
    /// coming back to Swap shows it rather than a fresh form.
    #[test]
    fn a_swap_answer_reaches_its_panel_from_another_page() {
        use coincube_spark_protocol::{
            AssetAmount, ConversionLimits, ConversionQuote, PrepareSwapOk, SwapOk, SwapStatus,
            WalletAsset,
        };
        let _guard = crate::app::session::test_guard();
        let root = std::env::temp_dir().join(format!("swap-routing-{}", uuid::Uuid::new_v4()));
        let (mut app, _) = super::shell_tests::bitcoin_app(&root);
        let amount = |asset, amount| AssetAmount { asset, amount };
        let request = app
            .panels
            .spark_swap
            .start_swapping_for_tests(PrepareSwapOk {
                handle: "h".into(),
                quote: ConversionQuote {
                    amount_in: amount(WalletAsset::Bitcoin, 100_000),
                    amount_out: amount(WalletAsset::Usdb, 80_000_000),
                    fee: amount(WalletAsset::Bitcoin, 100),
                    amount_adjustment: None,
                    max_slippage_bps: 50,
                },
                limits: ConversionLimits::default(),
            });
        drop(app.set_current_panel(Menu::Spark(menu::SparkSubMenu::Overview)));
        drop(app.update(Message::View(view::Message::SparkSwap(
            view::spark::SparkSwapMessage::Swapped {
                request,
                result: crate::app::state::spark::swap::SwapAttempt::Done(SwapOk {
                    swap_id: "swap".into(),
                    status: SwapStatus::Completed,
                }),
            },
        ))));
        drop(app.set_current_panel(Menu::Spark(menu::SparkSubMenu::Swap)));
        assert!(matches!(
            app.panels.spark_swap.phase(),
            crate::app::state::spark::swap::SwapPhase::Swapped(_)
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The Vault and Spark pages show Activity's rows and detail pane, so
    /// Activity's results and intents reach it from those routes, and "View
    /// all transactions" opens Activity on that wallet alone.
    #[test]
    fn wallet_pages_route_activity_and_view_all_opens_filtered_activity() {
        let _guard = crate::app::session::test_guard();
        let root = std::env::temp_dir().join(format!("recent-activity-{}", uuid::Uuid::new_v4()));
        let (mut app, _) = super::shell_tests::bitcoin_app(&root);
        app.panels.current = Menu::Vault(menu::VaultSubMenu::Overview);

        let generation = app.panels.activity.generation_for_tests();
        drop(app.update(Message::ActivitySparkLoaded(
            generation,
            Ok((vec![receipt("recv-1")], false)),
        )));
        let entries = app
            .panels
            .activity
            .wallet_entries(view::activity::WalletFilter::Spark);
        assert_eq!(
            entries.len(),
            1,
            "a load off the Activity route still lands"
        );

        let id = entries[0].id.clone();
        drop(app.update(Message::View(view::Message::Activity(
            view::activity::ActivityMessage::Select(id.clone()),
        ))));
        assert_eq!(
            app.panels.activity.selected_id.as_deref(),
            Some(id.as_str())
        );
        assert_eq!(
            app.panels.current,
            Menu::Vault(menu::VaultSubMenu::Overview)
        );
        drop(app.view());

        // The Vault page: its own rows, and the selected row's pane beside
        // the page at 1440 and over it at the 1024 minimum.
        let generation = app.panels.activity.generation_for_tests();
        drop(app.update(Message::ActivityVaultLoaded(
            generation,
            Ok((vec![vault_payment()], false)),
        )));
        let vault_id = app
            .panels
            .activity
            .wallet_entries(view::activity::WalletFilter::Vault)[0]
            .id
            .clone();
        drop(app.update(Message::View(view::Message::Activity(
            view::activity::ActivityMessage::Select(vault_id.clone()),
        ))));
        app.cache.balance_privacy = Some(state::preferences::BalancePrivacyChrome {
            privacy: state::preferences::BalancePrivacy::Shown,
            busy: false,
            interactive: false,
        });
        for (outer, appearance, theme) in [
            ((1440, 900), "dark", coincube_ui::theme::Theme::dark()),
            ((1024, 768), "light", coincube_ui::theme::Theme::light()),
        ] {
            let page = crate::app::view::shared::landing::capture::inspect(
                "vault-recent-activity-selected",
                app.view(),
                outer,
                appearance,
                theme,
            );
            assert!(page.contains("Recent activity"));
            assert!(page.contains("View all transactions"));
            assert!(!page.contains("Synthetic receipt"), "Spark rows stay off");
            assert!(page.has_focus(view::activity::row_focus_id(&vault_id)));
            assert!(page.has_focus(iced::advanced::widget::Id::from(
                view::activity::DETAIL_CLOSE_ID
            )));
            assert!(page.contains("Txid"));
            assert!(!page.contains("Incoming payment"));
        }

        drop(app.update(Message::View(view::Message::Activity(
            view::activity::ActivityMessage::ViewAll(view::activity::WalletFilter::Vault),
        ))));
        assert_eq!(app.panels.current, Menu::Cube(menu::CubeSubMenu::Activity));
        assert_eq!(
            app.panels.activity.filters.wallet,
            view::activity::WalletFilter::Vault
        );
        assert!(app.panels.activity.selected_id.is_none());

        // Returning to a wallet page opens it with no row selected.
        app.panels.activity.selected_id = Some(id);
        drop(app.set_current_panel(Menu::Vault(menu::VaultSubMenu::Overview)));
        assert!(app.panels.activity.selected_id.is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod toast_layer_tree_tests {
    use crate::app::menu::{CubeSettingsOption, CubeSubMenu, Menu};
    use coincube_ui::{font, theme, widget::Renderer};
    use iced::advanced::{
        renderer::Headless,
        widget::{operation, operation::focusable, Id, Operation},
    };
    use iced::{mouse, Event, Point, Size};
    use iced_runtime::user_interface::{Cache as UiCache, State, UserInterface};

    type Ui<'a> = UserInterface<'a, super::Message, theme::Theme, Renderer>;

    const SIZE: Size = Size::new(1024.0, 768.0);

    fn renderer() -> Renderer {
        for bytes in font::load() {
            iced::advanced::graphics::text::font_system()
                .write()
                .unwrap()
                .load_font(bytes);
        }
        iced::futures::executor::block_on(<Renderer as Headless>::new(
            font::REGULAR,
            16.into(),
            Some("tiny-skia"),
        ))
        .unwrap()
    }

    fn apply(ui: &mut Ui<'_>, renderer: &Renderer, operation: impl Operation<()> + 'static) {
        let mut operation: Box<dyn Operation<()>> = Box::new(operation);
        loop {
            ui.operate(renderer, &mut operation::black_box(operation.as_mut()));
            match operation.finish() {
                operation::Outcome::None | operation::Outcome::Some(()) => break,
                operation::Outcome::Chain(next) => operation = next,
            }
        }
    }

    fn focused(ui: &mut Ui<'_>, renderer: &Renderer) -> Option<Id> {
        use std::sync::{Arc, Mutex};
        let result = Arc::new(Mutex::new(None));
        let output = Arc::clone(&result);
        apply(
            ui,
            renderer,
            operation::map(focusable::find_focused(), move |id| {
                *output.lock().unwrap() = Some(id)
            }),
        );
        let found = result.lock().unwrap().take();
        found
    }

    /// Showing the first toast and dismissing the last one must not remount
    /// the content beneath the toast layer: the root keeps the same shape, so
    /// widget state such as keyboard focus survives the tree diff.
    #[test]
    fn content_widget_state_survives_toast_show_and_dismiss() {
        let _guard = crate::app::session::test_guard();
        let root = std::env::temp_dir().join(format!("toast-layer-tree-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("bitcoin")).unwrap();
        let (mut app, _wallet) = super::shell_tests::bitcoin_app(&root);
        app.panels.current = Menu::Cube(CubeSubMenu::Settings(CubeSettingsOption::AiAccess));
        app.show_recovery_alerts_prompt = false;
        assert!(app.errors.is_empty());

        let switch = Id::from("tenshu.ai-access.enable");
        let mut renderer = renderer();

        let mut ui = UserInterface::build(app.view(), SIZE, UiCache::new(), &mut renderer);
        apply(&mut ui, &renderer, focusable::focus::<()>(switch.clone()));
        assert_eq!(focused(&mut ui, &renderer), Some(switch.clone()));
        let cache = ui.into_cache();

        // First toast appears.
        drop(
            app.update(super::Message::View(super::view::Message::ShowToast(
                log::Level::Info,
                "Saved".into(),
            ))),
        );
        assert_eq!(app.errors.len(), 1);
        let toast_id = app.errors[0].0;
        let mut ui = UserInterface::build(app.view(), SIZE, cache, &mut renderer);
        assert_eq!(
            focused(&mut ui, &renderer),
            Some(switch.clone()),
            "showing a toast must not reset the content's widget state"
        );
        let cache = ui.into_cache();

        // Last toast goes.
        drop(
            app.update(super::Message::View(super::view::Message::DismissToast(
                toast_id,
            ))),
        );
        assert!(app.errors.is_empty());
        let mut ui = UserInterface::build(app.view(), SIZE, cache, &mut renderer);
        assert_eq!(
            focused(&mut ui, &renderer),
            Some(switch),
            "dismissing the last toast must not reset the content's widget state"
        );
        drop(ui);
        drop(app);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Showing and answering the one-time recovery-alerts prompt must not
    /// remount the content beneath it either.
    #[test]
    fn content_widget_state_survives_recovery_alerts_prompt() {
        let _guard = crate::app::session::test_guard();
        let root =
            std::env::temp_dir().join(format!("recovery-prompt-tree-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("bitcoin")).unwrap();
        let (mut app, _wallet) = super::shell_tests::bitcoin_app(&root);
        app.panels.current = Menu::Cube(CubeSubMenu::Settings(CubeSettingsOption::AiAccess));
        app.show_recovery_alerts_prompt = false;

        let switch = Id::from("tenshu.ai-access.enable");
        let mut renderer = renderer();

        let mut ui = UserInterface::build(app.view(), SIZE, UiCache::new(), &mut renderer);
        apply(&mut ui, &renderer, focusable::focus::<()>(switch.clone()));
        assert_eq!(focused(&mut ui, &renderer), Some(switch.clone()));
        let cache = ui.into_cache();

        app.show_recovery_alerts_prompt = true;
        let mut ui = UserInterface::build(app.view(), SIZE, cache, &mut renderer);
        let mut probe = Texts::default();
        ui.operate(&renderer, &mut operation::black_box(&mut probe));
        assert!(
            probe.0.iter().any(|text| text == "Alert your keyholders?"),
            "the prompt is mounted"
        );
        assert_eq!(
            focused(&mut ui, &renderer),
            Some(switch.clone()),
            "showing the prompt must not reset the content's widget state"
        );
        let cache = ui.into_cache();

        app.show_recovery_alerts_prompt = false;
        let mut ui = UserInterface::build(app.view(), SIZE, cache, &mut renderer);
        assert_eq!(
            focused(&mut ui, &renderer),
            Some(switch),
            "answering the prompt must not reset the content's widget state"
        );
        drop(ui);
        drop(app);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[derive(Default)]
    struct Texts(Vec<String>);
    impl Operation for Texts {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            operate(self);
        }
        fn text(&mut self, _: Option<&Id>, _: iced::Rectangle, value: &str) {
            self.0.push(value.to_owned());
        }
    }

    /// With no toasts the always-mounted overlay layer must be transparent to
    /// the pointer, or the Stack would levitate the cursor away from the
    /// content beneath it.
    #[test]
    fn empty_toast_overlay_has_no_mouse_interaction() {
        let mut renderer = renderer();
        let overlay = super::view::toast_overlay(std::iter::empty());
        let mut ui: UserInterface<'_, super::view::Message, theme::Theme, Renderer> =
            UserInterface::build(overlay, SIZE, UiCache::new(), &mut renderer);
        for x in [0.0, 1.0, 256.0, 512.0, 768.0, 1023.0] {
            for y in [0.0, 1.0, 384.0, 700.0, 744.0, 767.0] {
                let position = Point::new(x, y);
                let mut messages = Vec::new();
                let (state, _) = ui.update(
                    &[Event::Mouse(mouse::Event::CursorMoved { position })],
                    mouse::Cursor::Available(position),
                    &mut renderer,
                    &mut iced::advanced::clipboard::Null,
                    &mut messages,
                );
                match state {
                    State::Updated {
                        mouse_interaction, ..
                    } => assert_eq!(
                        mouse_interaction,
                        mouse::Interaction::None,
                        "empty toast overlay claims the pointer at {position:?}"
                    ),
                    State::Outdated => panic!("empty toast overlay went outdated"),
                }
                assert!(messages.is_empty());
            }
        }
    }
}

#[cfg(test)]
mod cube_name_routing_tests {
    use super::*;
    use iced::futures::StreamExt;

    fn with_session(test: impl std::future::Future<Output = ()>) {
        let _guard = crate::app::session::test_guard();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(test);
    }

    #[test]
    fn submitted_name_is_saved_unsynced_before_completion_changes_memory() {
        with_session(async {
            let root = tempfile::tempdir().unwrap();
            let (mut app, _) = super::shell_tests::bitcoin_app(root.path());
            let network_dir = app.datadir.network_directory(app.cube_settings.network);
            let mut saved = app.cube_settings.clone();
            saved.remote_synced = true;
            let sibling =
                settings::CubeSettings::new("Sibling".into(), crate::chain::ChainId::Bitcoin);
            let sibling_id = sibling.id.clone();
            settings::update_settings_file(&network_dir, move |mut settings| {
                settings.cubes = vec![saved, sibling];
                Some(settings)
            })
            .await
            .unwrap();
            let old_name = app.cube_settings.name.clone();
            let task = app.update(Message::View(view::Message::Settings(
                view::SettingsMessage::CubeNameSubmitted("  New name  ".into()),
            )));
            assert_eq!(app.cube_settings.name, old_name);
            let output = iced_runtime::task::into_stream(task)
                .unwrap()
                .next()
                .await
                .unwrap();
            assert!(
                matches!(output, iced_runtime::Action::Output(Message::CubeNameSaved(Ok(name))) if name == "New name")
            );
            assert_eq!(app.cube_settings.name, old_name);
            let persisted = settings::Settings::from_file(&network_dir).unwrap();
            let own = persisted
                .cubes
                .iter()
                .find(|cube| cube.id == app.cube_settings.id)
                .unwrap();
            assert_eq!(own.name, "New name");
            assert!(!own.remote_synced);
            assert_eq!(
                persisted
                    .cubes
                    .iter()
                    .find(|cube| cube.id == sibling_id)
                    .unwrap()
                    .name,
                "Sibling"
            );
        });
    }

    #[test]
    fn saved_cube_name_settles_all_local_owners_without_navigating() {
        with_session(async {
            let root = tempfile::tempdir().unwrap();
            let (mut app, _) = super::shell_tests::bitcoin_app(root.path());
            let route = Menu::Cube(menu::CubeSubMenu::Settings(menu::CubeSettingsOption::About));
            app.panels.current = route.clone();
            app.panels.connect.cube.server_cube_id = None;

            drop(app.update(Message::CubeNameSaved(Ok("New name".into()))));

            assert_eq!(app.cube_settings.name, "New name");
            assert_eq!(app.cache.cube_name, "New name");
            assert_eq!(app.panels.connect.cube.cube_name, "New name");
            assert_eq!(app.panels.current, route);
        });
    }

    #[test]
    fn failed_cube_name_save_reports_error_without_mutating_names_or_route() {
        with_session(async {
            let root = tempfile::tempdir().unwrap();
            let (mut app, _) = super::shell_tests::bitcoin_app(root.path());
            let before = (
                app.cube_settings.name.clone(),
                app.cache.cube_name.clone(),
                app.panels.connect.cube.cube_name.clone(),
                app.panels.current.clone(),
            );
            let task = app.update(Message::CubeNameSaved(Err("synthetic disk failure".into())));
            let output = iced_runtime::task::into_stream(task)
                .unwrap()
                .next()
                .await
                .unwrap();
            assert!(matches!(output, iced_runtime::Action::Output(Message::View(
            view::Message::ShowError(error)
        )) if error.contains("synthetic disk failure")));
            assert_eq!(
                (
                    app.cube_settings.name.clone(),
                    app.cache.cube_name.clone(),
                    app.panels.connect.cube.cube_name.clone(),
                    app.panels.current.clone()
                ),
                before
            );
        });
    }

    #[test]
    fn remote_name_completion_never_replaces_local_name_or_route() {
        with_session(async {
            let root = tempfile::tempdir().unwrap();
            let (mut app, _) = super::shell_tests::bitcoin_app(root.path());
            app.cube_settings.name = "Newer local name".into();
            app.cache.cube_name = "Newer local name".into();
            app.panels.connect.cube.cube_name = "Newer local name".into();
            let route = app.panels.current.clone();
            for result in [Ok(()), Err("synthetic remote failure".into())] {
                let task = app.update(Message::CubeNameSynced {
                    cube_id: "older-cube".into(),
                    result,
                });
                assert!(iced_runtime::task::into_stream(task).is_none());
                assert_eq!(app.cube_settings.name, "Newer local name");
                assert_eq!(app.cache.cube_name, "Newer local name");
                assert_eq!(app.panels.connect.cube.cube_name, "Newer local name");
                assert_eq!(app.panels.current, route);
            }
        });
    }
}

#[cfg(test)]
mod recovery_settings_routing_tests {
    use super::*;
    use crate::services::coincube::{RecoveryRecipients, VaultMonitoringStatus};

    fn with_app(test: impl FnOnce(&mut App)) {
        let _guard = crate::app::session::test_guard();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        let root = tempfile::tempdir().unwrap();
        let (mut app, _) = super::shell_tests::bitcoin_app(root.path());
        app.panels.current =
            Menu::Cube(menu::CubeSubMenu::Settings(menu::CubeSettingsOption::About));
        app.cube_settings.recovery_alerts_prompt_answered = false;
        app.show_recovery_alerts_prompt = false;
        test(&mut app);
    }

    fn alerts(message: view::RecoveryAlertsMessage) -> Message {
        Message::View(view::Message::Settings(
            view::SettingsMessage::RecoveryAlerts(message),
        ))
    }

    #[test]
    fn stale_recovery_results_cannot_answer_consent_hydrate_or_open_a_prompt() {
        with_app(|app| {
            let stale = app
                .panels
                .connect
                .account
                .session_generation()
                .wrapping_add(1);
            let route = app.panels.current.clone();
            let messages = [
                view::RecoveryAlertsMessage::StatusLoaded(
                    Ok((
                        77,
                        RecoveryRecipients::default(),
                        VaultMonitoringStatus::default(),
                    )),
                    stale,
                ),
                view::RecoveryAlertsMessage::ChangeResult(
                    Ok(VaultMonitoringStatus::default()),
                    stale,
                    None,
                ),
            ];
            for message in messages {
                drop(app.update(alerts(message)));
                assert!(!app.cube_settings.recovery_alerts_prompt_answered);
                assert!(!app.show_recovery_alerts_prompt);
                assert!(app
                    .panels
                    .global_settings
                    .recovery_alerts
                    .vault_id
                    .is_none());
                assert!(app.panels.global_settings.recovery_alerts.status.is_none());
                assert_eq!(app.panels.current, route);
            }
        });
    }

    #[test]
    fn failed_recovery_status_routes_off_page_without_marking_consent_answered() {
        with_app(|app| {
            let generation = app.panels.connect.account.session_generation();
            let route = app.panels.current.clone();
            drop(app.update(alerts(view::RecoveryAlertsMessage::StatusLoaded(
                Err("synthetic status failure".into()),
                generation,
            ))));
            assert_eq!(
                app.panels.global_settings.recovery_alerts.error.as_deref(),
                Some("synthetic status failure")
            );
            assert!(app.panels.global_settings.recovery_alerts.loaded_once);
            assert!(!app.cube_settings.recovery_alerts_prompt_answered);
            assert!(!app.show_recovery_alerts_prompt);
            assert_eq!(app.panels.current, route);
        });
    }

    #[test]
    fn current_status_hydrates_off_page_but_is_not_consent() {
        with_app(|app| {
            let generation = app.panels.connect.account.session_generation();
            let route = app.panels.current.clone();
            drop(app.update(alerts(view::RecoveryAlertsMessage::StatusLoaded(
                Ok((
                    77,
                    RecoveryRecipients::default(),
                    VaultMonitoringStatus::default(),
                )),
                generation,
            ))));
            assert_eq!(
                app.panels.global_settings.recovery_alerts.vault_id,
                Some(77)
            );
            assert!(app.panels.global_settings.recovery_alerts.status.is_some());
            assert!(!app.cube_settings.recovery_alerts_prompt_answered);
            assert_eq!(app.panels.current, route);
        });
    }

    #[test]
    fn only_current_successful_change_marks_recovery_consent_answered() {
        with_app(|app| {
            let generation = app.panels.connect.account.session_generation();
            let route = app.panels.current.clone();
            drop(app.update(alerts(view::RecoveryAlertsMessage::ChangeResult(
                Err("synthetic change failure".into()),
                generation,
                None,
            ))));
            assert!(!app.cube_settings.recovery_alerts_prompt_answered);
            drop(app.update(alerts(view::RecoveryAlertsMessage::ChangeResult(
                Ok(VaultMonitoringStatus::default()),
                generation,
                None,
            ))));
            assert!(app.cube_settings.recovery_alerts_prompt_answered);
            assert_eq!(app.panels.current, route);
        });
    }

    #[test]
    fn recovery_kit_completion_settles_its_card_while_another_page_is_active() {
        with_app(|app| {
            let route = app.panels.current.clone();
            app.panels.global_settings.recovery_kit.status_loading = true;
            app.panels.global_settings.recovery_kit.status_stale = true;
            drop(app.update(Message::View(view::Message::Settings(
                view::SettingsMessage::RecoveryKit(view::RecoveryKitMessage::StatusLoaded(Ok(
                    None,
                ))),
            ))));
            let card = &app.panels.global_settings.recovery_kit;
            assert!(!card.status_loading);
            assert!(!card.status_stale);
            assert!(!card.status.as_ref().unwrap().has_recovery_kit);
            assert_eq!(app.panels.current, route);
        });
    }
    #[test]
    fn declining_consent_dismisses_and_marks_answered_without_changing_route() {
        with_app(|app| {
            let route = app.panels.current.clone();
            app.show_recovery_alerts_prompt = true;
            let task = app.update(Message::View(view::Message::RecoveryAlertsConsent(false)));
            assert!(!app.show_recovery_alerts_prompt);
            assert!(app.cube_settings.recovery_alerts_prompt_answered);
            assert_eq!(app.panels.current, route);
            assert!(iced_runtime::task::into_stream(task).is_some());
        });
    }

    #[test]
    fn accepting_without_connect_dismisses_but_does_not_record_success() {
        with_app(|app| {
            app.show_recovery_alerts_prompt = true;
            let task = app.update(Message::View(view::Message::RecoveryAlertsConsent(true)));
            assert!(!app.show_recovery_alerts_prompt);
            assert!(!app.cube_settings.recovery_alerts_prompt_answered);
            use iced::futures::StreamExt;
            let output = iced::futures::executor::block_on(
                iced_runtime::task::into_stream(task).unwrap().next(),
            )
            .unwrap();
            assert!(
                matches!(output, iced_runtime::Action::Output(Message::View(view::Message::ShowError(error))) if error.contains("recovery alerts weren't turned on"))
            );
        });
    }

    #[test]
    fn ineligible_status_does_not_open_consent_or_clear_focus() {
        with_app(|app| {
            assert!(!app.show_recovery_alerts_prompt);
            assert!(
                iced_runtime::task::into_stream(app.maybe_show_recovery_alerts_prompt()).is_none()
            );
            assert!(!app.show_recovery_alerts_prompt);
            assert!(!app.cube_settings.recovery_alerts_prompt_answered);
        });
    }
}
