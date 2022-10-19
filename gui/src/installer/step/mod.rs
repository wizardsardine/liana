use std::path::PathBuf;
use std::str::FromStr;

use iced::pure::Element;
use minisafe::{
    descriptors::InheritanceDescriptor,
    miniscript::descriptor::{Descriptor, DescriptorPublicKey},
};

use crate::ui::component::form;

use crate::installer::{
    config,
    message::{self, Message},
    view,
};

pub trait Step {
    fn update(&mut self, message: Message);
    fn view(&self) -> Element<Message>;
    fn load_context(&mut self, _ctx: &Context) {}
    fn skip(&self, _ctx: &Context) -> bool {
        false
    }
    fn apply(&mut self, _ctx: &mut Context, _config: &mut config::Config) -> bool {
        true
    }
}

#[derive(Clone)]
pub struct Context {
    pub network: bitcoin::Network,
}

impl Context {
    pub fn new(network: bitcoin::Network) -> Self {
        Self { network }
    }
}

impl Default for Context {
    fn default() -> Self {
        Self::new(bitcoin::Network::Bitcoin)
    }
}

pub struct Welcome {
    network: bitcoin::Network,
}

impl Welcome {
    pub fn new(network: bitcoin::Network) -> Self {
        Self { network }
    }
}

impl Step for Welcome {
    fn update(&mut self, message: Message) {
        if let message::Message::Network(network) = message {
            self.network = network;
        }
    }
    fn apply(&mut self, ctx: &mut Context, config: &mut config::Config) -> bool {
        ctx.network = self.network;
        config.bitcoin_config.network = self.network;
        true
    }
    fn view(&self) -> Element<Message> {
        view::welcome(&self.network)
    }
}

impl Default for Welcome {
    fn default() -> Self {
        Self::new(bitcoin::Network::Bitcoin)
    }
}

impl From<Welcome> for Box<dyn Step> {
    fn from(s: Welcome) -> Box<dyn Step> {
        Box::new(s)
    }
}

pub struct DefineDescriptor {
    imported_descriptor: form::Value<String>,
    user_xpub: form::Value<String>,
    heir_xpub: form::Value<String>,
    sequence: form::Value<String>,
    error: Option<String>,
}

impl DefineDescriptor {
    pub fn new() -> Self {
        Self {
            imported_descriptor: form::Value::default(),
            user_xpub: form::Value::default(),
            heir_xpub: form::Value::default(),
            sequence: form::Value::default(),
            error: None,
        }
    }
}

impl Step for DefineDescriptor {
    // form value is set as valid each time it is edited.
    // Verification of the values is happening when the user click on Next button.
    fn update(&mut self, message: Message) {
        if let Message::DefineDescriptor(msg) = message {
            match msg {
                message::DefineDescriptor::ImportDescriptor(desc) => {
                    self.imported_descriptor.value = desc;
                    self.imported_descriptor.valid = true;
                }
                message::DefineDescriptor::UserXpubEdited(xpub) => {
                    self.user_xpub.value = xpub;
                    self.user_xpub.valid = true;
                }
                message::DefineDescriptor::HeirXpubEdited(xpub) => {
                    self.heir_xpub.value = xpub;
                    self.heir_xpub.valid = true;
                }
                message::DefineDescriptor::SequenceEdited(seq) => {
                    self.sequence.valid = true;
                    if seq.is_empty() || seq.parse::<u32>().is_ok() {
                        self.sequence.value = seq;
                    }
                }
            };
        };
    }

    fn apply(&mut self, _ctx: &mut Context, config: &mut config::Config) -> bool {
        // descriptor forms for import or creation cannot be both empty or filled.
        if self.imported_descriptor.value.is_empty()
            == (self.user_xpub.value.is_empty()
                || self.heir_xpub.value.is_empty()
                || self.sequence.value.is_empty())
        {
            if !self.user_xpub.value.is_empty() {
                self.user_xpub.valid = DescriptorPublicKey::from_str(&self.user_xpub.value).is_ok();
            }
            if !self.heir_xpub.value.is_empty() {
                self.heir_xpub.valid = DescriptorPublicKey::from_str(&self.heir_xpub.value).is_ok();
            }
            if !self.sequence.value.is_empty() {
                self.sequence.valid = self.sequence.value.parse::<u32>().is_ok();
            }
            if !self.imported_descriptor.value.is_empty() {
                self.imported_descriptor.valid =
                    Descriptor::<DescriptorPublicKey>::from_str(&self.imported_descriptor.value)
                        .is_ok();
            }
            false
        } else if !self.imported_descriptor.value.is_empty() {
            if let Ok(desc) = InheritanceDescriptor::from_str(&self.imported_descriptor.value) {
                config.main_descriptor = Some(desc);
                true
            } else {
                self.imported_descriptor.valid = false;
                false
            }
        } else {
            let user_key = DescriptorPublicKey::from_str(&self.user_xpub.value);
            self.user_xpub.valid = user_key.is_ok();

            let heir_key = DescriptorPublicKey::from_str(&self.heir_xpub.value);
            self.user_xpub.valid = user_key.is_ok();

            let sequence = self.sequence.value.parse::<u32>();
            self.sequence.valid = sequence.is_ok();

            if !self.user_xpub.valid || !self.heir_xpub.valid || !self.sequence.valid {
                return false;
            }

            match InheritanceDescriptor::new(
                user_key.unwrap(),
                heir_key.unwrap(),
                sequence.unwrap(),
            ) {
                Ok(desc) => {
                    config.main_descriptor = Some(desc);
                    true
                }
                Err(e) => {
                    self.error = Some(e.to_string());
                    false
                }
            }
        }
    }

