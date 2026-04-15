use coincube_core::miniscript::bitcoin::Amount;

use crate::app::wallets::{DomainPaymentDetails, DomainPaymentStatus};
use coincube_ui::{
    color,
    component::{
        amount::*,
        button,
        text::*,
        transaction::{TransactionDirection, TransactionListItem},
    },
    icon::{self, receipt_icon},
    theme,
    widget::*,
};
use iced::{
    widget::{button as iced_button, container, Column, Container, Row},
    Alignment, Background, Length,
};

use crate::app::breez_liquid::assets::format_usdt_display;
use crate::app::view::{liquid::RecentTransaction, FiatAmountConverter, LiquidOverviewMessage};

#[allow(clippy::too_many_arguments)]
pub fn liquid_overview_view<'a>(
    btc_balance: Amount,
    usdt_balance: u64,
    fiat_converter: Option<FiatAmountConverter>,
    recent_transaction: &[RecentTransaction],
    error: Option<&'a str>,
    bitcoin_unit: BitcoinDisplayUnit,
    btc_usd_price: Option<f64>,
    show_direction_badges: bool,
) -> Element<'a, LiquidOverviewMessage> {
    let mut content = Column::new().spacing(20);

    let btc_fiat = fiat_converter.as_ref().map(|c| c.convert(btc_balance));

    // Only sum BTC pending amounts; USDt transactions have usdt_display set
    // and their amount field holds USDt base units, not BTC sats.
    let pending_outgoing_sats: u64 = recent_transaction
        .iter()
        .filter(|t| {
            !t.is_incoming && t.usdt_display.is_none() && matches!(t.status, DomainPaymentStatus::Pending)
        })
        .map(|t| (t.amount + t.fees_sat).to_sat())
        .sum();

    let pending_incoming_sats: u64 = recent_transaction
        .iter()
        .filter(|t| {
            t.is_incoming && t.usdt_display.is_none() && matches!(t.status, DomainPaymentStatus::Pending)
        })
        .map(|t| t.amount.to_sat())
        .sum();

    // ── Unified portfolio card ─────────────────────────────────────────────
    // USDt is pegged to USD, so always use BTC/USD price for conversion.
    let usdt_fiat_value = usdt_balance as f64 / 1e8; // USDt base units → dollars
    let usdt_as_sats = if let Some(btc_price_usd) = btc_usd_price {
        if btc_price_usd > 0.0 {
            (usdt_fiat_value / btc_price_usd * 1e8) as u64
        } else {
            0
        }
    } else {
        0
    };
    let total_sats = btc_balance.to_sat() + usdt_as_sats;
    let total_balance = Amount::from_sat(total_sats);
    let total_fiat = fiat_converter.as_ref().map(|c| c.convert(total_balance));

    // Total balance header
    let mut total_col =
        Column::new()
            .spacing(4)
            .push(h4_bold("Balance"))
            .push(amount_with_size_and_unit(
                &total_balance,
                H2_SIZE,
                bitcoin_unit,
            ));
    if let Some(fiat) = total_fiat {
        total_col = total_col.push(
            text(format!("~{} {}", fiat.to_rounded_string(), fiat.currency()))
                .size(P1_SIZE)
                .style(theme::text::secondary),
        );
    }
    // Pending indicators
    if pending_outgoing_sats > 0 {
        total_col = total_col.push(
            Row::new()
                .spacing(6)
                .align_y(Alignment::Center)
                .push(icon::warning_icon().size(12).style(theme::text::secondary))
                .push(text("-").size(P2_SIZE).style(theme::text::secondary))
                .push(amount_with_size_and_unit(
                    &Amount::from_sat(pending_outgoing_sats),
                    P2_SIZE,
                    bitcoin_unit,
                ))
                .push(text("pending").size(P2_SIZE).style(theme::text::secondary)),
        );
    }
    if pending_incoming_sats > 0 {
        total_col = total_col.push(
            Row::new()
                .spacing(6)
                .align_y(Alignment::Center)
                .push(icon::warning_icon().size(12).style(theme::text::secondary))
                .push(text("+").size(P2_SIZE).style(theme::text::secondary))
                .push(amount_with_size_and_unit(
                    &Amount::from_sat(pending_incoming_sats),
                    P2_SIZE,
                    bitcoin_unit,
                ))
                .push(text("pending").size(P2_SIZE).style(theme::text::secondary)),
        );
    }

    // L-BTC asset row
    let lbtc_fiat_str = btc_fiat
        .as_ref()
        .map(|f| format!("~{} {}", f.to_rounded_string(), f.currency()))
        .unwrap_or_default();
    let lbtc_row = Row::new()
        .spacing(10)
        .align_y(Alignment::Center)
        .push(coincube_ui::image::asset_network_logo::<
            LiquidOverviewMessage,
        >("lbtc", "liquid", 28.0))
        .push(
            text("L-BTC")
                .size(P1_SIZE)
                .bold()
                .width(Length::Fixed(60.0)),
        )
        .push(amount_with_size_and_unit(
            &btc_balance,
            P1_SIZE,
            bitcoin_unit,
        ))
        .push(
            text(lbtc_fiat_str)
                .size(P2_SIZE)
                .style(theme::text::secondary)
                .width(Length::Fill),
        )
        .push(
            button::primary(None, "Send")
                .on_press(LiquidOverviewMessage::SendLbtc)
                .width(Length::Fixed(90.0)),
        )
        .push(
            iced_button(
                Container::new(text("Receive").size(13))
                    .padding([6, 12])
                    .center_x(Length::Fill),
            )
            .on_press(LiquidOverviewMessage::ReceiveLbtc)
            .width(Length::Fixed(90.0))
            .style(|_, _| iced::widget::button::Style {
                background: Some(Background::Color(iced::Color::TRANSPARENT)),
                text_color: color::ORANGE,
                border: iced::Border {
                    color: color::ORANGE,
                    width: 1.0,
                    radius: 25.0.into(),
                },
                ..Default::default()
            }),
        );

    // USDt asset row
    let usdt_display = format!("{} USDt", format_usdt_display(usdt_balance));
    let usdt_fiat_str = format!("~${:.2}", usdt_fiat_value);
    let usdt_row = Row::new()
        .spacing(10)
        .align_y(Alignment::Center)
        .push(coincube_ui::image::asset_network_logo::<
            LiquidOverviewMessage,
        >("usdt", "liquid", 28.0))
        .push(text("USDt").size(P1_SIZE).bold().width(Length::Fixed(60.0)))
        .push(text(usdt_display).size(P1_SIZE))
        .push(
            text(usdt_fiat_str)
                .size(P2_SIZE)
                .style(theme::text::secondary)
                .width(Length::Fill),
        )
        .push(
            button::primary(None, "Send")
                .on_press(LiquidOverviewMessage::SendUsdt)
                .width(Length::Fixed(90.0)),
        )
        .push(
            iced_button(
                Container::new(text("Receive").size(13))
                    .padding([6, 12])
                    .center_x(Length::Fill),
            )
            .on_press(LiquidOverviewMessage::ReceiveUsdt)
            .width(Length::Fixed(90.0))
            .style(|_, _| iced::widget::button::Style {
                background: Some(Background::Color(iced::Color::TRANSPARENT)),
                text_color: color::ORANGE,
                border: iced::Border {
                    color: color::ORANGE,
                    width: 1.0,
                    radius: 25.0.into(),
                },
                ..Default::default()
            }),
        );

    let portfolio_card = Container::new(
        Column::new()
            .spacing(16)
            .push(total_col)
            .push(lbtc_row)
            .push(usdt_row),
    )
    .padding(20)
    .width(Length::Fill)
    .style(|t| container::Style {
        background: Some(Background::Color(t.colors.cards.simple.background)),
        border: iced::Border {
            color: color::ORANGE,
            width: 0.2,
            radius: 25.0.into(),
        },
        ..Default::default()
    });

    content = content.push(portfolio_card);

    content = content.push(Column::new().spacing(10).push(h4_bold("Last transactions")));

    if !recent_transaction.is_empty() {
        for (idx, tx) in recent_transaction.iter().enumerate() {
            let direction = if tx.is_incoming {
                TransactionDirection::Incoming
            } else {
                TransactionDirection::Outgoing
            };

            let is_usdt = tx.usdt_display.is_some();

            let tx_icon = if is_usdt {
                coincube_ui::image::asset_network_logo("usdt", "liquid", 40.0)
            } else {
                match &tx.details {
                    DomainPaymentDetails::Lightning { .. } => {
                        coincube_ui::image::asset_network_logo("btc", "lightning", 40.0)
                    }
                    DomainPaymentDetails::LiquidAsset { .. } => {
                        coincube_ui::image::asset_network_logo("lbtc", "liquid", 40.0)
                    }
                    DomainPaymentDetails::OnChainBitcoin { .. } => {
                        coincube_ui::image::asset_network_logo("btc", "bitcoin", 40.0)
                    }
                }
            };

            let display_amount = if tx.is_incoming {
                tx.amount
            } else {
                tx.amount + tx.fees_sat
            };

            let mut item = TransactionListItem::new(direction, &display_amount, bitcoin_unit)
                .with_custom_icon(tx_icon)
                .with_show_direction_badge(show_direction_badges)
                .with_label(tx.description.clone())
                .with_time_ago(tx.time_ago.clone());

            if let Some(ref usdt_display) = tx.usdt_display {
                item = item.with_amount_override(usdt_display.clone());
            } else {
                let fiat_str = tx
                    .fiat_amount
                    .as_ref()
                    .map(|fiat| format!("~{} {}", fiat.to_rounded_string(), fiat.currency()));
                if let Some(fiat) = fiat_str {
                    item = item.with_fiat_amount(fiat);
                }
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

            content = content.push(item.view(LiquidOverviewMessage::SelectTransaction(idx)));
        }
    } else {
        content = content.push(placeholder(
            receipt_icon().size(80),
            "No transactions yet",
            "Your transaction history will appear here once you send or receive coins.",
        ));
    }

    let view_transactions_button = {
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
        .style(|_theme: &theme::Theme, _| iced_button::Style {
            background: Some(Background::Color(color::TRANSPARENT)),
            text_color: color::ORANGE,
            border: iced::Border {
                radius: 20.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .on_press(LiquidOverviewMessage::History)
    };

    if !recent_transaction.is_empty() {
        content = content
            .push(iced::widget::Space::new().height(Length::Fixed(20.0)))
            .push(
                Container::new(view_transactions_button)
                    .width(Length::Fill)
                    .center_x(Length::Fill),
            )
            .push(iced::widget::Space::new().height(Length::Fixed(40.0)));
    }

    if let Some(err) = error {
        content = content.push(
            Container::new(text(err.to_string()).size(14).color(color::RED))
                .padding(10)
                .style(theme::card::invalid)
                .width(Length::Fill)
                .max_width(800),
        );
    }

    content.into()
}

pub fn placeholder<'a, T: Into<Element<'a, LiquidOverviewMessage>>>(
    icon: T,
    title: &'a str,
    subtitle: &'a str,
) -> Element<'a, LiquidOverviewMessage> {
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
