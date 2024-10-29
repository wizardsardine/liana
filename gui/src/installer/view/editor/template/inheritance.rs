use iced::{alignment, widget::Space, Alignment, Length};

use liana_ui::{
    color,
    component::{
        button, collapse,
        text::{h3, p1_regular, text, Text},
    },
    icon, image, theme,
    widget::*,
};

use crate::installer::{
    context,
    message::{self, Message},
    step::descriptor::editor::key::Key,
    view::{
        editor::{define_descriptor_advanced_settings, defined_key, path, undefined_key},
        layout,
    },
};

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
                p1_regular("For this Inheritance wallet you will need 2 Keys: Your Primary Key and an Inheritance Key to be given to a chosen heir. For security reasons, we suggest you use 2 Hardware Wallets to store them.")
                .style(color::GREY_2)
                .horizontal_alignment(alignment::Horizontal::Left)
            ).align_x(alignment::Horizontal::Left).width(Length::Fill))
            .push(image::inheritance_template_description().width(Length::Fill))
            .push(Container::new(
                p1_regular("Your relative’s Inheritance Key will become active only if you don’t move the coins in your wallet for the defined period of time, enabling him/her to recover your funds while not being able to access them before that.")
                .style(color::GREY_2)
                .horizontal_alignment(alignment::Horizontal::Left)
            ).align_x(alignment::Horizontal::Left).width(Length::Fill))
            .push(Row::new().push(Space::with_width(Length::Fill)).push(button::primary(None, "Select").width(Length::Fixed(200.0)).on_press(Message::Next)))
            .spacing(20),
        true,
        Some(Message::Previous),
    )
}

pub fn inheritance_template<'a>(
    progress: (usize, usize),
    use_taproot: bool,
    primary_key: Option<&'a Key>,
    recovery_key: Option<&'a Key>,
    sequence: u16,
    valid: bool,
) -> Element<'a, Message> {
    layout(
        progress,
        None,
        "Set keys",
        Column::new()
            .align_items(Alignment::Start)
            .max_width(1000.0)
            .push(collapse::Collapse::new(
                || {
                    Button::new(
                        Row::new()
                            .align_items(Alignment::Center)
                            .spacing(10)
                            .push(text("Advanced settings").small().bold())
                            .push(icon::collapse_icon()),
                    )
                    .style(theme::Button::Transparent)
                },
                || {
                    Button::new(
                        Row::new()
                            .align_items(Alignment::Center)
                            .spacing(10)
                            .push(text("Advanced settings").small().bold())
                            .push(icon::collapsed_icon()),
                    )
                    .style(theme::Button::Transparent)
                },
                move || define_descriptor_advanced_settings(use_taproot),
            ))
            .push(
                path(
                    color::GREEN,
                    None,
                    0,
                    false,
                    1,
                    vec![if let Some(key) = primary_key {
                        defined_key(
                            &key.name,
                            color::GREEN,
                            "Primary key",
                            if use_taproot && !key.is_compatible_taproot {
                                Some("Key is not compatible with Taproot")
                            } else {
                                None
                            },
                            true,
                        )
                    } else {
                        undefined_key(color::GREEN, "Primary key", true, true)
                    }
                    .map(|msg| message::DefinePath::Key(0, msg))],
                    true,
                )
                .map(|msg| Message::DefineDescriptor(message::DefineDescriptor::Path(0, msg))),
            )
            .push(
                path(
                    color::WHITE,
                    None,
                    sequence,
                    false,
                    1,
                    vec![if let Some(key) = recovery_key {
                        defined_key(
                            &key.name,
                            color::WHITE,
                            "Inheritance key",
                            if use_taproot && !key.is_compatible_taproot {
                                Some("Key is not compatible with taproot")
                            } else {
                                None
                            },
                            true,
                        )
                    } else {
                        undefined_key(color::WHITE, "Inheritance key", primary_key.is_some(), true)
                    }
                    .map(|msg| message::DefinePath::Key(0, msg))],
                    true,
                )
                .map(|msg| Message::DefineDescriptor(message::DefineDescriptor::Path(1, msg))),
            )
            .push(
                Row::new()
                    .push(
                        button::secondary(None, "Customize")
                            .width(Length::Fixed(200.0))
                            .on_press(Message::DefineDescriptor(
                                message::DefineDescriptor::ChangeTemplate(
                                    context::DescriptorTemplate::Custom,
                                ),
                            )),
                    )
                    .push(Space::with_width(Length::Fill))
                    .push(
                        button::primary(None, "Continue")
                            .width(Length::Fixed(200.0))
                            .on_press_maybe(if valid { Some(Message::Next) } else { None }),
                    ),
            )
            .spacing(20),
        true,
        Some(Message::Previous),
    )
}
