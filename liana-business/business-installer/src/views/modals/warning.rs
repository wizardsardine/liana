use crate::state::{views::modals::WarningModalState, Msg};
use iced::{widget::Space, Length};
use liana_ui::{
    component::{
        button::btn_ok,
        modal::{modal_view, none_fn, ModalWidth},
        text,
    },
    theme,
    widget::*,
};

pub fn warning_modal_view(modal_state: &WarningModalState) -> Element<'_, Msg> {
    let message = text::p1_medium(&modal_state.message).style(theme::text::primary);

    let footer = Row::new()
        .spacing(10)
        .push(Space::with_width(Length::Fill))
        .push(btn_ok(Some(Msg::WarningCloseModal)));

    let body = Column::new().push(message).push(footer).spacing(15);

    modal_view(
        Some(modal_state.title.clone()),
        none_fn(),
        Some(|| Msg::WarningCloseModal),
        ModalWidth::M,
        body,
    )
}
