//! Cube › Settings › About: the installed versions as a plain list, with one
//! action to copy them. Device ID, account and stream status live in
//! Connections.

use coincube_ui::locale::copy as localized_copy;
use coincube_ui::localized_format;
use iced::widget::Space;
use iced::{Alignment, Length};

use coincube_ui::widget::{Column, Element, Row};

use crate::app::cache;
use crate::app::menu::{CubeSettingsOption, CubeSubMenu, Menu};
use crate::app::view::dashboard;
use crate::app::view::message::Message;
use crate::app::view::settings::page;

pub const COPY_VERSIONS_ID: &str = "tenshu.settings.about.copy-versions";
pub const OPEN_CONNECTIONS_ID: &str = "tenshu.settings.about.connections";

pub fn about_section<'a>(
    menu: &'a Menu,
    cache: &'a cache::Cache,
    coincubed_version: Option<&'a str>,
) -> Element<'a, Message> {
    dashboard(menu, cache, about_content(coincubed_version))
}

/// The (label, value) pairs shown and copied, in order. The wallet service
/// only appears when this Cube runs it locally.
fn versions(coincubed_version: Option<&str>) -> Vec<(&'static str, String)> {
    let mut versions = vec![
        ("COINCUBE Desktop", crate::VERSION.to_string()),
        ("Interface", format!("coincube-gui {}", crate::VERSION)),
    ];
    if let Some(version) = coincubed_version {
        versions.push((
            "Wallet service",
            localized_format!("coincubed {version}", "acuñado {version}"),
        ));
    }
    versions
}

/// What "Copy version details" puts on the clipboard: one "Label: value"
/// line per version, as a support request would quote them.
pub fn version_details(coincubed_version: Option<&str>) -> String {
    versions(coincubed_version)
        .into_iter()
        .map(|(label, value)| format!("{label}: {value}"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn about_content<'a>(coincubed_version: Option<&'a str>) -> Element<'a, Message> {
    let head = Row::new()
        .align_y(Alignment::Center)
        .push(page::heading(localized_copy("Version")))
        .push(Space::new().width(Length::Fill))
        .push(page::button(
            COPY_VERSIONS_ID,
            localized_copy("Copy version details"),
            Some(Message::Clipboard(version_details(coincubed_version))),
        ));
    let rows = versions(coincubed_version)
        .into_iter()
        .map(|(label, value)| page::row(label).control(page::muted(value)).into());
    let moved = Row::new()
        .spacing(8)
        .align_y(Alignment::Center)
        .push(page::muted(localized_copy(
            "Device ID, account and stream status now live in Connections, beside the other \
             health checks.",
        )))
        .push(page::link(
            OPEN_CONNECTIONS_ID,
            localized_copy("Open Connections"),
            Message::Menu(Menu::Cube(CubeSubMenu::Settings(
                CubeSettingsOption::Connections,
            ))),
        ));
    Column::new()
        .width(Length::Fill)
        .push(Space::new().height(Length::Fixed(page::GROUP_GAP)))
        .push(head)
        .push(Space::new().height(Length::Fixed(12.0)))
        .push(page::rows(rows))
        .push(Space::new().height(Length::Fixed(12.0)))
        .push(moved)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::view::shared::landing::capture;

    #[test]
    fn about_lists_versions_and_copies_them_without_device_details() {
        let probe = capture::inspect(
            "settings-about",
            about_content(Some("0.9.0-dev")),
            (1440, 900),
            "dark",
            coincube_ui::theme::Theme::dark(),
        );
        for label in [
            "Version",
            "Copy version details",
            "COINCUBE Desktop",
            "Interface",
            "Wallet service",
            "coincubed 0.9.0-dev",
        ] {
            assert!(probe.contains(label), "{}", label);
        }
        assert!(!probe.contains("Device ID") || probe.contains("now live in Connections"));
        assert!(!probe.contains("Re-register"));

        let copied = capture::activate(
            about_content(Some("0.9.0-dev")),
            iced::advanced::widget::Id::new(COPY_VERSIONS_ID),
            (1440, 900),
            true,
        );
        assert!(copied.iter().any(|m| matches!(
            m,
            Message::Clipboard(details) if details == &version_details(Some("0.9.0-dev"))
        )));
        assert!(version_details(Some("0.9.0-dev")).ends_with("Wallet service: coincubed 0.9.0-dev"));
        assert!(!version_details(None).contains("Wallet service"));
    }
}
