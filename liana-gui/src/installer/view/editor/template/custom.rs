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
    message::{self, Message},
    step::descriptor::editor::key::Key,
    view::{
        editor::{define_descriptor_advanced_settings, defined_key, path, undefined_key},
        layout,
    },
};

pub fn custom_template_description(progress: (usize, usize)) -> Element<'static, Message> {
    layout(
        progress,
        None,
        "Introduction",
        Column::new()
            .align_x(Alignment::Start)
            .push(h3("Build your own"))
            .max_width(800.0)
            .push(Container::new(
                p1_regular("For this setup you will need to define your primary and recovery spending policies. For security reasons, we suggest you use a separate Hardware Wallet for each key belonging to them.")
                .color(color::GREY_2)
                .align_x(alignment::Horizontal::Left)
            ).align_x(alignment::Horizontal::Left).width(Length::Fill))
            .push(Container::new(
                p1_regular("The keys belonging to your primary policy can always spend. Those belonging to the recovery policies will be able to spend only after a defined time of wallet inactivity, allowing for secure recovery and advanced spending policies.")
                .color(color::GREY_2)
                .align_x(alignment::Horizontal::Left)
            ).align_x(alignment::Horizontal::Left).width(Length::Fill))
            .push(image::custom_template_description().width(Length::Fill))
            .push(Row::new().push(Space::with_width(Length::Fill)).push(button::primary(None, "Next").width(Length::Fixed(200.0)).on_press(Message::Next)))
            .push(Space::with_height(50.0))
            .spacing(20),
        true,
        Some(Message::Previous),
    )
}

pub struct Path<'a> {
    pub keys: Vec<Option<&'a Key>>,
    pub sequence: u16,
    pub duplicate_sequence: bool,
    pub threshold: usize,
}

pub fn custom_template<'a>(
    progress: (usize, usize),
    use_taproot: bool,
    primary_path: Path<'a>,
    recovery_paths: &mut dyn Iterator<Item = Path<'a>>,
    num_recovery_paths: usize,
    valid: bool,
) -> Element<'a, Message> {
    let prim_keys_fixed = primary_path.keys.len() < 2; // can only delete a primary key if there are 2 or more
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
                    Some("Primary spending option:".to_string()),
                    primary_path.sequence,
                    primary_path.duplicate_sequence,
                    primary_path.threshold,
                    primary_path
                        .keys
                        .iter()
                        .enumerate()
                        .map(|(i, primary_key)| {
                            if let Some(key) = primary_key {
                                defined_key(
                                    &key.name,
                                    color::GREEN,
                                    "Primary key",
                                    if use_taproot && !key.is_compatible_taproot {
                                        Some("This device does not support Taproot")
                                    } else {
                                        None
                                    },
                                    prim_keys_fixed,
                                )
                            } else {
                                undefined_key(
                                    color::GREEN,
                                    "Primary key",
                                    !primary_path.keys[0..i].iter().any(|k| k.is_none()),
                                    prim_keys_fixed,
                                )
                            }
                            .map(move |msg| message::DefinePath::Key(i, msg))
                        })
                        .collect(),
                    false,
                )
                .map(|msg| Message::DefineDescriptor(message::DefineDescriptor::Path(0, msg))),
            )
            .push(recovery_paths.into_iter().enumerate().fold(
                Column::new().spacing(20),
                |col, (i, p)| {
                    col.push(
                        path(
                            color::ORANGE,
                            Some(format!("Recovery option #{}:", i + 1)),
                            p.sequence,
                            p.duplicate_sequence,
                            p.threshold,
                            p.keys
                                .iter()
                                .enumerate()
                                .map(|(j, recovery_key)| {
                                    // We cannot delete a key if doing so would remove all recovery paths,
                                    // i.e. if there is only 1 recovery path and it contains only 1 key.
                                    let fixed = num_recovery_paths < 2 && p.keys.len() < 2;
                                    if let Some(key) = recovery_key {
                                        defined_key(
                                            &key.name,
                                            color::ORANGE,
                                            "Recovery key",
                                            if use_taproot && !key.is_compatible_taproot {
                                                Some("This device does not support Taproot")
                                            } else {
                                                None
                                            },
                                            fixed,
                                        )
                                    } else {
                                        undefined_key(
                                            color::ORANGE,
                                            "Recovery key",
                                            !p.keys[0..j].iter().any(|k| k.is_none()),
                                            fixed,
                                        )
                                    }
                                    .map(move |msg| message::DefinePath::Key(j, msg))
                                })
                                .collect(),
                            false,
                        )
                        .map(move |msg| {
                            Message::DefineDescriptor(message::DefineDescriptor::Path(i + 1, msg))
                        }),
                    )
                },
            ))
            .push(
                Row::new()
                    .push(
                        button::secondary(Some(icon::plus_icon()), "Add recovery option")
                            .width(Length::Fixed(210.0))
                            .on_press(Message::DefineDescriptor(
                                message::DefineDescriptor::AddRecoveryPath,
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
