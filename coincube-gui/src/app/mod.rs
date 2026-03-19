pub mod breez;
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
    CoinsPanel, CreateSpendPanel, GlobalHome, LiquidOverview, LiquidReceive, LiquidSend,
    LiquidSettings, LiquidTransactions, PsbtsPanel, State, UsdtOverview, UsdtReceive, UsdtSend,
    UsdtTransactions, VaultOverview, VaultReceivePanel, VaultTransactionsPanel,
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
    daemon::{embedded::EmbeddedDaemon, Daemon, DaemonBackend, DaemonError},
    dir::CoincubeDirectory,
    node::{
        bitcoind::{internal_bitcoind_datadir, internal_bitcoind_debug_log_path, Bitcoind},
        NodeType,
    },
};

use self::state::settings::SettingsState as GeneralSettingsState;
use self::state::vault::settings::SettingsState as VaultSettingsState;

struct Panels {
    current: Menu,
    vault_expanded: bool,
    liquid_expanded: bool,
    usdt_expanded: bool,
    // Always available panels
    global_home: GlobalHome,
    liquid_overview: LiquidOverview,
    liquid_send: LiquidSend,
    liquid_receive: LiquidReceive,
    liquid_transactions: LiquidTransactions,
    liquid_settings: LiquidSettings,
    usdt_overview: UsdtOverview,
    usdt_send: UsdtSend,
    usdt_receive: UsdtReceive,
    usdt_transactions: UsdtTransactions,
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
}

