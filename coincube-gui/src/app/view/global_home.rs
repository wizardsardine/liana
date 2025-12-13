use coincube_ui::{
    color,
    component::{amount::*, button, form, text::*},
    icon::{
        arrow_down_up_icon, arrow_right, eye_outline_icon, eye_slash_icon, lightning_icon,
        vault_icon,
    },
    theme,
    widget::*,
};
use iced::{
    widget::{Button, Column, Space, Stack},
    Alignment, Length,
};

use crate::app::{
    menu::Menu,
    view::{vault::receive::address_card, vault::warning::warn, FiatAmountConverter, HomeMessage},
};
use crate::app::{
    menu::{ActiveSubMenu, VaultSubMenu},
    view::message::Message,
};
use coincube_core::miniscript::bitcoin::Amount;

#[derive(Clone, Copy, Debug)]
enum WalletType {
    Active,
    Vault,
}

fn wallet_card<'a>(
    wallet_type: WalletType,
    balance: &Amount,
    fiat_converter: Option<FiatAmountConverter>,
    balance_masked: bool,
    has_vault: bool,
) -> Element<'a, Message> {
    let fiat_balance = fiat_converter.as_ref().map(|c| c.convert(*balance));

    let (icon, title, title_color, send_action, receive_action) = match wallet_type {
        WalletType::Active => (
            lightning_icon().color(color::ORANGE),
            "Active",
            Some(color::ORANGE),
            Message::Menu(Menu::Active(ActiveSubMenu::Send)),
            Message::Menu(Menu::Active(ActiveSubMenu::Receive)),
        ),
        WalletType::Vault => (
            vault_icon(),
            "Vault",
            None,
            Message::Menu(Menu::Vault(VaultSubMenu::Send)),
            Message::Menu(Menu::Vault(VaultSubMenu::Receive)),
        ),
    };

    let content = match wallet_type {
        WalletType::Vault if !has_vault => Column::new().spacing(12).push(
            Row::new()
                .spacing(8)
                .align_y(Alignment::Center)
                .push(vault_icon())
                .push(text("Vault").size(14))
                .push(
                    Row::new()
                        .align_y(Alignment::Center)
                        .push(Space::new().width(Length::Fill))
                        .push(
                            button::primary(None, "Create Vault")
                                .width(Length::Fixed(160.0))
                                .on_press(Message::SetupVault),
                        ),
                ),
        ),
        _ => Column::new()
            .spacing(12)
            .push(
                Row::new()
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .push(icon.size(16))
                    .push(
                        text(title)
                            .color(title_color.unwrap_or(color::GREY_2))
                            .size(14),
                    ),
            )
            .push(
                Row::new()
                    .align_y(Alignment::Center)
                    .push(
                        Column::new()
                            .spacing(4)
                            .push(if balance_masked {
                                Row::new().push(text("********").size(H2_SIZE))
                            } else {
                                amount_with_size(balance, H2_SIZE)
                            })
                            .push_maybe(if balance_masked {
                                Some(text("********").size(P1_SIZE))
                            } else {
                                fiat_balance
                                    .map(|fiat| fiat.to_text().size(P1_SIZE).color(color::GREY_2))
                            }),
                    )
                    .push(Space::new().width(Length::Fill))
                    .push(
                        button::primary(None, "Send")
                            .width(Length::Fixed(120.0))
                            .on_press(send_action),
                    )
                    .push(Space::new().width(Length::Fixed(8.0)))
                    .push(
                        button::secondary(None, "Receive")
                            .style(|_t, _s| iced::widget::button::Style {
                                text_color: color::ORANGE,
                                border: iced::Border {
                                    color: color::ORANGE,
                                    width: 1.0,
                                    radius: 25.0.into(),
                                },
                                ..Default::default()
                            })
                            .width(Length::Fixed(120.0))
                            .on_press(receive_action),
                    ),
            ),
    };

    Container::new(content)
        .padding(20)
        .style(move |t| match wallet_type {
            WalletType::Active => iced::widget::container::Style {
                border: iced::Border {
                    color: color::ORANGE,
                    width: 0.2,
                    radius: 25.0.into(),
                },
                background: Some(iced::Background::Color(color::GREY_6)),
                ..Default::default()
            },
            WalletType::Vault => theme::card::simple(t),
        })
        .into()
}

