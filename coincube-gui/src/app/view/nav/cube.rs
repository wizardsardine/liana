use super::{NavContext, SubItem};
use crate::app::menu::{CubeSettingsOption, CubeSubMenu, Menu};
use coincube_ui::icon::{home_icon, settings_icon};

/// Secondary-rail items for the Cube section.
/// Clicking Settings lands on General (first third-rail option).
pub fn items(_ctx: &NavContext) -> Vec<SubItem> {
    vec![
        SubItem::new(
            "Overview",
            home_icon,
            Menu::Cube(CubeSubMenu::Overview),
            |m| matches!(m, Menu::Cube(CubeSubMenu::Overview)),
        ),
        SubItem::new(
            "Settings",
            settings_icon,
            Menu::Cube(CubeSubMenu::Settings(CubeSettingsOption::General)),
            |m| matches!(m, Menu::Cube(CubeSubMenu::Settings(_))),
        ),
    ]
}
