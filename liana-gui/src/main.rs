#![windows_subsystem = "windows"]

use std::{error::Error, io::Write};

use tracing::error;
extern crate serde;
extern crate serde_json;

use liana::miniscript::bitcoin::Network;
use liana_ui::theme;

use liana_gui::{
    args::{args_to_config, parse_args},
    dir::LianaDirectory,
    gui::LianaGUI,
    logger::parse_log_level,
    node::bitcoind::delete_all_bitcoind_locks_for_process,
    window::{create_app_settings, create_window_settings, load_initial_size},
    VERSION,
};

fn main() -> Result<(), Box<dyn Error>> {
    use Network::{Bitcoin, Regtest, Signet, Testnet4};

    let args = parse_args(
        std::env::args().collect(),
        VERSION,
        // NOTE: this is only related to networks we allow to pass as
        // a CLI arg, it does not handle here which network are selectable
        // in the GUI istself.
        &[Bitcoin, Testnet4, Signet, Regtest],
        None,
    )?;

    let config = args_to_config(&args, None, "Liana".into())?;
    let log_level = parse_log_level()?;

    setup_panic_hook(&config.liana_directory);

    let settings = create_app_settings("Liana");
    let initial_size = load_initial_size(&config.liana_directory, None);
    let window_settings = create_window_settings("Liana", initial_size);

    if let Err(e) = iced::application(LianaGUI::title, LianaGUI::update, LianaGUI::view)
        .theme(|_| theme::Theme::default())
        .scale_factor(LianaGUI::scale_factor)
        .subscription(LianaGUI::subscription)
        .settings(settings)
        .window(window_settings)
        .run_with(move || LianaGUI::new((config, log_level)))
    {
        log::error!("{}", e);
        Err(format!("Failed to launch UI: {}", e).into())
    } else {
        Ok(())
    }
}

// A panic in any thread should stop the main thread, and print the panic.
fn setup_panic_hook(liana_directory: &LianaDirectory) {
    let bitcoind_dir = liana_directory.bitcoind_directory();
    std::panic::set_hook(Box::new(move |panic_info| {
        error!("Panic occurred");
        if let Err(e) = delete_all_bitcoind_locks_for_process(bitcoind_dir.clone()) {
            error!("Failed to delete internal bitcoind locks: {}", e);
        }
        let file = panic_info
            .location()
            .map(|l| l.file())
            .unwrap_or_else(|| "'unknown'");
        let line = panic_info
            .location()
            .map(|l| l.line().to_string())
            .unwrap_or_else(|| "'unknown'".to_string());

        let bt = backtrace::Backtrace::new();
        let info = panic_info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| panic_info.payload().downcast_ref::<String>().cloned());
        error!(
            "panic occurred at line {} of file {}: {:?}\n{:?}",
            line, file, info, bt
        );

        std::io::stdout().flush().expect("Flushing stdout");
        std::process::exit(1);
    }));
}
