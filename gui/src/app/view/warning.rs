use std::convert::From;

use iced::Length;

use liana_ui::{component::notification, widget::*};

use crate::{
    app::error::Error,
    daemon::{client::error::RpcErrorCode, DaemonError},
};

/// Simple warning message displayed to non technical user.
pub struct WarningMessage(String);

impl From<&Error> for WarningMessage {
    fn from(error: &Error) -> WarningMessage {
        match error {
            Error::Config(e) => WarningMessage(e.to_owned()),
            Error::Wallet(_) => WarningMessage("Wallet error".to_string()),
            Error::Daemon(e) => match e {
                DaemonError::Rpc(code, _) => {
                    if *code == RpcErrorCode::JSONRPC2_INVALID_PARAMS as i32 {
                        WarningMessage("Some fields are invalid".to_string())
                    } else {
                        WarningMessage("Internal error".to_string())
                    }
                }
                DaemonError::Unexpected(_) => WarningMessage("Unknown error".to_string()),
                DaemonError::Start(_) => WarningMessage("Daemon failed to start".to_string()),
                DaemonError::ClientNotSupported => {
                    WarningMessage("Daemon client is not supported".to_string())
                }
                DaemonError::NoAnswer | DaemonError::Transport(..) => {
                    WarningMessage("Communication with Daemon failed".to_string())
                }
                DaemonError::DaemonStopped => WarningMessage("Daemon stopped".to_string()),
            },
            Error::Unexpected(_) => WarningMessage("Unknown error".to_string()),
            Error::HardwareWallet(_) => WarningMessage("Hardware wallet error".to_string()),
        }
    }
}

impl std::fmt::Display for WarningMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

pub fn warn<'a, T: 'a + Clone>(error: Option<&Error>) -> Container<'a, T> {
    if let Some(w) = error {
        let message: WarningMessage = w.into();
        notification::warning(message.to_string(), w.to_string()).width(Length::Fill)
    } else {
        Container::new(Column::new()).width(Length::Fill)
    }
}
