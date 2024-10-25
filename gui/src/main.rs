#![windows_subsystem = "windows"]

use std::{
    collections::HashMap, error::Error, io::Write, path::PathBuf, process, str::FromStr, sync::Arc,
};

use iced::{
    event::{self, Event},
    executor, keyboard,
    widget::{focus_next, focus_previous},
    window::settings::PlatformSpecific,
    Application, Command, Settings, Size, Subscription,
};
use tracing::{error, info};
use tracing_subscriber::filter::LevelFilter;
extern crate serde;
extern crate serde_json;

use liana::{config::Config as DaemonConfig, miniscript::bitcoin};
use liana_ui::{component::text, font, image, theme, widget::Element};

use liana_gui::{
    app::{self, cache::Cache, config::default_datadir, wallet::Wallet, App},
    datadir,
    hw::HardwareWalletConfig,
    installer::{self, Installer},
    launcher::{self, Launcher},
    lianalite::{
        client::{backend::api, backend::BackendWalletClient},
        login,
    },
    loader::{self, Loader},
    logger::Logger,
    VERSION,
};

#[derive(Debug, PartialEq)]
enum Arg {
    ConfigPath(PathBuf),
    DatadirPath(PathBuf),
    Network(bitcoin::Network),
}

fn parse_args(args: Vec<String>) -> Result<Vec<Arg>, Box<dyn Error>> {
    let mut res = Vec::new();

    if args.len() > 1 && (args[1] == "--version" || args[1] == "-v") {
        eprintln!("{}", VERSION);
        process::exit(1);
    }

    if args.len() > 1 && (args[1] == "--help" || args[1] == "-h") {
        eprintln!(
            r#"
Usage: liana-gui [OPTIONS]

Options:
    --conf <PATH>       Path of configuration file (gui.toml)
    --datadir <PATH>    Path of liana datadir
    -v, --version       Display liana-gui version
    -h, --help          Print help
    --bitcoin           Use bitcoin network
    --testnet           Use testnet network
    --signet            Use signet network
    --regtest           Use regtest network
        "#
        );
        process::exit(1);
    }

    for (i, arg) in args.iter().enumerate() {
        if arg == "--conf" {
            if let Some(a) = args.get(i + 1) {
                res.push(Arg::ConfigPath(PathBuf::from(a)));
            } else {
                return Err("missing arg to --conf".into());
            }
        } else if arg == "--datadir" {
            if let Some(a) = args.get(i + 1) {
                res.push(Arg::DatadirPath(PathBuf::from(a)));
            } else {
                return Err("missing arg to --datadir".into());
            }
        } else if arg.contains("--") {
            let network = bitcoin::Network::from_str(args[i].trim_start_matches("--"))?;
            res.push(Arg::Network(network));
        }
    }

    Ok(res)
}

pub struct GUI {
    state: State,
    logger: Logger,
    // if set up, it overrides the level filter of the logger.
    log_level: Option<LevelFilter>,
}

enum State {
    Launcher(Box<Launcher>),
    Installer(Box<Installer>),
    Loader(Box<Loader>),
    Login(Box<login::LianaLiteLogin>),
    App(App),
}

#[derive(Debug)]
pub enum Key {
    Tab(bool),
}

#[derive(Debug)]
pub enum Message {
    CtrlC,
    FontLoaded(Result<(), iced::font::Error>),
    Launch(Box<launcher::Message>),
    Install(Box<installer::Message>),
    Load(Box<loader::Message>),
    Run(Box<app::Message>),
    Login(Box<login::Message>),
    KeyPressed(Key),
    Event(iced::Event),
}

impl From<Result<(), iced::font::Error>> for Message {
    fn from(value: Result<(), iced::font::Error>) -> Self {
        Self::FontLoaded(value)
    }
}

async fn ctrl_c() -> Result<(), ()> {
    if let Err(e) = tokio::signal::ctrl_c().await {
        error!("{}", e);
    };
    info!("Signal received, exiting");
    Ok(())
}

impl Application for GUI {
    type Executor = executor::Default;
    type Message = Message;
    type Flags = (Config, Option<LevelFilter>);
    type Theme = theme::Theme;

    fn title(&self) -> String {
        match self.state {
            State::Installer(_) => format!("Liana v{} Installer", VERSION),
            _ => format!("Liana v{}", VERSION),
        }
    }

