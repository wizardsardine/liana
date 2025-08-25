pub mod cache;
pub mod config;
pub mod menu;
pub mod message;
pub mod settings;
pub mod state;
pub mod view;
pub mod wallet;

mod error;

use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

use iced::{clipboard, time, Subscription, Task};
use tokio::runtime::Handle;
use tracing::{error, info, warn};

pub use liana::miniscript::bitcoin;
use liana_ui::{
    component::network_banner,
    widget::{Column, Element},
};
pub use lianad::{commands::CoinStatus, config::Config as DaemonConfig};

pub use config::Config;
pub use message::Message;

use state::{
    CoinsPanel, CreateSpendPanel, Home, PsbtsPanel, ReceivePanel, State, TransactionsPanel,
};
use wallet::{sync_status, SyncStatus};

use crate::{
    app::{
        cache::{Cache, DaemonCache, FIAT_PRICE_UPDATE_INTERVAL_SECS},
        error::Error,
        menu::Menu,
        message::FiatMessage,
        settings::WalletId,
        wallet::Wallet,
    },
    daemon::{embedded::EmbeddedDaemon, Daemon, DaemonBackend},
    dir::LianaDirectory,
    node::{bitcoind::Bitcoind, NodeType},
    utils::now,
};

use self::state::SettingsState;

struct Panels {
    current: Menu,
    home: Home,
    coins: CoinsPanel,
    transactions: TransactionsPanel,
    psbts: PsbtsPanel,
    recovery: CreateSpendPanel,
    receive: ReceivePanel,
    create_spend: CreateSpendPanel,
    settings: SettingsState,
}

impl Panels {
    fn new(
        cache: &Cache,
        wallet: Arc<Wallet>,
        data_dir: LianaDirectory,
        daemon_backend: DaemonBackend,
        internal_bitcoind: Option<&Bitcoind>,
        config: Arc<Config>,
        restored_from_backup: bool,
    ) -> Panels {
        let show_rescan_warning = restored_from_backup
            && daemon_backend.is_lianad()
            && daemon_backend
                .node_type()
                .map(|nt| nt == NodeType::Bitcoind)
                // We don't know the node type for external lianad so assume it's bitcoind.
                .unwrap_or(true);
        Self {
            current: Menu::Home,
            home: Home::new(
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
            ),
            coins: CoinsPanel::new(cache.coins(), wallet.main_descriptor.first_timelock_value()),
            transactions: TransactionsPanel::new(wallet.clone()),
            psbts: PsbtsPanel::new(wallet.clone()),
            recovery: new_recovery_panel(wallet.clone(), cache),
            receive: ReceivePanel::new(data_dir.clone(), wallet.clone()),
            create_spend: CreateSpendPanel::new(
                wallet.clone(),
                cache.coins(),
                cache.blockheight() as u32,
                cache.network,
            ),
            settings: state::SettingsState::new(
                data_dir,
                wallet.clone(),
                daemon_backend,
                internal_bitcoind.is_some(),
                config.clone(),
            ),
        }
    }

    fn current(&self) -> &dyn State {
        match self.current {
            Menu::Home => &self.home,
            Menu::Receive => &self.receive,
            Menu::PSBTs => &self.psbts,
            Menu::Transactions => &self.transactions,
            Menu::TransactionPreSelected(_) => &self.transactions,
            Menu::Settings | Menu::SettingsPreSelected(_) => &self.settings,
            Menu::Coins => &self.coins,
            Menu::CreateSpendTx => &self.create_spend,
            Menu::Recovery => &self.recovery,
            Menu::RefreshCoins(_) => &self.create_spend,
            Menu::PsbtPreSelected(_) => &self.psbts,
        }
    }

    fn current_mut(&mut self) -> &mut dyn State {
        match self.current {
            Menu::Home => &mut self.home,
            Menu::Receive => &mut self.receive,
            Menu::PSBTs => &mut self.psbts,
            Menu::Transactions => &mut self.transactions,
            Menu::TransactionPreSelected(_) => &mut self.transactions,
            Menu::Settings | Menu::SettingsPreSelected(_) => &mut self.settings,
            Menu::Coins => &mut self.coins,
            Menu::CreateSpendTx => &mut self.create_spend,
            Menu::Recovery => &mut self.recovery,
            Menu::RefreshCoins(_) => &mut self.create_spend,
            Menu::PsbtPreSelected(_) => &mut self.psbts,
        }
    }
}

