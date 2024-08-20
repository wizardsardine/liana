use std::{
    fmt::Debug,
    sync::mpsc::{self, TryRecvError},
    time::Duration,
};

use crate::hw::{ledger_id, HardwareWallet, HardwareWalletMessage, HardwareWallets};
use async_hwi::{
    ledger::{self, HidApi},
    Version, HWI,
};
use iced::{subscription, Subscription};
use ledger_manager::{
    ledger_transport_hidapi::TransportNativeHID,
    utils::{install_app, ledger_api, InstallStep, Step},
};
use liana::miniscript::bitcoin::Network;

#[derive(Debug, Clone)]
pub struct UpgradeProgress {
    pub id: String,
    pub step: InstallStep<Version>,
    pub percent: Option<f32>,
}

#[derive(Debug, Clone)]
pub enum UpgradeMessage {
    None { id: String },
    Progress(UpgradeProgress),
    Completed { id: String, version: Version },
    Error { id: String, msg: String },
    Stopped { id: String },
}

impl From<UpgradeMessage> for HardwareWalletMessage {
    fn from(value: UpgradeMessage) -> Self {
        HardwareWalletMessage::Upgrade(value)
    }
}

impl UpgradeMessage {
    pub fn id(&self) -> &String {
        match self {
            UpgradeMessage::Progress(UpgradeProgress { id, .. }) => id,
            UpgradeMessage::Completed { id, .. } => id,
            UpgradeMessage::Error { id, .. } => id,
            UpgradeMessage::None { id } => id,
            UpgradeMessage::Stopped { id } => id,
        }
    }

    pub fn update(self, hw: &mut HardwareWallet) {
        match self {
            UpgradeMessage::Progress(UpgradeProgress { step, .. }) => {
                if step.is_message() {
                    let msg = step.message();
                    hw.push_log(msg);
                }
            }
            UpgradeMessage::Completed { version, .. } => {
                hw.upgrade_ended(version);
            }
            UpgradeMessage::Error { msg, .. } => {
                hw.push_log(msg);
                hw.upgrade_failed();
            }
            _ => {}
        }
    }
}

pub enum State {
    Idle,
    Running,
    Stopped,
}

pub struct UpgradeState {
    id: String,
    state: State,
    step: InstallStep<Version>,
    chunks: usize,
    percent: f32,
    testnet: bool,
    receiver: mpsc::Receiver<InstallStep<Version>>,
    sender: mpsc::Sender<InstallStep<Version>>,
}

impl UpgradeState {
    pub fn new(id: String, testnet: bool) -> Self {
        let (sender, receiver) = mpsc::channel();
        UpgradeState {
            id,
            step: InstallStep::Info("Not yet started".into()),
            chunks: 0,
            percent: 0.0,
            receiver,
            state: State::Idle,
            sender,
            testnet,
        }
    }

    fn start(&mut self) {
        let sender = self.sender.clone();
        let testnet = self.testnet;
        let id = self.id.clone();
        tokio::spawn(async move {
            let (i, s) = (id.clone(), sender.clone());
            // install the app
            install_app(
                id,
                connect,
                |msg| {
                    let _ = sender.send(msg);
                },
                testnet,
                true,
            );

            // then check installed version
            let mut installed_version = None;
            if let Ok(api) = HidApi::new() {
                if let Some(hw) = ledger::Ledger::<ledger::TransportHID>::enumerate(&api)
                    .find(|dev| ledger_id(dev) == i)
                {
                    if let Ok(device) = ledger::Ledger::<ledger::TransportHID>::connect(&api, hw) {
                        if let Ok(v) = device.get_version().await {
                            installed_version = Some(v);
                        }
                    }
                }
            }
            // report to the GUI the installed version
            if let Some(version) = installed_version {
                let _ = s.send(InstallStep::InstalledVersion(version));
            } else {
                let _ = s.send(InstallStep::Error("Cannot check installed version".into()));
            }
        });
        self.state = State::Running;
    }

