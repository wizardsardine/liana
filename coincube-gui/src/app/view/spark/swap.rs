//! View for [`crate::app::state::spark::swap::SparkSwap`]: Swap BTC ⇄ USDB
//! (boards H-Convert, H-Convert-Review and P-Swap).
//!
//! The figures on Review are the bridge's estimate. The price is not
//! locked, so the copy says "estimated", never "quoted" (plan S3, master I6).

use coincube_spark_protocol::{
    AssetAmount, ConversionQuote, PrepareSwapOk, SwapStatus, WalletAsset,
};
use coincube_ui::locale::copy as localized_copy;
use coincube_ui::locale::Text as CopyText;
use coincube_ui::{
    component::tenshu::{icon_action, steps, ActionButton, ActionKind},
    component::text::*,
    image::asset_network_logo,
    theme,
    widget::{Column, ColumnExt, Container, Element, Row, RowExt},
};
use iced::{
    advanced::widget::Id,
    widget::{container, text_input, Space},
    Alignment, Background, Length,
};

use crate::app::breez_spark::assets::format_token_display_u128;
use crate::app::cache::Cache;
use crate::app::menu::{Menu, SparkSubMenu};
use crate::app::state::spark::swap::{format_asset, swap_available, SparkSwap, SwapPhase};
use crate::app::view::shared::payment_columns::{payment_columns, rail_card};
use crate::app::view::spark::{last_tx::recent_rail_section, SparkSwapMessage};
use crate::app::view::Message;

/// The flow on the head's rule.
pub const SWAP_STEPS: [&str; 3] = ["Details", "Review", "Swapped"];

pub const AMOUNT_ID: &str = "tenshu.spark.swap.amount";
pub const MAX_ID: &str = "tenshu.spark.swap.max";
pub const REVERSE_ID: &str = "tenshu.spark.swap.reverse";
pub const GET_QUOTE_ID: &str = "tenshu.spark.swap.get-quote";
pub const EDIT_ID: &str = "tenshu.spark.swap.edit";
pub const CONFIRM_ID: &str = "tenshu.spark.swap.confirm";
pub const AGAIN_ID: &str = "tenshu.spark.swap.again";
pub const ACTIVITY_ID: &str = "tenshu.spark.swap.activity";

/// What Review says about the price (master I6): the figures are an
/// estimate, and the slippage cap is the only protection.
pub const PRICE_DISCLOSURE: &str = "These figures are an estimate, not a locked price. If the \
     price moves more than the max price change before the swap settles, it doesn't go \
     through, and anything already taken is refunded.";

/// Where the swap stands on [`SWAP_STEPS`].
pub fn swap_step(phase: &SwapPhase) -> usize {
    match phase {
        SwapPhase::Details { .. } | SwapPhase::Quoting | SwapPhase::Refused(_) => 0,
        SwapPhase::Review(_) | SwapPhase::Swapping(_) | SwapPhase::Unknown(_) => 1,
        SwapPhase::Swapped(_) => 2,
    }
}

fn swap_message(message: SparkSwapMessage) -> Message {
    Message::SparkSwap(message)
}

fn asset_name(asset: WalletAsset) -> &'static str {
    match asset {
        WalletAsset::Bitcoin => "Bitcoin",
        WalletAsset::Usdb => "US Dollar",
    }
}

fn other(asset: WalletAsset) -> WalletAsset {
    match asset {
        WalletAsset::Bitcoin => WalletAsset::Usdb,
        WalletAsset::Usdb => WalletAsset::Bitcoin,
    }
}

/// `dollars` with cents and thousands separators: "82,700.00".
pub fn dollars(dollars: f64) -> String {
    format_token_display_u128((dollars.max(0.0) * 100.0).round() as u128, 2)
}

/// The US-dollar value of `amount`: sats at the BTC/USD `price`, USDB at par.
fn usd_value(amount: AssetAmount, price: Option<f64>, usdb_decimals: u32) -> Option<f64> {
    match amount.asset {
        WalletAsset::Bitcoin => price.map(|price| amount.amount as f64 / 100_000_000.0 * price),
        WalletAsset::Usdb => Some(amount.amount as f64 / 10_f64.powi(usdb_decimals as i32)),
    }
}

/// What `amount` converts to at the BTC/USD `price`, in the other asset.
fn converted_at(amount: AssetAmount, price: f64, usdb_decimals: u32) -> AssetAmount {
    match amount.asset {
        WalletAsset::Bitcoin => AssetAmount {
            asset: WalletAsset::Usdb,
            amount: (amount.amount as f64 / 100_000_000.0
                * price
                * 10_f64.powi(usdb_decimals as i32)) as u128,
        },
        WalletAsset::Usdb => AssetAmount {
            asset: WalletAsset::Bitcoin,
            amount: (amount.amount as f64 / 10_f64.powi(usdb_decimals as i32) / price
                * 100_000_000.0) as u128,
        },
    }
}

