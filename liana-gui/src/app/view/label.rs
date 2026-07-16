use iced::{advanced::text::Shaping, widget::row, Alignment};

use liana_ui::{
    color,
    component::{
        button::{self, btn_add_label, btn_edit},
        form,
        label::LABEL_LENGTH_WARNING,
    },
    widget::*,
};

use crate::app::view;

pub fn label_editable(
    labelled: Vec<String>,
    label: Option<&String>,
    size: u32,
) -> Element<'_, view::Message> {
    if let Some(label) = label {
        if !label.is_empty() {
            return Container::new(
                row!(
                    iced::widget::Text::new(label)
                        .size(size)
                        .shaping(Shaping::Advanced),
                    btn_edit(Some(view::Message::Label(
                        labelled,
                        view::message::LabelMessage::Edited(label.to_string())
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

pub fn label_editing(
    labelled: Vec<String>,
    label: &form::Value<String>,
    size: u32,
) -> Element<'_, view::Message> {
    let e: Element<view::LabelMessage> = Container::new(
        row!(
            form::Form::new("Label", label, view::LabelMessage::Edited)
                .warning(LABEL_LENGTH_WARNING)
                .size(size)
                .padding(10),
            if label.valid {
                button::secondary(None, "Save").on_press(view::message::LabelMessage::Confirm)
            } else {
                button::secondary(None, "Save")
            },
            button::secondary(None, "Cancel").on_press(view::message::LabelMessage::Cancel)
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
    let label_text = label.map(|s| s.as_str()).unwrap_or("(External Output)");

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
