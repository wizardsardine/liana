pub mod descriptor;

mod backend;
mod mnemonic;
mod node;
mod share_xpubs;
mod wallet_alias;

pub use node::{
    bitcoind::{DownloadState, InstallState, InternalBitcoindStep, SelectBitcoindTypeStep},
    DefineNode,
};

pub use descriptor::{
    editor::template::{ChooseDescriptorTemplate, DescriptorTemplateDescription},
    editor::DefineDescriptor,
    BackupDescriptor, ImportDescriptor, RegisterDescriptor,
};

pub use backend::{ChooseBackend, ImportRemoteWallet, RemoteBackendLogin};
pub use mnemonic::{BackupMnemonic, RecoverMnemonic};
pub use share_xpubs::ShareXpubs;
use tracing::warn;
pub use wallet_alias::WalletAlias;

use std::collections::HashMap;

use iced::{Subscription, Task};

use liana_ui::widget::*;

use crate::{
    app::settings::{ProviderKey, WalletSettings},
    hw::HardwareWallets,
    installer::{context::Context, message::Message, view},
    node::bitcoind::Bitcoind,
    services,
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
}

impl Final {
    pub fn new() -> Self {
        Self {
            internal_bitcoind: None,
            generating: false,
            warning: None,
            wallet_settings: None,
            key_redemptions: HashMap::new(),
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
    }
    fn load(&self) -> Task<Message> {
        if !self.generating && self.wallet_settings.is_none() {
            Task::perform(async {}, |_| Message::Install)
        } else {
            Task::none()
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
                let mut has_error = false;
                for (pk, res) in &self.key_redemptions {
                    if let Some(res) = res {
                        if let Err(e) = res {
                            warn!("Error redeeming key for token '{}': '{}'.", pk.token, e);
                            has_error = true;
                        }
                    } else {
                        // We expect to have all redemption results by now.
                        warn!("Missing redemption info for token '{}'.", pk.token);
                        has_error = true;
                    }
                }
                // Now exit the installer whether or not any redemption errors occurred.
                let internal_bitcoind = self.internal_bitcoind.clone();
                let settings = self.wallet_settings.clone().expect("Install is done");
                // If there were any errors, don't remove the installer log.
                return Task::perform(
                    async move { (settings, internal_bitcoind, has_error) },
                    |(settings, internal_bitcoind, has_error)| {
                        Message::Exit(Box::new(settings), internal_bitcoind, !has_error)
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
                    // Now redeem any provider keys.
                    return Task::perform(async move {}, |_| Message::RedeemNextKey);
                }
            },
            Message::Install => {
                self.generating = true;
                self.wallet_settings = None;
                self.warning = None;
            }
            _ => {}
        };
        Task::none()
    }

    fn view<'a>(
        &'a self,
        _hws: &'a HardwareWallets,
        progress: (usize, usize),
        email: Option<&'a str>,
    ) -> Element<Message> {
        view::install(
            progress,
            email,
            self.generating,
            self.wallet_settings.is_some(),
            self.warning.as_ref(),
        )
    }
}

impl From<Final> for Box<dyn Step> {
    fn from(s: Final) -> Box<dyn Step> {
        Box::new(s)
    }
}
