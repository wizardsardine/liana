use crate::state::{views::keys::EditKeyModalState, Message, State};
use iced::{
    widget::{pick_list, Space},
    Alignment, Length,
};
use liana_ui::{
    component::{button, card, form, text},
    icon,
    widget::*,
};

pub fn key_modal_view(state: &State) -> Option<Element<'_, Message>> {
    if let Some(modal_state) = &state.views.keys.edit_key {
        return Some(edit_key_modal_view(modal_state));
    }
    None
}

pub fn edit_key_modal_view(modal_state: &EditKeyModalState) -> Element<'_, Message> {
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

    // Alias input - validate (must not be empty)
    // No warning if empty, but Save button will be disabled
    let alias_valid = !modal_state.alias.trim().is_empty();
    let alias_value = form::Value {
        value: modal_state.alias.clone(),
        warning: None, // No warning displayed for empty field
        valid: alias_valid || modal_state.alias.trim().is_empty(), // Don't show red border if empty
    };
    let alias_input = Column::new()
        .spacing(5)
        .push(text::p1_regular("Alias"))
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
        .push(text::p1_regular("Description"))
        .push(form::Form::new(
            "Enter description",
            &desc_value,
            Message::KeyUpdateDescr,
        ));

    // Email input - validate (same as login flow)
    // No warning if empty, but Save button will be disabled
    // Only show warning if not empty but invalid format
    let is_empty = modal_state.email.trim().is_empty();
    let email_valid = if is_empty {
        false // Empty is invalid (required field)
    } else {
        email_address::EmailAddress::parse_with_options(
            &modal_state.email,
            email_address::Options::default().with_required_tld(),
        )
        .is_ok()
    };
    let email_value = form::Value {
        value: modal_state.email.clone(),
        warning: if is_empty {
            None // No warning for empty field
        } else if !email_valid {
            Some("Invalid email!") // Only show warning if not empty but invalid
        } else {
            None
        },
        valid: email_valid || is_empty, // Don't show red border if empty
    };
    let email_input = Column::new()
        .spacing(5)
        .push(text::p1_regular("Email"))
        .push(form::Form::new(
            "Enter email",
            &email_value,
            Message::KeyUpdateEmail,
        ));

    // Key type picker
    let key_types: &[liana_connect::KeyType] = &[
        liana_connect::KeyType::Internal,
        liana_connect::KeyType::External,
        liana_connect::KeyType::Cosigner,
        liana_connect::KeyType::SafetyNet,
    ];
    let key_type_picker = Column::new()
        .spacing(5)
        .push(text::p1_regular("Key Type"))
        .push(
            pick_list(key_types, Some(modal_state.key_type), Message::KeyUpdateType)
                .width(Length::Fill),
        );

    // Footer - Cancel and Save buttons (aligned right)
    // Save button is disabled if alias or email is invalid
    let can_save = alias_valid && email_valid;
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
        .push(alias_input)
        .push(description_input)
        .push(email_input)
        .push(key_type_picker)
        .push(footer)
        .spacing(15)
        .padding(20.0)
        .width(Length::Fixed(500.0));

    card::modal(content).into()
}
