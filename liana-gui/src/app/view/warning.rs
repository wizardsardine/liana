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
            Error::Wallet(_) => WarningMessage(crate::t!("warning-wallet-error")),
            Error::Daemon(e) => match e {
                DaemonError::Rpc(code, _) => {
                    if *code == RpcErrorCode::JSONRPC2_INVALID_PARAMS as i32 {
                        WarningMessage(crate::t!("warning-fields-invalid"))
                    } else {
                        WarningMessage(crate::t!("warning-internal-error"))
                    }
                }
                DaemonError::Http(Some(code), error) => WarningMessage(crate::t!(
                    "warning-http-code-error",
                    code = code,
                    error = error
                )),
                DaemonError::Http(None, error) => {
                    WarningMessage(crate::t!("warning-http-error", error = error))
                }
                DaemonError::Unexpected(_) => WarningMessage(crate::t!("error-unknown")),
                DaemonError::Start(_) => WarningMessage(crate::t!("warning-daemon-start-failed")),
                DaemonError::ClientNotSupported => {
                    WarningMessage(crate::t!("warning-daemon-client-unsupported"))
                }
                DaemonError::NoAnswer | DaemonError::RpcSocket(..) => {
                    WarningMessage(crate::t!("warning-daemon-communication-failed"))
                }
                DaemonError::DaemonStopped => WarningMessage(crate::t!("warning-daemon-stopped")),
                DaemonError::CoinSelectionError => {
                    WarningMessage(crate::t!("warning-coin-selection-error"))
                }
                DaemonError::NotImplemented => {
                    WarningMessage(crate::t!("warning-backend-feature-unimplemented"))
                }
            },
            Error::Unexpected(_) => WarningMessage(crate::t!("error-unknown")),
            Error::HardwareWallet(_) => WarningMessage(crate::t!("warning-hardware-wallet-error")),
            Error::Desc(e) => WarningMessage(crate::t!(
                "warning-descriptor-analysis-error",
                error = e.to_string()
            )),
            Error::Spend(e) => WarningMessage(crate::t!(
                "warning-spend-creation-error",
                error = e.to_string()
            )),
            Error::ImportExport(e) => WarningMessage(format!("{e}")),
            Error::RestoreBackup(e) => WarningMessage(crate::t!(
                "warning-restore-backup-failed",
                error = e.to_string()
            )),
            Error::FiatPrice(e) => {
                WarningMessage(crate::t!("warning-fiat-price-error", error = e.to_string()))
            }
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
