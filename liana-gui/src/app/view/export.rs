use iced::{
    alignment::{self, Horizontal},
    widget::{progress_bar, Column, Container, Row, Space},
    Length,
};
use liana_ui::{
    component::{
        button, card, form,
        text::{h4_bold, text},
    },
    icon,
    widget::{ColumnExt, Element, RowExt, SpaceExt},
};

use crate::export::ImportExportState;
use crate::export::{Error, ImportExportMessage, ImportExportType};

/// Return the modal view for an export task
pub fn export_modal<'a, Message: From<ImportExportMessage> + Clone + 'static>(
    state: &ImportExportState,
    error: Option<&'a Error>,
    title: &str,
    import_export_type: &ImportExportType,
    manual_path: &'a form::Value<String>,
) -> Element<'a, Message> {
    let cancel = match state {
        ImportExportState::Started | ImportExportState::Progress(_) => {
            Some(button::secondary(None, "Cancel").on_press(ImportExportMessage::UserStop.into()))
        }
        _ => None,
    }
    .map(Container::new);

    let cross = match state {
        ImportExportState::Ended | ImportExportState::TimedOut | ImportExportState::Aborted => {
            Some(
                button::transparent(Some(icon::cross_icon().size(30)), "")
                    .on_press(ImportExportMessage::Close.into()),
            )
        }
        _ => None,
    };

    let msg = if let Some(error) = error {
        format!("{error}")
    } else {
        match state {
            ImportExportState::Init => "".to_string(),
            ImportExportState::ChoosePath => {
                "Select the path you want to export in the popup window...".into()
            }
            ImportExportState::ManualPath => {
                "The file picker returned no path. Enter one manually or close this dialog.".into()
            }
            ImportExportState::Path(_) => "".into(),
            ImportExportState::Started => "Starting export...".into(),
            ImportExportState::Progress(p) => format!("Progress: {}%", p.round()),
            ImportExportState::TimedOut => "Export failed: timeout".into(),
            ImportExportState::Aborted => "Export canceled".into(),
            ImportExportState::Ended => import_export_type.end_message().into(),
            ImportExportState::Closed => "".into(),
        }
    };
    let labels_btn = (
        "Labels conflict, what do you want to do?".to_string(),
        Some(Container::new(
            Row::new()
                .push(
                    button::secondary(None, "Overwrite")
                        .on_press(ImportExportMessage::Overwrite.into()),
                )
                .push(Space::with_width(30))
                .push(
                    button::secondary(None, "Ignore").on_press(ImportExportMessage::Ignore.into()),
                ),
        )),
    );
    let aliases_btn = (
        "Aliases conflict, what do you want to do?".to_string(),
        Some(Container::new(
            Row::new()
                .push(
                    button::secondary(None, "Overwrite")
                        .on_press(ImportExportMessage::Overwrite.into()),
                )
                .push(Space::with_width(30))
                .push(
                    button::secondary(None, "Ignore").on_press(ImportExportMessage::Ignore.into()),
                ),
        )),
    );
    let (msg, button) = match import_export_type {
        ImportExportType::ImportBackup {
            overwrite_labels,
            overwrite_aliases,
            ..
        } => match (overwrite_labels, overwrite_aliases) {
            (Some(_), _) => labels_btn,

            (_, Some(_)) => aliases_btn,
            _ => (msg, cancel),
        },
        _ => (msg, cancel),
    };
    let button = button.map(|b| {
        Container::new(b)
            .align_x(Horizontal::Center)
            .width(Length::Fill)
    });
    let manual_path_form = matches!(state, ImportExportState::ManualPath).then(|| {
        form::Form::<Message>::new_trimmed("Path", manual_path, |path| {
            Message::from(ImportExportMessage::PathEdited(path))
        })
        .warning("Please enter a filesystem path")
        .padding(10)
    });
    let manual_path_actions = matches!(state, ImportExportState::ManualPath).then(|| {
        Container::new(
            Row::new()
                .spacing(10)
                .push(Space::with_width(Length::Fill))
                .push(button::secondary(None, "Close").on_press(ImportExportMessage::Close.into()))
                .push(
                    button::secondary(None, "Use path")
                        .on_press(ImportExportMessage::ConfirmPath.into()),
                ),
        )
        .align_x(Horizontal::Center)
        .width(Length::Fill)
    });

    let mut p = match state {
        ImportExportState::Init => 0.0,
        ImportExportState::ChoosePath
        | ImportExportState::ManualPath
        | ImportExportState::Path(_)
        | ImportExportState::Started => 5.0,
        ImportExportState::Progress(p) => *p,
        ImportExportState::TimedOut
        | ImportExportState::Aborted
        | ImportExportState::Ended
        | ImportExportState::Closed => 100.0,
    };
    // keep progress bar visible
    if p == 0.0 {
        p += 2.5;
    }
    let progress_bar_row = Row::new()
        .push(Space::with_width(30))
        .push(progress_bar(0.0..=100.0, p))
        .push(Space::with_width(30));
    card::simple(
        Column::new()
            .spacing(10)
            .push(
                Row::new()
                    .push(Space::with_width(20))
                    .push(h4_bold(title))
                    .push(Space::with_width(Length::Fill))
                    .push_maybe(cross)
                    .align_y(alignment::Vertical::Center),
            )
            .push(Space::with_height(Length::Fill))
            .push(progress_bar_row)
            .push(Space::with_height(Length::Fill))
            .push(Row::new().push(text(msg)))
            .push_maybe(manual_path_form)
            .push(Space::with_height(Length::Fill))
            .push_maybe(manual_path_actions)
            .push_maybe(button)
            .push(Space::with_height(5)),
    )
    .width(Length::Fixed(500.0))
    .height(Length::Fixed(300.0))
    .into()
}
