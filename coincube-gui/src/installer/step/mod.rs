pub mod descriptor;
pub mod import_descriptor;

mod backend;
mod coincube_connect;
mod mnemonic;
pub(crate) mod node;
mod share_xpubs;
mod wallet_alias;

pub use node::{
    bitcoind::{
        DownloadState, DownloadUpdate, InstallState, InternalBitcoindStep, SelectBitcoindTypeStep,
    },
    DefineNode,
};

pub use descriptor::{
    editor::template::{ChooseDescriptorTemplate, DescriptorTemplateDescription},
    editor::DefineDescriptor,
    BackupDescriptor, ImportDescriptor, RegisterDescriptor,
};

pub use backend::{ChooseBackend, ImportRemoteWallet, RemoteBackendLogin};
pub use coincube_connect::CoincubeConnectStep;
pub use mnemonic::{BackupMnemonic, RecoverMnemonic};
pub use share_xpubs::ShareXpubs;
use tracing::warn;
pub use wallet_alias::WalletAlias;

use std::collections::HashMap;

use iced::{Subscription, Task};

use coincube_ui::widget::*;

use crate::{
    app::settings::{ProviderKey, WalletSettings},
    hw::HardwareWallets,
    installer::{
        connect_vault::{self, ConnectVaultError, ConnectVaultOutcome},
        context::{ConnectVaultMemberPayload, Context},
        message::Message,
        view,
    },
    node::bitcoind::Bitcoind,
    services::{self, coincube::CoincubeClient},
};

pub trait Step {
    fn update(&mut self, _hws: &mut HardwareWallets, _message: Message) -> Task<Message> {
        Task::none()
    }
    fn subscription(&self, _hws: &HardwareWallets) -> Subscription<Message> {
        Subscription::none()
    }
    fn view<'a>(
        &'a self,
        _hws: &'a HardwareWallets,
        progress: (usize, usize),
        email: Option<&'a str>,
    ) -> Element<'a, Message>;

    fn load_context(&mut self, _ctx: &Context) {}
    fn load(&self) -> Task<Message> {
        Task::none()
    }
    fn skip(&self, _ctx: &Context) -> bool {
        false
    }
    fn apply(&mut self, _ctx: &mut Context) -> bool {
        true
    }
    fn revert(&self, _ctx: &mut Context) {}
    fn stop(&mut self) {}
}

pub struct Final {
    generating: bool,
    internal_bitcoind: Option<Bitcoind>,
    warning: Option<String>,
    wallet_settings: Option<WalletSettings>,
    key_redemptions: HashMap<ProviderKey, Option<Result<(), services::keys::Error>>>,
    // --- Backend `ConnectVault` hand-off (populated in `load_context`) ---
    coincube_client: Option<CoincubeClient>,
    cube_uuid: Option<String>,
    cube_name: Option<String>,
    network: String,
    connect_vault_members: Vec<ConnectVaultMemberPayload>,
    connect_vault_timelock_days: Option<i32>,
    /// `Some` once the vault-create round-trip resolves. Drives the
    /// success caption in the Final view.
    connect_vault_outcome: Option<ConnectVaultOutcome>,
}

impl Final {
    pub fn new() -> Self {
        Self {
            internal_bitcoind: None,
            generating: false,
            warning: None,
            wallet_settings: None,
            key_redemptions: HashMap::new(),
            coincube_client: None,
            cube_uuid: None,
            cube_name: None,
            network: String::new(),
            connect_vault_members: Vec::new(),
            connect_vault_timelock_days: None,
            connect_vault_outcome: None,
        }
    }
}

impl Default for Final {
    fn default() -> Self {
        Self::new()
    }
}

