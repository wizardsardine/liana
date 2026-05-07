//! Example: a debug page built without [`crate::debug_page!`].
//!
//! The macro is just sugar over [`debug_section`] and [`layout_page`]. When
//! the standard layout doesn't fit (asymmetric columns, custom chrome,
//! interleaved widgets, no card background, etc.), declare the [`ENTRY`]
//! and the view function by hand.
//!
//! This file is intentionally **not** wired into the registry — it isn't
//! declared in `mod.rs` and `&example::ENTRY` isn't appended to
//! [`crate::debug::PAGES`]. Copy it as a starting point for a new page.

use iced::{Alignment, Length};
use liana_ui::{component::text, theme, widget::*};

use crate::debug::{debug_section, DebugMessage, DebugPageEntry, Sample};

pub static ENTRY: DebugPageEntry = DebugPageEntry { view };

#[rustfmt::skip]
fn headings() -> Sample<5> {
    [
        (Container::new(text::h1("h1 — display heading")), "liana_ui::component::text::h1"),
        (Container::new(text::h2("h2 — section heading")), "liana_ui::component::text::h2"),
        (Container::new(text::h3("h3 — subsection")), "liana_ui::component::text::h3"),
        (Container::new(text::h4_bold("h4_bold")), "liana_ui::component::text::h4_bold"),
        (Container::new(text::h5_medium("h5_medium")), "liana_ui::component::text::h5_medium"),
    ]
}

#[rustfmt::skip]
fn paragraphs() -> Sample<4> {
    [
        (Container::new(text::p1_bold("p1_bold")), "liana_ui::component::text::p1_bold"),
        (Container::new(text::p1_regular("p1_regular")), "liana_ui::component::text::p1_regular"),
        (Container::new(text::p2_regular("p2_regular")), "liana_ui::component::text::p2_regular"),
        (Container::new(text::caption("caption")), "liana_ui::component::text::caption"),
    ]
}

/// Two ways to build the view:
///
/// **Option A** — reuse `crate::debug::layout_page` for the standard chrome +
/// columns:
/// ```ignore
/// use crate::debug::{debug_section, layout_page};
/// fn view() -> Element<'static, DebugMessage> {
///     layout_page("Text styles", [
///         vec![debug_section("Headings", headings())],
///         vec![debug_section("Paragraphs", paragraphs())],
///     ])
/// }
/// ```
///
/// **Option B** (used below) — go fully custom: build whatever `Element` you
/// like; the only contract is the `fn() -> Element<'static, DebugMessage>`
/// signature on [`ENTRY`].
fn view() -> Element<'static, DebugMessage> {
    let body = Row::new()
        .spacing(40)
        .push(debug_section("Headings", headings()))
        .push(debug_section("Paragraphs", paragraphs()));

    Container::new(
        Column::new()
            .spacing(20)
            .padding(40)
            .align_x(Alignment::Center)
            .push(text::h2("Text styles — fully custom layout"))
            .push(text::p2_regular(
                "No card background, centered chrome, single row of sections.",
            ))
            .push(body),
    )
    .style(theme::container::background)
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}
