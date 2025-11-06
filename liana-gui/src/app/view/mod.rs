mod label;
mod message;
mod warning;

pub mod active;
pub mod coins;
pub mod export;
pub mod fiat;
pub mod global_home;
pub mod home;
pub mod hw;

#[cfg(feature = "buysell")]
pub mod buysell;

pub mod psbt;
pub mod psbts;
pub mod receive;
pub mod recovery;
pub mod settings;
pub mod spend;
pub mod transactions;

pub use fiat::FiatAmountConverter;
pub use message::*;
use warning::warn;

use iced::{
    widget::{column, responsive, row, scrollable, Space},
    Length,
};

use liana_ui::{
    color,
    component::{button, text::*},
    icon::{
        coins_icon, cross_icon, down_icon, history_icon, home_icon, lightning_icon, receive_icon,
        recovery_icon, send_icon, settings_icon, up_icon, vault_icon,
    },
    image::*,
    theme,
    widget::*,
};

#[cfg(feature = "buysell")]
use liana_ui::icon::bitcoin_icon;

use crate::app::{cache::Cache, error::Error, menu::Menu};

fn menu_bar_highlight<'a, T: 'a>() -> Container<'a, T> {
    Container::new(Space::with_width(Length::Fixed(5.0)))
        .height(Length::Fixed(50.0))
        .style(theme::container::custom(color::ORANGE))
}

