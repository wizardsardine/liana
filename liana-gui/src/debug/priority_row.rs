//! A priority row and a middle cut text rendered at several widths, so the order in which children
//! give way can be checked.

use iced::{
    alignment::Vertical,
    widget::{column, row, Space},
};
use liana::miniscript::bitcoin::Amount;
use liana_ui::{
    component::{amount::amount_with_font, button::btn_copy, card, pill, text::new},
    priority_row,
    spacing::{HSpacing, VSpacing},
    theme,
    widget::{priority, Column, Element, SpaceExt, TruncatedText},
};

use crate::debug::{debug_chrome, DebugMessage, DebugPageEntry};

pub static ENTRY: DebugPageEntry = DebugPageEntry { view };

const ROW_WIDTHS: [u32; 5] = [900, 600, 450, 300, 200];
const EMAIL_WIDTHS: [u32; 4] = [400, 250, 150, 50];

const LABEL: &str = "Payment to the hardware wallet supplier";
const EMAIL: &str = "alice.longname@wizardsardine.com";

const ROW_PATH: &str = "liana_ui::priority_row![liana_ui::widget::priority::truncate(1, liana_ui::widget::TruncatedText::new(label, new::H2_SPEC).tooltip()), Space::fill_width(), liana_ui::widget::priority::hide(2, liana_ui::component::pill::batch()), liana_ui::widget::priority::hide(3, liana_ui::component::pill::payjoin()), liana_ui::component::amount::amount_with_font(..), liana_ui::component::button::btn_copy(..)]";
const EMAIL_PATH: &str =
    "liana_ui::widget::TruncatedText::new(email, new::B3_SPEC).middle().tooltip()";

fn sample_row() -> Element<'static, DebugMessage> {
    let label = TruncatedText::new(LABEL, new::H2_SPEC)
        .style(theme::text::primary)
        .tooltip();
    let amount = amount_with_font(&Amount::from_sat(123_456_789), new::H2_SPEC);
    priority_row![
        priority::truncate(1, label),
        Space::fill_width(),
        priority::hide(2, pill::batch()),
        priority::hide(3, pill::payjoin()),
        amount,
        btn_copy(Some(())),
    ]
    .spacing(HSpacing::M)
    .align_y(Vertical::Center)
    .into()
}

fn sample_email() -> Element<'static, DebugMessage> {
    TruncatedText::new(EMAIL, new::B3_SPEC)
        .middle()
        .tooltip()
        .into()
}

/// One section: the constructor path, then the sample in a card at each width.
fn section(
    path: &'static str,
    widths: &[u32],
    sample: fn() -> Element<'static, DebugMessage>,
) -> Column<'static, DebugMessage> {
    let path = new::b4(path).style(theme::text::secondary);
    widths.iter().fold(
        Column::new().spacing(VSpacing::M).push(path),
        |column, width| {
            let caption = new::caption(format!("{width}px")).width(80);
            let card = card::simple(sample()).width(*width);
            column.push(row![caption, card].spacing(HSpacing::L))
        },
    )
}

fn view() -> Element<'static, DebugMessage> {
    let rows = section(ROW_PATH, &ROW_WIDTHS, sample_row);
    let emails = section(EMAIL_PATH, &EMAIL_WIDTHS, sample_email);
    debug_chrome("Priority row", column![rows, emails].spacing(VSpacing::XL))
}