pub struct App {
    cache: Cache,
    wallet: Arc<Wallet>,
    daemon: Arc<dyn Daemon + Sync + Send>,
    internal_bitcoind: Option<Bitcoind>,

    panels: Panels,
}

impl App {
    pub fn new(
        cache: Cache,
        wallet: Arc<Wallet>,
        config: Config,
        daemon: Arc<dyn Daemon + Sync + Send>,
        data_dir: LianaDirectory,
        internal_bitcoind: Option<Bitcoind>,
        restored_from_backup: bool,
    ) -> (App, Task<Message>) {
        let config = Arc::new(config);
        let mut panels = Panels::new(
            &cache,
            wallet.clone(),
            data_dir,
            daemon.backend(),
            internal_bitcoind.as_ref(),
            config.clone(),
            restored_from_backup,
        );
        let mut cmds = vec![];
        cmds.push(panels.home.reload(daemon.clone(), wallet.clone()));
        // If the fiat price setting is enabled, fetch the fiat price when app starts.
        if wallet
            .fiat_price_setting
            .as_ref()
            .is_some_and(|sett| sett.is_enabled)
        {
            cmds.push(Task::perform(async move {}, |_| {
                Message::Fiat(FiatMessage::GetPrice)
            }));
        }
        (
            Self {
                panels,
                cache,
                daemon,
                wallet,
                internal_bitcoind,
            },
            Task::batch(cmds),
        )
    }

    pub fn wallet_id(&self) -> WalletId {
        self.wallet.id()
    }

    pub fn title(&self) -> &str {
        if let Some(alias) = &self.wallet.alias {
            if !alias.is_empty() {
                return alias;
            }
        }
        "Liana wallet"
    }

