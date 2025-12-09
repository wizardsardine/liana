pub mod breez;
#[cfg(feature = "buysell")]
pub mod buysell;
pub mod cache;
pub mod config;
pub mod error;
pub mod menu;
pub mod message;
pub mod settings;
pub mod state;
pub mod view;
pub mod wallet;

use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

use iced::{clipboard, time, widget::Column, Subscription, Task};
use tokio::runtime::Handle;
use tracing::{error, info, warn};

pub use coincube_core::miniscript::bitcoin;
use coincube_ui::{component::network_banner, widget::Element};
pub use coincubed::{commands::CoinStatus, config::Config as DaemonConfig};

pub use config::Config;
pub use message::Message;

use state::{
    ActiveOverview, ActiveReceive, ActiveSend, ActiveSettings, ActiveTransactions, CoinsPanel,
    CreateSpendPanel, GlobalHome, PsbtsPanel, State, VaultOverview, VaultReceivePanel,
    VaultTransactionsPanel,
};
use wallet::{sync_status, SyncStatus};

use crate::{
    app::{
        breez::BreezClient,
        cache::{Cache, DaemonCache},
        error::Error,
        menu::Menu,
        message::FiatMessage,
        settings::WalletId,
        wallet::Wallet,
    },
    daemon::{embedded::EmbeddedDaemon, Daemon, DaemonBackend},
    dir::CoincubeDirectory,
    node::{bitcoind::Bitcoind, NodeType},
};

use self::state::SettingsState;

struct Panels {
    current: Menu,
    vault_expanded: bool,
    active_expanded: bool,
    // Always available panels
    global_home: GlobalHome,
    active_overview: ActiveOverview,
    active_send: ActiveSend,
    active_receive: ActiveReceive,
    active_transactions: ActiveTransactions,
    active_settings: ActiveSettings,
    // Vault-only panels - None when no vault exists
    vault_overview: Option<VaultOverview>,
    coins: Option<CoinsPanel>,
    transactions: Option<VaultTransactionsPanel>,
    psbts: Option<PsbtsPanel>,
    recovery: Option<CreateSpendPanel>,
    receive: Option<VaultReceivePanel>,
    create_spend: Option<CreateSpendPanel>,
    settings: Option<SettingsState>,
    #[cfg(feature = "buysell")]
    buy_sell: Option<crate::app::view::buysell::BuySellPanel>,
}

impl Panels {
    fn new_without_vault(breez_client: Arc<BreezClient>, wallet: Option<Arc<Wallet>>) -> Panels {
        // NO VAULT - All vault panels are None, but Active panels always work
        // The UI layer prevents navigation to vault panels when has_vault=false

        Self {
            current: Menu::Home,
            vault_expanded: false,
            active_expanded: false,
            // Active panels always available (use BreezClient, not Vault wallet)
            global_home: if let Some(w) = &wallet {
                GlobalHome::new(w.clone())
            } else {
                GlobalHome::new_without_wallet()
            },
            active_overview: ActiveOverview::new(breez_client.clone()),
            active_send: ActiveSend::new(breez_client.clone()),
            active_receive: ActiveReceive::new(breez_client.clone()),
            active_transactions: ActiveTransactions::new(breez_client.clone()),
            active_settings: ActiveSettings::new(breez_client),
            // All vault panels are None - no vault exists
            vault_overview: None,
            coins: None,
            transactions: None,
            psbts: None,
            recovery: None,
            receive: None,
            create_spend: None,
            settings: None,
            #[cfg(feature = "buysell")]
            buy_sell: None,
        }
    }

