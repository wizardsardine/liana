//! Gallery of every `liana_ui::component::card::*` constructor, every
//! `liana_ui::theme::card::*` style, and the common stacked/wrapped patterns
//! found in real Liana flows.
//!
//! Three pages:
//! - **Constructors** — `card::simple/modal/invalid/warning/error/...` plus
//!   `clickable_card` in both interactive and disabled forms.
//! - **Theme styles** — every `theme::card::*` function applied to the same
//!   labeled container, so palettes can be compared side by side.
//! - **Wrapped** — real production arrangements where one card (constructor or
//!   `theme::card::*`-styled container) contains another. Each row is sourced
//!   from a real call site in `liana-gui` or `liana-business` (paths in
//!   comments above each row); update this gallery when new combinations
//!   appear in production.

use iced::{Alignment, Length};
use liana_ui::{
    component::{card, modal, text},
    theme,
    widget::*,
};

use crate::debug::{debug_chrome, DebugMessage, DebugPageEntry};

pub static ENTRY_CONSTRUCTORS: DebugPageEntry = DebugPageEntry {
    view: constructors_view,
};
pub static ENTRY_THEMES: DebugPageEntry = DebugPageEntry { view: themes_view };
pub static ENTRY_WRAPPED: DebugPageEntry = DebugPageEntry { view: wrapped_view };

const ROW_SPACING: f32 = 30.0;

// ----- Layout helpers ------------------------------------------------------

fn entry(
    path: &'static str,
    widget: Element<'static, DebugMessage>,
) -> Column<'static, DebugMessage> {
    Column::new()
        .spacing(8)
        .push(text::p1_regular(path))
        .push(Container::new(widget).width(Length::Fixed(modal::BTN_W as f32)))
}

fn build_page<W>(
    title: &'static str,
    rows: Vec<(&'static str, W)>,
) -> Element<'static, DebugMessage>
where
    W: Into<Element<'static, DebugMessage>>,
{
    let mid = rows.len().div_ceil(2);
    let mut iter = rows.into_iter().map(|(p, w)| entry(p, w.into()));
    let left = (&mut iter)
        .take(mid)
        .fold(Column::new().spacing(ROW_SPACING), Column::push);
    let right = iter.fold(Column::new().spacing(ROW_SPACING), Column::push);
    let body = Row::new()
        .spacing(40)
        .align_y(Alignment::Start)
        .push(left)
        .push(right);
    debug_chrome(title, body)
}

// ----- Constructors view ---------------------------------------------------

fn sample() -> Container<'static, DebugMessage> {
    Container::new(text::p1_regular("Card content")).padding(0)
}

fn clickable_row() -> Row<'static, DebugMessage> {
    Row::new()
        .spacing(10)
        .align_y(Alignment::Center)
        .push(text::p1_regular("Click me"))
}

fn constructors_view() -> Element<'static, DebugMessage> {
    #[rustfmt::skip]
    let rows = vec![
        ("card::simple(<text>)",                          card::simple(sample()).into()),
        ("card::modal(<text>)",                           card::modal(sample()).into()),
        ("card::invalid(<text>)",                         card::invalid(sample()).into()),
        ("card::warning(\"Warning message\".into())",     card::warning("Warning message".to_string()).into()),
        ("card::error(\"Error\", \"<details>\".into())",  card::error("Error", "Detailed error tooltip".to_string()).into()),
        ("card::home_warning(<text>)",                    card::home_warning(sample())),
        ("card::home_hint(<text>)",                       card::home_hint(sample())),
        ("card::clickable_card(<row>, Some(()))",         card::clickable_card(clickable_row(), Some(()))),
        ("card::clickable_card(<row>, None)",             card::clickable_card(clickable_row(), None)),
    ];

    build_page("Cards — constructors", rows)
}

// ----- Theme-styles view ---------------------------------------------------

type StyleFn = fn(&theme::Theme) -> iced::widget::container::Style;

