use crate::{
    gui::Message,
    gui::Message::LedgerServiceMsg,
    ledger_lib::{
        self, ledger_api
    },
    listener,
    service::ServiceFn,
};

use ledger_transport_hidapi::TransportNativeHID;
use std::fmt::{Display, Formatter};
use std::time::Duration;

listener!(LedgerListener, LedgerMessage, Message, LedgerServiceMsg);

#[derive(Debug, Clone)]
pub enum Version {
    Installed(String),
    NotInstalled,
    None,
}

impl Display for Version {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Version::Installed(version) => {
                write!(f, "{}", version)
            }
            Version::NotInstalled => {
                write!(f, "Not installed!")
            }
            Version::None => {
                write!(f, " - ")
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum Model {
    NanoS,
    NanoSP,
    NanoX,
    Unknown,
}

impl Display for Model {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Model::NanoS => {
                write!(f, "Nano S")
            }
            Model::NanoSP => {
                write!(f, "Nano S+")
            }
            Model::NanoX => {
                write!(f, "Nano X")
            }
            _ => {
                write!(f, "")
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum LedgerMessage {
    #[allow(unused)]
    UpdateMain,
    InstallMain,
    #[allow(unused)]
    UpdateTest,
    InstallTest,
    TryConnect,

    Connected(Option<String>, Option<String>),
    MainAppVersion(Version),
    #[allow(unused)]
    MainAppNextVersion(Version),
    TestAppVersion(Version),
    #[allow(unused)]
    TestAppNextVersion(Version),
    DisplayMessage(String, bool),
}

pub struct LedgerService {
    sender: Sender<LedgerMessage>,
    receiver: Receiver<LedgerMessage>,
    loopback: Sender<LedgerMessage>,
    device_version: Option<String>,
    mainnet_version: Version,
    testnet_version: Version,
}

impl LedgerService {
    pub fn start(mut self) {
        tokio::spawn(async move {
            self.run().await;
        });
    }
    /// Send a LedgerMessage to the GUI via async-channel
    fn send_to_gui(&self, msg: LedgerMessage) {
        let sender = self.sender.clone();
        tokio::spawn(async move {
            if sender.send(msg).await.is_err() {
                log::debug!("LedgerService.send_to_gui() -> Fail to send Message")
            };
        });
    }

    /// Handle a LedgerMessage received from the GUI via async-channel
    fn handle_message(&mut self, msg: LedgerMessage) {
        match &msg {
            LedgerMessage::TryConnect => {
                if self.device_version.is_none() {
                    self.poll_later();
                    self.poll();
                }
            }
            LedgerMessage::UpdateMain => self.update_main(),
            LedgerMessage::InstallMain => self.install_main(),
            LedgerMessage::UpdateTest => self.update_test(),
            LedgerMessage::InstallTest => self.install_test(),
            _ => {
                log::debug!("LedgerService.handle_message({:?}) -> unhandled!", msg)
            }
        }
    }

    /// Delayed self sent message in order to call poll() later
    fn poll_later(&self) {
        let loopback = self.loopback.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            if loopback.send(LedgerMessage::TryConnect).await.is_err() {
                log::debug!("Fail to send Message")
            };
        });
    }

    /// Try to connect to the ledger device and get firmware and bitcoin apps versions
    fn poll(&mut self) {
        if self.device_version.is_none() {
            let sender = self.sender.clone();
            log::info!("Try to poll device...");
            if let Some(transport) = self.connect() {
                if let Ok(info) = ledger_lib::get_version_info(
                    transport,
                    &self.device_version,
                    |model, version| {
                        self.send_to_gui(LedgerMessage::Connected(model, version));
                    },
                    |msg, alarm| Self::display_message(&sender, msg, alarm),
                ) {
                    match (info.device_model, info.device_version) {
                        (None, Some(version)) => {
                            self.device_version = Some(version);
                        }
                        (Some(model), Some(version)) => {
                            self.device_version = Some(version.clone());
                            self.send_to_gui(LedgerMessage::Connected(
                                Some(model.to_string()),
                                Some(version),
                            ));
                        }
                        _ => {}
                    }
                    if let (Some(main), Some(test)) = (info.mainnet_version, info.testnet_version) {
                        self.mainnet_version = main;
                        self.testnet_version = test;
                        self.update_apps_version();
                    }
                }
            } else {
                // Inform GUI that ledger disconnected
                self.send_to_gui(LedgerMessage::Connected(None, None));
                log::debug!("No transport");
            }
        }
    }

    fn connect(&self) -> Option<TransportNativeHID> {
        if let Some(api) = &ledger_api().ok() {
            TransportNativeHID::new(api).ok()
        } else {
            None
        }
    }

    fn update_apps_version(&self) {
        match &self.mainnet_version {
            Version::None => {}
            _ => {
                self.send_to_gui(LedgerMessage::MainAppVersion(self.mainnet_version.clone()));
            }
        }
        match &self.testnet_version {
            Version::None => {}
            _ => {
                self.send_to_gui(LedgerMessage::TestAppVersion(self.testnet_version.clone()));
            }
        }
    }

    fn install(&mut self, testnet: bool) {
        self.send_to_gui(LedgerMessage::MainAppVersion(Version::None));
        self.send_to_gui(LedgerMessage::TestAppVersion(Version::None));

        self.install_app(testnet);

        self.device_version = None;
        self.poll();
    }

    fn install_app(&mut self, testnet: bool) {
        let sender = self.sender.clone();
        if let Some(transport) = self.connect() {
            ledger_lib::install_app(&transport,
                                    |msg, alarm| Self::display_message(&sender, msg, alarm),
                                    testnet)
        }

    }

    fn install_main(&mut self) {
        self.install(false);
    }

    fn update_main(&mut self) {
        self.install(false);
    }

    fn install_test(&mut self) {
        self.install(true);
    }

    fn update_test(&mut self) {
        self.install(true);
    }

    fn display_message(sender: &Sender<LedgerMessage>, msg: &str, alarm: bool) {
        let sender = sender.clone();
        let msg = LedgerMessage::DisplayMessage(msg.to_string(), alarm);
        tokio::spawn(async move {
            if sender.send(msg).await.is_err() {
                log::debug!("LedgerService.send_to_gui() -> Fail to send Message")
            };
        });
    }
}

impl ServiceFn<LedgerMessage, Sender<LedgerMessage>> for LedgerService {
    fn new(
        sender: Sender<LedgerMessage>,
        receiver: Receiver<LedgerMessage>,
        loopback: Sender<LedgerMessage>,
    ) -> Self {
        LedgerService {
            sender,
            receiver,
            loopback,
            device_version: None,
            mainnet_version: Version::None,
            testnet_version: Version::None,
        }
    }

    async fn run(&mut self) {
        self.poll();
        self.poll_later();
        loop {
            if let Ok(msg) = self.receiver.try_recv() {
                self.handle_message(msg);
            }
            // cpu load is not visible w/ 10ns but we can increase it w/o performance penalty
            tokio::time::sleep(Duration::from_nanos(10)).await;
        }
    }
}
