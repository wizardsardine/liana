use breez_sdk_liquid::{
    model::{PaymentDetails, PaymentState},
    InputType,
};
use coincube_core::miniscript::bitcoin::Amount;
use coincube_ui::{
    color,
    component::{button, form, text::*},
    icon, theme,
    widget::*,
};
use iced::{
    widget::{Column, Container, Row, Space},
    Alignment, Length,
};

use crate::app::{
    cache::Cache,
    menu::Menu,
    state::active::send::{ActiveSendFlowState, Modal},
    view::{self, vault::fiat::FiatAmount, ActiveSendMessage, FiatAmountConverter, Message},
};

pub fn active_send_with_flow<'a>(
    flow_state: &'a ActiveSendFlowState,
    btc_balance: Amount,
    fiat_converter: Option<FiatAmountConverter>,
    recent_transaction: &'a Vec<RecentTransaction>,
    input: &'a form::Value<String>,
    error: Option<&'a str>,
    amount_input: &'a form::Value<String>,
    comment: String,
    description: Option<&'a str>,
    lightning_limits: Option<(u64, u64)>,
    amount: Amount,
    prepare_response: Option<&'a breez_sdk_liquid::prelude::PrepareSendResponse>,
    is_sending: bool,
    menu: &'a Menu,
    cache: &'a Cache,
    input_type: &'a Option<InputType>,
    onchain_limits: Option<(u64, u64)>,
) -> Element<'a, Message> {
    let base_content = match flow_state {
        ActiveSendFlowState::Main { modal } => {
            let send_view = active_send_view(
                btc_balance,
                fiat_converter.clone(),
                recent_transaction,
                input,
                input_type,
            )
            .map(Message::ActiveSend);

            let content = view::dashboard(menu, cache, None, send_view);

            // Show modal if needed
            match modal {
                Modal::AmountInput => {
                    let modal_content = amount_input_model(
                        amount_input,
                        comment,
                        fiat_converter.is_some(),
                        btc_balance,
                        description,
                        lightning_limits,
                        onchain_limits,
                        input_type,
                    )
                    .map(Message::ActiveSend);
                    coincube_ui::widget::modal::Modal::new(content, modal_content)
                        .on_blur(Some(Message::ActiveSend(ActiveSendMessage::PopupMessage(
                            view::SendPopupMessage::Close,
                        ))))
                        .into()
                }
                Modal::FiatInput {
                    fiat_input,
                    currencies,
                    selected_currency,
                    converters,
                } => {
                    let modal_content =
                        fiat_input_model(fiat_input, currencies, selected_currency, converters)
                            .map(Message::ActiveSend);
                    coincube_ui::widget::modal::Modal::new(content, modal_content)
                        .on_blur(Some(Message::ActiveSend(ActiveSendMessage::PopupMessage(
                            view::SendPopupMessage::FiatClose,
                        ))))
                        .into()
                }
                Modal::None => content,
            }
        }
        ActiveSendFlowState::FinalCheck => {
            let content = final_check_page(
                amount,
                comment,
                description,
                fiat_converter.as_ref(),
                prepare_response,
                is_sending,
            )
            .map(Message::ActiveSend);
            view::dashboard(menu, cache, None, content)
        }
        ActiveSendFlowState::Sent => {
            let content = sent_page(amount).map(Message::ActiveSend);
            view::dashboard(menu, cache, None, content)
        }
    };

    if let Some(err) = error {
        Column::new()
            .push(
                Container::new(
                    Container::new(text(err).size(14).color(color::RED))
                        .padding(10)
                        .center_x(Length::Fill)
                        .style(theme::card::error)
                        .width(Length::Fill)
                        .max_width(800),
                )
                .width(Length::Fill)
                .padding([20, 40])
                .align_x(Alignment::Center),
            )
            .push(base_content)
            .into()
    } else {
        base_content
    }
}

