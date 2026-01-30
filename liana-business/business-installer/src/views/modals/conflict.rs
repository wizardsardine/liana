use crate::state::{views::modals::ConflictModalState, Msg};
use iced::{widget::Space, Alignment, Length};
use liana_ui::{
    component::{button, card, text},
    icon, theme,
    widget::*,
};

pub fn conflict_modal_view(modal_state: &ConflictModalState) -> Element<'_, Msg> {
    let header = Row::new()
        .spacing(10)
        .align_y(Alignment::Center)
        .push(text::h3(&modal_state.title))
        .push(Space::with_width(Length::Fill))
        .push(button::transparent(Some(icon::cross_icon()), "").on_press(Msg::ConflictDismiss));

    let message = text::p1_medium(&modal_state.message).style(theme::text::primary);

    // Buttons - different based on whether this is a choice or info-only
    let footer = if modal_state.is_choice() {
        // Two-button choice: "Keep my changes" and "Reload"
        Row::new()
            .spacing(10)
            .push(Space::with_width(Length::Fill))
            .push(
                button::secondary(None, "Keep my changes")
                    .on_press(Msg::ConflictKeepLocal)
                    .width(Length::Fixed(160.0)),
            )
            .push(
                button::primary(None, "Reload")
                    .on_press(Msg::ConflictReload)
                    .width(Length::Fixed(120.0)),
            )
    } else {
        // Single dismiss button for info-only conflicts
        Row::new()
            .spacing(10)
            .push(Space::with_width(Length::Fill))
            .push(
                button::primary(None, "OK")
                    .on_press(Msg::ConflictDismiss)
                    .width(Length::Fixed(120.0)),
            )
    };

    let content = Column::new()
        .push(header)
        .push(message)
        .push(footer)
        .spacing(15)
        .padding(20.0)
        .width(Length::Fixed(500.0));

    card::modal(content).into()
}
