use std::collections::HashSet;
use std::convert::From;
use std::sync::Arc;

use iced::{Subscription, Task};

use coincube_core::{miniscript::bitcoin::bip32::Fingerprint, wallet_descriptor::WalletDescriptor};

use coincube_ui::{
    component::form,
    widget::{modal, Element},
};

use crate::{
    app::settings::WALLET_ALIAS_MAXIMUM_LENGTH,
    app::{
        cache::{Cache, NameSave},
        error::Error,
        menu::Menu,
        message::Message,
        settings::{self, update_settings_file},
        state::{vault::export::VaultExportModal, State},
        view,
        wallet::Wallet,
        Config,
    },
    daemon::Daemon,
    dir::CoincubeDirectory,
    export::{ImportExportMessage, ImportExportType},
    hw::{HardwareWallet, HardwareWalletConfig, HardwareWallets},
};

enum Modal {
    None,
    RegisterWallet(RegisterWalletModal),
    ImportExport(VaultExportModal),
}

impl Modal {
    fn is_none(&self) -> bool {
        matches!(self, Modal::None)
    }
}

pub struct WalletSettingsState {
    data_dir: CoincubeDirectory,
    warning: Option<Error>,
    descriptor: WalletDescriptor,
    keys_aliases: Vec<(Fingerprint, form::Value<String>)>,
    wallet: Arc<Wallet>,
    wallet_alias: form::Value<String>,
    /// How far the last Vault name save got, for the field's status line.
    name_save: NameSave,
    modal: Modal,
    processing: bool,
    updated: bool,
    expert_open: bool,
    descriptor_copy_review: bool,
    _config: Arc<Config>,
}

impl WalletSettingsState {
    pub fn new(data_dir: CoincubeDirectory, wallet: Arc<Wallet>, config: Arc<Config>) -> Self {
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
            name_save: NameSave::Idle,
            modal: Modal::None,
            processing: false,
            updated: false,
            expert_open: false,
            descriptor_copy_review: false,
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

        keys_aliases.sort_by_key(|(fg1, _)| *fg1);
        keys_aliases
    }
}

impl WalletSettingsState {
    /// The Vault name the Save button would write, or `None` when it would
    /// leave the saved name alone.
    fn changed_alias(&self) -> Option<String> {
        let typed = &self.wallet_alias.value;
        match self.wallet.alias.as_ref() {
            Some(saved) if saved == typed => None,
            None if typed.is_empty() => None,
            _ => Some(typed.clone()),
        }
    }

    /// The Vault name field's status line. A name too long to save already
    /// shows its own error, so it gets none.
    pub fn name_status(&self) -> Option<(crate::app::view::settings::page::Status, &'static str)> {
        crate::app::view::settings::page::local_name_status(
            self.wallet_alias.valid,
            self.changed_alias().is_some(),
            self.name_save,
        )
    }

    /// A save that leaves the Vault name alone means the saved name is the
    /// one wanted, so an earlier failed name save no longer applies.
    fn settle_failed_name(&mut self) {
        if self.name_save == NameSave::Failed {
            self.name_save = NameSave::Idle;
        }
    }

    /// Whether Settings offers "Register on hardware device". It does on
    /// every chain, Bitcoin Blake2b included (HW-1 Amendment B): devices may
    /// build a BTCB2 Vault, and the signing picker sends an unregistered
    /// device here.
    fn hardware_registration_allowed(&self) -> bool {
        self.wallet.supports_inheritance()
    }
}

impl State for WalletSettingsState {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        let content = view::vault::settings::wallet_settings(
            menu,
            cache,
            &self.descriptor,
            self.wallet.id_fingerprint(),
            &self.wallet_alias,
            &self.keys_aliases,
            &self.wallet.provider_keys,
            self.processing,
            self.updated,
            self.name_status(),
            self.hardware_registration_allowed(),
            self.expert_open,
        );

