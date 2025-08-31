use iced::Task;
use std::{sync::Arc, time::Duration};

use iced_webview::{Action as WebviewAction, PageType, WebView};
use liana_ui::widget::Element;

use crate::{
    app::{
        self,
        buysell::{
            meld::{MeldClient, MeldError},
            ServiceProvider,
        },
        cache::Cache,
        message::Message,
        state::State,
        view::{
            self,
            meld_buysell::{meld_buysell_view, BuySellPanel},
            BuySellMessage, Message as ViewMessage,
        },
        wallet::Wallet,
    },
    daemon::Daemon,
};

#[cfg(feature = "webview")]
#[derive(Debug, Clone)]
pub enum WebviewMessage {
    Action(iced_webview::Action),
    Created,
    UrlChanged(String),
}

/// Create optimized webview with performance settings
fn init_webview<E: iced_webview::Engine + Default>() -> iced_webview::WebView<E, WebviewMessage> {
    WebView::new()
        .on_create_view(WebviewMessage::Created)
        .on_url_change(WebviewMessage::UrlChanged)
}

/// Map webview messages to main app messages (static version for Task::map)
fn map_webview_message_static(webview_msg: WebviewMessage) -> Message {
    match webview_msg {
        WebviewMessage::Action(action) => {
            Message::View(ViewMessage::BuySell(BuySellMessage::WebviewAction(action)))
        }
        WebviewMessage::Created => {
            Message::View(ViewMessage::BuySell(BuySellMessage::WebviewCreated))
        }
        WebviewMessage::UrlChanged(url) => {
            Message::View(ViewMessage::BuySell(BuySellMessage::WebviewUrlChanged(url)))
        }
    }
}

impl Default for BuySellPanel {
    fn default() -> Self {
        Self::new(liana::miniscript::bitcoin::Network::Bitcoin)
    }
}

impl State for BuySellPanel {
    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, ViewMessage> {
        // Return the meld view directly - dashboard wrapper will be applied by app/mod.rs
        view::dashboard(&app::Menu::BuySell, cache, None, meld_buysell_view(self))
    }

