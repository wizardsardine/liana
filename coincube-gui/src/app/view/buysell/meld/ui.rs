use std::iter::FromIterator;

use coincube_core::miniscript::bitcoin;
use coincube_ui::{color, component::*, icon, theme};
use iced::{widget, Alignment, Length};

use crate::{app::view, services::meld::api::*};

pub(crate) fn not_supported_ux<'a>(
    c: &'a crate::services::coincube::Country,
) -> coincube_ui::widget::Element<'a, view::Message> {
    widget::column![
        widget::Space::new().height(25),
        widget::row![
            widget::container(icon::warning_icon().size(35).color(iced::Color::BLACK))
                .style(|_| { widget::container::background(color::RED) })
                .align_x(iced::Alignment::Center)
                .align_y(iced::Alignment::Center)
                .padding(35),
            widget::container(
                text::p2_bold(format!(
                    "Your country: {} currently isn't supported for BuySell",
                    c.name
                ))
                .size(15)
                .color(iced::Color::WHITE)
            )
            .style(|_| {
                let mut init = widget::container::background(color::BLACK);
                init.border = iced::Border::default().color(color::RED).width(2);
                init
            })
            .padding(25)
        ]
        .align_y(iced::Alignment::Center),
    ]
    .into()
}

pub(crate) fn region_checks_ux() -> coincube_ui::widget::Element<'static, view::Message> {
    widget::column![
        widget::Space::new().height(25),
        widget::container(
            widget::row![
                widget::container(icon::globe_icon().size(35).color(iced::Color::BLACK))
                    .style(|_| { widget::container::background(color::ORANGE) })
                    .align_x(iced::Alignment::Center)
                    .align_y(iced::Alignment::Center)
                    .padding(35),
                widget::container(
                    text::p2_bold("Checking for regional support and transactional limits...")
                        .size(15)
                        .color(iced::Color::WHITE)
                )
                .padding(25)
            ]
            .align_y(iced::Alignment::Center),
        )
        .style(|_| {
            let mut init = widget::container::background(color::BLACK);
            init.border = iced::Border::default().color(color::ORANGE).width(2);
            init
        })
    ]
    .into()
}

