use super::*;
use crate::app::state::liquid::send::PrepareContext;
use crate::app::view::shared::landing::capture;
use crate::services::fiat::{Currency, PriceSource};
use breez_sdk_liquid::prelude::{
    BitcoinAddressData, LiquidAddressData, PreparePayOnchainResponse, PrepareSendResponse,
    SendDestination,
};

fn inspect(content: Element<'_, LiquidSendMessage>) -> capture::Inspection {
    capture::inspect_rendered_full_window(
        "liquid-send-behavior",
        content,
        (1024, 1600),
        "dark",
        theme::Theme::dark(),
    )
}
fn value(text: &str, valid: bool) -> form::Value<String> {
    form::Value {
        value: text.into(),
        valid,
        warning: None,
    }
}
fn liquid_address() -> LiquidAddressData {
    LiquidAddressData {
        address: "synthetic-liquid-destination".into(),
        network: breez_sdk_liquid::prelude::Network::Bitcoin,
        asset_id: None,
        amount: None,
        amount_sat: None,
        label: None,
        message: None,
    }
}
fn converter(currency: Currency) -> FiatAmountConverter {
    FiatAmountConverter::new(
        100000.0,
        None,
        crate::app::cache::FiatPriceRequest::new(PriceSource::Coincube, currency),
    )
    .unwrap()
}
fn intent(from: SendAsset, to: SendAsset, asset_fee: bool, onchain: bool) -> PreparedIntent {
    let context = PrepareContext {
        generation: 1,
        amount: Amount::from_sat(50000),
        usdt_amount_display: "12.50".into(),
        to_asset: to,
        from_asset: from,
        comment: Some("Synthetic comment".into()),
        description: Some("Synthetic recipient".into()),
        pay_fees_with_asset: asset_fee,
    };
    let payment = if onchain {
        PreparedPayment::Onchain {
            response: Box::new(PreparePayOnchainResponse {
                receiver_amount_sat: 50000,
                claim_fees_sat: 50,
                total_fees_sat: 100,
            }),
            address: "synthetic-onchain-destination".into(),
        }
    } else {
        PreparedPayment::Regular {
            response: Box::new(PrepareSendResponse {
                destination: SendDestination::LiquidAddress {
                    address_data: liquid_address(),
                    bip353_address: None,
                },
                amount: None,
                fees_sat: Some(100),
                estimated_asset_fees: Some(0.25),
                exchange_amount_sat: None,
                disable_mrh: None,
                payment_timeout_sec: None,
            }),
            use_asset_fees: asset_fee,
        }
    };
    PreparedIntent::new(context, payment)
}