    fn new(
        breez_client: Arc<BreezClient>,
        cache: &Cache,
        wallet: Arc<Wallet>,
        data_dir: CoincubeDirectory,
        daemon_backend: DaemonBackend,
        internal_bitcoind: Option<&Bitcoind>,
        config: Arc<Config>,
        restored_from_backup: bool,
    ) -> Panels {
        let show_rescan_warning = restored_from_backup
            && daemon_backend.is_coincubed()
            && daemon_backend
                .node_type()
                .map(|nt| nt == NodeType::Bitcoind)
                // We don't know the node type for external coincubed so assume it's bitcoind.
                .unwrap_or(true);
        Self {
            current: Menu::Home,
            vault_expanded: false,
            active_expanded: false,
            global_home: GlobalHome::new(wallet.clone()),
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
            active_overview: ActiveOverview::new(breez_client.clone()),
            active_send: ActiveSend::new(breez_client.clone()),
            active_receive: ActiveReceive::new(breez_client.clone()),
            active_transactions: ActiveTransactions::new(breez_client.clone()),
            active_settings: ActiveSettings::new(breez_client),
            coins: Some(CoinsPanel::new(
                cache.coins(),
                wallet.main_descriptor.first_timelock_value(),
            )),
            transactions: Some(VaultTransactionsPanel::new(wallet.clone())),
            psbts: Some(PsbtsPanel::new(wallet.clone())),
            recovery: Some(new_recovery_panel(wallet.clone(), cache)),
            receive: Some(VaultReceivePanel::new(data_dir.clone(), wallet.clone())),
            create_spend: Some(CreateSpendPanel::new(
                wallet.clone(),
                cache.coins(),
                cache.blockheight() as u32,
                cache.network,
            )),
            settings: Some(state::SettingsState::new(
                data_dir.clone(),
                wallet.clone(),
                daemon_backend,
                internal_bitcoind.is_some(),
                config.clone(),
            )),
            #[cfg(feature = "buysell")]
            buy_sell: Some(crate::app::view::buysell::BuySellPanel::new(
                cache.network,
                wallet,
            )),
        }
    }

    /// Rebuilds all vault-specific panels when a vault wallet is added to an app that didn't have one.
    /// This is called when transitioning from no-vault to has-vault state.
    fn build_vault_panels(
        &mut self,
        wallet: Arc<Wallet>,
        cache: &Cache,
        daemon_backend: DaemonBackend,
        data_dir: CoincubeDirectory,
        internal_bitcoind: Option<&Bitcoind>,
        config: Arc<Config>,
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
        self.recovery = Some(new_recovery_panel(wallet.clone(), cache));
        self.receive = Some(VaultReceivePanel::new(data_dir.clone(), wallet.clone()));
        self.create_spend = Some(CreateSpendPanel::new(
            wallet.clone(),
            cache.coins(),
            cache.blockheight() as u32,
            cache.network,
        ));
        self.settings = Some(state::SettingsState::new(
            data_dir.clone(),
            wallet.clone(),
            daemon_backend,
            internal_bitcoind.is_some(),
            config.clone(),
        ));
        #[cfg(feature = "buysell")]
        {
            self.buy_sell = Some(crate::app::view::buysell::BuySellPanel::new(
                cache.network,
                wallet,
            ));
        }
    }

    fn current(&self) -> Option<&dyn State> {
        match &self.current {
            Menu::Home => Some(&self.global_home),
            Menu::Active(submenu) => match submenu {
                crate::app::menu::ActiveSubMenu::Overview => Some(&self.active_overview),
                crate::app::menu::ActiveSubMenu::Send => Some(&self.active_send),
                crate::app::menu::ActiveSubMenu::Receive => Some(&self.active_receive),
                crate::app::menu::ActiveSubMenu::Transactions(_) => Some(&self.active_transactions),
                crate::app::menu::ActiveSubMenu::Settings(_) => Some(&self.active_settings),
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
                    self.settings.as_ref().map(|v| v as &dyn State)
                }
            },
            #[cfg(feature = "buysell")]
            Menu::BuySell => self.buy_sell.as_ref().map(|v| v as &dyn State),
            // Legacy menu items
            Menu::Receive => self.receive.as_ref().map(|v| v as &dyn State),
            Menu::PSBTs => self.psbts.as_ref().map(|v| v as &dyn State),
            Menu::Transactions => self.transactions.as_ref().map(|v| v as &dyn State),
            Menu::TransactionPreSelected(_) => self.transactions.as_ref().map(|v| v as &dyn State),
            Menu::Settings | Menu::SettingsPreSelected(_) => {
                self.settings.as_ref().map(|v| v as &dyn State)
            }
            Menu::Coins => self.coins.as_ref().map(|v| v as &dyn State),
            Menu::CreateSpendTx => self.create_spend.as_ref().map(|v| v as &dyn State),
            Menu::Recovery => self.recovery.as_ref().map(|v| v as &dyn State),
            Menu::RefreshCoins(_) => self.create_spend.as_ref().map(|v| v as &dyn State),
            Menu::PsbtPreSelected(_) => self.psbts.as_ref().map(|v| v as &dyn State),
        }
    }

    fn current_mut(&mut self) -> Option<&mut dyn State> {
        match &self.current {
            Menu::Home => Some(&mut self.global_home),
            Menu::Active(submenu) => match submenu {
                crate::app::menu::ActiveSubMenu::Overview => Some(&mut self.active_overview),
                crate::app::menu::ActiveSubMenu::Send => Some(&mut self.active_send),
                crate::app::menu::ActiveSubMenu::Receive => Some(&mut self.active_receive),
                crate::app::menu::ActiveSubMenu::Transactions(_) => {
                    Some(&mut self.active_transactions)
                }
                crate::app::menu::ActiveSubMenu::Settings(_) => Some(&mut self.active_settings),
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
                    self.settings.as_mut().map(|v| v as &mut dyn State)
                }
            },
            #[cfg(feature = "buysell")]
            Menu::BuySell => self.buy_sell.as_mut().map(|v| v as &mut dyn State),
            // Legacy menu items
            Menu::Receive => self.receive.as_mut().map(|v| v as &mut dyn State),
            Menu::PSBTs => self.psbts.as_mut().map(|v| v as &mut dyn State),
            Menu::Transactions => self.transactions.as_mut().map(|v| v as &mut dyn State),
            Menu::TransactionPreSelected(_) => {
                self.transactions.as_mut().map(|v| v as &mut dyn State)
            }
            Menu::Settings | Menu::SettingsPreSelected(_) => {
                self.settings.as_mut().map(|v| v as &mut dyn State)
            }
            Menu::Coins => self.coins.as_mut().map(|v| v as &mut dyn State),
            Menu::CreateSpendTx => self.create_spend.as_mut().map(|v| v as &mut dyn State),
            Menu::Recovery => self.recovery.as_mut().map(|v| v as &mut dyn State),
            Menu::RefreshCoins(_) => self.create_spend.as_mut().map(|v| v as &mut dyn State),
            Menu::PsbtPreSelected(_) => self.psbts.as_mut().map(|v| v as &mut dyn State),
        }
    }
}

