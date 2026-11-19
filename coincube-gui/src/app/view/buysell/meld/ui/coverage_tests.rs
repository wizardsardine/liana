use super::super::{MeldFlowStep, MeldMessage, MeldState};
use super::*;
use crate::app::view::buysell::{panel::LabelledAddress, BuyOrSell};
use crate::app::view::shared::landing::capture;
use crate::services::coincube::{get_countries, CoincubeClient};
use coincube_core::miniscript::bitcoin::{Address, Amount, Network, ScriptBuf};

fn limits() -> CurrencyLimit {
    CurrencyLimit {
        currency_code: "USD".into(),
        default_amount: 100.0,
        minimum_amount: 10.0,
        maximum_amount: 1000.0,
    }
}
fn state(mode: BuyOrSell, step: MeldFlowStep) -> MeldState {
    MeldState {
        steps: vec![step],
        buy_or_sell: mode,
        country: get_countries().iter().find(|c| c.code == "US").unwrap(),
        network: Network::Bitcoin,
        sse_retries: 0,
        webview_manager: super::super::WebviewManager::new(),
    }
}
fn inspect(content: coincube_ui::widget::Element<'_, view::Message>) -> capture::Inspection {
    capture::inspect_rendered_full_window(
        "meld-behavior",
        content,
        (1400, 1800),
        "dark",
        theme::Theme::dark(),
    )
}
fn region() -> Vec<MeldRegion> {
    vec![
        MeldRegion {
            region_code: "US-NY".into(),
            name: "New York".into(),
        },
        MeldRegion {
            region_code: "US-CA".into(),
            name: "California".into(),
        },
    ]
}
fn quote() -> Quote {
    Quote {
        transaction_type: TransactionType::CryptoPurchase,
        wallet_address: None,
        source_amount: 100.0,
        destination_amount: 0.001,
        exchange_rate: Some(100000.0),
        total_fee: 2.0,
        source_currency_code: "USD".into(),
        destination_currency_code: "BTC".into(),
        service_provider: "Synthetic Provider".into(),
        customer_score: 4.5,
    }
}
fn address() -> LabelledAddress {
    LabelledAddress {
        address: Address::p2wsh(&ScriptBuf::new(), Network::Bitcoin),
        index: 1.into(),
        label: Some("Synthetic deposit".into()),
    }
}
fn update(s: &mut MeldState, message: MeldMessage) {
    // Only synchronous state changes are executed; provider/daemon tasks are dropped.
    let _ = s.update(
        message,
        &crate::app::Cache::default(),
        None,
        &CoincubeClient::new(),
        &std::sync::Arc::new(crate::services::split_test_wallets::imported_vault(
            crate::services::split_test_wallets::SHAPES[0],
        )),
    );
}

#[test]
fn amount_form_rejects_nonfinite_out_of_range_and_malformed_values() {
    for (amount, expected) in [
        ("bad", "Improper number input"),
        ("NaN", "Now how did you manage"),
        ("inf", "Now how did you manage"),
        ("9", "Minimum Transaction"),
        ("1001", "Maximum Transaction"),
    ] {
        let s = state(
            BuyOrSell::Buy,
            MeldFlowStep::AmountInputForm {
                amount: amount.into(),
                limits: limits(),
                btc_balance: Amount::from_sat(100000),
                processing_request: false,
            },
        );
        let view = inspect(s.view());
        assert!(view.contains(expected), "{}: {:?}", amount, view.text);
        assert!(!view.contains("Select/Generate an Address for Deposit"));
    }
    for mode in [BuyOrSell::Buy, BuyOrSell::Sell] {
        let (amount, limits, expected_amount) = match mode {
            BuyOrSell::Buy => ("100", limits(), 100.0),
            BuyOrSell::Sell => (
                "0.0005",
                CurrencyLimit {
                    currency_code: "BTC".into(),
                    default_amount: 0.0005,
                    minimum_amount: 0.0001,
                    maximum_amount: 0.001,
                },
                0.0005,
            ),
        };
        for processing in [false, true] {
            let s = state(
                mode,
                MeldFlowStep::AmountInputForm {
                    amount: amount.into(),
                    limits: limits.clone(),
                    btc_balance: Amount::from_sat(100000),
                    processing_request: processing,
                },
            );
            let view = inspect(s.view());
            assert!(view.contains(&format!("Input {} amount", limits.currency_code)));
            let label = if mode == BuyOrSell::Buy {
                "Select/Generate an Address for Deposit"
            } else if processing {
                "Getting Quotes..."
            } else {
                "Get Quotes"
            };
            assert!(view.contains(label));
            let messages = capture::click_text(s.view(), label, 0, (1400, 1800));
            assert_eq!(messages.iter().any(|m| matches!(m,view::Message::BuySell(view::BuySellMessage::Meld(MeldMessage::SubmitInputAmount(v))) if *v==expected_amount)),!processing);
        }
    }
}

