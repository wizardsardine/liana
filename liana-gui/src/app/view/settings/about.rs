use iced::{widget::column, Length};

use liana_ui::{
    component::panels::setting::{header, version_card, SectionKind},
    spacing::VSpacing,
    widget::Element,
};

use crate::app::{
    cache::Cache,
    error::Error,
    menu::Menu,
    view::{
        dashboard,
        message::{Message, SettingsMessage},
        settings::SETTING_MSG,
    },
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

    let versions = std::iter::once(format!("liana-gui v{}", crate::VERSION))
        .chain(lianad_version.map(|version| format!("lianad v{version}")))
        .collect();
    let version_card = version_card(versions);

    let content = column![header, version_card]
        .spacing(VSpacing::L)
        .width(Length::Fill);

    dashboard(&Menu::Settings, cache, warning, content)
}
