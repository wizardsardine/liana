use crate::{
    state::{views::keys::EditKeyModalState, Message, State},
    views::format_last_edit_info,
};
use iced::{
    alignment::Vertical,
    widget::{column, row, Space},
    Alignment, Length,
};
use liana_connect::ws_business;
use liana_ui::{
    component::{
        button::{btn_cancel, btn_save},
        combobox::{self, EditableMenuActions, MenuEntry},
        form,
        modal::{modal_view, ModalWidth},
        pick_list, text, tooltip,
    },
    icon, theme,
    widget::*,
};
use std::str::FromStr;

pub fn key_modal_view(state: &State) -> Option<Element<'_, Message>> {
    let modal_state = state.views.keys.edit_key_modal.as_ref()?;
    Some(edit_key_modal_view(state, modal_state))
}

pub fn edit_key_modal_view<'a>(
    state: &'a State,
    modal_state: &'a EditKeyModalState,
) -> Element<'a, Message> {
    let title = if modal_state.is_new {
        "Add a key"
    } else {
        "Edit key"
    };
    let current_user_email_lower = state.views.login.email.form.value.to_lowercase();
    let last_edit_info: Option<Element<'_, Message>> = (!modal_state.is_new)
        .then_some({
            state.app.keys().get(&modal_state.key_id).and_then(|key| {
                format_last_edit_info(
                    key.last_edited,
                    key.last_editor,
                    state,
                    &current_user_email_lower,
                )
            })
        })
        .flatten();

    let intro = text::new::caption(
        "Define the key's name and who holds it. You'll set it up later, by connecting a device or adding its public key.",
    )
    .style(theme::text::secondary);

    let alias_value = form::Value {
        value: modal_state.alias.clone(),
        warning: None,
        valid: state.views.keys.is_alias_valid() || modal_state.alias.trim().is_empty(),
    };
    let alias_field = column![
        field_label("Key Alias"),
        form::Form::new(
            "e.g. Alice, Treasury, Lawyer…",
            &alias_value,
            Message::KeyUpdateAlias,
        ),
    ]
    .spacing(5);

    let key_type_options = [
        ws_business::KeyType::Internal,
        ws_business::KeyType::External,
        ws_business::KeyType::Cosigner,
        ws_business::KeyType::SafetyNet,
    ];
    let key_type_hint = "Internal: Held by a member of your organization.\n\
External: Held by a trusted third party.\n\
Cosigner: Professional co-signing service.\n\
SafetyNet: Professional recovery service.";
    let key_type_label = row![field_label("Key Type"), tooltip::tooltip(key_type_hint),]
        .align_y(Alignment::Center)
        .spacing(5);
    let key_type_field = column![
        key_type_label,
        pick_list::pick_list(
            key_type_options,
            Some(modal_state.key_type),
            Message::KeyUpdateType,
        )
        .width(Length::Fill),
    ]
    .spacing(5);

    let identity_field = if uses_token_identity(modal_state.key_type) {
        token_field(modal_state)
    } else {
        signer_field(state, modal_state)
    };

    let footer = footer(state.views.keys.can_save());
    let body = if let Some(last_edit_info) = last_edit_info {
        column![
            intro,
            last_edit_info,
            alias_field,
            key_type_field,
            identity_field,
            footer,
        ]
    } else {
        column![intro, alias_field, key_type_field, identity_field, footer]
    }
    .spacing(16);

    modal_view(
        Some(title.to_string()),
        None::<Message>,
        Some(Message::KeyCancelModal),
        ModalWidth::M,
        body,
    )
}

fn token_field(modal_state: &EditKeyModalState) -> Element<'_, Message> {
    let is_empty = modal_state.token.trim().is_empty();
    let token_valid = modal_state.token_warning.is_none()
        && liana_connect::keys::token::Token::from_str(&modal_state.token).is_ok();
    let token_value = form::Value {
        value: modal_state.token.clone(),
        warning: if is_empty {
            None
        } else {
            modal_state.token_warning
        },
        valid: token_valid || is_empty,
    };

    let provider_line: Element<'_, Message> = if let Some(provider) = &modal_state.provider {
        row![
            text::new::caption(format!("Provider · {}", provider.name)).style(theme::text::success),
            icon::check_icon().size(13).style(theme::text::success),
        ]
        .spacing(6)
        .align_y(Alignment::Center)
        .into()
    } else {
        text::new::caption(
            "Paste the token from the provider. Their firm name is fetched automatically.",
        )
        .style(theme::text::secondary)
        .into()
    };

    column![
        field_label("Service token"),
        form::Form::new(
            "e.g. 42-absent-cake-eagle",
            &token_value,
            Message::KeyUpdateToken,
        ),
        provider_line,
    ]
    .spacing(5)
    .into()
}

