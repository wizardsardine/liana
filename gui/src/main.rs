#![windows_subsystem = "windows"]

use std::{error::Error, io::Write, path::PathBuf, str::FromStr};

use iced::{executor, Application, Command, Element, Settings, Subscription};
use tracing::{error, info};
use tracing_subscriber::filter::LevelFilter;
extern crate serde;
extern crate serde_json;

use liana::{config::Config as DaemonConfig, miniscript::bitcoin};

use liana_gui::{
    app::{
        self,
        config::{default_datadir, ConfigError},
        App,
    },
    installer::{self, Installer},
    launcher::{self, Launcher},
    loader::{self, Loader},
    logger::Logger,
};

#[derive(Debug, PartialEq)]
enum Arg {
    ConfigPath(PathBuf),
    DatadirPath(PathBuf),
    Network(bitcoin::Network),
}

fn parse_args(args: Vec<String>) -> Result<Vec<Arg>, Box<dyn Error>> {
    let mut res = Vec::new();
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
}

enum State {
    Launcher(Box<Launcher>),
    Installer(Box<Installer>),
    Loader(Box<Loader>),
    App(App),
}

#[derive(Debug)]
pub enum Message {
    CtrlC,
    Launch(Box<launcher::Message>),
    Install(Box<installer::Message>),
    Load(Box<loader::Message>),
    Run(Box<app::Message>),
    Event(iced_native::Event),
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
    type Flags = Config;
    type Theme = iced::Theme;

    fn title(&self) -> String {
        match self.state {
            State::Installer(_) => String::from("Liana Installer"),
            _ => String::from("Liana"),
        }
    }

    fn new(config: Config) -> (GUI, Command<Self::Message>) {
        let logger = Logger::setup(LevelFilter::INFO);
        match config {
            Config::Launcher(datadir_path) => {
                let launcher = Launcher::new(datadir_path);
                (
                    Self {
                        state: State::Launcher(Box::new(launcher)),
                        logger,
                    },
                    Command::perform(ctrl_c(), |_| Message::CtrlC),
                )
            }
            Config::Install(datadir_path, network) => {
                logger.set_installer_mode(datadir_path.clone(), LevelFilter::INFO);
                let (install, command) = Installer::new(datadir_path, network);
                (
                    Self {
                        state: State::Installer(Box::new(install)),
                        logger,
                    },
                    Command::batch(vec![
                        command.map(|msg| Message::Install(Box::new(msg))),
                        Command::perform(ctrl_c(), |_| Message::CtrlC),
                    ]),
                )
            }
            Config::Run(datadir_path, cfg, network) => {
                logger.set_running_mode(
                    datadir_path.clone(),
                    network,
                    cfg.log_level().unwrap_or(LevelFilter::INFO),
                );
                let (loader, command) = Loader::new(datadir_path, cfg, network);
                (
                    Self {
                        state: State::Loader(Box::new(loader)),
                        logger,
                    },
                    Command::batch(vec![
                        command.map(|msg| Message::Load(Box::new(msg))),
                        Command::perform(ctrl_c(), |_| Message::CtrlC),
                    ]),
                )
            }
        }
    }

    fn update(&mut self, message: Self::Message) -> Command<Self::Message> {
        match (&mut self.state, message) {
            (_, Message::CtrlC)
            | (
                _,
                Message::Event(iced_native::Event::Window(
                    iced_native::window::Event::CloseRequested,
                )),
            ) => {
                match &mut self.state {
                    State::Loader(s) => s.stop(),
                    State::Launcher(s) => s.stop(),
                    State::Installer(s) => s.stop(),
                    State::App(s) => s.stop(),
                };
                iced::window::close()
            }
            (State::Launcher(l), Message::Launch(msg)) => match *msg {
                launcher::Message::Install(datadir_path) => {
                    self.logger
                        .set_installer_mode(datadir_path.clone(), LevelFilter::INFO);
                    let (install, command) =
                        Installer::new(datadir_path, bitcoin::Network::Bitcoin);
                    self.state = State::Installer(Box::new(install));
                    command.map(|msg| Message::Install(Box::new(msg)))
                }
                launcher::Message::Run(datadir_path, cfg, network) => {
                    self.logger.set_running_mode(
                        datadir_path.clone(),
                        network,
                        cfg.log_level().unwrap_or(LevelFilter::INFO),
                    );
                    let (loader, command) = Loader::new(datadir_path, cfg, network);
                    self.state = State::Loader(Box::new(loader));
                    command.map(|msg| Message::Load(Box::new(msg)))
                }
                _ => l.update(*msg).map(|msg| Message::Launch(Box::new(msg))),
            },
            (State::Installer(i), Message::Install(msg)) => {
                if let installer::Message::Exit(path) = *msg {
                    let cfg = app::Config::from_file(&path).unwrap();
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
                        cfg.log_level().unwrap_or(LevelFilter::INFO),
                    );
                    self.logger.remove_install_log_file(datadir_path.clone());
                    let (loader, command) =
                        Loader::new(datadir_path, cfg, daemon_cfg.bitcoin_config.network);
                    self.state = State::Loader(Box::new(loader));
                    command.map(|msg| Message::Load(Box::new(msg)))
                } else {
                    i.update(*msg).map(|msg| Message::Install(Box::new(msg)))
                }
            }
            (State::Loader(loader), Message::Load(msg)) => match *msg {
                loader::Message::View(loader::ViewMessage::SwitchNetwork) => {
                    self.state =
                        State::Launcher(Box::new(Launcher::new(loader.datadir_path.clone())));
                    Command::none()
                }
                loader::Message::Synced(Ok((wallet, cache, daemon))) => {
                    let (app, command) = App::new(cache, wallet, loader.gui_config.clone(), daemon);
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
            },
            iced_native::subscription::events().map(Self::Message::Event),
        ])
    }

    fn view(&self) -> Element<Self::Message> {
        match &self.state {
            State::Installer(v) => v.view().map(|msg| Message::Install(Box::new(msg))),
            State::App(v) => v.view().map(|msg| Message::Run(Box::new(msg))),
            State::Launcher(v) => v.view().map(|msg| Message::Launch(Box::new(msg))),
            State::Loader(v) => v.view().map(|msg| Message::Load(Box::new(msg))),
        }
    }

    fn scale_factor(&self) -> f64 {
        1.0
    }
}

pub enum Config {
    Run(PathBuf, app::Config, bitcoin::Network),
    Launcher(PathBuf),
    Install(PathBuf, bitcoin::Network),
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
                Err(ConfigError::NotFound) => Ok(Config::Install(datadir_path, network)),
                Err(e) => Err(format!("Failed to read configuration file: {}", e).into()),
            }
        } else if !datadir_path.exists() {
            Ok(Config::Install(datadir_path, bitcoin::Network::Bitcoin))
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

    setup_panic_hook();

    let mut settings = Settings::with_flags(config);
    settings.exit_on_close_request = false;

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
