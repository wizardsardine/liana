use iced::{alignment, widget::Space, Alignment, Length};

use liana_ui::{
    color,
    component::{
        button,
        text::{h3, p1_regular, Text, H3_SIZE},
    },
    icon, image, theme,
    widget::*,
};

use crate::installer::{
    descriptor::{Path, PathKind, PathSequence},
    message::{self, Message},
    view::{
        editor::{defined_key, path, undefined_key, uneditable_defined_key},
        layout,
    },
};
use crate::t;

pub fn multisig_security_template_description(
    progress: (usize, usize),
) -> Element<'static, Message> {
    layout(
        progress,
        None,
        t!("installer-introduction"),
        Column::new()
            .align_x(Alignment::Start)
            .push(h3(t!("installer-expanding-multisig-wallet")))
            .max_width(800.0)
            .push(
                Container::new(
                    p1_regular(t!("installer-multisig-description-1"))
                        .style(theme::text::secondary)
                        .align_x(alignment::Horizontal::Left),
                )
                .align_x(alignment::Horizontal::Left)
                .width(Length::Fill),
            )
            .push(
                Row::new()
                    .spacing(30)
                    .push(
                        Row::new()
                            .align_y(Alignment::Center)
                            .spacing(10)
                            .push(
                                icon::round_key_icon()
                                    .size(H3_SIZE)
                                    .style(theme::text::success),
                            )
                            .push(
                                p1_regular(t!("installer-primary-key-number", number = 1)).bold(),
                            ),
                    )
                    .push(
                        Row::new()
                            .align_y(Alignment::Center)
                            .spacing(10)
                            .push(
                                icon::round_key_icon()
                                    .size(H3_SIZE)
                                    .style(theme::text::success),
                            )
                            .push(
                                p1_regular(t!("installer-primary-key-number", number = 2)).bold(),
                            ),
                    )
                    .push(
                        Row::new()
                            .align_y(Alignment::Center)
                            .spacing(10)
                            .push(
                                icon::round_key_icon()
                                    .size(H3_SIZE)
                                    .style(theme::text::success),
                            )
                            .push(p1_regular(t!("installer-recovery-key")).bold()),
                    ),
            )
            .push(
                Container::new(
                    p1_regular(t!("installer-multisig-description-2"))
                        .style(theme::text::secondary)
                        .align_x(alignment::Horizontal::Left),
                )
                .align_x(alignment::Horizontal::Left)
                .width(Length::Fill),
            )
            .push(image::multisig_security_template_description().width(Length::Fill))
            .push(
                Row::new().push(Space::with_width(Length::Fill)).push(
                    button::primary(None, t!("common-next"))
                        .width(Length::Fixed(200.0))
                        .on_press(Message::Next),
                ),
            )
            .push(Space::with_height(50.0))
            .spacing(20),
        true,
        Some(Message::Previous),
    )
}

pub fn multisig_security_template<'a>(
    progress: (usize, usize),
    use_taproot: bool,
    primary_path: &'a Path,
    recovery_path: &'a Path,
    valid: bool,
    processing: bool,
) -> Element<'a, Message> {
    let advanced_settings = super::advanced_settings_collapse(use_taproot);

    let primary = path(
        color::GREEN,
        None,
        PathSequence::Primary,
        primary_path.warning,
        primary_path.keys.len(),
        primary_path
            .keys
            .iter()
            .enumerate()
            .map(|(i, primary_key)| {
                if let Some(key) = primary_key {
                    defined_key(
                        &key.name,
                        color::GREEN,
                        t!("installer-primary-key-number", number = i + 1),
                        if use_taproot && !key.source.is_compatible_taproot() {
                            Some(t!("installer-device-no-taproot"))
                        } else {
                            None
                        },
                        true,
                    )
                } else {
                    undefined_key(
                        color::GREEN,
                        t!("installer-primary-key-number", number = i + 1),
                        !primary_path.keys[0..i].iter().any(|k| k.is_none()),
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
            Message::DefineDescriptor(message::DefineDescriptor::KeysEdit(
                PathKind::Primary,
                vec![(0, i), (1, i)],
            ))
        } else {
            Message::DefineDescriptor(message::DefineDescriptor::Path(0, msg))
        }
    });

    let recovery = path(
        color::ORANGE,
        None,
        recovery_path.sequence,
        recovery_path.warning,
        recovery_path.threshold,
        recovery_path
            .keys
            .iter()
            .enumerate()
            .map(|(j, recovery_key)| {
                if let Some(key) = recovery_key {
                    if j < 2 {
                        uneditable_defined_key(
                            &key.name,
                            color::GREEN,
                            t!("installer-primary-key-number", number = j + 1),
                            if use_taproot && !key.source.is_compatible_taproot() {
                                Some(t!("installer-device-no-taproot"))
                            } else {
                                None
                            },
                        )
                    } else {
                        defined_key(
                            &key.name,
                            color::ORANGE,
                            t!("installer-recovery-key"),
                            if use_taproot && !key.source.is_compatible_taproot() {
                                Some(t!("installer-device-no-taproot"))
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
                            t!("installer-primary-key-number", number = j + 1)
                        } else {
                            t!("installer-recovery-key")
                        },
                        !(primary_path.keys.iter().any(|k| k.is_none())
                            || recovery_path.keys[0..j].iter().any(|k| k.is_none())),
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
            let (path_kind, keys) = if i < 2 {
                (PathKind::Primary, vec![(0, i), (1, i)])
            } else {
                // recovery path is the path with three keys
                (PathKind::Recovery, vec![(1, i)])
            };
            Message::DefineDescriptor(message::DefineDescriptor::KeysEdit(path_kind, keys))
        } else {
            Message::DefineDescriptor(message::DefineDescriptor::Path(1, msg))
        }
    });

    let footer = super::template_footer(valid, processing, true);

    layout(
        progress,
        None,
        t!("installer-set-keys"),
        Column::new()
            .align_x(Alignment::Start)
            .max_width(super::MAX_WIDTH)
            .push(advanced_settings)
            .push(primary)
            .push(recovery)
            .push(Space::with_height(10))
            .push(footer)
            .push(Space::with_height(super::BOTTOM_PADDING))
            .spacing(20),
        true,
        Some(Message::Previous),
    )
}
