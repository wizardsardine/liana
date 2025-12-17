pub mod ui;

use super::panel::*;
use crate::app::view;
use crate::services::coincube::Country;
use coincube_core::miniscript::bitcoin;

#[derive(Debug, Clone)]
pub enum MeldMessage {
    // Webview specific messages
    StartWebview(String),
    InitWryWebviewWithUrl(iced_wry::ExtractedWindowId, String),
    WebviewManagerUpdate(iced_wry::IcedWryMessage),
}

pub enum MeldFlowStep {
    Initialization,
    WebviewRenderer { active: iced_wry::IcedWebview },
}

pub struct MeldState {
    pub step: MeldFlowStep,
    pub buy_or_sell: BuyOrSell,
    pub country: Country,

    pub webview_manager: iced_wry::IcedWebviewManager,
}

impl MeldState {
    pub fn new(buy_or_sell: BuyOrSell, country: Country) -> MeldState {
        MeldState {
            buy_or_sell,
            country,
            webview_manager: iced_wry::IcedWebviewManager::new(),
            step: MeldFlowStep::Initialization,
        }
    }

    pub(crate) fn view<'a>(
        &'a self,
        network: &'a bitcoin::Network,
    ) -> coincube_ui::widget::Element<'a, view::Message> {
        match &self.step {
            MeldFlowStep::Initialization => todo!(),
            MeldFlowStep::WebviewRenderer { active } => ui::webview_ux(active, network),
        }
    }

    pub(crate) fn update<'a>(&'a mut self, msg: MeldMessage) -> iced::Task<view::Message> {
        match msg {
            // initialize a webview
            MeldMessage::StartWebview(url) => {
                // extract the main window's raw_window_handle
                return iced_wry::extract_window_id(None).map(move |w| {
                    view::Message::BuySell(view::BuySellMessage::Meld(
                        MeldMessage::InitWryWebviewWithUrl(w, url.clone()),
                    ))
                });
            }
            MeldMessage::InitWryWebviewWithUrl(id, url) => {
                let attrs = iced_wry::wry::WebViewAttributes {
                    url: Some(url),
                    devtools: cfg!(debug_assertions),
                    incognito: true,
                    ..Default::default()
                };

                match self.webview_manager.new_webview(attrs, id) {
                    Some(active) => self.step = MeldFlowStep::WebviewRenderer { active },
                    None => tracing::error!("Unable to instantiate wry webview"),
                }
            }
            MeldMessage::WebviewManagerUpdate(msg) => self.webview_manager.update(msg),
        };

        iced::Task::none()
    }
}
