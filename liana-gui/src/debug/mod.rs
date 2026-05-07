//! Developer-only debug overlays.
//!
//! The whole module is gated by the `debugger` cargo feature, enabled by
//! default. Reproducible and release builds disable it via
//! `--no-default-features`, at which point this module does not compile.
//!
//! ## Pages and stacks
//!
//! Debug pages are organised into named [`DebugStack`]s. Within a stack,
//! [`Ctrl + D + ←/→`] steps through pages; between stacks,
//! [`Ctrl + D + ↑/↓`] cycles (wrapping at both ends). [`Ctrl + D + Esc`]
//! closes the overlay.
//!
//! ## Adding a new page
//!
//! 1. Create `debug/foo.rs` and write a single [`debug_page!`] invocation in it.
//! 2. Declare the module here (`pub mod foo;`).
//! 3. Append `&foo::ENTRY` to the appropriate stack's `pages` slice below.
//!
//! The macro generates the view function and the [`DebugPageEntry`]; chord
//! detection and dispatch read [`STACKS`] dynamically.

use std::{cell::Cell, sync::OnceLock};

use iced::{widget::scrollable, Alignment, Length};
use liana_ui::{component::text, theme, widget::*};

use crate::app::{cache::Cache, menu::Menu, view::Message as ViewMessage};

pub mod badges;
pub mod buttons;
pub mod cards;
pub mod forms;
pub mod home;
pub mod hw;
pub mod icons;
pub mod psbts;
pub mod texts;
pub mod transactions;

/// Navigation hint shown in every debug page's chrome.
pub const NAV_HINT: &str = "Ctrl + D + ←/→ pages · Ctrl + D + ↑/↓ stacks · Ctrl + D + Esc close";

thread_local! {
    /// Current `(global_index, total)` position of the page being rendered,
    /// set by [`render_location`] right before invoking the page's view fn
    /// and cleared after. Chrome helpers read this to display
    /// `<view_index>/<total>` next to the page title.
    static CURRENT_POS: Cell<Option<(usize, usize)>> = const { Cell::new(None) };
}

/// 1-based page index within the current stack, formatted as
/// `index/total` for display next to the title. Returns `None` outside a
/// [`render_location`] dispatch. Empty stacks count as one slot (the
/// placeholder page).
fn current_position_label() -> Option<String> {
    CURRENT_POS.with(Cell::get).map(|(i, t)| format!("{i}/{t}"))
}

fn compute_position(
    stacks: &[&'static DebugStack],
    stack_idx: usize,
    page_idx: usize,
) -> (usize, usize) {
    let stack = stacks[stack_idx];
    let total = stack.pages.len().max(1);
    // page_idx is clamped to [0, total-1] for non-empty stacks; for empty
    // stacks total==1 and page_idx==0 → contributes 1.
    let index = page_idx.min(total - 1) + 1;
    (index, total)
}

// Static `Menu` values so `dashboard_chrome` can hand out `&'static Menu`
// references without leaking on every render. Menu is `Sync` (no interior
// mutability), so a `static` is sound. They are `pub` so debug pages in
// other crates (`liana-business`, `business-installer`) can pick the menu
// entry their stack should highlight.
pub static HOME_MENU: Menu = Menu::Home;
pub static SEND_MENU: Menu = Menu::CreateSpendTx;
pub static RECEIVE_MENU: Menu = Menu::Receive;
pub static PSBTS_MENU: Menu = Menu::PSBTs;
pub static RECOVERY_MENU: Menu = Menu::Recovery;
pub static TRANSACTIONS_MENU: Menu = Menu::Transactions;
pub static COINS_MENU: Menu = Menu::Coins;
pub static SETTINGS_MENU: Menu = Menu::Settings;

/// `Cache` contains a `Cell<Size>` (mutated by `dashboard`'s responsive
/// callback during layout), so it is `!Sync`. iced renders on the main
/// thread, so sharing one `&'static Cache` between debug-overlay frames is
/// safe in practice — we wrap the cache in a unit struct with an `unsafe
/// impl Sync` to satisfy `OnceLock`'s bound.
struct CacheCell(Cache);
// SAFETY: only accessed from the iced view/layout thread.
unsafe impl Sync for CacheCell {}

/// Per-binary `liana_ui::Variant` for the debug-overlay cache (controls
/// e.g. which sidebar logo `view::dashboard` shows). Set from the same
/// site that builds the running `Cache` for the App, so the overlay and
/// the live App stay in sync without the binary's `main` having to know
/// about variants. Defaults to [`liana_ui::Variant::Liana`] when unset.
static DEBUG_VARIANT: OnceLock<liana_ui::Variant> = OnceLock::new();

/// Set the [`liana_ui::Variant`] used by the debug-overlay's shared
/// `Cache`. Called from the same function that constructs the running
/// `Cache` for the App, alongside `Cache { variant: …, … }`. liana-gui
/// never calls this; liana-business calls it from
/// `BusinessSettings::create_app_for_remote_backend` so the dashboard
/// sidebar in the debug overlay shows the blue business logo rather
/// than the green liana-gui one.
///
/// Subsequent calls after the cache has been initialised are silently
/// ignored — the cache is built once and the variant is baked in.
pub fn set_variant(v: liana_ui::Variant) {
    let _ = DEBUG_VARIANT.set(v);
}

fn static_cache() -> &'static Cache {
    static CACHE: OnceLock<CacheCell> = OnceLock::new();
    &CACHE
        .get_or_init(|| {
            let mut cache = Cache::default();
            if let Some(v) = DEBUG_VARIANT.get() {
                cache.variant = *v;
            }
            CacheCell(cache)
        })
        .0
}