        if self.descriptor_copy_review {
            return modal::Modal::new(content, view::vault::settings::descriptor_copy_review())
                .on_blur(Some(view::Message::Settings(
                    view::SettingsMessage::CancelDescriptorCopy,
                )))
                .into();
        }
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
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        let Some(daemon) = daemon else {
            tracing::warn!("WalletSettingsState::update called without daemon");
            return Task::none();
        };
        match message {
            Message::WalletUpdated(res) => {
                self.processing = false;
                // Before the modal check: a Register opened while the save
                // ran takes the result, and the field must not stay Saving.
                if self.name_save == NameSave::Saving {
                    self.name_save = if res.is_ok() {
                        NameSave::Saved
                    } else {
                        NameSave::Failed
                    };
                }
                if let Modal::RegisterWallet(modal) = &mut self.modal {
                    modal.update(Some(daemon.clone()), cache, Message::WalletUpdated(res))
                } else {
                    match res {
                        Ok(wallet) => {
                            self.keys_aliases = Self::keys_aliases(&wallet);
                            self.wallet = wallet;
                            self.updated = true;
                            Task::none()
                        }
                        Err(e) => {
                            let err_msg = crate::user_error::report(&e);
                            self.warning = Some(e);
                            Task::done(Message::View(view::Message::ShowError(err_msg)))
                        }
                    }
                }
            }
            Message::View(view::Message::Settings(view::SettingsMessage::WalletAliasEdited(
                alias,
            ))) => {
                self.wallet_alias.valid = alias.len() < WALLET_ALIAS_MAXIMUM_LENGTH;
                self.wallet_alias.value = alias;
                self.updated = false;
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
                    self.updated = false;
                }
                Task::none()
            }
            Message::View(view::Message::Settings(view::SettingsMessage::Save)) => {
                self.modal = Modal::None;
                self.processing = true;
                self.updated = false;
                let alias = self.changed_alias();
                if alias.is_some() {
                    self.name_save = NameSave::Saving;
                } else {
                    self.settle_failed_name();
                }
                Task::perform(
                    update_aliases(
                        self.data_dir.clone(),
                        self.wallet.clone(),
                        alias,
                        self.keys_aliases
                            .iter()
                            .map(|(fg, name)| (*fg, name.value.to_owned()))
                            .collect(),
                    ),
                    Message::WalletUpdated,
                )
            }
            Message::View(view::Message::Settings(view::SettingsMessage::ToggleWalletExpert)) => {
                self.expert_open = !self.expert_open;
                self.descriptor_copy_review = false;
                Task::none()
            }
            Message::View(view::Message::Settings(view::SettingsMessage::ReviewDescriptorCopy)) => {
                if self.expert_open {
                    self.descriptor_copy_review = true;
                }
                Task::none()
            }
            Message::View(view::Message::Settings(view::SettingsMessage::CancelDescriptorCopy)) => {
                self.descriptor_copy_review = false;
                Task::none()
            }
            Message::View(view::Message::Settings(
                view::SettingsMessage::ConfirmDescriptorCopy,
            )) => {
                if !self.descriptor_copy_review || !self.expert_open {
                    return Task::none();
                }
                self.descriptor_copy_review = false;
                let descriptor_text = match &self.descriptor {
                    WalletDescriptor::Vault(vault) => vault.to_string(),
                    WalletDescriptor::Imported(_) => format!(
                        "Receive: {}\nChange: {}",
                        self.descriptor.receive_descriptor(),
                        self.descriptor.change_descriptor()
                    ),
                };
                Task::done(Message::View(view::Message::Clipboard(descriptor_text)))
            }
            Message::View(view::Message::Close) => {
                self.modal = Modal::None;
                Task::none()
            }
            Message::View(view::Message::Settings(view::SettingsMessage::RegisterWallet)) => {
                // Every chain, Bitcoin Blake2b included (HW-1): a device
                // registers the same descriptor string it did at creation.
                self.modal = Modal::RegisterWallet(RegisterWalletModal::new(
                    self.data_dir.clone(),
                    self.wallet.clone(),
                ));
                Task::none()
            }

