mod bitcoind;
mod descriptor;
mod mnemonic;

pub use bitcoind::{
    DefineBitcoind, DownloadState, InstallState, InternalBitcoindConfig, InternalBitcoindStep,
    SelectBitcoindTypeStep,
};

pub use descriptor::{
    BackupDescriptor, DefineDescriptor, ImportDescriptor, ParticipateXpub, RegisterDescriptor,
};

pub use mnemonic::{BackupMnemonic, RecoverMnemonic};

use std::path::PathBuf;

use iced::{Command, Subscription};
use liana::miniscript::bitcoin::bip32::Fingerprint;

use liana_ui::widget::*;

use crate::{
    bitcoind::Bitcoind,
    installer::{context::Context, message::Message, view},
};

pub trait Step {
    fn update(&mut self, _message: Message) -> Command<Message> {
        Command::none()
    }
    fn subscription(&self) -> Subscription<Message> {
        Subscription::none()
    }
    fn view(&self, progress: (usize, usize)) -> Element<Message>;
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
    fn view(&self, _progress: (usize, usize)) -> Element<Message> {
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
    hot_signer_fingerprint: Fingerprint,
    hot_signer_is_not_used: bool,
}

impl Final {
    pub fn new(hot_signer_fingerprint: Fingerprint) -> Self {
        Self {
            internal_bitcoind: None,
            generating: false,
            warning: None,
            config_path: None,
            hot_signer_fingerprint,
            hot_signer_is_not_used: false,
        }
    }
}

impl Step for Final {
    fn load_context(&mut self, ctx: &Context) {
        self.internal_bitcoind = ctx.internal_bitcoind.clone();
        if let Some(signer) = &ctx.recovered_signer {
            self.hot_signer_fingerprint = signer.fingerprint();
            self.hot_signer_is_not_used = false;
        } else if ctx
            .descriptor
            .as_ref()
            .unwrap()
            .to_string()
            .contains(&self.hot_signer_fingerprint.to_string())
        {
            self.hot_signer_is_not_used = false;
        } else {
            self.hot_signer_is_not_used = true;
        }
    }
    fn load(&self) -> Command<Message> {
        if !self.generating && self.config_path.is_none() {
            Command::perform(async {}, |_| Message::Install)
        } else {
            Command::none()
        }
    }
    fn update(&mut self, message: Message) -> Command<Message> {
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

    fn view(&self, progress: (usize, usize)) -> Element<Message> {
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
