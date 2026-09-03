use iced::{
    widget::{column, row, Space},
    Alignment, Length,
};
use liana::miniscript::bitcoin::Network;

use liana_ui::{
    color,
    component::{
        button::{btn_add_recovery_option, btn_add_safety_net},
        text::new,
    },
    image,
    spacing::{HSpacing, VSpacing},
    widget::*,
};

use crate::installer::{
    descriptor::Path,
    message::{self, Message},
    view::{
        editor::{
            defined_key, path,
            template::{
                caption_block, row_next, BOTTOM_PADDING, DESCRIPTION_BOTTOM_PADDING, FOOTER_SPACING,
            },
            undefined_key,
        },
        layout,
    },
};
use crate::t;

pub fn custom_template_description(
    progress: (usize, usize),
    network: Network,
) -> Element<'static, Message> {
    let title = new::b1_bold(t!("installer-build-your-own"));

    let intro = caption_block(t!("installer-custom-template-description-1"));

    let explanation = caption_block(t!("installer-custom-template-description-2"));

    let diagram = image::custom_template_description().width(Length::Fill);

    let content = column![
        title,
        intro,
        explanation,
        diagram,
        row_next(),
        Space::with_height(DESCRIPTION_BOTTOM_PADDING),
    ]
    .align_x(Alignment::Start)
    .spacing(VSpacing::L);

    layout(
        progress,
        network,
        None,
        t!("installer-introduction"),
        content,
        Some(Message::Previous),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn custom_template<'a>(
    progress: (usize, usize),
    network: Network,
    use_taproot: bool,
    primary_path: &'a Path,
    recovery_paths: &mut dyn Iterator<Item = (usize, &'a Path)>,
    safety_net_path: Option<(usize, &'a Path)>,
    num_non_primary_paths: usize,
    valid: bool,
    processing: bool,
) -> Element<'a, Message> {
    let prim_keys_fixed = primary_path.keys.len() < 2; // can only delete a primary key if there are 2 or more

    let advanced_settings = super::advanced_settings_collapse(use_taproot);

    let primary = path(
        color::GREEN,
        Some(t!("installer-primary-spending-option")),
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
                        t!("installer-primary-key"),
                        if use_taproot && !key.source.is_compatible_taproot() {
                            Some(t!("installer-device-no-taproot"))
                        } else {
                            None
                        },
                        prim_keys_fixed,
                    )
                } else {
                    undefined_key(
                        color::GREEN,
                        t!("installer-primary-key"),
                        !primary_path.keys[0..i].iter().any(|k| k.is_none()),
                        prim_keys_fixed,
                    )
                }
                .map(move |msg| message::DefinePath::Key(i, msg))
            })
            .collect(),
        false,
    )
    .map(|msg| Message::DefineDescriptor(message::DefineDescriptor::Path(0, msg)));

    let recovery_paths = recovery_paths.into_iter().enumerate().fold(
        column![].spacing(VSpacing::L),
        |col, (i, (p_idx, p))| {
            col.push(
                path(
                    color::ORANGE,
                    Some(t!("installer-recovery-option", number = i + 1)),
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
                                    t!("installer-recovery-key"),
                                    if use_taproot && !key.source.is_compatible_taproot() {
                                        Some(t!("installer-device-no-taproot"))
                                    } else {
                                        None
                                    },
                                    fixed,
                                )
                            } else {
                                undefined_key(
                                    color::ORANGE,
                                    t!("installer-recovery-key"),
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
                }),
            )
        },
    );

    let add_recov_option = Some(Message::DefineDescriptor(
        message::DefineDescriptor::AddRecoveryPath,
    ));

    let safety_net =
        safety_net_path
            .is_none()
            .then_some(btn_add_safety_net(Some(Message::DefineDescriptor(
                message::DefineDescriptor::AddSafetyNetPath,
            ))));

    let btn_row = row![btn_add_recovery_option(add_recov_option), safety_net].spacing(HSpacing::M);

    let safety_net = safety_net_path.map(|(sn_index, sn_path)| {
        path(
            color::WHITE,
            Some(t!("installer-safety-net")),
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
                            t!("installer-safety-net-key"),
                            if use_taproot && !key.source.is_compatible_taproot() {
                                Some(t!("installer-device-no-taproot"))
                            } else {
                                None
                            },
                            fixed,
                        )
                    } else {
                        undefined_key(
                            color::WHITE,
                            t!("installer-safety-net-key"),
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
    });

    let last_btn_row = super::template_footer(valid, processing, false);

    let content = column![
        advanced_settings,
        primary,
        recovery_paths,
        btn_row,
        safety_net,
        Space::with_height(FOOTER_SPACING),
        last_btn_row,
        Space::with_height(BOTTOM_PADDING),
    ]
    .align_x(Alignment::Start)
    .spacing(VSpacing::L);

    layout(
        progress,
        network,
        None,
        t!("btn-set-keys"),
        content,
        Some(Message::Previous),
    )
}