            Message::View(view::Message::ImportExport(ImportExportMessage::UpdateAliases(
                aliases,
            ))) => {
                self.settle_failed_name();
                self.processing = true;
                self.updated = false;
                Task::perform(
                    update_aliases(
                        self.data_dir.clone(),
                        self.wallet.clone(),
                        None,
                        aliases.into_iter().map(|(fg, ks)| (fg, ks.name)).collect(),
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
                    let Some(descriptor) = self.wallet.inheritance_descriptor().cloned() else {
                        return Task::none();
                    };
                    let modal = VaultExportModal::new(
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
                Modal::RegisterWallet(m) => m.update(Some(daemon.clone()), cache, message),
                _ => Task::none(),
            },
        }
    }

    fn reload(
        &mut self,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        wallet: Option<Arc<Wallet>>,
    ) -> Task<Message> {
        let Some(daemon) = daemon else {
            tracing::warn!("WalletSettingsState::reload called without daemon");
            return Task::none();
        };
        let Some(wallet) = wallet else {
            tracing::warn!("WalletSettingsState::reload called without wallet");
            return Task::none();
        };
        self.descriptor = wallet.main_descriptor.clone();
        self.keys_aliases = Self::keys_aliases(&wallet);
        self.wallet = wallet;
        self.expert_open = false;
        self.descriptor_copy_review = false;
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
    data_dir: CoincubeDirectory,
    wallet: Arc<Wallet>,
    warning: Option<Error>,
    chosen_hw: Option<usize>,
    hws: HardwareWallets,
    registered: HashSet<Fingerprint>,
    processing: bool,
}

impl RegisterWalletModal {
    pub fn new(data_dir: CoincubeDirectory, wallet: Arc<Wallet>) -> Self {
        let mut registered = HashSet::new();
        for hw in &wallet.hardware_wallets {
            registered.insert(hw.fingerprint);
        }
        Self {
            data_dir: data_dir.clone(),
            warning: None,
            chosen_hw: None,
            hws: HardwareWallets::new(data_dir, wallet.chain.bitcoin_network())
                .with_wallet(wallet.clone()),
            wallet,
            processing: false,
            registered,
        }
    }
}

impl RegisterWalletModal {
    fn view(&self) -> Element<'_, view::Message> {
        view::vault::settings::register_wallet_modal(
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
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        _cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        if daemon.is_none() {
            tracing::warn!("RegisterWalletModal::update called without daemon");
            return Task::none();
        }
        match message {
            Message::View(view::Message::Reload) => {
                self.chosen_hw = None;
                self.warning = None;
                Task::none()
            }
            Message::HardwareWallets(msg) => match self.hws.update(msg) {
                Ok(cmd) => cmd.map(Message::HardwareWallets),
                Err(e) => {
                    let err: Error = e.into();
                    let err_msg = crate::user_error::report(&err);
                    self.warning = Some(err);
                    Task::done(Message::View(view::Message::ShowError(err_msg)))
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
                            let err_msg = crate::user_error::report(&e);
                            self.warning = Some(e);
                            return Task::done(Message::View(view::Message::ShowError(err_msg)));
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
                            device.clone(),
                            *fingerprint,
                            self.wallet.clone(),
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
    data_dir: CoincubeDirectory,
    hw: std::sync::Arc<dyn async_hwi::HWI + Send + Sync>,
    fingerprint: Fingerprint,
    wallet: Arc<Wallet>,
) -> Result<Arc<Wallet>, Error> {
    let descriptor = wallet.inheritance_descriptor().ok_or_else(|| {
        Error::Unexpected("This account uses an imported public descriptor.".into())
    })?;
    let hmac = hw
        .register_wallet(&wallet.name, &descriptor.to_string())
        .await
        .map_err(Error::from)?;

    if let Some(hmac) = hmac {
        let kind = hw.device_kind().to_string();
        let hw_cfg = HardwareWalletConfig {
            kind: kind.clone(),
            token: hex::encode(hmac),
            fingerprint,
        };

        let network_dir = data_dir.network_directory(wallet.chain);
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

            Some(settings)
        })
        .await?;

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
        return Ok(Arc::new(wallet));
    }

    Ok(wallet)
}

pub async fn update_aliases(
    data_dir: CoincubeDirectory,
    wallet: Arc<Wallet>,
    wallet_alias: Option<String>,
    keys_aliases: Vec<(Fingerprint, String)>,
) -> Result<Arc<Wallet>, Error> {
    let mut wallet = wallet.as_ref().clone();

    if let Some(wallet_alias) = wallet_alias.as_ref() {
        wallet = wallet.with_alias(Some(wallet_alias.clone()));
        let network_dir = data_dir.network_directory(wallet.chain);
        let wallet_id = wallet.id();
        update_settings_file(&network_dir, |mut settings| {
            if let Some(wallet_setting) = settings
                .wallets
                .iter_mut()
                .find(|w| w.wallet_id() == wallet_id)
            {
                wallet_setting.alias = Some(wallet_alias.clone());
            }

            Some(settings)
        })
        .await?;
    }

    let network_dir = data_dir.network_directory(wallet.chain);
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
                    is_border_wallet: wallet
                        .border_wallet_fingerprints
                        .contains(master_fingerprint),
                    // Carried through verbatim: this rewrite is about
                    // aliases, and re-deriving the provenance from nothing
                    // would silently downgrade every key to unrecorded.
                    grid_seed_source: wallet
                        .border_wallet_grid_seed
                        .get(master_fingerprint)
                        .copied(),
                    replay_protected: wallet.replay_marks.get(master_fingerprint).copied(),
                    keychain_key_id: wallet.keychain_key_ids.get(master_fingerprint).copied(),
                })
                .collect();
        }

        Some(settings)
    })
    .await?;

    wallet.keys_aliases = keys_aliases.into_iter().collect();

    Ok(Arc::new(wallet))
}

