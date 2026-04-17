mod message;

pub mod buysell;
pub mod connect;
pub mod global_home;
pub mod liquid;
pub mod p2p;
pub mod settings;
pub mod spark;

pub mod vault;

use std::iter::FromIterator;

pub use liquid::*;
pub use message::*;
pub use spark::{
    SparkOverviewMessage, SparkOverviewView, SparkReceiveMessage, SparkReceiveView,
    SparkSendMessage, SparkSendView, SparkSettingsMessage, SparkSettingsStatus, SparkSettingsView,
    SparkStatus, SparkTransactionsMessage, SparkTransactionsStatus, SparkTransactionsView,
};
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
        bitcoin_icon, chat_icon, clipboard_icon, coins_icon, connect_icon, cross_icon, cube_icon,
        down_icon, home_icon, lightning_icon, person_icon, plus_icon, receipt_icon, receive_icon,
        recovery_icon, send_icon, settings_icon, shop_icon, up_icon, vault_icon,
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

/// Wraps `content` in the shared balance card style used across wallet overview and send screens
/// (themed card background, orange border, rounded corners).
pub fn balance_header_card<'a, Msg: 'a>(content: impl Into<Element<'a, Msg>>) -> Element<'a, Msg> {
    container(content)
        .padding(20)
        .width(Length::Fill)
        .style(theme::container::balance_header)
        .into()
}

/// A compact "master seed not backed up" warning strip, rendered at the
/// top of every page by the `dashboard` wrapper when
/// `cache.current_cube_backed_up` is false and the user hasn't dismissed
/// it this session.
///
/// Constrained to the same width as the main content column so it lines
/// up with the page content below it. Clicking "Back Up Now" routes to
/// General Settings; clicking the × dismisses for the session — it
/// returns on app restart until the user actually backs up.
pub fn backup_warning_banner<'a>() -> Element<'a, Message> {
    let body = container(
        row![
            coincube_ui::icon::warning_icon().color(color::BLACK),
            text::p2_regular(
                "Your master seed phrase is not backed up. Back it up to avoid \
                 losing access to your Cube."
            )
            .color(color::BLACK),
            Space::new().width(Length::Fill),
            button::secondary(None, "Back Up Now")
                .padding([6, 14])
                .width(Length::Fixed(140.0))
                .on_press(Message::Menu(Menu::Settings(
                    crate::app::menu::SettingsSubMenu::General,
                ))),
            iced::widget::Button::new(
                cross_icon()
                    .align_x(Alignment::Center)
                    .align_y(Alignment::Center),
            )
            .padding([8, 10])
            .style(theme::button::secondary)
            .on_press(Message::DismissBackupWarning),
        ]
        .spacing(10)
        .align_y(Alignment::Center),
    )
    .padding([8, 16])
    .width(Length::Fill)
    .style(theme::notification::warning);

    // Constrain to the same FillPortion(1/8/1) layout used by the
    // dashboard content column so the banner lines up horizontally.
    container(row![
        Space::new().width(Length::FillPortion(1)),
        container(body)
            .width(Length::FillPortion(8))
            .max_width(1500),
        Space::new().width(Length::FillPortion(1)),
    ])
    .padding([8, 0])
    .width(Length::Fill)
    .into()
}

fn menu_bar_highlight<'a, T: 'a>() -> Container<'a, T> {
    Container::new(Space::new().width(Length::Fixed(5.0)))
        .height(Length::Fixed(50.0))
        .style(theme::container::custom(color::ORANGE))
}

