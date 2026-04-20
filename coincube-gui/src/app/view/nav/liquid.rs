use super::{NavContext, SubItem};
use crate::app::menu::{LiquidSubMenu, Menu};
use coincube_ui::icon::{home_icon, receipt_icon, receive_icon, send_icon};

/// Secondary-rail items for the Liquid wallet section.
/// Settings is omitted for now — the panel currently has no content
/// worth surfacing in the rail.
pub fn items(_ctx: &NavContext) -> Vec<SubItem> {
    vec![
        SubItem {
            label: "Overview",
            icon: home_icon,
            route: Menu::Liquid(LiquidSubMenu::Overview),
            matches: |m| matches!(m, Menu::Liquid(LiquidSubMenu::Overview)),
        },
        SubItem {
            label: "Send",
            icon: send_icon,
            route: Menu::Liquid(LiquidSubMenu::Send),
            matches: |m| matches!(m, Menu::Liquid(LiquidSubMenu::Send)),
        },
        SubItem {
            label: "Receive",
            icon: receive_icon,
            route: Menu::Liquid(LiquidSubMenu::Receive),
            matches: |m| matches!(m, Menu::Liquid(LiquidSubMenu::Receive)),
        },
        SubItem {
            label: "Transactions",
            icon: receipt_icon,
            route: Menu::Liquid(LiquidSubMenu::Transactions(None)),
            matches: |m| matches!(m, Menu::Liquid(LiquidSubMenu::Transactions(_))),
        },
    ]
}
