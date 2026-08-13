use std::collections::HashSet;
use std::convert::From;
use std::sync::Arc;

use iced::{Subscription, Task};

use liana::{
    descriptors::LianaDescriptor,
    miniscript::bitcoin::{bip32::Fingerprint, Network},
};

use liana_ui::{
    component::form,
    widget::{modal, Element},
};

use crate::{
    airgap::{AirgappedRequest, AirgappedSignerConfig, PolicyRegistration, RegistrationState},
    app::{
        cache::Cache,
        error::Error,
        message::Message,
        settings::{self, update_settings_file, LianaSettings},
        state::{
            airgap::{AirgapModal, AirgapOutcome},
            export::ExportModal,
            State,
        },
        view,
        wallet::Wallet,
        Config,
    },
    daemon::{Daemon, DaemonBackend},
    dir::LianaDirectory,
    export::{ImportExportMessage, ImportExportType},
    hw::{HardwareWallet, HardwareWalletConfig, HardwareWallets},
    services::connect::client::backend::WALLET_ALIAS_MAXIMUM_LENGTH,
};

#[allow(clippy::large_enum_variant)]
enum Modal {
    None,
    RegisterWallet(RegisterWalletModal),
    RegisterAirgappedSigner(AirgappedRegistrationModal),
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
    _config: Arc<Config>,
}

