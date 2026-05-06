//! Icon-related debug pages, paginated to keep each one fast to render.
//!
//! Layout per font-viewer page: two side-by-side **columns**, each with a
//! header showing the codepoint range it covers and 20 rows of 16 glyph
//! cells (320 codepoints per column → 640 per page). Pages cover the
//! bootstrap-icons PUA range in 4 chunks.
//!
//! The Iconex (`Untitled1`) font is laid out separately: its 19 glyphs sit at
//! scattered codepoints across the BMP (extracted from the font's charset),
//! so the page lists each known codepoint individually rather than walking a
//! contiguous range.

use iced::{Alignment, Font, Length};
use liana_ui::{component::text, icon, theme, widget::*};

use crate::debug::{debug_chrome, DebugMessage, DebugPageEntry};

const BOOTSTRAP_FONT: Font = Font::with_name("bootstrap-icons");
const ICONEX_FONT: Font = Font::with_name("Untitled1");

const HELPER_CELL: Length = Length::Fixed(220.0);
const GLYPH_CELL: Length = Length::Fixed(28.0);
const ROW_LABEL: Length = Length::Fixed(60.0);
const ROW_SPACING: f32 = 6.0;

const CELLS_PER_ROW: u32 = 16;
const ROWS_PER_COLUMN: u32 = 20;
const CODES_PER_COLUMN: u32 = ROWS_PER_COLUMN * CELLS_PER_ROW; // 320
const CODES_PER_PAGE: u32 = 2 * CODES_PER_COLUMN; // 640

// ----- Page entries ---------------------------------------------------------

pub static ENTRY_HELPERS: DebugPageEntry = DebugPageEntry { view: helpers_view };
pub static ENTRY_BOOTSTRAP_1: DebugPageEntry = DebugPageEntry {
    view: bootstrap_page_1,
};
pub static ENTRY_BOOTSTRAP_2: DebugPageEntry = DebugPageEntry {
    view: bootstrap_page_2,
};
pub static ENTRY_BOOTSTRAP_3: DebugPageEntry = DebugPageEntry {
    view: bootstrap_page_3,
};
pub static ENTRY_BOOTSTRAP_4: DebugPageEntry = DebugPageEntry {
    view: bootstrap_page_4,
};
pub static ENTRY_ICONEX: DebugPageEntry = DebugPageEntry { view: iconex_view };

// ----- Helpers section ------------------------------------------------------

type IconFn = fn() -> Text<'static>;

#[rustfmt::skip]
const BOOTSTRAP_HELPERS: &[(&str, IconFn)] = &[
    ("cross_icon", icon::cross_icon),
    ("arrow_down", icon::arrow_down),
    ("arrow_back", icon::arrow_back),
    ("arrow_right", icon::arrow_right),
    ("arrow_return_right", icon::arrow_return_right),
    ("chevron_right", icon::chevron_right),
    ("recovery_icon", icon::recovery_icon),
    ("plug_icon", icon::plug_icon),
    ("reload_icon", icon::reload_icon),
    ("import_icon", icon::import_icon),
    ("wallet_icon", icon::wallet_icon),
    ("bitcoin_icon", icon::bitcoin_icon),
    ("block_icon", icon::block_icon),
    ("dot_icon", icon::dot_icon),
    ("person_icon", icon::person_icon),
    ("tooltip_icon", icon::tooltip_icon),
    ("plus_icon", icon::plus_icon),
    ("warning_icon", icon::warning_icon),
    ("warning_fill_icon", icon::warning_fill_icon),
    ("chip_icon", icon::chip_icon),
    ("trash_icon", icon::trash_icon),
    ("pencil_icon", icon::pencil_icon),
    ("collapse_icon", icon::collapse_icon),
    ("collapsed_icon", icon::collapsed_icon),
    ("down_icon", icon::down_icon),
    ("up_icon", icon::up_icon),
    ("network_icon", icon::network_icon),
    ("previous_icon", icon::previous_icon),
    ("check_icon", icon::check_icon),
    ("round_key_icon", icon::round_key_icon),
    ("backup_icon", icon::backup_icon),
    ("restore_icon", icon::restore_icon),
    ("wrench_icon", icon::wrench_icon),
    ("link_icon", icon::link_icon),
    ("paste_icon", icon::paste_icon),
    ("usb_icon", icon::usb_icon),
    ("usb_drive_icon", icon::usb_drive_icon),
    ("hdd_icon", icon::hdd_icon),
    ("enter_box_icon", icon::enter_box_icon),
    ("collection_icon", icon::collection_icon),
    ("coins_icon", icon::coins_icon),
    ("receive_icon", icon::receive_icon),
    ("send_icon", icon::send_icon),
    ("settings_icon", icon::settings_icon),
];

