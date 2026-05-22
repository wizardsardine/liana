//! Shared Prev / Next pagination control used by the Spark, Liquid, and
//! Vault Transactions panels.
//!
//! Layout: `[< Prev]   Page N   [Next >]` centered inside a
//! `theme::card::simple` container so it matches the panel cards above.
//! Each side disables independently — Prev when on page 0, Next when the
//! last response returned fewer rows than `PAGE_SIZE`. Both disable while
//! a fetch is in flight so rapid double-clicks can't skip pages.

use iced::{alignment, Length};

use crate::{
    theme,
    widget::{Button, Container, Element, Row},
};

use super::text::{text, P1_SIZE};

pub fn pagination_controls<'a, Message: Clone + 'a>(
    on_prev: Message,
    on_next: Message,
    prev_enabled: bool,
    next_enabled: bool,
    processing: bool,
    current_page: u32,
) -> Element<'a, Message> {
    let interactive = !processing;
    let prev_button = Button::new(
        text("< Prev")
            .size(P1_SIZE)
            .align_x(alignment::Horizontal::Center)
            .width(Length::Fill),
    )
    .width(Length::Fixed(120.0))
    .padding(12)
    .style(theme::button::transparent_border)
    .on_press_maybe((prev_enabled && interactive).then_some(on_prev));

    let next_button = Button::new(
        text("Next >")
            .size(P1_SIZE)
            .align_x(alignment::Horizontal::Center)
            .width(Length::Fill),
    )
    .width(Length::Fixed(120.0))
    .padding(12)
    .style(theme::button::transparent_border)
    .on_press_maybe((next_enabled && interactive).then_some(on_next));

    let label = if processing {
        "Loading…".to_string()
    } else {
        format!("Page {}", current_page.saturating_add(1))
    };

    let row = Row::new()
        .align_y(iced::Alignment::Center)
        .spacing(20)
        .push(prev_button)
        .push(
            Container::new(text(label).size(P1_SIZE))
                .center_x(Length::Fill)
                .width(Length::Fill),
        )
        .push(next_button);

    Container::new(row)
        .width(Length::Fill)
        .padding(8)
        .style(theme::card::simple)
        .into()
}