pub struct App {
    cache: Cache,
    wallet: Option<Arc<Wallet>>,
    breez_client: Arc<BreezClient>,
    daemon: Option<Arc<dyn Daemon + Sync + Send>>,
    internal_bitcoind: Option<Bitcoind>,
    cube_settings: settings::CubeSettings,
    config: Arc<Config>,
    datadir: CoincubeDirectory,

    panels: Panels,
}

impl App {
    pub fn new(
        cache: Cache,
        wallet: Arc<Wallet>,
        breez_client: Arc<BreezClient>,
        config: Config,
        daemon: Arc<dyn Daemon + Sync + Send>,
        data_dir: CoincubeDirectory,
        internal_bitcoind: Option<Bitcoind>,
        restored_from_backup: bool,
    ) -> (App, Task<Message>) {
        let config_arc = Arc::new(config);
        let cube_settings = settings::CubeSettings::new(
            wallet
                .alias
                .clone()
                .unwrap_or_else(|| "My Cube".to_string()),
            cache.network,
        )
        .with_vault(wallet.id());

        let mut panels = Panels::new(
            breez_client.clone(),
            &cache,
            wallet.clone(),
            data_dir.clone(),
            daemon.backend(),
            internal_bitcoind.as_ref(),
            config_arc.clone(),
            restored_from_backup,
        );
        let cmd = if let Some(vault_overview) = panels.vault_overview.as_mut() {
            vault_overview.reload(daemon.clone(), wallet.clone())
        } else {
            tracing::warn!("vault_overview not present in App::new despite vault being configured");
            Task::none()
        };
        let mut cache_with_vault = cache;
        cache_with_vault.has_vault = true;
        (
            Self {
                panels,
                cache: cache_with_vault,
                daemon: Some(daemon),
                wallet: Some(wallet),
                breez_client,
                internal_bitcoind,
                cube_settings,
                config: config_arc,
                datadir: data_dir,
            },
            cmd,
        )
    }

    pub fn new_without_wallet(
        breez_client: Arc<BreezClient>,
        config: Config,
        datadir: CoincubeDirectory,
        network: coincube_core::miniscript::bitcoin::Network,
        cube_settings: settings::CubeSettings,
    ) -> (App, Task<Message>) {
        tracing::info!(
            "Creating app without wallet for cube: {}",
            cube_settings.name
        );
        let config_arc = Arc::new(config);
        let cache = Cache {
            network,
            datadir_path: datadir.clone(),
            has_vault: false,
            ..Default::default()
        };
        tracing::debug!("Cache configured with has_vault=false");

        // Create panels without vault - Active wallet always available via BreezClient
        let panels = Panels::new_without_vault(breez_client.clone(), None);

        tracing::info!("App created without vault successfully");
        (
            Self {
                panels,
                cache,
                daemon: None,
                wallet: None,
                breez_client,
                internal_bitcoind: None,
                cube_settings,
                config: config_arc,
                datadir,
            },
            Task::none(),
        )
    }

