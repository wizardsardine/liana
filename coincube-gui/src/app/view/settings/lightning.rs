//! App-level **Settings → Lightning** page.
//!
//! Today this hosts a single card — the "Default Lightning backend"
//! picker. The picker decides which wallet fulfills incoming
//! Lightning Address invoices for the current cube (Spark or Liquid).
//! Surfacing it at the app level rather than inside the Spark
//! wallet's own Settings panel matches the mental model: Lightning
//! routing is a cross-wallet concern, not a Spark-specific control.
//!
//! Additional Lightning-flavored preferences (BOLT12 toggles, LNURL
//! pay defaults, etc.) can pile into this page as they land.

use iced::widget::{Column, Row, Space};
use iced::{Alignment, Length};

use coincube_ui::component::{button, card, text::*};
use coincube_ui::theme;
use coincube_ui::widget::Element;

use crate::app::cache::Cache;
use crate::app::menu::Menu;
use crate::app::view::dashboard;
use crate::app::view::message::*;
use crate::app::wallets::WalletKind;

pub fn lightning_section<'a>(menu: &'a Menu, cache: &'a Cache) -> Element<'a, Message> {
    let col = Column::new()
        .spacing(20)
        .push(super::header(
            "Lightning",
            SettingsMessage::LightningSection,
        ))
        .push(backend_picker_card(cache.default_lightning_backend));

    dashboard(menu, cache, col)
}

fn backend_picker_card<'a>(current: WalletKind) -> Element<'a, Message> {
    let spark_btn = picker_chip(
        "Spark",
        current == WalletKind::Spark,
        Some(SettingsMessage::DefaultLightningBackendChanged(WalletKind::Spark).into()),
    );
    let liquid_btn = picker_chip(
        "Liquid",
        current == WalletKind::Liquid,
        Some(SettingsMessage::DefaultLightningBackendChanged(WalletKind::Liquid).into()),
    );

    card::simple(
        Column::new()
            .spacing(14)
            .push(text("Default Lightning backend").bold())
            .push(
                text(
                    "Chooses which wallet fulfills incoming Lightning \
                     Address invoices for this cube. Spark is the default. \
                     NIP-57 zaps always route through Liquid regardless — \
                     their description is too long for Spark's BOLT11 \
                     description field.",
                )
                .size(P2_SIZE),
            )
            .push(
                Row::new()
                    .spacing(12)
                    .align_y(Alignment::Center)
                    .push(spark_btn)
                    .push(liquid_btn)
                    .push(Space::new().width(Length::Fill)),
            ),
    )
    .width(Length::Fill)
    .into()
}

/// Picker chip styled like the Home page Send/Receive buttons:
/// the active chip uses `button::primary` (solid orange) and the
/// inactive chip uses `button::orange_outline` (orange outline →
/// solid orange on hover). Both helpers center their text, so the
/// two chips sit flush side-by-side.
fn picker_chip<'a>(
    label: &'static str,
    active: bool,
    on_press: Option<Message>,
) -> Element<'a, Message> {
    let btn = if active {
        button::primary(None, label)
    } else {
        button::orange_outline(None, label)
    };
    btn.width(Length::Fixed(140.0))
        .padding([8, 16])
        .on_press_maybe(on_press)
        .into()
}
