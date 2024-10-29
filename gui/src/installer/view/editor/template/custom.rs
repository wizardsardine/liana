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
    prompt,
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
            .align_items(Alignment::Start)
            .push(h3("Custom wallet"))
            .max_width(800.0)
            .push(Container::new(
                p1_regular("Through this setup you can choose how many keys you want to use. For security reasons, we suggest you use Hardware Wallets to store them.")
                .style(color::GREY_2)
                .horizontal_alignment(alignment::Horizontal::Left)
            ).align_x(alignment::Horizontal::Left).width(Length::Fill))
            .push(Container::new(
                p1_regular("For this Custom wallet you will need to define your Primary and Recovery Sets of Keys.")
                .style(color::GREY_2)
                .horizontal_alignment(alignment::Horizontal::Left)
            ).align_x(alignment::Horizontal::Left).width(Length::Fill))
            .push(image::custom_template_description().width(Length::Fill))
            .push(Container::new(
                p1_regular("The Primary set of Keys will always be able to spend. Your Recovery set(s) of Keys will activate only after a defined time of wallet inactivity, allowing for secure recovery and advanced spending policies. You can define more than one set of Recovery Keys activating at different times.")
                .style(color::GREY_2)
                .horizontal_alignment(alignment::Horizontal::Left)
            ).align_x(alignment::Horizontal::Left).width(Length::Fill))
            .push(Row::new().push(Space::with_width(Length::Fill)).push(button::primary(None, "Select").width(Length::Fixed(200.0)).on_press(Message::Next)))
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
            .push(p1_regular(prompt::DEFINE_DESCRIPTOR_PRIMARY_PATH_TOOLTIP).style(color::GREY_2))
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
                                        Some("Key is not compatible with taproot")
                                    } else {
                                        None
                                    },
                                    i == 0,
                                )
                            } else {
                                undefined_key(
                                    color::GREEN,
                                    "Primary key",
                                    !primary_path.keys[0..i].iter().any(|k| k.is_none()),
                                    i == 0,
                                )
                            }
                            .map(move |msg| message::DefinePath::Key(i, msg))
                        })
                        .collect(),
                    false,
                )
                .map(|msg| Message::DefineDescriptor(message::DefineDescriptor::Path(0, msg))),
            )
            .push(p1_regular(prompt::DEFINE_DESCRIPTOR_RECOVERY_PATH_TOOLTIP).style(color::GREY_2))
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
                                    if let Some(key) = recovery_key {
                                        defined_key(
                                            &key.name,
                                            color::ORANGE,
                                            "Recovery key",
                                            if use_taproot && !key.is_compatible_taproot {
                                                Some("Key is not compatible with Taproot")
                                            } else {
                                                None
                                            },
                                            false,
                                        )
                                    } else {
                                        undefined_key(
                                            color::ORANGE,
                                            "Recovery key",
                                            !p.keys[0..j].iter().any(|k| k.is_none()),
                                            false,
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
                            .width(Length::Fixed(200.0))
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
