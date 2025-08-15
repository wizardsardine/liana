use iced::{Subscription, Task};
use std::{
    collections::{HashMap, HashSet},
    convert::TryInto,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use liana_ui::widget::Element;

use crate::{
    app::{
        cache::Cache,
        error::Error,
        message::Message,
        state::{label::LabelsEdited, State},
        view::{buysell::buysell_view, BuySellMessage, Message as ViewMessage},
        wallet::Wallet,
    },
    daemon::model::{self, LabelsLoader},
    daemon::Daemon,
    export::{ImportExportMessage, ImportExportType},
};

#[derive(Debug, Clone)]
pub enum AccountType {
    Individual,
    Business
    
}

#[derive(Debug, Clone)]
pub struct BuyAndSellPanel {
    pub show_account_selection: bool,
    pub selected_account_type: Option<AccountType>,
}

impl BuyAndSellPanel {
    pub fn new() -> Self {
        Self {
            show_account_selection: false,
            selected_account_type: None,
        }
    }
    
    pub fn show_account_selection(&mut self) {
        self.show_account_selection = true;
        self.selected_account_type = None;
    }
     
    pub fn hide_account_selection(&mut self) {
        self.show_account_selection = false;
        self.selected_account_type = None;
    }
    
    pub fn select_account_type(&mut self, account_type: AccountType) {
        self.selected_account_type = Some(account_type);
    }
    
    pub fn get_started(&mut self) -> Task<Message> {
        self.hide_account_selection();
        Task::none()
    }
}

impl State for BuyAndSellPanel {
    fn view<'a>(&'a self, _cache: &'a Cache) -> Element<'a, ViewMessage> {
        buysell_view(self)
    }

    fn update(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        match message {
            Message::View(ViewMessage::BuySell(BuySellMessage::ShowAccountSelection)) => {
                self.show_account_selection();
                Task::none()
            }
            Message::View(ViewMessage::BuySell(BuySellMessage::HideAccountSelection)) => {
                self.hide_account_selection();
                Task::none()
            }
            Message::View(ViewMessage::BuySell(BuySellMessage::SelectAccountType(account_type))) => {
                self.select_account_type(account_type);
                Task::none()
            }
            Message::View(ViewMessage::BuySell(BuySellMessage::GetStarted)) => {
                self.get_started()
            }
            _ => Task::none(),
        }
    }

    fn reload(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        _wallet: Arc<Wallet>,
    ) -> Task<Message> {
        Task::none()
    }

    fn subscription(&self) -> iced::Subscription<Message> {
        iced::Subscription::none()
    }
    
}