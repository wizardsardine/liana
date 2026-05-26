use std::time::Duration;

use iced::{widget::Space, Alignment, Length};

use crate::{
    color,
    component::{
        amount::{amount_with_font, amount_with_font_blink},
        button, spinner,
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
