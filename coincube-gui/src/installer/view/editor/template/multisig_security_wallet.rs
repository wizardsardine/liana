use iced::{alignment, widget::Space, Alignment, Length};

use coincube_ui::{
    color,
    component::{
        button,
        text::{h3, p1_regular, Text, H3_SIZE},
    },
    icon, theme,
    widget::*,
};

use crate::installer::{
    context,
    descriptor::{Path, PathKind, PathSequence},
    message::{self, Message},
    view::{
        editor::{
            defined_key, path,
            template::diagram::{policy_timeline, PolicyRow, Timelock},
            undefined_key, uneditable_defined_key,
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
            .align_x(Alignment::Start)
            .push(h3("Expanding multisig wallet"))
            .max_width(800.0)
            .push(Container::new(
                p1_regular("For this setup you will need 3 keys: two Primary Keys and a Recovery Key. For security reasons, we suggest you use a separate Hardware Wallet for each key.")
                .style(theme::text::secondary)
                .align_x(alignment::Horizontal::Left)
            ).align_x(alignment::Horizontal::Left).width(Length::Fill))
            .push(Row::new()
                .spacing(30)
                .push(
                    Row::new()
                    .align_y(Alignment::Center)
                    .spacing(10)
                    .push(icon::round_key_icon().size(H3_SIZE).style(theme::text::success))
                    .push(p1_regular("Primary key #1").bold())
                ).push(
                    Row::new()
                    .align_y(Alignment::Center)
                    .spacing(10)
                    .push(icon::round_key_icon().size(H3_SIZE).style(theme::text::success))
                    .push(p1_regular("Primary key #2").bold())
                ).push(
                    Row::new()
                        .align_y(Alignment::Center)
                        .spacing(10)
                        .push(icon::round_key_icon().size(H3_SIZE).style(theme::text::adaptive(color::ORANGE)))
                        .push(p1_regular("Recovery key").bold())
            ))
            .push(Container::new(
                p1_regular("The Primary Keys will compose a 2-of-2 multisig which will always be able to spend. In case one of your keys becomes unavailable, after a period of inactivity you will be able to recover your funds using the Recovery Key together with one of your Primary Keys (2-of-3 multisig):")
                .style(theme::text::secondary)
                .align_x(alignment::Horizontal::Left)
            ).align_x(alignment::Horizontal::Left).width(Length::Fill))
            .push(Container::new(
                p1_regular("Tip: you don't need to gather every hardware wallet in one place. You can invite a family member or friend to contribute a Keychain key for the Recovery Key — they accept an invitation and provide their key remotely, dramatically simplifying setup.")
                .style(theme::text::secondary)
                .align_x(alignment::Horizontal::Left)
            ).align_x(alignment::Horizontal::Left).width(Length::Fill))
            .push(policy_timeline(vec![
                PolicyRow {
                    label: "Primary spending policy:",
                    keys: vec![(1, color::GREEN), (2, color::GREEN)],
                    box_label: "2-of-2 multisig",
                    timelock: Timelock::None,
                },
                PolicyRow {
                    label: "Recovery spending policy:",
                    keys: vec![(1, color::GREEN), (2, color::GREEN), (3, color::ORANGE)],
                    box_label: "2-of-3 multisig",
                    timelock: Timelock::After(0.5),
                },
            ]))
            .push(Row::new().push(Space::new().width(Length::Fill)).push(button::primary(None, "Next").width(Length::Fixed(200.0)).on_press(Message::Next)))
            .push(Space::new().height(50.0))
            .spacing(20),
        true,
        Some(Message::Previous),
    )
}

pub fn multisig_inheritance_recovery_description(
    progress: (usize, usize),
) -> Element<'static, Message> {
    layout(
        progress,
        None,
        "Introduction",
        Column::new()
            .align_x(Alignment::Start)
            .push(h3("Multisig with inheritance & backup recovery"))
            .max_width(800.0)
            .push(Container::new(
                p1_regular("For this setup you will need 7 keys: three Primary Keys, three Inheritance Keys and a second Recovery Key. For security reasons, we suggest you use a separate Hardware Wallet for each key.")
                .style(theme::text::secondary)
                .align_x(alignment::Horizontal::Left)
            ).align_x(alignment::Horizontal::Left).width(Length::Fill))
            .push(Row::new()
                .spacing(30)
                .push(
                    Row::new()
                    .align_y(Alignment::Center)
                    .spacing(10)
                    .push(icon::round_key_icon().size(H3_SIZE).style(theme::text::success))
                    .push(p1_regular("Primary keys #1-3").bold())
                ).push(
                    Row::new()
                    .align_y(Alignment::Center)
                    .spacing(10)
                    .push(icon::round_key_icon().size(H3_SIZE).style(theme::text::adaptive(color::BLUE)))
                    .push(p1_regular("Inheritance keys #4-6").bold())
                ).push(
                    Row::new()
                        .align_y(Alignment::Center)
                        .spacing(10)
                        .push(icon::round_key_icon().size(H3_SIZE).style(theme::text::adaptive(color::ORANGE)))
                        .push(p1_regular("Recovery key #7").bold())
            ))
            .push(Container::new(
                p1_regular("Any two of your three Primary Keys form a 2-of-3 multisig that can always spend. After a period of inactivity, any two of your three Inheritance Keys can recover the funds, and a second Recovery Key becomes available as an additional backup after a longer period. Both recovery timelocks can be configured in the next step.")
                .style(theme::text::secondary)
                .align_x(alignment::Horizontal::Left)
            ).align_x(alignment::Horizontal::Left).width(Length::Fill))
            .push(Container::new(
                p1_regular("Tip: you don't need to gather every hardware wallet in one place. You can invite family or friends to contribute Keychain keys for the inheritance keys — they accept an invitation and provide their key remotely, dramatically simplifying setup.")
                .style(theme::text::secondary)
                .align_x(alignment::Horizontal::Left)
            ).align_x(alignment::Horizontal::Left).width(Length::Fill))
            .push(policy_timeline(vec![
                PolicyRow {
                    label: "Primary spending policy:",
                    keys: vec![(1, color::GREEN), (2, color::GREEN), (3, color::GREEN)],
                    box_label: "2-of-3 multisig",
                    timelock: Timelock::None,
                },
                PolicyRow {
                    label: "Inheritance spending policy:",
                    keys: vec![(4, color::BLUE), (5, color::BLUE), (6, color::BLUE)],
                    box_label: "2-of-3 multisig",
                    timelock: Timelock::After(0.5),
                },
                PolicyRow {
                    label: "Second recovery policy:",
                    keys: vec![(7, color::ORANGE)],
                    box_label: "Recovery Key",
                    // Longer timelock than the inheritance path: the spend
                    // window opens only after the inheritance path is active.
                    timelock: Timelock::After(0.68),
                },
            ]))
            .push(Row::new().push(Space::new().width(Length::Fill)).push(button::primary(None, "Next").width(Length::Fixed(200.0)).on_press(Message::Next)))
            .push(Space::new().height(50.0))
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
) -> Element<'a, Message> {
    layout(
        progress,
        None,
        "Set keys",
        Column::new()
            .align_x(Alignment::Start)
            .max_width(1000.0)
            .push(
                path(
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
                                    format!("Primary key #{}", i + 1),
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
                                    format!("Primary key #{}", i + 1),
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
                }),
            )
            .push(
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
                            if let Some(key) = recovery_key {
                                if j < 2 {
                                    uneditable_defined_key(
                                        &key.name,
                                        color::GREEN,
                                        format!("Primary key #{}", j + 1),
                                        if use_taproot && !key.source.is_compatible_taproot() {
                                            Some("This device does not support Taproot")
                                        } else {
                                            None
                                        },
                                    )
                                } else {
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
                                }
                            } else {
                                undefined_key(
                                    if j < 2 { color::GREEN } else { color::ORANGE },
                                    if j < 2 {
                                        format!("Primary key #{}", j + 1)
                                    } else {
                                        "Recovery key".to_string()
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
                        Message::DefineDescriptor(message::DefineDescriptor::KeysEdit(
                            path_kind, keys,
                        ))
                    } else {
                        Message::DefineDescriptor(message::DefineDescriptor::Path(1, msg))
                    }
                }),
            )
            .push(Space::new().height(10))
            .push(
                Row::new()
                    .push(
                        button::secondary(None, "Clear All")
                            .width(Length::Fixed(120.0))
                            .on_press(Message::DefineDescriptor(message::DefineDescriptor::Reset)),
                    )
                    .push(Space::new().width(40))
                    .push(
                        button::secondary(None, "Customize")
                            .width(Length::Fixed(120.0))
                            .on_press(Message::DefineDescriptor(
                                message::DefineDescriptor::ChangeTemplate(
                                    context::DescriptorTemplate::Custom,
                                ),
                            )),
                    )
                    .push(Space::new().width(Length::Fill))
                    .push(
                        button::primary(None, "Continue")
                            .width(Length::Fixed(200.0))
                            .on_press_maybe(if valid { Some(Message::Next) } else { None }),
                    ),
            )
            .push(Space::new().height(100.0))
            .spacing(20),
        true,
        Some(Message::Previous),
    )
}

