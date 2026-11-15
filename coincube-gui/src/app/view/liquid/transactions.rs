use coincube_ui::locale::copy as localized_copy;
use coincube_ui::localized_format;
use std::collections::HashMap;

use coincube_core::miniscript::bitcoin::Amount;
use coincube_ui::locale::Text as CopyText;
use iced::widget::image;

use crate::app::wallets::{
    DomainPayment, DomainPaymentDetails, DomainPaymentStatus, DomainRefundableSwap,
};

use coincube_ui::{
    component::{
        amount::DisplayAmount,
        button, card, form,
        pagination::pagination_controls,
        quote_display::{self, Quote, QuoteDisplayProps},
        tenshu::{segment, segmented, SegmentSize},
        text::*,
        transaction::{TransactionDirection, TransactionListItem},
    },
    icon, theme,
    widget::{action::Action, ColumnExt, Element, RowExt},
};
use iced::{
    widget::{scrollable, Column, Container, Row, Space},
    Alignment, Length,
};

use coincube_ui::image::asset_network_logo;

use crate::app::breez_liquid::assets::format_usdt_display;
use crate::app::breez_liquid::swap_status::{classify_payment, BtcSwapReceiveStatus};
use crate::app::menu::Menu;
use crate::app::state::liquid::transactions::{AssetFilter, InFlightRefund};
use crate::app::view::message::{FeeratePriority, Message};
use crate::app::view::FiatAmountConverter;
use crate::export::ImportExportMessage;
use crate::utils::{format_time_ago, format_timestamp, truncate_middle};

const REFUND_BACK_ACTION_ID: &str = "liquid-refund-back";
const REFUND_SUBMIT_ACTION_ID: &str = "liquid-refund-submit";

/// Styled status cell for the payment detail card. For BTC onchain swap
/// payments this routes through `classify_payment`, which gives us the full
/// Boltz lifecycle (including the `Failed` → `Refundable` upgrade when the SDK
/// still lists the swap as refundable and the `Pending` → `PendingConfirmation`
/// vs `PendingSwapCompletion` split once the lockup tx is seen). Direct Liquid
/// and Lightning payments fall back to raw SDK labels, because swap-specific
/// mappings like "Complete Send → Refunded" don't apply to them.
fn payment_status_text(
    payment: &DomainPayment,
    refundable_swap_addresses: &[String],
) -> Element<'static, Message> {
    let (label, style) = payment_detail_status(payment, refundable_swap_addresses);
    text(localized_copy(label)).style(style).into()
}

fn payment_detail_status(
    payment: &DomainPayment,
    refundable_swap_addresses: &[String],
) -> (&'static str, StatusTextStyle) {
    if matches!(payment.details, DomainPaymentDetails::OnChainBitcoin { .. }) {
        let status = classify_payment(payment, refundable_swap_addresses);
        let style = match status {
            BtcSwapReceiveStatus::Completed | BtcSwapReceiveStatus::Refunded => {
                theme::text::success
            }
            BtcSwapReceiveStatus::Failed | BtcSwapReceiveStatus::Refundable => {
                theme::text::destructive
            }
            _ => theme::text::secondary,
        };
        let label = if status == BtcSwapReceiveStatus::Completed {
            CopyText::Confirmed.get()
        } else {
            status.label()
        };
        return (label, style);
    }

    match payment.status {
        DomainPaymentStatus::Complete => (CopyText::Confirmed.get(), theme::text::success),
        DomainPaymentStatus::Pending => ("Pending", theme::text::secondary),
        DomainPaymentStatus::Created => ("Created", theme::text::secondary),
        DomainPaymentStatus::Failed => ("Failed", theme::text::destructive),
        DomainPaymentStatus::TimedOut => ("Timed Out", theme::text::destructive),
        DomainPaymentStatus::Refundable => ("Refundable", theme::text::destructive),
        DomainPaymentStatus::RefundPending => ("Refund Pending", theme::text::secondary),
        DomainPaymentStatus::WaitingFeeAcceptance => {
            ("Waiting Fee Acceptance", theme::text::secondary)
        }
    }
}

/// Best-effort description for a USDt payment: prefer payer_note, then
/// invoice description, then fall back to "USDt Transfer".  Mirrors the
/// logic in `send.rs` so the transactions list and the send-confirmation
/// screen show the same label.
fn usdt_description(payment: &DomainPayment) -> String {
    let desc = payment.details.description();
    if desc.is_empty() {
        localized_copy("USDt Transfer").to_owned()
    } else {
        desc.to_owned()
    }
}

/// Returns `Some(formatted_usdt_string)` when the payment is a USDt asset payment.
fn usdt_amount_str(payment: &DomainPayment, usdt_id: &str) -> Option<String> {
    if let DomainPaymentDetails::LiquidAsset {
        asset_id,
        asset_info,
        ..
    } = &payment.details
    {
        if !usdt_id.is_empty() && asset_id == usdt_id {
            let minor = asset_info
                .as_ref()
                .map(|i| i.amount_minor)
                .unwrap_or(payment.amount_sat);
            return Some(format!("{} USDt", format_usdt_display(minor)));
        }
    }
    None
}

// The SDK reports the L-BTC fee as a number of sats. Zero does not prove that
// the fee was paid in USDt; it may also mean no fee or unavailable fee data.
fn usdt_fee_caption(fees_sat: u64) -> &'static str {
    if fees_sat == 0 {
        localized_copy("No L-BTC fee reported")
    } else {
        localized_copy("Fees (L-BTC)")
    }
}

