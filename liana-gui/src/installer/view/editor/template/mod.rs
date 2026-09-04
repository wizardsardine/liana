pub mod custom;
pub mod inheritance;
pub mod multisig_security_wallet;

use std::fmt::Display;

use iced::{
    alignment,
    widget::{column, row, Space},
    Alignment, Length,
};
use liana::miniscript::bitcoin::Network;

use liana_ui::{
    component::{
        button::{self, btn_clear_all, btn_customize, btn_next},
        collapse, list,
        text::{new, p1_bold, H3_SIZE},
    },
    icon,
    spacing::HSpacing,
    theme,
    widget::*,
};

use crate::installer::{
    context,
    message::{self, Message},
    view::{editor::define_descriptor_advanced_settings, layout},
};
use crate::t;

/// Bottom padding below the footer of the editor templates.
pub const BOTTOM_PADDING: f32 = 100.0;

/// Bottom padding below the Next button of the template introductions.
pub const DESCRIPTION_BOTTOM_PADDING: f32 = 50.0;

/// Gap between the last spending path and the footer.
pub const FOOTER_SPACING: f32 = 10.0;

/// Gap between the key legend items of the template introductions.
pub const KEY_LEGEND_SPACING: f32 = 30.0;

pub fn advanced_settings_collapse<'a>(use_taproot: bool) -> Element<'a, Message> {
    fn collapse<'a>(collapsed: bool) -> Element<'a, Message> {
        let icn = if collapsed {
            icon::collapsed_icon()
        } else {
            icon::collapse_icon()
        };
        row![p1_bold(t!("installer-advanced-settings")), icn]
            .align_y(Alignment::Center)
            .spacing(10)
            .into()
    }
    collapse::Collapse::new(
        collapse(false),
        collapse(true),
        define_descriptor_advanced_settings(use_taproot),
    )
    .style(theme::button::transparent)
    .into()
}

pub fn template_footer<'a>(valid: bool, processing: bool, customize: bool) -> Row<'a, Message> {
    let clear_all = btn_clear_all(Some(Message::DefineDescriptor(
        message::DefineDescriptor::Reset,
    )));

    let customize = customize.then_some(btn_customize(Some(Message::DefineDescriptor(
        message::DefineDescriptor::ChangeTemplate(context::DescriptorTemplate::Custom),
    ))));

    let msg = (!processing & valid).then_some(Message::Next);
    let next = btn_next(msg);

    row![clear_all, Space::with_width(40)]
        .push_maybe(customize)
        .push(Space::fill_width())
        .push(next)
}

pub fn choose_descriptor_template(network: Network) -> Element<'static, Message> {
    let simple_inheritance = template_option(
        t!("installer-simple-inheritance"),
        t!("installer-simple-inheritance-description"),
        context::DescriptorTemplate::SimpleInheritance,
    );
    let expanding_multisig = template_option(
        t!("installer-expanding-multisig"),
        t!("installer-expanding-multisig-description"),
        context::DescriptorTemplate::MultisigSecurity,
    );
    let custom = template_option(
        t!("installer-build-your-own"),
        t!("installer-build-your-own-description"),
        context::DescriptorTemplate::Custom,
    );
    let content = column![simple_inheritance, expanding_multisig, custom,]
        .align_x(Alignment::Center)
        .width(Length::Fill)
        .spacing(20);

    layout(
        (0, 0),
        network,
        None,
        t!("installer-choose-wallet-type"),
        content,
        Some(Message::Previous),
    )
}

fn template_option(
    title: String,
    description: String,
    template: context::DescriptorTemplate,
) -> Element<'static, Message> {
    let content = column![
        new::b1_bold(title),
        new::caption(description).style(theme::text::secondary),
    ]
    .align_x(Alignment::Start)
    .width(Length::Fill);

    list::list_entry_chevron(
        None,
        content,
        None,
        None,
        button::EntryWidth::Standard,
        Some(Message::SelectDescriptorTemplate(template)),
    )
}

pub fn key_legend<'a>(
    style: fn(&theme::Theme) -> iced::widget::text::Style,
    label: impl Display,
) -> Row<'a, Message> {
    row![
        icon::round_key_icon().size(H3_SIZE).style(style),
        new::b5_bold(label),
    ]
    .align_y(Alignment::Center)
    .spacing(HSpacing::M)
}

pub fn caption_block<'a>(content: impl Display) -> Container<'a, Message> {
    Container::new(
        new::caption(content)
            .style(theme::text::secondary)
            .align_x(alignment::Horizontal::Left),
    )
    .align_x(alignment::Horizontal::Left)
    .width(Length::Fill)
}

pub fn row_next<'a>() -> Row<'a, Message> {
    row![Space::fill_width(), btn_next(Some(Message::Next))]
}
