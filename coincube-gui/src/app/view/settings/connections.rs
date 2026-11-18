//! Cube › Settings › Connections: the Connect account and this computer's
//! live updates, in everyday words. Nothing technical shows day to day: the
//! device ID appears only under Details for support, inside the notice that
//! automatic renewal failed. The Spark Connections tab links here with
//! [`cube_connections_row`].

use coincube_ui::locale::copy as localized_copy;
use iced::Length;

use coincube_ui::component::tenshu::Tone;
use coincube_ui::widget::{Column, Element, Row};

use crate::app::cache::Cache;
use crate::app::menu::{CubeSettingsOption, CubeSubMenu, Menu};
use crate::app::state::settings::connections::ConnectionsSettingsState;
use crate::app::view::dashboard;
use crate::app::view::message::{Message, SettingsMessage};
use crate::app::view::settings::page;
use crate::app::ConnectionStatus;

pub const MANAGE_CONNECT_ID: &str = "tenshu.settings.cube-connections.connect";
pub const COPY_DEVICE_ID: &str = "tenshu.settings.cube-connections.copy-device";
pub const TRY_AGAIN_ID: &str = "tenshu.settings.cube-connections.try-again";
pub const SUPPORT_DETAILS_ID: &str = "tenshu.settings.cube-connections.support";
pub const SIGN_IN_ID: &str = "tenshu.settings.cube-connections.sign-in";

/// Where the Vault and Spark Connections tabs send the reader.
pub const CUBE_CONNECTIONS_ROUTE: Menu =
    Menu::Cube(CubeSubMenu::Settings(CubeSettingsOption::Connections));

pub fn connections_section<'a>(
    menu: &'a Menu,
    cache: &'a Cache,
    state: &'a ConnectionsSettingsState,
) -> Element<'a, Message> {
    dashboard(menu, cache, connections_content(cache, state.support_open))
}

/// The status of this computer's live updates, in the tone its word takes
/// everywhere: On green, Connecting… and Offline grey, Reconnecting… amber.
pub fn live_updates_status(status: ConnectionStatus) -> (page::Status, &'static str) {
    let kind = match status {
        ConnectionStatus::Connected => page::Status::Ok,
        ConnectionStatus::Inactive | ConnectionStatus::Connecting => page::Status::Busy,
        ConnectionStatus::Reconnecting | ConnectionStatus::Renewing => page::Status::Warn,
        ConnectionStatus::Offline | ConnectionStatus::RenewFailed => page::Status::Off,
    };
    (kind, localized_copy(status.word()))
}

pub fn connections_content<'a>(cache: &'a Cache, support_open: bool) -> Element<'a, Message> {
    let signed_in = cache.connect_authenticated;
    let mut account = page::row(localized_copy("Account")).control(if signed_in {
        page::status(page::Status::Ok, localized_copy("Signed in"))
    } else {
        page::status(page::Status::Unknown, localized_copy("Signed out"))
    });
    if let Some(email) = cache.connect_email.as_deref().filter(|_| signed_in) {
        account = account.line(page::muted(email));
    }
    let manage = page::link(
        MANAGE_CONNECT_ID,
        if signed_in {
            "Manage in Connect"
        } else {
            "Sign in"
        },
        if signed_in {
            Message::OpenConnectSwitchAccount
        } else {
            Message::OpenConnectSignIn
        },
    );
    let mut content = Column::new()
        .width(Length::Fill)
        .push(page::group_with_action(
            "Connect account",
            manage,
            page::rows([account.into()]),
        ));

    if !signed_in {
        if let Some(reason) = cache.connect_signed_out_by {
            content = content.push(page::note(
                Tone::Bad,
                Column::new()
                    .spacing(4)
                    .push(page::heading(reason.title()).size(14))
                    .push(page::muted(reason.detail())),
                Some(page::primary(
                    SIGN_IN_ID,
                    localized_copy("Sign in"),
                    Some(Message::OpenConnectSignIn),
                )),
            ));
        }
        return content.into();
    }

    let status = cache.connect_stream_status;
    let (kind, word) = live_updates_status(status);
    let mut live = page::row(localized_copy("Live updates")).control(page::status(kind, word));
    if let Some(line) = status.line() {
        live = live.description(line);
    }
    content = content.push(page::group(
        localized_copy("This computer"),
        None,
        page::rows([live.into()]),
    ));

    // Only when automatic renewal didn't work.
    if status == ConnectionStatus::RenewFailed {
        content = content.push(renewal_failure_note(cache, support_open));
    }
    content.into()
}

