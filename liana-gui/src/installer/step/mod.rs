pub mod descriptor;

mod backend;
mod mnemonic;
mod node;
mod share_xpubs;

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

use std::{collections::HashMap, path::PathBuf};

use iced::{Subscription, Task};

use liana_ui::widget::*;

use crate::{
    app::settings::ProviderKey,
    hw::HardwareWallets,
    installer::{context::Context, message::Message, view},
    node::bitcoind::Bitcoind,
    services,
};

pub trait Step {
    fn update(
        &mut self,
        _hws: &mut HardwareWallets,
        _message: Message,
        _ctx: &Context,
    ) -> Task<Message> {
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
    fn stop(&self) {}
}

pub struct Final {
    generating: bool,
    internal_bitcoind: Option<Bitcoind>,
    warning: Option<String>,
    config_path: Option<PathBuf>,
    key_redemptions: HashMap<ProviderKey, Option<Result<(), services::Error>>>,
}

impl Final {
    pub fn new() -> Self {
        Self {
            internal_bitcoind: None,
            generating: false,
            warning: None,
            config_path: None,
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
        if !self.generating && self.config_path.is_none() {
            Task::perform(async {}, |_| Message::Install)
        } else {
            Task::none()
        }
    }
    fn update(
        &mut self,
        _hws: &mut HardwareWallets,
        message: Message,
        _ctx: &Context,
    ) -> Task<Message> {
        match message {
            Message::RedeemNextKey => {
                if let Some((pk, _)) = self.key_redemptions.iter().find(|(_, v)| v.is_none()) {
                    let client = services::Client::new();
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
                let path = self.config_path.clone().expect("config path already set");
                // If there were any errors, don't remove the installer log.
                return Task::perform(
                    async move { (path, internal_bitcoind, has_error) },
                    |(path, internal_bitcoind, has_error)| {
                        Message::Exit(path, internal_bitcoind, !has_error)
                    },
                );
            }
            Message::Installed(res) => match res {
                Err(e) => {
                    self.generating = false;
                    self.config_path = None;
                    self.warning = Some(e.to_string());
                }
                Ok(path) => {
                    self.config_path = Some(path.clone());
                    // Now redeem any provider keys.
                    return Task::perform(async move {}, |_| Message::RedeemNextKey);
                }
            },
            Message::Install => {
                self.generating = true;
                self.config_path = None;
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
            self.config_path.as_ref(),
            self.warning.as_ref(),
        )
    }
}

impl From<Final> for Box<dyn Step> {
    fn from(s: Final) -> Box<dyn Step> {
        Box::new(s)
    }
}
