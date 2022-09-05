pub mod client;
pub mod embedded;
pub mod model;

use std::fmt::Debug;
use std::io::ErrorKind;

use minisafe::config::Config;

#[derive(Debug, Clone)]
pub enum DaemonError {
    /// Something was wrong with the request.
    Rpc(i32, String),
    /// Something was wrong with the communication.
    Transport(Option<ErrorKind>, String),
    /// Something unexpected happened.
    Unexpected(String),
    /// No response.
    NoAnswer,
    // Error at start up.
    Start(String),
}

impl std::fmt::Display for DaemonError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Rpc(code, e) => write!(f, "Daemon error rpc call: [{:?}] {}", code, e),
            Self::NoAnswer => write!(f, "Daemon returned no answer"),
            Self::Transport(kind, e) => write!(f, "Daemon transport error: [{:?}] {}", kind, e),
            Self::Unexpected(e) => write!(f, "Daemon unexpected error: {}", e),
            Self::Start(e) => write!(f, "Daemon did not start: {}", e),
        }
    }
}

pub trait Daemon: Debug {
    fn is_external(&self) -> bool;

    fn load_config(&mut self, _cfg: Config) -> Result<(), DaemonError> {
        Ok(())
    }

    fn config(&self) -> &Config;

    fn stop(&mut self) -> Result<(), DaemonError>;

    fn get_info(&self) -> Result<model::GetInfoResult, DaemonError>;

    fn get_new_address(&self) -> Result<model::GetAddressResult, DaemonError>;

    fn list_coins(&self) -> Result<model::ListCoinsResult, DaemonError>;
}