pub fn sidebar<'a>(menu: &Menu, cache: &'a Cache) -> Container<'a, Message> {
    // Top-level Home button
    let home_button = if *menu == Menu::Home {
        row!(
            button::menu_active(Some(home_icon()), "Home")
                .on_press(Message::Reload)
                .width(iced::Length::Fill),
            menu_bar_highlight(),
        )
    } else {
        row!(button::menu(Some(home_icon()), "Home")
            .on_press(Message::Menu(Menu::Home))
            .width(iced::Length::Fill),)
    };

    #[cfg(feature = "buysell")]
    let buy_sell_button = {
        if *menu == Menu::BuySell {
            row!(
                button::menu_active(Some(bitcoin_icon()), "Buy/Sell")
                    .on_press(Message::Reload)
                    .width(iced::Length::Fill),
                menu_bar_highlight()
            )
        } else {
            row!(button::menu(Some(bitcoin_icon()), "Buy/Sell")
                .on_press(Message::Menu(Menu::BuySell))
                .width(iced::Length::Fill))
        }
    };

    // Build the main menu column
    let mut menu_column = Column::new()
        .spacing(0)
        .width(Length::Fill)
        .push(
            Container::new(liana_logotype().width(Length::Fill))
                .padding(10)
                .align_x(iced::Alignment::Center)
                .width(Length::Fill),
        )
        .push(home_button);

    // Check if Active submenu is expanded from cache
    let is_active_expanded = cache.active_expanded;

    // Active menu button with expand/collapse chevron
    let active_chevron = if is_active_expanded {
        up_icon()
    } else {
        down_icon()
    };
    let active_button = Button::new(
        Row::new()
            .spacing(10)
            .align_y(iced::alignment::Vertical::Center)
            .push(lightning_icon().style(theme::text::secondary))
            .push(text("Active").size(15))
            .push(Space::with_width(Length::Fill))
            .push(active_chevron.style(theme::text::secondary))
            .padding(10),
    )
    .width(iced::Length::Fill)
    .style(theme::button::menu)
    .on_press(Message::ToggleActive);

    menu_column = menu_column.push(active_button);

    // Add Active submenu items if expanded
    if is_active_expanded {
        use crate::app::menu::ActiveSubMenu;

        // Active Send
        let active_send_button = if matches!(menu, Menu::Active(ActiveSubMenu::Send)) {
            row!(
                Space::with_width(Length::Fixed(20.0)),
                button::menu_active(Some(send_icon()), "Send")
                    .on_press(Message::Reload)
                    .width(iced::Length::Fill),
                menu_bar_highlight()
            )
            .width(Length::Fill)
        } else {
            row!(
                Space::with_width(Length::Fixed(20.0)),
                button::menu(Some(send_icon()), "Send")
                    .on_press(Message::Menu(Menu::Active(ActiveSubMenu::Send)))
                    .width(iced::Length::Fill),
            )
            .width(Length::Fill)
        };

        // Active Receive
        let active_receive_button = if matches!(menu, Menu::Active(ActiveSubMenu::Receive)) {
            row!(
                Space::with_width(Length::Fixed(20.0)),
                button::menu_active(Some(receive_icon()), "Receive")
                    .on_press(Message::Reload)
                    .width(iced::Length::Fill),
                menu_bar_highlight()
            )
            .width(Length::Fill)
        } else {
            row!(
                Space::with_width(Length::Fixed(20.0)),
                button::menu(Some(receive_icon()), "Receive")
                    .on_press(Message::Menu(Menu::Active(ActiveSubMenu::Receive)))
                    .width(iced::Length::Fill),
            )
            .width(Length::Fill)
        };

        // Active Transactions
        let active_transactions_button =
            if matches!(menu, Menu::Active(ActiveSubMenu::Transactions(_))) {
                row!(
                    Space::with_width(Length::Fixed(20.0)),
                    button::menu_active(Some(history_icon()), "Transactions")
                        .on_press(Message::Reload)
                        .width(iced::Length::Fill),
                    menu_bar_highlight()
                )
                .width(Length::Fill)
            } else {
                row!(
                    Space::with_width(Length::Fixed(20.0)),
                    button::menu(Some(history_icon()), "Transactions")
                        .on_press(Message::Menu(Menu::Active(ActiveSubMenu::Transactions(
                            None
                        ))))
                        .width(iced::Length::Fill),
                )
                .width(Length::Fill)
            };

        // Active Settings
        let active_settings_button = if matches!(menu, Menu::Active(ActiveSubMenu::Settings(_))) {
            row!(
                Space::with_width(Length::Fixed(20.0)),
                button::menu_active(Some(settings_icon()), "Settings")
                    .on_press(Message::Reload)
                    .width(iced::Length::Fill),
                menu_bar_highlight()
            )
            .width(Length::Fill)
        } else {
            row!(
                Space::with_width(Length::Fixed(20.0)),
                button::menu(Some(settings_icon()), "Settings")
                    .on_press(Message::Menu(Menu::Active(ActiveSubMenu::Settings(None))))
                    .width(iced::Length::Fill),
            )
            .width(Length::Fill)
        };

        menu_column = menu_column
            .push(active_send_button)
            .push(active_receive_button)
            .push(active_transactions_button)
            .push(active_settings_button);
    }

    // Check if Vault submenu is expanded from cache
    let is_vault_expanded = cache.vault_expanded;

    // Vault menu button with expand/collapse chevron
    let vault_chevron = if is_vault_expanded {
        up_icon()
    } else {
        down_icon()
    };
    let vault_button = Button::new(
        Row::new()
            .spacing(10)
            .align_y(iced::alignment::Vertical::Center)
            .push(vault_icon().style(theme::text::secondary))
            .push(text("Vault").size(15))
            .push(Space::with_width(Length::Fill))
            .push(vault_chevron.style(theme::text::secondary))
            .padding(10),
    )
    .width(iced::Length::Fill)
    .style(theme::button::menu)
    .on_press(Message::ToggleVault);

    menu_column = menu_column.push(vault_button);

    // Add Vault submenu items if expanded
    if is_vault_expanded {
        use crate::app::menu::VaultSubMenu;

        // Home
        let vault_home_button = if matches!(menu, Menu::Vault(VaultSubMenu::Home)) {
            row!(
                Space::with_width(Length::Fixed(20.0)),
                button::menu_active(Some(home_icon()), "Home")
                    .on_press(Message::Reload)
                    .width(iced::Length::Fill),
                menu_bar_highlight(),
            )
            .width(Length::Fill)
        } else {
            row!(
                Space::with_width(Length::Fixed(20.0)),
                button::menu(Some(home_icon()), "Home")
                    .on_press(Message::Menu(Menu::Vault(VaultSubMenu::Home)))
                    .width(iced::Length::Fill),
            )
            .width(Length::Fill)
        };

        // Send
        let vault_send_button = if matches!(menu, Menu::Vault(VaultSubMenu::Send)) {
            row!(
                Space::with_width(Length::Fixed(20.0)),
                button::menu_active(Some(send_icon()), "Send")
                    .on_press(Message::Reload)
                    .width(iced::Length::Fill),
                menu_bar_highlight()
            )
            .width(Length::Fill)
        } else {
            row!(
                Space::with_width(Length::Fixed(20.0)),
                button::menu(Some(send_icon()), "Send")
                    .on_press(Message::Menu(Menu::Vault(VaultSubMenu::Send)))
                    .width(iced::Length::Fill),
            )
            .width(Length::Fill)
        };

        // Receive
        let vault_receive_button = if matches!(menu, Menu::Vault(VaultSubMenu::Receive)) {
            row!(
                Space::with_width(Length::Fixed(20.0)),
                button::menu_active(Some(receive_icon()), "Receive")
                    .on_press(Message::Reload)
                    .width(iced::Length::Fill),
                menu_bar_highlight()
            )
            .width(Length::Fill)
        } else {
            row!(
                Space::with_width(Length::Fixed(20.0)),
                button::menu(Some(receive_icon()), "Receive")
                    .on_press(Message::Menu(Menu::Vault(VaultSubMenu::Receive)))
                    .width(iced::Length::Fill),
            )
            .width(Length::Fill)
        };

        // Coins
        let vault_coins_button = if matches!(menu, Menu::Vault(VaultSubMenu::Coins(_))) {
            row!(
                Space::with_width(Length::Fixed(20.0)),
                button::menu_active(Some(coins_icon()), "Coins")
                    .on_press(Message::Reload)
                    .width(iced::Length::Fill),
                menu_bar_highlight()
            )
            .width(Length::Fill)
        } else {
            row!(
                Space::with_width(Length::Fixed(20.0)),
                button::menu(Some(coins_icon()), "Coins")
                    .on_press(Message::Menu(Menu::Vault(VaultSubMenu::Coins(None))))
                    .width(iced::Length::Fill),
            )
            .width(Length::Fill)
        };

        // Transactions
        let vault_transactions_button =
            if matches!(menu, Menu::Vault(VaultSubMenu::Transactions(_))) {
                row!(
                    Space::with_width(Length::Fixed(20.0)),
                    button::menu_active(Some(history_icon()), "Transactions")
                        .on_press(Message::Reload)
                        .width(iced::Length::Fill),
                    menu_bar_highlight()
                )
                .width(Length::Fill)
            } else {
                row!(
                    Space::with_width(Length::Fixed(20.0)),
                    button::menu(Some(history_icon()), "Transactions")
                        .on_press(Message::Menu(Menu::Vault(VaultSubMenu::Transactions(None))))
                        .width(iced::Length::Fill),
                )
                .width(Length::Fill)
            };

        // PSBTs
        let vault_psbts_button = if matches!(menu, Menu::Vault(VaultSubMenu::PSBTs(_))) {
            row!(
                Space::with_width(Length::Fixed(20.0)),
                button::menu_active(Some(history_icon()), "PSBTs")
                    .on_press(Message::Reload)
                    .width(iced::Length::Fill),
                menu_bar_highlight()
            )
            .width(Length::Fill)
        } else {
            row!(
                Space::with_width(Length::Fixed(20.0)),
                button::menu(Some(history_icon()), "PSBTs")
                    .on_press(Message::Menu(Menu::Vault(VaultSubMenu::PSBTs(None))))
                    .width(iced::Length::Fill),
            )
            .width(Length::Fill)
        };

        // Recovery
        let vault_recovery_button = if matches!(menu, Menu::Vault(VaultSubMenu::Recovery)) {
            row!(
                Space::with_width(Length::Fixed(20.0)),
                button::menu_active(Some(recovery_icon()), "Recovery")
                    .on_press(Message::Reload)
                    .width(iced::Length::Fill),
                menu_bar_highlight()
            )
            .width(Length::Fill)
        } else {
            row!(
                Space::with_width(Length::Fixed(20.0)),
                button::menu(Some(recovery_icon()), "Recovery")
                    .on_press(Message::Menu(Menu::Vault(VaultSubMenu::Recovery)))
                    .width(iced::Length::Fill),
            )
            .width(Length::Fill)
        };

        // Settings
        let vault_settings_button = if matches!(menu, Menu::Vault(VaultSubMenu::Settings(_))) {
            row!(
                Space::with_width(Length::Fixed(20.0)),
                button::menu_active(Some(settings_icon()), "Settings")
                    .on_press(Message::Reload)
                    .width(iced::Length::Fill),
                menu_bar_highlight()
            )
            .width(Length::Fill)
        } else {
            row!(
                Space::with_width(Length::Fixed(20.0)),
                button::menu(Some(settings_icon()), "Settings")
                    .on_press(Message::Menu(Menu::Vault(VaultSubMenu::Settings(None))))
                    .width(iced::Length::Fill),
            )
            .width(Length::Fill)
        };

        menu_column = menu_column
            .push(vault_home_button)
            .push(vault_send_button)
            .push(vault_receive_button)
            .push(vault_coins_button)
            .push(vault_transactions_button)
            .push(vault_psbts_button)
            .push(vault_recovery_button)
            .push(vault_settings_button);
    }

    // Add Buy/Sell button after submenu items
    menu_column = menu_column.push_maybe({
        #[cfg(feature = "buysell")]
        {
            Some(buy_sell_button)
        }
        #[cfg(not(feature = "buysell"))]
        {
            None::<Row<'_, Message>>
        }
    });

    Container::new(
        Column::new().push(menu_column.height(Length::Fill)).push(
            Container::new(
                Column::new()
                    .spacing(10)
                    .push_maybe(cache.rescan_progress().map(|p| {
                        Container::new(text(format!("  Rescan...{:.2}%  ", p * 100.0)))
                            .padding(5)
                            .style(theme::pill::simple)
                    })),
            )
            .width(Length::Fill)
            .height(Length::Shrink),
        ),
    )
    .style(theme::container::foreground)
}

