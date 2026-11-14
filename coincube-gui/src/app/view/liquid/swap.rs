//! Liquid cross-asset Swap view.
//!
//! Single-input screen (Aqua-style) plus a locked-quote review step.
//! The editable amount is the **receive** amount (the `to` asset); the
//! pay amount and fee are read-only quote output. See
//! [`crate::app::state::liquid::swap`] for the quote model.

use coincube_core::miniscript::bitcoin::Amount;
use coincube_ui::locale::copy as localized_copy;
use coincube_ui::locale::Text as CopyText;
use coincube_ui::{
    color,
    component::{
        amount::{format_f64_as_string, BitcoinDisplayUnit, DisplayAmount},
        button, form,
        text::*,
        transaction::{TransactionDirection, TransactionListItem},
    },
    icon::{left_right_icon, warning_icon},
    image::asset_network_logo,
    theme,
    widget::{action::Action, Column, Container, Element, Row},
};
use iced::{
    widget::{button as iced_button, container, Space},
    Alignment, Background, Length,
};

use crate::app::breez_liquid::assets::format_usdt_display;
use crate::app::state::liquid::send::SendAsset;
use crate::app::state::liquid::swap::{SwapPhase, SwapQuote};
use crate::app::view::{FiatAmountConverter, LiquidSwapMessage};
use crate::app::wallets::{DomainPayment, DomainPaymentDetails, DomainPaymentStatus};
use crate::utils::format_time_ago;

/// Inputs the Swap view needs to render.
pub struct LiquidSwapConfig<'a> {
    pub phase: SwapPhase,
    pub from_asset: SendAsset,
    pub to_asset: SendAsset,
    pub btc_balance: Amount,
    pub usdt_balance: u64,
    /// "You pay" (from-asset) input.
    pub pay_input: &'a form::Value<String>,
    /// "You receive" (to-asset) input.
    pub receive_input: &'a form::Value<String>,
    pub quote: Option<&'a SwapQuote>,
    /// Latest known rate (`to` per `from`), from a quote or the background
    /// probe — drives the rate chip even before the user enters an amount.
    pub rate: Option<f64>,
    pub quoting: bool,
    pub quote_remaining: u32,
    /// Whether Continue (advance to review) is enabled.
    pub quote_actionable: bool,
    /// Whether Confirm (execute) is enabled — also requires a synced wallet.
    pub confirm_enabled: bool,
    pub is_sending: bool,
    /// Whether the Liquid wallet is still catching up. While true the swap
    /// would fail server-side (its inputs aren't ready), so Confirm is paused.
    pub syncing: bool,
    /// Whether the last wallet sync failed. Confirm stays paused, but the
    /// banner reflects the failure rather than implying progress.
    pub sync_failed: bool,
    /// Paying USDt with zero L-BTC — the swap can't fund the Liquid network
    /// fee (paid in L-BTC), so we warn up front.
    pub needs_lbtc_for_fees: bool,
    /// Display unit for L-BTC amounts (BTC vs SATS). USDt always renders
    /// as a decimal regardless of this setting.
    pub bitcoin_unit: BitcoinDisplayUnit,
    pub error: Option<&'a str>,
    pub sent_amount_display: &'a str,
    pub sent_quote: &'a coincube_ui::component::quote_display::Quote,
    pub sent_image_handle: &'a iced::widget::image::Handle,
    /// Recent swap-leg payments (rendered as "Last swaps", selectable into
    /// the Transactions detail), newest first.
    pub recent_swaps: &'a [DomainPayment],
    pub usdt_id: &'a str,
    pub fiat_converter: Option<FiatAmountConverter>,
    pub mask_amounts: bool,
}

fn ticker(asset: SendAsset) -> &'static str {
    match asset {
        SendAsset::Lbtc => "L-BTC",
        SendAsset::Usdt => "USDt",
    }
}

/// 8-dp base-unit balance for an asset.
fn balance_base(asset: SendAsset, btc_balance: Amount, usdt_balance: u64) -> u64 {
    match asset {
        SendAsset::Lbtc => btc_balance.to_sat(),
        SendAsset::Usdt => usdt_balance,
    }
}

/// Format a USDt base-unit amount for display (2-dp, matching the rest
/// of the app — USDt is a stablecoin shown to the cent).
fn fmt_usdt(base: u64) -> String {
    format_usdt_display(base)
}

