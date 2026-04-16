//! View renderer for [`crate::app::state::spark::overview::SparkOverview`].
//!
//! Mirrors the Liquid overview layout almost exactly:
//! - Unified portfolio card with a total balance header + one asset row
//!   for BTC. No USDt (that's a Liquid-only asset) and no
//!   L-BTC branding — Spark runs on real Bitcoin.
//! - Recent transactions list using the shared `TransactionListItem`
//!   widget, so Lightning / on-chain rows look identical across wallets.
//! - A "Stable" badge on the balance when the SDK reports an active
//!   Stable Balance token.

use coincube_core::miniscript::bitcoin::Amount;

use crate::app::wallets::DomainPaymentStatus;
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

use crate::app::state::spark::overview::SparkBalanceSnapshot;
use crate::app::view::vault::fiat::FiatAmount;
use crate::app::view::{FiatAmountConverter, SparkOverviewMessage};

/// High-level status of the Spark backend for the current cube.
#[derive(Debug, Clone)]
pub enum SparkStatus {
    /// No Spark signer configured for this cube (or bridge spawn failed).
    Unavailable,
    /// First `get_info` is still in flight.
    Loading,
    /// Bridge returned a balance snapshot.
    Connected(SparkBalanceSnapshot),
    /// Bridge returned an error response.
    Error(String),
}

/// Which underlying Spark payment rail fulfilled a transaction. Drives
/// the asset icon on the overview row — Spark-native and Lightning
/// payments share the same wallet balance but should look different at
/// a glance.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SparkPaymentMethod {
    Lightning,
    OnChainBitcoin,
    Spark,
}

/// Row data the overview renderer needs for each recent payment.
/// Mirrors `view::liquid::RecentTransaction` but drops the USDt-only
/// `usdt_display` field (Spark has no USDt support).
pub struct SparkRecentTransaction {
    pub description: String,
    pub time_ago: String,
    pub amount: Amount,
    pub fees_sat: Amount,
    pub fiat_amount: Option<FiatAmount>,
    pub is_incoming: bool,
    pub status: DomainPaymentStatus,
    pub method: SparkPaymentMethod,
}

/// View wrapper for the Spark Overview panel.
pub struct SparkOverviewView<'a> {
    pub status: SparkStatus,
    pub recent_transactions: &'a [SparkRecentTransaction],
    pub fiat_converter: Option<FiatAmountConverter>,
    pub bitcoin_unit: BitcoinDisplayUnit,
    pub show_direction_badges: bool,
    /// Phase 6: `true` when the SDK reports an active Stable Balance
    /// token. Rendered as a small "Stable" badge next to the balance
    /// header.
    pub stable_balance_active: bool,
}

impl<'a> SparkOverviewView<'a> {
    pub fn render(self) -> Element<'a, SparkOverviewMessage> {
        match &self.status {
            SparkStatus::Unavailable => Column::new()
                .spacing(20)
                .push(Container::new(h2("Spark Wallet")))
                .push(p1_regular(
                    "Spark is not configured for this cube yet. Set up a Spark \
                     signer and restart the app to connect the bridge.",
                ))
                .into(),
            SparkStatus::Loading => Column::new()
                .spacing(20)
                .push(Container::new(h2("Spark Wallet")))
                .push(p1_regular("Connecting to the Spark bridge…"))
                .into(),
            SparkStatus::Error(err) => Column::new()
                .spacing(20)
                .push(Container::new(h2("Spark Wallet")))
                .push(p1_regular("Spark bridge error"))
                .push(p2_regular(err.clone()))
                .into(),
            SparkStatus::Connected(snapshot) => connected_view(
                snapshot.balance_sats,
                self.recent_transactions,
                self.fiat_converter,
                self.bitcoin_unit,
                self.show_direction_badges,
                self.stable_balance_active,
            ),
        }
    }
}