/// Wrap a debug-page body in the production sidebar/dashboard chrome,
/// highlighting the given menu entry. Sidebar click messages are swallowed
/// at the boundary via `.map(|_| ())`.
pub fn dashboard_chrome<B>(
    menu: &'static Menu,
    title: &'static str,
    body: B,
) -> Element<'static, DebugMessage>
where
    B: Into<Element<'static, DebugMessage>>,
{
    let body_msg: Element<'static, ViewMessage> = body.into().map(|_| ViewMessage::Reload);
    let content: Column<'static, ViewMessage> = Column::new()
        .spacing(30)
        .push(header_row::<ViewMessage>(title, None))
        .push(body_msg);
    crate::app::view::dashboard(menu, static_cache(), None, content).map(|_| ())
}

/// Variant of [`debug_chrome`] for installer / wizard / modal pages.
/// Uses the darker `theme::container::sidebar` background instead of
/// `theme::container::background`. Modal cards drawn with
/// `theme::card::modal` use the standard chrome background colour
/// (LIGHT_BLACK), so the boundary disappears; switching to the sidebar
/// colour (BLACK / `menu_background`) restores contrast. Useful for any
/// debug page rendering a modal-style card directly on the chrome.
///
/// `path` is the qualified production function (e.g. `"liana_business::
/// settings::views::wallet_view"`) that the page is rendering, surfaced
/// under the title so developers can jump straight to the source.
///
/// A `Space::fill_height()` is appended after the body so the dark
/// background covers the whole viewport even when the body is shorter
/// than the page.
pub fn installer_chrome<B>(
    title: &'static str,
    path: &'static str,
    body: B,
) -> Element<'static, DebugMessage>
where
    B: Into<Element<'static, DebugMessage>>,
{
    // No outer scrollable: the production wizard views (`layout_inner`)
    // already wrap their content in a scrollable + Container with
    // `height(Length::Fill)`. Wrapping that in another scrollable here
    // collapses the inner Length::Fill (scrollable content height ==
    // natural child height) and hides whichever rows the inner layout
    // expected to receive vertical space — that's why account / org /
    // wallet / key lists were rendering empty.
    Container::new(
        Column::new()
            .spacing(15)
            .padding(30)
            .push(header_row::<DebugMessage>(title, Some(path)))
            .push(body.into()),
    )
    .style(theme::container::background)
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// Wrap `modal_body` in a true [`liana_ui::widget::modal::Modal`] overlay
/// on top of the production dashboard for `menu`. The modal widget paints
/// `BLACK_80` over the base before drawing the centered modal body, so the
/// modal card sits on a darker overlay distinct from its own
/// `theme::card::modal` background. Use this for any debug page that
/// represents a true modal in production (e.g. `verify_address_modal`,
/// `register_wallet_modal`, `create_rbf_modal`).
pub fn dashboard_with_modal<M>(
    menu: &'static Menu,
    title: &'static str,
    modal_body: M,
) -> Element<'static, DebugMessage>
where
    M: Into<Element<'static, DebugMessage>>,
{
    let placeholder: Element<'static, ViewMessage> =
        text::p1_regular("(modal — production panel sits behind the dim overlay)").into();
    let dash_content: Row<'static, ViewMessage> = Row::new()
        .spacing(30)
        .push(text::h2(title))
        .push(text::p1_regular(NAV_HINT))
        .push(placeholder);
    let dashboard_elem: Element<'static, DebugMessage> =
        crate::app::view::dashboard(menu, static_cache(), None, dash_content).map(|_| ());
    iced::widget::stack![
        dashboard_elem,
        Modal::new_modal(modal_body.into()).into_element(),
    ]
    .into()
}

/// Like [`dashboard_with_modal`] but the base is the standalone
/// [`installer_chrome`] (no sidebar) — for modals that, in production,
/// overlay a wizard / installer page rather than a dashboard panel.
pub fn installer_with_modal<M>(
    title: &'static str,
    path: &'static str,
    modal_body: M,
) -> Element<'static, DebugMessage>
where
    M: Into<Element<'static, DebugMessage>>,
{
    let placeholder: Element<'static, DebugMessage> =
        text::p1_regular("(modal — production wizard step sits behind the dim overlay)").into();
    let base = installer_chrome(title, path, placeholder);
    iced::widget::stack![base, Modal::new_modal(modal_body.into()).into_element()].into()
}

/// Tiny wrapper around `liana_ui::widget::modal::Modal` whose only purpose
/// is to convert a single overlay-style modal body into an `Element` that
/// `iced::widget::stack` can pair with a base. We don't need a real Modal
/// here because the dim overlay is drawn ourselves via [`DebugDimmer`]
/// below — `Modal::new` requires base + overlay together, but in our case
/// the base is already rendered as a stack sibling.
struct DebugDimmer;

impl DebugDimmer {
    fn into_element() -> Element<'static, DebugMessage> {
        Container::new(Column::new())
            .style(|_: &theme::Theme| iced::widget::container::Style {
                background: Some(iced::Background::Color(liana_ui::color::BLACK_80)),
                ..Default::default()
            })
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}

mod _modal_helper {
    use super::DebugMessage;
    use liana_ui::widget::Element;

    /// Marker that turns any `Element` into a centered overlay that
    /// `iced::widget::stack` will draw on top of a base. This avoids
    /// relying on `liana_ui::widget::modal::Modal`, which produces a
    /// proper modal but layouts the base + overlay together — for our
    /// case `stack![base, overlay]` is simpler and pictures the modal
    /// the same way.
    pub struct Modal;

    impl Modal {
        pub fn new_modal(body: Element<'static, DebugMessage>) -> CenteredOverlay {
            CenteredOverlay { body }
        }
    }

    pub struct CenteredOverlay {
        body: Element<'static, DebugMessage>,
    }

    impl CenteredOverlay {
        pub fn into_element(self) -> Element<'static, DebugMessage> {
            use iced::Length;
            use liana_ui::widget::Container;

            // Draw the dim quad first, then center the modal body on top.
            let dim = super::DebugDimmer::into_element();
            let centered: Element<'static, DebugMessage> = Container::new(self.body)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .padding(20)
                .into();
            iced::widget::stack![dim, centered].into()
        }
    }
}

