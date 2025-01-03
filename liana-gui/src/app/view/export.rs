use iced::{
    widget::{progress_bar, Column, Container, Row, Space},
    Length,
};
use liana_ui::{
    component::{
        button, card,
        text::{h4_bold, text},
    },
    widget::Element,
};

use crate::export::{Error, ExportMessage};
use crate::{app::view::message::Message, export::ExportState};

/// Return the modal view for an export task
pub fn export_modal<'a>(
    state: &ExportState,
    error: Option<&'a Error>,
    export_type: &str,
) -> Element<'a, Message> {
    let button = match state {
        ExportState::Started | ExportState::Progress(_) => {
            Some(button::secondary(None, "Cancel").on_press(ExportMessage::UserStop.into()))
        }
        ExportState::Ended | ExportState::TimedOut | ExportState::Aborted => {
            Some(button::secondary(None, "Close").on_press(ExportMessage::Close.into()))
        }
        _ => None,
    };
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
            ExportState::Progress(p) => format!("Progress: {}%", p.round()),
            ExportState::TimedOut => "Export failed: timeout".into(),
            ExportState::Aborted => "Export canceled".into(),
            ExportState::Ended => "Export successful!".into(),
            ExportState::Closed => "".into(),
        }
    };
    let p = match state {
        ExportState::Init => 0.0,
        ExportState::ChoosePath | ExportState::Path(_) | ExportState::Started => 5.0,
        ExportState::Progress(p) => *p,
        ExportState::TimedOut | ExportState::Aborted | ExportState::Ended | ExportState::Closed => {
            100.0
        }
    };
    let progress_bar_row = Row::new()
        .push(Space::with_width(30))
        .push(progress_bar(0.0..=100.0, p))
        .push(Space::with_width(30));
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
            .push(progress_bar_row)
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
