use iced::widget::row;
use liana::miniscript::bitcoin::{OutPoint, Txid};
use liana_ui::{
    component::button::{self, menu_active},
    icon,
};

#[derive(Debug, Clone, Copy, Default)]
pub enum MenuWidth {
    #[default]
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
}

/// Pre-selectable settings options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsOption {
    Node,
}

fn menu_entry<'a>(
    active: &Menu,
    menu: Menu,
    icon: liana_ui::widget::Text<'a>,
    text: &'static str,
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
    pub fn title(&self) -> &'static str {
        match self {
            Menu::Home => "Dashboard",
            Menu::Receive => "Receive",
            Menu::PSBTs => "Drafts & Approvals",
            Menu::Transactions => "Transactions",
            Menu::Settings => "Settings",
            Menu::Coins => "Coins/UTXOs",
            Menu::CreateSpendTx => "Send",
            Menu::Recovery => "Recovery",
            Menu::RefreshCoins(_)
            | Menu::PsbtPreSelected(_)
            | Menu::TransactionPreSelected(_)
            | Menu::SettingsPreSelected(_) => "",
        }
    }

    fn icon(&self) -> liana_ui::widget::Text<'static> {
        match self {
            Menu::Home => icon::home_icon(),
            Menu::Receive => icon::receive_icon(),
            Menu::PSBTs => icon::pencil_icon(),
            Menu::Transactions => icon::collection_icon(),
            Menu::Settings => icon::settings_icon(),
            Menu::Coins => icon::coins_icon(),
            Menu::CreateSpendTx => icon::send_icon(),
            Menu::Recovery => icon::recovery_icon(),
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
            | Menu::PsbtPreSelected(_) => false,
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
