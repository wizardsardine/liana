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
    descriptor::{Key, Path, PathKind, PathSequence},
    message::{self, Message},
    view::{
        editor::{
            define_descriptor_advanced_settings, defined_key, path,
            template::diagram::{PolicyRow, Timelock},
            undefined_key, uneditable_defined_key,
        },
        layout_with_actions,
    },
};

/// The "Second recovery option" path section (orange, single backup key)
/// shared verbatim by the two inheritance + backup-recovery "Set keys"
/// editors (`multisig_inheritance_recovery_template` and
/// `expanding_multisig_inheritance_template`).
fn second_recovery_path_section(use_taproot: bool, recovery_path: &Path) -> Element<'_, Message> {
    path(
        color::ORANGE,
        Some("Second recovery option:".to_string()),
        recovery_path.sequence,
        recovery_path.warning,
        recovery_path.threshold,
        recovery_path
            .keys
            .iter()
            .enumerate()
            .map(|(i, rec_key)| {
                if let Some(key) = rec_key {
                    defined_key(
                        &key.name,
                        color::ORANGE,
                        "Recovery key".to_string(),
                        if use_taproot && !key.source.is_compatible_taproot() {
                            Some("This device does not support Taproot")
                        } else {
                            None
                        },
                        true,
                    )
                } else {
                    undefined_key(
                        color::ORANGE,
                        "Recovery key",
                        !recovery_path.keys[0..i].iter().any(|k| k.is_none()),
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
                vec![(2, i)],
            ))
        } else {
            Message::DefineDescriptor(message::DefineDescriptor::Path(2, msg))
        }
    })
}

pub fn multisig_security_template_description(
    progress: (usize, usize),
    ui_instance: u64,
    show_keychain_tip: bool,
) -> Element<'static, Message> {
    super::template_introduction(
        progress,
        ui_instance,
        show_keychain_tip,
        "Expanding multisig wallet",
        "You need 3 keys: two Primary Keys and a Recovery Key.",
        "Both Primary Keys are needed to spend now. After the configured inactivity delay, any two of the three keys can recover your funds.",
        "Choose a separate signing device for the recovery key where practical.",
        vec![
            PolicyRow {
                label: "Primary spending policy",
                keys: vec![(1, color::GREEN), (2, color::GREEN)],
                box_label: "2-of-2 multisig",
                timelock: Timelock::None,
            },
            PolicyRow {
                label: "Recovery spending policy",
                keys: vec![(1, color::GREEN), (2, color::GREEN), (3, color::ORANGE)],
                box_label: "2-of-3 multisig",
                timelock: Timelock::After(0.5),
            },
        ],
    )
}

pub fn multisig_inheritance_recovery_description(
    progress: (usize, usize),
    ui_instance: u64,
    show_keychain_tip: bool,
) -> Element<'static, Message> {
    super::template_introduction(
        progress,
        ui_instance,
        show_keychain_tip,
        "Multisig with inheritance & backup recovery",
        "You need 7 keys: three Primary, three Inheritance and one Recovery Key.",
        "Any two Primary Keys can spend now. After inactivity, any two Inheritance Keys can recover. The Recovery Key becomes available after a longer delay.",
        "Choose separate signing devices for the inheritance keys where practical.",
        vec![
            PolicyRow {
                label: "Primary spending policy",
                keys: vec![(1, color::GREEN), (2, color::GREEN), (3, color::GREEN)],
                box_label: "2-of-3 multisig",
                timelock: Timelock::None,
            },
            PolicyRow {
                label: "Inheritance recovery policy",
                keys: vec![(4, color::BLUE), (5, color::BLUE), (6, color::BLUE)],
                box_label: "2-of-3 multisig",
                timelock: Timelock::After(0.42),
            },
            PolicyRow {
                label: "Second recovery policy",
                keys: vec![(7, color::ORANGE)],
                box_label: "Recovery Key",
                timelock: Timelock::After(0.7),
            },
        ],
    )
}

pub fn multisig_security_template<'a>(
    progress: (usize, usize),
    use_taproot: bool,
    allow_taproot: bool,
    primary_path: &'a Path,
    recovery_path: &'a Path,
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
                    None,
                    PathSequence::Primary,
                    primary_path.warning,
                    primary_path.keys.len(),
                    numbered_primary_keys(use_taproot, primary_path),
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
                }),
            )
            .push(security_recovery_path_section(
                use_taproot,
                primary_path,
                recovery_path,
            ))
            .spacing(20),
        true,
        Some(Message::Previous),
        super::template_actions(valid, true),
    )
}