pub fn expanding_multisig_inheritance_recovery_description(
    progress: (usize, usize),
) -> Element<'static, Message> {
    layout(
        progress,
        None,
        "Introduction",
        Column::new()
            .align_x(Alignment::Start)
            .push(h3("Expanding multisig with inheritance & backup recovery"))
            .max_width(800.0)
            .push(Container::new(
                p1_regular("For this setup you will need 7 keys: three Primary Keys, three Inheritance Keys and a second Recovery Key. Your Primary Keys are reused in the inheritance path, so no extra devices are needed. For security reasons, we suggest you use a separate Hardware Wallet for each key.")
                .style(theme::text::secondary)
                .align_x(alignment::Horizontal::Left)
            ).align_x(alignment::Horizontal::Left).width(Length::Fill))
            .push(Row::new()
                .spacing(30)
                .push(
                    Row::new()
                    .align_y(Alignment::Center)
                    .spacing(10)
                    .push(icon::round_key_icon().size(H3_SIZE).style(theme::text::success))
                    .push(p1_regular("Primary keys #1-3").bold())
                ).push(
                    Row::new()
                    .align_y(Alignment::Center)
                    .spacing(10)
                    .push(icon::round_key_icon().size(H3_SIZE).style(theme::text::adaptive(color::BLUE)))
                    .push(p1_regular("Inheritance keys #4-6").bold())
                ).push(
                    Row::new()
                        .align_y(Alignment::Center)
                        .spacing(10)
                        .push(icon::round_key_icon().size(H3_SIZE).style(theme::text::adaptive(color::ORANGE)))
                        .push(p1_regular("Recovery key #7").bold())
            ))
            .push(Container::new(
                p1_regular("Any two of your three Primary Keys form a 2-of-3 multisig that can always spend. After a period of inactivity, any two of the six keys — your Primary Keys together with your Inheritance Keys — can recover the funds (2-of-6), so a single heir plus one of your own keys is enough. A second Recovery Key becomes available as a final backup after a longer period. Both recovery timelocks can be configured in the next step.")
                .style(theme::text::secondary)
                .align_x(alignment::Horizontal::Left)
            ).align_x(alignment::Horizontal::Left).width(Length::Fill))
            .push(Container::new(
                p1_regular("Tip: you don't need to gather every hardware wallet in one place. You can invite family or friends to contribute Keychain keys for the inheritance keys — they accept an invitation and provide their key remotely, dramatically simplifying setup.")
                .style(theme::text::secondary)
                .align_x(alignment::Horizontal::Left)
            ).align_x(alignment::Horizontal::Left).width(Length::Fill))
            .push(policy_timeline(vec![
                PolicyRow {
                    label: "Primary spending policy:",
                    keys: vec![(1, color::GREEN), (2, color::GREEN), (3, color::GREEN)],
                    box_label: "2-of-3 multisig",
                    timelock: Timelock::None,
                },
                PolicyRow {
                    label: "Inheritance spending policy:",
                    keys: vec![
                        (1, color::GREEN),
                        (2, color::GREEN),
                        (3, color::GREEN),
                        (4, color::BLUE),
                        (5, color::BLUE),
                        (6, color::BLUE),
                    ],
                    box_label: "2-of-6 multisig",
                    timelock: Timelock::After(0.5),
                },
                PolicyRow {
                    label: "Second recovery policy:",
                    keys: vec![(7, color::ORANGE)],
                    box_label: "Recovery Key",
                    // Longer timelock than the inheritance path: the spend
                    // window opens only after the inheritance path is active.
                    timelock: Timelock::After(0.68),
                },
            ]))
            .push(Row::new().push(Space::new().width(Length::Fill)).push(button::primary(None, "Next").width(Length::Fixed(200.0)).on_press(Message::Next)))
            .push(Space::new().height(50.0))
            .spacing(20),
        true,
        Some(Message::Previous),
    )
}