/// The estimate's own rate: dollars per bitcoin, from its two amounts.
pub fn estimated_rate(ok: &PrepareSwapOk, usdb_decimals: u32) -> Option<f64> {
    quote_rate(&ok.quote, usdb_decimals)
}

/// A conversion quote's rate, dollars per bitcoin. Send's Review shows it.
pub fn quote_rate(quote: &ConversionQuote, usdb_decimals: u32) -> Option<f64> {
    let (btc, usdb) = match quote.amount_in.asset {
        WalletAsset::Bitcoin => (quote.amount_in.amount, quote.amount_out.amount),
        WalletAsset::Usdb => (quote.amount_out.amount, quote.amount_in.amount),
    };
    (btc > 0)
        .then(|| (usdb as f64 / 10_f64.powi(usdb_decimals as i32)) / (btc as f64 / 100_000_000.0))
}

/// The fee as a share of what is converted: "1,000 SATS (0.10%)".
pub fn fee_line(ok: &PrepareSwapOk, usdb_decimals: u32) -> String {
    quote_fee_line(&ok.quote, usdb_decimals)
}

/// A conversion quote's fee, with its share of the amount converted when
/// both are in the same asset. Send's Review shows it too.
pub fn quote_fee_line(quote: &ConversionQuote, usdb_decimals: u32) -> String {
    let mut fee = format_asset(quote.fee.asset, quote.fee.amount, usdb_decimals);
    // A fee under a cent would read as free: say it is under a cent.
    if quote.fee.asset == WalletAsset::Usdb && quote.fee.amount > 0 && fee.starts_with("0.00 ") {
        fee = "< 0.01 USDB".to_owned();
    }
    if quote.amount_in.amount == 0 || quote.fee.asset != quote.amount_in.asset {
        return fee;
    }
    let percent = quote.fee.amount as f64 / quote.amount_in.amount as f64 * 100.0;
    format!("{} ({:.2}%)", fee, percent)
}

/// "0.5%" for 50 basis points.
pub fn price_change(bps: u32) -> String {
    let percent = bps as f64 / 100.0;
    if bps.is_multiple_of(100) {
        format!("{}%", bps / 100)
    } else {
        format!("{}%", percent)
    }
}

fn card_style(theme: &theme::Theme) -> container::Style {
    let shell = &theme.colors.shell;
    container::Style {
        background: Some(Background::Color(shell.panel)),
        border: iced::Border {
            color: shell.line,
            width: 1.0,
            radius: 16.0.into(),
        },
        ..Default::default()
    }
}

fn kv<'a>(label: &'a str, value: String) -> Element<'a, Message> {
    Row::new()
        .spacing(12)
        .align_y(Alignment::Center)
        .push(Container::new(p2_regular(label).style(theme::text::secondary)).width(Length::Fill))
        .push(p2_regular(value))
        .into()
}

fn action<'a>(
    id: &'static str,
    label: &'static str,
    kind: ActionKind,
    on_press: Option<Message>,
) -> Element<'a, Message> {
    ActionButton::new(Id::new(id), label, kind, Message::FocusAction)
        .on_press_maybe(on_press)
        .into()
}

fn section<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    Container::new(content)
        .padding([18, 24])
        .width(Length::Fill)
        .into()
}

/// The Swap page under its pinned head.
pub fn swap_page<'a>(
    menu: &'a Menu,
    cache: &'a Cache,
    swap: &'a SparkSwap,
) -> Element<'a, Message> {
    let head = crate::app::view::spark::sub_page_head(cache, "Swap BTC ⇄ USDB")
        .rule(steps(&SWAP_STEPS, swap_step(swap.phase())));
    crate::app::view::dashboard_payment(menu, cache, Some(head), swap_body(cache, swap))
}

/// The page below the head: the form or the result, and the side panel.
/// The result card's body and heading for the bridge's answer. Only a
/// completed swap is headed "Swapped".
fn swapped_copy(status: &SwapStatus) -> (&'static str, &'static str) {
    match status {
        SwapStatus::Completed => (
            localized_copy("The swap is complete."),
            localized_copy("Swapped"),
        ),
        SwapStatus::Pending => (
            localized_copy("The swap is settling. It shows as complete in Activity once it lands."),
            localized_copy("Swap settling"),
        ),
        SwapStatus::Failed => (
            localized_copy("The swap didn't go through. Nothing was swapped."),
            localized_copy("Swap didn't go through"),
        ),
        SwapStatus::RefundNeeded => (
            localized_copy("The swap didn't go through. Your funds are being refunded."),
            localized_copy("Swap being refunded"),
        ),
        SwapStatus::Refunded => (
            localized_copy("The swap didn't go through. Your funds were refunded."),
            localized_copy("Swap refunded"),
        ),
    }
}

