use iced::{
    widget::{column, row, Space},
    Alignment, Length,
};
use liana::miniscript::bitcoin::Network;

use liana_ui::{color, component::text::new, image, spacing::VSpacing, theme, widget::*};

use crate::installer::{
    descriptor::{Path, PathKind, PathSequence},
    message::{self, Message},
    view::{
        editor::{
            defined_key, path,
            template::{
                caption_block, key_legend, row_next, BOTTOM_PADDING, DESCRIPTION_BOTTOM_PADDING,
                FOOTER_SPACING, KEY_LEGEND_SPACING,
            },
            undefined_key, uneditable_defined_key,
        },
        layout,
    },
};
use crate::t;

pub fn multisig_security_template_description(
    progress: (usize, usize),
    network: Network,
) -> Element<'static, Message> {
    let title = new::b1_bold(t!("installer-expanding-multisig-wallet"));

    let intro = caption_block(t!("installer-multisig-description-1"));

    let keys = row![
        key_legend(
            theme::text::success,
            t!("installer-primary-key-number", number = 1)
        ),
        key_legend(
            theme::text::success,
            t!("installer-primary-key-number", number = 2)
        ),
        key_legend(theme::text::warning, t!("installer-recovery-key")),
    ]
    .spacing(KEY_LEGEND_SPACING);

    let explanation = caption_block(t!("installer-multisig-description-2"));

    let diagram = image::multisig_security_template_description().width(Length::Fill);

    let content = column![
        title,
        intro,
        keys,
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

pub fn multisig_security_template<'a>(
    progress: (usize, usize),
    network: Network,
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

    let content = column![
        advanced_settings,
        primary,
        recovery,
        Space::with_height(FOOTER_SPACING),
        footer,
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
