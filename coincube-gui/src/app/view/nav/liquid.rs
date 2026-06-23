use super::{NavContext, SubItem};
use crate::app::menu::{LiquidSubMenu, Menu};
use crate::app::state::liquid::swap::swap_supported;
use coincube_ui::icon::{arrow_down_up_icon, home_icon, receipt_icon, receive_icon, send_icon};

/// Secondary-rail items for the Liquid wallet section.
/// Settings is omitted for now — the panel currently has no content
/// worth surfacing in the rail.
pub fn items(ctx: &NavContext) -> Vec<SubItem> {
    let mut items = vec![
        SubItem::new(
            "Overview",
            home_icon,
            Menu::Liquid(LiquidSubMenu::Overview),
            |m| matches!(m, Menu::Liquid(LiquidSubMenu::Overview)),
        ),
        SubItem::new("Send", send_icon, Menu::Liquid(LiquidSubMenu::Send), |m| {
            matches!(m, Menu::Liquid(LiquidSubMenu::Send))
        }),
    ];

    // Cross-asset swaps go through SideSwap, which is mainnet-only, so the
    // Swap rail item is hidden off-mainnet (same gate as the Overview
    // entry point and the quote engine).
    if swap_supported(ctx.network) {
        items.push(SubItem::new(
            "Swap",
            arrow_down_up_icon,
            Menu::Liquid(LiquidSubMenu::Swap),
            |m| matches!(m, Menu::Liquid(LiquidSubMenu::Swap)),
        ));
    }

    items.push(SubItem::new(
        "Receive",
        receive_icon,
        Menu::Liquid(LiquidSubMenu::Receive),
        |m| matches!(m, Menu::Liquid(LiquidSubMenu::Receive)),
    ));
    items.push(SubItem::new(
        "Transactions",
        receipt_icon,
        Menu::Liquid(LiquidSubMenu::Transactions(None)),
        |m| matches!(m, Menu::Liquid(LiquidSubMenu::Transactions(_))),
    ));

    items
}
