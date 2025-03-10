use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use iced::{Subscription, Task};
use liana_ui::{component::modal::Modal, widget::Element};
use tokio::task::JoinHandle;

use crate::{
    app::{
        self,
        view::{self, export::export_modal},
    },
    daemon::Daemon,
    export::{self, get_path, ExportMessage, ExportProgress, ExportState, ExportType},
};

#[derive(Debug)]
pub struct ExportModal {
    path: Option<PathBuf>,
    handle: Option<Arc<Mutex<JoinHandle<()>>>>,
    state: ExportState,
    error: Option<export::Error>,
    daemon: Arc<dyn Daemon + Sync + Send>,
    export_type: ExportType,
}

impl ExportModal {
    #[allow(clippy::new_without_default)]
    pub fn new(daemon: Arc<dyn Daemon + Sync + Send>, export_type: ExportType) -> Self {
        Self {
            path: None,
            handle: None,
            state: ExportState::Init,
            error: None,
            daemon,
            export_type,
        }
    }

    pub fn default_filename(&self) -> String {
        match &self.export_type {
            ExportType::Transactions => {
                let date = chrono::Local::now().format("%Y-%m-%dT%H-%M-%S");
                format!("liana-txs-{date}.csv")
            }
            ExportType::Psbt(_) => todo!(),
            ExportType::Descriptor(descriptor) => {
                let checksum = descriptor
                    .to_string()
                    .split_once('#')
                    .map(|(_, checksum)| checksum)
                    .expect("cannot fail")
                    .to_string();
                format!("liana-{}.descriptor", checksum)
            }
        }
    }

    pub fn launch(&self) -> Task<app::message::Message> {
        Task::perform(get_path(self.default_filename()), |m| {
            app::message::Message::View(view::Message::Export(ExportMessage::Path(m)))
        })
    }

    pub fn update(&mut self, message: ExportMessage) -> Task<app::message::Message> {
        match message {
            ExportMessage::ExportProgress(m) => match m {
                ExportProgress::Started(handle) => {
                    self.handle = Some(handle);
                    self.state = ExportState::Progress(0.0);
                }
                ExportProgress::Progress(p) => {
                    if let ExportState::Progress(_) = self.state {
                        self.state = ExportState::Progress(p);
                    }
                }
                ExportProgress::Finished | ExportProgress::Ended => self.state = ExportState::Ended,
                ExportProgress::Error(e) => self.error = Some(e),
                ExportProgress::None => {}
            },
            ExportMessage::TimedOut => {
                self.stop(ExportState::TimedOut);
            }
            ExportMessage::UserStop => {
                self.stop(ExportState::Aborted);
            }
            ExportMessage::Path(p) => {
                if let Some(path) = p {
                    self.path = Some(path);
                    self.start();
                } else {
                    return Task::perform(async {}, |_| {
                        app::message::Message::View(view::Message::Export(ExportMessage::Close))
                    });
                }
            }
            ExportMessage::Close | ExportMessage::Open => { /* unreachable */ }
        }
        Task::none()
    }
    pub fn view<'a>(&'a self, content: Element<'a, view::Message>) -> Element<view::Message> {
        let modal = Modal::new(
            content,
            export_modal(&self.state, self.error.as_ref(), "Transactions"),
        );
        match self.state {
            ExportState::TimedOut
            | ExportState::Aborted
            | ExportState::Ended
            | ExportState::Closed => modal.on_blur(Some(view::Message::Close)),
            _ => modal,
        }
        .into()
    }

    pub fn start(&mut self) {
        self.state = ExportState::Started;
    }

    pub fn stop(&mut self, state: ExportState) {
        if let Some(handle) = self.handle.take() {
            handle.lock().expect("poisoned").abort();
            self.state = state;
        }
    }

    pub fn subscription(&self) -> Option<Subscription<export::ExportProgress>> {
        if let Some(path) = &self.path {
            match &self.state {
                ExportState::Started | ExportState::Progress(_) => {
                    Some(iced::Subscription::run_with_id(
                        "transactions",
                        export::export_subscription(
                            self.daemon.clone(),
                            path.to_path_buf(),
                            ExportType::Transactions,
                        ),
                    ))
                }
                _ => None,
            }
        } else {
            None
        }
    }
}