pub fn small_sidebar<'a>(menu: &Menu, cache: &'a Cache) -> Container<'a, Message> {
    // Home button
    let home_button = if *menu == Menu::Home {
        row!(
            button::menu_active_small(home_icon())
                .on_press(Message::Reload)
                .width(iced::Length::Fill),
            menu_bar_highlight(),
        )
    } else {
        row!(button::menu_small(home_icon())
            .on_press(Message::Menu(Menu::Home))
            .width(iced::Length::Fill),)
    };

    // Build menu column starting with logo and home button
    let mut menu_column = Column::new()
        .push(Container::new(liana_logotype().width(Length::Fixed(85.0))).padding(10))
        .push(home_button);

    // Active button - toggle with ToggleActive message
    let active_button = row!(button::menu_small(lightning_icon())
        .on_press(Message::ToggleActive)
        .width(iced::Length::Fill),);

    // Check if Active submenu is expanded from cache
    let is_active_expanded = cache.active_expanded;

    menu_column = menu_column.push(active_button);

    // Add Active submenu items if expanded
    if is_active_expanded {
        use crate::app::menu::ActiveSubMenu;

        // Active Send
        let active_send_button = if matches!(menu, Menu::Active(ActiveSubMenu::Send)) {
            row!(
                button::menu_active_small(send_icon())
                    .on_press(Message::Reload)
                    .width(iced::Length::Fill),
                menu_bar_highlight(),
            )
        } else {
            row!(button::menu_small(send_icon())
                .on_press(Message::Menu(Menu::Active(ActiveSubMenu::Send)))
                .width(iced::Length::Fill),)
        };

        // Active Receive
        let active_receive_button = if matches!(menu, Menu::Active(ActiveSubMenu::Receive)) {
            row!(
                button::menu_active_small(receive_icon())
                    .on_press(Message::Reload)
                    .width(iced::Length::Fill),
                menu_bar_highlight()
            )
        } else {
            row!(button::menu_small(receive_icon())
                .on_press(Message::Menu(Menu::Active(ActiveSubMenu::Receive)))
                .width(iced::Length::Fill),)
        };

        // Active Transactions
        let active_transactions_button =
            if matches!(menu, Menu::Active(ActiveSubMenu::Transactions(_))) {
                row!(
                    button::menu_active_small(history_icon())
                        .on_press(Message::Reload)
                        .width(iced::Length::Fill),
                    menu_bar_highlight()
                )
            } else {
                row!(button::menu_small(history_icon())
                    .on_press(Message::Menu(Menu::Active(ActiveSubMenu::Transactions(
                        None
                    ))))
                    .width(iced::Length::Fill),)
            };

        // Active Settings
        let active_settings_button = if matches!(menu, Menu::Active(ActiveSubMenu::Settings(_))) {
            row!(
                button::menu_active_small(settings_icon())
                    .on_press(Message::Reload)
                    .width(iced::Length::Fill),
                menu_bar_highlight()
            )
        } else {
            row!(button::menu_small(settings_icon())
                .on_press(Message::Menu(Menu::Active(ActiveSubMenu::Settings(None))))
                .width(iced::Length::Fill),)
        };

        menu_column = menu_column
            .push(active_send_button)
            .push(active_receive_button)
            .push(active_transactions_button)
            .push(active_settings_button);
    }

    // Check if Vault submenu is expanded from cache
    let is_vault_expanded = cache.vault_expanded;

    // Vault button - toggle with ToggleVault message
    let vault_button = row!(button::menu_small(vault_icon())
        .on_press(Message::ToggleVault)
        .width(iced::Length::Fill),);

    menu_column = menu_column.push(vault_button);

    // Add Vault submenu items if expanded
    if is_vault_expanded {
        use crate::app::menu::VaultSubMenu;

        // Vault Home
        let vault_home_button = if matches!(menu, Menu::Vault(VaultSubMenu::Home)) {
            row!(
                button::menu_active_small(home_icon())
                    .on_press(Message::Reload)
                    .width(iced::Length::Fill),
                menu_bar_highlight(),
            )
        } else {
            row!(button::menu_small(home_icon())
                .on_press(Message::Menu(Menu::Vault(VaultSubMenu::Home)))
                .width(iced::Length::Fill),)
        };

        // Vault Send
        let vault_send_button = if matches!(menu, Menu::Vault(VaultSubMenu::Send)) {
            row!(
                button::menu_active_small(send_icon())
                    .on_press(Message::Reload)
                    .width(iced::Length::Fill),
                menu_bar_highlight()
            )
        } else {
            row!(button::menu_small(send_icon())
                .on_press(Message::Menu(Menu::Vault(VaultSubMenu::Send)))
                .width(iced::Length::Fill),)
        };

        // Vault Receive
        let vault_receive_button = if matches!(menu, Menu::Vault(VaultSubMenu::Receive)) {
            row!(
                button::menu_active_small(receive_icon())
                    .on_press(Message::Reload)
                    .width(iced::Length::Fill),
                menu_bar_highlight()
            )
        } else {
            row!(button::menu_small(receive_icon())
                .on_press(Message::Menu(Menu::Vault(VaultSubMenu::Receive)))
                .width(iced::Length::Fill),)
        };

        // Vault Coins
        let vault_coins_button = if matches!(menu, Menu::Vault(VaultSubMenu::Coins(_))) {
            row!(
                button::menu_active_small(coins_icon())
                    .on_press(Message::Reload)
                    .width(iced::Length::Fill),
                menu_bar_highlight()
            )
        } else {
            row!(button::menu_small(coins_icon())
                .on_press(Message::Menu(Menu::Vault(VaultSubMenu::Coins(None))))
                .width(iced::Length::Fill),)
        };

        // Vault Transactions
        let vault_transactions_button =
            if matches!(menu, Menu::Vault(VaultSubMenu::Transactions(_))) {
                row!(
                    button::menu_active_small(history_icon())
                        .on_press(Message::Reload)
                        .width(iced::Length::Fill),
                    menu_bar_highlight()
                )
            } else {
                row!(button::menu_small(history_icon())
                    .on_press(Message::Menu(Menu::Vault(VaultSubMenu::Transactions(None))))
                    .width(iced::Length::Fill),)
            };

        // Vault PSBTs
        let vault_psbts_button = if matches!(menu, Menu::Vault(VaultSubMenu::PSBTs(_))) {
            row!(
                button::menu_active_small(history_icon())
                    .on_press(Message::Reload)
                    .width(iced::Length::Fill),
                menu_bar_highlight()
            )
        } else {
            row!(button::menu_small(history_icon())
                .on_press(Message::Menu(Menu::Vault(VaultSubMenu::PSBTs(None))))
                .width(iced::Length::Fill),)
        };

        // Vault Recovery
        let vault_recovery_button = if matches!(menu, Menu::Vault(VaultSubMenu::Recovery)) {
            row!(
                button::menu_active_small(recovery_icon())
                    .on_press(Message::Reload)
                    .width(iced::Length::Fill),
                menu_bar_highlight()
            )
        } else {
            row!(button::menu_small(recovery_icon())
                .on_press(Message::Menu(Menu::Vault(VaultSubMenu::Recovery)))
                .width(iced::Length::Fill),)
        };

        // Vault Settings
        let vault_settings_button = if matches!(menu, Menu::Vault(VaultSubMenu::Settings(_))) {
            row!(
                button::menu_active_small(settings_icon())
                    .on_press(Message::Reload)
                    .width(iced::Length::Fill),
                menu_bar_highlight()
            )
        } else {
            row!(button::menu_small(settings_icon())
                .on_press(Message::Menu(Menu::Vault(VaultSubMenu::Settings(None))))
                .width(iced::Length::Fill),)
        };

        menu_column = menu_column
            .push(vault_home_button)
            .push(vault_send_button)
            .push(vault_receive_button)
            .push(vault_coins_button)
            .push(vault_transactions_button)
            .push(vault_psbts_button)
            .push(vault_recovery_button)
            .push(vault_settings_button);
    }

    // Buy/Sell button
    #[cfg(feature = "buysell")]
    let buy_sell_button = if *menu == Menu::BuySell {
        row!(
            button::menu_active_small(bitcoin_icon())
                .on_press(Message::Reload)
                .width(iced::Length::Fill),
            menu_bar_highlight()
        )
    } else {
        row!(button::menu_small(bitcoin_icon())
            .on_press(Message::Menu(Menu::BuySell))
            .width(iced::Length::Fill))
    };

    // Add Buy/Sell button to menu column
    #[cfg(feature = "buysell")]
    {
        menu_column = menu_column.push(buy_sell_button);
    }

    Container::new(
        Column::new()
            .push(
                menu_column
                    .align_x(iced::Alignment::Center)
                    .height(Length::Fill),
            )
            .push(
                Container::new(
                    Column::new()
                        .spacing(10)
                        .push_maybe(cache.rescan_progress().map(|p| {
                            Container::new(text(format!("{:.2}%  ", p * 100.0)))
                                .padding(5)
                                .style(theme::pill::simple)
                        })),
                )
                .height(Length::Shrink),
            )
            .align_x(iced::Alignment::Center),
    )
    .style(theme::container::foreground)
}

