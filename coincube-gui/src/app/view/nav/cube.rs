use super::{NavContext, SubItem};
use crate::app::menu::{CubeSettingsOption, CubeSubMenu, Menu};
use coincube_ui::icon::{home_icon, settings_icon};

/// Secondary-rail items for the Cube section.
/// Clicking Settings lands on General (first third-rail option).
pub fn items(_ctx: &NavContext) -> Vec<SubItem> {
    vec![
        SubItem {
            label: "Overview",
            icon: home_icon,
            route: Menu::Cube(CubeSubMenu::Overview),
            matches: |m| matches!(m, Menu::Cube(CubeSubMenu::Overview)),
        },
        SubItem {
            label: "Settings",
            icon: settings_icon,
            route: Menu::Cube(CubeSubMenu::Settings(CubeSettingsOption::General)),
            matches: |m| matches!(m, Menu::Cube(CubeSubMenu::Settings(_))),
        },
    ]
}