pub fn swap_body<'a>(cache: &'a Cache, swap: &'a SparkSwap) -> Element<'a, Message> {
    if !swap_available(cache.network) {
        return p1_regular(localized_copy(
            "Swap needs Bitcoin mainnet: USDB exists only there.",
        ))
        .into();
    }
    let price = crate::app::state::spark::reference_btc_usd_price(cache);
    let decimals = swap.usdb_decimals_for_view();
    let card: Element<'a, Message> = match swap.phase() {
        SwapPhase::Details { .. } | SwapPhase::Quoting => {
            details_card(swap, price, decimals, cache.balance_amounts_masked())
        }
        SwapPhase::Review(ok) => review_card(ok, cache.chain(), price, decimals, false),
        SwapPhase::Swapping(ok) => review_card(ok, cache.chain(), price, decimals, true),
        SwapPhase::Swapped(ok) => {
            let (body, heading) = swapped_copy(&ok.status);
            result_card(body, heading, true)
        }
        SwapPhase::Refused(_) => result_card(
            coincube_ui::locale::copy(
                "The swap didn't start, so nothing was swapped. Get a new estimate to try again.",
            ),
            coincube_ui::locale::copy("Swap didn't start"),
            true,
        ),
        SwapPhase::Unknown(_) => result_card(
            coincube_ui::locale::copy(
                "We couldn't confirm the swap, and it may still go through. Check Activity before \
             you swap again.",
            ),
            coincube_ui::locale::copy("Swap not confirmed"),
            false,
        ),
    };
    let aside: Element<'a, Message> = match swap.phase() {
        SwapPhase::Review(ok) | SwapPhase::Swapping(ok) => {
            after_swap(swap, ok, decimals, cache.balance_amounts_masked())
        }
        _ => crate::app::view::spark::SparkBalances::new(
            cache,
            swap.btc_sats_for_view(),
            swap.usdb_for_view(),
        )
        .view(),
    };
    // The side panel, as on Send and Receive: what is in Spark (or what the
    // swap leaves), what auto-convert does next, then Spark's latest payments.
    let rail = Column::new()
        .spacing(20)
        .push(aside)
        .push_maybe(swap.auto_convert_for_view().map(auto_convert_line))
        .push(iced::widget::rule::horizontal(1))
        .push(recent_rail_section(
            swap.recent_transactions_for_view(),
            cache.bitcoin_unit,
            cache.balance_amounts_masked(),
            |idx| swap_message(SparkSwapMessage::SelectTransaction(idx)),
            swap_message(SparkSwapMessage::History),
        ));
    payment_columns(
        Container::new(card).width(Length::Fill).style(card_style),
        rail_card(rail),
    )
}

pub const WALLET_SETTINGS_ID: &str = "tenshu.spark.swap.wallet-settings";

/// What happens to bitcoin received after the swap (H-Convert's side panel),
/// with the way to change it.
pub fn auto_convert_line<'a>(
    line: crate::app::state::spark::swap::AutoConvertLine,
) -> Element<'a, Message> {
    use crate::app::state::spark::swap::AutoConvertLine;
    let (title, body) = match line {
        AutoConvertLine::On => (
            "Auto-convert is on",
            "Incoming bitcoin payments convert to USDB. Bitcoin from manual swaps stays bitcoin.",
        ),
        AutoConvertLine::Held => (
            "Auto-convert is waiting",
            "A swap out of USDB is settling. Bitcoin received meanwhile stays bitcoin.",
        ),
        AutoConvertLine::Off => (
            "Auto-convert is off",
            "Incoming bitcoin payments stay bitcoin. Your USDB stays USDB.",
        ),
    };
    Column::new()
        .spacing(4)
        .push(iced::widget::rule::horizontal(1))
        .push(Space::new().height(Length::Fixed(8.0)))
        .push(p2_bold(title))
        .push(p2_regular(body).style(theme::text::secondary))
        .push(
            coincube_ui::widget::action::Action::new(
                Id::new(WALLET_SETTINGS_ID),
                p2_regular(localized_copy("Change in Wallet settings")),
                Message::FocusAction,
            )
            .padding([4, 0])
            .style(|theme: &theme::Theme, status| {
                let mut style = theme::button::link(theme, status);
                style.text_color = theme.colors.shell.accent_ink;
                style
            })
            .on_press(Message::Menu(Menu::Spark(SparkSubMenu::Settings(Some(
                crate::app::menu::SparkSettingsOption::General,
            ))))),
        )
        .into()
}

/// A From or To tile, as compact as Send's and Receive's route tiles: the
/// coin, the caption, "‹asset› in Spark", then `line`.
fn asset_tile<'a>(caption: &'static str, asset: WalletAsset, line: String) -> Element<'a, Message> {
    let logo = match asset {
        WalletAsset::Bitcoin => asset_network_logo("btc", "spark", 36.0),
        WalletAsset::Usdb => asset_network_logo("usdb", "spark", 36.0),
    };
    Container::new(super::route_tile_body_with_detail(
        logo,
        caption,
        None,
        localized_copy(asset_name(asset)).to_owned(),
        localized_copy("in Spark").to_owned(),
        Some(line),
        None,
    ))
    .width(Length::Fill)
    .style(super::send::fixed_tile_style)
    .into()
}

