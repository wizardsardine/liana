use iced::{
    alignment,
    widget::{tooltip, Container, Space},
    Alignment, Length,
};

use liana_ui::{
    color,
    component::{
        button::{self},
        text::{h3, p1_regular, text},
    },
    icon, image, theme,
    widget::*,
};

use crate::installer::{
    descriptor::Path,
    message::{self, Message},
    view::{
        editor::{defined_key, path, undefined_key},
        layout,
    },
};

const SAFETY_NET_DESCRIPTION: &str = "This adds a final recovery option containing keys from professional key agents.\n\nUse this option if you have been provided one or more Safety Net tokens.";

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
                .style(theme::text::secondary)
                .align_x(alignment::Horizontal::Left)
            ).align_x(alignment::Horizontal::Left).width(Length::Fill))
            .push(Container::new(
                p1_regular("The keys belonging to your primary policy can always spend. Those belonging to the recovery policies will be able to spend only after a defined time of wallet inactivity, allowing for secure recovery and advanced spending policies.")
                .style(theme::text::secondary)
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

#[allow(clippy::too_many_arguments)]
pub fn custom_template<'a>(
    progress: (usize, usize),
    use_taproot: bool,
    primary_path: &'a Path,
    recovery_paths: &mut dyn Iterator<Item = (usize, &'a Path)>,
    safety_net_path: Option<(usize, &'a Path)>,
    num_non_primary_paths: usize,
    valid: bool,
    processing: bool,
) -> Element<'a, Message> {
    let prim_keys_fixed = primary_path.keys.len() < 2; // can only delete a primary key if there are 2 or more

    let advanced_settings = super::advanced_settings_collapse(use_taproot, !processing);

    let primary = path(
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
                        "Primary key",
                        if use_taproot && !key.source.is_compatible_taproot() {
                            Some("This device does not support Taproot")
                        } else {
                            None
                        },
                        prim_keys_fixed,
                        !processing,
                    )
                } else {
                    undefined_key(
                        color::GREEN,
                        "Primary key",
                        !primary_path.keys[0..i].iter().any(|k| k.is_none()),
                        prim_keys_fixed,
                        !processing,
                    )
                }
                .map(move |msg| message::DefinePath::Key(i, msg))
            })
            .collect(),
        false,
        !processing,
    )
    .map(|msg| Message::DefineDescriptor(message::DefineDescriptor::Path(0, msg)));

    let recovery_paths = recovery_paths.into_iter().enumerate().fold(
        Column::new().spacing(20),
        |col, (i, (p_idx, p))| {
            col.push(
                path(
                    color::ORANGE,
                    Some(format!("Recovery option #{}:", i + 1)),
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
                                    "Recovery key",
                                    if use_taproot && !key.source.is_compatible_taproot() {
                                        Some("This device does not support Taproot")
                                    } else {
                                        None
                                    },
                                    fixed,
                                    !processing,
                                )
                            } else {
                                undefined_key(
                                    color::ORANGE,
                                    "Recovery key",
                                    !p.keys[0..j].iter().any(|k| k.is_none()),
                                    fixed,
                                    !processing,
                                )
                            }
                            .map(move |msg| message::DefinePath::Key(j, msg))
                        })
                        .collect(),
                    false,
                    !processing,
                )
                .map(move |msg| {
                    Message::DefineDescriptor(message::DefineDescriptor::Path(
                        p_idx + 1, // add one to index to account for primary path.
                        msg,
                    ))
                }),
            )
        },
    );

    let add_recovery_msg = (!processing).then_some(Message::DefineDescriptor(
        message::DefineDescriptor::AddRecoveryPath,
    ));
    let add_safety_net_msg = (!processing).then_some(Message::DefineDescriptor(
        message::DefineDescriptor::AddSafetyNetPath,
    ));

    let btn_row = Row::new()
        .push(
            button::secondary(Some(icon::plus_icon()), "Add recovery option")
                .width(210)
                .on_press_maybe(add_recovery_msg),
        )
        .push_maybe(
            safety_net_path.is_none().then_some(tooltip::Tooltip::new(
                button::secondary(Some(icon::plus_icon()), "Add Safety Net")
                    .width(210)
                    .on_press_maybe(add_safety_net_msg),
                Container::new(text(SAFETY_NET_DESCRIPTION))
                    .style(theme::card::simple)
                    .padding(10),
                tooltip::Position::Bottom,
            )),
        )
        .spacing(10);

    let safety_net = safety_net_path.map(|(sn_index, sn_path)| {
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
                            "Safety Net key",
                            if use_taproot && !key.source.is_compatible_taproot() {
                                Some("This key source does not support Taproot")
                            } else {
                                None
                            },
                            fixed,
                            !processing,
                        )
                    } else {
                        undefined_key(
                            color::WHITE,
                            "Safety Net key",
                            !sn_path.keys[0..i].iter().any(|k| k.is_none()),
                            fixed,
                            !processing,
                        )
                    }
                    .map(move |msg| message::DefinePath::Key(i, msg))
                })
                .collect(),
            false,
            !processing,
        )
        .map(move |msg| {
            // Add 1 to index to account for primary path.
            Message::DefineDescriptor(message::DefineDescriptor::Path(sn_index + 1, msg))
        })
    });

    let last_btn_row = super::template_footer(valid, processing, false);

    layout(
        progress,
        None,
        "Set keys",
        Column::new()
            .align_x(Alignment::Start)
            .max_width(super::MAX_WIDTH)
            .push(advanced_settings)
            .push(primary)
            .push(recovery_paths)
            .push(btn_row)
            .push_maybe(safety_net)
            .push(Space::with_height(10))
            .push(last_btn_row)
            .push(Space::with_height(super::BOTTOM_PADDING))
            .spacing(20),
        true,
        Some(Message::Previous),
    )
}