pub fn active_send_view<'a>(
    btc_balance: Amount,
    fiat_converter: Option<FiatAmountConverter>,
    recent_transaction: &Vec<RecentTransaction>,
    input: &'a form::Value<String>,
    input_type: &'a Option<InputType>,
) -> Element<'a, ActiveSendMessage> {
    let mut content = Column::new()
        .spacing(10)
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .padding(40);

    let mut balance_section = Column::new().spacing(10).align_x(Alignment::Center).push(
        Row::new()
            .spacing(10)
            .align_y(Alignment::Center)
            .push(
                text(format!("{:.8}", btc_balance.to_btc()))
                    .size(48)
                    .bold()
                    .color(color::ORANGE),
            )
            .push(text("BTC").size(32).color(color::ORANGE)),
    );

    if let Some(converter) = &fiat_converter {
        let fiat_amount = converter.convert(btc_balance);
        balance_section = balance_section.push(fiat_amount.to_text().size(18).color(color::GREY_3));
    }

    content = content.push(balance_section);

    if !recent_transaction.is_empty() {
        for tx in recent_transaction {
            let row = Row::new()
                .spacing(15)
                .align_y(Alignment::Start)
                .push(if let PaymentDetails::Bitcoin { .. } = tx.details {
                    Container::new(icon::bitcoin_icon().size(24).color(color::ORANGE)).padding(10)
                } else {
                    Container::new(icon::lightning_icon().size(24).color(color::ORANGE)).padding(10)
                })
                .push(
                    Column::new()
                        .spacing(5)
                        .push(p1_bold(&tx.description).bold())
                        .push(
                            Row::new()
                                .push_maybe(if !matches!(tx.status, PaymentState::Pending) {
                                    Some(p2_regular(&tx.time_ago).style(theme::text::secondary))
                                } else {
                                    None
                                })
                                .push_maybe({
                                    if matches!(tx.status, PaymentState::Pending) {
                                        let (bg, fg) = (color::GREY_3, color::BLACK);
                                        Some(
                                            Container::new(
                                                Row::new()
                                                    .push(icon::warning_icon().size(14).style(
                                                        move |_| iced::widget::text::Style {
                                                            color: Some(fg),
                                                        },
                                                    ))
                                                    .push(text("Pending").bold().size(14).style(
                                                        move |_| iced::widget::text::Style {
                                                            color: Some(fg),
                                                        },
                                                    ))
                                                    .spacing(4),
                                            )
                                            .padding([2, 8])
                                            .style(
                                                move |_| iced::widget::container::Style {
                                                    background: Some(iced::Background::Color(bg)),
                                                    border: iced::Border {
                                                        radius: 12.0.into(),
                                                        ..Default::default()
                                                    },
                                                    ..Default::default()
                                                },
                                            ),
                                        )
                                    } else {
                                        None
                                    }
                                })
                                .spacing(8),
                        ),
                )
                .push(iced::widget::Space::new().width(Length::Fill))
                .push(
                    Column::new()
                        .spacing(5)
                        .align_x(Alignment::End)
                        .push(
                            text(format!("{} {:.8} BTC", tx.sign, tx.amount.to_btc()))
                                .size(16)
                                .color(if tx.is_incoming {
                                    color::GREEN
                                } else {
                                    color::RED
                                }),
                        )
                        .push(if let Some(fiat_amount) = &tx.fiat_amount {
                            text(format!(
                                "about {} {}",
                                fiat_amount.to_rounded_string(),
                                fiat_amount.currency().to_string()
                            ))
                            .size(14)
                            .color(color::GREY_3)
                        } else {
                            text("").size(12)
                        }),
                );
            let tx = Container::new(row)
                .padding(20)
                .style(theme::card::simple)
                .width(Length::Fill)
                .max_width(800);
            content = content.push(tx);
            content = content.push(Space::new().width(Length::Fill).height(5));
        }
    }

    let history_button = button::transparent(Some(icon::history_icon()), "History")
        .on_press(ActiveSendMessage::History)
        .width(Length::Fixed(150.0));

    content = content
        .push(iced::widget::Space::new().height(Length::Fixed(10.0)))
        .push(
            Container::new(history_button)
                .width(Length::Fill)
                .align_x(Alignment::Center),
        );

    content = content.push(iced::widget::Space::new().height(Length::Fixed(20.0)));

    // Input Section
    let input_section = Column::new()
        .spacing(20)
        .width(Length::Fill)
        .max_width(800)
        .align_x(Alignment::Center)
        .push(
            Container::new(
                text("Enter Invoice, Lightning Address, or BTC Address")
                    .size(16)
                    .bold(),
            )
            .width(Length::Fill)
            .align_x(Alignment::Center),
        )
        .push(
            Row::new()
                .spacing(10)
                .align_y(Alignment::Center)
                .push(
                    form::Form::new(
                        "e.g. satoshi@nakamoto.com",
                        input,
                        ActiveSendMessage::InputEdited,
                    )
                    .size(16)
                    .padding(15),
                )
                .push(
                    Container::new(
                        iced::widget::button(
                            Container::new(icon::arrow_right())
                                .width(Length::Fill)
                                .height(Length::Fill)
                                .align_x(Alignment::Center)
                                .align_y(Alignment::Center),
                        )
                        .on_press_maybe(
                            if input.valid && !input.value.trim().is_empty() && input_type.is_some()
                            {
                                Some(ActiveSendMessage::Send)
                            } else {
                                None
                            },
                        )
                        .width(Length::Fixed(50.0))
                        .height(Length::Fixed(50.0))
                        .style(theme::button::primary),
                    )
                    .width(Length::Fixed(50.0))
                    .height(Length::Fixed(50.0)),
                ),
        );

    content = content.push(input_section);

    content.into()
}