fn details_card<'a>(
    swap: &'a SparkSwap,
    price: Option<f64>,
    decimals: u32,
    masked: bool,
) -> Element<'a, Message> {
    // Hidden amounts hide the balances, as the side panel does.
    let balance = |asset: WalletAsset| {
        if masked {
            coincube_ui::component::tenshu::MASK.to_owned()
        } else {
            format_asset(asset, swap.balance(asset), decimals)
        }
    };
    let from = swap.from();
    let to = other(from);
    let quoting = matches!(swap.phase(), SwapPhase::Quoting);
    let pair = Row::new()
        .spacing(12)
        .align_y(Alignment::Center)
        .push(asset_tile(
            "From",
            from,
            format!("Available {}", balance(from)),
        ))
        .push(icon_action(
            Id::new(REVERSE_ID),
            coincube_ui::icon::arrow_left_right_icon(),
            Message::FocusAction,
            (!quoting).then(|| swap_message(SparkSwapMessage::Reverse)),
        ))
        .push(asset_tile("To", to, format!("Balance {}", balance(to))));

    let unit = match from {
        WalletAsset::Bitcoin => "SATS",
        WalletAsset::Usdb => "USDB",
    };
    let placeholder = if swap.max_for_view() {
        "Max"
    } else {
        match from {
            WalletAsset::Bitcoin => "Amount in sats",
            WalletAsset::Usdb => "Amount in USDB",
        }
    };
    let input = text_input(placeholder, swap.amount_input_for_view())
        .id(Id::new(AMOUNT_ID))
        .padding(10);
    let input = if quoting {
        input
    } else {
        input.on_input(|value| swap_message(SparkSwapMessage::AmountEdited(value)))
    };
    let amount_row = Row::new()
        .spacing(10)
        .align_y(Alignment::Center)
        .push(input)
        .push(p2_regular(unit).style(theme::text::secondary))
        .push(action(
            MAX_ID,
            coincube_ui::locale::copy("Max"),
            ActionKind::Secondary,
            (!quoting).then(|| swap_message(SparkSwapMessage::Max)),
        ));

    let minimum = swap.minimum().map(|min| {
        coincube_ui::localized_format!(
            "Minimum {}",
            "Mínimo {}",
            format_asset(min.asset, min.amount, decimals)
        )
    });
    let shown_error = match swap.phase() {
        SwapPhase::Details { error: Some(error) } => Some(error.clone()),
        // Say what's wrong only once there is something typed (or Max).
        _ if !swap.amount_input_for_view().trim().is_empty() || swap.max_for_view() => {
            swap.form_error()
        }
        _ => None,
    };
    let estimate = swap_details_estimate(swap, from, price, decimals, masked);

    let figures = Column::new()
        .spacing(8)
        .push(kv(
            localized_copy("You get about"),
            estimate.unwrap_or_else(|| "—".to_owned()),
        ))
        .push(kv(
            localized_copy("Indicative rate"),
            price.map_or_else(
                || "Waiting for a price".to_owned(),
                |price| format!("1 BTC ≈ ${}", dollars(price)),
            ),
        ))
        .push(kv(
            localized_copy("Conversion fee"),
            "Shown with the estimate".to_owned(),
        ));

    let can_quote = !quoting && swap.form_error().is_none();
    let footer = Row::new()
        .spacing(12)
        .align_y(Alignment::Center)
        .push(
            Container::new(
                p2_regular(localized_copy(
                    "Get estimate asks for the rate and fee. Nothing is swapped yet.",
                ))
                .style(theme::text::secondary),
            )
            .width(Length::Fill),
        )
        .push(action(
            GET_QUOTE_ID,
            if quoting {
                "Getting estimate…"
            } else {
                "Get estimate"
            },
            ActionKind::Primary,
            can_quote.then(|| swap_message(SparkSwapMessage::GetQuote)),
        ));

    Column::new()
        .push(section(h4_bold(localized_copy("Swap details"))))
        .push(iced::widget::rule::horizontal(1))
        .push(section(pair))
        .push(iced::widget::rule::horizontal(1))
        .push(section(
            Column::new()
                .spacing(8)
                .push(p2_bold(localized_copy("Amount to swap")))
                .push(amount_row)
                .push_maybe(minimum.map(|line| p2_regular(line).style(theme::text::secondary)))
                .push_maybe(shown_error.map(|error| {
                    p2_regular(coincube_ui::locale::notice(error)).style(theme::text::warning)
                })),
        ))
        .push(iced::widget::rule::horizontal(1))
        .push(section(figures))
        .push(iced::widget::rule::horizontal(1))
        .push(section(footer))
        .into()
}