/// "Set keys" editor for the Multisig + inheritance + backup recovery
/// template: a 2-of-3 primary policy (green), a 2-of-3 inheritance policy
/// (white, independent keys — no reuse), and a single backup key (orange).
pub fn multisig_inheritance_recovery_template<'a>(
    progress: (usize, usize),
    use_taproot: bool,
    primary_path: &'a Path,
    inheritance_path: &'a Path,
    recovery_path: &'a Path,
    valid: bool,
) -> Element<'a, Message> {
    layout(
        progress,
        None,
        "Set keys",
        Column::new()
            .align_x(Alignment::Start)
            .max_width(1000.0)
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
                                    format!("Primary key #{}", i + 1),
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
                                    format!("Primary key #{}", i + 1),
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
                                    format!("Inheritance key #{}", j + 1),
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
                                    format!("Inheritance key #{}", j + 1),
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
            .push(
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
                }),
            )
            .push(Space::new().height(10))
            .push(
                Row::new()
                    .push(
                        button::secondary(None, "Clear All")
                            .width(Length::Fixed(120.0))
                            .on_press(Message::DefineDescriptor(message::DefineDescriptor::Reset)),
                    )
                    .push(Space::new().width(40))
                    .push(
                        button::secondary(None, "Customize")
                            .width(Length::Fixed(120.0))
                            .on_press(Message::DefineDescriptor(
                                message::DefineDescriptor::ChangeTemplate(
                                    context::DescriptorTemplate::Custom,
                                ),
                            )),
                    )
                    .push(Space::new().width(Length::Fill))
                    .push(
                        button::primary(None, "Continue")
                            .width(Length::Fixed(200.0))
                            .on_press_maybe(if valid { Some(Message::Next) } else { None }),
                    ),
            )
            .push(Space::new().height(100.0))
            .spacing(20),
        true,
        Some(Message::Previous),
    )
}