pub fn expanding_multisig_inheritance_recovery_description(
    progress: (usize, usize),
    ui_instance: u64,
    show_keychain_tip: bool,
) -> Element<'static, Message> {
    super::template_introduction(
        progress,
        ui_instance,
        show_keychain_tip,
        "Expanding multisig with inheritance & backup recovery",
        "You need 7 keys. The Primary Keys are reused in the inheritance path.",
        "Any two Primary Keys can spend now. After inactivity, any two of the six Primary and Inheritance Keys can recover. The Recovery Key opens after a longer delay.",
        "Choose separate signing devices for the inheritance keys where practical.",
        vec![
            PolicyRow {
                label: "Primary spending policy",
                keys: vec![(1, color::GREEN), (2, color::GREEN), (3, color::GREEN)],
                box_label: "2-of-3 multisig",
                timelock: Timelock::None,
            },
            PolicyRow {
                label: "Inheritance recovery policy",
                keys: vec![
                    (1, color::GREEN),
                    (2, color::GREEN),
                    (3, color::GREEN),
                    (4, color::BLUE),
                    (5, color::BLUE),
                    (6, color::BLUE),
                ],
                box_label: "2-of-6 multisig",
                timelock: Timelock::After(0.42),
            },
            PolicyRow {
                label: "Second recovery policy",
                keys: vec![(7, color::ORANGE)],
                box_label: "Recovery Key",
                timelock: Timelock::After(0.7),
            },
        ],
    )
}

/// "Set keys" editor for the Multisig + inheritance + backup recovery
/// template: a 2-of-3 primary policy (green), a 2-of-3 inheritance policy
/// (white, independent keys — no reuse), and a single backup key (orange).
pub fn multisig_inheritance_recovery_template<'a>(
    progress: (usize, usize),
    use_taproot: bool,
    allow_taproot: bool,
    primary_path: &'a Path,
    inheritance_path: &'a Path,
    recovery_path: &'a Path,
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
                    numbered_primary_keys(use_taproot, primary_path),
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
            .push(
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
                        .map(|(j, inh_key)| {
                            if let Some(key) = inh_key {
                                defined_key(
                                    &key.name,
                                    color::BLUE,
                                    localized_format!(
                                        "Inheritance key #{}",
                                        "Clave de herencia #{}",
                                        j + 1
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
                                    color::BLUE,
                                    localized_format!(
                                        "Inheritance key #{}",
                                        "Clave de herencia #{}",
                                        j + 1
                                    ),
                                    !(primary_path.keys.iter().any(|k| k.is_none())
                                        || inheritance_path.keys[0..j].iter().any(|k| k.is_none())),
                                    true,
                                )
                            }
                            .map(move |msg| message::DefinePath::Key(j, msg))
                        })
                        .collect(),
                    true,
                )
                .map(move |msg| {
                    if let message::DefinePath::Key(j, message::DefineKey::Edit) = msg {
                        Message::DefineDescriptor(message::DefineDescriptor::KeysEdit(
                            PathKind::Recovery,
                            vec![(1, j)],
                        ))
                    } else {
                        Message::DefineDescriptor(message::DefineDescriptor::Path(1, msg))
                    }
                }),
            )
            .push(second_recovery_path_section(use_taproot, recovery_path))
            .spacing(20),
        true,
        Some(Message::Previous),
        super::template_actions(valid, true),
    )
}

/// Bespoke "Set keys" editor for the Expanding multisig + inheritance +
/// backup recovery template. The three primary keys are mirrored into the
/// first three slots of the 2-of-6 inheritance path (editing a primary key
/// fills both), then three inheritance keys and a single backup key.
pub fn expanding_multisig_inheritance_template<'a>(
    progress: (usize, usize),
    use_taproot: bool,
    allow_taproot: bool,
    primary_path: &'a Path,
    inheritance_path: &'a Path,
    recovery_path: &'a Path,
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
                    numbered_primary_keys(use_taproot, primary_path),
                    true,
                )
                .map(move |msg| {
                    if let message::DefinePath::Key(i, message::DefineKey::Edit) = msg {
                        // Mirror the primary key into the same slot of the
                        // 2-of-6 inheritance path.
                        Message::DefineDescriptor(message::DefineDescriptor::KeysEdit(
                            PathKind::Primary,
                            vec![(0, i), (1, i)],
                        ))
                    } else {
                        Message::DefineDescriptor(message::DefineDescriptor::Path(0, msg))
                    }
                }),
            )
            .push(expanding_inheritance_path_section(
                use_taproot,
                primary_path,
                inheritance_path,
            ))
            .push(second_recovery_path_section(use_taproot, recovery_path))
            .spacing(20),
        true,
        Some(Message::Previous),
        super::template_actions(valid, true),
    )
}