fn swap_details_estimate(
    swap: &SparkSwap,
    from: WalletAsset,
    price: Option<f64>,
    decimals: u32,
    masked: bool,
) -> Option<String> {
    price
        .filter(|_| !(masked && swap.max_for_view()))
        .and_then(|price| {
            let amount = if swap.max_for_view() {
                Some(swap.balance(from))
            } else {
                swap.typed_amount()
            }?;
            let out = converted_at(
                AssetAmount {
                    asset: from,
                    amount,
                },
                price,
                decimals,
            );
            Some(format_asset(out.asset, out.amount, decimals))
        })
}

fn review_card<'a>(
    ok: &'a PrepareSwapOk,
    chain: crate::chain::ChainId,
    price: Option<f64>,
    decimals: u32,
    swapping: bool,
) -> Element<'a, Message> {
    let quote = &ok.quote;
    let approx = |amount: AssetAmount| -> Option<String> {
        match amount.asset {
            WalletAsset::Bitcoin => {
                usd_value(amount, price, decimals).map(|usd| format!("≈ ${}", dollars(usd)))
            }
            WalletAsset::Usdb => price.map(|price| {
                let sats = converted_at(amount, price, decimals);
                format!("≈ {}", format_asset(sats.asset, sats.amount, decimals))
            }),
        }
    };
    let line = |label: &'static str, amount: AssetAmount| -> Element<'a, Message> {
        Row::new()
            .spacing(12)
            .align_y(Alignment::Center)
            .push(
                Container::new(p2_regular(label).style(theme::text::secondary)).width(Length::Fill),
            )
            .push(p1_bold(format_asset(amount.asset, amount.amount, decimals)))
            .push_maybe(approx(amount).map(|text| p2_regular(text).style(theme::text::secondary)))
            .into()
    };
    let cta = match quote.amount_in.asset {
        WalletAsset::Bitcoin => "Swap to USDB",
        WalletAsset::Usdb => "Swap to bitcoin",
    };
    let figures = Column::new()
        .spacing(10)
        .push(line(localized_copy("You swap"), quote.amount_in))
        .push(line(localized_copy("You get about"), quote.amount_out))
        .push(kv(
            localized_copy("Estimated rate"),
            estimated_rate(ok, decimals).map_or_else(
                || "—".to_owned(),
                |rate| format!("1 BTC ≈ ${}", dollars(rate)),
            ),
        ))
        .push(kv(localized_copy("Conversion fee"), fee_line(ok, decimals)))
        .push(kv(
            localized_copy("Max price change"),
            price_change(quote.max_slippage_bps),
        ));
    let actions = Row::new()
        .spacing(12)
        .align_y(Alignment::Center)
        .push(action(
            EDIT_ID,
            coincube_ui::locale::copy("Edit amount"),
            ActionKind::Secondary,
            (!swapping).then(|| swap_message(SparkSwapMessage::EditAmount)),
        ))
        .push(Space::new().width(Length::Fill))
        .push(action(
            CONFIRM_ID,
            if swapping { "Swapping…" } else { cta },
            ActionKind::Primary,
            (!swapping).then(|| swap_message(SparkSwapMessage::Confirm)),
        ));
    Column::new()
        .push(section(
            Column::new()
                .spacing(6)
                .push(h4_bold(CopyText::Review.get()))
                .push(p2_regular(crate::chain::canonical_network_name(chain))),
        ))
        .push(iced::widget::rule::horizontal(1))
        .push(section(figures))
        .push(iced::widget::rule::horizontal(1))
        .push(section(
            p2_regular(PRICE_DISCLOSURE).style(theme::text::secondary),
        ))
        .push(iced::widget::rule::horizontal(1))
        .push(section(actions))
        .into()
}

fn result_card<'a>(line: &'static str, title: &'static str, again: bool) -> Element<'a, Message> {
    let actions = Row::new()
        .spacing(12)
        .push(action(
            ACTIVITY_ID,
            coincube_ui::locale::copy("See in Activity"),
            ActionKind::Secondary,
            Some(Message::Activity(
                crate::app::view::activity::ActivityMessage::ViewAll(
                    crate::app::view::activity::WalletFilter::Spark,
                ),
            )),
        ))
        .push(action(
            AGAIN_ID,
            if again {
                "Swap again"
            } else {
                "Start a new swap"
            },
            if again {
                ActionKind::Primary
            } else {
                ActionKind::Secondary
            },
            Some(swap_message(SparkSwapMessage::SwapAgain)),
        ));
    Column::new()
        .push(section(h4_bold(title)))
        .push(iced::widget::rule::horizontal(1))
        .push(section(p1_regular(line)))
        .push(section(actions))
        .into()
}

