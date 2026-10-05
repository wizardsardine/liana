pub mod about;
pub mod backend;
pub mod general;
pub mod import_export;
pub mod node;
pub mod wallet;

use iced::widget::tooltip as iced_tooltip;

use liana_ui::{
    component::{
        self, button,
        panels::setting::{header, settings_section, SectionKind},
        text::legacy,
    },
    icon,
    theme::{self},
    widget::*,
};

use super::{dashboard, message::*};

use crate::app::{cache::Cache, menu::Menu};

const SETTING_MSG: Message = Message::Menu(Menu::Settings);

pub fn list(cache: &Cache, is_remote_backend: bool) -> Element<'_, Message> {
    let general = settings_section(
        SectionKind::General,
        Message::Settings(SettingsMessage::GeneralSection),
    );

    let node = settings_section(
        SectionKind::Node,
        Message::Settings(SettingsMessage::EditBitcoindSettings),
    );

    let backend = settings_section(
        SectionKind::Backend,
        Message::Settings(SettingsMessage::EditRemoteBackendSettings),
    );

    let wallet = settings_section(
        SectionKind::Wallet,
        Message::Settings(SettingsMessage::EditWalletSettings),
    );

    let import_export = settings_section(
        SectionKind::ImportExport,
        Message::Settings(SettingsMessage::ImportExportSection),
    );

    let about = settings_section(
        SectionKind::About,
        Message::Settings(SettingsMessage::AboutSection),
    );

    let backend = if !is_remote_backend { node } else { backend };

    #[rustfmt::skip]
    let entries = vec![
        general,
        backend,
        wallet,
        import_export,
        about
    ];

    let content = component::panels::setting::section_list(entries);
    dashboard(&Menu::Settings, cache, None, content)
}

pub fn link<'a>(url: &str, link_text: impl std::fmt::Display) -> Element<'a, Message> {
    let link_btn = button::link(Some(icon::link_icon()), link_text)
        .on_press(Message::OpenUrl(url.to_string()));
    let url_tooltip = Container::new(legacy::text(url))
        .style(theme::card::simple)
        .padding(10);

    iced_tooltip::Tooltip::new(link_btn, url_tooltip, iced_tooltip::Position::Bottom).into()
}