#[test]
fn region_filter_keeps_original_index_and_blocks_unselected_continue() {
    let regions = region();
    let view = inspect(region_selection_ux(&regions, None, "us-ca"));
    assert!(view.contains("California"));
    assert!(!view.contains("New York"));
    let messages = capture::click_text(
        region_selection_ux(&regions, None, "cal"),
        "California",
        0,
        (1400, 1800),
    );
    assert!(messages.iter().any(|m| matches!(
        m,
        view::Message::BuySell(view::BuySellMessage::Meld(MeldMessage::SelectRegion(1)))
    )));
    assert!(inspect(region_selection_ux(&regions, None, "missing"))
        .contains("No regions match your search"));
    assert!(inspect(region_selection_ux(&regions, Some(1), "")).contains("Continue"));
    let mut s = state(BuyOrSell::Buy, MeldFlowStep::RegionChecks);
    assert!(inspect(s.view()).contains("Checking for regional support"));
    update(
        &mut s,
        MeldMessage::InitializeCountryData(limits(), Some(regions)),
    );
    update(&mut s, MeldMessage::SelectRegion(1));
    update(&mut s, MeldMessage::SelectRegion(1));
    assert!(matches!(
        s.steps.last(),
        Some(MeldFlowStep::RegionSelection { selected: None, .. })
    ));
    update(&mut s, MeldMessage::SelectRegion(0));
    update(&mut s, MeldMessage::SetRegionFilter("cal".into()));
    assert!(matches!(
        s.steps.last(),
        Some(MeldFlowStep::RegionSelection { selected: None, .. })
    ));
    update(&mut s, MeldMessage::SelectRegion(1));
    update(&mut s, MeldMessage::ConfirmRegion);
    assert!(
        matches!(s.steps.last(),Some(MeldFlowStep::AmountInputForm {amount,..}) if amount=="10")
    );
    update(&mut s, MeldMessage::NavigateBack);
    update(&mut s, MeldMessage::CountryNotSupported);
    assert!(inspect(s.view()).contains("currently isn't supported"));
}

#[test]
fn quote_cards_preserve_fee_currency_and_gate_session_start() {
    let mut quotes = vec![quote()];
    for mode in [BuyOrSell::Buy, BuyOrSell::Sell] {
        quotes[0] = quote();
        if mode == BuyOrSell::Sell {
            quotes[0].transaction_type = TransactionType::CryptoSell;
            quotes[0].source_currency_code = "BTC".into();
            quotes[0].destination_currency_code = "USD".into();
            quotes[0].source_amount = 0.001;
            quotes[0].destination_amount = 100.0;
        }
        for selected in [None, Some(0)] {
            for pending in [false, true] {
                let view = inspect(quote_selection_ux(&quotes, selected, pending, &mode));
                assert!(view.contains("Synthetic Provider"));
                assert!(view.contains("2 USD"));
                assert_eq!(
                    view.contains("Start Session"),
                    selected.is_some() && !pending
                );
            }
        }
    }
    quotes[0] = quote();
    quotes[0].exchange_rate = None;
    assert!(
        !inspect(quote_selection_ux(&quotes, None, false, &BuyOrSell::Buy))
            .contains("Exchange Rate")
    );
    let messages = capture::click_text(
        quote_selection_ux(&quotes, Some(0), false, &BuyOrSell::Buy),
        "Start Session",
        0,
        (1400, 1800),
    );
    assert!(messages.iter().any(|m| matches!(
        m,
        view::Message::BuySell(view::BuySellMessage::Meld(
            MeldMessage::ConfirmSelectedQuote(0)
        ))
    )));
}