/// The unit suffix shown after an L-BTC amount, per the user's setting.
fn lbtc_unit_label(unit: BitcoinDisplayUnit) -> &'static str {
    match unit {
        BitcoinDisplayUnit::BTC => "BTC",
        BitcoinDisplayUnit::Sats => "SATS",
    }
}

/// Format an asset base-unit amount with its unit. L-BTC honours the
/// user's BTC/SATS preference (matching the rest of the app); USDt is
/// always a decimal.
fn fmt_amount(asset: SendAsset, base: u64, unit: BitcoinDisplayUnit) -> String {
    match asset {
        SendAsset::Lbtc => format!(
            "{} {}",
            Amount::from_sat(base).to_formatted_string_with_unit(unit),
            lbtc_unit_label(unit)
        ),
        SendAsset::Usdt => format!("{} USDt", fmt_usdt(base)),
    }
}

/// Format the `to`-side of a rate ("1 from = …"). USDt is kept concise;
/// L-BTC honours the BTC/SATS preference.
fn fmt_rate_value(to_asset: SendAsset, rate: f64, unit: BitcoinDisplayUnit) -> String {
    match to_asset {
        SendAsset::Usdt => {
            let v = if rate >= 1.0 {
                format_f64_as_string(rate, ",", 2, false)
            } else if rate > 0.0 {
                format_f64_as_string(rate, ",", 8, false)
            } else {
                "—".to_string()
            };
            format!("{v} USDt")
        }
        // `rate` is `to`-base per `from`-base; per 1 whole `from` (1e8 base)
        // that's `rate * 1e8` `to`-base units.
        SendAsset::Lbtc => fmt_amount(SendAsset::Lbtc, (rate * 1e8).round() as u64, unit),
    }
}

fn asset_slug(asset: SendAsset) -> &'static str {
    match asset {
        SendAsset::Lbtc => "lbtc",
        SendAsset::Usdt => "usdt",
    }
}

const FLIP_ACTION_ID: &str = "tenshu.liquid.swap.flip";
const SWAP_ALL_ACTION_ID: &str = "tenshu.liquid.swap.all";
const CONTINUE_ACTION_ID: &str = "tenshu.liquid.swap.continue";

/// A rounded filled card (matches the Liquid → Send "YOU SEND" cards).
fn card<'a>(
    content: impl Into<Element<'a, LiquidSwapMessage>>,
) -> Container<'a, LiquidSwapMessage> {
    Container::new(content)
        .padding(16)
        .width(Length::Fill)
        .style(theme::card::simple)
}

/// Asset badge above the amount, as in the Liquid swap artboard.
fn asset_badge<'a>(asset: SendAsset) -> Element<'a, LiquidSwapMessage> {
    Row::new()
        .spacing(8)
        .align_y(Alignment::Center)
        .push(asset_network_logo(asset_slug(asset), "liquid", 22.0))
        .push(text(ticker(asset)).size(P1_SIZE).bold())
        .into()
}

/// "Balance: …" line shown under each card's amount.
fn balance_row<'a>(
    asset: SendAsset,
    base: u64,
    unit: BitcoinDisplayUnit,
    mask_amounts: bool,
) -> Element<'a, LiquidSwapMessage> {
    Row::new()
        .spacing(6)
        .push(
            text(localized_copy("Balance:"))
                .size(P2_SIZE)
                .style(theme::text::secondary),
        )
        .push(
            text(if mask_amounts {
                "••••••".into()
            } else {
                fmt_amount(asset, base, unit)
            })
            .size(P2_SIZE)
            .style(theme::text::secondary),
        )
        .into()
}

/// Circular ⇄ flip button between the two (horizontal) cards.
fn flip_button<'a>() -> Element<'a, LiquidSwapMessage> {
    Action::new(
        FLIP_ACTION_ID,
        Container::new(left_right_icon().size(18).color(color::ORANGE))
            .width(Length::Fixed(40.0))
            .height(Length::Fixed(40.0))
            .center_x(Length::Fixed(40.0))
            .center_y(Length::Fixed(40.0)),
        LiquidSwapMessage::FocusAction,
    )
    .padding(0)
    .focus_radius(8.0)
    .on_press(LiquidSwapMessage::FlipAssets)
    .style(|t: &theme::Theme, _| iced_button::Style {
        background: Some(Background::Color(t.colors.shell.background)),
        border: iced::Border {
            color: t.colors.shell.line,
            width: 1.0,
            radius: 8.0.into(),
        },
        ..Default::default()
    })
    .into()
}

