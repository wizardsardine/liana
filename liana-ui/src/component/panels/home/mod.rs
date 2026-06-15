pub mod payment;

use std::time::Duration;

use bitcoin::Amount;
use iced::{
    widget::{column, row, Space},
    Alignment, Length,
};

use crate::{
    component::{
        amount::{
            amount_with_fiat, amount_with_font, amount_with_font_blink, AmountSize, FiatAmount,
        },
        button::{btn_dismiss, btn_go_to_rescan, btn_reset_timelock},
        card::{self, info, warning},
        spinner,
        text::{
            new::{self, D2_SPEC},
            text, TextSpec,
        },
    },
    icon, theme,
    widget::{Element, SpaceExt},
};

const RESCAN_WARNING: &str = "As this wallet was restored from a backup, you may need to rescan the blockchain to see past transactions.";

/// Card prompting a rescan after restoring a wallet from backup.
pub fn rescan_warning<'a, M: Clone + 'static>(go_to_rescan: M, dismiss: M) -> Element<'a, M> {
    let icon = icon::warning_fill_icon().size(icon::ICON_SIZE_M as u32);
    let msg = row![
        Space::with_width(10),
        icon,
        Space::with_width(15),
        new::h3(RESCAN_WARNING),
    ]
    .align_y(Alignment::Center);

    let buttons = row![
        Space::fill_width(),
        btn_go_to_rescan(Some(go_to_rescan)),
        btn_dismiss(Some(dismiss)),
    ]
    .spacing(5);

    card::soft_warning(column![msg, buttons].spacing(10))
}

/// Unconfirmed balance line: `+ <amount> unconfirmed`, with optional fiat value.
pub fn unconfirmed_balance<'a, M: 'a>(
    amount: &'a bitcoin::Amount,
    fiat: Option<String>,
) -> Element<'a, M> {
    let fiat = fiat.map(|fiat| {
        row![
            Space::with_width(10), // total spacing = 20 including row spacing
            new::h3(fiat).style(|t| theme::amount::zeroes(t, false))
        ]
        .align_y(Alignment::Center)
    });

    row![
        new::h3("+").style(|t| theme::amount::sats(t, false)),
        amount_with_font(amount, new::H3_SPEC),
        new::h3("unconfirmed").style(|t| theme::amount::sats(t, false)),
        fiat
    ]
    .spacing(10)
    .align_y(Alignment::Center)
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
pub fn balance<'a, M: Clone + 'a, F: Fn(Amount) -> FiatAmount>(
    amount: &'a bitcoin::Amount,
    to_fiat: Option<F>,
    syncing: bool,
) -> Element<'a, M> {
    const BALANCE_FONT: TextSpec = D2_SPEC;
    if syncing {
        let blinking = spinner::Carousel::new(
            Duration::from_millis(1000),
            vec![
                amount_with_font(amount, BALANCE_FONT),
                amount_with_font_blink(amount, BALANCE_FONT),
            ],
        );
        row![blinking].wrap().into()
    } else {
        amount_with_fiat(amount, to_fiat, AmountSize::L)
    }
}

/// Sync-progress line shown below the balance while the wallet catches up.
pub fn syncing<'a, M: Clone + 'a>(progress: SyncProgress) -> Element<'a, M> {
    let label = match progress {
        SyncProgress::Blockchain(progress) => {
            format!("Syncing blockchain ({:.2}%)", 100.0 * progress)
        }
        SyncProgress::FullScan => "Syncing".to_string(),
        SyncProgress::Transactions => "Checking for new transactions".to_string(),
    };

    row![
        new::h3(label).style(|t| theme::amount::sats(t, false)),
        spinner::typing_text_carousel("...", true, Duration::from_millis(2000), |content| text(
            content
        )
        .style(|t| theme::amount::sats(t, false)),),
    ]
    .align_y(Alignment::End)
    .into()
}

/// Hint showing the time left before the first recovery path becomes available.
pub fn recovery_hint<'a, M: Clone + 'a>(units_left: String) -> Element<'a, M> {
    let icon = icon::tooltip_icon().size(icon::ICON_SIZE_M as u32);
    let content = row![
        new::h3(format!(
            "≈ {units_left} left before first recovery path becomes available.",
        ))
        .width(Length::Fill),
        icon,
    ]
    .spacing(15)
    .align_y(Alignment::Center)
    .width(Length::Fill);
    info(content)
}

/// Warning that a recovery path is or will soon be available, with a button
/// to reset the timelock for the affected coins.
pub fn recovery_warning<'a, M: Clone + 'static>(coin_count: usize, reset: M) -> Element<'a, M> {
    let content = row![
        Space::with_width(10),
        icon::warning_fill_icon().size(icon::ICON_SIZE_M as u32),
        Space::with_width(15),
        new::h3(format!(
            "Recovery path is or will soon be available for {coin_count} coin(s)."
        ))
        .width(Length::Fill),
        btn_reset_timelock(Some(reset.clone())),
    ]
    .align_y(Alignment::Center);
    warning(content)
}
