mod descriptor;
pub use descriptor::{DefineDescriptor, ImportDescriptor, RegisterDescriptor};

use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use async_hwi::DeviceKind;
use iced::{Command, Element};
use liana::{
    config::{BitcoinConfig, BitcoindConfig},
    descriptors::MultipathDescriptor,
    miniscript::bitcoin,
};

use crate::ui::component::form;

use crate::installer::{
    message::{self, Message},
    view,
};

pub trait Step {
    fn update(&mut self, _message: Message) -> Command<Message> {
        Command::none()
    }
    fn view(&self) -> Element<Message>;
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
}

#[derive(Clone)]
pub struct Context {
    pub bitcoin_config: BitcoinConfig,
    pub bitcoind_config: Option<BitcoindConfig>,
    pub descriptor: Option<MultipathDescriptor>,
    pub hw_tokens: Vec<(DeviceKind, bitcoin::util::bip32::Fingerprint, [u8; 32])>,
    pub data_dir: PathBuf,
}

impl Context {
    pub fn new(network: bitcoin::Network, data_dir: PathBuf) -> Self {
        Self {
            bitcoin_config: BitcoinConfig {
                network,
                poll_interval_secs: Duration::from_secs(30),
            },
            hw_tokens: Vec::new(),
            bitcoind_config: None,
            descriptor: None,
            data_dir,
        }
    }
}

#[derive(Default)]
pub struct Welcome {}

impl Step for Welcome {
    fn view(&self) -> Element<Message> {
        view::welcome()
    }
}

impl From<Welcome> for Box<dyn Step> {
    fn from(s: Welcome) -> Box<dyn Step> {
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
            self.cookie_path.value =
                bitcoind_default_cookie_path(&ctx.bitcoin_config.network).unwrap_or_default()
        }
        if self.address.value.is_empty() {
            self.address.value = bitcoind_default_address(&ctx.bitcoin_config.network);
        }
    }
    fn update(&mut self, message: Message) -> Command<Message> {
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
        Command::none()
    }

    fn apply(&mut self, ctx: &mut Context) -> bool {
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
                ctx.bitcoind_config = Some(BitcoindConfig {
                    cookie_path: path,
                    addr,
                });
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
    fn update(&mut self, message: Message) -> Command<Message> {
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
        Command::none()
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
