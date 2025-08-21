pub mod buysell;
pub mod cache;
pub mod config;
pub mod menu;
pub mod message;
pub mod settings;
pub mod state;
pub mod view;
pub mod wallet;

#[cfg(feature = "webview")]
mod webview_utils;

mod error;

use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

#[cfg(all(feature = "dev-coincube", not(feature = "dev-meld")))]
use crate::app::state::BuyAndSellPanel;

use iced::{clipboard, time, Subscription, Task};
use tokio::runtime::Handle;
use tracing::{error, info, warn};

#[cfg(feature = "webview")]
use iced_webview;

pub use liana::miniscript::bitcoin;
use liana_ui::{
    component::{network_banner},
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
        cache::{Cache, DaemonCache},
        error::Error,
        menu::Menu,
        settings::WalletId,
        wallet::Wallet,
    },
    daemon::{embedded::EmbeddedDaemon, Daemon, DaemonBackend},
    dir::LianaDirectory,
    node::{bitcoind::Bitcoind, NodeType},
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
    #[cfg(all(feature = "dev-coincube", not(feature = "dev-meld")))]
    buy_and_sell: BuyAndSellPanel,
    #[cfg(feature = "dev-meld")]
    meld_buy_and_sell: crate::app::view::meld_buysell::MeldBuySellPanel,
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
            #[cfg(all(feature = "dev-coincube", not(feature = "dev-meld")))]
            buy_and_sell: BuyAndSellPanel::new(),
            #[cfg(feature = "dev-meld")]
            meld_buy_and_sell: crate::app::view::meld_buysell::MeldBuySellPanel::new(cache.network),
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
            #[cfg(all(feature = "dev-coincube", not(feature = "dev-meld")))]
            Menu::BuyAndSell => &self.buy_and_sell,
            #[cfg(feature = "dev-meld")]
            Menu::BuyAndSell => &self.meld_buy_and_sell,
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
            #[cfg(all(feature = "dev-coincube", not(feature = "dev-meld")))]
            Menu::BuyAndSell => &mut self.buy_and_sell,
            #[cfg(feature = "dev-meld")]
            Menu::BuyAndSell => &mut self.meld_buy_and_sell,
        }
    }
}

pub struct App {
    cache: Cache,
    wallet: Arc<Wallet>,
    daemon: Arc<dyn Daemon + Sync + Send>,
    internal_bitcoind: Option<Bitcoind>,

    panels: Panels,

    // WebView for Meld widget integration
    #[cfg(feature = "webview")]
    meld_webview: Option<iced_webview::WebView<iced_webview::Ultralight, view::Message>>,

    // Flag to indicate when webview should be rendered instead of normal panels
    #[cfg(feature = "webview")]
    webview_mode: bool,

    // Flag to track webview loading state
    #[cfg(feature = "webview")]
    webview_loading: bool,