pub struct RecentTransaction {
    pub description: String,
    pub time_ago: String,
    pub amount: Amount,
    pub fiat_amount: Option<FiatAmount>,
    pub is_incoming: bool,
    pub sign: &'static str,
    pub status: PaymentState,
    pub details: PaymentDetails,
}

pub fn amount_input_model<'a>(
    amount: &'a form::Value<String>,
    comment: String,
    has_fiat_converter: bool,
    btc_balance: Amount,
    description: Option<&'a str>,
    lightning_limits: Option<(u64, u64)>,
    onchain_limits: Option<(u64, u64)>,
    input_type: &'a Option<InputType>,
) -> Element<'a, ActiveSendMessage> {
    let mut content = Column::new()
        .spacing(20)
        .padding(30)
        .width(Length::Fixed(500.0))
        .align_x(Alignment::Center);

    let header = Row::new()
        .push(iced::widget::Space::new().width(Length::Fill))
        .push(text("BALANCE: ").size(16))
        .push(
            text(format!("{} BTC", btc_balance.to_btc()))
                .size(16)
                .color(color::ORANGE),
        )
        .width(Length::Fill)
        .align_y(Alignment::Center);

    content = content.push(header);

    if let Some(desc) = description {
        content = content.push(
            Container::new(text(desc).size(16))
                .padding([10, 20])
                .width(Length::Fill)
                .style(
                    |_theme: &coincube_ui::theme::Theme| iced::widget::container::Style {
                        background: Some(iced::Background::Color(iced::Color::from_rgb(
                            0.15, 0.15, 0.15,
                        ))),
                        border: iced::Border {
                            color: iced::Color::from_rgb(0.6, 0.4, 0.2),
                            width: 2.0,
                            radius: 50.0.into(),
                        },
                        ..Default::default()
                    },
                ),
        );
    }

    let mut amount_label_section = Column::new().spacing(2);

    let amount_row = Row::new()
        .spacing(10)
        .push(text("Amount").size(16))
        .push(iced::widget::Space::new().width(Length::Fill))
        .align_y(Alignment::Center);

    let amount_row = if has_fiat_converter {
        amount_row.push(
            button::transparent(None, "⇄")
                .on_press(ActiveSendMessage::PopupMessage(
                    view::SendPopupMessage::FiatConvert,
                ))
                .width(Length::Shrink),
        )
    } else {
        amount_row
    };

    amount_label_section = amount_label_section.push(amount_row);

    let mut amount_input_section = Column::new().spacing(5);

    amount_input_section = amount_input_section.push(
        form::Form::new_amount_btc("Enter amount", &amount, |v| {
            ActiveSendMessage::PopupMessage(view::SendPopupMessage::AmountEdited(v))
        })
        .padding(10),
    );

    if let Some(input_type) = input_type {
        if matches!(input_type, InputType::BitcoinAddress { .. }) {
            if let Some((min_sat, max_sat)) = onchain_limits {
                let min_btc = Amount::from_sat(min_sat).to_btc();
                let max_btc = Amount::from_sat(max_sat).to_btc();
                amount_input_section = amount_input_section.push(
                    text(format!(
                        "Enter an amount between {} BTC and {} BTC",
                        min_btc, max_btc
                    ))
                    .size(12),
                );
            }
        } else {
            if let Some((min_sat, max_sat)) = lightning_limits {
                let min_btc = Amount::from_sat(min_sat).to_btc();
                let max_btc = Amount::from_sat(max_sat).to_btc();
                amount_input_section = amount_input_section.push(
                    text(format!(
                        "Enter an amount between {} BTC and {} BTC",
                        min_btc, max_btc
                    ))
                    .size(12),
                );
            }
        }
    }

    amount_label_section = amount_label_section.push(amount_input_section);
    content = content.push(amount_label_section);

    content = content.push(iced::widget::Space::new().height(Length::Fixed(5.0)));

    let mut comment_section = Column::new().spacing(5);
    comment_section = comment_section.push(text("Comment").size(16));
    comment_section = comment_section.push(
        iced::widget::text_input("Comment (Optional)", &comment)
            .on_input(|v| ActiveSendMessage::PopupMessage(view::SendPopupMessage::CommentEdited(v)))
            .padding(10),
    );

    content = content.push(comment_section);

    let next_button = button::primary(None, "Next").width(Length::Fill);
    let next_button = if !amount.valid || amount.value.is_empty() {
        next_button
    } else {
        next_button.on_press(ActiveSendMessage::PopupMessage(
            view::SendPopupMessage::Done,
        ))
    };

    content = content.push(next_button);

    Container::new(content)
        .padding(20)
        .style(coincube_ui::theme::card::simple)
        .into()
}

