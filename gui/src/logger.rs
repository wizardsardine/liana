use liana::miniscript::bitcoin::Network;
use std::path::PathBuf;
use std::{fs::File, sync::Arc};
use tracing::{error, Event, Subscriber};
use tracing_subscriber::{
    filter,
    fmt::{format, writer::BoxMakeWriter, Layer},
    layer,
    prelude::*,
    reload, Registry,
};

use crossbeam_channel::unbounded;

const INSTALLER_LOG_FILE_NAME: &str = "installer.log";
const GUI_LOG_FILE_NAME: &str = "liana-gui.log";

#[derive(Debug)]
pub enum LoggerError {
    Io(std::io::Error),
    Reload(reload::Error),
}

impl From<std::io::Error> for LoggerError {
    fn from(e: std::io::Error) -> LoggerError {
        LoggerError::Io(e)
    }
}

impl From<reload::Error> for LoggerError {
    fn from(e: reload::Error) -> LoggerError {
        LoggerError::Reload(e)
    }
}

pub struct Logger {
    file_handle: reload::Handle<
        Layer<Registry, format::DefaultFields, format::Format, BoxMakeWriter>,
        Registry,
    >,
    level_handle: reload::Handle<filter::LevelFilter, Registry>,
    receiver: crossbeam_channel::Receiver<String>,
}

impl Logger {
    pub fn setup(log_level: filter::LevelFilter) -> Logger {
        let (log_level, level_handle) = reload::Layer::new(log_level);
        let writer = BoxMakeWriter::new(std::io::stderr);
        let file_log = tracing_subscriber::fmt::layer()
            .with_writer(writer)
            .with_file(false);
        let (file_log, file_handle) = reload::Layer::new(file_log);
        let stdout_log = tracing_subscriber::fmt::layer().pretty().with_file(false);
        let (sender, receiver) = unbounded::<String>();
        let streamer_log = LogStream { sender }.with_filter(filter::LevelFilter::INFO);
        tracing_subscriber::registry()
            .with(
                stdout_log
                    .and_then(file_log)
                    .with_filter(log_level)
                    // Add a filter to *both* layers that rejects spans and
                    // events whose targets start with `<prefix>`.
                    .with_filter(filter::filter_fn(|metadata| {
                        !metadata.target().starts_with("iced_wgpu")
                            && !metadata.target().starts_with("iced_winit")
                            && !metadata.target().starts_with("wgpu_core")
                            && !metadata.target().starts_with("wgpu_hal")
                            && !metadata.target().starts_with("gfx_backend_vulkan")
                            && !metadata.target().starts_with("iced_glutin")
                            && !metadata.target().starts_with("iced_glow")
                            && !metadata.target().starts_with("glow_glyph")
                            && !metadata.target().starts_with("naga")
                            && !metadata.target().starts_with("mio")
                            && !metadata.target().starts_with("ledger_transport_hid")
                    })),
            )
            .with(streamer_log)
            .init();
        Self {
            file_handle,
            level_handle,
            receiver,
        }
    }

    pub fn set_installer_mode(&self, mut datadir: PathBuf, log_level: filter::LevelFilter) {
        datadir.push(INSTALLER_LOG_FILE_NAME);
        if let Err(e) = self.set_layer(datadir, log_level) {
            error!("Failed to change logger settings: {:#?}", e);
        }
    }

    pub fn set_running_mode(
        &self,
        mut datadir: PathBuf,
        network: Network,
        log_level: filter::LevelFilter,
    ) {
        datadir.push(network.to_string());
        datadir.push(GUI_LOG_FILE_NAME);
        if let Err(e) = self.set_layer(datadir, log_level) {
            error!("Failed to change logger settings: {:#?}", e);
        }
    }

    pub fn remove_install_log_file(&self, mut datadir: PathBuf) {
        datadir.push(INSTALLER_LOG_FILE_NAME);
        if let Err(e) = std::fs::remove_file(&datadir) {
            error!(
                "Failed to remove installer log file {} error:{:#?}",
                datadir.to_string_lossy(),
                e
            );
        }
    }

    pub fn set_layer(
        &self,
        destination_path: PathBuf,
        log_level: filter::LevelFilter,
    ) -> Result<(), LoggerError> {
        let file = File::create(destination_path)?;
        self.file_handle
            .modify(|layer| *layer.writer_mut() = BoxMakeWriter::new(Arc::new(file)))?;
        self.level_handle.modify(|filter| *filter = log_level)?;
        Ok(())
    }

    pub fn receiver(&self) -> crossbeam_channel::Receiver<String> {
        self.receiver.clone()
    }
}

/// Used as a layer to send log messages to a channel.
pub struct LogStream {
    pub sender: crossbeam_channel::Sender<String>,
}

impl<S> layer::Layer<S> for LogStream
where
    S: Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    fn on_event(&self, event: &Event<'_>, _ctx: layer::Context<'_, S>) {
        let mut visitor = LogStreamVisitor {
            sender: &self.sender,
        };
        event.record(&mut visitor);
    }
}

/// Used to record log messages by sending them to the channel.
struct LogStreamVisitor<'a> {
    sender: &'a crossbeam_channel::Sender<String>,
}

impl<'a> tracing::field::Visit for LogStreamVisitor<'a> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let msg = format!("{:?}", value);
            let _ = self.sender.send(msg);
        }
    }
}
