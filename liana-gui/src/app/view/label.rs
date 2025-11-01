use iced::{advanced::text::Shaping, widget::row, Alignment};

use liana_ui::{
    color,
    component::{button, form},
    icon,
    widget::*,
};

use crate::app::view;

pub fn label_editable(
    labelled: Vec<String>,
    label: Option<&String>,
    size: u16,
) -> Element<'_, view::Message> {
    if let Some(label) = label {
        if !label.is_empty() {
            return Container::new(
                row!(
                    iced::widget::Text::new(label)
                        .size(size)
                        .shaping(Shaping::Advanced),
                    button::secondary(Some(icon::pencil_icon()), "Edit").on_press(
                        view::Message::Label(
                            labelled,
                            view::message::LabelMessage::Edited(label.to_string())
                        )
                    )
                )
                .spacing(5)
                .align_y(Alignment::Center),
            )
            .into();
        }
    }
    Container::new(
        button::secondary(Some(icon::pencil_icon()), "Add label").on_press(view::Message::Label(
            labelled,
            view::message::LabelMessage::Edited(String::default()),
        )),
    )
    .into()
}

pub fn label_editing(
    labelled: Vec<String>,
    label: &form::Value<String>,
    size: u16,
) -> Element<view::Message> {
    let e: Element<view::LabelMessage> = Container::new(
        row!(
            form::Form::new("Label", label, view::LabelMessage::Edited)
                .warning("Invalid label length, cannot be superior to 100")
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
    size: u16,
) -> Element<view::Message> {
    let label_text = label.map(|s| s.as_str()).unwrap_or("(External Ouput)");

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
