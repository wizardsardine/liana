//! Secondary (~72px) left nav rail.
//!
//! Styled identically to [`super::primary`] — same dark background, same
//! square icon+label buttons, same active-state treatment. The only
//! visual differences are the orange active-indicator strip (right edge
//! here, left edge on the primary rail) and which side is the content
//! area.

use super::{NavContext, SubItem};
use crate::app::{
    menu::{Menu, TopLevel},
    view::Message,
};
use coincube_ui::{
    color, theme,
    widget::{Button, Column, Element, Row},
};
use iced::{
    widget::{column, container, row, Space},
    Alignment, Length,
};

pub const RAIL_WIDTH: f32 = 72.0;
pub const ITEM_HEIGHT: f32 = 64.0;
const HIGHLIGHT_WIDTH: f32 = 5.0;
const ICON_SIZE: f32 = 22.0;
const LABEL_SIZE: f32 = 10.0;

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
        list = list.push(item_row(menu, &item));
    }

    container(list)
        .width(Length::Fixed(RAIL_WIDTH))
        .height(Length::Fill)
        .style(theme::container::sidebar_primary)
        .into()
}

fn item_row<'a>(menu: &Menu, item: &SubItem) -> Element<'a, Message> {
    let active = (item.matches)(menu);
    let icon = (item.icon)().size(ICON_SIZE);

    let body: Column<Message> = column![
        icon,
        iced::widget::text(item.label)
            .size(LABEL_SIZE)
            .align_x(Alignment::Center),
    ]
    .spacing(4)
    .align_x(Alignment::Center)
    .width(Length::Fill);

    let on_press = if active {
        Message::Reload
    } else {
        Message::Menu(item.route.clone())
    };

    let button: Button<Message> = Button::new(
        container(body)
            .padding([8, 0])
            .width(Length::Fill)
            .center_x(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fixed(ITEM_HEIGHT))
    .style(if active {
        theme::button::rail_active
    } else {
        theme::button::rail
    })
    .on_press(on_press);

    // Orange strip on the trailing (right) edge for the active item —
    // mirror of the primary rail's leading-edge strip.
    let highlight: Element<'a, Message> = if active {
        container(Space::new().width(Length::Fixed(HIGHLIGHT_WIDTH)))
            .height(Length::Fixed(ITEM_HEIGHT))
            .style(theme::container::custom(color::ORANGE))
            .into()
    } else {
        container(
            Space::new()
                .width(Length::Fixed(HIGHLIGHT_WIDTH))
                .height(Length::Fixed(ITEM_HEIGHT)),
        )
        .into()
    };

    let r: Row<Message> = row![button, highlight].width(Length::Fixed(RAIL_WIDTH));
    r.into()
}
