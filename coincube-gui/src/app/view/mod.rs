mod message;

pub mod buysell;
pub mod global_home;
pub mod liquid;
pub mod settings;

pub mod vault;

use std::iter::FromIterator;

pub use liquid::*;
pub use message::*;
pub use vault::fiat::FiatAmountConverter;
pub use vault::warning::warn;

use iced::{
    widget::{column, container, row, scrollable, Space},
    Alignment, Length,
};

use coincube_ui::{
    color,
    component::{button, text, text::*},
    icon::{
        bitcoin_icon, coins_icon, cross_icon, cube_icon, down_icon, home_icon, lightning_icon,
        plus_icon, receipt_icon, receive_icon, recovery_icon, send_icon, settings_icon, up_icon,
        vault_icon,
    },
    image::*,
    theme,
    widget::*,
};

use crate::app::{cache::Cache, menu::Menu};

/// Simple toast notification for clipboard copy and other success messages
pub fn simple_toast(message: &str) -> Container<Message> {
    container(text::p2_regular(message))
        .padding(15)
        .style(theme::notification::success)
        .max_width(400.0)
}

fn menu_bar_highlight<'a, T: 'a>() -> Container<'a, T> {
    Container::new(Space::new().width(Length::Fixed(5.0)))
        .height(Length::Fixed(50.0))
        .style(theme::container::custom(color::ORANGE))
}

