use super::{NavContext, SubItem};
use crate::app::menu::{HomeSettingsOption, HomeSubMenu, Menu};
use coincube_ui::icon::{home_icon, settings_icon};

/// Secondary-rail items for the Cube (a.k.a. Home) section.
/// Clicking Settings lands on General (first third-rail option).
pub fn items(_ctx: &NavContext) -> Vec<SubItem> {
    vec![
        SubItem {
            label: "Overview",
            icon: home_icon,
            route: Menu::Home(HomeSubMenu::Overview),
            matches: |m| matches!(m, Menu::Home(HomeSubMenu::Overview)),
        },
        SubItem {
            label: "Settings",
            icon: settings_icon,
            route: Menu::Home(HomeSubMenu::Settings(HomeSettingsOption::General)),
            matches: |m| matches!(m, Menu::Home(HomeSubMenu::Settings(_))),
        },
    ]
}
