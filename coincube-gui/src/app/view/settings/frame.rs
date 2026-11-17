//! Route-derived Settings navigation, all in the pinned head (`H-Settings`):
//! the scope switch among the head's actions and the section tabs on its
//! rule.

use coincube_ui::component::tenshu::{segment, segmented, tabs, PageHead, SegmentSize};
use coincube_ui::icon;
use coincube_ui::locale::copy as localized_copy;

use crate::app::cache::Cache;
use crate::app::features;
use crate::app::menu::{
    CubeSettingsOption, CubeSubMenu, Menu, SettingsOption, SparkSettingsOption, SparkSubMenu,
    VaultSubMenu,
};
use crate::app::view::message::Message;
use crate::app::view::nav::shell::wallet_label;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RouteScope {
    Cube,
    Vault,
    Spark,
}

impl RouteScope {
    fn id(self) -> &'static str {
        match self {
            RouteScope::Cube => "tenshu.settings.route.scope.cube",
            RouteScope::Vault => "tenshu.settings.route.scope.vault",
            RouteScope::Spark => "tenshu.settings.route.scope.spark",
        }
    }

    fn label(self) -> &'static str {
        match self {
            RouteScope::Cube => "Cube",
            RouteScope::Vault => "Vault",
            RouteScope::Spark => "Spark",
        }
    }

    fn icon(self) -> coincube_ui::widget::Text<'static> {
        match self {
            RouteScope::Cube => icon::cube_icon(),
            RouteScope::Vault => icon::vault_icon(),
            RouteScope::Spark => icon::spark_icon(),
        }
    }

    /// The scope's first tab.
    fn landing(self) -> Menu {
        match self {
            RouteScope::Cube => Menu::Cube(CubeSubMenu::Settings(CubeSettingsOption::General)),
            RouteScope::Vault => Menu::Vault(VaultSubMenu::Settings(Some(SettingsOption::Wallet))),
            RouteScope::Spark => {
                Menu::Spark(SparkSubMenu::Settings(Some(SparkSettingsOption::General)))
            }
        }
    }
}

fn route_scope(menu: &Menu) -> Option<RouteScope> {
    match menu {
        Menu::Cube(CubeSubMenu::Settings(_)) => Some(RouteScope::Cube),
        Menu::Vault(VaultSubMenu::Settings(_)) => Some(RouteScope::Vault),
        Menu::Spark(SparkSubMenu::Settings(_)) => Some(RouteScope::Spark),
        _ => None,
    }
}

/// The scopes this Cube has, in the switch's order.
fn scopes(cache: &Cache) -> Vec<RouteScope> {
    let mut scopes = vec![RouteScope::Cube];
    if cache.has_vault || cache.has_imported_bitcoin {
        scopes.push(RouteScope::Vault);
    }
    if features::spark(cache.chain()).is_available() {
        scopes.push(RouteScope::Spark);
    }
    scopes
}

#[derive(Clone)]
struct RouteChoice {
    id: &'static str,
    label: String,
    route: Menu,
    selected: bool,
    enabled: bool,
}

/// Where choosing `target` in the scope switch goes from `menu`: the tab of
/// the same name when `target` has one (Data & exports is in Cube and Vault,
/// Connections in all three), else `target`'s first tab. Choosing the
/// current scope stays on the current tab.
fn scope_route(menu: &Menu, target: RouteScope, cache: &Cache) -> Menu {
    if route_scope(menu) == Some(target) {
        return menu.clone();
    }
    let current = section_routes(menu, cache)
        .into_iter()
        .find(|tab| tab.selected)
        .map(|tab| tab.label);
    let landing = target.landing();
    current
        .and_then(|label| {
            section_routes(&landing, cache)
                .into_iter()
                .find(|tab| tab.label == label && tab.enabled)
        })
        .map(|tab| tab.route)
        .unwrap_or(landing)
}

