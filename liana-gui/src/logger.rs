use liana::miniscript::bitcoin::Network;
use std::path::PathBuf;
use std::{fs::File, sync::Arc};
use tracing::error;
use tracing_subscriber::{
    filter,
    fmt::{format, writer::BoxMakeWriter, Layer},
    prelude::*,
    reload, Registry,
};

use crate::dir::LianaDirectory;

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
                            && !metadata.target().starts_with("winit")
                            && !metadata.target().starts_with("mio")
                            && !metadata.target().starts_with("ledger_transport_hid")
                            && !metadata.target().starts_with("cosmic_text")
                    })),
            )
            .init();
        Self {
            file_handle,
            level_handle,
        }
    }

    pub fn set_installer_mode(&self, datadir: LianaDirectory, log_level: filter::LevelFilter) {
        let mut datadir = datadir.path().to_path_buf();
        datadir.push(INSTALLER_LOG_FILE_NAME);
        if let Err(e) = self.set_layer(datadir, log_level) {
            error!("Failed to change logger settings: {:#?}", e);
        }
    }

    pub fn set_running_mode(
        &self,
        datadir: LianaDirectory,
        network: Network,
        log_level: filter::LevelFilter,
    ) {
        let mut datadir = datadir.path().to_path_buf();
        datadir.push(network.to_string());
        datadir.push(GUI_LOG_FILE_NAME);
        if let Err(e) = self.set_layer(datadir, log_level) {
            error!("Failed to change logger settings: {:#?}", e);
        }
    }

    pub fn remove_install_log_file(&self, datadir: LianaDirectory) {
        let mut datadir = datadir.path().to_path_buf();
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
}
