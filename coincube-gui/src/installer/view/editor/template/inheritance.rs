use coincube_ui::locale::copy as localized_copy;
use coincube_ui::localized_format;
use iced::Alignment;

use coincube_ui::{
    color,
    component::{
        collapse,
        text::{text, Text},
    },
    icon, theme,
    widget::*,
};

use crate::installer::{
    descriptor::{Path, PathKind, PathSequence},
    message::{self, Message},
    view::{
        editor::{
            define_descriptor_advanced_settings, defined_key, path,
            template::diagram::{PolicyRow, Timelock},
            undefined_key,
        },
        layout_with_actions,
    },
};

pub fn inheritance_template_description(
    progress: (usize, usize),
    ui_instance: u64,
    show_keychain_tip: bool,
) -> Element<'static, Message> {
    super::template_introduction(
        progress,
        ui_instance,
        show_keychain_tip,
        "Simple inheritance wallet",
        "You need 2 keys: your Primary Key and an Inheritance Key for your heir.",
        "Your Primary Key can always spend. After the configured inactivity delay, the Inheritance Key can recover your funds.",
        "Choose a separate signing device for your heir's key where practical.",
        vec![
            PolicyRow {
                label: "Primary spending policy",
                keys: vec![(1, color::GREEN)],
                box_label: "Primary Key",
                timelock: Timelock::None,
            },
            PolicyRow {
                label: "Inheritance recovery policy",
                keys: vec![(2, color::BLUE)],
                box_label: "Inheritance Key",
                timelock: Timelock::After(0.5),
            },
        ],
    )
}

pub fn two_of_three_inheritance_description(
    progress: (usize, usize),
    ui_instance: u64,
    show_keychain_tip: bool,
) -> Element<'static, Message> {
    super::template_introduction(
        progress,
        ui_instance,
        show_keychain_tip,
        "2-of-3 multisig with inheritance",
        "You need 4 keys: three Primary Keys and an Inheritance Key.",
        "Any two Primary Keys can spend now. After the configured inactivity delay, the Inheritance Key can recover your funds.",
        "Choose a separate signing device for your heir's key where practical.",
        vec![
            PolicyRow {
                label: "Primary spending policy",
                keys: vec![(1, color::GREEN), (2, color::GREEN), (3, color::GREEN)],
                box_label: "2-of-3 multisig",
                timelock: Timelock::None,
            },
            PolicyRow {
                label: "Inheritance recovery policy",
                keys: vec![(4, color::BLUE)],
                box_label: "Inheritance Key",
                timelock: Timelock::After(0.5),
            },
        ],
    )
}

/// "Set keys" editor for the 2-of-3 multisig + simple inheritance template:
/// a 2-of-3 primary policy (green) and a single inheritance key (white).
pub fn two_of_three_inheritance_template<'a>(
    progress: (usize, usize),
    use_taproot: bool,
    allow_taproot: bool,
    primary_path: &'a Path,
    inheritance_path: &'a Path,
    valid: bool,
) -> Element<'a, Message> {
    layout_with_actions(
        progress,
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
                            .push(text(localized_copy("Advanced settings")).small().bold())
                            .push(icon::collapse_icon()),
                    )
                    .style(theme::button::transparent)
                },
                || {
                    Button::new(
                        Row::new()
                            .align_y(Alignment::Center)
                            .spacing(10)
                            .push(text(localized_copy("Advanced settings")).small().bold())
                            .push(icon::collapsed_icon()),
                    )
                    .style(theme::button::transparent)
                },
                move || define_descriptor_advanced_settings(use_taproot, allow_taproot),
            ))
            .push(
                path(
                    color::GREEN,
                    Some("Primary spending option:".to_string()),
                    PathSequence::Primary,
                    primary_path.warning,
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
                                    localized_format!(
                                        "Primary key #{}",
                                        "Clave principal #{}",
                                        i + 1
                                    ),
                                    if use_taproot && !key.source.is_compatible_taproot() {
                                        Some("This device does not support Taproot")
                                    } else {
                                        None
                                    },
                                    true,
                                )
                            } else {
                                undefined_key(
                                    color::GREEN,
                                    localized_format!(
                                        "Primary key #{}",
                                        "Clave principal #{}",
                                        i + 1
                                    ),
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
                            vec![(0, i)],
                        ))
                    } else {
                        Message::DefineDescriptor(message::DefineDescriptor::Path(0, msg))
                    }
                }),
            )
            .push(two_of_three_inheritance_path_section(
                use_taproot,
                primary_path,
                inheritance_path,
            ))
            .spacing(20),
        true,
        Some(Message::Previous),
        super::template_actions(valid, true),
    )
}