impl WalletSettingsState {
    pub fn new(data_dir: LianaDirectory, wallet: Arc<Wallet>, config: Arc<Config>) -> Self {
        WalletSettingsState {
            data_dir,
            descriptor: wallet.main_descriptor.clone(),
            keys_aliases: Self::keys_aliases(&wallet),
            wallet_alias: form::Value {
                value: wallet.alias.clone().unwrap_or_default(),
                warning: None,
                valid: true,
            },
            wallet,
            warning: None,
            modal: Modal::None,
            processing: false,
            updated: false,
            _config: config,
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
                        warning: None,
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
            Modal::RegisterAirgappedSigner(m) => {
                modal::Modal::new(content, m.exchange.view()).into()
            }
            Modal::ImportExport(m) => m.view(content),
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        match &self.modal {
            Modal::None => Subscription::none(),
            Modal::RegisterWallet(modal) => modal.subscription(),
            Modal::RegisterAirgappedSigner(modal) => modal.exchange.subscription(),
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
            Message::View(view::Message::Settings(
                view::SettingsMessage::RegisterAirgappedSigner(fingerprint),
            )) => {
                match AirgappedRegistrationModal::new(
                    self.wallet.clone(),
                    cache.network,
                    fingerprint,
                ) {
                    Ok(modal) => self.modal = Modal::RegisterAirgappedSigner(modal),
                    Err(error) => self.warning = Some(Error::Unexpected(error)),
                }
                Task::none()
            }
            Message::View(view::Message::Airgap(action)) => {
                let Modal::RegisterAirgappedSigner(modal) = &mut self.modal else {
                    return Task::none();
                };
                let command = modal.exchange.update(action);
                let Some(outcome) = modal.exchange.take_outcome() else {
                    return command;
                };
                let registration = modal.registration.clone();
                let signer = modal.signer.clone();
                let state = match outcome {
                    AirgapOutcome::Exported => RegistrationState::Exported {
                        descriptor_checksum: registration
                            .descriptor_checksum()
                            .expect("validated registration has a checksum"),
                    },
                    AirgapOutcome::Cancelled => {
                        self.modal = Modal::None;
                        return Task::none();
                    }
                    _ => {
                        self.warning = Some(Error::Unexpected(
                            "Signer returned the wrong response".to_owned(),
                        ));
                        self.modal = Modal::None;
                        return Task::none();
                    }
                };
                self.processing = true;
                self.modal = Modal::None;
                Task::perform(
                    update_airgapped_registration(
                        self.data_dir.clone(),
                        cache.network,
                        self.wallet.clone(),
                        signer,
                        state,
                    ),
                    Message::WalletUpdated,
                )
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
            Message::View(view::Message::Settings(
                view::SettingsMessage::ExportEncryptedDescriptor,
            )) => {
                if self.modal.is_none() {
                    let descriptor = self.wallet.main_descriptor.clone();
                    let modal = ExportModal::new(
                        Some(daemon),
                        ImportExportType::ExportEncryptedDescriptor(Box::new(descriptor)),
                    );
                    let launch = modal.launch(true);
                    self.modal = Modal::ImportExport(modal);
                    return launch;
                }
                Task::none()
            }
            _ => match &mut self.modal {
                Modal::RegisterWallet(m) => m.update(daemon, cache, message),
                Modal::RegisterAirgappedSigner(_) => Task::none(),
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

struct AirgappedRegistrationModal {
    exchange: AirgapModal,
    registration: PolicyRegistration,
    signer: AirgappedSignerConfig,
}

impl AirgappedRegistrationModal {
    fn new(
        wallet: Arc<Wallet>,
        network: Network,
        fingerprint: Fingerprint,
    ) -> Result<Self, String> {
        let signer = wallet
            .airgapped_signer_candidates(network)
            .into_iter()
            .find(|signer| signer.fingerprint == fingerprint)
            .ok_or_else(|| "Air-gapped signer is not configured for this wallet".to_owned())?;
        let registration = PolicyRegistration::from_descriptor(
            wallet.name.clone(),
            network,
            &wallet.main_descriptor,
        )
        .map_err(|error| error.to_string())?;
        let filename = format!("liana-{}-policy.json", wallet.descriptor_checksum);
        Ok(Self {
            exchange: AirgapModal::new(
                "Register wallet policy on air-gapped signer",
                AirgappedRequest::RegisterPolicy(registration.clone()),
                filename,
            ),
            registration,
            signer,
        })
    }
}

async fn update_airgapped_registration(
    data_dir: LianaDirectory,
    network: Network,
    wallet: Arc<Wallet>,
    signer: AirgappedSignerConfig,
    registration: RegistrationState,
) -> Result<Arc<Wallet>, Error> {
    let mut wallet = wallet.as_ref().clone();
    apply_airgapped_registration(&mut wallet, signer, registration);
    let signers: Vec<AirgappedSignerConfig> = wallet.airgapped_signers.clone();
    let wallet_id = wallet.id();
    let network_dir = data_dir.network_directory(network);
    update_settings_file(&network_dir, |mut settings: LianaSettings| {
        if let Some(wallet_setting) = settings
            .wallets
            .iter_mut()
            .find(|candidate| candidate.wallet_id() == wallet_id)
        {
            wallet_setting.airgapped_signers = signers.clone();
        }
        settings
    })
    .await?;
    Ok(Arc::new(wallet))
}

fn apply_airgapped_registration(
    wallet: &mut Wallet,
    mut signer: AirgappedSignerConfig,
    registration: RegistrationState,
) {
    if let Some(existing) = wallet
        .airgapped_signers
        .iter_mut()
        .find(|existing| existing.fingerprint == signer.fingerprint)
    {
        existing.registration = registration;
    } else {
        // Legacy wallets are migrated only after the user confirms that the
        // policy was registered, keeping cancellation side-effect free.
        signer.registration = registration;
        wallet.airgapped_signers.push(signer);
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
    airgapped_signers: Vec<AirgappedSignerConfig>,
    registered: HashSet<Fingerprint>,
    processing: bool,
}

impl RegisterWalletModal {
    pub fn new(data_dir: LianaDirectory, wallet: Arc<Wallet>, network: Network) -> Self {
        let mut registered = HashSet::new();
        for hw in &wallet.hardware_wallets {
            registered.insert(hw.fingerprint);
        }
        let airgapped_signers = wallet.airgapped_signer_candidates(network);
        Self {
            data_dir: data_dir.clone(),
            warning: None,
            chosen_hw: None,
            hws: HardwareWallets::new(data_dir, network).with_wallet(wallet.clone()),
            airgapped_signers,
            wallet,
            processing: false,
            registered,
        }
    }
}

impl RegisterWalletModal {
    pub fn view(&self) -> Element<'_, view::Message> {
        view::settings::register_wallet_modal(
            self.warning.as_ref(),
            &self.hws.list,
            &self.airgapped_signers,
            &self.wallet.main_descriptor,
            self.processing,
            self.chosen_hw,
            &self.registered,
        )
    }

    pub fn subscription(&self) -> Subscription<Message> {
        self.hws.refresh().map(Message::HardwareWallets)
    }

    pub fn update(
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
                        self.airgapped_signers = wallet.airgapped_signer_candidates(cache.network);
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

pub async fn register_wallet(
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
            update_settings_file(&network_dir, |mut settings: LianaSettings| {
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
        update_settings_file(&network_dir, |mut settings: LianaSettings| {
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
        update_settings_file(&network_dir, |mut settings: LianaSettings| {
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

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    const LEGACY_DESCRIPTOR: &str = "wsh(or_d(multi(2,[f714c228/48'/1'/0'/2']tpubDEwJnTwfKoMvu8AXXBPydBVWDpzNP5tatjjZ56q4TQioGL7iL9xzTbMoCCQ3tfGihtff7vtR4xsjcRuhZ7HWARVAkGZ1HZcpBhVdou76k7j/<0;1>/*,[2522f23c/48'/1'/0'/2']tpubDEoTU4bDW1EXN1rnLXnRfue1a7DeqjJcs39PkEeLcVXhVKzCnFo9yQX2EeeXJ6kh4hgbz5o9v7YAc1EE97AEJpJbKNmDxE3ZQo4msGPSp2J/<0;1>/*),and_v(v:thresh(1,pkh([f714c228/48'/1'/0'/2']tpubDEwJnTwfKoMvu8AXXBPydBVWDpzNP5tatjjZ56q4TQioGL7iL9xzTbMoCCQ3tfGihtff7vtR4xsjcRuhZ7HWARVAkGZ1HZcpBhVdou76k7j/<2;3>/*),a:pkh([2522f23c/48'/1'/0'/2']tpubDEoTU4bDW1EXN1rnLXnRfue1a7DeqjJcs39PkEeLcVXhVKzCnFo9yQX2EeeXJ6kh4hgbz5o9v7YAc1EE97AEJpJbKNmDxE3ZQo4msGPSp2J/<2;3>/*)),older(65535))))#9s8ekrce";

    fn legacy_wallet() -> Wallet {
        Wallet::new(LianaDescriptor::from_str(LEGACY_DESCRIPTOR).unwrap())
    }

    #[test]
    fn legacy_bip48_keys_are_offered_as_qr_signer_candidates() {
        let mut wallet = legacy_wallet();
        let fingerprint = Fingerprint::from_str("f714c228").unwrap();
        wallet
            .keys_aliases
            .insert(fingerprint, "Legacy QR signer".to_owned());

        let signers = wallet.airgapped_signer_candidates(Network::Testnet4);

        assert_eq!(signers.len(), 2);
        assert_eq!(
            signers
                .iter()
                .find(|signer| signer.fingerprint == fingerprint)
                .and_then(|signer| signer.alias.as_deref()),
            Some("Legacy QR signer")
        );
    }

    #[test]
    fn known_usb_keys_are_not_migrated_to_qr_signers() {
        let mut wallet = legacy_wallet();
        let fingerprint = Fingerprint::from_str("f714c228").unwrap();
        wallet.hardware_wallets.push(HardwareWalletConfig {
            kind: "ledger".to_owned(),
            fingerprint,
            token: String::new(),
        });

        let signers = wallet.airgapped_signer_candidates(Network::Testnet4);

        assert_eq!(signers.len(), 1);
        assert_ne!(signers[0].fingerprint, fingerprint);
    }

    #[test]
    fn confirmed_legacy_candidate_is_added_without_duplicates() {
        let mut wallet = legacy_wallet();
        let signer = wallet
            .airgapped_signer_candidates(Network::Testnet4)
            .into_iter()
            .next()
            .unwrap();
        let fingerprint = signer.fingerprint;
        let registration = RegistrationState::Exported {
            descriptor_checksum: wallet.descriptor_checksum.clone(),
        };

        apply_airgapped_registration(&mut wallet, signer.clone(), registration.clone());
        apply_airgapped_registration(&mut wallet, signer, registration.clone());

        assert_eq!(wallet.airgapped_signers.len(), 1);
        assert_eq!(wallet.airgapped_signers[0].fingerprint, fingerprint);
        assert_eq!(wallet.airgapped_signers[0].registration, registration);
    }
}
