use std::sync::{Arc, Mutex};

use coincube_core::signer::MasterSigner;
use coincube_ui::component::text::*;
use coincube_ui::theme;
use coincube_ui::widget::{Button, Element};
use iced::widget::Column;
use iced::Length;

use crate::app::view::message::Message;

/// Liquid wallet settings view.
///
/// NOTE: The master seed backup flow has been moved to General Settings
/// (Cube-level backup) since the master seed is shared across all wallets.
pub fn liquid_settings_view<'a>(
    _liquid_signer: Option<Arc<Mutex<MasterSigner>>>,
) -> Element<'a, Message> {
    let header = Button::new(text("Liquid Settings").size(30).bold())
        .style(theme::button::transparent)
        .on_press(Message::Menu(crate::app::menu::Menu::Liquid(
            crate::app::menu::LiquidSubMenu::Settings(None),
        )));

    Column::new()
        .spacing(20)
        .width(Length::Fill)
        .push(header)
        .push(
            text("Seed phrase backup has moved to Cube Settings (General section).")
                .style(theme::text::secondary),
        )
        .into()
}