#[allow(clippy::too_many_arguments)]
pub fn liquid_transactions_view<'a>(
    payments: &'a [DomainPayment],
    refundables: &'a [DomainRefundableSwap],
    in_flight_refunds: &'a HashMap<String, InFlightRefund>,
    _balance: &'a Amount,
    fiat_converter: Option<FiatAmountConverter>,
    loading: bool,
    bitcoin_unit: coincube_ui::component::amount::BitcoinDisplayUnit,
    usdt_id: &'a str,
    asset_filter: AssetFilter,
    show_direction_badges: bool,
    empty_state_quote: &'a Quote,
    empty_state_image_handle: &'a image::Handle,
    current_page: u32,
    is_last_page: bool,
    processing: bool,
    swap_tx_ids: &'a std::collections::HashSet<String>,
    mask_amounts: bool,
) -> Element<'a, Message> {
    let mut content = Column::new().spacing(20).width(Length::Fill);

    // Keep the asset filter beside the title, as in the Liquid artboard.
    // The source state still owns selection, server filtering and paging.
    let filter_row = segmented(
        [
            segment(
                "liquid-transactions-filter-all",
                AssetFilter::All,
                localized_copy("All"),
            ),
            segment(
                "liquid-transactions-filter-lbtc",
                AssetFilter::LbtcOnly,
                "L-BTC",
            ),
            segment(
                "liquid-transactions-filter-usdt",
                AssetFilter::UsdtOnly,
                "USDt",
            ),
        ],
        Some(asset_filter),
        Message::FocusAction,
    )
    .size(SegmentSize::Compact)
    .on_select(Message::SetAssetFilter);
    content = content.push(
        Row::new()
            .spacing(12)
            .align_y(Alignment::Center)
            .push(Container::new(h3(localized_copy("Transactions")).bold()))
            .push(Space::new().width(Length::Fill))
            .push(filter_row)
            .push(
                button::secondary(Some(icon::backup_icon()), localized_copy("Export"))
                    .on_press(ImportExportMessage::Open.into()),
            ),
    );

    // A refundable BTC → L-BTC swap is still activity even when the regular
    // payment page is empty. The USDt tab must never show these BTC refunds.
    let show_refundables = refundables_visible(refundables.len(), asset_filter);

    // Put recoverable funds first so they are not buried after a paged list.
    if show_refundables {
        content = content.push(
            Container::new(
                Column::new()
                    .spacing(8)
                    .push(Container::new(h4_bold(localized_copy(
                        "Refundable Transactions",
                    ))))
                    .push(refundables.iter().enumerate().fold(
                        Column::new().spacing(0),
                        |col, (i, refundable)| {
                            col.push(refundable_row(
                                i,
                                refundable,
                                in_flight_refunds.get(&refundable.swap_address),
                                fiat_converter,
                                bitcoin_unit,
                                mask_amounts,
                            ))
                        },
                    )),
            )
            .padding([10, 16])
            .width(Length::Fill)
            .style(|theme: &theme::Theme| iced::widget::container::Style {
                background: Some(iced::Background::Color(theme.colors.shell.panel)),
                border: iced::Border {
                    color: theme.colors.shell.warn,
                    width: 1.0,
                    radius: 12.0.into(),
                },
                ..Default::default()
            }),
        );
    }

    // On page 0 with no rows we render the empty state; on later pages an
    // empty response just means we've paged past the end — keep the list
    // shell and pagination controls visible so the user can Prev back.
    if loading && payments.is_empty() && current_page == 0 && !show_refundables {
        content = content.push(
            p1_regular(CopyText::LoadingLiquidTransactions.get()).style(theme::text::secondary),
        );
    } else if payments.is_empty() && current_page == 0 && !show_refundables {
        // Empty state with Kage quote
        content = content.push(
            Column::new()
                .spacing(20)
                .width(Length::Fill)
                .align_x(Alignment::Center)
                .push(Space::new().height(Length::Fixed(40.0)))
                .push(quote_display::display(
                    &QuoteDisplayProps::new(
                        "empty-wallet",
                        empty_state_quote,
                        empty_state_image_handle,
                    )
                    .existing_artwork(),
                ))
                .push(Space::new().height(Length::Fixed(10.0)))
                .push(
                    text(match asset_filter {
                        AssetFilter::All => CopyText::LiquidHistoryEmpty.get(),
                        AssetFilter::LbtcOnly => CopyText::NoLBtcTransactionsOnThisPage.get(),
                        AssetFilter::UsdtOnly => CopyText::NoUsdtTransactionsOnThisPage.get(),
                    })
                    .size(16)
                    .style(theme::text::secondary)
                    .wrapping(iced::widget::text::Wrapping::Word)
                    .align_x(iced::alignment::Horizontal::Center),
                )
                .push_maybe(matches!(asset_filter, AssetFilter::All).then(|| {
                    Row::new()
                        .spacing(15)
                        .push(
                            button::primary(None, localized_copy("Send"))
                                .on_press(Message::Menu(Menu::Liquid(
                                    crate::app::menu::LiquidSubMenu::Send,
                                )))
                                .padding(15)
                                .width(Length::Fixed(150.0)),
                        )
                        .push(
                            button::transparent_border(None, localized_copy("Receive"))
                                .on_press(Message::Menu(Menu::Liquid(
                                    crate::app::menu::LiquidSubMenu::Receive,
                                )))
                                .padding(15)
                                .width(Length::Fixed(150.0)),
                        )
                })),
        );
    } else if !payments.is_empty() || current_page > 0 {
        // The dashboard already wraps the whole panel in a page-level
        // scrollable, so the list is pushed directly — nesting another
        // scrollable here would make its `Fill` height resolve against an
        // unbounded parent and break layout. Pagination keeps each page
        // bounded (PAGE_SIZE rows), so the page scroll is enough.
        let refundable_swap_addresses: Vec<String> = refundables
            .iter()
            .map(|swap| swap.swap_address.clone())
            .collect();
        let list =
            payments
                .iter()
                .enumerate()
                .fold(Column::new().spacing(10), |col, (i, payment)| {
                    let is_swap = payment
                        .tx_id
                        .as_ref()
                        .is_some_and(|id| swap_tx_ids.contains(id));
                    col.push(transaction_row(
                        i,
                        payment,
                        fiat_converter,
                        bitcoin_unit,
                        usdt_id,
                        show_direction_badges,
                        is_swap,
                        &refundable_swap_addresses,
                        mask_amounts,
                    ))
                });
        content = content.push(list);
        content = content.push(pagination_controls(
            Message::LiquidPrevPage,
            Message::LiquidNextPage,
            current_page > 0,
            !is_last_page,
            processing,
            current_page,
        ));
    }

    content = content.push(Space::new().height(Length::Fixed(30.0)));
    content.into()
}

#[allow(clippy::too_many_arguments)]
fn transaction_row<'a>(
    i: usize,
    payment: &'a DomainPayment,
    fiat_converter: Option<FiatAmountConverter>,
    bitcoin_unit: coincube_ui::component::amount::BitcoinDisplayUnit,
    usdt_id: &str,
    show_direction_badges: bool,
    is_swap: bool,
    refundable_swap_addresses: &[String],
    mask_amounts: bool,
) -> Element<'a, Message> {
    let is_receive = payment.is_incoming();
    let usdt_str = (!mask_amounts)
        .then(|| usdt_amount_str(payment, usdt_id))
        .flatten();

    // Extract description — prefer payer_note/description, fall back to "USDt
    // Transfer". A locally-recorded swap overrides the label with "Swap".
    let is_usdt = matches!(&payment.details, DomainPaymentDetails::LiquidAsset { asset_id, .. } if !usdt_id.is_empty() && asset_id == usdt_id);
    let description: String = if is_swap {
        "Swap".to_owned()
    } else if is_usdt {
        usdt_description(payment)
    } else {
        payment.details.description().to_owned()
    };

    let time_ago = format_time_ago(payment.timestamp.into());

    let direction = if is_receive {
        TransactionDirection::Incoming
    } else {
        TransactionDirection::Outgoing
    };

    // Determine the combo icon based on payment type
    let combo_icon = payment_row_icon(is_usdt, &payment.details);

    let status = payment_list_status(payment, refundable_swap_addresses);

    if is_usdt {
        let mut item = TransactionListItem::new(direction, &Amount::ZERO, bitcoin_unit)
            .with_label(description.clone())
            .with_time_ago(time_ago)
            .with_custom_icon(combo_icon)
            .with_show_direction_badge(show_direction_badges)
            .with_amount_override(usdt_str.unwrap_or_else(|| "••••••".to_owned()));
        if let Some((label, style)) = status {
            item = item.with_custom_status(status_badge(label, style));
        }
        return item.view(Message::Select(i)).into();
    }

    let mut btc_amount = Amount::from_sat(payment.amount_sat);
    if !is_receive {
        btc_amount += Amount::from_sat(payment.fees_sat);
    }

    let mut item = TransactionListItem::new(direction, &btc_amount, bitcoin_unit)
        .with_label(description)
        .with_time_ago(time_ago)
        .with_custom_icon(combo_icon)
        .with_show_direction_badge(show_direction_badges);
    if mask_amounts {
        item = item.with_amount_override("••••••".to_owned());
    }

    if let Some(fiat_amount) = fiat_converter.filter(|_| !mask_amounts).map(|converter| {
        let fiat = converter.convert(btc_amount);
        localized_format!(
            "{} {} · current rate",
            "{} {} · tasa actual",
            fiat.to_formatted_string(),
            fiat.currency()
        )
    }) {
        item = item.with_fiat_amount(fiat_amount);
    }

    if let Some((label, style)) = status {
        item = item.with_custom_status(status_badge(label, style));
    }

    item.view(Message::Select(i)).into()
}