use _modal_helper::Modal;

/// A named group of related debug pages. Stacks form the outer navigation
/// axis (`Ctrl + D + ↑/↓`), pages within a stack form the inner axis
/// (`Ctrl + D + ←/→`).
///
/// `menu` controls which sidebar entry is highlighted when the stack is
/// rendered through [`dashboard_chrome`]; `None` means the stack uses the
/// plain [`debug_chrome`] (no sidebar). It is also used by the empty-stack
/// placeholder in [`render_location`].
pub struct DebugStack {
    pub name: &'static str,
    pub menu: Option<&'static Menu>,
    pub pages: &'static [&'static DebugPageEntry],
}

pub const DESIGN_SYSTEM: DebugStack = DebugStack {
    name: "Design system",
    menu: None,
    #[rustfmt::skip]
    pages: &[
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
    ],
};

pub const HOME_PANEL: DebugStack = DebugStack {
    name: "Home panel",
    menu: Some(&HOME_MENU),
    pages: &[&home::ENTRY],
};

pub const SEND_PANEL: DebugStack = DebugStack {
    name: "Send panel",
    menu: Some(&SEND_MENU),
    pages: &[],
};

pub const RECEIVE_PANEL: DebugStack = DebugStack {
    name: "Receive panel",
    menu: Some(&RECEIVE_MENU),
    pages: &[],
};

pub const PSBT_PANEL: DebugStack = DebugStack {
    name: "PSBT",
    menu: Some(&PSBTS_MENU),
    pages: &[
        &psbts::ENTRY,
        &psbts::ENTRY_IMPORT_EMPTY,
        &psbts::ENTRY_IMPORT_TYPED,
        &psbts::ENTRY_IMPORT_PROCESSING,
        &psbts::ENTRY_IMPORT_SUCCESS,
        &psbts::ENTRY_RBF_BUMP,
        &psbts::ENTRY_RBF_REPLACED,
        &psbts::ENTRY_RBF_CANCEL,
        &psbts::ENTRY_PSBT_PENDING,
        &psbts::ENTRY_PSBT_BROADCAST,
        &psbts::ENTRY_PSBT_SPENT,
        &psbts::ENTRY_PSBT_RECOVERY,
    ],
};

