use iced::{
    widget::{column, row, Space},
    Length,
};

use liana_ui::{
    component::{
        button, card, form,
        panels::setting::{header, SectionKind},
        text::legacy,
    },
    theme,
    widget::{Element, SpaceExt},
};

use crate::{
    app::{
        cache::Cache,
        error::Error,
        menu::Menu,
        view::{
            dashboard,
            message::{Message, RemoteBackendSettingsMessage, SettingsMessage},
            settings::{link, SETTING_MSG},
        },
    },
    help, t,
};

pub fn remote_backend_section<'a>(
    cache: &'a Cache,
    email_form: &form::Value<String>,
    processing: bool,
    success: bool,
    warning: Option<&'a Error>,
) -> Element<'a, Message> {
    let header = header(
        Some(SETTING_MSG),
        Some(SectionKind::Backend.title()),
        Some(SettingsMessage::EditRemoteBackendSettings.into()),
    );

    let description = legacy::text(t!("settings-grant-wallet-access"));
    let email = form::Form::new_trimmed(&t!("settings-user-email"), email_form, |email| {
        Message::Settings(SettingsMessage::RemoteBackendSettings(
            RemoteBackendSettingsMessage::EditInvitationEmail(email),
        ))
    })
    .warning(t!("settings-email-invalid"))
    .size(legacy::P1_SIZE)
    .padding(10);
    let invitation_sent =
        success.then_some(legacy::text(t!("settings-invitation-sent")).style(theme::text::success));
    let send_msg = (!processing && email_form.valid).then_some(Message::Settings(
        SettingsMessage::RemoteBackendSettings(RemoteBackendSettingsMessage::SendInvitation),
    ));
    let send = button::secondary(None, t!("btn-send-invitation")).on_press_maybe(send_msg);
    let actions = row![invitation_sent, Space::fill_width(), send];
    let invitation_card =
        card::simple(column![description, email, actions].spacing(20)).width(Length::Fill);
    let help_link = link(
        help::CHANGE_BACKEND_OR_NODE_URL,
        t!("settings-connect-own-node"),
    );

    let content = column![header, invitation_card, help_link].spacing(20);

    dashboard(&Menu::Settings, cache, warning, content)
}