#[rustfmt::skip]
const ICONEX_HELPERS: &[(&str, IconFn)] = &[
    ("arrow_repeat", icon::arrow_repeat),
    ("home_icon", icon::home_icon),
    ("key_icon", icon::key_icon),
    ("history_icon", icon::history_icon),
    ("clock_icon", icon::clock_icon),
    ("clipboard_icon", icon::clipboard_icon),
    ("circle_check_icon", icon::circle_check_icon),
    ("circle_cross_icon", icon::circle_cross_icon),
];

fn helper_cell(name: &'static str, ctor: IconFn) -> Container<'static, DebugMessage> {
    Container::new(
        Row::new()
            .spacing(10)
            .align_y(Alignment::Center)
            .push(Container::new(ctor()).center_x(GLYPH_CELL))
            .push(text::p2_regular(name)),
    )
    .width(HELPER_CELL)
}

fn helpers_grid(items: &'static [(&'static str, IconFn)]) -> Column<'static, DebugMessage> {
    items
        .chunks(4)
        .fold(Column::new().spacing(ROW_SPACING), |col, chunk| {
            let row = chunk.iter().fold(
                Row::new().spacing(20).align_y(Alignment::Center),
                |r, (n, f)| r.push(helper_cell(n, *f)),
            );
            col.push(row)
        })
}

fn helpers_view() -> Element<'static, DebugMessage> {
    let body = Column::new()
        .spacing(40)
        .push(text::h3("bootstrap-icons helpers"))
        .push(helpers_grid(BOOTSTRAP_HELPERS))
        .push(text::h3("Untitled1 (Iconex) helpers"))
        .push(helpers_grid(ICONEX_HELPERS));

    debug_chrome("Icons — helpers", body)
}

// ----- Bootstrap viewer (paged) --------------------------------------------

fn glyph(c: char, font: Font) -> Container<'static, DebugMessage> {
    Container::new(
        Text::new(c.to_string())
            .font(font)
            .size(18)
            .align_x(iced::alignment::Horizontal::Center),
    )
    .center_x(GLYPH_CELL)
    .center_y(GLYPH_CELL)
    .style(theme::card::border)
}

/// Build one of the two side-by-side columns: header + 20 rows × 16 cells.
fn viewer_column(font: Font, start: u32) -> Column<'static, DebugMessage> {
    let end = start + CODES_PER_COLUMN - 1;
    let header = Container::new(text::p1_bold(format!("U+{start:04X} → U+{end:04X}")))
        .padding([4, 8])
        .style(theme::card::simple);

    let mut col = Column::new().spacing(ROW_SPACING).push(header);
    for r in 0..ROWS_PER_COLUMN {
        let row_start = start + r * CELLS_PER_ROW;
        let mut row = Row::new()
            .spacing(4)
            .align_y(Alignment::Center)
            .push(Container::new(text::caption(format!("U+{row_start:04X}"))).width(ROW_LABEL));
        for c in 0..CELLS_PER_ROW {
            if let Some(ch) = char::from_u32(row_start + c) {
                row = row.push(glyph(ch, font));
            }
        }
        col = col.push(row);
    }
    col
}