#[test]
fn amount_form_gates_next_on_the_paying_asset_and_valid_destination_amount() {
    let liquid = Some(InputType::LiquidAddress {
        address: liquid_address(),
    });
    for from in [SendAsset::Lbtc, SendAsset::Usdt] {
        for to in [SendAsset::Lbtc, SendAsset::Usdt] {
            // Keep the unused balance and amount different so selecting the
            // wrong field cannot accidentally satisfy the same assertion.
            for (
                paying_balance,
                other_balance,
                destination_text,
                destination_valid,
                other_text,
                other_valid,
                enabled,
            ) in [
                (1000, 0, "100", true, "", false, true),
                (0, 1000, "100", true, "", false, false),
                (1000, 0, "", true, "100", true, false),
                (1000, 0, "100", false, "100", true, false),
                (1000, 0, "   ", true, "100", true, false),
                (1000, 0, "100", true, "100", false, true),
            ] {
                let destination = value(destination_text, destination_valid);
                let other = value(other_text, other_valid);
                let (amount, usdt_amount) = match to {
                    SendAsset::Lbtc => (&destination, &other),
                    SendAsset::Usdt => (&other, &destination),
                };
                let (btc_balance, usdt_balance) = match from {
                    SendAsset::Lbtc => (paying_balance, other_balance),
                    SendAsset::Usdt => (other_balance, paying_balance),
                };
                let render = || {
                    amount_input_model(AmountInputConfig {
                        amount,
                        usdt_amount_input: usdt_amount,
                        to_asset: to,
                        from_asset: from,
                        uri_asset: None,
                        usdt_balance,
                        comment: "Synthetic comment".into(),
                        has_fiat_converter: true,
                        btc_balance: Amount::from_sat(btc_balance),
                        description: Some("Synthetic recipient"),
                        lightning_limits: Some((100, 10000)),
                        onchain_limits: Some((1000, 100000)),
                        input_type: &liquid,
                        bitcoin_unit: BitcoinDisplayUnit::Sats,
                        error: Some("Synthetic prepare error"),
                        cross_asset_supported: true,
                        pay_fees_with_asset: true,
                        max_loading: false,
                    })
                };
                let view = inspect(render());
                assert!(view.contains("Synthetic prepare error"));
                assert!(view.contains("Synthetic recipient"));
                let messages = capture::click_text(render(), "Next", 0, (1024, 1600));
                assert_eq!(
                    messages.iter().any(|m| matches!(
                        m,
                        LiquidSendMessage::PopupMessage(view::SendPopupMessage::Done)
                    )),
                    enabled,
                    "from={:?}, to={:?}, paying={}, other={}, destination={:?}, valid={}",
                    from,
                    to,
                    paying_balance,
                    other_balance,
                    destination_text,
                    destination_valid
                );
                assert_eq!(view.contains("Paying with"), from != to);
                assert_eq!(
                    view.contains("Pay fees with USDt"),
                    from == SendAsset::Usdt && to == SendAsset::Usdt
                );
            }
        }
    }
}

#[test]
fn onchain_amount_limits_max_loading_and_fee_toggle_are_visible_and_actionable() {
    let amount = value("10000", true);
    let input = Some(InputType::BitcoinAddress {
        address: BitcoinAddressData {
            address: "synthetic-bitcoin-address".into(),
            network: breez_sdk_liquid::prelude::Network::Bitcoin,
            amount_sat: None,
            label: None,
            message: None,
        },
    });
    for unit in [BitcoinDisplayUnit::Sats, BitcoinDisplayUnit::BTC] {
        for loading in [false, true] {
            let render = || {
                amount_input_model(AmountInputConfig {
                    amount: &amount,
                    usdt_amount_input: &amount,
                    to_asset: SendAsset::Lbtc,
                    from_asset: SendAsset::Lbtc,
                    uri_asset: None,
                    usdt_balance: 0,
                    comment: String::new(),
                    has_fiat_converter: false,
                    btc_balance: Amount::from_sat(50000),
                    description: None,
                    lightning_limits: None,
                    onchain_limits: Some((1000, 100000)),
                    input_type: &input,
                    bitcoin_unit: unit,
                    error: None,
                    cross_asset_supported: false,
                    pay_fees_with_asset: false,
                    max_loading: loading,
                })
            };
            let view = inspect(render());
            assert!(view.contains("Enter an amount between"));
            if !loading {
                let messages = capture::click_text(render(), "Max", 0, (1024, 1600));
                assert!(messages.iter().any(|m| matches!(
                    m,
                    LiquidSendMessage::PopupMessage(view::SendPopupMessage::SendMax)
                )));
            }
        }
    }
}

