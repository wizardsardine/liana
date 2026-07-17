use crate::state::message::Msg;
use iced::{
    widget::{column, Space},
    Alignment, Length,
};
use liana_ui::{component::text, theme, widget::*};
use miniscript::bitcoin::Network;

use super::layout;

/// Loading view shown during wallet opening transition.
/// When `has_error` is true, shows error msg with enabled Previous button.
pub fn loading_view(network: Network, has_error: bool) -> Element<'static, Msg> {
    let (status_text, status_detail, previous_msg): (&str, Option<[&str; 2]>, Option<Msg>) =
        if has_error {
            (
            "Unable to load wallet",
            Some([
                "The service is temporarily unavailable. Your wallet data and funds are not affected.",
                "Please try again shortly. If the issue persists, contact support.",
            ]),
            Some(Msg::BackToLogin),
        )
        } else {
            ("Loading wallet...", None, None)
        };

    let content = Container::new(
        column![
            text::new::d3("Liana Business"),
            Space::with_height(30),
            text::new::caption(status_text).style(theme::text::secondary),
            status_detail
                .as_ref()
                .map(|details| text::new::caption(details[0]).style(theme::text::secondary)),
            status_detail
                .map(|details| text::new::caption(details[1]).style(theme::text::secondary))
        ]
        .align_x(Alignment::Center)
        .width(Length::Fill),
    );

    layout((0, 0), network, None, &[], content, previous_msg)
}
