use std::collections::HashMap;
use std::sync::Arc;

use coincube_core::miniscript::bitcoin::{bip32::ChildNumber, Address, Amount};
use coincube_ui::component::form;
use coincube_ui::widget::*;
use iced::Task;

use super::{fiat_converter_for_wallet, Cache, Menu, State};
use crate::app::error::Error;
use crate::app::state::vault::label::LabelsEdited;
use crate::app::state::vault::receive::ShowQrCodeModal;
use crate::app::view::global_home::{GlobalViewConfig, HomeView, TransferDirection};
use crate::app::view::HomeMessage;
use crate::app::{message::Message, view, wallet::Wallet};
use crate::daemon::model::{LabelItem, Labelled};
use crate::daemon::Daemon;

#[derive(Default)]
pub enum Modal {
    ShowQrCode(ShowQrCodeModal),
    #[default]
    None,
}

#[derive(Debug)]
pub struct ReceiveAddressInfo {
    pub address: Address,
    pub index: ChildNumber,
    pub labels: HashMap<String, String>,
}

impl Labelled for ReceiveAddressInfo {
    fn labelled(&self) -> Vec<LabelItem> {
        vec![LabelItem::Address(self.address.clone())]
    }

    fn labels(&mut self) -> &mut HashMap<String, String> {
        &mut self.labels
    }
}

#[derive(Default)]
pub struct GlobalHome {
    wallet: Option<Arc<Wallet>>,
    balance_masked: bool,
    transfer_direction: Option<TransferDirection>,
    current_view: HomeView,
    entered_amount: form::Value<String>,
    receive_address_info: Option<ReceiveAddressInfo>,
    labels_edited: LabelsEdited,
    address_expanded: bool,
    modal: Modal,
    empty_labels: HashMap<String, String>,
    warning: Option<Error>,
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

        let content = view::dashboard(
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
                receive_address: self.receive_address_info.as_ref().map(|info| &info.address),
                receive_index: self.receive_address_info.as_ref().map(|info| &info.index),
                labels: self
                    .receive_address_info
                    .as_ref()
                    .map_or(&self.empty_labels, |info| &info.labels),
                labels_editing: self.labels_edited.cache(),
                address_expanded: self.address_expanded,
                warning: self.warning.as_ref(),
            }),
        );

        let overlay = match &self.modal {
            Modal::ShowQrCode(m) => m.view(),
            Modal::None => return content,
        };

        coincube_ui::widget::modal::Modal::new(content, overlay)
            .on_blur(Some(view::Message::Close))
            .into()
    }

    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        match message {
            Message::View(view::Message::Home(msg)) => match msg {
                HomeMessage::ToggleBalanceMask => {
                    self.balance_masked = !self.balance_masked;
                    Task::none()
                }
                HomeMessage::NextStep => {
                    if self.current_view.step == 2 {
                        if let Some(TransferDirection::ActiveToVault) = self.transfer_direction {
                            self.current_view.next();
                            return Task::perform(
                                async move {
                                    match daemon.get_new_address().await {
                                        Ok(res) => Ok((res.address, res.derivation_index)),
                                        Err(e) => Err(e.into()),
                                    }
                                },
                                Message::ReceiveAddress,
                            );
                        }
                    }
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
                HomeMessage::AmountEdited(amount) => {
                    self.entered_amount.value = amount;
                    Task::none()
                }
                HomeMessage::ConfirmTransfer => {
                    // TODO: Implement transfer confirmation logic (we don't have active wallet yet)
                    Task::none()
                }
            },
            Message::ReceiveAddress(res) => match res {
                Ok((address, index)) => {
                    self.warning = None;
                    self.receive_address_info = Some(ReceiveAddressInfo {
                        address,
                        index,
                        labels: HashMap::new(),
                    });
                    Task::none()
                }
                Err(e) => {
                    self.warning = Some(e);
                    self.receive_address_info = None;
                    Task::none()
                }
            },
            Message::View(view::Message::SelectAddress(_addr)) => {
                self.address_expanded = !self.address_expanded;
                Task::none()
            }
            Message::View(view::Message::Label(_, _)) | Message::LabelsUpdated(_) => {
                match self.labels_edited.update(
                    daemon,
                    message,
                    self.receive_address_info
                        .iter_mut()
                        .map(|info| info as &mut dyn crate::daemon::model::LabelsLoader),
                ) {
                    Ok(cmd) => {
                        self.warning = None;
                        cmd
                    }
                    Err(e) => {
                        self.warning = Some(e);
                        Task::none()
                    }
                }
            }
            Message::View(view::Message::ShowQrCode(_)) => {
                if let Some(info) = &self.receive_address_info {
                    if let Some(modal) = ShowQrCodeModal::new(&info.address, info.index) {
                        self.modal = Modal::ShowQrCode(modal);
                    }
                }
                Task::none()
            }
            Message::View(view::Message::Close) => {
                self.modal = Modal::None;
                Task::none()
            }
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
