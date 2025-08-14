use iced::{
    alignment::Vertical,
    widget::{
        button::{Status, Style},
        row, Space,
    },
    Length,
};

use crate::{
    color,
    component::{
        button,
        form::{self, Value},
        text,
    },
    icon,
    theme::Theme,
};

use super::{Button, Column, Element, Row, Text};

pub const MODAL_WIDTH: u16 = 550;
pub const BTN_W: u16 = 400;
pub const BTN_H: u16 = 40;
pub const SPACING: u16 = 10;

fn widget_style(theme: &Theme, status: Status) -> Style {
    crate::theme::button::secondary(theme, status)
}

pub fn header<'a, Message, Back, Close>(
    label: Option<String>,
    back_message: Option<Back>,
    close_message: Option<Close>,
) -> Element<'a, Message>
where
    Back: 'static + Fn() -> Message,
    Close: 'static + Fn() -> Message,
    Message: Clone + 'static,
{
    let back = back_message
        .map(|m| button::transparent(Some(icon::arrow_back().size(25)), "").on_press(m()));
    let title = label.map(text::h3);
    let close = close_message
        .map(|m| button::transparent(Some(icon::cross_icon().size(40)), "").on_press(m()));
    Row::new()
        .push_maybe(back)
        .push_maybe(title)
        .push(Space::with_width(Length::Fill))
        .push_maybe(close)
        .align_y(Vertical::Center)
        .into()
}

#[allow(clippy::too_many_arguments)]
pub fn collapsible_input_button<'a, Message, Paste, Collapse, Input>(
    collapsed: bool,
    icon: Option<Text<'static>>,
    label: String,
    input_placeholder: String,
    input_value: &Value<String>,
    input_message: Option<Input>,
    paste_message: Option<Paste>,
    collapse_message: Collapse,
) -> Element<'a, Message>
where
    Input: 'static + Fn(String) -> Message,
    Paste: 'static + Fn() -> Message,
    Collapse: 'static + Fn() -> Message,
    Message: Clone + 'static,
{
    let form = if let Some(input_message) = input_message {
        form::Form::new(&input_placeholder, input_value, input_message)
    } else {
        form::Form::new_disabled(&input_placeholder, input_value)
    }
    .padding(10);
    let paste = paste_message
        .map(|m| Button::new(icon::clipboard_icon().color(color::BLACK)).on_press(m()));

    let icon = icon.map(|i| i.color(color::WHITE));

    if collapsed {
        let line = Row::new().push(form).push_maybe(paste).spacing(SPACING);
        let col = Column::new()
            .push(row![
                text::p1_regular(label).color(color::WHITE),
                Space::with_width(Length::Fill)
            ])
            .push(line);
        let row = Row::new()
            .push_maybe(icon)
            .push(col)
            .align_y(Vertical::Center)
            .spacing(SPACING);

        Button::new(row).style(widget_style)
    } else {
        let row = Row::new()
            .push_maybe(icon)
            .push(text::p1_regular(label))
            .height(BTN_H)
            .spacing(SPACING)
            .align_y(Vertical::Center);
        Button::new(row)
            .on_press(collapse_message())
            .style(widget_style)
    }
    .width(BTN_W)
    .into()
}
