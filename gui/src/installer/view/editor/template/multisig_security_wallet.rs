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
        editor::{
            define_descriptor_advanced_settings, defined_key, path, undefined_key,
            uneditable_defined_key,
        },
        layout,
    },
};

pub fn multisig_security_template_description(
    progress: (usize, usize),
) -> Element<'static, Message> {
    layout(
        progress,
        None,
        "Introduction",
        Column::new()
            .align_items(Alignment::Start)
            .push(h3("Multisig security wallet"))
            .max_width(800.0)
            .push(Container::new(
                p1_regular("For this setup you will need 3 keys: two Primary Keys and a Recovery Key. For security reasons, we suggest you use 3 Hardware Wallets to store them.")
                .style(color::GREY_2)
                .horizontal_alignment(alignment::Horizontal::Left)
            ).align_x(alignment::Horizontal::Left).width(Length::Fill))
            .push(Row::new()
                .spacing(30)
                .push(
                    Row::new()
                    .align_items(Alignment::Center)
                    .spacing(10)
                    .push(icon::round_key_icon().size(H3_SIZE).style(color::GREEN))
                    .push(p1_regular("Primary key #1").bold())
                ).push(
                    Row::new()
                    .align_items(Alignment::Center)
                    .spacing(10)
                    .push(icon::round_key_icon().size(H3_SIZE).style(color::GREEN))
                    .push(p1_regular("Primary key #2").bold())
                ).push(
                    Row::new()
                        .align_items(Alignment::Center)
                        .spacing(10)
                        .push(icon::round_key_icon().size(H3_SIZE).style(color::ORANGE))
                        .push(p1_regular("Recovery key").bold())
            ))
            .push(Container::new(
                p1_regular("The Primary Keys will compose a 2-of-2 multisig which will be your always-active spending policy. In case one of your keys becomes unavailable, after a period of inactivity you will be able to recover your funds using the Recovery Key together with one of your Primary Keys (2-of-3 multisig):")
                .style(color::GREY_2)
                .horizontal_alignment(alignment::Horizontal::Left)
            ).align_x(alignment::Horizontal::Left).width(Length::Fill))
            .push(image::multisig_security_template_description().width(Length::Fill))
            .push(Row::new().push(Space::with_width(Length::Fill)).push(button::primary(None, "Select").width(Length::Fixed(200.0)).on_press(Message::Next)))
            .spacing(20),
        true,
        Some(Message::Previous),
    )
}

pub fn multisig_security_template<'a>(
    progress: (usize, usize),
    use_taproot: bool,
    primary_keys: Vec<Option<&'a Key>>,
    recovery_keys: Vec<Option<&'a Key>>,
    sequence: u16,
    threshold: usize,
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
                    primary_keys.len(),
                    primary_keys
                        .iter()
                        .enumerate()
                        .map(|(i, primary_key)| {
                            if let Some(key) = primary_key {
                                defined_key(
                                    &key.name,
                                    color::GREEN,
                                    format!("Primary key #{}", i + 1),
                                    if use_taproot && !key.is_compatible_taproot {
                                        Some("Key is not compatible with taproot")
                                    } else {
                                        None
                                    },
                                    true,
                                )
                            } else {
                                undefined_key(
                                    color::GREEN,
                                    format!("Primary key #{}", i + 1),
                                    !primary_keys[0..i].iter().any(|k| k.is_none()),
                                    true,
                                )
                            }
                            .map(move |msg| message::DefinePath::Key(i, msg))
                        })
                        .collect(),
                    true,
                )
                .map(move |msg| {
                    if let message::DefinePath::Key(i, message::DefineKey::Edit) = msg {
                        Message::DefineDescriptor(message::DefineDescriptor::KeysEdit(vec![
                            (0, i),
                            (1, i),
                        ]))
                    } else {
                        Message::DefineDescriptor(message::DefineDescriptor::Path(0, msg))
                    }
                }),
            )
            .push(
                path(
                    color::ORANGE,
                    None,
                    sequence,
                    false,
                    threshold,
                    recovery_keys
                        .iter()
                        .enumerate()
                        .map(|(j, recovery_key)| {
                            if let Some(key) = recovery_key {
                                if j < 2 {
                                    uneditable_defined_key(
                                        &key.name,
                                        color::GREEN,
                                        format!("Primary key #{}", j + 1),
                                        if use_taproot && !key.is_compatible_taproot {
                                            Some("Key is not compatible with Taproot")
                                        } else {
                                            None
                                        },
                                    )
                                } else {
                                    defined_key(
                                        &key.name,
                                        color::ORANGE,
                                        "Recovery key".to_string(),
                                        if use_taproot && !key.is_compatible_taproot {
                                            Some("Key is not compatible with Taproot")
                                        } else {
                                            None
                                        },
                                        true,
                                    )
                                }
                            } else {
                                undefined_key(
                                    if j < 2 { color::GREEN } else { color::ORANGE },
                                    if j < 2 {
                                        format!("Primary key #{}", j + 1)
                                    } else {
                                        "Recovery key".to_string()
                                    },
                                    !(primary_keys.iter().any(|k| k.is_none())
                                        || recovery_keys[0..j].iter().any(|k| k.is_none())),
                                    true,
                                )
                            }
                            .map(move |msg| message::DefinePath::Key(j, msg))
                        })
                        .collect(),
                    true,
                )
                .map(move |msg| {
                    if let message::DefinePath::Key(i, message::DefineKey::Edit) = msg {
                        Message::DefineDescriptor(message::DefineDescriptor::KeysEdit(if i < 2 {
                            vec![(0, i), (1, i)]
                        } else {
                            // recovery path is the path with three keys
                            vec![(1, i)]
                        }))
                    } else {
                        Message::DefineDescriptor(message::DefineDescriptor::Path(1, msg))
                    }
                }),
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
            .push(Space::with_height(100.0))
            .spacing(20),
        true,
        Some(Message::Previous),
    )
}
