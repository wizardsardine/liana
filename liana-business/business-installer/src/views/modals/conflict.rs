use crate::state::{views::modals::ConflictModalState, Msg};
use iced::widget::{column, row, Space};
use liana_ui::{
    component::{
        button::{btn_keep_changes, btn_ok, btn_reload},
        modal::{modal_view, ModalWidth},
    },
    spacing::{HSpacing, VSpacing},
    widget::*,
};

pub fn conflict_modal_view(modal_state: &ConflictModalState) -> Element<'_, Msg> {
    let message = super::installer_modal(&modal_state.message);

    // Buttons - different based on whether this is a choice or info-only
    let footer = if modal_state.is_choice() {
        // Two-button choice: "Keep my changes" and "Reload"
        row![
            Space::fill_width(),
            btn_keep_changes(Some(Msg::ConflictKeepLocal)),
            btn_reload(Some(Msg::ConflictReload)),
            Space::fill_width()
        ]
        .spacing(HSpacing::M)
    } else {
        // Single dismiss button for info-only conflicts
        row![
            Space::fill_width(),
            btn_ok(Some(Msg::ConflictDismiss)),
            Space::fill_width()
        ]
        .spacing(HSpacing::M)
    };

    let body = column![message, footer].spacing(VSpacing::M);

    modal_view(
        Some(modal_state.title.clone()),
        None,
        Some(Msg::ConflictDismiss),
        ModalWidth::M,
        body,
    )
}