    fn set_current_panel(&mut self, menu: Menu) -> Task<Message> {
        self.panels.current_mut().interrupt();

        match &menu {
            menu::Menu::TransactionPreSelected(txid) => {
                if let Ok(Some(tx)) = Handle::current().block_on(async {
                    self.daemon
                        .get_history_txs(&[*txid])
                        .await
                        .map(|txs| txs.first().cloned())
                }) {
                    self.panels.transactions.preselect(tx);
                    self.panels.current = menu;
                    return Task::none();
                };
            }
            menu::Menu::PsbtPreSelected(txid) => {
                // Get preselected spend from DB in case it's not yet in the cache.
                // We only need this single spend as we will go straight to its view and not show the PSBTs list.
                // In case of any error loading the spend or if it doesn't exist, load PSBTs list in usual way.
                if let Ok(Some(spend_tx)) = Handle::current().block_on(async {
                    self.daemon
                        .list_spend_transactions(Some(&[*txid]))
                        .await
                        .map(|txs| txs.first().cloned())
                }) {
                    self.panels.psbts.preselect(spend_tx);
                    self.panels.current = menu;
                    return Task::none();
                };
            }
            menu::Menu::SettingsPreSelected(setting) => {
                self.panels.current = menu.clone();
                return self.panels.current_mut().update(
                    self.daemon.clone(),
                    &self.cache,
                    Message::View(view::Message::Settings(match setting {
                        &menu::SettingsOption::Node => view::SettingsMessage::EditBitcoindSettings,
                    })),
                );
            }
            menu::Menu::RefreshCoins(preselected) => {
                self.panels.create_spend = CreateSpendPanel::new_self_send(
                    self.wallet.clone(),
                    self.cache.coins(),
                    self.cache.blockheight() as u32,
                    preselected,
                    self.cache.network,
                );
            }
            menu::Menu::CreateSpendTx => {
                // redo the process of spending only if user want to start a new one.
                if !self.panels.create_spend.keep_state() {
                    self.panels.create_spend = CreateSpendPanel::new(
                        self.wallet.clone(),
                        self.cache.coins(),
                        self.cache.blockheight() as u32,
                        self.cache.network,
                    );
                }
            }
            menu::Menu::Recovery => {
                if !self.panels.recovery.keep_state() {
                    self.panels.recovery = new_recovery_panel(self.wallet.clone(), &self.cache);
                }
            }
            _ => {}
        };

        self.panels.current = menu;
        self.panels
            .current_mut()
            .reload(self.daemon.clone(), self.wallet.clone())
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![
            time::every(Duration::from_secs(
                match sync_status(
                    self.daemon.backend(),
                    self.cache.blockheight(),
                    self.cache.sync_progress(),
                    self.cache.last_poll_timestamp(),
                    self.cache.last_poll_at_startup,
                ) {
                    SyncStatus::BlockchainSync(_) => 5, // Only applies to local backends
                    SyncStatus::WalletFullScan
                        if self.daemon.backend() == DaemonBackend::RemoteBackend =>
                    {
                        10
                    } // If remote backend, don't ping too often
                    SyncStatus::WalletFullScan | SyncStatus::LatestWalletSync => 3,
                    SyncStatus::Synced => {
                        if self.daemon.backend() == DaemonBackend::RemoteBackend {
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
            self.panels.current().subscription(),
        ];
        // Add fiat price subscription if enabled.
        if let Some(sett) = self
            .wallet
            .fiat_price_setting
            .as_ref()
            .filter(|sett| sett.is_enabled)
        {
            // Force a new subscription to be created if the source or currency changes. This way, the first tick
            // will occur `FIAT_PRICE_UPDATE_INTERVAL_SECS` seconds after the initial cache entry for this pair.
            if self
                .cache
                .fiat_price_cache
                .fiat_price
                .as_ref()
                .is_some_and(|price| {
                    price.source() == sett.source && price.currency() == sett.currency
                })
            {
                subs.push(
                    time::every(Duration::from_secs(FIAT_PRICE_UPDATE_INTERVAL_SECS))
                        .map(|_| Message::Fiat(FiatMessage::GetPrice)),
                )
            }
        }
        Subscription::batch(subs)
    }

    pub fn stop(&mut self) {
        info!("Close requested");
        if self.daemon.backend().is_embedded() {
            if let Err(e) = Handle::current().block_on(async { self.daemon.stop().await }) {
                error!("{}", e);
            } else {
                info!("Internal daemon stopped");
            }
            if let Some(bitcoind) = self.internal_bitcoind.take() {
                bitcoind.stop();
            }
        }
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tick => {
                let daemon = self.daemon.clone();
                let datadir_path = self.cache.datadir_path.clone();
                let network = self.cache.network;
                Task::perform(
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
                        })
                    },
                    Message::UpdateDaemonCache,
                )
            }
            Message::Fiat(FiatMessage::GetPrice) => {
                if let Some(price_setting) = self
                    .wallet
                    .fiat_price_setting
                    .as_ref()
                    .filter(|sett| sett.is_enabled)
                {
                    let now = now().as_secs();
                    // Do nothing if the last request was recent and was for the same source & currency, where
                    // "recent" means within half the update interval.
                    // Using half the update interval is sufficient as we are mostly concerned with preventing
                    // multiple requests being sent within seconds of each other (e.g. after the GUI window is
                    // inactive for an extended period). Using the full update interval could lead to a kind
                    // of race condition and cause a regular subscription message to be missed.
                    if let Some(recent_request) = self
                        .cache
                        .fiat_price_cache
                        .last_request
                        .as_ref()
                        .filter(|req| {
                            req.source == price_setting.source
                                && req.currency == price_setting.currency
                                && req.timestamp + FIAT_PRICE_UPDATE_INTERVAL_SECS / 2 > now
                        })
                    {
                        // Cached request is still valid, no need to fetch a new one.
                        tracing::debug!(
                            "Using cached fiat price request for {} from {}",
                            recent_request.currency,
                            recent_request.source,
                        );
                        return Task::none();
                    }
                    let new_request = cache::FiatPriceRequest {
                        source: price_setting.source,
                        currency: price_setting.currency,
                        timestamp: now,
                    };
                    self.cache.fiat_price_cache.last_request = Some(new_request.clone());
                    tracing::debug!(
                        "Getting fiat price in {} from {}",
                        price_setting.currency,
                        price_setting.source,
                    );
                    return Task::perform(
                        async move { new_request.send_default().await },
                        |fiat_price| Message::Fiat(FiatMessage::GetPriceResult(fiat_price)),
                    );
                }
                Task::none()
            }
            Message::Fiat(FiatMessage::GetPriceResult(fiat_price)) => {
                if Some(&fiat_price.request) != self.cache.fiat_price_cache.last_request.as_ref() {
                    tracing::debug!(
                        "Ignoring fiat price result for {} from {} as it is not the last request",
                        fiat_price.currency(),
                        fiat_price.source(),
                    );
                    return Task::none();
                }
                if let Err(e) = fiat_price.res.as_ref() {
                    tracing::error!(
                        "Failed to get fiat price in {} from {}: {}",
                        fiat_price.currency(),
                        fiat_price.source(),
                        e
                    );
                }
                // Update the cache with the result even if there was an error.
                self.cache.fiat_price_cache.fiat_price = Some(fiat_price);
                Task::perform(async {}, |_| Message::CacheUpdated)
            }
            Message::UpdateDaemonCache(res) => {
                match res {
                    Ok(daemon_cache) => {
                        self.cache.daemon_cache = daemon_cache;
                        return Task::perform(async {}, |_| Message::CacheUpdated);
                    }
                    Err(e) => tracing::error!("Failed to update daemon cache: {}", e),
                }
                Task::none()
            }
            Message::CacheUpdated => {
                // These are the panels to update with the cache.
                let mut panels = [
                    (&mut self.panels.home as &mut dyn State, Menu::Home),
                    (&mut self.panels.settings as &mut dyn State, Menu::Settings),
                ];
                let daemon = self.daemon.clone();
                let current = &self.panels.current;
                let cache = self.cache.clone();
                let commands: Vec<_> = panels
                    .iter_mut()
                    .map(|(panel, menu)| {
                        panel.update(
                            daemon.clone(),
                            &cache,
                            Message::UpdatePanelCache(current == menu),
                        )
                    })
                    .collect();
                Task::batch(commands)
            }
            Message::LoadDaemonConfig(cfg) => {
                let res = self.load_daemon_config(self.cache.datadir_path.clone(), *cfg);
                self.update(Message::DaemonConfigLoaded(res))
            }
            Message::WalletUpdated(Ok(wallet)) => {
                self.wallet = wallet.clone();
                self.panels.current_mut().update(
                    self.daemon.clone(),
                    &self.cache,
                    Message::WalletUpdated(Ok(wallet)),
                )
            }
            Message::View(view::Message::Menu(menu)) => self.set_current_panel(menu),
            Message::View(view::Message::OpenUrl(url)) => {
                if let Err(e) = open::that_detached(&url) {
                    tracing::error!("Error opening '{}': {}", url, e);
                }
                Task::none()
            }
            Message::View(view::Message::Clipboard(text)) => clipboard::write(text),
            _ => self
                .panels
                .current_mut()
                .update(self.daemon.clone(), &self.cache, message),
        }
    }

    pub fn load_daemon_config(
        &mut self,
        datadir_path: LianaDirectory,
        cfg: DaemonConfig,
    ) -> Result<(), Error> {
        Handle::current().block_on(async { self.daemon.stop().await })?;
        let network = cfg.bitcoin_config.network;
        let daemon = EmbeddedDaemon::start(cfg)?;
        self.daemon = Arc::new(daemon);
        let mut daemon_config_path = datadir_path
            .network_directory(network)
            .lianad_data_directory(&self.wallet.id())
            .path()
            .to_path_buf();
        daemon_config_path.push("daemon.toml");

        let content =
            toml::to_string(&self.daemon.config()).map_err(|e| Error::Config(e.to_string()))?;

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

    pub fn view(&self) -> Element<Message> {
        let content = self.panels.current().view(&self.cache).map(Message::View);
        if self.cache.network != bitcoin::Network::Bitcoin {
            Column::with_children(vec![network_banner(self.cache.network).into(), content]).into()
        } else {
            content
        }
    }

    pub fn datadir_path(&self) -> &LianaDirectory {
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
