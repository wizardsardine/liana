//! Liquid cross-asset Swap view.
//!
//! Single-input screen (Aqua-style) plus a locked-quote review step.
//! The editable amount is the **receive** amount (the `to` asset); the
//! pay amount and fee are read-only quote output. See
//! [`crate::app::state::liquid::swap`] for the quote model.

use coincube_core::miniscript::bitcoin::Amount;
use coincube_ui::{
    color,
    component::{
        amount::{BitcoinDisplayUnit, DisplayAmount},
        button, form,
        text::*,
    },
    icon::{arrow_down_up_icon, warning_icon},
    image::asset_network_logo,
    theme,
    widget::{Column, Container, Element, Row},
};
use iced::{
    widget::{button as iced_button, container, Space},
    Alignment, Background, Length,
};

use crate::app::breez_liquid::assets::format_usdt_display;
use crate::app::state::liquid::send::SendAsset;
use crate::app::state::liquid::swap::{SwapPhase, SwapQuote};
use crate::app::view::LiquidSwapMessage;

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
                format!("{rate:.2}")
            } else if rate > 0.0 {
                format!("{rate:.8}")
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

/// A rounded filled card (matches the Liquid → Send "YOU SEND" cards).
fn card<'a>(
    content: impl Into<Element<'a, LiquidSwapMessage>>,
) -> Container<'a, LiquidSwapMessage> {
    Container::new(content)
        .padding(20)
        .width(Length::Fill)
        .style(theme::card::simple)
}

/// Small bordered "LIQUID" pill, as on the Send cards (both swap assets
/// live on the Liquid network).
fn liquid_pill<'a>() -> Element<'a, LiquidSwapMessage> {
    Container::new(text("LIQUID").size(11).color(color::ORANGE))
        .padding([2, 8])
        .style(|_: &theme::Theme| container::Style {
            border: iced::Border {
                color: color::ORANGE,
                width: 1.0,
                radius: 8.0.into(),
            },
            ..Default::default()
        })
        .into()
}

/// Asset badge: circular network logo + ticker, mirroring the Send cards.
fn asset_badge<'a>(asset: SendAsset) -> Element<'a, LiquidSwapMessage> {
    Row::new()
        .spacing(8)
        .align_y(Alignment::Center)
        .push(asset_network_logo(asset_slug(asset), "liquid", 32.0))
        .push(text(ticker(asset)).size(H4_SIZE).bold())
        .into()
}

/// "Balance: …" line shown under each card's amount.
fn balance_row<'a>(
    asset: SendAsset,
    base: u64,
    unit: BitcoinDisplayUnit,
) -> Element<'a, LiquidSwapMessage> {
    Row::new()
        .spacing(6)
        .push(text("Balance:").size(P2_SIZE).style(theme::text::secondary))
        .push(
            text(fmt_amount(asset, base, unit))
                .size(P2_SIZE)
                .style(theme::text::secondary),
        )
        .into()
}

/// Circular ⇅ flip button between the two cards (Aqua-style).
fn flip_button<'a>() -> Element<'a, LiquidSwapMessage> {
    iced_button(
        Container::new(arrow_down_up_icon().size(18).color(color::ORANGE))
            .width(Length::Fixed(40.0))
            .height(Length::Fixed(40.0))
            .center_x(Length::Fixed(40.0))
            .center_y(Length::Fixed(40.0)),
    )
    .padding(0)
    .on_press(LiquidSwapMessage::FlipAssets)
    .style(|t: &theme::Theme, _| iced_button::Style {
        background: Some(Background::Color(t.colors.cards.simple.background)),
        border: iced::Border {
            color: color::ORANGE,
            width: 1.0,
            radius: 20.0.into(),
        },
        ..Default::default()
    })
    .into()
}

