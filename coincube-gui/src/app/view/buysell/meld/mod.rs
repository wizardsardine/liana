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
    InitializeCountryData(meld::api::CurrencyLimit, Option<Vec<meld::api::MeldRegion>>),
    CountryNotSupported,
    // Region selection
    SetRegionFilter(String),
    SelectRegion(usize),
    ConfirmRegion,
    // Input form
    SetAmount(String),
    SetMaxAmount,
    SubmitInputAmount(f64),

    // Address selection (if in `buy` mode)
    ToggleAddressPicker,
    LoadMoreAddresses,
    ReceivedAddresses {
        addresses: Vec<LabelledAddress>,
        continue_from: Option<coincube_core::miniscript::bitcoin::bip32::ChildNumber>,
    },
    SelectExistingAddress(usize),
    CreateNewAddress,
    NewAddressCreated(coincubed::commands::GetAddressResult),
    CopyAddressToClipboard,

    GetQuotes,
    ReceivedQuotes(Vec<meld::api::Quote>),
    // Quote selection
    SelectQuote(usize),
    ConfirmSelectedQuote(usize),
    CreateSession(usize),
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
    NotSupported {
        msg: std::borrow::Cow<'static, str>,
    },
    RegionSelection {
        search: String,
        regions: Vec<meld::api::MeldRegion>,
        limits: meld::api::CurrencyLimit,
        selected: Option<usize>,
    },
    AmountInputForm {
        amount: String,
        limits: meld::api::CurrencyLimit,
        btc_balance: bitcoin::Amount,

        processing_request: bool,
    },
    AddressSelection {
        amount: f64,
        deposit_address: Option<(String, iced::widget::qr_code::Data)>,
        was_picked_from_existing_addresses: bool,

        // None = not loaded, Some([]) = empty
        existing_addresses: Option<Vec<LabelledAddress>>,
        addresses_continue_from: Option<bitcoin::bip32::ChildNumber>, // For pagination
        address_picker_open: bool,

        processing_request: bool,
    },
    QuoteSelection {
        quotes: Vec<meld::api::Quote>,
        selected: Option<usize>,
        webview_pending: bool,
    },
    ActiveSession {
        // only stores the hash of the event id in a BTreeSet
        event_idempotency_set: std::collections::BTreeSet<u64>,
        session: meld::api::CreateSessionResponse,
        active: iced_wry::IcedWebview,
        deposit_address: Option<String>,
    },
}

pub struct MeldState {
    pub steps: Vec<MeldFlowStep>,
    pub buy_or_sell: BuyOrSell,
    pub country: &'static Country,
    pub network: bitcoin::Network,

