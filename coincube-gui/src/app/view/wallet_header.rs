//! Shared wallet-header balance block.
//!
//! Renders the primary/secondary balance pair fiat-first: fiat is the
//! large number, SATS moves to the smaller secondary line. The same
//! helper drives Home wallet cards, per-wallet Overview pages, and the
//! Total Balance block, so the typography tiers Total → Card → Overview
//! stay centralised.
//!
//! Also absorbs the richer balance states Vault Overview exposes
//! (pulsing balance + progress row during sync, "+X unconfirmed" row)
//! so those don't regress when Vault Overview adopts the shared helper.

use std::time::Duration;

use coincube_core::miniscript::bitcoin::Amount;

use coincube_ui::{
    color,
    component::{
        amount::{
            amount_with_size_and_unit, amount_with_size_colors_and_unit,
            unconfirmed_amount_with_size_and_unit, BitcoinDisplayUnit,
        },
        spinner,
        text::{text, Text as TextExt, H1_SIZE, H2_SIZE, H3_SIZE, H4_SIZE, P1_SIZE, P2_SIZE},
    },
    icon::warning_icon,
    theme,
    widget::{Column, ColumnExt, Element, Row, RowExt},
};
use iced::{
    mouse,
    widget::{mouse_area, Space},
    Alignment, Length,
};

use crate::app::settings::display::DisplayMode;
use crate::app::view::vault::fiat::FiatAmount;
use crate::services::fiat::Currency;

/// Typography tier. Jumbo drives the Total Balance block; Card drives
/// Home wallet cards; Overview drives per-wallet Overview headers.
#[derive(Clone, Copy)]
pub enum HeaderVariant {
    Jumbo,
    Card,
    Overview,
}

/// Balance sync / catch-up state. Only `Synced` produces a static
/// render; other variants pulse the secondary (SATS) number and add a
/// progress row, matching Vault Overview's existing treatment.
pub enum SyncState {
    Synced,
    Syncing {
        progress: Option<f64>,
        label: String,
    },
    Checking,
}

/// Unconfirmed-balance overlay ("+X unconfirmed Y fiat"). Only rendered
/// when sync is `Synced` and the amount is non-zero.
pub struct UnconfirmedBalance {
    pub amount: Amount,
    pub fiat: Option<FiatAmount>,
}

pub struct WalletHeaderProps<M> {
    pub sats: Amount,
    pub fiat: Option<FiatAmount>,
    pub balance_masked: bool,
    pub bitcoin_unit: BitcoinDisplayUnit,
    pub variant: HeaderVariant,
    pub sync: SyncState,
    pub unconfirmed: Option<UnconfirmedBalance>,
    pub pending_send_sats: u64,
    pub pending_receive_sats: u64,
    /// Global display preference. When `BitcoinNative`, the primary
    /// (large) value is bitcoin and fiat moves to the secondary line.
    pub display_mode: DisplayMode,
    /// Optional click-to-swap message. When `Some`, the primary value is
    /// wrapped in a `mouse_area` that emits this message on tap and a
    /// pointer cursor on hover; an adjacent shuffle glyph hints at the
    /// affordance. `None` for non-interactive renders (e.g. read-only
    /// embeddings, screenshot tests).
    pub on_swap: Option<M>,
}

impl<M> WalletHeaderProps<M> {
    /// Convenience for the common case: synced, no unconfirmed, no pending.
    pub fn new(
        sats: Amount,
        fiat: Option<FiatAmount>,
        balance_masked: bool,
        bitcoin_unit: BitcoinDisplayUnit,
        variant: HeaderVariant,
        display_mode: DisplayMode,
        on_swap: Option<M>,
    ) -> Self {
        Self {
            sats,
            fiat,
            balance_masked,
            bitcoin_unit,
            variant,
            sync: SyncState::Synced,
            unconfirmed: None,
            pending_send_sats: 0,
            pending_receive_sats: 0,
            display_mode,
            on_swap,
        }
    }
}

