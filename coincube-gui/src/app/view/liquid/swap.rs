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
    icon::arrow_down_up_icon,
    theme,
    widget::{Column, Container, Element, Row},
};
use iced::{
    widget::{container, Space},
    Alignment, Background, Length,
};

use crate::app::breez_liquid::assets::{format_asset_amount, AssetKind};
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
    pub entered_amount: &'a form::Value<String>,
    pub quote: Option<&'a SwapQuote>,
    pub quoting: bool,
    pub quote_remaining: u32,
    pub quote_actionable: bool,
    pub is_sending: bool,
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

/// Format a USDt base-unit amount as a decimal string (8-dp).
fn fmt_usdt(base: u64) -> String {
    format_asset_amount(base, AssetKind::Usdt.precision())
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

fn card<'a>(
    content: impl Into<Element<'a, LiquidSwapMessage>>,
) -> Container<'a, LiquidSwapMessage> {
    Container::new(content)
        .padding(20)
        .width(Length::Fill)
        .style(|t| container::Style {
            background: Some(Background::Color(t.colors.cards.simple.background)),
            border: iced::Border {
                radius: 20.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
}

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

/// The single-input swap screen.
fn input_screen<'a>(config: &LiquidSwapConfig<'a>) -> Element<'a, LiquidSwapMessage> {
    let from_balance = balance_base(config.from_asset, config.btc_balance, config.usdt_balance);
    let to_balance = balance_base(config.to_asset, config.btc_balance, config.usdt_balance);

    // ── You pay (from) — read-only quote output ──────────────────────────
    let pay_value: Element<'a, LiquidSwapMessage> = match config.quote {
        Some(q) => text(fmt_amount(
            config.from_asset,
            q.from_total_base(),
            config.bitcoin_unit,
        ))
        .size(H4_SIZE)
        .bold()
        .into(),
        None => text(format!("— {}", ticker(config.from_asset)))
            .size(H4_SIZE)
            .style(theme::text::secondary)
            .into(),
    };

    let from_card = card(
        Column::new()
            .spacing(10)
            .push(
                Row::new()
                    .align_y(Alignment::Center)
                    .push(text("You pay").size(P2_SIZE).style(theme::text::secondary))
                    .push(Space::new().width(Length::Fill))
                    .push(
                        button::secondary(None, "Swap All")
                            .on_press(LiquidSwapMessage::SwapAll)
                            .width(Length::Fixed(110.0)),
                    ),
            )
            .push(pay_value)
            .push(balance_row(
                config.from_asset,
                from_balance,
                config.bitcoin_unit,
            )),
    );

    // ── Flip control + rate chip ─────────────────────────────────────────
    let rate_chip: Element<'a, LiquidSwapMessage> = match config.quote {
        Some(q) => Container::new(
            text(format!(
                "1 {} = {}",
                ticker(config.from_asset),
                fmt_rate_value(config.to_asset, q.rate_to_per_from(), config.bitcoin_unit)
            ))
            .size(P2_SIZE)
            .style(theme::text::secondary),
        )
        .padding([4, 12])
        .style(|t| container::Style {
            background: Some(Background::Color(t.colors.cards.simple.background)),
            border: iced::Border {
                radius: 14.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into(),
        None => Space::new().height(Length::Fixed(0.0)).into(),
    };

    let flip_row = Row::new()
        .spacing(12)
        .align_y(Alignment::Center)
        .push(Space::new().width(Length::Fill))
        .push(
            button::secondary(Some(arrow_down_up_icon()), "Flip")
                .on_press(LiquidSwapMessage::FlipAssets)
                .width(Length::Fixed(90.0)),
        )
        .push(rate_chip)
        .push(Space::new().width(Length::Fill));

    // ── You receive (to) — the editable amount ───────────────────────────
    // Match the input mode to the asset/unit: whole sats for L-BTC in SATS
    // mode, decimal BTC for L-BTC in BTC mode, decimal for USDt.
    let amount_form = match (config.to_asset, config.bitcoin_unit) {
        (SendAsset::Lbtc, BitcoinDisplayUnit::Sats) => {
            form::Form::new_amount_sats("0", config.entered_amount, LiquidSwapMessage::AmountEdited)
        }
        (SendAsset::Lbtc, BitcoinDisplayUnit::BTC) => form::Form::new_amount_btc(
            "0.00000000",
            config.entered_amount,
            LiquidSwapMessage::AmountEdited,
        ),
        (SendAsset::Usdt, _) => form::Form::new_amount_numeric(
            "0.00",
            config.entered_amount,
            LiquidSwapMessage::AmountEdited,
        ),
    };
    let amount_field = amount_form
        .maybe_warning(config.entered_amount.warning)
        .padding(10)
        .into_container();

    let to_card = card(
        Column::new()
            .spacing(10)
            .push(
                Row::new()
                    .align_y(Alignment::Center)
                    .push(
                        text("You receive")
                            .size(P2_SIZE)
                            .style(theme::text::secondary),
                    )
                    .push(Space::new().width(Length::Fill))
                    .push(text(ticker(config.to_asset)).size(P1_SIZE).bold()),
            )
            .push(amount_field)
            .push(balance_row(
                config.to_asset,
                to_balance,
                config.bitcoin_unit,
            )),
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

    let mut content = Column::new()
        .spacing(16)
        .max_width(560)
        .push(h4_bold("Swap"))
        .push(from_card)
        .push(flip_row)
        .push(to_card)
        .push(status)
        .push(continue_btn);

    if let Some(err) = config.error {
        content = content.push(error_card(err));
    }

    Container::new(content)
        .width(Length::Fill)
        .center_x(Length::Fill)
        .into()
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
        } else {
            "Confirm swap"
        };
        let mut b = button::primary(None, label).width(Length::Fill);
        // Never confirm an expired quote or while a send is in flight.
        if config.quote_actionable {
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
        .push(h4_bold("Review swap"))
        .push(summary)
        .push(confirm_btn)
        .push(back_btn);

    if let Some(err) = config.error {
        content = content.push(error_card(err));
    }

    Container::new(content)
        .width(Length::Fill)
        .center_x(Length::Fill)
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
