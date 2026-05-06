//! Gallery of every pill constructor exposed by `liana-ui`.

use liana_ui::component::pill;

use crate::debug::Sample;

#[rustfmt::skip]
fn pill_components_a() -> Sample<11> {
    [
        (pill::recovery(),                             "liana_ui::component::pill::recovery()"),
        (pill::batch(),                                "liana_ui::component::pill::batch()"),
        (pill::deprecated(),                           "liana_ui::component::pill::deprecated()"),
        (pill::spent(),                                "liana_ui::component::pill::spent()"),
        (pill::unsigned(),                             "liana_ui::component::pill::unsigned()"),
        (pill::signed(),                               "liana_ui::component::pill::signed()"),
        (pill::unconfirmed(),                          "liana_ui::component::pill::unconfirmed()"),
        (pill::confirmed(),                            "liana_ui::component::pill::confirmed()"),
        (pill::register(),                             "liana_ui::component::pill::register()"),
        (pill::xpub_set(),                             "liana_ui::component::pill::xpub_set()"),
        (pill::xpub_not_set(),                         "liana_ui::component::pill::xpub_not_set()"),
    ]
}

#[rustfmt::skip]
fn pill_components_b() -> Sample<14> {
    [
        (pill::rescan(1.0_f64, false),                        "liana_ui::component::pill::rescan(1.0, false)"),
        (pill::rescan(1.0_f64, true),                        "liana_ui::component::pill::rescan(1.0, true)"),
        (pill::fingerprint("deadbeef", Some("alice")), "liana_ui::component::pill::fingerprint(_, Some(_))"),
        (pill::fingerprint("abcd1234", None),          "liana_ui::component::pill::fingerprint(_, None)"),
        (pill::coin_sequence(0),                       "liana_ui::component::pill::coin_sequence(0)"),
        (pill::coin_sequence(50),                      "liana_ui::component::pill::coin_sequence(50)"),
        (pill::coin_sequence(200),                      "liana_ui::component::pill::coin_sequence(200)"),
        (pill::coin_sequence(5_000),                    "liana_ui::component::pill::coin_sequence(5_000)"),
        (pill::coin_sequence(50_000),                   "liana_ui::component::pill::coin_sequence(50_000)"),
        (pill::coin_sequence(288 + 60),                   "liana_ui::component::pill::coin_sequence(288+60)"),
        (pill::coin_sequence(4_383),                   "liana_ui::component::pill::coin_sequence(4_383)"),
        (pill::coin_sequence(1_440),                   "liana_ui::component::pill::coin_sequence(1_440)"),
        (pill::coin_sequence(52_596),                   "liana_ui::component::pill::coin_sequence(5_296)"),
        (pill::coin_sequence(52_596 + 4_383 ),                   "liana_ui::component::pill::coin_sequence(5_296+60)"),
    ]
}

#[rustfmt::skip]
fn pill_components_c() -> Sample<9> {
    [
        (pill::ws_admin(),                                 "liana_ui::component::pill::ws_admin()"),
        (pill::key_internal(),                             "liana_ui::component::pill::key_internal()"),
        (pill::key_external(),                             "liana_ui::component::pill::key_external()"),
        (pill::key_safety_net(),                           "liana_ui::component::pill::key_safety_net()"),
        (pill::key_cosigner(),                             "liana_ui::component::pill::key_cosigner()"),
        (pill::to_approve(),                               "liana_ui::component::pill::to_approve()"),
        (pill::draft(),                                    "liana_ui::component::pill::draft()"),
        (pill::set_keys(),                                 "liana_ui::component::pill::set_keys()"),
        (pill::active(),                                   "liana_ui::component::pill::active()"),
    ]
}

crate::debug_page!(
    "Pill debug view",
    [
        [("Pill components", pill_components_a())],
        [
            ("Pill components", pill_components_b()),
            ("Pill components (business-installer)", pill_components_c())
        ],
    ],
);
