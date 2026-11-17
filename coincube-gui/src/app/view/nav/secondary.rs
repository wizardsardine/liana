//! Secondary (~72px) left nav rail.
//!
//! Styled identically to [`super::primary`] — same dark background, same
//! square icon+label buttons, same active-state treatment. The only
//! visual differences are the orange active-indicator strip (right edge
//! here, left edge on the primary rail) and which side is the content
//! area.

use super::items::render_item_row;
use super::NavContext;
use crate::app::{
    menu::{Menu, TopLevel},
    view::Message,
};
use coincube_ui::widget::{Column, Element};
use iced::{widget::container, Length};

pub const RAIL_WIDTH: f32 = 72.0;

/// Secondary rail: the submenu of whichever [`TopLevel`] section `menu`
/// currently resolves to, one [`render_item_row`] per [`super::SubItem`].
///
/// The per-section item lists live in the sibling modules (`cube`,
/// `spark`, `vault`, `marketplace`); this function only picks
/// the right one and lays it out.
///
/// Like [`super::primary::rail`], the column is sized to its content and
/// carries no background of its own — [`super::sidebar`] wraps both rails
/// in one scrollable, full-height container styled with
/// `sidebar_primary`, so a rail taller than the window scrolls instead of
/// being clipped.
pub fn rail<'a>(menu: &Menu, ctx: &NavContext<'a>) -> Element<'a, Message> {
    let current: TopLevel = menu.into();

    let items = match current {
        TopLevel::Cube => super::cube::items(ctx),
        TopLevel::Spark => super::spark::items(ctx),
        TopLevel::Vault => super::vault::items(ctx),
        TopLevel::Marketplace => super::marketplace::items(ctx),
    };

    let mut list: Column<Message> = Column::new().spacing(0).width(Length::Fill);
    for item in items {
        list = list.push(render_item_row(menu, &item, RAIL_WIDTH));
    }

    container(list).width(Length::Fixed(RAIL_WIDTH)).into()
}
