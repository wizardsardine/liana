use crate::state::{views::modals::WarningModalState, Msg};
use iced::{widget::Space, Length};
use liana_ui::{
    component::{
        button,
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
        .push(
            button::primary(None, "OK")
                .on_press(Msg::WarningCloseModal)
                .width(Length::Fixed(120.0)),
        );

    let body = Column::new().push(message).push(footer).spacing(15);

    modal_view(
        Some(modal_state.title.clone()),
        none_fn(),
        Some(|| Msg::WarningCloseModal),
        ModalWidth::M,
        body,
    )
}