fn transfer_direction_card<'a>(
    title: &str,
    description: &str,
    direction: TransferDirection,
    is_selected: bool,
) -> Element<'a, Message> {
    Container::new(
        Column::new().push(
            Button::new(
                Column::new()
                    .width(Length::Fill)
                    .align_x(Alignment::Center)
                    .push(
                        text(title)
                            .bold()
                            .style(theme::text::primary)
                            .size(P1_SIZE)
                            .align_x(Alignment::Center),
                    )
                    .push(
                        text(description)
                            .style(theme::text::secondary)
                            .align_x(Alignment::Center),
                    ),
            )
            .padding(20)
            .width(Length::Fill)
            .style(move |t, s| {
                if is_selected {
                    iced::widget::button::Style {
                        border: iced::Border {
                            color: color::ORANGE,
                            width: 1.0,
                            radius: 25.0.into(),
                        },
                        ..Default::default()
                    }
                } else {
                    theme::button::secondary(t, s)
                }
            })
            .on_press(Message::Home(HomeMessage::SelectTransferDirection(
                direction,
            ))),
        ),
    )
    .width(Length::Fill)
    .style(theme::card::simple)
    .into()
}

fn select_transfer_direction_view<'a>(
    direction: Option<TransferDirection>,
) -> Element<'a, Message> {
    let content =
        Column::new()
            .width(Length::Fill)
            .push(Space::new().height(Length::Fixed(60.0)))
            .push(
                button::secondary(None, "< Previous")
                    .width(Length::Fixed(150.0))
                    .on_press(Message::Home(HomeMessage::PreviousStep)),
            )
            .push(Space::new().height(Length::Fixed(20.0)))
            .push(
                Container::new(
                    Column::new()
                        .push(
                            Column::new()
                                .spacing(10)
                                .push(text("Transfer Between Wallets").bold().size(H2_SIZE))
                                .push(text("How do you want to move your funds?").size(P1_SIZE))
                                .align_x(Alignment::Center)
                                .width(Length::Fill),
                        )
                        .spacing(60)
                        .push(
                            Column::new()
                                .spacing(20)
                                .push(transfer_direction_card(
                                    "From Active to Vault",
                                    "Move funds into your secure Vault Wallet.",
                                    TransferDirection::ActiveToVault,
                                    matches!(direction, Some(TransferDirection::ActiveToVault)),
                                ))
                                .push(transfer_direction_card(
                                    "From Vault to Active",
                                    "Move funds back into your Active Wallet.",
                                    TransferDirection::VaultToActive,
                                    matches!(direction, Some(TransferDirection::VaultToActive)),
                                ))
                                .width(Length::Fill),
                        )
                        .push(button::primary(None, "Continue").on_press_maybe(
                            direction.map(|_dir| Message::Home(HomeMessage::NextStep)),
                        ))
                        .height(Length::Fixed(800.0))
                        .width(Length::Fixed(600.0))
                        .align_x(Alignment::Center),
                )
                .width(Length::Fill)
                .center_x(Length::Fill),
            );

    Container::new(content)
        .width(Length::Fill)
        .height(Length::Fixed(800.0))
        .center_y(Length::Fixed(800.0))
        .into()
}

fn balance_summary_card<'a>(
    wallet_name: &str,
    is_active: bool,
    balance: &Amount,
    fiat_converter: Option<FiatAmountConverter>,
) -> Element<'a, Message> {
    let fiat_balance = fiat_converter.as_ref().map(|c| c.convert(*balance));

    let (icon, title_color) = if is_active {
        (lightning_icon().color(color::ORANGE), Some(color::ORANGE))
    } else {
        (vault_icon(), None)
    };

    let content = Column::new()
        .spacing(12)
        .push(
            Row::new()
                .spacing(8)
                .align_y(Alignment::Center)
                .push(icon.size(16))
                .push(
                    text(wallet_name)
                        .color(title_color.unwrap_or(color::GREY_2))
                        .size(14),
                ),
        )
        .push(
            Row::new().align_y(Alignment::Center).push(
                Column::new()
                    .spacing(4)
                    .push(amount_with_size(balance, H2_SIZE))
                    .push_maybe(
                        fiat_balance.map(|fiat| fiat.to_text().size(P1_SIZE).color(color::GREY_2)),
                    ),
            ),
        );

    Container::new(content)
        .padding(20)
        .width(Length::Fill)
        .style(move |t| {
            if is_active {
                iced::widget::container::Style {
                    border: iced::Border {
                        color: color::ORANGE,
                        width: 0.2,
                        radius: 25.0.into(),
                    },
                    background: Some(iced::Background::Color(color::GREY_6)),
                    ..Default::default()
                }
            } else {
                theme::card::simple(t)
            }
        })
        .into()
}