pub fn fiat_input_model<'a>(
    fiat_input: &'a form::Value<String>,
    currencies: &'a [crate::services::fiat::Currency; 4],
    selected_currency: &'a crate::services::fiat::Currency,
    converters: &'a std::collections::HashMap<crate::services::fiat::Currency, FiatAmountConverter>,
) -> Element<'a, ActiveSendMessage> {
    use coincube_ui::component::amount::DisplayAmount;
    use coincube_ui::icon::cross_icon;

    let mut content = Column::new()
        .spacing(15)
        .padding(30)
        .width(Length::Fixed(500.0))
        .align_x(Alignment::Center);

    let header = Row::new()
        .push(text("Select Fiat Currency:").size(20).bold())
        .push(iced::widget::Space::new().width(Length::Fill))
        .push(
            button::transparent(Some(cross_icon()), "")
                .on_press(ActiveSendMessage::PopupMessage(
                    view::SendPopupMessage::FiatClose,
                ))
                .width(Length::Shrink),
        )
        .width(Length::Fill)
        .align_y(Alignment::Center);

    content = content.push(header);

    let mut currency_row = Row::new().spacing(10).align_y(Alignment::Center);

    for currency in currencies.iter() {
        let is_selected = currency == selected_currency;
        let currency_str = &currency.to_static_str();

        let capsule = button::primary(None, currency_str)
            .on_press(ActiveSendMessage::PopupMessage(
                view::SendPopupMessage::FiatCurrencySelected(*currency),
            ))
            .width(Length::Shrink)
            .style(move |_theme, status| {
                let bg_color = if is_selected {
                    iced::Color::from_rgb(1.0, 0.647, 0.0)
                } else {
                    iced::Color::from_rgb(0.15, 0.15, 0.15)
                };

                let text_color = if is_selected {
                    iced::Color::BLACK
                } else {
                    iced::Color::WHITE
                };

                let base_style = iced::widget::button::Style {
                    background: Some(iced::Background::Color(bg_color)),
                    text_color,
                    border: iced::Border {
                        radius: 20.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                };

                match status {
                    iced::widget::button::Status::Hovered => iced::widget::button::Style {
                        background: Some(iced::Background::Color(iced::Color {
                            a: 0.8,
                            ..bg_color
                        })),
                        ..base_style
                    },
                    _ => base_style,
                }
            });

        currency_row = currency_row.push(capsule);
    }

    content = content.push(
        Container::new(currency_row)
            .width(Length::Fill)
            .align_x(Alignment::Center),
    );

    content = content.push(
        text(format!("Amount in {}", selected_currency))
            .size(16)
            .width(Length::Fill),
    );

    content = content.push(
        form::Form::new_amount_numeric(&format!("{} amount", selected_currency), fiat_input, |v| {
            ActiveSendMessage::PopupMessage(view::SendPopupMessage::FiatInputEdited(v))
        })
        .padding(10),
    );

    let (btc_amount_str, rate_str) = if let Some(converter) = converters.get(selected_currency) {
        let btc_amount = if !fiat_input.value.is_empty() {
            if let Ok(fiat_amount) = FiatAmount::from_str_in(&fiat_input.value, *selected_currency)
            {
                if let Ok(btc_amt) = converter.convert_to_btc(&fiat_amount) {
                    format!("{:.8} BTC", btc_amt.to_btc())
                } else {
                    "0.00000000 BTC".to_string()
                }
            } else {
                "0.00000000 BTC".to_string()
            }
        } else {
            "0.00000000 BTC".to_string()
        };

        let rate = format!(
            "1 BTC = {} {}",
            converter.to_fiat_amount().to_formatted_string(),
            selected_currency
        );

        (btc_amount, rate)
    } else {
        ("Loading...".to_string(), "Fetching rate...".to_string())
    };

    let btc_conversion_section = Column::new()
        .spacing(2)
        .align_x(Alignment::Center)
        .push(text(btc_amount_str).size(18).bold())
        .push(
            text(rate_str)
                .size(14)
                .color(iced::Color::from_rgb(0.7, 0.7, 0.7)),
        );

    content = content.push(
        Container::new(btc_conversion_section)
            .width(Length::Fill)
            .align_x(Alignment::Center),
    );

    content = content.push(iced::widget::Space::new().height(Length::Fixed(5.0)));

    let done_button = button::primary(None, "Done").width(Length::Fill);
    let done_button = if !fiat_input.valid || fiat_input.value.is_empty() {
        done_button
    } else {
        done_button.on_press(ActiveSendMessage::PopupMessage(
            view::SendPopupMessage::FiatDone,
        ))
    };

    content = content.push(done_button);

    Container::new(content)
        .padding(20)
        .style(coincube_ui::theme::card::simple)
        .into()
}

