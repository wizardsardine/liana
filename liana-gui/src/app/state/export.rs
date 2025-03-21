use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use iced::{Subscription, Task};
use liana_ui::{component::modal::Modal, widget::Element};
use tokio::task::JoinHandle;

use crate::{
    app::view::{export::export_modal, Close},
    daemon::Daemon,
    export::{self, get_path, ImportExportMessage, ImportExportState, ImportExportType, Progress},
};

#[derive(Debug)]
pub struct ExportModal {
    path: Option<PathBuf>,
    handle: Option<Arc<Mutex<JoinHandle<()>>>>,
    state: ImportExportState,
    error: Option<export::Error>,
    daemon: Option<Arc<dyn Daemon + Sync + Send>>,
    import_export_type: ImportExportType,
}

impl ExportModal {
    #[allow(clippy::new_without_default)]
    pub fn new(
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        export_type: ImportExportType,
    ) -> Self {
        Self {
            path: None,
            handle: None,
            state: ImportExportState::Init,
            error: None,
            daemon,
            import_export_type: export_type,
        }
    }

    pub fn modal_title(&self) -> &'static str {
        match self.import_export_type {
            ImportExportType::Transactions => "Export Transactions",
            ImportExportType::ExportPsbt(_) => "Export PSBT",
            ImportExportType::ExportBackup(_) => "Export Backup",
            ImportExportType::Descriptor(_) => "Export Descriptor",
            ImportExportType::ExportProcessBackup(..) | ImportExportType::ExportLabels => {
                "Export Labels"
            }
            ImportExportType::ImportPsbt => "Import PSBT",
            ImportExportType::ImportDescriptor => "Import Descriptor",
            ImportExportType::ImportBackup(..) => "Restore Backup",
            ImportExportType::WalletFromBackup => "Import existing wallet from backup",
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
                    .expect("cannot fail")
                    .to_string();
                format!("liana-{}.descriptor", checksum)
            }
            ImportExportType::ImportPsbt => "psbt.psbt".into(),
            ImportExportType::ImportDescriptor => "descriptor.descriptor".into(),
            ImportExportType::ExportLabels => format!("liana-labels-{date}.jsonl"),
            ImportExportType::ExportBackup(_) | ImportExportType::ExportProcessBackup(..) => {
                format!("liana-backup-{date}.json")
            }
            ImportExportType::WalletFromBackup | ImportExportType::ImportBackup(_, _) => {
                "liana-backup.json".to_string()
            }
        }
    }

    pub fn launch<M: From<ImportExportMessage> + Send + 'static>(&self, write: bool) -> Task<M> {
        Task::perform(get_path(self.default_filename(), write), move |m| {
            ImportExportMessage::Path(m).into()
        })
    }

    pub fn update<M: From<ImportExportMessage> + Send + 'static>(
        &mut self,
        message: ImportExportMessage,
    ) -> Task<M> {
        match message {
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
                Progress::KeyAliasesConflict(ref sender) => {
                    if let ImportExportType::ImportBackup(_, None) = &self.import_export_type {
                        self.import_export_type =
                            ImportExportType::ImportBackup(None, Some(sender.clone()));
                    }
                }
                Progress::LabelsConflict(ref sender) => {
                    if let ImportExportType::ImportBackup(None, _) = &self.import_export_type {
                        self.import_export_type =
                            ImportExportType::ImportBackup(Some(sender.clone()), None);
                    }
                }
                Progress::Error(e) => {
                    self.error = Some(e.clone());
                }
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
                Progress::UpdateAliases(map) => {
                    return Task::perform(async {}, move |_| {
                        ImportExportMessage::UpdateAliases(map.clone()).into()
                    });
                }
                Progress::WalletFromBackup(_) => {}
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
                    return Task::perform(async {}, |_| ImportExportMessage::Close.into());
                }
            }
            ImportExportMessage::Close | ImportExportMessage::Open => { /* unreachable */ }
            ImportExportMessage::Overwrite => {
                if let ImportExportType::ImportBackup(labels, aliases) =
                    &mut self.import_export_type
                {
                    if let Some(sender) = labels.take() {
                        return Task::perform(
                            async move {
                                if sender.send(true).await.is_err() {
                                    tracing::error!(
                                        "ExportModal.update(): fail to send labels NACK"
                                    );
                                }
                            },
                            |_| ImportExportMessage::Ignore.into(),
                        );
                    } else if let Some(sender) = aliases.take() {
                        return Task::perform(
                            async move {
                                if sender.send(true).await.is_err() {
                                    tracing::error!(
                                        "ExportModal.update(): fail to send aliases NACK"
                                    );
                                }
                            },
                            |_| ImportExportMessage::Ignore.into(),
                        );
                    }
                }
            }
            ImportExportMessage::Ignore => {
                if let ImportExportType::ImportBackup(labels, aliases) =
                    &mut self.import_export_type
                {
                    if let Some(sender) = labels.take() {
                        return Task::perform(
                            async move {
                                if sender.send(false).await.is_err() {
                                    tracing::error!(
                                        "ExportModal.update(): fail to send labels NACK"
                                    );
                                }
                            },
                            |_| ImportExportMessage::Ignore.into(),
                        );
                    } else if let Some(sender) = aliases.take() {
                        return Task::perform(
                            async move {
                                if sender.send(false).await.is_err() {
                                    tracing::error!(
                                        "ExportModal.update(): fail to send aliases NACK"
                                    );
                                }
                            },
                            |_| ImportExportMessage::Ignore.into(),
                        );
                    }
                }
            }
            ImportExportMessage::UpdateAliases(_) => { /* unexpected */ }
        }
        Task::none()
    }

    pub fn view<'a, M>(&'a self, content: Element<'a, M>) -> Element<M>
    where
        M: 'a + Close + Clone + From<export::ImportExportMessage>,
    {
        let modal = Modal::new(
            content,
            export_modal(
                &self.state,
                self.error.as_ref(),
                self.modal_title(),
                &self.import_export_type,
            ),
        );
        match self.state {
            ImportExportState::TimedOut
            | ImportExportState::Aborted
            | ImportExportState::Ended
            | ImportExportState::Closed => modal.on_blur(Some(M::close())),
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
                        self.modal_title(),
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
