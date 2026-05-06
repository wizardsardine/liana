//! Gallery of every pill constructor and pill style exposed by `liana-ui`.

use liana_ui::{
    component::{
        self,
        pill::{self, PillWidth::M, PillWidth::S},
    },
    theme,
    widget::*,
};

use crate::debug::{DebugMessage, Sample};

fn pill_sample(
    label: &'static str,
    width: pill::PillWidth,
    style: fn(&theme::Theme) -> iced::widget::container::Style,
) -> Container<'static, DebugMessage> {
    component::pill::pill(label, "tooltip", width, style)
}

#[rustfmt::skip]
fn pill_components() -> Sample<12> {
    [
        (pill::recovery(),       "liana_ui::component::pill::recovery()"),
        (pill::batch(),          "liana_ui::component::pill::batch()"),
        (pill::deprecated(),     "liana_ui::component::pill::deprecated()"),
        (pill::spent(),          "liana_ui::component::pill::spent()"),
        (pill::unsigned(),       "liana_ui::component::pill::unsigned()"),
        (pill::signed(),         "liana_ui::component::pill::signed()"),
        (pill::unconfirmed(),    "liana_ui::component::pill::unconfirmed()"),
        (pill::confirmed(),      "liana_ui::component::pill::confirmed()"),
        (pill::key_internal(),   "liana_ui::component::pill::key_internal()"),
        (pill::key_external(),   "liana_ui::component::pill::key_external()"),
        (pill::key_safety_net(), "liana_ui::component::pill::key_safety_net()"),
        (pill::key_cosigner(),   "liana_ui::component::pill::key_cosigner()"),
    ]
}

#[rustfmt::skip]
fn pill_styles() -> Sample<11> {
    [
        (pill_sample("S",            S, theme::pill::simple),       "liana_ui::component::pill::PillWidth::S"),
        (pill_sample("M",            M, theme::pill::simple),       "liana_ui::component::pill::PillWidth::M"),
        (pill_sample("simple",       M, theme::pill::simple),       "liana_ui::theme::pill::simple"),
        (pill_sample("success",      M, theme::pill::success),      "liana_ui::theme::pill::success"),
        (pill_sample("soft_success", M, theme::pill::soft_success), "liana_ui::theme::pill::soft_success"),
        (pill_sample("warning",      M, theme::pill::warning),      "liana_ui::theme::pill::warning"),
        (pill_sample("soft_warning", M, theme::pill::soft_warning), "liana_ui::theme::pill::soft_warning"),
        (pill_sample("internal",     M, theme::pill::internal),     "liana_ui::theme::pill::internal"),
        (pill_sample("external",     M, theme::pill::external),     "liana_ui::theme::pill::external"),
        (pill_sample("safety_net",   M, theme::pill::safety_net),   "liana_ui::theme::pill::safety_net"),
        (pill_sample("batch",        M, theme::pill::batch),        "liana_ui::theme::pill::batch"),
    ]
}

crate::debug_page!(
    "Pill debug view",
    [
        [("Pill components", pill_components())],
        [("Pill styles", pill_styles())],
    ],
);
