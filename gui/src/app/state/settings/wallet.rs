use std::collections::HashSet;
use std::convert::From;
use std::path::PathBuf;
use std::sync::Arc;

use iced::{Command, Element};

use liana::miniscript::bitcoin::{hashes::hex::ToHex, util::bip32::Fingerprint, Network};

use crate::{
    app::{
        cache::Cache, error::Error, message::Message, settings, state::State, view, wallet::Wallet,
    },
    daemon::Daemon,
    hw::{list_hardware_wallets, HardwareWallet, HardwareWalletConfig},
    ui::component::modal,
};

pub struct WalletSettingsState {
    data_dir: PathBuf,
    warning: Option<Error>,
    descriptor: String,
    wallet: Arc<Wallet>,
    modal: Option<RegisterWalletModal>,
}

impl WalletSettingsState {
    pub fn new(data_dir: PathBuf, wallet: Arc<Wallet>) -> Self {
        WalletSettingsState {
            data_dir,
            descriptor: wallet.main_descriptor.to_string(),
            wallet,
            warning: None,
            modal: None,
        }
    }
}

impl State for WalletSettingsState {
    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        let content =
            view::settings::wallet_settings(cache, self.warning.as_ref(), &self.descriptor);
        if let Some(m) = &self.modal {
            modal::Modal::new(content, m.view())
                .on_blur(Some(view::Message::Close))
                .into()
        } else {
            content
        }
    }

    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        message: Message,
    ) -> Command<Message> {
        match message {
            Message::WalletLoaded(res) => {
                match res {
                    Ok(wallet) => {
                        if let Some(modal) = &mut self.modal {
                            modal.wallet = wallet.clone();
                        }
                        self.wallet = wallet;
                    }
                    Err(e) => self.warning = Some(e),
                };
                Command::none()
            }
            Message::View(view::Message::Close) => {
                self.modal = None;
                Command::none()
            }
            Message::View(view::Message::Settings(view::SettingsMessage::RegisterWallet)) => {
                self.modal = Some(RegisterWalletModal::new(
                    self.data_dir.clone(),
                    self.wallet.clone(),
                ));
                self.modal
                    .as_ref()
                    .map(|m| m.load(daemon))
                    .unwrap_or_else(Command::none)
            }
            _ => self
                .modal
                .as_mut()
                .map(|m| m.update(daemon, cache, message))
                .unwrap_or_else(Command::none),
        }
    }

    fn load(&self, daemon: Arc<dyn Daemon + Sync + Send>) -> Command<Message> {
        Command::perform(
            async move { daemon.get_info().map_err(|e| e.into()) },
            Message::Info,
        )
    }
}

impl From<WalletSettingsState> for Box<dyn State> {
    fn from(s: WalletSettingsState) -> Box<dyn State> {
        Box::new(s)
    }
}

pub struct RegisterWalletModal {
    data_dir: PathBuf,
    wallet: Arc<Wallet>,
    warning: Option<Error>,
    chosen_hw: Option<usize>,
    hws: Vec<HardwareWallet>,
    registered: HashSet<Fingerprint>,
    processing: bool,
}

impl RegisterWalletModal {
    pub fn new(data_dir: PathBuf, wallet: Arc<Wallet>) -> Self {
        let mut registered = HashSet::new();
        for hw in &wallet.hardware_wallets {
            registered.insert(hw.fingerprint);
        }
        Self {
            data_dir,
            wallet,
            warning: None,
            chosen_hw: None,
            hws: Vec::new(),
            processing: false,
            registered,
        }
    }
}

impl RegisterWalletModal {
    fn view(&self) -> Element<view::Message> {
        view::settings::register_wallet_modal(
            self.warning.as_ref(),
            &self.hws,
            self.processing,
            self.chosen_hw,
            &self.registered,
        )
    }

    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        message: Message,
    ) -> Command<Message> {
        match message {
            Message::View(view::Message::Reload) => {
                self.hws = Vec::new();
                self.chosen_hw = None;
                self.warning = None;
                self.load(daemon)
            }
            Message::ConnectedHardwareWallets(hws) => {
                self.hws = hws;
                Command::none()
            }
            Message::WalletRegistered(res) => {
                self.processing = false;
                self.chosen_hw = None;
                match res {
                    Ok(fingerprint) => {
                        self.registered.insert(fingerprint);
                        return Command::perform(async {}, |_| Message::LoadWallet);
                    }
                    Err(e) => self.warning = Some(e),
                }
                Command::none()
            }
            Message::View(view::Message::SelectHardwareWallet(i)) => {
                if let Some(HardwareWallet::Supported {
                    fingerprint,
                    device,
                    ..
                }) = self.hws.get(i)
                {
                    self.chosen_hw = Some(i);
                    self.processing = true;
                    Command::perform(
                        register_wallet(
                            self.data_dir.clone(),
                            cache.network,
                            device.clone(),
                            *fingerprint,
                            self.wallet.clone(),
                        ),
                        Message::WalletRegistered,
                    )
                } else {
                    Command::none()
                }
            }
            _ => Command::none(),
        }
    }

    fn load(&self, _daemon: Arc<dyn Daemon + Sync + Send>) -> Command<Message> {
        Command::perform(
            list_hws(self.wallet.clone()),
            Message::ConnectedHardwareWallets,
        )
    }
}

async fn register_wallet(
    data_dir: PathBuf,
    network: Network,
    hw: std::sync::Arc<dyn async_hwi::HWI + Send + Sync>,
    fingerprint: Fingerprint,
    wallet: Arc<Wallet>,
) -> Result<Fingerprint, Error> {
    let hmac = hw
        .register_wallet(&wallet.name, &wallet.main_descriptor.to_string())
        .await
        .map_err(Error::from)?;

    if let Some(hmac) = hmac {
        let mut settings = settings::Settings::from_file(data_dir.clone(), network)?;
        let checksum = wallet.descriptor_checksum();
        if let Some(wallet_setting) = settings
            .wallets
            .iter_mut()
            .find(|w| w.descriptor_checksum == checksum)
        {
            let kind = hw.device_kind().to_string();
            if let Some(hw_config) = wallet_setting
                .hardware_wallets
                .iter_mut()
                .find(|cfg| cfg.kind == kind && cfg.fingerprint == fingerprint)
            {
                hw_config.token = hmac.to_hex();
            } else {
                wallet_setting.hardware_wallets.push(HardwareWalletConfig {
                    kind,
                    token: hmac.to_hex(),
                    fingerprint,
                })
            }
        }

        settings.to_file(data_dir, network)?;
    }

    Ok(fingerprint)
}

async fn list_hws(wallet: Arc<Wallet>) -> Vec<HardwareWallet> {
    list_hardware_wallets(
        &wallet.hardware_wallets,
        Some((&wallet.name, &wallet.main_descriptor.to_string())),
    )
    .await
}
