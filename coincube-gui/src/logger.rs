use std::{fs::File, sync::Arc};
use tracing_subscriber::{filter, fmt::writer::BoxMakeWriter, prelude::*, reload};

use crate::dir::CoincubeDirectory;

/// Folder under the data directory that holds the GUI's log files.
pub const LOGS_DIR: &str = "logs";

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

/// Name of the log file for a GUI run started at `started`.
///
/// Zero-padded year-first fields keep names sorting by start time, `-` stands
/// in for `:` (which NTFS rejects), and the `.log` extension is what
/// Diagnostics counts in [`LOGS_DIR`].
fn log_file_name<Tz: chrono::TimeZone>(started: chrono::DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    started.format("%Y-%m-%dT%H-%M-%S.log").to_string()
}

pub fn setup_logger(
    log_level: filter::LevelFilter,
    datadir: &CoincubeDirectory,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut log_path = datadir.path().to_path_buf();

    log_path.push(LOGS_DIR);
    std::fs::create_dir_all(&log_path)?;

    log_path.push(log_file_name(chrono::Local::now()));

    let file = File::create(log_path)?;
    let writer = BoxMakeWriter::new(Arc::new(file));

    let file_log = tracing_subscriber::fmt::layer()
        .with_writer(writer)
        .with_line_number(false)
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

    tracing_log::LogTracer::init()?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn log_file_name_is_valid_on_every_platform() {
        // A single-digit day and hour: the old `%v+%H:%M-%S` format gave
        // " 5-Nov-2026+04:03-02.log" here, with a leading space and two `:`.
        let started = chrono::Utc.with_ymd_and_hms(2026, 11, 5, 4, 3, 2).unwrap();
        let name = log_file_name(started);
        assert_eq!(name, "2026-11-05T04-03-02.log");
        assert!(
            !name.contains(['<', '>', ':', '"', '/', '\\', '|', '?', '*']),
            "{:?} holds a character Windows rejects in file names",
            name
        );
        assert!(!name.chars().any(|c| c.is_control() || c.is_whitespace()));
    }

    #[test]
    fn log_file_names_sort_by_start_time() {
        let times = [
            chrono::Utc.with_ymd_and_hms(2026, 9, 30, 23, 59, 59),
            chrono::Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0),
            chrono::Utc.with_ymd_and_hms(2026, 10, 1, 9, 0, 0),
            chrono::Utc.with_ymd_and_hms(2026, 10, 1, 10, 0, 0),
            chrono::Utc.with_ymd_and_hms(2027, 1, 1, 0, 0, 0),
        ];
        let names: Vec<String> = times.map(|t| log_file_name(t.unwrap())).to_vec();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
    }

    #[test]
    fn log_file_can_be_created_and_is_counted_as_a_log() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(log_file_name(chrono::Local::now()));
        File::create(&path).unwrap();
        assert_eq!(path.extension().and_then(|e| e.to_str()), Some("log"));
    }
}