pub fn inheritance_template<'a>(
    progress: (usize, usize),
    use_taproot: bool,
    allow_taproot: bool,
    primary_path: &'a Path,
    recovery_path: &'a Path,
    valid: bool,
) -> Element<'a, Message> {
    let primary_key = if let Some(first) = primary_path.keys.first() {
        first.as_ref()
    } else {
        None
    };
    layout_with_actions(
        progress,
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
                            .push(text(localized_copy("Advanced settings")).small().bold())
                            .push(icon::collapse_icon()),
                    )
                    .style(theme::button::transparent)
                },
                || {
                    Button::new(
                        Row::new()
                            .align_y(Alignment::Center)
                            .spacing(10)
                            .push(text(localized_copy("Advanced settings")).small().bold())
                            .push(icon::collapsed_icon()),
                    )
                    .style(theme::button::transparent)
                },
                move || define_descriptor_advanced_settings(use_taproot, allow_taproot),
            ))
            .push(
                path(
                    color::GREEN,
                    None,
                    PathSequence::Primary,
                    primary_path.warning,
                    1,
                    vec![if let Some(key) = primary_key {
                        defined_key(
                            &key.name,
                            color::GREEN,
                            "Primary key".to_string(),
                            if use_taproot && !key.source.is_compatible_taproot() {
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
                    color::BLUE,
                    None,
                    recovery_path.sequence,
                    recovery_path.warning,
                    1,
                    vec![if let Some(Some(key)) = recovery_path.keys.first() {
                        defined_key(
                            &key.name,
                            color::BLUE,
                            "Inheritance key".to_string(),
                            if use_taproot && !key.source.is_compatible_taproot() {
                                Some("This device does not support Taproot")
                            } else {
                                None
                            },
                            true,
                        )
                    } else {
                        undefined_key(color::BLUE, "Inheritance key", primary_key.is_some(), true)
                    }
                    .map(|msg| message::DefinePath::Key(0, msg))],
                    true,
                )
                .map(|msg| Message::DefineDescriptor(message::DefineDescriptor::Path(1, msg))),
            )
            .spacing(20),
        true,
        Some(Message::Previous),
        super::template_actions(valid, true),
    )
}

fn two_of_three_inheritance_path_section<'a>(
    use_taproot: bool,
    primary_path: &'a Path,
    inheritance_path: &'a Path,
) -> Element<'a, Message> {
    path(
        color::BLUE,
        Some("Inheritance recovery option:".to_string()),
        inheritance_path.sequence,
        inheritance_path.warning,
        inheritance_path.threshold,
        inheritance_path
            .keys
            .iter()
            .enumerate()
            .map(|(i, inh_key)| {
                if let Some(key) = inh_key {
                    defined_key(
                        &key.name,
                        color::BLUE,
                        "Inheritance key".to_string(),
                        if use_taproot && !key.source.is_compatible_taproot() {
                            Some("This device does not support Taproot")
                        } else {
                            None
                        },
                        true,
                    )
                } else {
                    undefined_key(
                        color::BLUE,
                        "Inheritance key",
                        !(primary_path.keys.iter().any(|k| k.is_none())
                            || inheritance_path.keys[0..i].iter().any(|k| k.is_none())),
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
                PathKind::Recovery,
                vec![(1, i)],
            ))
        } else {
            Message::DefineDescriptor(message::DefineDescriptor::Path(1, msg))
        }
    })
}
