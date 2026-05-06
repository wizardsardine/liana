//! Two galleries:
//!
//! - **Themes** — every `liana_ui::theme::button::*` style across the four
//!   interactive states (Active, Hovered, Pressed, Disabled). Samples are
//!   styled `Container`s, not real `Button` widgets, so all four states can
//!   be displayed unconditionally.
//! - **Constructors** — every hardcoded `liana_ui::component::button::*`
//!   helper rendered as real `Button` widgets, in both the interactive form
//!   (`on_press` set) and the disabled form (`on_press` omitted). Click
//!   events are swallowed at the GUI boundary by mapping `DebugMessage` to
//!   `Message::DebugNoOp`.

use iced::{widget::button, Alignment, Length};
use liana_ui::{
    component::{button as btn, text},
    icon, theme,
    widget::*,
};

use crate::debug::{debug_chrome, DebugMessage, DebugPageEntry};

type StyleFn = fn(&theme::Theme, button::Status) -> button::Style;

pub static ENTRY_THEMES: DebugPageEntry = DebugPageEntry { view: themes_view };
pub static ENTRY_CONSTRUCTORS_THEMED: DebugPageEntry = DebugPageEntry {
    view: constructors_themed_view,
};
pub static ENTRY_CONSTRUCTORS_WIDTHS: DebugPageEntry = DebugPageEntry {
    view: constructors_widths_view,
};
pub static ENTRY_CONSTRUCTORS_HELPERS: DebugPageEntry = DebugPageEntry {
    view: constructors_helpers_view,
};

const NAME_WIDTH: Length = Length::Fixed(220.0);
const STATE_WIDTH: Length = Length::Fixed(140.0);
const PATH_WIDTH: Length = Length::Fixed(600.0);
const SAMPLE_WIDTH: Length = Length::Fixed(220.0);
const ROW_SPACING: f32 = 12.0;
const STATES: [(button::Status, &str); 4] = [
    (button::Status::Active, "Active"),
    (button::Status::Hovered, "Hovered"),
    (button::Status::Pressed, "Pressed"),
    (button::Status::Disabled, "Disabled"),
];

#[rustfmt::skip]
const THEMES: &[(&str, StyleFn)] = &[
    ("primary", theme::button::primary),
    ("secondary", theme::button::secondary),
    ("tertiary", theme::button::tertiary),
    ("destructive", theme::button::destructive),
    ("container", theme::button::container),
    ("container_border", theme::button::container_border),
    ("clickable_card", theme::button::clickable_card),
    ("menu", theme::button::menu),
    ("tab_menu", theme::button::tab_menu),
    ("transparent", theme::button::transparent),
    ("transparent_primary_text", theme::button::transparent_primary_text),
    ("transparent_border", theme::button::transparent_border),
    ("link", theme::button::link),
];

// ----- Themes table ---------------------------------------------------------

/// Non-interactive label styled exactly like a button in `status`. The style
/// fn is re-evaluated at draw time so palette changes are picked up.
fn fake_button(style_fn: StyleFn, status: button::Status) -> Container<'static, DebugMessage> {
    Container::new(text::p1_regular("Sample"))
        .padding(10)
        .style(move |theme: &theme::Theme| {
            let bs = style_fn(theme, status);
            iced::widget::container::Style {
                text_color: Some(bs.text_color),
                background: bs.background,
                border: bs.border,
                shadow: bs.shadow,
                ..Default::default()
            }
        })
}

fn header_cell(label: &'static str, width: Length) -> Container<'static, DebugMessage> {
    Container::new(text::p1_bold(label)).center_x(width)
}

fn state_cell(style_fn: StyleFn, status: button::Status) -> Container<'static, DebugMessage> {
    Container::new(fake_button(style_fn, status)).center_x(STATE_WIDTH)
}