pub fn final_check_page<'a>(
    amount: Amount,
    comment: String,
    description: Option<&'a str>,
    fiat_converter: Option<&FiatAmountConverter>,
    prepare_response: Option<&'a breez_sdk_liquid::prelude::PrepareSendResponse>,
    is_sending: bool,
) -> Element<'a, ActiveSendMessage> {
    let header = Row::new()
        .push(
            button::transparent(Some(icon::previous_icon()), "Previous").on_press(
                ActiveSendMessage::PopupMessage(view::SendPopupMessage::Close),
            ),
        )
        .push(Space::new().width(Length::Fill))
        .width(Length::Fill)
        .padding([0, 40])
        .align_y(Alignment::Center);

    let mut content = Column::new()
        .spacing(25)
        .padding(40)
        .width(Length::Fill)
        .max_width(600)
        .align_x(Alignment::Center);

    if let Some(desc) = description {
        content = content.push(
            Container::new(text(desc).size(22).bold())
                .width(Length::Fill)
                .align_x(Alignment::Center),
        );
    }

    content = content.push(Space::new().height(Length::Fixed(2.0)));

    let (fees_sat, total_sat) = if let Some(prepare) = prepare_response {
        let fees = prepare.fees_sat.unwrap_or(0);
        let total = amount.to_sat() + fees;
        (fees, total)
    } else {
        (0, amount.to_sat())
    };

    let fees_amount = Amount::from_sat(fees_sat);
    let total_amount = Amount::from_sat(total_sat);

    content = content.push(
        Container::new(
            text(format!("{:.8} BTC", amount.to_btc()))
                .size(38)
                .bold()
                .color(color::ORANGE),
        )
        .width(Length::Fill)
        .align_x(Alignment::Center),
    );

    if let Some(converter) = fiat_converter {
        let fiat_amount = converter.convert(amount);
        content = content.push(
            Container::new(fiat_amount.to_text().size(18).color(color::GREY_3))
                .width(Length::Fill)
                .align_x(Alignment::Center),
        );
    }

    content = content.push(Space::new().height(Length::Fixed(10.0)));

    let mut details_box = Column::new().spacing(15).width(Length::Fill).padding(20);

    details_box = details_box.push(
        Row::new()
            .push(text("Amount:").size(16))
            .push(Space::new().width(Length::Fill))
            .push(text(format!("{:.8} BTC", amount.to_btc())).size(16).bold())
            .width(Length::Fill)
            .align_y(Alignment::Center),
    );

    details_box = details_box.push(
        Container::new(Space::new().height(Length::Fixed(1.0)))
            .width(Length::Fill)
            .style(
                |_theme: &coincube_ui::theme::Theme| iced::widget::container::Style {
                    background: Some(iced::Background::Color(color::GREY_3)),
                    ..Default::default()
                },
            ),
    );

    details_box = details_box.push(
        Row::new()
            .push(text("Fees:").size(16))
            .push(Space::new().width(Length::Fill))
            .push(
                text(format!("+ {:.8} BTC", fees_amount.to_btc()))
                    .size(16)
                    .bold(),
            )
            .width(Length::Fill)
            .align_y(Alignment::Center),
    );

    details_box = details_box.push(
        Container::new(Space::new().height(Length::Fixed(1.0)))
            .width(Length::Fill)
            .style(
                |_theme: &coincube_ui::theme::Theme| iced::widget::container::Style {
                    background: Some(iced::Background::Color(color::GREY_3)),
                    ..Default::default()
                },
            ),
    );

    details_box = details_box.push(
        Row::new()
            .push(text("Total:").size(18).bold())
            .push(Space::new().width(Length::Fill))
            .push(
                text(format!("{:.8} BTC", total_amount.to_btc()))
                    .size(18)
                    .bold()
                    .color(color::ORANGE),
            )
            .width(Length::Fill)
            .align_y(Alignment::Center),
    );

    if !comment.is_empty() {
        details_box = details_box.push(
            Container::new(Space::new().height(Length::Fixed(1.0)))
                .width(Length::Fill)
                .style(
                    |_theme: &coincube_ui::theme::Theme| iced::widget::container::Style {
                        background: Some(iced::Background::Color(color::GREY_3)),
                        ..Default::default()
                    },
                ),
        );

        details_box = details_box.push(
            Row::new()
                .push(text("Comment:").size(16))
                .push(Space::new().width(Length::Fill))
                .push(text(&comment).size(16).bold())
                .width(Length::Fill)
                .align_y(Alignment::Center),
        );
    }

    content = content.push(Container::new(details_box).width(Length::Fill).style(
        |_theme: &coincube_ui::theme::Theme| iced::widget::container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgb(
                0.15, 0.15, 0.15,
            ))),
            border: iced::Border {
                radius: 12.0.into(),
                ..Default::default()
            },
            ..Default::default()
        },
    ));

    content = content.push(Space::new().height(Length::Fixed(30.0)));

    let send_button = button::primary(None, "Send").width(Length::Fill);
    content = content.push(if is_sending {
        send_button
    } else {
        send_button.on_press(ActiveSendMessage::ConfirmSend)
    });

    Column::new()
        .push(header)
        .push(
            Container::new(content)
                .width(Length::Fill)
                .align_x(Alignment::Center),
        )
        .width(Length::Fill)
        .into()
}