/// One asset card: label, asset, amount, then balance and optional action.
fn asset_card<'a>(
    header: &'a str,
    header_accessory: Option<Element<'a, LiquidSwapMessage>>,
    amount: Element<'a, LiquidSwapMessage>,
    asset: SendAsset,
    balance_base_units: u64,
    unit: BitcoinDisplayUnit,
    mask_amounts: bool,
) -> Element<'a, LiquidSwapMessage> {
    let mut balance = Row::new()
        .align_y(Alignment::Center)
        .push(balance_row(asset, balance_base_units, unit, mask_amounts))
        .push(Space::new().width(Length::Fill));
    if let Some(accessory) = header_accessory {
        balance = balance.push(accessory);
    }

    card(
        Column::new()
            .spacing(10)
            .push(text(header).size(P2_SIZE).style(theme::text::secondary))
            .push(asset_badge(asset))
            .push(amount)
            .push(balance),
    )
    .into()
}

/// An editable amount field whose input mode matches the asset/unit:
/// whole sats for L-BTC in SATS mode, decimal BTC for L-BTC in BTC mode,
/// decimal for USDt.
fn amount_input_field<'a>(
    value: &'a form::Value<String>,
    asset: SendAsset,
    unit: BitcoinDisplayUnit,
    on_edit: impl Fn(String) -> LiquidSwapMessage + 'static + Clone,
) -> Element<'a, LiquidSwapMessage> {
    let form = match (asset, unit) {
        (SendAsset::Lbtc, BitcoinDisplayUnit::Sats) => {
            form::Form::new_amount_sats("0", value, on_edit)
        }
        (SendAsset::Lbtc, BitcoinDisplayUnit::BTC) => {
            form::Form::new_amount_btc("0.00000000", value, on_edit)
        }
        (SendAsset::Usdt, _) => form::Form::new_amount_numeric("0.00", value, on_edit),
    };
    form.maybe_warning(value.warning.map(coincube_ui::locale::copy))
        .size(H3_SIZE)
        .padding(10)
        .into_container()
        .into()
}

/// "Last swaps" list — swap-leg payments rendered with the same
/// `TransactionListItem` as Overview/Transactions, clickable into the
/// Transactions detail. Empty when there are no recorded swaps.
fn last_swaps_section<'a>(config: &LiquidSwapConfig<'a>) -> Option<Element<'a, LiquidSwapMessage>> {
    if config.recent_swaps.is_empty() {
        return None;
    }
    let mut col = Column::new()
        .spacing(10)
        .push(h4_bold(localized_copy("Last swaps")));
    for (idx, payment) in config.recent_swaps.iter().enumerate() {
        col = col.push(swap_leg_row(idx, payment, config));
    }
    Some(col.into())
}

/// A single swap-leg row, mirroring the Transactions row but labelled
/// "Swap" and selecting into the Transactions detail.
fn swap_leg_row<'a>(
    idx: usize,
    payment: &'a DomainPayment,
    config: &LiquidSwapConfig<'a>,
) -> Element<'a, LiquidSwapMessage> {
    let unit = config.bitcoin_unit;
    let is_incoming = payment.is_incoming();

    // USDt leg → its asset amount; otherwise the L-BTC (sat) amount.
    let usdt_minor = match &payment.details {
        DomainPaymentDetails::LiquidAsset {
            asset_id,
            asset_info,
            ..
        } if !config.usdt_id.is_empty() && asset_id == config.usdt_id => {
            asset_info.as_ref().map(|i| i.amount_minor)
        }
        _ => None,
    };
    let is_usdt = usdt_minor.is_some();

    let amount = match usdt_minor {
        Some(minor) => Amount::from_sat(minor),
        None if is_incoming => Amount::from_sat(payment.amount_sat),
        None => Amount::from_sat(payment.amount_sat) + Amount::from_sat(payment.fees_sat),
    };

    let direction = if is_incoming {
        TransactionDirection::Incoming
    } else {
        TransactionDirection::Outgoing
    };

    let icon = if is_usdt {
        asset_network_logo("usdt", "liquid", 40.0)
    } else {
        asset_network_logo("lbtc", "liquid", 40.0)
    };

    let mut item = TransactionListItem::new(direction, &amount, unit)
        .with_custom_icon(icon)
        .with_label("Swap".to_owned())
        .with_time_ago(format_time_ago(payment.timestamp.into()));

    if config.mask_amounts {
        item = item.with_amount_override("••••••".into());
    } else if is_usdt {
        item = item.with_amount_override(format!("{} USDt", format_usdt_display(amount.to_sat())));
    } else if let Some(fiat) = config.fiat_converter.as_ref().map(|c| c.convert(amount)) {
        item = item.with_fiat_amount(format!(
            "{} {}",
            fiat.to_formatted_string(),
            fiat.currency()
        ));
    }

    if matches!(payment.status, DomainPaymentStatus::Pending) {
        item = item.with_custom_status(swap_pending_badge());
    }

    item.view(LiquidSwapMessage::SelectSwap(idx)).into()
}