type StatusTextStyle = fn(&theme::Theme) -> iced::widget::text::Style;

fn refundables_visible(count: usize, filter: AssetFilter) -> bool {
    count > 0 && matches!(filter, AssetFilter::All | AssetFilter::LbtcOnly)
}

/// List status uses the same swap classifier as detail, so a failed payment
/// that remains in the SDK refundable set reads "Refundable" in both places.
fn payment_list_status(
    payment: &DomainPayment,
    refundable_swap_addresses: &[String],
) -> Option<(&'static str, StatusTextStyle)> {
    if matches!(payment.details, DomainPaymentDetails::OnChainBitcoin { .. }) {
        let status = classify_payment(payment, refundable_swap_addresses);
        return match status {
            BtcSwapReceiveStatus::Completed => {
                Some((CopyText::Confirmed.get(), theme::text::success))
            }
            BtcSwapReceiveStatus::Refunded => Some(("Refunded", theme::text::success)),
            BtcSwapReceiveStatus::Failed | BtcSwapReceiveStatus::Refundable => {
                Some((status.label(), theme::text::error))
            }
            status => Some((status.label(), theme::text::warning)),
        };
    }
    match payment.status {
        DomainPaymentStatus::Complete => Some((CopyText::Confirmed.get(), theme::text::success)),
        DomainPaymentStatus::Pending => Some(("Pending", theme::text::warning)),
        DomainPaymentStatus::Created => Some(("Created", theme::text::secondary)),
        DomainPaymentStatus::Failed => Some(("Failed", theme::text::error)),
        DomainPaymentStatus::TimedOut => Some(("Timed Out", theme::text::error)),
        DomainPaymentStatus::Refundable => Some(("Refundable", theme::text::error)),
        DomainPaymentStatus::RefundPending => Some(("Refund Pending", theme::text::warning)),
        DomainPaymentStatus::WaitingFeeAcceptance => {
            Some(("Waiting Fee Acceptance", theme::text::warning))
        }
    }
}

fn status_badge(label: &'static str, style: StatusTextStyle) -> Element<'static, Message> {
    Container::new(
        text(localized_copy(label))
            .size(CAPTION_SIZE)
            .bold()
            .style(style),
    )
    .padding([2, 8])
    .style(|theme: &theme::Theme| iced::widget::container::Style {
        border: iced::Border {
            color: theme.colors.rule,
            width: 1.0,
            radius: 10.0.into(),
        },
        ..Default::default()
    })
    .into()
}

fn refundable_row<'a>(
    i: usize,
    refundable: &'a DomainRefundableSwap,
    in_flight: Option<&'a InFlightRefund>,
    fiat_converter: Option<FiatAmountConverter>,
    bitcoin_unit: coincube_ui::component::amount::BitcoinDisplayUnit,
    mask_amounts: bool,
) -> Element<'a, Message> {
    let btc_amount = Amount::from_sat(refundable.amount_sat);
    let time_ago = format_time_ago(refundable.timestamp.into());

    // This is a recoverable amount, not money already received. Keep it
    // unsigned and report submission until the SDK returns a broadcast txid.
    let status = in_flight
        .map(|entry| refund_status(false, Some(entry)))
        .unwrap_or_else(|| "Refundable".to_owned());

    let amount = if mask_amounts {
        "••••••".to_owned()
    } else {
        btc_amount.to_formatted_string_with_unit(bitcoin_unit)
    };
    let mut amount_column = Column::new()
        .spacing(4)
        .align_x(Alignment::End)
        .push(p1_bold(amount));
    if let Some(converter) = fiat_converter.filter(|_| !mask_amounts) {
        let fiat = converter.convert(btc_amount);
        amount_column = amount_column.push(
            p2_regular(localized_format!(
                "{} {} · current rate",
                "{} {} · tasa actual",
                fiat.to_formatted_string(),
                fiat.currency()
            ))
            .style(theme::text::secondary),
        );
    }
    let row = Row::new()
        .spacing(12)
        .align_y(Alignment::Center)
        .push(icon::warning_icon().style(theme::text::warning))
        .push(
            Column::new()
                .spacing(4)
                .width(Length::Fill)
                .push(p1_regular(localized_copy("Refundable Swap")))
                .push(
                    Row::new()
                        .spacing(8)
                        .align_y(Alignment::Center)
                        .push(p2_regular(time_ago).style(theme::text::secondary))
                        .push(p2_regular(status).style(theme::text::warning)),
                ),
        )
        .push(amount_column);
    iced::widget::button(row.padding([10, 0]))
        .width(Length::Fill)
        .on_press(Message::SelectRefundable(i))
        .style(theme::button::transparent)
        .into()
}

pub fn transaction_detail_view<'a>(
    payment: &'a DomainPayment,
    fiat_converter: Option<FiatAmountConverter>,
    bitcoin_unit: coincube_ui::component::amount::BitcoinDisplayUnit,
    usdt_id: &str,
    refundable_swap_addresses: &[String],
    mask_amounts: bool,
) -> Element<'a, Message> {
    let is_receive = payment.is_incoming();
    let usdt_str = usdt_amount_str(payment, usdt_id);
    let btc_amount = Amount::from_sat(payment.amount_sat);
    let fees_sat = Amount::from_sat(payment.fees_sat);
    let mut total_amount = btc_amount;

    if !is_receive {
        total_amount += fees_sat;
    }

    // Format full date/time
    let date_text =
        format_timestamp(payment.timestamp as u64).unwrap_or_else(|| "Unknown".to_string());

    // Extract description — prefer payer_note/description, fall back to "USDt Transfer"
    let description: String = if usdt_str.is_some() {
        usdt_description(payment)
    } else {
        payment.details.description().to_owned()
    };

    let title = if is_receive {
        localized_copy("Incoming payment")
    } else {
        localized_copy("Outgoing payment")
    };

    // Helper: combo icon for detail view based on payment type
    let make_detail_icon =
        |is_usdt: bool, details: &DomainPaymentDetails| -> (&'static str, &'static str) {
            if is_usdt {
                ("usdt", "liquid")
            } else {
                match details {
                    DomainPaymentDetails::Lightning { .. } => ("btc", "lightning"),
                    DomainPaymentDetails::LiquidAsset { .. } => ("lbtc", "liquid"),
                    DomainPaymentDetails::OnChainBitcoin { .. } => ("btc", "bitcoin"),
                }
            }
        };

    if let Some(ref usdt_display) = usdt_str {
        // USDt detail view: show USDt amount + L-BTC fees
        let amount_row = usdt_headline(payment, mask_amounts, is_receive);
        return Column::new()
            .spacing(20)
            .push(detail_back_button())
            .push(Container::new(h3(title)).width(Length::Fill))
            .push({
                let (a, n) = make_detail_icon(true, &payment.details);
                Row::new()
                    .spacing(10)
                    .align_y(Alignment::Center)
                    .push(asset_network_logo::<Message>(a, n, 32.0))
                    .push(p1_regular(description))
            })
            .push(
                Column::new()
                    .spacing(20)
                    .push(Column::new().push(Container::new(amount_row))),
            )
            .push(usdt_detail_card(
                payment,
                refundable_swap_addresses,
                usdt_display,
                fees_sat,
                bitcoin_unit,
                mask_amounts,
                date_text,
            ))
            .into();
    }

    Column::new()
        .spacing(20)
        .push(detail_back_button())
        .push(if is_receive {
            Container::new(h3(localized_copy("Incoming payment"))).width(Length::Fill)
        } else {
            Container::new(h3(localized_copy("Outgoing payment"))).width(Length::Fill)
        })
        .push({
            let (a, n) = make_detail_icon(false, &payment.details);
            Row::new()
                .spacing(10)
                .align_y(Alignment::Center)
                .push(asset_network_logo::<Message>(a, n, 32.0))
                .push(p1_regular(description))
        })
        .push(btc_detail_headline(
            is_receive,
            btc_amount,
            total_amount,
            bitcoin_unit,
            fiat_converter,
            mask_amounts,
        ))
        .push(card::simple(
            Column::new()
                .push(
                    Row::new()
                        .push(
                            Column::new()
                                .width(Length::FillPortion(1))
                                .push(text(localized_copy("Date")).bold()),
                        )
                        .push(
                            Column::new()
                                .width(Length::FillPortion(2))
                                .push(text(date_text)),
                        )
                        .spacing(20),
                )
                .push(
                    Row::new()
                        .push(
                            Column::new()
                                .width(Length::FillPortion(1))
                                .push(text(localized_copy("Status")).bold()),
                        )
                        .push(
                            Column::new()
                                .width(Length::FillPortion(2))
                                .push(payment_status_text(payment, refundable_swap_addresses)),
                        )
                        .spacing(20),
                )
                .push(
                    Row::new()
                        .push(
                            Column::new()
                                .width(Length::FillPortion(1))
                                .push(text(localized_copy("Amount")).bold()),
                        )
                        .push(Column::new().width(Length::FillPortion(2)).push(text(
                            if mask_amounts {
                                "••••••".to_owned()
                            } else {
                                btc_amount.to_formatted_string_with_unit(bitcoin_unit)
                            },
                        )))
                        .spacing(20),
                )
                .push(
                    Row::new()
                        .push(
                            Column::new()
                                .width(Length::FillPortion(1))
                                .push(text(localized_copy("Fees")).bold()),
                        )
                        .push(Column::new().width(Length::FillPortion(2)).push(text(
                            if mask_amounts {
                                "••••••".to_owned()
                            } else {
                                fees_sat.to_formatted_string_with_unit(bitcoin_unit)
                            },
                        )))
                        .spacing(20),
                )
                .spacing(15),
        ))
        .into()
}

