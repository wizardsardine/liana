use breez_sdk_liquid::{
    model::{PaymentDetails, PaymentState},
    InputType,
};
use coincube_core::miniscript::bitcoin::Amount;
use coincube_ui::{
    color,
    component::{
        amount::*,
        button, form,
        text::*,
        transaction::{TransactionDirection, TransactionListItem, TransactionType},
    },
    icon::{self, receipt_icon},
    theme,
    widget::*,
};
use iced::{
    widget::{button as iced_button, container, Column, Container, Row, Space},
    Alignment, Background, Length,
};

use crate::app::breez::assets::{format_usdt_display, AssetKind};
use crate::app::menu::Menu;
use crate::app::state::liquid::send::{LiquidSendFlowState, Modal, SendAsset};
use crate::app::view::{
    self, vault::fiat::FiatAmount, FiatAmountConverter, LiquidSendMessage, Message,
};
use crate::{app::cache::Cache, loading::loading_indicator};

pub struct LiquidSendFlowConfig<'a> {
    pub flow_state: &'a LiquidSendFlowState,
    pub btc_balance: Amount,
    pub usdt_balance: u64,
    pub fiat_converter: Option<FiatAmountConverter>,
    pub recent_transaction: &'a Vec<RecentTransaction>,
    pub input: &'a form::Value<String>,
    pub amount_input: &'a form::Value<String>,
    pub usdt_amount_input: &'a form::Value<String>,
    pub send_asset: SendAsset,
    pub from_asset: Option<SendAsset>,
    pub uri_asset: Option<AssetKind>,
    pub usdt_asset_id: &'a str,
    pub comment: String,
    pub description: Option<&'a str>,
    pub lightning_limits: Option<(u64, u64)>,
    pub amount: Amount,
    pub prepare_response: Option<&'a breez_sdk_liquid::prelude::PrepareSendResponse>,
    pub is_sending: bool,
    pub menu: &'a Menu,
    pub cache: &'a Cache,
    pub input_type: &'a Option<InputType>,
    pub onchain_limits: Option<(u64, u64)>,
    pub bitcoin_unit: BitcoinDisplayUnit,
    pub prepare_onchain_response: Option<&'a breez_sdk_liquid::prelude::PreparePayOnchainResponse>,
}

