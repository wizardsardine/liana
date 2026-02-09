use crate::state::message::Msg;
use iced::{Alignment, Length};
use liana_ui::{component::text, theme, widget::*};

use super::layout;

/// Loading view shown during wallet opening transition
pub fn loading_view() -> Element<'static, Msg> {
    layout(
        (0, 0),
        None,
        None,
        &[],
        Container::new(
            Column::new()
                .spacing(30)
                .align_x(Alignment::Center)
                .width(Length::Fill)
                .push(text::h2("Liana Business"))
                .push(text::p1_medium("Loading wallet...").style(theme::text::secondary)),
        ),
        true,
        None,
    )
}