/// "Pending" status pill for unconfirmed swap legs (matches Overview /
/// Transactions).
fn swap_pending_badge() -> Element<'static, LiquidSwapMessage> {
    let fg = color::BLACK;
    Container::new(
        Row::new()
            .spacing(4)
            .align_y(Alignment::Center)
            .push(
                warning_icon()
                    .size(14)
                    .style(move |_| iced::widget::text::Style { color: Some(fg) }),
            )
            .push(
                text(localized_copy("Pending"))
                    .bold()
                    .size(14)
                    .style(move |_| iced::widget::text::Style { color: Some(fg) }),
            ),
    )
    .padding([2, 8])
    .style(move |_: &theme::Theme| container::Style {
        background: Some(Background::Color(color::GREY_3)),
        border: iced::Border {
            radius: 12.0.into(),
            ..Default::default()
        },
        ..Default::default()
    })
    .into()
}

/// The swap input screen — editable "You pay" and "You receive" cards.
fn input_screen<'a>(config: &LiquidSwapConfig<'a>) -> Element<'a, LiquidSwapMessage> {
    let from_balance = balance_base(config.from_asset, config.btc_balance, config.usdt_balance);
    let to_balance = balance_base(config.to_asset, config.btc_balance, config.usdt_balance);

    // ── You pay (from) — editable ────────────────────────────────────────
    let pay_field = amount_input_field(
        config.pay_input,
        config.from_asset,
        config.bitcoin_unit,
        LiquidSwapMessage::AmountEditedPay,
    );

    let swap_all = Action::new(
        SWAP_ALL_ACTION_ID,
        text(localized_copy("Swap All"))
            .size(P2_SIZE)
            .bold()
            .style(|t: &theme::Theme| iced::widget::text::Style {
                color: Some(t.colors.shell.accent_ink),
            }),
        LiquidSwapMessage::FocusAction,
    )
    .padding(2)
    .on_press(LiquidSwapMessage::SwapAll)
    .style(theme::button::transparent)
    .into();

    let from_card = asset_card(
        coincube_ui::locale::copy("YOU PAY"),
        Some(swap_all),
        pay_field,
        config.from_asset,
        from_balance,
        config.bitcoin_unit,
        config.mask_amounts,
    );

    // ── Rate chip + direction control ────────────────────────────────────
    let rate_chip: Element<'a, LiquidSwapMessage> = match config.rate {
        Some(rate) => Container::new(
            text(format!(
                "1 {} = {}",
                ticker(config.from_asset),
                fmt_rate_value(config.to_asset, rate, config.bitcoin_unit)
            ))
            .size(P2_SIZE)
            .style(theme::text::primary),
        )
        .padding([6, 12])
        .style(|t: &theme::Theme| container::Style {
            border: iced::Border {
                color: t.colors.shell.line,
                width: 1.0,
                radius: 16.0.into(),
            },
            ..Default::default()
        })
        .into(),
        None => Space::new().width(Length::Fixed(0.0)).into(),
    };

    // ── You receive (to) — editable ──────────────────────────────────────
    let receive_field = amount_input_field(
        config.receive_input,
        config.to_asset,
        config.bitcoin_unit,
        LiquidSwapMessage::AmountEditedReceive,
    );

    let to_card = asset_card(
        coincube_ui::locale::copy("YOU RECEIVE"),
        None,
        receive_field,
        config.to_asset,
        to_balance,
        config.bitcoin_unit,
        config.mask_amounts,
    );

    // ── Fee / status line ────────────────────────────────────────────────
    let mut status = Row::new().spacing(16).align_y(Alignment::Center);
    if let Some(q) = config.quote {
        status = status.push(
            Row::new()
                .spacing(6)
                .push(
                    text(localized_copy("Fee:"))
                        .size(P2_SIZE)
                        .style(theme::text::secondary),
                )
                .push(
                    text(fmt_amount(
                        config.from_asset,
                        q.fee_base,
                        config.bitcoin_unit,
                    ))
                    .size(P2_SIZE)
                    .style(theme::text::secondary),
                ),
        );
    } else if config.quoting {
        status = status.push(
            text(localized_copy("Fetching quote…"))
                .size(P2_SIZE)
                .style(theme::text::secondary),
        );
    }

    let continue_btn = Action::new(
        CONTINUE_ACTION_ID,
        text(localized_copy("Continue"))
            .size(P1_SIZE)
            .font(coincube_ui::font::MEDIUM)
            .width(Length::Fill)
            .align_x(Alignment::Center),
        LiquidSwapMessage::FocusAction,
    )
    .padding([10, 16])
    .style(theme::button::primary)
    .width(Length::Fixed(240.0))
    .on_press_maybe(
        config
            .quote_actionable
            .then_some(LiquidSwapMessage::Continue),
    );

    // ── Horizontal pay / direction / receive cards ───────────────────────
    let cards_row = Row::new()
        .spacing(12)
        .align_y(Alignment::Start)
        .width(Length::Fill)
        .push(Container::new(from_card).width(Length::FillPortion(1)))
        .push(
            Column::new()
                .push(Space::new().height(Length::Fixed(56.0)))
                .push(flip_button()),
        )
        .push(Container::new(to_card).width(Length::FillPortion(1)));

    let mut content = Column::new().spacing(16).width(Length::Fill);
    if config.syncing {
        content = content.push(hint_banner(coincube_ui::locale::copy(
            "Wallet is still syncing — swaps are paused until it finishes.",
        )));
    } else if config.sync_failed {
        content = content.push(hint_banner(coincube_ui::locale::copy(
            "Couldn't sync the wallet — swaps are paused. It'll retry automatically.",
        )));
    }
    if config.needs_lbtc_for_fees {
        content = content.push(hint_banner(coincube_ui::locale::copy(
            "You need a little L-BTC to pay network fees for this swap. Receive some L-BTC first.",
        )));
    }
    content = content
        .push(cards_row)
        .push(
            Container::new(
                Row::new()
                    .spacing(16)
                    .align_y(Alignment::Center)
                    .push(rate_chip)
                    .push(status),
            )
            .width(Length::Fill)
            .center_x(Length::Fill),
        )
        .push(
            Container::new(continue_btn)
                .width(Length::Fill)
                .center_x(Length::Fill),
        );

    if let Some(err) = config.error {
        content = content.push(error_card(err));
    }

    if let Some(last_swaps) = last_swaps_section(config) {
        content = content
            .push(Space::new().height(Length::Fixed(8.0)))
            .push(last_swaps);
    }

    Container::new(content).width(Length::Fill).into()
}

