use iced::{
    widget::{Button, Column, Container, Row, Space},
    Length,
};
use liana_ui::{
    component::{
        card,
        text::{h4_bold, text},
    },
    theme,
    widget::Element,
};

use crate::app::{export::Error, state::export::ExportState};
use crate::app::{state::export::ExportMessage, view::message::Message};

/// Return the modal view for an export task
pub fn export_modal<'a>(
    state: &ExportState,
    error: Option<&'a Error>,
    export_type: &str,
) -> Element<'a, Message> {
    let button = match state {
        ExportState::Started | ExportState::Progress(_) => {
            Some(Button::new("Cancel").on_press(ExportMessage::UserStop.into()))
        }
        ExportState::Ended | ExportState::TimedOut | ExportState::Aborted => {
            Some(Button::new("Close").on_press(ExportMessage::Close.into()))
        }
        _ => None,
    }
    .map(|b| b.height(32).style(theme::Button::Primary));
    let msg = if let Some(error) = error {
        format!("{:?}", error)
    } else {
        match state {
            ExportState::Init => "".to_string(),
            ExportState::ChoosePath => {
                "Select the path you want to export in the popup window...".into()
            }
            ExportState::Path(_) => "".into(),
            ExportState::Started => "Starting export...".into(),
            ExportState::Progress(p) => format!("Progress: {}%", p),
            ExportState::TimedOut => "Export failed: timeout".into(),
            ExportState::Aborted => "Export canceled".into(),
            ExportState::Ended => "Export successfull!".into(),
            ExportState::Closed => "".into(),
        }
    };
    let button_row = button.map(|b| {
        Row::new()
            .push(Space::with_width(Length::Fill))
            .push(b)
            .push(Space::with_width(Length::Fill))
    });
    card::simple(
        Column::new()
            .spacing(10)
            .push(Container::new(h4_bold(format!("Export {export_type}"))).width(Length::Fill))
            .push(Space::with_height(Length::Fill))
            .push(
                Row::new()
                    .push(Space::with_width(Length::Fill))
                    .push(text(msg))
                    .push(Space::with_width(Length::Fill)),
            )
            .push(Space::with_height(Length::Fill))
            .push_maybe(button_row)
            .push(Space::with_height(5)),
    )
    .width(Length::Fixed(500.0))
    .height(Length::Fixed(220.0))
    .into()
}
