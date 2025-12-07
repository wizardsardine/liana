use std::sync::Arc;

use coincube_core::miniscript::bitcoin::Amount;
use coincube_ui::component::form;
use coincube_ui::widget::*;
use iced::Task;

use super::{fiat_converter_for_wallet, Cache, Menu, State};
use crate::app::view::global_home::{GlobalViewConfig, HomeView, TransferDirection};
use crate::app::view::{HomeMessage, TransferFlowMessage};
use crate::app::{message::Message, view, wallet::Wallet};
use crate::daemon::Daemon;

#[derive(Default)]
pub struct GlobalHome {
    wallet: Option<Arc<Wallet>>,
    balance_masked: bool,
    transfer_direction: Option<TransferDirection>,
    current_view: HomeView,
    entered_amount: form::Value<String>,
}

impl GlobalHome {
    pub fn new(wallet: Arc<Wallet>) -> Self {
        Self {
            wallet: Some(wallet),
            ..Default::default()
        }
    }

    pub fn new_without_wallet() -> Self {
        Self::default()
    }
}

impl State for GlobalHome {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        let vault_balance = cache
            .coins()
            .iter()
            .filter(|coin| coin.spend_info.is_none())
            .fold(Amount::from_sat(0), |acc, coin| acc + coin.amount);

        let active_balance = Amount::from_sat(0);

        let fiat_converter = self
            .wallet
            .as_ref()
            .and_then(|w| fiat_converter_for_wallet(w, cache));

        view::dashboard(
            menu,
            cache,
            None,
            view::global_home::global_home_view(GlobalViewConfig {
                active_balance,
                vault_balance,
                fiat_converter,
                balance_masked: self.balance_masked,
                has_vault: cache.has_vault,
                current_view: self.current_view,
                transfer_direction: self.transfer_direction,
                entered_amount: &self.entered_amount,
            }),
        )
    }

    fn update(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        match message {
            Message::View(view::Message::Home(msg)) => match msg {
                HomeMessage::ToggleBalanceMask => {
                    self.balance_masked = !self.balance_masked;
                    Task::none()
                }
                HomeMessage::GoToStep(step) => {
                    self.current_view.goto(step);
                    Task::none()
                }
                HomeMessage::NextStep => {
                    self.current_view.next();
                    Task::none()
                }
                HomeMessage::PreviousStep => {
                    self.current_view.previous();
                    Task::none()
                }
                HomeMessage::SelectTransferDirection(direction) => {
                    self.transfer_direction = Some(direction);
                    Task::none()
                }
                HomeMessage::TransferFlow(flow) => match flow {
                    TransferFlowMessage::Start => {
                        self.current_view.goto(HomeView::SELECT_TRANSFER_DIRECTION);
                        self.transfer_direction = None;
                        self.entered_amount = form::Value::default();
                        Task::none()
                    }
                    TransferFlowMessage::EnterAmount(direction) => {
                        self.transfer_direction = Some(direction);
                        self.current_view.goto(HomeView::ENTER_AMOUNT);
                        Task::none()
                    }
                    TransferFlowMessage::AmountEdited(amount) => {
                        self.entered_amount = form::Value {
                            value: amount,
                            ..Default::default()
                        };
                        Task::none()
                    }
                    TransferFlowMessage::ConfirmTransfer(_direction) => {
                        // TODO: Implement transfer confirmation logic
                        Task::none()
                    }
                    TransferFlowMessage::Cancel => {
                        self.current_view.reset();
                        self.transfer_direction = None;
                        self.entered_amount = form::Value::default();
                        Task::none()
                    }
                },
            },
            _ => Task::none(),
        }
    }

    fn reload(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        wallet: Arc<Wallet>,
    ) -> Task<Message> {
        self.wallet = Some(wallet);
        Task::none()
    }
}
