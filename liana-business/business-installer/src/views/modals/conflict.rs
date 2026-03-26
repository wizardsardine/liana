use crate::state::{views::modals::ConflictModalState, Msg};
use iced::{widget::Space, Length};
use liana_ui::{
    component::{
        button::{btn_ok, btn_primary, btn_secondary, BtnWidth},
        modal::{modal_view, none_fn, ModalWidth},
        text,
    },
    theme,
    widget::*,
};

pub fn conflict_modal_view(modal_state: &ConflictModalState) -> Element<'_, Msg> {
    let message = text::p1_medium(&modal_state.message).style(theme::text::primary);

    // Buttons - different based on whether this is a choice or info-only
    let footer = if modal_state.is_choice() {
        // Two-button choice: "Keep my changes" and "Reload"
        Row::new()
            .spacing(10)
            .push(Space::with_width(Length::Fill))
            .push(btn_secondary(
                None,
                "Keep my changes",
                BtnWidth::L,
                Some(Msg::ConflictKeepLocal),
            ))
            .push(btn_primary(
                None,
                "Reload",
                BtnWidth::M,
                Some(Msg::ConflictReload),
            ))
    } else {
        // Single dismiss button for info-only conflicts
        Row::new()
            .spacing(10)
            .push(Space::with_width(Length::Fill))
            .push(btn_ok(Some(Msg::ConflictDismiss)))
    };

    let body = Column::new().push(message).push(footer).spacing(15);

    modal_view(
        Some(modal_state.title.clone()),
        none_fn(),
        Some(|| Msg::ConflictDismiss),
        ModalWidth::M,
        body,
    )
}
