use std::collections::HashSet;
use std::convert::From;
use std::sync::Arc;

use iced::{Subscription, Task};

use liana::{
    descriptors::LianaDescriptor,
    miniscript::bitcoin::{bip32::Fingerprint, Network},
};

use liana_ui::{
    component::{form, modal},
    widget::Element,
};

use crate::{
    app::{
        cache::Cache,
        error::Error,
        message::Message,
        settings::{self, update_settings_file},
        state::{export::ExportModal, State},
        view,
        wallet::Wallet,
        Config,
    },
    daemon::{Daemon, DaemonBackend},
    dir::LianaDirectory,
    export::{ImportExportMessage, ImportExportType},
    hw::{HardwareWallet, HardwareWalletConfig, HardwareWallets},
    services::connect::client::backend::api::WALLET_ALIAS_MAXIMUM_LENGTH,
};

enum Modal {
    None,
    RegisterWallet(RegisterWalletModal),
    ImportExport(ExportModal),
}

impl Modal {
    fn is_none(&self) -> bool {
        matches!(self, Modal::None)
    }
}

pub struct WalletSettingsState {
    data_dir: LianaDirectory,
    warning: Option<Error>,
    descriptor: LianaDescriptor,
    keys_aliases: Vec<(Fingerprint, form::Value<String>)>,
    wallet: Arc<Wallet>,
    wallet_alias: form::Value<String>,
    modal: Modal,
    processing: bool,
    updated: bool,
    config: Arc<Config>,
}

impl WalletSettingsState {
    pub fn new(data_dir: LianaDirectory, wallet: Arc<Wallet>, config: Arc<Config>) -> Self {
        WalletSettingsState {
            data_dir,
            descriptor: wallet.main_descriptor.clone(),
            keys_aliases: Self::keys_aliases(&wallet),
            wallet_alias: form::Value {
                value: wallet.alias.clone().unwrap_or_default(),
                valid: true,
            },
            wallet,
            warning: None,
            modal: Modal::None,
            processing: false,
            updated: false,
            config,
        }
    }

    fn keys_aliases(wallet: &Wallet) -> Vec<(Fingerprint, form::Value<String>)> {
        let mut keys_aliases: Vec<(Fingerprint, form::Value<String>)> = wallet
            .keys_aliases
            .clone()
            .into_iter()
            .map(|(fg, name)| {
                (
                    fg,
                    form::Value {
                        value: name,
                        valid: true,
                    },
                )
            })
            .collect();

        for fingerprint in wallet.descriptor_keys().into_iter() {
            if !wallet.keys_aliases.contains_key(&fingerprint) {
                keys_aliases.push((fingerprint, form::Value::default()));
            }
        }

        keys_aliases.sort_by(|(fg1, _), (fg2, _)| fg1.cmp(fg2));
        keys_aliases
    }
}

impl State for WalletSettingsState {
    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        let content = view::settings::wallet_settings(
            cache,
            self.warning.as_ref(),
            &self.descriptor,
            &self.wallet_alias,
            &self.keys_aliases,
            &self.wallet.provider_keys,
            self.processing,
            self.updated,
        );

