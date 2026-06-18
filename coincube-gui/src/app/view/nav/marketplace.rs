use super::{NavContext, SubItem};
use crate::app::features;
use crate::app::menu::{MarketplaceSubMenu, Menu, P2PSubMenu};
use coincube_ui::icon::{bitcoin_icon, person_icon};

/// Secondary-rail items for the Marketplace section. Buy/Sell is only
/// shown when a vault is present (KYC flow requires on-chain send);
/// P2P is only shown when `has_p2p` is set. The P2P sub-leaves
/// (Overview / My Trades / Chat / Create Order / Settings) are NOT in
/// the rail — they render as a tab bar inside the P2P content panel
/// (plan §12, decision Q1-B).
///
/// On top of those structural gates, each item carries a network gate
/// (via [`crate::app::features`]): rather than being hidden on networks
/// where the feature has no backend, it stays in the rail but renders
/// greyed out with an explanatory popover.
pub fn items(ctx: &NavContext) -> Vec<SubItem> {
    let mut items = Vec::new();

    if ctx.has_p2p {
        items.push(
            SubItem::new(
                "P2P",
                person_icon,
                Menu::Marketplace(MarketplaceSubMenu::P2P(P2PSubMenu::Overview)),
                |m| matches!(m, Menu::Marketplace(MarketplaceSubMenu::P2P(_))),
            )
            .disabled(
                features::p2p(ctx.network, ctx.p2p_test_coordinator)
                    .reason()
                    .map(str::to_string),
            ),
        );
    }

    // Buy/Sell requires a constructed `BuySellPanel`, which in turn
    // requires a vault wallet. Until that panel can render a "needs
    // vault" placeholder without a wallet, keep it gated here so
    // users aren't sent to an orphan route.
    if ctx.has_vault {
        items.push(
            SubItem::new(
                "Buy/Sell",
                bitcoin_icon,
                Menu::Marketplace(MarketplaceSubMenu::BuySell),
                |m| matches!(m, Menu::Marketplace(MarketplaceSubMenu::BuySell)),
            )
            .disabled(features::buy_sell(ctx.network).reason().map(str::to_string)),
        );
    }

    items
}
