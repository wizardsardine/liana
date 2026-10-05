use iced::{
    widget::{column, row, Space},
    Alignment, Length,
};

use liana_ui::{
    component::{
        badge, card,
        panels::setting::{header, SectionKind},
        separation,
        text::{legacy, Text},
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

pub fn about_section<'a>(
    cache: &'a Cache,
    warning: Option<&'a Error>,
    lianad_version: Option<&String>,
) -> Element<'a, Message> {
    let header = header(
        Some(SETTING_MSG),
        Some(SectionKind::About.title()),
        Some(SettingsMessage::AboutSection.into()),
    );

    let version_title = row![
        badge::tooltip(),
        legacy::text(t!("settings-version")).bold()
    ]
    .padding(10)
    .spacing(20)
    .align_y(Alignment::Center)
    .width(Length::Fill);
    let gui_version = legacy::text(format!("liana-gui v{}", crate::VERSION));
    let daemon_version = lianad_version.map(|version| legacy::text(format!("lianad v{version}")));
    let versions = row![Space::fill_width(), column![gui_version, daemon_version]];
    let version_card = card::simple(column![
        version_title,
        separation().width(Length::Fill),
        Space::with_height(10),
        versions
    ]);

    let content = column![header, version_card]
        .spacing(20)
        .width(Length::Fill);

    dashboard(&Menu::Settings, cache, warning, content)
}
