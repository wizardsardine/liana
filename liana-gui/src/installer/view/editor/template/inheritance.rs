use iced::{alignment, widget::Space, Alignment, Length};

use liana_ui::{
    color,
    component::{
        button, collapse,
        text::{h3, p1_regular, text, Text, H3_SIZE},
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
            .align_x(Alignment::Start)
            .push(h3("Simple inheritance wallet"))
            .max_width(800.0)
            .push(Container::new(
                p1_regular("For this setup you will need 2 Keys: Your Primary Key (for yourself) and an Inheritance Key (for your heir). For security reasons, we suggest you use a separate Hardware Wallet for each key.")
                .color(color::GREY_2)
                .align_x(alignment::Horizontal::Left)
            ).align_x(alignment::Horizontal::Left).width(Length::Fill))
            .push(Row::new()
                .spacing(30)
                .push(
                    Row::new()
                    .align_y(Alignment::Center)
                    .spacing(10)
                    .push(icon::round_key_icon().size(H3_SIZE).color(color::GREEN))
                    .push(p1_regular("Primary key").bold())
                ).push(
                    Row::new()
                        .align_y(Alignment::Center)
                        .spacing(10)
                        .push(icon::round_key_icon().size(H3_SIZE).color(color::WHITE))
                        .push(p1_regular("Inheritance key").bold())
            ))
            .push(Container::new(
                p1_regular("You will always be able to spend using your Primary Key.
After a period of inactivity (but not before that) your Inheritance Key will become able to recover your funds.")
                .color(color::GREY_2)
                .align_x(alignment::Horizontal::Left)
            ).align_x(alignment::Horizontal::Left).width(Length::Fill))
            .push(image::inheritance_template_description().width(Length::Fill))
            .push(Row::new().push(Space::with_width(Length::Fill)).push(button::primary(None, "Next").width(Length::Fixed(200.0)).on_press(Message::Next)))
            .push(Space::with_height(50.0))
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
            .align_x(Alignment::Start)
            .max_width(1000.0)
            .push(collapse::Collapse::new(
                || {
                    Button::new(
                        Row::new()
                            .align_y(Alignment::Center)
                            .spacing(10)
                            .push(text("Advanced settings").small().bold())
                            .push(icon::collapse_icon()),
                    )
                    .style(theme::button::transparent)
                },
                || {
                    Button::new(
                        Row::new()
                            .align_y(Alignment::Center)
                            .spacing(10)
                            .push(text("Advanced settings").small().bold())
                            .push(icon::collapsed_icon()),
                    )
                    .style(theme::button::transparent)
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
                                Some("This device does not support Taproot")
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
                                Some("This device does not support Taproot")
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
