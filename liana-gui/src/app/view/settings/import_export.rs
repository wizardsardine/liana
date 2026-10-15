use iced::{
    alignment::Vertical,
    widget::{column, row, rule},
    Length,
};

use liana_ui::{
    component::{
        panels::setting::{export_section, header, ImportExportKind, SectionKind},
        text::new,
    },
    spacing::{HSpacing, VSpacing},
    theme,
    widget::Element,
};

use crate::{
    app::{
        cache::Cache,
        error::Error,
        menu::Menu,
        view::{
            dashboard,
            message::{Message, SettingsMessage},
            settings::SETTING_MSG,
        },
    },
    t,
};

pub fn import_export<'a>(cache: &'a Cache, warning: Option<&'a Error>) -> Element<'a, Message> {
    let header = header(
        Some(SETTING_MSG),
        Some(SectionKind::ImportExport.title()),
        Some(SettingsMessage::ImportExportSection.into()),
    );

    let description =
        new::b2(t!("settings-import-export-description")).style(theme::text::secondary);

    let export_encrypted_descriptor = export_section(
        ImportExportKind::ExportEncryptedDescriptor,
        Message::Settings(SettingsMessage::ExportEncryptedDescriptor),
    );

    let export_descriptor = export_section(
        ImportExportKind::ExportDescriptor,
        Message::Settings(SettingsMessage::ExportPlaintextDescriptor),
    );

    let export_transactions = export_section(
        ImportExportKind::ExportTransactions,
        Message::Settings(SettingsMessage::ExportTransactions),
    );

    let export_labels = export_section(
        ImportExportKind::ExportLabels,
        Message::Settings(SettingsMessage::ExportLabels),
    );

    let export_wallet = export_section(
        ImportExportKind::ExportWallet,
        Message::Settings(SettingsMessage::ExportWallet),
    );

    let import_wallet = export_section(
        ImportExportKind::ImportWallet,
        Message::Settings(SettingsMessage::ImportWallet),
    );

    let separator = row![
        new::b4_medium(t!("settings-other-formats")).style(theme::text::secondary),
        rule::horizontal(1)
    ]
    .spacing(HSpacing::L)
    .align_y(Vertical::Center);

    let content = column![
        header,
        description,
        export_encrypted_descriptor,
        export_wallet,
        import_wallet,
        separator,
        export_labels,
        export_transactions,
        export_descriptor
    ]
    .spacing(VSpacing::L)
    .width(Length::Fill);

    dashboard(&Menu::Settings, cache, warning, content)
}