// TODO: Rework sidebar UI and implementation, use buttons without rounded borders
pub fn sidebar<'a>(
    menu: &Menu,
    cache: &'a Cache,
    has_vault: bool,
    cube_name: &'a str,
    avatar_handle: Option<&'a iced::widget::image::Handle>,
    lightning_address: Option<&'a str>,
    has_p2p: bool,
) -> Container<'a, Message> {
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

    // ── Marketplace nav group (Buy/Sell + P2P) ──────────────────────────────
    use crate::app::menu::{MarketplaceSubMenu, P2PSubMenu};

    // Build the main menu column
    let mut menu_column = Column::new().spacing(0).width(Length::Fill).push(
        Container::new(coincube_wordmark(28.0))
            .padding(10)
            .center_x(Length::Fill),
    );

    // Avatar + Cube name + Lightning Address below logo (skip if no identity set)
    if !cube_name.is_empty() || avatar_handle.is_some() || lightning_address.is_some() {
        let avatar_widget: Element<Message> = if let Some(handle) = avatar_handle {
            iced::widget::image(handle.clone())
                .width(Length::Fixed(60.0))
                .height(Length::Fixed(60.0))
                .into()
        } else {
            container(cube_icon().size(30).color(color::GREY_3))
                .width(Length::Fixed(60.0))
                .height(Length::Fixed(60.0))
                .center_x(Length::Fixed(60.0))
                .center_y(Length::Fixed(60.0))
                .style(|t| container::Style {
                    background: Some(iced::Background::Color(t.colors.cards.simple.background)),
                    border: iced::Border {
                        radius: 30.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .into()
        };

        let mut info_col = Column::new()
            .push(avatar_widget)
            .push(Space::new().height(Length::Fixed(6.0)))
            .push(
                text::p2_bold(cube_name)
                    .style(theme::text::primary)
                    .align_x(iced::Alignment::Center),
            )
            .align_x(iced::Alignment::Center)
            .width(Length::Fill);

        if let Some(addr) = lightning_address {
            let display_addr = if addr.contains('@') {
                addr.to_string()
            } else {
                format!("{}@coincube.io", addr)
            };
            info_col = info_col.push(
                Button::new(
                    Row::new()
                        .push(text::caption(display_addr.clone()).color(color::GREY_3))
                        .push(clipboard_icon().size(10).color(color::GREY_3))
                        .spacing(4)
                        .align_y(iced::Alignment::Center),
                )
                .style(theme::button::transparent)
                .on_press(Message::Clipboard(display_addr)),
            );
        }

        menu_column = menu_column.push(
            container(info_col)
                .padding(iced::Padding::from([8, 10]))
                .width(Length::Fill)
                .center_x(Length::Fill),
        );
    }

    menu_column = menu_column.push(home_button);

    // ── Spark wallet section ──────────────────────────────────────────────
    //
    // Sits above Liquid in the sidebar because Spark is the default
    // wallet for everyday Lightning UX; Liquid is the advanced slot
    // for L-BTC, USDt, and other Liquid-native flows.
    let is_spark_expanded = cache.spark_expanded;

    let spark_chevron = if is_spark_expanded {
        up_icon()
    } else {
        down_icon()
    };
    let spark_button = Button::new(
        Row::new()
            .spacing(10)
            .align_y(iced::alignment::Vertical::Center)
            .push(coincube_ui::icon::lightning_icon().style(theme::text::secondary))
            .push(text("Spark").size(15))
            .push(Space::new().width(Length::Fill))
            .push(spark_chevron.style(theme::text::secondary))
            .padding(10),
    )
    .width(iced::Length::Fill)
    .style(theme::button::menu)
    .on_press(Message::ToggleSpark);

    menu_column = menu_column.push(spark_button);

    if is_spark_expanded {
        use crate::app::menu::SparkSubMenu;

        let spark_overview_button = if matches!(menu, Menu::Spark(SparkSubMenu::Overview)) {
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
                    .on_press(Message::Menu(Menu::Spark(SparkSubMenu::Overview)))
                    .width(iced::Length::Fill),
            )
            .width(Length::Fill)
        };

        let spark_send_button = if matches!(menu, Menu::Spark(SparkSubMenu::Send)) {
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
                    .on_press(Message::Menu(Menu::Spark(SparkSubMenu::Send)))
                    .width(iced::Length::Fill),
            )
            .width(Length::Fill)
        };

        let spark_receive_button = if matches!(menu, Menu::Spark(SparkSubMenu::Receive)) {
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
                    .on_press(Message::Menu(Menu::Spark(SparkSubMenu::Receive)))
                    .width(iced::Length::Fill),
            )
            .width(Length::Fill)
        };

        let spark_transactions_button =
            if matches!(menu, Menu::Spark(SparkSubMenu::Transactions(_))) {
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
                        .on_press(Message::Menu(Menu::Spark(SparkSubMenu::Transactions(None))))
                        .width(iced::Length::Fill),
                )
                .width(Length::Fill)
            };

        let spark_settings_button = if matches!(menu, Menu::Spark(SparkSubMenu::Settings(_))) {
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
                    .on_press(Message::Menu(Menu::Spark(SparkSubMenu::Settings(None))))
                    .width(iced::Length::Fill),
            )
            .width(Length::Fill)
        };

        menu_column = menu_column
            .push(spark_overview_button)
            .push(spark_send_button)
            .push(spark_receive_button)
            .push(spark_transactions_button)
            .push(spark_settings_button);
    }

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
            .push(coincube_ui::icon::droplet_fill_icon().style(theme::text::secondary))
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

    // ── Marketplace accordion ──────────────────────────────────────────────
    if has_vault || has_p2p {
        let is_marketplace_expanded = cache.marketplace_expanded;
        let marketplace_chevron = if is_marketplace_expanded {
            up_icon()
        } else {
            down_icon()
        };
        let marketplace_button = Button::new(
            Row::new()
                .spacing(10)
                .align_y(iced::alignment::Vertical::Center)
                .push(shop_icon().style(theme::text::secondary))
                .push(text("Marketplace").size(15))
                .push(Space::new().width(Length::Fill))
                .push(marketplace_chevron.style(theme::text::secondary))
                .padding(10),
        )
        .width(iced::Length::Fill)
        .style(theme::button::menu)
        .on_press(Message::ToggleMarketplace);

        menu_column = menu_column.push(marketplace_button);

        if is_marketplace_expanded {
            // Buy/Sell (KYC) child — flat button at 20px indent
            if has_vault {
                let buy_sell_button =
                    if matches!(menu, Menu::Marketplace(MarketplaceSubMenu::BuySell)) {
                        row!(
                            Space::new().width(Length::Fixed(20.0)),
                            button::menu_active(Some(bitcoin_icon()), "Buy/Sell (KYC)")
                                .on_press(Message::Reload)
                                .width(iced::Length::Fill),
                            menu_bar_highlight()
                        )
                        .width(Length::Fill)
                    } else {
                        row!(
                            Space::new().width(Length::Fixed(20.0)),
                            button::menu(Some(bitcoin_icon()), "Buy/Sell (KYC)")
                                .on_press(Message::Menu(Menu::Marketplace(
                                    MarketplaceSubMenu::BuySell,
                                )))
                                .width(iced::Length::Fill),
                        )
                        .width(Length::Fill)
                    };
                menu_column = menu_column.push(buy_sell_button);
            }

            // P2P Exchange child — nested accordion at 20px indent
            if has_p2p {
                let is_p2p_expanded = cache.marketplace_p2p_expanded;
                let p2p_chevron = if is_p2p_expanded {
                    up_icon()
                } else {
                    down_icon()
                };
                let p2p_button: Element<Message> = row!(
                    Space::new().width(Length::Fixed(20.0)),
                    Button::new(
                        Row::new()
                            .spacing(10)
                            .align_y(iced::alignment::Vertical::Center)
                            .push(person_icon().style(theme::text::secondary))
                            .push(text("P2P Exchange").size(15))
                            .push(Space::new().width(Length::Fill))
                            .push(p2p_chevron.style(theme::text::secondary))
                            .padding(10),
                    )
                    .width(iced::Length::Fill)
                    .style(theme::button::menu)
                    .on_press(Message::ToggleMarketplaceP2P),
                )
                .width(Length::Fill)
                .into();
                menu_column = menu_column.push(p2p_button);

                if is_p2p_expanded {
                    let p2p_overview_button = if matches!(
                        menu,
                        Menu::Marketplace(MarketplaceSubMenu::P2P(P2PSubMenu::Overview))
                    ) {
                        row!(
                            Space::new().width(Length::Fixed(40.0)),
                            button::menu_active(Some(home_icon()), "Order Book")
                                .on_press(Message::Reload)
                                .width(iced::Length::Fill),
                            menu_bar_highlight()
                        )
                        .width(Length::Fill)
                    } else {
                        row!(
                            Space::new().width(Length::Fixed(40.0)),
                            button::menu(Some(home_icon()), "Order Book")
                                .on_press(Message::Menu(Menu::Marketplace(
                                    MarketplaceSubMenu::P2P(P2PSubMenu::Overview),
                                )))
                                .width(iced::Length::Fill),
                        )
                        .width(Length::Fill)
                    };

                    let p2p_my_trades_button = if matches!(
                        menu,
                        Menu::Marketplace(MarketplaceSubMenu::P2P(P2PSubMenu::MyTrades))
                    ) {
                        row!(
                            Space::new().width(Length::Fixed(40.0)),
                            button::menu_active(Some(receipt_icon()), "My Trades")
                                .on_press(Message::Reload)
                                .width(iced::Length::Fill),
                            menu_bar_highlight()
                        )
                        .width(Length::Fill)
                    } else {
                        row!(
                            Space::new().width(Length::Fixed(40.0)),
                            button::menu(Some(receipt_icon()), "My Trades")
                                .on_press(Message::Menu(Menu::Marketplace(
                                    MarketplaceSubMenu::P2P(P2PSubMenu::MyTrades),
                                )))
                                .width(iced::Length::Fill),
                        )
                        .width(Length::Fill)
                    };

                    let p2p_chat_button = if matches!(
                        menu,
                        Menu::Marketplace(MarketplaceSubMenu::P2P(P2PSubMenu::Chat))
                    ) {
                        row!(
                            Space::new().width(Length::Fixed(40.0)),
                            button::menu_active(Some(chat_icon()), "Chat")
                                .on_press(Message::Reload)
                                .width(iced::Length::Fill),
                            menu_bar_highlight()
                        )
                        .width(Length::Fill)
                    } else {
                        row!(
                            Space::new().width(Length::Fixed(40.0)),
                            button::menu(Some(chat_icon()), "Chat")
                                .on_press(Message::Menu(Menu::Marketplace(
                                    MarketplaceSubMenu::P2P(P2PSubMenu::Chat),
                                )))
                                .width(iced::Length::Fill),
                        )
                        .width(Length::Fill)
                    };

                    let p2p_create_order_button = if matches!(
                        menu,
                        Menu::Marketplace(MarketplaceSubMenu::P2P(P2PSubMenu::CreateOrder))
                    ) {
                        row!(
                            Space::new().width(Length::Fixed(40.0)),
                            button::menu_active(Some(plus_icon()), "Create Order")
                                .on_press(Message::Reload)
                                .width(iced::Length::Fill),
                            menu_bar_highlight()
                        )
                        .width(Length::Fill)
                    } else {
                        row!(
                            Space::new().width(Length::Fixed(40.0)),
                            button::menu(Some(plus_icon()), "Create Order")
                                .on_press(Message::Menu(Menu::Marketplace(
                                    MarketplaceSubMenu::P2P(P2PSubMenu::CreateOrder),
                                )))
                                .width(iced::Length::Fill),
                        )
                        .width(Length::Fill)
                    };

                    let p2p_settings_button = if matches!(
                        menu,
                        Menu::Marketplace(MarketplaceSubMenu::P2P(P2PSubMenu::Settings))
                    ) {
                        row!(
                            Space::new().width(Length::Fixed(40.0)),
                            button::menu_active(Some(settings_icon()), "Settings")
                                .width(iced::Length::Fill),
                            menu_bar_highlight()
                        )
                        .width(Length::Fill)
                    } else {
                        row!(
                            Space::new().width(Length::Fixed(40.0)),
                            button::menu(Some(settings_icon()), "Settings")
                                .on_press(Message::Menu(Menu::Marketplace(
                                    MarketplaceSubMenu::P2P(P2PSubMenu::Settings),
                                )))
                                .width(iced::Length::Fill),
                        )
                        .width(Length::Fill)
                    };

                    menu_column = menu_column
                        .push(p2p_overview_button)
                        .push(p2p_my_trades_button)
                        .push(p2p_chat_button)
                        .push(p2p_create_order_button)
                        .push(p2p_settings_button);
                }
            }
        }
    }

    // ── Connect nav group ────────────────────────────────────────────────────
    let is_connect_expanded = cache.connect_expanded;
    let is_connect_authenticated = cache.connect_authenticated;

    let connect_button: Element<Message> = if is_connect_authenticated {
        let connect_chevron = if is_connect_expanded {
            up_icon()
        } else {
            down_icon()
        };
        Button::new(
            Row::new()
                .spacing(10)
                .align_y(iced::alignment::Vertical::Center)
                .push(connect_icon().style(theme::text::secondary))
                .push(text("Connect").size(15))
                .push(Space::new().width(Length::Fill))
                .push(connect_chevron.style(theme::text::secondary))
                .padding(10),
        )
        .width(iced::Length::Fill)
        .style(theme::button::menu)
        .on_press(Message::ToggleConnect)
        .into()
    } else if matches!(menu, Menu::Connect(_)) {
        row!(
            button::menu_active(Some(connect_icon()), "Connect")
                .on_press(Message::Reload)
                .width(iced::Length::Fill),
            menu_bar_highlight(),
        )
        .width(Length::Fill)
        .into()
    } else {
        row!(button::menu(Some(connect_icon()), "Connect")
            .on_press(Message::ToggleConnect)
            .width(iced::Length::Fill),)
        .into()
    };

    menu_column = menu_column.push(connect_button);

    if is_connect_expanded && is_connect_authenticated {
        use crate::app::menu::ConnectSubMenu;

        let connect_ln_address_button =
            if matches!(menu, Menu::Connect(ConnectSubMenu::LightningAddress)) {
                row!(
                    Space::new().width(Length::Fixed(20.0)),
                    button::menu_active(Some(lightning_icon()), "Lightning Address")
                        .on_press(Message::Reload)
                        .width(iced::Length::Fill),
                    menu_bar_highlight()
                )
                .width(Length::Fill)
            } else {
                row!(
                    Space::new().width(Length::Fixed(20.0)),
                    button::menu(Some(lightning_icon()), "Lightning Address")
                        .on_press(Message::Menu(Menu::Connect(
                            ConnectSubMenu::LightningAddress,
                        )))
                        .width(iced::Length::Fill),
                )
                .width(Length::Fill)
            };

        let connect_avatar_button = if matches!(menu, Menu::Connect(ConnectSubMenu::Avatar)) {
            row!(
                Space::new().width(Length::Fixed(20.0)),
                button::menu_active(Some(coins_icon()), "Avatar")
                    .on_press(Message::Reload)
                    .width(iced::Length::Fill),
                menu_bar_highlight()
            )
            .width(Length::Fill)
        } else {
            row!(
                Space::new().width(Length::Fixed(20.0)),
                button::menu(Some(coins_icon()), "Avatar")
                    .on_press(Message::Menu(Menu::Connect(ConnectSubMenu::Avatar)))
                    .width(iced::Length::Fill),
            )
            .width(Length::Fill)
        };

        let connect_contacts_button = if matches!(menu, Menu::Connect(ConnectSubMenu::Contacts)) {
            row!(
                Space::new().width(Length::Fixed(20.0)),
                button::menu_active(Some(person_icon()), "Contacts")
                    .on_press(Message::Reload)
                    .width(iced::Length::Fill),
                menu_bar_highlight()
            )
            .width(Length::Fill)
        } else {
            row!(
                Space::new().width(Length::Fixed(20.0)),
                button::menu(Some(person_icon()), "Contacts")
                    .on_press(Message::Menu(Menu::Connect(ConnectSubMenu::Contacts)))
                    .width(iced::Length::Fill),
            )
            .width(Length::Fill)
        };

        let connect_invites_button = if matches!(menu, Menu::Connect(ConnectSubMenu::Invites)) {
            row!(
                Space::new().width(Length::Fixed(20.0)),
                button::menu_active(Some(plus_icon()), "Invites")
                    .on_press(Message::Reload)
                    .width(iced::Length::Fill),
                menu_bar_highlight()
            )
            .width(Length::Fill)
        } else {
            row!(
                Space::new().width(Length::Fixed(20.0)),
                button::menu(Some(plus_icon()), "Invites")
                    .on_press(Message::Menu(Menu::Connect(ConnectSubMenu::Invites)))
                    .width(iced::Length::Fill),
            )
            .width(Length::Fill)
        };

        menu_column = menu_column
            .push(connect_ln_address_button)
            .push(connect_avatar_button)
            .push(connect_contacts_button)
            .push(connect_invites_button);
    }

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

    let theme_toggle_btn =
        coincube_ui::image::theme_toggle_button(cache.theme_mode, Message::ToggleTheme);

    Container::new(
        Column::new()
            .push(menu_column.height(Length::Fill))
            .push(
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
            )
            .push(
                Container::new(theme_toggle_btn)
                    .padding(iced::Padding {
                        top: 4.0,
                        right: 8.0,
                        bottom: 16.0,
                        left: 8.0,
                    })
                    .center_x(Length::Fill),
            ),
    )
    .style(theme::container::foreground)
}