/// The selected priority remains visible while its fee estimate loads. Each
/// option is keyboard reachable and arrow keys move between enabled options.
fn fee_priority_buttons(
    pending: Option<FeeratePriority>,
    refunding: bool,
) -> Element<'static, Message> {
    segmented(
        [
            segment(
                "liquid-refund-fee-low",
                FeeratePriority::Low,
                if pending == Some(FeeratePriority::Low) {
                    "Low…"
                } else {
                    "Low"
                },
            )
            .enabled(pending != Some(FeeratePriority::Low)),
            segment(
                "liquid-refund-fee-medium",
                FeeratePriority::Medium,
                if pending == Some(FeeratePriority::Medium) {
                    "Medium…"
                } else {
                    "Medium"
                },
            )
            .enabled(pending != Some(FeeratePriority::Medium)),
            segment(
                "liquid-refund-fee-high",
                FeeratePriority::High,
                if pending == Some(FeeratePriority::High) {
                    "High…"
                } else {
                    "High"
                },
            )
            .enabled(pending != Some(FeeratePriority::High)),
        ],
        pending,
        Message::FocusAction,
    )
    .size(SegmentSize::Compact)
    .on_select_maybe((!refunding).then_some(Message::RefundFeeratePrioritySelected))
    .into()
}

fn refund_status(refunding: bool, in_flight: Option<&InFlightRefund>) -> String {
    match in_flight {
        Some(InFlightRefund {
            refund_txid: Some(txid),
            ..
        }) => format!("Refund broadcast · {}", truncate_middle(txid, 6, 6)),
        Some(_) => "Submitting refund…".to_owned(),
        None if refunding => "Submitting refund…".to_owned(),
        None => "This swap can be refunded".to_owned(),
    }
}

fn refund_can_submit(
    refund_address: &form::Value<String>,
    refund_feerate: &form::Value<String>,
    refunding: bool,
    in_flight: bool,
) -> bool {
    !refunding
        && !in_flight
        && refund_address.valid
        && !refund_address.value.trim().is_empty()
        && refund_feerate.valid
        && refund_feerate
            .value
            .trim()
            .parse::<u32>()
            .is_ok_and(|rate| rate > 0)
}

#[allow(clippy::too_many_arguments)]
pub fn refundable_detail_view<'a>(
    chain: crate::chain::ChainId,
    refundable: &'a DomainRefundableSwap,
    fiat_converter: Option<FiatAmountConverter>,
    bitcoin_unit: coincube_ui::component::amount::BitcoinDisplayUnit,
    refund_address: &'a form::Value<String>,
    refund_feerate: &'a form::Value<String>,
    refunding: bool,
    pending_fee_priority: Option<FeeratePriority>,
    in_flight: Option<&'a InFlightRefund>,
    has_vault: bool,
    mask_amounts: bool,
) -> Element<'a, Message> {
    let btc_amount = Amount::from_sat(refundable.amount_sat);
    let header_amount: Element<'_, Message> = if mask_amounts {
        text("••••••").size(H1_SIZE).into()
    } else {
        coincube_ui::component::amount::amount_with_size_and_unit::<Message>(
            &btc_amount,
            H1_SIZE,
            bitcoin_unit,
        )
        .into()
    };

    let date_text =
        format_timestamp(refundable.timestamp as u64).unwrap_or_else(|| "Unknown".to_string());

    let can_refund = pending_fee_priority.is_none()
        && refund_can_submit(
            refund_address,
            refund_feerate,
            refunding,
            in_flight.is_some(),
        );

    let header_status = refund_status(refunding, in_flight);

    Column::new()
        .spacing(20)
        .push(refund_back_button())
        .push(
            Row::new()
                .spacing(12)
                .align_y(Alignment::Center)
                .push(icon::warning_icon().style(theme::text::warning))
                .push(h3(localized_copy("Refundable Swap"))),
        )
        .push(
            Column::new()
                .push(p1_regular(header_status).style(theme::text::warning))
                .spacing(10),
        )
        .push(
            Column::new().spacing(20).push(
                Column::new()
                    .push(Container::new(Row::new().spacing(5).push(header_amount)))
                    .push_maybe(fiat_converter.filter(|_| !mask_amounts).map(|converter| {
                        let fiat = converter.convert(btc_amount);
                        Row::new().align_y(Alignment::Center).push(
                            fiat.to_text()
                                .size(H2_SIZE)
                                .color(coincube_ui::color::GREY_2),
                        )
                    })),
            ),
        )
        .push(card::simple(
            Column::new()
                .push(
                    Row::new()
                        .push(
                            Column::new()
                                .width(Length::FillPortion(1))
                                .push(text(localized_copy("Date")).bold()),
                        )
                        .push(
                            Column::new()
                                .width(Length::FillPortion(2))
                                .push(text(date_text)),
                        )
                        .spacing(20),
                )
                .push(
                    Row::new()
                        .push(
                            Column::new()
                                .width(Length::FillPortion(1))
                                .push(text(localized_copy("Swap Address")).bold()),
                        )
                        .push(
                            Column::new()
                                .width(Length::FillPortion(2))
                                // Long taproot swap addresses overflow the
                                // card otherwise. Show a middle-truncated
                                // preview with a copy button so the user can
                                // still grab the full address.
                                .push(
                                    Row::new()
                                        .align_y(Alignment::Center)
                                        .spacing(8)
                                        .push(Container::new(
                                            scrollable(
                                                text(truncate_middle(
                                                    &refundable.swap_address,
                                                    10,
                                                    10,
                                                ))
                                                .size(14),
                                            )
                                            .direction(scrollable::Direction::Horizontal(
                                                scrollable::Scrollbar::new()
                                                    .width(2)
                                                    .scroller_width(2),
                                            )),
                                        ))
                                        .push(
                                            iced::widget::button(
                                                icon::clipboard_icon()
                                                    .style(theme::text::secondary),
                                            )
                                            .on_press(Message::Clipboard(
                                                refundable.swap_address.clone(),
                                            ))
                                            .style(theme::button::transparent_border),
                                        ),
                                ),
                        )
                        .spacing(20),
                )
                .push(
                    Row::new()
                        .push(
                            Column::new()
                                .width(Length::FillPortion(1))
                                .push(text(localized_copy("Amount")).bold()),
                        )
                        .push(Column::new().width(Length::FillPortion(2)).push(text(
                            if mask_amounts {
                                "••••••".to_owned()
                            } else {
                                btc_amount.to_formatted_string_with_unit(bitcoin_unit)
                            },
                        )))
                        .spacing(20),
                )
                .spacing(15),
        ))
        .push_maybe((in_flight.is_none() || refunding).then(|| {
            Column::new()
                .spacing(15)
                .push(Container::new(h4_bold(localized_copy("Refund Details"))).width(Length::Fill))
                .push(p2_regular(CopyText::DestinationNetwork.get().replacen(
                    "{network}",
                    crate::chain::canonical_network_name(chain),
                    1,
                )))
                .push(p1_regular(localized_format!(
                    "Refund amount: {}",
                    "Cantidad de reembolso: {}",
                    btc_amount.to_formatted_string_with_unit(bitcoin_unit)
                )))
                .push(refund_address_controls(
                    refund_address,
                    refunding,
                    has_vault,
                ))
                .push(refund_fee_controls(
                    refund_feerate,
                    pending_fee_priority,
                    refunding,
                ))
                .push(
                    Row::new().spacing(10).push(
                        button::primary_action(
                            REFUND_SUBMIT_ACTION_ID,
                            None,
                            if refunding { "Refund…" } else { "Refund" },
                            Message::FocusAction,
                        )
                        .on_press_maybe(can_refund.then_some(Message::SubmitRefund))
                        .width(Length::Shrink),
                    ),
                )
                .push_maybe((pending_fee_priority.is_some() && !refunding).then(|| {
                    p2_regular(localized_copy("Fetching fee rate…")).style(theme::text::secondary)
                }))
                .push_maybe(
                    (!can_refund && !refunding && pending_fee_priority.is_none()).then(|| {
                        p2_regular(localized_copy(
                            "Enter a valid Bitcoin address and fee rate to enable Refund.",
                        ))
                        .style(theme::text::secondary)
                    }),
                )
        }))
        .into()
}

