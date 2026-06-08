use coincube_core::miniscript::bitcoin::{self, constants::ChainHash, hashes::Hash, BlockHash};
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
    placeholder: String,
    network: Option<bitcoin::Network>,
}

impl DefineEsplora {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load_context(&mut self, ctx: &Context) {
        self.placeholder = super::super::super::connect_url(ctx.network);
        self.network = Some(ctx.network);
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
        if let Some(addr) = crate::node::esplora::normalize_esplora_address(&self.address.value) {
            ctx.bitcoin_backend = Some(coincubed::config::BitcoinBackend::Esplora(EsploraConfig {
                addr,
                token: None,
                fallback_addr: None,
                fallback_token: None,
                secondary_fallback_addr: None,
                secondary_fallback_token: None,
            }));
            return true;
        }
        false
    }

    pub fn view(&self) -> Element<Message> {
        view::define_esplora(&self.address, &self.placeholder)
    }

    pub fn ping(&self) -> Result<(), Error> {
        let addr = crate::node::esplora::normalize_esplora_address(&self.address.value)
            .ok_or_else(|| Error::Esplora("Invalid Esplora URL".to_string()))?;
        let network = self
            .network
            .ok_or_else(|| Error::Esplora("Bitcoin network is not selected".to_string()))?;
        let client = esplora_client::Builder::new(&addr)
            .timeout(3)
            .build_blocking();
        let height = client
            .get_height()
            .map_err(|e| Error::Esplora(e.to_string()))?;
        let server_genesis = client
            .get_block_hash(0)
            .map_err(|e| Error::Esplora(e.to_string()))?;
        let expected_genesis =
            BlockHash::from_byte_array(*ChainHash::using_genesis_block(network).as_bytes());
        if server_genesis != expected_genesis {
            return Err(Error::Esplora(format!(
                "Esplora URL is not for {} (height {}, genesis {})",
                network, height, server_genesis
            )));
        }
        Ok(())
    }
}