    // Current webview URL for display
    #[cfg(feature = "webview")]
    current_webview_url: Option<String>,
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
        let cmd = panels.home.reload(daemon.clone(), wallet.clone());
        (
            Self {
                panels,
                cache,
                daemon,
                wallet,
                internal_bitcoind,
                #[cfg(feature = "webview")]
                meld_webview: None,
                #[cfg(feature = "webview")]
                webview_mode: false,
                #[cfg(feature = "webview")]
                webview_loading: false,
                #[cfg(feature = "webview")]
                current_webview_url: None,
            },
            cmd,
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

    /// Check if webview is currently loading
    #[cfg(feature = "webview")]
    pub fn is_webview_loading(&self) -> bool {
        self.webview_loading
    }

    /// Create and load a webview with the given URL
    #[cfg(feature = "webview")]
    pub fn load_webview(&mut self, url: String) -> Task<Message> {
        tracing::info!("Loading webview with URL: {}", url);

        // Prevent multiple simultaneous webview creation attempts
        if self.webview_loading {
            tracing::warn!("Webview is already loading, ignoring request");
            return Task::none();
        }

        // Check if we already have an active webview
        if let Some(webview) = &mut self.meld_webview {
            tracing::info!("Reusing existing webview, loading new URL");
            self.webview_loading = true;
            // Reuse existing webview and just load the new URL
            let create_task = webview.update(iced_webview::Action::CreateView(iced_webview::PageType::Url(url.clone())));

            // CRITICAL: After creating a view, we must set it as the current view
            // Since we just created a view, it will be at the last index
            // For simplicity, we'll use index 0 which should work for most cases
            let change_task = webview.update(iced_webview::Action::ChangeView(0));

            self.webview_mode = true;
            // Store the current URL for display
            self.current_webview_url = Some(url.clone());
            tracing::info!("URL loaded in existing webview: {}", url);
            return Task::batch([create_task, change_task]).map(Message::View);
        }

        // Set loading state to prevent multiple creation attempts
        self.webview_loading = true;

        // No existing webview, create a new one
        tracing::info!("Creating new webview instance");

        // Create webview data directory within Liana's data directory
        let webview_data_dir = self.cache.datadir_path.path().join("webview_data");
        if let Err(e) = std::fs::create_dir_all(&webview_data_dir) {
            tracing::warn!("Failed to create webview data directory: {}", e);
        }

        // Store current directory and change to webview data directory
        let original_dir = std::env::current_dir().unwrap_or_default();
        if let Err(e) = std::env::set_current_dir(&webview_data_dir) {
            tracing::warn!("Failed to change to webview data directory: {}", e);
        }

        // Use a safer approach to create webview with error handling
        let webview_result = std::panic::catch_unwind(|| {
            iced_webview::WebView::<iced_webview::Ultralight, view::Message>::new()
                .on_create_view(view::Message::WebviewCreated)
                .on_url_change(view::Message::WebviewUrlChanged)
        });

        // Restore original directory
        if let Err(e) = std::env::set_current_dir(&original_dir) {
            tracing::warn!("Failed to restore original directory: {}", e);
        }

        match webview_result {
            Ok(webview) => {
                tracing::info!("Webview instance created successfully");

                // Store the webview in our app state first
                self.meld_webview = Some(webview);
                // Store the current URL for display
                self.current_webview_url = Some(url.clone());

                // Create a view with the URL - this will trigger WebviewViewCreated message
                if let Some(webview) = &mut self.meld_webview {
                    let create_task = webview.update(iced_webview::Action::CreateView(iced_webview::PageType::Url(url.clone())));
                    create_task.map(Message::View)
                } else {
                    Task::none()
                }
            }
            Err(e) => {
                tracing::error!("Failed to create webview instance: {:?}", e);
                tracing::info!("Falling back to external browser");

                // Reset loading state on failure
                self.webview_loading = false;

                // Fallback to external browser with original URL
                if let Err(e) = open::that_detached(&url) {
                    tracing::error!("Error opening '{}': {}", url, e);
                }
                Task::none()
            }
        }
    }



    /// Get a reference to the webview if it exists
    #[cfg(feature = "webview")]
    pub fn get_webview(&self) -> Option<&iced_webview::WebView<iced_webview::Ultralight, view::Message>> {
        self.meld_webview.as_ref()
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
        let mut subscriptions = vec![
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

        // Add webview subscription for periodic updates
        #[cfg(feature = "webview")]
        {
            if self.meld_webview.is_some() && self.webview_mode {
                subscriptions.push(
                    iced::time::every(std::time::Duration::from_millis(16)) // ~60 FPS
                        .map(|_| iced_webview::Action::Update)
                        .map(|action| Message::View(view::Message::WebviewAction(action)))
                );
            }
        }

        Subscription::batch(subscriptions)
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
            Message::View(view::Message::Menu(menu)) => {
                match menu {
                    menu::Menu::BuyAndSell => {
                        // Switch to buy/sell panel and show modal
                        self.panels.current = menu;
                        #[cfg(all(feature = "dev-coincube", not(feature = "dev-meld")))]
                        {
                            self.panels.buy_and_sell.show_modal();
                        }
                        #[cfg(feature = "dev-meld")]
                        {
                            // No need to show modal - form is always visible
                        }
                        Task::none()
                    }
                    _ => self.set_current_panel(menu),
                }
            }
            Message::View(view::Message::OpenUrl(url)) => {
                if let Err(e) = open::that_detached(&url) {
                    tracing::error!("Error opening '{}': {}", url, e);
                }
                Task::none()
            }
            Message::View(view::Message::Clipboard(text)) => clipboard::write(text),

            Message::View(view::Message::OpenWebview(url)) => {
                #[cfg(feature = "webview")]
                {
                    // Load URL into embedded webview
                    self.load_webview(url)
                }
                #[cfg(not(feature = "webview"))]
                {
                    // Fallback to external browser
                    if let Err(e) = open::that_detached(&url) {
                        tracing::error!("Error opening '{}': {}", url, e);
                    }
                    Task::none()
                }
            }
            #[cfg(feature = "webview")]
            Message::View(view::Message::WebviewAction(action)) => {
                // Handle webview actions (like URL loading, navigation, etc.)
                tracing::info!("Received webview action: {:?}", action);

                // Check if this is a page load completion to reset loading state
                match &action {
                    iced_webview::Action::Update => {
                        // Page is updating/loading
                    }
                    iced_webview::Action::Resize(size) => {
                        // Handle resize events - completely block excessive dimensions
                        if size.height > 1000 || size.width > 2000 || size.height == 4294967295 {
                            tracing::error!("Webview resize with excessive dimensions: {:?}, BLOCKING to prevent memory leak", size);

                            // Disable webview mode to prevent further issues
                            self.webview_mode = false;
                            self.webview_loading = false;

                            // Clear the webview instance to free memory
                            self.meld_webview = None;
                            self.current_webview_url = None;

                            tracing::info!("Webview disabled due to size issues. Please restart to try again.");
                            return Task::none(); // Block this resize completely
                        }
                        tracing::debug!("Webview resize: {:?}", size);
                    }
                    _ => {
                        // For other actions, assume loading is complete
                        if self.webview_loading {
                            tracing::info!("Webview loading completed");
                            self.webview_loading = false;
                        }
                    }
                }

                // Update the webview if it exists
                if let Some(webview) = &mut self.meld_webview {
                    webview.update(action).map(Message::View)
                } else {
                    Task::none()
                }
            }
            #[cfg(feature = "webview")]
            Message::View(view::Message::WebviewCreated) => {
                tracing::info!("Webview created successfully");
                self.webview_loading = false;
                Task::none()
            }

            #[cfg(feature = "webview")]
            Message::View(view::Message::WebviewUrlChanged(url)) => {
                tracing::info!("Webview URL changed to: {}", url);
                Task::none()
            }
            Message::View(view::Message::CloseWebview) => {
                #[cfg(feature = "webview")]
                {
                    // Close the webview by removing it from the app state
                    self.meld_webview = None;
                    self.webview_mode = false;
                    self.webview_loading = false;
                    self.current_webview_url = None;
                    tracing::info!("WebView closed by user");
                }
                Task::none()
            }



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
        // Check if we should show webview content within the buy/sell panel
        #[cfg(feature = "webview")]
        {
            if self.webview_mode && self.meld_webview.is_some() && matches!(self.panels.current, Menu::BuyAndSell) {
                // Show webview content within the dashboard layout for buy/sell panel
                let webview_content = if let Some(webview) = &self.meld_webview {
                    use iced::widget::{Container, Column, Row, Space, container};
                    use iced::{Length, Padding, Alignment};
                    use liana_ui::{color, component::text::text};

                    // Safety check: Don't render webview if we're still loading
                    let webview_element = if self.webview_loading {
                        // Show loading state instead of potentially problematic webview
                        Container::new(
                            Column::new()
                                .push(text("Loading webview...").size(16).color(color::GREY_2))
                                .push(Space::with_height(Length::Fixed(10.0)))
                                .push(text("Please wait while the content loads.").size(12).color(color::GREY_3))
                                .align_x(Alignment::Center)
                                .spacing(5)
                        )
                        .width(Length::Fill)
                        .height(Length::Fixed(580.0))
                        .center_x(Length::Fill)
                        .center_y(Length::Fixed(580.0))
                        .style(container::transparent)
                        .into()
                    } else {
                        // Render the webview using basic webview pattern (no ViewId needed)
                        webview.view().map(view::Message::WebviewAction)
                    };

                    // Create a container with close button and webview
                    let webview_container = Container::new(
                        Column::new()
                            .push(
                                // Header with close button
                                Row::new()
                                    .push(
                                        text("✕ Close Webview")
                                            .size(14)
                                            .color(color::GREY_2)
                                    )
                                    .push(Space::with_width(Length::Fill))
                                    .push(
                                        text(self.current_webview_url.as_deref().unwrap_or("Loading..."))
                                            .size(12)
                                            .color(color::GREY_3)
                                    )
                                    .align_y(Alignment::Center)
                                    .padding(Padding::new(10.0))
                            )
                            .push(
                                // The actual webview content with constrained size
                                Container::new(webview_element)
                                    .width(Length::Fill)
                                    .height(Length::Fixed(600.0)) // Fixed height to prevent overflow
                                    .style(container::transparent)
                            )
                            .spacing(0)
                    )
                    .width(Length::Fill)
                    .height(Length::Fixed(650.0)) // Fixed total height
                    .padding(Padding::ZERO);

                    // Wrap in dashboard layout
                    view::dashboard(
                        &Menu::BuyAndSell,
                        &self.cache,
                        None,
                        webview_container,
                    )
                } else {
                    // Fallback to normal panel view
                    self.panels.current().view(&self.cache)
                };

                if self.cache.network != bitcoin::Network::Bitcoin {
                    Column::with_children(vec![network_banner(self.cache.network).into(), webview_content.map(Message::View)]).into()
                } else {
                    webview_content.map(Message::View)
                }
            } else {
                // Normal panel view
                let content = self.panels.current().view(&self.cache).map(Message::View);
                if self.cache.network != bitcoin::Network::Bitcoin {
                    Column::with_children(vec![network_banner(self.cache.network).into(), content]).into()
                } else {
                    content
                }
            }
        }

        #[cfg(not(feature = "webview"))]
        {
            // Normal panel view when webview feature is disabled
            let content = self.panels.current().view(&self.cache).map(Message::View);
            if self.cache.network != bitcoin::Network::Bitcoin {
                Column::with_children(vec![network_banner(self.cache.network).into(), content]).into()
            } else {
                content
            }
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