    pub fn wallet_id(&self) -> Option<WalletId> {
        self.wallet.as_ref().map(|w| w.id())
    }

    pub fn title(&self) -> &str {
        if let Some(wallet) = &self.wallet {
            if let Some(alias) = &wallet.alias {
                if !alias.is_empty() {
                    return alias;
                }
            }
            "Coincube Vault Wallet"
        } else {
            &self.cube_settings.name
        }
    }

    pub fn cache(&self) -> &Cache {
        &self.cache
    }

    pub fn breez_client(&self) -> Arc<BreezClient> {
        self.breez_client.clone()
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
                                        daemon.clone(),
                                        &self.cache,
                                        Message::View(view::Message::Settings(match setting {
                                            menu::SettingsOption::Node => {
                                                view::SettingsMessage::EditBitcoindSettings
                                            }
                                        })),
                                    );
                                }
                            }
                        }
                        menu::VaultSubMenu::Coins(Some(preselected)) => {
                            self.panels.create_spend = Some(CreateSpendPanel::new_self_send(
                                wallet.clone(),
                                self.cache.coins(),
                                self.cache.blockheight() as u32,
                                preselected,
                                self.cache.network,
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
                                self.panels.create_spend = Some(CreateSpendPanel::new(
                                    wallet.clone(),
                                    self.cache.coins(),
                                    self.cache.blockheight() as u32,
                                    self.cache.network,
                                ));
                            }
                        }
                        menu::VaultSubMenu::Recovery => {
                            if self
                                .panels
                                .recovery
                                .as_ref()
                                .is_none_or(|p| !p.keep_state())
                            {
                                self.panels.recovery =
                                    Some(new_recovery_panel(wallet.clone(), &self.cache));
                            }
                        }
                        _ => {}
                    }
                }
            }
            menu::Menu::Active(submenu) => {
                if let menu::ActiveSubMenu::Transactions(Some(txid)) = submenu {
                    if let Some(daemon) = &self.daemon {
                        if let Ok(Some(tx)) = Handle::current().block_on(async {
                            daemon
                                .get_history_txs(&[*txid])
                                .await
                                .map(|txs| txs.first().cloned())
                        }) {
                            self.panels.active_transactions.preselect(tx);
                            self.panels.current = menu;
                            return Task::none();
                        }
                    }
                }
            }
            _ => {
                tracing::debug!(
                    "Menu variant {:?} has no special handling in set_current_panel",
                    menu
                );
            }
        };

        self.panels.current = menu;
        if let (Some(daemon), Some(wallet)) = (&self.daemon, &self.wallet) {
            if let Some(panel) = self.panels.current_mut() {
                panel.reload(daemon.clone(), wallet.clone())
            } else {
                Task::none()
            }
        } else {
            Task::none()
        }
    }

    pub fn subscription(&self) -> Subscription<Message> {
        tracing::trace!(
            "App::subscription() called, has_vault={}",
            self.cache.has_vault
        );
        // Only create tick subscription if we have a vault (daemon exists)
        let subscriptions = if self.daemon.is_some() {
            vec![
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
                self.panels
                    .current()
                    .unwrap_or(&self.panels.global_home)
                    .subscription(),
            ]
        } else {
            // No vault - only subscribe to panel events, no tick updates
            vec![self
                .panels
                .current()
                .unwrap_or(&self.panels.global_home)
                .subscription()]
        };

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
                vec![panel.update(daemon.clone(), &self.cache, Message::Tick)]
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
            tracing::debug!("Updating daemon cache");

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

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Fiat(FiatMessage::GetPriceResult(fiat_price)) => {
                if self.wallet.as_ref().map(|w| w.fiat_price_is_relevant(&fiat_price)).unwrap_or(false)
                    // make sure we only update if the price is newer than the cached one
                    && !self.cache.fiat_price.as_ref().is_some_and(|cached| {
                        cached.source() == fiat_price.source()
                            && cached.currency() == fiat_price.currency()
                            && cached.requested_at() >= fiat_price.requested_at()
                    })
                {
                    self.cache.fiat_price = Some(fiat_price);
                    return Task::done(Message::CacheUpdated);
                }
            }
            Message::UpdateDaemonCache(res) => match res {
                Ok(daemon_cache) => {
                    self.cache.daemon_cache = daemon_cache;
                    return Task::done(Message::CacheUpdated);
                }
                Err(e) => tracing::error!("Failed to update daemon cache: {}", e),
            },
            Message::CacheUpdated => {
                // Update vault panels with cache if they exist
                if let (Some(daemon), Some(vault_overview), Some(settings)) = (
                    &self.daemon,
                    self.panels.vault_overview.as_mut(),
                    self.panels.settings.as_mut(),
                ) {
                    let daemon = daemon.clone();
                    let current = &self.panels.current;
                    let cache = self.cache.clone();

                    let is_settings_current = matches!(
                        current,
                        Menu::Settings
                            | Menu::SettingsPreSelected(_)
                            | Menu::Vault(crate::app::menu::VaultSubMenu::Settings(_))
                    );

                    let commands = [
                        vault_overview.update(
                            daemon.clone(),
                            &cache,
                            Message::UpdatePanelCache(
                                current == &Menu::Vault(crate::app::menu::VaultSubMenu::Overview),
                            ),
                        ),
                        settings.update(
                            daemon.clone(),
                            &cache,
                            Message::UpdatePanelCache(is_settings_current),
                        ),
                    ];
                    return Task::batch(commands);
                }
            }
            Message::LoadDaemonConfig(cfg) => {
                // Only load daemon config if we have a vault (daemon and wallet exist)
                if self.daemon.is_some() && self.wallet.is_some() {
                    let res = self.load_daemon_config(self.cache.datadir_path.clone(), *cfg);
                    return self.update(Message::DaemonConfigLoaded(res));
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
                        tracing::info!("Vault added to app - rebuilding vault panels");
                        self.panels.build_vault_panels(
                            wallet.clone(),
                            &self.cache,
                            daemon.backend(),
                            self.datadir.clone(),
                            self.internal_bitcoind.as_ref(),
                            self.config.clone(),
                        );
                    }
                }

                // Forward the message to the current panel
                if let (Some(daemon), Some(panel)) =
                    (self.daemon.clone(), self.panels.current_mut())
                {
                    return panel.update(daemon, &self.cache, Message::WalletUpdated(Ok(wallet)));
                }
            }
            Message::View(view::Message::Menu(menu)) => {
                if let Some(panel) = self.panels.current_mut() {
                    return Task::batch([panel.close(), self.set_current_panel(menu)]);
                }
            }
            Message::View(view::Message::ToggleVault) => {
                self.panels.vault_expanded = !self.panels.vault_expanded;
                self.cache.vault_expanded = self.panels.vault_expanded;
                // If we're expanding Vault, collapse Active
                if self.panels.vault_expanded {
                    self.panels.active_expanded = false;
                    self.cache.active_expanded = false;
                }
            }
            Message::View(view::Message::ToggleActive) => {
                self.panels.active_expanded = !self.panels.active_expanded;
                self.cache.active_expanded = self.panels.active_expanded;
                // If we're expanding Active, collapse Vault
                if self.panels.active_expanded {
                    self.panels.vault_expanded = false;
                    self.cache.vault_expanded = false;
                }
            }
            Message::View(view::Message::OpenUrl(url)) => {
                if let Err(e) = open::that_detached(&url) {
                    tracing::error!("Error opening '{}': {}", url, e);
                }
            }
            Message::View(view::Message::Clipboard(text)) => return clipboard::write(text),

            msg => {
                if let (Some(daemon), Some(panel)) =
                    (self.daemon.clone(), self.panels.current_mut())
                {
                    return panel.update(daemon, &self.cache, msg);
                }
            }
        };

        Task::none()
    }

    pub fn load_daemon_config(
        &mut self,
        datadir_path: CoincubeDirectory,
        cfg: DaemonConfig,
    ) -> Result<(), Error> {
        if let Some(daemon) = &self.daemon {
            Handle::current().block_on(async { daemon.stop().await })?;
        }
        let network = cfg.bitcoin_config.network;
        let daemon = EmbeddedDaemon::start(cfg)?;
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

    pub fn view(&self) -> Element<'_, Message> {
        let view = self
            .panels
            .current()
            .unwrap_or(&self.panels.global_home)
            .view(&self.panels.current, &self.cache);

        if self.cache.network != bitcoin::Network::Bitcoin {
            Column::with_children([
                network_banner(self.cache.network).into(),
                view.map(Message::View),
            ])
            .into()
        } else {
            view.map(Message::View)
        }
    }

    pub fn datadir_path(&self) -> &CoincubeDirectory {
        &self.cache.datadir_path
    }
}

fn new_recovery_panel(wallet: Arc<Wallet>, cache: &Cache) -> CreateSpendPanel {
    CreateSpendPanel::new_recovery(
        wallet,
        cache.coins(),
        cache.blockheight() as u32,
        cache.network,
    )
}
