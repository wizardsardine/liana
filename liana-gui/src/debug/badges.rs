//! Gallery of every badge constructor exposed by `liana-ui`.

use liana_ui::component::badge::{self};

use crate::debug::Sample;

#[rustfmt::skip]
fn icon_badges() -> Sample<4> {
    [
        (badge::receive(),      "liana_ui::component::badge::receive()"),
        (badge::cycle(),        "liana_ui::component::badge::cycle()"),
        (badge::spend(),        "liana_ui::component::badge::spend()"),
        (badge::success(),         "liana_ui::component::badge::success()"),
    ]
}

crate::debug_page!("Badge debug view", [[("Icon badges", icon_badges())]],);
