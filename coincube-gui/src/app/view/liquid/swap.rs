//! Liquid cross-asset Swap view.
//!
//! Single-input screen (Aqua-style) plus a locked-quote review step.
//! The editable amount is the **receive** amount (the `to` asset); the
//! pay amount and fee are read-only quote output. See
//! [`crate::app::state::liquid::swap`] for the quote model.

use coincube_core::miniscript::bitcoin::Amount;
use coincube_ui::{
    color,
    component::{button, form, text::*},
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

/// Format an asset base-unit amount for display (8-dp, trimmed).
fn fmt_asset(base: u64) -> String {
    format_asset_amount(base, AssetKind::Usdt.precision())
}

/// Format a swap rate (`to` per 1 `from`), widening precision for small rates.
fn fmt_rate(rate: f64) -> String {
    if rate >= 1.0 {
        format!("{rate:.2}")
    } else if rate > 0.0 {
        format!("{rate:.8}")
    } else {
        "—".to_string()
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

fn balance_row<'a>(label: &'a str, base: u64) -> Element<'a, LiquidSwapMessage> {
    Row::new()
        .spacing(6)
        .push(text(label).size(P2_SIZE).style(theme::text::secondary))
        .push(
            text(fmt_asset(base))
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
        Some(q) => text(format!(
            "{} {}",
            fmt_asset(q.from_total_base()),
            ticker(config.from_asset)
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
            .push(balance_row("Balance:", from_balance)),
    );

    // ── Flip control + rate chip ─────────────────────────────────────────
    let rate_chip: Element<'a, LiquidSwapMessage> = match config.quote {
        Some(q) => Container::new(
            text(format!(
                "1 {} = {} {}",
                ticker(config.from_asset),
                fmt_rate(q.rate_to_per_from()),
                ticker(config.to_asset)
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
    let amount_field = form::Form::new_amount_numeric("0.00", config.entered_amount, |v| {
        LiquidSwapMessage::AmountEdited(v)
    })
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
            .push(balance_row("Balance:", to_balance)),
    );

    // ── Fee / status line ────────────────────────────────────────────────
    let mut status = Column::new().spacing(6);
    if let Some(q) = config.quote {
        status = status.push(
            Row::new()
                .spacing(6)
                .push(text("Fee:").size(P2_SIZE).style(theme::text::secondary))
                .push(
                    text(format!(
                        "{} {}",
                        fmt_asset(q.fee_base),
                        ticker(config.from_asset)
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
        // Quote expired out from under the review — show a re-fetch prompt.
        let content = Column::new()
            .spacing(16)
            .max_width(480)
            .push(h4_bold("Review swap"))
            .push(
                text("Quote expired — fetching a fresh one…")
                    .size(P1_SIZE)
                    .style(theme::text::secondary),
            )
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
                format!(
                    "{} {}",
                    fmt_asset(q.from_total_base()),
                    ticker(config.from_asset)
                ),
            ))
            .push(line(
                "You receive",
                format!("{} {}", fmt_asset(q.receiver_base), ticker(config.to_asset)),
            ))
            .push(line(
                "Rate",
                format!(
                    "1 {} = {} {}",
                    ticker(config.from_asset),
                    fmt_rate(q.rate_to_per_from()),
                    ticker(config.to_asset)
                ),
            ))
            .push(line(
                "SideSwap + network fee",
                format!("{} {}", fmt_asset(q.fee_base), ticker(config.from_asset)),
            ))
            .push(line(
                "Quote expires in",
                format!("{}s", config.quote_remaining),
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