fn themes_table() -> Column<'static, DebugMessage> {
    let header = STATES.iter().fold(
        Row::new()
            .spacing(ROW_SPACING)
            .align_y(Alignment::Center)
            .push(header_cell("Theme", NAME_WIDTH)),
        |row, (_, label)| row.push(header_cell(label, STATE_WIDTH)),
    );

    THEMES.iter().fold(
        Column::new().spacing(ROW_SPACING).push(header),
        |col, (name, style_fn)| {
            let row = STATES.iter().fold(
                Row::new()
                    .spacing(ROW_SPACING)
                    .align_y(Alignment::Center)
                    .push(Container::new(text::p1_regular(*name)).width(NAME_WIDTH)),
                |row, (status, _)| row.push(state_cell(*style_fn, *status)),
            );
            col.push(row)
        },
    )
}

// ----- Constructors table ---------------------------------------------------

type ConstructorRow = (
    &'static str,
    Element<'static, DebugMessage>,
    Element<'static, DebugMessage>,
);

fn constructor_row(
    path: &'static str,
    interactive: impl Into<Element<'static, DebugMessage>>,
    disabled: impl Into<Element<'static, DebugMessage>>,
) -> Row<'static, DebugMessage> {
    Row::new()
        .spacing(ROW_SPACING)
        .align_y(Alignment::Center)
        .push(Container::new(text::p1_regular(path)).width(PATH_WIDTH))
        .push(Container::new(interactive).center_x(SAMPLE_WIDTH))
        .push(Container::new(disabled).center_x(SAMPLE_WIDTH))
}

/// Build a `(path, interactive, disabled)` row from a single button
/// constructor closure. The closure must produce a button with no
/// `on_press` set (i.e. its disabled form); the helper calls it twice and
/// derives the interactive form by attaching `on_press(())`.
fn row(path: &'static str, builder: impl Fn() -> Button<'static, DebugMessage>) -> ConstructorRow {
    (path, builder().on_press(()).into(), builder().into())
}

/// Compose a constructors table from pre-built rows. Caller is responsible
/// for keeping the row count under the per-page cap (`MAX_ROWS_PER_PAGE`).
fn constructors_table(rows: Vec<ConstructorRow>) -> Column<'static, DebugMessage> {
    let header = Row::new()
        .spacing(ROW_SPACING)
        .align_y(Alignment::Center)
        .push(header_cell("Constructor", PATH_WIDTH))
        .push(header_cell("Interactive", SAMPLE_WIDTH))
        .push(header_cell("Disabled", SAMPLE_WIDTH));

    rows.into_iter().fold(
        Column::new().spacing(ROW_SPACING).push(header),
        |col, (path, interactive, disabled)| col.push(constructor_row(path, interactive, disabled)),
    )
}

/// Per-page cap on row count for constructor tables. Splitting at 15 keeps
/// each page short enough to render and compare without scrolling far.
const MAX_ROWS_PER_PAGE: usize = 15;

fn themes_view() -> Element<'static, DebugMessage> {
    debug_chrome("Button themes", themes_table())
}

fn constructors_themed_view() -> Element<'static, DebugMessage> {
    #[rustfmt::skip]
    let rows = vec![
        row("liana_ui::component::button::primary(None, \"Sample\")",            || btn::primary(None, "Sample")),
        row("liana_ui::component::button::secondary(None, \"Sample\")",          || btn::secondary(None, "Sample")),
        row("liana_ui::component::button::tertiary(None, \"Sample\")",           || btn::tertiary(None, "Sample")),
        row("liana_ui::component::button::destructive(None, \"Sample\")",        || btn::destructive(None, "Sample")),
        row("liana_ui::component::button::alert(None, \"Sample\")",              || btn::alert(None, "Sample")),
        row("liana_ui::component::button::transparent(None, \"Sample\")",        || btn::transparent(None, "Sample")),
        row("liana_ui::component::button::flat(None, \"Sample\")",               || btn::flat(None, "Sample")),
        row("liana_ui::component::button::border(None, \"Sample\")",             || btn::border(None, "Sample")),
        row("liana_ui::component::button::transparent_border(None, \"Sample\")", || btn::transparent_border(None, "Sample")),
        row("liana_ui::component::button::link(None, \"Sample\")",               || btn::link(None, "Sample")),
    ];
    debug_assert!(rows.len() <= MAX_ROWS_PER_PAGE);
    debug_chrome("Button constructors — themed", constructors_table(rows))
}

