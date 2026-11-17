use super::{NavContext, SubItem};
use crate::app::menu::{Menu, VaultSubMenu};
use coincube_ui::icon::{
    claim_icon, coins_outline_icon, home_icon, receipt_icon, receive_icon, recovery_icon,
    send_icon, settings_icon,
};
use coincube_ui::locale::copy as localized_copy;

/// Secondary-rail items for the Vault wallet section.
pub fn items(ctx: &NavContext) -> Vec<SubItem> {
    let mut items = vec![
        SubItem::new(
            "Overview",
            home_icon,
            Menu::Vault(VaultSubMenu::Overview),
            |m| matches!(m, Menu::Vault(VaultSubMenu::Overview)),
        ),
        SubItem::new("Send", send_icon, Menu::Vault(VaultSubMenu::Send), |m| {
            matches!(m, Menu::Vault(VaultSubMenu::Send))
        }),
        SubItem::new(
            "Receive",
            receive_icon,
            Menu::Vault(VaultSubMenu::Receive),
            |m| matches!(m, Menu::Vault(VaultSubMenu::Receive)),
        ),
        SubItem::new(
            "Coins",
            coins_outline_icon,
            Menu::Vault(VaultSubMenu::Coins(None)),
            |m| matches!(m, Menu::Vault(VaultSubMenu::Coins(_))),
        ),
        SubItem::new(
            "Transactions",
            receipt_icon,
            Menu::Vault(VaultSubMenu::Transactions(None)),
            |m| matches!(m, Menu::Vault(VaultSubMenu::Transactions(_))),
        ),
        SubItem::new(
            "PSBTs",
            receipt_icon,
            Menu::Vault(VaultSubMenu::PSBTs(None)),
            |m| matches!(m, Menu::Vault(VaultSubMenu::PSBTs(_))),
        ),
        SubItem::new(
            "Settings",
            settings_icon,
            Menu::Vault(VaultSubMenu::Settings(Some(
                crate::app::menu::SettingsOption::Wallet,
            ))),
            |m| matches!(m, Menu::Vault(VaultSubMenu::Settings(_))),
        ),
    ];
    // Imported descriptors retain the original wallet's spending policy and
    // signers. The Vault inheritance recovery route cannot operate on them.
    if !ctx.has_imported_bitcoin {
        let settings_at = items.len().saturating_sub(1);
        items.insert(
            settings_at,
            SubItem::new(
                "Recovery",
                recovery_icon,
                Menu::Vault(VaultSubMenu::Recovery),
                |m| matches!(m, Menu::Vault(VaultSubMenu::Recovery)),
            ),
        );
    }
    // Split coins checklist (Lane B1.5): only while Bitcoin Blake2b is on for
    // this Vault, like Coin control's Chains column.
    let blake2b_on = ctx.network.is_blake2b()
        || (ctx.network == crate::chain::ChainId::Bitcoin && ctx.btcb2_server_enabled);
    if blake2b_on && ctx.has_vault && !ctx.has_imported_bitcoin {
        let settings_at = items.len().saturating_sub(1);
        items.insert(
            settings_at,
            SubItem::new(
                "Split coins",
                claim_icon,
                Menu::Vault(VaultSubMenu::SplitCoins),
                |m| matches!(m, Menu::Vault(VaultSubMenu::SplitCoins)),
            ),
        );
    }
    items
}

