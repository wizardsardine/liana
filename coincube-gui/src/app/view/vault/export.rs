use coincube_ui::{
    component::{
        button, card, hw,
        text::{h4_bold, text},
    },
    icon,
    widget::Element,
};
use iced::{
    alignment::{self, Horizontal},
    widget::{progress_bar, Column, Container, Row, Space},
    Length,
};

use crate::export::ImportExportState;
use crate::export::{Error, ImportExportMessage, ImportExportType};

/// Return the modal view for an export task
pub fn export_modal<'a, Message: From<ImportExportMessage> + Clone + 'static>(
    state: &ImportExportState,
    error: Option<&'a Error>,
    title: &'a str,
    import_export_type: &ImportExportType,
    advisory: Option<&'static crate::hw_advisory::Advisory>,
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
        format!("{}", error)
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
                .push(Space::new().width(30))
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
                .push(Space::new().width(30))
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

    let mut p = match state {
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
    // keep progress bar visible
    if p == 0.0 {
        p += 2.5;
    }
    let progress_bar_row = Row::new()
        .push(Space::new().width(30))
        .push(progress_bar(0.0..=100.0, p))
        .push(Space::new().width(30));

    // Firmware advisory for the signer this file came from. Informational
    // only: the import has already succeeded by the time it appears.
    let has_advisory = advisory.is_some();
    let advisory = advisory.map(|advisory| {
        Row::new()
            .push(Space::new().width(20))
            .push(hw::advisory_panel(
                advisory.headline,
                None,
                advisory.file_import,
                "Read the rotation guide",
                Some(ImportExportMessage::OpenAdvisoryUrl(advisory.url.to_string()).into()),
                None,
            ))
            .push(Space::new().width(20))
    });
    card::simple(
        Column::new()
            .spacing(10)
            .push(
                Row::new()
                    .push(Space::new().width(20))
                    .push(h4_bold(title))
                    .push(Space::new().width(Length::Fill))
                    .push(cross)
                    .align_y(alignment::Vertical::Center),
            )
            .push(Space::new().height(Length::Fill))
            .push(progress_bar_row)
            .push(Space::new().height(Length::Fill))
            .push(Row::new().push(text(msg)))
            .push(advisory)
            .push(Space::new().height(Length::Fill))
            .push(button)
            .push(Space::new().height(5)),
    )
    .width(Length::Fixed(500.0))
    // The fixed height is what every other import/export modal uses; an
    // advisory adds a paragraph, so that one case sizes to its content.
    .height(if has_advisory {
        Length::Shrink
    } else {
        Length::Fixed(300.0)
    })
    .into()
}