/// Returns the balance column (primary value, secondary value, and any
/// applicable sync / unconfirmed / pending rows). Whether the primary
/// is fiat or bitcoin is governed by `display_mode`. Callers compose
/// the wallet-title row and surrounding chrome themselves — this helper
/// only unifies the balance hierarchy.
pub fn wallet_header<'a, M: 'a + Clone>(props: WalletHeaderProps<M>) -> Column<'a, M> {
    let WalletHeaderProps {
        sats,
        fiat,
        balance_masked,
        bitcoin_unit,
        variant,
        sync,
        unconfirmed,
        pending_send_sats,
        pending_receive_sats,
        display_mode,
        on_swap,
    } = props;

    let primary_size = match variant {
        HeaderVariant::Jumbo => H1_SIZE,
        HeaderVariant::Card | HeaderVariant::Overview => H2_SIZE,
    };
    let secondary_size = P1_SIZE;

    // Build fiat / bitcoin sub-elements once each, then assemble in the
    // order the display_mode requests. Falling back to bitcoin if no
    // fiat data is available keeps the header from going blank.
    let bitcoin_primary: Element<'a, M> = if balance_masked {
        text("********").size(primary_size).bold().into()
    } else {
        amount_with_size_and_unit::<M>(&sats, primary_size, bitcoin_unit).into()
    };
    let bitcoin_secondary: Element<'a, M> = if balance_masked {
        text("********")
            .size(secondary_size)
            .style(theme::text::secondary)
            .into()
    } else {
        match &sync {
            SyncState::Synced => {
                amount_with_size_and_unit::<M>(&sats, secondary_size, bitcoin_unit).into()
            }
            SyncState::Syncing { .. } | SyncState::Checking => Row::<'a, M>::new()
                .push(spinner::Carousel::new(
                    Duration::from_millis(1000),
                    vec![
                        amount_with_size_and_unit::<M>(&sats, secondary_size, bitcoin_unit),
                        amount_with_size_colors_and_unit::<M>(
                            &sats,
                            secondary_size,
                            color::GREY_3,
                            Some(color::GREY_3),
                            bitcoin_unit,
                        ),
                    ],
                ))
                .into(),
        }
    };
    let fiat_primary: Option<Element<'a, M>> = if balance_masked {
        Some(text("********").size(primary_size).bold().into())
    } else {
        fiat.as_ref()
            .map(|f| fiat_amount_row::<M>(f, primary_size, FiatStyle::Primary).into())
    };
    let fiat_secondary: Option<Element<'a, M>> = if balance_masked {
        Some(
            text("********")
                .size(secondary_size)
                .style(theme::text::secondary)
                .into(),
        )
    } else {
        fiat.as_ref()
            .map(|f| fiat_amount_row::<M>(f, secondary_size, FiatStyle::Secondary).into())
    };

    // Pick primary/secondary per display_mode. If fiat-native but no
    // fiat data is available, fall through to bitcoin-primary so the
    // header isn't blank.
    let (primary, secondary): (Element<'a, M>, Element<'a, M>) = match display_mode {
        DisplayMode::FiatNative => match fiat_primary {
            Some(fp) => (fp, bitcoin_secondary),
            None => (
                bitcoin_primary,
                fiat_secondary.unwrap_or_else(|| Space::new().into()),
            ),
        },
        DisplayMode::BitcoinNative => {
            (bitcoin_primary, fiat_secondary.unwrap_or(bitcoin_secondary))
        }
    };

    // Wrap the primary in a click-to-swap mouse_area when `on_swap` is
    // provided. Pointer cursor on hover is the only discoverability cue
    // — no adjacent glyph, per design feedback.
    let primary_row: Element<'a, M> = if let Some(swap) = on_swap {
        mouse_area(primary)
            .interaction(mouse::Interaction::Pointer)
            .on_press(swap)
            .into()
    } else {
        primary
    };

    let progress_row: Option<Element<'a, M>> = match &sync {
        SyncState::Synced => None,
        SyncState::Syncing { progress, label } => {
            let line = match progress {
                Some(p) => format!("{} ({:.1}%)", label, 100.0 * p),
                None => label.clone(),
            };
            Some(
                Row::<'a, M>::new()
                    .push(text(line).size(P2_SIZE).style(theme::text::secondary))
                    .push(spinner::typing_text_carousel(
                        "...",
                        true,
                        Duration::from_millis(2000),
                        |content| text(content).style(theme::text::secondary),
                    ))
                    .into(),
            )
        }
        SyncState::Checking => Some(
            Row::<'a, M>::new()
                .push(
                    text("Checking for new transactions")
                        .size(P2_SIZE)
                        .style(theme::text::secondary),
                )
                .push(spinner::typing_text_carousel(
                    "...",
                    true,
                    Duration::from_millis(2000),
                    |content| text(content).style(theme::text::secondary),
                ))
                .into(),
        ),
    };

    let unconfirmed_row: Option<Element<'a, M>> = match (&sync, unconfirmed.as_ref()) {
        (SyncState::Synced, Some(u)) if u.amount.to_sat() != 0 && !balance_masked => Some(
            Row::<'a, M>::new()
                .spacing(10)
                .align_y(Alignment::Center)
                .push(text("+").size(H3_SIZE).style(theme::text::secondary))
                .push(unconfirmed_amount_with_size_and_unit::<M>(
                    &u.amount,
                    H3_SIZE,
                    bitcoin_unit,
                ))
                .push(
                    text("unconfirmed")
                        .size(H3_SIZE)
                        .style(theme::text::secondary),
                )
                .push_maybe(u.fiat.as_ref().map(|f| {
                    let fiat_text: Element<'a, M> =
                        f.to_text().size(H4_SIZE).color(color::GREY_3).into();
                    Row::<'a, M>::new()
                        .align_y(Alignment::Center)
                        .push(Space::new().width(Length::Fixed(10.0)))
                        .push(fiat_text)
                }))
                .wrap()
                .into(),
        ),
        _ => None,
    };

    let pending_send_row: Option<Element<'a, M>> = (!balance_masked && pending_send_sats > 0)
        .then(|| pending_row(pending_send_sats, bitcoin_unit, "-"));
    let pending_receive_row: Option<Element<'a, M>> = (!balance_masked && pending_receive_sats > 0)
        .then(|| pending_row(pending_receive_sats, bitcoin_unit, "+"));

    Column::<'a, M>::new()
        .spacing(4)
        .push(primary_row)
        .push(secondary)
        .push_maybe(progress_row)
        .push_maybe(unconfirmed_row)
        .push_maybe(pending_send_row)
        .push_maybe(pending_receive_row)
}

