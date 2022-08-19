use std::convert::From;

use iced::{
    pure::{column, container, row, widget},
    Length,
};

use crate::{
    app::error::Error,
    daemon::{client::error::RpcErrorCode, DaemonError},
    ui::{color, component::text::*, icon},
};

/// Simple warning message displayed to non technical user.
pub struct WarningMessage(String);

impl From<&Error> for WarningMessage {
    fn from(error: &Error) -> WarningMessage {
        match error {
            Error::Config(e) => WarningMessage(e.to_owned()),
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
                DaemonError::NoAnswer | DaemonError::Transport(..) => {
                    WarningMessage("Communication with Daemon failed".to_string())
                }
            },
            Error::Unexpected(_) => WarningMessage("Unknown error".to_string()),
        }
    }
}

impl std::fmt::Display for WarningMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

pub fn warn<'a, T: 'a>(error: Option<&Error>) -> widget::Container<'a, T> {
    if let Some(w) = error {
        let message: WarningMessage = w.into();
        warning(&message.to_string(), &w.to_string()).width(Length::Fill)
    } else {
        container(column()).width(Length::Fill)
    }
}

pub fn warning<'a, T: 'a>(message: &str, error: &str) -> widget::Container<'a, T> {
    container(
        widget::Tooltip::new(
            row()
                .push(icon::warning_icon())
                .push(text(message))
                .spacing(20),
            error,
            widget::tooltip::Position::Bottom,
        )
        .style(TooltipWarningStyle),
    )
    .padding(15)
    .center_x()
    .style(WarningStyle)
    .width(Length::Fill)
}

struct WarningStyle;
impl widget::container::StyleSheet for WarningStyle {
    fn style(&self) -> widget::container::Style {
        widget::container::Style {
            border_radius: 0.0,
            text_color: iced::Color::BLACK.into(),
            background: color::WARNING.into(),
            border_color: color::WARNING,
            ..widget::container::Style::default()
        }
    }
}

struct TooltipWarningStyle;
impl widget::container::StyleSheet for TooltipWarningStyle {
    fn style(&self) -> widget::container::Style {
        widget::container::Style {
            border_radius: 0.0,
            border_width: 1.0,
            text_color: color::WARNING.into(),
            background: color::FOREGROUND.into(),
            border_color: color::WARNING,
        }
    }
}
