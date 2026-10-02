use iced::{advanced::text::Shaping, widget::row, Alignment};

use liana::label::{Label, LabelPrefix};
use liana_ui::{
    color,
    component::{
        button::{self, btn_add_label, btn_edit},
        form,
    },
    widget::*,
};

use crate::{app::view, t};

#[derive(Debug, Clone, Copy)]
#[repr(u32)]
pub enum LabelSize {
    Body = 16,
    Title = 24,
    Display = 32,
}

impl From<LabelSize> for u32 {
    fn from(size: LabelSize) -> Self {
        size as u32
    }
}

pub fn label_editable(
    labelled: Vec<String>,
    label: Option<&String>,
    size: u32,
) -> Element<'_, view::Message> {
    label_view(
        labelled,
        label.cloned().map(Label::Own).unwrap_or_default(),
        size,
    )
}

pub fn prefixed(prefix: LabelPrefix, label: &str) -> String {
    match prefix {
        LabelPrefix::From => t!("label-from", label = label),
        LabelPrefix::Address => t!("payment-address-label", label = label),
    }
}

/// Editable label showing an inherited label until the item gets its own.
fn label_view<'a>(labelled: Vec<String>, label: Label, size: u32) -> Element<'a, view::Message> {
    if let Some(text) = label.text(prefixed) {
        if !text.is_empty() {
            let value = match label {
                Label::Own(value) => value,
                Label::None | Label::Address(_) | Label::From(_) => String::new(),
            };
            return Container::new(
                row!(
                    iced::widget::Text::new(text)
                        .size(size)
                        .shaping(Shaping::Advanced),
                    btn_edit(Some(view::Message::Label(
                        labelled,
                        view::message::LabelMessage::Edited(value)
                    )))
                )
                .spacing(5)
                .align_y(Alignment::Center),
            )
            .into();
        }
    }
    let add_label_msg = Some(view::Message::Label(
        labelled,
        view::message::LabelMessage::Edited(String::default()),
    ));
    btn_add_label(add_label_msg).into()
}

pub fn label_field<'a>(
    labelled: Vec<String>,
    editing: Option<&'a form::Value<String>>,
    label: Label,
    size: LabelSize,
) -> Element<'a, view::Message> {
    match editing {
        Some(editing) => label_editing(labelled, editing, size.into()),
        None => label_view(labelled, label, size.into()),
    }
}

pub fn label_editing(
    labelled: Vec<String>,
    label: &form::Value<String>,
    size: u32,
) -> Element<'_, view::Message> {
    let e: Element<view::LabelMessage> = Container::new(
        row!(
            form::Form::new(&t!("label-label"), label, view::LabelMessage::Edited)
                .warning(t!("label-invalid-length"))
                .size(size)
                .padding(10),
            if label.valid {
                button::secondary(None, t!("btn-save"))
                    .on_press(view::message::LabelMessage::Confirm)
            } else {
                button::secondary(None, t!("btn-save"))
            },
            button::secondary(None, t!("btn-cancel")).on_press(view::message::LabelMessage::Cancel)
        )
        .spacing(5)
        .align_y(Alignment::Center),
    )
    .into();
    e.map(move |msg| view::Message::Label(labelled.clone(), msg))
}

pub fn label_non_editable(
    labelled: Vec<String>,
    label: Option<&String>,
    size: u32,
) -> Element<'_, view::Message> {
    let label_text = label
        .cloned()
        .unwrap_or_else(|| t!("label-external-output"));

    let e: Element<view::LabelMessage> = Container::new(
        row![Container::new(
            Text::new(label_text)
                .size(size)
                .width(iced::Length::Fill)
                .color(color::GREY_1)
        ),]
        .spacing(5)
        .align_y(Alignment::Center),
    )
    .into();

    e.map(move |msg| view::Message::Label(labelled.clone(), msg))
}
