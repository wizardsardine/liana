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
            defined_key,
            template::diagram::{policy_timeline, PolicyRow, Timelock},
            path, undefined_key,
        },
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
                .style(theme::text::secondary)
                .align_x(alignment::Horizontal::Left)
            ).align_x(alignment::Horizontal::Left).width(Length::Fill))
            .push(Row::new()
                .spacing(30)
                .push(
                    Row::new()
                    .align_y(Alignment::Center)
                    .spacing(10)
                    .push(icon::round_key_icon().size(H3_SIZE).style(theme::text::adaptive(color::GREEN)))
                    .push(p1_regular("Primary key").bold())
                ).push(
                    Row::new()
                        .align_y(Alignment::Center)
                        .spacing(10)
                        .push(icon::round_key_icon().size(H3_SIZE).style(theme::text::adaptive(color::BLUE)))
                        .push(p1_regular("Inheritance key").bold())
            ))
            .push(Container::new(
                p1_regular("You will always be able to spend using your Primary Key.
After a period of inactivity (but not before that) your Inheritance Key will become able to recover your funds.")
                .style(theme::text::secondary)
                .align_x(alignment::Horizontal::Left)
            ).align_x(alignment::Horizontal::Left).width(Length::Fill))
            .push(Container::new(
                p1_regular("Tip: instead of coordinating a hardware wallet for your heir, you can invite a family member or friend to contribute a Keychain key for the inheritance key — they accept an invitation and provide their key remotely, dramatically simplifying setup.")
                .style(theme::text::secondary)
                .align_x(alignment::Horizontal::Left)
            ).align_x(alignment::Horizontal::Left).width(Length::Fill))
            .push(policy_timeline(vec![
                PolicyRow {
                    label: "Primary spending policy:",
                    keys: vec![(1, color::GREEN)],
                    box_label: "Primary Key",
                    timelock: Timelock::None,
                },
                PolicyRow {
                    label: "Recovery spending policy:",
                    keys: vec![(2, color::BLUE)],
                    box_label: "Inheritance Key",
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

pub fn two_of_three_inheritance_description(
    progress: (usize, usize),
) -> Element<'static, Message> {
    layout(
        progress,
        None,
        "Introduction",
        Column::new()
            .align_x(Alignment::Start)
            .push(h3("2-of-3 multisig with inheritance"))
            .max_width(800.0)
            .push(Container::new(
                p1_regular("For this setup you will need 4 keys: three Primary Keys and an Inheritance Key. For security reasons, we suggest you use a separate Hardware Wallet for each key.")
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
                        .push(p1_regular("Inheritance key").bold())
            ))
            .push(Container::new(
                p1_regular("Any two of your three Primary Keys form a 2-of-3 multisig that can always spend. After a period of inactivity (but not before that) your Inheritance Key will become able to recover your funds.")
                .style(theme::text::secondary)
                .align_x(alignment::Horizontal::Left)
            ).align_x(alignment::Horizontal::Left).width(Length::Fill))
            .push(Container::new(
                p1_regular("Tip: instead of coordinating a hardware wallet for your heir, you can invite a family member or friend to contribute a Keychain key for the inheritance key — they accept an invitation and provide their key remotely, dramatically simplifying setup.")
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
                    label: "Recovery spending policy:",
                    keys: vec![(4, color::BLUE)],
                    box_label: "Inheritance Key",
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

/// "Set keys" editor for the 2-of-3 multisig + simple inheritance template:
/// a 2-of-3 primary policy (green) and a single inheritance key (white).
pub fn two_of_three_inheritance_template<'a>(
    progress: (usize, usize),
    use_taproot: bool,
    primary_path: &'a Path,
    inheritance_path: &'a Path,
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

pub fn inheritance_template<'a>(
    progress: (usize, usize),
    use_taproot: bool,
    primary_path: &'a Path,
    recovery_path: &'a Path,
    valid: bool,
) -> Element<'a, Message> {
    let primary_key = if let Some(first) = primary_path.keys.first() {
        first.as_ref()
    } else {
        None
    };
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
            .spacing(20),
        true,
        Some(Message::Previous),
    )
}
