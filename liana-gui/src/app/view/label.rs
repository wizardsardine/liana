use iced::{widget::row, Alignment};

use liana::label::Label;
use liana_ui::{
    color,
    component::{
        button, form,
        label::editable_label,
        text::{apply, new, TextSpec},
    },
    widget::*,
};

use crate::{app::view, t};

#[derive(Debug, Clone, Copy)]
pub enum LabelSize {
    Body,
    Entry,
    Display,
}

impl LabelSize {
    pub fn spec(self) -> TextSpec {
        match self {
            LabelSize::Body => new::CAPTION_SPEC,
            LabelSize::Entry => new::H2_SPEC,
            LabelSize::Display => new::B0_SPEC,
        }
    }
}

pub fn label_field<'a>(
    labelled: Vec<String>,
    editing: Option<&'a form::Value<String>>,
    label: &Label,
    size: LabelSize,
) -> Element<'a, view::Message> {
    match editing {
        Some(editing) => label_editing(labelled, editing),
        None => {
            let value = match label {
                Label::Own(label) => label.clone(),
                Label::None
                | Label::Payment(_)
                | Label::Transaction(_)
                | Label::Address(_)
                | Label::Funding(_) => String::new(),
            };
            let edit = view::Message::Label(labelled, view::message::LabelMessage::Edited(value));
            editable_label(label, size.spec(), edit)
        }
    }
}

pub fn label_editing(
    labelled: Vec<String>,
    label: &form::Value<String>,
) -> Element<'_, view::Message> {
    let e: Element<view::LabelMessage> = Container::new(
        row!(
            form::Form::new(&t!("label-label"), label, view::LabelMessage::Edited)
                .warning(t!("label-invalid-length")),
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
    size: LabelSize,
) -> Element<'_, view::Message> {
    let label_text = label
        .cloned()
        .unwrap_or_else(|| t!("label-external-output"));

    let e: Element<view::LabelMessage> = Container::new(
        row![Container::new(
            apply(label_text, size.spec())
                .width(iced::Length::Fill)
                .color(color::GREY_1)
        ),]
        .spacing(5)
        .align_y(Alignment::Center),
    )
    .into();

    e.map(move |msg| view::Message::Label(labelled.clone(), msg))
}
