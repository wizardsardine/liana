use iced::{widget::row, Alignment};

use liana_ui::{
    color,
    component::{button, form},
    font, icon,
    widget::*,
};

use crate::app::view;

pub fn label_editable(
    labelled: String,
    label: Option<&String>,
    size: u16,
) -> Element<'_, view::Message> {
    if let Some(label) = label {
        if !label.is_empty() {
            return Container::new(
                row!(
                    iced::widget::Text::new(label).size(size).font(font::BOLD),
                    button::primary(Some(icon::pencil_icon()), "Edit").on_press(
                        view::Message::Label(
                            labelled,
                            view::message::LabelMessage::Edited(label.to_string())
                        )
                    )
                )
                .spacing(5)
                .align_items(Alignment::Center),
            )
            .into();
        }
    }
    Container::new(
        row!(
            iced::widget::Text::new("Add Label")
                .size(size)
                .font(font::BOLD)
                .style(color::GREY_3),
            button::primary(Some(icon::pencil_icon()), "Edit").on_press(view::Message::Label(
                labelled,
                view::message::LabelMessage::Edited(String::default())
            ))
        )
        .spacing(5)
        .align_items(Alignment::Center),
    )
    .into()
}

pub fn label_editing(
    labelled: String,
    label: &form::Value<String>,
    size: u16,
) -> Element<view::Message> {
    let e: Element<view::LabelMessage> = Container::new(
        row!(
            form::Form::new("Label", label, view::LabelMessage::Edited)
                .warning("Invalid label length, cannot be superior to 100")
                .size(size)
                .padding(10),
            button::primary(None, "Save").on_press(view::message::LabelMessage::Confirm),
            button::primary(None, "Cancel").on_press(view::message::LabelMessage::Cancel)
        )
        .spacing(5)
        .align_items(Alignment::Center),
    )
    .into();
    e.map(move |msg| view::Message::Label(labelled.clone(), msg))
}
