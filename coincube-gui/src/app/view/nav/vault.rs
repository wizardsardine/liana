use super::{NavContext, SubItem};
use crate::app::menu::{Menu, VaultSubMenu};
use coincube_ui::icon::{
    coins_icon, home_icon, receipt_icon, receive_icon, recovery_icon, send_icon, settings_icon,
};

/// Secondary-rail items for the Vault wallet section.
pub fn items(_ctx: &NavContext) -> Vec<SubItem> {
    vec![
        SubItem {
            label: "Overview",
            icon: home_icon,
            route: Menu::Vault(VaultSubMenu::Overview),
            matches: |m| matches!(m, Menu::Vault(VaultSubMenu::Overview)),
        },
        SubItem {
            label: "Send",
            icon: send_icon,
            route: Menu::Vault(VaultSubMenu::Send),
            matches: |m| matches!(m, Menu::Vault(VaultSubMenu::Send)),
        },
        SubItem {
            label: "Receive",
            icon: receive_icon,
            route: Menu::Vault(VaultSubMenu::Receive),
            matches: |m| matches!(m, Menu::Vault(VaultSubMenu::Receive)),
        },
        SubItem {
            label: "Coins",
            icon: coins_icon,
            route: Menu::Vault(VaultSubMenu::Coins(None)),
            matches: |m| matches!(m, Menu::Vault(VaultSubMenu::Coins(_))),
        },
        SubItem {
            label: "Transactions",
            icon: receipt_icon,
            route: Menu::Vault(VaultSubMenu::Transactions(None)),
            matches: |m| matches!(m, Menu::Vault(VaultSubMenu::Transactions(_))),
        },
        SubItem {
            label: "PSBTs",
            icon: receipt_icon,
            route: Menu::Vault(VaultSubMenu::PSBTs(None)),
            matches: |m| matches!(m, Menu::Vault(VaultSubMenu::PSBTs(_))),
        },
        SubItem {
            label: "Recovery",
            icon: recovery_icon,
            route: Menu::Vault(VaultSubMenu::Recovery),
            matches: |m| matches!(m, Menu::Vault(VaultSubMenu::Recovery)),
        },
        SubItem {
            label: "Settings",
            icon: settings_icon,
            route: Menu::Vault(VaultSubMenu::Settings(Some(
                crate::app::menu::SettingsOption::Node,
            ))),
            matches: |m| matches!(m, Menu::Vault(VaultSubMenu::Settings(_))),
        },
    ]
}
