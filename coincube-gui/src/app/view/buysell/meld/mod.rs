pub mod ui;

use super::panel::*;
use crate::{
    app::{view, Cache},
    services::{coincube::*, meld},
};
use coincube_core::miniscript::bitcoin;

#[derive(Debug, Clone)]
pub enum MeldMessage {
    NavigateBack,
    SessionError(String, &'static str),
    // Meld API messages
    SetLimits(meld::api::CurrencyLimit, Vec<meld::api::MeldRegion>),
    CountryNotSupported,
    // Region selection
    SetRegionFilter(String),
    SelectRegion(usize),
    ConfirmRegion,
    // Input form
    SetAmount(String),
    SetMaxAmount,
    SubmitInputForm(f64),
    ReceivedQuotes(Vec<meld::api::Quote>),
    // Quote selection
    SelectQuote(usize),
    StartSessionPressed(usize),
    CreateSession(meld::api::Quote),
    SessionCreated(meld::api::CreateSessionResponse),
    // Webview specific messages
    CreateWebviewSession(
        iced_wry::ExtractedWindowId,
        meld::api::CreateSessionResponse,
    ),
    WebviewManagerUpdate(iced_wry::IcedWryMessage),
    // Active session updates
    EventSourceConnected,
    EventSourceDisconnected(String),
    SseEvent(reqwest_sse::Event),
    // Clipboard
    CopyAddress(String),
    ClearToast,
}

pub enum MeldFlowStep {
    RegionChecks,
    NotSupported {
        msg: std::borrow::Cow<'static, str>,
    },
    RegionSelection {
        selected: Option<usize>,
        filter: String,
    },
    InputForm {
        amount: String,
        limits: meld::api::CurrencyLimit,
        btc_balance: bitcoin::Amount,
        sending_request: bool,
    },
    QuoteSelection {
        quotes: Vec<meld::api::Quote>,
        selected: Option<usize>,
        webview_pending: bool,
    },
    ActiveSession {
        idempotency_set: std::collections::BTreeSet<String>,
        session: meld::api::CreateSessionResponse,
        active: iced_wry::IcedWebview,
    },
}

pub struct MeldState {
    pub steps: Vec<MeldFlowStep>,
    pub buy_or_sell: BuyOrSell,
    pub country: &'static Country,
    pub network: bitcoin::Network,
    pub regions: Vec<meld::api::MeldRegion>,
    pub selected_region: Option<usize>,
    pub limits: Option<meld::api::CurrencyLimit>,

    // sse data
    pub sse_retries: usize,
    pub webview_manager: iced_wry::IcedWebviewManager,

    // toast notification
    pub toast: Option<String>,
}

impl MeldState {
    pub fn new(
        buy_or_sell: BuyOrSell,
        country: &'static Country,
        client: CoincubeClient,
        network: bitcoin::Network,
    ) -> (MeldState, iced::Task<view::Message>) {
        let state = MeldState {
            buy_or_sell: buy_or_sell.clone(),
            country,
            sse_retries: 0,
            webview_manager: iced_wry::IcedWebviewManager::new(),
            steps: vec![MeldFlowStep::RegionChecks],
            network,
            regions: Vec::new(),
            selected_region: None,
            limits: None,
            toast: None,
        };

        let task = iced::Task::perform(
            async move {
                let meld_client = meld::MeldClient(&client);
                let countries = meld_client.get_supported_countries().await?;

                let matching_country = countries
                    .into_iter()
                    .find(|c| c.country_code == country.code);

                let regions = match &matching_country {
                    Some(c) => c.regions.clone().unwrap_or_default(),
                    None => return Err(CoincubeError::Api("COUNTRY_NOT_SUPPORTED".to_string())),
                };

                // fetch limits
                let limits = match buy_or_sell {
                    BuyOrSell::Sell => {
                        meld_client
                            .get_crypto_sell_limits(country.currency.code, country.code)
                            .await
                    }
                    BuyOrSell::Buy { .. } => {
                        meld_client
                            .get_fiat_purchase_limits(country.currency.code, country.code)
                            .await
                    }
                }?;

                Ok(limits.first().cloned().map(|l| (l, regions)))
            },
            |res| match res {
                Ok(Some((l, regions))) => view::Message::BuySell(view::BuySellMessage::Meld(
                    MeldMessage::SetLimits(l, regions),
                )),
                Ok(None) => view::Message::BuySell(view::BuySellMessage::Meld(
                    MeldMessage::CountryNotSupported,
                )),
                Err(CoincubeError::Api(msg)) if msg.as_str() == "COUNTRY_NOT_SUPPORTED" => {
                    view::Message::BuySell(view::BuySellMessage::Meld(
                        MeldMessage::CountryNotSupported,
                    ))
                }
                Err(err) => {
                    view::Message::BuySell(view::BuySellMessage::Meld(MeldMessage::SessionError(
                        err.to_string(),
                        "Unable to acquire regional information",
                    )))
                }
            },
        );

        (state, task)
    }

