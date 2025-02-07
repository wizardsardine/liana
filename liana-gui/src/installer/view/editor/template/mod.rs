pub mod custom;
pub mod inheritance;
pub mod multisig_security_wallet;

use iced::{Alignment, Length};

use liana_ui::{
    component::text::{h3, p2_regular},
    theme,
    widget::*,
};

use crate::installer::context;
use crate::installer::{message::Message, view::layout};

pub fn choose_descriptor_template(progress: (usize, usize)) -> Element<'static, Message> {
    layout(
        progress,
        None,
        "Choose wallet type",
        Column::new()
            .max_width(800.0)
            .align_x(Alignment::Start)
            .push(
                Button::new(
                    Column::new()
                        .align_x(Alignment::Start)
                        .push(h3("Simple inheritance"))
                        .push(p2_regular("Two keys required, one for yourself to spend and another for your heir.").style(theme::text::secondary))
                        .width(Length::Fill)
                )
                .padding(15)
                .on_press(
                        Message::SelectDescriptorTemplate(
                            context::DescriptorTemplate::SimpleInheritance,
                        )
                ).style(theme::button::secondary)
                .width(Length::Fill),
            )
            .push(
                Button::new(
                    Column::new()
                        .align_x(Alignment::Start)
                        .push(h3("Expanding multisig"))
                        .push(p2_regular("Two keys required to spend, with an extra key as a backup.").style(theme::text::secondary))
                        .width(Length::Fill)
                )
                .padding(15)
                .on_press(
                        Message::SelectDescriptorTemplate(
                            context::DescriptorTemplate::MultisigSecurity,
                        )
                ).style(theme::button::secondary)
                .width(Length::Fill),
            )
            .push(
                Button::new(
                    Column::new()
                        .align_x(Alignment::Start)
                        .push(h3("Build your own"))
                        .push(p2_regular("Create a custom setup that fits all your needs.").style(theme::text::secondary))
                        .width(Length::Fill)
                )
                .padding(15)
                .on_press(
                        Message::SelectDescriptorTemplate(
                            context::DescriptorTemplate::Custom,
                        )
                ).style(theme::button::secondary)
                .width(Length::Fill),
            )
            .spacing(20),
        true,
        Some(Message::Previous),
    )
}