        match &self.modal {
            Modal::None => content,
            Modal::RegisterWallet(m) => modal::Modal::new(content, m.view())
                .on_blur(Some(view::Message::Close))
                .into(),
            Modal::ImportExport(m) => m.view(content),
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        match &self.modal {
            Modal::None => Subscription::none(),
            Modal::RegisterWallet(modal) => modal.subscription(),
            Modal::ImportExport(modal) => {
                if let Some(sub) = modal.subscription() {
                    sub.map(|m| {
                        Message::View(view::Message::Settings(
                            view::SettingsMessage::ImportExport(ImportExportMessage::Progress(m)),
                        ))
                    })
                } else {
                    Subscription::none()
                }
            }
        }
    }

    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        match message {
            Message::WalletUpdated(res) => {
                self.processing = false;
                if let Modal::RegisterWallet(modal) = &mut self.modal {
                    modal.update(daemon, cache, Message::WalletUpdated(res))
                } else {
                    match res {
                        Ok(wallet) => {
                            self.keys_aliases = Self::keys_aliases(&wallet);
                            self.wallet = wallet;
                            self.updated = true;
                        }
                        Err(e) => self.warning = Some(e),
                    };
                    Task::none()
                }
            }
            Message::View(view::Message::Settings(view::SettingsMessage::WalletAliasEdited(
                alias,
            ))) => {
                self.wallet_alias.valid = alias.len() < WALLET_ALIAS_MAXIMUM_LENGTH;
                self.wallet_alias.value = alias;
                Task::none()
            }
            Message::View(view::Message::Settings(
                view::SettingsMessage::FingerprintAliasEdited(fg, value),
            )) => {
                if let Some((_, name)) = self
                    .keys_aliases
                    .iter_mut()
                    .find(|(fingerprint, _)| fg == *fingerprint)
                {
                    name.value = value;
                }
                Task::none()
            }
            Message::View(view::Message::Settings(view::SettingsMessage::Save)) => {
                self.modal = Modal::None;
                self.processing = true;
                self.updated = false;
                Task::perform(
                    update_aliases(
                        self.data_dir.clone(),
                        cache.network,
                        self.wallet.clone(),
                        match self
                            .wallet
                            .alias
                            .as_ref()
                            .map(|a| *a == self.wallet_alias.value)
                        {
                            Some(true) => None,
                            Some(false) => Some(self.wallet_alias.value.clone()),
                            None => {
                                if self.wallet_alias.value.is_empty() {
                                    None
                                } else {
                                    Some(self.wallet_alias.value.clone())
                                }
                            }
                        },
                        self.keys_aliases
                            .iter()
                            .map(|(fg, name)| (*fg, name.value.to_owned()))
                            .collect(),
                        daemon,
                    ),
                    Message::WalletUpdated,
                )
            }
            Message::View(view::Message::Close) => {
                self.modal = Modal::None;
                Task::none()
            }
            Message::View(view::Message::Settings(view::SettingsMessage::RegisterWallet)) => {
                self.modal = Modal::RegisterWallet(RegisterWalletModal::new(
                    self.data_dir.clone(),
                    self.wallet.clone(),
                    cache.network,
                ));
                Task::none()
            }

            Message::View(view::Message::ImportExport(ImportExportMessage::UpdateAliases(
                aliases,
            ))) => {
                self.processing = true;
                self.updated = false;
                Task::perform(
                    update_aliases(
                        self.data_dir.clone(),
                        cache.network,
                        self.wallet.clone(),
                        None,
                        aliases.into_iter().map(|(fg, ks)| (fg, ks.name)).collect(),
                        daemon,
                    ),
                    Message::WalletUpdated,
                )
            }
            Message::View(view::Message::ImportExport(ImportExportMessage::Close)) => {
                if let Modal::ImportExport(_) = &self.modal {
                    self.modal = Modal::None;
                }
                Task::none()
            }
            Message::View(view::Message::ImportExport(m)) => {
                if let Modal::ImportExport(modal) = &mut self.modal {
                    modal.update(m)
                } else {
                    Task::none()
                }
            }
            Message::View(view::Message::Settings(view::SettingsMessage::ImportExport(m))) => {
                if let Modal::ImportExport(modal) = &mut self.modal {
                    modal.update(m)
                } else {
                    Task::none()
                }
            }
            Message::View(view::Message::Settings(view::SettingsMessage::ExportWallet)) => {
                if self.modal.is_none() {
                    let datadir = cache.datadir_path.clone();
                    let network = cache.network;
                    let config = self.config.clone();
                    let wallet = self.wallet.clone();
                    let daemon = daemon.clone();
                    let modal = ExportModal::new(
                        Some(daemon),
                        ImportExportType::ExportProcessBackup(datadir, network, config, wallet),
                    );
                    let launch = modal.launch(true);
                    self.modal = Modal::ImportExport(modal);
                    launch
                } else {
                    Task::none()
                }
            }
            Message::View(view::Message::Settings(view::SettingsMessage::ImportWallet)) => {
                if self.modal.is_none() {
                    let modal = ExportModal::new(
                        Some(daemon),
                        ImportExportType::ImportBackup {
                            network_dir: cache.datadir_path.network_directory(cache.network),
                            wallet: self.wallet.clone(),
                            overwrite_labels: None,
                            overwrite_aliases: None,
                        },
                    );
                    let launch = modal.launch(false);
                    self.modal = Modal::ImportExport(modal);
                    launch
                } else {
                    Task::none()
                }
            }
            _ => match &mut self.modal {
                Modal::RegisterWallet(m) => m.update(daemon, cache, message),
                _ => Task::none(),
            },
        }
    }

    fn reload(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        wallet: Arc<Wallet>,
    ) -> Task<Message> {
        self.descriptor = wallet.main_descriptor.clone();
        self.keys_aliases = Self::keys_aliases(&wallet);
        self.wallet = wallet;
        Task::perform(
            async move { daemon.get_info().await.map_err(|e| e.into()) },
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
    data_dir: LianaDirectory,
    wallet: Arc<Wallet>,
    warning: Option<Error>,
    chosen_hw: Option<usize>,
    hws: HardwareWallets,
    registered: HashSet<Fingerprint>,
    processing: bool,
}

impl RegisterWalletModal {
    pub fn new(data_dir: LianaDirectory, wallet: Arc<Wallet>, network: Network) -> Self {
        let mut registered = HashSet::new();
        for hw in &wallet.hardware_wallets {
            registered.insert(hw.fingerprint);
        }
        Self {
            data_dir: data_dir.clone(),
            warning: None,
            chosen_hw: None,
            hws: HardwareWallets::new(data_dir, network).with_wallet(wallet.clone()),
            wallet,
            processing: false,
            registered,
        }
    }
}

impl RegisterWalletModal {
    fn view(&self) -> Element<view::Message> {
        view::settings::register_wallet_modal(
            self.warning.as_ref(),
            &self.hws.list,
            self.processing,
            self.chosen_hw,
            &self.registered,
        )
    }

    fn subscription(&self) -> Subscription<Message> {
        self.hws.refresh().map(Message::HardwareWallets)
    }

    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        match message {
            Message::View(view::Message::Reload) => {
                self.chosen_hw = None;
                self.warning = None;
                Task::none()
            }
            Message::HardwareWallets(msg) => match self.hws.update(msg) {
                Ok(cmd) => cmd.map(Message::HardwareWallets),
                Err(e) => {
                    self.warning = Some(e.into());
                    Task::none()
                }
            },
            Message::WalletUpdated(res) => {
                self.processing = false;
                self.chosen_hw = None;
                match res {
                    Ok(wallet) => {
                        self.registered = HashSet::new();
                        for hw in &wallet.hardware_wallets {
                            self.registered.insert(hw.fingerprint);
                        }
                        self.wallet = wallet;
                    }
                    Err(e) => {
                        if !matches!(e, Error::HardwareWallet(async_hwi::Error::UserRefused)) {
                            self.warning = Some(e)
                        }
                    }
                }
                Task::none()
            }
            Message::View(view::Message::SelectHardwareWallet(i)) => {
                if let Some(HardwareWallet::Supported {
                    fingerprint,
                    device,
                    ..
                }) = self.hws.list.get(i)
                {
                    self.chosen_hw = Some(i);
                    self.processing = true;
                    Task::perform(
                        register_wallet(
                            self.data_dir.clone(),
                            cache.network,
                            device.clone(),
                            *fingerprint,
                            self.wallet.clone(),
                            daemon,
                        ),
                        Message::WalletUpdated,
                    )
                } else {
                    Task::none()
                }
            }
            _ => Task::none(),
        }
    }
}

