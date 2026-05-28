use iced::{alignment::Horizontal, Length};

use crate::{
    component::text,
    theme,
    widget::{Button, Container, Element},
};

/// "See more" button paginating an history. Shows "Fetching ..." and
/// is disabled while `processing`.
pub fn see_more<'a, M: Clone + 'a>(processing: bool, next: M) -> Element<'a, M> {
    let label = if processing {
        "Fetching ..."
    } else {
        "See more"
    };

    let button = Button::new(
        text::text(label)
            .width(Length::Fill)
            .align_x(Horizontal::Center),
    )
    .width(Length::Fill)
    .padding(15)
    .style(theme::button::transparent_border)
    .on_press_maybe((!processing).then_some(next));

    Container::new(button)
        .width(Length::Fill)
        .style(theme::card::simple)
        .into()
}
