//! Gallery of every badge constructor exposed by `liana-ui`.

use liana_ui::{
    component::badge::{self, badge},
    icon::tooltip_icon,
};

use crate::debug::Sample;

#[rustfmt::skip]
fn icon_badges() -> Sample<5> {
    [
        (badge(tooltip_icon()), "liana_ui::component::badge::badge(<icon>)"),
        (badge::receive(),      "liana_ui::component::badge::receive()"),
        (badge::cycle(),        "liana_ui::component::badge::cycle()"),
        (badge::spend(),        "liana_ui::component::badge::spend()"),
        (badge::coin(),         "liana_ui::component::badge::coin()"),
    ]
}

crate::debug_page!("Badge debug view", [[("Icon badges", icon_badges())]],);
