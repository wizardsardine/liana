use breez_sdk_liquid::InputType;
use coincube_core::miniscript::bitcoin::Amount;
use coincube_ui::{
    color,
    component::{
        amount::*,
        button, form,
        text::*,
        transaction::{TransactionDirection, TransactionListItem},
    },
    icon::{self, receipt_icon},
    theme,
    widget::*,
};
use iced::{
    widget::{button as iced_button, container, Column, Container, Row, Space},
    Alignment, Background, Length,
};

use crate::app::breez_liquid::assets::{format_usdt_display, AssetKind};
use crate::app::menu::Menu;
use crate::app::state::liquid::send::{LiquidSendFlowState, Modal, ReceiveNetwork, SendAsset};
use crate::app::view::{
    self, vault::fiat::FiatAmount, FiatAmountConverter, LiquidSendMessage, Message,
};
use crate::app::wallets::{DomainPaymentDetails, DomainPaymentStatus};
use crate::{app::cache::Cache, loading::loading_indicator};
use coincube_ui::image::asset_network_logo;

pub struct LiquidSendFlowConfig<'a> {
    pub flow_state: &'a LiquidSendFlowState,
    pub btc_balance: Amount,
    pub usdt_balance: u64,
    pub fiat_converter: Option<FiatAmountConverter>,
    pub recent_transaction: &'a Vec<RecentTransaction>,
    pub input: &'a form::Value<String>,
    pub amount_input: &'a form::Value<String>,
    pub usdt_amount_input: &'a form::Value<String>,
    pub to_asset: SendAsset,
    pub from_asset: SendAsset,
    pub receive_network: ReceiveNetwork,
    pub send_picker_open: bool,
    pub receive_picker_open: bool,
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
    pub error: Option<&'a str>,
    pub cross_asset_supported: bool,
    pub pay_fees_with_asset: bool,
    pub max_loading: bool,
    pub sent_celebration_context: &'a str,
    pub sent_amount_display: &'a str,
    pub sent_quote: &'a coincube_ui::component::quote_display::Quote,
    pub sent_image_handle: &'a iced::widget::image::Handle,
}