fn refund_back_button() -> Element<'static, Message> {
    Action::new(
        REFUND_BACK_ACTION_ID,
        text(localized_copy("‹ Back")),
        Message::FocusAction,
    )
    .width(Length::Shrink)
    .padding(5)
    .on_press(Message::Close)
    .style(theme::button::link)
    .into()
}

fn detail_back_button() -> Element<'static, Message> {
    button::secondary(None, "< Back")
        .width(Length::Fixed(150.0))
        .on_press(Message::Close)
        .into()
}

fn payment_row_icon(is_usdt: bool, details: &DomainPaymentDetails) -> Element<'static, Message> {
    if is_usdt {
        asset_network_logo("usdt", "liquid", 40.0)
    } else {
        match details {
            DomainPaymentDetails::Lightning { .. } => asset_network_logo("btc", "lightning", 40.0),
            DomainPaymentDetails::LiquidAsset { .. } => asset_network_logo("lbtc", "liquid", 40.0),
            DomainPaymentDetails::OnChainBitcoin { .. } => {
                asset_network_logo("btc", "bitcoin", 40.0)
            }
        }
    }
}

fn usdt_headline<'a>(
    payment: &DomainPayment,
    mask_amounts: bool,
    is_receive: bool,
) -> coincube_ui::widget::Row<'a, Message> {
    let usdt_num = match &payment.details {
        DomainPaymentDetails::LiquidAsset { asset_info, .. } => asset_info
            .as_ref()
            .map(|i| format_usdt_display(i.amount_minor))
            .unwrap_or_else(|| format_usdt_display(payment.amount_sat)),
        _ => format_usdt_display(payment.amount_sat),
    };
    if mask_amounts {
        Row::new().push(text("••••••").size(H1_SIZE).bold())
    } else if is_receive {
        Row::new()
            .spacing(5)
            .align_y(Alignment::Center)
            .push(text("+").size(H1_SIZE))
            .push(text(usdt_num).size(H1_SIZE).bold())
            .push(text("USDt").size(H1_SIZE).color(coincube_ui::color::GREY_3))
    } else {
        Row::new()
            .spacing(5)
            .align_y(Alignment::Center)
            .push(text("-").size(H1_SIZE))
            .push(
                Row::new()
                    .spacing(10)
                    .align_y(Alignment::Center)
                    .push(text(usdt_num).size(H1_SIZE).bold())
                    .push(text("USDt").size(H1_SIZE).color(coincube_ui::color::GREY_3)),
            )
    }
}

fn usdt_detail_card<'a>(
    payment: &DomainPayment,
    refundable_swap_addresses: &[String],
    usdt_display: &str,
    fees_sat: Amount,
    bitcoin_unit: coincube_ui::component::amount::BitcoinDisplayUnit,
    mask_amounts: bool,
    date_text: String,
) -> Element<'a, Message> {
    card::simple(
        Column::new()
            .push(
                Row::new()
                    .push(
                        Column::new()
                            .width(Length::FillPortion(1))
                            .push(text(localized_copy("Date")).bold()),
                    )
                    .push(
                        Column::new()
                            .width(Length::FillPortion(2))
                            .push(text(date_text)),
                    )
                    .spacing(20),
            )
            .push(
                Row::new()
                    .push(
                        Column::new()
                            .width(Length::FillPortion(1))
                            .push(text(localized_copy("Status")).bold()),
                    )
                    .push(
                        Column::new()
                            .width(Length::FillPortion(2))
                            .push(payment_status_text(payment, refundable_swap_addresses)),
                    )
                    .spacing(20),
            )
            .push(
                Row::new()
                    .push(
                        Column::new()
                            .width(Length::FillPortion(1))
                            .push(text(localized_copy("Asset Amount")).bold()),
                    )
                    .push(
                        Column::new()
                            .width(Length::FillPortion(2))
                            .push(text(if mask_amounts {
                                "••••••".to_owned()
                            } else {
                                usdt_display.to_owned()
                            })),
                    )
                    .spacing(20),
            )
            .push(if fees_sat.to_sat() == 0 {
                Row::new()
                    .push(
                        Column::new()
                            .width(Length::FillPortion(1))
                            .push(text(usdt_fee_caption(0)).bold()),
                    )
                    .push(Column::new().width(Length::FillPortion(2)).push(text("—")))
                    .spacing(20)
            } else {
                Row::new()
                    .push(
                        Column::new()
                            .width(Length::FillPortion(1))
                            .push(text(usdt_fee_caption(fees_sat.to_sat())).bold()),
                    )
                    .push(
                        Column::new()
                            .width(Length::FillPortion(2))
                            .push(text(if mask_amounts {
                                "••••••".to_owned()
                            } else {
                                fees_sat.to_formatted_string_with_unit(bitcoin_unit)
                            })),
                    )
                    .spacing(20)
            })
            .spacing(15),
    )
    .into()
}