/// Locked-quote review + confirm step.
fn review_screen<'a>(config: &LiquidSwapConfig<'a>) -> Element<'a, LiquidSwapMessage> {
    let Some(q) = config.quote else {
        // No live quote on the review screen — either it expired or the
        // previous swap failed and we're fetching a fresh one. Surface the
        // error (if any) so a failed swap isn't silent.
        let mut content = Column::new()
            .spacing(16)
            .max_width(680)
            .push(h4_bold(localized_copy("Review swap")));
        if let Some(err) = config.error {
            content = content.push(error_card(err));
        }
        let status = if config.quoting {
            localized_copy("Fetching a fresh quote…")
        } else {
            localized_copy("Quote expired.")
        };
        content = content
            .push(text(status).size(P1_SIZE).style(theme::text::secondary))
            .push(
                button::secondary(None, CopyText::Back.get())
                    .on_press(LiquidSwapMessage::BackToInput)
                    .width(Length::Fixed(120.0)),
            );
        return Container::new(content)
            .width(Length::Fill)
            .width(Length::Fill)
            .into();
    };

    let line = |label: &'a str, value: String| -> Element<'a, LiquidSwapMessage> {
        Container::new(
            Row::new()
                .push(text(label).size(P1_SIZE).style(theme::text::secondary))
                .push(Space::new().width(Length::Fill))
                .push(text(value).size(P1_SIZE).bold()),
        )
        .padding([12, 0])
        .width(Length::Fill)
        .into()
    };
    let rule = || -> Element<'a, LiquidSwapMessage> {
        Container::new(Space::new().height(Length::Fixed(1.0)))
            .width(Length::Fill)
            .style(|t: &theme::Theme| container::Style {
                background: Some(Background::Color(t.colors.shell.line)),
                ..Default::default()
            })
            .into()
    };

    let summary = Column::new()
        .width(Length::Fill)
        .push(rule())
        .push(line(CopyText::Network.get(), "Liquid".to_owned()))
        .push(rule())
        .push(line(
            localized_copy("You pay"),
            fmt_amount(config.from_asset, q.from_total_base(), config.bitcoin_unit),
        ))
        .push(rule())
        .push(line(
            localized_copy("You receive"),
            fmt_amount(config.to_asset, q.receiver_base, config.bitcoin_unit),
        ))
        .push(rule())
        .push(line(
            localized_copy("Rate"),
            format!(
                "1 {} = {}",
                ticker(config.from_asset),
                fmt_rate_value(config.to_asset, q.rate_to_per_from(), config.bitcoin_unit)
            ),
        ))
        .push(rule())
        .push(line(
            localized_copy("SideSwap + network fee"),
            fmt_amount(config.from_asset, q.fee_base, config.bitcoin_unit),
        ))
        .push(rule())
        .push(line(
            localized_copy("Quote expires in"),
            if config.is_sending {
                "processing…".to_string()
            } else {
                format!("{}s", config.quote_remaining)
            },
        ))
        .push(rule());

    let confirm_btn = {
        let label = if config.is_sending {
            localized_copy("Swapping…")
        } else if config.syncing {
            localized_copy("Waiting for sync…")
        } else if config.sync_failed {
            localized_copy("Sync failed")
        } else {
            localized_copy("Confirm swap")
        };
        let mut b = button::primary(None, label).width(Length::Fixed(180.0));
        // Disabled for an expired quote, a send in flight, or a wallet that
        // hasn't finished syncing (the swap would fail server-side).
        if config.confirm_enabled {
            b = b.on_press(LiquidSwapMessage::Confirm);
        }
        b
    };

    let back_btn = button::secondary(None, CopyText::Back.get())
        .on_press(LiquidSwapMessage::BackToInput)
        .width(Length::Fixed(120.0));

    let mut content = Column::new()
        .spacing(16)
        .max_width(680)
        .push(h4_bold(localized_copy("Review swap")));
    if config.syncing {
        content = content.push(hint_banner(coincube_ui::locale::copy(
            "Wallet is still syncing — swaps are paused until it finishes.",
        )));
    } else if config.sync_failed {
        content = content.push(hint_banner(coincube_ui::locale::copy(
            "Couldn't sync the wallet — swaps are paused. It'll retry automatically.",
        )));
    }
    content = content.push(summary).push(
        Row::new()
            .align_y(Alignment::Center)
            .push(back_btn)
            .push(Space::new().width(Length::Fill))
            .push(confirm_btn),
    );

    if let Some(err) = config.error {
        content = content.push(error_card(err));
    }

    Container::new(content)
        .width(Length::Fill)
        .width(Length::Fill)
        .into()
}

