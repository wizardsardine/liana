use crate::{
    state::{views::keys::EditKeyModalState, Message, State},
    views::format_last_edit_info,
};
use iced::{widget::Space, Alignment, Length};
use liana_connect::ws_business;
use liana_i18n::t;
use liana_ui::{
    component::{
        button::{btn_cancel, btn_save},
        form,
        modal::{modal_view, none_fn, ModalWidth},
        pick_list, text, tooltip,
    },
    theme,
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
        t!("business-new-key")
    } else {
        t!("business-edit-key")
    };

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
        .push(text::p1_medium(t!("business-key-alias")).style(theme::text::primary))
        .push(form::Form::new(
            &t!("business-enter-key-alias"),
            &alias_value,
            Message::KeyUpdateAlias,
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
        .push(text::p1_medium(t!("business-key-type")).style(theme::text::primary))
        .push(tooltip::tooltip(t!("business-key-type-tooltip")));
    let key_type_picker = Column::new().spacing(5).push(key_type_label).push(
        pick_list::pick_list(
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
            warning: None,
            valid: email_valid || is_empty,
        };
        let email_form = form::Form::new(
            &t!("business-enter-email-address"),
            &email_value,
            Message::KeyUpdateEmail,
        );
        let email_form = if !is_empty && !email_valid {
            email_form.warning(t!("settings-email-invalid"))
        } else {
            email_form
        };
        Some(
            Column::new()
                .spacing(5)
                .push(text::p1_medium(t!("business-key-manager-email")).style(theme::text::primary))
                .push(email_form)
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
            warning: if is_empty { None } else { None },
            valid: (token_valid && modal_state.token_warning.is_none()) || is_empty,
        };
        let token_warning = modal_state.token_warning.map(token_warning);
        let token_form = form::Form::new(
            &t!("business-enter-token-placeholder"),
            &token_value,
            Message::KeyUpdateToken,
        );
        let token_form = if let Some(warning) = token_warning {
            token_form.warning(warning)
        } else {
            token_form
        };
        Some(
            Column::new()
                .spacing(5)
                .push(text::p1_medium(t!("common-token")).style(theme::text::primary))
                .push(token_form)
                .into(),
        )
    } else {
        None
    };

    // Footer
    let can_save = state.views.keys.can_save();
    let save_button = btn_save(can_save.then_some(Message::KeySave));
    let footer = Row::new()
        .spacing(10)
        .push(Space::with_width(Length::Fill))
        .push(btn_cancel(Some(Message::KeyCancelModal)))
        .push(save_button);

    let body = Column::new()
        .push_maybe(last_edit_info)
        .push(alias_input)
        .push(key_type_picker)
        .push_maybe(email_input)
        .push_maybe(token_input)
        .push(footer)
        .spacing(15);

    modal_view(
        Some(title),
        none_fn(),
        Some(|| Message::KeyCancelModal),
        ModalWidth::M,
        body,
    )
}

fn token_warning(key: &str) -> String {
    match key {
        "business-token-invalid" => t!("business-token-invalid"),
        "business-token-duplicate" => t!("business-token-duplicate"),
        _ => key.to_string(),
    }
}
