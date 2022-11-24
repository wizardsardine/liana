use std::path::PathBuf;
use std::str::FromStr;

use iced::{Command, Element};
use liana::{
    descriptors::MultipathDescriptor,
    miniscript::{
        bitcoin::{
            util::bip32::{DerivationPath, Fingerprint},
            Network,
        },
        descriptor::{DescriptorMultiXKey, DescriptorPublicKey, Wildcard},
    },
};

use crate::{
    hw::{list_hardware_wallets, HardwareWallet},
    installer::{
        message::{self, Message},
        step::{Context, Step},
        view, Error,
    },
    ui::component::form,
};

pub struct DefineDescriptor {
    network: Network,
    network_valid: bool,
    data_dir: Option<PathBuf>,
    user_xpub: form::Value<String>,
    heir_xpub: form::Value<String>,
    sequence: form::Value<String>,
    modal: Option<GetHardwareWalletXpubModal>,

    error: Option<String>,
}

impl DefineDescriptor {
    pub fn new() -> Self {
        Self {
            network: Network::Bitcoin,
            data_dir: None,
            network_valid: true,
            user_xpub: form::Value::default(),
            heir_xpub: form::Value::default(),
            sequence: form::Value::default(),
            modal: None,
            error: None,
        }
    }
}

impl Step for DefineDescriptor {
    // form value is set as valid each time it is edited.
    // Verification of the values is happening when the user click on Next button.
    fn update(&mut self, message: Message) -> Command<Message> {
        match message {
            Message::Close => {
                self.modal = None;
            }
            Message::Network(network) => {
                self.network = network;
                let mut network_datadir = self.data_dir.clone().unwrap();
                network_datadir.push(self.network.to_string());
                self.network_valid = !network_datadir.exists();
            }
            Message::DefineDescriptor(msg) => {
                match msg {
                    message::DefineDescriptor::UserXpubEdited(xpub) => {
                        self.user_xpub.value = xpub;
                        self.user_xpub.valid = true;
                        self.modal = None;
                    }
                    message::DefineDescriptor::HeirXpubEdited(xpub) => {
                        self.heir_xpub.value = xpub;
                        self.heir_xpub.valid = true;
                        self.modal = None;
                    }
                    message::DefineDescriptor::SequenceEdited(seq) => {
                        self.sequence.valid = true;
                        if seq.is_empty() || seq.parse::<u16>().is_ok() {
                            self.sequence.value = seq;
                        }
                    }
                    message::DefineDescriptor::ImportUserHWXpub => {
                        let modal = GetHardwareWalletXpubModal::new(false);
                        let cmd = modal.load();
                        self.modal = Some(modal);
                        return cmd;
                    }
                    message::DefineDescriptor::ImportHeirHWXpub => {
                        let modal = GetHardwareWalletXpubModal::new(true);
                        let cmd = modal.load();
                        self.modal = Some(modal);
                        return cmd;
                    }
                    _ => {
                        if let Some(modal) = &mut self.modal {
                            return modal.update(Message::DefineDescriptor(msg));
                        }
                    }
                };
            }
            _ => {
                if let Some(modal) = &mut self.modal {
                    return modal.update(message);
                }
            }
        };
        Command::none()
    }

    fn load_context(&mut self, ctx: &Context) {
        self.network = ctx.bitcoin_config.network;
        self.data_dir = Some(ctx.data_dir.clone());
        let mut network_datadir = ctx.data_dir.clone();
        network_datadir.push(self.network.to_string());
        self.network_valid = !network_datadir.exists();
    }

    fn apply(&mut self, ctx: &mut Context) -> bool {
        ctx.bitcoin_config.network = self.network;
        // descriptor forms for import or creation cannot be both empty or filled.
        let user_key = DescriptorPublicKey::from_str(&self.user_xpub.value);
        self.user_xpub.valid = user_key.is_ok();
        if let Ok(key) = &user_key {
            self.user_xpub.valid = check_key_network(key, self.network);
        }

        let heir_key = DescriptorPublicKey::from_str(&self.heir_xpub.value);
        self.heir_xpub.valid = heir_key.is_ok();
        if let Ok(key) = &heir_key {
            self.heir_xpub.valid = check_key_network(key, self.network);
        }

        let sequence = self.sequence.value.parse::<u16>();
        self.sequence.valid = sequence.is_ok();

        if !self.network_valid
            || !self.user_xpub.valid
            || !self.heir_xpub.valid
            || !self.sequence.valid
        {
            return false;
        }

        let desc =
            match MultipathDescriptor::new(user_key.unwrap(), heir_key.unwrap(), sequence.unwrap())
            {
                Ok(desc) => desc,
                Err(e) => {
                    self.error = Some(e.to_string());
                    return false;
                }
            };

        ctx.descriptor = Some(desc);
        true
    }

