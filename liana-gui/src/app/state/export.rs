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
    export::{self, get_path, ExportMessage, ExportProgress, ExportState},
};

#[derive(Debug)]
pub struct ExportModal {
    path: Option<PathBuf>,
    handle: Option<Arc<Mutex<JoinHandle<()>>>>,
    state: ExportState,
    error: Option<export::Error>,
    daemon: Arc<dyn Daemon + Sync + Send>,
}

impl ExportModal {
    #[allow(clippy::new_without_default)]
    pub fn new(daemon: Arc<dyn Daemon + Sync + Send>) -> Self {
        Self {
            path: None,
            handle: None,
            state: ExportState::Init,
            error: None,
            daemon,
        }
    }

    pub fn launch(&self) -> Task<Message> {
        Task::perform(get_path(), |m| {
            Message::View(view::Message::Export(ExportMessage::Path(m)))
        })
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        if let Message::View(view::Message::Export(m)) = message {
            match m {
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
                    ExportProgress::Finished | ExportProgress::Ended => {
                        self.state = ExportState::Ended
                    }
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
                            Message::View(view::Message::Export(ExportMessage::Close))
                        });
                    }
                }
                ExportMessage::Close | ExportMessage::Open => { /* unreachable */ }
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
                        export::export_subscription(self.daemon.clone(), path.to_path_buf()),
                    ))
                }
                _ => None,
            }
        } else {
            None
        }
    }
}
