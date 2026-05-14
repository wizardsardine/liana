pub mod about;
pub mod backup;
pub mod general;
pub mod install_stats;
pub mod lightning;
pub mod recovery_kit;

use coincube_ui::{component::text::*, widget::*};

use crate::app::view::message::*;

/// Page heading shown at the top of each Cube → Settings sub-page.
/// Matches the plain `h3` heading style used on the Home / Wallets page
/// so Settings pages don't pick up extra button padding that made them
/// sit slightly lower than other sections. The rail already communicates
/// the Settings → {section} hierarchy, so no breadcrumb is needed here.
///
/// The `SettingsMessage` argument is kept for API compatibility with the
/// existing call sites but is no longer dispatched.
pub fn header<'a>(title: &'a str, _msg: SettingsMessage) -> Element<'a, Message> {
    h3(title).bold().into()
}
