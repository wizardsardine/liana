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
    ReceivedQuotes(Vec<meld::api::Quote>, Option<String>),
    // Quote selection
    SelectQuote(usize),
    DeselectQuote,
    StartSessionPressed(usize),
    CreateWebviewSession(meld::api::Quote),
    SessionCreated(meld::api::CreateSessionResponse),
    // Webview specific messages
    ExtractWindowId(iced_wry::ExtractedWindowId),
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
        recommended_provider: Option<String>,
    },
    ActiveSession {
        session: meld::api::CreateSessionResponse,
        active: Option<iced_wry::IcedWebview>,
    },
}

pub struct MeldState {
    pub steps: Vec<MeldFlowStep>,
    pub buy_or_sell: BuyOrSell,
    pub country: &'static Country,

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
            Some(MeldFlowStep::ActiveSession { active, .. }) => {
                ui::webview_ux(active.as_ref(), network)
            }
            Some(MeldFlowStep::CountryNotSupported) => ui::not_supported_ux(&self.country),
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
                recommended_provider,
            }) => ui::quote_selection_ux(quotes, selected.clone(), recommended_provider.as_deref()),
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
            MeldMessage::SessionError(err, desc) => {
                if desc == "QUOTE_ACQUISITION_FAILED" {
                    if let Some(MeldFlowStep::InputForm {
                        sending_request, ..
                    }) = self.steps.last_mut()
                    {
                        *sending_request = false;
                    }
                };

                // TODO: Ideally, should be unified into global error interface
                log::error!("[MELD] ({}): {}", desc, err);
            }
            // initialization
            MeldMessage::CountryNotSupported => self.steps.push(MeldFlowStep::CountryNotSupported),
            MeldMessage::SetLimits(l) => {
                self.steps.push(MeldFlowStep::InputForm {
                    amount: l.minimum_amount.to_string(),
                    limits: l,
                    btc_balance: cache
                        .coins()
                        .into_iter()
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
                            let request = match buy_or_sell.clone() {
                                BuyOrSell::Sell => meld::api::GetQuotesRequest {
                                    country_code: country.code,
                                    state_code: None,
                                    destination_currency: country.currency.code,
                                    source_currency: "BTC",
                                    source_amount,
                                    // TODO: Why do we need to provide an address for `sell` quotes?
                                    wallet_address: Some(
                                        "3GetNke9R6dQNDP2TDrM3BxzPDcEQYkrNW".to_string(),
                                    ),
                                },
                                BuyOrSell::Buy { address } => meld::api::GetQuotesRequest {
                                    country_code: country.code,
                                    state_code: None,
                                    destination_currency: "BTC",
                                    source_currency: country.currency.code,
                                    source_amount,
                                    wallet_address: Some(address.address.to_string()),
                                },
                            };

                            let meld_client = meld::MeldClient(&client);
                            meld_client.get_quotes(request).await
                        },
                        |res| match res {
                            Ok(meld::api::GetQuoteResponse {
                                quotes,
                                message,
                                error,
                                recommended_provider,
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

                                MeldMessage::ReceivedQuotes(quotes, recommended_provider)
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
            MeldMessage::ReceivedQuotes(mut quotes, r) => {
                if let Some(MeldFlowStep::InputForm {
                    sending_request,
                    amount,
                    ..
                }) = self.steps.last_mut()
                {
                    *sending_request = false;

                    log::info!(
                        "[MELD] Successfully received quotes: {}, Recommended = {:?}",
                        quotes.len(),
                        r
                    );

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
                                view::BuySellMessage::Meld(MeldMessage::CreateWebviewSession(
                                    q.clone(),
                                )),
                            )));
                        }
                        _ => {
                            // TODO: Use Meld's recommended quote ranking
                            quotes.sort_by(|a, b| a.customer_score.total_cmp(&b.customer_score));
                            self.steps.push(MeldFlowStep::QuoteSelection {
                                quotes,
                                selected: None,
                                recommended_provider: r,
                            });
                        }
                    }
                }
            }
            // Quote Selection
            MeldMessage::SelectQuote(idx) => {
                if let Some(MeldFlowStep::QuoteSelection { selected, .. }) = self.steps.last_mut() {
                    *selected = Some(idx)
                }
            }
            MeldMessage::DeselectQuote => {
                if let Some(MeldFlowStep::QuoteSelection { selected, .. }) = self.steps.last_mut() {
                    *selected = None;
                }
            }
            MeldMessage::StartSessionPressed(selected) => {
                if let Some(MeldFlowStep::QuoteSelection { quotes, .. }) = self.steps.last() {
                    if let Some(quote) = quotes.get(selected) {
                        return Some(iced::Task::done(view::Message::BuySell(
                            view::BuySellMessage::Meld(MeldMessage::CreateWebviewSession(
                                quote.clone(),
                            )),
                        )));
                    }
                }
            }
            MeldMessage::CreateWebviewSession(pick) => {
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
                            quote_provider: &pick.service_provider,
                            source_amount: pick.source_amount,
                            source_currency: &pick.source_currency_code,
                            destination_currency: &pick.destination_currency_code,
                            country_code: country.code,
                            state_code: None,
                            wallet_address: match buy_or_sell {
                                BuyOrSell::Sell => {
                                    Some("3GetNke9R6dQNDP2TDrM3BxzPDcEQYkrNW".to_string())
                                }
                                BuyOrSell::Buy { address } => Some(address.address.to_string()),
                            },
                        };

                        let client = meld::MeldClient(&coincube_client);
                        client.create_session(req).await
                    },
                    |res| match res {
                        Ok(s) => MeldMessage::SessionCreated(s),
                        Err(e) => MeldMessage::SessionError(
                            e.to_string(),
                            "Unable to create Meld webview session",
                        ),
                    },
                )
                .map(|msg| view::Message::BuySell(view::BuySellMessage::Meld(msg)));

                return Some(task);
            }
            // webview session
            MeldMessage::SessionCreated(session) => {
                log::info!(
                    "[MELD] Successfully created Webview Session with ID: {}",
                    session.session_id
                );

                // start webview session
                self.steps.push(MeldFlowStep::ActiveSession {
                    active: None,
                    session,
                });

                // extract the main window's raw_window_handle, to instantiate a webview with
                return Some(iced_wry::extract_window_id(None).map(move |w| {
                    view::Message::BuySell(view::BuySellMessage::Meld(
                        MeldMessage::ExtractWindowId(w),
                    ))
                }));
            }
            MeldMessage::ExtractWindowId(id) => {
                if let Some(MeldFlowStep::ActiveSession { active, session }) = self.steps.last_mut()
                {
                    let attrs = iced_wry::wry::WebViewAttributes {
                        url: Some(session.widget_url.clone()),
                        devtools: cfg!(debug_assertions),
                        incognito: true,
                        ..Default::default()
                    };

                    match self.webview_manager.new_webview(attrs, id) {
                        Some(a) => *active = Some(a),
                        None => tracing::error!("Unable to instantiate wry webview"),
                    }
                } else {
                    unreachable!()
                }
            }
            MeldMessage::WebviewManagerUpdate(msg) => self.webview_manager.update(msg),
            // sse updates
            MeldMessage::EventSourceConnected => {
                log::info!("[MELD] Successfully connected to EventSource");
            }
            MeldMessage::EventSourceDisconnected(msg) => {
                log::warn!("[MELD] EventSource has disconnected: {}", msg);
            }
            MeldMessage::SseEvent(event) => {
                log::info!("[MELD] Got SSE event: {:#?}", event);
            }
        };

        None
    }
}