/// Bespoke "Set keys" editor for the Expanding multisig + inheritance +
/// backup recovery template. The three primary keys are mirrored into the
/// first three slots of the 2-of-6 inheritance path (editing a primary key
/// fills both), then three inheritance keys and a single backup key.
pub fn expanding_multisig_inheritance_template<'a>(
    progress: (usize, usize),
    use_taproot: bool,
    primary_path: &'a Path,
    inheritance_path: &'a Path,
    recovery_path: &'a Path,
    valid: bool,
) -> Element<'a, Message> {
    layout(
        progress,
        None,
        "Set keys",
        Column::new()
            .align_x(Alignment::Start)
            .max_width(1000.0)
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
                                    format!("Primary key #{}", i + 1),
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
                                    format!("Primary key #{}", i + 1),
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
                                if j < 3 {
                                    uneditable_defined_key(
                                        &key.name,
                                        color::GREEN,
                                        format!("Primary key #{}", j + 1),
                                        if use_taproot && !key.source.is_compatible_taproot() {
                                            Some("This device does not support Taproot")
                                        } else {
                                            None
                                        },
                                    )
                                } else {
                                    defined_key(
                                        &key.name,
                                        color::BLUE,
                                        format!("Inheritance key #{}", j - 2),
                                        if use_taproot && !key.source.is_compatible_taproot() {
                                            Some("This device does not support Taproot")
                                        } else {
                                            None
                                        },
                                        true,
                                    )
                                }
                            } else {
                                undefined_key(
                                    if j < 3 { color::GREEN } else { color::BLUE },
                                    if j < 3 {
                                        format!("Primary key #{}", j + 1)
                                    } else {
                                        format!("Inheritance key #{}", j - 2)
                                    },
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
                        let (path_kind, keys) = if j < 3 {
                            (PathKind::Primary, vec![(0, j), (1, j)])
                        } else {
                            (PathKind::Recovery, vec![(1, j)])
                        };
                        Message::DefineDescriptor(message::DefineDescriptor::KeysEdit(
                            path_kind, keys,
                        ))
                    } else {
                        Message::DefineDescriptor(message::DefineDescriptor::Path(1, msg))
                    }
                }),
            )
            .push(
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
                }),
            )
            .push(Space::new().height(10))
            .push(
                Row::new()
                    .push(
                        button::secondary(None, "Clear All")
                            .width(Length::Fixed(120.0))
                            .on_press(Message::DefineDescriptor(message::DefineDescriptor::Reset)),
                    )
                    .push(Space::new().width(40))
                    .push(
                        button::secondary(None, "Customize")
                            .width(Length::Fixed(120.0))
                            .on_press(Message::DefineDescriptor(
                                message::DefineDescriptor::ChangeTemplate(
                                    context::DescriptorTemplate::Custom,
                                ),
                            )),
                    )
                    .push(Space::new().width(Length::Fill))
                    .push(
                        button::primary(None, "Continue")
                            .width(Length::Fixed(200.0))
                            .on_press_maybe(if valid { Some(Message::Next) } else { None }),
                    ),
            )
            .push(Space::new().height(100.0))
            .spacing(20),
        true,
        Some(Message::Previous),
    )
}
