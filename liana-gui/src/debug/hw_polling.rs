//! Stateful debug screen that runs two hardware-wallet polling backends
//! side by side: the GUI's own [`crate::hw::HardwareWallets`] wrapper on
//! the left, and [`async_hwi::service::HwiService`] (used directly by the
//! business installer) on the right.
//!
//! Unlike every other debug page, this one is interactive: the host
//! ([`crate::gui`]) detects the page by pointer equality with
//! [`ENTRY`] and renders [`HwPollingState::view`] / wires
//! [`HwPollingState::update`] and [`HwPollingState::subscription`] into
//! its own message loop instead of going through
//! [`super::render_location`].

use std::{collections::BTreeMap, sync::Arc};

use async_hwi::{
    service::{HwiService, SigningDevice, SigningDeviceMsg},
    DeviceKind, Version,
};
use crossbeam::channel;
use iced::{
    futures::{SinkExt, Stream},
    Alignment, Length, Subscription, Task,
};
use liana::miniscript::bitcoin::{bip32::Fingerprint, Network};
use liana_ui::{
    component::{button, text},
    theme,
    widget::*,
};

use crate::dir::LianaDirectory;
use crate::hw::{HardwareWallet, HardwareWalletMessage, HardwareWallets, UnsupportedReason};
use crate::utils::subscription::run_with_id;

use super::{installer_chrome, DebugMessage, DebugPageEntry};

/// Placeholder entry — the host renders the stateful view directly. The
/// placeholder is only reached if the host forgets to special-case this
/// page (which would be a bug); rendering something readable beats a
/// panic.
pub static ENTRY: DebugPageEntry = DebugPageEntry {
    view: placeholder_view,
};

fn placeholder_view() -> Element<'static, DebugMessage> {
    installer_chrome(
        "HW polling",
        "liana_gui::debug::hw_polling",
        text::p1_regular(
            "(stateful page — host should render via HwPollingState; \
             this placeholder means the dispatch path is wrong)",
        ),
    )
}

/// Message produced by the HW polling debug page.
///
/// `From<SigningDeviceMsg>` is required by [`HwiService`]: the service
/// writes its own enum into the consumer's message type via this
/// conversion.
#[derive(Debug, Clone)]
pub enum HwPollingMessage {
    ToggleLegacy,
    ToggleService,
    LegacyHw(HardwareWalletMessage),
    ServiceHw(SigningDeviceMsg),
    /// Periodic re-poll of `HwiService::list`. Needed because upstream
    /// emits `SigningDeviceMsg::Update` synchronously when it spawns a
    /// device-init task, but the resulting device is only inserted into
    /// the shared map once that async task finishes — and no further
    /// Update is sent (the listener's `should_poll` then sees the device
    /// as known). A short tick covers that gap.
    ServiceTick,
}

impl From<SigningDeviceMsg> for HwPollingMessage {
    fn from(v: SigningDeviceMsg) -> Self {
        HwPollingMessage::ServiceHw(v)
    }
}

/// State shared between the two halves of the screen. Each half owns its
/// poller and its own on/off flag; the only shared piece is the network +
/// datadir used to construct both at creation time.
pub struct HwPollingState {
    legacy: HardwareWallets,
    legacy_polling: bool,

    service: Arc<HwiService<HwPollingMessage>>,
    service_polling: bool,
    service_devices: BTreeMap<String, SigningDevice<HwPollingMessage>>,
    service_sender: channel::Sender<HwPollingMessage>,
    service_receiver: channel::Receiver<HwPollingMessage>,
}

impl std::fmt::Debug for HwPollingState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HwPollingState")
            .field("legacy_polling", &self.legacy_polling)
            .field("service_polling", &self.service_polling)
            .field("service_devices", &self.service_devices.len())
            .finish()
    }
}

impl HwPollingState {
    pub fn new(datadir: LianaDirectory, network: Network) -> Self {
        let (service_sender, service_receiver) = channel::unbounded::<HwPollingMessage>();
        let rt = tokio::runtime::Handle::try_current().ok();
        let service = Arc::new(HwiService::new(network, rt));
        Self {
            legacy: HardwareWallets::new(datadir, network),
            legacy_polling: false,
            service,
            service_polling: false,
            service_devices: BTreeMap::new(),
            service_sender,
            service_receiver,
        }
    }

