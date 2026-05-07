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

use crate::app::settings::display::DisplayMode;
use crate::app::state::spark::overview::SparkBalanceSnapshot;
use crate::app::view::vault::fiat::FiatAmount;
use crate::app::view::wallet_header::{wallet_header, HeaderVariant, SyncState, WalletHeaderProps};
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
    /// Stable Balance conversion leg (BTC ↔ USDB). Surfaces in the
    /// transaction list whenever the SDK records the receive side of
    /// a conversion as a top-level `method=token` payment. Rendered
    /// with the "Stable" branding rather than as a bitcoin transfer
    /// — the amount is in token base units, not sats.
    StableBalance,
}

/// Row data the overview renderer needs for each recent payment.
/// Mirrors `view::liquid::RecentTransaction` plus a `token_display`
/// override that the Stable Balance USDB rows use the same way Liquid
/// uses `usdt_display` — when set, the row shows that string in place
/// of the BTC amount and skips the BTC pending-sums.
///
/// Carries the Spark payment `id` and raw `timestamp` so the detail
/// view can render a full date and a copy-to-clipboard payment ID
/// without needing to hold onto the originating `PaymentSummary`.
#[derive(Debug, Clone)]
pub struct SparkRecentTransaction {
    pub id: String,
    pub description: String,
    pub time_ago: String,
    pub timestamp: u64,
    pub amount: Amount,
    pub fees_sat: Amount,
    pub fiat_amount: Option<FiatAmount>,
    pub is_incoming: bool,
    pub status: DomainPaymentStatus,
    pub method: SparkPaymentMethod,
    /// When set, render this string instead of the BTC `amount` (e.g.
    /// "1.58 USDB"). Token-method payments populate this; bitcoin
    /// rows leave it `None`.
    pub token_display: Option<String>,
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
    /// Reference BTC/USD price used to fold USDB into the unified
    /// portfolio total at the current rate. `None` skips the fold —
    /// matches the Liquid panel's USDt behaviour.
    pub btc_usd_price: Option<f64>,
    pub display_mode: DisplayMode,
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
                snapshot.stable_balance.as_ref(),
                self.btc_usd_price,
                self.recent_transactions,
                self.fiat_converter,
                self.bitcoin_unit,
                self.show_direction_badges,
                self.stable_balance_active,
                self.display_mode,
            ),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn connected_view<'a>(
    balance_sats: u64,
    stable_balance: Option<&coincube_spark_protocol::StableBalanceSnapshot>,
    btc_usd_price: Option<f64>,
    recent: &'a [SparkRecentTransaction],
    fiat_converter: Option<FiatAmountConverter>,
    bitcoin_unit: BitcoinDisplayUnit,
    show_direction_badges: bool,
    stable_balance_active: bool,
    display_mode: DisplayMode,
) -> Element<'a, SparkOverviewMessage> {
    use crate::app::breez_spark::assets::stable_token_as_sats;

    let mut content = Column::new().spacing(20);

    // Fold USDB into the unified portfolio total at the current
    // BTC/USD price — same pattern as Liquid's USDt fold. The
    // Stable Balance feature promises the spendable balance stays
    // pegged to fiat; if we showed only `balance_sats` here, toggling
    // Stable Balance ON would look like the wallet was emptied.
    let usdb_as_sats = stable_balance
        .map(|sb| stable_token_as_sats(sb.balance, sb.decimals, btc_usd_price))
        .unwrap_or(0);
    let total_balance = Amount::from_sat(balance_sats.saturating_add(usdb_as_sats));
    let total_fiat = fiat_converter.as_ref().map(|c| c.convert(total_balance));

    // Sum pending BTC (in/out) from the recent-transactions list for
    // the small "pending" indicators underneath the total. Skip
    // token-display rows so a USDB conversion entry doesn't get
    // counted as pending sats.
    let pending_outgoing_sats: u64 = recent
        .iter()
        .filter(|t| {
            !t.is_incoming
                && t.token_display.is_none()
                && matches!(t.status, DomainPaymentStatus::Pending)
        })
        .map(|t| (t.amount + t.fees_sat).to_sat())
        .sum();
    let pending_incoming_sats: u64 = recent
        .iter()
        .filter(|t| {
            t.is_incoming
                && t.token_display.is_none()
                && matches!(t.status, DomainPaymentStatus::Pending)
        })
        .map(|t| t.amount.to_sat())
        .sum();

    // ── Unified portfolio card ─────────────────────────────────────────
    let total_col = Column::new()
        .spacing(4)
        .push(
            Row::new()
                .spacing(8)
                .align_y(Alignment::Center)
                .push(h4_bold("Balance"))
                .push_maybe(stable_balance_active.then(stable_badge)),
        )
        .push(wallet_header::<SparkOverviewMessage>(WalletHeaderProps {
            sats: total_balance,
            fiat: total_fiat,
            balance_masked: false,
            bitcoin_unit,
            variant: HeaderVariant::Overview,
            sync: SyncState::Synced,
            unconfirmed: None,
            pending_send_sats: pending_outgoing_sats,
            pending_receive_sats: pending_incoming_sats,
            display_mode,
            on_swap: Some(SparkOverviewMessage::FlipDisplayMode),
        }));

    // Stable Balance auto-sweeps the BTC balance into USDB, so the
    // raw `balance_sats` reads 0 even though the user can still send
    // bitcoin normally — the SDK converts USDB back to sats as needed
    // when sending. Surface the spendable total (= raw BTC + USDB
    // folded at the current price) so the row matches the header and
    // accurately represents what's spendable.
    let btc_row_amount = total_balance;
    let btc_row_fiat = total_fiat;

    let btc_fiat_str = btc_row_fiat
        .as_ref()
        .map(|f| format!("{} {}", f.to_rounded_string(), f.currency()))
        .unwrap_or_default();
    let btc_row = Row::new()
        .spacing(10)
        .align_y(Alignment::Center)
        .push(coincube_ui::image::asset_network_logo::<SparkOverviewMessage>("btc", "spark", 40.0))
        .push(text("BTC").size(P1_SIZE).bold().width(Length::Fixed(60.0)))
        .push(amount_with_size_and_unit(
            &btc_row_amount,
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
                SparkPaymentMethod::StableBalance => {
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

            if let Some(token_str) = tx.token_display.as_ref() {
                // Token rows: replace the BTC headline with the token
                // string. The fiat label still rides along underneath
                // because USDB is pegged to USD.
                item = item.with_amount_override(token_str.clone());
            }
            if let Some(fiat) = tx.fiat_amount.as_ref() {
                item = item.with_fiat_amount(format!(
                    "{} {}",
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
