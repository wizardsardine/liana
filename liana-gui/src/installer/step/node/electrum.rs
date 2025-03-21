use iced::Task;
use liana_ui::{component::form, widget::*};
use lianad::{
    config::ElectrumConfig,
    electrum_client::{self, ElectrumApi},
};

use crate::{
    installer::{
        context::Context,
        message::{self, Message},
        view, Error,
    },
    node::electrum::ConfigField,
};

#[derive(Clone)]
pub struct DefineElectrum {
    address: form::Value<String>,
    validate_domain: bool,
}

impl Default for DefineElectrum {
    fn default() -> Self {
        Self {
            address: Default::default(),
            validate_domain: true,
        }
    }
}

impl DefineElectrum {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn can_try_ping(&self) -> bool {
        !self.address.value.is_empty() && self.address.valid
    }

    pub fn update(&mut self, message: message::DefineNode) -> Task<Message> {
        if let message::DefineNode::DefineElectrum(msg) = message {
            match msg {
                message::DefineElectrum::ConfigFieldEdited(field, value) => match field {
                    ConfigField::Address => {
                        self.address.value.clone_from(&value); // save the value including any prefix
                        self.address.valid =
                            crate::node::electrum::is_electrum_address_valid(&value);
                    }
                },
                message::DefineElectrum::ValidDomainChanged(v) => self.validate_domain = v,
            };
        };
        Task::none()
    }

    pub fn apply(&mut self, ctx: &mut Context) -> bool {
        if self.can_try_ping() {
            ctx.bitcoin_backend = Some(lianad::config::BitcoinBackend::Electrum(ElectrumConfig {
                addr: self.address.value.clone(),
                validate_domain: self.validate_domain,
            }));
            return true;
        }
        false
    }

    pub fn view(&self) -> Element<Message> {
        view::define_electrum(&self.address, self.validate_domain)
    }

    pub fn ping(&self) -> Result<(), Error> {
        let builder = electrum_client::Config::builder();
        let config = builder
            .timeout(Some(3))
            .validate_domain(self.validate_domain)
            .build();
        let client = electrum_client::Client::from_config(&self.address.value, config)
            .map_err(|e| Error::Electrum(e.to_string()))?;
        client
            .raw_call("server.ping", [])
            .map_err(|e| Error::Electrum(e.to_string()))?;
        Ok(())
    }
}