// TODO: Rework sidebar UI and implementation, use buttons without rounded borders
pub fn sidebar<'a>(menu: &Menu, cache: &'a Cache, has_vault: bool) -> Container<'a, Message> {
    // Top-level Home button
    let home_button = if *menu == Menu::Home {
        row!(
            button::menu_active(Some(cube_icon()), "Home")
                .on_press(Message::Reload)
                .width(iced::Length::Fill),
            menu_bar_highlight(),
        )
    } else {
        row!(button::menu(Some(cube_icon()), "Home")
            .on_press(Message::Menu(Menu::Home))
            .width(iced::Length::Fill),)
    };

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
            Container::new(coincube_logotype().width(Length::Fill))
                .padding(10)
                .align_x(iced::Alignment::Center)
                .width(Length::Fill),
        )
        .push(home_button);

    // Check if Liquid submenu is expanded from cache
    let is_liquid_expanded = cache.liquid_expanded;

    // Liquid menu button with expand/collapse chevron
    let liquid_chevron = if is_liquid_expanded {
        up_icon()
    } else {
        down_icon()
    };
    let liquid_button = Button::new(
        Row::new()
            .spacing(10)
            .align_y(iced::alignment::Vertical::Center)
            .push(lightning_icon().style(theme::text::secondary))
            .push(text("Liquid").size(15))
            .push(Space::new().width(Length::Fill))
            .push(liquid_chevron.style(theme::text::secondary))
            .padding(10),
    )
    .width(iced::Length::Fill)
    .style(theme::button::menu)
    .on_press(Message::ToggleLiquid);

    menu_column = menu_column.push(liquid_button);

    // Add Liquid submenu items if expanded
    if is_liquid_expanded {
        use crate::app::menu::LiquidSubMenu;

        // Liquid Overview
        let liquid_overview_button = if matches!(menu, Menu::Liquid(LiquidSubMenu::Overview)) {
            row!(
                Space::new().width(Length::Fixed(20.0)),
                button::menu_active(Some(home_icon()), "Overview")
                    .on_press(Message::Reload)
                    .width(iced::Length::Fill),
                menu_bar_highlight()
            )
            .width(Length::Fill)
        } else {
            row!(
                Space::new().width(Length::Fixed(20.0)),
                button::menu(Some(home_icon()), "Overview")
                    .on_press(Message::Menu(Menu::Liquid(LiquidSubMenu::Overview)))
                    .width(iced::Length::Fill),
            )
            .width(Length::Fill)
        };

        // Liquid Send
        let liquid_send_button = if matches!(menu, Menu::Liquid(LiquidSubMenu::Send)) {
            row!(
                Space::new().width(Length::Fixed(20.0)),
                button::menu_active(Some(send_icon()), "Send")
                    .on_press(Message::Reload)
                    .width(iced::Length::Fill),
                menu_bar_highlight()
            )
            .width(Length::Fill)
        } else {
            row!(
                Space::new().width(Length::Fixed(20.0)),
                button::menu(Some(send_icon()), "Send")
                    .on_press(Message::Menu(Menu::Liquid(LiquidSubMenu::Send)))
                    .width(iced::Length::Fill),
            )
            .width(Length::Fill)
        };

        // Liquid Receive
        let liquid_receive_button = if matches!(menu, Menu::Liquid(LiquidSubMenu::Receive)) {
            row!(
                Space::new().width(Length::Fixed(20.0)),
                button::menu_active(Some(receive_icon()), "Receive")
                    .on_press(Message::Reload)
                    .width(iced::Length::Fill),
                menu_bar_highlight()
            )
            .width(Length::Fill)
        } else {
            row!(
                Space::new().width(Length::Fixed(20.0)),
                button::menu(Some(receive_icon()), "Receive")
                    .on_press(Message::Menu(Menu::Liquid(LiquidSubMenu::Receive)))
                    .width(iced::Length::Fill),
            )
            .width(Length::Fill)
        };

        // Liquid Transactions
        let liquid_transactions_button =
            if matches!(menu, Menu::Liquid(LiquidSubMenu::Transactions(_))) {
                row!(
                    Space::new().width(Length::Fixed(20.0)),
                    button::menu_active(Some(receipt_icon()), "Transactions")
                        .on_press(Message::Reload)
                        .width(iced::Length::Fill),
                    menu_bar_highlight()
                )
                .width(Length::Fill)
            } else {
                row!(
                    Space::new().width(Length::Fixed(20.0)),
                    button::menu(Some(receipt_icon()), "Transactions")
                        .on_press(Message::Menu(Menu::Liquid(LiquidSubMenu::Transactions(
                            None
                        ))))
                        .width(iced::Length::Fill),
                )
                .width(Length::Fill)
            };

        // Liquid Settings
        let liquid_settings_button = if matches!(menu, Menu::Liquid(LiquidSubMenu::Settings(_))) {
            row!(
                Space::new().width(Length::Fixed(20.0)),
                button::menu_active(Some(settings_icon()), "Settings")
                    .on_press(Message::Reload)
                    .width(iced::Length::Fill),
                menu_bar_highlight()
            )
            .width(Length::Fill)
        } else {
            row!(
                Space::new().width(Length::Fixed(20.0)),
                button::menu(Some(settings_icon()), "Settings")
                    .on_press(Message::Menu(Menu::Liquid(LiquidSubMenu::Settings(None))))
                    .width(iced::Length::Fill),
            )
            .width(Length::Fill)
        };

        menu_column = menu_column
            .push(liquid_overview_button)
            .push(liquid_send_button)
            .push(liquid_receive_button)
            .push(liquid_transactions_button)
            .push(liquid_settings_button);
    }

    // Check if Vault submenu is expanded from cache
    let is_vault_expanded = cache.vault_expanded;

    // Vault menu button - show "Vault +" if no vault exists, otherwise show expandable "Vault"
    if !has_vault {
        // No vault - show "Vault +" button that launches installer
        let vault_plus_button = Button::new(
            Row::new()
                .spacing(10)
                .align_y(iced::alignment::Vertical::Center)
                .push(vault_icon().style(theme::text::secondary))
                .push(text("Vault").size(15))
                .push(Space::new().width(Length::Fill))
                .push(
                    Container::new(plus_icon().style(theme::text::secondary))
                        .padding(iced::Padding::from([3.0, 0.0])) // Add 3px top and bottom padding for better centering
                        .align_y(iced::alignment::Vertical::Top),
                )
                .padding(10),
        )
        .width(iced::Length::Fill)
        .style(theme::button::menu)
        .on_press(Message::SetupVault);

        menu_column = menu_column.push(vault_plus_button);
    } else {
        // Has vault - show expandable Vault menu
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
                .push(Space::new().width(Length::Fill))
                .push(vault_chevron.style(theme::text::secondary))
                .padding(10),
        )
        .width(iced::Length::Fill)
        .style(theme::button::menu)
        .on_press(Message::ToggleVault);

        menu_column = menu_column.push(vault_button);
    }

    // Add Vault submenu items if expanded (and vault exists)
    if has_vault && is_vault_expanded {
        use crate::app::menu::VaultSubMenu;

        // Overview
        let vault_overview_button = if matches!(menu, Menu::Vault(VaultSubMenu::Overview)) {
            row!(
                Space::new().width(Length::Fixed(20.0)),
                button::menu_active(Some(home_icon()), "Overview")
                    .on_press(Message::Reload)
                    .width(iced::Length::Fill),
                menu_bar_highlight(),
            )
            .width(Length::Fill)
        } else {
            row!(
                Space::new().width(Length::Fixed(20.0)),
                button::menu(Some(home_icon()), "Overview")
                    .on_press(Message::Menu(Menu::Vault(VaultSubMenu::Overview)))
                    .width(iced::Length::Fill),
            )
            .width(Length::Fill)
        };

        // Send
        let vault_send_button = if matches!(menu, Menu::Vault(VaultSubMenu::Send)) {
            row!(
                Space::new().width(Length::Fixed(20.0)),
                button::menu_active(Some(send_icon()), "Send")
                    .on_press(Message::Reload)
                    .width(iced::Length::Fill),
                menu_bar_highlight()
            )
            .width(Length::Fill)
        } else {
            row!(
                Space::new().width(Length::Fixed(20.0)),
                button::menu(Some(send_icon()), "Send")
                    .on_press(Message::Menu(Menu::Vault(VaultSubMenu::Send)))
                    .width(iced::Length::Fill),
            )
            .width(Length::Fill)
        };

        // Receive
        let vault_receive_button = if matches!(menu, Menu::Vault(VaultSubMenu::Receive)) {
            row!(
                Space::new().width(Length::Fixed(20.0)),
                button::menu_active(Some(receive_icon()), "Receive")
                    .on_press(Message::Reload)
                    .width(iced::Length::Fill),
                menu_bar_highlight()
            )
            .width(Length::Fill)
        } else {
            row!(
                Space::new().width(Length::Fixed(20.0)),
                button::menu(Some(receive_icon()), "Receive")
                    .on_press(Message::Menu(Menu::Vault(VaultSubMenu::Receive)))
                    .width(iced::Length::Fill),
            )
            .width(Length::Fill)
        };

        // Coins
        let vault_coins_button = if matches!(menu, Menu::Vault(VaultSubMenu::Coins(_))) {
            row!(
                Space::new().width(Length::Fixed(20.0)),
                button::menu_active(Some(coins_icon()), "Coins")
                    .on_press(Message::Reload)
                    .width(iced::Length::Fill),
                menu_bar_highlight()
            )
            .width(Length::Fill)
        } else {
            row!(
                Space::new().width(Length::Fixed(20.0)),
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
                    Space::new().width(Length::Fixed(20.0)),
                    button::menu_active(Some(receipt_icon()), "Transactions")
                        .on_press(Message::Reload)
                        .width(iced::Length::Fill),
                    menu_bar_highlight()
                )
                .width(Length::Fill)
            } else {
                row!(
                    Space::new().width(Length::Fixed(20.0)),
                    button::menu(Some(receipt_icon()), "Transactions")
                        .on_press(Message::Menu(Menu::Vault(VaultSubMenu::Transactions(None))))
                        .width(iced::Length::Fill),
                )
                .width(Length::Fill)
            };

        // PSBTs
        let vault_psbts_button = if matches!(menu, Menu::Vault(VaultSubMenu::PSBTs(_))) {
            row!(
                Space::new().width(Length::Fixed(20.0)),
                button::menu_active(Some(receipt_icon()), "PSBTs")
                    .on_press(Message::Reload)
                    .width(iced::Length::Fill),
                menu_bar_highlight()
            )
            .width(Length::Fill)
        } else {
            row!(
                Space::new().width(Length::Fixed(20.0)),
                button::menu(Some(receipt_icon()), "PSBTs")
                    .on_press(Message::Menu(Menu::Vault(VaultSubMenu::PSBTs(None))))
                    .width(iced::Length::Fill),
            )
            .width(Length::Fill)
        };

        // Recovery
        let vault_recovery_button = if matches!(menu, Menu::Vault(VaultSubMenu::Recovery)) {
            row!(
                Space::new().width(Length::Fixed(20.0)),
                button::menu_active(Some(recovery_icon()), "Recovery")
                    .on_press(Message::Reload)
                    .width(iced::Length::Fill),
                menu_bar_highlight()
            )
            .width(Length::Fill)
        } else {
            row!(
                Space::new().width(Length::Fixed(20.0)),
                button::menu(Some(recovery_icon()), "Recovery")
                    .on_press(Message::Menu(Menu::Vault(VaultSubMenu::Recovery)))
                    .width(iced::Length::Fill),
            )
            .width(Length::Fill)
        };

        // Settings
        let vault_settings_button = if matches!(menu, Menu::Vault(VaultSubMenu::Settings(_))) {
            row!(
                Space::new().width(Length::Fixed(20.0)),
                button::menu_active(Some(settings_icon()), "Settings")
                    .on_press(Message::Reload)
                    .width(iced::Length::Fill),
                menu_bar_highlight()
            )
            .width(Length::Fill)
        } else {
            row!(
                Space::new().width(Length::Fixed(20.0)),
                button::menu(Some(settings_icon()), "Settings")
                    .on_press(Message::Menu(Menu::Vault(VaultSubMenu::Settings(None))))
                    .width(iced::Length::Fill),
            )
            .width(Length::Fill)
        };

        menu_column = menu_column
            .push(vault_overview_button)
            .push(vault_send_button)
            .push(vault_receive_button)
            .push(vault_coins_button)
            .push(vault_transactions_button)
            .push(vault_psbts_button)
            .push(vault_recovery_button)
            .push(vault_settings_button);
    }

    menu_column = menu_column.push(has_vault.then_some(buy_sell_button));

    // Global Settings button (always visible at bottom of main menu)
    let global_settings_button = if matches!(menu, Menu::Settings(_)) {
        row!(
            button::menu_active(Some(settings_icon()), "Settings")
                .on_press(Message::Reload)
                .width(iced::Length::Fill),
            menu_bar_highlight(),
        )
    } else {
        row!(button::menu(Some(settings_icon()), "Settings")
            .on_press(Message::Menu(Menu::Settings(
                crate::app::menu::SettingsSubMenu::General
            )))
            .width(iced::Length::Fill),)
    };

    menu_column = menu_column.push(global_settings_button);

    Container::new(
        Column::new().push(menu_column.height(Length::Fill)).push(
            Container::new(
                Column::new()
                    .spacing(10)
                    .push(cache.rescan_progress().map(|p| {
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

pub fn dashboard<'a, T: Into<Element<'a, Message>>>(
    menu: &'a Menu,
    cache: &'a Cache,
    content: T,
) -> Element<'a, Message> {
    let has_vault = cache.has_vault; // Copy the bool value before moving into closure
    Row::new()
        .push(
            sidebar(menu, cache, has_vault)
                .height(Length::Fill)
                .width(Length::Fixed(190.0)),
        )
        .push(
            Column::new()
                .push(warn(None))
                .push(
                    Container::new(
                        scrollable(row!(
                            Space::new().width(Length::FillPortion(1)),
                            column!(Space::new().height(Length::Fixed(30.0)), content.into())
                                .width(Length::FillPortion(8))
                                .max_width(1500),
                            Space::new().width(Length::FillPortion(1)),
                        ))
                        .on_scroll(|w| Message::Scroll(w.absolute_offset().y)),
                    )
                    .center_x(Length::Fill)
                    .style(theme::container::background)
                    .height(Length::Fill),
                )
                .width(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

pub fn modal<'a, T: Into<Element<'a, Message>>, F: Into<Element<'a, Message>>>(
    is_previous: bool,
    content: T,
    fixed_footer: Option<F>,
) -> Element<'a, Message> {
    Column::new()
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
        .push(fixed_footer)
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

pub fn placeholder<'a, T: Into<Element<'a, Message>>>(
    icon: T,
    title: &'a str,
    subtitle: &'a str,
) -> Element<'a, Message> {
    let content = Column::new()
        .push(icon)
        .push(text(title).style(theme::text::secondary).bold())
        .push(
            text(subtitle)
                .size(P2_SIZE)
                .style(theme::text::secondary)
                .align_x(Alignment::Center),
        )
        .spacing(16)
        .align_x(Alignment::Center);

    Container::new(content)
        .width(Length::Fill)
        .padding(60)
        .center_x(Length::Fill)
        .style(|_| container::Style {
            background: Some(iced::Background::Color(color::GREY_6)),
            border: iced::Border {
                radius: 20.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}

pub fn error_toast_overlay<'a, I: Iterator<Item = (usize, &'a str)>>(
    iter: I,
) -> coincube_ui::widget::Element<'a, Message> {
    use coincube_ui::{color, component::text, icon::cross_icon};

    let toast = |id: usize, content: &'a str| {
        const WIDGET_HEIGHT: u32 = 80;
        iced::widget::row![
            container(text::p1_bold(content).color(color::WHITE))
                .width(600)
                .height(WIDGET_HEIGHT)
                .padding(15)
                .align_y(iced::Alignment::Center)
                .style(|_| {
                    iced::widget::container::Style::default()
                        .background(iced::Color::BLACK)
                        .border(iced::Border::default().width(1).color(color::RED))
                }),
            iced::widget::Button::new(
                cross_icon()
                    .color(color::BLACK)
                    .size(36)
                    .align_x(iced::Alignment::Center)
                    .align_y(iced::Alignment::Center)
                    .height(iced::Length::Fill)
            )
            .height(WIDGET_HEIGHT)
            .width(60)
            .style(|_, _| iced::widget::button::Style::default().with_background(color::RED))
            .on_press(Message::DismissToast(id))
        ]
    };

    let centered = iced::widget::row![
        // offset the toast by the space covered by the dashboard
        iced::widget::Space::new().width(190.0),
        // center toasts horizontally
        iced::widget::Space::new().width(iced::Length::Fill),
        iced::widget::Column::from_iter(iter.map(|(id, content)| toast(id, content).into()))
            .spacing(10),
        iced::widget::Space::new().width(iced::Length::Fill),
    ];

    // full screen positioning
    let column = iced::widget::column![
        iced::widget::Space::new().height(iced::Length::Fill),
        centered,
        iced::widget::Space::new().height(25),
    ];

    container(column)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .into()
}
