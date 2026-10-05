use iced::{
    alignment::Vertical,
    widget::{column, row, rule, Space},
    Length,
};

use liana_ui::{
    component::{
        panels::setting::{export_section, header, ImportExportKind, SectionKind},
        text::legacy,
    },
    widget::{Element, SpaceExt},
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

    let description = row![
        Space::with_width(15),
        legacy::text(t!("settings-import-export-description")),
        Space::fill_width()
    ];

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
        Space::with_width(30),
        legacy::text(t!("settings-other-formats")),
        Space::with_width(15),
        rule::horizontal(2),
        Space::with_width(30)
    ]
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
    .spacing(20)
    .width(Length::Fill);

    dashboard(&Menu::Settings, cache, warning, content)
}
