use iced::{
    widget::{column, row, Space},
    Length,
};

use liana_ui::{
    component::{
        button::btn_send_invitation,
        card, form,
        panels::setting::{header, SectionKind},
        text::new,
    },
    spacing::VSpacing,
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

    let description = new::b2(t!("settings-grant-wallet-access")).style(theme::text::secondary);
    let email = form::Form::new_trimmed(&t!("settings-user-email"), email_form, |email| {
        Message::Settings(SettingsMessage::RemoteBackendSettings(
            RemoteBackendSettingsMessage::EditInvitationEmail(email),
        ))
    })
    .warning(t!("settings-email-invalid"));
    let invitation_sent =
        success.then_some(new::caption(t!("settings-invitation-sent")).style(theme::text::success));
    let send_msg = (!processing && email_form.valid).then_some(Message::Settings(
        SettingsMessage::RemoteBackendSettings(RemoteBackendSettingsMessage::SendInvitation),
    ));
    let send = btn_send_invitation(send_msg);
    let actions = row![invitation_sent, Space::fill_width(), send];
    let invitation_card =
        card::simple(column![description, email, actions].spacing(VSpacing::L)).width(Length::Fill);
    let help_link = link(
        help::CHANGE_BACKEND_OR_NODE_URL,
        t!("settings-connect-own-node"),
    );

    let content = column![header, invitation_card, help_link].spacing(VSpacing::L);

    dashboard(&Menu::Settings, cache, warning, content)
}