#[cfg(test)]
mod chain_tests {
    use super::*;
    use crate::chain::ChainId;
    use iced::futures::StreamExt;

    /// The Vault name field says what happened: not saved yet while typed,
    /// Saving after Return, then saved, or the failure. A later save that
    /// leaves the name alone (signer names only) clears the failure.
    #[tokio::test]
    async fn vault_name_field_shows_saving_then_saved_or_the_failure() {
        use crate::app::view::settings::page::Status;
        let descriptor = crate::app::state::vault::test_support::unified::fixture().descriptor;
        let wallet = Arc::new(Wallet::new(descriptor).with_chain(ChainId::Bitcoin));
        let dir = CoincubeDirectory::new(
            std::env::temp_dir().join(format!("coincube-vault-name-{}", uuid::Uuid::new_v4())),
        );
        let daemon: Arc<dyn Daemon + Sync + Send> = Arc::new(
            crate::daemon::client::Coincubed::new(crate::utils::mock::Daemon::new(vec![]).run()),
        );
        let mut state =
            WalletSettingsState::new(dir.clone(), wallet.clone(), Arc::new(Config::new(false)));
        let cache = Cache::default();
        let kind = |state: &WalletSettingsState| state.name_status().map(|(kind, _)| kind);
        let send = |state: &mut WalletSettingsState, message| {
            drop(state.update(Some(daemon.clone()), &cache, message))
        };
        let settings = |intent| Message::View(view::Message::Settings(intent));

        assert_eq!(kind(&state), None);
        send(&mut state, settings(view::SettingsMessage::Save));
        assert_eq!(kind(&state), None, "no name change, nothing to report");

        send(
            &mut state,
            settings(view::SettingsMessage::WalletAliasEdited("Savings".into())),
        );
        assert_eq!(kind(&state), Some(Status::Warn));
        send(&mut state, settings(view::SettingsMessage::Save));
        assert_eq!(kind(&state), Some(Status::Busy));
        send(
            &mut state,
            Message::WalletUpdated(Ok(Arc::new(
                wallet.as_ref().clone().with_alias(Some("Savings".into())),
            ))),
        );
        assert_eq!(kind(&state), Some(Status::Ok));

        send(
            &mut state,
            settings(view::SettingsMessage::WalletAliasEdited("Estate".into())),
        );
        send(&mut state, settings(view::SettingsMessage::Save));
        send(
            &mut state,
            Message::WalletUpdated(Err(Error::Unexpected("synthetic disk failure".into()))),
        );
        assert_eq!(kind(&state), Some(Status::Bad));

        // Back to the saved name, and a signer name changed: the save leaves
        // the Vault name alone and the failure no longer applies.
        send(
            &mut state,
            settings(view::SettingsMessage::WalletAliasEdited("Savings".into())),
        );
        let signer = state.keys_aliases[0].0;
        send(
            &mut state,
            settings(view::SettingsMessage::FingerprintAliasEdited(
                signer,
                "Phone".into(),
            )),
        );
        send(&mut state, settings(view::SettingsMessage::Save));
        assert_eq!(kind(&state), None);

        // Too long to save, even right after a save: the field's own error,
        // no status line.
        send(
            &mut state,
            settings(view::SettingsMessage::WalletAliasEdited("Holiday".into())),
        );
        send(&mut state, settings(view::SettingsMessage::Save));
        send(
            &mut state,
            Message::WalletUpdated(Ok(Arc::new(
                wallet.as_ref().clone().with_alias(Some("Holiday".into())),
            ))),
        );
        assert_eq!(kind(&state), Some(Status::Ok));
        send(
            &mut state,
            settings(view::SettingsMessage::WalletAliasEdited(
                "a".repeat(WALLET_ALIAS_MAXIMUM_LENGTH),
            )),
        );
        assert_eq!(kind(&state), None);
        let _ = std::fs::remove_dir_all(dir.path());
    }