impl Panels {
    fn new_without_vault(
        breez_client: Arc<BreezClient>,
        wallet: Option<Arc<Wallet>>,
        datadir: &CoincubeDirectory,
        network: bitcoin::Network,
        cube_id: String,
    ) -> Panels {
        // NO VAULT - All vault panels are None, but Liquid panels always work
        // The UI layer prevents navigation to vault panels when has_vault=false

        Self {
            current: Menu::Home,
            vault_expanded: false,
            liquid_expanded: false,
            usdt_expanded: false,
            // Liquid panels always available (use BreezClient, not Vault wallet)
            global_home: if let Some(w) = &wallet {
                GlobalHome::new(
                    w.clone(),
                    breez_client.clone(),
                    datadir.clone(),
                    network,
                    cube_id.clone(),
                )
            } else {
                GlobalHome::new_without_wallet(
                    breez_client.clone(),
                    datadir.clone(),
                    network,
                    cube_id.clone(),
                )
            },
            liquid_overview: LiquidOverview::new(breez_client.clone()),
            liquid_send: LiquidSend::new(breez_client.clone()),
            liquid_receive: LiquidReceive::new(breez_client.clone()),
            liquid_transactions: LiquidTransactions::new(breez_client.clone()),
            liquid_settings: LiquidSettings::new(breez_client.clone()),
            usdt_overview: UsdtOverview::new(breez_client.clone()),
            usdt_send: UsdtSend::new(LiquidSend::new_usdt_only(breez_client.clone())),
            usdt_receive: UsdtReceive::new(LiquidReceive::new(breez_client.clone())),
            usdt_transactions: UsdtTransactions::new(LiquidTransactions::new_usdt_only(
                breez_client.clone(),
            )),
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
            buy_sell: None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn new(
        breez_client: Arc<BreezClient>,
        cache: &Cache,
        wallet: Arc<Wallet>,
        data_dir: CoincubeDirectory,
        daemon_backend: DaemonBackend,
        internal_bitcoind: Option<&Bitcoind>,
        config: Arc<Config>,
        restored_from_backup: bool,
        cube_id: String,
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
            liquid_expanded: false,
            usdt_expanded: false,
            global_home: GlobalHome::new(
                wallet.clone(),
                breez_client.clone(),
                data_dir.clone(),
                cache.network,
                cube_id.clone(),
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
            liquid_overview: LiquidOverview::new(breez_client.clone()),
            liquid_send: LiquidSend::new(breez_client.clone()),
            liquid_receive: LiquidReceive::new(breez_client.clone()),
            liquid_transactions: LiquidTransactions::new(breez_client.clone()),
            liquid_settings: LiquidSettings::new(breez_client.clone()),
            usdt_overview: UsdtOverview::new(breez_client.clone()),
            usdt_send: UsdtSend::new(LiquidSend::new_usdt_only(breez_client.clone())),
            usdt_receive: UsdtReceive::new(LiquidReceive::new(breez_client.clone())),
            usdt_transactions: UsdtTransactions::new(LiquidTransactions::new_usdt_only(
                breez_client.clone(),
            )),
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
            buy_sell: Some(crate::app::view::buysell::BuySellPanel::new(
                cache.network,
                wallet,
                breez_client,
            )),
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
            Menu::Home => Some(&self.global_home),
            Menu::Liquid(submenu) => match submenu {
                crate::app::menu::LiquidSubMenu::Overview => Some(&self.liquid_overview),
                crate::app::menu::LiquidSubMenu::Send => Some(&self.liquid_send),
                crate::app::menu::LiquidSubMenu::Receive => Some(&self.liquid_receive),
                crate::app::menu::LiquidSubMenu::Transactions(_) => Some(&self.liquid_transactions),
                crate::app::menu::LiquidSubMenu::Settings(_) => Some(&self.liquid_settings),
            },
            Menu::Usdt(submenu) => match submenu {
                crate::app::menu::UsdtSubMenu::Overview => Some(&self.usdt_overview),
                crate::app::menu::UsdtSubMenu::Send => Some(&self.usdt_send),
                crate::app::menu::UsdtSubMenu::Receive => Some(&self.usdt_receive),
                crate::app::menu::UsdtSubMenu::Transactions(_) => Some(&self.usdt_transactions),
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
            Menu::BuySell => self.buy_sell.as_ref().map(|v| v as &dyn State),
            Menu::Settings(_) => Some(&self.global_settings as &dyn State),
        }
    }

    fn current_mut(&mut self) -> Option<&mut dyn State> {
        match &self.current {
            Menu::Home => Some(&mut self.global_home),
            Menu::Liquid(submenu) => match submenu {
                crate::app::menu::LiquidSubMenu::Overview => Some(&mut self.liquid_overview),
                crate::app::menu::LiquidSubMenu::Send => Some(&mut self.liquid_send),
                crate::app::menu::LiquidSubMenu::Receive => Some(&mut self.liquid_receive),
                crate::app::menu::LiquidSubMenu::Transactions(_) => {
                    Some(&mut self.liquid_transactions)
                }
                crate::app::menu::LiquidSubMenu::Settings(_) => Some(&mut self.liquid_settings),
            },
            Menu::Usdt(submenu) => match submenu {
                crate::app::menu::UsdtSubMenu::Overview => Some(&mut self.usdt_overview),
                crate::app::menu::UsdtSubMenu::Send => Some(&mut self.usdt_send),
                crate::app::menu::UsdtSubMenu::Receive => Some(&mut self.usdt_receive),
                crate::app::menu::UsdtSubMenu::Transactions(_) => Some(&mut self.usdt_transactions),
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
            Menu::BuySell => self.buy_sell.as_mut().map(|v| v as &mut dyn State),
            Menu::Settings(_) => Some(&mut self.global_settings as &mut dyn State),
        }
    }
}

/// Interval between bitcoind sync progress polls (in seconds).
const BITCOIND_SYNC_POLL_INTERVAL: Duration = Duration::from_secs(10);

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
    errors: std::collections::BinaryHeap<(usize, std::time::Instant, String)>,
    current_error_id: usize,
    /// True while a check_bitcoind_sync_progress probe is in flight; prevents
    /// multiple concurrent RPC calls from piling up across subscription ticks.
    bitcoind_sync_probe_in_progress: bool,
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

impl App {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        cache: Cache,
        wallet: Arc<Wallet>,
        breez_client: Arc<BreezClient>,
        config: Config,
        daemon: Arc<dyn Daemon + Sync + Send>,
        data_dir: CoincubeDirectory,
        internal_bitcoind: Option<Bitcoind>,
        restored_from_backup: bool,
        cube_settings: settings::CubeSettings,
    ) -> (App, Task<Message>) {
        let config_arc = Arc::new(config);

        let mut panels = Panels::new(
            breez_client.clone(),
            &cache,
            wallet.clone(),
            data_dir.clone(),
            daemon.backend(),
            internal_bitcoind.as_ref(),
            config_arc.clone(),
            restored_from_backup,
            cube_settings.id.clone(),
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
        let cmd = Task::batch(tasks);
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
                errors: std::collections::BinaryHeap::with_capacity(8),
                current_error_id: 256,
                bitcoind_sync_probe_in_progress: false,
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
        let config_arc = Arc::new(config);
        // Load bitcoin_unit from cube settings if available
        let bitcoin_unit = {
            let network_dir = datadir.network_directory(network);
            settings::Settings::from_file(&network_dir)
                .ok()
                .and_then(|s| {
                    s.cubes
                        .iter()
                        .find(|c| c.id == cube_settings.id)
                        .map(|c| c.unit_setting.display_unit)
                })
                .unwrap_or_default()
        };
        let cache = Cache {
            network,
            datadir_path: datadir.clone(),
            has_vault: false,
            bitcoin_unit,
            ..Default::default()
        };

        let mut panels = Panels::new_without_vault(
            breez_client.clone(),
            None,
            &datadir,
            network,
            cube_settings.id.clone(),
        );

        let cmd = panels.global_home.reload(None, None);

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
                errors: std::collections::BinaryHeap::with_capacity(8),
                current_error_id: 256,
                bitcoind_sync_probe_in_progress: false,
            },
            cmd,
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
                                        Some(daemon.clone()),
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
            menu::Menu::Usdt(_submenu) => {
                // USDt panels: preselection handled via PreselectPayment message
            }
            _ => {
                tracing::debug!(
                    "Menu variant {:?} has no special handling in set_current_panel",
                    menu
                );
            }
        };

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

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::View(view::Message::DismissToast(id)) => {
                self.errors.retain(|(i, ..)| *i != id);
            }
            Message::View(view::Message::ShowError(msg)) => {
                // Redirect ShowError to ShowToast with Error level
                return self.update(Message::View(view::Message::ShowToast(log::Level::Error, msg)));
            }
            Message::View(view::Message::ShowToast(_level, msg)) => {
                // Show toast with specified level
                self.errors
                    .push((self.current_error_id, std::time::Instant::now(), msg));
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
                        // the immediately reversal that occurs when ConnectLoginVerified
                        // saves an already-synced Bitcoind into pending_bitcoind: the
                        // first poll would otherwise see ibd=false and switch back.
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
                                        let cfg_task =
                                            self.update(Message::DaemonConfigLoaded(Ok(())));
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

                        // Clear cached fiat price if disabled
                        if !cube.fiat_price.as_ref().is_some_and(|p| p.is_enabled) {
                            self.cache.fiat_price = None;
                        }
                    }
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
                // Check if fiat price is relevant based on cube settings (applies to both Liquid and Vault)
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
                    return Task::done(Message::CacheUpdated);
                }
            }
            Message::UpdateDaemonCache(res) => match res {
                Ok(daemon_cache) => {
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
                                    new_cfg.bitcoin_backend = Some(
                                        coincubed::config::BitcoinBackend::Esplora(fb.clone()),
                                    );
                                    new_cfg.fallback_esplora = None;
                                    new_cfg
                                })
                            });
                        if let Some(new_cfg) = fallback {
                            let datadir = self.cache.datadir_path.clone();
                            match self.load_daemon_config(datadir, new_cfg) {
                                Ok(()) => {
                                    info!("Switched to COINCUBE | Connect fallback after Bitcoind failure");
                                    let cfg_task = self.update(Message::DaemonConfigLoaded(Ok(())));
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
                // Update vault panels with cache if they exist
                if let (Some(daemon), Some(vault_overview), Some(vault_settings)) = (
                    &self.daemon,
                    self.panels.vault_overview.as_mut(),
                    self.panels.vault_settings.as_mut(),
                ) {
                    let daemon = daemon.clone();
                    let current = &self.panels.current;
                    let cache = self.cache.clone();

                    let is_settings_current = matches!(
                        current,
                        Menu::Settings(_)
                            | Menu::Vault(crate::app::menu::VaultSubMenu::Settings(_))
                    );

                    let is_spend_current =
                        matches!(current, Menu::Vault(crate::app::menu::VaultSubMenu::Send));

                    let mut commands = vec![
                        vault_overview.update(
                            Some(daemon.clone()),
                            &cache,
                            Message::UpdatePanelCache(
                                current == &Menu::Vault(crate::app::menu::VaultSubMenu::Overview),
                            ),
                        ),
                        vault_settings.update(
                            Some(daemon.clone()),
                            &cache,
                            Message::UpdatePanelCache(is_settings_current),
                        ),
                    ];

                    // Also update create_spend panel if it exists
                    if let Some(create_spend) = self.panels.create_spend.as_mut() {
                        commands.push(create_spend.update(
                            Some(daemon.clone()),
                            &cache,
                            Message::UpdatePanelCache(is_spend_current),
                        ));
                    }

                    return Task::batch(commands);
                }
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
                if let Some(panel) = self.panels.current_mut() {
                    return Task::batch([panel.close(), self.set_current_panel(menu)]);
                }
            }
            Message::View(view::Message::ToggleVault) => {
                self.panels.vault_expanded = !self.panels.vault_expanded;
                self.cache.vault_expanded = self.panels.vault_expanded;
                // If we're expanding Vault, collapse Liquid and USDt
                if self.panels.vault_expanded {
                    self.panels.liquid_expanded = false;
                    self.cache.liquid_expanded = false;
                    self.panels.usdt_expanded = false;
                    self.cache.usdt_expanded = false;
                }
            }
            Message::View(view::Message::ToggleLiquid) => {
                self.panels.liquid_expanded = !self.panels.liquid_expanded;
                self.cache.liquid_expanded = self.panels.liquid_expanded;
                // If we're expanding Liquid, collapse Vault and USDt
                if self.panels.liquid_expanded {
                    self.panels.vault_expanded = false;
                    self.cache.vault_expanded = false;
                    self.panels.usdt_expanded = false;
                    self.cache.usdt_expanded = false;
                }
            }
            Message::View(view::Message::ToggleUsdt) => {
                self.panels.usdt_expanded = !self.panels.usdt_expanded;
                self.cache.usdt_expanded = self.panels.usdt_expanded;
                // If we're expanding USDt, collapse Liquid and Vault
                if self.panels.usdt_expanded {
                    self.panels.liquid_expanded = false;
                    self.cache.liquid_expanded = false;
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
            msg @ Message::View(view::Message::Home(_)) => {
                return self
                    .panels
                    .global_home
                    .update(self.daemon.clone(), &self.cache, msg);
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

                        return Task::batch(vec![
                            Task::done(Message::Tick),
                            Task::done(Message::View(view::Message::LiquidSend(
                                view::LiquidSendMessage::RefreshRequested,
                            ))),
                            Task::done(Message::View(view::Message::LiquidOverview(
                                view::LiquidOverviewMessage::RefreshRequested,
                            ))),
                            Task::done(Message::View(view::Message::Home(
                                view::HomeMessage::RefreshLiquidBalance,
                            ))),
                            home_task.unwrap_or_else(Task::none),
                        ]);
                    }
                    SdkEvent::PaymentSucceeded { details } => {
                        let home_task = swap_id_for_bitcoin_send(&details).map(|swap_id| {
                            Task::done(Message::View(view::Message::Home(
                                view::HomeMessage::LiquidToVaultSucceeded(Some(swap_id)),
                            )))
                        });

                        return Task::batch(vec![
                            Task::done(Message::Tick),
                            Task::done(Message::View(view::Message::LiquidSend(
                                view::LiquidSendMessage::RefreshRequested,
                            ))),
                            Task::done(Message::View(view::Message::LiquidOverview(
                                view::LiquidOverviewMessage::RefreshRequested,
                            ))),
                            Task::done(Message::View(view::Message::UsdtOverview(
                                view::UsdtOverviewMessage::RefreshRequested,
                            ))),
                            Task::done(Message::View(view::Message::Home(
                                view::HomeMessage::RefreshLiquidBalance,
                            ))),
                            home_task.unwrap_or_else(Task::none),
                        ]);
                    }
                    SdkEvent::PaymentFailed { details } => {
                        let home_task = swap_id_for_bitcoin_send(&details).map(|swap_id| {
                            Task::done(Message::View(view::Message::Home(
                                view::HomeMessage::LiquidToVaultFailed(Some(swap_id)),
                            )))
                        });

                        return Task::batch(vec![
                            Task::done(Message::Tick),
                            Task::done(Message::View(view::Message::LiquidSend(
                                view::LiquidSendMessage::RefreshRequested,
                            ))),
                            Task::done(Message::View(view::Message::LiquidOverview(
                                view::LiquidOverviewMessage::RefreshRequested,
                            ))),
                            Task::done(Message::View(view::Message::UsdtOverview(
                                view::UsdtOverviewMessage::RefreshRequested,
                            ))),
                            Task::done(Message::View(view::Message::Home(
                                view::HomeMessage::RefreshLiquidBalance,
                            ))),
                            home_task.unwrap_or_else(Task::none),
                        ]);
                    }
                    SdkEvent::PaymentWaitingConfirmation { details } => {
                        let home_task = swap_id_for_bitcoin_send(&details).map(|swap_id| {
                            Task::done(Message::View(view::Message::Home(
                                view::HomeMessage::LiquidToVaultWaitingConfirmation(Some(swap_id)),
                            )))
                        });

                        return Task::batch(vec![
                            Task::done(Message::Tick),
                            Task::done(Message::View(view::Message::LiquidSend(
                                view::LiquidSendMessage::RefreshRequested,
                            ))),
                            Task::done(Message::View(view::Message::LiquidOverview(
                                view::LiquidOverviewMessage::RefreshRequested,
                            ))),
                            Task::done(Message::View(view::Message::UsdtOverview(
                                view::UsdtOverviewMessage::RefreshRequested,
                            ))),
                            Task::done(Message::View(view::Message::Home(
                                view::HomeMessage::RefreshLiquidBalance,
                            ))),
                            home_task.unwrap_or_else(Task::none),
                        ]);
                    }
                    SdkEvent::Synced => {
                        // Payment state changed - trigger cache update
                        return Task::batch(vec![
                            Task::done(Message::Tick),
                            Task::done(Message::View(view::Message::LiquidSend(
                                view::LiquidSendMessage::RefreshRequested,
                            ))),
                            Task::done(Message::View(view::Message::LiquidOverview(
                                view::LiquidOverviewMessage::RefreshRequested,
                            ))),
                            Task::done(Message::View(view::Message::UsdtOverview(
                                view::UsdtOverviewMessage::RefreshRequested,
                            ))),
                            Task::done(Message::View(view::Message::Home(
                                view::HomeMessage::RefreshLiquidBalance,
                            ))),
                        ]);
                    }
                    _ => {
                        // Other events - just log
                        log::debug!("Unhandled Breez event: {:?}", event);
                    }
                }
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
        };

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

    pub fn view(&self) -> Element<'_, Message> {
        let view = self
            .panels
            .current()
            .unwrap_or(&self.panels.global_home)
            .view(&self.panels.current, &self.cache);

        let content = if self.cache.network != bitcoin::Network::Bitcoin {
            iced::widget::column![network_banner(self.cache.network), view.map(Message::View)]
                .into()
        } else {
            view.map(Message::View)
        };

        // Overlay toast at bottom if present
        match self.errors.is_empty() {
            true => content,
            false => iced::widget::Stack::new()
                .push(content)
                .push(
                    view::toast_overlay(
                        self.errors.iter().map(|(id, _, msg)| (*id, log::Level::Error, msg.as_str())),
                    )
                    .map(Message::View),
                )
                .into(),
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
