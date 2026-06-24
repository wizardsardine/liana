use crate::state::message::Msg;
use iced::{
    widget::{column, Space},
    Alignment, Length,
};
use liana_ui::{component::text, theme, widget::*};

use super::layout;

/// Loading view shown during wallet opening transition.
/// When `has_error` is true, shows error msg with enabled Previous button.
pub fn loading_view(has_error: bool) -> Element<'static, Msg> {
    let (status_text, status_detail, previous_msg) = if has_error {
        (
            "Unable to load wallet",
            Some(("The service is temporarily unavailable. Your wallet data and funds are not affected.".to_string(),
                "Please try again shortly. If the issue persists, contact support.".to_string())),
            Some(Msg::BackToLogin),
        )
    } else {
        ("Loading wallet...", None, None)
    };

    layout(
        (0, 0),
        None,
        &[],
        Container::new(
            column![
                text::new::d3("Liana Business"),
                Space::with_height(30),
                text::new::caption(status_text).style(theme::text::secondary),
                status_detail
                    .as_ref()
                    .map(|d| text::new::caption(d.0.to_string()).style(theme::text::secondary)),
                status_detail.map(|d| text::new::caption(d.1).style(theme::text::secondary))
            ]
            .align_x(Alignment::Center)
            .width(Length::Fill),
        ),
        true,
        previous_msg,
    )
}
