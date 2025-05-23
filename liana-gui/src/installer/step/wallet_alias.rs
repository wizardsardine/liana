use iced::Task;

use liana::miniscript::bitcoin::Network;
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
        match (
            ctx.wallet_alias.is_empty(),
            self.wallet_alias.value.is_empty(),
        ) {
            // Alias from context is the first one to be set.
            (false, _) => {
                self.wallet_alias.value = ctx.wallet_alias.clone();
                self.wallet_alias.valid = true;
            }
            // No alias at all, we set a default value.
            (true, true) => {
                self.wallet_alias.value = format!(
                    "My Liana {} wallet",
                    match ctx.network {
                        Network::Bitcoin => "Bitcoin",
                        Network::Signet => "Signet",
                        Network::Testnet => "Testnet",
                        Network::Regtest => "Regtest",
                        _ => "",
                    }
                );
                self.wallet_alias.valid = true;
            }
            // We keep the current value.
            (true, false) => {}
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