fn styled(label: &'static str, style: StyleFn) -> Container<'static, DebugMessage> {
    Container::new(text::p1_regular(label))
        .padding(15)
        .style(style)
}

fn themes_view() -> Element<'static, DebugMessage> {
    #[rustfmt::skip]
    let rows = vec![
        ("theme::card::simple",        styled("Sample content", theme::card::simple)),
        ("theme::card::button_simple", styled("Sample content", theme::card::button_simple)),
        ("theme::card::transparent",   styled("Sample content", theme::card::transparent)),
        ("theme::card::modal",         styled("Sample content", theme::card::modal)),
        ("theme::card::border",        styled("Sample content", theme::card::border)),
        ("theme::card::invalid",       styled("Sample content", theme::card::invalid)),
        ("theme::card::warning",       styled("Sample content", theme::card::warning)),
        ("theme::card::home_warning",  styled("Sample content", theme::card::home_warning)),
        ("theme::card::home_hint",     styled("Sample content", theme::card::home_hint)),
        ("theme::card::error",         styled("Sample content", theme::card::error)),
    ];

    build_page("Cards — theme styles", rows)
}

// ----- Wrapped — real production patterns ----------------------------------
//
// Every row below mirrors a real arrangement found in liana-gui or
// liana-business. Adjust this gallery whenever a new combination appears in
// production so the visual reference stays current.