fn enter_amount_card<'a>(
    direction: TransferDirection,
    amount: &'a form::Value<String>,
) -> Element<'a, Message> {
    let content = Column::new()
        .push(text("Enter Amount").bold().size(H2_SIZE))
        .push(Space::new().height(Length::Fixed(10.0)))
        .push(
            Row::new()
                .spacing(4)
                .push(text("Sending from"))
                .push(text(direction.display()).bold()),
        )
        .push(Space::new().height(Length::Fixed(80.0)))
        .push(
            Column::new()
                .push(Container::new(
                    form::Form::new_amount_btc("Amount in BTC", amount, |msg| {
                        Message::Home(HomeMessage::AmountEdited(msg))
                    })
                    .warning("Please enter an amount")
                    .size(20)
                    .padding(10),
                ))
                .push(
                    button::primary(None, "Next").on_press_maybe(if amount.value.is_empty() {
                        None
                    } else {
                        Some(Message::Home(HomeMessage::NextStep))
                    }),
                )
                .spacing(80)
                .width(Length::Fixed(460.0)),
        )
        .width(Length::Fill)
        .align_x(Alignment::Center);

    Container::new(content)
        .padding([40, 20])
        .height(Length::Fixed(400.0))
        .width(Length::Fill)
        .style(theme::card::simple)
        .into()
}

fn enter_amount_view<'a>(
    direction: TransferDirection,
    active_balance: &Amount,
    vault_balance: &Amount,
    fiat_converter: Option<FiatAmountConverter>,
    entered_amount: &'a form::Value<String>,
) -> Element<'a, Message> {
    let (from_balance, to_balance, from_name, to_name) = match direction {
        TransferDirection::ActiveToVault => (active_balance, vault_balance, "Active", "Vault"),
        TransferDirection::VaultToActive => (vault_balance, active_balance, "Vault", "Active"),
    };

    let cards_row = match direction {
        TransferDirection::ActiveToVault => Row::new()
            .spacing(20)
            .push(balance_summary_card(
                from_name,
                true,
                from_balance,
                fiat_converter,
            ))
            .push(balance_summary_card(
                to_name,
                false,
                to_balance,
                fiat_converter,
            )),
        TransferDirection::VaultToActive => Row::new()
            .spacing(20)
            .push(balance_summary_card(
                from_name,
                false,
                from_balance,
                fiat_converter,
            ))
            .push(balance_summary_card(
                to_name,
                true,
                to_balance,
                fiat_converter,
            )),
    };

    let content = Column::new()
        .push(Space::new().height(Length::Fixed(60.0)))
        .spacing(20)
        .push(
            Column::new()
                .push(
                    Row::new()
                        .push(
                            button::secondary(None, "< Previous")
                                .width(Length::Fixed(150.0))
                                .on_press(Message::Home(HomeMessage::PreviousStep)),
                        )
                        .push(
                            text("Transfer Between Wallets")
                                .bold()
                                .size(H2_SIZE)
                                .width(Length::Fill)
                                .align_x(Alignment::Center),
                        )
                        .push(Space::new().width(Length::Fixed(150.0))),
                )
                .width(Length::Fill),
        )
        .push(
            Stack::new().push(cards_row).push(
                Container::new(
                    Container::new(
                        arrow_right()
                            .width(Length::Fill)
                            .height(Length::Fill)
                            .align_x(Alignment::Center)
                            .align_y(Alignment::Center),
                    )
                    .style(|_| iced::widget::container::Style {
                        border: iced::Border {
                            color: color::ORANGE,
                            radius: 30.0.into(),
                            width: 0.5,
                        },
                        background: Some(iced::Background::Color(color::GREY_6)),
                        text_color: Some(color::ORANGE),
                        ..Default::default()
                    })
                    .height(Length::Fixed(40.0))
                    .width(Length::Fixed(40.0)),
                )
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(Alignment::Center)
                .align_y(Alignment::Center),
            ),
        )
        .push(enter_amount_card(direction, entered_amount))
        .padding(20)
        .width(Length::Fill)
        .align_x(Alignment::Center);

    Container::new(content)
        .width(Length::Fill)
        .center_x(Length::Fill)
        .into()
}

