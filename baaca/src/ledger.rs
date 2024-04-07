use crate::{
    client::ClientFn,
    gui::Message,
    gui::Message::LedgerClientMsg,
    ledger_lib::{
        bitcoin_app, list_installed_apps, query_via_websocket, DeviceInfo, BASE_SOCKET_URL,
    }, ledger_manager::{device_info, ledger_api}, listener
};

use form_urlencoded::Serializer as UrlSerializer;
use ledger_transport_hidapi::TransportNativeHID;
use std::fmt::{Display, Formatter};
use std::time::Duration;

listener!(LedgerListener, LedgerMessage, Message, LedgerClientMsg);

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

pub struct LedgerClient {
    sender: Sender<LedgerMessage>,
    receiver: Receiver<LedgerMessage>,
    loopback: Sender<LedgerMessage>,
    device_version: Option<String>,
    mainnet_version: Version,
    testnet_version: Version,
}

impl LedgerClient {
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
                log::debug!("LedgerClient.send_to_gui() -> Fail to send Message")
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
                log::debug!("LedgerClient.handle_message({:?}) -> unhandled!", msg)
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
        log::info!("Try to poll device...");
        if let Some(transport) = self.connect() {
            let mut device_version: Option<String> = None;

            let info = match device_info(&transport) {
                Ok(info) => {
                    log::info!("Device connected");
                    log::debug!("Device version: {}", &info.version);
                    self.display_message(
                        &format!("Device connected, version: {}", &info.version),
                        false,
                    );
                    if self.device_version.is_none() {
                        self.send_to_gui(LedgerMessage::Connected(
                            Some("Ledger".to_string()),
                            Some(info.version.clone()),
                        ));
                    }
                    device_version = Some(info.version.clone());
                    Some(info)
                }
                Err(e) => {
                    log::debug!("Failed connect device: {}", &e);
                    self.display_message(&e, true);
                    None
                }
            };

            if let Some(info) = info {
                // if it's our first connection, we check the if apps are installed & version
                self.display_message("Querying installed apps. Please confirm on device.", false);
                if self.device_version.is_none() && device_version.is_some() {
                    if let Ok((main_installed, test_installed)) =
                        self.check_apps_installed(&transport)
                    {
                        // get the mainnet app version name
                        let (main_model, main_version) = if main_installed {
                            match self.get_app_version(&info, true) {
                                Ok((model, version)) => (model, version),
                                Err(e) => {
                                    self.display_message(&e, true);
                                    (Model::Unknown, Version::None)
                                }
                            }
                        } else {
                            log::debug!("Mainnet app not installed!");
                            // self.display_message("Mainnet app not installed!", false);
                            (Model::Unknown, Version::NotInstalled)
                        };

                        // get the testnet app version name
                        let (test_model, test_version) = if test_installed {
                            match self.get_app_version(&info, true) {
                                Ok((model, version)) => (model, version),
                                Err(e) => {
                                    self.display_message(&e, false);
                                    (Model::Unknown, Version::None)
                                }
                            }
                        } else {
                            log::debug!("Testnet app not installed!");
                            // self.display_message("Testnet app not installed!", false);
                            (Model::Unknown, Version::NotInstalled)
                        };

                        let model = match (&main_model, &test_model) {
                            (Model::Unknown, _) => test_model,
                            _ => main_model,
                        };
                        self.send_to_gui(LedgerMessage::Connected(
                            Some(model.to_string()),
                            device_version.clone(),
                        ));
                        self.display_message("", false);
                        self.mainnet_version = main_version;
                        self.testnet_version = test_version;
                        self.update_apps_version();
                    }
                }
                self.device_version = device_version;
            }
        } else {
            self.send_to_gui(LedgerMessage::Connected(None, None));
            log::debug!("No transport");
        }
    }

    fn connect(&self) -> Option<TransportNativeHID> {
        if let Some(api) = &ledger_api().ok() {
            TransportNativeHID::new(api).ok()
        } else {
            None
        }
    }

    fn check_apps_installed(&mut self, transport: &TransportNativeHID) -> Result<(bool, bool), ()> {
        self.display_message("Querying installed apps. Please confirm on device.", false);
        let mut mainnet = false;
        let mut testnet = false;
        match list_installed_apps(transport) {
            Ok(apps) => {
                log::debug!("List installed apps:");
                self.display_message("List installed apps...", false);
                for app in apps {
                    log::debug!("  [{}]", &app.name);
                    if app.name == "Bitcoin" {
                        mainnet = true
                    }
                    if app.name == "Bitcoin Test" {
                        testnet = true
                    }
                }
            }
            Err(e) => {
                log::debug!("Error listing installed applications: {}.", e);
                self.send_to_gui(LedgerMessage::DisplayMessage(
                    format!("Error listing installed applications: {}.", e),
                    true,
                ));
                return Err(());
            }
        }
        if mainnet {
            log::debug!("Mainnet App installed");
        }
        if testnet {
            log::debug!("Testnet App installed");
        }
        self.display_message("", false);
        Ok((mainnet, testnet))
    }

    fn get_app_version(
        &mut self,
        info: &DeviceInfo,
        testnet: bool,
    ) -> Result<(Model, Version), String> {
        log::debug!("get_app_version()");
        match bitcoin_app(info, testnet) {
            Ok(r) => {
                log::debug!("decoding app data");
                // example for nano s
                // BitcoinAppV2 { version_name: "Bitcoin Test", perso: "perso_11", delete_key: "nanos/2.1.0/bitcoin_testnet/app_2.2.1_del_key", firmware: "nanos/2.1.0/bitcoin_testnet/app_2.2.1", firmware_key: "nanos/2.1.0/bitcoin_testnet/app_2.2.1_key", hash: "7f07efc20d96faaf8c93bd179133c88d1350113169da914f88e52beb35fcdd1e" }
                // example for nano s+
                // BitcoinAppV2 { version_name: "Bitcoin Test", perso: "perso_11", delete_key: "nanos+/1.1.0/bitcoin_testnet/app_2.2.0-beta_del_key", firmware: "nanos+/1.1.0/bitcoin_testnet/app_2.2.0-beta", firmware_key: "nanos+/1.1.0/bitcoin_testnet/app_2.2.0-beta_key", hash: "3c6d6ebebb085da948c0211434b90bc4504a04a133b8d0621aa0ee91fd3a0b4f" }
                if let Some(app) = r {
                    let chunks: Vec<&str> = app.firmware.split('/').collect();
                    let model = chunks.first().map(|m| m.to_string());
                    let version = chunks.last().map(|m| m.to_string());
                    if let (Some(model), Some(version)) = (model, version) {
                        let model = if model == "nanos" {
                            Model::NanoS
                        } else if model == "nanos+" {
                            Model::NanoSP
                        // i guess `nanox` for the nano x but i don't have device to test    
                        } else if model == "nanox" {
                            Model::NanoX
                        } else {
                            Model::Unknown
                        };

                        let version = if version.contains("app_") {
                            version.replace("app_", "")
                        } else {
                            version
                        };

                        let version = Version::Installed(version);
                        if testnet {
                            log::debug!(
                                "Testnet Model{}, Version{}",
                                model.clone(),
                                version.clone()
                            );
                        } else {
                            log::debug!(
                                "Mainnet Model{}, Version{}",
                                model.clone(),
                                version.clone()
                            );
                        }
                        Ok((model, version))
                    } else {
                        Err(format!("Failed to parse  model/version in {:?}", chunks))
                    }
                } else {
                    log::debug!("Fail to get version info");
                    Err("Fail to get version info".to_string())
                }
            }
            Err(e) => {
                log::debug!("Fail to get version info: {}", e);
                Err(format!("Fail to get version info: {}", e))
            }
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
        log::debug!("install_app(testnet={})", testnet);
        if let Some(api) = self.connect() {
            self.display_message("Get device info from API...", false);
            if let Ok(device_info) = device_info(&api) {
                let bitcoin_app = match bitcoin_app(&device_info, testnet) {
                    Ok(Some(a)) => a,
                    Ok(None) => {
                        self.display_message("Could not get info about Bitcoin app.", true);
                        return;
                    }
                    Err(e) => {
                        self.display_message(
                            &format!("Error querying info about Bitcoin app: {}.", e),
                            true,
                        );
                        return;
                    }
                };
                self.display_message(
                    "Installing, please allow Ledger manager on device...",
                    false,
                );
                // Now install the app by connecting through their websocket thing to their HSM. Make sure to
                // properly escape the parameters in the request's parameter.
                let install_ws_url = UrlSerializer::new(format!("{}/install?", BASE_SOCKET_URL))
                    .append_pair("targetId", &device_info.target_id.to_string())
                    .append_pair("perso", &bitcoin_app.perso)
                    .append_pair("deleteKey", &bitcoin_app.delete_key)
                    .append_pair("firmware", &bitcoin_app.firmware)
                    .append_pair("firmwareKey", &bitcoin_app.firmware_key)
                    .append_pair("hash", &bitcoin_app.hash)
                    .finish();
                self.display_message("Install app...", false);
                if let Err(e) = query_via_websocket(&api, &install_ws_url) {
                    self.display_message(&format!("Got an error when installing Bitcoin app from Ledger's remote HSM: {}.", e), false);
                    return;
                }
                self.display_message("Successfully installed the app.", false);
            } else {
                self.display_message("Fail to fetch device info!", true);
            }
        } else {
            self.display_message("Fail to connect to device!", true);
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

    fn display_message(&mut self, msg: &str, alarm: bool) {
        self.send_to_gui(LedgerMessage::DisplayMessage(msg.to_string(), alarm));
    }
}

impl ClientFn<LedgerMessage, Sender<LedgerMessage>> for LedgerClient {
    fn new(
        sender: Sender<LedgerMessage>,
        receiver: Receiver<LedgerMessage>,
        loopback: Sender<LedgerMessage>,
    ) -> Self {
        LedgerClient {
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
            // cpu load is not visible w/ 1ns but we can increase it w/o performance penalty
            tokio::time::sleep(Duration::from_nanos(1)).await;
        }
    }
}
