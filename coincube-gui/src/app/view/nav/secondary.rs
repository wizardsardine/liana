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
use coincube_ui::{
    theme,
    widget::{Column, Element},
};
use iced::{widget::container, Length};

pub const RAIL_WIDTH: f32 = 72.0;

pub fn rail<'a>(menu: &Menu, ctx: &NavContext<'a>) -> Element<'a, Message> {
    let current: TopLevel = menu.into();

    let items = match current {
        TopLevel::Home => super::home::items(ctx),
        TopLevel::Spark => super::spark::items(ctx),
        TopLevel::Liquid => super::liquid::items(ctx),
        TopLevel::Vault => super::vault::items(ctx),
        TopLevel::Marketplace => super::marketplace::items(ctx),
        TopLevel::Connect => super::connect::items(ctx),
    };

    let mut list: Column<Message> = Column::new().spacing(0).width(Length::Fill);
    for item in items {
        list = list.push(render_item_row(menu, &item, RAIL_WIDTH));
    }

    container(list)
        .width(Length::Fixed(RAIL_WIDTH))
        .height(Length::Fill)
        .style(theme::container::sidebar_primary)
        .into()
}
