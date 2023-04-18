use crate::{
    color,
    component::{collapse, text},
    icon, theme,
    widget::*,
};
use iced::{Alignment, Length};

pub fn warning<'a, T: 'a + Clone>(message: String, error: String) -> Container<'a, T> {
    let message_clone = message.clone();
    Container::new(Container::new(collapse::Collapse::new(
        move || {
            Button::new(
                Row::new()
                    .push(
                        Container::new(
                            text::p1_bold(message_clone.to_string()).style(color::WHITE),
                        )
                        .width(Length::Fill),
                    )
                    .push(
                        Row::new()
                            .align_items(Alignment::Center)
                            .spacing(10)
                            .push(text::p1_bold("Learn more").style(color::WHITE))
                            .push(icon::collapse_icon()),
                    ),
            )
            .style(theme::Button::Transparent)
        },
        move || {
            Button::new(
                Row::new()
                    .push(
                        Container::new(text::p1_bold(message.to_owned()).style(color::WHITE))
                            .width(Length::Fill),
                    )
                    .push(
                        Row::new()
                            .align_items(Alignment::Center)
                            .spacing(10)
                            .push(text::p1_bold("Learn more").style(color::WHITE))
                            .push(icon::collapsed_icon()),
                    ),
            )
            .style(theme::Button::Transparent)
        },
        move || Element::<'a, T>::from(text::p2_regular(error.to_owned())),
    )))
    .padding(15)
    .style(theme::Container::Card(theme::Card::Warning))
    .width(Length::Fill)
}