fn btc_detail_headline<'a>(
    is_receive: bool,
    btc_amount: Amount,
    total_amount: Amount,
    bitcoin_unit: coincube_ui::component::amount::BitcoinDisplayUnit,
    fiat_converter: Option<FiatAmountConverter>,
    mask_amounts: bool,
) -> coincube_ui::widget::Column<'a, Message> {
    Column::new().spacing(20).push(
        Column::new()
            .push(if mask_amounts {
                Container::new(text("••••••").size(H1_SIZE))
            } else if is_receive {
                Container::new(coincube_ui::component::amount::amount_with_size_and_unit(
                    &btc_amount,
                    H1_SIZE,
                    bitcoin_unit,
                ))
            } else {
                Container::new(Row::new().spacing(5).push(text("-").size(H1_SIZE)).push(
                    coincube_ui::component::amount::amount_with_size_and_unit(
                        &total_amount,
                        H1_SIZE,
                        bitcoin_unit,
                    ),
                ))
            })
            .push_maybe(fiat_converter.filter(|_| !mask_amounts).map(|converter| {
                // Use total_amount for outgoing payments to match headline (includes fees)
                let amount_for_conversion = if is_receive { btc_amount } else { total_amount };
                let fiat = converter.convert(amount_for_conversion);
                Row::new().align_y(Alignment::Center).push(
                    text(localized_format!(
                        "≈ {} {} · current rate",
                        "♥ {} {} · tasa actual",
                        fiat.to_formatted_string(),
                        fiat.currency()
                    ))
                    .size(H2_SIZE)
                    .style(theme::text::secondary),
                )
            })),
    )
}

fn refund_address_controls<'a>(
    refund_address: &'a form::Value<String>,
    refunding: bool,
    has_vault: bool,
) -> coincube_ui::widget::Column<'a, Message> {
    Column::new()
        .spacing(10)
        .push(text(localized_copy("Bitcoin Address")).size(P2_SIZE).bold())
        .push(
            Row::new()
                .spacing(8)
                .align_y(Alignment::Center)
                .push(
                    if refunding {
                        form::Form::new_disabled(
                            coincube_ui::locale::copy("Enter Bitcoin address for refund"),
                            refund_address,
                        )
                    } else {
                        form::Form::new(
                            coincube_ui::locale::copy("Enter Bitcoin address for refund"),
                            refund_address,
                            Message::RefundAddressEdited,
                        )
                    }
                    .maybe_warning(if refunding {
                        None
                    } else {
                        refund_address.warning.map(coincube_ui::locale::copy)
                    })
                    .size(14)
                    .padding(12),
                )
                .push_maybe(has_vault.then(|| {
                    button::transparent_border(None, localized_copy("Use Vault address"))
                        .on_press_maybe((!refunding).then_some(Message::GenerateVaultRefundAddress))
                        .padding([6, 14])
                })),
        )
}

