use iced::{widget::Space, Alignment, Length};

use crate::{
    component::{button, text::text},
    icon, theme,
    widget::{Column, Container, Element, Row, SpaceExt},
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
