use std::{
    fmt::Debug,
    marker::PhantomData,
    sync::mpsc::{self, TryRecvError},
    time::Duration,
};

use crate::{
    app::Message as AppMessage,
    hw::{ledger_id, HardwareWallet, HardwareWallets},
    installer::Message as InstallerMessage,
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
    pub step: InstallStep,
    pub percent: Option<f32>,
}

#[derive(Debug, Clone)]
pub enum UpgradeMessage {
    None { id: String },
    Progress(UpgradeProgress),
    Completed { id: String },
    Error { id: String, msg: String },
    Stopped { id: String },
}

impl UpgradeMessage {
    pub fn id(&self) -> &String {
        match self {
            UpgradeMessage::Progress(UpgradeProgress { id, .. }) => id,
            UpgradeMessage::Completed { id } => id,
            UpgradeMessage::Error { id, .. } => id,
            UpgradeMessage::None { id } => id,
            UpgradeMessage::Stopped { id } => id,
        }
    }

    pub fn update(self, hw: &mut HardwareWallet) {
        match self {
            UpgradeMessage::Progress(UpgradeProgress { step, .. }) => {
                // TODO: handle percentage progress
                if step.is_message() {
                    let msg = step.message();
                    hw.push_log(msg);
                }
            }
            UpgradeMessage::Completed { .. } => {
                hw.upgrade_ended();
                // TODO: did refresh() will detect as Supported now?
            }
            UpgradeMessage::Error { msg, .. } => {
                hw.push_log(msg);
                hw.upgrade_failed();
            }
            _ => {}
        }
    }
}

impl From<UpgradeMessage> for AppMessage {
    fn from(value: UpgradeMessage) -> Self {
        AppMessage::Upgrade(value)
    }
}

impl From<UpgradeMessage> for InstallerMessage {
    fn from(value: UpgradeMessage) -> Self {
        InstallerMessage::Upgrade(value)
    }
}

pub enum State {
    Idle,
    Running,
    Stopped,
}

pub struct UpgradeState<T: From<UpgradeMessage>> {
    id: String,
    state: State,
    step: InstallStep,
    chunks: usize,
    percent: f32,
    testnet: bool,
    receiver: mpsc::Receiver<InstallStep>,
    sender: mpsc::Sender<InstallStep>,
    _marker: PhantomData<T>,
}

#[allow(unused)]
impl<T: From<UpgradeMessage>> UpgradeState<T> {
    pub fn new(id: String, testnet: bool) -> Self {
        let (sender, receiver) = mpsc::channel();
        UpgradeState::<T> {
            id,
            step: InstallStep::Info("Not yet started".into()),
            chunks: 0,
            percent: 0.0,
            receiver,
            _marker: Default::default(),
            state: State::Idle,
            sender,
            testnet,
        }
    }

    fn start(&mut self) {
        if let Some(api) = connect(self.id.clone()) {
            let sender = self.sender.clone();
            let testnet = self.testnet;
            tokio::spawn(async move {
                install_app(
                    &api,
                    |msg| {
                        let _ = sender.send(msg);
                    },
                    testnet,
                )
            });
            self.state = State::Running;
        } else {
            let _ = self
                .sender
                .send(InstallStep::Error("Fail to get ledger HID".into()));
        }
    }

    fn update(&mut self, msg: InstallStep) -> T {
        log::info!("UpgradeState.update({:?})", msg);
        if !matches!(msg, InstallStep::Info(_)) {
            self.step = msg.clone();
        }
        let percent = match &msg {
            InstallStep::NotStarted => Some(0.0),
            InstallStep::CloseApp => Some(1.0),
            InstallStep::Started => Some(5.0),
            InstallStep::Info(_) => None,
            InstallStep::AllowInstall => Some(10.0),
            InstallStep::Chunk => Some(self.chunks as f32 * 4.0 + 10.0),
            InstallStep::Completed => Some(100.0),
            InstallStep::Error(_) => None,
        };
        if let Some(p) = percent {
            self.percent = p;
        }

        match msg {
            InstallStep::Completed => {
                self.state = State::Stopped;
                UpgradeMessage::Completed {
                    id: self.id.clone(),
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

// impl<T: From<UpgradeMessage> + Unpin + Debug> Stream for UpgradeState<T> {
//     type Item = T;
//
//     fn poll_next(
//         self: std::pin::Pin<&mut Self>,
//         _cx: &mut std::task::Context<'_>,
//     ) -> std::task::Poll<Option<Self::Item>> {
//         let state = self.get_mut();
//         if state.stop {
//             Poll::Ready(None)
//         } else {
//             match state.receiver.try_recv() {
//                 Ok(msg) => {
//                     log::info!("{:?}", msg);
//                     let ret = state.update(msg.clone());
//                     Poll::Ready(Some(ret))
//                 }
//                 Err(TryRecvError::Empty) => Poll::Pending,
//                 Err(TryRecvError::Disconnected) => {
//                     log::info!("UpgradeState: Disconnected!");
//                     let msg = InstallStep::Error("Disconnected".into());
//                     let ret = state.update(msg);
//                     Poll::Ready(Some(ret))
//                 }
//             }
//         }
//     }
// }

fn connect(id: String) -> Option<TransportNativeHID> {
    if let Ok(api) = ledger_api() {
        let device = TransportNativeHID::list_ledgers(&api).find(|device| {
            let dev_id = ledger_id(device.path(), device.vendor_id(), device.product_id());
            dev_id == id
        })?;
        return TransportNativeHID::open_device(&api, device).ok();
    }
    None
}

pub fn maybe_start_upgrade(id: String, hws: &mut HardwareWallets, network: Network) {
    if let Some(hw) = hws.list.iter_mut().find(|h| h.id() == &id) {
        if !hw.is_upgrade_in_progress() {
            hw.start_upgrade(network)
        }
    }
}

pub fn update_upgrade_state(msg: UpgradeMessage, hws: &mut HardwareWallets) {
    if let Some(hw) = hws.list.iter_mut().find(|hw| hw.id() == msg.id()) {
        msg.update(hw)
    }
}

pub fn maybe_ledger_upgrade_subscription<
    T: From<UpgradeMessage> + Debug + Unpin + Send + 'static,
>(
    hws: &HardwareWallets,
) -> Vec<Subscription<T>> {
    hws.list
        .iter()
        .filter_map(|hw| {
            if let HardwareWallet::Supported {
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

async fn ledger_subscription<Message: From<UpgradeMessage> + Debug>(
    mut state: UpgradeState<Message>,
) -> (Message, UpgradeState<Message>) {
    fn poll<M: From<UpgradeMessage> + Debug>(
        mut state: UpgradeState<M>,
    ) -> (Option<M>, UpgradeState<M>) {
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
            log::info!("ledger_subscription() -> {:?}", message);
            return (message, s);
        } else {
            tokio::time::sleep(Duration::from_millis(500)).await;
            state = s;
        }
    }
}
