use iced::Command;
use liana::{
    config::ElectrumConfig,
    electrum_client::{self, ElectrumApi},
};
use liana_ui::{component::form, widget::*};

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
}

impl DefineElectrum {
    pub fn new() -> Self {
        Self {
            address: form::Value::default(),
        }
    }

    pub fn can_try_ping(&self) -> bool {
        !self.address.value.is_empty() && self.address.valid
    }

    pub fn update(&mut self, message: message::DefineNode) -> Command<Message> {
        if let message::DefineNode::DefineElectrum(msg) = message {
            match msg {
                message::DefineElectrum::ConfigFieldEdited(field, value) => match field {
                    ConfigField::Address => {
                        let value_noprefix = if value.starts_with("ssl://") {
                            value.replacen("ssl://", "", 1)
                        } else {
                            value.replacen("tcp://", "", 1)
                        };
                        let noprefix_parts: Vec<_> = value_noprefix.split(':').collect();
                        self.address.value.clone_from(&value); // save the value including any prefix
                        self.address.valid = noprefix_parts.len() == 2
                            && !noprefix_parts
                                .first()
                                .expect("there are two parts")
                                .is_empty()
                            && noprefix_parts
                                .last()
                                .expect("there are two parts")
                                .parse::<u16>() // check it is a port
                                .is_ok();
                    }
                },
            };
        };
        Command::none()
    }

    pub fn apply(&mut self, ctx: &mut Context) -> bool {
        if self.can_try_ping() {
            ctx.bitcoin_backend = Some(liana::config::BitcoinBackend::Electrum(ElectrumConfig {
                addr: self.address.value.clone(),
            }));
            return true;
        }
        false
    }

    pub fn view(&self, is_running: bool) -> Element<Message> {
        view::define_electrum(&self.address, is_running)
    }

    pub fn ping(&self) -> Result<(), Error> {
        let builder = electrum_client::Config::builder();
        let config = builder.timeout(Some(3)).build();
        let client = electrum_client::Client::from_config(&self.address.value, config)
            .map_err(|e| Error::Electrum(e.to_string()))?;
        client
            .raw_call("server.ping", [])
            .map_err(|e| Error::Electrum(e.to_string()))?;
        Ok(())
    }

    pub fn is_ssl(&self) -> bool {
        self.address.value.starts_with("ssl://")
    }

    pub fn force_ssl(mut self) -> Self {
        if self.address.value.starts_with("tcp://") {
            self.address.value = self.address.value.replacen("tcp://", "ssl://", 1);
        } else if !self.address.value.starts_with("ssl://") {
            self.address.value = format!("ssl://{}", self.address.value);
        }
        self
    }

    pub fn force_tcp(mut self) -> Self {
        if self.address.value.starts_with("ssl://") {
            self.address.value = self.address.value.replacen("ssl://", "tcp://", 1);
        } else if !self.address.value.starts_with("tcp://") {
            self.address.value = format!("tcp://{}", self.address.value);
        }
        self
    }
}