pub fn liquid_send_with_flow<'a>(config: LiquidSendFlowConfig<'a>) -> Element<'a, Message> {
    let base_content = match config.flow_state {
        LiquidSendFlowState::Main { modal } => {
            let send_view = liquid_send_view(
                config.btc_balance,
                config.usdt_balance,
                config.send_asset,
                config.fiat_converter,
                config.recent_transaction,
                config.input,
                config.input_type,
                config.bitcoin_unit,
                config.usdt_asset_id,
            )
            .map(Message::LiquidSend);

            let content = view::dashboard(config.menu, config.cache, send_view);

            // Show modal if needed
            match modal {
                Modal::AmountInput => {
                    let modal_content = amount_input_model(AmountInputConfig {
                        amount: config.amount_input,
                        usdt_amount_input: config.usdt_amount_input,
                        send_asset: config.send_asset,
                        from_asset: config.from_asset,
                        uri_asset: config.uri_asset,
                        usdt_balance: config.usdt_balance,
                        comment: config.comment,
                        has_fiat_converter: config.fiat_converter.is_some(),
                        btc_balance: config.btc_balance,
                        description: config.description,
                        lightning_limits: config.lightning_limits,
                        onchain_limits: config.onchain_limits,
                        input_type: config.input_type,
                        bitcoin_unit: config.bitcoin_unit,
                    })
                    .map(Message::LiquidSend);
                    coincube_ui::widget::modal::Modal::new(content, modal_content)
                        .on_blur(Some(Message::LiquidSend(LiquidSendMessage::PopupMessage(
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
                    let modal_content = fiat_input_model(
                        fiat_input,
                        currencies,
                        selected_currency,
                        converters,
                        config.bitcoin_unit,
                    )
                    .map(Message::LiquidSend);
                    coincube_ui::widget::modal::Modal::new(content, modal_content)
                        .on_blur(Some(Message::LiquidSend(LiquidSendMessage::PopupMessage(
                            view::SendPopupMessage::FiatClose,
                        ))))
                        .into()
                }
                Modal::None => content,
            }
        }
        LiquidSendFlowState::FinalCheck => {
            let content = final_check_page(
                config.amount,
                config.comment,
                config.description,
                config.fiat_converter.as_ref(),
                config.prepare_response,
                config.is_sending,
                config.bitcoin_unit,
                config.input_type,
                config.prepare_onchain_response,
                config.send_asset,
                config.usdt_amount_input.value.trim(),
                config.from_asset,
            )
            .map(Message::LiquidSend);
            view::dashboard(config.menu, config.cache, content)
        }
        LiquidSendFlowState::Sent => {
            let content = sent_page(
                config.amount,
                config.bitcoin_unit,
                config.send_asset,
                config.usdt_amount_input.value.trim(),
            )
            .map(Message::LiquidSend);
            view::dashboard(config.menu, config.cache, content)
        }
    };

    base_content
}

#[allow(clippy::too_many_arguments)]
pub fn liquid_send_view<'a>(
    btc_balance: Amount,
    usdt_balance: u64,
    send_asset: SendAsset,
    fiat_converter: Option<FiatAmountConverter>,
    recent_transaction: &[RecentTransaction],
    input: &'a form::Value<String>,
    input_type: &'a Option<InputType>,
    bitcoin_unit: BitcoinDisplayUnit,
    usdt_asset_id: &str,
) -> Element<'a, LiquidSendMessage> {
    let mut content = Column::new().spacing(20);

    // Balance card — matching Liquid Overview style
    let amount_col = match send_asset {
        SendAsset::Usdt => Column::new()
            .spacing(4)
            .push(
                Row::new()
                    .spacing(10)
                    .align_y(Alignment::Center)
                    .push(text(format_usdt_display(usdt_balance)).size(H2_SIZE).bold())
                    .push(text("USDt").size(H2_SIZE).color(color::GREY_3)),
            )
            .push(
                text("Liquid Network")
                    .size(P1_SIZE)
                    .style(theme::text::secondary),
            ),
        SendAsset::Btc => {
            let fiat_balance = fiat_converter.as_ref().map(|c| c.convert(btc_balance));
            let mut col = Column::new().spacing(4).push(amount_with_size_and_unit(
                &btc_balance,
                H2_SIZE,
                bitcoin_unit,
            ));
            if let Some(fiat) = fiat_balance {
                col = col.push(fiat.to_text().size(P1_SIZE).style(theme::text::secondary));
            }
            col
        }
    };
    let balance_inner = Column::new()
        .spacing(8)
        .push(h4_bold("Balance"))
        .push(amount_col);
    content = content.push(view::balance_header_card(balance_inner));

    // Input hint changes based on asset
    let hint_text = match send_asset {
        SendAsset::Usdt => "Enter Liquid Address",
        SendAsset::Btc => "Enter Invoice, Lightning Address, Liquid BTC Address or BTC Address",
    };

    // Input Section - centered
    let input_section = Column::new()
        .spacing(20)
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .push(
            Container::new(h4_bold(hint_text))
                .padding(iced::Padding::new(0.0).top(5))
                .width(Length::Fill),
        )
        .push(
            Row::new()
                .spacing(10)
                .align_y(Alignment::Center)
                .push(
                    form::Form::new(
                        "e.g. satoshi@nakamoto.com",
                        input,
                        LiquidSendMessage::InputEdited,
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
                                Some(LiquidSendMessage::Send)
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

    // Last transactions section - left justified, after input
    content = content.push(Column::new().spacing(10).push(h4_bold("Last transactions")));

    if !recent_transaction.is_empty() {
        for (idx, tx) in recent_transaction.iter().enumerate() {
            let direction = if tx.is_incoming {
                TransactionDirection::Incoming
            } else {
                TransactionDirection::Outgoing
            };

            let tx_type = match &tx.details {
                PaymentDetails::Lightning { .. } => TransactionType::Lightning,
                PaymentDetails::Liquid { .. } | PaymentDetails::Bitcoin { .. } => {
                    TransactionType::Bitcoin
                }
            };

            let usdt_display = if let PaymentDetails::Liquid { asset_id, .. } = &tx.details {
                if !usdt_asset_id.is_empty() && asset_id == usdt_asset_id {
                    Some(format!("{} USDt", format_usdt_display(tx.amount.to_sat())))
                } else {
                    None
                }
            } else {
                None
            };

            let fiat_str = if usdt_display.is_some() {
                None
            } else {
                tx.fiat_amount
                    .as_ref()
                    .map(|fiat| format!("~{} {}", fiat.to_rounded_string(), fiat.currency()))
            };

            let display_amount = if usdt_display.is_some() {
                Amount::ZERO
            } else if tx.is_incoming {
                tx.amount
            } else {
                tx.amount + tx.fees_sat
            };

            let mut item = TransactionListItem::new(direction, &display_amount, bitcoin_unit)
                .with_type(tx_type)
                .with_label(tx.description.clone())
                .with_time_ago(tx.time_ago.clone());

            if let Some(ref usdt_str) = usdt_display {
                item = item.with_amount_override(usdt_str.clone());
            }

            if matches!(tx.status, PaymentState::Pending) {
                let (bg, fg) = (color::GREY_3, color::BLACK);
                let pending_badge = Container::new(
                    Row::new()
                        .push(
                            icon::warning_icon()
                                .size(14)
                                .style(move |_| iced::widget::text::Style { color: Some(fg) }),
                        )
                        .push(
                            text("Pending")
                                .bold()
                                .size(14)
                                .style(move |_| iced::widget::text::Style { color: Some(fg) }),
                        )
                        .spacing(4),
                )
                .padding([2, 8])
                .style(move |_| iced::widget::container::Style {
                    background: Some(iced::Background::Color(bg)),
                    border: iced::Border {
                        radius: 12.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                });
                item = item.with_custom_status(pending_badge.into());
            }

            if let Some(fiat) = fiat_str {
                item = item.with_fiat_amount(fiat);
            }

            content = content.push(item.view(LiquidSendMessage::SelectTransaction(idx)));
        }
    } else {
        content = content.push(empty_tx_placeholder(
            receipt_icon().size(80),
            "No transactions yet",
            "Your transaction history will appear here once you send or receive coins.",
        ));
    }

    let view_transaction_button = {
        let icon = icon::history_icon()
            .size(18)
            .style(|_theme: &theme::Theme| iced::widget::text::Style {
                color: Some(color::ORANGE),
            });

        let label = text("View All Transactions")
            .size(15)
            .style(|_theme: &theme::Theme| iced::widget::text::Style {
                color: Some(color::ORANGE),
            });

        let button_content = Row::new()
            .spacing(8)
            .align_y(iced::alignment::Vertical::Center)
            .push(icon)
            .push(label);

        iced_button(Container::new(button_content).padding([10, 20]).style(
            |_theme: &theme::Theme| container::Style {
                background: Some(Background::Color(color::TRANSPARENT)),
                border: iced::Border {
                    color: color::ORANGE,
                    width: 1.5,
                    radius: 20.0.into(),
                },
                ..Default::default()
            },
        ))
        .style(|_theme: &theme::Theme, status| iced_button::Style {
            background: Some(Background::Color(color::TRANSPARENT)),
            text_color: match status {
                iced_button::Status::Hovered => iced::color!(0xFF9D42),
                iced_button::Status::Pressed => color::ORANGE,
                _ => color::ORANGE,
            },
            border: iced::Border {
                radius: 20.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .on_press(LiquidSendMessage::History)
    };

    if !recent_transaction.is_empty() {
        content = content
            .push(iced::widget::Space::new().height(Length::Fixed(20.0)))
            .push(
                Container::new(view_transaction_button)
                    .width(Length::Fill)
                    .center_x(Length::Fill),
            );
    }

    content.into()
}

pub struct RecentTransaction {
    pub description: String,
    pub time_ago: String,
    pub amount: Amount,
    pub fees_sat: Amount,
    pub fiat_amount: Option<FiatAmount>,
    pub is_incoming: bool,
    pub status: PaymentState,
    pub details: PaymentDetails,
}

pub struct AmountInputConfig<'a> {
    pub amount: &'a form::Value<String>,
    pub usdt_amount_input: &'a form::Value<String>,
    pub send_asset: SendAsset,
    pub from_asset: Option<SendAsset>,
    pub uri_asset: Option<AssetKind>,
    pub usdt_balance: u64,
    pub comment: String,
    pub has_fiat_converter: bool,
    pub btc_balance: Amount,
    pub description: Option<&'a str>,
    pub lightning_limits: Option<(u64, u64)>,
    pub onchain_limits: Option<(u64, u64)>,
    pub input_type: &'a Option<InputType>,
    pub bitcoin_unit: BitcoinDisplayUnit,
}

pub fn amount_input_model<'a>(config: AmountInputConfig<'a>) -> Element<'a, LiquidSendMessage> {
    let mut content = Column::new()
        .spacing(20)
        .padding(30)
        .width(Length::Fixed(500.0))
        .align_x(Alignment::Center);

    // Show balance of the asset being paid from (from_asset if cross-asset, else send_asset)
    let paying_asset = config.from_asset.unwrap_or(config.send_asset);
    let balance_text = match paying_asset {
        SendAsset::Usdt => format!("{} USDt", format_usdt_display(config.usdt_balance)),
        SendAsset::Btc => format!(
            "{} {}",
            if matches!(config.bitcoin_unit, BitcoinDisplayUnit::BTC) {
                config.btc_balance.to_btc().to_string()
            } else {
                config.btc_balance.to_sat().to_string()
            },
            config.bitcoin_unit
        ),
    };

    let header = Row::new()
        .push(iced::widget::Space::new().width(Length::Fill))
        .push(text("BALANCE: ").size(16))
        .push(text(balance_text).size(16).bold().color(color::ORANGE))
        .width(Length::Fill)
        .align_y(Alignment::Center);

    content = content.push(header);

    // Cross-asset swap indicator and toggle
    if config.uri_asset.is_some() || config.from_asset.is_some() {
        let is_cross_asset = config.from_asset.is_some();
        let toggle_label = if is_cross_asset {
            let paying_with = match config.from_asset.unwrap() {
                SendAsset::Btc => "L-BTC",
                SendAsset::Usdt => "USDt",
            };
            let receiving = match config.send_asset {
                SendAsset::Btc => "L-BTC",
                SendAsset::Usdt => "USDt",
            };
            format!(
                "Paying with {} → Receiver gets {} (swap)",
                paying_with, receiving
            )
        } else {
            let asset_name = match config.send_asset {
                SendAsset::Btc => "L-BTC",
                SendAsset::Usdt => "USDt",
            };
            format!("Paying with {}", asset_name)
        };

        let swap_toggle =
            iced_button(
                Container::new(
                    Row::new()
                        .spacing(6)
                        .align_y(Alignment::Center)
                        .push(icon::left_right_icon().size(14).style(|_| {
                            iced::widget::text::Style {
                                color: Some(color::ORANGE),
                            }
                        }))
                        .push(text(toggle_label).size(13).color(color::ORANGE)),
                )
                .padding([4, 12]),
            )
            .on_press(LiquidSendMessage::PopupMessage(
                view::SendPopupMessage::ToggleSendAsset,
            ))
            .style(|_, _| iced::widget::button::Style {
                background: Some(Background::Color(iced::Color::TRANSPARENT)),
                text_color: color::ORANGE,
                border: iced::Border {
                    color: color::ORANGE,
                    width: 1.0,
                    radius: 15.0.into(),
                },
                ..Default::default()
            });

        content = content.push(
            Container::new(swap_toggle)
                .width(Length::Fill)
                .center_x(Length::Fill),
        );
    }

    if let Some(desc) = config.description {
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

    // Amount section — branched on the pre-selected asset (no toggle)
    match config.send_asset {
        SendAsset::Usdt => {
            let mut usdt_col = Column::new()
                .spacing(5)
                .push(text("Amount (USDt)").size(16))
                .push(
                    iced::widget::text_input("e.g. 1.50", &config.usdt_amount_input.value)
                        .on_input(|v| {
                            LiquidSendMessage::PopupMessage(
                                view::SendPopupMessage::UsdtAmountEdited(v),
                            )
                        })
                        .padding(10),
                );
            if let Some(warn) = config.usdt_amount_input.warning {
                usdt_col = usdt_col.push(text(warn).size(12).color(color::ORANGE));
            }
            content = content.push(usdt_col);
        }
        SendAsset::Btc => {
            let mut amount_label_section = Column::new().spacing(2);

            let amount_row = Row::new()
                .spacing(10)
                .push(text(format!("Amount ({})", config.bitcoin_unit)).size(16))
                .push(iced::widget::Space::new().width(Length::Fill))
                .align_y(Alignment::Center);

            let amount_row = if config.has_fiat_converter {
                amount_row.push(
                    button::transparent(None, "⇄")
                        .on_press(LiquidSendMessage::PopupMessage(
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
                if matches!(config.bitcoin_unit, BitcoinDisplayUnit::BTC) {
                    form::Form::new_amount_btc("Enter amount", config.amount, |v| {
                        LiquidSendMessage::PopupMessage(view::SendPopupMessage::AmountEdited(v))
                    })
                    .padding(10)
                } else {
                    form::Form::new_amount_sats("Enter amount", config.amount, |v| {
                        LiquidSendMessage::PopupMessage(view::SendPopupMessage::AmountEdited(v))
                    })
                    .padding(10)
                },
            );

            if let Some(input_type) = config.input_type {
                if matches!(input_type, InputType::BitcoinAddress { .. }) {
                    if let Some((min_sat, max_sat)) = config.onchain_limits {
                        let min_btc = Amount::from_sat(min_sat);
                        let max_btc = Amount::from_sat(max_sat);
                        amount_input_section = amount_input_section.push(
                            text(format!(
                                "Enter an amount between {} and {}",
                                min_btc.to_formatted_string_with_unit(config.bitcoin_unit),
                                max_btc.to_formatted_string_with_unit(config.bitcoin_unit),
                            ))
                            .size(12),
                        );
                    }
                } else if let Some((min_sat, max_sat)) = config.lightning_limits {
                    let min_btc = Amount::from_sat(min_sat);
                    let max_btc = Amount::from_sat(max_sat);
                    amount_input_section = amount_input_section.push(
                        text(format!(
                            "Enter an amount between {} and {}",
                            min_btc.to_formatted_string_with_unit(config.bitcoin_unit),
                            max_btc.to_formatted_string_with_unit(config.bitcoin_unit),
                        ))
                        .size(12),
                    );
                }
            }

            amount_label_section = amount_label_section.push(amount_input_section);
            content = content.push(amount_label_section);
        }
    }

    content = content.push(iced::widget::Space::new().height(Length::Fixed(5.0)));

    let mut comment_section = Column::new().spacing(5);
    comment_section = comment_section.push(text("Comment").size(16));
    comment_section = comment_section.push(
        iced::widget::text_input("Comment (Optional)", &config.comment)
            .on_input(|v| LiquidSendMessage::PopupMessage(view::SendPopupMessage::CommentEdited(v)))
            .padding(10),
    );

    content = content.push(comment_section);

    // Check that the paying asset has sufficient balance
    let paying_asset = config.from_asset.unwrap_or(config.send_asset);
    let has_balance = match paying_asset {
        SendAsset::Usdt => config.usdt_balance > 0,
        SendAsset::Btc => config.btc_balance.to_sat() > 0,
    };

    let is_next_enabled = has_balance
        && match config.send_asset {
            SendAsset::Usdt
                if matches!(config.input_type, Some(InputType::LiquidAddress { .. })) =>
            {
                config.usdt_amount_input.valid && !config.usdt_amount_input.value.trim().is_empty()
            }
            _ => config.amount.valid && !config.amount.value.trim().is_empty(),
        };

    let next_button = button::primary(None, "Next").width(Length::Fill);
    let next_button = if !is_next_enabled {
        next_button
    } else {
        next_button.on_press(LiquidSendMessage::PopupMessage(
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
    bitcoin_unit: BitcoinDisplayUnit,
) -> Element<'a, LiquidSendMessage> {
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
                .on_press(LiquidSendMessage::PopupMessage(
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
            .on_press(LiquidSendMessage::PopupMessage(
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
            LiquidSendMessage::PopupMessage(view::SendPopupMessage::FiatInputEdited(v))
        })
        .padding(10),
    );

    let (btc_amount_str, rate_str) = if let Some(converter) = converters.get(selected_currency) {
        let default_string = format!(
            "{} {}",
            if matches!(bitcoin_unit, BitcoinDisplayUnit::BTC) {
                "0.00000000".to_string()
            } else {
                "0".to_string()
            },
            bitcoin_unit
        );
        let btc_amount = if !fiat_input.value.is_empty() {
            if let Ok(fiat_amount) = FiatAmount::from_str_in(&fiat_input.value, *selected_currency)
            {
                if let Ok(btc_amt) = converter.convert_to_btc(&fiat_amount) {
                    format!(
                        "{} {}",
                        if matches!(bitcoin_unit, BitcoinDisplayUnit::BTC) {
                            btc_amt.to_btc().to_string()
                        } else {
                            btc_amt.to_sat().to_string()
                        },
                        bitcoin_unit
                    )
                } else {
                    default_string
                }
            } else {
                default_string
            }
        } else {
            default_string
        };

        let amount = if matches!(bitcoin_unit, BitcoinDisplayUnit::BTC) {
            "1"
        } else {
            "1000"
        };

        let fiat_value = if matches!(bitcoin_unit, BitcoinDisplayUnit::BTC) {
            converter.to_fiat_amount().to_formatted_string()
        } else {
            (converter.price_per_btc() / 100000.0f64).to_string()
        };

        let rate = format!(
            "{} {} = {} {}",
            amount, bitcoin_unit, fiat_value, selected_currency
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
        done_button.on_press(LiquidSendMessage::PopupMessage(
            view::SendPopupMessage::FiatDone,
        ))
    };

    content = content.push(done_button);

    Container::new(content)
        .padding(20)
        .style(coincube_ui::theme::card::simple)
        .into()
}

#[allow(clippy::too_many_arguments)]
pub fn final_check_page<'a>(
    amount: Amount,
    comment: String,
    description: Option<&'a str>,
    fiat_converter: Option<&FiatAmountConverter>,
    prepare_response: Option<&'a breez_sdk_liquid::prelude::PrepareSendResponse>,
    is_sending: bool,
    bitcoin_unit: BitcoinDisplayUnit,
    input_type: &'a Option<InputType>,
    prepare_onchain_response: Option<&'a breez_sdk_liquid::prelude::PreparePayOnchainResponse>,
    send_asset: SendAsset,
    usdt_send_amount: &'a str,
    from_asset: Option<SendAsset>,
) -> Element<'a, LiquidSendMessage> {
    let header = Row::new()
        .push(
            button::transparent(Some(icon::previous_icon()), "Previous").on_press(
                LiquidSendMessage::PopupMessage(view::SendPopupMessage::Close),
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
            Container::new(text(desc.to_string()).size(22).bold())
                .width(Length::Fill)
                .align_x(Alignment::Center),
        );
    }

    // Cross-asset swap indicator
    if let Some(fa) = from_asset {
        if fa != send_asset {
            let paying_with = match fa {
                SendAsset::Btc => "L-BTC",
                SendAsset::Usdt => "USDt",
            };
            let receiving = match send_asset {
                SendAsset::Btc => "L-BTC",
                SendAsset::Usdt => "USDt",
            };
            content = content.push(
                Container::new(
                    text(format!(
                        "Cross-asset swap: paying with {} → receiver gets {}",
                        paying_with, receiving
                    ))
                    .size(14)
                    .color(color::ORANGE),
                )
                .padding([6, 12])
                .width(Length::Fill)
                .center_x(Length::Fill),
            );
        }
    }

    content = content.push(Space::new().height(Length::Fixed(2.0)));

    let fees_sat = if send_asset == SendAsset::Usdt {
        prepare_response.and_then(|p| p.fees_sat).unwrap_or(0)
    } else if let Some(input_type) = input_type {
        match input_type {
            InputType::BitcoinAddress { .. } => prepare_onchain_response
                .map(|p| p.total_fees_sat)
                .unwrap_or(0),
            _ => prepare_response.and_then(|p| p.fees_sat).unwrap_or(0),
        }
    } else {
        0
    };

    let fees_amount = Amount::from_sat(fees_sat);

    if send_asset == SendAsset::Usdt {
        // USDt send: show USDt amount prominently, L-BTC fees separately
        content = content.push(
            Container::new(
                text(format!("{} USDt", usdt_send_amount))
                    .size(38)
                    .bold()
                    .color(color::ORANGE),
            )
            .width(Length::Fill)
            .align_x(Alignment::Center),
        );

        content = content.push(Space::new().height(Length::Fixed(10.0)));

        let mut details_box = Column::new().spacing(15).width(Length::Fill).padding(20);

        details_box = details_box.push(
            Row::new()
                .push(text("Send amount:").size(16))
                .push(Space::new().width(Length::Fill))
                .push(text(format!("{} USDt", usdt_send_amount)).size(16).bold())
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
                .push(text("Fees (L-BTC):").size(16))
                .push(Space::new().width(Length::Fill))
                .push(
                    text(format!(
                        "{} {}",
                        if matches!(bitcoin_unit, BitcoinDisplayUnit::BTC) {
                            fees_amount.to_btc().to_string()
                        } else {
                            fees_amount.to_sat().to_string()
                        },
                        bitcoin_unit
                    ))
                    .size(16)
                    .bold(),
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
                    .push(text(comment.clone()).size(16).bold())
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
            send_button.on_press(LiquidSendMessage::ConfirmSend)
        });

        if is_sending {
            content = content.push(loading_indicator(None))
        }

        return Column::new()
            .push(header)
            .push(
                Container::new(content)
                    .width(Length::Fill)
                    .align_x(Alignment::Center),
            )
            .width(Length::Fill)
            .into();
    }

    let total_sat = amount.to_sat() + fees_sat;
    let total_amount = Amount::from_sat(total_sat);

    content = content.push(
        Container::new(
            text(format!(
                "{} {}",
                if matches!(bitcoin_unit, BitcoinDisplayUnit::BTC) {
                    amount.to_btc().to_string()
                } else {
                    amount.to_sat().to_string()
                },
                bitcoin_unit
            ))
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
            .push(
                text(format!(
                    "{} {}",
                    if matches!(bitcoin_unit, BitcoinDisplayUnit::BTC) {
                        amount.to_btc().to_string()
                    } else {
                        amount.to_sat().to_string()
                    },
                    bitcoin_unit
                ))
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
            .push(text("Fees:").size(16))
            .push(Space::new().width(Length::Fill))
            .push(
                text(format!(
                    "{} {}",
                    if matches!(bitcoin_unit, BitcoinDisplayUnit::BTC) {
                        fees_amount.to_btc().to_string()
                    } else {
                        fees_amount.to_sat().to_string()
                    },
                    bitcoin_unit
                ))
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
                text(format!(
                    "{} {}",
                    if matches!(bitcoin_unit, BitcoinDisplayUnit::BTC) {
                        total_amount.to_btc().to_string()
                    } else {
                        total_amount.to_sat().to_string()
                    },
                    bitcoin_unit
                ))
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
                .push(text(comment).size(16).bold())
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
        send_button.on_press(LiquidSendMessage::ConfirmSend)
    });

    if is_sending {
        content = content.push(loading_indicator(None))
    }

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

pub fn sent_page<'a>(
    amount: Amount,
    bitcoin_unit: BitcoinDisplayUnit,
    send_asset: SendAsset,
    usdt_send_amount: &str,
) -> Element<'a, LiquidSendMessage> {
    use coincube_ui::widget::{Column, Row};

    let sent_amount_str = if send_asset == SendAsset::Usdt && !usdt_send_amount.is_empty() {
        format!("{} USDt", usdt_send_amount)
    } else if matches!(bitcoin_unit, BitcoinDisplayUnit::BTC) {
        format!("{} {}", amount.to_btc(), bitcoin_unit)
    } else {
        format!("{} {}", amount.to_sat(), bitcoin_unit)
    };

    Column::new()
        .spacing(20)
        .width(Length::Fill)
        .push(Space::new().height(Length::Fixed(20.0)))
        .push(
            Row::new()
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .push(Space::new().width(Length::Fill))
                .push(icon::check_circle_icon().size(140).color(color::ORANGE))
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
                        .push(text(sent_amount_str).size(20).color(color::ORANGE).font(
                            iced::Font {
                                style: iced::font::Style::Italic,
                                ..Default::default()
                            },
                        ))
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
                        .on_press(LiquidSendMessage::BackToHome),
                )
                .push(Space::new().width(Length::Fill)),
        )
        .into()
}

pub fn empty_tx_placeholder<'a, T: Into<Element<'a, LiquidSendMessage>>>(
    icon: T,
    title: &'a str,
    subtitle: &'a str,
) -> Element<'a, LiquidSendMessage> {
    let content = Column::new()
        .push(icon)
        .push(text(title).style(theme::text::secondary).bold())
        .push(
            text(subtitle)
                .size(P2_SIZE)
                .style(theme::text::secondary)
                .align_x(Alignment::Center),
        )
        .spacing(16)
        .align_x(Alignment::Center);

    Container::new(content)
        .width(Length::Fill)
        .padding(60)
        .center_x(Length::Fill)
        .style(|_| container::Style {
            background: Some(iced::Background::Color(color::GREY_6)),
            border: iced::Border {
                radius: 20.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}
