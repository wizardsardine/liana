pub mod custom;
pub mod inheritance;
pub mod multisig_security_wallet;

use iced::{
    widget::{column, row, Space},
    Alignment, Length,
};

use liana_ui::{
    component::{
        button::{self, btn_clear_all, btn_customize, btn_next},
        collapse,
        text::{new, p1_bold},
    },
    icon, theme,
    widget::*,
};

use crate::installer::{
    context,
    message::{self, Message},
    view::{editor::define_descriptor_advanced_settings, layout},
};

/// Max width of the editor templates' main content column.
pub const MAX_WIDTH: f32 = 1000.0;

/// Bottom padding below the footer of the editor templates.
pub const BOTTOM_PADDING: f32 = 100.0;

pub fn advanced_settings_collapse<'a>(use_taproot: bool) -> Element<'a, Message> {
    fn collapse<'a>(collapsed: bool) -> Element<'a, Message> {
        let icn = if collapsed {
            icon::collapsed_icon()
        } else {
            icon::collapse_icon()
        };
        row![p1_bold("Advanced settings"), icn]
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

pub fn choose_descriptor_template(progress: (usize, usize)) -> Element<'static, Message> {
    let simple_inheritance = template_option(
        "Simple inheritance",
        "Two keys required, one for yourself to spend and another for your heir.",
        context::DescriptorTemplate::SimpleInheritance,
    );
    let expanding_multisig = template_option(
        "Expanding multisig",
        "Two keys required to spend, with an extra key as a backup.",
        context::DescriptorTemplate::MultisigSecurity,
    );
    let custom = template_option(
        "Build your own",
        "Create a custom setup that fits all your needs.",
        context::DescriptorTemplate::Custom,
    );
    let content = column![simple_inheritance, expanding_multisig, custom,]
        .max_width(800.0)
        .align_x(Alignment::Start)
        .spacing(20);

    layout(
        progress,
        None,
        "Choose wallet type",
        content,
        true,
        Some(Message::Previous),
    )
}

fn template_option(
    title: &'static str,
    description: &'static str,
    template: context::DescriptorTemplate,
) -> Element<'static, Message> {
    let content = column![
        new::b1_bold(title),
        new::caption(description).style(theme::text::secondary),
    ]
    .align_x(Alignment::Start)
    .width(Length::Fill);

    button::list_entry(
        content,
        None,
        button::EntryWidth::Fill,
        Some(Message::SelectDescriptorTemplate(template)),
    )
}
