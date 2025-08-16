use iced::Task;
use std::sync::Arc;

use liana_ui::{widget::Element, component::form};

use crate::{
    app::{
        cache::Cache,
        message::Message,
        state::State,
        view::{buysell::buysell_view, BuySellMessage, Message as ViewMessage},
        wallet::Wallet,
    },
    daemon::Daemon,
};

#[derive(Debug, Clone)]
pub enum AccountType {
    Individual,
    Business,
}

#[derive(Debug, Clone)]
pub enum BuySellStep {
    Initial,
    AccountSelection,
    AccountForm,
}

#[derive(Debug, Clone)]
pub struct AccountFormData {
    pub first_name: form::Value<String>,
    pub last_name: form::Value<String>,
    pub email: form::Value<String>,
    pub password: form::Value<String>,
    pub confirm_password: form::Value<String>,
    pub terms_accepted: bool,
}

impl Default for AccountFormData {
    fn default() -> Self {
        Self {
            first_name: form::Value::default(),
            last_name: form::Value::default(),
            email: form::Value::default(),
            password: form::Value::default(),
            confirm_password: form::Value::default(),
            terms_accepted: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BuyAndSellPanel {
    pub current_step: BuySellStep,
    pub selected_account_type: Option<AccountType>,
    pub form_data: AccountFormData,
    pub show_modal: bool,
}

impl BuyAndSellPanel {
    pub fn new() -> Self {
        Self {
            current_step: BuySellStep::AccountSelection,
            selected_account_type: None,
            form_data: AccountFormData::default(),
            show_modal: false,
        }
    }

    pub fn show_modal(&mut self) {
        self.show_modal = true;
        self.current_step = BuySellStep::AccountSelection;
    }

    pub fn close_modal(&mut self) {
        self.show_modal = false;
        self.current_step = BuySellStep::AccountSelection;
        self.selected_account_type = None;
        self.form_data = AccountFormData::default();
    }

    pub fn show_account_selection(&mut self) {
        self.current_step = BuySellStep::AccountSelection;
        self.selected_account_type = None;
    }

    pub fn hide_account_selection(&mut self) {
        self.current_step = BuySellStep::AccountSelection;
        self.selected_account_type = None;
    }

    pub fn select_account_type(&mut self, account_type: AccountType) {
        self.selected_account_type = Some(account_type);
    }

    pub fn get_started(&mut self) -> Task<Message> {
        if self.selected_account_type.is_some() {
            self.current_step = BuySellStep::AccountForm;
        }
        Task::none()
    }

    pub fn go_back(&mut self) {
        match self.current_step {
            BuySellStep::AccountForm => {
                self.current_step = BuySellStep::AccountSelection;
            }
            BuySellStep::AccountSelection => {
                // Go back to home - this will be handled by the menu navigation
                self.current_step = BuySellStep::AccountSelection;
            }
            BuySellStep::Initial => {
                // Should not happen - we start with AccountSelection
                self.current_step = BuySellStep::AccountSelection;
            }
        }
    }

    pub fn update_form_field(&mut self, field: &str, value: String) {
        match field {
            "first_name" => {
                self.form_data.first_name.value = value;
                self.form_data.first_name.valid = !self.form_data.first_name.value.trim().is_empty();
            }
            "last_name" => {
                self.form_data.last_name.value = value;
                self.form_data.last_name.valid = !self.form_data.last_name.value.trim().is_empty();
            }
            "email" => {
                self.form_data.email.value = value;
                self.form_data.email.valid = self.is_valid_email(&self.form_data.email.value);
            }
            "password" => {
                self.form_data.password.value = value;
                self.form_data.password.valid = self.form_data.password.value.len() >= 8;
                // Re-validate confirm password when password changes
                self.form_data.confirm_password.valid =
                    !self.form_data.confirm_password.value.is_empty() &&
                    self.form_data.password.value == self.form_data.confirm_password.value;
            }
            "confirm_password" => {
                self.form_data.confirm_password.value = value;
                self.form_data.confirm_password.valid =
                    self.form_data.password.value == self.form_data.confirm_password.value;
            }
            _ => {}
        }
    }

    pub fn toggle_terms_acceptance(&mut self) {
        self.form_data.terms_accepted = !self.form_data.terms_accepted;
    }

    pub fn is_form_valid(&self) -> bool {
        self.form_data.first_name.valid
            && self.form_data.last_name.valid
            && self.form_data.email.valid
            && self.form_data.password.valid
            && self.form_data.confirm_password.valid
            && self.form_data.terms_accepted
    }

    fn is_valid_email(&self, email: &str) -> bool {
        // Basic email validation
        email.contains('@') && email.contains('.') && email.len() > 5
    }

    pub fn create_account(&mut self) -> Task<Message> {
        if self.is_form_valid() {
            // TODO: Implement actual account creation logic
            // For now, just reset to account selection
            self.current_step = BuySellStep::AccountSelection;
            self.form_data = AccountFormData::default();
            self.selected_account_type = None;
        }
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
            Message::View(ViewMessage::BuySell(BuySellMessage::ShowModal)) => {
                self.show_modal();
                Task::none()
            }
            Message::View(ViewMessage::BuySell(BuySellMessage::CloseModal)) => {
                self.close_modal();
                Task::none()
            }
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
            Message::View(ViewMessage::BuySell(BuySellMessage::GoBack)) => {
                self.go_back();
                Task::none()
            }
            Message::View(ViewMessage::BuySell(BuySellMessage::FormFieldEdited(field, value))) => {
                self.update_form_field(&field, value);
                Task::none()
            }
            Message::View(ViewMessage::BuySell(BuySellMessage::ToggleTermsAcceptance)) => {
                self.toggle_terms_acceptance();
                Task::none()
            }
            Message::View(ViewMessage::BuySell(BuySellMessage::CreateAccount)) => {
                self.create_account()
            }
            _ => Task::none(),
        }
    }

    fn reload(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _wallet: Arc<Wallet>,
    ) -> Task<Message> {
        Task::none()
    }

    fn subscription(&self) -> iced::Subscription<Message> {
        iced::Subscription::none()
    }
    
}