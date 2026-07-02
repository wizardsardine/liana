use crate::state::{views::modals::WarningModalState, Msg};
use iced::widget::{column, row, Space};
use liana_ui::{
    component::{
        button::btn_ok,
        modal::{modal_view, ModalWidth},
    },
    widget::*,
};

use super::installer_modal;

pub fn warning_modal_view(modal_state: &WarningModalState) -> Element<'_, Msg> {
    let message = installer_modal(&modal_state.message);

    let footer = row![
        Space::fill_width(),
        btn_ok(Some(Msg::WarningCloseModal)),
        Space::fill_width()
    ]
    .spacing(10);

    let body = column![message, footer].spacing(15);

    modal_view(
        Some(modal_state.title.clone()),
        None,
        Some(Msg::WarningCloseModal),
        ModalWidth::M,
        body,
    )
}
