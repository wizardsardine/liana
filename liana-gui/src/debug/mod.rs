//! Developer-only debug overlays.
//!
//! The whole module is gated by the `debugger` cargo feature, enabled by
//! default. Reproducible and release builds disable it via
//! `--no-default-features`, at which point this module does not compile.
//!
//! ## Adding a new page
//!
//! 1. Create `debug/foo.rs` and write a single [`debug_page!`] invocation in it.
//! 2. Declare the module here (`pub mod foo;`) and append `&foo::ENTRY` to
//!    [`PAGES`] below.
//!
//! That's it — the macro generates the view function and the
//! [`DebugPageEntry`]; chord detection and dispatch read [`PAGES`] dynamically.

use iced::{widget::scrollable, Alignment, Length};
use liana_ui::{component::text, theme, widget::*};

pub mod badges;
pub mod buttons;
pub mod cards;
pub mod forms;
pub mod hw;
pub mod icons;
pub mod texts;

/// Every registered debug page. Append `&module::ENTRY` here when adding a new
/// debug module — that's the one line of glue beyond the `debug_page!` call.
/// Order matters: navigation walks this slice front-to-back.
#[rustfmt::skip]
pub const PAGES: &[&DebugPageEntry] = &[
    &badges::ENTRY,
    &buttons::ENTRY_THEMES,
    &buttons::ENTRY_CONSTRUCTORS_THEMED,
    &buttons::ENTRY_CONSTRUCTORS_WIDTHS,
    &buttons::ENTRY_CONSTRUCTORS_HELPERS,
    &hw::ENTRY_PAGE_1,
    &hw::ENTRY_PAGE_2,
    &forms::ENTRY,
    &cards::ENTRY_CONSTRUCTORS,
    &cards::ENTRY_THEMES,
    &cards::ENTRY_WRAPPED,
    &texts::ENTRY_CONSTRUCTORS,
    &texts::ENTRY_THEMES,
    &icons::ENTRY_HELPERS,
    &icons::ENTRY_BOOTSTRAP_1,
    &icons::ENTRY_BOOTSTRAP_2,
    &icons::ENTRY_BOOTSTRAP_3,
    &icons::ENTRY_BOOTSTRAP_4,
    &icons::ENTRY_ICONEX,
    // <- add new entry here
];

/// Message type produced by debug widgets.
///
/// We use `()` so debug pages can render real interactive widgets (e.g. a
/// `Button` with `on_press(())`) for showcase purposes. Clicks are swallowed
/// at the GUI boundary by mapping to a no-op message.
pub type DebugMessage = ();

/// Standard array shape for a section: `N` `(widget, code-path)` pairs. Use
/// this as the return type of helper fns that build a section's items.
pub type Sample<const N: usize> = [(Container<'static, DebugMessage>, &'static str); N];

/// One registered debug page. Constructed by the [`debug_page!`] macro.
///
/// Pages are navigated via the chord (`Ctrl + D + ←/→/Esc`); their order in
/// [`PAGES`] determines the order encountered when stepping forward/backward.
pub struct DebugPageEntry {
    /// Renders the page's overlay.
    pub view: fn() -> Element<'static, DebugMessage>,
}

/// Render one labeled section. Each item is paired with the code path used to
/// construct it; the conversion to [`Element`] is done here so callers can
/// keep their arrays free of `.into()` noise.
pub fn debug_section<T, I, W>(title: &'static str, items: I) -> Column<'static, T>
where
    T: 'static,
    I: IntoIterator<Item = (W, &'static str)>,
    W: Into<Element<'static, T>>,
{
    items.into_iter().fold(
        Column::new().spacing(15).push(text::h3(title)),
        |col, (sample, path)| {
            col.push(
                Row::new()
                    .spacing(20)
                    .align_y(Alignment::Center)
                    .push(Container::new(sample.into()).width(Length::Fixed(160.0)))
                    .push(text::p1_regular(path)),
            )
        },
    )
}

/// Wrap an arbitrary body in the standard debug chrome (page title,
/// navigation hint, scroll, card background). Use this directly when a page
/// needs a layout that doesn't fit the columns-of-sections shape that
/// [`layout_page`] produces.
pub fn debug_chrome<T>(
    title: &'static str,
    body: impl Into<Element<'static, T>>,
) -> Element<'static, T>
where
    T: 'static,
{
    Container::new(scrollable(
        Column::new()
            .spacing(30)
            .padding(30)
            .push(text::h2(title))
            .push(text::p1_regular(
                "Ctrl + D + ←/→ to navigate · Ctrl + D + Esc to close",
            ))
            .push(body.into()),
    ))
    .style(theme::container::background)
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// Lay out pre-built columns of sections inside the standard debug chrome.
/// Each column is an iterator of [`Column`] sections; an empty column is
/// allowed.
pub fn layout_page<T, C, S>(title: &'static str, columns: C) -> Element<'static, T>
where
    T: 'static,
    C: IntoIterator<Item = S>,
    S: IntoIterator<Item = Column<'static, T>>,
{
    let body = columns
        .into_iter()
        .fold(Row::new().spacing(60), |row, sections| {
            let column = sections
                .into_iter()
                .fold(Column::new().spacing(30), Column::push);
            row.push(Container::new(column).width(Length::FillPortion(1)))
        });
    debug_chrome(title, body)
}

/// Define a debug page at module top level.
///
/// Generates a `pub static ENTRY: DebugPageEntry` and a private view function.
///
/// The macro cannot register the page on its own — there is no compile-time
/// inventory in this crate. After calling it, the developer must add
/// `&module_name::ENTRY` to the [`PAGES`] slice above; its position there
/// determines the navigation order (Ctrl + D + ←/→).
///
/// Arguments (positional): `title`, then nested column arrays.
///
/// ```ignore
/// debug_page!(
///     "Badge debug view",
///     [
///         [
///             ("Icon badges", icon_badges),
///             ("Pill badges", pill_badges),
///         ],
///         [
///             ("Pill styles", pill_styles),
///         ],
///     ],
/// );
/// ```
#[macro_export]
macro_rules! debug_page {
    (
        $title:literal,
        [ $(
            [ $( ( $section_title:expr , $items:expr ) ),* $(,)? ]
        ),* $(,)? ] $(,)?
    ) => {
        pub static ENTRY: $crate::debug::DebugPageEntry = $crate::debug::DebugPageEntry {
            view: __debug_view,
        };

        fn __debug_view() -> ::liana_ui::widget::Element<'static, $crate::debug::DebugMessage> {
            $crate::debug::layout_page(
                $title,
                [ $(
                    ::std::vec![
                        $( $crate::debug::debug_section($section_title, $items) ),*
                    ]
                ),* ],
            )
        }
    };
}