fn refund_fee_controls<'a>(
    refund_feerate: &'a form::Value<String>,
    pending_fee_priority: Option<FeeratePriority>,
    refunding: bool,
) -> coincube_ui::widget::Column<'a, Message> {
    Column::new()
        .spacing(10)
        .push(
            text(localized_copy("Fee Rate (sat/vB)"))
                .size(P2_SIZE)
                .bold(),
        )
        .push(
            Row::new()
                .spacing(10)
                .push(
                    if refunding {
                        form::Form::new_disabled(
                            coincube_ui::locale::copy("Enter fee rate"),
                            refund_feerate,
                        )
                    } else {
                        form::Form::new_amount_sats(
                            coincube_ui::locale::copy("Enter fee rate"),
                            refund_feerate,
                            Message::RefundFeerateEdited,
                        )
                    }
                    .size(14)
                    .padding(12),
                )
                .push(fee_priority_buttons(pending_fee_priority, refunding)),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::view::shared::landing::capture;
    use crate::app::wallets::DomainPaymentDirection;
    use coincube_ui::component::amount::BitcoinDisplayUnit;
    use iced::advanced::widget::Id;
    use iced::keyboard::key::Named;
    use std::collections::HashSet;
    use std::time::Instant;

    fn synthetic_refund_list<'a>(
        refund: &'a DomainRefundableSwap,
        in_flight: &'a HashMap<String, InFlightRefund>,
        balance: &'a Amount,
        quote: &'a Quote,
        image: &'a image::Handle,
        swap_ids: &'a HashSet<String>,
        mask_amounts: bool,
    ) -> Element<'a, Message> {
        liquid_transactions_view(
            &[],
            std::slice::from_ref(refund),
            in_flight,
            balance,
            None,
            false,
            BitcoinDisplayUnit::Sats,
            "synthetic-usdt",
            AssetFilter::All,
            false,
            quote,
            image,
            0,
            true,
            false,
            swap_ids,
            mask_amounts,
        )
    }

    fn payment(status: DomainPaymentStatus, details: DomainPaymentDetails) -> DomainPayment {
        DomainPayment {
            tx_id: None,
            destination: None,
            timestamp: 0,
            amount_sat: 9_000,
            fees_sat: 100,
            direction: DomainPaymentDirection::Receive,
            status,
            details,
        }
    }

    #[test]
    fn liquid_list_preserves_failure_and_fee_review_statuses() {
        for (state, expected) in [
            (DomainPaymentStatus::Pending, Some("Pending")),
            (DomainPaymentStatus::TimedOut, Some("Timed Out")),
            (DomainPaymentStatus::RefundPending, Some("Refund Pending")),
            (
                DomainPaymentStatus::WaitingFeeAcceptance,
                Some("Waiting Fee Acceptance"),
            ),
            (DomainPaymentStatus::Complete, Some("Confirmed")),
        ] {
            let row = payment(
                state,
                DomainPaymentDetails::Lightning {
                    swap_id: None,
                    description: String::new(),
                    payer_note: None,
                },
            );
            assert_eq!(
                payment_list_status(&row, &[]).map(|(label, _)| label),
                expected
            );
            if state == DomainPaymentStatus::Complete {
                assert_eq!(payment_detail_status(&row, &[]).0, "Confirmed");
            }
        }
    }

    #[test]
    fn failed_bitcoin_receive_still_refundable_is_labelled_refundable() {
        let address = "bc1qsyntheticrefund".to_string();
        let row = payment(
            DomainPaymentStatus::Failed,
            DomainPaymentDetails::OnChainBitcoin {
                swap_id: Some("synthetic-swap".into()),
                bitcoin_address: Some(address.clone()),
                description: String::new(),
                auto_accepted_fees: false,
                liquid_expiration_blockheight: 0,
                bitcoin_expiration_blockheight: 0,
                lockup_tx_id: Some("synthetic-lockup".into()),
                claim_tx_id: None,
                refund_tx_id: None,
                refund_tx_amount_sat: None,
            },
        );
        assert_eq!(
            payment_list_status(&row, std::slice::from_ref(&address)).map(|(label, _)| label),
            Some("Refundable")
        );
        assert_eq!(
            payment_detail_status(&row, std::slice::from_ref(&address)).0,
            "Refundable"
        );
    }

    #[test]
    fn completed_bitcoin_swap_has_visible_confirmed_status() {
        let row = payment(
            DomainPaymentStatus::Complete,
            DomainPaymentDetails::OnChainBitcoin {
                swap_id: Some("synthetic-swap".into()),
                bitcoin_address: Some("bc1qsyntheticcompleted".into()),
                description: String::new(),
                auto_accepted_fees: false,
                liquid_expiration_blockheight: 0,
                bitcoin_expiration_blockheight: 0,
                lockup_tx_id: Some("synthetic-lockup".into()),
                claim_tx_id: Some("synthetic-claim".into()),
                refund_tx_id: None,
                refund_tx_amount_sat: None,
            },
        );
        assert_eq!(
            payment_list_status(&row, &[]).map(|(label, _)| label),
            Some("Confirmed")
        );
        assert_eq!(payment_detail_status(&row, &[]).0, "Confirmed");
    }

    #[test]
    fn completed_direct_liquid_payment_has_visible_confirmed_status() {
        let row = payment(
            DomainPaymentStatus::Complete,
            DomainPaymentDetails::LiquidAsset {
                asset_id: "synthetic-asset".into(),
                asset_info: None,
                description: String::new(),
                payer_note: None,
            },
        );
        assert_eq!(
            payment_list_status(&row, &[]).map(|(label, _)| label),
            Some("Confirmed")
        );
        assert_eq!(payment_detail_status(&row, &[]).0, "Confirmed");
    }

    #[test]
    fn refundables_gated_out_of_usdt_filter() {
        // Contract guard: refundables are BTC→L-BTC swap refunds, so they
        // should only surface under All / L-BTC. Regression from prior bug
        // where they appeared under the USDt tab.
        for (filter, expected) in [
            (AssetFilter::All, true),
            (AssetFilter::LbtcOnly, true),
            (AssetFilter::UsdtOnly, false),
        ] {
            let show = refundables_visible(1, filter);
            assert_eq!(
                show, expected,
                "refundables visibility for filter {:?}",
                filter
            );
        }
        assert!(!refundables_visible(0, AssetFilter::All));
    }

    #[test]
    fn zero_reported_lbtc_fee_does_not_claim_usdt_fee_payment() {
        assert_eq!(usdt_fee_caption(0), "No L-BTC fee reported");
        assert_eq!(usdt_fee_caption(42), "Fees (L-BTC)");
    }

    #[test]
    fn refund_status_distinguishes_submission_and_broadcast() {
        assert_eq!(refund_status(false, None), "This swap can be refunded");
        assert_eq!(refund_status(true, None), "Submitting refund…");
        let pending = InFlightRefund {
            refund_txid: None,
            submitted_at: Instant::now(),
        };
        assert_eq!(refund_status(true, Some(&pending)), "Submitting refund…");
        assert_eq!(refund_status(false, Some(&pending)), "Submitting refund…");
        let broadcast = InFlightRefund {
            refund_txid: Some("synthetic-refund-txid".to_owned()),
            submitted_at: Instant::now(),
        };
        assert!(refund_status(false, Some(&broadcast)).starts_with("Refund broadcast · "));
    }

    #[test]
    fn refund_button_requires_valid_address_and_positive_fee() {
        let mut address = form::Value::default();
        let mut fee = form::Value::default();
        address.value = "bc1qsyntheticrefund".to_owned();
        fee.value = "2".to_owned();
        assert!(refund_can_submit(&address, &fee, false, false));
        assert!(!refund_can_submit(&address, &fee, true, false));
        assert!(!refund_can_submit(&address, &fee, false, true));
        address.valid = false;
        assert!(!refund_can_submit(&address, &fee, false, false));
        address.valid = true;
        fee.value = "0".to_owned();
        assert!(!refund_can_submit(&address, &fee, false, false));
        fee.value = "not a fee".to_owned();
        assert!(!refund_can_submit(&address, &fee, false, false));
    }

    #[test]
    fn liquid_refund_capture_matrix() {
        // The production presenters are mounted by the shared software
        // renderer. R06A_CAPTURE_DIR optionally writes content-only PNGs.
        let refund = DomainRefundableSwap {
            swap_address: "bc1qsyntheticrefundaddressonly".to_owned(),
            timestamp: 1_728_000_000,
            amount_sat: 24_869,
        };
        let amount = Amount::from_sat(refund.amount_sat);
        let exact_amount = amount.to_formatted_string_with_unit(BitcoinDisplayUnit::Sats);
        let balance = Amount::ZERO;
        let quote = quote_display::random_quote("empty-wallet");
        let image = quote_display::image_handle_for_context("empty-wallet");
        let swap_ids = HashSet::new();
        let idle = HashMap::new();
        let submitting_entry = InFlightRefund {
            refund_txid: None,
            submitted_at: Instant::now(),
        };
        let submitting = HashMap::from([(refund.swap_address.clone(), submitting_entry.clone())]);
        let broadcast_entry = InFlightRefund {
            refund_txid: Some("synthetic-refund-txid-1234567890".to_owned()),
            submitted_at: Instant::now(),
        };
        let broadcast = HashMap::from([(refund.swap_address.clone(), broadcast_entry.clone())]);
        let address = form::Value {
            value: "bc1qsyntheticdestinationonly".to_owned(),
            warning: None,
            valid: true,
        };
        let fee = form::Value {
            value: "2".to_owned(),
            warning: None,
            valid: true,
        };
        let invalid_fee = form::Value {
            value: "0".to_owned(),
            warning: None,
            valid: true,
        };
        macro_rules! detail {
            ($fee:expr, $refunding:expr, $in_flight:expr, $mask_amounts:expr) => {
                refundable_detail_view(
                    crate::chain::ChainId::Bitcoin,
                    &refund,
                    None,
                    BitcoinDisplayUnit::Sats,
                    &address,
                    $fee,
                    $refunding,
                    None,
                    $in_flight,
                    true,
                    $mask_amounts,
                )
            };
        }

        for (appearance, theme) in [
            ("dark", theme::Theme::dark()),
            ("light", theme::Theme::light()),
        ] {
            for outer in [(1024, 768), (1440, 900)] {
                let list = capture::inspect(
                    "liquid-refund-list",
                    synthetic_refund_list(
                        &refund, &idle, &balance, &quote, &image, &swap_ids, false,
                    ),
                    outer,
                    appearance,
                    theme,
                );
                assert!(list.contains("Refundable Transactions"));
                assert!(list.contains(&exact_amount));
                assert!(list.has_focus(Id::new("liquid-transactions-filter-usdt")));

                let hidden = capture::inspect(
                    "liquid-refund-list-hidden",
                    synthetic_refund_list(
                        &refund, &idle, &balance, &quote, &image, &swap_ids, true,
                    ),
                    outer,
                    appearance,
                    theme,
                );
                assert!(hidden.contains("••••••"));
                assert!(!hidden.contains(&exact_amount));

                let idle_detail = capture::inspect(
                    "liquid-refund-detail-idle",
                    detail!(&fee, false, None, true),
                    outer,
                    appearance,
                    theme,
                );
                assert!(idle_detail.contains("This swap can be refunded"));
                assert!(idle_detail.contains(&format!("Refund amount: {exact_amount}")));
                assert!(idle_detail.contains("••••••"));
                assert!(idle_detail.has_focus(Id::new(REFUND_BACK_ACTION_ID)));
                assert!(idle_detail.has_focus(Id::new(REFUND_SUBMIT_ACTION_ID)));
                assert!(idle_detail.has_focus(Id::new("liquid-refund-fee-high")));

                let pending_detail = capture::inspect(
                    "liquid-refund-detail-submitting",
                    detail!(&fee, true, Some(&submitting_entry), false),
                    outer,
                    appearance,
                    theme,
                );
                assert!(pending_detail.contains("Submitting refund…"));
                assert!(pending_detail.contains("Refund Details"));
                assert!(pending_detail.contains("Refund…"));
                assert!(!pending_detail.contains("Refund broadcast"));
                assert!(!pending_detail.has_focus(Id::new(REFUND_SUBMIT_ACTION_ID)));

                let broadcast_detail = capture::inspect(
                    "liquid-refund-detail-broadcast",
                    detail!(&fee, false, Some(&broadcast_entry), false),
                    outer,
                    appearance,
                    theme,
                );
                assert!(broadcast_detail.contains("Refund broadcast"));
                assert!(!broadcast_detail.contains("Refund Details"));

                let pending_list = capture::inspect(
                    "liquid-refund-list-submitting",
                    synthetic_refund_list(
                        &refund,
                        &submitting,
                        &balance,
                        &quote,
                        &image,
                        &swap_ids,
                        false,
                    ),
                    outer,
                    appearance,
                    theme,
                );
                assert!(pending_list.contains("Submitting refund…"));
                let broadcast_list = capture::inspect(
                    "liquid-refund-list-broadcast",
                    synthetic_refund_list(
                        &refund, &broadcast, &balance, &quote, &image, &swap_ids, false,
                    ),
                    outer,
                    appearance,
                    theme,
                );
                assert!(broadcast_list.contains("Refund broadcast"));

                let disabled = capture::inspect(
                    "liquid-refund-detail-invalid-fee",
                    detail!(&invalid_fee, false, None, false),
                    outer,
                    appearance,
                    theme,
                );
                assert!(!disabled.has_focus(Id::new(REFUND_SUBMIT_ACTION_ID)));
                assert!(disabled.contains("Enter a valid Bitcoin address and fee rate"));

                for key in [Named::Enter, Named::Space] {
                    let filter = capture::activate_with_key(
                        synthetic_refund_list(
                            &refund, &idle, &balance, &quote, &image, &swap_ids, false,
                        ),
                        Id::new("liquid-transactions-filter-usdt"),
                        outer,
                        false,
                        key,
                    );
                    assert!(filter.iter().any(|message| matches!(
                        message,
                        Message::SetAssetFilter(AssetFilter::UsdtOnly)
                    )));
                    let priority = capture::activate_with_key(
                        detail!(&fee, false, None, false),
                        Id::new("liquid-refund-fee-high"),
                        outer,
                        false,
                        key,
                    );
                    assert!(priority.iter().any(|message| matches!(
                        message,
                        Message::RefundFeeratePrioritySelected(FeeratePriority::High)
                    )));
                    let back = capture::activate_with_key(
                        detail!(&fee, false, None, false),
                        Id::new(REFUND_BACK_ACTION_ID),
                        outer,
                        false,
                        key,
                    );
                    assert!(back.iter().any(|message| matches!(message, Message::Close)));
                    let submit = capture::activate_with_key(
                        detail!(&fee, false, None, false),
                        Id::new(REFUND_SUBMIT_ACTION_ID),
                        outer,
                        false,
                        key,
                    );
                    assert!(submit
                        .iter()
                        .any(|message| matches!(message, Message::SubmitRefund)));
                }
            }
        }
    }
    fn ordinary_payment_cases() -> Vec<(DomainPayment, bool)> {
        use crate::app::wallets::DomainLiquidAssetInfo;
        vec![
            (
                payment(
                    DomainPaymentStatus::Complete,
                    DomainPaymentDetails::Lightning {
                        swap_id: None,
                        description: "Synthetic payment".into(),
                        payer_note: None,
                    },
                ),
                false,
            ),
            (
                payment(
                    DomainPaymentStatus::Complete,
                    DomainPaymentDetails::LiquidAsset {
                        asset_id: "synthetic-lbtc".into(),
                        asset_info: None,
                        description: "Synthetic payment".into(),
                        payer_note: None,
                    },
                ),
                false,
            ),
            (
                payment(
                    DomainPaymentStatus::Complete,
                    DomainPaymentDetails::LiquidAsset {
                        asset_id: "synthetic-usdt".into(),
                        asset_info: Some(DomainLiquidAssetInfo {
                            amount_minor: 123_456_789,
                            precision: 6,
                        }),
                        description: "Synthetic payment".into(),
                        payer_note: None,
                    },
                ),
                true,
            ),
        ]
    }

    fn assert_ordinary_payment_case(payment: &DomainPayment, is_usdt: bool, masked: bool) {
        use crate::app::cache::FiatPriceRequest;
        use crate::services::fiat::{Currency, PriceSource};
        let converter = Some(
            FiatAmountConverter::new(
                100_000.0,
                None,
                FiatPriceRequest::new(PriceSource::Coincube, Currency::USD),
            )
            .unwrap(),
        );
        let row_view = || {
            transaction_row(
                7,
                payment,
                converter,
                BitcoinDisplayUnit::Sats,
                "synthetic-usdt",
                true,
                false,
                &[],
                masked,
            )
        };
        let detail_view = || {
            transaction_detail_view(
                payment,
                converter,
                BitcoinDisplayUnit::Sats,
                "synthetic-usdt",
                &[],
                masked,
            )
        };
        let row = capture::inspect_full_window(
            "liquid-ordinary-row",
            row_view(),
            (1440, 1000),
            "light",
            theme::Theme::light(),
        );
        let detail = capture::inspect_full_window(
            "liquid-ordinary-detail",
            detail_view(),
            (1440, 1000),
            "light",
            theme::Theme::light(),
        );
        assert!(row.contains("Synthetic payment"));
        assert!(row.contains("Confirmed"));
        assert!(detail.contains("Confirmed"));
        if masked {
            assert!(row.contains("••••••"));
            assert!(detail.contains("••••••"));
            for hidden in ["9,000", "9,100", "123.46", "current rate"] {
                assert!(!row.contains(hidden));
                assert!(!detail.contains(hidden));
            }
        } else if is_usdt {
            assert!(row.contains("123.46 USDt"));
            assert!(detail.contains("123.46"));
            assert!(detail.contains("USDt"));
            assert!(detail.contains("Fees (L-BTC)"));
            assert!(detail.text.iter().any(|(value, _)| value == "100"));
        } else {
            let headline = if payment.direction == DomainPaymentDirection::Send {
                "9,100"
            } else {
                "9,000"
            };
            assert!(row.contains(headline));
            assert!(detail.contains(headline));
            assert!(detail.contains("9,000"));
            assert!(detail.contains("Fees"));
            assert!(detail.text.iter().any(|(value, _)| value == "100"));
            assert!(row.contains("current rate"));
            assert!(detail.contains("current rate"));
        }
        let selection = capture::click_text(row_view(), "Synthetic payment", 0, (1440, 1000));
        assert!(selection
            .iter()
            .any(|message| matches!(message, Message::Select(7))));
        let back = capture::click_text(detail_view(), "< Back", 0, (1440, 1000));
        assert!(back.iter().any(|message| matches!(message, Message::Close)));
    }

    #[test]
    fn ordinary_liquid_presenters_preserve_asset_cost_privacy_and_selection() {
        use crate::app::state::preferences::BalancePrivacy;
        for privacy in [
            BalancePrivacy::Shown,
            BalancePrivacy::Hidden,
            BalancePrivacy::Unresolved,
        ] {
            for direction in [
                DomainPaymentDirection::Receive,
                DomainPaymentDirection::Send,
            ] {
                for (mut payment, is_usdt) in ordinary_payment_cases() {
                    payment.direction = direction;
                    assert_ordinary_payment_case(&payment, is_usdt, privacy.masks_amounts());
                }
            }
        }
    }
}
