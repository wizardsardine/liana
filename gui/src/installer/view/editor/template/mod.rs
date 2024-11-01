pub mod custom;
pub mod inheritance;
pub mod multisig_security_wallet;

use iced::{alignment, Alignment, Length};

use liana_ui::{
    color,
    component::text::{h3, p1_regular, p2_regular},
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
            .align_items(Alignment::Start)
            .push(Container::new(
                p1_regular("What do you want your wallet for? This depends on the amount of funds you have, the more funds, the higher the security should be. Not sure about the wallet type? We can help you.")
                    .style(color::GREY_2)
                    .horizontal_alignment(alignment::Horizontal::Left)
            ).align_x(alignment::Horizontal::Left).width(Length::Fill))
            .push(
                Button::new(
                    Column::new()
                        .align_items(Alignment::Start)
                        .push(h3("Simple inheritance"))
                        .push(p2_regular("Two keys required, one for yourself to spend and another for your heir.").style(color::GREY_2))
                        .width(Length::Fill)
                )
                .padding(15)
                .on_press(
                        Message::SelectDescriptorTemplate(
                            context::DescriptorTemplate::SimpleInheritance,
                        )
                ).style(theme::Button::Secondary)
                .width(Length::Fill),
            )
            .push(
                Button::new(
                    Column::new()
                        .align_items(Alignment::Start)
                        .push(h3("Expanding multisig"))
                        .push(p2_regular("Two keys required to spend, with an extra key as a backup.").style(color::GREY_2))
                        .width(Length::Fill)
                )
                .padding(15)
                .on_press(
                        Message::SelectDescriptorTemplate(
                            context::DescriptorTemplate::MultisigSecurity,
                        )
                ).style(theme::Button::Secondary)
                .width(Length::Fill),
            )
            .push(
                Button::new(
                    Column::new()
                        .align_items(Alignment::Start)
                        .push(h3("Build your own"))
                        .push(p2_regular("Create a custom setup that fits all your needs.").style(color::GREY_2))
                        .width(Length::Fill)
                )
                .padding(15)
                .on_press(
                        Message::SelectDescriptorTemplate(
                            context::DescriptorTemplate::Custom,
                        )
                ).style(theme::Button::Secondary)
                .width(Length::Fill),
            )
            .spacing(20),
        true,
        Some(Message::Previous),
    )
}
