pub mod custom;
pub mod inheritance;
pub mod multisig_security_wallet;

use iced::{alignment, Alignment, Length};

use liana_ui::{
    color,
    component::{
        button, card,
        text::{h3, p1_regular, p2_regular},
    },
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
                card::simple(
                    Row::new()
                        .align_items(Alignment::Center)
                        .push(
                            Column::new()
                                .align_items(Alignment::Start)
                                .push(h3("Simple inheritance"))
                                .push(p2_regular("Two keys required, one for yourself to spend and another for your heir.").style(color::GREY_2))
                                .width(Length::Fill)
                        )
                        .push(button::secondary(None, "Select").on_press(
                            Message::SelectDescriptorTemplate(
                                context::DescriptorTemplate::SimpleInheritance,
                            ),
                        )),
                )
                .width(Length::Fill),
            )
            .push(
                card::simple(
                    Row::new()
                        .align_items(Alignment::Center)
                        .push(
                            Column::new()
                                .align_items(Alignment::Start)
                                .push(h3("Multisig security wallet"))
                                .push(p2_regular("A secure scheme requiring stricter multiparty signature and recovery.").style(color::GREY_2))
                                .width(Length::Fill)
                        )
                        .push(button::secondary(None, "Select").on_press(
                            Message::SelectDescriptorTemplate(
                                context::DescriptorTemplate::MultisigSecurity,
                            ),
                        )),
                )
                .width(Length::Fill),
            )
            .push(
                card::simple(
                    Row::new()
                        .align_items(Alignment::Center)
                        .push(
                            Column::new()
                                .align_items(Alignment::Start)
                                .push(h3("Custom (choose your own)"))
                                .push(p2_regular("Create a custom setup that fits all your needs").style(color::GREY_2))
                                .width(Length::Fill)
                        )
                        .push(button::secondary(None, "Select").on_press(
                            Message::SelectDescriptorTemplate(
                                context::DescriptorTemplate::Custom ,
                            ),
                        )),
                )
                .width(Length::Fill),
            )
            .spacing(20),
        true,
        Some(Message::Previous),
    )
}
