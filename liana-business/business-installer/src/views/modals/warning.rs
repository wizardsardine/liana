use crate::state::{views::modals::WarningModalState, Msg};
use iced::{widget::Space, Alignment, Length};
use liana_ui::{
    component::{button, card, text},
    icon, theme,
    widget::*,
};

pub fn warning_modal_view(modal_state: &WarningModalState) -> Element<'_, Msg> {
    let header = Row::new()
        .spacing(10)
        .align_y(Alignment::Center)
        .push(text::h3(&modal_state.title))
        .push(Space::with_width(Length::Fill))
        .push(button::transparent(Some(icon::cross_icon()), "").on_press(Msg::WarningCloseModal));

    let message = text::p1_medium(&modal_state.message).style(theme::text::primary);

    let footer = Row::new()
        .spacing(10)
        .push(Space::with_width(Length::Fill))
        .push(
            button::primary(None, "OK")
                .on_press(Msg::WarningCloseModal)
                .width(Length::Fixed(120.0)),
        );

    let content = Column::new()
        .push(header)
        .push(message)
        .push(footer)
        .spacing(15)
        .padding(20.0)
        .width(Length::Fixed(500.0));

    card::modal(content).into()
}