/// Non-blocking orange banner with a warning icon.
fn hint_banner<'a>(message: &'a str) -> Element<'a, LiquidSwapMessage> {
    Container::new(
        Row::new()
            .spacing(8)
            .align_y(Alignment::Center)
            .push(warning_icon().size(16).color(color::ORANGE))
            .push(text(message).size(P2_SIZE).style(theme::text::secondary)),
    )
    .padding([8, 12])
    .width(Length::Fill)
    .style(|t| container::Style {
        background: Some(Background::Color(t.colors.cards.simple.background)),
        border: iced::Border {
            color: color::ORANGE,
            width: 1.0,
            radius: 12.0.into(),
        },
        ..Default::default()
    })
    .into()
}

fn error_card(err: &str) -> Element<'_, LiquidSwapMessage> {
    Container::new(
        text(coincube_ui::locale::notice(err.to_string()))
            .size(14)
            .color(color::RED),
    )
    .padding(10)
    .style(theme::card::invalid)
    .width(Length::Fill)
    .into()
}

pub fn liquid_swap_view<'a>(config: LiquidSwapConfig<'a>) -> Element<'a, LiquidSwapMessage> {
    match config.phase {
        SwapPhase::Input => input_screen(&config),
        SwapPhase::Review => review_screen(&config),
        SwapPhase::Sent => {
            use coincube_ui::component::payment_result::{payment_result, Outcome};
            payment_result(
                Outcome::Confirming,
                config.sent_amount_display,
                "liquid-send",
                config.sent_quote,
                config.sent_image_handle,
                LiquidSwapMessage::Done,
            )
            .headline(CopyText::SwapSent.get())
            .into()
        }
    }
}

