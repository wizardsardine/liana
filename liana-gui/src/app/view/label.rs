use iced::{widget::row, Alignment};

use liana::label::{Label, LabelPrefix};
use liana_ui::{
    color,
    component::{
        button::{self, btn_icon_edit},
        form,
        text::{apply, new, TextSpec},
    },
    spacing::VSpacing,
    widget::*,
};

use crate::{app::view, t};

#[derive(Debug, Clone, Copy)]
pub enum LabelSize {
    Body,
    Title,
    Display,
}

impl LabelSize {
    fn spec(self) -> TextSpec {
        match self {
            LabelSize::Body => new::CAPTION_SPEC,
            LabelSize::Title => new::B1_SPEC,
            LabelSize::Display => new::B0_SPEC,
        }
    }
}

pub fn label_editable(
    labelled: Vec<String>,
    label: Option<&String>,
    size: LabelSize,
) -> Element<'_, view::Message> {
    label_view(
        labelled,
        label.cloned().map(Label::Own).unwrap_or_default(),
        size,
    )
}

pub fn prefixed(prefix: LabelPrefix, label: &str) -> String {
    match prefix {
        LabelPrefix::Payment => t!("label-to", label = label),
        LabelPrefix::Transaction | LabelPrefix::Funding => t!("label-from", label = label),
        LabelPrefix::Address => t!("payment-address-label", label = label),
    }
}

/// Editable label showing an inherited label until the item gets its own.
fn label_view<'a>(
    labelled: Vec<String>,
    label: Label,
    size: LabelSize,
) -> Element<'a, view::Message> {
    let value = label.own().unwrap_or_default().to_string();
    let edit_msg = view::message::LabelMessage::Edited(value);
    let edit_msg = view::Message::Label(labelled, edit_msg);
    let label_txt = label
        .text(prefixed)
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| t!("common-no-label-parenthesized"));

    let label = apply(label_txt, size.spec());
    let btn = btn_icon_edit(Some(edit_msg));
    row![label, btn]
        .spacing(VSpacing::L)
        .align_y(Alignment::Center)
        .into()
}

pub fn label_field<'a>(
    labelled: Vec<String>,
    editing: Option<&'a form::Value<String>>,
    label: Label,
    size: LabelSize,
) -> Element<'a, view::Message> {
    match editing {
        Some(editing) => label_editing(labelled, editing),
        None => label_view(labelled, label, size),
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
