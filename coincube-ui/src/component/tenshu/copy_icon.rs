//! The square copy button (`.ids .copy`): the canvas's 16px copy glyph in a
//! 28px bordered square, with a "Copy …" tooltip on hover. Activity's detail
//! panel and the payment result page both use it, so every inline copy in the
//! app looks and behaves the same.

use std::borrow::Cow;

use iced::advanced::text::Shaping;
use iced::widget::{button, svg, tooltip};
use iced::{Background, Border, Length};

use crate::font;
use crate::theme::{self, Theme};
use crate::widget::{Button, Container, Element};

/// The canvas's 18px-grid copy stroke icon, tinted like `currentColor`.
pub const COPY_GLYPH: &str = concat!(
    r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 18 18" fill="none" stroke="#fff" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">"##,
    r#"<rect x="6.5" y="6.5" width="8.5" height="8.5" rx="1.5"/><path d="M11.5 6.5V4.25c0-.7-.55-1.25-1.25-1.25h-6c-.7 0-1.25.55-1.25 1.25v6c0 .7.55 1.25 1.25 1.25H6.5"/>"#,
    "</svg>"
);

const GLYPH_SIZE: f32 = 16.0;
/// The button's side.
pub const SIDE: f32 = 28.0;

/// The glyph centred in the button's square, ready to wrap in a button.
pub fn content<'a, M: 'a>() -> Element<'a, M> {
    Container::new(
        svg(svg::Handle::from_memory(COPY_GLYPH.as_bytes()))
            .width(Length::Fixed(GLYPH_SIZE))
            .height(Length::Fixed(GLYPH_SIZE))
            .style(|theme: &Theme, _| svg::Style {
                color: Some(theme.colors.shell.ink),
            }),
    )
    .center_x(Length::Fixed(SIDE))
    .center_y(Length::Fixed(SIDE))
    .into()
}

/// Hairline square; the selected wash on hover and press.
pub fn style(theme: &Theme, status: button::Status) -> button::Style {
    let shell = theme.colors.shell;
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: hovered.then_some(Background::Color(shell.selected)),
        text_color: shell.ink,
        border: Border {
            color: shell.line,
            width: 1.0,
            radius: 6.0.into(),
        },
        ..Default::default()
    }
}

/// Wraps a copy button in its hover tooltip (`tip`, such as "Copy Payment
/// hash"), shown to the button's left.
pub fn with_tooltip<'a, M: 'a>(
    button: impl Into<Element<'a, M>>,
    tip: impl Into<Cow<'a, str>>,
) -> Element<'a, M> {
    tooltip(
        button,
        Container::new(
            iced::widget::text(tip.into())
                .shaping(Shaping::Advanced)
                .size(12.0)
                .font(font::REGULAR)
                .style(|theme: &Theme| iced::widget::text::Style {
                    color: Some(theme.colors.shell.muted),
                }),
        )
        .padding([6, 10])
        .style(theme::container::shell_menu),
        tooltip::Position::Left,
    )
    .into()
}

/// A plain (unfocusable) copy button sending `on_press`, with its tooltip.
pub fn copy_icon<'a, M: Clone + 'a>(tip: impl Into<Cow<'a, str>>, on_press: M) -> Element<'a, M> {
    with_tooltip(
        Button::new(content())
            .padding(0)
            .style(style)
            .on_press(on_press),
        tip,
    )
}