    #[tokio::test]
    async fn descriptor_copy_requires_expert_review_and_emits_once() {
        let descriptor = crate::app::state::vault::test_support::unified::fixture().descriptor;
        let wallet = Arc::new(Wallet::new(descriptor.clone()).with_chain(ChainId::Bitcoin));
        let dir = CoincubeDirectory::new(
            std::env::temp_dir().join(format!("coincube-descriptor-copy-{}", uuid::Uuid::new_v4())),
        );
        let daemon: Arc<dyn Daemon + Sync + Send> = Arc::new(
            crate::daemon::client::Coincubed::new(crate::utils::mock::Daemon::new(vec![]).run()),
        );
        let mut state = WalletSettingsState::new(dir, wallet.clone(), Arc::new(Config::new(false)));
        let cache = Cache::default();
        let mut send = |intent| {
            state.update(
                Some(daemon.clone()),
                &cache,
                Message::View(view::Message::Settings(intent)),
            )
        };

        assert!(iced_runtime::task::into_stream(send(
            view::SettingsMessage::ConfirmDescriptorCopy,
        ))
        .is_none());
        drop(send(view::SettingsMessage::ReviewDescriptorCopy));
        assert!(iced_runtime::task::into_stream(send(
            view::SettingsMessage::ConfirmDescriptorCopy,
        ))
        .is_none());

        drop(send(view::SettingsMessage::ToggleWalletExpert));
        drop(send(view::SettingsMessage::ReviewDescriptorCopy));
        drop(send(view::SettingsMessage::CancelDescriptorCopy));
        assert!(iced_runtime::task::into_stream(send(
            view::SettingsMessage::ConfirmDescriptorCopy,
        ))
        .is_none());

        drop(send(view::SettingsMessage::ReviewDescriptorCopy));
        let mut stream =
            iced_runtime::task::into_stream(send(view::SettingsMessage::ConfirmDescriptorCopy))
                .expect("confirmed copy emits clipboard payload");
        match stream.next().await {
            Some(iced_runtime::Action::Output(Message::View(view::Message::Clipboard(text)))) => {
                assert_eq!(text, descriptor.to_string());
            }
            _ => panic!("confirmed copy did not emit the original Clipboard payload"),
        }
        assert!(stream.next().await.is_none(), "copy must emit once");
        assert!(iced_runtime::task::into_stream(send(
            view::SettingsMessage::ConfirmDescriptorCopy,
        ))
        .is_none());

        drop(send(view::SettingsMessage::ReviewDescriptorCopy));
        drop(state.reload(Some(daemon.clone()), Some(wallet.clone())));
        assert!(!state.expert_open && !state.descriptor_copy_review);
        assert!(iced_runtime::task::into_stream(state.update(
            Some(daemon),
            &cache,
            Message::View(view::Message::Settings(
                view::SettingsMessage::ConfirmDescriptorCopy,
            )),
        ))
        .is_none());
    }

