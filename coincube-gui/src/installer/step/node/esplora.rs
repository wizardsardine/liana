use coincube_ui::{component::form, widget::*};
use coincubed::{config::EsploraConfig, esplora_client};
use iced::Task;

use crate::{
    installer::{
        context::Context,
        message::{self, Message},
        view, Error,
    },
    node::esplora::ConfigField,
};

#[derive(Clone, Default)]
pub struct DefineEsplora {
    address: form::Value<String>,
}

impl DefineEsplora {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn can_try_ping(&self) -> bool {
        !self.address.value.is_empty() && self.address.valid
    }

    pub fn update(&mut self, message: message::DefineNode) -> Task<Message> {
        if let message::DefineNode::DefineEsplora(message::DefineEsplora::ConfigFieldEdited(
            field,
            value,
        )) = message
        {
            match field {
                ConfigField::Address => {
                    self.address.value.clone_from(&value);
                    self.address.valid = crate::node::esplora::is_esplora_address_valid(&value);
                }
            }
        }
        Task::none()
    }

    pub fn apply(&mut self, ctx: &mut Context) -> bool {
        if self.can_try_ping() {
            ctx.bitcoin_backend = Some(coincubed::config::BitcoinBackend::Esplora(EsploraConfig {
                addr: self.address.value.clone(),
                token: None,
            }));
            return true;
        }
        false
    }

    pub fn view(&self) -> Element<Message> {
        view::define_esplora(&self.address)
    }

    pub fn ping(&self) -> Result<(), Error> {
        let client = esplora_client::Builder::new(&self.address.value)
            .timeout(3)
            .build_blocking();
        client
            .get_height()
            .map(|_| ())
            .map_err(|e| Error::Esplora(e.to_string()))
    }
}
