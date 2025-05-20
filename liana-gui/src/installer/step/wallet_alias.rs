use iced::Task;

use liana_ui::{component::form, widget::*};

use crate::{
    hw::HardwareWallets,
    installer::{context::Context, message::Message, step::Step, view},
    services::connect::client::backend::api::WALLET_ALIAS_MAXIMUM_LENGTH,
};

#[derive(Default)]
pub struct WalletAlias {
    wallet_alias: form::Value<String>,
}

impl Step for WalletAlias {
    fn load_context(&mut self, ctx: &Context) {
        if !ctx.wallet_alias.is_empty() {
            self.wallet_alias.value = ctx.wallet_alias.clone();
            self.wallet_alias.valid = true;
        }
    }

    fn update(&mut self, _hws: &mut HardwareWallets, message: Message) -> Task<Message> {
        if let Message::WalletAliasEdited(alias) = message {
            self.wallet_alias.valid = alias.len() < WALLET_ALIAS_MAXIMUM_LENGTH;
            self.wallet_alias.value = alias;
        }
        Task::none()
    }

    fn view<'a>(
        &'a self,
        _hws: &'a HardwareWallets,
        progress: (usize, usize),
        email: Option<&'a str>,
    ) -> Element<Message> {
        view::wallet_alias(progress, email, &self.wallet_alias)
    }

    fn apply(&mut self, ctx: &mut Context) -> bool {
        if self.wallet_alias.valid {
            ctx.wallet_alias = self.wallet_alias.value.trim().to_string();
            true
        } else {
            false
        }
    }
}

impl From<WalletAlias> for Box<dyn Step> {
    fn from(s: WalletAlias) -> Box<dyn Step> {
        Box::new(s)
    }
}
