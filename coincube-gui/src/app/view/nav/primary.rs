//! Primary (72px) left nav rail.
//!
//! Column of icon+label buttons for each `TopLevel`. Settings is pinned
//! to the bottom. Active item gets a 5px orange strip on the leading
//! (left) edge. The avatar + identity block lives in [`super::sidebar`]
//! above both rails, not here.

use super::NavContext;
use crate::app::{
    menu::{MarketplaceSubMenu, Menu, P2PSubMenu, TopLevel},
    view::Message,
};
use coincube_ui::{
    color,
    icon::{
        connect_icon, cube_icon, droplet_fill_icon, lightning_icon, shield_plus_icon, shop_icon,
        vault_icon,
    },
    theme,
    widget::{Button, Column, Element, Row, Text},
};
use iced::{
    widget::{column, container, row, Space},
    Alignment, Length,
};

pub const RAIL_WIDTH: f32 = 72.0;
const ITEM_HEIGHT: f32 = 64.0;
const HIGHLIGHT_WIDTH: f32 = 5.0;
const ICON_SIZE: f32 = 22.0;
const LABEL_SIZE: f32 = 10.0;

pub fn rail<'a>(menu: &Menu, ctx: &NavContext<'a>) -> Element<'a, Message> {
    let current: TopLevel = menu.into();

    let mut top: Column<Message> = Column::new().width(Length::Fixed(RAIL_WIDTH)).spacing(0);
    for &t in &[TopLevel::Home, TopLevel::Spark, TopLevel::Liquid] {
        top = top.push(item(t, current == t));
    }

    // Vault slot: regular nav item when a vault exists, otherwise a
    // "setup vault" action button in the same row slot. Keeping the
    // slot always occupied means the vault-aligned secondary/tertiary
    // offsets stay stable across cube configurations.
    if ctx.has_vault {
        top = top.push(item(TopLevel::Vault, current == TopLevel::Vault));
    } else {
        top = top.push(setup_vault_item());
    }

    // Marketplace is hidden entirely when the cube has neither P2P nor a
    // vault — those are the only two surfaces it can link into, and
    // `TopLevel::Marketplace.default_menu()` would otherwise route the
    // user to a P2P Overview panel that isn't mounted (blank content).
    if ctx.has_p2p || ctx.has_vault {
        let landing = marketplace_landing_menu(ctx);
        top = top.push(item_with_route(
            TopLevel::Marketplace,
            current == TopLevel::Marketplace,
            landing,
        ));
    }
    top = top.push(item(TopLevel::Connect, current == TopLevel::Connect));

    container(top)
        .width(Length::Fixed(RAIL_WIDTH))
        .height(Length::Fill)
        .style(theme::container::sidebar_primary)
        .into()
}

fn item<'a>(t: TopLevel, active: bool) -> Element<'a, Message> {
    item_with_route(t, active, t.default_menu())
}

fn item_with_route<'a>(t: TopLevel, active: bool, route: Menu) -> Element<'a, Message> {
    let body: Column<Message> = column![
        icon_for(t).size(ICON_SIZE),
        iced::widget::text(t.label())
            .size(LABEL_SIZE)
            .align_x(Alignment::Center),
    ]
    .spacing(4)
    .align_x(Alignment::Center)
    .width(Length::Fill);

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
    .on_press(Message::Menu(route));

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

    let r: Row<Message> = row![highlight, button].width(Length::Fixed(RAIL_WIDTH));
    r.into()
}

/// Landing route for a Marketplace rail click.
///
/// The secondary rail only surfaces P2P when `has_p2p` is set and
/// Buy/Sell when `has_vault` is set. Picking the first available entry
/// keeps the click consistent with what the user will actually see in
/// the secondary rail. Callers must gate the Marketplace button
/// themselves — this helper assumes at least one is available and
/// falls back to P2P to match `TopLevel::default_menu`.
fn marketplace_landing_menu(ctx: &NavContext<'_>) -> Menu {
    if ctx.has_p2p {
        Menu::Marketplace(MarketplaceSubMenu::P2P(P2PSubMenu::Overview))
    } else if ctx.has_vault {
        Menu::Marketplace(MarketplaceSubMenu::BuySell)
    } else {
        Menu::Marketplace(MarketplaceSubMenu::P2P(P2PSubMenu::Overview))
    }
}

fn icon_for<'a>(t: TopLevel) -> Text<'a> {
    match t {
        TopLevel::Home => cube_icon(),
        TopLevel::Spark => lightning_icon(),
        TopLevel::Liquid => droplet_fill_icon(),
        TopLevel::Vault => vault_icon(),
        TopLevel::Marketplace => shop_icon(),
        TopLevel::Connect => connect_icon(),
    }
}

/// "Add Vault" action button that occupies the Vault slot when no
/// vault is configured. Same dimensions as a regular rail item so the
/// Y-alignment math for the secondary/tertiary rails stays correct;
/// never renders the active-state highlight strip.
fn setup_vault_item<'a>() -> Element<'a, Message> {
    let body: Column<Message> = column![
        shield_plus_icon().size(ICON_SIZE),
        iced::widget::text("Vault")
            .size(LABEL_SIZE)
            .align_x(Alignment::Center),
    ]
    .spacing(4)
    .align_x(Alignment::Center)
    .width(Length::Fill);

    let button: Button<Message> = Button::new(
        container(body)
            .padding([8, 0])
            .width(Length::Fill)
            .center_x(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fixed(ITEM_HEIGHT))
    .style(theme::button::rail)
    .on_press(Message::SetupVault);

    // Blank left strip so horizontal dimensions match item() exactly.
    let spacer = container(
        Space::new()
            .width(Length::Fixed(HIGHLIGHT_WIDTH))
            .height(Length::Fixed(ITEM_HEIGHT)),
    );

    let r: Row<Message> = row![spacer, button].width(Length::Fixed(RAIL_WIDTH));
    r.into()
}