    fn selected_state_code(&self) -> Option<String> {
        self.selected_region
            .and_then(|idx| self.regions.get(idx).map(|r| r.region_code.clone()))
    }

    fn push_input_form(&mut self, cache: &Cache) {
        let limits = self
            .limits
            .clone()
            .expect("limits must be set before input form");
        self.steps.push(MeldFlowStep::InputForm {
            amount: limits.minimum_amount.to_string(),
            limits,
            btc_balance: cache
                .coins()
                .iter()
                .filter(|c| c.block_height.is_some() && c.is_from_self)
                .map(|c| c.amount)
                .sum(),
            sending_request: false,
        });
    }

    pub(crate) fn view<'a>(&'a self) -> coincube_ui::widget::Element<'a, view::Message> {
        match &self.steps.last() {
            None | Some(MeldFlowStep::RegionChecks) => ui::region_checks_ux(),
            Some(MeldFlowStep::NotSupported { msg }) => ui::not_supported_ux(msg.as_ref()),
            Some(MeldFlowStep::RegionSelection { selected, filter }) => {
                ui::region_selection_ux(&self.regions, *selected, filter)
            }
            Some(MeldFlowStep::InputForm {
                amount,
                limits,
                btc_balance,
                sending_request,
            }) => ui::input_form_ux(
                amount,
                limits,
                btc_balance,
                &self.buy_or_sell,
                *sending_request,
            ),
            Some(MeldFlowStep::QuoteSelection {
                quotes,
                selected,
                webview_pending,
            }) => ui::quote_selection_ux(quotes, *selected, *webview_pending, &self.buy_or_sell),
            Some(MeldFlowStep::ActiveSession { active, .. }) => {
                let wallet_address = match &self.buy_or_sell {
                    BuyOrSell::Buy { address } => Some(address.address.to_string()),
                    BuyOrSell::Sell => None,
                };
                ui::webview_ux(active, wallet_address)
            }
        }
    }

    pub(crate) fn update<'a>(
        &'a mut self,
        msg: MeldMessage,
        cache: &Cache,
        client: &'a CoincubeClient,
    ) -> Option<iced::Task<view::Message>> {
        match msg {
            MeldMessage::NavigateBack => {
                if let Some(MeldFlowStep::ActiveSession { active, .. }) = self.steps.last() {
                    self.webview_manager.clear_view(active);
                }
                self.steps.pop();
            }
            MeldMessage::SessionError(mut err, description) => {
                #[derive(serde::Deserialize)]
                struct ErrorExtractor {
                    message: String,
                }

                // attempt to extract message from JSON response
                if let Ok(ErrorExtractor { message }) = serde_json::from_str::<ErrorExtractor>(&err)
                {
                    err = message;
                };

                let msg = match description {
                    "QUOTE_ACQUISITION_FAILED" => {
                        if let Some(MeldFlowStep::InputForm {
                            sending_request, ..
                        }) = self.steps.last_mut()
                        {
                            *sending_request = false;
                        }

                        format!("MELD | Unable to acquire Quotes from API | {}", err)
                    }
                    "WEBVIEW_INIT_FAILURE" => {
                        // reset webview loading state
                        if let Some(MeldFlowStep::QuoteSelection {
                            webview_pending, ..
                        }) = self.steps.last_mut()
                        {
                            *webview_pending = false;
                        }

                        format!("MELD | {}", err)
                    }
                    desc => format!("MELD | {} | {}", desc, err),
                };

                return Some(iced::Task::done(view::Message::ShowError(msg)));
            }
            // initialization
            MeldMessage::CountryNotSupported => self.steps.push(MeldFlowStep::NotSupported {
                msg: format!(
                    "Your country: {} currently isn't supported for BuySell",
                    self.country
                )
                .into(),
            }),
            MeldMessage::SetLimits(l, regions) => {
                self.regions = regions;
                self.limits = Some(l);

                if matches!(self.steps.last(), Some(MeldFlowStep::RegionChecks)) {
                    self.steps.pop();
                }

                if !self.regions.is_empty() {
                    self.steps.push(MeldFlowStep::RegionSelection {
                        selected: None,
                        filter: String::new(),
                    });
                } else {
                    self.push_input_form(cache);
                }
            }
            // Region Selection
            MeldMessage::SetRegionFilter(f) => {
                if let Some(MeldFlowStep::RegionSelection {
                    filter, selected, ..
                }) = self.steps.last_mut()
                {
                    *filter = f;
                    *selected = None;
                }
            }
            MeldMessage::SelectRegion(idx) => {
                if let Some(MeldFlowStep::RegionSelection { selected, .. }) = self.steps.last_mut()
                {
                    if *selected == Some(idx) {
                        *selected = None
                    } else {
                        *selected = Some(idx)
                    }
                }
            }
            MeldMessage::ConfirmRegion => {
                if let Some(&MeldFlowStep::RegionSelection {
                    selected: Some(idx),
                    ..
                }) = self.steps.last()
                {
                    self.selected_region = Some(idx);
                    self.push_input_form(cache);
                }
            }
            // input form
            MeldMessage::SetAmount(a) => {
                if let Some(MeldFlowStep::InputForm { amount, .. }) = self.steps.last_mut() {
                    *amount = a;
                }
            }
            MeldMessage::SetMaxAmount => {
                if let Some(MeldFlowStep::InputForm {
                    amount,
                    btc_balance,
                    ..
                }) = self.steps.last_mut()
                {
                    *amount = btc_balance.to_btc().to_string();
                }
            }
            MeldMessage::SubmitInputForm(source_amount) => {
                if let Some(MeldFlowStep::InputForm {
                    sending_request, ..
                }) = self.steps.last_mut()
                {
                    *sending_request = true;

                    let buy_or_sell = self.buy_or_sell.clone();
                    let country = self.country;
                    let client = client.clone();
                    let state_code = self.selected_state_code();

                    let task = iced::Task::perform(
                        async move {
                            let req = match buy_or_sell.clone() {
                                BuyOrSell::Sell => meld::api::GetQuotesRequest {
                                    session_type: meld::api::SessionType::Sell,
                                    country_code: country.code,
                                    state_code: state_code.as_deref(),
                                    destination_currency: country.currency.code,
                                    source_currency: "BTC",
                                    source_amount,
                                    wallet_address: None,
                                },
                                BuyOrSell::Buy { address } => meld::api::GetQuotesRequest {
                                    session_type: meld::api::SessionType::Buy,
                                    country_code: country.code,
                                    state_code: state_code.as_deref(),
                                    destination_currency: "BTC",
                                    source_currency: country.currency.code,
                                    source_amount,
                                    wallet_address: Some(address.address.to_string()),
                                },
                            };

                            let meld_client = meld::MeldClient(&client);
                            meld_client.get_quotes(req).await
                        },
                        |res| match res {
                            Ok(meld::api::GetQuoteResponse {
                                quotes,
                                message,
                                error,
                            }) => {
                                if let Some(e) = error {
                                    log::error!(
                                        "[MELD] Encountered an issue while getting quotes: {}",
                                        e
                                    )
                                };
                                if let Some(msg) = message {
                                    log::info!("[MELD] {}", msg)
                                };

                                MeldMessage::ReceivedQuotes(quotes)
                            }
                            Err(e) => {
                                MeldMessage::SessionError(e.to_string(), "QUOTE_ACQUISITION_FAILED")
                            }
                        },
                    )
                    .map(|msg| view::Message::BuySell(view::BuySellMessage::Meld(msg)));

                    return Some(task);
                }
            }
            MeldMessage::ReceivedQuotes(mut quotes) => {
                if let Some(MeldFlowStep::InputForm {
                    sending_request,
                    amount,
                    ..
                }) = self.steps.last_mut()
                {
                    *sending_request = false;
                    log::trace!("[MELD] Successfully received quotes: {}", quotes.len(),);

                    match quotes.as_slice() {
                        [] => {
                            let msg = format!(
                                "[MELD] No quotes available for {} and amount {}",
                                self.country, amount
                            );

                            return Some(iced::Task::done(view::Message::BuySell(
                                view::BuySellMessage::Meld(MeldMessage::SessionError(
                                    msg,
                                    "Please set your transaction amount within reasonable limits",
                                )),
                            )));
                        }
                        [q] => {
                            return Some(iced::Task::done(view::Message::BuySell(
                                view::BuySellMessage::Meld(MeldMessage::CreateSession(q.clone())),
                            )));
                        }
                        _ => {
                            // TODO: Use Meld's recommended quote ranking
                            quotes.sort_by(|a, b| b.customer_score.total_cmp(&a.customer_score));
                            self.steps.push(MeldFlowStep::QuoteSelection {
                                quotes,
                                selected: None,
                                webview_pending: false,
                            });
                        }
                    }
                }
            }
            // Quote Selection
            MeldMessage::SelectQuote(idx) => {
                if let Some(MeldFlowStep::QuoteSelection { selected, .. }) = self.steps.last_mut() {
                    if *selected == Some(idx) {
                        *selected = None
                    } else {
                        *selected = Some(idx)
                    }
                }
            }
            MeldMessage::StartSessionPressed(selected) => {
                if let Some(MeldFlowStep::QuoteSelection {
                    quotes,
                    webview_pending,
                    ..
                }) = self.steps.last_mut()
                {
                    if let Some(quote) = quotes.get(selected).cloned() {
                        *webview_pending = true;
                        return Some(iced::Task::done(view::Message::BuySell(
                            view::BuySellMessage::Meld(MeldMessage::CreateSession(quote)),
                        )));
                    }
                }
            }
            MeldMessage::CreateSession(pick) => {
                log::info!(
                    "[MELD] Starting session for provider: {:?}",
                    pick.service_provider
                );

                let state_code = self.selected_state_code();

                // setup request
                let coincube_client = client.clone();
                let country = self.country;
                let buy_or_sell = self.buy_or_sell.clone();

                let task = iced::Task::perform(
                    async move {
                        let req = meld::api::CreateSessionRequest {
                            session_type: match buy_or_sell {
                                BuyOrSell::Sell => meld::api::SessionType::Sell,
                                BuyOrSell::Buy { .. } => meld::api::SessionType::Buy,
                            },
                            quote_provider: &pick.service_provider,
                            source_amount: pick.source_amount,
                            source_currency: &pick.source_currency_code,
                            destination_currency: &pick.destination_currency_code,
                            country_code: country.code,
                            state_code: state_code.as_deref(),
                            wallet_address: match buy_or_sell {
                                BuyOrSell::Sell => None,
                                BuyOrSell::Buy { address } => Some(address.address.to_string()),
                            },
                        };

                        let client = meld::MeldClient(&coincube_client);
                        client.create_session(req).await
                    },
                    |res| match res {
                        Ok(s) => MeldMessage::SessionCreated(s),
                        Err(e) => MeldMessage::SessionError(e.to_string(), "WEBVIEW_INIT_FAILURE"),
                    },
                )
                .map(|msg| view::Message::BuySell(view::BuySellMessage::Meld(msg)));

                return Some(task);
            }
            // webview session
            MeldMessage::SessionCreated(session) => {
                // extract the main window's raw_window_handle, to instantiate a webview with
                return Some(iced_wry::extract_window_id(None).map(move |w| {
                    view::Message::BuySell(view::BuySellMessage::Meld(
                        MeldMessage::CreateWebviewSession(w, session.clone()),
                    ))
                }));
            }
            MeldMessage::CreateWebviewSession(id, session) => {
                let url = session
                    .service_provider_widget_url
                    .as_deref()
                    .unwrap_or(session.widget_url.as_str());

                let attrs = iced_wry::wry::WebViewAttributes {
                    url: Some(url.to_owned()),
                    devtools: cfg!(debug_assertions),
                    incognito: true,
                    ..Default::default()
                };

                match self.webview_manager.new_webview(attrs, id) {
                    Some(active) => {
                        log::trace!(
                            "[MELD] Successfully created Webview Session with ID: {}",
                            session.session_id
                        );

                        // reset webview loading state
                        if let Some(MeldFlowStep::QuoteSelection {
                            webview_pending, ..
                        }) = self.steps.last_mut()
                        {
                            *webview_pending = false;
                        }

                        self.steps.push(MeldFlowStep::ActiveSession {
                            idempotency_set: std::collections::BTreeSet::new(),
                            session,
                            active,
                        })
                    }
                    None => {
                        return Some(iced::Task::done(view::Message::BuySell(
                            view::BuySellMessage::Meld(MeldMessage::SessionError(
                                "Unable to start ActiveSession".into(),
                                "Webview failed to initialize, check logs for specific details",
                            )),
                        )));
                    }
                }
            }
            MeldMessage::WebviewManagerUpdate(msg) => self.webview_manager.update(msg),
            // sse updates
            MeldMessage::EventSourceConnected => {
                log::info!("[MELD] Successfully connected to EventSource");
            }
            MeldMessage::EventSourceDisconnected(msg) => {
                log::warn!("[MELD] EventSource has disconnected: {}", msg);

                // incrementing sse_retries updates the data hash for the subscription, thus recreating it
                self.sse_retries += 1;
            }
            MeldMessage::SseEvent(event) => {
                match serde_json::from_str::<meld::api::MeldEvent>(&event.data) {
                    Ok(ev) => {
                        // ensure event belongs to current session
                        if let Some(MeldFlowStep::ActiveSession {
                            session,
                            idempotency_set,
                            ..
                        }) = self.steps.last_mut()
                        {
                            if ev.meld_session_id != session.session_id {
                                log::debug!(
                                    "[MELD] Ignoring event received for previous session: {:?}",
                                    ev.meld_session_id
                                );

                                return None;
                            }

                            // check for duplicate events
                            if !idempotency_set.insert(ev.event_id) {
                                log::trace!("[MELD] Received duplicate SSE Event: {:?}", event);
                                return None;
                            }
                        }

                        // check if event was a success
                        match ev.status {
                            meld::api::TransactionStatus::Settled => {
                                // wait a few seconds before resetting widget
                                let task = iced::Task::perform(
                                    async move {
                                        tokio::time::sleep(std::time::Duration::from_secs(5)).await
                                    },
                                    |_| view::Message::BuySell(view::BuySellMessage::ResetWidget),
                                );

                                return Some(task);
                            }
                            meld::api::TransactionStatus::Error => {
                                let task = iced::Task::done(view::Message::BuySell(
                                    view::BuySellMessage::Meld(MeldMessage::SessionError(
                                        "Unable to complete transaction".to_string(),
                                        "Unable to settle transaction",
                                    )),
                                ));

                                return Some(task);
                            }
                            meld::api::TransactionStatus::Pending
                            | meld::api::TransactionStatus::Settling => {
                                log::info!("[MELD] Transaction is currently in flight");
                            }
                        }
                    }
                    Err(er) => {
                        log::error!(
                            "[MELD] Event data from SSE was malformed: {:?}\n{:?}",
                            er,
                            event
                        )
                    }
                }
            }
            MeldMessage::CopyAddress(address) => {
                self.toast = Some("Copied address to clipboard".to_string());
                let clear = iced::Task::perform(
                    async { tokio::time::sleep(std::time::Duration::from_secs(3)).await },
                    |_| view::Message::BuySell(view::BuySellMessage::Meld(MeldMessage::ClearToast)),
                );
                return Some(iced::Task::batch([iced::clipboard::write(address), clear]));
            }
            MeldMessage::ClearToast => {
                self.toast = None;
            }
        };

        None
    }

    /// Clears the active webview if one exists.
    pub fn clear_active_webview(&mut self) {
        if let Some(MeldFlowStep::ActiveSession { active, .. }) = self.steps.last() {
            self.webview_manager.clear_view(active);
        }
    }
}