impl Step for Final {
    fn load_context(&mut self, ctx: &Context) {
        self.internal_bitcoind.clone_from(&ctx.internal_bitcoind);
        self.key_redemptions = ctx
            .keys
            .values()
            .filter_map(|ks| ks.provider_key.as_ref().map(|pk| (pk.clone(), None)))
            .collect();
        self.coincube_client.clone_from(&ctx.coincube_client);
        self.cube_uuid.clone_from(&ctx.cube_id);
        self.cube_name.clone_from(&ctx.cube_name);
        self.network = match ctx.bitcoin_config.network {
            coincube_core::miniscript::bitcoin::Network::Bitcoin => "mainnet".to_string(),
            other => other.to_string(),
        };
        self.connect_vault_members
            .clone_from(&ctx.connect_vault_members);
        self.connect_vault_timelock_days = ctx.connect_vault_timelock_days;
    }
    fn load(&self) -> Task<Message> {
        if self.generating {
            Task::none()
        } else if self.wallet_settings.is_some() {
            // If installation is already done, just retry cube save
            Task::perform(async {}, |_| Message::RetryCubeSave)
        } else {
            Task::perform(async {}, |_| Message::Install)
        }
    }
    fn update(&mut self, _hws: &mut HardwareWallets, message: Message) -> Task<Message> {
        match message {
            Message::RedeemNextKey => {
                if let Some((pk, _)) = self.key_redemptions.iter().find(|(_, v)| v.is_none()) {
                    let client = services::keys::Client::new();
                    let pk = pk.clone();
                    return Task::perform(
                        async move { (pk.clone(), client.redeem_key(pk.uuid, pk.token).await) },
                        |(pk, res)| Message::KeyRedeemed(pk, res.map(|_| ())),
                    );
                }
                return Task::perform(async move {}, |_| Message::AllKeysRedeemed);
            }
            Message::KeyRedeemed(pk, res) => {
                if let Some(v) = self.key_redemptions.get_mut(&pk) {
                    *v = Some(res);
                }
                return Task::perform(async move {}, |_| Message::RedeemNextKey);
            }
            Message::AllKeysRedeemed => {
                self.generating = false;
                // If any errors occurred redeeming tokens, add a warning to the log.
                for (pk, res) in &self.key_redemptions {
                    if let Some(res) = res {
                        if let Err(e) = res {
                            warn!("Error redeeming key for token '{}': '{}'.", pk.token, e);
                        }
                    } else {
                        // We expect to have all redemption results by now.
                        warn!("Missing redemption info for token '{}'.", pk.token);
                    }
                }
                // Now exit the installer whether or not any redemption errors occurred.
                let internal_bitcoind = self.internal_bitcoind.clone();
                let settings = self.wallet_settings.clone().expect("Install is done");
                return Task::perform(
                    async move { (settings, internal_bitcoind) },
                    |(settings, internal_bitcoind)| {
                        Message::Exit(Box::new(settings), internal_bitcoind)
                    },
                );
            }
            Message::Installed(_, res) => match res {
                Err(e) => {
                    self.generating = false;
                    self.wallet_settings = None;
                    self.warning = Some(e.to_string());
                }
                Ok(wallet_settings) => {
                    self.wallet_settings = Some(wallet_settings);
                    // Kick off the backend vault-create orchestration
                    // before token redemption. If it isn't applicable
                    // (no Connect client or no keychain members) the
                    // inner function short-circuits and the Final step
                    // treats the `NotApplicable` error as a no-op.
                    let client = self.coincube_client.clone();
                    let cube_uuid = self.cube_uuid.clone();
                    let cube_name = self.cube_name.clone();
                    let network = self.network.clone();
                    let members = self.connect_vault_members.clone();
                    let timelock = self.connect_vault_timelock_days;
                    return Task::perform(
                        connect_vault::create_connect_vault(
                            client, cube_uuid, cube_name, network, members, timelock,
                        ),
                        Message::ConnectVaultCreated,
                    );
                }
            },
            Message::ConnectVaultCreated(result) => match result {
                Ok(outcome) => {
                    self.connect_vault_outcome = Some(outcome);
                    // Clear any previous warning so the success caption
                    // wins over a transient banner from an earlier retry.
                    self.warning = None;
                    return Task::perform(async move {}, |_| Message::RedeemNextKey);
                }
                Err(ConnectVaultError::NotApplicable) => {
                    // No Connect client, no cube, or no keychain
                    // members — silently continue with the local-only
                    // install.
                    return Task::perform(async move {}, |_| Message::RedeemNextKey);
                }
                Err(ConnectVaultError::KeyAlreadyUsedInVault { key_id }) => {
                    // W9 race condition: the vault shell was rolled
                    // back upstream. Surface the dedicated copy and
                    // continue with redemption so the local install
                    // doesn't stall. The user can re-run the installer
                    // to retry the vault.
                    self.warning = Some(format!(
                        "This key (#{}) was already used in another Vault. \
                         A key can only participate in one Vault. Your local \
                         wallet is installed; restart the Vault Builder and \
                         pick a different key to create the Connect vault.",
                        key_id
                    ));
                    return Task::perform(async move {}, |_| Message::RedeemNextKey);
                }
                Err(ConnectVaultError::Other(msg)) => {
                    // Transient / unexpected failure. Warn but continue
                    // — the local wallet is already persisted, and the
                    // user can retry via the in-app Connect flow later.
                    self.warning = Some(format!(
                        "Connect vault sync failed: {}. Your local wallet is \
                         installed; the Connect vault can be retried later.",
                        msg
                    ));
                    return Task::perform(async move {}, |_| Message::RedeemNextKey);
                }
            },
            Message::RetryCubeSave => {
                self.warning = None;

                let Some(settings) = self.wallet_settings.clone() else {
                    self.warning =
                        Some("Cannot retry cube save: installation not complete".to_string());
                    return Task::none();
                };
                let internal_bitcoind = self.internal_bitcoind.clone();

                self.generating = true;

                return Task::perform(
                    async move { (settings, internal_bitcoind) },
                    |(settings, internal_bitcoind)| {
                        Message::Exit(Box::new(settings), internal_bitcoind)
                    },
                );
            }
            Message::CubeSaveFailed(err) => {
                // Cube save failed after installation
                self.generating = false;
                self.warning = Some(err.to_string());
            }
            Message::Install => {
                self.generating = true;
                self.wallet_settings = None;
                self.warning = None;
            }
            _ => {}
        }
        Task::none()
    }

    fn view<'a>(
        &'a self,
        _hws: &'a HardwareWallets,
        progress: (usize, usize),
        email: Option<&'a str>,
    ) -> Element<'a, Message> {
        // Surface the approximate-timelock caveat here — the days
        // value is derived client-side from block-count, which varies
        // with block cadence, so the user should know it's a best-effort
        // figure rather than an exact day count.
        let caption = self.connect_vault_outcome.as_ref().map(|out| {
            format!(
                "Connect vault attached — approx. {}-day timelock ({} signer{}). \
                 The timelock is approximate; on-chain block time drives the actual expiry.",
                out.timelock_days,
                out.members_added,
                if out.members_added == 1 { "" } else { "s" }
            )
        });
        view::install(
            progress,
            email,
            self.generating,
            self.wallet_settings.is_some() && self.warning.is_none(),
            self.warning.as_ref(),
            caption,
        )
    }
}

impl From<Final> for Box<dyn Step> {
    fn from(s: Final) -> Box<dyn Step> {
        Box::new(s)
    }
}
