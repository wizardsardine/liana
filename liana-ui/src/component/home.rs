use std::time::Duration;

use iced::{widget::Space, Alignment, Length};

use crate::{
    color,
    component::{
        amount::{amount_with_font, amount_with_font_blink, unconfirmed_amount_with_size},
        button,
        card::{home_hint, home_warning},
        spinner,
        text::{legacy, text},
    },
    font::MANROPE_MEDIUM,
    icon, theme,
    widget::{Column, Container, Element, Row, RowExt, SpaceExt},
};

const RESCAN_WARNING: &str = "As this wallet was restored from a backup, you may need to rescan the blockchain to see past transactions.";

/// Card prompting a rescan after restoring a wallet from backup.
pub fn rescan_warning<'a, M: Clone + 'static>(go_to_rescan: M, dismiss: M) -> Element<'a, M> {
    Container::new(
        Column::new()
            .spacing(10)
            .push(
                Row::new()
                    .spacing(5)
                    .push(icon::warning_icon().style(theme::text::warning))
                    .push(text(RESCAN_WARNING).style(theme::text::warning))
                    .align_y(Alignment::Center),
            )
            .push(
                Row::new()
                    .spacing(5)
                    .push(Space::with_width(Length::Fill))
                    .push(button::secondary(None, "Go to rescan").on_press(go_to_rescan))
                    .push(button::secondary(Some(icon::cross_icon()), "Dismiss").on_press(dismiss)),
            ),
    )
    .padding(25)
    .style(theme::card::border)
    .into()
}

/// Unconfirmed balance line: `+ <amount> unconfirmed`, with optional fiat value.
pub fn unconfirmed_balance<'a, M: 'a>(
    amount: &'a bitcoin::Amount,
    fiat: Option<String>,
) -> Element<'a, M> {
    Row::new()
        .spacing(10)
        .align_y(Alignment::Center)
        .push(
            text("+")
                .size(legacy::H3_SIZE)
                .style(theme::text::secondary),
        )
        .push(unconfirmed_amount_with_size(amount, legacy::H3_SIZE))
        .push(
            text("unconfirmed")
                .size(legacy::H3_SIZE)
                .style(theme::text::secondary),
        )
        .push_maybe(fiat.map(|fiat| {
            Row::new()
                .align_y(Alignment::Center)
                .push(Space::with_width(10)) // total spacing = 20 including row spacing
                .push(text(fiat).size(legacy::H4_SIZE).color(color::GREY_3))
        }))
        .wrap()
        .into()
}

/// Progress shown while the wallet is not yet synced.
pub enum SyncProgress {
    Blockchain(f64),
    FullScan,
    Transactions,
}

/// Wallet balance amount, with optional fiat value. While `syncing` the amount
/// blinks (the fiat value is hidden); pair it with [`syncing`] for the progress
/// line.
pub fn balance<'a, M: Clone + 'a>(
    amount: &'a bitcoin::Amount,
    fiat: Option<String>,
    syncing: bool,
) -> Element<'a, M> {
    if syncing {
        Row::new()
            .push(spinner::Carousel::new(
                Duration::from_millis(1000),
                vec![
                    amount_with_font(amount, legacy::H1_SPEC),
                    amount_with_font_blink(amount, legacy::H1_SPEC),
                ],
            ))
            .wrap()
            .into()
    } else {
        Row::new()
            .align_y(Alignment::Center)
            .push(amount_with_font(amount, legacy::H1_SPEC))
            .push_maybe(fiat.map(|fiat| {
                Row::new()
                    .align_y(Alignment::Center)
                    .push(Space::with_width(20))
                    .push(
                        text(fiat)
                            .font(MANROPE_MEDIUM)
                            .size(legacy::H2_SIZE)
                            .color(color::GREY_2),
                    )
            }))
            .wrap()
            .into()
    }
}

/// Sync-progress line shown below the balance while the wallet catches up.
pub fn syncing<'a, M: Clone + 'a>(progress: SyncProgress) -> Element<'a, M> {
    Row::new()
        .push(
            text(match progress {
                SyncProgress::Blockchain(progress) => {
                    format!("Syncing blockchain ({:.2}%)", 100.0 * progress)
                }
                SyncProgress::FullScan => "Syncing".to_string(),
                SyncProgress::Transactions => "Checking for new transactions".to_string(),
            })
            .style(theme::text::secondary),
        )
        .push(spinner::typing_text_carousel(
            "...",
            true,
            Duration::from_millis(2000),
            |content| text(content).style(theme::text::secondary),
        ))
        .into()
}

/// Hint showing the time left before the first recovery path becomes available.
pub fn recovery_hint<'a, M: Clone + 'a>(units_left: String) -> Element<'a, M> {
    let content = Row::new()
        .spacing(15)
        .align_y(Alignment::Center)
        .push(
            legacy::h4_regular(format!(
                "≈ {units_left} left before first recovery path becomes available.",
            ))
            .width(Length::Fill),
        )
        .push(
            icon::tooltip_icon()
                .size(20)
                .style(theme::text::secondary)
                .width(Length::Fixed(20.0)),
        )
        .width(Length::Fill);
    home_hint(content)
}

/// Warning that a recovery path is or will soon be available, with a button
/// to reset the timelock for the affected coins.
pub fn recovery_warning<'a, M: Clone + 'static>(coin_count: usize, reset: M) -> Element<'a, M> {
    let content = Row::new()
        .push(icon::warning_fill_icon().size(icon::ICON_SIZE_M as u32))
        .push(
            legacy::h4_regular(format!(
                "Recovery path is or will soon be available for {coin_count} coin(s)."
            ))
            .width(Length::Fill),
        )
        .push(button::primary(Some(icon::arrow_repeat()), "Reset timelock").on_press(reset))
        .spacing(15)
        .align_y(Alignment::Center);
    home_warning(content)
}