/// Review's side panel: each balance now and after the swap, estimated.
fn after_swap<'a>(
    swap: &'a SparkSwap,
    ok: &'a PrepareSwapOk,
    decimals: u32,
    masked: bool,
) -> Element<'a, Message> {
    let after = |asset: WalletAsset| -> u128 {
        let now = swap.balance(asset);
        if asset == ok.quote.amount_in.asset {
            now.saturating_sub(ok.quote.amount_in.amount)
        } else {
            now.saturating_add(ok.quote.amount_out.amount)
        }
    };
    let row = |asset: WalletAsset| -> Element<'a, Message> {
        Column::new()
            .spacing(2)
            .push(p2_regular(asset_name(asset)))
            .push(
                p2_regular(if masked {
                    let mask = coincube_ui::component::tenshu::MASK;
                    format!("{} → {}", mask, mask)
                } else {
                    format!(
                        "{} → {}",
                        format_asset(asset, swap.balance(asset), decimals),
                        format_asset(asset, after(asset), decimals)
                    )
                })
                .style(theme::text::secondary),
            )
            .into()
    };
    Column::new()
        .spacing(10)
        .push(p2_regular(localized_copy("After the swap, estimated")).style(theme::text::secondary))
        .push(row(WalletAsset::Bitcoin))
        .push(row(WalletAsset::Usdb))
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use coincube_spark_protocol::{ConversionLimits, ConversionQuote};

    fn quote(from: WalletAsset, amount_in: u128, amount_out: u128, fee: u128) -> PrepareSwapOk {
        PrepareSwapOk {
            handle: "h".into(),
            quote: ConversionQuote {
                amount_in: AssetAmount {
                    asset: from,
                    amount: amount_in,
                },
                amount_out: AssetAmount {
                    asset: other(from),
                    amount: amount_out,
                },
                fee: AssetAmount {
                    asset: from,
                    amount: fee,
                },
                amount_adjustment: None,
                max_slippage_bps: 50,
            },
            limits: ConversionLimits::default(),
        }
    }

    #[test]
    fn only_a_completed_swap_is_headed_swapped() {
        assert_eq!(swapped_copy(&SwapStatus::Completed).1, "Swapped");
        for status in [
            SwapStatus::Pending,
            SwapStatus::Failed,
            SwapStatus::RefundNeeded,
            SwapStatus::Refunded,
        ] {
            let (body, heading) = swapped_copy(&status);
            assert_ne!(heading, "Swapped", "{:?}", status);
            if body.contains("didn't go through") {
                assert!(
                    heading.contains("didn't go through") || heading.contains("refund"),
                    "{:?}: {}",
                    status,
                    heading
                );
            }
        }
    }

    fn mainnet() -> Cache {
        Cache {
            network: coincube_core::miniscript::bitcoin::Network::Bitcoin,
            btc_usd_price: Some(80_000.0),
            balance_privacy: Some(crate::app::state::preferences::BalancePrivacyChrome {
                privacy: crate::app::state::preferences::BalancePrivacy::Shown,
                busy: false,
                interactive: false,
            }),
            ..Cache::default()
        }
    }

    fn funded() -> SparkSwap {
        let mut swap = SparkSwap::new(None);
        drop(swap.on_message(SparkSwapMessage::BalanceLoaded(Some((
            2_000_000,
            Some(coincube_spark_protocol::UsdbBalanceSnapshot {
                token_identifier: "btkn1synthetic".into(),
                balance: 500_000_000,
                decimals: 6,
                ticker: "USDB".into(),
            }),
        )))));
        swap
    }

    fn inspect<'a>(
        name: &str,
        element: Element<'a, Message>,
    ) -> crate::app::view::shared::landing::capture::Inspection {
        crate::app::view::shared::landing::capture::inspect(
            name,
            element,
            (1440, 900),
            "dark",
            theme::Theme::dark(),
        )
    }

    /// The form and the rail fill the page column, as on Send and Receive:
    /// compact tiles, and the rail ending at the column's right edge with
    /// Spark's latest payments.
    #[test]
    fn the_form_and_the_rail_fill_the_page_column() {
        use crate::app::view::shared::payment_columns::RAIL_WIDTH;
        let cache = mainnet();
        let mut swap = funded();
        swap.set_recent_payments(
            &[crate::app::state::spark::test_payment("pay-1", 1_001)],
            &cache,
        );
        let width = 1106;
        let probe = crate::app::view::shared::landing::capture::inspect_wide_content(
            "spark-swap-columns",
            swap_body(&cache, &swap),
            (1440, 900),
            "dark",
            theme::Theme::dark(),
            width,
        );
        for label in [
            "in Spark",
            "Recent in Spark",
            "See all in Activity",
            "1,001",
        ] {
            assert!(probe.contains(label), "{}", label);
        }
        let x = |needle: &str| {
            probe
                .text
                .iter()
                .find_map(|(value, bounds)| (value == needle).then_some(bounds.x))
                .unwrap_or_else(|| panic!("{} not shown", needle))
        };
        // The rail's caption sits inside the rail card, beside the form.
        let rail = width as f32 - RAIL_WIDTH;
        assert!(
            x("Available in Spark") > rail && x("Available in Spark") < rail + 40.0,
            "rail caption at x = {}",
            x("Available in Spark")
        );
        assert!(x("Swap details") < 40.0);
    }

    #[test]
    fn details_offers_an_estimate_only_for_a_valid_amount() {
        let cache = mainnet();
        let mut swap = funded();
        let probe = inspect("spark-swap-details-empty", swap_body(&cache, &swap));
        for label in [
            "From",
            "To",
            "Bitcoin",
            "US Dollar",
            "Amount to swap",
            "Indicative rate",
            "Get estimate",
        ] {
            assert!(probe.contains(label), "{}", label);
        }
        assert!(probe.contains("1 BTC ≈ $80,000.00"));
        // Get estimate is disabled, so not a focus target, for an empty amount.
        assert!(!probe.has_focus(Id::new(GET_QUOTE_ID)));

        drop(swap.on_message(SparkSwapMessage::AmountEdited("1000000".into())));
        let probe = inspect("spark-swap-details-filled", swap_body(&cache, &swap));
        // 1,000,000 sats at $80,000 is about $800 of USDB.
        assert!(probe.contains("800.00 USDB"));
        let messages = crate::app::view::shared::landing::capture::activate(
            swap_body(&cache, &swap),
            Id::new(GET_QUOTE_ID),
            (1440, 900),
            true,
        );
        assert!(messages
            .iter()
            .any(|m| matches!(m, Message::SparkSwap(SparkSwapMessage::GetQuote))));
    }

    #[test]
    fn review_shows_the_estimate_and_swaps_with_it() {
        let cache = mainnet();
        let mut swap = funded();
        swap.set_phase_for_tests(SwapPhase::Review(Box::new(quote(
            WalletAsset::Bitcoin,
            1_000_000,
            800_600_000,
            1_000,
        ))));
        let probe = inspect("spark-swap-review", swap_body(&cache, &swap));
        for label in [
            "You swap",
            "1,000,000 SATS",
            "You get about",
            "800.60 USDB",
            "Estimated rate",
            "1,000 SATS (0.10%)",
            "Max price change",
            "0.5%",
            "Swap to USDB",
            "After the swap, estimated",
            "2,000,000 SATS → 1,000,000 SATS",
            "500.00 USDB → 1,300.60 USDB",
        ] {
            assert!(probe.contains(label), "{}", label);
        }
        assert!(!probe.contains("Quoted"));
        let messages = crate::app::view::shared::landing::capture::activate(
            swap_body(&cache, &swap),
            Id::new(CONFIRM_ID),
            (1440, 900),
            true,
        );
        assert!(messages
            .iter()
            .any(|m| matches!(m, Message::SparkSwap(SparkSwapMessage::Confirm))));

        // While it swaps, nothing on the card acts.
        let SwapPhase::Review(ok) = swap.phase().clone() else {
            unreachable!()
        };
        swap.set_phase_for_tests(SwapPhase::Swapping(ok));
        let probe = inspect("spark-swap-swapping", swap_body(&cache, &swap));
        assert!(probe.contains("Swapping…"));
        for id in [CONFIRM_ID, EDIT_ID] {
            assert!(
                !probe.has_focus(Id::new(id)),
                "{} is disabled while swapping",
                id
            );
        }
    }

    #[test]
    fn hidden_amounts_hide_balances_on_the_form_and_review() {
        let cache = Cache {
            balance_privacy: Some(crate::app::state::preferences::BalancePrivacyChrome {
                privacy: crate::app::state::preferences::BalancePrivacy::Hidden,
                busy: false,
                interactive: false,
            }),
            ..mainnet()
        };
        assert!(cache.balance_amounts_masked());
        let mut swap = funded();
        let probe = inspect("spark-swap-hidden", swap_body(&cache, &swap));
        assert!(!probe.contains("2,000,000") && !probe.contains("500.00"));
        swap.set_phase_for_tests(SwapPhase::Review(Box::new(quote(
            WalletAsset::Bitcoin,
            1_000_000,
            800_600_000,
            1_000,
        ))));
        let probe = inspect("spark-swap-hidden-review", swap_body(&cache, &swap));
        assert!(!probe.contains("2,000,000 SATS →") && !probe.contains("500.00 USDB →"));
    }

    #[test]
    fn an_unknown_swap_points_to_activity_and_never_says_failed() {
        let cache = mainnet();
        let mut swap = funded();
        swap.set_phase_for_tests(SwapPhase::Unknown("timed out".into()));
        let probe = inspect("spark-swap-unknown", swap_body(&cache, &swap));
        assert!(probe.contains("Check Activity before"));
        assert!(probe.contains("See in Activity"));
        assert!(!probe.contains("didn't go through") && !probe.contains("failed"));
    }

    /// H-Convert: the side panel says what happens to bitcoin received
    /// after the swap, and links to Wallet settings, once auto-convert is read.
    #[test]
    fn the_side_panel_says_what_auto_convert_does() {
        let cache = mainnet();
        let mut swap = funded();
        let probe = inspect("spark-swap-auto-convert-unread", swap_body(&cache, &swap));
        assert!(!probe.contains("Auto-convert is"));
        drop(swap.on_message(SparkSwapMessage::AutoConvertLoaded {
            read: 0,
            line: Some(crate::app::state::spark::swap::AutoConvertLine::Off),
        }));
        let probe = inspect("spark-swap-auto-convert-off", swap_body(&cache, &swap));
        assert!(probe.contains("Auto-convert is off"));
        assert!(probe.contains("Incoming bitcoin payments stay bitcoin. Your USDB stays USDB."));
        drop(swap.on_message(SparkSwapMessage::AutoConvertLoaded {
            read: 0,
            line: Some(crate::app::state::spark::swap::AutoConvertLine::Held),
        }));
        let probe = inspect("spark-swap-auto-convert-held", swap_body(&cache, &swap));
        assert!(probe.contains("Auto-convert is waiting"));
        assert!(probe.contains("Bitcoin received meanwhile stays bitcoin."));
        assert!(!probe.contains("converts to USDB"));
        drop(swap.on_message(SparkSwapMessage::AutoConvertLoaded {
            read: 0,
            line: Some(crate::app::state::spark::swap::AutoConvertLine::On),
        }));
        let probe = inspect("spark-swap-auto-convert-on", swap_body(&cache, &swap));
        assert!(probe.contains("Auto-convert is on"));
        assert!(probe.contains("Incoming bitcoin payments convert to USDB."));
        assert!(probe.contains("Bitcoin from manual swaps stays bitcoin."));
        let messages = crate::app::view::shared::landing::capture::activate(
            swap_body(&cache, &swap),
            Id::new(WALLET_SETTINGS_ID),
            (1440, 900),
            true,
        );
        assert!(messages.iter().any(|m| matches!(
            m,
            Message::Menu(Menu::Spark(SparkSubMenu::Settings(Some(
                crate::app::menu::SparkSettingsOption::General
            ))))
        )));
    }

    #[test]
    fn swap_says_why_it_is_missing_off_mainnet() {
        let cache = Cache {
            network: coincube_core::miniscript::bitcoin::Network::Regtest,
            ..Cache::default()
        };
        let swap = funded();
        let probe = inspect("spark-swap-regtest", swap_body(&cache, &swap));
        assert!(probe.contains("Swap needs Bitcoin mainnet"));
        assert!(!probe.contains("Get estimate"));
    }

    #[test]
    fn review_figures_come_from_the_estimate() {
        // 1,000,000 sats for 800.60 USDB, 1,000 sats fee.
        let to_usdb = quote(WalletAsset::Bitcoin, 1_000_000, 800_600_000, 1_000);
        assert_eq!(dollars(estimated_rate(&to_usdb, 6).unwrap()), "80,060.00");
        assert_eq!(fee_line(&to_usdb, 6), "1,000 SATS (0.10%)");
        // 1,000.00 USDB for 1,246,560 sats, 1.00 USDB fee.
        let to_btc = quote(WalletAsset::Usdb, 1_000_000_000, 1_246_560, 1_000_000);
        assert_eq!(fee_line(&to_btc, 6), "1.00 USDB (0.10%)");
        // A fee under a cent never reads as free.
        let tiny = quote(WalletAsset::Usdb, 750_000, 1_012, 750);
        assert_eq!(fee_line(&tiny, 6), "< 0.01 USDB (0.10%)");
        assert_eq!(price_change(50), "0.5%");
        assert_eq!(price_change(100), "1%");
    }

    #[test]
    fn steps_follow_the_phase() {
        let ok = Box::new(quote(WalletAsset::Bitcoin, 1, 1, 0));
        assert_eq!(swap_step(&SwapPhase::Details { error: None }), 0);
        assert_eq!(swap_step(&SwapPhase::Quoting), 0);
        assert_eq!(swap_step(&SwapPhase::Review(ok.clone())), 1);
        assert_eq!(swap_step(&SwapPhase::Swapping(ok)), 1);
        assert_eq!(
            swap_step(&SwapPhase::Swapped(coincube_spark_protocol::SwapOk {
                swap_id: "s".into(),
                status: SwapStatus::Completed,
            })),
            2
        );
    }

    #[test]
    fn the_disclosure_never_promises_a_locked_price() {
        assert!(PRICE_DISCLOSURE.contains("estimate"));
        assert!(!PRICE_DISCLOSURE.to_lowercase().contains("quoted"));
        assert!(!PRICE_DISCLOSURE.to_lowercase().contains("guarantee"));
    }
}
