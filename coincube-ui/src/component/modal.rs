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
        form::{self, Value},
        text, tooltip,
    },
    icon,
    theme::{self, Theme},
};

use crate::widget::{Button, Column, Element, Row, Text};

pub const MODAL_WIDTH: u32 = 650;
pub const BTN_W: u32 = 500;
pub const BTN_H: u32 = 40;
pub const V_SPACING: u32 = 10;
pub const H_SPACING: u32 = 5;

pub fn widget_style(theme: &Theme, status: Status) -> Style {
    theme::button::secondary(theme, status)
}

/// Standard modal header used across the app: left-aligned title plus a
/// right-aligned close ×. Back navigation — when the modal has it — now
/// lives in a footer row rendered via [`back_button`] placed next to the
/// primary action (matches the Recovery Phrase / Border Wallet pattern).
/// The × is sized to match the chevron glyph used by [`back_button`] so
/// the two nav controls read as the same visual weight.
pub fn header<'a, Message, Close>(
    label: Option<String>,
    close_message: Option<Close>,
) -> Element<'a, Message>
where
    Close: 'static + Fn() -> Message,
    Message: Clone + 'static,
{
    let title = label.map(text::h3);
    // Build the close button without the `button::transparent` helper:
    // that helper assumes an icon+text pair and pads for the spacing
    // gap + empty label, which leaves a visible gutter between the ×
    // and the modal's right edge. A minimal `Button::new(icon)` with
    // zero padding sits flush to the right instead.
    let close = close_message.map(|m| {
        iced::widget::Button::new(icon::cross_icon())
            .style(theme::button::transparent)
            .padding(0)
            .on_press(m())
    });
    // Layout: [title] [flex-fill] [close]. The `width(Fill)` on the Row
    // is what makes the flex spacer actually push `close` to the right
    // edge of the modal — without it the Row defaults to `Shrink` and
    // both children collapse together in the middle.
    Row::new()
        .push(title)
        .push(Space::new().width(Length::Fill))
        .push(close)
        .align_y(Vertical::Center)
        .spacing(H_SPACING)
        .width(Length::Fill)
        .into()
}

/// Reusable "⟨ Back" button placed in a modal footer row (left-aligned,
/// next to the primary action). Matches the style already used by the
/// Recovery Phrase / Select Pattern / etc. Border Wallet wizard screens.
pub fn back_button<'a, Message, F>(on_press: F) -> Element<'a, Message>
where
    Message: Clone + 'a,
    F: 'static + Fn() -> Message,
{
    iced::widget::Button::new(
        Row::new()
            .push(icon::previous_icon().style(theme::text::secondary))
            .push(Space::new().width(Length::Fixed(4.0)))
            .push(text::p1_medium("Back").style(theme::text::secondary))
            .spacing(4)
            .align_y(Vertical::Center),
    )
    .style(theme::button::transparent)
    .on_press(on_press())
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
        .push(text::p1_bold(title))
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
        let line = Row::new().push(form).push(paste).spacing(V_SPACING);
        let col = Column::new()
            .push(row![
                text::p1_regular(label).color(color::WHITE),
                Space::new().width(Length::Fill)
            ])
            .push(line);
        let row = Row::new()
            .push(icon)
            .push(col)
            .align_y(Vertical::Center)
            .spacing(V_SPACING);

        Button::new(row).style(widget_style)
    } else {
        let row = Row::new()
            .push(icon)
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
        .push(icon.as_ref().map(|_| Space::new().width(H_SPACING)))
        .push(icon)
        .push(Space::new().width(H_SPACING))
        .push(designation)
        .push(message)
        .push(error)
        .push(Space::new().width(Length::Fill))
        .push(tt)
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
        .push(icon)
        .push(text::p1_regular(label))
        .push(Space::new().width(Length::Fill))
        .push(tt)
        .spacing(H_SPACING)
        .align_y(Vertical::Center)
        .height(BTN_H);

    let col = Column::new().push(row).push(error);

    let mut btn = Button::new(container(col)).style(widget_style).width(BTN_W);
    if let Some(msg) = on_press {
        let msg = msg();
        btn = btn.on_press(msg);
    }
    btn.into()
}