/// `Container.style(theme::card::simple)` info inset — the recurring panel
/// used inside xpub modals, PSBT tooltips, and disabled-state hints.
fn simple_inset(label: &'static str) -> Container<'static, DebugMessage> {
    Container::new(text::p1_regular(label))
        .padding(10)
        .width(Length::Fill)
        .style(theme::card::simple)
}

/// `Container.style(theme::card::border)` — the bordered panel used by the
/// home rescan-warning and disabled hardware-wallet entries.
fn bordered_inset(label: &'static str) -> Container<'static, DebugMessage> {
    Container::new(text::p1_regular(label))
        .padding(10)
        .width(Length::Fill)
        .style(theme::card::border)
}

fn wrapped_view() -> Element<'static, DebugMessage> {
    let click_row = || -> Row<'static, DebugMessage> {
        Row::new()
            .spacing(10)
            .align_y(Alignment::Center)
            .push(text::p1_regular("Choose option"))
    };

    #[rustfmt::skip]
    let rows: Vec<(&'static str, Element<'static, DebugMessage>)> = vec![
        // xpub modal: modal_view body holds a stack of `theme::card::simple` info insets.
        // (business-installer/src/views/xpub/modal.rs)
        ("modal_view (theme::card::modal) { card::simple × 3 }",
            Container::new(
                Column::new()
                    .spacing(10)
                    .push(simple_inset("Status: this key already has an xpub."))
                    .push(simple_inset("Current xpub: tpub6Cv…"))
                    .push(simple_inset("Validation: ok"))
            )
            .padding(15)
            .style(theme::card::modal)
            .into()),
        // Settings descriptor card: card::simple wrapping a Column of label + content + button row.
        // (liana-business/src/settings/views/mod.rs)
        ("card::simple { label + content + button row }",
            card::simple(
                Column::new()
                    .spacing(10)
                    .push(text::p1_bold("Wallet descriptor:"))
                    .push(text::p2_regular("wsh(or_d(pk(<key>),and_v(v:pkh(<key>),older(<seq>)))) …"))
                    .push(
                        Row::new()
                            .push(Container::new(text::p1_regular("")).width(Length::Fill))
                            .push(text::p1_regular("[Register on device]"))
                    ),
            ).into()),
        // Card::simple with a card::simple inset inside it (current value display).
        // (business-installer/src/views/xpub/modal.rs lines 78-94)
        ("card::simple { text + Container(theme::card::simple) inset }",
            card::simple(
                Column::new()
                    .spacing(10)
                    .push(text::p1_bold("Current xpub:"))
                    .push(simple_inset("tpub6Cv5p1nXk3K…"))
            ).into()),
        // Settings menu entry: theme::card::button_simple wrapping a Row with badge + label.
        // (liana-business/src/settings/views/mod.rs:175-191)
        ("Container(theme::card::button_simple) { badge + label row }",
            Container::new(
                Row::new()
                    .padding(10)
                    .spacing(20)
                    .align_y(Alignment::Center)
                    .push(text::p1_bold("Settings entry"))
            )
            .width(Length::Fill)
            .style(theme::card::button_simple)
            .into()),
        // Home rescan-warning panel: theme::card::border wrapping content + button row.
        // (liana-gui/src/app/view/home.rs:50-67)
        ("Container(theme::card::border) { warning text + buttons }",
            Container::new(
                Column::new()
                    .spacing(10)
                    .push(text::p1_bold("Rescan recommended"))
                    .push(text::p1_regular("New addresses were imported, please run a rescan."))
                    .push(
                        Row::new()
                            .spacing(10)
                            .push(text::p1_regular("[Go to rescan]"))
                            .push(text::p1_regular("[Dismiss]"))
                    ),
            )
            .padding(25)
            .style(theme::card::border)
            .into()),
        // Recovery key tooltip: pill row containing a Tooltip whose body is theme::card::simple.
        // (liana-gui/src/app/view/recovery.rs:120-135)
        ("card::simple { Row + tooltip(theme::card::simple) body }",
            card::simple(
                Column::new()
                    .spacing(8)
                    .push(text::p1_regular("Recovery path 144 blocks"))
                    .push(simple_inset("Tooltip body: keys held by Alice, Bob"))
            ).into()),
        // PSBT pending: card::simple wrapping a Row with details and a clickable_card action.
        // (liana-gui/src/app/view/psbt.rs around 350-413, simplified)
        ("card::simple { content + clickable_card }",
            card::simple(
                Column::new()
                    .spacing(10)
                    .push(text::p1_bold("PSBT pending"))
                    .push(text::p1_regular("Tx ID: …"))
                    .push(card::clickable_card(click_row(), Some(())))
            ).into()),
        // Installer backup view: two card::simple cards stacked as siblings inside a Column.
        // (liana-gui/src/installer/view/mod.rs:728-761)
        ("Column { card::simple, card::simple } (siblings)",
            Column::new()
                .spacing(10)
                .push(card::simple(text::p1_regular("Descriptor: wsh(…)")))
                .push(card::simple(text::p1_regular("Policy: 2-of-3 with 144-block recovery")))
                .into()),
        // Form with validation error: card::simple wrapping inputs + a card::invalid notice.
        // (common pattern, e.g. installer descriptor edit + validation)
        ("card::simple { inputs + card::invalid notice }",
            card::simple(
                Column::new()
                    .spacing(10)
                    .push(text::p1_regular("Field 1"))
                    .push(text::p1_regular("Field 2"))
                    .push(card::invalid(text::p1_regular("Validation failed")))
            ).into()),
        // Bordered hint with a simple inset (home_hint pattern with sub-content).
        ("Container(theme::card::border) { card::simple inset }",
            Container::new(simple_inset("Inner: theme::card::simple"))
                .padding(15)
                .style(theme::card::border)
                .into()),
        // Modal containing both simple and bordered insets — shows theme contrast
        // between the most common inner styles inside the most common outer.
        ("Container(theme::card::modal) { simple + border × 2 }",
            Container::new(
                Column::new()
                    .spacing(10)
                    .push(simple_inset("theme::card::simple"))
                    .push(bordered_inset("theme::card::border"))
                    .push(simple_inset("theme::card::simple"))
                    .push(bordered_inset("theme::card::border"))
            )
            .padding(15)
            .style(theme::card::modal)
            .into()),
    ];

    build_page("Cards — wrapped (production patterns)", rows)
}