    pub fn update(&mut self, msg: HwPollingMessage) -> Task<HwPollingMessage> {
        match msg {
            HwPollingMessage::ToggleLegacy => {
                if self.legacy_polling {
                    self.legacy_polling = false;
                    self.legacy.reset_watch_list();
                } else {
                    self.legacy_polling = true;
                }
                Task::none()
            }
            HwPollingMessage::ToggleService => {
                if self.service_polling {
                    self.service.stop();
                    self.service_polling = false;
                    self.service_devices.clear();
                } else {
                    self.service.start(self.service_sender.clone());
                    self.service_polling = true;
                }
                Task::none()
            }
            HwPollingMessage::LegacyHw(m) => match self.legacy.update(m) {
                Ok(t) => t.map(HwPollingMessage::LegacyHw),
                Err(e) => {
                    tracing::warn!("hw_polling legacy update error: {}", e);
                    Task::none()
                }
            },
            HwPollingMessage::ServiceHw(SigningDeviceMsg::Update) => {
                self.service_devices = self.service.list();
                Task::none()
            }
            HwPollingMessage::ServiceHw(other) => {
                tracing::debug!("hw_polling ignoring service msg: {:?}", other);
                Task::none()
            }
            HwPollingMessage::ServiceTick => {
                if self.service_polling {
                    self.service_devices = self.service.list();
                }
                Task::none()
            }
        }
    }

    pub fn subscription(&self) -> Subscription<HwPollingMessage> {
        let mut subs: Vec<Subscription<HwPollingMessage>> = Vec::new();
        if self.legacy_polling {
            subs.push(self.legacy.refresh().map(HwPollingMessage::LegacyHw));
        }
        // Receiver stream is always active when the page is mounted; it
        // simply yields nothing while `service_polling` is false (no one
        // is feeding the channel).
        subs.push(run_with_id(
            "hw_polling::service",
            service_recv_stream(self.service_receiver.clone()),
        ));
        if self.service_polling {
            subs.push(
                iced::time::every(std::time::Duration::from_millis(500))
                    .map(|_| HwPollingMessage::ServiceTick),
            );
        }
        Subscription::batch(subs)
    }

    pub fn view(&self) -> Element<'_, HwPollingMessage> {
        let body = Row::new()
            .spacing(30)
            .push(Container::new(self.legacy_column()).width(Length::FillPortion(1)))
            .push(Container::new(self.service_column()).width(Length::FillPortion(1)))
            .into();
        installer_chrome_owned("HW polling", "liana_gui::debug::hw_polling", body)
    }

    fn legacy_column(&self) -> Column<'_, HwPollingMessage> {
        let toggle = button_for(self.legacy_polling).on_press(HwPollingMessage::ToggleLegacy);
        let status = if self.legacy_polling {
            "running"
        } else {
            "stopped"
        };
        let mut col = Column::new()
            .spacing(15)
            .push(text::h3("liana-gui · HardwareWallets"))
            .push(text::caption(format!(
                "crate::hw::HardwareWallets · {} device(s) · {}",
                self.legacy.list.len(),
                status,
            )))
            .push(toggle);
        for hw in &self.legacy.list {
            col = col.push(legacy_device_row(hw));
        }
        if self.legacy.list.is_empty() {
            col = col.push(text::p2_regular("(no devices)").style(theme::text::secondary));
        }
        col
    }

    fn service_column(&self) -> Column<'_, HwPollingMessage> {
        let toggle = button_for(self.service_polling).on_press(HwPollingMessage::ToggleService);
        let status = if self.service_polling {
            "running"
        } else {
            "stopped"
        };
        let mut col = Column::new()
            .spacing(15)
            .push(text::h3("async-hwi · HwiService"))
            .push(text::caption(format!(
                "async_hwi::service::HwiService · {} device(s) · {}",
                self.service_devices.len(),
                status,
            )))
            .push(toggle);
        for dev in self.service_devices.values() {
            col = col.push(service_device_row(dev));
        }
        if self.service_devices.is_empty() {
            col = col.push(text::p2_regular("(no devices)").style(theme::text::secondary));
        }
        col
    }
}

fn button_for(running: bool) -> iced::widget::Button<'static, HwPollingMessage, theme::Theme> {
    if running {
        button::destructive(None, "Stop")
    } else {
        button::primary(None, "Start")
    }
}

fn legacy_device_row(hw: &HardwareWallet) -> Container<'static, HwPollingMessage> {
    let (variant, kind, fingerprint, version, extra): (
        &'static str,
        DeviceKind,
        Option<Fingerprint>,
        Option<Version>,
        Option<String>,
    ) = match hw {
        HardwareWallet::Unsupported {
            kind,
            version,
            reason,
            ..
        } => (
            "Unsupported",
            *kind,
            None,
            version.clone(),
            Some(format_unsupported_reason(reason)),
        ),
        HardwareWallet::Locked {
            kind, pairing_code, ..
        } => (
            "Locked",
            *kind,
            None,
            None,
            pairing_code.as_ref().map(|c| format!("pairing: {c}")),
        ),
        HardwareWallet::Supported {
            kind,
            fingerprint,
            version,
            ..
        } => (
            "Supported",
            *kind,
            Some(*fingerprint),
            version.clone(),
            None,
        ),
    };
    device_row(variant, kind, fingerprint, version, extra)
}