fn signer_field<'a>(state: &'a State, modal_state: &'a EditKeyModalState) -> Element<'a, Message> {
    let is_empty = modal_state.email.trim().is_empty();
    let email_valid = state.views.keys.is_email_valid();
    let hint = match modal_state.key_type {
        ws_business::KeyType::Internal => {
            "The organization member who will set up and sign with this key. They'll be invited by email."
        }
        _ => "The third party who will set up and sign with this key. They'll be invited by email.",
    };
    let signer_picker = column![combobox::editable_menu_combobox(
        "Search organization members or enter email",
        modal_state.email.clone(),
        |selection| Message::KeySelectSigner(selection.email().to_string()),
        signer_entries(modal_state),
        EditableMenuActions {
            on_input: Some(Message::KeyUpdateEmail),
        },
    )];
    let invalid_email: Option<Element<'_, Message>> = (!is_empty && !email_valid).then_some({
        text::new::small_caption("Invalid email!")
            .style(theme::text::error)
            .into()
    });

    let hint_text = text::new::caption(hint).style(theme::text::secondary);
    if let Some(invalid_email) = invalid_email {
        column![
            field_label("Signer"),
            signer_picker,
            invalid_email,
            hint_text,
        ]
    } else {
        column![field_label("Signer"), signer_picker, hint_text]
    }
    .spacing(5)
    .into()
}

fn signer_entries<'a>(
    modal_state: &'a EditKeyModalState,
) -> Vec<MenuEntry<'a, crate::state::views::keys::modal::SignerComboboxOption, Message>> {
    use crate::state::views::keys::modal::SignerComboboxOption;

    let filtered_options = modal_state.filtered_signer_options();
    let fallback_email = modal_state.fallback_signer();
    let selected_email = modal_state.email.trim().to_lowercase();
    let mut entries = Vec::new();

    if !filtered_options.is_empty() {
        let header = if modal_state.key_type == ws_business::KeyType::Internal {
            "From your organization"
        } else {
            crate::views::key_kind_label(&modal_state.key_type)
        };
        entries.push(MenuEntry::Header(combobox::menu_header(header)));
        entries.extend(filtered_options.into_iter().map(|option| {
            let selected = option.email.to_lowercase() == selected_email;
            let tag = member_tag(option.already_used, selected);
            // Members show name over email; a bare email (reused signer) shows just the email.
            let (value, primary, secondary) = if option.name.is_empty() {
                (
                    SignerComboboxOption::FreeEmail(option.email.clone()),
                    option.email.as_str(),
                    "",
                )
            } else {
                (
                    SignerComboboxOption::Member(option.clone()),
                    option.name.as_str(),
                    option.email.as_str(),
                )
            };
            MenuEntry::Option {
                value,
                body: combobox::email_entry(&initials(primary), primary, secondary, tag),
                selected,
            }
        }));
    } else {
        entries.push(MenuEntry::Empty(
            text::new::small_caption("No members match")
                .style(theme::text::secondary)
                .into(),
        ));
    }

    if let Some(email) = fallback_email {
        let selected = email.to_lowercase() == selected_email;
        let tag = if selected {
            combobox::Tag::Selected
        } else {
            combobox::Tag::None
        };
        let name = format!("Use {email}");
        entries.push(MenuEntry::Option {
            value: SignerComboboxOption::FreeEmail(email),
            body: combobox::email_entry("+", &name, "New email address", tag),
            selected,
        });
    }

    entries
}

fn member_tag(already_used: bool, selected: bool) -> combobox::Tag {
    match (already_used, selected) {
        (true, true) => combobox::Tag::AlreadySignerSelected,
        (true, false) => combobox::Tag::AlreadySigner,
        (false, true) => combobox::Tag::Selected,
        (false, false) => combobox::Tag::None,
    }
}

fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    match words.as_slice() {
        [] => String::new(),
        [single] => single.chars().take(2).collect::<String>().to_uppercase(),
        [first, second, ..] => [first, second]
            .iter()
            .filter_map(|word| word.chars().next())
            .collect::<String>()
            .to_uppercase(),
    }
}

fn footer<'a>(can_save: bool) -> Element<'a, Message> {
    row![
        Space::fill_width(),
        btn_cancel(Some(Message::KeyCancelModal)),
        btn_save(can_save.then_some(Message::KeySave)),
    ]
    .spacing(12)
    .align_y(Vertical::Center)
    .into()
}

fn field_label(label: &'static str) -> Element<'static, Message> {
    text::new::b5_bold(label).style(theme::text::primary).into()
}

fn uses_token_identity(key_type: ws_business::KeyType) -> bool {
    matches!(
        key_type,
        ws_business::KeyType::Cosigner | ws_business::KeyType::SafetyNet
    )
}
