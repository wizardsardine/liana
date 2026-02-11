use std::iter::FromIterator;

use coincube_ui::{color, component::*, icon, theme};
use iced::{widget, Alignment, Length};

use crate::{app::view, services::meld::api::*};

pub(crate) fn not_supported_ux<'a>(
    msg: &'a str,
) -> coincube_ui::widget::Element<'a, view::Message> {
    widget::column![
        widget::Space::new().height(25),
        widget::row![
            widget::container(icon::warning_icon().size(35).color(iced::Color::BLACK))
                .style(|_| { widget::container::background(color::RED) })
                .align_x(iced::Alignment::Center)
                .align_y(iced::Alignment::Center)
                .padding(35),
            widget::container(text::p2_bold(msg).size(15).color(iced::Color::WHITE))
                .style(|_| {
                    let mut init = widget::container::background(color::BLACK);
                    init.border = iced::Border::default().color(color::RED).width(2);
                    init
                })
                .align_y(iced::Alignment::Center)
                .height(iced::Length::Fill)
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
    state: &'a super::MeldState,
) -> coincube_ui::widget::Element<'a, view::Message> {
    let Some(super::MeldFlowStep::AmountInputForm {
        amount,
        limits,
        btc_balance,
        processing_request,
    }) = state.steps.last()
    else {
        unreachable!()
    };

    let amount_is_crypto = matches!(state.buy_or_sell, view::buysell::BuyOrSell::Sell);
    let mut amount_parsed = 0f64;

    // form validation
    let mut validation_messages: Vec<std::borrow::Cow<'static, str>> = vec![];

    match amount.parse::<f64>() {
        Err(_) => validation_messages.push("Improper number input".into()),
        Ok(ca) if ca.is_finite() => {
            amount_parsed = ca;

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

            if ca > btc_balance.to_btc() && !cfg!(debug_assertions) && amount_is_crypto {
                validation_messages.push(
                    format!(
                        "Input Amount of {} is greater than your BTC balance of {}",
                        amount,
                        btc_balance.to_btc()
                    )
                    .into(),
                )
            }
        }
        Ok(_) => validation_messages.push("Now how did you manage to input that?".into()),
    };

    let amount_input = widget::text_input(&limits.currency_code, amount)
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
                    view::buysell::meld::MeldMessage::SubmitInputAmount(amount_parsed),
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

    let validation_messages_ui = || {
        validation_messages
            .iter()
            .map(|msg| widget::container(text::p2_medium(msg)))
            .fold(
                widget::column![]
                    .align_x(iced::Alignment::Center)
                    .spacing(15),
                |col, msg| col.push(msg),
            )
    };

    widget::column![
        widget::Space::new().height(25),
        text::p2_medium(format!("Input {} amount", limits.currency_code))
            .color(color::WHITE)
            .align_x(iced::Alignment::Start)
            .align_y(iced::Alignment::Center)
            .width(iced::Length::Fill),
        widget::row![
            amount_input,
            amount_is_crypto.then(|| widget::button("MAX")
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
        (!validation_messages.is_empty()).then(validation_messages_ui),
        // submit
        validation_messages.is_empty().then_some(
            button::primary(
                Some(icon::enter_box_icon()),
                match amount_is_crypto {
                    true => match processing_request {
                        true => "Getting Quotes...",
                        false => "Get Quotes",
                    },
                    false => "Select/Generate an Address for Deposit",
                }
            )
            .on_press_maybe((!*processing_request).then_some(view::Message::BuySell(
                view::BuySellMessage::Meld(view::buysell::meld::MeldMessage::SubmitInputAmount(
                    amount_parsed
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

pub(crate) fn address_selection_ux<'a>(
    state: &'a super::MeldState,
) -> coincube_ui::widget::Element<'a, view::Message> {
    let Some(super::MeldFlowStep::AddressSelection {
        deposit_address,
        existing_addresses,
        addresses_continue_from,
        address_picker_open,
        was_picked_from_existing_addresses,
        processing_request,
        ..
    }) = state.steps.last()
    else {
        unreachable!()
    };

    // Initial loading state (no addresses loaded yet)
    let address_picker = || {
        match existing_addresses.as_deref() {
            // simple loading spinner
            None => card::simple(
                widget::row![
                    text::p1_regular("Loading addresses"),
                    widget::container(spinner::typing_text_carousel(
                        "...",
                        true,
                        std::time::Duration::from_millis(500),
                        text::p1_regular,
                    ))
                ]
                .spacing(10)
                .align_y(Alignment::Center),
            ),
            Some([]) => card::simple(
                widget::row![
                    widget::container(widget::Space::new())
                        .style(|_| {
                            widget::container::background(iced::Background::Color(color::GREY_3))
                        })
                        .height(Length::Fixed(1.0))
                        .width(Length::Fill),
                    text::p1_regular("Generating a new address for deposit..").color(color::GREY_2),
                    widget::container(widget::Space::new())
                        .style(|_| {
                            widget::container::background(iced::Background::Color(color::GREY_3))
                        })
                        .height(Length::Fixed(1.0))
                        .width(Length::Fill),
                ]
                .spacing(10)
                .align_y(Alignment::Center),
            ),
            Some(addresses) => {
                // dropdown header button
                let dropdown_header = widget::Button::new(
                    widget::row![
                        text::p2_regular("Pick an existing address")
                            .width(Length::Fill)
                            .color(color::GREY_2),
                        if *address_picker_open {
                            icon::up_icon()
                        } else {
                            icon::down_icon()
                        }
                        .size(14)
                        .style(theme::text::secondary),
                    ]
                    .align_y(Alignment::Center),
                )
                .on_press(view::Message::BuySell(view::BuySellMessage::Meld(
                    view::buysell::meld::MeldMessage::ToggleAddressPicker,
                )))
                .padding([12, 16])
                .width(Length::Fill)
                .style(theme::button::secondary);

                // existing addresses list (only shown when expanded)
                let dropdown_content = || {
                    let addresses_list = widget::Column::from_iter(
                        addresses.iter().enumerate().map(|(idx, addr)| {
                            let label_text = addr.label.clone();
                            let addr_str = addr.address.to_string();
                            widget::Button::new(
                                widget::column![
                                    label_text
                                        .map(|l| { text::p2_regular(l).color(color::GREY_2) }),
                                    text::p2_regular(addr_str).width(Length::Fill)
                                ]
                                .spacing(2),
                            )
                            .on_press(view::Message::BuySell(view::BuySellMessage::Meld(
                                view::buysell::meld::MeldMessage::SelectExistingAddress(idx),
                            )))
                            .padding([8, 12])
                            .width(Length::Fill)
                            .style(theme::button::menu)
                            .into()
                        }),
                    )
                    // Add "Load more" at the end
                    .push(addresses_continue_from.is_some().then(|| {
                        widget::Button::new(
                            widget::container(
                                widget::row![
                                    text::p2_regular("Load more").color(color::ORANGE),
                                    icon::down_icon().size(12).style(|_| widget::text::Style {
                                        color: Some(color::ORANGE),
                                    })
                                ]
                                .spacing(8)
                                .align_y(Alignment::Center),
                            )
                            .width(Length::Fill)
                            .align_x(Alignment::Center),
                        )
                        .on_press(view::Message::BuySell(view::BuySellMessage::Meld(
                            view::buysell::meld::MeldMessage::LoadMoreAddresses,
                        )))
                        .padding([8, 12])
                        .width(Length::Fill)
                        .style(theme::button::transparent_border)
                    }))
                    .spacing(2);

                    widget::container(
                        widget::scrollable(addresses_list).direction(
                            widget::scrollable::Direction::Vertical(
                                widget::scrollable::Scrollbar::new()
                                    .width(4)
                                    .scroller_width(4),
                            ),
                        ),
                    )
                    .padding(5)
                    .max_height(180.0)
                    .style(theme::card::simple)
                    .width(Length::Fill)
                };

                card::simple(
                    widget::column![
                        text::h3("Select a Deposit Address")
                            .width(iced::Length::Fill)
                            .align_x(iced::Alignment::Center)
                            .align_y(iced::Alignment::Center),
                        widget::Space::new().height(5),
                        dropdown_header,
                        address_picker_open.then(dropdown_content),
                        // fancy textual separator
                        widget::row![
                            widget::container(widget::Space::new())
                                .style(|_| {
                                    widget::container::background(iced::Background::Color(
                                        color::GREY_3,
                                    ))
                                })
                                .height(Length::Fixed(1.0))
                                .width(Length::Fill),
                            text::p2_regular("or").color(color::GREY_2),
                            widget::container(widget::Space::new())
                                .style(|_| {
                                    widget::container::background(iced::Background::Color(
                                        color::GREY_3,
                                    ))
                                })
                                .height(Length::Fixed(1.0))
                                .width(Length::Fill),
                        ]
                        .spacing(10)
                        .align_y(Alignment::Center),
                        // generates a new address
                        button::secondary(Some(icon::plus_icon()), "Generate New Address")
                            .on_press(view::Message::BuySell(view::BuySellMessage::Meld(
                                view::buysell::meld::MeldMessage::CreateNewAddress,
                            )))
                            .width(iced::Length::Fill),
                    ]
                    .spacing(10),
                )
            }
        }
        .width(Length::Fill)
    };

    match deposit_address {
        None => address_picker(),
        // allows user to verify an address and proceed with the UX flow
        Some((address, qr_code_data)) => card::simple(
            widget::column![
                widget::column![
                    text::caption(match was_picked_from_existing_addresses {
                        true => "YOUR SELECTED ADDRESS: ",
                        false => "GENERATED ADDRESS: ",
                    })
                    .color(color::BLUE)
                    .width(iced::Length::Fill)
                    .align_y(iced::Alignment::Center),
                    widget::row![
                        widget::container(
                            widget::text(address)
                                .font(iced::font::Font {
                                    weight: iced::font::Weight::Medium,
                                    ..iced::font::Font::MONOSPACE
                                })
                                .size(15)
                        )
                        .style(|_| {
                            widget::container::Style::default()
                                .background(color::WHITE)
                                .color(color::BLACK)
                                .border(iced::Border::default().width(0))
                        })
                        .align_x(iced::Alignment::Center)
                        .width(iced::Length::Fill)
                        .padding(7),
                        widget::button(
                            icon::clipboard_icon()
                                .color(color::WHITE)
                                .size(17)
                                .width(38)
                        )
                        .on_press(view::Message::BuySell(view::BuySellMessage::Meld(
                            view::buysell::meld::MeldMessage::CopyAddressToClipboard
                        )))
                        .style(|th, st| {
                            let mut base = theme::button::secondary(th, st);
                            base.border = iced::Border::default().rounded(0).width(0);
                            base.background = Some(color::GREY_6.into());
                            base
                        })
                        .padding(6)
                    ]
                    .spacing(1)
                ]
                .align_x(iced::Alignment::Start)
                .spacing(2),
                // qr code for scanning
                widget::container(widget::qr_code(qr_code_data).cell_size(8).style(|_| {
                    widget::qr_code::Style {
                        background: color::WHITE,
                        cell: color::BLACK,
                    }
                }))
                .padding(10)
                .width(iced::Length::Fill)
                .align_x(iced::Alignment::Center)
                .align_y(iced::Alignment::Center)
                .style(|_| {
                    widget::container::Style::default()
                        .background(color::WHITE)
                        .color(iced::Color::BLACK)
                }),
                // dash
                widget::row![
                    button::secondary_compact(Some(icon::arrow_back()), "Go Back")
                        .on_press(view::Message::BuySell(view::BuySellMessage::Meld(
                            view::buysell::meld::MeldMessage::NavigateBack
                        )))
                        .style(|th, st| {
                            let mut base = theme::button::secondary(th, st);
                            base.border = iced::Border::default().rounded(0);
                            base
                        }),
                    widget::Space::new().width(iced::Length::Fill),
                    // submit
                    match processing_request {
                        true => {
                            button::primary_compact(Some(icon::clock_icon()), "Fetching Quotes")
                        }
                        false => {
                            button::primary_compact(Some(icon::arrow_return_right()), "Proceed")
                                .on_press(view::Message::BuySell(view::BuySellMessage::Meld(
                                    view::buysell::meld::MeldMessage::GetQuotes,
                                )))
                        }
                    }
                    .style(|th, st| {
                        let mut base = theme::button::primary(th, st);
                        base.border = iced::Border::default().rounded(0);
                        base
                    })
                ]
                .align_y(iced::Alignment::Center)
            ]
            .spacing(10),
        )
        .height(iced::Length::Shrink)
        .width(800)
        .style(|_| {
            widget::container::Style::default()
                .background(color::BLACK)
                .color(iced::Color::WHITE)
                .border(
                    iced::Border::default()
                        .color(color::GREY_4)
                        .width(1)
                        .rounded(5),
                )
        }),
    }
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
        let card = widget::container(
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
            widget::container::Style::default()
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

        widget::button(card)
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
        widget::container(widget::Space::new().height(2))
            .style(|_| widget::container::background(iced::Background::Color(color::GREY_3)))
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
                    .on_press(view::buysell::meld::MeldMessage::ConfirmSelectedQuote(s))
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
                button::secondary_compact(Some(icon::arrow_back()), "Go Back To Input Form")
                    .on_press(view::buysell::meld::MeldMessage::NavigateBack)
                    .style(|th, st| {
                        let mut base = theme::button::secondary(th, st);
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

pub(crate) fn region_selection_ux<'a>(
    regions: &'a [MeldRegion],
    selected: Option<usize>,
    filter: &'a str,
) -> coincube_ui::widget::Element<'a, view::Message> {
    let region_card = |region: &MeldRegion, is_selected: bool, idx: usize| {
        let card = widget::container(
            widget::row![
                widget::Space::new().width(30),
                widget::column![
                    text::h3_bold(&region.name),
                    text::caption(&region.region_code).color(color::GREY_3),
                ]
                .align_x(iced::Alignment::Start),
                widget::Space::new().width(iced::Length::Fill),
                is_selected.then(|| icon::check_icon().size(20).color(color::ORANGE)),
                widget::Space::new().width(20),
            ]
            .align_y(iced::Alignment::Center),
        )
        .align_x(iced::Alignment::Center)
        .align_y(iced::Alignment::Center)
        .width(iced::Length::Fill)
        .style(move |_| {
            widget::container::Style::default()
                .background(iced::Color::BLACK)
                .color(iced::Color::WHITE)
                .border(match is_selected {
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
        .padding([15, 0]);

        widget::button(card)
            .on_press(view::buysell::meld::MeldMessage::SelectRegion(idx))
            .style(theme::button::transparent)
    };

    let filter_lower = filter.to_lowercase();
    let filtered_regions: Vec<(usize, &MeldRegion)> = regions
        .iter()
        .enumerate()
        .filter(|(_, r)| {
            filter.is_empty()
                || r.name.to_lowercase().contains(&filter_lower)
                || r.region_code.to_lowercase().contains(&filter_lower)
        })
        .collect();

    let search_input = widget::text_input("Search regions...", filter)
        .on_input(view::buysell::meld::MeldMessage::SetRegionFilter)
        .size(14)
        .padding([8, 12])
        .width(iced::Length::Fill)
        .style(|th: &theme::Theme, st| {
            let mut base = theme::text_input::primary(th, st);
            base.background = iced::Color::BLACK.into();
            base.border = iced::Border::default()
                .width(1)
                .rounded(5)
                .color(color::GREY_4);
            base
        })
        .font(iced::font::Font::MONOSPACE);

    let region_list = if filtered_regions.is_empty() {
        widget::column![
            widget::Space::new().height(20),
            text::p2_medium("No regions match your search").color(color::GREY_3),
            widget::Space::new().height(20),
        ]
        .align_x(iced::Alignment::Center)
        .width(iced::Length::Fill)
    } else {
        widget::Column::from_iter(
            filtered_regions
                .iter()
                .map(|(idx, region)| region_card(region, Some(*idx) == selected, *idx).into()),
        )
    };

    let column = widget::column![
        widget::row![
            text::h4_bold("Select your region"),
            widget::Space::new().width(iced::Length::Fill),
            button::secondary_compact(Some(icon::arrow_back()), "Go Back")
                .on_press(view::buysell::meld::MeldMessage::NavigateBack)
                .style(|th, st| {
                    let mut base = theme::button::secondary(th, st);
                    base.border = iced::Border::default().rounded(0);
                    base
                })
        ]
        .align_y(iced::Alignment::Center),
        widget::Space::new().height(5),
        search_input,
        widget::Space::new().height(5),
        widget::scrollable(region_list).height(300).anchor_top(),
        widget::Space::new().height(10),
        widget::container(widget::Space::new().height(2))
            .style(|_| widget::container::background(iced::Background::Color(color::GREY_3)))
            .width(Length::Fill),
        widget::Space::new().height(10),
        selected.map(|_| {
            button::primary(Some(icon::globe_icon()), "Continue")
                .on_press(view::buysell::meld::MeldMessage::ConfirmRegion)
                .style(|th, st| {
                    let mut base = theme::button::primary(th, st);
                    base.border = iced::Border::default().rounded(3);
                    base
                })
        }),
    ]
    .width(700)
    .spacing(5);

    let elem: iced::Element<view::buysell::meld::MeldMessage, theme::Theme> = column.into();
    elem.map(|m| view::Message::BuySell(view::BuySellMessage::Meld(m)))
}

pub(super) fn webview_ux<'a>(
    active: &'a iced_wry::IcedWebview,
    wallet_address: Option<&'a str>,
) -> coincube_ui::widget::Element<'a, view::Message> {
    let col = widget::column![
        active.view(Length::Fixed(640.0), Length::Fixed(640.0)),
        wallet_address.map(|addr| {
            widget::column![
                widget::container(
                    widget::row![
                        widget::text_input("", &addr)
                            .size(12)
                            .padding([6, 10])
                            .width(Length::Fill)
                            .style(|_, _| widget::text_input::Style {
                                background: color::BLACK.into(),
                                border: iced::Border::default()
                                    .width(1)
                                    .rounded(0)
                                    .color(color::GREY_5),
                                icon: color::WHITE,
                                placeholder: color::GREY_3,
                                value: color::WHITE,
                                selection: color::ORANGE,
                            })
                            .font(iced::font::Font::MONOSPACE),
                        widget::Button::new(icon::clipboard_icon().style(theme::text::secondary),)
                            .on_press(view::BuySellMessage::Meld(super::MeldMessage::CopyAddressToClipboard))
                            .style(theme::button::transparent_border)
                            .padding([6, 10]),
                    ]
                    .spacing(4)
                    .align_y(Alignment::Center),
                )
                .width(Length::Fixed(640.0)),
                if cfg!(target_os = "macos") {
                    widget::container(
                        text::caption("Keyboard shortcuts are not supported in the webview. Right-click the input field to paste.")
                            .color(color::GREY_3)
                    )
                    .width(Length::Fixed(640.0))
                    .padding([4, 0])
                } else {
                    widget::container(widget::Space::new())
                },
            ]
        }),
    ];

    let elem: iced::Element<view::BuySellMessage, theme::Theme> = col.into();
    elem.map(view::Message::BuySell)
}
