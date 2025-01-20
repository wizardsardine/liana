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

use std::path::PathBuf;

use iced::{Subscription, Task};

use liana_ui::widget::*;

use crate::{
    hw::HardwareWallets,
    installer::{context::Context, message::Message, view},
    node::bitcoind::Bitcoind,
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
    fn stop(&self) {}
}

pub struct Final {
    generating: bool,
    internal_bitcoind: Option<Bitcoind>,
    warning: Option<String>,
    config_path: Option<PathBuf>,
}

impl Final {
    pub fn new() -> Self {
        Self {
            internal_bitcoind: None,
            generating: false,
            warning: None,
            config_path: None,
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
    }
    fn load(&self) -> Task<Message> {
        if !self.generating && self.config_path.is_none() {
            Task::perform(async {}, |_| Message::Install)
        } else {
            Task::none()
        }
    }
    fn update(&mut self, _hws: &mut HardwareWallets, message: Message) -> Task<Message> {
        match message {
            Message::Installed(res) => {
                self.generating = false;
                match res {
                    Err(e) => {
                        self.config_path = None;
                        self.warning = Some(e.to_string());
                    }
                    Ok(path) => {
                        self.config_path = Some(path.clone());
                        let internal_bitcoind = self.internal_bitcoind.clone();
                        let path = path.clone();
                        return Task::perform(
                            async { (path, internal_bitcoind) },
                            |(path, internal_bitcoind)| Message::Exit(path, internal_bitcoind),
                        );
                    }
                }
            }
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