    fn new((config, log_level): (Config, Option<LevelFilter>)) -> (GUI, Command<Self::Message>) {
        let logger = Logger::setup(log_level.unwrap_or(LevelFilter::INFO));
        let mut cmds = font::loads();
        cmds.push(Command::perform(ctrl_c(), |_| Message::CtrlC));
        let state = match config {
            Config::Launcher(datadir_path) => {
                let (launcher, command) = Launcher::new(datadir_path, None);
                cmds.push(command.map(|msg| Message::Launch(Box::new(msg))));
                State::Launcher(Box::new(launcher))
            }
            Config::Run(datadir_path, cfg, network) => {
                logger.set_running_mode(
                    datadir_path.clone(),
                    network,
                    log_level.unwrap_or_else(|| cfg.log_level().unwrap_or(LevelFilter::INFO)),
                );
                let (loader, command) = Loader::new(datadir_path, cfg, network, None);
                cmds.push(command.map(|msg| Message::Load(Box::new(msg))));
                State::Loader(Box::new(loader))
            }
        };
        (
            Self {
                state,
                logger,
                log_level,
            },
            Command::batch(cmds),
        )
    }

    fn update(&mut self, message: Self::Message) -> Command<Self::Message> {
        match (&mut self.state, message) {
            (_, Message::CtrlC)
            | (_, Message::Event(iced::Event::Window(_, iced::window::Event::CloseRequested))) => {
                match &mut self.state {
                    State::Loader(s) => s.stop(),
                    State::Launcher(s) => s.stop(),
                    State::Installer(s) => s.stop(),
                    State::App(s) => s.stop(),
                    State::Login(_) => {}
                };
                iced::window::close(iced::window::Id::MAIN)
            }
            (_, Message::KeyPressed(Key::Tab(shift))) => {
                log::debug!("Tab pressed!");
                if shift {
                    focus_previous()
                } else {
                    focus_next()
                }
            }
            (State::Launcher(l), Message::Launch(msg)) => match *msg {
                launcher::Message::Install(datadir_path, network, init) => {
                    if !datadir_path.exists() {
                        // datadir is created right before launching the installer
                        // so logs can go in <datadir_path>/installer.log
                        if let Err(e) = datadir::create_directory(&datadir_path) {
                            error!("Failed to create datadir: {}", e);
                        } else {
                            info!(
                                "Created a fresh data directory at {}",
                                &datadir_path.to_string_lossy()
                            );
                        }
                    }
                    self.logger.set_installer_mode(
                        datadir_path.clone(),
                        self.log_level.unwrap_or(LevelFilter::INFO),
                    );

                    let (install, command) = Installer::new(datadir_path, network, None, init);
                    self.state = State::Installer(Box::new(install));
                    command.map(|msg| Message::Install(Box::new(msg)))
                }
                launcher::Message::Run(datadir_path, cfg, network) => {
                    self.logger.set_running_mode(
                        datadir_path.clone(),
                        network,
                        self.log_level
                            .unwrap_or_else(|| cfg.log_level().unwrap_or(LevelFilter::INFO)),
                    );
                    if let Ok(settings) =
                        app::settings::Settings::from_file(datadir_path.clone(), network)
                    {
                        if settings
                            .wallets
                            .first()
                            .map(|w| w.remote_backend_auth.is_some())
                            == Some(true)
                        {
                            let (login, command) =
                                login::LianaLiteLogin::new(datadir_path, network, settings);
                            self.state = State::Login(Box::new(login));
                            command.map(|msg| Message::Login(Box::new(msg)))
                        } else {
                            let (loader, command) = Loader::new(datadir_path, cfg, network, None);
                            self.state = State::Loader(Box::new(loader));
                            command.map(|msg| Message::Load(Box::new(msg)))
                        }
                    } else {
                        let (loader, command) = Loader::new(datadir_path, cfg, network, None);
                        self.state = State::Loader(Box::new(loader));
                        command.map(|msg| Message::Load(Box::new(msg)))
                    }
                }
                _ => l.update(*msg).map(|msg| Message::Launch(Box::new(msg))),
            },
            (State::Login(l), Message::Login(msg)) => match *msg {
                login::Message::View(login::ViewMessage::BackToLauncher(network)) => {
                    let (launcher, command) = Launcher::new(l.datadir.clone(), Some(network));
                    self.state = State::Launcher(Box::new(launcher));
                    command.map(|msg| Message::Launch(Box::new(msg)))
                }
                login::Message::Install(remote_backend) => {
                    let (install, command) = Installer::new(
                        l.datadir.clone(),
                        l.network,
                        remote_backend,
                        installer::UserFlow::CreateWallet,
                    );
                    self.state = State::Installer(Box::new(install));
                    command.map(|msg| Message::Install(Box::new(msg)))
                }
                login::Message::Run(Ok((backend_client, wallet))) => {
                    let config = app::Config::from_file(
                        &l.datadir
                            .join(l.network.to_string())
                            .join(app::config::DEFAULT_FILE_NAME),
                    )
                    .expect("A gui configuration file must be present");
                    self.logger.set_running_mode(
                        l.datadir.clone(),
                        l.network,
                        config.log_level().unwrap_or(LevelFilter::INFO),
                    );

                    let (app, command) = create_app_with_remote_backend(
                        backend_client,
                        wallet,
                        l.datadir.clone(),
                        l.network,
                        config,
                    );

                    self.state = State::App(app);
                    command.map(|msg| Message::Run(Box::new(msg)))
                }
                _ => l.update(*msg).map(|msg| Message::Login(Box::new(msg))),
            },
            (State::Installer(i), Message::Install(msg)) => {
                if let installer::Message::Exit(path, internal_bitcoind) = *msg {
                    let settings = app::settings::Settings::from_file(i.datadir.clone(), i.network)
                        .expect("A settings file was created");
                    if settings
                        .wallets
                        .first()
                        .map(|w| w.remote_backend_auth.is_some())
                        == Some(true)
                    {
                        let (login, command) =
                            login::LianaLiteLogin::new(i.datadir.clone(), i.network, settings);
                        self.state = State::Login(Box::new(login));
                        command.map(|msg| Message::Login(Box::new(msg)))
                    } else {
                        let cfg = app::Config::from_file(&path).expect("A config file was created");
                        let daemon_cfg =
                            DaemonConfig::from_file(cfg.daemon_config_path.clone()).unwrap();
                        let datadir_path = daemon_cfg
                            .data_dir
                            .as_ref()
                            .expect("Installer must have set it")
                            .clone();

                        self.logger.set_running_mode(
                            datadir_path.clone(),
                            daemon_cfg.bitcoin_config.network,
                            self.log_level
                                .unwrap_or_else(|| cfg.log_level().unwrap_or(LevelFilter::INFO)),
                        );
                        self.logger.remove_install_log_file(datadir_path.clone());
                        let (loader, command) = Loader::new(
                            datadir_path,
                            cfg,
                            daemon_cfg.bitcoin_config.network,
                            internal_bitcoind,
                        );
                        self.state = State::Loader(Box::new(loader));
                        command.map(|msg| Message::Load(Box::new(msg)))
                    }
                } else if let installer::Message::BackToLauncher(network) = *msg {
                    let (launcher, command) = Launcher::new(i.destination_path(), Some(network));
                    self.state = State::Launcher(Box::new(launcher));
                    command.map(|msg| Message::Launch(Box::new(msg)))
                } else {
                    i.update(*msg).map(|msg| Message::Install(Box::new(msg)))
                }
            }
            (State::Loader(loader), Message::Load(msg)) => match *msg {
                loader::Message::View(loader::ViewMessage::SwitchNetwork) => {
                    let (launcher, command) =
                        Launcher::new(loader.datadir_path.clone(), Some(loader.network));
                    self.state = State::Launcher(Box::new(launcher));
                    command.map(|msg| Message::Launch(Box::new(msg)))
                }
                loader::Message::Synced(Ok((wallet, cache, daemon, bitcoind))) => {
                    let (app, command) = App::new(
                        cache,
                        wallet,
                        loader.gui_config.clone(),
                        daemon,
                        loader.datadir_path.clone(),
                        bitcoind,
                    );
                    self.state = State::App(app);
                    command.map(|msg| Message::Run(Box::new(msg)))
                }
                _ => loader.update(*msg).map(|msg| Message::Load(Box::new(msg))),
            },
            (State::App(i), Message::Run(msg)) => {
                i.update(*msg).map(|msg| Message::Run(Box::new(msg)))
            }
            _ => Command::none(),
        }
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        Subscription::batch(vec![
            match &self.state {
                State::Installer(v) => v.subscription().map(|msg| Message::Install(Box::new(msg))),
                State::Loader(v) => v.subscription().map(|msg| Message::Load(Box::new(msg))),
                State::App(v) => v.subscription().map(|msg| Message::Run(Box::new(msg))),
                State::Launcher(v) => v.subscription().map(|msg| Message::Launch(Box::new(msg))),
                State::Login(_) => Subscription::none(),
            },
            iced::event::listen_with(|event, status| match (&event, status) {
                (
                    Event::Keyboard(keyboard::Event::KeyPressed {
                        key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Tab),
                        modifiers,
                        ..
                    }),
                    event::Status::Ignored,
                ) => Some(Message::KeyPressed(Key::Tab(modifiers.shift()))),
                (
                    iced::Event::Window(_, iced::window::Event::CloseRequested),
                    event::Status::Ignored,
                ) => Some(Message::Event(event)),
                _ => None,
            }),
        ])
        .with_filter(|event| {
            matches!(
                event,
                iced::Event::Window(_, iced::window::Event::CloseRequested)
                    | iced::Event::Keyboard(_)
            )
        })
    }

    fn view(&self) -> Element<Self::Message> {
        match &self.state {
            State::Installer(v) => v.view().map(|msg| Message::Install(Box::new(msg))),
            State::App(v) => v.view().map(|msg| Message::Run(Box::new(msg))),
            State::Launcher(v) => v.view().map(|msg| Message::Launch(Box::new(msg))),
            State::Loader(v) => v.view().map(|msg| Message::Load(Box::new(msg))),
            State::Login(v) => v.view().map(|msg| Message::Login(Box::new(msg))),
        }
    }

    fn scale_factor(&self) -> f64 {
        1.0
    }
}