fn numbered_primary_keys(
    use_taproot: bool,
    primary_path: &Path,
) -> Vec<Element<'_, message::DefinePath>> {
    primary_path
        .keys
        .iter()
        .enumerate()
        .map(|(i, primary_key)| {
            if let Some(key) = primary_key {
                defined_key(
                    &key.name,
                    color::GREEN,
                    localized_format!("Primary key #{}", "Clave principal #{}", i + 1),
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
                    localized_format!("Primary key #{}", "Clave principal #{}", i + 1),
                    !primary_path.keys[0..i].iter().any(|k| k.is_none()),
                    true,
                )
            }
            .map(move |msg| message::DefinePath::Key(i, msg))
        })
        .collect()
}

fn security_recovery_path_section<'a>(
    use_taproot: bool,
    primary_path: &'a Path,
    recovery_path: &'a Path,
) -> Element<'a, Message> {
    path(
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
                security_recovery_key(
                    use_taproot,
                    primary_path,
                    recovery_path,
                    j,
                    recovery_key.as_ref(),
                )
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
    })
}

fn expanding_inheritance_path_section<'a>(
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
            .map(|(j, inh_key)| {
                expanding_inheritance_key(
                    use_taproot,
                    primary_path,
                    inheritance_path,
                    j,
                    inh_key.as_ref(),
                )
                .map(move |msg| message::DefinePath::Key(j, msg))
            })
            .collect(),
        true,
    )
    .map(move |msg| {
        if let message::DefinePath::Key(j, message::DefineKey::Edit) = msg {
            let (path_kind, keys) = if j < 3 {
                (PathKind::Primary, vec![(0, j), (1, j)])
            } else {
                (PathKind::Recovery, vec![(1, j)])
            };
            Message::DefineDescriptor(message::DefineDescriptor::KeysEdit(path_kind, keys))
        } else {
            Message::DefineDescriptor(message::DefineDescriptor::Path(1, msg))
        }
    })
}

fn security_recovery_key<'a>(
    use_taproot: bool,
    primary_path: &Path,
    recovery_path: &Path,
    j: usize,
    recovery_key: Option<&'a Key>,
) -> Element<'a, message::DefineKey> {
    if let Some(key) = recovery_key {
        if j < 2 {
            uneditable_defined_key(
                &key.name,
                color::GREEN,
                localized_format!("Primary key #{}", "Clave principal #{}", j + 1),
                taproot_device_warning(use_taproot, key),
            )
        } else {
            defined_key(
                &key.name,
                color::ORANGE,
                "Recovery key".to_string(),
                taproot_device_warning(use_taproot, key),
                true,
            )
        }
    } else {
        undefined_key(
            if j < 2 { color::GREEN } else { color::ORANGE },
            if j < 2 {
                localized_format!("Primary key #{}", "Clave principal #{}", j + 1)
            } else {
                "Recovery key".to_string()
            },
            !(primary_path.keys.iter().any(|k| k.is_none())
                || recovery_path.keys[0..j].iter().any(|k| k.is_none())),
            true,
        )
    }
}

fn expanding_inheritance_key<'a>(
    use_taproot: bool,
    primary_path: &Path,
    inheritance_path: &Path,
    j: usize,
    inh_key: Option<&'a Key>,
) -> Element<'a, message::DefineKey> {
    if let Some(key) = inh_key {
        if j < 3 {
            uneditable_defined_key(
                &key.name,
                color::GREEN,
                localized_format!("Primary key #{}", "Clave principal #{}", j + 1),
                taproot_device_warning(use_taproot, key),
            )
        } else {
            defined_key(
                &key.name,
                color::BLUE,
                localized_format!("Inheritance key #{}", "Clave de herencia #{}", j - 2),
                taproot_device_warning(use_taproot, key),
                true,
            )
        }
    } else {
        undefined_key(
            if j < 3 { color::GREEN } else { color::BLUE },
            if j < 3 {
                localized_format!("Primary key #{}", "Clave principal #{}", j + 1)
            } else {
                localized_format!("Inheritance key #{}", "Clave de herencia #{}", j - 2)
            },
            !(primary_path.keys.iter().any(|k| k.is_none())
                || inheritance_path.keys[0..j].iter().any(|k| k.is_none())),
            true,
        )
    }
}

fn taproot_device_warning(use_taproot: bool, key: &Key) -> Option<&'static str> {
    if use_taproot && !key.source.is_compatible_taproot() {
        Some("This device does not support Taproot")
    } else {
        None
    }
}
