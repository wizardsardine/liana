use super::{NavContext, SubItem};
use crate::app::menu::{Menu, VaultSubMenu};
use coincube_ui::icon::{
    coins_outline_icon, home_icon, receipt_icon, receive_icon, recovery_icon, send_icon,
    settings_icon,
};

/// Secondary-rail items for the Vault wallet section.
pub fn items(_ctx: &NavContext) -> Vec<SubItem> {
    vec![
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
            "Recovery",
            recovery_icon,
            Menu::Vault(VaultSubMenu::Recovery),
            |m| matches!(m, Menu::Vault(VaultSubMenu::Recovery)),
        ),
        SubItem::new(
            "Settings",
            settings_icon,
            Menu::Vault(VaultSubMenu::Settings(Some(
                crate::app::menu::SettingsOption::Node,
            ))),
            |m| matches!(m, Menu::Vault(VaultSubMenu::Settings(_))),
        ),
    ]
}