/// The Vault's tools, which the canvas reaches from the Vault page rather
/// than the sidebar: coin control, PSBT proposals, recovery path and split
/// coins, each only where [`items`] offers its route.
pub fn tools(ctx: &NavContext) -> Vec<(&'static str, Menu)> {
    items(ctx)
        .into_iter()
        .filter_map(|item| {
            let label = match &item.route {
                Menu::Vault(VaultSubMenu::Coins(_)) => localized_copy("Coin control"),
                Menu::Vault(VaultSubMenu::PSBTs(_)) => localized_copy("PSBT proposals"),
                Menu::Vault(VaultSubMenu::Recovery) => localized_copy("Recovery path"),
                Menu::Vault(VaultSubMenu::SplitCoins) => localized_copy("Split coins"),
                _ => return None,
            };
            Some((label, item.route))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(
        status: &'a crate::app::ConnectionStatus,
        network: crate::chain::ChainId,
        btcb2_server_enabled: bool,
        btcb2_already_claimed: bool,
    ) -> NavContext<'a> {
        NavContext {
            has_vault: true,
            has_imported_bitcoin: false,
            has_p2p: false,
            network,
            p2p_test_coordinator: false,
            marketplace_flags: crate::app::features::MarketplaceServerFlags::OFF,
            cube_name: "test cube",
            spark_alias: None,
            vault_alias: None,
            lightning_address: None,
            avatar: None,
            theme_mode: coincube_ui::theme::palette::ThemeMode::default(),
            connect_authenticated: btcb2_server_enabled,
            connect_email: None,
            connect_stream_status: status,
            connect: Default::default(),
            btcb2_server_enabled,
            btcb2_already_claimed,
        }
    }

    fn labels(items: &[SubItem]) -> Vec<&str> {
        items.iter().map(|i| i.label).collect()
    }

    #[test]
    fn vault_page_tools_follow_the_routes_the_vault_offers() {
        let status = crate::app::ConnectionStatus::default();
        let vault = ctx(&status, crate::chain::ChainId::Bitcoin, false, false);
        let names = |context: &NavContext| -> Vec<&'static str> {
            tools(context).into_iter().map(|(label, _)| label).collect()
        };
        assert_eq!(
            names(&vault),
            ["Coin control", "PSBT proposals", "Recovery path"]
        );
        assert_eq!(
            tools(&vault)[0].1,
            Menu::Vault(VaultSubMenu::Coins(None)),
            "Coin control opens the coins route"
        );
        // Imported descriptors cannot use the inheritance recovery route.
        let imported = NavContext {
            has_vault: false,
            has_imported_bitcoin: true,
            ..vault
        };
        assert!(!names(&imported).contains(&"Recovery path"));
        // Never Send, Receive, Transactions or Settings: those are not tools.
        for (_, route) in tools(&vault) {
            assert!(matches!(
                route,
                Menu::Vault(
                    VaultSubMenu::Coins(_)
                        | VaultSubMenu::PSBTs(_)
                        | VaultSubMenu::Recovery
                        | VaultSubMenu::SplitCoins
                )
            ));
        }
    }

    #[test]
    fn vault_settings_enters_wallet_first_and_keeps_pair_available() {
        let status = crate::app::ConnectionStatus::default();
        let context = ctx(&status, crate::chain::ChainId::Bitcoin, false, false);
        let nav = items(&context);
        assert!(matches!(
            &nav.last().unwrap().route,
            Menu::Vault(VaultSubMenu::Settings(Some(
                crate::app::menu::SettingsOption::Wallet
            )))
        ));
        let sections = crate::app::view::nav::tertiary::items_for(
            &Menu::Vault(VaultSubMenu::Settings(None)),
            &context,
        )
        .unwrap();
        assert_eq!(
            labels(&sections),
            ["Wallet & signers", "Node", "Mobile", "Data & exports"]
        );
        assert!(sections[2].disabled_reason.is_none());
        assert!((sections[0].matches)(&Menu::Vault(VaultSubMenu::Settings(
            None
        ))));
    }

    #[test]
    fn imported_wallet_has_no_inheritance_or_split_actions() {
        let status = crate::app::ConnectionStatus::default();
        let mut context = ctx(&status, crate::chain::ChainId::Bitcoin, true, false);
        context.has_vault = false;
        context.has_imported_bitcoin = true;
        assert_eq!(
            labels(&items(&context)),
            [
                "Overview",
                "Send",
                "Receive",
                "Coins",
                "Transactions",
                "PSBTs",
                "Settings"
            ]
        );
        assert!(matches!(
            &items(&context).last().unwrap().route,
            Menu::Vault(VaultSubMenu::Settings(Some(
                crate::app::menu::SettingsOption::Wallet
            )))
        ));
        let settings = crate::app::view::nav::tertiary::items_for(
            &Menu::Vault(VaultSubMenu::Settings(None)),
            &context,
        )
        .unwrap();
        assert_eq!(
            labels(&settings),
            ["Wallet & signers", "Node", "Mobile", "Data & exports"]
        );
        assert!(settings[2].disabled_reason.is_some());
        context.network = crate::chain::ChainId::BitcoinBlake2b;
        assert!(!labels(&items(&context)).contains(&"Split coins"));
    }

    /// With the account grant Split coins appears, immediately before
    /// Settings — and without it the rail is byte-for-byte what it was.
    #[test]
    fn split_coins_appears_only_with_the_account_grant_and_keeps_settings_last() {
        let status = crate::app::ConnectionStatus::default();
        let granted = items(&ctx(&status, crate::chain::ChainId::Bitcoin, true, false));
        assert_eq!(
            labels(&granted),
            [
                "Overview",
                "Send",
                "Receive",
                "Coins",
                "Transactions",
                "PSBTs",
                "Recovery",
                "Split coins",
                "Settings",
            ]
        );

        // Flag off: no BTCB2 surface anywhere on the rail.
        let ungranted = items(&ctx(&status, crate::chain::ChainId::Bitcoin, false, false));
        assert_eq!(
            labels(&ungranted),
            [
                "Overview",
                "Send",
                "Receive",
                "Coins",
                "Transactions",
                "PSBTs",
                "Recovery",
                "Settings",
            ]
        );

        // A copy already exists: the hub still shows (it checks it off).
        assert_eq!(
            labels(&items(&ctx(
                &status,
                crate::chain::ChainId::Bitcoin,
                true,
                true
            ))),
            labels(&granted)
        );
        // A Blake2b Cube: its own Split coins checklist.
        assert_eq!(
            labels(&items(&ctx(
                &status,
                crate::chain::ChainId::BitcoinBlake2b,
                true,
                false
            ))),
            [
                "Overview",
                "Send",
                "Receive",
                "Coins",
                "Transactions",
                "PSBTs",
                "Recovery",
                "Split coins",
                "Settings",
            ]
        );
    }
}