#[cfg(test)]
mod presentation_tests {
    use super::*;
    use crate::app::view::shared::landing::capture;
    use breez_sdk_liquid::prelude::{
        LiquidAddressData, Network, PrepareSendResponse, SendDestination,
    };
    use coincube_ui::component::quote_display;

    #[test]
    fn swap_input_and_review_fit_compact_and_wide_themes() {
        let pay = form::Value::<String>::default();
        let receive = form::Value::<String>::default();
        let sent_quote = quote_display::random_quote("synthetic-liquid-swap");
        let image = quote_display::image_handle_for_context("synthetic-liquid-swap");
        let quote = SwapQuote {
            prepare: PrepareSendResponse {
                destination: SendDestination::LiquidAddress {
                    address_data: LiquidAddressData {
                        address: "lq1synthetic-swap-fixture".into(),
                        network: Network::Bitcoin,
                        asset_id: None,
                        amount: None,
                        amount_sat: None,
                        label: None,
                        message: None,
                    },
                    bip353_address: None,
                },
                amount: None,
                fees_sat: Some(210),
                estimated_asset_fees: None,
                exchange_amount_sat: Some(5_790),
                disable_mrh: None,
                payment_timeout_sec: None,
            },
            receiver_base: 500_000_000,
            exchange_base: 5_790,
            fee_base: 210,
        };
        let render = |phase, actionable| {
            liquid_swap_view(LiquidSwapConfig {
                phase,
                from_asset: SendAsset::Lbtc,
                to_asset: SendAsset::Usdt,
                btc_balance: Amount::from_sat(12_784),
                usdt_balance: 1_037_000_000,
                pay_input: &pay,
                receive_input: &receive,
                quote: Some(&quote),
                rate: Some(82760.12),
                quoting: false,
                quote_remaining: if actionable { 24 } else { 0 },
                quote_actionable: actionable,
                confirm_enabled: actionable,
                is_sending: false,
                syncing: false,
                sync_failed: false,
                needs_lbtc_for_fees: false,
                bitcoin_unit: BitcoinDisplayUnit::Sats,
                error: None,
                sent_amount_display: "",
                sent_quote: &sent_quote,
                sent_image_handle: &image,
                recent_swaps: &[],
                usdt_id: "synthetic-usdt-id",
                fiat_converter: None,
                mask_amounts: false,
            })
        };
        for (width, height) in [(1024, 768), (1440, 900)] {
            for (appearance, theme) in [
                ("dark", theme::Theme::dark()),
                ("light", theme::Theme::light()),
            ] {
                for (phase, actionable, suffix) in [
                    (SwapPhase::Input, true, "input-ready"),
                    (SwapPhase::Input, false, "input-disabled"),
                    (SwapPhase::Review, true, "review-confirm"),
                    (SwapPhase::Review, false, "review-disabled"),
                ] {
                    let inspection = capture::inspect(
                        &format!("liquid-swap-{suffix}"),
                        render(phase, actionable),
                        (width, height),
                        appearance,
                        theme,
                    );
                    assert!(inspection.contains(if phase == SwapPhase::Input {
                        "YOU PAY"
                    } else {
                        "You pay"
                    }));
                    assert!(inspection.contains(if phase == SwapPhase::Input {
                        "YOU RECEIVE"
                    } else {
                        "You receive"
                    }));
                    assert!(inspection.contains(if phase == SwapPhase::Input {
                        "Continue"
                    } else {
                        "Confirm swap"
                    }));
                    assert!(inspection.contains(if phase == SwapPhase::Input {
                        "82,760.12 USDt"
                    } else {
                        "83,333.33 USDt"
                    }));
                    if phase == SwapPhase::Review {
                        assert!(inspection.contains("Back"));
                    } else {
                        for id in [FLIP_ACTION_ID, SWAP_ALL_ACTION_ID] {
                            assert!(inspection.has_focus(iced::advanced::widget::Id::new(id)));
                        }
                        assert_eq!(
                            inspection
                                .has_focus(iced::advanced::widget::Id::new(CONTINUE_ACTION_ID)),
                            actionable
                        );
                        let label_y = |label| {
                            inspection
                                .text
                                .iter()
                                .find(|(value, _)| value == label)
                                .expect("swap card heading")
                                .1
                                .y
                        };
                        assert!(
                            (label_y("YOU PAY") - label_y("YOU RECEIVE")).abs() <= 1.0,
                            "swap cards should share a top edge"
                        );
                    }
                }
            }
        }
        for (id, expected) in [
            (FLIP_ACTION_ID, LiquidSwapMessage::FlipAssets),
            (SWAP_ALL_ACTION_ID, LiquidSwapMessage::SwapAll),
            (CONTINUE_ACTION_ID, LiquidSwapMessage::Continue),
        ] {
            for key in [
                iced::keyboard::key::Named::Enter,
                iced::keyboard::key::Named::Space,
            ] {
                let messages = capture::activate_with_key(
                    render(SwapPhase::Input, true),
                    iced::advanced::widget::Id::new(id),
                    (1024, 768),
                    false,
                    key,
                );
                assert!(messages.iter().any(|message| {
                    std::mem::discriminant(message) == std::mem::discriminant(&expected)
                }));
            }
        }
    }
}

