use crate::{
    color,
    component::text::{new, text},
    icon, theme,
    widget::*,
};
use iced::{
    widget::{button, row},
    Alignment, Length, Padding,
};
const CARD_PADDING: [u16; 2] = [15, 30];
pub(crate) const SOFT_CARD_PADDING: [u16; 2] = [13, 16];

macro_rules! cards {
    ($($entry:tt),* $(,)?) => {
        $( cards!(@one $entry); )*
    };
    // Runtime-padding card: the caller supplies the padding, e.g. `(flat, pad_arg)`.
    (@one ($name:ident, pad_arg)) => {
        pub fn $name<'a, T: 'a, C: Into<Element<'a, T>>>(
            content: C,
            padding: impl Into<Padding>,
        ) -> Container<'a, T> {
            Container::new(content)
                .padding(padding)
                .style(theme::card::$name)
        }
    };
    // Element-returning card, e.g. `(warning, elem, 20)`.
    (@one ($name:ident, elem, $padding:expr)) => {
        pub fn $name<'a, T: 'a, C: Into<Element<'a, T>>>(content: C) -> Element<'a, T> {
            Container::new(content)
                .padding($padding)
                .style(theme::card::$name)
                .into()
        }
    };
    // Constant-padding card, e.g. `(modal, 15)`.
    (@one ($name:ident, $padding:expr)) => {
        pub fn $name<'a, T: 'a, C: Into<Element<'a, T>>>(content: C) -> Container<'a, T> {
            Container::new(content)
                .padding($padding)
                .style(theme::card::$name)
        }
    };
    // Bare ident defaults to the soft-card padding.
    (@one $name:ident) => {
        cards!(@one ($name, SOFT_CARD_PADDING));
    };
}

cards! {
    soft_warning,
    success,
    (modal, 15),
    (simple, 15),
    (invalid, 15),
    (section, 0),
    (flat, pad_arg),
    (warning, elem, 20),
}

/// display an error card with the message and the error in a tooltip.
pub fn legacy_warning<'a, T: 'a>(message: String) -> Container<'a, T> {
    Container::new(
        Row::new()
            .spacing(20)
            .align_y(iced::Alignment::Center)
            .push(icon::warning_icon())
            .push(text(message)),
    )
    .padding(15)
    .style(theme::card::legacy_warning)
}

/// display an error card with the message and the error in a tooltip.
pub fn error<'a, T: 'a>(message: &'static str, error: String) -> Container<'a, T> {
    Container::new(
        iced::widget::tooltip::Tooltip::new(
            Row::new()
                .spacing(20)
                .align_y(iced::Alignment::Center)
                .push(icon::warning_icon().color(color::RED))
                .push(text(message).color(color::RED)),
            Text::new(error),
            iced::widget::tooltip::Position::Bottom,
        )
        .style(theme::card::error),
    )
    .padding(15)
    .style(theme::card::error)
}

pub fn list_entry<'a, M>(content: Row<'a, M>, msg: Option<M>) -> Element<'a, M>
where
    M: Clone + 'a,
{
    list_entry_with_padding(content, msg, CARD_PADDING)
}

pub fn list_entry_with_padding<'a, M>(
    content: Row<'a, M>,
    msg: Option<M>,
    padding: impl Into<Padding>,
) -> Element<'a, M>
where
    M: Clone + 'a,
{
    button(content.align_y(Alignment::Center).padding(padding.into()))
        .on_press_maybe(msg)
        .style(theme::button::list_entry)
        .into()
}

pub fn info<'a, M: 'a>(body: impl std::fmt::Display + 'a) -> Container<'a, M> {
    Container::new(
        row![
            icon::tooltip_icon().size(16).style(theme::text::secondary),
            new::caption(body).style(theme::text::secondary),
        ]
        .spacing(10)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding(SOFT_CARD_PADDING)
    .style(theme::card::info)
}