async fn register_wallet(
    data_dir: LianaDirectory,
    network: Network,
    hw: std::sync::Arc<dyn async_hwi::HWI + Send + Sync>,
    fingerprint: Fingerprint,
    wallet: Arc<Wallet>,
    daemon: Arc<dyn Daemon + Sync + Send>,
) -> Result<Arc<Wallet>, Error> {
    let hmac = hw
        .register_wallet(&wallet.name, &wallet.main_descriptor.to_string())
        .await
        .map_err(Error::from)?;

    if let Some(hmac) = hmac {
        let kind = hw.device_kind().to_string();
        let hw_cfg = HardwareWalletConfig {
            kind: kind.clone(),
            token: hex::encode(hmac),
            fingerprint,
        };

        if daemon.backend() != DaemonBackend::RemoteBackend {
            let network_dir = data_dir.network_directory(network);
            let wallet_id = wallet.id();
            update_settings_file(&network_dir, |mut settings| {
                if let Some(wallet_setting) = settings
                    .wallets
                    .iter_mut()
                    .find(|w| w.wallet_id() == wallet_id)
                {
                    if let Some(hw_config) = wallet_setting
                        .hardware_wallets
                        .iter_mut()
                        .find(|cfg| cfg.kind == kind && cfg.fingerprint == fingerprint)
                    {
                        *hw_config = hw_cfg.clone();
                    } else {
                        wallet_setting.hardware_wallets.push(hw_cfg.clone())
                    }
                }

                settings
            })
            .await?;
        }

        let mut wallet = wallet.as_ref().clone();
        if let Some(hw_config) = wallet
            .hardware_wallets
            .iter_mut()
            .find(|cfg| cfg.kind == kind && cfg.fingerprint == fingerprint)
        {
            *hw_config = hw_cfg.clone();
        } else {
            wallet.hardware_wallets.push(hw_cfg)
        }
        daemon
            .update_wallet_metadata(None, &wallet.keys_aliases, &wallet.hardware_wallets)
            .await?;
        return Ok(Arc::new(wallet));
    }

    Ok(wallet)
}

