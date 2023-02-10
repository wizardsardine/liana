use crate::installer::{context::Context, message::Message, step::Step, view};

use iced::{Command, Element};

#[derive(Default)]
pub struct BackupMnemonic {
    words: [&'static str; 12],
    done: bool,
}

impl From<BackupMnemonic> for Box<dyn Step> {
    fn from(s: BackupMnemonic) -> Box<dyn Step> {
        Box::new(s)
    }
}

impl Step for BackupMnemonic {
    fn load_context(&mut self, ctx: &Context) {
        if let Some(signer) = &ctx.signer {
            self.words = signer.mnemonic();
        }
    }
    fn update(&mut self, message: Message) -> Command<Message> {
        if let Message::UserActionDone(done) = message {
            self.done = done;
        }
        Command::none()
    }
    fn skip(&self, ctx: &Context) -> bool {
        ctx.signer.is_none()
    }
    fn view(&self, progress: (usize, usize)) -> Element<Message> {
        view::backup_mnemonic(progress, &self.words, self.done)
    }
}
