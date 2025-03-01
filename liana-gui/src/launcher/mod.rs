pub(super) mod prelude {
    pub(super) use std::path::PathBuf;

    pub(super) use iced::{
        alignment::Horizontal,
        widget::{pick_list, scrollable, Button, Space},
        Alignment, Length, Subscription, Task,
    };

    pub(super) use liana::miniscript::bitcoin::Network;
    pub(super) use liana_ui::{
        component::{button, card, modal::Modal, network_banner, notification, text::*},
        icon, image, theme,
        widget::*,
    };
    pub(super) use lianad::config::ConfigError;

    pub(super) use crate::{app, installer::UserFlow};

    pub(super) const NETWORKS: [Network; 4] = [
        Network::Bitcoin,
        Network::Testnet,
        Network::Signet,
        Network::Regtest,
    ];
}

mod launcher_update;
mod launcher_view;

#[allow(clippy::module_inception)]
pub mod launcher {
    pub use super::launcher_update::*;
    pub(super) use super::prelude::*;
}

pub use launcher::*;
