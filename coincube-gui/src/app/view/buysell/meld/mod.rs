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
    SetLimits(meld::api::CurrencyLimit),
    CountryNotSupported,
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
}

pub enum MeldFlowStep {
    RegionChecks,
    CountryNotSupported,
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
        session: meld::api::CreateSessionResponse,
        active: iced_wry::IcedWebview,
    },
}

pub struct MeldState {
    pub steps: Vec<MeldFlowStep>,
    pub buy_or_sell: BuyOrSell,
    pub country: &'static Country,

    // sse data
    pub idempotency_set: std::collections::BTreeSet<String>,
    pub sse_retries: usize,

    pub webview_manager: iced_wry::IcedWebviewManager,
}

impl MeldState {
    pub fn new(
        buy_or_sell: BuyOrSell,
        country: &'static Country,
        client: CoincubeClient,
    ) -> (MeldState, iced::Task<view::Message>) {
        let state = MeldState {
            buy_or_sell: buy_or_sell.clone(),
            country,
            idempotency_set: std::collections::BTreeSet::new(),
            sse_retries: 0,
            webview_manager: iced_wry::IcedWebviewManager::new(),
            steps: vec![MeldFlowStep::RegionChecks],
        };

        let task = iced::Task::perform(
            async move {
                let meld_client = meld::MeldClient(&client);
                let countries = meld_client.get_supported_countries().await?;

                if !countries.iter().any(|c| c.country_code == country.code) {
                    return Err(CoincubeError::Api("COUNTRY_NOT_SUPPORTED".to_string()));
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

                Ok(limits.first().cloned())
            },
            |res| match res {
                Ok(Some(l)) => {
                    view::Message::BuySell(view::BuySellMessage::Meld(MeldMessage::SetLimits(l)))
                }
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

    pub(crate) fn view<'a>(
        &'a self,
        network: &'a bitcoin::Network,
    ) -> coincube_ui::widget::Element<'a, view::Message> {
        match &self.steps.last() {
            None | Some(MeldFlowStep::RegionChecks) => ui::region_checks_ux(),
            Some(MeldFlowStep::ActiveSession { active, .. }) => ui::webview_ux(active, network),
            Some(MeldFlowStep::CountryNotSupported) => ui::not_supported_ux(self.country),
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
                self.steps.pop();
            }
            MeldMessage::SessionError(err, description) => {
                let msg = match description {
                    "QUOTE_ACQUISITION_FAILED" => {
                        if let Some(MeldFlowStep::InputForm {
                            sending_request, ..
                        }) = self.steps.last_mut()
                        {
                            *sending_request = false;
                        }

                        format!("[MELD] (Unable to acquire Quotes from API): {}", err)
                    }
                    "WEBVIEW_INIT_FAILURE" => {
                        // reset webview loading state
                        if let Some(MeldFlowStep::QuoteSelection {
                            webview_pending, ..
                        }) = self.steps.last_mut()
                        {
                            *webview_pending = false;
                        }

                        format!("[MELD] (Unable to start webview): {}", err)
                    }
                    desc => format!("[MELD] ({}): {}", desc, err),
                };

                return Some(iced::Task::done(view::Message::ShowError(msg)));
            }
            // initialization
            MeldMessage::CountryNotSupported => self.steps.push(MeldFlowStep::CountryNotSupported),
            MeldMessage::SetLimits(l) => {
                self.steps.push(MeldFlowStep::InputForm {
                    amount: l.minimum_amount.to_string(),
                    limits: l,
                    btc_balance: cache
                        .coins()
                        .iter()
                        .filter(|c| c.block_height.is_some() && c.is_from_self)
                        .map(|c| c.amount)
                        .sum(),
                    sending_request: false,
                });
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

                    let task = iced::Task::perform(
                        async move {
                            // TODO: include us state-code input (if in the US)
                            let req = match buy_or_sell.clone() {
                                BuyOrSell::Sell => meld::api::GetQuotesRequest {
                                    session_type: meld::api::SessionType::Sell,
                                    country_code: country.code,
                                    state_code: None,
                                    destination_currency: country.currency.code,
                                    source_currency: "BTC",
                                    source_amount,
                                    wallet_address: None,
                                },
                                BuyOrSell::Buy { address } => meld::api::GetQuotesRequest {
                                    session_type: meld::api::SessionType::Buy,
                                    country_code: country.code,
                                    state_code: None,
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
                    if let Some(quote) = quotes.get(selected) {
                        *webview_pending = true;

                        return Some(iced::Task::done(view::Message::BuySell(
                            view::BuySellMessage::Meld(MeldMessage::CreateSession(quote.clone())),
                        )));
                    }
                }
            }
            MeldMessage::CreateSession(pick) => {
                log::info!(
                    "[MELD] Starting session for provider: {:?}",
                    pick.service_provider
                );

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
                            state_code: None,
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

                        self.steps
                            .push(MeldFlowStep::ActiveSession { session, active })
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
                        // check for duplicate events
                        if !self.idempotency_set.insert(ev.event_id) {
                            log::trace!("[MELD] Received duplicate SSE Event: {:?}", event);
                            return None;
                        }

                        // ensure event belongs to current session
                        if let Some(MeldFlowStep::ActiveSession { session, .. }) = self.steps.last()
                        {
                            if ev.meld_session_id != session.session_id {
                                log::debug!(
                                    "[MELD] Ignoring event received for previous session: {:?}",
                                    ev.meld_session_id
                                );

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
        };

        None
    }
}
