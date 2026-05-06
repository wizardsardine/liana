//! Gallery of every badge constructor and pill style exposed by `liana-ui`.

use liana_ui::{
    component::{
        badge::{self, badge},
        pill, text,
    },
    icon::tooltip_icon,
    theme,
    widget::*,
};

use crate::debug::{DebugMessage, Sample};

fn pill_sample(
    label: &'static str,
    style: fn(&theme::Theme) -> iced::widget::container::Style,
) -> Container<'static, DebugMessage> {
    Container::new(text::p2_regular(label))
        .padding(10)
        .style(style)
}

#[rustfmt::skip]
fn icon_badges() -> Sample<5> {
    [
        (badge(tooltip_icon()), "liana_ui::component::badge::badge(<icon>)"),
        (badge::receive(), "liana_ui::component::badge::receive()"),
        (badge::cycle(), "liana_ui::component::badge::cycle()"),
        (badge::spend(), "liana_ui::component::badge::spend()"),
        (badge::coin(), "liana_ui::component::badge::coin()"),
    ]
}

#[rustfmt::skip]
fn pill_badges() -> Sample<5> {
    [
        (pill::recovery(), "liana_ui::component::pill::recovery()"),
        (pill::unconfirmed(), "liana_ui::component::pill::unconfirmed()"),
        (pill::batch(), "liana_ui::component::pill::batch()"),
        (pill::deprecated(), "liana_ui::component::pill::deprecated()"),
        (pill::spent(), "liana_ui::component::pill::spent()"),
    ]
}

#[rustfmt::skip]
fn pill_styles() -> Sample<7> {
    [
        (pill_sample("  simple  ", theme::pill::simple), "liana_ui::theme::pill::simple"),
        (pill_sample("  primary  ", theme::pill::primary), "liana_ui::theme::pill::primary"),
        (pill_sample("  success  ", theme::pill::success), "liana_ui::theme::pill::success"),
        (pill_sample("  warning  ", theme::pill::warning), "liana_ui::theme::pill::warning"),
        (pill_sample("  internal  ", theme::pill::internal), "liana_ui::theme::pill::internal"),
        (pill_sample("  external  ", theme::pill::external), "liana_ui::theme::pill::external"),
        (pill_sample("  safety_net  ", theme::pill::safety_net), "liana_ui::theme::pill::safety_net"),
    ]
}

crate::debug_page!(
    "Badge debug view",
    [
        [
            ("Icon badges", icon_badges()),
            ("Pill badges", pill_badges()),
        ],
        [("Pill styles", pill_styles()),],
    ],
);
