use iced::{
    alignment::Horizontal,
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

use crate::export::{Error, ImportExportMessage, ImportExportState};

/// Return the modal view for an export task
pub fn export_modal<'a, Message: From<ImportExportMessage> + Clone + 'a>(
    state: &ImportExportState,
    error: Option<&'a Error>,
    export_type: &str,
) -> Element<'a, Message> {
    let button = match state {
        ImportExportState::Started | ImportExportState::Progress(_) => {
            Some(button::secondary(None, "Cancel").on_press(ImportExportMessage::UserStop.into()))
        }
        ImportExportState::Ended | ImportExportState::TimedOut | ImportExportState::Aborted => {
            Some(button::secondary(None, "Close").on_press(ImportExportMessage::Close.into()))
        }
        _ => None,
    };
    let msg = if let Some(error) = error {
        format!("{:?}", error)
    } else {
        match state {
            ImportExportState::Init => "".to_string(),
            ImportExportState::ChoosePath => {
                "Select the path you want to export in the popup window...".into()
            }
            ImportExportState::Path(_) => "".into(),
            ImportExportState::Started => "Starting export...".into(),
            ImportExportState::Progress(p) => format!("Progress: {}%", p.round()),
            ImportExportState::TimedOut => "Export failed: timeout".into(),
            ImportExportState::Aborted => "Export canceled".into(),
            ImportExportState::Ended => "Export successful!".into(),
            ImportExportState::Closed => "".into(),
        }
    };
    let p = match state {
        ImportExportState::Init => 0.0,
        ImportExportState::ChoosePath | ImportExportState::Path(_) | ImportExportState::Started => {
            5.0
        }
        ImportExportState::Progress(p) => *p,
        ImportExportState::TimedOut
        | ImportExportState::Aborted
        | ImportExportState::Ended
        | ImportExportState::Closed => 100.0,
    };
    let progress_bar_row = Row::new()
        .push(Space::with_width(30))
        .push(progress_bar(0.0..=100.0, p))
        .push(Space::with_width(30));
    card::simple(
        Column::new()
            .spacing(10)
            .push(Container::new(h4_bold(export_type)).width(Length::Fill))
            .push(Space::with_height(Length::Fill))
            .push(progress_bar_row)
            .push(Space::with_height(Length::Fill))
            .push(Row::new().push(text(msg)))
            .push(Space::with_height(Length::Fill))
            .push_maybe(button.map(|b| {
                Container::new(b)
                    .align_x(Horizontal::Right)
                    .width(Length::Fill)
            }))
            .push(Space::with_height(5)),
    )
    .width(Length::Fixed(500.0))
    .height(Length::Fixed(220.0))
    .into()
}
