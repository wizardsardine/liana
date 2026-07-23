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

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(status: &'a crate::app::ConnectionStatus) -> NavContext<'a> {
        NavContext {
            has_vault: false,
            has_p2p: false,
            network: coincube_core::miniscript::bitcoin::Network::Bitcoin,
            p2p_test_coordinator: false,
            marketplace_flags: crate::app::features::MarketplaceServerFlags::OFF,
            liquid_gate: crate::app::features::LiquidGate::HIDDEN,
            cube_name: "test cube",
            lightning_address: None,
            avatar: None,
            theme_mode: coincube_ui::theme::palette::ThemeMode::default(),
            connect_authenticated: false,
            connect_stream_status: status,
        }
    }

    #[test]
    fn spark_secondary_nav_items_keep_expected_order_and_routes() {
        let status = crate::app::ConnectionStatus::default();
        let items = items(&ctx(&status));

        let labels: Vec<_> = items.iter().map(|item| item.label).collect();
        assert_eq!(
            labels,
            ["Overview", "Send", "Receive", "Transactions", "Settings"]
        );
        assert_eq!(items[0].route, Menu::Spark(SparkSubMenu::Overview));
        assert_eq!(items[1].route, Menu::Spark(SparkSubMenu::Send));
        assert_eq!(items[2].route, Menu::Spark(SparkSubMenu::Receive));
        assert_eq!(
            items[3].route,
            Menu::Spark(SparkSubMenu::Transactions(None))
        );
        assert_eq!(
            items[4].route,
            Menu::Spark(SparkSubMenu::Settings(Some(SparkSettingsOption::General)))
        );
    }

    #[test]
    fn spark_secondary_nav_matchers_group_payload_variants() {
        let status = crate::app::ConnectionStatus::default();
        let items = items(&ctx(&status));
        let txid = "0000000000000000000000000000000000000000000000000000000000000000"
            .parse()
            .unwrap();

        assert!((items[0].matches)(&Menu::Spark(SparkSubMenu::Overview)));
        assert!(!(items[0].matches)(&Menu::Spark(SparkSubMenu::Send)));

        assert!((items[3].matches)(&Menu::Spark(
            SparkSubMenu::Transactions(None)
        )));
        assert!((items[3].matches)(&Menu::Spark(
            SparkSubMenu::Transactions(Some(txid))
        )));
        assert!(!(items[3].matches)(&Menu::Spark(SparkSubMenu::Receive)));

        assert!((items[4].matches)(&Menu::Spark(SparkSubMenu::Settings(
            None
        ))));
        assert!((items[4].matches)(&Menu::Spark(SparkSubMenu::Settings(
            Some(SparkSettingsOption::LightningAddress)
        ))));
        assert!(!(items[4].matches)(&Menu::Cube(
            crate::app::menu::CubeSubMenu::Overview
        )));
    }
}