pub async fn update_aliases(
    data_dir: LianaDirectory,
    network: Network,
    wallet: Arc<Wallet>,
    wallet_alias: Option<String>,
    keys_aliases: Vec<(Fingerprint, String)>,
    daemon: Arc<dyn Daemon + Sync + Send>,
) -> Result<Arc<Wallet>, Error> {
    let mut wallet = wallet.as_ref().clone();

    if let Some(wallet_alias) = wallet_alias.as_ref() {
        wallet = wallet.with_alias(Some(wallet_alias.clone()));
        let network_dir = data_dir.network_directory(network);
        let wallet_id = wallet.id();
        update_settings_file(&network_dir, |mut settings| {
            if let Some(wallet_setting) = settings
                .wallets
                .iter_mut()
                .find(|w| w.wallet_id() == wallet_id)
            {
                wallet_setting.alias = Some(wallet_alias.clone());
            }

            settings
        })
        .await?;
    }

    if daemon.backend() != DaemonBackend::RemoteBackend {
        let network_dir = data_dir.network_directory(network);
        let wallet_id = wallet.id();
        update_settings_file(&network_dir, |mut settings| {
            if let Some(wallet_setting) = settings
                .wallets
                .iter_mut()
                .find(|w| w.wallet_id() == wallet_id)
            {
                wallet_setting.keys = keys_aliases
                    .iter()
                    .map(|(master_fingerprint, name)| settings::KeySetting {
                        master_fingerprint: *master_fingerprint,
                        name: name.clone(),
                        provider_key: wallet.provider_keys.get(master_fingerprint).cloned(),
                    })
                    .collect();
            }

            settings
        })
        .await?;
    }

    wallet.keys_aliases = keys_aliases.into_iter().collect();

    daemon
        .update_wallet_metadata(wallet_alias, &wallet.keys_aliases, &wallet.hardware_wallets)
        .await?;

    Ok(Arc::new(wallet))
}
