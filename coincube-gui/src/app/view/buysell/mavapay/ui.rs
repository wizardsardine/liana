use crate::{
    app::view::{
        buysell::{panel::BuyOrSell, MavapayFlowStep, MavapayState},
        BuySellMessage, Message as ViewMessage,
    },
    services::mavapay::*,
};

use iced::{widget::*, Alignment, Length};

use coincube_ui::{color, icon::*, theme, widget::Column};
use coincube_ui::{
    component::{button, text},
    widget::ColumnExt as _,
};

pub fn form<'a>(state: &'a MavapayState) -> iced::Element<'a, ViewMessage, theme::Theme> {
    let form = match &state.step {
        MavapayFlowStep::Transaction { .. } => transactions_form,
        MavapayFlowStep::Checkout { .. } => checkout_form,
    };

    let element: iced::Element<'a, BuySellMessage, theme::Theme> = form(state).into();
    element.map(|b| ViewMessage::BuySell(b))
}

fn checkout_form<'a>(state: &'a MavapayState) -> Column<'a, BuySellMessage> {
    let MavapayFlowStep::Checkout {
        buy_or_sell,
        fulfilled_order,
        ..
    } = &state.step
    else {
        unreachable!()
    };

    match fulfilled_order {
        None => {
            iced::widget::column![
                text::h4_bold("Checkout"),
                match buy_or_sell {
                    BuyOrSell::Buy { address: _ } => {
                        // TODO: display bank deposit details, equivalent BTC amount and the generated address for deposit
                        container(text::p1_italic("Display account deposit details here..."))
                    }
                    BuyOrSell::Sell => {
                        // TODO: display bitcoin address or lightning invoice for deposit, and beneficiary input forms
                        container(text::p1_italic(
                            "Display lightning invoice or bitcoin address for deposit here...",
                        ))
                    }
                    .style(theme::card::simple),
                }
            ]
            .push_maybe(
                (cfg!(debug_assertions) && option_env!("MAVAPAY_API_KEY").is_none()).then(|| {
                    button::primary(Some(wrench_icon()), "Simulate Pay-In (Developer Option)")
                        .on_press_maybe(
                            fulfilled_order
                                .is_none()
                                .then_some(BuySellMessage::Mavapay(MavapayMessage::SimulatePayIn)),
                        )
                }),
            )
        }
        Some(order) => {
            // TODO: Improve success UI
            iced::widget::column![
                match buy_or_sell {
                    BuyOrSell::Sell => text::p1_italic("Withdrawal Was Successful"),
                    BuyOrSell::Buy { .. } => text::p1_italic("Purchase Was Successful"),
                },
                text::p2_bold(format!("{:?}", order)),
                button::primary(Some(reload_icon()), "Start Over")
                    .on_press(BuySellMessage::ResetWidget)
            ]
        }
    }
    .align_x(Alignment::Center)
    .width(600)
}

fn transactions_form<'a>(state: &'a MavapayState) -> Column<'a, BuySellMessage> {
    let MavapayFlowStep::Transaction {
        sat_amount,
        btc_price: current_price,
        buy_or_sell,
        country,
        transfer_speed,
        sending_quote,
        ..
    } = &state.step
    else {
        unreachable!()
    };

    let input_form = match current_price {
        Some(price) => Container::new(
            iced::widget::column![
                Space::new().height(17),
                iced::widget::row![
                    iced::widget::column![
                        text(format!(
                            "{} ({})",
                            country.currency.name, country.currency.code
                        ))
                        .size(14)
                        .color(color::GREY_2),
                        Space::new().height(5),
                        iced_aw::number_input(
                            &{
                                *sat_amount as f64
                                    * (price.btc_price_in_unit_currency / 100_000_000.0)
                            }
                            .round(),
                            ..,
                            |a| { BuySellMessage::Mavapay(MavapayMessage::FiatAmountChanged(a)) }
                        )
                        .on_submit(BuySellMessage::Mavapay(MavapayMessage::NormalizeAmounts))
                        .align_x(Alignment::Center)
                        .step(500.0)
                        .width(150)
                        .set_size(18)
                        .padding(10)
                    ]
                    .align_x(Alignment::Center),
                    container(left_right_icon().size(20).center()).padding(12),
                    iced::widget::column![
                        text("Satoshis (BTCSAT)").size(14).color(color::GREY_2),
                        Space::new().height(5),
                        iced_aw::number_input(&(*sat_amount as f64), .., |a| {
                            BuySellMessage::Mavapay(MavapayMessage::SatAmountChanged(a))
                        })
                        .on_submit(BuySellMessage::Mavapay(MavapayMessage::NormalizeAmounts))
                        .align_x(Alignment::Center)
                        .step(1000.0)
                        .width(150)
                        .set_size(18)
                        .padding(10)
                    ]
                    .align_x(Alignment::Center)
                ]
                .align_y(Alignment::End)
                .spacing(20)
                .padding(0),
                iced::widget::row![
                    Space::new().width(Length::Fill),
                    text::p1_medium("Select an onchain transfer speed: ")
                        .width(Length::Shrink)
                        .center(),
                    iced::widget::pick_list(
                        OnchainTransferSpeed::all(),
                        Some(transfer_speed),
                        |s| { BuySellMessage::Mavapay(MavapayMessage::TransferSpeedChanged(s)) }
                    )
                    .padding(10)
                    .width(100),
                    Space::new().width(Length::Fill),
                ]
                .width(Length::Fill)
                .align_y(Alignment::Center),
                Space::new().height(20),
                match buy_or_sell {
                    // TODO: ensure user has BTC balance to satisfy quote
                    BuyOrSell::Sell => {
                        // TODO: onchain sell currently unsupported, lightning integration will be required to proceed
                        button::primary(Some(send_icon()), "Send Bitcoin (Currently Unsupported)")
                    }
                    BuyOrSell::Buy { .. } => match sending_quote {
                        true => button::primary(Some(reload_icon()), "Processing Quote..."),
                        false => {
                            button::primary(Some(card_icon()), "Proceed to Payment")
                                .on_press(BuySellMessage::Mavapay(MavapayMessage::CreateQuote))
                        }
                    },
                }
                .width(Length::Fill)
            ]
            .spacing(10)
            .align_x(Alignment::Center)
            .width(Length::Fill),
        ),
        None => Container::new(
            text::p1_italic("Currently loading BTC price, please wait")
                .width(Length::Fill)
                .center(),
        )
        .align_y(Alignment::Center)
        .align_x(Alignment::Center),
    }
    .padding(15)
    .style(theme::card::simple)
    .width(Length::Fixed(600.0));

    // combine UI, render beneficiary input form using card styling
    iced::widget::column![
        // separator
        Space::new().height(Length::Fixed(5.0)),
        container(Space::new().height(Length::Fixed(4.0)).width(Length::Fill))
            .style(theme::card::simple),
        Space::new().height(Length::Fixed(5.0)),
        // header text
        text::h4_bold(match buy_or_sell {
            BuyOrSell::Sell => "Sell Bitcoin to Fiat Money",
            BuyOrSell::Buy { .. } => "Buy Bitcoin using Fiat Money",
        })
        .color(color::WHITE)
        .center(),
        Space::new().height(Length::Fixed(20.0)),
        input_form,
        Space::new().height(Length::Fixed(5.0)),
        text::p2_medium("Powered by Mavapay").color(color::GREY_3)
    ]
    .align_x(Alignment::Center)
}