    #[tokio::test]
    async fn alias_updates_touch_only_exact_chain_directory() {
        let descriptor = crate::app::state::vault::test_support::unified::fixture().descriptor;
        for chain in [
            ChainId::BitcoinBlake2b,
            ChainId::BitcoinBlake2bTestnet4,
            ChainId::Bitcoin,
        ] {
            let root =
                std::env::temp_dir().join(format!("coincube-settings-{}", uuid::Uuid::new_v4()));
            let dir = CoincubeDirectory::new(root.clone());
            let wallet = Arc::new(Wallet::new(descriptor.clone()).with_chain(chain));
            let other = if chain.is_blake2b() {
                ChainId::from(chain.bitcoin_network())
            } else {
                ChainId::BitcoinBlake2b
            };
            let settings = settings::Settings {
                wallets: vec![settings::WalletSettings {
                    name: wallet.name.clone(),
                    alias: None,
                    descriptor_checksum: wallet.descriptor_checksum.clone(),
                    pinned_at: wallet.pinned_at,
                    keys: vec![],
                    hardware_wallets: vec![],
                    remote_backend_auth: None,
                    start_internal_bitcoind: None,
                    pending_rescan: None,
                    keychain_keys_recorded: false,
                }],
                ..Default::default()
            };
            let original = serde_json::to_vec(&settings).unwrap();
            for target in [chain, other] {
                let path = dir.network_directory(target);
                std::fs::create_dir_all(path.path()).unwrap();
                std::fs::write(path.path().join(settings::SETTINGS_FILE_NAME), &original).unwrap();
            }
            let fingerprint = Fingerprint::from([1, 2, 3, 4]);
            let updated = update_aliases(
                dir.clone(),
                wallet,
                Some("isolated".into()),
                vec![(fingerprint, "synthetic key".into())],
            )
            .await
            .unwrap();
            assert_eq!(updated.alias.as_deref(), Some("isolated"));
            let stored = settings::Settings::from_file(&dir.network_directory(chain)).unwrap();
            assert_eq!(stored.wallets[0].alias.as_deref(), Some("isolated"));
            assert_eq!(stored.wallets[0].keys[0].master_fingerprint, fingerprint);
            assert_eq!(
                std::fs::read(
                    dir.network_directory(other)
                        .path()
                        .join(settings::SETTINGS_FILE_NAME)
                )
                .unwrap(),
                original
            );
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    /// Renaming a key rewrites the whole key list; the Keychain id must come
    /// through it, or the Pair panel would stop offering the renamed key.
    #[tokio::test]
    async fn alias_update_keeps_keychain_key_id() {
        use std::collections::HashMap;
        let descriptor = crate::app::state::vault::test_support::unified::fixture().descriptor;
        let root = std::env::temp_dir().join(format!("coincube-settings-{}", uuid::Uuid::new_v4()));
        let dir = CoincubeDirectory::new(root.clone());
        let phone = Fingerprint::from([1, 2, 3, 4]);
        let other = Fingerprint::from([5, 6, 7, 8]);
        let wallet = Arc::new(
            Wallet::new(descriptor)
                .with_chain(ChainId::Bitcoin)
                .with_keychain_keys(HashMap::from([(phone, 7)]), true),
        );
        let settings = settings::Settings {
            wallets: vec![settings::WalletSettings {
                name: wallet.name.clone(),
                alias: None,
                descriptor_checksum: wallet.descriptor_checksum.clone(),
                pinned_at: wallet.pinned_at,
                keys: vec![],
                hardware_wallets: vec![],
                remote_backend_auth: None,
                start_internal_bitcoind: None,
                pending_rescan: None,
                keychain_keys_recorded: true,
            }],
            ..Default::default()
        };
        let path = dir.network_directory(ChainId::Bitcoin);
        std::fs::create_dir_all(path.path()).unwrap();
        std::fs::write(
            path.path().join(settings::SETTINGS_FILE_NAME),
            serde_json::to_vec(&settings).unwrap(),
        )
        .unwrap();
        let updated = update_aliases(
            dir.clone(),
            wallet,
            None,
            vec![(phone, "Renamed phone".into()), (other, "Ledger".into())],
        )
        .await
        .unwrap();
        assert_eq!(updated.keychain_key_ids.get(&phone), Some(&7));

        let stored = settings::Settings::from_file(&path).unwrap();
        assert!(stored.wallets[0].keychain_keys_recorded);
        let keys: HashMap<_, _> = stored.wallets[0]
            .keys
            .iter()
            .map(|k| (k.master_fingerprint, k))
            .collect();
        assert_eq!(keys[&phone].name, "Renamed phone");
        assert_eq!(keys[&phone].keychain_key_id, Some(7));
        assert_eq!(keys[&other].keychain_key_id, None);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// HW-1 Amendment B: on both Bitcoin Blake2b chains, as on Bitcoin,
    /// Settings' "Register wallet" opens the registration modal with no
    /// warning.
    #[test]
    fn hardware_registration_message_opens_the_modal_on_every_chain() {
        for chain in [
            ChainId::BitcoinBlake2b,
            ChainId::BitcoinBlake2bTestnet4,
            ChainId::Bitcoin,
        ] {
            let wallet = Arc::new(
                Wallet::new(crate::app::state::vault::test_support::unified::fixture().descriptor)
                    .with_chain(chain),
            );
            let dir = CoincubeDirectory::new(
                std::env::temp_dir().join(format!("coincube-hw-register-{}", uuid::Uuid::new_v4())),
            );
            let daemon = Arc::new(crate::daemon::client::Coincubed::new(
                crate::utils::mock::Daemon::new(vec![]).run(),
            ));
            let mut state =
                WalletSettingsState::new(dir.clone(), wallet, Arc::new(Config::new(false)));
            let _task = state.update(
                Some(daemon),
                &Cache::default(),
                Message::View(view::Message::Settings(
                    view::SettingsMessage::RegisterWallet,
                )),
            );
            assert!(state.warning.is_none(), "{:?}", chain);
            assert!(
                matches!(state.modal, Modal::RegisterWallet(_)),
                "{:?}",
                chain
            );
            let _ = std::fs::remove_dir_all(dir.path());
        }
    }

    /// HW-1 Amendment B: the Settings view offers "Register on hardware
    /// device" on both Bitcoin Blake2b chains, as on Bitcoin.
    #[test]
    fn hardware_registration_is_offered_on_every_chain() {
        for chain in [
            ChainId::BitcoinBlake2b,
            ChainId::BitcoinBlake2bTestnet4,
            ChainId::Bitcoin,
            ChainId::Testnet4,
        ] {
            let wallet = Arc::new(
                Wallet::new(crate::app::state::vault::test_support::unified::fixture().descriptor)
                    .with_chain(chain),
            );
            let state = WalletSettingsState::new(
                CoincubeDirectory::new(std::path::PathBuf::new()),
                wallet,
                Arc::new(Config::new(false)),
            );
            assert!(state.hardware_registration_allowed(), "{:?}", chain);
        }
    }

    /// A device that records each registration request and answers with an
    /// HMAC.
    #[derive(Debug, Default)]
    struct RecordingDevice {
        calls: std::sync::Mutex<Vec<(String, String)>>,
    }

    #[async_trait::async_trait]
    impl async_hwi::HWI for RecordingDevice {
        fn device_kind(&self) -> async_hwi::DeviceKind {
            async_hwi::DeviceKind::Ledger
        }
        async fn get_version(&self) -> Result<async_hwi::Version, async_hwi::Error> {
            Err(async_hwi::Error::UnimplementedMethod)
        }
        async fn get_master_fingerprint(&self) -> Result<Fingerprint, async_hwi::Error> {
            Err(async_hwi::Error::UnimplementedMethod)
        }
        async fn get_extended_pubkey(
            &self,
            _path: &coincube_core::miniscript::bitcoin::bip32::DerivationPath,
        ) -> Result<coincube_core::miniscript::bitcoin::bip32::Xpub, async_hwi::Error> {
            Err(async_hwi::Error::UnimplementedMethod)
        }
        async fn register_wallet(
            &self,
            name: &str,
            policy: &str,
        ) -> Result<Option<[u8; 32]>, async_hwi::Error> {
            self.calls
                .lock()
                .unwrap()
                .push((name.to_string(), policy.to_string()));
            Ok(Some([7; 32]))
        }
        async fn is_wallet_registered(
            &self,
            _name: &str,
            _policy: &str,
        ) -> Result<bool, async_hwi::Error> {
            Ok(false)
        }
        async fn display_address(
            &self,
            _script: &async_hwi::AddressScript,
        ) -> Result<(), async_hwi::Error> {
            Err(async_hwi::Error::UnimplementedMethod)
        }
        async fn sign_tx(
            &self,
            _psbt: &mut coincube_core::miniscript::bitcoin::Psbt,
        ) -> Result<(), async_hwi::Error> {
            Err(async_hwi::Error::UnimplementedMethod)
        }
    }

    /// HW-1 Amendment B: on both Bitcoin Blake2b chains, as on Bitcoin,
    /// `register_wallet` sends the device the Vault's name and descriptor
    /// string unchanged, stores the returned HMAC in that chain's settings
    /// only, and returns the Vault with the device recorded.
    #[tokio::test]
    async fn register_wallet_reaches_the_device_on_every_chain() {
        let descriptor = crate::app::state::vault::test_support::unified::fixture().descriptor;
        let fingerprint = Fingerprint::from([9, 9, 9, 9]);
        for chain in [
            ChainId::BitcoinBlake2b,
            ChainId::BitcoinBlake2bTestnet4,
            ChainId::Bitcoin,
        ] {
            let root =
                std::env::temp_dir().join(format!("coincube-hw-register-{}", uuid::Uuid::new_v4()));
            let dir = CoincubeDirectory::new(root.clone());
            let wallet = Arc::new(Wallet::new(descriptor.clone()).with_chain(chain));
            let other = if chain.is_blake2b() {
                ChainId::from(chain.bitcoin_network())
            } else {
                ChainId::BitcoinBlake2b
            };
            let settings = settings::Settings {
                wallets: vec![settings::WalletSettings {
                    name: wallet.name.clone(),
                    alias: None,
                    descriptor_checksum: wallet.descriptor_checksum.clone(),
                    pinned_at: wallet.pinned_at,
                    keys: vec![],
                    hardware_wallets: vec![],
                    remote_backend_auth: None,
                    start_internal_bitcoind: None,
                    pending_rescan: None,
                    keychain_keys_recorded: false,
                }],
                ..Default::default()
            };
            let original = serde_json::to_vec(&settings).unwrap();
            for target in [chain, other] {
                let path = dir.network_directory(target);
                std::fs::create_dir_all(path.path()).unwrap();
                std::fs::write(path.path().join(settings::SETTINGS_FILE_NAME), &original).unwrap();
            }
            let device = Arc::new(RecordingDevice::default());
            let updated = register_wallet(dir.clone(), device.clone(), fingerprint, wallet.clone())
                .await
                .unwrap_or_else(|e| panic!("{:?}: {:?}", chain, e));

            assert_eq!(
                *device.calls.lock().unwrap(),
                vec![(
                    wallet.name.clone(),
                    wallet.inheritance_descriptor().unwrap().to_string()
                )],
                "{:?}",
                chain
            );
            let fields = |cfgs: &[HardwareWalletConfig]| {
                cfgs.iter()
                    .map(|c| (c.kind.clone(), c.token.clone(), c.fingerprint))
                    .collect::<Vec<_>>()
            };
            let expected = vec![(
                async_hwi::DeviceKind::Ledger.to_string(),
                hex::encode([7u8; 32]),
                fingerprint,
            )];
            assert_eq!(fields(&updated.hardware_wallets), expected, "{:?}", chain);
            let stored = settings::Settings::from_file(&dir.network_directory(chain)).unwrap();
            assert_eq!(
                fields(&stored.wallets[0].hardware_wallets),
                expected,
                "{:?}",
                chain
            );
            assert_eq!(
                std::fs::read(
                    dir.network_directory(other)
                        .path()
                        .join(settings::SETTINGS_FILE_NAME)
                )
                .unwrap(),
                original
            );
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}
