use super::{NavContext, SubItem};
use crate::app::menu::{LiquidSubMenu, Menu};
use coincube_ui::icon::{home_icon, receipt_icon, receive_icon, send_icon};

/// Secondary-rail items for the Liquid wallet section.
/// Settings is omitted for now — the panel currently has no content
/// worth surfacing in the rail.
pub fn items(_ctx: &NavContext) -> Vec<SubItem> {
    vec![
        SubItem::new(
            "Overview",
            home_icon,
            Menu::Liquid(LiquidSubMenu::Overview),
            |m| matches!(m, Menu::Liquid(LiquidSubMenu::Overview)),
        ),
        SubItem::new("Send", send_icon, Menu::Liquid(LiquidSubMenu::Send), |m| {
            matches!(m, Menu::Liquid(LiquidSubMenu::Send))
        }),
        SubItem::new(
            "Receive",
            receive_icon,
            Menu::Liquid(LiquidSubMenu::Receive),
            |m| matches!(m, Menu::Liquid(LiquidSubMenu::Receive)),
        ),
        SubItem::new(
            "Transactions",
            receipt_icon,
            Menu::Liquid(LiquidSubMenu::Transactions(None)),
            |m| matches!(m, Menu::Liquid(LiquidSubMenu::Transactions(_))),
        ),
    ]
}