#[test]
fn address_selection_loads_appends_selects_and_displays_deposit_without_rpc() {
    let mut s = state(
        BuyOrSell::Buy,
        MeldFlowStep::AddressSelection {
            amount: 100.0,
            deposit_address: None,
            was_picked_from_existing_addresses: false,
            existing_addresses: None,
            addresses_continue_from: None,
            address_picker_open: false,
            processing_request: false,
        },
    );
    assert!(inspect(s.view()).contains("Loading addresses"));
    update(
        &mut s,
        MeldMessage::ReceivedAddresses {
            addresses: vec![address()],
            continue_from: Some(2.into()),
        },
    );
    update(&mut s, MeldMessage::ToggleAddressPicker);
    let view = inspect(s.view());
    assert!(view.contains("Synthetic deposit"));
    assert!(view.contains("Load more"));
    update(
        &mut s,
        MeldMessage::ReceivedAddresses {
            addresses: vec![address()],
            continue_from: None,
        },
    );
    assert!(
        matches!(s.steps.last(),Some(MeldFlowStep::AddressSelection {existing_addresses:Some(a),..}) if a.len()==2)
    );
    update(&mut s, MeldMessage::SelectExistingAddress(99));
    assert!(matches!(
        s.steps.last(),
        Some(MeldFlowStep::AddressSelection {
            deposit_address: None,
            ..
        })
    ));
    update(&mut s, MeldMessage::SelectExistingAddress(0));
    let view = inspect(s.view());
    assert!(view.contains("YOUR SELECTED ADDRESS:"));
    assert!(view.contains("Proceed"));
    assert!(matches!(
        s.steps.last(),
        Some(MeldFlowStep::AddressSelection {
            was_picked_from_existing_addresses: true,
            address_picker_open: false,
            ..
        })
    ));
    assert!(
        matches!(s.steps.last(), Some(MeldFlowStep::AddressSelection { deposit_address: Some((value, _)), .. }) if value == &address().address.to_string())
    );
    update(
        &mut s,
        MeldMessage::SessionError("synthetic failure".into(), "QUOTE_ACQUISITION_FAILED"),
    );
    assert!(matches!(
        s.steps.last(),
        Some(MeldFlowStep::AddressSelection {
            processing_request: false,
            ..
        })
    ));
}

#[test]
fn received_quotes_and_form_edits_preserve_back_navigation() {
    let mut s = state(BuyOrSell::Sell, MeldFlowStep::RegionChecks);
    update(&mut s, MeldMessage::InitializeCountryData(limits(), None));
    update(&mut s, MeldMessage::SetAmount("250".into()));
    assert!(
        matches!(s.steps.last(),Some(MeldFlowStep::AmountInputForm {amount,..}) if amount=="250")
    );
    update(&mut s, MeldMessage::SetMaxAmount);
    assert!(
        matches!(s.steps.last(),Some(MeldFlowStep::AmountInputForm {amount,..}) if amount=="0")
    );
    update(&mut s, MeldMessage::ReceivedQuotes(vec![quote(), quote()]));
    update(&mut s, MeldMessage::SelectQuote(0));
    assert!(matches!(
        s.steps.last(),
        Some(MeldFlowStep::QuoteSelection {
            selected: Some(0),
            ..
        })
    ));
    update(
        &mut s,
        MeldMessage::SessionError("{}".into(), "WEBVIEW_INIT_FAILURE"),
    );
    assert!(matches!(
        s.steps.last(),
        Some(MeldFlowStep::QuoteSelection {
            webview_pending: false,
            ..
        })
    ));
    update(&mut s, MeldMessage::NavigateBack);
    assert!(matches!(
        s.steps.last(),
        Some(MeldFlowStep::AmountInputForm { .. })
    ));
}

