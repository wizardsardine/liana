use super::{NavContext, SubItem};
use crate::app::menu::{Menu, SparkSettingsOption, SparkSubMenu};
use coincube_ui::icon::{home_icon, receipt_icon, receive_icon, send_icon, settings_icon};

/// Secondary-rail items for the Spark wallet section.
pub fn items(_ctx: &NavContext) -> Vec<SubItem> {
    vec![
        SubItem::new(
            "Overview",
            home_icon,
            Menu::Spark(SparkSubMenu::Overview),
            |m| matches!(m, Menu::Spark(SparkSubMenu::Overview)),
        ),
        SubItem::new("Send", send_icon, Menu::Spark(SparkSubMenu::Send), |m| {
            matches!(m, Menu::Spark(SparkSubMenu::Send))
        }),
        SubItem::new(
            "Receive",
            receive_icon,
            Menu::Spark(SparkSubMenu::Receive),
            |m| matches!(m, Menu::Spark(SparkSubMenu::Receive)),
        ),
        SubItem::new(
            "Transactions",
            receipt_icon,
            Menu::Spark(SparkSubMenu::Transactions(None)),
            |m| matches!(m, Menu::Spark(SparkSubMenu::Transactions(_))),
        ),
        SubItem::new(
            "Settings",
            settings_icon,
            Menu::Spark(SparkSubMenu::Settings(Some(SparkSettingsOption::General))),
            |m| matches!(m, Menu::Spark(SparkSubMenu::Settings(_))),
        ),
    ]
}