    fn view(&self) -> Element<Message> {
        view::define_descriptor(
            &self.imported_descriptor,
            &self.user_xpub,
            &self.heir_xpub,
            &self.sequence,
            self.error.as_ref(),
        )
    }
}

impl Default for DefineDescriptor {
    fn default() -> Self {
        Self::new()
    }
}

impl From<DefineDescriptor> for Box<dyn Step> {
    fn from(s: DefineDescriptor) -> Box<dyn Step> {
        Box::new(s)
    }
}

pub struct DefineBitcoind {
    cookie_path: form::Value<String>,
    address: form::Value<String>,
}

fn bitcoind_default_cookie_path(network: &bitcoin::Network) -> Option<String> {
    #[cfg(target_os = "linux")]
    let configs_dir = dirs::home_dir();

    #[cfg(not(target_os = "linux"))]
    let configs_dir = dirs::config_dir();

    if let Some(mut path) = configs_dir {
        #[cfg(target_os = "linux")]
        path.push(".bitcoin");

        #[cfg(not(target_os = "linux"))]
        path.push("Bitcoin");

        match network {
            bitcoin::Network::Bitcoin => {
                path.push(".cookie");
            }
            bitcoin::Network::Testnet => {
                path.push("testnet3/.cookie");
            }
            bitcoin::Network::Regtest => {
                path.push("regtest/.cookie");
            }
            bitcoin::Network::Signet => {
                path.push("signet/.cookie");
            }
        }

        return path.to_str().map(|s| s.to_string());
    }
    None
}

fn bitcoind_default_address(network: &bitcoin::Network) -> String {
    match network {
        bitcoin::Network::Bitcoin => "127.0.0.1:8332".to_string(),
        bitcoin::Network::Testnet => "127.0.0.1:18332".to_string(),
        bitcoin::Network::Regtest => "127.0.0.1:18443".to_string(),
        bitcoin::Network::Signet => "127.0.0.1:38332".to_string(),
    }
}

impl DefineBitcoind {
    pub fn new() -> Self {
        Self {
            cookie_path: form::Value::default(),
            address: form::Value::default(),
        }
    }
}

impl Step for DefineBitcoind {
    fn load_context(&mut self, ctx: &Context) {
        if self.cookie_path.value.is_empty() {
            self.cookie_path.value = bitcoind_default_cookie_path(&ctx.network).unwrap_or_default()
        }
        if self.address.value.is_empty() {
            self.address.value = bitcoind_default_address(&ctx.network);
        }
    }
    fn update(&mut self, message: Message) {
        if let Message::DefineBitcoind(msg) = message {
            match msg {
                message::DefineBitcoind::AddressEdited(address) => {
                    self.address.value = address;
                    self.address.valid = true;
                }
                message::DefineBitcoind::CookiePathEdited(path) => {
                    self.cookie_path.value = path;
                    self.address.valid = true;
                }
            };
        };
    }

    fn apply(&mut self, _ctx: &mut Context, config: &mut config::Config) -> bool {
        match (
            PathBuf::from_str(&self.cookie_path.value),
            std::net::SocketAddr::from_str(&self.address.value),
        ) {
            (Err(_), Ok(_)) => {
                self.cookie_path.valid = false;
                false
            }
            (Ok(_), Err(_)) => {
                self.address.valid = false;
                false
            }
            (Err(_), Err(_)) => {
                self.cookie_path.valid = false;
                self.address.valid = false;
                false
            }
            (Ok(path), Ok(addr)) => {
                config.bitcoind_config.cookie_path = path;
                config.bitcoind_config.addr = addr;
                true
            }
        }
    }

    fn view(&self) -> Element<Message> {
        view::define_bitcoin(&self.address, &self.cookie_path)
    }
}

impl Default for DefineBitcoind {
    fn default() -> Self {
        Self::new()
    }
}

impl From<DefineBitcoind> for Box<dyn Step> {
    fn from(s: DefineBitcoind) -> Box<dyn Step> {
        Box::new(s)
    }
}

pub struct Final {
    generating: bool,
    warning: Option<String>,
    config_path: Option<PathBuf>,
}

impl Final {
    pub fn new() -> Self {
        Self {
            generating: false,
            warning: None,
            config_path: None,
        }
    }
}

impl Step for Final {
    fn update(&mut self, message: Message) {
        match message {
            Message::Installed(res) => {
                self.generating = false;
                match res {
                    Err(e) => {
                        self.config_path = None;
                        self.warning = Some(e.to_string());
                    }
                    Ok(path) => self.config_path = Some(path),
                }
            }
            Message::Install => {
                self.generating = true;
                self.config_path = None;
                self.warning = None;
            }
            _ => {}
        };
    }

    fn view(&self) -> Element<Message> {
        view::install(
            self.generating,
            self.config_path.as_ref(),
            self.warning.as_ref(),
        )
    }
}

impl Default for Final {
    fn default() -> Self {
        Self::new()
    }
}

impl From<Final> for Box<dyn Step> {
    fn from(s: Final) -> Box<dyn Step> {
        Box::new(s)
    }
}