fn constructors_widths_view() -> Element<'static, DebugMessage> {
    use btn::BtnWidth;
    #[rustfmt::skip]
    let rows = vec![
        // Each preset width applied to btn_primary, then to btn_secondary so
        // the difference between Primary and Secondary at every size is
        // visible side by side.
        row("btn_primary(None, \"S\",   BtnWidth::S,   _)",   || btn::btn_primary(None, "S",   BtnWidth::S,   None)),
        row("btn_primary(None, \"M\",   BtnWidth::M,   _)",   || btn::btn_primary(None, "M",   BtnWidth::M,   None)),
        row("btn_primary(None, \"L\",   BtnWidth::L,   _)",   || btn::btn_primary(None, "L",   BtnWidth::L,   None)),
        row("btn_primary(None, \"XL\",  BtnWidth::XL,  _)",   || btn::btn_primary(None, "XL",  BtnWidth::XL,  None)),
        row("btn_primary(None, \"XXL\", BtnWidth::XXL, _)",   || btn::btn_primary(None, "XXL", BtnWidth::XXL, None)),
        row("btn_secondary(None, \"S\",   BtnWidth::S,   _)", || btn::btn_secondary(None, "S",   BtnWidth::S,   None)),
        row("btn_secondary(None, \"M\",   BtnWidth::M,   _)", || btn::btn_secondary(None, "M",   BtnWidth::M,   None)),
        row("btn_secondary(None, \"L\",   BtnWidth::L,   _)", || btn::btn_secondary(None, "L",   BtnWidth::L,   None)),
        row("btn_secondary(None, \"XL\",  BtnWidth::XL,  _)", || btn::btn_secondary(None, "XL",  BtnWidth::XL,  None)),
        row("btn_secondary(None, \"XXL\", BtnWidth::XXL, _)", || btn::btn_secondary(None, "XXL", BtnWidth::XXL, None)),
    ];
    debug_assert!(rows.len() <= MAX_ROWS_PER_PAGE);
    debug_chrome(
        "Button constructors — preset widths",
        constructors_table(rows),
    )
}

fn constructors_helpers_view() -> Element<'static, DebugMessage> {
    #[rustfmt::skip]
    let rows = vec![
        // Preset-width helpers (built with `None` so the row helper can attach on_press).
        row("liana_ui::component::button::btn_save",                            || btn::btn_save(None)),
        row("liana_ui::component::button::btn_cancel",                          || btn::btn_cancel(None)),
        row("liana_ui::component::button::btn_ok",                              || btn::btn_ok(None)),
        row("liana_ui::component::button::btn_clear",                           || btn::btn_clear(None)),
        row("liana_ui::component::button::btn_retry",                           || btn::btn_retry(None)),
        row("liana_ui::component::button::btn_yes",                             || btn::btn_yes(None)),
        row("liana_ui::component::button::btn_no",                              || btn::btn_no(None)),
        // Round icon button.
        row("liana_ui::component::button::icon_btn(<icon>)",                    || btn::icon_btn(icon::tooltip_icon(), None)),
        // Menu constructors.
        row("liana_ui::component::button::menu(None, \"Item\", false)",         || btn::menu(None, "Item", false)),
        row("liana_ui::component::button::menu(None, \"Item\", true)",          || btn::menu(None, "Item", true)),
        row("liana_ui::component::button::menu_active(None, \"Item\", false)",  || btn::menu_active(None, "Item", false)),
        row("liana_ui::component::button::menu_active(None, \"Item\", true)",   || btn::menu_active(None, "Item", true)),
        row("liana_ui::component::button::menu_small(<icon>)",                  || btn::menu_small(icon::wallet_icon())),
        row("liana_ui::component::button::menu_active_small(<icon>)",           || btn::menu_active_small(icon::wallet_icon())),
    ];
    debug_assert!(rows.len() <= MAX_ROWS_PER_PAGE);
    debug_chrome(
        "Button constructors — helpers & menu",
        constructors_table(rows),
    )
}
