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

pub fn multisig_security_template_description(
    progress: (usize, usize),
    network: Network,
) -> Element<'static, Message> {
    let title = new::b1_bold("Expanding multisig wallet");

    let intro = caption_block("For this setup you will need 3 keys: two Primary Keys and a Recovery Key. For security reasons, we suggest you use a separate Hardware Wallet for each key.");

    let keys = row![
        key_legend(theme::text::success, "Primary key #1"),
        key_legend(theme::text::success, "Primary key #2"),
        key_legend(theme::text::success, "Recovery key"),
    ]
    .spacing(KEY_LEGEND_SPACING);

    let explanation = caption_block("The Primary Keys will compose a 2-of-2 multisig which will always be able to spend. In case one of your keys becomes unavailable, after a period of inactivity you will be able to recover your funds using the Recovery Key together with one of your Primary Keys (2-of-3 multisig):");

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
        "Introduction",
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
        "Set keys",
        content,
        Some(Message::Previous),
    )
}