    fn view(&self) -> Element<Message> {
        if let Some(modal) = &self.modal {
            modal.view()
        } else {
            view::define_descriptor(
                self.network,
                self.network_valid,
                &self.user_xpub,
                &self.heir_xpub,
                &self.sequence,
                self.error.as_ref(),
            )
        }
    }
}

fn check_key_network(key: &DescriptorPublicKey, network: Network) -> bool {
    match key {
        DescriptorPublicKey::XPub(key) => {
            if network == Network::Bitcoin {
                key.xkey.network == Network::Bitcoin
            } else {
                key.xkey.network == Network::Testnet
            }
        }
        DescriptorPublicKey::MultiXPub(key) => {
            if network == Network::Bitcoin {
                key.xkey.network == Network::Bitcoin
            } else {
                key.xkey.network == Network::Testnet
            }
        }
        _ => true,
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

pub struct GetHardwareWalletXpubModal {
    is_heir: bool,
    chosen_hw: Option<usize>,
    processing: bool,
    hws: Vec<HardwareWallet>,
    error: Option<Error>,
}

impl GetHardwareWalletXpubModal {
    fn new(is_heir: bool) -> Self {
        Self {
            is_heir,
            chosen_hw: None,
            processing: false,
            hws: Vec::new(),
            error: None,
        }
    }
    fn load(&self) -> Command<Message> {
        Command::perform(
            list_hardware_wallets(&[], None),
            Message::ConnectedHardwareWallets,
        )
    }
    fn update(&mut self, message: Message) -> Command<Message> {
        match message {
            Message::Select(i) => {
                if let Some(hw) = self.hws.get(i) {
                    let device = hw.device.clone();
                    self.chosen_hw = Some(i);
                    self.processing = true;
                    return Command::perform(get_extended_pubkey(device, hw.fingerprint), |res| {
                        Message::DefineDescriptor(message::DefineDescriptor::XpubImported(
                            res.map(|key| key.to_string()),
                        ))
                    });
                }
            }
            Message::ConnectedHardwareWallets(hws) => {
                self.hws = hws;
            }
            Message::Reload => {
                return self.load();
            }
            Message::DefineDescriptor(message::DefineDescriptor::XpubImported(res)) => {
                self.processing = false;
                match res {
                    Ok(key) => {
                        if self.is_heir {
                            return Command::perform(
                                async move { key },
                                message::DefineDescriptor::HeirXpubEdited,
                            )
                            .map(Message::DefineDescriptor);
                        } else {
                            return Command::perform(
                                async move { key },
                                message::DefineDescriptor::UserXpubEdited,
                            )
                            .map(Message::DefineDescriptor);
                        }
                    }
                    Err(e) => {
                        self.error = Some(e);
                    }
                }
            }
            _ => {}
        };
        Command::none()
    }
    fn view(&self) -> Element<Message> {
        view::hardware_wallet_xpubs_modal(
            self.is_heir,
            &self.hws,
            self.error.as_ref(),
            self.processing,
            self.chosen_hw,
        )
    }
}

async fn get_extended_pubkey(
    hw: std::sync::Arc<dyn async_hwi::HWI + Send + Sync>,
    fingerprint: Fingerprint,
) -> Result<DescriptorPublicKey, Error> {
    let derivation_path = DerivationPath::master();
    let xkey = hw
        .get_extended_pubkey(&derivation_path, true)
        .await
        .map_err(Error::from)?;
    Ok(DescriptorPublicKey::MultiXPub(DescriptorMultiXKey {
        origin: Some((fingerprint, derivation_path)),
        derivation_paths: vec![
            DerivationPath::from_str("m/0").unwrap(),
            DerivationPath::from_str("m/1").unwrap(),
        ],
        xkey,
        wildcard: Wildcard::Unhardened,
    }))
}

pub struct ImportDescriptor {
    network: Network,
    network_valid: bool,
    data_dir: Option<PathBuf>,
    imported_descriptor: form::Value<String>,
    error: Option<String>,
}

impl ImportDescriptor {
    pub fn new() -> Self {
        Self {
            network: Network::Bitcoin,
            network_valid: true,
            data_dir: None,
            imported_descriptor: form::Value::default(),
            error: None,
        }
    }
}

impl Step for ImportDescriptor {
    // form value is set as valid each time it is edited.
    // Verification of the values is happening when the user click on Next button.
    fn update(&mut self, message: Message) -> Command<Message> {
        match message {
            Message::Network(network) => {
                self.network = network;
                let mut network_datadir = self.data_dir.clone().unwrap();
                network_datadir.push(self.network.to_string());
                self.network_valid = !network_datadir.exists();
            }
            Message::DefineDescriptor(message::DefineDescriptor::ImportDescriptor(desc)) => {
                self.imported_descriptor.value = desc;
                self.imported_descriptor.valid = true;
            }
            _ => {}
        };
        Command::none()
    }

    fn load_context(&mut self, ctx: &Context) {
        self.network = ctx.bitcoin_config.network;
        self.data_dir = Some(ctx.data_dir.clone());
        let mut network_datadir = ctx.data_dir.clone();
        network_datadir.push(self.network.to_string());
        self.network_valid = !network_datadir.exists();
    }

    fn apply(&mut self, ctx: &mut Context) -> bool {
        ctx.bitcoin_config.network = self.network;
        // descriptor forms for import or creation cannot be both empty or filled.
        if !self.imported_descriptor.value.is_empty() {
            if let Ok(desc) = MultipathDescriptor::from_str(&self.imported_descriptor.value) {
                ctx.descriptor = Some(desc);
                true
            } else {
                self.imported_descriptor.valid = false;
                false
            }
        } else {
            false
        }
    }

    fn view(&self) -> Element<Message> {
        view::import_descriptor(
            self.network,
            self.network_valid,
            &self.imported_descriptor,
            self.error.as_ref(),
        )
    }
}

impl Default for ImportDescriptor {
    fn default() -> Self {
        Self::new()
    }
}

impl From<ImportDescriptor> for Box<dyn Step> {
    fn from(s: ImportDescriptor) -> Box<dyn Step> {
        Box::new(s)
    }
}

#[derive(Default)]
pub struct RegisterDescriptor {
    descriptor: Option<MultipathDescriptor>,
    processing: bool,
    chosen_hw: Option<usize>,
    hws: Vec<(HardwareWallet, Option<[u8; 32]>)>,
    error: Option<Error>,
}

impl Step for RegisterDescriptor {
    fn load_context(&mut self, ctx: &Context) {
        self.descriptor = ctx.descriptor.clone();
    }
    fn update(&mut self, message: Message) -> Command<Message> {
        match message {
            Message::Select(i) => {
                if let Some((hw, hmac)) = self.hws.get(i) {
                    if hmac.is_none() {
                        let device = hw.device.clone();
                        let descriptor = self.descriptor.as_ref().unwrap().to_string();
                        self.chosen_hw = Some(i);
                        self.processing = true;
                        self.error = None;
                        return Command::perform(
                            register_wallet(device, hw.fingerprint, descriptor),
                            Message::WalletRegistered,
                        );
                    }
                }
            }
            Message::WalletRegistered(res) => {
                self.processing = false;
                self.chosen_hw = None;
                match res {
                    Ok((fingerprint, hmac)) => {
                        if let Some(hw_h) = self
                            .hws
                            .iter_mut()
                            .find(|hw_h| hw_h.0.fingerprint == fingerprint)
                        {
                            hw_h.1 = Some(hmac.unwrap_or([0x00; 32]));
                        }
                    }
                    Err(e) => self.error = Some(e),
                }
            }
            Message::ConnectedHardwareWallets(hws) => {
                for hw in hws {
                    if !self
                        .hws
                        .iter()
                        .any(|(h, _)| h.fingerprint == hw.fingerprint)
                    {
                        self.hws.push((hw, None));
                    }
                }
            }
            Message::Reload => {
                return self.load();
            }
            _ => {}
        };
        Command::none()
    }
    fn apply(&mut self, ctx: &mut Context) -> bool {
        for (hw, token) in &self.hws {
            if let Some(token) = token {
                if *token != [0x00; 32] {
                    ctx.hw_tokens.push((hw.kind, hw.fingerprint, *token));
                }
            }
        }
        true
    }
    fn load(&self) -> Command<Message> {
        Command::perform(
            list_hardware_wallets(&[], None),
            Message::ConnectedHardwareWallets,
        )
    }
    fn view(&self) -> Element<Message> {
        let desc = self.descriptor.as_ref().unwrap();
        view::register_descriptor(
            desc.to_string(),
            &self.hws,
            self.error.as_ref(),
            self.processing,
            self.chosen_hw,
        )
    }
}

async fn register_wallet(
    hw: std::sync::Arc<dyn async_hwi::HWI + Send + Sync>,
    fingerprint: Fingerprint,
    descriptor: String,
) -> Result<(Fingerprint, Option<[u8; 32]>), Error> {
    let hmac = hw
        .register_wallet("Liana", &descriptor)
        .await
        .map_err(Error::from)?;
    Ok((fingerprint, hmac))
}

impl From<RegisterDescriptor> for Box<dyn Step> {
    fn from(s: RegisterDescriptor) -> Box<dyn Step> {
        Box::new(s)
    }
}