pub fn create_app_with_remote_backend(
    remote_backend: BackendWalletClient,
    wallet: api::Wallet,
    datadir: PathBuf,
    network: bitcoin::Network,
    config: app::Config,
) -> (app::App, iced::Command<app::Message>) {
    let hws: Vec<HardwareWalletConfig> = wallet
        .metadata
        .ledger_hmacs
        .into_iter()
        .map(|ledger_hmac| HardwareWalletConfig {
            kind: async_hwi::DeviceKind::Ledger.to_string(),
            fingerprint: ledger_hmac.fingerprint,
            token: ledger_hmac.hmac,
        })
        .collect();
    let aliases: HashMap<bitcoin::bip32::Fingerprint, String> = wallet
        .metadata
        .fingerprint_aliases
        .into_iter()
        .filter_map(|a| {
            if a.user_id == remote_backend.user_id() {
                Some((a.fingerprint, a.alias))
            } else {
                None
            }
        })
        .collect();
    App::new(
        Cache {
            network,
            coins: Vec::new(),
            rescan_progress: None,
            datadir_path: datadir.clone(),
            blockheight: wallet.tip_height.unwrap_or(0),
            last_poll_timestamp: None, // We ignore this field for remote backend.
        },
        Arc::new(
            Wallet::new(wallet.descriptor)
                .with_name(wallet.name)
                .with_key_aliases(aliases)
                .with_hardware_wallets(hws)
                .load_hotsigners(&datadir, network)
                .expect("Datadir should be conform"),
        ),
        config,
        Arc::new(remote_backend),
        datadir,
        None,
    )
}

