use crate::app::menu::Menu;
use crate::app::view::Message;
use coincube_ui::{
    color, theme,
    widget::{Button, Column, Element, Row, Text},
};
use iced::{
    widget::{column, container, row, Space},
    Alignment, Length,
};

/// One row in the secondary rail.
///
/// `matches` exists because some submenu variants carry payload
/// (`Transactions(Option<Txid>)`, `Settings(Option<SettingsOption>)`, etc.)
/// so a plain `menu == &item.route` check won't highlight both the empty
/// and payload-carrying instances of the same logical item.
pub struct SubItem {
    pub label: &'static str,
    pub icon: fn() -> Text<'static>,
    pub route: Menu,
    pub matches: fn(&Menu) -> bool,
}

// Sizing shared by the secondary and tertiary rails. The two rails
// render the same `SubItem` shape with the same square icon+label
// buttons, so the constants and layout live here to keep them from
// drifting.
pub const RAIL_ITEM_HEIGHT: f32 = 72.0;
const HIGHLIGHT_WIDTH: f32 = 5.0;
const ICON_SIZE: f32 = 22.0;
const LABEL_SIZE: f32 = 10.0;

/// Render one row — icon + label button with a trailing orange strip
/// on the active row. Shared by the secondary and tertiary rails so
/// their rows stay visually identical.
pub fn render_item_row<'a>(menu: &Menu, item: &SubItem, rail_width: f32) -> Element<'a, Message> {
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

    let on_press = if menu == &item.route {
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
    .height(Length::Fixed(RAIL_ITEM_HEIGHT))
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
            .height(Length::Fixed(RAIL_ITEM_HEIGHT))
            .style(theme::container::custom(color::ORANGE))
            .into()
    } else {
        container(
            Space::new()
                .width(Length::Fixed(HIGHLIGHT_WIDTH))
                .height(Length::Fixed(RAIL_ITEM_HEIGHT)),
        )
        .into()
    };

    let r: Row<Message> = row![button, highlight].width(Length::Fixed(rail_width));
    r.into()
}