/// Whether the fiat row is the Primary (large + bold value, dimmed
/// annotations) or Secondary (everything dimmed at the smaller size).
#[derive(Clone, Copy)]
enum FiatStyle {
    Primary,
    Secondary,
}

/// Render a fiat amount as a styled Row:
/// - Leading "~" approximation marker at `value_size`, in `color::GREY_3`
///   to match the SATS suffix on the bitcoin line.
/// - For USD, the dollar sign is glued to the value as a prefix
///   (`$X.XX`) and styled like the value (bold + value_size in
///   Primary mode; dimmed + value_size in Secondary mode).
/// - For other currencies, the trailing currency code (e.g. " EUR",
///   " NGN") renders at `value_size` in `color::GREY_3` — same look as
///   the SATS suffix that follows the satoshi balance.
fn fiat_amount_row<'a, M: 'a>(fiat: &FiatAmount, value_size: u32, style: FiatStyle) -> Row<'a, M> {
    let value_str = fiat.to_rounded_string();
    let value_text = match style {
        FiatStyle::Primary => text(value_str).size(value_size).bold(),
        FiatStyle::Secondary => text(value_str).size(value_size).color(color::GREY_3),
    };

    let mut row = Row::<'a, M>::new()
        .spacing(8)
        .align_y(Alignment::Center)
        .push(text("~").size(value_size).color(color::GREY_3));

    match fiat.currency() {
        Currency::USD => {
            let prefix = match style {
                FiatStyle::Primary => text("$").size(value_size).bold(),
                FiatStyle::Secondary => text("$").size(value_size).color(color::GREY_3),
            };
            // Joined "$X.XX" — group prefix + value in an inner row with
            // zero spacing so the sigil sits flush against the number.
            row = row.push(
                Row::<'a, M>::new()
                    .spacing(0)
                    .align_y(Alignment::Center)
                    .push(prefix)
                    .push(value_text),
            );
        }
        other => {
            row = row.push(value_text).push(
                text(other.to_string())
                    .size(value_size)
                    .color(color::GREY_3),
            );
        }
    }
    row
}

fn pending_row<'a, M: 'a>(
    sats: u64,
    bitcoin_unit: BitcoinDisplayUnit,
    sign: &'static str,
) -> Element<'a, M> {
    Row::<'a, M>::new()
        .spacing(6)
        .align_y(Alignment::Center)
        .push(warning_icon().size(12).style(theme::text::secondary))
        .push(text(sign).size(P2_SIZE).style(theme::text::secondary))
        .push(amount_with_size_and_unit::<M>(
            &Amount::from_sat(sats),
            P2_SIZE,
            bitcoin_unit,
        ))
        .push(text("pending").size(P2_SIZE).style(theme::text::secondary))
        .into()
}