pub(crate) fn input_form_ux<'a>(
    current_amount_str: &'a str,
    limits: &'a CurrencyLimit,
    balance: &'a bitcoin::Amount,
    buy_or_sell: &'a view::buysell::BuyOrSell,
    sending_request: bool,
) -> coincube_ui::widget::Element<'a, view::Message> {
    let amount_is_crypto = matches!(buy_or_sell, view::buysell::BuyOrSell::Sell);
    let mut amount = 0f64;

    // form validation
    let mut validation_messages: Vec<std::borrow::Cow<'static, str>> = vec![];

    match current_amount_str.parse::<f64>() {
        Err(_) => validation_messages.push("Improper number input".into()),
        Ok(ca) if ca.is_finite() => {
            amount = ca;

            if ca > limits.maximum_amount {
                validation_messages.push(
                    format!(
                        "Maximum Transaction of {} is {}",
                        limits.currency_code, limits.maximum_amount
                    )
                    .into(),
                )
            }

            if ca < limits.minimum_amount {
                validation_messages.push(
                    format!(
                        "Minimum Transaction of {} is {}",
                        limits.currency_code, limits.minimum_amount
                    )
                    .into(),
                )
            }

            if ca > balance.to_btc() && !cfg!(debug_assertions) && amount_is_crypto {
                validation_messages.push(
                    format!(
                        "Input Amount of {} is greater than your BTC balance of {}",
                        current_amount_str,
                        balance.to_btc()
                    )
                    .into(),
                )
            }
        }
        Ok(_) => validation_messages.push("Now how did you manage to input that?".into()),
    };

    let amount_input = widget::text_input(&limits.currency_code, current_amount_str)
        .on_input(|am| {
            view::Message::BuySell(view::BuySellMessage::Meld(
                view::buysell::meld::MeldMessage::SetAmount(am),
            ))
        })
        .size(35)
        .padding(10)
        .on_submit_maybe(
            validation_messages
                .is_empty()
                .then_some(view::Message::BuySell(view::BuySellMessage::Meld(
                    view::buysell::meld::MeldMessage::SubmitInputForm(amount),
                ))),
        )
        .align_x(iced::Alignment::Start)
        .width(iced::Length::Fill)
        .style(|th: &theme::Theme, _| widget::text_input::Style {
            background: iced::Color::BLACK.into(),
            border: iced::Border::default()
                .width(2)
                .rounded(0)
                .color(color::GREY_4),
            icon: th.colors.text_inputs.primary.active.icon,
            placeholder: th.colors.text_inputs.primary.active.placeholder,
            value: th.colors.text_inputs.primary.active.value,
            selection: th.colors.text_inputs.primary.active.selection,
        })
        .font(iced::font::Font::MONOSPACE);

    let validation_messages_ui = validation_messages
        .iter()
        .map(|msg| widget::container(text::p2_medium(msg)))
        .fold(
            widget::column![]
                .align_x(iced::Alignment::Center)
                .spacing(15),
            |col, msg| col.push(msg),
        );

    widget::column![
        widget::Space::new().height(25),
        text::p2_medium(format!("Input {} amount", limits.currency_code))
            .color(color::WHITE)
            .align_x(iced::Alignment::Start)
            .align_y(iced::Alignment::Center)
            .width(iced::Length::Fill),
        widget::row![
            amount_input,
            amount_is_crypto.then(|| button::primary_compact(None, "MAX")
                .on_press(view::Message::BuySell(view::BuySellMessage::Meld(
                    view::buysell::meld::MeldMessage::SetMaxAmount,
                )))
                .padding(17)
                .style(|th, st| {
                    let mut base = theme::button::primary(th, st);
                    base.border = iced::Border::default()
                        .rounded(0)
                        .width(0)
                        .color(color::ORANGE);
                    base
                }))
        ]
        .height(iced::Length::Shrink),
        widget::Space::new().height(10),
        (!validation_messages.is_empty()).then_some(validation_messages_ui),
        // submit
        validation_messages.is_empty().then_some(
            button::primary(
                Some(icon::enter_box_icon()),
                match sending_request {
                    true => "Please Wait...",
                    false => "Get Quotes",
                }
            )
            .on_press_maybe((!sending_request).then_some(view::Message::BuySell(
                view::BuySellMessage::Meld(view::buysell::meld::MeldMessage::SubmitInputForm(
                    amount
                ))
            )))
            .style(|th, st| {
                let mut base = theme::button::primary(th, st);
                base.border = iced::Border::default().rounded(0);
                base
            })
        ),
        // reset form
        widget::Space::new().height(15),
        widget::container(
            widget::Space::new()
                .height(Length::Fixed(3.0))
                .width(Length::Fill)
        )
        .style(|_| { color::GREY_6.into() }),
        widget::Space::new().height(15),
        button::secondary(Some(icon::arrow_back()), "Reset Widget")
            .on_press(view::Message::BuySell(view::BuySellMessage::ResetWidget))
            .style(|th, st| {
                let mut base = theme::button::secondary(th, st);
                base.border = iced::Border::default().rounded(0);
                base
            })
    ]
    .align_x(iced::Alignment::Center)
    .width(400)
    .into()
}

