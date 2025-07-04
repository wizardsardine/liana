use std::{fs::File, sync::Arc};
use tracing_subscriber::{filter, fmt::writer::BoxMakeWriter, prelude::*, reload};

use crate::dir::LianaDirectory;

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

pub fn setup_logger(
    log_level: filter::LevelFilter,
    datadir: LianaDirectory,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut log_path = datadir.path().to_path_buf();
    log_path.push(GUI_LOG_FILE_NAME);

    let file = File::create(log_path)?;
    let writer = BoxMakeWriter::new(Arc::new(file));

    let file_log = tracing_subscriber::fmt::layer()
        .with_writer(writer)
        .with_file(false);

    let stdout_log = tracing_subscriber::fmt::layer().pretty().with_file(false);

    tracing_subscriber::registry()
        .with(
            stdout_log
                .and_then(file_log)
                .with_filter(log_level)
                // Add a filter to *both* layers that rejects spans and
                // events whose targets start with specific prefixes.
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

    Ok(())
}
