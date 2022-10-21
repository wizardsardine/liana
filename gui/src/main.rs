use std::{error::Error, path::PathBuf, str::FromStr};

use iced::pure::{Application, Element};
use iced::{executor, Command, Settings, Subscription};
extern crate serde;
extern crate serde_json;

use minisafe::{config::Config as DaemonConfig, miniscript::bitcoin};

use minisafe_gui::{
    app::{
        self,
        cache::Cache,
        config::{default_datadir, ConfigError},
        App,
    },
    installer::{self, Installer},
    loader::{self, Loader},
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

fn log_level_from_config(config: &app::Config) -> Result<log::LevelFilter, Box<dyn Error>> {
    if let Some(level) = &config.log_level {
        match level.as_ref() {
            "info" => Ok(log::LevelFilter::Info),
            "debug" => Ok(log::LevelFilter::Debug),
            "trace" => Ok(log::LevelFilter::Trace),
            _ => Err(format!("Unknown loglevel '{:?}'.", level).into()),
        }
    } else if let Some(true) = config.debug {
        Ok(log::LevelFilter::Debug)
    } else {
        Ok(log::LevelFilter::Info)
    }
}

pub struct GUI {
    state: State,
}

enum State {
    Installer(Box<Installer>),
    Loader(Box<Loader>),
    App(App),
}

#[derive(Debug)]
pub enum Message {
    CtrlC,
    Install(Box<installer::Message>),
    Load(Box<loader::Message>),
    Run(Box<app::Message>),
}

async fn ctrl_c() -> Result<(), ()> {
    if let Err(e) = tokio::signal::ctrl_c().await {
        log::error!("{}", e);
    };
    log::info!("Signal received, exiting");
    Ok(())
}

impl Application for GUI {
    type Executor = executor::Default;
    type Message = Message;
    type Flags = Config;

    fn title(&self) -> String {
        match self.state {
            State::Installer(_) => String::from("Minisafe Installer"),
            State::App(_) => String::from("Minisafe"),
            State::Loader(..) => String::from("Minisafe"),
        }
    }

    fn new(config: Config) -> (GUI, Command<Self::Message>) {
        match config {
            Config::Install(config_path, network) => {
                let (install, command) = Installer::new(config_path, network);
                (
                    Self {
                        state: State::Installer(Box::new(install)),
                    },
                    Command::batch(vec![
                        command.map(|msg| Message::Install(Box::new(msg))),
                        Command::perform(ctrl_c(), |_| Message::CtrlC),
                    ]),
                )
            }
            Config::Run(cfg) => {
                let daemon_cfg =
                    DaemonConfig::from_file(Some(cfg.minisafed_config_path.clone())).unwrap();
                let (loader, command) = Loader::new(cfg, daemon_cfg);
                (
                    Self {
                        state: State::Loader(Box::new(loader)),
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
            (State::Installer(i), Message::CtrlC) => {
                i.stop();
                Command::none()
            }
            (State::Loader(i), Message::CtrlC) => {
                i.stop();
                Command::none()
            }
            (State::App(i), Message::CtrlC) => {
                i.stop();
                Command::none()
            }
            (State::Installer(i), Message::Install(msg)) => {
                if let installer::Message::Exit(path) = *msg {
                    let cfg = app::Config::from_file(&path).unwrap();
                    let daemon_cfg =
                        DaemonConfig::from_file(Some(cfg.minisafed_config_path.clone())).unwrap();
                    let (loader, command) = Loader::new(cfg, daemon_cfg);
                    self.state = State::Loader(Box::new(loader));
                    command.map(|msg| Message::Load(Box::new(msg)))
                } else {
                    i.update(*msg).map(|msg| Message::Install(Box::new(msg)))
                }
            }
            (State::Loader(loader), Message::Load(msg)) => {
                if let loader::Message::Synced(info, coins, minisafed) = *msg {
                    let cache = Cache {
                        blockheight: info.blockheight,
                        coins,
                    };

                    let (app, command) = App::new(cache, loader.gui_config.clone(), minisafed);
                    self.state = State::App(app);
                    command.map(|msg| Message::Run(Box::new(msg)))
                } else {
                    loader.update(*msg).map(|msg| Message::Load(Box::new(msg)))
                }
            }
            (State::App(i), Message::Run(msg)) => {
                i.update(*msg).map(|msg| Message::Run(Box::new(msg)))
            }
            _ => Command::none(),
        }
    }

    fn should_exit(&self) -> bool {
        match &self.state {
            State::Installer(v) => v.should_exit(),
            State::Loader(v) => v.should_exit(),
            State::App(v) => v.should_exit(),
        }
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        match &self.state {
            State::Installer(v) => v.subscription().map(|msg| Message::Install(Box::new(msg))),
            State::Loader(v) => v.subscription().map(|msg| Message::Load(Box::new(msg))),
            State::App(v) => v.subscription().map(|msg| Message::Run(Box::new(msg))),
        }
    }

    fn view(&self) -> Element<Self::Message> {
        match &self.state {
            State::Installer(v) => v.view().map(|msg| Message::Install(Box::new(msg))),
            State::App(v) => v.view().map(|msg| Message::Run(Box::new(msg))),
            State::Loader(v) => v.view().map(|msg| Message::Load(Box::new(msg))),
        }
    }

    fn scale_factor(&self) -> f64 {
        1.0
    }
}

pub enum Config {
    Run(app::Config),
    Install(PathBuf, bitcoin::Network),
}

impl Config {
    pub fn new(datadir_path: PathBuf, network: bitcoin::Network) -> Result<Self, Box<dyn Error>> {
        let mut path = datadir_path.clone();
        path.push(network.to_string());
        path.push(app::config::DEFAULT_FILE_NAME);
        match app::Config::from_file(&path) {
            Ok(cfg) => Ok(Config::Run(cfg)),
            Err(ConfigError::NotFound) => Ok(Config::Install(datadir_path, network)),
            Err(e) => Err(format!("Failed to read configuration file: {}", e).into()),
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = parse_args(std::env::args().collect())?;
    let config = match args.as_slice() {
        [] => {
            let datadir_path = default_datadir().unwrap();
            Config::new(datadir_path, bitcoin::Network::Bitcoin)
        }
        [Arg::Network(network)] => {
            let datadir_path = default_datadir().unwrap();
            Config::new(datadir_path, *network)
        }
        [Arg::ConfigPath(path)] => Ok(Config::Run(app::Config::from_file(path)?)),
        [Arg::DatadirPath(datadir_path)] => {
            Config::new(datadir_path.clone(), bitcoin::Network::Bitcoin)
        }
        [Arg::DatadirPath(datadir_path), Arg::Network(network)]
        | [Arg::Network(network), Arg::DatadirPath(datadir_path)] => {
            Config::new(datadir_path.clone(), *network)
        }
        _ => {
            return Err("Unknown args combination".into());
        }
    }?;

    let level = if let Config::Run(cfg) = &config {
        log_level_from_config(cfg)?
    } else {
        log::LevelFilter::Info
    };
    setup_logger(level)?;

    let mut settings = Settings::with_flags(config);
    settings.exit_on_close_request = false;

    if let Err(e) = GUI::run(settings) {
        return Err(format!("Failed to launch UI: {}", e).into());
    };
    Ok(())
}

// This creates the log file automagically if it doesn't exist, and logs on stdout
// if None is given
pub fn setup_logger(log_level: log::LevelFilter) -> Result<(), fern::InitError> {
    let dispatcher = fern::Dispatch::new()
        .format(|out, message, record| {
            out.finish(format_args!(
                "[{}][{}][{}] {}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_else(|e| {
                        println!("Can't get time since epoch: '{}'. Using a dummy value.", e);
                        std::time::Duration::from_secs(0)
                    })
                    .as_secs(),
                record.target(),
                record.level(),
                message
            ))
        })
        .level(log_level)
        .level_for("iced_wgpu", log::LevelFilter::Off)
        .level_for("wgpu_core", log::LevelFilter::Off)
        .level_for("wgpu_hal", log::LevelFilter::Off)
        .level_for("gfx_backend_vulkan", log::LevelFilter::Off)
        .level_for("naga", log::LevelFilter::Off)
        .level_for("mio", log::LevelFilter::Off);

    dispatcher.chain(std::io::stdout()).apply()?;

    Ok(())
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