pub fn liquid_send_with_flow<'a>(config: LiquidSendFlowConfig<'a>) -> Element<'a, Message> {
    let base_content = match config.flow_state {
        LiquidSendFlowState::Main { modal } => {
            let send_view = liquid_send_view(
                config.btc_balance,
                config.usdt_balance,
                config.from_asset,
                config.to_asset,
                config.receive_network,
                config.fiat_converter,
                config.recent_transaction,
                config.input,
                config.input_type,
                config.bitcoin_unit,
                config.usdt_asset_id,
                config.cache.show_direction_badges,
            )
            .map(Message::LiquidSend);

            let content = view::dashboard(config.menu, config.cache, send_view);

            // Show picker modals if open
            if config.send_picker_open {
                let modal_content = send_picker_modal(
                    config.btc_balance,
                    config.usdt_balance,
                    config.from_asset,
                    config.bitcoin_unit,
                )
                .map(Message::LiquidSend);
                return coincube_ui::widget::modal::Modal::new(content, modal_content)
                    .on_blur(Some(Message::LiquidSend(LiquidSendMessage::ClosePicker)))
                    .into();
            }

            if config.receive_picker_open {
                let modal_content = receive_picker_modal(
                    config.from_asset,
                    config.to_asset,
                    config.receive_network,
                    config.cross_asset_supported,
                )
                .map(Message::LiquidSend);
                return coincube_ui::widget::modal::Modal::new(content, modal_content)
                    .on_blur(Some(Message::LiquidSend(LiquidSendMessage::ClosePicker)))
                    .into();
            }

            // Show amount modal if needed
            match modal {
                Modal::AmountInput => {
                    let modal_content = amount_input_model(AmountInputConfig {
                        amount: config.amount_input,
                        usdt_amount_input: config.usdt_amount_input,
                        to_asset: config.to_asset,
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
                        error: config.error,
                        cross_asset_supported: config.cross_asset_supported,
                        pay_fees_with_asset: config.pay_fees_with_asset,
                        max_loading: config.max_loading,
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
                config.to_asset,
                config.usdt_amount_input.value.trim(),
                config.from_asset,
            )
            .map(Message::LiquidSend);
            view::dashboard(config.menu, config.cache, content)
        }
        LiquidSendFlowState::Sent => {
            let content = coincube_ui::component::sent_celebration_page(
                config.sent_celebration_context,
                config.sent_amount_display,
                config.sent_quote,
                config.sent_image_handle,
                LiquidSendMessage::BackToHome,
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
    from_asset: SendAsset,
    to_asset: SendAsset,
    receive_network: ReceiveNetwork,
    _fiat_converter: Option<FiatAmountConverter>,
    recent_transaction: &[RecentTransaction],
    input: &'a form::Value<String>,
    input_type: &'a Option<InputType>,
    bitcoin_unit: BitcoinDisplayUnit,
    usdt_asset_id: &str,
    show_direction_badges: bool,
) -> Element<'a, LiquidSendMessage> {
    let mut content = Column::new().spacing(20);

    // ── Two-card "You Send → They Receive" layout ───────────────────────────
    let you_send_card = {
        let (asset_label, balance_text) = match from_asset {
            SendAsset::Lbtc => (
                "L-BTC",
                format!(
                    "Balance: {}",
                    btc_balance.to_formatted_string_with_unit(bitcoin_unit)
                ),
            ),
            SendAsset::Usdt => (
                "USDt",
                format!("Balance: {} USDt", format_usdt_display(usdt_balance)),
            ),
        };
        let asset_slug = match from_asset {
            SendAsset::Lbtc => "lbtc",
            SendAsset::Usdt => "usdt",
        };
        let ico: Element<'_, LiquidSendMessage> = asset_network_logo(asset_slug, "liquid", 40.0);

        let card_content = Column::new()
            .spacing(6)
            .push(text("YOU SEND").size(P2_SIZE).style(theme::text::secondary))
            .push(
                Row::new()
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .push(ico)
                    .push(
                        text(asset_label)
                            .size(H3_SIZE)
                            .bold()
                            .style(theme::text::primary),
                    ),
            )
            .push(
                text(balance_text)
                    .size(P2_SIZE)
                    .style(theme::text::secondary),
            )
            .push(
                Container::new(text("LIQUID").size(11).color(color::ORANGE))
                    .padding([2, 8])
                    .style(|_: &theme::Theme| container::Style {
                        border: iced::Border {
                            color: color::ORANGE,
                            width: 1.0,
                            radius: 8.0.into(),
                        },
                        ..Default::default()
                    }),
            );

        iced_button(
            Container::new(card_content)
                .padding(16)
                .width(Length::Fill)
                .height(Length::Fixed(160.0))
                .style(theme::card::simple),
        )
        .on_press(LiquidSendMessage::OpenSendPicker)
        .style(|_: &theme::Theme, status| iced_button::Style {
            background: Some(Background::Color(color::TRANSPARENT)),
            border: iced::Border {
                color: if matches!(status, iced_button::Status::Hovered) {
                    color::ORANGE
                } else {
                    color::TRANSPARENT
                },
                width: 1.0,
                radius: 16.0.into(),
            },
            ..Default::default()
        })
    };

    let they_receive_card = {
        let asset_label = match (to_asset, receive_network) {
            (SendAsset::Lbtc, ReceiveNetwork::Lightning) => "BTC",
            (SendAsset::Lbtc, ReceiveNetwork::Bitcoin) => "BTC",
            (SendAsset::Lbtc, _) => "L-BTC",
            (SendAsset::Usdt, _) => "USDt",
        };
        let asset_slug = match to_asset {
            SendAsset::Lbtc => "btc",
            SendAsset::Usdt => "usdt",
        };
        let network_slug = match receive_network {
            ReceiveNetwork::Lightning => "lightning",
            ReceiveNetwork::Liquid => "liquid",
            ReceiveNetwork::Bitcoin => "bitcoin",
            ReceiveNetwork::Ethereum => "ethereum",
            ReceiveNetwork::Tron => "tron",
            ReceiveNetwork::Binance => "bsc",
            ReceiveNetwork::Solana => "solana",
        };
        let ico: Element<'_, LiquidSendMessage> =
            asset_network_logo(asset_slug, network_slug, 40.0);

        let network_label = receive_network.display_name();

        let card_content = Column::new()
            .spacing(6)
            .push(
                text("THEY RECEIVE")
                    .size(P2_SIZE)
                    .style(theme::text::secondary),
            )
            .push(
                Row::new()
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .push(ico)
                    .push(
                        text(asset_label)
                            .size(H3_SIZE)
                            .bold()
                            .style(theme::text::primary),
                    ),
            )
            .push(
                Container::new(
                    text(network_label.to_uppercase())
                        .size(11)
                        .color(color::ORANGE),
                )
                .padding([2, 8])
                .style(|_: &theme::Theme| container::Style {
                    border: iced::Border {
                        color: color::ORANGE,
                        width: 1.0,
                        radius: 8.0.into(),
                    },
                    ..Default::default()
                }),
            );

        iced_button(
            Container::new(card_content)
                .padding(16)
                .width(Length::Fill)
                .height(Length::Fixed(160.0))
                .style(theme::card::simple),
        )
        .on_press(LiquidSendMessage::OpenReceivePicker)
        .style(|_: &theme::Theme, status| iced_button::Style {
            background: Some(Background::Color(color::TRANSPARENT)),
            border: iced::Border {
                color: if matches!(status, iced_button::Status::Hovered) {
                    color::ORANGE
                } else {
                    color::TRANSPARENT
                },
                width: 1.0,
                radius: 16.0.into(),
            },
            ..Default::default()
        })
    };

    let arrow = text("→").size(H3_SIZE).style(theme::text::secondary);

    let cards_row = Row::new()
        .spacing(12)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .push(Container::new(you_send_card).width(Length::FillPortion(1)))
        .push(arrow)
        .push(Container::new(they_receive_card).width(Length::FillPortion(1)));

    content = content.push(cards_row);

    // ── Address input ────────────────────────────────────────────────────────
    let hint_text = match (to_asset, receive_network) {
        (_, ReceiveNetwork::Lightning) => "Enter Lightning Invoice or Address",
        (_, ReceiveNetwork::Bitcoin) => "Enter Bitcoin Address",
        (_, ReceiveNetwork::Liquid) if to_asset == SendAsset::Usdt => "Enter Liquid USDt Address",
        (_, ReceiveNetwork::Liquid) => "Enter Liquid Address",
        (_, net) if net.is_sideshift() => match net {
            ReceiveNetwork::Ethereum => "Enter Ethereum USDt Address",
            ReceiveNetwork::Tron => "Enter Tron USDt Address",
            ReceiveNetwork::Binance => "Enter Binance Smart Chain USDt Address",
            ReceiveNetwork::Solana => "Enter Solana USDt Address",
            _ => "Enter Address",
        },
        _ => "Enter Address",
    };

    // For SideShift sends, don't require Breez input validation — accept any text
    let can_proceed = if receive_network.is_sideshift() {
        !input.value.trim().is_empty()
    } else {
        input.valid && !input.value.trim().is_empty() && input_type.is_some()
    };

    let input_section = Column::new()
        .spacing(12)
        .width(Length::Fill)
        .push(
            text("RECEIVING ADDRESS")
                .size(P2_SIZE)
                .style(theme::text::secondary),
        )
        .push(
            Row::new()
                .spacing(10)
                .align_y(Alignment::Center)
                .push(
                    form::Form::new(hint_text, input, LiquidSendMessage::InputEdited)
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
                        .on_press_maybe(if can_proceed {
                            Some(LiquidSendMessage::Send)
                        } else {
                            None
                        })
                        .width(Length::Fixed(50.0))
                        .height(Length::Fixed(50.0))
                        .style(theme::button::primary),
                    )
                    .width(Length::Fixed(50.0))
                    .height(Length::Fixed(50.0)),
                ),
        );

    content = content.push(input_section);

    // ── Last transactions ────────────────────────────────────────────────────
    content = content.push(Column::new().spacing(10).push(h4_bold("Last transactions")));

    if !recent_transaction.is_empty() {
        for (idx, tx) in recent_transaction.iter().enumerate() {
            let direction = if tx.is_incoming {
                TransactionDirection::Incoming
            } else {
                TransactionDirection::Outgoing
            };

            let fiat_str = if tx.usdt_display.is_some() {
                None
            } else {
                tx.fiat_amount
                    .as_ref()
                    .map(|fiat| format!("~{} {}", fiat.to_rounded_string(), fiat.currency()))
            };

            let display_amount = if tx.usdt_display.is_some() {
                Amount::ZERO
            } else if tx.is_incoming {
                tx.amount
            } else {
                tx.amount + tx.fees_sat
            };

            // Determine combo icon from payment details
            let tx_icon = match &tx.details {
                DomainPaymentDetails::Lightning { .. } => {
                    asset_network_logo("btc", "lightning", 40.0)
                }
                DomainPaymentDetails::LiquidAsset { asset_id, .. }
                    if !usdt_asset_id.is_empty() && asset_id == usdt_asset_id =>
                {
                    asset_network_logo("usdt", "liquid", 40.0)
                }
                DomainPaymentDetails::LiquidAsset { .. } => {
                    asset_network_logo("lbtc", "liquid", 40.0)
                }
                DomainPaymentDetails::OnChainBitcoin { .. } => {
                    asset_network_logo("btc", "bitcoin", 40.0)
                }
            };

            let mut item = TransactionListItem::new(direction, &display_amount, bitcoin_unit)
                .with_custom_icon(tx_icon)
                .with_show_direction_badge(show_direction_badges)
                .with_label(tx.description.clone())
                .with_time_ago(tx.time_ago.clone());

            if let Some(ref usdt_str) = tx.usdt_display {
                item = item.with_amount_override(usdt_str.clone());
            }

            if matches!(tx.status, DomainPaymentStatus::Pending) {
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
        let the_icon = icon::history_icon()
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
            .push(the_icon)
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
            )
            .push(iced::widget::Space::new().height(Length::Fixed(40.0)));
    }

    content.into()
}

// ── Picker modal views ──────────────────────────────────────────────────────

/// Render the "You Send" picker modal content.
pub fn send_picker_modal<'a>(
    btc_balance: Amount,
    usdt_balance: u64,
    current: SendAsset,
    bitcoin_unit: BitcoinDisplayUnit,
) -> Element<'a, LiquidSendMessage> {
    let title = text("YOU SEND").size(H4_SIZE).bold();

    let lbtc_row = picker_row(
        asset_network_logo("lbtc", "liquid", 36.0),
        "L-BTC",
        &btc_balance.to_formatted_string_with_unit(bitcoin_unit),
        "Liquid",
        current == SendAsset::Lbtc,
        LiquidSendMessage::SetSendAsset(SendAsset::Lbtc),
    );

    let usdt_row = picker_row(
        asset_network_logo("usdt", "liquid", 36.0),
        "USDt",
        &format!("{} USDt", format_usdt_display(usdt_balance)),
        "Liquid",
        current == SendAsset::Usdt,
        LiquidSendMessage::SetSendAsset(SendAsset::Usdt),
    );

    Column::new()
        .spacing(16)
        .padding(24)
        .max_width(420)
        .push(title)
        .push(lbtc_row)
        .push(usdt_row)
        .into()
}

/// Render the "They Receive" picker modal content.
pub fn receive_picker_modal<'a>(
    from_asset: SendAsset,
    current_to: SendAsset,
    current_network: ReceiveNetwork,
    cross_asset_supported: bool,
) -> Element<'a, LiquidSendMessage> {
    let title = text("THEY RECEIVE").size(H4_SIZE).bold();

    let options = ReceiveNetwork::options_for_send_asset(from_asset, cross_asset_supported);

    let mut col = Column::new()
        .spacing(8)
        .padding(24)
        .max_width(420)
        .push(title);

    for (asset, network) in options {
        let is_selected = asset == current_to && network == current_network;
        let (asset_slug, label, network_slug, network_label) = match (asset, network) {
            (SendAsset::Lbtc, ReceiveNetwork::Lightning) => {
                ("btc", "BTC", "lightning", "Lightning")
            }
            (SendAsset::Lbtc, ReceiveNetwork::Liquid) => ("lbtc", "L-BTC", "liquid", "Liquid"),
            (SendAsset::Lbtc, ReceiveNetwork::Bitcoin) => ("btc", "BTC", "bitcoin", "Bitcoin"),
            (SendAsset::Usdt, ReceiveNetwork::Liquid) => ("usdt", "USDt", "liquid", "Liquid"),
            (SendAsset::Usdt, ReceiveNetwork::Ethereum) => ("usdt", "USDt", "ethereum", "Ethereum"),
            (SendAsset::Usdt, ReceiveNetwork::Tron) => ("usdt", "USDt", "tron", "Tron"),
            (SendAsset::Usdt, ReceiveNetwork::Binance) => ("usdt", "USDt", "bsc", "Binance"),
            (SendAsset::Usdt, ReceiveNetwork::Solana) => ("usdt", "USDt", "solana", "Solana"),
            _ => continue,
        };
        let ico: Element<'_, LiquidSendMessage> =
            asset_network_logo(asset_slug, network_slug, 36.0);

        col = col.push(picker_row(
            ico,
            label,
            "",
            network_label,
            is_selected,
            LiquidSendMessage::SetReceiveTarget(asset, network),
        ));
    }

    col.into()
}