pub fn dashboard<'a, T: Into<Element<'a, Message>>>(
    menu: &'a Menu,
    cache: &'a Cache,
    warning: Option<&Error>,
    content: T,
) -> Element<'a, Message> {
    Row::new()
        .push(
            Container::new(responsive(move |size| {
                if size.width > 162.0 {
                    sidebar(menu, cache).height(Length::Fill).into()
                } else {
                    small_sidebar(menu, cache).height(Length::Fill).into()
                }
            }))
            .width(Length::FillPortion(20)),
        )
        .push(
            Column::new()
                .push(warn(warning))
                .push(
                    Container::new(
                        scrollable(row!(
                            Space::with_width(Length::FillPortion(1)),
                            column!(Space::with_height(Length::Fixed(30.0)), content.into())
                                .width(Length::FillPortion(8))
                                .max_width(1500),
                            Space::with_width(Length::FillPortion(1)),
                        ))
                        .on_scroll(|w| Message::Scroll(w.absolute_offset().y)),
                    )
                    .center_x(Length::Fill)
                    .style(theme::container::background)
                    .height(Length::Fill),
                )
                .width(Length::FillPortion(130)),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

pub fn modal<'a, T: Into<Element<'a, Message>>, F: Into<Element<'a, Message>>>(
    is_previous: bool,
    warning: Option<&Error>,
    content: T,
    fixed_footer: Option<F>,
) -> Element<'a, Message> {
    Column::new()
        .push(warn(warning))
        .push(
            Container::new(
                Row::new()
                    .push(if is_previous {
                        Column::new()
                            .push(
                                button::transparent(None, "< Previous").on_press(Message::Previous),
                            )
                            .width(Length::Fill)
                    } else {
                        Column::new().width(Length::Fill)
                    })
                    .align_y(iced::Alignment::Center)
                    .push(button::secondary(Some(cross_icon()), "Close").on_press(Message::Close)),
            )
            .padding(10)
            .style(theme::container::background),
        )
        .push(modal_section(Container::new(scrollable(content))))
        .push_maybe(fixed_footer)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn modal_section<'a, T: 'a>(menu: Container<'a, T>) -> Container<'a, T> {
    Container::new(menu.max_width(1500))
        .style(theme::container::background)
        .center_x(Length::Fill)
        .width(Length::Fill)
        .height(Length::Fill)
}