fn confirm_transfer_view<'a>(
    direction: TransferDirection,
    amount: &'a form::Value<String>,
    receive_address: Option<&'a coincube_core::miniscript::bitcoin::Address>,
    labels: &'a std::collections::HashMap<String, String>,
    labels_editing: &'a std::collections::HashMap<String, form::Value<String>>,
    address_expanded: bool,
    warning: Option<&'a crate::app::error::Error>,
) -> Element<'a, Message> {
    const NUM_ADDR_CHARS: usize = 16;

    let content = Column::new()
        .width(Length::Fill)
        .push(Space::new().height(Length::Fixed(60.0)))
        .push(
            button::secondary(None, "< Previous")
                .width(Length::Fixed(150.0))
                .on_press(Message::Home(HomeMessage::PreviousStep)),
        )
        .push(Space::new().height(Length::Fixed(20.0)))
        .push_maybe(warning.map(|w| warn(Some(w))))
        .push(Container::new(
            Column::new()
                .push(
                    Column::new()
                        .spacing(10)
                        .push(text("Confirm Transfer").bold().size(H2_SIZE))
                        .push(
                            Row::new()
                                .spacing(4)
                                .push(text("Sending from"))
                                .push(text(direction.display()).bold()),
                        )
                        .align_x(Alignment::Center)
                        .width(Length::Fill),
                )
                .push(Space::new().height(60))
                .push_maybe(match direction {
                    TransferDirection::ActiveToVault => Some(
                        Column::new()
                            .spacing(10)
                            .push(
                                text("Receiving Address")
                                    .bold()
                                    .width(Length::Fill)
                                    .align_x(Alignment::Center),
                            )
                            .push_maybe(receive_address.map(|addr| -> Element<'a, Message> {
                                if address_expanded {
                                    Button::new(address_card(0, addr, labels, labels_editing))
                                        .padding(0)
                                        .on_press(Message::SelectAddress(addr.clone()))
                                        .style(theme::button::transparent_border)
                                        .into()
                                } else {
                                    let addr_str = addr.to_string();
                                    let addr_len = addr_str.chars().count();

                                    Container::new(
                                        Button::new(
                                            Row::new()
                                                .spacing(10)
                                                .push(
                                                    Container::new(
                                                        p2_regular(
                                                            if addr_len > 2 * NUM_ADDR_CHARS {
                                                                format!(
                                                                    "{}...{}",
                                                                    addr_str
                                                                        .chars()
                                                                        .take(NUM_ADDR_CHARS)
                                                                        .collect::<String>(),
                                                                    addr_str
                                                                        .chars()
                                                                        .skip(
                                                                            addr_len
                                                                                - NUM_ADDR_CHARS
                                                                        )
                                                                        .collect::<String>(),
                                                                )
                                                            } else {
                                                                addr_str.clone()
                                                            },
                                                        )
                                                        .small()
                                                        .style(theme::text::secondary),
                                                    )
                                                    .padding(10)
                                                    .width(Length::Fixed(350.0)),
                                                )
                                                .push(
                                                    Container::new(
                                                        text(
                                                            labels
                                                                .get(&addr_str)
                                                                .cloned()
                                                                .unwrap_or_default(),
                                                        )
                                                        .small()
                                                        .style(theme::text::secondary),
                                                    )
                                                    .padding(10)
                                                    .width(Length::Fill),
                                                )
                                                .align_y(Alignment::Center),
                                        )
                                        .on_press(Message::SelectAddress(addr.clone()))
                                        .padding(20)
                                        .width(Length::Fill)
                                        .style(theme::button::secondary),
                                    )
                                    .style(theme::card::simple)
                                    .into()
                                }
                            }))
                            .push_maybe(receive_address.is_none().then(|| {
                                text("No receiving address available. Please generate one first.")
                                    .style(theme::text::secondary)
                            })),
                    ),
                    TransferDirection::VaultToActive => {
                        // TODO: This should be implemented once Active Wallet is done
                        Some(
                            Column::new()
                                .spacing(10)
                                .push(text("Receiving Wallet").bold())
                                .push(
                                    text("Transferring to Active wallet")
                                        .style(theme::text::secondary),
                                )
                                .push(
                                    text("(Active wallet address generation not yet implemented)")
                                        .size(12)
                                        .style(theme::text::secondary),
                                ),
                        )
                    }
                }),
        ))
        .push(Space::new().height(Length::Fixed(20.0)))
        .push(
            Container::new(
                Row::new()
                    .padding(20)
                    .push(text("Amount:"))
                    .push(Space::new().width(Length::Fill))
                    .push(text(&amount.value))
                    .push(Space::new().width(4))
                    .push(text("BTC")),
            )
            .width(Length::Fill)
            .style(theme::card::simple),
        )
        .push(Space::new().height(Length::Fixed(60.0)))
        .push(
            button::primary(None, "Confirm Transfer")
                .on_press(Message::Home(HomeMessage::ConfirmTransfer)),
        );

    Container::new(content)
        .width(Length::Fill)
        .center_x(Length::Fill)
        .into()
}

