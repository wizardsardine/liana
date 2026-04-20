use crate::app::menu::Menu;
use coincube_ui::widget::Text;

/// One row in the secondary rail.
///
/// `matches` exists because some submenu variants carry payload
/// (`Transactions(Option<Txid>)`, `Settings(Option<SettingsOption>)`, etc.)
/// so a plain `menu == &item.route` check won't highlight both the empty
/// and payload-carrying instances of the same logical item.
pub struct SubItem {
    pub label: &'static str,
    pub icon: fn() -> Text<'static>,
    pub route: Menu,
    pub matches: fn(&Menu) -> bool,
}
