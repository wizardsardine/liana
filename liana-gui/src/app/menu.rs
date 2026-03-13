use iced::widget::row;
use liana::miniscript::bitcoin::{OutPoint, Txid};
use liana_ui::{
    component::button::{self, menu_active},
    icon,
};

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
    small: bool,
) -> liana_ui::widget::Row<'a, Message> {
    if *active == menu {
        let msg = if reload {
            Message::Reload
        } else {
            Message::Menu(menu)
        };
        let btn = if small {
            button::menu_active_small(icon)
        } else {
            menu_active(Some(icon), text)
        };

        row!(btn.on_press(msg).width(iced::Length::Fill),)
    } else {
        let msg = Message::Menu(menu);
        let btn = if small {
            button::menu_small(icon)
        } else {
            button::menu(Some(icon), text)
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
            Menu::PSBTs => icon::history_icon(),
            Menu::Transactions => icon::history_icon(),
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

    pub fn entry<'a>(self, active: &Menu, small: bool) -> liana_ui::widget::Row<'a, Message> {
        menu_entry(
            active,
            self.clone(),
            self.icon(),
            self.title(),
            self.reload(),
            small,
        )
    }
}
