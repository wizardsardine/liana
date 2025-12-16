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

pub fn render_modal(state: &State) -> Option<Element<'_, Message>> {
    if let Some(modal_state) = &state.views.keys.edit_key {
        return Some(edit_key_modal(modal_state));
    }
    None
}

pub fn edit_key_modal(modal_state: &EditKeyModalState) -> Element<'_, Message> {
    let mut content = Column::new()
        .spacing(15)
        .padding(20.0)
        .width(Length::Fixed(500.0));

    // Header
    content = content.push(
        Row::new()
            .spacing(10)
            .align_y(Alignment::Center)
            .push(text::h3("Edit Key"))
            .push(Space::with_width(Length::Fill))
            .push(
                button::transparent(Some(icon::cross_icon()), "").on_press(Message::KeyCancelModal),
            ),
    );

    // Alias input
    let alias_value = form::Value {
        value: modal_state.alias.clone(),
        warning: None,
        valid: true,
    };
    content = content.push(
        Column::new()
            .spacing(5)
            .push(text::p1_regular("Alias"))
            .push(form::Form::new(
                "Enter key alias",
                &alias_value,
                Message::KeyUpdateAlias,
            )),
    );

    // Description input
    let desc_value = form::Value {
        value: modal_state.description.clone(),
        warning: None,
        valid: true,
    };
    content = content.push(
        Column::new()
            .spacing(5)
            .push(text::p1_regular("Description"))
            .push(form::Form::new(
                "Enter description",
                &desc_value,
                Message::KeyUpdateDescr,
            )),
    );

    // Email input
    let email_value = form::Value {
        value: modal_state.email.clone(),
        warning: None,
        valid: true,
    };
    content = content.push(
        Column::new()
            .spacing(5)
            .push(text::p1_regular("Email"))
            .push(form::Form::new(
                "Enter email",
                &email_value,
                Message::KeyUpdateEmail,
            )),
    );

    // Key type picker
    let key_types: &[liana_connect::KeyType] = &[
        liana_connect::KeyType::Internal,
        liana_connect::KeyType::External,
        liana_connect::KeyType::Cosigner,
        liana_connect::KeyType::SafetyNet,
    ];
    let current_type = modal_state.key_type;
    content = content.push(
        Column::new()
            .spacing(5)
            .push(text::p1_regular("Key Type"))
            .push(
                pick_list(key_types, Some(current_type), Message::KeyUpdateType)
                    .width(Length::Fill),
            ),
    );

    // Buttons
    content = content.push(
        Row::new()
            .spacing(10)
            .push(
                button::secondary(None, "Cancel")
                    .on_press(Message::KeyCancelModal)
                    .width(Length::Fixed(120.0)),
            )
            .push(
                button::primary(None, "Save")
                    .on_press(Message::KeySave)
                    .width(Length::Fixed(120.0)),
            ),
    );

    card::modal(content).into()
}