/// A single row in a picker modal.
fn picker_row<'a>(
    ico: impl Into<Element<'a, LiquidSendMessage>>,
    label: &str,
    balance: &str,
    network: &str,
    is_selected: bool,
    on_press: LiquidSendMessage,
) -> Element<'a, LiquidSendMessage> {
    let mut row = Row::new()
        .spacing(12)
        .align_y(Alignment::Center)
        .push(ico)
        .push(
            Column::new()
                .spacing(2)
                .push(
                    text(label.to_string())
                        .size(P1_SIZE)
                        .bold()
                        .style(theme::text::primary),
                )
                .push_maybe(if !balance.is_empty() {
                    Some(
                        text(balance.to_string())
                            .size(P2_SIZE)
                            .style(theme::text::secondary),
                    )
                } else {
                    None
                }),
        )
        .push(
            Container::new(text(network.to_uppercase()).size(10).color(color::ORANGE))
                .padding([2, 6])
                .style(|_: &theme::Theme| container::Style {
                    border: iced::Border {
                        color: color::ORANGE,
                        width: 1.0,
                        radius: 6.0.into(),
                    },
                    ..Default::default()
                }),
        )
        .push(Space::new().width(Length::Fill));

    if is_selected {
        row = row.push(icon::check2_icon().size(18).color(color::ORANGE));
    }

    iced_button(
        Container::new(row)
            .padding([12, 16])
            .width(Length::Fill)
            .style(if is_selected {
                picker_row_selected
            } else {
                theme::card::simple
            }),
    )
    .on_press(on_press)
    .style(|_: &theme::Theme, _| iced_button::Style {
        background: Some(Background::Color(color::TRANSPARENT)),
        border: iced::Border {
            radius: 12.0.into(),
            ..Default::default()
        },
        ..Default::default()
    })
    .width(Length::Fill)
    .into()
}

