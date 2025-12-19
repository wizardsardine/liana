use crate::state::{views::modals::ConflictModalState, Msg};
use iced::{widget::Space, Alignment, Length};
use liana_ui::{
    component::{button, card, text},
    icon,
    widget::*,
};

pub fn render_conflict_modal(modal_state: &ConflictModalState) -> Element<'_, Msg> {
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
                button::transparent(Some(icon::cross_icon()), "").on_press(Msg::ConflictDismiss),
            ),
    );

    // Message
    content = content.push(text::p1_regular(&modal_state.message));

    // Buttons - different based on whether this is a choice or info-only
    if modal_state.is_choice() {
        // Two-button choice: "Keep my changes" and "Reload"
        content = content.push(
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
                ),
        );
    } else {
        // Single dismiss button for info-only conflicts
        content = content.push(
            Row::new()
                .spacing(10)
                .push(Space::with_width(Length::Fill))
                .push(
                    button::primary(None, "OK")
                        .on_press(Msg::ConflictDismiss)
                        .width(Length::Fixed(120.0)),
                ),
        );
    }

    card::modal(content).into()
}
