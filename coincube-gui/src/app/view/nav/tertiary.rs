//! Tertiary (~72px) left nav rail — the third column that appears only
//! when the current route has a third level (e.g. Cube → Settings →
//! {General/Lightning/About}, Marketplace → P2P → {Order Book / My Trades
//! / ...}, Vault → Settings → {Node / Wallet / Import-Export / About}).
//!
//! Styled like [`super::primary`] and [`super::secondary`], but with a
//! slightly lighter background so the third level reads as "deeper".
//! Renders nothing when the active route has no third level — callers
//! check [`items_for`] for that and omit the rail entirely.

use super::{NavContext, SubItem};
use crate::app::{
    menu::{
        HomeSettingsOption, HomeSubMenu, MarketplaceSubMenu, Menu, P2PSubMenu, SettingsOption,
        VaultSubMenu,
    },
    view::Message,
};
use coincube_ui::{
    color,
    icon::{
        bitcoin_icon, chat_icon, graph_icon, home_icon, lightning_icon, plus_icon, receipt_icon,
        settings_icon, tooltip_icon, wallet_icon, wrench_icon,
    },
    theme,
    widget::{Button, Column, Element, Row},
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

/// Returns the tertiary-rail items for `menu`, or `None` when the route
/// has no third level (and the rail should be hidden).
pub fn items_for(menu: &Menu, _ctx: &NavContext) -> Option<Vec<SubItem>> {
    match menu {
        Menu::Home(HomeSubMenu::Settings(_)) => Some(home_settings_items()),
        Menu::Marketplace(MarketplaceSubMenu::P2P(_)) => Some(p2p_items()),
        Menu::Vault(VaultSubMenu::Settings(_)) => Some(vault_settings_items()),
        _ => None,
    }
}

pub fn rail<'a>(menu: &Menu, ctx: &NavContext<'a>) -> Option<Element<'a, Message>> {
    let items = items_for(menu, ctx)?;

    // Build the items column at its natural height so the tertiary
    // background only extends behind the items themselves. No trailing
    // fill — when used as an overlay the bottom of the rail needs to
    // stay click-through so content beneath it stays interactive.
    let items_height = items.len() as f32 * ITEM_HEIGHT;
    let mut items_col: Column<Message> = Column::new().spacing(0).width(Length::Fill);
    for item in items {
        items_col = items_col.push(item_row(menu, &item));
    }

    Some(
        container(items_col)
            .width(Length::Fixed(RAIL_WIDTH))
            .height(Length::Fixed(items_height))
            .style(theme::container::sidebar_tertiary)
            .into(),
    )
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
    // same treatment as the secondary rail.
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

fn home_settings_items() -> Vec<SubItem> {
    vec![
        SubItem {
            label: "General",
            icon: wrench_icon,
            route: Menu::Home(HomeSubMenu::Settings(HomeSettingsOption::General)),
            matches: |m| {
                matches!(
                    m,
                    Menu::Home(HomeSubMenu::Settings(HomeSettingsOption::General))
                )
            },
        },
        SubItem {
            label: "Lightning",
            icon: lightning_icon,
            route: Menu::Home(HomeSubMenu::Settings(HomeSettingsOption::Lightning)),
            matches: |m| {
                matches!(
                    m,
                    Menu::Home(HomeSubMenu::Settings(HomeSettingsOption::Lightning))
                )
            },
        },
        SubItem {
            label: "About",
            icon: tooltip_icon,
            route: Menu::Home(HomeSubMenu::Settings(HomeSettingsOption::About)),
            matches: |m| {
                matches!(
                    m,
                    Menu::Home(HomeSubMenu::Settings(HomeSettingsOption::About))
                )
            },
        },
        SubItem {
            label: "Stats",
            icon: graph_icon,
            route: Menu::Home(HomeSubMenu::Settings(HomeSettingsOption::Stats)),
            matches: |m| {
                matches!(
                    m,
                    Menu::Home(HomeSubMenu::Settings(HomeSettingsOption::Stats))
                )
            },
        },
    ]
}

fn p2p_items() -> Vec<SubItem> {
    vec![
        SubItem {
            label: "Book",
            icon: home_icon,
            route: Menu::Marketplace(MarketplaceSubMenu::P2P(P2PSubMenu::Overview)),
            matches: |m| {
                matches!(
                    m,
                    Menu::Marketplace(MarketplaceSubMenu::P2P(P2PSubMenu::Overview))
                )
            },
        },
        SubItem {
            label: "My Trades",
            icon: receipt_icon,
            route: Menu::Marketplace(MarketplaceSubMenu::P2P(P2PSubMenu::MyTrades)),
            matches: |m| {
                matches!(
                    m,
                    Menu::Marketplace(MarketplaceSubMenu::P2P(P2PSubMenu::MyTrades))
                )
            },
        },
        SubItem {
            label: "Chat",
            icon: chat_icon,
            route: Menu::Marketplace(MarketplaceSubMenu::P2P(P2PSubMenu::Chat)),
            matches: |m| {
                matches!(
                    m,
                    Menu::Marketplace(MarketplaceSubMenu::P2P(P2PSubMenu::Chat))
                )
            },
        },
        SubItem {
            label: "Create",
            icon: plus_icon,
            route: Menu::Marketplace(MarketplaceSubMenu::P2P(P2PSubMenu::CreateOrder)),
            matches: |m| {
                matches!(
                    m,
                    Menu::Marketplace(MarketplaceSubMenu::P2P(P2PSubMenu::CreateOrder))
                )
            },
        },
        SubItem {
            label: "Settings",
            icon: settings_icon,
            route: Menu::Marketplace(MarketplaceSubMenu::P2P(P2PSubMenu::Settings)),
            matches: |m| {
                matches!(
                    m,
                    Menu::Marketplace(MarketplaceSubMenu::P2P(P2PSubMenu::Settings))
                )
            },
        },
    ]
}

fn vault_settings_items() -> Vec<SubItem> {
    vec![
        SubItem {
            label: "Node",
            icon: bitcoin_icon,
            route: Menu::Vault(VaultSubMenu::Settings(Some(SettingsOption::Node))),
            matches: |m| {
                matches!(
                    m,
                    Menu::Vault(VaultSubMenu::Settings(Some(SettingsOption::Node)))
                        | Menu::Vault(VaultSubMenu::Settings(None))
                )
            },
        },
        SubItem {
            label: "Wallet",
            icon: wallet_icon,
            route: Menu::Vault(VaultSubMenu::Settings(Some(SettingsOption::Wallet))),
            matches: |m| {
                matches!(
                    m,
                    Menu::Vault(VaultSubMenu::Settings(Some(SettingsOption::Wallet)))
                )
            },
        },
        SubItem {
            label: "Import/Export",
            icon: wallet_icon,
            route: Menu::Vault(VaultSubMenu::Settings(Some(SettingsOption::ImportExport))),
            matches: |m| {
                matches!(
                    m,
                    Menu::Vault(VaultSubMenu::Settings(Some(SettingsOption::ImportExport)))
                )
            },
        },
        // About intentionally omitted — already reachable via
        // Cube → Settings → About, no need for a Vault duplicate.
    ]
}