#[test]
fn sell_quote_failure_clears_amount_form_loading_state() {
    let mut s = state(
        BuyOrSell::Sell,
        MeldFlowStep::AmountInputForm {
            amount: "0.0005".into(),
            limits: limits(),
            btc_balance: Amount::from_sat(100000),
            processing_request: true,
        },
    );
    update(
        &mut s,
        MeldMessage::SessionError("synthetic failure".into(), "QUOTE_ACQUISITION_FAILED"),
    );
    assert!(matches!(
        s.steps.last(),
        Some(MeldFlowStep::AmountInputForm {
            processing_request: false,
            ..
        })
    ));
}

#[test]
fn empty_region_list_skips_region_selection() {
    let mut s = state(BuyOrSell::Buy, MeldFlowStep::RegionChecks);
    update(
        &mut s,
        MeldMessage::InitializeCountryData(limits(), Some(vec![])),
    );
    assert!(
        matches!(s.steps.last(), Some(MeldFlowStep::AmountInputForm { amount, .. }) if amount == "10")
    );
}

#[test]
fn address_pick_is_ignored_while_quote_request_is_in_flight() {
    let mut s = state(
        BuyOrSell::Buy,
        MeldFlowStep::AddressSelection {
            amount: 100.0,
            deposit_address: None,
            was_picked_from_existing_addresses: false,
            existing_addresses: Some(vec![address()]),
            addresses_continue_from: None,
            address_picker_open: true,
            processing_request: true,
        },
    );
    update(&mut s, MeldMessage::SelectExistingAddress(0));
    assert!(matches!(
        s.steps.last(),
        Some(MeldFlowStep::AddressSelection {
            deposit_address: None,
            was_picked_from_existing_addresses: false,
            address_picker_open: true,
            ..
        })
    ));
    if let Some(MeldFlowStep::AddressSelection {
        processing_request, ..
    }) = s.steps.last_mut()
    {
        *processing_request = false;
    }
    update(&mut s, MeldMessage::SelectExistingAddress(0));
    assert!(matches!(
        s.steps.last(),
        Some(MeldFlowStep::AddressSelection {
            deposit_address: Some(_),
            was_picked_from_existing_addresses: true,
            ..
        })
    ));
}

#[test]
fn sse_events_without_an_active_session_schedule_nothing() {
    let mut s = state(
        BuyOrSell::Buy,
        MeldFlowStep::QuoteSelection {
            quotes: vec![quote(), quote()],
            selected: Some(0),
            webview_pending: false,
        },
    );
    for (event_type, status) in [
        ("TRANSACTION_CRYPTO_COMPLETE", "SETTLED"),
        ("TRANSACTION_CRYPTO_FAILED", "ERROR"),
    ] {
        let data = serde_json::json!({
            "eventType": event_type,
            "eventId": format!("synthetic-{status}"),
            "meldSessionId": "synthetic-session",
            "transactionType": "CRYPTO_PURCHASE",
            "status": status,
        })
        .to_string();
        // guard against a vacuous pass through the malformed-event branch
        assert!(serde_json::from_str::<crate::services::meld::api::MeldEvent>(&data).is_ok());
        let task = s.update(
            MeldMessage::SseEvent(reqwest_sse::Event {
                event_type: "message".into(),
                data,
                last_event_id: None,
                retry: None,
            }),
            &crate::app::Cache::default(),
            None,
            &CoincubeClient::new(),
            &std::sync::Arc::new(crate::services::split_test_wallets::imported_vault(
                crate::services::split_test_wallets::SHAPES[0],
            )),
        );
        assert!(task.is_none(), "{}", status);
    }
    assert!(matches!(
        s.steps.last(),
        Some(MeldFlowStep::QuoteSelection {
            selected: Some(0),
            ..
        })
    ));
}
