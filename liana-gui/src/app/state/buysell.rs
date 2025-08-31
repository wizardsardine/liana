use iced::Task;
use std::{sync::Arc, time::Duration};

use iced_webview::{advanced::Action as WebviewAction, PageType};
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
    Action(iced_webview::advanced::Action),
    Created(iced_webview::ViewId),
}

/// Map webview messages to main app messages (static version for Task::map)
fn map_webview_message_static(webview_msg: WebviewMessage) -> Message {
    match webview_msg {
        WebviewMessage::Action(action) => {
            Message::View(ViewMessage::BuySell(BuySellMessage::WebviewAction(action)))
        }
        WebviewMessage::Created(id) => {
            Message::View(ViewMessage::BuySell(BuySellMessage::WebviewCreated(id)))
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
            BuySellMessage::ViewTick(id) => {
                let action = WebviewAction::Update(id);
                return self.webview.update(action).map(map_webview_message_static);
            }
            BuySellMessage::WebviewAction(action) => {
                return self.webview.update(action).map(map_webview_message_static);
            }
            BuySellMessage::WebviewOpenUrl(url) => {
                // Load URL into Ultralight webview
                tracing::info!("🌐 [LIANA] Loading Ultralight webview with URL: {}", url);
                self.session_url = Some(url.clone());

                // Create webview with URL string and immediately update to ensure content loads
                return self
                    .webview
                    .update(WebviewAction::CreateView(PageType::Url(url)))
                    .map(map_webview_message_static);
            }
            BuySellMessage::WebviewCreated(id) => {
                tracing::info!("🌐 [LIANA] Activating Webview Page: {}", id);

                // set active page to selected view id
                self.active_page = Some(id);
            }
            BuySellMessage::CloseWebview => {
                tracing::info!("🌐 [LIANA] Closing webview");

                if let Some(id) = self.active_page.take() {
                    return self
                        .webview
                        .update(WebviewAction::CloseView(id))
                        .map(map_webview_message_static);
                };
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
        if let Some(id) = self.active_page {
            // 24 FPS refresh rate
            return iced::time::every(Duration::from_millis(500))
                .with(id)
                .map(|(i, ..)| Message::View(ViewMessage::BuySell(BuySellMessage::ViewTick(i))));
        }

        iced::Subscription::none()
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
