use std::sync::{Arc, Mutex};

use iced::{Subscription, Task};
use liana::miniscript::bitcoin::{
    bip32::{ChildNumber, Fingerprint},
    Network,
};

use liana_ui::widget::Element;

use crate::{
    hw::{HardwareWallet, HardwareWallets},
    installer::{
        message::Message,
        step::{
            descriptor::editor::key::{default_derivation_path, get_extended_pubkey},
            Context, Step,
        },
        view, Error,
    },
    signer::Signer,
};

pub struct HardwareWalletXpubs {
    fingerprint: Fingerprint,
    xpubs: Vec<String>,
    processing: bool,
    error: Option<Error>,
}

pub struct SignerXpubs {
    signer: Arc<Mutex<Signer>>,
    xpubs: Vec<String>,
    next_account: ChildNumber,
    words: [&'static str; 12],
    did_backup: bool,
}

impl SignerXpubs {
    fn new(signer: Arc<Mutex<Signer>>) -> Self {
        let words = { signer.lock().unwrap().mnemonic() };
        Self {
            words,
            signer,
            xpubs: Vec::new(),
            next_account: ChildNumber::from_hardened_idx(0).unwrap(),
            did_backup: false,
        }
    }

    fn select(&mut self, network: Network) {
        self.next_account = self.next_account.increment().unwrap();
        let signer = self.signer.lock().unwrap();
        let derivation_path = default_derivation_path(network);
        // We keep only one for the moment.
        self.xpubs = vec![format!(
            "[{}/{}]{}",
            signer.fingerprint(),
            derivation_path.to_string().trim_start_matches("m/"),
            signer.get_extended_pubkey(&derivation_path)
        )];
    }

    pub fn view(&self) -> Element<Message> {
        view::signer_xpubs(&self.xpubs, &self.words, self.did_backup)
    }
}

pub struct ShareXpubs {
    network: Network,
    hw_xpubs: Vec<HardwareWalletXpubs>,
    xpubs_signer: SignerXpubs,
}

impl ShareXpubs {
    pub fn new(network: Network, signer: Arc<Mutex<Signer>>) -> Self {
        Self {
            network,
            hw_xpubs: Vec::new(),
            xpubs_signer: SignerXpubs::new(signer),
        }
    }
}

impl Step for ShareXpubs {
    // form value is set as valid each time it is edited.
    // Verification of the values is happening when the user click on Next button.
    fn update(
        &mut self,
        hws: &mut HardwareWallets,
        message: Message,
        _ctx: &Context,
    ) -> Task<Message> {
        match message {
            Message::ImportXpub(fg, res) => {
                if let Some(hw_xpubs) = self.hw_xpubs.iter_mut().find(|x| x.fingerprint == fg) {
                    hw_xpubs.processing = false;
                    match res {
                        Err(e) => {
                            hw_xpubs.error = e.into();
                        }
                        Ok(xpub) => {
                            hw_xpubs.error = None;
                            // We keep only one for the moment.
                            hw_xpubs.xpubs = vec![xpub.to_string()];
                        }
                    }
                }
            }
            Message::UseHotSigner => {
                self.xpubs_signer.select(self.network);
            }
            Message::UserActionDone(done) => {
                self.xpubs_signer.did_backup = done;
            }
            Message::Select(i) => {
                if let Some(HardwareWallet::Supported {
                    device,
                    fingerprint,
                    ..
                }) = hws.list.get(i)
                {
                    let device = device.clone();
                    let fingerprint = *fingerprint;
                    let network = self.network;
                    if let Some(hw_xpubs) = self
                        .hw_xpubs
                        .iter_mut()
                        .find(|x| x.fingerprint == fingerprint)
                    {
                        hw_xpubs.processing = true;
                        hw_xpubs.error = None;
                    } else {
                        self.hw_xpubs.push(HardwareWalletXpubs {
                            fingerprint,
                            xpubs: Vec::new(),
                            processing: true,
                            error: None,
                        });
                    }
                    return Task::perform(
                        async move {
                            (
                                fingerprint,
                                get_extended_pubkey(device, fingerprint, network).await,
                            )
                        },
                        |(fingerprint, res)| Message::ImportXpub(fingerprint, res),
                    );
                }
            }
            _ => {}
        };
        Task::none()
    }

    fn subscription(&self, hws: &HardwareWallets) -> Subscription<Message> {
        hws.refresh().map(Message::HardwareWallets)
    }

    fn apply(&mut self, ctx: &mut Context) -> bool {
        ctx.bitcoin_config.network = self.network;
        // Drop connections to hardware wallets.
        self.hw_xpubs = Vec::new();
        true
    }

    fn view<'a>(
        &'a self,
        hws: &'a HardwareWallets,
        _progress: (usize, usize),
        email: Option<&'a str>,
    ) -> Element<Message> {
        view::share_xpubs(
            email,
            hws.list
                .iter()
                .enumerate()
                .map(|(i, hw)| {
                    if let Some(hw_xpubs) = self
                        .hw_xpubs
                        .iter()
                        .find(|h| hw.fingerprint() == Some(h.fingerprint))
                    {
                        view::hardware_wallet_xpubs(
                            i,
                            hw,
                            Some(&hw_xpubs.xpubs),
                            hw_xpubs.processing,
                            hw_xpubs.error.as_ref(),
                        )
                    } else {
                        view::hardware_wallet_xpubs(i, hw, None, false, None)
                    }
                })
                .collect(),
            self.xpubs_signer.view(),
        )
    }
}

impl From<ShareXpubs> for Box<dyn Step> {
    fn from(s: ShareXpubs) -> Box<dyn Step> {
        Box::new(s)
    }
}
