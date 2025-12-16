pub mod modal;

use crate::state::{Msg, State};
use iced::{widget::Space, Alignment, Length};
use liana_ui::{
    component::{button, card, text},
    icon,
    widget::*,
};

pub fn keys_view(state: &State) -> Element<'_, Msg> {
    let mut column = Column::new()
        .spacing(20)
        .padding(20.0)
        .width(Length::Fill)
        .align_x(Alignment::Start);

    // Header
    column = column.push(
        Row::new()
            .spacing(10)
            .align_y(Alignment::Center)
            .push(button::transparent(Some(icon::arrow_back()), "").on_press(Msg::NavigateToHome))
            .push(text::h2("Keys")),
    );

    // Add key button
    column = column.push(
        button::primary(Some(icon::plus_icon()), "Add Key")
            .on_press(Msg::KeyAdd)
            .width(Length::Fixed(200.0)),
    );

    // List of keys
    for (key_id, key) in state.app.keys.iter() {
        let key_card = key_card(*key_id, key);
        column = column.push(key_card);
    }

    Container::new(column.width(Length::Fill).height(Length::Fill))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn key_card<'a>(key_id: u8, key: &liana_connect::Key) -> Element<'a, Msg> {
    // Single row with all key information
    let mut key_row = Row::new().spacing(15).align_y(Alignment::Center);

    // Icon and alias
    key_row = key_row.push(icon::key_icon());
    key_row = key_row.push(text::h5_medium(&key.alias));

    // Key details
    key_row = key_row.push(text::p2_regular(format!("Type: {}", key.key_type.as_str())));

    if !key.description.is_empty() {
        key_row = key_row.push(text::p2_regular(format!(
            "Description: {}",
            key.description
        )));
    }

    if !key.email.is_empty() {
        key_row = key_row.push(text::p2_regular(format!("Email: {}", key.email)));
    }

    // Push buttons to the right side
    key_row = key_row.push(Space::with_width(Length::Fill));
    key_row = key_row
        .push(button::transparent(Some(icon::pencil_icon()), "").on_press(Msg::KeyEdit(key_id)));
    key_row = key_row
        .push(button::transparent(Some(icon::trash_icon()), "").on_press(Msg::KeyDelete(key_id)));

    card::simple(key_row).into()
}
