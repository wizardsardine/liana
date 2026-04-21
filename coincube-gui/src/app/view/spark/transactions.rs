//! View renderer for [`crate::app::state::spark::transactions::SparkTransactions`].
//!
//! Mirrors [`crate::app::view::liquid::transactions::liquid_transactions_view`]
//! almost exactly:
//! - Header row with the "Transactions" title.
//! - Transaction list using the shared `TransactionListItem` widget so
//!   rows look identical to the Liquid panel.
//! - Empty state with a Kage quote card and Send/Receive shortcut
//!   buttons.
//!
//! Differences from Liquid:
//! - No asset filter tabs (Spark holds only BTC; there's nothing to
//!   toggle between).
//! - No "Refundable Transactions" section (Spark has no boltz-style
//!   swap refunds).
//! - No "Export" button in the header yet — CSV export is currently
//!   Liquid-specific and a Spark export path is a future follow-up.

use coincube_core::miniscript::bitcoin::Amount;
use coincube_spark_protocol::PaymentSummary;
use iced::widget::image;

use crate::app::wallets::DomainPaymentStatus;
use crate::export::ImportExportMessage;
use crate::utils::{format_timestamp, truncate_middle};
use coincube_ui::{
    component::{
        amount::{amount_with_size_and_unit, BitcoinDisplayUnit},
        button, card,
        quote_display::{self, Quote, QuoteDisplayProps},
        text::*,
        transaction::{TransactionDirection, TransactionListItem},
    },
    icon,
    image::asset_network_logo,
    theme,
    widget::*,
};
use iced::{widget::Space, Alignment, Length};

use crate::app::menu::{Menu, SparkSubMenu};
use crate::app::view::message::Message;
use crate::app::view::spark::{SparkPaymentMethod, SparkRecentTransaction};
use crate::app::view::FiatAmountConverter;

/// Tri-state the panel can be in while the bridge talks.
#[derive(Debug, Clone)]
pub enum SparkTransactionsStatus {
    Unavailable,
    Loading,
    Error(String),
    Loaded(Vec<PaymentSummary>),
}

/// View wrapper for the Spark Transactions panel.
pub struct SparkTransactionsView<'a> {
    pub status: SparkTransactionsStatus,
    pub recent_transactions: &'a [SparkRecentTransaction],
    pub fiat_converter: Option<FiatAmountConverter>,
    pub bitcoin_unit: BitcoinDisplayUnit,
    pub show_direction_badges: bool,
    pub empty_state_quote: &'a Quote,
    pub empty_state_image_handle: &'a image::Handle,
}

impl<'a> SparkTransactionsView<'a> {
    pub fn render(self) -> Element<'a, Message> {
        let mut content = Column::new().spacing(20).width(Length::Fill);

        content = content.push(
            Row::new()
                .push(Container::new(h3("Transactions").bold()))
                .push(Space::new().width(Length::Fill))
                .push(
                    button::secondary(Some(icon::backup_icon()), "Export")
                        .on_press(ImportExportMessage::Open.into()),
                ),
        );

        match self.status {
            SparkTransactionsStatus::Unavailable => {
                content = content.push(Column::new().spacing(10).push(p1_regular(
                    "Spark is not configured for this cube. Set up a Spark \
                             signer to see your payment history here.",
                )));
                return content.into();
            }
            SparkTransactionsStatus::Loading => {
                content = content.push(
                    Column::new()
                        .push(p1_regular("Loading payment history from the Spark bridge…")),
                );
                return content.into();
            }
            SparkTransactionsStatus::Error(err) => {
                content = content.push(
                    Column::new()
                        .spacing(10)
                        .push(p1_regular("Failed to load payment history"))
                        .push(p2_regular(err)),
                );
                return content.into();
            }
            SparkTransactionsStatus::Loaded(_) => {}
        }

