use iced::{
    widget::{column, row, Space},
    Alignment, Length,
};
use liana::miniscript::bitcoin::Network;

use liana_ui::{color, component::text::new, image, spacing::VSpacing, theme, widget::*};

use crate::installer::{
    descriptor::{Path, PathSequence},
    message::{self, Message},
    view::{
        editor::{
            defined_key, path,
            template::{
                caption_block, key_legend, row_next, BOTTOM_PADDING, DESCRIPTION_BOTTOM_PADDING,
                FOOTER_SPACING, HARDWARE_WALLET_ADVICE, INHERITANCE_KEY, INTRODUCTION_TITLE,
                KEY_LEGEND_SPACING, PRIMARY_KEY, SET_KEYS_TITLE, UNSUPPORTED_TAPROOT_WARNING,
            },
            undefined_key,
        },
        layout,
    },
};

pub fn inheritance_template_description(
    progress: (usize, usize),
    network: Network,
) -> Element<'static, Message> {
    let title = new::b1_bold("Simple inheritance wallet");

    let intro = caption_block(format!("For this setup you will need 2 Keys: Your Primary Key (for yourself) and an Inheritance Key (for your heir). {HARDWARE_WALLET_ADVICE}."));

    let keys = row![
        key_legend(theme::text::success, PRIMARY_KEY),
        key_legend(theme::text::primary, INHERITANCE_KEY),
    ]
    .spacing(KEY_LEGEND_SPACING);

    let explanation = caption_block("You will always be able to spend using your Primary Key.
After a period of inactivity (but not before that) your Inheritance Key will become able to recover your funds.");

    let diagram = image::inheritance_template_description().width(Length::Fill);

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
        INTRODUCTION_TITLE,
        content,
        Some(Message::Previous),
    )
}

pub fn inheritance_template<'a>(
    progress: (usize, usize),
    network: Network,
    use_taproot: bool,
    primary_path: &'a Path,
    recovery_path: &'a Path,
    valid: bool,
    processing: bool,
) -> Element<'a, Message> {
    let primary_key = if let Some(first) = primary_path.keys.first() {
        first.as_ref()
    } else {
        None
    };

    let advanced_settings = super::advanced_settings_collapse(use_taproot);

    let primary = path(
        color::GREEN,
        None,
        PathSequence::Primary,
        primary_path.warning,
        1,
        vec![if let Some(key) = primary_key {
            defined_key(
                &key.name,
                color::GREEN,
                PRIMARY_KEY,
                if use_taproot && !key.source.is_compatible_taproot() {
                    Some(UNSUPPORTED_TAPROOT_WARNING)
                } else {
                    None
                },
                true,
            )
        } else {
            undefined_key(color::GREEN, PRIMARY_KEY, true, true)
        }
        .map(|msg| message::DefinePath::Key(0, msg))],
        true,
    )
    .map(|msg| Message::DefineDescriptor(message::DefineDescriptor::Path(0, msg)));

    let recovery = path(
        color::WHITE,
        None,
        recovery_path.sequence,
        recovery_path.warning,
        1,
        vec![if let Some(Some(key)) = recovery_path.keys.first() {
            defined_key(
                &key.name,
                color::WHITE,
                INHERITANCE_KEY,
                if use_taproot && !key.source.is_compatible_taproot() {
                    Some(UNSUPPORTED_TAPROOT_WARNING)
                } else {
                    None
                },
                true,
            )
        } else {
            undefined_key(color::WHITE, INHERITANCE_KEY, primary_key.is_some(), true)
        }
        .map(|msg| message::DefinePath::Key(0, msg))],
        true,
    )
    .map(|msg| Message::DefineDescriptor(message::DefineDescriptor::Path(1, msg)));

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
        SET_KEYS_TITLE,
        content,
        Some(Message::Previous),
    )
}
