use super::{NavContext, SubItem};
use crate::app::menu::{Menu, SparkSubMenu};
use coincube_ui::icon::{home_icon, receipt_icon, receive_icon, send_icon, settings_icon};

/// Secondary-rail items for the Spark wallet section.
pub fn items(_ctx: &NavContext) -> Vec<SubItem> {
    vec![
        SubItem {
            label: "Overview",
            icon: home_icon,
            route: Menu::Spark(SparkSubMenu::Overview),
            matches: |m| matches!(m, Menu::Spark(SparkSubMenu::Overview)),
        },
        SubItem {
            label: "Send",
            icon: send_icon,
            route: Menu::Spark(SparkSubMenu::Send),
            matches: |m| matches!(m, Menu::Spark(SparkSubMenu::Send)),
        },
        SubItem {
            label: "Receive",
            icon: receive_icon,
            route: Menu::Spark(SparkSubMenu::Receive),
            matches: |m| matches!(m, Menu::Spark(SparkSubMenu::Receive)),
        },
        SubItem {
            label: "Transactions",
            icon: receipt_icon,
            route: Menu::Spark(SparkSubMenu::Transactions(None)),
            matches: |m| matches!(m, Menu::Spark(SparkSubMenu::Transactions(_))),
        },
        SubItem {
            label: "Settings",
            icon: settings_icon,
            route: Menu::Spark(SparkSubMenu::Settings(None)),
            matches: |m| matches!(m, Menu::Spark(SparkSubMenu::Settings(_))),
        },
    ]
}