pub fn sent_page<'a>(amount: Amount) -> Element<'a, ActiveSendMessage> {
    use coincube_ui::widget::{Column, Row};
    Column::new()
        .spacing(20)
        .width(Length::Fill)
        .push(Space::new().height(Length::Fixed(20.0)))
        .push(
            Row::new()
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .push(Space::new().width(Length::Fill))
                .push(icon::check_circle().size(140).color(color::ORANGE))
                .push(Space::new().width(Length::Fill)),
        )
        .push(Space::new().height(Length::Fixed(16.0)))
        .push(
            Row::new()
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .push(Space::new().width(Length::Fill))
                .push(
                    Column::new()
                        .width(Length::Shrink)
                        .align_x(Alignment::Center)
                        .push(h3("Transaction complete!")),
                )
                .push(Space::new().width(Length::Fill)),
        )
        .push(
            Row::new()
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .push(Space::new().width(Length::Fill))
                .push(
                    Row::new()
                        .spacing(5)
                        .push(
                            text(format!("{:.8} BTC", amount.to_btc()))
                                .size(20)
                                .color(color::ORANGE)
                                .font(iced::Font {
                                    style: iced::font::Style::Italic,
                                    ..Default::default()
                                }),
                        )
                        .push(
                            text("has been sent successfully.")
                                .size(20)
                                .font(iced::Font {
                                    style: iced::font::Style::Italic,
                                    ..Default::default()
                                }),
                        ),
                )
                .push(Space::new().width(Length::Fill)),
        )
        .push(Space::new().height(Length::Fixed(20.0)))
        .push(
            Row::new()
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .push(Space::new().width(Length::Fill))
                .push(
                    button::primary(None, "Back")
                        .width(Length::Fixed(150.0))
                        .on_press(ActiveSendMessage::BackToHome),
                )
                .push(Space::new().width(Length::Fill)),
        )
        .into()
}
