use coincube_ui::locale::copy as localized_copy;
use coincube_ui::localized_format;
use iced::{Alignment, Length};

use coincube_ui::{
    color,
    component::{
        button, collapse,
        text::{text, Text},
    },
    icon, theme,
    widget::*,
};

use crate::installer::{
    descriptor::Path,
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

pub fn custom_template_description(
    progress: (usize, usize),
    ui_instance: u64,
    show_keychain_tip: bool,
) -> Element<'static, Message> {
    super::template_introduction(
        progress,
        ui_instance,
        show_keychain_tip,
        "Build your own",
        "Define your primary and recovery spending policies.",
        "Your primary policy can always spend. Each recovery option becomes available after its configured inactivity delay.",
        "Choose separate signing devices where practical. You can define more recovery options in the next step.",
        vec![
            PolicyRow {
                label: "Primary spending policy",
                keys: vec![(1, color::GREEN)],
                box_label: "Primary Keys",
                timelock: Timelock::None,
            },
            PolicyRow {
                label: "Recovery spending policy",
                keys: vec![(2, color::ORANGE)],
                box_label: "Recovery Keys",
                timelock: Timelock::After(0.5),
            },
        ],
    )
}

#[allow(clippy::too_many_arguments)]
pub fn custom_template<'a>(
    progress: (usize, usize),
    use_taproot: bool,
    allow_taproot: bool,
    primary_path: &'a Path,
    recovery_paths: &mut dyn Iterator<Item = (usize, &'a Path)>,
    safety_net_path: Option<(usize, &'a Path)>,
    num_non_primary_paths: usize,
    valid: bool,
) -> Element<'a, Message> {
    let prim_keys_fixed = primary_path.keys.len() < 2; // can only delete a primary key if there are 2 or more
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
            .push(custom_primary_path_section(
                use_taproot,
                primary_path,
                prim_keys_fixed,
            ))
            .push(recovery_paths.into_iter().enumerate().fold(
                Column::new().spacing(20),
                |col, (i, (p_idx, p))| {
                    col.push(custom_recovery_path_section(
                        use_taproot,
                        num_non_primary_paths,
                        i,
                        p_idx,
                        p,
                    ))
                },
            ))
            .push(
                Row::new()
                    .push(
                        button::secondary(
                            Some(icon::plus_icon()),
                            localized_copy("Add recovery option"),
                        )
                        .width(Length::Fixed(210.0))
                        .on_press(Message::DefineDescriptor(
                            message::DefineDescriptor::AddRecoveryPath,
                        )),
                    )
                    .spacing(10),
            )
            .push(safety_net_path.map(|(sn_index, sn_path)| {
                custom_safety_net_path_section(
                    use_taproot,
                    num_non_primary_paths,
                    sn_index,
                    sn_path,
                )
            }))
            .spacing(20),
        true,
        Some(Message::Previous),
        super::template_actions(valid, false),
    )
}

fn custom_primary_path_section(
    use_taproot: bool,
    primary_path: &Path,
    prim_keys_fixed: bool,
) -> Element<'_, Message> {
    path(
        color::GREEN,
        Some("Primary spending option:".to_string()),
        primary_path.sequence,
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
                        "Primary key".to_string(),
                        if use_taproot && !key.source.is_compatible_taproot() {
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
    .map(|msg| Message::DefineDescriptor(message::DefineDescriptor::Path(0, msg)))
}

fn custom_recovery_path_section(
    use_taproot: bool,
    num_non_primary_paths: usize,
    i: usize,
    p_idx: usize,
    p: &Path,
) -> Element<'_, Message> {
    path(
        color::ORANGE,
        Some(localized_format!(
            "Recovery option #{}:",
            "Opción de recuperación #{}:",
            i + 1
        )),
        p.sequence,
        p.warning,
        p.threshold,
        p.keys
            .iter()
            .enumerate()
            .map(|(j, recovery_key)| {
                // We cannot delete a key if doing so would remove all recovery paths,
                // i.e. if there is only 1 recovery path and it contains only 1 key,
                // and there is no safety net path.
                let fixed = num_non_primary_paths < 2 && p.keys.len() < 2;
                if let Some(key) = recovery_key {
                    defined_key(
                        &key.name,
                        color::ORANGE,
                        "Recovery key".to_string(),
                        if use_taproot && !key.source.is_compatible_taproot() {
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
        Message::DefineDescriptor(message::DefineDescriptor::Path(
            p_idx + 1, // add one to index to account for primary path.
            msg,
        ))
    })
}

fn custom_safety_net_path_section(
    use_taproot: bool,
    num_non_primary_paths: usize,
    sn_index: usize,
    sn_path: &Path,
) -> Element<'_, Message> {
    path(
        color::WHITE,
        Some("Safety Net:".to_string()),
        sn_path.sequence,
        sn_path.warning,
        sn_path.threshold,
        sn_path
            .keys
            .iter()
            .enumerate()
            .map(|(i, sn_key)| {
                // Cannot delete safety net key if doing so would remove the safety net path
                // and there are no other recovery paths.
                let fixed = num_non_primary_paths < 2 && sn_path.keys.len() < 2;
                if let Some(key) = sn_key {
                    defined_key(
                        &key.name,
                        color::WHITE,
                        "Safety Net key".to_string(),
                        if use_taproot && !key.source.is_compatible_taproot() {
                            Some("This key source does not support Taproot")
                        } else {
                            None
                        },
                        fixed,
                    )
                } else {
                    undefined_key(
                        color::WHITE,
                        "Safety Net key",
                        !sn_path.keys[0..i].iter().any(|k| k.is_none()),
                        fixed,
                    )
                }
                .map(move |msg| message::DefinePath::Key(i, msg))
            })
            .collect(),
        false,
    )
    .map(move |msg| {
        // Add 1 to index to account for primary path.
        Message::DefineDescriptor(message::DefineDescriptor::Path(sn_index + 1, msg))
    })
}