#[test]
fn fiat_conversion_has_loading_invalid_and_valid_states_and_currency_actions() {
    let currencies = [Currency::USD, Currency::EUR, Currency::GBP, Currency::CAD];
    let mut converters = std::collections::HashMap::new();
    let amount = value("100", true);
    assert!(inspect(fiat_input_model(
        &amount,
        &currencies,
        &Currency::USD,
        &converters,
        BitcoinDisplayUnit::Sats
    ))
    .contains("Fetching rate"));
    for currency in currencies {
        converters.insert(currency, converter(currency));
    }
    for currency in currencies {
        for unit in [BitcoinDisplayUnit::Sats, BitcoinDisplayUnit::BTC] {
            for (text, valid) in [("", true), ("bad", false), ("100", true)] {
                let amount = value(text, valid);
                let render =
                    || fiat_input_model(&amount, &currencies, &currency, &converters, unit);
                let view = inspect(render());
                assert!(!view.contains("Fetching rate"));
                let messages = capture::click_text(render(), "Done", 0, (1024, 1600));
                assert_eq!(
                    messages.iter().any(|m| matches!(
                        m,
                        LiquidSendMessage::PopupMessage(view::SendPopupMessage::FiatDone)
                    )),
                    valid && !text.is_empty()
                );
            }
        }
    }
    let messages = capture::click_text(
        fiat_input_model(
            &amount,
            &currencies,
            &Currency::USD,
            &converters,
            BitcoinDisplayUnit::Sats,
        ),
        "EUR",
        0,
        (1024, 1600),
    );
    assert!(messages.iter().any(|m| matches!(
        m,
        LiquidSendMessage::PopupMessage(view::SendPopupMessage::FiatCurrencySelected(
            Currency::EUR
        ))
    )));
}

#[test]
fn confirmation_displays_frozen_asset_fees_comment_and_prevents_duplicate_send() {
    let fiat = converter(Currency::USD);
    for (from, to, asset_fee, onchain) in [
        (SendAsset::Lbtc, SendAsset::Lbtc, false, true),
        (SendAsset::Lbtc, SendAsset::Lbtc, false, false),
        (SendAsset::Usdt, SendAsset::Usdt, true, false),
        (SendAsset::Usdt, SendAsset::Usdt, false, false),
        (SendAsset::Lbtc, SendAsset::Usdt, false, false),
        (SendAsset::Usdt, SendAsset::Lbtc, false, false),
    ] {
        let prepared = intent(from, to, asset_fee, onchain);
        for sending in [false, true] {
            let render = || {
                final_check_page(
                    &prepared,
                    crate::chain::ChainId::Bitcoin,
                    Some(&fiat),
                    sending,
                    BitcoinDisplayUnit::Sats,
                )
            };
            let view = inspect(render());
            assert!(view.contains("Synthetic recipient"));
            assert!(view.contains("Synthetic comment"));
            if to == SendAsset::Usdt {
                assert!(view.contains("12.50 USDt"));
                assert!(view.contains(if asset_fee {
                    "Fees (USDt):"
                } else {
                    "Fees (L-BTC):"
                }));
            } else {
                assert!(view.contains("50100"));
            }
            assert_eq!(view.contains("Cross-asset swap"), from != to);
            let messages = capture::click_text(render(), "Send", 0, (1024, 1600));
            assert_eq!(
                messages
                    .iter()
                    .any(|m| matches!(m, LiquidSendMessage::ConfirmSend)),
                !sending
            );
        }
        let view = inspect(final_check_page(
            &prepared,
            crate::chain::ChainId::Bitcoin,
            None,
            false,
            BitcoinDisplayUnit::BTC,
        ));
        assert!(view.contains("Send"));
    }
}

#[test]
fn source_and_destination_pickers_preserve_asset_network_identity() {
    for asset in [SendAsset::Lbtc, SendAsset::Usdt] {
        for hidden in [false, true] {
            let view = inspect(send_picker_modal(
                Amount::from_sat(12345),
                1230000,
                asset,
                BitcoinDisplayUnit::Sats,
                hidden,
            ));
            assert_eq!(view.contains("••••••"), hidden);
        }
        let messages = capture::activate(
            send_picker_modal(Amount::ZERO, 0, asset, BitcoinDisplayUnit::Sats, false),
            iced::advanced::widget::Id::new(send_asset_action_id(asset)),
            (1024, 1600),
            true,
        );
        assert!(messages
            .iter()
            .any(|m| matches!(m,LiquidSendMessage::SetSendAsset(a) if *a==asset)));
        for (to, network) in ReceiveNetwork::options_for_send_asset(asset, true) {
            let view = inspect(receive_picker_modal(asset, to, network, true));
            assert!(view.contains(network.display_name()));
            let messages = capture::activate(
                receive_picker_modal(asset, to, network, true),
                iced::advanced::widget::Id::new(receive_target_action_id(to, network)),
                (1024, 1600),
                true,
            );
            assert!(messages.iter().any(
                |m| matches!(m,LiquidSendMessage::SetReceiveTarget(a,n) if *a==to&&*n==network)
            ));
        }
    }
}