pub const RECOVERY_PANEL: DebugStack = DebugStack {
    name: "Recovery",
    menu: Some(&RECOVERY_MENU),
    pages: &[],
};

pub const TRANSACTIONS_PANEL: DebugStack = DebugStack {
    name: "Transactions",
    menu: Some(&TRANSACTIONS_MENU),
    pages: &[&transactions::ENTRY],
};

pub const COINS_PANEL: DebugStack = DebugStack {
    name: "Coins",
    menu: Some(&COINS_MENU),
    pages: &[],
};

pub const SETTINGS_PANEL: DebugStack = DebugStack {
    name: "Settings",
    menu: Some(&SETTINGS_MENU),
    pages: &[],
};

pub const HW_MODALS: DebugStack = DebugStack {
    name: "HW modals",
    menu: None,
    pages: &[],
};

pub const INSTALLER_MODALS: DebugStack = DebugStack {
    name: "Installer modals",
    menu: None,
    pages: &[],
};

/// All registered debug stacks, in navigation order. `Ctrl + D + ↑/↓`
/// cycles through this slice (wrapping at both ends).
pub const STACKS: &[&DebugStack] = &[
    &DESIGN_SYSTEM,
    &HOME_PANEL,
    &SEND_PANEL,
    &RECEIVE_PANEL,
    &PSBT_PANEL,
    &RECOVERY_PANEL,
    &TRANSACTIONS_PANEL,
    &COINS_PANEL,
    &SETTINGS_PANEL,
    &HW_MODALS,
    &INSTALLER_MODALS,
];

/// Render the page at `(stack_idx, page_idx)` within the given `stacks`
/// slice. The slice is supplied by the host binary so that downstream
/// crates (`liana-business`, `business-installer`) can extend the default
/// [`STACKS`] without touching this module.
///
/// If the stack has no pages registered yet, render a placeholder so the
/// stack is still reachable during navigation. Empty panel stacks (those
/// with a `menu`) use the same dashboard chrome the real panels will
/// eventually use, so navigation already shows the correct sidebar
/// highlight.
pub fn render_location(
    stacks: &[&'static DebugStack],
    stack_idx: usize,
    page_idx: usize,
) -> Element<'static, DebugMessage> {
    let pos = compute_position(stacks, stack_idx, page_idx);
    CURRENT_POS.with(|c| c.set(Some(pos)));
    let result = (|| {
        let stack = stacks[stack_idx];
        if let Some(entry) = stack.pages.get(page_idx) {
            return (entry.view)();
        }
        let placeholder = text::p1_regular("No debug pages registered for this stack yet.");
        if let Some(menu) = stack.menu {
            return dashboard_chrome(menu, stack.name, placeholder);
        }
        Container::new(scrollable(
            Column::new()
                .spacing(30)
                .padding(30)
                .push(header_row::<DebugMessage>(stack.name, None))
                .push(placeholder),
        ))
        .style(theme::container::background)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    })();
    CURRENT_POS.with(|c| c.set(None));
    result
}

/// Single-row page header: title (h2), `index/total` position caption
/// next to it, optional production function path, then a stretch and the
/// chord-navigation reminder right-aligned. Aligned on the baseline so
/// the big h2 reads naturally next to the smaller captions.
fn header_row<T: 'static>(title: &'static str, path: Option<&'static str>) -> Row<'static, T> {
    use liana_ui::widget::SpaceExt;
    let mut row = Row::new()
        .spacing(15)
        .align_y(Alignment::End)
        .push(text::h2(title));
    if let Some(label) = current_position_label() {
        row = row.push(text::caption(label).style(theme::text::secondary));
    }
    if let Some(path) = path {
        row = row.push(text::caption(path).style(theme::text::secondary));
    }
    row = row
        .push(iced::widget::Space::fill_width())
        .push(text::p1_regular(NAV_HINT).style(theme::text::secondary));
    row
}

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
/// Pages live inside a [`DebugStack`]; their position there determines the
/// order encountered when stepping forward/backward (`Ctrl + D + ←/→`).
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
            .push(header_row::<T>(title, None))
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
/// `&module_name::ENTRY` to the appropriate stack's `pages` slice in
/// [`STACKS`] above; its position there determines the navigation order
/// (`Ctrl + D + ←/→`).
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