pub fn dashboard<'a, T: Into<Element<'a, Message>>>(
    menu: &'a Menu,
    cache: &'a Cache,
    content: T,
) -> Element<'a, Message> {
    dashboard_with_info(
        menu,
        cache,
        content,
        &cache.cube_name,
        None,
        cache.lightning_address.as_deref(),
    )
}

pub fn dashboard_with_info<'a, T: Into<Element<'a, Message>>>(
    menu: &'a Menu,
    cache: &'a Cache,
    content: T,
    cube_name: &'a str,
    avatar_handle: Option<&'a iced::widget::image::Handle>,
    lightning_address: Option<&'a str>,
) -> Element<'a, Message> {
    let has_vault = cache.has_vault;
    let has_p2p = cache.has_p2p;
    let show_backup_warning = !cache.current_cube_backed_up
        && !cache.current_cube_is_passkey
        && !cache.backup_warning_dismissed;
    Row::new()
        .push(
            sidebar(
                menu,
                cache,
                has_vault,
                cube_name,
                avatar_handle,
                lightning_address,
                has_p2p,
            )
            .height(Length::Fill)
            .width(Length::Fixed(190.0)),
        )
        .push(
            Column::new()
                .push(warn(None))
                .push_maybe(show_backup_warning.then(backup_warning_banner))
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
        .style(|t| container::Style {
            background: Some(iced::Background::Color(t.colors.cards.simple.background)),
            border: iced::Border {
                radius: 20.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}

pub fn toast_overlay<'a, I: Iterator<Item = (usize, log::Level, &'a str)>>(
    iter: I,
    theme: &coincube_ui::theme::Theme,
) -> coincube_ui::widget::Element<'a, Message> {
    use coincube_ui::{color, component::text, icon::cross_icon, theme::notification};

    // Color mapping for toast levels using the theme
    let toast = |id: usize, level: log::Level, content: &'a str| {
        let content_owned = content.to_string();
        const WIDGET_HEIGHT: u32 = 80;

        // Use theme palette for the toast background
        let palette = notification::palette_for_level(&level, theme);
        let bg_color = palette.background;
        let border_color = palette.border.unwrap_or(palette.background);
        let text_color = palette.text.unwrap_or(color::WHITE);

        let bg = iced::Background::Color(bg_color);
        let border = iced::Border {
            width: 1.0,
            color: border_color,
            radius: 25.0.into(),
        };

        let inner = iced::widget::row![
            container(text::p1_bold(content_owned).color(text_color))
                .width(600)
                .height(WIDGET_HEIGHT)
                .padding(15)
                .align_y(iced::Alignment::Center),
            iced::widget::Button::new(
                cross_icon()
                    .color(text_color)
                    .size(36)
                    .align_x(iced::Alignment::Center)
                    .align_y(iced::Alignment::Center)
                    .height(iced::Length::Fill)
            )
            .height(WIDGET_HEIGHT)
            .width(60)
            .style(move |_, status| {
                let base = iced::widget::button::Style::default();
                match status {
                    iced::widget::button::Status::Hovered => base.with_background(iced::Color {
                        a: 0.2,
                        ..color::BLACK
                    }),
                    _ => base,
                }
            })
            .on_press(Message::DismissToast(id))
        ];

        // Wrap the entire row in a single styled container so the close
        // button sits inside the rounded rectangle. clip(true) ensures
        // the hover highlight respects the border radius.
        container(inner)
            .style(move |_| {
                iced::widget::container::Style::default()
                    .background(bg)
                    .border(border)
            })
            .clip(true)
    };

    let centered = iced::widget::row![
        // offset the toast by the space covered by the dashboard
        iced::widget::Space::new().width(190.0),
        // center toasts horizontally
        iced::widget::Space::new().width(iced::Length::Fill),
        iced::widget::Column::from_iter(
            iter.map(|(id, level, content)| toast(id, level, content).into())
        )
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