fn service_device_row(
    dev: &SigningDevice<HwPollingMessage>,
) -> Container<'static, HwPollingMessage> {
    let (variant, extra): (&'static str, Option<String>) = match dev {
        SigningDevice::Unsupported { reason, .. } => ("Unsupported", Some(format!("{reason:?}"))),
        SigningDevice::Locked { pairing_code, .. } => (
            "Locked",
            pairing_code.as_ref().map(|c| format!("pairing: {c}")),
        ),
        SigningDevice::Supported(_) => ("Supported", None),
    };
    let kind = *dev.kind();
    let fingerprint = dev.fingerprint();
    let version = service_device_version(dev);
    device_row(variant, kind, fingerprint, version, extra)
}

fn service_device_version(dev: &SigningDevice<HwPollingMessage>) -> Option<Version> {
    match dev {
        SigningDevice::Unsupported { version, .. } => version.clone(),
        SigningDevice::Locked { .. } => None,
        SigningDevice::Supported(supported) => supported.version().cloned(),
    }
}

fn device_row(
    variant: &'static str,
    kind: DeviceKind,
    fingerprint: Option<Fingerprint>,
    version: Option<Version>,
    extra: Option<String>,
) -> Container<'static, HwPollingMessage> {
    let fp = fingerprint
        .map(|f| f.to_string())
        .unwrap_or_else(|| "·".to_string());
    let ver = version
        .as_ref()
        .map(|v| v.to_string())
        .unwrap_or_else(|| "·".to_string());
    let mut col = Column::new()
        .spacing(4)
        .push(
            Row::new()
                .spacing(10)
                .align_y(Alignment::Center)
                .push(text::p1_bold(format!("{kind:?}")))
                .push(text::caption(variant).style(theme::text::secondary)),
        )
        .push(
            Row::new()
                .spacing(15)
                .push(text::p2_regular(format!("fp: {fp}")))
                .push(text::p2_regular(format!("v: {ver}"))),
        );
    if let Some(e) = extra {
        col = col.push(text::p2_regular(e).style(theme::text::secondary));
    }
    Container::new(col)
        .padding(10)
        .style(theme::card::simple)
        .width(Length::Fill)
}

fn format_unsupported_reason(reason: &UnsupportedReason) -> String {
    match reason {
        UnsupportedReason::Version {
            minimal_supported_version,
        } => format!("min version: {minimal_supported_version}"),
        UnsupportedReason::Method(m) => format!("unsupported method: {m}"),
        UnsupportedReason::NotPartOfWallet(fp) => format!("not in wallet: {fp}"),
        UnsupportedReason::WrongNetwork => "wrong network".to_string(),
        UnsupportedReason::AppIsNotOpen => "app not open".to_string(),
    }
}

/// Owned-message variant of [`super::installer_chrome`]. The shared helper
/// hard-codes `DebugMessage` in its return type, which would force us to
/// `.map(|_| ())` and lose interactivity — so we duplicate the chrome here
/// over the generic message type.
fn installer_chrome_owned<'a, T: 'a>(
    title: &'static str,
    path: &'static str,
    body: Element<'a, T>,
) -> Element<'a, T> {
    Container::new(
        Column::new()
            .spacing(15)
            .padding(30)
            .push(
                Row::new()
                    .spacing(15)
                    .align_y(Alignment::End)
                    .push(text::h2(title))
                    .push(text::caption(path).style(theme::text::secondary)),
            )
            .push(
                Row::new()
                    .spacing(15)
                    .align_y(Alignment::End)
                    .push(text::p1_regular(super::NAV_HINT).style(theme::text::secondary)),
            )
            .push(body),
    )
    .style(theme::container::background)
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// Bridge a crossbeam receiver into an iced-compatible async Stream.
/// Drives the recv loop on a blocking thread with a short timeout so the
/// stream cooperatively wakes — pure `recv()` would park the blocking
/// thread until shutdown and never release it.
fn service_recv_stream(
    rx: channel::Receiver<HwPollingMessage>,
) -> impl Stream<Item = HwPollingMessage> + Send + 'static {
    type Sender = iced::futures::channel::mpsc::Sender<HwPollingMessage>;
    iced::stream::channel(100, move |mut output: Sender| async move {
        loop {
            let rx2 = rx.clone();
            let res = tokio::task::spawn_blocking(move || {
                rx2.recv_timeout(std::time::Duration::from_millis(500))
            })
            .await;
            match res {
                Ok(Ok(msg)) => {
                    if output.send(msg).await.is_err() {
                        break;
                    }
                }
                Ok(Err(channel::RecvTimeoutError::Timeout)) => continue,
                Ok(Err(channel::RecvTimeoutError::Disconnected)) => break,
                Err(_) => break,
            }
        }
    })
}
