use crate::ui::{
    color,
    component::{button, collapse, text::*},
    icon,
};
use iced::{
    widget::{container, Button, Container, Row},
    Alignment, Element, Length,
};

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
            .style(button::Style::Transparent.into())
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
            .style(button::Style::Transparent.into())
        },
        move || Element::<'a, T>::from(text(error.to_owned()).small()),
    )))
    .padding(15)
    .style(WarningStyle)
    .width(Length::Fill)
}

pub struct WarningStyle;
impl container::StyleSheet for WarningStyle {
    type Style = iced::Theme;
    fn appearance(&self, _style: &Self::Style) -> container::Appearance {
        container::Appearance {
            border_radius: 0.0,
            text_color: iced::Color::BLACK.into(),
            background: color::WARNING.into(),
            border_color: color::WARNING,
            ..container::Appearance::default()
        }
    }
}

impl From<WarningStyle> for Box<dyn container::StyleSheet<Style = iced::Theme>> {
    fn from(s: WarningStyle) -> Box<dyn container::StyleSheet<Style = iced::Theme>> {
        Box::new(s)
    }
}

impl From<WarningStyle> for iced::theme::Container {
    fn from(i: WarningStyle) -> iced::theme::Container {
        iced::theme::Container::Custom(i.into())
    }
}
