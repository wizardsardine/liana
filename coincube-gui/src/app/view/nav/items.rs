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
    /// `Some(reason)` => the item is greyed out and inert, with `reason`
    /// shown in a hover popover. Used for network-gated features (e.g.
    /// Buy/Sell off mainnet). `None` => a normal, clickable item.
    pub disabled_reason: Option<String>,
}

impl SubItem {
    /// A normal, enabled rail item.
    pub fn new(
        label: &'static str,
        icon: fn() -> Text<'static>,
        route: Menu,
        matches: fn(&Menu) -> bool,
    ) -> Self {
        Self {
            label,
            icon,
            route,
            matches,
            disabled_reason: None,
        }
    }

    /// Mark the item disabled with the given popover reason when `reason`
    /// is `Some`. `None` leaves it enabled — convenient for piping an
    /// [`crate::app::features::Availability`] reason straight through.
    pub fn disabled(mut self, reason: Option<String>) -> Self {
        self.disabled_reason = reason;
        self
    }
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
    let disabled = item.disabled_reason.is_some();
    // A disabled item is inert and never reads as the active route.
    let active = !disabled && (item.matches)(menu);

    // "General" items use a hand-authored stroke-only SVG because the Bootstrap
    // `wrench` glyph is a solid silhouette with no outline twin in the font.
    let icon_el: Element<'a, Message> = if item.label == "General" {
        coincube_ui::image::wrench_outline_icon(ICON_SIZE).into()
    } else {
        (item.icon)().size(ICON_SIZE).into()
    };

    let body: Column<Message> = column![
        icon_el,
        iced::widget::text(item.label)
            .size(LABEL_SIZE)
            .align_x(Alignment::Center),
    ]
    .spacing(4)
    .align_x(Alignment::Center)
    .width(Length::Fill);

    let button_inner = container(body)
        .padding([8, 0])
        .width(Length::Fill)
        .center_x(Length::Fill);

    // A disabled item drops `on_press` (iced renders a press-less Button
    // disabled) and uses the greyed rail style; otherwise it's a normal
    // clickable rail item.
    let button: Button<Message> = if disabled {
        Button::new(button_inner)
            .width(Length::Fill)
            .height(Length::Fixed(RAIL_ITEM_HEIGHT))
            .style(theme::button::rail_disabled)
    } else {
        let on_press = if menu == &item.route {
            Message::Reload
        } else {
            Message::Menu(item.route.clone())
        };
        Button::new(button_inner)
            .width(Length::Fill)
            .height(Length::Fixed(RAIL_ITEM_HEIGHT))
            .style(if active {
                theme::button::rail_active
            } else {
                theme::button::rail
            })
            .on_press(on_press)
    };

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

    // Hover popover explaining why the feature is unavailable on this
    // network — same tooltip widget as the Connect status dot.
    match &item.disabled_reason {
        Some(reason) => iced::widget::tooltip(
            r,
            // Padded, bordered popover so the text reads as a floating
            // bubble instead of overlapping the adjacent rail icons.
            container(coincube_ui::component::text::caption(reason.clone()))
                .padding([6, 10])
                .style(theme::container::border_grey),
            iced::widget::tooltip::Position::Right,
        )
        .gap(6)
        .into(),
        None => r.into(),
    }
}