pub enum Config {
    Run(PathBuf, app::Config, bitcoin::Network),
    Launcher(PathBuf),
}

impl Config {
    pub fn new(
        datadir_path: PathBuf,
        network: Option<bitcoin::Network>,
    ) -> Result<Self, Box<dyn Error>> {
        if let Some(network) = network {
            let mut path = datadir_path.clone();
            path.push(network.to_string());
            path.push(app::config::DEFAULT_FILE_NAME);
            match app::Config::from_file(&path) {
                Ok(cfg) => Ok(Config::Run(datadir_path, cfg, network)),
                Err(e) => Err(format!("Failed to read configuration file: {}", e).into()),
            }
        } else {
            Ok(Config::Launcher(datadir_path))
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = parse_args(std::env::args().collect())?;
    let config = match args.as_slice() {
        [] => {
            let datadir_path = default_datadir().unwrap();
            Config::new(datadir_path, None)
        }
        [Arg::Network(network)] => {
            let datadir_path = default_datadir().unwrap();
            Config::new(datadir_path, Some(*network))
        }
        [Arg::ConfigPath(path)] => {
            let cfg = app::Config::from_file(path)?;
            if let Some(daemon_config_path) = cfg.daemon_config_path.clone() {
                let daemon_cfg = DaemonConfig::from_file(Some(daemon_config_path))?;
                let datadir_path = daemon_cfg
                    .data_dir
                    .unwrap_or_else(|| default_datadir().unwrap());
                Ok(Config::Run(
                    datadir_path,
                    cfg,
                    daemon_cfg.bitcoin_config.network,
                ))
            } else {
                Err("Application cannot guess network".into())
            }
        }
        [Arg::ConfigPath(path), Arg::Network(network)]
        | [Arg::Network(network), Arg::ConfigPath(path)] => {
            let cfg = app::Config::from_file(path)?;
            if let Some(daemon_config_path) = cfg.daemon_config_path.clone() {
                let daemon_cfg = DaemonConfig::from_file(Some(daemon_config_path))?;
                let datadir_path = daemon_cfg
                    .data_dir
                    .unwrap_or_else(|| default_datadir().unwrap());
                Ok(Config::Run(
                    datadir_path,
                    cfg,
                    daemon_cfg.bitcoin_config.network,
                ))
            } else {
                Ok(Config::Run(default_datadir().unwrap(), cfg, *network))
            }
        }
        [Arg::DatadirPath(datadir_path)] => Config::new(datadir_path.clone(), None),
        [Arg::DatadirPath(datadir_path), Arg::Network(network)]
        | [Arg::Network(network), Arg::DatadirPath(datadir_path)] => {
            Config::new(datadir_path.clone(), Some(*network))
        }
        _ => {
            return Err("Unknown args combination".into());
        }
    }?;

    let log_level = if let Ok(l) = std::env::var("LOG_LEVEL") {
        Some(LevelFilter::from_str(&l)?)
    } else {
        None
    };

    setup_panic_hook();

    let mut settings = Settings::with_flags((config, log_level));
    settings.window.icon = Some(image::liana_app_icon());
    settings.window.min_size = Some(Size {
        width: 1000.0,
        height: 650.0,
    });
    settings.default_text_size = text::P1_SIZE.into();
    settings.default_font = liana_ui::font::REGULAR;
    settings.window.exit_on_close_request = false;

    settings.id = Some("Liana".to_string());

    #[cfg(target_os = "linux")]
    {
        settings.window.platform_specific = PlatformSpecific {
            application_id: "Liana".to_string(),
        };
    }

    if let Err(e) = GUI::run(settings) {
        return Err(format!("Failed to launch UI: {}", e).into());
    };
    Ok(())
}

// A panic in any thread should stop the main thread, and print the panic.
fn setup_panic_hook() {
    std::panic::set_hook(Box::new(move |panic_info| {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_args() {
        assert!(parse_args(vec!["--meth".into()]).is_err());
        assert!(parse_args(vec!["--datadir".into()]).is_err());
        assert!(parse_args(vec!["--conf".into()]).is_err());
        assert_eq!(
            Some(vec![
                Arg::DatadirPath(PathBuf::from(".")),
                Arg::ConfigPath(PathBuf::from("hello.toml")),
            ]),
            parse_args(
                "--datadir . --conf hello.toml"
                    .split(' ')
                    .map(|a| a.to_string())
                    .collect()
            )
            .ok()
        );
        assert_eq!(
            Some(vec![Arg::Network(bitcoin::Network::Regtest)]),
            parse_args(vec!["--regtest".into()]).ok()
        );
        assert_eq!(
            Some(vec![
                Arg::DatadirPath(PathBuf::from("hello")),
                Arg::Network(bitcoin::Network::Testnet)
            ]),
            parse_args(
                "--datadir hello --testnet"
                    .split(' ')
                    .map(|a| a.to_string())
                    .collect()
            )
            .ok()
        );
        assert_eq!(
            Some(vec![
                Arg::Network(bitcoin::Network::Testnet),
                Arg::DatadirPath(PathBuf::from("hello"))
            ]),
            parse_args(
                "--testnet --datadir hello"
                    .split(' ')
                    .map(|a| a.to_string())
                    .collect()
            )
            .ok()
        );
    }
}