fn renewal_failure_note(cache: &Cache, support_open: bool) -> Element<'_, Message> {
    let mut notice = Column::new()
        .spacing(4)
        .push(page::heading(localized_copy(crate::app::RENEW_FAILED_TITLE)).size(14))
        .push(page::muted(crate::app::renew_failed_detail()))
        .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
        .push(
            Row::new()
                .spacing(12)
                .align_y(iced::Alignment::Center)
                .push(page::primary(
                    TRY_AGAIN_ID,
                    localized_copy("Try again"),
                    Some(Message::Settings(SettingsMessage::ReregisterConnectDevice)),
                ))
                .push(page::link(
                    SUPPORT_DETAILS_ID,
                    if support_open {
                        "Hide details for support"
                    } else {
                        "Details for support"
                    },
                    Message::Settings(SettingsMessage::ToggleConnectSupportDetails),
                )),
        );
    if support_open {
        if let Some(id) = cache.connect_device_id.as_deref() {
            notice = notice.push(
                Row::new()
                    .spacing(10)
                    .align_y(iced::Alignment::Center)
                    .push(page::muted(localized_copy("Device ID")))
                    .push(page::muted(id).font(coincube_ui::font::MONO_REGULAR))
                    .push(page::button(
                        COPY_DEVICE_ID,
                        localized_copy("Copy"),
                        Some(Message::Clipboard(id.to_owned())),
                    )),
            );
        }
    }
    page::note(Tone::Bad, notice, None)
}

/// The one row the Spark Connections tab keeps for this computer's Connect
/// live updates, with a way to Cube › Connections.
pub fn cube_connections_row<'a>(cache: &Cache, id: &'static str) -> Element<'a, Message> {
    let (kind, word) = if !cache.connect_authenticated {
        (page::Status::Unknown, "Signed out")
    } else {
        live_updates_status(cache.connect_stream_status)
    };
    page::row(localized_copy("Connect live updates"))
        .accessory(page::status(kind, word))
        .description(localized_copy("Shared by every wallet in this Cube."))
        .control(page::button(
            id,
            localized_copy("Cube connections"),
            Some(Message::Menu(CUBE_CONNECTIONS_ROUTE)),
        ))
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::view::shared::landing::capture;

    fn signed_in() -> Cache {
        Cache {
            connect_authenticated: true,
            connect_email: Some("alex@example.test".into()),
            connect_device_id: Some("160".into()),
            connect_stream_status: ConnectionStatus::Connected,
            current_cube_server_id: Some(1),
            ..Cache::default()
        }
    }

    fn inspect(content: Element<'_, Message>) -> capture::Inspection {
        capture::inspect(
            "settings-cube-connections",
            content,
            (1440, 900),
            "dark",
            coincube_ui::theme::Theme::dark(),
        )
    }

    #[test]
    fn everyday_view_shows_account_and_live_updates_only() {
        let cache = signed_in();
        let probe = inspect(connections_content(&cache, false));
        for label in [
            "Connect account",
            "alex@example.test",
            "Signed in",
            "Manage in Connect",
            "This computer",
            "Live updates",
            "On",
            "Mobile signing requests and signatures arrive as they happen.",
        ] {
            assert!(probe.contains(label), "{}", label);
        }
        for gone in [
            "Device ID",
            "160",
            "Re-register",
            "Troubleshooting",
            "Realtime stream",
        ] {
            assert!(!probe.contains(gone), "{}", gone);
        }
    }

    #[test]
    fn failed_renewal_offers_try_again_and_support_details() {
        let cache = Cache {
            connect_stream_status: ConnectionStatus::RenewFailed,
            ..signed_in()
        };
        let closed = inspect(connections_content(&cache, false));
        assert!(closed.contains(crate::app::RENEW_FAILED_TITLE));
        assert!(closed.contains("Details for support"));
        assert!(!closed.contains("160"));
        let open = inspect(connections_content(&cache, true));
        assert!(open.contains("Device ID") && open.contains("160"));
        let pressed = capture::activate(
            connections_content(&cache, false),
            iced::advanced::widget::Id::new(TRY_AGAIN_ID),
            (1440, 900),
            true,
        );
        assert!(pressed.iter().any(|m| matches!(
            m,
            Message::Settings(SettingsMessage::ReregisterConnectDevice)
        )));
        let copied = capture::activate(
            connections_content(&cache, true),
            iced::advanced::widget::Id::new(COPY_DEVICE_ID),
            (1440, 900),
            true,
        );
        assert!(copied
            .iter()
            .any(|m| matches!(m, Message::Clipboard(id) if id == "160")));
    }

    #[test]
    fn signed_out_by_connect_says_why_and_offers_sign_in() {
        let cache = Cache {
            connect_signed_out_by: Some(crate::app::SignedOutBy::ComputerRemoved),
            ..Cache::default()
        };
        let probe = inspect(connections_content(&cache, false));
        assert!(probe.contains("Signed out"));
        assert!(probe.contains("This computer was removed from your Connect account"));
        assert!(!probe.contains("Live updates"));
        let plain = inspect(connections_content(&Cache::default(), false));
        assert!(!plain.contains("removed"));
    }

    #[test]
    fn the_spark_link_row_uses_the_live_updates_words() {
        let cache = Cache {
            connect_stream_status: ConnectionStatus::Reconnecting,
            ..signed_in()
        };
        let probe = inspect(cube_connections_row(&cache, "test.link"));
        assert!(probe.contains("Reconnecting…"));
        let sent = capture::activate(
            cube_connections_row(&cache, "test.link"),
            iced::advanced::widget::Id::new("test.link"),
            (1440, 900),
            true,
        );
        assert!(sent
            .iter()
            .any(|m| matches!(m, Message::Menu(menu) if *menu == CUBE_CONNECTIONS_ROUTE)));
    }
}