        if self.recent_transactions.is_empty() {
            // Same empty-state layout as Liquid, minus the Liquid copy.
            content = content.push(
                Column::new()
                    .spacing(20)
                    .width(Length::Fill)
                    .align_x(Alignment::Center)
                    .push(Space::new().height(Length::Fixed(40.0)))
                    .push(quote_display::display(&QuoteDisplayProps::new(
                        "empty-wallet",
                        self.empty_state_quote,
                        self.empty_state_image_handle,
                    )))
                    .push(Space::new().height(Length::Fixed(10.0)))
                    .push(
                        text(
                            "Your Spark wallet is ready. Once you send or receive\nfunds, they'll show up here.",
                        )
                        .size(16)
                        .style(theme::text::secondary)
                        .wrapping(iced::widget::text::Wrapping::Word)
                        .align_x(iced::alignment::Horizontal::Center),
                    )
                    .push(
                        Row::new()
                            .spacing(15)
                            .push(
                                button::primary(None, "Send sats")
                                    .on_press(Message::Menu(Menu::Spark(SparkSubMenu::Send)))
                                    .padding(15)
                                    .width(Length::Fixed(150.0)),
                            )
                            .push(
                                button::transparent_border(None, "Receive sats")
                                    .on_press(Message::Menu(Menu::Spark(SparkSubMenu::Receive)))
                                    .padding(15)
                                    .width(Length::Fixed(150.0)),
                            ),
                    ),
            );
            return content.into();
        }

        content = content.push(self.recent_transactions.iter().enumerate().fold(
            Column::new().spacing(10),
            |col, (i, tx)| {
                col.push(transaction_row(
                    i,
                    tx,
                    self.fiat_converter,
                    self.bitcoin_unit,
                    self.show_direction_badges,
                ))
            },
        ));

        content.into()
    }
}

fn transaction_row<'a>(
    i: usize,
    tx: &'a SparkRecentTransaction,
    fiat_converter: Option<FiatAmountConverter>,
    bitcoin_unit: BitcoinDisplayUnit,
    show_direction_badges: bool,
) -> Element<'a, Message> {
    let direction = if tx.is_incoming {
        TransactionDirection::Incoming
    } else {
        TransactionDirection::Outgoing
    };

    let combo_icon: Element<'_, Message> = match tx.method {
        SparkPaymentMethod::Lightning => asset_network_logo("btc", "lightning", 40.0),
        SparkPaymentMethod::OnChainBitcoin => asset_network_logo("btc", "bitcoin", 40.0),
        SparkPaymentMethod::Spark => asset_network_logo("btc", "spark", 40.0),
    };

    // Outgoing amount includes fees so the headline figure matches what
    // actually left the wallet. Incoming shows the net credit.
    let display_amount = if tx.is_incoming {
        tx.amount
    } else {
        tx.amount + tx.fees_sat
    };

    let mut item = TransactionListItem::new(direction, &display_amount, bitcoin_unit)
        .with_label(tx.description.clone())
        .with_time_ago(tx.time_ago.clone())
        .with_custom_icon(combo_icon)
        .with_show_direction_badge(show_direction_badges);

    if let Some(fiat_amount) = fiat_converter.map(|converter| {
        let fiat = converter.convert(display_amount);
        format!("{} {}", fiat.to_rounded_string(), fiat.currency())
    }) {
        item = item.with_fiat_amount(fiat_amount);
    }

    // Phase 7 fallback: clicking a row currently just no-ops via the
    // panel's message handler. A detail pane can land later.
    let _ = i;
    item.view(Message::SparkTransactions(
        crate::app::view::spark::SparkTransactionsMessage::Select(i),
    ))
    .into()
}