fn bootstrap_page(page_start: u32) -> Element<'static, DebugMessage> {
    let left = viewer_column(BOOTSTRAP_FONT, page_start);
    let right = viewer_column(BOOTSTRAP_FONT, page_start + CODES_PER_COLUMN);
    let body = Row::new()
        .spacing(40)
        .align_y(Alignment::Start)
        .push(left)
        .push(right);

    let title: &'static str = match page_start {
        0xF000 => "Icons — bootstrap U+F000..U+F27F",
        0xF280 => "Icons — bootstrap U+F280..U+F4FF",
        0xF500 => "Icons — bootstrap U+F500..U+F77F",
        0xF780 => "Icons — bootstrap U+F780..U+F9FF",
        _ => "Icons — bootstrap",
    };
    debug_chrome(title, body)
}

fn bootstrap_page_1() -> Element<'static, DebugMessage> {
    bootstrap_page(0xF000)
}
fn bootstrap_page_2() -> Element<'static, DebugMessage> {
    bootstrap_page(0xF000 + CODES_PER_PAGE)
}
fn bootstrap_page_3() -> Element<'static, DebugMessage> {
    bootstrap_page(0xF000 + 2 * CODES_PER_PAGE)
}
fn bootstrap_page_4() -> Element<'static, DebugMessage> {
    bootstrap_page(0xF000 + 3 * CODES_PER_PAGE)
}

// ----- Iconex viewer (explicit codepoints) ---------------------------------

/// Iconex glyph codepoints, extracted from the font's charset. The font
/// generator (see `liana-ui/static/icons/iconex/svg_to_ttf.py`) places each
/// glyph at `hash(name) % 0xFFFF`, which is non-deterministic across Python
/// runs — so this list is the authoritative record. If a new SVG is added,
/// re-extract via `fc-query path/to/iconex-icons.ttf`.
#[rustfmt::skip]
const ICONEX_CODEPOINTS: &[u32] = &[
    0x19DA, 0x2CEE, 0x3038, 0x3D0F, 0x46BB, 0x532D, 0x605B, 0x9F25,
    0xB0CA, 0xBD58, 0xBD6B, 0xBEBA, 0xC722, 0xC882, 0xD163, 0xE2F9,
    0xEDE9, 0xF8D3, 0xFFEC,
];

fn iconex_row(code: u32) -> Row<'static, DebugMessage> {
    let cell: Element<'static, DebugMessage> = match char::from_u32(code) {
        Some(c) => glyph(c, ICONEX_FONT).into(),
        None => Container::new(text::caption("?"))
            .center_x(GLYPH_CELL)
            .into(),
    };
    Row::new()
        .spacing(10)
        .align_y(Alignment::Center)
        .push(Container::new(text::p2_regular(format!("U+{code:04X}"))).width(ROW_LABEL))
        .push(cell)
}

fn iconex_view() -> Element<'static, DebugMessage> {
    let mid = ICONEX_CODEPOINTS.len().div_ceil(2);
    let (left_codes, right_codes) = ICONEX_CODEPOINTS.split_at(mid);

    let column = |codes: &[u32]| -> Column<'static, DebugMessage> {
        let first = *codes.first().unwrap();
        let last = *codes.last().unwrap();
        let header = Container::new(text::p1_bold(format!("U+{first:04X} → U+{last:04X}")))
            .padding([4, 8])
            .style(theme::card::simple);
        codes.iter().fold(
            Column::new().spacing(ROW_SPACING).push(header),
            |col, &c| col.push(iconex_row(c)),
        )
    };

    let body = Row::new()
        .spacing(40)
        .align_y(Alignment::Start)
        .push(column(left_codes))
        .push(column(right_codes));

    debug_chrome("Icons — Untitled1 (Iconex) glyphs", body)
}
