use chrono::Local;
use colored::Colorize;

pub fn set_logger(verbose: bool) {
    fern::Dispatch::new()
        .format(move |out, message, record| {
            let color = match record.level() {
                log::Level::Error => "red",
                log::Level::Warn => "yellow",
                log::Level::Info => "green",
                log::Level::Debug => "blue",
                log::Level::Trace => "magenta",
            };

            let formatted = if verbose {
                format!(
                    "[{}][{}][{}] {}",
                    Local::now().format("%Y-%m-%d %H:%M:%S"),
                    record.target(),
                    record.level(),
                    message
                )
            } else {
                format!("[{}] {}", record.level(), message)
            };
            out.finish(format_args!("{}", formatted.color(color)))
        })
        .level(log::LevelFilter::Info)
        .level_for("bacca", log::LevelFilter::Debug)
        .level_for("ledger_transport_hidapi", log::LevelFilter::Error)
        .chain(std::io::stdout())
        .apply()
        .unwrap();
}