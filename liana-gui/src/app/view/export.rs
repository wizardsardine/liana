use iced::{
    widget::{column, progress_bar, row, Space},
    Alignment, Length,
};
use liana_ui::{
    component::{
        button::{btn_cancel, btn_ignore, btn_overwrite},
        modal::{modal_view, ModalWidth},
        text::new,
    },
    spacing::{HSpacing, VSpacing},
    theme,
    widget::{Element, SpaceExt},
};

use crate::export::ImportExportState;
use crate::export::{Error, ImportExportMessage, ImportExportType};

/// Return the modal view for an export task
pub fn export_modal<'a, Message: From<ImportExportMessage> + Clone + 'static>(
    state: &ImportExportState,
    error: Option<&'a Error>,
    title: &str,
    import_export_type: &ImportExportType,
) -> Element<'a, Message> {
    let cancel: Option<Element<'a, Message>> = match state {
        ImportExportState::Started | ImportExportState::Progress(_) => {
            Some(btn_cancel(Some(ImportExportMessage::UserStop.into())))
        }
        _ => None,
    }
    .map(Into::into);

    let close = match state {
        ImportExportState::Ended | ImportExportState::TimedOut | ImportExportState::Aborted => {
            Some(ImportExportMessage::Close.into())
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
            ImportExportState::Path(_) => "".into(),
            ImportExportState::Started => "Starting export...".into(),
            ImportExportState::Progress(p) => format!("Progress: {}%", p.round()),
            ImportExportState::TimedOut => "Export failed: timeout".into(),
            ImportExportState::Aborted => "Export canceled".into(),
            ImportExportState::Ended => import_export_type.end_message().into(),
            ImportExportState::Closed => "".into(),
        }
    };
    let conflict_buttons = || -> Element<'a, Message> {
        row![
            Space::fill_width(),
            btn_overwrite(Some(ImportExportMessage::Overwrite.into())),
            btn_ignore(Some(ImportExportMessage::Ignore.into())),
            Space::fill_width(),
        ]
        .spacing(HSpacing::M)
        .into()
    };
    let labels_btn = (
        "Labels conflict, what do you want to do?".to_string(),
        Some(conflict_buttons()),
    );
    let aliases_btn = (
        "Aliases conflict, what do you want to do?".to_string(),
        Some(conflict_buttons()),
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
    let button = button.map(|b| row![Space::fill_width(), b, Space::fill_width()]);

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
    let mut progress = progress_bar(0.0..=100.0, p);
    let mut msg = new::caption(msg);
    if error.is_some() {
        progress = progress.style(theme::progress_bar::error);
        msg = msg.style(theme::text::warning)
    }
    let progress_bar_row = row![Space::with_width(30), progress, Space::with_width(30),];
    let content = column![progress_bar_row, msg, button,]
        .spacing(VSpacing::M)
        .align_x(Alignment::Center)
        .width(Length::Fill);

    modal_view(Some(title), None, close, ModalWidth::M, content)
}
