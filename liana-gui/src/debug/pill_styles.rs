//! Gallery of every pill style exposed by `liana-ui`.

use liana_ui::{
    component::{
        self,
        pill::{
            self,
            PillWidth::{L, M, S},
        },
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
fn pill_styles() -> Sample<10> {
    [
        (pill_sample("S",            S, theme::pill::simple),       "liana_ui::component::pill::PillWidth::S"),
        (pill_sample("M",            M, theme::pill::simple),       "liana_ui::component::pill::PillWidth::M"),
        (pill_sample("L",            L, theme::pill::simple),       "liana_ui::component::pill::PillWidth::L"),
        (pill_sample("simple",       M, theme::pill::simple),       "liana_ui::theme::pill::simple"),
        (pill_sample("success",      M, theme::pill::success),      "liana_ui::theme::pill::success"),
        (pill_sample("warning",      M, theme::pill::warning),      "liana_ui::theme::pill::warning"),
        (pill_sample("soft_warning", M, theme::pill::soft_warning), "liana_ui::theme::pill::soft_warning"),
        (pill_sample("internal",     M, theme::pill::internal),     "liana_ui::theme::pill::internal"),
        (pill_sample("external",     M, theme::pill::external),     "liana_ui::theme::pill::external"),
        (pill_sample("safety_net",   M, theme::pill::safety_net),   "liana_ui::theme::pill::safety_net"),
    ]
}

crate::debug_page!("Pill styles debug view", [[("Pill styles", pill_styles())]],);