pub(crate) fn quote_selection_ux<'a>(
    quotes: &'a [Quote],
    selected: Option<usize>,
    webview_pending: bool,
    buy_or_sell: &'a view::buysell::BuyOrSell,
) -> coincube_ui::widget::Element<'a, view::Message> {
    // simple card UI displaying quote details
    let quote_display = |quote: &Quote, selected: bool, idx: usize| {
        let _card = widget::container(
            widget::row![
                widget::Space::new().width(30),
                // transactions display
                widget::row![
                    widget::column![
                        text::caption("YOU SEND").color(color::ORANGE),
                        text::h3_bold(format!(
                            "{} {}",
                            quote.source_amount, quote.source_currency_code
                        )),
                    ]
                    .align_x(iced::Alignment::Start),
                    widget::column![
                        text::caption("YOU RECEIVE").color(color::GREEN),
                        text::h3_bold(format!(
                            "{} {}",
                            quote.destination_amount, quote.destination_currency_code
                        )),
                    ]
                    .align_x(iced::Alignment::Start),
                ]
                .spacing(25),
                widget::Space::new().width(iced::Length::Fill),
                // separator
                widget::container(widget::Space::default().width(1).height(100))
                    .style(theme::card::border),
                widget::Space::new().width(25),
                // payment details
                widget::column![
                    text::caption("Provider").color(color::GREY_3),
                    text::p2_medium(quote.service_provider.as_str()),
                    widget::Space::new().height(5),
                    text::caption("Total Fee").color(color::GREY_3),
                    text::p2_medium(format!(
                        "{} {}",
                        quote.total_fee,
                        match buy_or_sell {
                            view::buysell::BuyOrSell::Sell => &quote.destination_currency_code,
                            view::buysell::BuyOrSell::Buy { .. } => &quote.source_currency_code,
                        }
                    )),
                    widget::Space::new().height(5),
                    // exchange rate display
                    quote
                        .exchange_rate
                        .is_some()
                        .then_some(text::caption("Exchange Rate").color(color::GREY_3)),
                    quote.exchange_rate.map(|rate| text::p2_medium(format!(
                        "1 {} = {} {}",
                        quote.source_currency_code,
                        match buy_or_sell {
                            view::buysell::BuyOrSell::Sell => rate,
                            view::buysell::BuyOrSell::Buy { .. } => 1.0 / rate,
                        },
                        quote.destination_currency_code
                    ))),
                ]
                .align_x(iced::Alignment::Start),
                widget::Space::new().width(20),
            ]
            .align_y(iced::Alignment::Center),
        )
        .align_x(iced::Alignment::Center)
        .align_y(iced::Alignment::Center)
        .width(iced::Length::Fill)
        .style(move |_| {
            iced::widget::container::Style::default()
                .background(iced::Color::BLACK)
                .color(iced::Color::WHITE)
                .border(match selected {
                    false => iced::Border::default()
                        .color(color::GREY_5)
                        .width(1)
                        .rounded(5),
                    true => iced::Border::default()
                        .color(color::ORANGE)
                        .width(3)
                        .rounded(5),
                })
        })
        .height(150);

        widget::button(_card)
            .on_press(view::buysell::meld::MeldMessage::SelectQuote(idx))
            .style(theme::button::transparent)
    };

    let column = widget::column![
        // quote selector
        widget::scrollable(widget::Column::from_iter(quotes.iter().enumerate().map(
            |(idx, quote)| quote_display(quote, Some(idx) == selected, idx).into()
        ),))
        .height(320)
        .anchor_top(),
        // separators
        widget::Space::new().height(10),
        iced::widget::container(widget::Space::new().height(2))
            .style(|_| iced::widget::container::background(iced::Background::Color(color::GREY_3)))
            .width(Length::Fill),
        widget::Space::new().height(10),
        // driver buttons
        selected.map(|s| {
            match webview_pending {
                true => button::primary(Some(icon::reload_icon()), "Loading Webview...").style(
                    |th, st| {
                        let mut base = theme::button::primary(th, st);
                        base.border = iced::Border::default().rounded(3);
                        base
                    },
                ),
                false => button::primary(Some(icon::globe_icon()), "Start Session")
                    .on_press(view::buysell::meld::MeldMessage::StartSessionPressed(s))
                    .style(|th, st| {
                        let mut base = theme::button::primary(th, st);
                        base.border = iced::Border::default().rounded(3);
                        base
                    }),
            }
        }),
        selected.is_none().then(|| {
            widget::row![
                text::h4_bold("Select a preferred provider"),
                widget::Space::new().width(iced::Length::Fill),
                button::primary_compact(Some(icon::arrow_back()), "Go Back To Input Form")
                    .on_press(view::buysell::meld::MeldMessage::NavigateBack)
                    .style(|th, st| {
                        let mut base = theme::button::primary(th, st);
                        base.border = iced::Border::default().rounded(0);
                        base
                    })
            ]
            .align_y(iced::Alignment::Center)
        })
    ]
    .width(700)
    .spacing(5);

    let elem: iced::Element<view::buysell::meld::MeldMessage, theme::Theme> = column.into();
    elem.map(|m| view::Message::BuySell(view::BuySellMessage::Meld(m)))
}

pub(super) fn webview_ux<'a>(
    active: &'a iced_wry::IcedWebview,
    network: &'a bitcoin::Network,
) -> coincube_ui::widget::Element<'a, view::Message> {
    let col = iced::widget::column![
        active.view(Length::Fixed(640.0), Length::Fixed(640.0)),
        // Network display banner
        widget::Space::new().height(Length::Fixed(15.0)),
        {
            let (network_name, network_color) = match network {
                bitcoin::Network::Bitcoin => ("Bitcoin Mainnet", color::GREEN),
                bitcoin::Network::Testnet => ("Bitcoin Testnet", color::ORANGE),
                bitcoin::Network::Testnet4 => ("Bitcoin Testnet4", color::ORANGE),
                bitcoin::Network::Signet => ("Bitcoin Signet", color::BLUE),
                bitcoin::Network::Regtest => ("Bitcoin Regtest", color::RED),
            };

            iced::widget::row![
                // currently selected bitcoin network display
                text::text("Network: ").size(12).color(color::GREY_3),
                text::text(network_name).size(12).color(network_color),
                // render a button that closes the webview
                widget::Space::new().width(Length::Fixed(20.0)),
                {
                    button::secondary(Some(icon::arrow_back()), "Start Over")
                        .on_press(view::BuySellMessage::ResetWidget)
                        .width(iced::Length::Fixed(300.0))
                }
            ]
            .spacing(5)
            .align_y(Alignment::Center)
        }
    ];

    let elem: iced::Element<view::BuySellMessage, theme::Theme> = col.into();
    elem.map(view::Message::BuySell)
}
