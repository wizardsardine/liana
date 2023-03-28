use crate::{
    component::{collapse, text::*},
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
                        Container::new(text(message_clone.to_string()).small().bold())
                            .width(Length::Fill),
                    )
                    .push(
                        Row::new()
                            .align_items(Alignment::Center)
                            .spacing(10)
                            .push(text("Learn more").small().bold())
                            .push(icon::collapse_icon()),
                    ),
            )
            .style(theme::Button::Transparent)
        },
        move || {
            Button::new(
                Row::new()
                    .push(
                        Container::new(text(message.to_owned()).small().bold()).width(Length::Fill),
                    )
                    .push(
                        Row::new()
                            .align_items(Alignment::Center)
                            .spacing(10)
                            .push(text("Learn more").small().bold())
                            .push(icon::collapsed_icon()),
                    ),
            )
            .style(theme::Button::Transparent)
        },
        move || Element::<'a, T>::from(text(error.to_owned()).small()),
    )))
    .padding(15)
    .style(theme::Container::Card(theme::Card::Warning))
    .width(Length::Fill)
}
