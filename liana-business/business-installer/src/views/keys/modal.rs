use crate::{
    state::{views::keys::EditKeyModalState, Message, State},
    views::format_last_edit_info,
};
use iced::{
    widget::{pick_list, Space},
    Alignment, Length,
};
use liana_connect::ws_business;
use liana_ui::{
    component::{button, card, form, text, tooltip},
    icon, theme,
    widget::*,
};

pub fn key_modal_view(state: &State) -> Option<Element<'_, Message>> {
    if let Some(modal_state) = &state.views.keys.edit_key_modal {
        return Some(edit_key_modal_view(state, modal_state));
    }
    None
}

pub fn edit_key_modal_view<'a>(
    state: &'a State,
    modal_state: &'a EditKeyModalState,
) -> Element<'a, Message> {
    // Header
    let title = if modal_state.is_new {
        "New Key"
    } else {
        "Edit Key"
    };
    let header = Row::new()
        .spacing(10)
        .align_y(Alignment::Center)
        .push(text::h3(title))
        .push(Space::with_width(Length::Fill))
        .push(
            button::transparent(Some(icon::cross_icon().size(32)), "")
                .on_press(Message::KeyCancelModal),
        );

    // Get last edit info for the key being edited (only for existing keys)
    let current_user_email_lower = state.views.login.email.form.value.to_lowercase();
    let last_edit_info: Option<Element<'_, Message>> = if !modal_state.is_new {
        state
            .app
            .keys
            .get(&modal_state.key_id)
            .and_then(|key| {
                format_last_edit_info(
                    key.last_edited,
                    key.last_editor,
                    state,
                    &current_user_email_lower,
                )
            })
            .map(|info| text::caption(info).style(theme::text::secondary).into())
    } else {
        None
    };

    // Alias input
    let alias_valid = state.views.keys.is_alias_valid();
    let alias_value = form::Value {
        value: modal_state.alias.clone(),
        warning: None,
        valid: alias_valid || modal_state.alias.trim().is_empty(),
    };
    let alias_input = Column::new()
        .spacing(5)
        .push(text::p1_medium("Key Alias").style(theme::text::primary))
        .push(form::Form::new(
            "Enter key alias",
            &alias_value,
            Message::KeyUpdateAlias,
        ));

    // Description input
    let desc_value = form::Value {
        value: modal_state.description.clone(),
        warning: None,
        valid: true,
    };
    let description_input = Column::new()
        .spacing(5)
        .push(text::p1_medium("Key Description").style(theme::text::primary))
        .push(form::Form::new(
            "Enter description",
            &desc_value,
            Message::KeyUpdateDescr,
        ));

    // Key type picker (placed before identity input)
    let key_types: &[ws_business::KeyType] = &[
        ws_business::KeyType::Internal,
        ws_business::KeyType::External,
        ws_business::KeyType::Cosigner,
        ws_business::KeyType::SafetyNet,
    ];
    let key_type_label = Row::new()
        .spacing(5)
        .align_y(Alignment::Center)
        .push(text::p1_medium("Key Type").style(theme::text::primary))
        .push(tooltip::tooltip(
            "Internal: keys held by your organization.\n \
                External: keys held by third parties.\n \
                Cosigner: Professional third party co-signing key.\n \
                SafetyNet: Professional third party recovery key.",
        ));
    let key_type_picker = Column::new().spacing(5).push(key_type_label).push(
        pick_list(
            key_types,
            Some(modal_state.key_type),
            Message::KeyUpdateType,
        )
        .width(Length::Fill),
    );

    // Identity input — conditional on key type
    let uses_token = matches!(
        modal_state.key_type,
        ws_business::KeyType::Cosigner | ws_business::KeyType::SafetyNet
    );

    let email_input: Option<Element<'a, Message>> = if !uses_token {
        let is_empty = modal_state.email.trim().is_empty();
        let email_valid = state.views.keys.is_email_valid();
        let email_value = form::Value {
            value: modal_state.email.clone(),
            warning: if is_empty {
                None
            } else if !email_valid {
                Some("Invalid email!")
            } else {
                None
            },
            valid: email_valid || is_empty,
        };
        Some(
            Column::new()
                .spacing(5)
                .push(
                    text::p1_medium("Email Address of the Key Manager").style(theme::text::primary),
                )
                .push(form::Form::new(
                    "Enter email address",
                    &email_value,
                    Message::KeyUpdateEmail,
                ))
                .into(),
        )
    } else {
        None
    };

    let token_input: Option<Element<'a, Message>> = if uses_token {
        let is_empty = modal_state.token.trim().is_empty();
        let token_valid = state.views.keys.is_token_format_valid();
        let token_value = form::Value {
            value: modal_state.token.clone(),
            warning: if is_empty {
                None
            } else {
                modal_state.token_warning
            },
            valid: (token_valid && modal_state.token_warning.is_none()) || is_empty,
        };
        Some(
            Column::new()
                .spacing(5)
                .push(text::p1_medium("Token").style(theme::text::primary))
                .push(form::Form::new(
                    "Enter token (e.g., 42-absent-cake-eagle)",
                    &token_value,
                    Message::KeyUpdateToken,
                ))
                .into(),
        )
    } else {
        None
    };

    // Footer
    let can_save = state.views.keys.can_save();
    let save_button = if can_save {
        button::primary(None, "Save")
            .on_press(Message::KeySave)
            .width(Length::Fixed(120.0))
    } else {
        button::secondary(None, "Save").width(Length::Fixed(120.0))
    };
    let footer = Row::new()
        .spacing(10)
        .push(Space::with_width(Length::Fill))
        .push(
            button::secondary(None, "Cancel")
                .on_press(Message::KeyCancelModal)
                .width(Length::Fixed(120.0)),
        )
        .push(save_button);

    let content = Column::new()
        .push(header)
        .push_maybe(last_edit_info)
        .push(alias_input)
        .push(description_input)
        .push(key_type_picker)
        .push_maybe(email_input)
        .push_maybe(token_input)
        .push(footer)
        .spacing(15)
        .padding(20.0)
        .width(Length::Fixed(500.0));

    card::modal(content).into()
}
