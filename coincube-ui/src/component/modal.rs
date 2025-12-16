use iced::{
    alignment::{Horizontal, Vertical},
    widget::{
        button::{Status, Style},
        column, container, row, Space,
    },
    Length,
};

use crate::{
    color,
    component::{
        button,
        form::{self, Value},
        text, tooltip,
    },
    icon,
    theme::{self, Theme},
};

use crate::widget::{Button, Column, ColumnExt, Element, Row, RowExt, Text};

pub const MODAL_WIDTH: u32 = 650;
pub const BTN_W: u32 = 500;
pub const BTN_H: u32 = 40;
pub const V_SPACING: u32 = 10;
pub const H_SPACING: u32 = 5;

pub fn widget_style(theme: &Theme, status: Status) -> Style {
    theme::button::secondary(theme, status)
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
        .push(Space::new().width(Length::Fill))
        .push_maybe(close)
        .align_y(Vertical::Center)
        .into()
}

pub fn optional_section<'a, Message, Collapse, Fold>(
    collapsed: bool,
    title: String,
    collapse: Collapse,
    fold: Fold,
) -> Element<'a, Message>
where
    Collapse: 'static + Fn() -> Message,
    Fold: 'static + Fn() -> Message,
    Message: Clone + 'static,
{
    let icon = if collapsed {
        icon::collapsed_icon().style(theme::text::secondary)
    } else {
        icon::collapse_icon().style(theme::text::secondary)
    };

    let msg = if !collapsed { collapse() } else { fold() };

    let row = Row::new()
        .push(text::p1_bold(&title))
        .push(icon)
        .align_y(Vertical::Center)
        .spacing(H_SPACING);

    Button::new(row)
        .style(theme::button::transparent_border)
        .on_press(msg)
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
    let paste =
        paste_message.map(|m| Button::new(icon::paste_icon().color(color::BLACK)).on_press(m()));

    let icon = icon.map(|i| i.color(color::WHITE));

    if collapsed {
        let line = Row::new().push(form).push_maybe(paste).spacing(V_SPACING);
        let col = Column::new()
            .push(row![
                text::p1_regular(label).color(color::WHITE),
                Space::new().width(Length::Fill)
            ])
            .push(line);
        let row = Row::new()
            .push_maybe(icon)
            .push(col)
            .align_y(Vertical::Center)
            .spacing(V_SPACING);

        Button::new(row).style(widget_style)
    } else {
        let row = Row::new()
            .push_maybe(icon)
            .push(text::p1_regular(label))
            .height(BTN_H)
            .spacing(V_SPACING)
            .align_y(Vertical::Center);
        Button::new(row)
            .on_press(collapse_message())
            .style(widget_style)
    }
    .width(BTN_W)
    .into()
}

pub fn key_entry<'a, Message, OnClick>(
    icon: Option<Text<'static>>,
    name: String,
    fingerprint: Option<String>,
    tooltip_str: Option<&'static str>,
    error: Option<String>,
    mut message: Option<String>,
    on_press: Option<OnClick>,
) -> Element<'a, Message>
where
    OnClick: 'static + Fn() -> Message,
    Message: Clone + 'static,
{
    if error.is_some() {
        message = None;
    }
    let message = message.map(text::p2_regular);
    let error = error.map(|e| text::p1_regular(e).color(color::ORANGE));
    let tt = tooltip_str.map(|s| tooltip(s));

    let designation = column![
        text::p1_bold(name),
        text::p1_regular(fingerprint.unwrap_or(" - ".to_string()))
    ]
    .align_x(Horizontal::Left)
    .width(200);
    let row = Row::new()
        .push_maybe(icon.as_ref().map(|_| Space::new().width(H_SPACING)))
        .push_maybe(icon)
        .push(Space::new().width(H_SPACING))
        .push(designation)
        .push_maybe(message)
        .push_maybe(error)
        .push(Space::new().width(Length::Fill))
        .push_maybe(tt)
        .align_y(Vertical::Center)
        .spacing(V_SPACING);
    let mut btn = Button::new(row).style(widget_style).width(BTN_W);
    if let Some(msg) = on_press {
        btn = btn.on_press(msg())
    }
    btn.into()
}

pub fn button_entry<'a, Message, OnClick>(
    icon: Option<Text<'static>>,
    label: &'a str,
    tooltip_str: Option<&'static str>,
    error: Option<String>,
    on_press: Option<OnClick>,
) -> Element<'a, Message>
where
    OnClick: 'static + Fn() -> Message,
    Message: Clone + 'static,
{
    let error = error.map(|e| {
        row![
            text::p1_regular(e).color(color::ORANGE),
            Space::new().width(Length::Fill)
        ]
    });

    let tt = tooltip_str.map(|s| tooltip(s));

    let row = Row::new()
        .push_maybe(icon)
        .push(text::p1_regular(label))
        .push(Space::new().width(Length::Fill))
        .push_maybe(tt)
        .spacing(H_SPACING)
        .align_y(Vertical::Center)
        .height(BTN_H);

    let col = Column::new().push(row).push_maybe(error);

    let mut btn = Button::new(container(col)).style(widget_style).width(BTN_W);
    if let Some(msg) = on_press {
        let msg = msg();
        btn = btn.on_press(msg);
    }
    btn.into()
}
