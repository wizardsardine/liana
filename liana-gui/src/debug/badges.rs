//! Gallery of every badge constructor exposed by `liana-ui`.

use liana_ui::component::badge::{self};

use crate::debug::Sample;

#[rustfmt::skip]
fn icon_badges() -> Sample<13> {
    [
        (badge::tooltip(),      "liana_ui::component::badge::tooltip()"),
        (badge::receive(),      "liana_ui::component::badge::receive()"),
        (badge::cycle(),        "liana_ui::component::badge::cycle()"),
        (badge::spend(),        "liana_ui::component::badge::spend()"),
        (badge::coin(),         "liana_ui::component::badge::coin()"),
        (badge::success(),         "liana_ui::component::badge::success()"),
        (badge::network(),         "liana_ui::component::badge::network()"),
        (badge::block(),         "liana_ui::component::badge::block()"),
        (badge::bitcoin(),         "liana_ui::component::badge::bitcoin()"),
        (badge::setting(),         "liana_ui::component::badge::setting()"),
        (badge::wallet(),         "liana_ui::component::badge::wallet()"),
        (badge::backup(),         "liana_ui::component::badge::backup()"),
        (badge::restore(),         "liana_ui::component::badge::restore()"),
    ]
}

crate::debug_page!("Badge debug view", [[("Icon badges", icon_badges())]],);
