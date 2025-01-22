use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use iced::{Subscription, Task};
use liana_ui::{component::modal::Modal, widget::Element};
use tokio::task::JoinHandle;

use crate::{
    app::{
        message::Message,
        view::{self, export::export_modal},
    },
    daemon::Daemon,
    export::{self, get_path, ImportExportMessage, ImportExportState, ImportExportType, Progress},
};

#[derive(Debug)]
pub struct ExportModal {
    path: Option<PathBuf>,
    handle: Option<Arc<Mutex<JoinHandle<()>>>>,
    state: ImportExportState,
    error: Option<export::Error>,
    daemon: Arc<dyn Daemon + Sync + Send>,
    import_export_type: ImportExportType,
}

impl ExportModal {
    #[allow(clippy::new_without_default)]
    pub fn new(daemon: Arc<dyn Daemon + Sync + Send>, export_type: ImportExportType) -> Self {
        Self {
            path: None,
            handle: None,
            state: ImportExportState::Init,
            error: None,
            daemon,
            import_export_type: export_type,
        }
    }

    pub fn default_filename(&self) -> String {
        let date = chrono::Local::now().format("%Y-%m-%dT%H-%M-%S");
        match &self.import_export_type {
            ImportExportType::Transactions => {
                format!("liana-txs-{date}.csv")
            }
            ImportExportType::ExportPsbt(_) => "psbt.psbt".into(),
            ImportExportType::Descriptor(descriptor) => {
                let checksum = descriptor
                    .to_string()
                    .split_once('#')
                    .map(|(_, checksum)| checksum)
                    .unwrap()
                    .to_string();
                format!("liana-{}.descriptor", checksum)
            }
            ImportExportType::ImportPsbt => "psbt.psbt".into(),
            ImportExportType::ImportDescriptor => "descriptor.descriptor".into(),
            ImportExportType::ExportLabels => format!("liana-labels-{date}.csv"),
        }
    }

    pub fn launch(&self) -> Task<Message> {
        Task::perform(get_path(self.default_filename()), |m| {
            Message::View(view::Message::ImportExport(ImportExportMessage::Path(m)))
        })
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        if let Message::View(view::Message::ImportExport(m)) = message {
            match m {
                ImportExportMessage::Progress(m) => match m {
                    Progress::Started(handle) => {
                        self.handle = Some(handle);
                        self.state = ImportExportState::Progress(0.0);
                    }
                    Progress::Progress(p) => {
                        if let ImportExportState::Progress(_) = self.state {
                            self.state = ImportExportState::Progress(p);
                        }
                    }
                    Progress::Finished | Progress::Ended => self.state = ImportExportState::Ended,
                    Progress::Error(e) => self.error = Some(e),
                    Progress::None => {}
                    Progress::Psbt(_) => {
                        if self.import_export_type == ImportExportType::ImportPsbt {
                            self.state = ImportExportState::Ended;
                        }
                        // TODO: forward PSBT
                    }
                    Progress::Descriptor(_) => {
                        if self.import_export_type == ImportExportType::ImportDescriptor {
                            self.state = ImportExportState::Ended;
                        }
                        // TODO: forward Descriptor
                    }
                },
                ImportExportMessage::TimedOut => {
                    self.stop(ImportExportState::TimedOut);
                }
                ImportExportMessage::UserStop => {
                    self.stop(ImportExportState::Aborted);
                }
                ImportExportMessage::Path(p) => {
                    if let Some(path) = p {
                        self.path = Some(path);
                        self.start();
                    } else {
                        return Task::perform(async {}, |_| {
                            Message::View(view::Message::ImportExport(ImportExportMessage::Close))
                        });
                    }
                }
                ImportExportMessage::Close | ImportExportMessage::Open => { /* unreachable */ }
            }
            Task::none()
        } else {
            Task::none()
        }
    }
    pub fn view<'a>(&'a self, content: Element<'a, view::Message>) -> Element<view::Message> {
        let modal = Modal::new(
            content,
            export_modal(&self.state, self.error.as_ref(), "Transactions"),
        );
        match self.state {
            ImportExportState::TimedOut
            | ImportExportState::Aborted
            | ImportExportState::Ended
            | ImportExportState::Closed => modal.on_blur(Some(view::Message::Close)),
            _ => modal,
        }
        .into()
    }

    pub fn start(&mut self) {
        self.state = ImportExportState::Started;
    }

    pub fn stop(&mut self, state: ImportExportState) {
        if let Some(handle) = self.handle.take() {
            handle.lock().expect("poisoned").abort();
            self.state = state;
        }
    }

    pub fn subscription(&self) -> Option<Subscription<export::Progress>> {
        if let Some(path) = &self.path {
            match &self.state {
                ImportExportState::Started | ImportExportState::Progress(_) => {
                    Some(iced::Subscription::run_with_id(
                        "transactions",
                        export::export_subscription(
                            self.daemon.clone(),
                            path.to_path_buf(),
                            self.import_export_type.clone(),
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