#[derive(Clone, Copy, Debug)]
pub enum TransferDirection {
    ActiveToVault,
    VaultToActive,
}

impl TransferDirection {
    pub fn from_wallet(&self) -> &'static str {
        match self {
            Self::ActiveToVault => "Active",
            Self::VaultToActive => "Vault",
        }
    }

    pub fn to_wallet(&self) -> &'static str {
        match self {
            Self::ActiveToVault => "Vault",
            Self::VaultToActive => "Active",
        }
    }

    pub fn display(&self) -> String {
        format!("{} → {}", self.from_wallet(), self.to_wallet())
    }
}

pub struct GlobalViewConfig<'a> {
    pub active_balance: Amount,
    pub vault_balance: Amount,
    pub fiat_converter: Option<FiatAmountConverter>,
    pub balance_masked: bool,
    pub has_vault: bool,
    pub current_view: HomeView,
    pub transfer_direction: Option<TransferDirection>,
    pub entered_amount: &'a form::Value<String>,
    pub receive_address: Option<&'a coincube_core::miniscript::bitcoin::Address>,
    pub receive_index: Option<&'a coincube_core::miniscript::bitcoin::bip32::ChildNumber>,
    pub labels: &'a std::collections::HashMap<String, String>,
    pub labels_editing: &'a std::collections::HashMap<String, form::Value<String>>,
    pub address_expanded: bool,
    pub warning: Option<&'a crate::app::error::Error>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HomeView {
    pub step: usize,
}

impl HomeView {
    pub fn next(&mut self) {
        self.step += 1;
    }

    pub fn previous(&mut self) {
        if self.step > 0 {
            self.step -= 1;
        }
    }

    pub fn reset(&mut self) {
        self.step = 0;
    }
}

pub fn global_home_view<'a>(config: GlobalViewConfig<'a>) -> Element<'a, Message> {
    let GlobalViewConfig {
        active_balance,
        vault_balance,
        fiat_converter,
        balance_masked,
        has_vault,
        current_view,
        transfer_direction,
        entered_amount,
        receive_address,
        receive_index: _receive_index,
        labels,
        labels_editing,
        address_expanded,
        warning,
    } = config;

    match current_view.step {
        1 => {
            return select_transfer_direction_view(transfer_direction);
        }
        2 => {
            if let Some(direction) = transfer_direction {
                return enter_amount_view(
                    direction,
                    &active_balance,
                    &vault_balance,
                    fiat_converter,
                    entered_amount,
                );
            }
        }
        3 => {
            if let Some(direction) = transfer_direction {
                return confirm_transfer_view(
                    direction,
                    entered_amount,
                    receive_address,
                    labels,
                    labels_editing,
                    address_expanded,
                    warning,
                );
            }
        }
        0 => {}
        _ => {}
    }

    let active_card = wallet_card(
        WalletType::Active,
        &active_balance,
        fiat_converter,
        balance_masked,
        false,
    );

    let vault_card_element = wallet_card(
        WalletType::Vault,
        &vault_balance,
        fiat_converter,
        balance_masked,
        has_vault,
    );

    Column::new()
        .spacing(20)
        .push(
            Row::new()
                .spacing(0)
                .width(Length::Fill)
                .push(h3("Wallets"))
                .push(
                    Button::new(if balance_masked {
                        eye_slash_icon()
                    } else {
                        eye_outline_icon()
                    })
                    .style(theme::button::container)
                    .on_press(Message::Home(HomeMessage::ToggleBalanceMask)),
                )
                .align_y(Alignment::Center),
        )
        .push(
            Stack::new()
                .push(
                    Column::new()
                        .spacing(40)
                        .push(active_card)
                        .push(vault_card_element),
                )
                .push(
                    Container::new(
                        button::secondary(Some(arrow_down_up_icon()), "Transfer")
                            .style(|_t, _s| iced::widget::button::Style {
                                text_color: color::ORANGE,
                                border: iced::Border {
                                    color: color::ORANGE,
                                    width: 1.0,
                                    radius: 35.0.into(),
                                },
                                background: Some(iced::Background::Color(color::GREY_6)),
                                ..Default::default()
                            })
                            .height(Length::Fixed(60.0))
                            .width(Length::Fixed(150.0))
                            .padding(iced::Padding::from([15, 0]))
                            .on_press(Message::Home(HomeMessage::NextStep)),
                    )
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(Alignment::Center)
                    .align_y(Alignment::Center),
                ),
        )
        .into()
}
