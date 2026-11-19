//! The Overview's Total balance row (`.hero`): the balance block on the left
//! and the worldview on the far right, its voice line beside the mode's art,
//! all standing on one baseline. A narrower column keeps the row with smaller
//! art; one too narrow for the row drops the art and sets the line under the
//! balance.
//!
//! The breakpoints are the width the row is given and what its parts need,
//! not the window's.

use iced::advanced::{layout, widget::Tree};
use iced::alignment::Horizontal;
use iced::widget::text::{Alignment as TextAlignment, LineHeight};
use iced::{Length, Size};

use super::arranged::arranged;
use crate::font;
use crate::theme::{self, Theme};
use crate::widget::{Column, Container, Element, Renderer};

/// `.hero { column-gap: 32px }`, between the balance and the worldview.
pub const COLUMN_GAP: f32 = 32.0;
/// `.wv { gap: 20px }`, between the voice line and the art.
pub const ART_GAP: f32 = 20.0;
/// The art matches the balance block's height (`.art`, 104px).
pub const ART_SIDE: f32 = 104.0;
/// `.narrow .art`, 80px.
pub const NARROW_ART_SIDE: f32 = 80.0;
/// Below this the row uses the narrow art and line.
pub const WIDE_MIN_WIDTH: f32 = 640.0;
/// Below this there is no row: the line sits under the balance, without art.
pub const ROW_MIN_WIDTH: f32 = 520.0;
/// The least the balance column keeps beside the worldview: room for a
/// jumbo fiat total in the hundreds of thousands. The balance fills its
/// column (`minmax(0, 1fr)`), so the worldview, at its own width, gives way.
pub const BALANCE_MIN_WIDTH: f32 = 280.0;

/// `.line { max-width: 190px }`, `.say { font-size: 18px }`.
const LINE_MAX: f32 = 190.0;
const LINE_SIZE: f32 = 18.0;
/// `.narrow .line { max-width: 150px }`, `.narrow .say { font-size: 16px }`.
const NARROW_LINE_MAX: f32 = 150.0;
const NARROW_LINE_SIZE: f32 = 16.0;
/// `.say { line-height: 1.35 }`.
const LINE_HEIGHT: f32 = 1.35;
/// `.rule { width: 28px; height: 1px; margin-bottom: 10px }`.
const RULE_WIDTH: f32 = 28.0;
const RULE_GAP: f32 = 10.0;
/// `.line { padding-bottom: 2px }`: the line's last baseline on the SATS line.
const LINE_BOTTOM: f32 = 2.0;
/// The line's distance under the balance when stacked.
const STACK_GAP: f32 = 12.0;

/// How the row arranges itself at a given width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeroLayout {
    /// The balance, then the line and 104px art at the far right.
    Wide,
    /// The same with the 80px art and the narrower, smaller line.
    Narrow,
    /// The line under the balance, with no art.
    Stacked,
}

/// The arrangement for a row `width` wide, given the worldview group's
/// width with the wide and with the narrow art and line.
pub fn hero_layout(width: f32, wide_side: f32, narrow_side: f32) -> HeroLayout {
    let fits = |side: f32| width - COLUMN_GAP - side >= BALANCE_MIN_WIDTH;
    if width >= WIDE_MIN_WIDTH && fits(wide_side) {
        HeroLayout::Wide
    } else if width >= ROW_MIN_WIDTH && fits(narrow_side) {
        HeroLayout::Narrow
    } else {
        HeroLayout::Stacked
    }
}

/// `balance` with the worldview's `line` (if the mode has one) and art.
pub fn worldview_hero<'a, M: 'a>(
    balance: impl Into<Element<'a, M>>,
    line: Option<&'a str>,
) -> Element<'a, M> {
    let voice = |size: f32, max: f32, align: Horizontal| -> Element<'a, M> {
        match line {
            Some(line) => voice_line(line, size, max, align),
            None => iced::widget::Space::new().into(),
        }
    };
    let children = vec![
        balance.into(),
        voice(LINE_SIZE, LINE_MAX, Horizontal::Right),
        voice(NARROW_LINE_SIZE, NARROW_LINE_MAX, Horizontal::Right),
        voice(NARROW_LINE_SIZE, f32::INFINITY, Horizontal::Left),
        crate::illustration::overview_art(ART_SIDE),
        crate::illustration::overview_art(NARROW_ART_SIDE),
    ];
    arranged(
        children,
        Size::new(Length::Fill, Length::Shrink),
        layout_hero,
    )
    .skip_empty()
    .into()
}