fn section_routes(menu: &Menu, cache: &Cache) -> Vec<RouteChoice> {
    match route_scope(menu) {
        Some(RouteScope::Cube) => {
            let mut sections = vec![
                (CubeSettingsOption::General, "Display & appearance", "display"),
                (CubeSettingsOption::Recovery, "Backup & protection", "backup"),
                (CubeSettingsOption::PaymentChecks, "Payment checks", "payment-checks"),
                (CubeSettingsOption::AiAccess, "AI access", "ai-access"),
                (CubeSettingsOption::Connections, "Connections", "connections"),
                (CubeSettingsOption::DataExports, "Data & exports", "data"),
            ];
            if crate::feature_flags::CUBE_MEMBERS_UI_ENABLED {
                sections.push((CubeSettingsOption::Members, "Members", "members"));
            }
            // Download stats and About moved to App preferences. Their routes
            // stay reachable, so show a tab for one only while it is open.
            match menu {
                Menu::Cube(CubeSubMenu::Settings(CubeSettingsOption::Stats)) => {
                    sections.push((CubeSettingsOption::Stats, "Download stats", "stats"))
                }
                Menu::Cube(CubeSubMenu::Settings(CubeSettingsOption::About)) => {
                    sections.push((CubeSettingsOption::About, "About", "about"))
                }
                _ => {}
            }
            sections
                .into_iter()
                .map(|(section, label, id)| RouteChoice {
                    id: match id {
                        "display" => "tenshu.settings.route.cube.display",
                        "backup" => "tenshu.settings.route.cube.backup",
                        "payment-checks" => "tenshu.settings.route.cube.payment-checks",
                        "ai-access" => "tenshu.settings.route.cube.ai-access",
                        "connections" => "tenshu.settings.route.cube.connections",
                        "data" => "tenshu.settings.route.cube.data",
                        "stats" => "tenshu.settings.route.cube.stats",
                        "about" => "tenshu.settings.route.cube.about",
                        _ => "tenshu.settings.route.cube.members",
                    },
                    label: label.to_owned(),
                    route: Menu::Cube(CubeSubMenu::Settings(section)),
                    // The avatar flow belongs to Cube identity on Display.
                    selected: matches!(menu, Menu::Cube(CubeSubMenu::Settings(current))
                        if *current == section
                            || (*current == CubeSettingsOption::Avatar
                                && section == CubeSettingsOption::General)),
                    enabled: true,
                })
                .collect()
        }
        Some(RouteScope::Vault) => [
            (SettingsOption::Wallet, "Wallet & signers", "wallet"),
            (SettingsOption::Node, "Node", "node"),
            (SettingsOption::LocalSigning, "Mobile", "pair"),
            (SettingsOption::Connections, "Connections", "connections"),
            (SettingsOption::ImportExport, "Data & exports", "data"),
        ]
        .iter()
        .copied()
        // A Vault without a Keychain key can never pair a phone, so it has
        // no Keychain tab, except while that tab is the one open.
        .filter(|(section, _, _)| {
            *section != SettingsOption::LocalSigning
                || !cache.vault_lacks_keychain_key
                || matches!(
                    menu,
                    Menu::Vault(VaultSubMenu::Settings(Some(SettingsOption::LocalSigning)))
                )
        })
        .map(|(section, label, id)| RouteChoice {
            id: match id {
                "wallet" => "tenshu.settings.route.vault.wallet",
                "node" => "tenshu.settings.route.vault.node",
                "pair" => "tenshu.settings.route.vault.pair",
                "connections" => "tenshu.settings.route.vault.connections",
                _ => "tenshu.settings.route.vault.data",
            },
            label: label.to_owned(),
            route: Menu::Vault(VaultSubMenu::Settings(Some(section))),
            selected: matches!(menu, Menu::Vault(VaultSubMenu::Settings(Some(current))) if *current == section)
                || (section == SettingsOption::Wallet
                    && matches!(menu, Menu::Vault(VaultSubMenu::Settings(None)))),
            enabled: true,
        })
        .collect(),
        Some(RouteScope::Spark) => [
            (
                SparkSettingsOption::General,
                "Balances & conversion",
                "tenshu.settings.route.spark.balances",
            ),
            (
                SparkSettingsOption::LightningAddress,
                "Lightning address",
                "tenshu.settings.route.spark.lightning-address",
            ),
            (
                SparkSettingsOption::Connections,
                "Connections",
                "tenshu.settings.route.spark.connections",
            ),
        ]
        .iter()
        .copied()
        .map(|(section, label, id)| RouteChoice {
            id,
            label: label.to_owned(),
            route: Menu::Spark(SparkSubMenu::Settings(Some(section))),
            selected: match menu {
                Menu::Spark(SparkSubMenu::Settings(Some(current))) => *current == section,
                Menu::Spark(SparkSubMenu::Settings(None)) => {
                    section == SparkSettingsOption::General
                }
                _ => false,
            },
            enabled: true,
        })
        .collect(),
        None => Vec::new(),
    }
}

