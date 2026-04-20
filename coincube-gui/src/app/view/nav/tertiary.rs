//! Tertiary (~72px) left nav rail — the third column that appears only
//! when the current route has a third level (e.g. Cube → Settings →
//! {General/Lightning/About}, Marketplace → P2P → {Order Book / My Trades
//! / ...}, Vault → Settings → {Node / Wallet / Import-Export}).
//!
//! Styled like [`super::primary`] and [`super::secondary`], but with a
//! slightly lighter background so the third level reads as "deeper".
//! Renders nothing when the active route has no third level — callers
//! check [`items_for`] for that and omit the rail entirely.

use super::items::{render_item_row, RAIL_ITEM_HEIGHT};
use super::{NavContext, SubItem};
use crate::app::{
    menu::{
        HomeSettingsOption, HomeSubMenu, MarketplaceSubMenu, Menu, P2PSubMenu, SettingsOption,
        VaultSubMenu,
    },
    view::Message,
};
use coincube_ui::{
    icon::{
        bitcoin_icon, chat_icon, graph_icon, home_icon, lightning_icon, plus_icon, receipt_icon,
        settings_icon, tooltip_icon, wallet_icon, wrench_icon,
    },
    theme,
    widget::{Column, Element},
};
use iced::{widget::container, Length};

pub const RAIL_WIDTH: f32 = 72.0;

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
    let items_height = items.len() as f32 * RAIL_ITEM_HEIGHT;
    let mut items_col: Column<Message> = Column::new().spacing(0).width(Length::Fill);
    for item in items {
        items_col = items_col.push(render_item_row(menu, &item, RAIL_WIDTH));
    }

    Some(
        container(items_col)
            .width(Length::Fixed(RAIL_WIDTH))
            .height(Length::Fixed(items_height))
            .style(theme::container::sidebar_tertiary)
            .into(),
    )
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
    ]
}
