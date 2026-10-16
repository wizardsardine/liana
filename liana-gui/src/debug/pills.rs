//! Gallery of every pill constructor exposed by `liana-ui`.

use liana_ui::component::{list::EntryKeyKind, pill};

use crate::{
    debug::{debug_section, layout_page, Sample},
    t,
};

#[rustfmt::skip]
fn pill_components_a() -> Sample<13> {
    [
        (pill::recovery(),                             "liana_ui::component::pill::recovery()"),
        (pill::batch(),                                "liana_ui::component::pill::batch()"),
        (pill::payjoin(),                              "liana_ui::component::pill::payjoin()"),
        (pill::deprecated(),                           "liana_ui::component::pill::deprecated()"),
        (pill::spent(),                                "liana_ui::component::pill::spent()"),
        (pill::unsigned(),                             "liana_ui::component::pill::unsigned()"),
        (pill::signed(),                               "liana_ui::component::pill::signed()"),
        (pill::timelocked(),                           "liana_ui::component::pill::timelocked()"),
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
        (pill::fingerprint("abcd1234", None::<String>),          "liana_ui::component::pill::fingerprint(_, None)"),
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
fn pill_components_c() -> Sample<14> {
    [
        (pill::ws_admin(),                                 "liana_ui::component::pill::ws_admin()"),
        (pill::role_manager(),                             "liana_ui::component::pill::role_manager()"),
        (pill::role_participant(),                         "liana_ui::component::pill::role_participant()"),
        (pill::draft(),                                    "liana_ui::component::pill::draft()"),
        (pill::to_approve(),                               "liana_ui::component::pill::to_approve()"),
        (pill::set_keys(),                                 "liana_ui::component::pill::set_keys()"),
        (pill::active(),                                   "liana_ui::component::pill::active()"),
        (pill::key_internal(),                             "liana_ui::component::pill::key_internal()"),
        (pill::key_external(),                             "liana_ui::component::pill::key_external()"),
        (pill::key_safety_net(),                           "liana_ui::component::pill::key_safety_net()"),
        (pill::key_cosigner(),                             "liana_ui::component::pill::key_cosigner()"),
        (pill::key_kind(EntryKeyKind::Internal, "alice"),  "liana_ui::component::pill::key_kind(Internal, \"alice\")"),
        (pill::key_kind(EntryKeyKind::SafetyNet, "Wizardsardine"), "liana_ui::component::pill::key_kind(SafetyNet, \"Wizardsardine\")"),
        (pill::registered(),                               "liana_ui::component::pill::registered()"),
    ]
}

#[rustfmt::skip]
fn pill_components_d() -> Sample<7> {
    [
        (pill::path_always_available(),                    "liana_ui::component::pill::path_always_available()"),
        (pill::path_timelock(t!("business-after-days", count = 30)), "liana_ui::component::pill::path_timelock(business-after-days)"),
        (pill::in_policy(2),                               "liana_ui::component::pill::in_policy(2)"),
        (pill::not_in_policy(),                            "liana_ui::component::pill::not_in_policy()"),
        (pill::coin_sequence_compact(0),                   "liana_ui::component::pill::coin_sequence_compact(0)"),
        (pill::coin_sequence_compact(200),                 "liana_ui::component::pill::coin_sequence_compact(200)"),
        (pill::coin_sequence_compact(52_596),              "liana_ui::component::pill::coin_sequence_compact(52_596)"),
    ]
}

#[rustfmt::skip]
crate::debug_pages!(
    (ENTRY_1, layout_page("Pill debug view 1/2", [vec![debug_section("Pill components", pill_components_a())], vec![debug_section("Pill components", pill_components_b())]])),
    (ENTRY_2, layout_page("Pill debug view 2/2", [vec![debug_section("Pill components (business-installer)", pill_components_c())], vec![debug_section("Pill components (business-installer)", pill_components_d())]])),
);
