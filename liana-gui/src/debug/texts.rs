//! Two galleries:
//!
//! - **Constructors** — every `liana_ui::component::text::*` helper, with
//!   its size / font family / weight extracted from the helper's source.
//! - **Themes** — every `liana_ui::theme::text::*` style applied to the same
//!   sample, so palette colors can be compared side by side.

use iced::{
    font::{Family, Weight},
    widget::Space,
    Alignment, Font, Length,
};
use liana_ui::{
    component::text::{
        self, TextSpec, BUTTON_TEXT_SPEC, CAPTION_SPEC, H1_SPEC, H2_SPEC, H3_SPEC, H4_BOLD_SPEC,
        H4_REGULAR_SPEC, H5_MEDIUM_SPEC, H5_REGULAR_SPEC, P1_BOLD_SPEC, P1_MEDIUM_SPEC,
        P1_REGULAR_SPEC, P2_MEDIUM_SPEC, P2_REGULAR_SPEC, PANEL_TITLE_SPEC,
    },
    theme,
    widget::*,
};

use crate::debug::{debug_chrome, DebugMessage, DebugPageEntry};

pub static ENTRY_CONSTRUCTORS: DebugPageEntry = DebugPageEntry {
    view: constructors_view,
};
pub static ENTRY_THEMES: DebugPageEntry = DebugPageEntry { view: themes_view };

const ROW_SPACING: f32 = 5.0;
const SAMPLE_WIDTH: Length = Length::Fixed(600.0);

const SAMPLE: &str = "The quick brown fox jumps";

/// Format a `Font` as `family · weight`, derived from its public fields.
/// Auto-updates if a helper switches font constants.
fn font_label(f: Font) -> String {
    let family = match f.family {
        Family::Name(n) => n.to_string(),
        Family::Serif => "Serif".to_string(),
        Family::SansSerif => "SansSerif".to_string(),
        Family::Cursive => "Cursive".to_string(),
        Family::Fantasy => "Fantasy".to_string(),
        Family::Monospace => "Monospace".to_string(),
    };
    let weight = match f.weight {
        Weight::Thin => "Thin",
        Weight::ExtraLight => "ExtraLight",
        Weight::Light => "Light",
        Weight::Normal => "Regular",
        Weight::Medium => "Medium",
        Weight::Semibold => "Semibold",
        Weight::Bold => "Bold",
        Weight::ExtraBold => "ExtraBold",
        Weight::Black => "Black",
    };
    format!("{family} · {weight}")
}

// ----- Page builders -------------------------------------------------------

/// Render an iterator of pre-built rows inside the standard chrome at a fixed
/// width of `2 × modal::BTN_W`.
fn render(
    title: &'static str,
    rows: Vec<Row<'static, DebugMessage>>,
) -> Element<'static, DebugMessage> {
    let body = rows
        .into_iter()
        .fold(Column::new().spacing(ROW_SPACING), Column::push)
        .width(Length::Fixed(650.0 * 2.0));
    debug_chrome(title, body)
}

// ----- Constructors --------------------------------------------------------

fn constructors_view() -> Element<'static, DebugMessage> {
    #[rustfmt::skip]
    let entries: Vec<(&'static str, TextSpec)> = vec![
        ("liana_ui::component::text::h1",                         H1_SPEC),
        ("liana_ui::component::text::h2",                         H2_SPEC),
        ("liana_ui::component::text::panel_title",                PANEL_TITLE_SPEC),
        ("liana_ui::component::text::h3",                         H3_SPEC),
        ("liana_ui::component::text::h4_bold",                    H4_BOLD_SPEC),
        ("liana_ui::component::text::h4_regular",                 H4_REGULAR_SPEC),
        ("liana_ui::component::text::h5_medium",                  H5_MEDIUM_SPEC),
        ("liana_ui::component::text::h5_regular",                 H5_REGULAR_SPEC),
        ("liana_ui::component::text::p1_bold",                    P1_BOLD_SPEC),
        ("liana_ui::component::text::p1_medium",                  P1_MEDIUM_SPEC),
        ("liana_ui::component::text::p1_regular",                 P1_REGULAR_SPEC),
        ("liana_ui::component::text::text (alias of p1_regular)", P1_REGULAR_SPEC),
        ("liana_ui::component::text::p2_medium",                  P2_MEDIUM_SPEC),
        ("liana_ui::component::text::p2_regular",                 P2_REGULAR_SPEC),
        ("liana_ui::component::text::caption",                    CAPTION_SPEC),
        ("liana_ui::component::text::button_text",                BUTTON_TEXT_SPEC),
    ];

    let rows = entries
        .into_iter()
        .map(|(path, spec)| {
            let size_str = match spec.size {
                Some(s) => format!("size {s}px"),
                None => "size — (caller sets)".to_string(),
            };
            let row = Row::new()
                .spacing(20)
                .align_y(Alignment::Center)
                .push(Space::with_width(30))
                .push(Container::new(text::apply(SAMPLE, spec)).width(SAMPLE_WIDTH))
                .push(
                    Column::new().spacing(2).push(text::p1_regular(path)).push(
                        text::caption(format!("{size_str} · {}", font_label(spec.font)))
                            .style(theme::text::secondary),
                    ),
                )
                .push(Space::with_width(30));
            Row::new()
                .push(
                    liana_ui::component::card::simple(row)
                        .padding(2)
                        .width(Length::Fill),
                )
                .width(1350)
        })
        .collect();

    render("Texts — constructors", rows)
}

// ----- Themes --------------------------------------------------------------

type StyleFn = fn(&theme::Theme) -> iced::widget::text::Style;

fn themes_view() -> Element<'static, DebugMessage> {
    #[rustfmt::skip]
    let entries: Vec<(&'static str, StyleFn)> = vec![
        ("liana_ui::theme::text::default     (no color, inherits)", theme::text::default),
        ("liana_ui::theme::text::primary",                          theme::text::primary),
        ("liana_ui::theme::text::secondary",                        theme::text::secondary),
        ("liana_ui::theme::text::success",                          theme::text::success),
        ("liana_ui::theme::text::warning",                          theme::text::warning),
        ("liana_ui::theme::text::destructive (alias of warning)",   theme::text::destructive),
        ("liana_ui::theme::text::error",                            theme::text::error),
        ("liana_ui::theme::text::accent",                           theme::text::accent),
    ];

    let rows = entries
        .into_iter()
        .map(|(path, style)| {
            Row::new()
                .spacing(20)
                .align_y(Alignment::Center)
                .push(Container::new(text::p1_regular(SAMPLE).style(style)).width(SAMPLE_WIDTH))
                .push(text::p1_regular(path))
        })
        .collect();

    render("Texts — themes", rows)
}