/// Selected row in picker modals — orange border with subtle tinted background.
fn picker_row_selected(theme: &theme::Theme) -> container::Style {
    let bg = match theme.mode {
        coincube_ui::theme::palette::ThemeMode::Dark => iced::color!(0x1a1a10),
        coincube_ui::theme::palette::ThemeMode::Light => iced::color!(0xFFF5E6),
    };
    container::Style {
        background: Some(Background::Color(bg)),
        border: iced::Border {
            color: color::ORANGE,
            width: 1.0,
            radius: 12.0.into(),
        },
        ..Default::default()
    }
}

pub struct RecentTransaction {
    pub description: String,
    pub time_ago: String,
    pub amount: Amount,
    pub fees_sat: Amount,
    pub fiat_amount: Option<FiatAmount>,
    pub is_incoming: bool,
    pub status: DomainPaymentStatus,
    pub details: DomainPaymentDetails,
    /// When set, the transaction displays this string instead of the BTC amount (e.g. "5.00 USDt").
    pub usdt_display: Option<String>,
}

pub struct AmountInputConfig<'a> {
    pub amount: &'a form::Value<String>,
    pub usdt_amount_input: &'a form::Value<String>,
    pub to_asset: SendAsset,
    pub from_asset: SendAsset,
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
    pub error: Option<&'a str>,
    pub cross_asset_supported: bool,
    pub pay_fees_with_asset: bool,
    pub max_loading: bool,
}

