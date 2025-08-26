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

#[cfg(all(feature = "dev-coincube", not(feature = "dev-meld")))]
use crate::app::state::BuyAndSellPanel;

use iced::{clipboard, time, Subscription, Task, widget::Column};
use tokio::runtime::Handle;
use tracing::{error, info, warn};


// Ultralight webview imports
#[cfg(feature = "webview")]
use iced_webview::{WebView, Ultralight, Action as WebviewAction, PageType};

// Separate message type for webview that implements Clone
#[cfg(feature = "webview")]
#[derive(Debug, Clone)]
pub enum WebviewMessage {
    Action(WebviewAction),
    Created,
    UrlChanged(String),
}

pub use liana::miniscript::bitcoin;
use liana_ui::{
    component::network_banner,
    widget::Element,
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

    // Ultralight webview component for Meld widget integration with performance optimizations
    #[cfg(feature = "webview")]
    webview: WebView<Ultralight, WebviewMessage>,

    // Flag to indicate when webview should be rendered instead of normal panels
    webview_mode: bool,

    // Flag to track webview loading state
    webview_loading: bool,

    // Flag to track if webview is ready to be rendered (view has been created)
    webview_ready: bool,

    // Flag to control whether webview widget is shown
    show_webview: bool,

    // Current webview URL for display
    current_webview_url: Option<String>,

    // Timestamp when webview loading started (for timeout detection)
    webview_loading_start: Option<std::time::Instant>,

    // Webview management fields (following iced_webview example pattern)
    num_webviews: u32,
    current_webview_index: Option<u32>,
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
                webview: {
                    // Create optimized webview with performance settings
                    WebView::new()
                        .on_create_view(WebviewMessage::Created)
                        .on_url_change(WebviewMessage::UrlChanged)
                },
                webview_mode: false,
                webview_loading: false,
                webview_ready: false,
                show_webview: false,
                current_webview_url: None,
                webview_loading_start: None,
                num_webviews: 0,
                current_webview_index: None,
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
    pub fn is_webview_loading(&self) -> bool {
        self.webview_loading
    }



    /// Map webview messages to main app messages (static version for Task::map)
    #[cfg(feature = "webview")]
    fn map_webview_message_static(webview_msg: WebviewMessage) -> Message {
        match webview_msg {
            WebviewMessage::Action(action) => Message::View(view::Message::WebviewAction(action)),
            WebviewMessage::Created => Message::View(view::Message::WebviewCreated),
            WebviewMessage::UrlChanged(url) => Message::View(view::Message::WebviewUrlChanged(url)),
        }
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

        // Add webview update subscription for smooth rendering when webview is active
        #[cfg(feature = "webview")]
        if self.webview_mode && self.show_webview && self.webview_ready {
            subscriptions.push(
                time::every(Duration::from_secs(1)) // 1 second for faster loading detection and content updates
                    .map(|_| {
                        use iced_webview::Action as WebViewAction;
                        Message::View(view::Message::WebviewAction(WebViewAction::Update))
                    })
            );
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
            // CheckWebviewTimeout message handler removed - Ultralight handles timeouts internally
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
                // Load URL into Ultralight webview
                #[cfg(feature = "webview")]
                {
                    tracing::info!("🌐 [LIANA] Loading Ultralight webview with URL: {}", url);
                    self.webview_mode = true;
                    self.webview_loading = true;
                    self.webview_loading_start = Some(std::time::Instant::now());
                    self.current_webview_url = Some(url.clone());



                    // Create webview with URL string and immediately update to ensure content loads
                    let create_task = self.webview.update(WebviewAction::CreateView(PageType::Url(url)))
                        .map(Self::map_webview_message_static);

                    // Add immediate update to trigger content loading
                    let immediate_update = Task::done(Message::View(view::Message::WebviewAction(
                        WebviewAction::Update
                    )));

                    Task::batch(vec![create_task, immediate_update])
                }
                #[cfg(not(feature = "webview"))]
                Task::none()
            }
            #[cfg(feature = "webview")]
            Message::View(view::Message::WebviewAction(action)) => {
                // Handle webview-only actions - does NOT trigger full app update
                tracing::debug!("🌐 [LIANA] Processing webview-only action: {:?}", action);

                use iced_webview::Action as WebViewAction;

                // Determine if this action needs a follow-up update for rendering
                let needs_update = matches!(action,
                    WebViewAction::CreateView(_) |
                    WebViewAction::Resize(_) |
                    WebViewAction::GoToUrl(_)
                );

                let main_task = self.webview.update(action)
                    .map(Self::map_webview_message_static);

                if needs_update {
                    // Add a single update after actions that change content/size
                    let update_task = Task::done(Message::View(view::Message::WebviewAction(
                        WebViewAction::Update
                    )));
                    Task::batch(vec![main_task, update_task])
                } else {
                    main_task
                }
            }
            #[cfg(feature = "webview")]
            Message::View(view::Message::WebviewCreated) => {
                tracing::info!("🌐 [LIANA] Webview created successfully");
                self.webview_mode = true;
                self.webview_loading = false;
                self.webview_ready = true;

                // Increment view count and switch to the first view (following iced_webview example pattern)
                self.num_webviews += 1;

                // Switch to the first view and immediately update to display content
                let switch_task = Task::done(Message::View(view::Message::SwitchToWebview(0)));
                let update_task = Task::done(Message::View(view::Message::WebviewAction(
                    iced_webview::Action::Update
                )));
                Task::batch(vec![switch_task, update_task])
            }
            #[cfg(feature = "webview")]
            Message::View(view::Message::SwitchToWebview(index)) => {
                tracing::info!("🌐 [LIANA] Switching to webview index: {}", index);

                // Update current view index in app state
                self.current_webview_index = Some(index);

                // Send ChangeView action to webview and immediately update to display content
                use iced_webview::Action as WebViewAction;
                let change_task = self.webview.update(WebViewAction::ChangeView(index))
                    .map(Self::map_webview_message_static);
                let update_task = Task::done(Message::View(view::Message::WebviewAction(
                    WebViewAction::Update
                )));
                Task::batch(vec![change_task, update_task])
            }
            #[cfg(feature = "webview")]
            Message::View(view::Message::WebviewUrlChanged(url)) => {
                tracing::info!("🌐 [LIANA] Webview URL changed to: {}", url);
                self.current_webview_url = Some(url);
                Task::none()
            }

            // Duplicate handlers removed
            Message::View(view::Message::CloseWebview) => {
                tracing::info!("🌐 [LIANA] Closing webview");
                #[cfg(feature = "webview")]
                {
                    self.webview_mode = false;
                    self.webview_loading = false;
                    self.webview_ready = false;
                    self.show_webview = false;
                    self.current_webview_url = None;
                    self.webview_loading_start = None;
                    self.num_webviews = 0;
                    self.current_webview_index = None;
                }
                Task::none()
            }
            Message::View(view::Message::ShowWebView) => {
                tracing::info!("🌐 [LIANA] Showing webview");
                self.show_webview = true;

                // Create a webview with the current URL if available
                if let Some(url) = &self.current_webview_url {
                    tracing::info!("🌐 [LIANA] Creating webview with URL: {}", url);
                    self.webview_ready = false; // Mark as not ready until view is created
                    self.webview_loading = true; // Mark as loading
                    self.webview_loading_start = Some(std::time::Instant::now()); // Track loading start time

                    #[cfg(feature = "webview")]
                    {
                        use iced::Size;
                        use iced_webview::{Action as WebViewAction, PageType};

                        // Resize webview to match meld container size (800px width, 600px height for better fit)
                        let webview_size = Size::new(800, 600);
                        let resize_task = self.webview.update(WebViewAction::Resize(webview_size))
                            .map(Self::map_webview_message_static);

                        // Create the view with the URL - this will trigger the webview to initialize
                        let create_task = Task::done(Message::View(view::Message::WebviewAction(
                            WebViewAction::CreateView(PageType::Url(url.clone()))
                        )));

                        Task::batch(vec![resize_task, create_task])
                    }
                } else {
                    tracing::warn!("🌐 [LIANA] No URL available for webview");
                    Task::none()
                }
            }
            #[cfg(feature = "dev-meld")]
            Message::View(view::Message::MeldBuySell(view::MeldBuySellMessage::SessionCreated(url))) => {
                tracing::info!("🌐 [LIANA] Meld session created with URL: {}", url);
                // Set the URL and show webview immediately - no intermediate button needed
                self.current_webview_url = Some(url.clone());
                self.webview_mode = true;
                self.show_webview = true; // Show webview immediately

                // Initialize loading timer for smart update strategy
                self.webview_loading_start = Some(std::time::Instant::now());
                self.webview_loading = true;
                self.webview_ready = false;

                // Resize webview to match the "Meld Payment Ready" container size
                // This ensures the webview will be properly sized when shown
                #[cfg(feature = "webview")]
                {
                    use iced::Size;
                    use iced_webview::Action as WebViewAction;

                    // Set webview size to match the meld container (600px width, 600px height)
                    let container_size = Size::new(600, 600);
                    let resize_task = self.webview.update(WebViewAction::Resize(container_size))
                        .map(Self::map_webview_message_static);

                    let panel_update_task = self.panels
                        .current_mut()
                        .update(
                            self.daemon.clone(),
                            &self.cache,
                            Message::View(view::Message::MeldBuySell(view::MeldBuySellMessage::SessionCreated(url)))
                        );

                    Task::batch(vec![resize_task, panel_update_task])
                }
                #[cfg(not(feature = "webview"))]
                {
                    self.panels
                        .current_mut()
                        .update(
                            self.daemon.clone(),
                            &self.cache,
                            Message::View(view::Message::MeldBuySell(view::MeldBuySellMessage::SessionCreated(url)))
                        )
                }
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

    pub fn view(&self) -> Element<'_, Message> {
        // Check if we should show embedded webview within the buy/sell panel
        #[cfg(feature = "webview")]
        if self.webview_mode && matches!(self.panels.current, menu::Menu::BuyAndSell) {
            // Create webview content that will be embedded below the "Previous" button
            let webview_widget = {
                use crate::app::view::webview::meld_webview_widget_ultralight;
                Some(meld_webview_widget_ultralight(
                    Some(&self.webview),
                    self.current_webview_url.as_deref(),
                    self.show_webview,
                    self.webview_ready,
                    self.webview_loading,
                    self.current_webview_index,
                ))
            };

            // Use meld buy/sell view with embedded webview for Buy/Sell panel in webview mode
            let panel_content = {
                use crate::app::view::meld_buysell::meld_buysell_view_with_webview;
                #[cfg(feature = "dev-meld")]
                {
                    meld_buysell_view_with_webview(&self.panels.meld_buy_and_sell, webview_widget)
                }
                #[cfg(not(feature = "dev-meld"))]
                {
                    // Fallback to normal panel view if meld feature is not enabled
                    self.panels.current().view(&self.cache)
                }
            };

            // Apply dashboard layout once to maintain left sidebar
            let dashboard_content = view::dashboard(&self.panels.current, &self.cache, None, panel_content);

            if self.cache.network != bitcoin::Network::Bitcoin {
                Column::with_children(vec![
                    network_banner(self.cache.network).into(),
                    dashboard_content.map(Message::View)
                ]).into()
            } else {
                dashboard_content.map(Message::View)
            }
        } else {
            // Normal panel view without webview
            let panel_content = self.panels.current().view(&self.cache);

            // Buy/Sell panel needs dashboard wrapper, other panels already have it internally
            let final_view = if matches!(self.panels.current, menu::Menu::BuyAndSell) {
                // Apply dashboard wrapper for Buy/Sell panel
                view::dashboard(&self.panels.current, &self.cache, None, panel_content)
            } else {
                // Other panels already apply dashboard() internally
                panel_content
            };

            if self.cache.network != bitcoin::Network::Bitcoin {
                Column::with_children(vec![
                    network_banner(self.cache.network).into(),
                    final_view.map(Message::View)
                ]).into()
            } else {
                final_view.map(Message::View)
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
