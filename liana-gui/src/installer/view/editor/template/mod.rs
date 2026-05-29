pub mod custom;
pub mod inheritance;
pub mod multisig_security_wallet;

use iced::{
    widget::{row, Space},
    Alignment, Length,
};

use liana_ui::{
    component::{
        button::{self, BtnWidth},
        collapse,
        text::{h3, p1_bold, p2_regular},
    },
    icon, theme,
    widget::*,
};

use crate::installer::{
    context,
    message::{self, Message},
    view::{editor::define_descriptor_advanced_settings, layout},
};
use crate::t;

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
    let clear_all = button::secondary(None, t!("common-clear-all"))
        .width(BtnWidth::M)
        .on_press(Message::DefineDescriptor(message::DefineDescriptor::Reset));

    let customize = customize.then_some(
        button::secondary(None, t!("installer-customize"))
            .width(BtnWidth::M)
            .on_press(Message::DefineDescriptor(
                message::DefineDescriptor::ChangeTemplate(context::DescriptorTemplate::Custom),
            )),
    );

    let msg = (!processing & valid).then_some(Message::Next);
    let msg_label = if processing {
        t!("common-processing")
    } else {
        t!("common-continue")
    };
    let contin = button::primary(None, msg_label)
        .width(210)
        .on_press_maybe(msg);

    row![clear_all, Space::with_width(40)]
        .push_maybe(customize)
        .push(Space::fill_width())
        .push(contin)
}

pub fn choose_descriptor_template(progress: (usize, usize)) -> Element<'static, Message> {
    layout(
        progress,
        None,
        t!("installer-choose-wallet-type"),
        Column::new()
            .max_width(800.0)
            .align_x(Alignment::Start)
            .push(
                Button::new(
                    Column::new()
                        .align_x(Alignment::Start)
                        .push(h3(t!("installer-simple-inheritance")))
                        .push(
                            p2_regular(t!("installer-simple-inheritance-description"))
                                .style(theme::text::secondary),
                        )
                        .width(Length::Fill),
                )
                .padding(15)
                .on_press(Message::SelectDescriptorTemplate(
                    context::DescriptorTemplate::SimpleInheritance,
                ))
                .style(theme::button::secondary)
                .width(Length::Fill),
            )
            .push(
                Button::new(
                    Column::new()
                        .align_x(Alignment::Start)
                        .push(h3(t!("installer-expanding-multisig")))
                        .push(
                            p2_regular(t!("installer-expanding-multisig-description"))
                                .style(theme::text::secondary),
                        )
                        .width(Length::Fill),
                )
                .padding(15)
                .on_press(Message::SelectDescriptorTemplate(
                    context::DescriptorTemplate::MultisigSecurity,
                ))
                .style(theme::button::secondary)
                .width(Length::Fill),
            )
            .push(
                Button::new(
                    Column::new()
                        .align_x(Alignment::Start)
                        .push(h3(t!("installer-build-your-own")))
                        .push(
                            p2_regular(t!("installer-build-your-own-description"))
                                .style(theme::text::secondary),
                        )
                        .width(Length::Fill),
                )
                .padding(15)
                .on_press(Message::SelectDescriptorTemplate(
                    context::DescriptorTemplate::Custom,
                ))
                .style(theme::button::secondary)
                .width(Length::Fill),
            )
            .spacing(20),
        true,
        Some(Message::Previous),
    )
}
