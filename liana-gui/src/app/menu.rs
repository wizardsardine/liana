use iced::widget::row;
use liana::miniscript::bitcoin::{OutPoint, Txid};
use liana_ui::{
    component::button::{self, menu_active},
    icon,
};

use crate::t;

#[derive(Debug, Clone, Copy)]
pub enum MenuWidth {
    Normal,
    Compact,
    Small,
}

impl MenuWidth {
    pub fn from_pane_width(w: f32) -> Self {
        if w < 700.0 {
            return Self::Small;
        } else if w < 1200.0 {
            return Self::Compact;
        }
        Self::Normal
    }

    pub fn is_small(&self) -> bool {
        matches!(self, &Self::Small)
    }

    pub fn is_compact(&self) -> bool {
        matches!(self, &Self::Compact)
    }
}

impl From<MenuWidth> for f32 {
    fn from(val: MenuWidth) -> Self {
        match val {
            MenuWidth::Normal => 380.0,
            MenuWidth::Compact => 210.0,
            MenuWidth::Small => 70.0,
        }
    }
}

use super::view::Message;
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Menu {
    Home,
    Receive,
    PSBTs,
    Transactions,
    TransactionPreSelected(Txid),
    Settings,
    SettingsPreSelected(SettingsOption),
    Coins,
    CreateSpendTx,
    Recovery,
    RefreshCoins(Vec<OutPoint>),
    PsbtPreSelected(Txid),
    Map(Option<MapFocus>),
}

/// Pre-selectable settings options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsOption {
    Node,
}

/// What the map centers on when opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MapFocus {
    Tx(Txid),
    Coin(OutPoint),
}

fn menu_entry<'a>(
    active: &Menu,
    menu: Menu,
    icon: liana_ui::widget::Text<'a>,
    text: String,
    reload: bool,
    menu_width: MenuWidth,
) -> liana_ui::widget::Row<'a, Message> {
    if *active == menu {
        let msg = if reload {
            Message::Reload
        } else {
            Message::Menu(menu)
        };
        let btn = if menu_width.is_small() {
            button::menu_active_small(icon)
        } else {
            menu_active(Some(icon), text, menu_width.is_compact())
        };

        row!(btn.on_press(msg).width(iced::Length::Fill),)
    } else {
        let msg = Message::Menu(menu);
        let btn = if menu_width.is_small() {
            button::menu_small(icon)
        } else {
            button::menu(Some(icon), text, menu_width.is_compact())
        };
        row!(btn.on_press(msg).width(iced::Length::Fill))
    }
}

impl Menu {
    pub fn title(&self) -> String {
        match self {
            Menu::Home => t!("menu-dashboard"),
            Menu::Receive => t!("menu-receive"),
            Menu::PSBTs => t!("menu-drafts-approvals"),
            Menu::Transactions => t!("menu-transactions"),
            Menu::Settings => t!("menu-settings"),
            Menu::Coins => t!("menu-coins-utxos"),
            Menu::CreateSpendTx => t!("menu-send"),
            Menu::Recovery => t!("common-recovery"),
            Menu::Map(_) => t!("map-title"),
            Menu::RefreshCoins(_)
            | Menu::PsbtPreSelected(_)
            | Menu::TransactionPreSelected(_)
            | Menu::SettingsPreSelected(_) => String::new(),
        }
    }

    fn icon(&self) -> liana_ui::widget::Text<'static> {
        match self {
            Menu::Home => icon::home_icon(),
            Menu::Receive => icon::receive_icon(),
            Menu::PSBTs => icon::edit_icon_padding(),
            Menu::Transactions => icon::collection_icon(),
            Menu::Settings => icon::settings_icon(),
            Menu::Coins => icon::coins_icon(),
            Menu::CreateSpendTx => icon::send_icon(),
            Menu::Recovery => icon::recovery_icon(),
            Menu::Map(_) => icon::diagram_3_icon(),
            Menu::RefreshCoins(_)
            | Menu::PsbtPreSelected(_)
            | Menu::TransactionPreSelected(_)
            | Menu::SettingsPreSelected(_) => icon::home_icon(),
        }
    }

    fn reload(&self) -> bool {
        match self {
            Menu::Home
            | Menu::Receive
            | Menu::PSBTs
            | Menu::Transactions
            | Menu::Coins
            | Menu::CreateSpendTx
            | Menu::Recovery => true,
            Menu::Settings
            | Menu::TransactionPreSelected(_)
            | Menu::SettingsPreSelected(_)
            | Menu::RefreshCoins(_)
            | Menu::PsbtPreSelected(_)
            | Menu::Map(_) => false,
        }
    }

    /// Menu the map back button returns to when entering `target` from `self`.
    pub fn map_return(&self, target: &Menu, previous: &Menu) -> Menu {
        match (self, target) {
            (Menu::Map(_), _) => previous.clone(),
            (_, Menu::Map(_)) => self.clone(),
            _ => previous.clone(),
        }
    }

    pub fn entry<'a>(
        self,
        active: &Menu,
        menu_width: MenuWidth,
    ) -> liana_ui::widget::Row<'a, Message> {
        menu_entry(
            active,
            self.clone(),
            self.icon(),
            self.title(),
            self.reload(),
            menu_width,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use liana::miniscript::bitcoin::Txid;

    use crate::app::menu::{MapFocus, Menu};

    fn txid() -> Txid {
        Txid::from_str("0000000000000000000000000000000000000000000000000000000000000001").unwrap()
    }

    #[test]
    fn map_return_from_panel() {
        assert_eq!(
            Menu::Transactions.map_return(&Menu::Map(None), &Menu::Home),
            Menu::Transactions
        );
    }

    #[test]
    fn map_return_keeps_origin_inside_map() {
        assert_eq!(
            Menu::Map(None).map_return(&Menu::Map(Some(MapFocus::Tx(txid()))), &Menu::Coins),
            Menu::Coins
        );
    }

    #[test]
    fn map_return_unchanged_for_other_targets() {
        assert_eq!(
            Menu::Home.map_return(&Menu::Coins, &Menu::Transactions),
            Menu::Transactions
        );
    }

    #[test]
    fn map_return_keeps_preselected_tx() {
        assert_eq!(
            Menu::TransactionPreSelected(txid()).map_return(&Menu::Map(None), &Menu::Home),
            Menu::TransactionPreSelected(txid())
        );
    }
}