/// "Max" button that shows an animated "." → ".." → "..." while the fee probe is in flight.
fn max_button(
    loading: bool,
) -> iced::Element<'static, LiquidSendMessage, coincube_ui::theme::Theme> {
    use coincube_ui::component::spinner::typing_text_carousel;
    use std::time::Duration;

    let label: iced::Element<'static, LiquidSendMessage, coincube_ui::theme::Theme> = if loading {
        typing_text_carousel("...", true, Duration::from_millis(300), |s| {
            text(s).size(13).color(color::ORANGE)
        })
        .into()
    } else {
        text("Max").size(13).color(color::ORANGE).into()
    };
    iced_button(
        Container::new(label)
            .padding([4, 12])
            .center_x(Length::Shrink),
    )
    .on_press_maybe(if loading {
        None
    } else {
        Some(LiquidSendMessage::PopupMessage(
            view::SendPopupMessage::SendMax,
        ))
    })
    .style(|_, _| iced::widget::button::Style {
        background: Some(Background::Color(iced::Color::TRANSPARENT)),
        text_color: color::ORANGE,
        border: iced::Border {
            color: color::ORANGE,
            width: 1.0,
            radius: 15.0.into(),
        },
        ..Default::default()
    })
    .into()
}

pub fn amount_input_model<'a>(config: AmountInputConfig<'a>) -> Element<'a, LiquidSendMessage> {
    let mut content = Column::new()
        .spacing(20)
        .padding(30)
        .width(Length::Fixed(500.0))
        .align_x(Alignment::Center);

    // Show inline error banner if present
    if let Some(err) = config.error {
        content = content.push(
            container(p1_regular(err).color(color::WHITE))
                .width(Length::Fill)
                .padding(12)
                .style(|_| {
                    iced::widget::container::Style::default()
                        .background(iced::Color::from_rgb(0.3, 0.05, 0.05))
                        .border(
                            iced::Border::default()
                                .width(1)
                                .color(color::RED)
                                .rounded(8),
                        )
                }),
        );
    }

    // Show balance of the asset being paid from
    let paying_asset = config.from_asset;
    let balance_text = match paying_asset {
        SendAsset::Usdt => format!("{} USDt", format_usdt_display(config.usdt_balance)),
        SendAsset::Lbtc => format!(
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

    // Cross-asset swap indicator and toggle (SideSwap, mainnet only)
    if config.cross_asset_supported
        && (config.uri_asset.is_some() || config.from_asset != config.to_asset)
    {
        let is_cross_asset = config.from_asset != config.to_asset;
        let toggle_label = if is_cross_asset {
            let paying_with = match config.from_asset {
                SendAsset::Lbtc => "L-BTC",
                SendAsset::Usdt => "USDt",
            };
            let receiving = match config.to_asset {
                SendAsset::Lbtc => "L-BTC",
                SendAsset::Usdt => "USDt",
            };
            format!(
                "Paying with {} → Receiver gets {} (swap)",
                paying_with, receiving
            )
        } else {
            let asset_name = match config.to_asset {
                SendAsset::Lbtc => "L-BTC",
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
    match config.to_asset {
        SendAsset::Usdt => {
            let mut usdt_col = Column::new()
                .spacing(5)
                .push(
                    Row::new()
                        .push(text("Amount (USDt)").size(16))
                        .push(Space::new().width(Length::Fill))
                        .push(max_button(config.max_loading))
                        .align_y(Alignment::Center),
                )
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

            // Fee asset toggle (USDt vs L-BTC) — only for same-asset sends
            if config.from_asset == config.to_asset {
                let fee_label = if config.pay_fees_with_asset {
                    "Pay fees with USDt"
                } else {
                    "Pay fees with L-BTC"
                };
                let fee_toggle = iced_button(
                    Container::new(
                        Row::new()
                            .spacing(6)
                            .align_y(Alignment::Center)
                            .push(icon::left_right_icon().size(14).style(|_| {
                                iced::widget::text::Style {
                                    color: Some(color::ORANGE),
                                }
                            }))
                            .push(text(fee_label).size(13).color(color::ORANGE)),
                    )
                    .padding([4, 12]),
                )
                .on_press(LiquidSendMessage::PopupMessage(
                    view::SendPopupMessage::ToggleFeeAsset,
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

                usdt_col = usdt_col.push(
                    Container::new(fee_toggle)
                        .width(Length::Fill)
                        .center_x(Length::Fill),
                );
            }

            content = content.push(usdt_col);
        }
        SendAsset::Lbtc => {
            let mut amount_label_section = Column::new().spacing(2);

            let max_btn = max_button(config.max_loading);

            let amount_row = Row::new()
                .spacing(10)
                .push(text(format!("Amount ({})", config.bitcoin_unit)).size(16))
                .push(iced::widget::Space::new().width(Length::Fill))
                .align_y(Alignment::Center);

            let amount_row = if config.has_fiat_converter {
                amount_row
                    .push(
                        button::transparent(None, "⇄")
                            .on_press(LiquidSendMessage::PopupMessage(
                                view::SendPopupMessage::FiatConvert,
                            ))
                            .width(Length::Shrink),
                    )
                    .push(max_btn)
            } else {
                amount_row.push(max_btn)
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
    let paying_asset = config.from_asset;
    let has_balance = match paying_asset {
        SendAsset::Usdt => config.usdt_balance > 0,
        SendAsset::Lbtc => config.btc_balance.to_sat() > 0,
    };

    let is_next_enabled = has_balance
        && match config.to_asset {
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
    to_asset: SendAsset,
    usdt_send_amount: &'a str,
    from_asset: SendAsset,
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
    if from_asset != to_asset {
        let paying_with = match from_asset {
            SendAsset::Lbtc => "L-BTC",
            SendAsset::Usdt => "USDt",
        };
        let receiving = match to_asset {
            SendAsset::Lbtc => "L-BTC",
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

    content = content.push(Space::new().height(Length::Fixed(2.0)));

    // Determine fee display: for USDt sends, the SDK may pay fees in USDt (asset fees)
    // or in L-BTC. Check `estimated_asset_fees` first — if set, fees are paid in USDt.
    let usdt_asset_fees: Option<f64> = if to_asset == SendAsset::Usdt {
        prepare_response.and_then(|p| p.estimated_asset_fees)
    } else {
        None
    };

    let fees_sat = if usdt_asset_fees.is_some() {
        // Fees paid in USDt — no L-BTC fee to display
        0
    } else if to_asset == SendAsset::Usdt {
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

    if to_asset == SendAsset::Usdt {
        // USDt send: show USDt amount prominently, fees separately
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

        if let Some(asset_fee) = usdt_asset_fees {
            // Fees paid in USDt — convert f64 to base units for consistent formatting
            let fee_base = (asset_fee
                * 10_u64.pow(crate::app::breez_liquid::assets::USDT_PRECISION as u32) as f64)
                .ceil() as u64;
            details_box = details_box.push(
                Row::new()
                    .push(text("Fees (USDt):").size(16))
                    .push(Space::new().width(Length::Fill))
                    .push(
                        text(format!(
                            "{} USDt",
                            crate::app::breez_liquid::assets::format_usdt_display(fee_base)
                        ))
                        .size(16)
                        .bold(),
                    )
                    .width(Length::Fill)
                    .align_y(Alignment::Center),
            );
        } else {
            // Fees paid in L-BTC
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
        }

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
        .style(|t| container::Style {
            background: Some(iced::Background::Color(t.colors.cards.simple.background)),
            border: iced::Border {
                radius: 20.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}

// NOTE: The old send_asset_toggle function has been removed.
// Asset selection now uses the two-card "You Send / They Receive" layout
// with picker modals (send_picker_modal / receive_picker_modal above).
//
// The following dead code is the remainder of the old toggle — delete after
// confirming the new layout works.
#[allow(dead_code)]
fn _old_send_asset_toggle(_current_asset: SendAsset) -> Element<'static, LiquidSendMessage> {
    let lbtc_active = _current_asset == SendAsset::Lbtc;
    let usdt_active = _current_asset == SendAsset::Usdt;

    let lbtc_button = {
        let ico = icon::bitcoin_icon()
            .size(18)
            .style(move |_theme: &theme::Theme| iced::widget::text::Style {
                color: Some(if lbtc_active {
                    color::ORANGE
                } else {
                    color::GREY_2
                }),
            });

        let label =
            text("L-BTC")
                .size(16)
                .style(move |_theme: &theme::Theme| iced::widget::text::Style {
                    color: Some(if lbtc_active {
                        color::ORANGE
                    } else {
                        color::GREY_2
                    }),
                });

        let button_content = Container::new(
            Row::new()
                .spacing(8)
                .align_y(iced::alignment::Vertical::Center)
                .push(ico)
                .push(label),
        )
        .width(Length::Fill)
        .align_x(iced::alignment::Horizontal::Center);

        Container::new(
            iced_button(Container::new(button_content).padding([10, 30]))
                .style(move |_theme: &theme::Theme, _status| iced_button::Style {
                    background: Some(Background::Color(iced::Color::TRANSPARENT)),
                    text_color: if lbtc_active {
                        color::WHITE
                    } else {
                        color::GREY_2
                    },
                    border: iced::Border {
                        radius: 50.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .on_press(LiquidSendMessage::PresetAsset(SendAsset::Lbtc)),
        )
        .style(move |_theme: &theme::Theme| container::Style {
            background: Some(Background::Color(if lbtc_active {
                iced::color!(0x161716)
            } else {
                color::TRANSPARENT
            })),
            border: iced::Border {
                radius: 50.0.into(),
                color: if lbtc_active {
                    color::ORANGE
                } else {
                    color::TRANSPARENT
                },
                width: if lbtc_active { 0.7 } else { 0.0 },
            },
            ..Default::default()
        })
    };

    let usdt_button = {
        let ico = icon::usd_icon()
            .size(18)
            .style(move |_theme: &theme::Theme| iced::widget::text::Style {
                color: Some(if usdt_active {
                    color::ORANGE
                } else {
                    color::GREY_2
                }),
            });

        let label =
            text("USDt")
                .size(16)
                .style(move |_theme: &theme::Theme| iced::widget::text::Style {
                    color: Some(if usdt_active {
                        color::ORANGE
                    } else {
                        color::GREY_2
                    }),
                });

        let button_content = Container::new(
            Row::new()
                .spacing(8)
                .align_y(iced::alignment::Vertical::Center)
                .push(ico)
                .push(label),
        )
        .width(Length::Fill)
        .align_x(iced::alignment::Horizontal::Center);

        Container::new(
            iced_button(Container::new(button_content).padding([10, 30]))
                .style(move |_theme: &theme::Theme, _status| iced_button::Style {
                    background: Some(Background::Color(iced::Color::TRANSPARENT)),
                    text_color: if usdt_active {
                        color::WHITE
                    } else {
                        color::GREY_2
                    },
                    border: iced::Border {
                        radius: 50.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .on_press(LiquidSendMessage::PresetAsset(SendAsset::Usdt)),
        )
        .style(move |_theme: &theme::Theme| container::Style {
            background: Some(Background::Color(if usdt_active {
                iced::color!(0x161716)
            } else {
                color::TRANSPARENT
            })),
            border: iced::Border {
                radius: 50.0.into(),
                color: if usdt_active {
                    color::ORANGE
                } else {
                    color::TRANSPARENT
                },
                width: if usdt_active { 0.7 } else { 0.0 },
            },
            ..Default::default()
        })
    };

    Container::new(Row::new().push(lbtc_button).push(usdt_button))
        .padding(4)
        .max_width(800.0)
        .style(|_theme: &theme::Theme| container::Style {
            background: Some(Background::Color(iced::color!(0x202020))),
            border: iced::Border {
                color: iced::color!(0x202020),
                radius: 50.0.into(),
                width: 2.0,
            },
            ..Default::default()
        })
        .into()
}