    fn update(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        let Message::View(ViewMessage::BuySell(message)) = message else {
            return Task::none();
        };

        match message {
            BuySellMessage::WalletAddressChanged(address) => {
                self.set_wallet_address(address);
            }
            BuySellMessage::CountryCodeChanged(code) => {
                self.set_country_code(code);
            }
            BuySellMessage::SourceAmountChanged(amount) => {
                self.set_source_amount(amount);
            }

            BuySellMessage::OpenWidgetInNewWindow(widget_url) => {
                // Open in a new window/browser tab - similar to OpenWidget but explicitly for new window
                tracing::info!(
                    "Attempting to open widget URL in new window: {}",
                    widget_url
                );

                let mut success = false;

                // Method 1: Try open::that_detached first (non-blocking)
                match open::that_detached(&widget_url) {
                    Ok(_) => {
                        tracing::info!(
                            "Successfully opened widget URL in new window with detached method"
                        );
                        success = true;
                    }
                    Err(e) => {
                        tracing::warn!("Failed to open browser with detached method: {}", e);
                    }
                }

                // Method 2: Try WSL-specific commands first, then Linux commands
                if !success {
                    // WSL-specific commands (these work better in WSL)
                    let wsl_commands = [
                        ("cmd.exe", vec!["/c", "start", &widget_url]),
                        ("powershell.exe", vec!["-c", "Start-Process", &widget_url]),
                        ("explorer.exe", vec![&widget_url]),
                    ];

                    // Try WSL commands first
                    for (cmd, args) in &wsl_commands {
                        match std::process::Command::new(cmd).args(args).spawn() {
                            Ok(_) => {
                                tracing::info!("Successfully opened widget URL in new window with WSL command: {}", cmd);
                                success = true;
                                break;
                            }
                            Err(_) => {
                                tracing::debug!("WSL command {} not available", cmd);
                            }
                        }
                    }

                    // If WSL commands failed, try Linux commands
                    if !success {
                        let linux_commands = [
                            ("xdg-open", [&widget_url]),
                            ("firefox", [&widget_url]),
                            ("google-chrome", [&widget_url]),
                            ("chromium", [&widget_url]),
                            ("sensible-browser", [&widget_url]),
                        ];

                        for (cmd, args) in &linux_commands {
                            match std::process::Command::new(cmd).args(args).spawn() {
                                Ok(_) => {
                                    tracing::info!("Successfully opened widget URL in new window with Linux command: {}", cmd);
                                    success = true;
                                    break;
                                }
                                Err(_) => {
                                    tracing::debug!("Linux command {} not available", cmd);
                                }
                            }
                        }
                    }
                }

                if !success {
                    tracing::error!("All browser opening methods failed for new window");
                    self.set_error("Could not open browser automatically. Please copy the URL manually and paste it into your browser.".to_string());
                }
            }

            BuySellMessage::CreateSession => {
                if self.is_form_valid() {
                    tracing::info!(
                        "🚀 [MELD] Creating new session - clearing any existing session data"
                    );

                    // init session
                    let wallet_address = self.wallet_address.value.clone();
                    let country_code = self.country_code.value.clone();
                    let source_amount = self.source_amount.value.clone();

                    tracing::info!(
                        "🚀 [MELD] Making fresh API call with: address={}, country={}, amount={}",
                        wallet_address,
                        country_code,
                        source_amount
                    );

                    return Task::perform(
                        create_meld_session(
                            wallet_address,
                            country_code,
                            source_amount,
                            // Use Transak as the default payment provider
                            ServiceProvider::Transak,
                            self.network,
                        ),
                        |result| match result {
                            Ok(widget_url) => {
                                tracing::info!(
                                    "🌐 [LIANA] Meld session created with URL: {}",
                                    widget_url
                                );

                                Message::View(ViewMessage::BuySell(BuySellMessage::WebviewOpenUrl(
                                    widget_url,
                                )))
                            }
                            Err(error) => {
                                tracing::error!("❌ [MELD] Session creation failed: {}", error);
                                Message::View(ViewMessage::BuySell(BuySellMessage::SessionError(
                                    error,
                                )))
                            }
                        },
                    );
                } else {
                    tracing::warn!("⚠️ [MELD] Cannot create session - form validation failed");
                }
            }
            BuySellMessage::SessionError(error) => {
                self.set_error(error);
            }

            // webview logic
            BuySellMessage::WebviewAction(action) => {
                let webview = self.webview.get_or_insert_with(init_webview);
                return webview.update(action).map(map_webview_message_static);
            }
            BuySellMessage::WebviewOpenUrl(url) => {
                // Load URL into Ultralight webview
                tracing::info!("🌐 [LIANA] Loading Ultralight webview with URL: {}", url);

                self.webview_loading_start = Some(std::time::Instant::now());
                self.current_webview_url = Some(url.clone());

                // Create webview with URL string and immediately update to ensure content loads
                let webview = self.webview.get_or_insert_with(init_webview);

                // resize webview to 600x600
                let create_view = webview
                    .update(WebviewAction::CreateView(PageType::Url(url)))
                    .map(map_webview_message_static);
                let set_webview_index = webview
                    .update(WebviewAction::ChangeView(self.num_webviews))
                    .map(map_webview_message_static);
                let resize_task = webview
                    .update(WebviewAction::Resize(iced::Size::new(600, 600)))
                    .map(map_webview_message_static);

                return Task::batch([create_view, set_webview_index, resize_task]);
            }
            BuySellMessage::WebviewCreated => {
                tracing::info!("🌐 [LIANA] Webview created successfully");
                // self.webview_ready = true;

                // Increment view count and switch to the first view (following iced_webview example pattern)
                self.num_webviews += 1;
            }
            BuySellMessage::WebviewUrlChanged(url) => {
                tracing::info!("🌐 [LIANA] Webview URL changed to: {}", url);
                self.current_webview_url = Some(url);
            }
            BuySellMessage::CloseWebview => {
                tracing::info!("🌐 [LIANA] Closing webview");

                self.webview = None;
                self.webview_ready = false;
                self.current_webview_url = None;
                self.webview_loading_start = None;
                self.num_webviews = 0;
            }
        };

        Task::none()
    }

    fn reload(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _wallet: Arc<Wallet>,
    ) -> Task<Message> {
        Task::none()
    }

    fn subscription(&self) -> iced::Subscription<Message> {
        // Add webview update subscription for smooth rendering when webview is active
        if self.webview.is_some() && self.webview_ready {
            // 24 FPS refresh rate
            iced::time::every(Duration::from_millis(40)).map(|_| {
                Message::View(ViewMessage::BuySell(BuySellMessage::WebviewAction(
                    iced_webview::Action::Update,
                )))
            })
        } else {
            iced::Subscription::none()
        }
    }
}

async fn create_meld_session(
    wallet_address: String,
    country_code: String,
    source_amount: String,
    provider: ServiceProvider,
    network: liana::miniscript::bitcoin::Network,
) -> Result<String, String> {
    let client = MeldClient::new();

    match client
        .create_widget_session(
            wallet_address,
            country_code,
            source_amount,
            provider,
            network,
        )
        .await
    {
        Ok(response) => Ok(response.widget_url),
        Err(MeldError::Network(e)) => Err(format!("Network error: {}", e)),
        Err(MeldError::Serialization(e)) => Err(format!("Data error: {}", e)),
        Err(MeldError::Api(e)) => Err(format!("API error: {}", e)),
    }
}