fn layout_hero<'a, M: 'a>(
    children: &mut [Element<'a, M>],
    trees: &mut [Tree],
    renderer: &Renderer,
    limits: &layout::Limits,
) -> layout::Node {
    let max = limits.max();
    let width = if max.width.is_finite() {
        max.width
    } else {
        WIDE_MIN_WIDTH
    };
    let free = layout::Limits::new(Size::ZERO, Size::new(width, f32::INFINITY));
    let mut lay = |index: usize, limits: &layout::Limits| {
        children[index]
            .as_widget_mut()
            .layout(&mut trees[index], renderer, limits)
    };
    // The worldview at its own widths: the lines are no wider than
    // their maximum and the art is square.
    let wide_line = lay(1, &free);
    let narrow_line = lay(2, &free);
    let wide_art = lay(4, &free);
    let narrow_art = lay(5, &free);
    let side = |line: &layout::Node, art: &layout::Node| {
        if line.size().width > 0.0 {
            line.size().width + ART_GAP + art.size().width
        } else {
            art.size().width
        }
    };
    let hidden = || layout::Node::new(Size::ZERO);
    let (nodes, height) = match hero_layout(
        width,
        side(&wide_line, &wide_art),
        side(&narrow_line, &narrow_art),
    ) {
        arrangement @ (HeroLayout::Wide | HeroLayout::Narrow) => {
            let (line, art) = if arrangement == HeroLayout::Wide {
                (wide_line, wide_art)
            } else {
                (narrow_line, narrow_art)
            };
            let column = width - COLUMN_GAP - side(&line, &art);
            let balance = lay(
                0,
                &layout::Limits::new(Size::ZERO, Size::new(column, f32::INFINITY)),
            );
            let (balance_size, line_size, art_size) = (balance.size(), line.size(), art.size());
            let height = balance_size
                .height
                .max(art_size.height)
                .max(line_size.height + LINE_BOTTOM);
            let balance = balance.move_to((0.0, height - balance_size.height));
            let art = art.move_to((width - art_size.width, height - art_size.height));
            let line = line.move_to((
                width - art_size.width - ART_GAP - line_size.width,
                height - LINE_BOTTOM - line_size.height,
            ));
            let nodes = if arrangement == HeroLayout::Wide {
                vec![balance, line, hidden(), hidden(), art, hidden()]
            } else {
                vec![balance, hidden(), line, hidden(), hidden(), art]
            };
            (nodes, height)
        }
        HeroLayout::Stacked => {
            let balance = lay(0, &free);
            let stacked = lay(3, &free);
            let balance_height = balance.size().height;
            let (stacked, height) = if stacked.size().width > 0.0 {
                let height = stacked.size().height;
                (
                    stacked.move_to((0.0, balance_height + STACK_GAP)),
                    balance_height + STACK_GAP + height,
                )
            } else {
                (hidden(), balance_height)
            };
            (
                vec![balance, hidden(), hidden(), stacked, hidden(), hidden()],
                height,
            )
        }
    };
    layout::Node::with_children(Size::new(width, height), nodes)
}

/// `.line`: the mode's 28px rule over the italic voice line, both aligned to
/// `align`.
fn voice_line<'a, M: 'a>(line: &'a str, size: f32, max: f32, align: Horizontal) -> Element<'a, M> {
    let rule = Container::new(iced::widget::Space::new())
        .width(Length::Fixed(RULE_WIDTH))
        .height(Length::Fixed(1.0))
        .style(|theme: &Theme| iced::widget::container::Style {
            background: Some(theme.worldview_accent().into()),
            ..Default::default()
        });
    let text_align = match align {
        Horizontal::Right => TextAlignment::Right,
        Horizontal::Center => TextAlignment::Center,
        Horizontal::Left => TextAlignment::Left,
    };
    Column::new()
        .spacing(RULE_GAP)
        .max_width(max)
        .align_x(align)
        .push(rule)
        .push(
            iced::widget::text(line)
                .shaping(iced::advanced::text::Shaping::Advanced)
                .size(size)
                .line_height(LineHeight::Relative(LINE_HEIGHT))
                .font(font::ITALIC)
                .align_x(text_align)
                .style(theme::tenshu::text::ink),
        )
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_row_narrows_then_stacks_as_its_column_shrinks() {
        let (wide, narrow) = (
            190.0 + ART_GAP + ART_SIDE,
            150.0 + ART_GAP + NARROW_ART_SIDE,
        );
        assert_eq!(hero_layout(840.0, wide, narrow), HeroLayout::Wide);
        assert_eq!(hero_layout(620.0, wide, narrow), HeroLayout::Narrow);
        assert_eq!(hero_layout(600.0, wide, narrow), HeroLayout::Narrow);
        assert_eq!(hero_layout(540.0, wide, narrow), HeroLayout::Stacked);
        assert_eq!(hero_layout(500.0, wide, narrow), HeroLayout::Stacked);
        // Art alone needs less room than art with a line.
        assert_eq!(
            hero_layout(640.0, ART_SIDE, NARROW_ART_SIDE),
            HeroLayout::Wide
        );
        assert_eq!(
            hero_layout(520.0, ART_SIDE, NARROW_ART_SIDE),
            HeroLayout::Narrow
        );
    }
}