/// Render the single-payment detail pane. Consumed by
/// `SparkTransactions` state when `selected_payment` is `Some`, and
/// also by Overview/Send/Receive's preselect flow. Richer than the
/// Liquid detail view: surfaces the raw Spark payment ID (with a
/// copy-to-clipboard affordance), a humanised payment-method row
/// (Lightning / On-chain Bitcoin / Spark transfer), and a Status row
/// that reflects every `DomainPaymentStatus` variant the bridge can
/// emit — not just Complete/Pending.
pub fn transaction_detail_view<'a>(
    tx: &'a SparkRecentTransaction,
    bitcoin_unit: BitcoinDisplayUnit,
) -> Element<'a, Message> {
    let is_incoming = tx.is_incoming;
    let title = if is_incoming {
        "Incoming payment"
    } else {
        "Outgoing payment"
    };
    let sign = if is_incoming { "+" } else { "-" };

    let method_icon = match tx.method {
        SparkPaymentMethod::Lightning => asset_network_logo::<Message>("btc", "lightning", 56.0),
        SparkPaymentMethod::OnChainBitcoin => asset_network_logo::<Message>("btc", "bitcoin", 56.0),
        SparkPaymentMethod::Spark => asset_network_logo::<Message>("btc", "spark", 56.0),
    };
    let method_text = match tx.method {
        SparkPaymentMethod::Lightning => "Lightning payment",
        SparkPaymentMethod::OnChainBitcoin => {
            if is_incoming {
                "On-chain deposit"
            } else {
                "On-chain withdrawal"
            }
        }
        SparkPaymentMethod::Spark => "Spark transfer",
    };
    let status_text = match tx.status {
        DomainPaymentStatus::Complete => "Completed",
        DomainPaymentStatus::Pending => "Pending",
        DomainPaymentStatus::Failed => "Failed",
        DomainPaymentStatus::TimedOut => "Timed out",
        DomainPaymentStatus::Created => "Created",
        DomainPaymentStatus::Refundable => "Refundable",
        DomainPaymentStatus::RefundPending => "Refund pending",
        DomainPaymentStatus::WaitingFeeAcceptance => "Awaiting fee",
    };
    let date_text = format_timestamp(tx.timestamp).unwrap_or_else(|| "Unknown".to_string());
    let fiat_str = tx
        .fiat_amount
        .as_ref()
        .map(|f| format!("{} {}", f.to_rounded_string(), f.currency()));

    let amount_row = Row::new()
        .spacing(10)
        .align_y(Alignment::Center)
        .push(text(sign).size(H1_SIZE))
        .push(amount_with_size_and_unit::<Message>(
            &tx.amount,
            H1_SIZE,
            bitcoin_unit,
        ));

    let mut amount_block = Column::new().spacing(4).push(amount_row);
    if let Some(s) = fiat_str {
        amount_block = amount_block.push(text(s).size(P1_SIZE).style(theme::text::secondary));
    }

    let kv = |label: &'static str, value: Element<'a, Message>| -> Row<'a, Message> {
        Row::new()
            .spacing(20)
            .push(
                Column::new()
                    .width(Length::FillPortion(1))
                    .push(text(label).bold()),
            )
            .push(Column::new().width(Length::FillPortion(2)).push(value))
    };

    let payment_id_display = truncate_middle(&tx.id, 10, 10);
    let payment_id_row: Element<'a, Message> = Row::new()
        .spacing(8)
        .align_y(Alignment::Center)
        .push(text(payment_id_display).small())
        .push(
            iced::widget::Button::new(icon::clipboard_icon())
                .on_press(Message::Clipboard(tx.id.clone()))
                .style(theme::button::transparent_border),
        )
        .into();

    let mut info = Column::new()
        .spacing(15)
        .push(kv("Date", text(date_text).into()))
        .push(kv("Status", text(status_text).into()))
        .push(kv("Method", text(method_text).into()))
        .push(kv("Payment ID", payment_id_row));

    if tx.fees_sat.to_sat() > 0 {
        info = info.push(kv(
            "Fees",
            amount_with_size_and_unit::<Message>(&tx.fees_sat, P1_SIZE, bitcoin_unit).into(),
        ));
    }

    Column::new()
        .spacing(20)
        .push(detail_back_button())
        .push(Container::new(h3(title)).width(Length::Fill))
        .push(
            Row::new()
                .spacing(16)
                .align_y(Alignment::Center)
                .push(method_icon)
                .push(amount_block),
        )
        .push(
            text(tx.description.clone())
                .size(P1_SIZE)
                .style(theme::text::secondary),
        )
        .push(card::simple(info))
        .into()
}

fn detail_back_button() -> Element<'static, Message> {
    button::secondary(None, "< Back")
        .width(Length::Fixed(150.0))
        .on_press(Message::Close)
        .into()
}

// `Amount` is kept in scope for future use (e.g. the detail pane).
#[allow(dead_code)]
fn _keep_amount_in_scope(_: Amount) {}
