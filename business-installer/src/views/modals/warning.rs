use crate::state::{views::modals::WarningModalState, Msg};
use iced::{widget::Space, Alignment, Length};
use liana_ui::{
    component::{button, card, text},
    icon,
    widget::*,
};

pub fn render_warning_modal(modal_state: &WarningModalState) -> Element<'_, Msg> {
    let mut content = Column::new()
        .spacing(15)
        .padding(20.0)
        .width(Length::Fixed(500.0));

    // Header
    content = content.push(
        Row::new()
            .spacing(10)
            .align_y(Alignment::Center)
            .push(text::h3(&modal_state.title))
            .push(Space::with_width(Length::Fill))
            .push(
                button::transparent(Some(icon::cross_icon()), "").on_press(Msg::WarningCloseModal),
            ),
    );

    // Message
    content = content.push(text::p1_regular(&modal_state.message));

    // OK button
    content = content.push(
        Row::new()
            .spacing(10)
            .push(Space::with_width(Length::Fill))
            .push(
                button::primary(None, "OK")
                    .on_press(Msg::WarningCloseModal)
                    .width(Length::Fixed(120.0)),
            ),
    );

    card::modal(content).into()
}