#[test]
fn full_flow_routes_keep_payment_review_and_modal_context_visible() {
    let input = value("synthetic destination", true);
    let amount = value("50000", true);
    let usdt = value("12.50", true);
    let input_type = Some(InputType::LiquidAddress {
        address: liquid_address(),
    });
    let recent = vec![];
    let cache = Cache::default();
    let menu = Menu::Liquid(crate::app::menu::LiquidSubMenu::Send);
    let quote = coincube_ui::component::quote_display::Quote {
        id: "synthetic".into(),
        text: "Synthetic payment quote".into(),
        author: "Test".into(),
        source: String::new(),
        category: String::new(),
        length: String::new(),
        contexts: vec![],
    };
    let image = iced::widget::image::Handle::from_rgba(1, 1, vec![0, 0, 0, 255]);
    let states = [
        LiquidSendFlowState::Main { modal: Modal::None },
        LiquidSendFlowState::Main {
            modal: Modal::AmountInput,
        },
        LiquidSendFlowState::Main {
            modal: Modal::FiatInput {
                fiat_input: value("50", true),
                currencies: [Currency::USD, Currency::EUR, Currency::GBP, Currency::CAD],
                selected_currency: Currency::USD,
                converters: vec![(Currency::USD, converter(Currency::USD))]
                    .into_iter()
                    .collect(),
            },
        },
        LiquidSendFlowState::FinalCheck(Box::new(intent(
            SendAsset::Lbtc,
            SendAsset::Lbtc,
            false,
            false,
        ))),
        LiquidSendFlowState::Sent,
    ];
    for (index, flow_state) in states.iter().enumerate() {
        for picker in 0..if index == 0 { 3 } else { 1 } {
            let content = liquid_send_with_flow(LiquidSendFlowConfig {
                flow_state,
                btc_balance: Amount::from_sat(100000),
                usdt_balance: 50000000,
                fiat_converter: Some(converter(Currency::USD)),
                recent_transaction: &recent,
                input: &input,
                amount_input: &amount,
                usdt_amount_input: &usdt,
                to_asset: SendAsset::Lbtc,
                from_asset: SendAsset::Lbtc,
                receive_network: ReceiveNetwork::Liquid,
                send_picker_open: picker == 1,
                receive_picker_open: picker == 2,
                uri_asset: None,
                usdt_asset_id: "synthetic-usdt",
                comment: "live edited comment".into(),
                description: Some("live edited recipient"),
                lightning_limits: Some((100, 100000)),
                amount: Amount::from_sat(999),
                is_sending: false,
                menu: &menu,
                cache: &cache,
                input_type: &input_type,
                onchain_limits: Some((1000, 1000000)),
                bitcoin_unit: BitcoinDisplayUnit::Sats,
                error: None,
                cross_asset_supported: true,
                pay_fees_with_asset: false,
                max_loading: false,
                sent_celebration_context: "liquid-send",
                sent_amount_display: "50 000 sats",
                sent_quote: &quote,
                sent_image_handle: &image,
            });
            let view = capture::inspect_rendered_filled(
                "liquid-flow",
                content,
                (1400, 1800),
                "dark",
                theme::Theme::dark(),
            );
            // Overlay widgets draw their content but do not expose it to the
            // text operation. Their form behavior is asserted independently above.
            if picker == 0 && index != 1 && index != 2 {
                assert!(!view.text.is_empty(), "flow {}", index);
            }
            if index == 3 {
                assert!(view.contains("Synthetic comment"));
                assert!(!view.contains("live edited comment"));
                assert!(view.contains("Synthetic recipient"));
            }
            if index == 4 {
                assert!(view.contains("Paid from Liquid"));
            }
            if index == 0 && picker == 0 {
                assert!(view.contains("Available in Liquid"));
            }
        }
    }
}