    fn update(&mut self, msg: InstallStep<Version>) -> HardwareWalletMessage {
        if !matches!(msg, InstallStep::Info(_)) {
            self.step = msg.clone();
        }
        let percent = match &msg {
            InstallStep::NotStarted => Some(0.0),
            InstallStep::Started => Some(1.0),
            InstallStep::CloseApp => Some(5.0),
            InstallStep::Info(_) => None,
            InstallStep::AllowInstall => Some(10.0),
            InstallStep::Chunk => Some(self.chunks as f32 * 4.0 + 10.0),
            InstallStep::Completed => Some(99.0),
            InstallStep::Error(_) => None,
            InstallStep::InstalledVersion(_) => Some(100.0),
        };
        if let Some(p) = percent {
            self.percent = p;
        }

        match msg {
            InstallStep::InstalledVersion(v) => {
                self.state = State::Stopped;
                UpgradeMessage::Completed {
                    id: self.id.clone(),
                    version: v,
                }
                .into()
            }
            InstallStep::Error(e) => {
                self.state = State::Stopped;
                UpgradeMessage::Error {
                    id: self.id.clone(),
                    msg: e,
                }
                .into()
            }
            s => UpgradeMessage::Progress(UpgradeProgress {
                id: self.id.clone(),
                step: s,
                percent: Some(self.percent),
            })
            .into(),
        }
    }
}

pub fn connect(id: &str) -> Option<TransportNativeHID> {
    if let Ok(api) = ledger_api() {
        let device = TransportNativeHID::list_ledgers(&api).find(|device| {
            let dev_id = ledger_id(device);
            dev_id == *id
        })?;
        return TransportNativeHID::open_device(&api, device).ok();
    }
    None
}

pub fn maybe_start_upgrade(
    id: String,
    hws: &mut HardwareWallets,
    network: Network,
) -> Option<bool> {
    let mut upgrading = None;
    if let Some(hw) = hws.list.iter_mut().find(|h| h.id() == &id) {
        if !hw.is_upgrade_in_progress() {
            upgrading = Some(true);
            hw.start_upgrade(network)
        }
    }
    upgrading
}

pub fn update_upgrade_state(msg: UpgradeMessage, hws: &mut HardwareWallets) -> Option<bool> {
    let mut upgrading = None;
    match msg {
        UpgradeMessage::Error { .. } | UpgradeMessage::Completed { .. } => upgrading = Some(false),
        _ => {}
    }
    if let Some(hw) = hws.list.iter_mut().find(|hw| hw.id() == msg.id()) {
        msg.update(hw)
    }
    upgrading
}

pub fn ledger_upgrade_subscriptions(
    hws: &HardwareWallets,
) -> Vec<Subscription<HardwareWalletMessage>> {
    hws.list
        .iter()
        .filter_map(|hw| {
            if let HardwareWallet::NeedUpgrade {
                upgrade_in_progress,
                upgrade_testnet,
                upgrade_step,
                id,
                ..
            } = hw
            {
                if *upgrade_in_progress && upgrade_step.is_none() {
                    Some(subscription::unfold(
                        id.clone(),
                        UpgradeState::new(id.clone(), *upgrade_testnet),
                        ledger_subscription,
                    ))
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect()
}

async fn ledger_subscription(mut state: UpgradeState) -> (HardwareWalletMessage, UpgradeState) {
    fn poll(mut state: UpgradeState) -> (Option<HardwareWalletMessage>, UpgradeState) {
        match &state.state {
            State::Idle => {
                state.start();
                (Some(state.update(InstallStep::NotStarted)), state)
            }
            State::Running => match state.receiver.try_recv() {
                Ok(msg) => (Some(state.update(msg)), state),
                Err(TryRecvError::Empty) => (None, state),
                Err(TryRecvError::Disconnected) => {
                    let error = InstallStep::Error("Disconnected".into());
                    (Some(state.update(error)), state)
                }
            },
            State::Stopped => (
                Some(
                    UpgradeMessage::Stopped {
                        id: state.id.clone(),
                    }
                    .into(),
                ),
                state,
            ),
        }
    }

    loop {
        let (m, s) = poll(state);
        if let Some(message) = m {
            return (message, s);
        } else {
            tokio::time::sleep(Duration::from_millis(500)).await;
            state = s;
        }
    }
}