/// The line under the title (`.idl`): the scope, the wallet's nickname when
/// it has one, the Cube's name and the network, as in "Spark · Daily
/// spending · Everyday · Bitcoin mainnet". Names get the side panel's
/// one-line cut, because the head doesn't scroll.
fn head_line(scope: RouteScope, cache: &Cache) -> String {
    let wallet = match scope {
        RouteScope::Cube => RouteScope::Cube.label().to_owned(),
        RouteScope::Vault => wallet_label("Vault", cache.wallet_names.vault.as_deref()),
        RouteScope::Spark => wallet_label("Spark", cache.wallet_names.spark.as_deref()),
    };
    let name = if cache.cube_name.is_empty() {
        wallet
    } else {
        format!("{wallet} · {}", crate::app::view::cube_display_name(cache))
    };
    crate::app::view::network_line(cache, &name)
}

/// The Settings head: "Settings" and its scope line, the scope switch on the
/// right, and the scope's section tabs on the rule. `None` off the Settings
/// routes. The tabs keep one row at the 1024px minimum window, so there is
/// no narrow-window substitute.
pub fn page_head(menu: &Menu, cache: &Cache) -> Option<PageHead<'static, Message>> {
    let scope = route_scope(menu)?;
    let mut head =
        coincube_ui::component::tenshu::page_head(localized_copy("Settings"), Message::FocusAction)
            .context(crate::app::view::cube_coin(cache), head_line(scope, cache));

    // A switch with one scope isn't a choice, so a Cube with neither a Vault
    // nor Spark shows none (master plan: segmented controls only for real
    // choices).
    let scopes = scopes(cache);
    if scopes.len() > 1 {
        let routes: Vec<Menu> = scopes
            .iter()
            .map(|target| scope_route(menu, *target, cache))
            .collect();
        head = head.action(
            segmented(
                scopes.iter().enumerate().map(|(index, target)| {
                    segment(target.id(), index, target.label()).icon(target.icon())
                }),
                scopes.iter().position(|target| *target == scope),
                Message::FocusAction,
            )
            .size(SegmentSize::Compact)
            .plain()
            .on_select(move |index| Message::Menu(routes[index].clone())),
        );
    }

    let sections = section_routes(menu, cache);
    let section_routes: Vec<Menu> = sections.iter().map(|tab| tab.route.clone()).collect();
    Some(
        head.rule(
            tabs(
                sections.iter().enumerate().map(|(index, tab)| {
                    segment(tab.id, index, localized_copy(&tab.label).to_owned())
                        .enabled(tab.enabled)
                }),
                sections.iter().position(|tab| tab.selected),
                Message::FocusAction,
            )
            .on_select(move |index| Message::Menu(section_routes[index].clone())),
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Bitcoin mainnet Cube with a Vault, so all three scopes show.
    fn spark_cache() -> Cache {
        Cache {
            has_vault: true,
            fiat_chain: crate::chain::ChainId::Bitcoin,
            ..Cache::default()
        }
    }

    #[test]
    fn imported_bitcoin_has_the_vault_tabs_without_phone_pairing() {
        // As the App sets it: an imported wallet has no Keychain key, so
        // like such a Vault it has no Mobile tab. Every other tab is there.
        let cache = Cache {
            has_vault: false,
            has_imported_bitcoin: true,
            vault_lacks_keychain_key: true,
            ..Cache::default()
        };
        let menu = Menu::Vault(VaultSubMenu::Settings(Some(SettingsOption::Wallet)));
        assert!(scopes(&cache).contains(&RouteScope::Vault));
        let labels: Vec<_> = section_routes(&menu, &cache)
            .into_iter()
            .map(|choice| choice.label)
            .collect();
        assert!(!labels.iter().any(|label| label == "Mobile"));
        for tab in ["Wallet & signers", "Node", "Connections", "Data & exports"] {
            assert!(labels.iter().any(|label| label == tab), "{}", tab);
        }
    }

    #[test]
    fn a_vault_without_a_keychain_key_has_no_keychain_tab() {
        let labels = |menu: &Menu, cache: &Cache| -> Vec<String> {
            section_routes(menu, cache)
                .into_iter()
                .map(|choice| choice.label)
                .collect()
        };
        let node = Menu::Vault(VaultSubMenu::Settings(Some(SettingsOption::Node)));
        let mut cache = spark_cache();
        assert!(labels(&node, &cache).contains(&"Mobile".to_owned()));

        cache.vault_lacks_keychain_key = true;
        assert!(!labels(&node, &cache).contains(&"Mobile".to_owned()));
        // Still listed while it is the tab open, so the row doesn't vanish
        // from under the reader when a Keychain lookup finds nothing.
        let open = Menu::Vault(VaultSubMenu::Settings(Some(SettingsOption::LocalSigning)));
        assert!(section_routes(&open, &cache)
            .iter()
            .any(|choice| choice.label == "Mobile" && choice.selected));
    }

    #[test]
    fn vault_settings_default_selects_wallet_and_signers() {
        let menu = Menu::Vault(VaultSubMenu::Settings(None));
        let sections = section_routes(&menu, &Cache::default());
        assert!(sections
            .iter()
            .any(|choice| choice.label == "Wallet & signers" && choice.selected));
        assert!(!sections
            .iter()
            .any(|choice| choice.label == "Node" && choice.selected));
    }

    #[test]
    fn cube_sections_keep_recipient_checks_separate() {
        let menu = Menu::Cube(CubeSubMenu::Settings(CubeSettingsOption::PaymentChecks));
        let sections = section_routes(&menu, &Cache::default());
        assert!(sections
            .iter()
            .any(|choice| choice.label == "Payment checks" && choice.selected));
        assert!(sections
            .iter()
            .any(|choice| choice.label == "Display & appearance" && !choice.selected));
    }

    #[test]
    fn cube_settings_exposes_ai_access_in_the_rendered_route_choices() {
        let menu = Menu::Cube(CubeSubMenu::Settings(CubeSettingsOption::General));
        let sections = section_routes(&menu, &Cache::default());
        assert!(sections.iter().any(|choice| {
            choice.id == "tenshu.settings.route.cube.ai-access"
                && choice.label == "AI access"
                && choice.enabled
                && !choice.selected
                && choice.route == Menu::Cube(CubeSubMenu::Settings(CubeSettingsOption::AiAccess))
        }));

        let ai_menu = Menu::Cube(CubeSubMenu::Settings(CubeSettingsOption::AiAccess));
        assert!(section_routes(&ai_menu, &Cache::default())
            .iter()
            .any(|choice| choice.label == "AI access" && choice.selected));
    }

    #[test]
    fn cube_data_exports_is_selected_only_on_its_own_route() {
        let data = Menu::Cube(CubeSubMenu::Settings(CubeSettingsOption::DataExports));
        let sections = section_routes(&data, &Cache::default());
        assert!(sections.iter().any(|choice| {
            choice.label == "Data & exports"
                && choice.selected
                && choice.enabled
                && choice.route
                    == Menu::Cube(CubeSubMenu::Settings(CubeSettingsOption::DataExports))
        }));
        let backup = Menu::Cube(CubeSubMenu::Settings(CubeSettingsOption::Recovery));
        assert!(section_routes(&backup, &Cache::default())
            .iter()
            .any(|choice| choice.label == "Data & exports" && !choice.selected));
    }

    #[test]
    fn cube_settings_keeps_about_and_stats_reachable() {
        let cache = Cache::default();
        for (current, label) in [
            (CubeSettingsOption::About, "About"),
            (CubeSettingsOption::Stats, "Download stats"),
        ] {
            let menu = Menu::Cube(CubeSubMenu::Settings(current));
            assert_eq!(route_scope(&menu), Some(RouteScope::Cube));

            let sections = section_routes(&menu, &cache);
            assert!(sections.iter().any(|choice| {
                choice.label == label
                    && choice.selected
                    && choice.enabled
                    && choice.route == Menu::Cube(CubeSubMenu::Settings(current))
            }));
            assert!(sections
                .iter()
                .any(|choice| choice.label == "Data & exports"));
        }
    }

    /// One tab set per scope, in head order.
    #[test]
    fn each_scope_has_its_tab_set() {
        let cache = spark_cache();
        let labels = |menu: Menu| -> Vec<String> {
            section_routes(&menu, &cache)
                .into_iter()
                .map(|tab| tab.label)
                .collect()
        };
        let mut cube = vec![
            "Display & appearance",
            "Backup & protection",
            "Payment checks",
            "AI access",
            "Connections",
            "Data & exports",
        ];
        if crate::feature_flags::CUBE_MEMBERS_UI_ENABLED {
            cube.push("Members");
        }
        assert_eq!(labels(RouteScope::Cube.landing()), cube);
        assert_eq!(
            labels(RouteScope::Vault.landing()),
            [
                "Wallet & signers",
                "Node",
                "Mobile",
                "Connections",
                "Data & exports"
            ]
        );
        assert_eq!(
            labels(RouteScope::Spark.landing()),
            ["Balances & conversion", "Lightning address", "Connections"]
        );
    }

    #[test]
    fn lightning_address_is_its_own_spark_tab() {
        let cache = spark_cache();
        for (menu, selected) in [
            (
                Menu::Spark(SparkSubMenu::Settings(None)),
                "Balances & conversion",
            ),
            (
                Menu::Spark(SparkSubMenu::Settings(Some(SparkSettingsOption::General))),
                "Balances & conversion",
            ),
            (
                Menu::Spark(SparkSubMenu::Settings(Some(
                    SparkSettingsOption::LightningAddress,
                ))),
                "Lightning address",
            ),
            (
                Menu::Spark(SparkSubMenu::Settings(Some(
                    SparkSettingsOption::Connections,
                ))),
                "Connections",
            ),
        ] {
            let current: Vec<String> = section_routes(&menu, &cache)
                .into_iter()
                .filter(|tab| tab.selected)
                .map(|tab| tab.label)
                .collect();
            assert_eq!(current, [selected], "{menu:?}");
        }
    }

    /// Switching scope keeps the tab when the other scope has it, else opens
    /// that scope's first tab.
    #[test]
    fn switching_scope_keeps_a_shared_tab_else_opens_the_first() {
        let cache = spark_cache();
        let vault_connections =
            Menu::Vault(VaultSubMenu::Settings(Some(SettingsOption::Connections)));
        let spark_connections = Menu::Spark(SparkSubMenu::Settings(Some(
            SparkSettingsOption::Connections,
        )));
        let cube_data = Menu::Cube(CubeSubMenu::Settings(CubeSettingsOption::DataExports));
        let vault_data = Menu::Vault(VaultSubMenu::Settings(Some(SettingsOption::ImportExport)));

        assert_eq!(
            scope_route(&vault_connections, RouteScope::Spark, &cache),
            spark_connections
        );
        assert_eq!(
            scope_route(&spark_connections, RouteScope::Vault, &cache),
            vault_connections
        );
        assert_eq!(
            scope_route(&cube_data, RouteScope::Vault, &cache),
            vault_data
        );
        assert_eq!(
            scope_route(&vault_data, RouteScope::Cube, &cache),
            cube_data
        );
        // Cube has Connections too; Spark has no Data & exports.
        assert_eq!(
            scope_route(&spark_connections, RouteScope::Cube, &cache),
            Menu::Cube(CubeSubMenu::Settings(CubeSettingsOption::Connections))
        );
        assert_eq!(
            scope_route(&vault_data, RouteScope::Spark, &cache),
            RouteScope::Spark.landing()
        );
        // The bare Spark route is Balances & conversion, which Vault lacks.
        assert_eq!(
            scope_route(
                &Menu::Spark(SparkSubMenu::Settings(None)),
                RouteScope::Vault,
                &cache
            ),
            RouteScope::Vault.landing()
        );
        // The current scope stays where it is.
        assert_eq!(
            scope_route(&vault_data, RouteScope::Vault, &cache),
            vault_data
        );
    }

    /// An imported-bitcoin Vault has no Data & exports, so Cube's opens the
    /// Vault's first tab.
    #[test]
    fn switching_to_a_scope_without_the_tab_opens_its_first() {
        let cache = Cache {
            has_vault: true,
            ..Cache::default()
        };
        // The Vault has no Backup & protection tab.
        let cube_backup = Menu::Cube(CubeSubMenu::Settings(CubeSettingsOption::Recovery));
        assert_eq!(
            scope_route(&cube_backup, RouteScope::Vault, &cache),
            RouteScope::Vault.landing()
        );
    }

    #[test]
    fn head_line_names_the_scope_nickname_cube_and_network() {
        let mut cache = Cache {
            cube_name: "Everyday".to_owned(),
            ..spark_cache()
        };
        assert_eq!(
            head_line(RouteScope::Cube, &cache),
            "Cube · Everyday · Bitcoin mainnet"
        );
        assert_eq!(
            head_line(RouteScope::Spark, &cache),
            "Spark · Everyday · Bitcoin mainnet"
        );
        cache.wallet_names.spark = Some("Daily spending".to_owned());
        cache.wallet_names.vault = Some("Family savings".to_owned());
        assert_eq!(
            head_line(RouteScope::Spark, &cache),
            "Spark · Daily spending · Everyday · Bitcoin mainnet"
        );
        assert_eq!(
            head_line(RouteScope::Vault, &cache),
            "Vault · Family savings · Everyday · Bitcoin mainnet"
        );
        // A Cube without a name leaves it out rather than repeating "Cube".
        cache.cube_name.clear();
        assert_eq!(
            head_line(RouteScope::Cube, &cache),
            "Cube · Bitcoin mainnet"
        );
        assert_eq!(
            head_line(RouteScope::Vault, &cache),
            "Vault · Family savings · Bitcoin mainnet"
        );
    }

    #[test]
    fn a_cube_with_one_scope_has_no_switch() {
        let cache = Cache {
            fiat_chain: crate::chain::ChainId::Signet,
            ..Cache::default()
        };
        assert_eq!(scopes(&cache), [RouteScope::Cube]);
        assert_eq!(
            scopes(&spark_cache()),
            [RouteScope::Cube, RouteScope::Vault, RouteScope::Spark]
        );
    }
}
