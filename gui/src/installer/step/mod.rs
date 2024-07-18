mod bitcoind;
mod descriptor;
mod mnemonic;
mod share_xpubs;

pub use bitcoind::{
    DefineBitcoind, DownloadState, InstallState, InternalBitcoindStep, SelectBitcoindTypeStep,
};

pub use descriptor::{BackupDescriptor, DefineDescriptor, ImportDescriptor, RegisterDescriptor};

pub use mnemonic::{BackupMnemonic, RecoverMnemonic};
pub use share_xpubs::ShareXpubs;

use std::path::PathBuf;

use iced::{Command, Subscription};

use liana_ui::widget::*;

use crate::{
    bitcoind::Bitcoind,
    hw::HardwareWallets,
    installer::{context::Context, message::Message, view},
};

pub trait Step {
    fn update(&mut self, _hws: &mut HardwareWallets, _message: Message) -> Command<Message> {
        Command::none()
    }
    fn subscription(&self, _hws: &HardwareWallets) -> Subscription<Message> {
        Subscription::none()
    }
    fn view<'a>(
        &'a self,
        _hws: &'a HardwareWallets,
        progress: (usize, usize),
    ) -> Element<'a, Message>;

    fn load_context(&mut self, _ctx: &Context) {}
    fn load(&self) -> Command<Message> {
        Command::none()
    }
    fn skip(&self, _ctx: &Context) -> bool {
        false
    }
    fn apply(&mut self, _ctx: &mut Context) -> bool {
        true
    }
    fn stop(&self) {}
}

#[derive(Default)]
pub struct Welcome {}

impl Step for Welcome {
    fn view(&self, _hws: &HardwareWallets, _progress: (usize, usize)) -> Element<Message> {
        view::welcome()
    }
}

impl From<Welcome> for Box<dyn Step> {
    fn from(s: Welcome) -> Box<dyn Step> {
        Box::new(s)
    }
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
    fn load(&self) -> Command<Message> {
        if !self.generating && self.config_path.is_none() {
            Command::perform(async {}, |_| Message::Install)
        } else {
            Command::none()
        }
    }
    fn update(&mut self, _hws: &mut HardwareWallets, message: Message) -> Command<Message> {
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
                        return Command::perform(async {}, move |_| {
                            Message::Exit(path.clone(), internal_bitcoind)
                        });
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
        Command::none()
    }

    fn view(&self, _hws: &HardwareWallets, progress: (usize, usize)) -> Element<Message> {
        view::install(
            progress,
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