fn connected_view<'a>(
    balance_sats: u64,
    recent: &'a [SparkRecentTransaction],
    fiat_converter: Option<FiatAmountConverter>,
    bitcoin_unit: BitcoinDisplayUnit,
    show_direction_badges: bool,
    stable_balance_active: bool,
) -> Element<'a, SparkOverviewMessage> {
    let mut content = Column::new().spacing(20);

    let btc_balance = Amount::from_sat(balance_sats);
    let btc_fiat = fiat_converter.as_ref().map(|c| c.convert(btc_balance));

    // Sum pending BTC (in/out) from the recent-transactions list for
    // the small "pending" indicators underneath the total.
    let pending_outgoing_sats: u64 = recent
        .iter()
        .filter(|t| !t.is_incoming && matches!(t.status, DomainPaymentStatus::Pending))
        .map(|t| (t.amount + t.fees_sat).to_sat())
        .sum();
    let pending_incoming_sats: u64 = recent
        .iter()
        .filter(|t| t.is_incoming && matches!(t.status, DomainPaymentStatus::Pending))
        .map(|t| t.amount.to_sat())
        .sum();

    // ── Unified portfolio card ─────────────────────────────────────────
    let mut total_col = Column::new()
        .spacing(4)
        .push(
            Row::new()
                .spacing(8)
                .align_y(Alignment::Center)
                .push(h4_bold("Balance"))
                .push_maybe(stable_balance_active.then(stable_badge)),
        )
        .push(amount_with_size_and_unit(
            &btc_balance,
            H2_SIZE,
            bitcoin_unit,
        ));
    if let Some(fiat) = btc_fiat.as_ref() {
        total_col = total_col.push(
            text(format!("~{} {}", fiat.to_rounded_string(), fiat.currency()))
                .size(P1_SIZE)
                .style(theme::text::secondary),
        );
    }
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

    let btc_fiat_str = btc_fiat
        .as_ref()
        .map(|f| format!("~{} {}", f.to_rounded_string(), f.currency()))
        .unwrap_or_default();
    let btc_row = Row::new()
        .spacing(10)
        .align_y(Alignment::Center)
        .push(coincube_ui::image::asset_network_logo::<SparkOverviewMessage>("btc", "spark", 28.0))
        .push(text("BTC").size(P1_SIZE).bold().width(Length::Fixed(60.0)))
        .push(amount_with_size_and_unit(
            &btc_balance,
            P1_SIZE,
            bitcoin_unit,
        ))
        .push(
            text(btc_fiat_str)
                .size(P2_SIZE)
                .style(theme::text::secondary)
                .width(Length::Fill),
        )
        .push(
            button::primary(None, "Send")
                .on_press(SparkOverviewMessage::SendBtc)
                .width(Length::Fixed(90.0)),
        )
        .push(
            button::orange_outline(None, "Receive")
                .on_press(SparkOverviewMessage::ReceiveBtc)
                .width(Length::Fixed(90.0)),
        );

    let portfolio_card = Container::new(Column::new().spacing(16).push(total_col).push(btc_row))
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

    // ── Recent transactions ───────────────────────────────────────────
    content = content.push(Column::new().spacing(10).push(h4_bold("Last transactions")));

    if !recent.is_empty() {
        for (idx, tx) in recent.iter().enumerate() {
            let direction = if tx.is_incoming {
                TransactionDirection::Incoming
            } else {
                TransactionDirection::Outgoing
            };

            let tx_icon = match tx.method {
                SparkPaymentMethod::Lightning => {
                    coincube_ui::image::asset_network_logo("btc", "lightning", 40.0)
                }
                SparkPaymentMethod::OnChainBitcoin => {
                    coincube_ui::image::asset_network_logo("btc", "bitcoin", 40.0)
                }
                SparkPaymentMethod::Spark => {
                    coincube_ui::image::asset_network_logo("btc", "spark", 40.0)
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

            if let Some(fiat) = tx.fiat_amount.as_ref() {
                item = item.with_fiat_amount(format!(
                    "~{} {}",
                    fiat.to_rounded_string(),
                    fiat.currency()
                ));
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

            content = content.push(item.view(SparkOverviewMessage::SelectTransaction(idx)));
        }
    } else {
        content = content.push(placeholder(
            receipt_icon().size(80),
            "No transactions yet",
            "Your transaction history will appear here once you send or receive coins.",
        ));
    }

    let view_transactions_button = {
        let btn_icon = icon::history_icon()
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
            .push(btn_icon)
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
        .on_press(SparkOverviewMessage::History)
    };

    if !recent.is_empty() {
        content = content
            .push(iced::widget::Space::new().height(Length::Fixed(20.0)))
            .push(
                Container::new(view_transactions_button)
                    .width(Length::Fill)
                    .center_x(Length::Fill),
            )
            .push(iced::widget::Space::new().height(Length::Fixed(40.0)));
    }

    content.into()
}

fn stable_badge<'a>() -> Element<'a, SparkOverviewMessage> {
    Container::new(
        text("Stable")
            .size(11)
            .style(|_: &theme::Theme| iced::widget::text::Style {
                color: Some(color::ORANGE),
            }),
    )
    .padding([2, 8])
    .style(|_: &theme::Theme| container::Style {
        background: Some(Background::Color(color::TRANSPARENT)),
        border: iced::Border {
            color: color::ORANGE,
            width: 1.0,
            radius: 10.0.into(),
        },
        ..Default::default()
    })
    .into()
}

fn placeholder<'a, T: Into<Element<'a, SparkOverviewMessage>>>(
    icon: T,
    title: &'a str,
    subtitle: &'a str,
) -> Element<'a, SparkOverviewMessage> {
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
