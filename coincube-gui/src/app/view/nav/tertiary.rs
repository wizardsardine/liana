//! Tertiary (~72px) left nav rail — the third column that appears only
//! when the current route has a third level (e.g. Cube → Settings →
//! {General/About/Stats}, Marketplace → P2P → {Order Book / My Trades
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
        CubeSettingsOption, CubeSubMenu, MarketplaceSubMenu, Menu, P2PSubMenu, SettingsOption,
        SparkSettingsOption, SparkSubMenu, VaultSubMenu,
    },
    view::Message,
};
use coincube_ui::{
    icon::{
        bitcoin_icon, chat_icon, coins_outline_icon, graph_icon, home_icon, lightning_outline_icon,
        person_icon, phone_icon, plus_icon, receipt_icon, settings_icon, tooltip_icon, wallet_icon,
        wrench_icon,
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
        Menu::Cube(CubeSubMenu::Settings(_)) => Some(cube_settings_items()),
        Menu::Spark(SparkSubMenu::Settings(_)) => Some(spark_settings_items()),
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

fn cube_settings_items() -> Vec<SubItem> {
    let mut items = vec![
        SubItem::new(
            "General",
            wrench_icon,
            Menu::Cube(CubeSubMenu::Settings(CubeSettingsOption::General)),
            |m| {
                matches!(
                    m,
                    Menu::Cube(CubeSubMenu::Settings(CubeSettingsOption::General))
                )
            },
        ),
        SubItem::new(
            "About",
            tooltip_icon,
            Menu::Cube(CubeSubMenu::Settings(CubeSettingsOption::About)),
            |m| {
                matches!(
                    m,
                    Menu::Cube(CubeSubMenu::Settings(CubeSettingsOption::About))
                )
            },
        ),
        SubItem::new(
            "Stats",
            graph_icon,
            Menu::Cube(CubeSubMenu::Settings(CubeSettingsOption::Stats)),
            |m| {
                matches!(
                    m,
                    Menu::Cube(CubeSubMenu::Settings(CubeSettingsOption::Stats))
                )
            },
        ),
        // Placeholder icon — the per-Cube Connect rail used a stacked-coins
        // glyph; swap to a face icon when one exists.
        SubItem::new(
            "Avatar",
            coins_outline_icon,
            Menu::Cube(CubeSubMenu::Settings(CubeSettingsOption::Avatar)),
            |m| {
                matches!(
                    m,
                    Menu::Cube(CubeSubMenu::Settings(CubeSettingsOption::Avatar))
                )
            },
        ),
    ];

    if crate::feature_flags::CUBE_MEMBERS_UI_ENABLED {
        items.push(SubItem::new(
            "Members",
            person_icon,
            Menu::Cube(CubeSubMenu::Settings(CubeSettingsOption::Members)),
            |m| {
                matches!(
                    m,
                    Menu::Cube(CubeSubMenu::Settings(CubeSettingsOption::Members))
                )
            },
        ));
    }

    items
}

fn spark_settings_items() -> Vec<SubItem> {
    vec![
        // `None` is treated as General by `set_current_panel`, so the
        // landing route (Settings(None) from a deep link) highlights here.
        SubItem::new(
            "General",
            wrench_icon,
            Menu::Spark(SparkSubMenu::Settings(Some(SparkSettingsOption::General))),
            |m| {
                matches!(
                    m,
                    Menu::Spark(SparkSubMenu::Settings(
                        Some(SparkSettingsOption::General) | None
                    ))
                )
            },
        ),
        SubItem::new(
            "Lightning Address",
            lightning_outline_icon,
            Menu::Spark(SparkSubMenu::Settings(Some(
                SparkSettingsOption::LightningAddress,
            ))),
            |m| {
                matches!(
                    m,
                    Menu::Spark(SparkSubMenu::Settings(Some(
                        SparkSettingsOption::LightningAddress
                    )))
                )
            },
        ),
    ]
}

fn p2p_items() -> Vec<SubItem> {
    vec![
        SubItem::new(
            "Book",
            home_icon,
            Menu::Marketplace(MarketplaceSubMenu::P2P(P2PSubMenu::Overview)),
            |m| {
                matches!(
                    m,
                    Menu::Marketplace(MarketplaceSubMenu::P2P(P2PSubMenu::Overview))
                )
            },
        ),
        SubItem::new(
            "My Trades",
            receipt_icon,
            Menu::Marketplace(MarketplaceSubMenu::P2P(P2PSubMenu::MyTrades)),
            |m| {
                matches!(
                    m,
                    Menu::Marketplace(MarketplaceSubMenu::P2P(P2PSubMenu::MyTrades))
                )
            },
        ),
        SubItem::new(
            "Chat",
            chat_icon,
            Menu::Marketplace(MarketplaceSubMenu::P2P(P2PSubMenu::Chat)),
            |m| {
                matches!(
                    m,
                    Menu::Marketplace(MarketplaceSubMenu::P2P(P2PSubMenu::Chat))
                )
            },
        ),
        SubItem::new(
            "Create",
            plus_icon,
            Menu::Marketplace(MarketplaceSubMenu::P2P(P2PSubMenu::CreateOrder)),
            |m| {
                matches!(
                    m,
                    Menu::Marketplace(MarketplaceSubMenu::P2P(P2PSubMenu::CreateOrder))
                )
            },
        ),
        SubItem::new(
            "Settings",
            settings_icon,
            Menu::Marketplace(MarketplaceSubMenu::P2P(P2PSubMenu::Settings)),
            |m| {
                matches!(
                    m,
                    Menu::Marketplace(MarketplaceSubMenu::P2P(P2PSubMenu::Settings))
                )
            },
        ),
    ]
}

fn vault_settings_items() -> Vec<SubItem> {
    vec![
        SubItem::new(
            "Node",
            bitcoin_icon,
            Menu::Vault(VaultSubMenu::Settings(Some(SettingsOption::Node))),
            |m| {
                matches!(
                    m,
                    Menu::Vault(VaultSubMenu::Settings(Some(SettingsOption::Node)))
                        | Menu::Vault(VaultSubMenu::Settings(None))
                )
            },
        ),
        SubItem::new(
            "Wallet",
            wallet_icon,
            Menu::Vault(VaultSubMenu::Settings(Some(SettingsOption::Wallet))),
            |m| {
                matches!(
                    m,
                    Menu::Vault(VaultSubMenu::Settings(Some(SettingsOption::Wallet)))
                )
            },
        ),
        SubItem::new(
            "Import/Export",
            wallet_icon,
            Menu::Vault(VaultSubMenu::Settings(Some(SettingsOption::ImportExport))),
            |m| {
                matches!(
                    m,
                    Menu::Vault(VaultSubMenu::Settings(Some(SettingsOption::ImportExport)))
                )
            },
        ),
        SubItem::new(
            "Pair",
            phone_icon,
            Menu::Vault(VaultSubMenu::Settings(Some(SettingsOption::LocalSigning))),
            |m| {
                matches!(
                    m,
                    Menu::Vault(VaultSubMenu::Settings(Some(SettingsOption::LocalSigning)))
                )
            },
        ),
    ]
}