/// One asset card: a header row, then an amount (left) beside the asset
/// badge (right), then the balance + LIQUID pill. Mirrors the Send cards.
fn asset_card<'a>(
    header: &'a str,
    header_accessory: Option<Element<'a, LiquidSwapMessage>>,
    amount: Element<'a, LiquidSwapMessage>,
    asset: SendAsset,
    balance_base_units: u64,
    unit: BitcoinDisplayUnit,
) -> Element<'a, LiquidSwapMessage> {
    let mut header_row = Row::new()
        .align_y(Alignment::Center)
        .push(text(header).size(P2_SIZE).style(theme::text::secondary))
        .push(Space::new().width(Length::Fill));
    if let Some(accessory) = header_accessory {
        header_row = header_row.push(accessory);
    }

    let amount_row = Row::new()
        .spacing(12)
        .align_y(Alignment::Center)
        .push(Container::new(amount).width(Length::Fill))
        .push(asset_badge(asset));

    let balance_row = Row::new()
        .align_y(Alignment::Center)
        .push(balance_row(asset, balance_base_units, unit))
        .push(Space::new().width(Length::Fill))
        .push(liquid_pill());

    card(
        Column::new()
            .spacing(14)
            .push(header_row)
            .push(amount_row)
            .push(balance_row),
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
    on_edit: impl Fn(String) -> LiquidSwapMessage + 'static,
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
    form.maybe_warning(value.warning)
        .size(H3_SIZE)
        .padding(10)
        .into_container()
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

    let swap_all = button::secondary(None, "Swap All")
        .on_press(LiquidSwapMessage::SwapAll)
        .width(Length::Fixed(110.0))
        .into();

    let from_card = asset_card(
        "YOU PAY",
        Some(swap_all),
        pay_field,
        config.from_asset,
        from_balance,
        config.bitcoin_unit,
    );

    // ── Rate chip + flip control (Aqua middle row) ───────────────────────
    let rate_chip: Element<'a, LiquidSwapMessage> = match config.rate {
        Some(rate) => Container::new(
            text(format!(
                "1 {} = {}",
                ticker(config.from_asset),
                fmt_rate_value(config.to_asset, rate, config.bitcoin_unit)
            ))
            .size(P2_SIZE)
            .color(color::ORANGE),
        )
        .padding([6, 12])
        .style(|_: &theme::Theme| container::Style {
            border: iced::Border {
                color: color::ORANGE,
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
        "YOU RECEIVE",
        None,
        receive_field,
        config.to_asset,
        to_balance,
        config.bitcoin_unit,
    );

    // ── Fee / status line ────────────────────────────────────────────────
    let mut status = Column::new().spacing(6);
    if let Some(q) = config.quote {
        status = status.push(
            Row::new()
                .spacing(6)
                .push(text("Fee:").size(P2_SIZE).style(theme::text::secondary))
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
            text("Fetching quote…")
                .size(P2_SIZE)
                .style(theme::text::secondary),
        );
    }

    let continue_btn = {
        let mut b = button::primary(None, "Continue").width(Length::Fill);
        if config.quote_actionable {
            b = b.on_press(LiquidSwapMessage::Continue);
        }
        b
    };

    // ── Horizontal "You pay → ⇅ → You receive" cards (Send/Receive style) ──
    let cards_row = Row::new()
        .spacing(12)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .push(Container::new(from_card).width(Length::FillPortion(1)))
        .push(flip_button())
        .push(Container::new(to_card).width(Length::FillPortion(1)));

    let mut content = Column::new().spacing(16).width(Length::Fill);
    if config.syncing {
        content = content.push(hint_banner(
            "Wallet is still syncing — swaps are paused until it finishes.",
        ));
    }
    if config.needs_lbtc_for_fees {
        content = content.push(hint_banner(
            "You need a little L-BTC to pay network fees for this swap. Receive some L-BTC first.",
        ));
    }
    content = content
        .push(cards_row)
        // Rate chip + fee, centered under the cards.
        .push(
            Container::new(rate_chip)
                .width(Length::Fill)
                .center_x(Length::Fill),
        )
        .push(
            Container::new(status)
                .width(Length::Fill)
                .center_x(Length::Fill),
        )
        .push(continue_btn);

    if let Some(err) = config.error {
        content = content.push(error_card(err));
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
            .max_width(480)
            .push(h4_bold("Review swap"));
        if let Some(err) = config.error {
            content = content.push(error_card(err));
        }
        let status = if config.quoting {
            "Fetching a fresh quote…"
        } else {
            "Quote expired."
        };
        content = content
            .push(text(status).size(P1_SIZE).style(theme::text::secondary))
            .push(
                button::secondary(None, "Back")
                    .on_press(LiquidSwapMessage::BackToInput)
                    .width(Length::Fill),
            );
        return Container::new(content)
            .width(Length::Fill)
            .center_x(Length::Fill)
            .into();
    };

    let line = |label: &'a str, value: String| -> Element<'a, LiquidSwapMessage> {
        Row::new()
            .push(text(label).size(P1_SIZE).style(theme::text::secondary))
            .push(Space::new().width(Length::Fill))
            .push(text(value).size(P1_SIZE).bold())
            .into()
    };

    let summary = card(
        Column::new()
            .spacing(12)
            .push(line(
                "You pay",
                fmt_amount(config.from_asset, q.from_total_base(), config.bitcoin_unit),
            ))
            .push(line(
                "You receive",
                fmt_amount(config.to_asset, q.receiver_base, config.bitcoin_unit),
            ))
            .push(line(
                "Rate",
                format!(
                    "1 {} = {}",
                    ticker(config.from_asset),
                    fmt_rate_value(config.to_asset, q.rate_to_per_from(), config.bitcoin_unit)
                ),
            ))
            .push(line(
                "SideSwap + network fee",
                fmt_amount(config.from_asset, q.fee_base, config.bitcoin_unit),
            ))
            .push(line(
                "Quote expires in",
                if config.is_sending {
                    "processing…".to_string()
                } else {
                    format!("{}s", config.quote_remaining)
                },
            )),
    );

    let confirm_btn = {
        let label = if config.is_sending {
            "Swapping…"
        } else if config.syncing {
            "Waiting for sync…"
        } else {
            "Confirm swap"
        };
        let mut b = button::primary(None, label).width(Length::Fill);
        // Disabled for an expired quote, a send in flight, or a wallet that
        // hasn't finished syncing (the swap would fail server-side).
        if config.confirm_enabled {
            b = b.on_press(LiquidSwapMessage::Confirm);
        }
        b
    };

    let back_btn = button::secondary(None, "Back")
        .on_press(LiquidSwapMessage::BackToInput)
        .width(Length::Fill);

    let mut content = Column::new()
        .spacing(16)
        .max_width(480)
        .push(h4_bold("Review swap"));
    if config.syncing {
        content = content.push(hint_banner(
            "Wallet is still syncing — swaps are paused until it finishes.",
        ));
    }
    content = content.push(summary).push(confirm_btn).push(back_btn);

    if let Some(err) = config.error {
        content = content.push(error_card(err));
    }

    Container::new(content)
        .width(Length::Fill)
        .center_x(Length::Fill)
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
    Container::new(text(err.to_string()).size(14).color(color::RED))
        .padding(10)
        .style(theme::card::invalid)
        .width(Length::Fill)
        .into()
}

pub fn liquid_swap_view<'a>(config: LiquidSwapConfig<'a>) -> Element<'a, LiquidSwapMessage> {
    match config.phase {
        SwapPhase::Input => input_screen(&config),
        SwapPhase::Review => review_screen(&config),
        SwapPhase::Sent => coincube_ui::component::received_celebration_page(
            "liquid-send",
            config.sent_amount_display,
            config.sent_quote,
            config.sent_image_handle,
            "is on its way.",
            LiquidSwapMessage::Done,
        ),
    }
}