#[cfg(test)]
mod privacy_tests {
    use super::*;
    use crate::app::view::shared::landing::capture;
    use crate::app::wallets::{DomainLiquidAssetInfo, DomainPaymentDirection};
    use coincube_ui::component::quote_display;

    #[test]
    fn swap_passive_balances_and_recent_legs_follow_global_mask() {
        let input = form::Value::<String>::default();
        let quote = quote_display::random_quote("empty-wallet");
        let image = quote_display::image_handle_for_context("empty-wallet");
        let payments = vec![
            DomainPayment {
                tx_id: Some("privacy-btc".into()),
                destination: None,
                timestamp: 1_700_000_000,
                amount_sat: 98_765_432,
                fees_sat: 0,
                direction: DomainPaymentDirection::Receive,
                status: DomainPaymentStatus::Complete,
                details: DomainPaymentDetails::LiquidAsset {
                    asset_id: "lbtc".into(),
                    asset_info: None,
                    description: "Swap".into(),
                    payer_note: None,
                },
            },
            DomainPayment {
                tx_id: Some("privacy-usdt".into()),
                destination: None,
                timestamp: 1_700_000_000,
                amount_sat: 246_813_000_000,
                fees_sat: 0,
                direction: DomainPaymentDirection::Receive,
                status: DomainPaymentStatus::Complete,
                details: DomainPaymentDetails::LiquidAsset {
                    asset_id: "usdt".into(),
                    asset_info: Some(DomainLiquidAssetInfo {
                        amount_minor: 246_813_000_000,
                        precision: 8,
                    }),
                    description: "Swap".into(),
                    payer_note: None,
                },
            },
        ];
        let render = |mask_amounts| {
            liquid_swap_view(LiquidSwapConfig {
                phase: SwapPhase::Input,
                from_asset: SendAsset::Lbtc,
                to_asset: SendAsset::Usdt,
                btc_balance: Amount::from_sat(12_345_678),
                usdt_balance: 135_791_113,
                pay_input: &input,
                receive_input: &input,
                quote: None,
                rate: None,
                quoting: false,
                quote_remaining: 0,
                quote_actionable: false,
                confirm_enabled: false,
                is_sending: false,
                syncing: false,
                sync_failed: false,
                needs_lbtc_for_fees: false,
                bitcoin_unit: BitcoinDisplayUnit::Sats,
                error: None,
                sent_amount_display: "",
                sent_quote: &quote,
                sent_image_handle: &image,
                recent_swaps: &payments,
                usdt_id: "usdt",
                fiat_converter: None,
                mask_amounts,
            })
        };
        let hidden = capture::inspect(
            "liquid-swap-hidden",
            render(true),
            (1440, 1800),
            "dark",
            theme::Theme::dark(),
        );
        assert!(hidden.contains("••••••"));
        let usdt_balance = format!("{} USDt", format_usdt_display(135_791_113));
        let usdt_recent = format_usdt_display(246_813_000_000);
        for sentinel in [
            "12,345,678",
            "98,765,432",
            usdt_recent.as_str(),
            usdt_balance.as_str(),
        ] {
            assert!(
                !hidden.contains(sentinel),
                "hidden Liquid swap amount {}",
                sentinel
            );
        }
        let shown = capture::inspect(
            "liquid-swap-shown",
            render(false),
            (1440, 1800),
            "dark",
            theme::Theme::dark(),
        );
        assert!(shown.contains("12,345,678"));
        assert!(shown.contains("98,765,432"));
        assert!(shown.contains(&usdt_recent));
        assert!(shown.contains(&usdt_balance));
    }
}
