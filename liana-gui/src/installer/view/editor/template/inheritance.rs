use iced::{alignment, widget::Space, Alignment, Length};

use liana_ui::{
    color,
    component::{
        button,
        text::{h3, p1_regular, Text, H3_SIZE},
    },
    icon, image, theme,
    widget::*,
};

use crate::installer::{
    descriptor::{Path, PathSequence},
    message::{self, Message},
    view::{
        editor::{defined_key, path, undefined_key},
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
                    .push(icon::round_key_icon().size(H3_SIZE).color(color::GREEN))
                    .push(p1_regular("Primary key").bold())
                ).push(
                    Row::new()
                        .align_y(Alignment::Center)
                        .spacing(10)
                        .push(icon::round_key_icon().size(H3_SIZE).color(color::WHITE))
                        .push(p1_regular("Inheritance key").bold())
            ))
            .push(Container::new(
                p1_regular("You will always be able to spend using your Primary Key.
After a period of inactivity (but not before that) your Inheritance Key will become able to recover your funds.")
                .style(theme::text::secondary)
                .align_x(alignment::Horizontal::Left)
            ).align_x(alignment::Horizontal::Left).width(Length::Fill))
            .push(image::inheritance_template_description().width(Length::Fill))
            .push(Row::new().push(Space::with_width(Length::Fill)).push(button::primary(None, "Next").width(Length::Fixed(200.0)).on_press(Message::Next)))
            .push(Space::with_height(50.0))
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
    processing: bool,
) -> Element<'a, Message> {
    let primary_key = if let Some(first) = primary_path.keys.first() {
        first.as_ref()
    } else {
        None
    };

    let advanced_settings = super::advanced_settings_collapse(use_taproot, !processing);

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
                "Primary key",
                if use_taproot && !key.source.is_compatible_taproot() {
                    Some("This device does not support Taproot")
                } else {
                    None
                },
                true,
                !processing,
            )
        } else {
            undefined_key(color::GREEN, "Primary key", true, true, !processing)
        }
        .map(|msg| message::DefinePath::Key(0, msg))],
        true,
        !processing,
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
                "Inheritance key",
                if use_taproot && !key.source.is_compatible_taproot() {
                    Some("This device does not support Taproot")
                } else {
                    None
                },
                true,
                !processing,
            )
        } else {
            undefined_key(
                color::WHITE,
                "Inheritance key",
                primary_key.is_some(),
                true,
                !processing,
            )
        }
        .map(|msg| message::DefinePath::Key(0, msg))],
        true,
        !processing,
    )
    .map(|msg| Message::DefineDescriptor(message::DefineDescriptor::Path(1, msg)));

    let footer = super::template_footer(valid, processing, true);

    layout(
        progress,
        None,
        "Set keys",
        Column::new()
            .align_x(Alignment::Start)
            .max_width(super::MAX_WIDTH)
            .push(advanced_settings)
            .push(primary)
            .push(recovery)
            .push(Space::with_height(10))
            .push(footer)
            .push(Space::with_height(super::BOTTOM_PADDING))
            .spacing(20),
        true,
        Some(Message::Previous),
    )
}
