use iced::{alignment, widget::Space, Alignment, Length};

use liana_ui::{
    color,
    component::{
        button,
        text::{h3, p1_regular},
    },
    image,
    widget::*,
};

use crate::installer::{message::Message, view::layout};

pub fn inheritance_template_description(progress: (usize, usize)) -> Element<'static, Message> {
    layout(
        progress,
        None,
        "Introduction",
        Column::new()
            .align_items(Alignment::Start)
            .push(h3("Inheritance wallet"))
            .max_width(800.0)
            .push(Container::new(
                p1_regular("In this current setup you will need 2 Keys for your wallet. For security reasons, we suggest you to use 2 Hardware Wallets to store them.")
                .style(color::GREY_3)
                .horizontal_alignment(alignment::Horizontal::Left)
            ).align_x(alignment::Horizontal::Left).width(Length::Fill))
            .push(Container::new(
                p1_regular("For this Inheritance wallet you will need 2 Keys: Your Primary Key and an Inheritance Key to be given to a chosen relative.")
                .style(color::GREY_3)
                .horizontal_alignment(alignment::Horizontal::Left)
            ).align_x(alignment::Horizontal::Left).width(Length::Fill))
            .push(image::inheritance_template_description().width(Length::Fill))
            .push(Container::new(
                p1_regular("Your relative’s Inheritance Key will become active only if you don’t move the coins in your wallet for the defined period of time, enabling him/her to recover your funds while not being able to access them before that.")
                .style(color::GREY_3)
                .horizontal_alignment(alignment::Horizontal::Left)
            ).align_x(alignment::Horizontal::Left).width(Length::Fill))
            .push(Row::new().push(Space::with_width(Length::Fill)).push(button::primary(None, "Select").width(Length::Fixed(200.0)).on_press(Message::Next)))
            .spacing(20),
        true,
        Some(Message::Previous),
    )
}