    // sse data
    pub sse_retries: usize,
    pub webview_manager: iced_wry::IcedWebviewManager,
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
        };

        let task = iced::Task::perform(
            async move {
                let meld_client = meld::MeldClient(&client);
                let countries = meld_client.get_supported_countries().await?;

                let matching_country = countries
                    .into_iter()
                    .find(|c| c.country_code == country.code);

                let regions = match matching_country {
                    Some(c) => c.regions,
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
                    MeldMessage::InitializeCountryData(l, regions),
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

    pub(crate) fn view<'a>(&'a self) -> coincube_ui::widget::Element<'a, view::Message> {
        match &self.steps.last() {
            None | Some(MeldFlowStep::RegionChecks) => ui::region_checks_ux(),
            Some(MeldFlowStep::NotSupported { msg }) => ui::not_supported_ux(msg.as_ref()),
            Some(MeldFlowStep::RegionSelection {
                selected,
                search,
                regions,
                ..
            }) => ui::region_selection_ux(&regions, *selected, search),
            Some(MeldFlowStep::AmountInputForm { .. }) => ui::input_form_ux(self),
            Some(MeldFlowStep::QuoteSelection {
                quotes,
                selected,
                webview_pending,
            }) => ui::quote_selection_ux(quotes, *selected, *webview_pending, &self.buy_or_sell),
            Some(MeldFlowStep::AddressSelection { .. }) => ui::address_selection_ux(self),
            Some(MeldFlowStep::ActiveSession {
                active,
                deposit_address,
                ..
            }) => ui::webview_ux(active, deposit_address.as_deref()),
        }
    }

    pub(crate) fn update<'a>(
        &'a mut self,
        msg: MeldMessage,
        cache: &Cache,
        daemon: Option<std::sync::Arc<dyn crate::daemon::Daemon + Sync + Send>>,
        client: &'a CoincubeClient,
    ) -> Option<iced::Task<view::Message>> {
        match msg {
            MeldMessage::NavigateBack => {
                if let Some(MeldFlowStep::ActiveSession { active, .. }) = self.steps.pop() {
                    self.webview_manager.clear_view(&active);
                }
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
                        if let Some(MeldFlowStep::AddressSelection {
                            processing_request, ..
                        }) = self.steps.last_mut()
                        {
                            *processing_request = false;
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
            MeldMessage::InitializeCountryData(limits, regions_) => {
                match regions_ {
                    Some(regions) => {
                        self.steps.push(MeldFlowStep::RegionSelection {
                            selected: None,
                            search: String::new(),
                            regions,
                            limits,
                        });
                    }
                    None => {
                        // automatically skip to input form
                        self.steps.push(MeldFlowStep::AmountInputForm {
                            amount: String::new(),
                            limits,
                            btc_balance: cache
                                .coins()
                                .iter()
                                .filter(|c| c.block_height.is_some() && c.is_from_self)
                                .map(|c| c.amount)
                                .sum(),
                            processing_request: false,
                        });
                    }
                }
            }

            // optional region Selection
            MeldMessage::SetRegionFilter(f) => {
                if let Some(MeldFlowStep::RegionSelection {
                    search: filter,
                    selected,
                    ..
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
                if let Some(MeldFlowStep::RegionSelection {
                    selected: Some(_idx),
                    limits,
                    ..
                }) = self.steps.last()
                {
                    self.steps.push(MeldFlowStep::AmountInputForm {
                        amount: String::new(),
                        limits: limits.clone(),
                        btc_balance: cache
                            .coins()
                            .iter()
                            .filter(|c| c.block_height.is_some() && c.is_from_self)
                            .map(|c| c.amount)
                            .sum(),
                        processing_request: false,
                    });
                }
            }

            // input form
            MeldMessage::SetAmount(a) => {
                if let Some(MeldFlowStep::AmountInputForm { amount, .. }) = self.steps.last_mut() {
                    *amount = a;
                }
            }
            MeldMessage::SetMaxAmount => {
                if let Some(MeldFlowStep::AmountInputForm {
                    amount,
                    btc_balance,
                    ..
                }) = self.steps.last_mut()
                {
                    *amount = btc_balance.to_btc().to_string();
                }
            }
            MeldMessage::SubmitInputAmount(btc_amount) => {
                if matches!(self.buy_or_sell, BuyOrSell::Buy) {
                    self.steps.push(MeldFlowStep::AddressSelection {
                        amount: btc_amount,
                        deposit_address: None,
                        was_picked_from_existing_addresses: false,
                        existing_addresses: None,
                        addresses_continue_from: None,
                        address_picker_open: false,
                        processing_request: false,
                    });

                    return Some(iced::Task::done(view::Message::BuySell(
                        view::BuySellMessage::Meld(MeldMessage::LoadMoreAddresses),
                    )));
                } else {
                    return Some(iced::Task::done(view::Message::BuySell(
                        view::BuySellMessage::Meld(MeldMessage::GetQuotes),
                    )));
                }
            }

            // address picker form
            MeldMessage::LoadMoreAddresses => {
                if matches!(self.buy_or_sell, BuyOrSell::Buy) {
                    if let Some(MeldFlowStep::AddressSelection {
                        addresses_continue_from,
                        ..
                    }) = self.steps.last_mut()
                    {
                        let daemon = daemon.expect("Daemon must be available for BuySell panel");
                        let start_index = addresses_continue_from.clone();

                        let task = iced::Task::perform(
                            async move {
                                daemon
                                    .list_revealed_addresses(false, false, 20, start_index)
                                    .await
                            },
                            |res| match res {
                                Ok(result) => {
                                    let addresses: Vec<LabelledAddress> = result
                                        .addresses
                                        .into_iter()
                                        // A new wallet always has index 0 "revealed", but we ignore it
                                        // as it was not generated by the user.
                                        .filter(|entry| entry.index != 0.into())
                                        .map(|entry| LabelledAddress {
                                            address: entry.address,
                                            index: entry.index,
                                            label: entry.label,
                                        })
                                        .collect();

                                    view::Message::BuySell(view::BuySellMessage::Meld(
                                        MeldMessage::ReceivedAddresses {
                                            addresses,
                                            continue_from: result.continue_from,
                                        },
                                    ))
                                }
                                Err(e) => {
                                    view::Message::BuySell(view::BuySellMessage::SessionError(
                                        "Unable to load addresses",
                                        e.to_string(),
                                    ))
                                }
                            },
                        );

                        return Some(task);
                    }
                }
            }
            MeldMessage::ReceivedAddresses {
                addresses,
                continue_from,
            } => {
                if let Some(MeldFlowStep::AddressSelection {
                    existing_addresses,
                    addresses_continue_from,
                    ..
                }) = self.steps.last_mut()
                {
                    *addresses_continue_from = continue_from;
                    match existing_addresses {
                        Some(e) => e.extend(addresses),
                        e => {
                            if addresses.is_empty() {
                                // generate a default deposit address
                                return Some(iced::Task::done(view::Message::BuySell(
                                    view::BuySellMessage::Meld(MeldMessage::CreateNewAddress),
                                )));
                            } else {
                                *e = Some(addresses)
                            };
                        }
                    }
                }
            }
            MeldMessage::ToggleAddressPicker => {
                if let Some(MeldFlowStep::AddressSelection {
                    address_picker_open,
                    ..
                }) = self.steps.last_mut()
                {
                    *address_picker_open = !*address_picker_open;
                }
            }
            MeldMessage::SelectExistingAddress(idx) => {
                if let Some(MeldFlowStep::AddressSelection {
                    existing_addresses: Some(addresses),
                    deposit_address,
                    address_picker_open,
                    was_picked_from_existing_addresses,
                    ..
                }) = self.steps.last_mut()
                {
                    if let Some(la) = addresses.get(idx).cloned() {
                        let address = la.address.to_string();
                        let qr_code_data =
                            iced::widget::qr_code::Data::new(address.as_bytes()).unwrap();

                        *deposit_address = Some((address, qr_code_data));
                        *address_picker_open = false;
                        *was_picked_from_existing_addresses = true;
                    }
                }
            }
            MeldMessage::CopyAddressToClipboard => {
                if let Some(address) = self.get_deposit_address() {
                    return Some(iced::Task::batch([
                        iced::Task::done(view::Message::Clipboard(address)),
                        iced::Task::done(view::Message::ShowError(
                            "Address copied to the clipboard".to_string(),
                        )),
                    ]));
                }
            }

            MeldMessage::CreateNewAddress => {
                let daemon = daemon.expect("Daemon must be available for BuySell panel");
                let task = iced::Task::perform(
                    async move { daemon.as_ref().get_new_address().await },
                    |res| match res {
                        Ok(new) => view::Message::BuySell(view::BuySellMessage::Meld(
                            MeldMessage::NewAddressCreated(new),
                        )),
                        Err(err) => view::Message::BuySell(view::BuySellMessage::SessionError(
                            "Error while creating new address",
                            err.to_string(),
                        )),
                    },
                );

                return Some(task);
            }
            MeldMessage::NewAddressCreated(coincubed::commands::GetAddressResult {
                address,
                ..
            }) => {
                if let Some(MeldFlowStep::AddressSelection {
                    deposit_address,
                    was_picked_from_existing_addresses,
                    ..
                }) = self.steps.last_mut()
                {
                    let address = address.to_string();
                    let qr_code_data =
                        iced::widget::qr_code::Data::new(address.as_bytes()).unwrap();
                    *was_picked_from_existing_addresses = false;
                    *deposit_address = Some((address, qr_code_data))
                }
            }

            MeldMessage::GetQuotes => {
                let (source_amount, deposit_address, processing) = match self.steps.last_mut() {
                    Some(MeldFlowStep::AddressSelection {
                        amount,
                        processing_request,
                        deposit_address: Some((address, _)),
                        ..
                    }) => (*amount, Some(address.clone()), processing_request),
                    Some(MeldFlowStep::AmountInputForm {
                        amount,
                        processing_request,
                        ..
                    }) => (amount.parse().unwrap(), None, processing_request),
                    _ => {
                        log::warn!("[MELD] Ignoring `GetQuotes` message, not in a valid state",);
                        return None;
                    }
                };

                *processing = true;

                let client = client.clone();

                let buy_or_sell = self.buy_or_sell.clone();
                let country = self.country;
                let state_code = self.get_region().map(|r| r.region_code.clone());

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
                            BuyOrSell::Buy => meld::api::GetQuotesRequest {
                                session_type: meld::api::SessionType::Buy,
                                country_code: country.code,
                                state_code: state_code.as_deref(),
                                destination_currency: "BTC",
                                source_currency: country.currency.code,
                                source_amount,
                                wallet_address: deposit_address.as_deref(),
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
            MeldMessage::ReceivedQuotes(mut quotes) => {
                let (deposit_address, processing) = match self.steps.last_mut() {
                    Some(MeldFlowStep::AddressSelection {
                        processing_request,
                        deposit_address,
                        ..
                    }) => (deposit_address.as_ref().map(|(a, _)| a), processing_request),
                    Some(MeldFlowStep::AmountInputForm {
                        processing_request, ..
                    }) => (None, processing_request),
                    _ => {
                        log::warn!("[MELD] Ignoring `GetQuotes` message, not in a valid state",);
                        return None;
                    }
                };

                *processing = false;
                log::trace!("[MELD] Successfully received quotes: {}", quotes.len(),);

                match quotes.as_slice() {
                    [] => {
                        let msg = format!("[MELD] No quotes available for {}", self.country);

                        return Some(iced::Task::done(view::Message::BuySell(
                            view::BuySellMessage::Meld(MeldMessage::SessionError(
                                msg,
                                "Please set your transaction amount within the expected limits",
                            )),
                        )));
                    }
                    [_] => {
                        return Some(iced::Task::done(view::Message::BuySell(
                            view::BuySellMessage::Meld(MeldMessage::CreateSession(0)),
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
            MeldMessage::ConfirmSelectedQuote(selected) => {
                if let Some(MeldFlowStep::QuoteSelection {
                    webview_pending, ..
                }) = self.steps.last_mut()
                {
                    *webview_pending = true;

                    return Some(iced::Task::done(view::Message::BuySell(
                        view::BuySellMessage::Meld(MeldMessage::CreateSession(selected)),
                    )));
                }
            }
            MeldMessage::CreateSession(pick) => {
                let Some(quotes) = self.steps.iter().rev().find_map(|a| match a {
                    MeldFlowStep::QuoteSelection { quotes, .. } => Some(quotes),
                    _ => None,
                }) else {
                    log::error!("Unable to create session, cannot find `QuoteSelection` data");
                    return None;
                };

                let Some(quote) = quotes.get(pick).cloned() else {
                    log::error!("Quote Index: {} is out of bounds", pick);
                    return None;
                };

                log::info!(
                    "[MELD] Starting session for provider: {:?}",
                    quote.service_provider
                );

                // setup request
                let coincube_client = client.clone();
                let country = self.country;
                let buy_or_sell = self.buy_or_sell.clone();

                let deposit_address = self.get_deposit_address();
                let region_code = self.get_region().map(|r| r.region_code.clone());

                let task = iced::Task::perform(
                    async move {
                        let req = meld::api::CreateSessionRequest {
                            session_type: match buy_or_sell {
                                BuyOrSell::Sell => meld::api::SessionType::Sell,
                                BuyOrSell::Buy => meld::api::SessionType::Buy,
                            },
                            quote_provider: &quote.service_provider,
                            source_amount: quote.source_amount,
                            source_currency: &quote.source_currency_code,
                            destination_currency: &quote.destination_currency_code,
                            country_code: country.code,
                            state_code: region_code.as_deref(),
                            wallet_address: deposit_address.as_deref(),
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
                        log::info!(
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
                            event_idempotency_set: std::collections::BTreeSet::new(),
                            deposit_address: self.get_deposit_address(),
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

                if let Some(MeldFlowStep::ActiveSession {
                    event_idempotency_set,
                    ..
                }) = self.steps.last_mut()
                {
                    // incrementing sse_retries updates the data hash for the subscription, thus recreating it
                    self.sse_retries += 1;
                    event_idempotency_set.clear();
                }
            }
            MeldMessage::SseEvent(event) => {
                match serde_json::from_str::<meld::api::MeldEvent>(&event.data) {
                    Ok(ev) => {
                        // ensure event belongs to current session
                        if let Some(MeldFlowStep::ActiveSession {
                            session,
                            event_idempotency_set,
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
                            let hash = {
                                use std::hash::Hasher;

                                let mut hash = std::hash::DefaultHasher::new();
                                hash.write(ev.event_id.as_bytes());
                                hash.finish()
                            };

                            if !event_idempotency_set.insert(hash) {
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
        };

        None
    }

    // utility functions
    fn get_region<'a>(&'a self) -> Option<&'a meld::api::MeldRegion> {
        self.steps.iter().find_map(|step| match step {
            MeldFlowStep::RegionSelection {
                regions,
                selected: Some(idx),
                ..
            } => regions.get(*idx),
            _ => None,
        })
    }

    fn get_deposit_address(&self) -> Option<String> {
        self.steps.iter().rev().find_map(|a| match a {
            MeldFlowStep::AddressSelection {
                deposit_address: Some((address, ..)),
                ..
            } => Some(address.to_owned()),
            _ => None,
        })
    }
}

impl Drop for MeldState {
    fn drop(&mut self) {
        // Clear the native webview before dropping the Meld state
        if let Some(MeldFlowStep::ActiveSession { active, .. }) = self.steps.last() {
            self.webview_manager.clear_view(active);
        }
    }
}
