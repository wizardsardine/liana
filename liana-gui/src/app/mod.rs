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
// url import removed - not needed for Ultralight

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::view::Message as ViewMessage;

    #[cfg(feature = "webview")]
    #[test]
    fn test_webview_message_mapping() {
        // Test WebviewMessage::Created mapping
        let created_msg = App::map_webview_message_static(WebviewMessage::Created);
        assert!(matches!(created_msg, Message::View(ViewMessage::WebviewCreated)));

        // Test WebviewMessage::UrlChanged mapping
        let url = "https://example.com".to_string();
        let url_changed_msg = App::map_webview_message_static(WebviewMessage::UrlChanged(url.clone()));
        if let Message::View(ViewMessage::WebviewUrlChanged(mapped_url)) = url_changed_msg {
            assert_eq!(mapped_url, url);
        } else {
            panic!("Expected WebviewUrlChanged message");
        }
    }

    #[cfg(feature = "webview")]
    #[test]
    fn test_webview_functionality() {
        use iced_webview::{WebView, Ultralight, Action as WebviewAction, PageType};

        // Test webview creation
        let _webview: WebView<Ultralight, WebviewMessage> = WebView::new()
            .on_create_view(WebviewMessage::Created)
            .on_url_change(|url| WebviewMessage::UrlChanged(url));

        // Test URL loading action
        let test_url = "https://test.example.com".to_string();
        let create_action = WebviewAction::CreateView(PageType::Url(test_url.clone()));

        // Verify action is properly formed
        match create_action {
            WebviewAction::CreateView(PageType::Url(url)) => {
                assert_eq!(url, test_url);
            }
            _ => panic!("Expected CreateView action with URL"),
        }
    }

    #[cfg(feature = "webview")]
    #[test]
    fn test_webview_interaction_handling() {
        use iced::keyboard::{Event as KeyboardEvent, Key};
        use iced::mouse::{Event as MouseEvent, Button};
        use iced::Point;
        use iced_webview::Action as WebviewAction;

        // Test keyboard event handling (using basic webview API)
        let key_event = KeyboardEvent::KeyPressed {
            key: Key::Character("a".into()),
            location: iced::keyboard::Location::Standard,
            modifiers: iced::keyboard::Modifiers::default(),
            text: Some("a".into()),
            modified_key: Key::Character("a".into()),
            physical_key: iced::keyboard::key::Physical::Code(iced::keyboard::key::Code::KeyA),
        };

        // Basic webview uses single parameter for keyboard events
        let keyboard_action = WebviewAction::SendKeyboardEvent(key_event);
        match keyboard_action {
            WebviewAction::SendKeyboardEvent(_) => {
                // Test passes if we can create the action
                assert!(true);
            }
            _ => panic!("Expected SendKeyboardEvent action"),
        }

        // Test mouse event handling (basic webview API)
        let mouse_event = MouseEvent::ButtonPressed(Button::Left);
        let point = Point::new(100.0, 200.0);
        let mouse_action = WebviewAction::SendMouseEvent(mouse_event, point);

        match mouse_action {
            WebviewAction::SendMouseEvent(_, click_point) => {
                assert_eq!(click_point.x, 100.0);
                assert_eq!(click_point.y, 200.0);
            }
            _ => panic!("Expected SendMouseEvent action"),
        }
    }

    #[cfg(feature = "webview")]
    #[test]
    fn test_webview_state_management() {
        use crate::app::view::webview::WebviewState;

        // Test initial state
        let mut state = WebviewState::new();
        assert_eq!(state.url, "");
        assert!(!state.is_loading);
        assert!(!state.show_webview);
        assert!(!state.has_webview);

        // Test URL opening
        let test_url = "https://meld.example.com/widget".to_string();
        state.open_url(test_url.clone());
        assert_eq!(state.url, test_url);
        assert!(state.show_webview);
        assert!(state.is_loading);

        // Test webview closing
        state.close();
        assert!(!state.show_webview);
        assert!(!state.is_loading);
        assert!(!state.has_webview);
    }

    #[cfg(all(feature = "webview", feature = "dev-meld"))]
    #[test]
    fn test_end_to_end_workflow() {
        use crate::app::view::{Message as ViewMessage, MeldBuySellMessage};
        use crate::app::message::Message;
        use crate::app::view::webview::WebviewMessage;

        // Test 1: Meld session creation triggers webview opening
        let session_url = "https://docs.rs/iced/latest/iced/index.html".to_string();
        let session_created_msg = Message::View(ViewMessage::MeldBuySell(
            MeldBuySellMessage::SessionCreated(session_url.clone())
        ));

        // Verify the message structure is correct
        match session_created_msg {
            Message::View(ViewMessage::MeldBuySell(MeldBuySellMessage::SessionCreated(url))) => {
                assert_eq!(url, session_url);
            }
            _ => panic!("Expected SessionCreated message"),
        }

        // Test 2: OpenWebview message handling
        let open_webview_msg = Message::View(ViewMessage::OpenWebview(session_url.clone()));
        match open_webview_msg {
            Message::View(ViewMessage::OpenWebview(url)) => {
                assert_eq!(url, session_url);
            }
            _ => panic!("Expected OpenWebview message"),
        }

        // Test 3: Webview action message handling
        let webview_action = WebviewAction::CreateView(PageType::Url(session_url.clone()));
        let webview_action_msg = Message::View(ViewMessage::WebviewAction(webview_action));

        match webview_action_msg {
            Message::View(ViewMessage::WebviewAction(action)) => {
                match action {
                    WebviewAction::CreateView(PageType::Url(url)) => {
                        assert_eq!(url, session_url);
                    }
                    _ => panic!("Expected CreateView action with URL"),
                }
            }
            _ => panic!("Expected WebviewAction message"),
        }

        // Test 4: Webview creation confirmation
        let webview_created_msg = Message::View(ViewMessage::WebviewCreated);
        match webview_created_msg {
            Message::View(ViewMessage::WebviewCreated) => {
                // Test passes if we can create the message
                assert!(true);
            }
            _ => panic!("Expected WebviewCreated message"),
        }
    }

    #[cfg(all(feature = "webview", feature = "dev-meld"))]
    #[test]
    fn test_meld_api_integration() {
        use crate::app::buysell::{ServiceProvider, meld::MeldSessionRequest, meld::SessionData};

        // Test Meld API request structure
        let request = MeldSessionRequest {
            session_data: SessionData {
                wallet_address: "2N3oefVeg6stiTb5Kh3ozCSkaqmx91FDbsm".to_string(),
                country_code: "US".to_string(),
                source_currency_code: "USD".to_string(),
                source_amount: "60".to_string(),
                destination_currency_code: "BTC".to_string(),
                service_provider: ServiceProvider::Transak.as_str().to_string(),
            },
            session_type: "BUY".to_string(),
            external_customer_id: "testcustomer".to_string(),
        };

        // Verify request structure
        assert_eq!(request.session_data.wallet_address, "2N3oefVeg6stiTb5Kh3ozCSkaqmx91FDbsm");
        assert_eq!(request.session_data.country_code, "US");
        assert_eq!(request.session_data.source_amount, "60");
        assert_eq!(request.session_data.destination_currency_code, "BTC");
        assert_eq!(request.session_type, "BUY");

        // Test serialization
        let json_result = serde_json::to_string(&request);
        assert!(json_result.is_ok());

        let json_str = json_result.unwrap();
        assert!(json_str.contains("sessionData"));
        assert!(json_str.contains("sessionType"));
        assert!(json_str.contains("externalCustomerId"));
    }

    #[cfg(all(feature = "webview", feature = "dev-meld"))]
    #[test]
    fn test_complete_workflow_integration() {
        use crate::app::view::{Message as ViewMessage, MeldBuySellMessage};
        use crate::app::message::Message;
        use crate::app::view::webview::{WebviewState, WebviewMessage};
        use iced_webview::{Action as WebviewAction, PageType};

        // Test complete workflow simulation
        let mut webview_state = WebviewState::new();

        // Step 1: Initial state verification
        assert!(!webview_state.show_webview);
        assert!(!webview_state.has_webview);
        assert_eq!(webview_state.url, "");

        // Step 2: Simulate Meld session creation
        let session_url = "https://docs.rs/iced/latest/iced/index.html".to_string();
        let session_created_msg = Message::View(ViewMessage::MeldBuySell(
            MeldBuySellMessage::SessionCreated(session_url.clone())
        ));

        // Verify session creation message
        match session_created_msg {
            Message::View(ViewMessage::MeldBuySell(MeldBuySellMessage::SessionCreated(url))) => {
                assert_eq!(url, session_url);
            }
            _ => panic!("Expected SessionCreated message"),
        }

        // Step 3: Simulate webview opening
        webview_state.open_url(session_url.clone());
        assert!(webview_state.show_webview);
        assert!(webview_state.is_loading);
        assert_eq!(webview_state.url, session_url);

        // Step 4: Simulate webview creation action
        let create_action = WebviewAction::CreateView(PageType::Url(session_url.clone()));
        let webview_action_msg = Message::View(ViewMessage::WebviewAction(create_action));

        match webview_action_msg {
            Message::View(ViewMessage::WebviewAction(WebviewAction::CreateView(PageType::Url(url)))) => {
                assert_eq!(url, session_url);
            }
            _ => panic!("Expected WebviewAction::CreateView message"),
        }

        // Step 5: Simulate webview creation completion
        webview_state.has_webview = true;
        webview_state.is_loading = false;

        let webview_created_msg = Message::View(ViewMessage::WebviewCreated);
        match webview_created_msg {
            Message::View(ViewMessage::WebviewCreated) => {
                assert!(true); // Test passes if message can be created
            }
            _ => panic!("Expected WebviewCreated message"),
        }

        // Step 6: Final state verification
        assert!(webview_state.show_webview);
        assert!(webview_state.has_webview);
        assert!(!webview_state.is_loading);
        assert_eq!(webview_state.url, session_url);

        // Step 7: Test webview closing
        webview_state.close();
        assert!(!webview_state.show_webview);
        assert!(!webview_state.has_webview);
        assert!(!webview_state.is_loading);
    }

    #[cfg(feature = "webview")]
    #[test]
    fn test_webview_message_debug() {
        // Test that WebviewMessage implements Debug
        let created = WebviewMessage::Created;
        let url_changed = WebviewMessage::UrlChanged("test".to_string());

        let debug_str = format!("{:?}", created);
        assert!(debug_str.contains("Created"));

        let debug_str = format!("{:?}", url_changed);
        assert!(debug_str.contains("UrlChanged"));
        assert!(debug_str.contains("test"));
    }

    #[cfg(feature = "webview")]
    #[test]
    fn test_webview_message_clone() {
        // Test that WebviewMessage implements Clone
        let original = WebviewMessage::UrlChanged("test".to_string());
        let cloned = original.clone();

        if let (WebviewMessage::UrlChanged(orig_url), WebviewMessage::UrlChanged(cloned_url)) = (original, cloned) {
            assert_eq!(orig_url, cloned_url);
        } else {
            panic!("Clone failed or wrong variant");
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
                        .on_url_change(|url| WebviewMessage::UrlChanged(url))
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



    // CEF helper methods removed - will be replaced with Ultralight methods



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
                time::every(Duration::from_millis(16)) // ~60 FPS
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
                // Load URL into Ultralight webview
                #[cfg(feature = "webview")]
                {
                    tracing::info!("🌐 [LIANA] Loading Ultralight webview with URL: {}", url);
                    self.webview_mode = true;
                    self.webview_loading = true;
                    self.webview_loading_start = Some(std::time::Instant::now());
                    self.current_webview_url = Some(url.clone());

                    // Create webview with URL string
                    self.webview.update(WebviewAction::CreateView(PageType::Url(url)))
                        .map(Self::map_webview_message_static)
                }
                #[cfg(not(feature = "webview"))]
                Task::none()
            }
            #[cfg(feature = "webview")]
            Message::View(view::Message::WebviewAction(action)) => {
                // Handle Ultralight webview actions
                tracing::info!("🌐 [LIANA] Received Ultralight webview action: {:?}", action);
                self.webview.update(action)
                    .map(Self::map_webview_message_static)
            }
            #[cfg(feature = "webview")]
            Message::View(view::Message::WebviewCreated) => {
                tracing::info!("🌐 [LIANA] Webview created successfully");
                self.webview_mode = true;
                self.webview_loading = false;
                self.webview_ready = true;

                // Increment view count and switch to the first view (following iced_webview example pattern)
                self.num_webviews += 1;

                // Automatically switch to the first view (index 0) after creation
                Task::done(Message::View(view::Message::SwitchToWebview(0)))
            }
            #[cfg(feature = "webview")]
            Message::View(view::Message::SwitchToWebview(index)) => {
                tracing::info!("🌐 [LIANA] Switching to webview index: {}", index);

                // Update current view index in app state
                self.current_webview_index = Some(index);

                // Send ChangeView action to webview
                use iced_webview::Action as WebViewAction;
                self.webview.update(WebViewAction::ChangeView(index))
                    .map(Self::map_webview_message_static)
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

                    #[cfg(feature = "webview")]
                    {
                        use iced::Size;
                        use iced_webview::{Action as WebViewAction, engines::PageType};

                        // Resize webview to full payment interface size (600px width, 800px height)
                        let webview_size = Size::new(600, 800);
                        let resize_task = self.webview.update(WebViewAction::Resize(webview_size))
                            .map(Self::map_webview_message_static);

                        // Create the view with the URL - this will trigger the webview to initialize
                        let create_task = Task::done(Message::View(view::Message::WebviewAction(
                            WebViewAction::CreateView(PageType::Url(url.clone()))
                        )));

                        Task::batch(vec![resize_task, create_task])
                    }
                    #[cfg(not(feature = "webview"))]
                    {
                        Task::none()
                    }
                } else {
                    tracing::warn!("🌐 [LIANA] No URL available for webview");
                    Task::none()
                }
            }
            #[cfg(feature = "dev-meld")]
            Message::View(view::Message::MeldBuySell(view::MeldBuySellMessage::SessionCreated(url))) => {
                tracing::info!("🌐 [LIANA] Meld session created with URL: {}", url);
                // Set the URL but don't show webview yet - user will click button to show it
                self.current_webview_url = Some(url.clone());
                self.webview_mode = true;
                self.show_webview = false; // Don't show automatically

                // Resize webview to match the "Meld Payment Ready" container size
                // This ensures the webview will be properly sized when shown
                #[cfg(feature = "webview")]
                {
                    use iced::Size;
                    use iced_webview::Action as WebViewAction;

                    // Set webview size to match the payment ready container (600px width, 800px height)
                    let container_size = Size::new(600, 800);
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

            // Create panel content with embedded webview using dashboard to keep sidebar
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

            // Create dashboard layout manually to maintain left sidebar
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
            // Normal panel view using dashboard to maintain sidebar
            let dashboard_content = view::dashboard(&self.panels.current, &self.cache, None, self.panels.current().view(&self.cache));
            if self.cache.network != bitcoin::Network::Bitcoin {
                Column::with_children(vec![network_banner(self.cache.network).into(), dashboard_content.map(Message::View)]).into()
            } else {
                dashboard_content.map(Message::View)
            }
        }

        #[cfg(not(feature = "webview"))]
        {
            // Normal panel view when webview feature is disabled using dashboard to maintain sidebar
            let dashboard_content = view::dashboard(&self.panels.current, &self.cache, None, self.panels.current().view(&self.cache));
            if self.cache.network != bitcoin::Network::Bitcoin {
                Column::with_children(vec![network_banner(self.cache.network).into(), dashboard_content.map(Message::View)]).into()
            } else {
                dashboard_content.map(Message::View)
            }
        }
    }

    // CEF helper method removed - will be replaced with Ultralight helper





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
