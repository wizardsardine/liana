use breez_sdk_liquid::InputType;
use coincube_core::miniscript::bitcoin::Amount;
use coincube_ui::locale::copy as localized_copy;
use coincube_ui::locale::Text as CopyText;
use coincube_ui::localized_format;
use coincube_ui::{
    color,
    component::{
        amount::*,
        button, form,
        tenshu::{segment, segmented},
        text::*,
        transaction::{TransactionDirection, TransactionListItem},
    },
    icon::{self, receipt_icon},
    theme,
    widget::{action::Action, ColumnExt, Element, RowExt},
};
use iced::{
    widget::{button as iced_button, container, Column, Container, Row, Space},
    Alignment, Background, Length,
};

use crate::app::breez_liquid::assets::{format_usdt_display, AssetKind};
use crate::app::menu::Menu;
use crate::app::state::liquid::send::{
    LiquidSendFlowState, Modal, PreparedIntent, PreparedPayment, ReceiveNetwork, SendAsset,
};
use crate::app::view::{
    self, vault::fiat::FiatAmount, FiatAmountConverter, LiquidSendMessage, Message,
};
use crate::app::wallets::{DomainPaymentDetails, DomainPaymentStatus};
use crate::{app::cache::Cache, loading::loading_indicator};
use coincube_ui::image::asset_network_logo;

pub struct LiquidSendFlowConfig<'a> {
    pub flow_state: &'a LiquidSendFlowState,
    pub btc_balance: Amount,
    pub usdt_balance: u64,
    pub fiat_converter: Option<FiatAmountConverter>,
    pub recent_transaction: &'a Vec<RecentTransaction>,
    pub input: &'a form::Value<String>,
    pub amount_input: &'a form::Value<String>,
    pub usdt_amount_input: &'a form::Value<String>,
    pub to_asset: SendAsset,
    pub from_asset: SendAsset,
    pub receive_network: ReceiveNetwork,
    pub send_picker_open: bool,
    pub receive_picker_open: bool,
    pub uri_asset: Option<AssetKind>,
    pub usdt_asset_id: &'a str,
    pub comment: String,
    pub description: Option<&'a str>,
    pub lightning_limits: Option<(u64, u64)>,
    pub amount: Amount,
    pub is_sending: bool,
    pub menu: &'a Menu,
    pub cache: &'a Cache,
    pub input_type: &'a Option<InputType>,
    pub onchain_limits: Option<(u64, u64)>,
    pub bitcoin_unit: BitcoinDisplayUnit,
    pub error: Option<&'a str>,
    pub cross_asset_supported: bool,
    pub pay_fees_with_asset: bool,
    pub max_loading: bool,
    pub sent_celebration_context: &'a str,
    pub sent_amount_display: &'a str,
    pub sent_quote: &'a coincube_ui::component::quote_display::Quote,
    pub sent_image_handle: &'a iced::widget::image::Handle,
}

fn send_payment_card_style(theme: &theme::Theme) -> container::Style {
    let shell = &theme.colors.shell;
    container::Style {
        background: Some(Background::Color(shell.panel)),
        text_color: Some(shell.ink),
        border: iced::Border {
            color: shell.line,
            width: 1.0,
            radius: 16.0.into(),
        },
        ..Default::default()
    }
}

fn send_route_action_style(
    theme: &theme::Theme,
    status: iced_button::Status,
) -> iced_button::Style {
    let shell = &theme.colors.shell;
    iced_button::Style {
        background: Some(Background::Color(shell.background)),
        text_color: shell.ink,
        border: iced::Border {
            color: if matches!(
                status,
                iced_button::Status::Hovered | iced_button::Status::Pressed
            ) {
                shell.accent_ink
            } else {
                shell.line
            },
            width: 1.0,
            radius: 8.0.into(),
        },
        ..Default::default()
    }
}

const CONTINUE_ACTION_ID: &str = "tenshu.liquid.send.continue";
const BREADCRUMB_ACTION_ID: &str = "tenshu.liquid.send.breadcrumb";
pub(crate) const SEND_ROUTE_ACTION_ID: &str = "tenshu.liquid.send.source-route";
pub(crate) const RECEIVE_ROUTE_ACTION_ID: &str = "tenshu.liquid.send.destination-route";

pub(crate) fn send_asset_action_id(asset: SendAsset) -> &'static str {
    match asset {
        SendAsset::Lbtc => "tenshu.liquid.send.source-lbtc",
        SendAsset::Usdt => "tenshu.liquid.send.source-usdt",
    }
}

pub(crate) fn receive_target_action_id(asset: SendAsset, network: ReceiveNetwork) -> &'static str {
    match (asset, network) {
        (SendAsset::Lbtc, ReceiveNetwork::Lightning) => "tenshu.liquid.send.target-lightning",
        (SendAsset::Lbtc, ReceiveNetwork::Liquid) => "tenshu.liquid.send.target-liquid-btc",
        (SendAsset::Lbtc, ReceiveNetwork::Bitcoin) => "tenshu.liquid.send.target-bitcoin",
        (SendAsset::Usdt, ReceiveNetwork::Liquid) => "tenshu.liquid.send.target-liquid-usdt",
        (SendAsset::Usdt, ReceiveNetwork::Ethereum) => "tenshu.liquid.send.target-ethereum",
        (SendAsset::Usdt, ReceiveNetwork::Tron) => "tenshu.liquid.send.target-tron",
        (SendAsset::Usdt, ReceiveNetwork::Binance) => "tenshu.liquid.send.target-binance",
        (SendAsset::Usdt, ReceiveNetwork::Solana) => "tenshu.liquid.send.target-solana",
        _ => "tenshu.liquid.send.target-unsupported",
    }
}

pub fn liquid_send_with_flow<'a>(config: LiquidSendFlowConfig<'a>) -> Element<'a, Message> {
    let base_content = match config.flow_state {
        LiquidSendFlowState::Main { modal } => {
            let send_view = liquid_send_view(
                config.btc_balance,
                config.usdt_balance,
                config.from_asset,
                config.to_asset,
                config.receive_network,
                config.fiat_converter,
                config.recent_transaction,
                config.input,
                config.input_type,
                config.bitcoin_unit,
                config.usdt_asset_id,
                config.cache.show_direction_badges,
                config.cache.balance_amounts_masked(),
            )
            .map(Message::LiquidSend);

            let send_page = send_page(
                send_view,
                config.cache,
                config.btc_balance,
                config.usdt_balance,
                config.from_asset,
            );
            // Liquid is hidden (plan L2); its pages keep their in-body titles.
            let content = view::dashboard_payment(config.menu, config.cache, None, send_page);

            // Show picker modals if open
            if config.send_picker_open {
                let modal_content = send_picker_modal(
                    config.btc_balance,
                    config.usdt_balance,
                    config.from_asset,
                    config.bitcoin_unit,
                    config.cache.balance_amounts_masked(),
                )
                .map(Message::LiquidSend);
                return coincube_ui::widget::modal::Modal::new(content, modal_content)
                    .on_blur(Some(Message::LiquidSend(LiquidSendMessage::ClosePicker)))
                    .into();
            }

            if config.receive_picker_open {
                let modal_content = receive_picker_modal(
                    config.from_asset,
                    config.to_asset,
                    config.receive_network,
                    config.cross_asset_supported,
                )
                .map(Message::LiquidSend);
                return coincube_ui::widget::modal::Modal::new(content, modal_content)
                    .on_blur(Some(Message::LiquidSend(LiquidSendMessage::ClosePicker)))
                    .into();
            }

            // Show amount modal if needed
            match modal {
                Modal::AmountInput => {
                    let modal_content = amount_input_model(AmountInputConfig {
                        amount: config.amount_input,
                        usdt_amount_input: config.usdt_amount_input,
                        to_asset: config.to_asset,
                        from_asset: config.from_asset,
                        uri_asset: config.uri_asset,
                        usdt_balance: config.usdt_balance,
                        comment: config.comment,
                        has_fiat_converter: config.fiat_converter.is_some(),
                        btc_balance: config.btc_balance,
                        description: config.description,
                        lightning_limits: config.lightning_limits,
                        onchain_limits: config.onchain_limits,
                        input_type: config.input_type,
                        bitcoin_unit: config.bitcoin_unit,
                        error: config.error,
                        cross_asset_supported: config.cross_asset_supported,
                        pay_fees_with_asset: config.pay_fees_with_asset,
                        max_loading: config.max_loading,
                    })
                    .map(Message::LiquidSend);
                    coincube_ui::widget::modal::Modal::new(content, modal_content)
                        .on_blur(Some(Message::LiquidSend(LiquidSendMessage::PopupMessage(
                            view::SendPopupMessage::Close,
                        ))))
                        .into()
                }
                Modal::FiatInput {
                    fiat_input,
                    currencies,
                    selected_currency,
                    converters,
                } => {
                    let modal_content = fiat_input_model(
                        fiat_input,
                        currencies,
                        selected_currency,
                        converters,
                        config.bitcoin_unit,
                    )
                    .map(Message::LiquidSend);
                    coincube_ui::widget::modal::Modal::new(content, modal_content)
                        .on_blur(Some(Message::LiquidSend(LiquidSendMessage::PopupMessage(
                            view::SendPopupMessage::FiatClose,
                        ))))
                        .into()
                }
                Modal::None => content,
            }
        }
        LiquidSendFlowState::FinalCheck(intent) => {
            // Everything on this screen comes off the intent. Nothing here reads
            // the live input fields, so what the user approves is what executes.
            let content = final_check_page(
                intent,
                config.cache.chain(),
                config.fiat_converter.as_ref(),
                config.is_sending,
                config.bitcoin_unit,
            )
            .map(Message::LiquidSend);
            view::dashboard(config.menu, config.cache, content)
        }
        LiquidSendFlowState::Sent => {
            use coincube_ui::component::payment_result::{payment_result, Outcome};
            let content = Element::from(
                payment_result(
                    // A Liquid→bitcoin swap is not final until it confirms.
                    if config.sent_celebration_context == "bitcoin-send" {
                        Outcome::Confirming
                    } else {
                        Outcome::Sent
                    },
                    config.sent_amount_display,
                    config.sent_celebration_context,
                    config.sent_quote,
                    config.sent_image_handle,
                    LiquidSendMessage::BackToHome,
                )
                .detail(localized_copy("Paid from Liquid"))
                .voice(coincube_ui::worldview_copy::VoiceLine::PaymentSent.get()),
            )
            .map(Message::LiquidSend);
            view::dashboard(config.menu, config.cache, content)
        }
    };

    base_content
}

fn send_page<'a>(
    send_view: Element<'a, Message>,
    cache: &'a Cache,
    btc_balance: Amount,
    usdt_balance: u64,
    from_asset: SendAsset,
) -> Element<'a, Message> {
    let network_name = match cache.network {
        coincube_core::miniscript::bitcoin::Network::Bitcoin => "Liquid network",
        coincube_core::miniscript::bitcoin::Network::Testnet
        | coincube_core::miniscript::bitcoin::Network::Signet
        | coincube_core::miniscript::bitcoin::Network::Regtest => "Liquid test network",
        _ => "Liquid network",
    };
    let balance = if cache.balance_amounts_masked() {
        "••••••".to_owned()
    } else {
        match from_asset {
            SendAsset::Lbtc => format!(
                "{} {}",
                btc_balance.to_formatted_string_with_unit(cache.bitcoin_unit),
                cache.bitcoin_unit.label()
            ),
            SendAsset::Usdt => format!("{} USDt", format_usdt_display(usdt_balance)),
        }
    };
    let header = Column::new()
        .spacing(4)
        .push(
            Action::new(
                BREADCRUMB_ACTION_ID,
                text(format!("‹ Liquid · {}", cache.cube_name)).color(color::ORANGE),
                |id| Message::LiquidSend(LiquidSendMessage::FocusAction(id)),
            )
            .padding([2, 0])
            .style(theme::button::transparent)
            .on_press(Message::Menu(Menu::Liquid(
                crate::app::menu::LiquidSubMenu::Overview,
            ))),
        )
        .push(h3(localized_copy("Send")).bold())
        .push(
            p2_regular(format!("{} · {}", cache.cube_name, network_name))
                .style(theme::text::secondary),
        );
    let aside = Column::new()
        .spacing(12)
        .push(liquid_send_step(
            "1",
            coincube_ui::locale::copy("Details"),
            true,
        ))
        .push(liquid_send_step("2", CopyText::Review.get(), false))
        .push(liquid_send_step(
            "3",
            coincube_ui::locale::copy("Sent"),
            false,
        ))
        .push(iced::widget::rule::horizontal(1))
        .push(p2_regular(localized_copy("Available in Liquid")).style(theme::text::secondary))
        .push(p1_bold(balance));
    let page = Column::new().spacing(20).push(header).push(
        Row::new()
            .spacing(40)
            .align_y(Alignment::Start)
            .push(Container::new(send_view).width(Length::Fixed(680.0)))
            .push(Container::new(aside).width(Length::Fixed(240.0)))
            .wrap()
            .vertical_spacing(20),
    );
    Container::new(Container::new(page).width(Length::Fill).max_width(960.0))
        .width(Length::Fill)
        .into()
}

fn liquid_send_step<'a>(
    number: &'static str,
    label: &'static str,
    active: bool,
) -> Element<'a, Message> {
    Row::new()
        .spacing(10)
        .align_y(Alignment::Center)
        .push(
            Container::new(
                text(number)
                    .size(P2_SIZE)
                    .style(move |theme: &theme::Theme| iced::widget::text::Style {
                        color: Some(if active {
                            iced::color!(0x1a1208)
                        } else {
                            theme.colors.shell.muted
                        }),
                    }),
            )
            .center_x(Length::Fixed(24.0))
            .center_y(Length::Fixed(24.0))
            .style(move |theme: &theme::Theme| container::Style {
                background: active.then_some(Background::Color(theme.colors.shell.accent)),
                border: iced::Border {
                    color: theme.colors.shell.line,
                    width: 1.0,
                    radius: 12.0.into(),
                },
                ..Default::default()
            }),
        )
        .push(if active {
            p1_bold(label)
        } else {
            p1_regular(label).style(theme::text::secondary)
        })
        .into()
}

#[allow(clippy::too_many_arguments)]
pub fn liquid_send_view<'a>(
    btc_balance: Amount,
    usdt_balance: u64,
    from_asset: SendAsset,
    to_asset: SendAsset,
    receive_network: ReceiveNetwork,
    _fiat_converter: Option<FiatAmountConverter>,
    recent_transaction: &[RecentTransaction],
    input: &'a form::Value<String>,
    input_type: &'a Option<InputType>,
    bitcoin_unit: BitcoinDisplayUnit,
    usdt_asset_id: &str,
    show_direction_badges: bool,
    mask_amounts: bool,
) -> Element<'a, LiquidSendMessage> {
    let mut content = Column::new().spacing(20);

    // ── Two-card "You Send → They Receive" layout ───────────────────────────
    let you_send_card = {
        let asset_label = match from_asset {
            SendAsset::Lbtc => "L-BTC",
            SendAsset::Usdt => "USDt",
        };
        let balance_text = if mask_amounts {
            "Balance: ••••••".into()
        } else {
            match from_asset {
                SendAsset::Lbtc => format!(
                    "Balance: {} {}",
                    btc_balance.to_formatted_string_with_unit(bitcoin_unit),
                    bitcoin_unit.label()
                ),
                SendAsset::Usdt => format!("Balance: {} USDt", format_usdt_display(usdt_balance)),
            }
        };
        let asset_slug = match from_asset {
            SendAsset::Lbtc => "lbtc",
            SendAsset::Usdt => "usdt",
        };
        let ico: Element<'_, LiquidSendMessage> = asset_network_logo(asset_slug, "liquid", 40.0);

        let card_content = Column::new()
            .spacing(6)
            .push(
                text(localized_copy("You send"))
                    .size(P2_SIZE)
                    .style(theme::text::secondary),
            )
            .push(
                Row::new()
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .push(ico)
                    .push(
                        text(asset_label)
                            .size(H3_SIZE)
                            .bold()
                            .style(theme::text::primary),
                    ),
            )
            .push(
                text(balance_text)
                    .size(P2_SIZE)
                    .style(theme::text::secondary),
            )
            .push(p2_regular(localized_copy("Liquid network")).style(theme::text::secondary));

        Action::new(
            SEND_ROUTE_ACTION_ID,
            card_content,
            LiquidSendMessage::FocusAction,
        )
        .padding(14)
        .width(Length::Fill)
        .height(Length::Fixed(132.0))
        .on_press(LiquidSendMessage::OpenSendPicker)
        .style(send_route_action_style)
    };

    let they_receive_card = {
        let asset_label = match (to_asset, receive_network) {
            (SendAsset::Lbtc, ReceiveNetwork::Lightning) => "BTC",
            (SendAsset::Lbtc, ReceiveNetwork::Bitcoin) => "BTC",
            (SendAsset::Lbtc, _) => "L-BTC",
            (SendAsset::Usdt, _) => "USDt",
        };
        let asset_slug = match to_asset {
            SendAsset::Lbtc => "btc",
            SendAsset::Usdt => "usdt",
        };
        let network_slug = match receive_network {
            ReceiveNetwork::Lightning => "lightning",
            ReceiveNetwork::Liquid => "liquid",
            ReceiveNetwork::Bitcoin => "bitcoin",
            ReceiveNetwork::Ethereum => "ethereum",
            ReceiveNetwork::Tron => "tron",
            ReceiveNetwork::Binance => "bsc",
            ReceiveNetwork::Solana => "solana",
        };
        let ico: Element<'_, LiquidSendMessage> =
            asset_network_logo(asset_slug, network_slug, 40.0);

        let network_label = receive_network.display_name();

        let card_content = Column::new()
            .spacing(6)
            .push(
                text(localized_copy("They receive"))
                    .size(P2_SIZE)
                    .style(theme::text::secondary),
            )
            .push(
                Row::new()
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .push(ico)
                    .push(
                        text(asset_label)
                            .size(H3_SIZE)
                            .bold()
                            .style(theme::text::primary),
                    ),
            )
            .push(p2_regular(network_label).style(theme::text::secondary));

        Action::new(
            RECEIVE_ROUTE_ACTION_ID,
            card_content,
            LiquidSendMessage::FocusAction,
        )
        .padding(14)
        .width(Length::Fill)
        .height(Length::Fixed(132.0))
        .on_press(LiquidSendMessage::OpenReceivePicker)
        .style(send_route_action_style)
    };

    let arrow = text("→").size(H3_SIZE).style(theme::text::secondary);

    let cards_row = Row::new()
        .spacing(12)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .push(Container::new(you_send_card).width(Length::FillPortion(1)))
        .push(arrow)
        .push(Container::new(they_receive_card).width(Length::FillPortion(1)));

    // ── Address input ────────────────────────────────────────────────────────
    let hint_text = match (to_asset, receive_network) {
        (_, ReceiveNetwork::Lightning) => "Enter Lightning Invoice or Address",
        (_, ReceiveNetwork::Bitcoin) => "Enter Bitcoin Address",
        (_, ReceiveNetwork::Liquid) if to_asset == SendAsset::Usdt => "Enter Liquid USDt Address",
        (_, ReceiveNetwork::Liquid) => "Enter Liquid Address",
        (_, net) if net.is_sideshift() => match net {
            ReceiveNetwork::Ethereum => "Enter Ethereum USDt Address",
            ReceiveNetwork::Tron => "Enter Tron USDt Address",
            ReceiveNetwork::Binance => "Enter Binance Smart Chain USDt Address",
            ReceiveNetwork::Solana => "Enter Solana USDt Address",
            _ => "Enter Address",
        },
        _ => "Enter Address",
    };

    // For SideShift sends, don't require Breez input validation — accept any text
    let can_proceed = if receive_network.is_sideshift() {
        !input.value.trim().is_empty()
    } else {
        input.valid && !input.value.trim().is_empty() && input_type.is_some()
    };

    // Match the bounded Send form used by Vault and Spark. The existing
    // validation and Send message still decide when Liquid advances to its
    // amount and review steps; this view only gives that action a readable
    // label and places it after the destination in keyboard order.
    let prepare = Action::new(
        CONTINUE_ACTION_ID,
        text(localized_copy("Continue to amount")).size(P1_SIZE),
        LiquidSendMessage::FocusAction,
    )
    .padding([10, 16])
    .style(theme::button::primary)
    .width(Length::Fixed(190.0))
    .on_press_maybe(can_proceed.then_some(LiquidSendMessage::Send));
    let payment_card = Container::new(
        Column::new()
            .push(
                Container::new(
                    Row::new()
                        .spacing(12)
                        .align_y(Alignment::Center)
                        .push(h4_bold(localized_copy("Payment details")).width(Length::Fill))
                        .push(
                            segmented(
                                [
                                    segment("liquid-send-kind-lbtc", SendAsset::Lbtc, "L-BTC"),
                                    segment("liquid-send-kind-usdt", SendAsset::Usdt, "USDt"),
                                ],
                                Some(from_asset),
                                LiquidSendMessage::FocusAction,
                            )
                            .on_select(LiquidSendMessage::SelectMasterAsset),
                        ),
                )
                .padding([16, 24])
                .width(Length::Fill),
            )
            .push(iced::widget::rule::horizontal(1))
            .push(
                Container::new(cards_row)
                    .padding([20, 24])
                    .width(Length::Fill),
            )
            .push(iced::widget::rule::horizontal(1))
            .push(
                Container::new(
                    Column::new()
                        .spacing(10)
                        .push(p1_bold(localized_copy("Destination")))
                        .push(
                            form::Form::new(hint_text, input, LiquidSendMessage::InputEdited)
                                .size(16)
                                .padding(12),
                        ),
                )
                .padding([20, 24])
                .width(Length::Fill),
            )
            .push(iced::widget::rule::horizontal(1))
            .push(
                Container::new(
                    Row::new()
                        .spacing(16)
                        .align_y(Alignment::Center)
                        .push(
                            p2_regular(if input.value.trim().is_empty() {
                                localized_copy("Enter a destination to continue.")
                            } else if can_proceed {
                                localized_copy("Next, review the amount and fees.")
                            } else {
                                localized_copy("Check the destination before continuing.")
                            })
                            .style(theme::text::secondary)
                            .width(Length::Fill),
                        )
                        .push(prepare),
                )
                .padding([16, 24])
                .width(Length::Fill),
            ),
    )
    .width(Length::Fill)
    .style(send_payment_card_style);

    content = content.push(payment_card);

    // ── Last transactions ────────────────────────────────────────────────────
    content = content.push(
        Column::new()
            .spacing(10)
            .push(h4_bold(localized_copy("Last transactions"))),
    );

    if !recent_transaction.is_empty() {
        for (idx, tx) in recent_transaction.iter().enumerate() {
            let direction = if tx.is_incoming {
                TransactionDirection::Incoming
            } else {
                TransactionDirection::Outgoing
            };

            let fiat_str = if mask_amounts || tx.usdt_display.is_some() {
                None
            } else {
                tx.fiat_amount
                    .as_ref()
                    .map(|fiat| format!("{} {}", fiat.to_formatted_string(), fiat.currency()))
            };

            let display_amount = if tx.usdt_display.is_some() {
                Amount::ZERO
            } else if tx.is_incoming {
                tx.amount
            } else {
                tx.amount + tx.fees_sat
            };

            // Determine combo icon from payment details
            let tx_icon = match &tx.details {
                DomainPaymentDetails::Lightning { .. } => {
                    asset_network_logo("btc", "lightning", 40.0)
                }
                DomainPaymentDetails::LiquidAsset { asset_id, .. }
                    if !usdt_asset_id.is_empty() && asset_id == usdt_asset_id =>
                {
                    asset_network_logo("usdt", "liquid", 40.0)
                }
                DomainPaymentDetails::LiquidAsset { .. } => {
                    asset_network_logo("lbtc", "liquid", 40.0)
                }
                DomainPaymentDetails::OnChainBitcoin { .. } => {
                    asset_network_logo("btc", "bitcoin", 40.0)
                }
            };

            let mut item = TransactionListItem::new(direction, &display_amount, bitcoin_unit)
                .with_custom_icon(tx_icon)
                .with_show_direction_badge(show_direction_badges)
                .with_label(tx.description.clone())
                .with_time_ago(tx.time_ago.clone());

            if mask_amounts {
                item = item.with_amount_override("••••••".into());
            } else if let Some(ref usdt_str) = tx.usdt_display {
                item = item.with_amount_override(usdt_str.clone());
            }

            if matches!(tx.status, DomainPaymentStatus::Pending) {
                let (bg, fg) = (color::GREY_3, color::BLACK);
                let pending_badge = Container::new(
                    Row::new()
                        .push(
                            icon::warning_icon()
                                .size(14)
                                .style(move |_| iced::widget::text::Style { color: Some(fg) }),
                        )
                        .push(
                            text(localized_copy("Pending"))
                                .bold()
                                .size(14)
                                .style(move |_| iced::widget::text::Style { color: Some(fg) }),
                        )
                        .spacing(4),
                )
                .padding([2, 8])
                .style(move |_| iced::widget::container::Style {
                    background: Some(iced::Background::Color(bg)),
                    border: iced::Border {
                        radius: 12.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                });
                item = item.with_custom_status(pending_badge.into());
            }

            if let Some(fiat) = fiat_str {
                item = item.with_fiat_amount(fiat);
            }

            content = content.push(item.view(LiquidSendMessage::SelectTransaction(idx)));
        }
    } else {
        content = content.push(coincube_ui::component::empty_placeholder(
            receipt_icon().size(80),
            coincube_ui::locale::copy("No transactions yet"),
            coincube_ui::locale::copy(
                "Your transaction history will appear here once you send or receive coins.",
            ),
        ));
    }

    let view_transaction_button = {
        let the_icon = icon::history_icon()
            .size(18)
            .style(|_theme: &theme::Theme| iced::widget::text::Style {
                color: Some(color::ORANGE),
            });

        let label = text(localized_copy("View All Transactions"))
            .size(15)
            .style(|_theme: &theme::Theme| iced::widget::text::Style {
                color: Some(color::ORANGE),
            });

        let button_content = Row::new()
            .spacing(8)
            .align_y(iced::alignment::Vertical::Center)
            .push(the_icon)
            .push(label);

        iced_button(Container::new(button_content).padding([10, 20]).style(
            |_theme: &theme::Theme| container::Style {
                background: Some(Background::Color(color::TRANSPARENT)),
                border: iced::Border {
                    color: color::ORANGE,
                    width: 1.5,
                    radius: 8.0.into(),
                },
                ..Default::default()
            },
        ))
        .style(|_theme: &theme::Theme, status| iced_button::Style {
            background: Some(Background::Color(color::TRANSPARENT)),
            text_color: match status {
                iced_button::Status::Hovered => iced::color!(0xFF9D42),
                iced_button::Status::Pressed => color::ORANGE,
                _ => color::ORANGE,
            },
            border: iced::Border {
                radius: 8.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .on_press(LiquidSendMessage::History)
    };

    if !recent_transaction.is_empty() {
        content = content
            .push(iced::widget::Space::new().height(Length::Fixed(20.0)))
            .push(
                Container::new(view_transaction_button)
                    .width(Length::Fill)
                    .center_x(Length::Fill),
            )
            .push(iced::widget::Space::new().height(Length::Fixed(40.0)));
    }

    // Bound the form and recent transactions together; the column starts at
    // the page gutter like every Cube page.
    Container::new(content.max_width(720))
        .width(Length::Fill)
        .into()
}

// ── Picker modal views ──────────────────────────────────────────────────────

/// Liquid Send's picker rows are Actions so a keyboard user can choose the
/// source asset or destination network after opening either route tile.
fn send_picker_action<'a>(
    id: &'static str,
    ico: Element<'a, LiquidSendMessage>,
    label: &'static str,
    balance: &str,
    network: &str,
    selected: bool,
    press: LiquidSendMessage,
) -> Element<'a, LiquidSendMessage> {
    let content = Row::new()
        .spacing(12)
        .align_y(Alignment::Center)
        .push(ico)
        .push(
            Column::new().spacing(2).push(p1_bold(label)).push_maybe(
                (!balance.is_empty())
                    .then(|| p2_regular(balance.to_owned()).style(theme::text::secondary)),
            ),
        )
        .push(Space::new().width(Length::Fill))
        .push(p2_regular(network.to_owned()).style(theme::text::secondary))
        .push_maybe(selected.then(|| icon::check2_icon().size(18).color(color::ORANGE)));
    Action::new(id, content, LiquidSendMessage::FocusAction)
        .width(Length::Fill)
        .padding([12, 16])
        .focus_radius(8.0)
        .style(move |theme, status| {
            let shell = &theme.colors.shell;
            iced_button::Style {
                background: Some(Background::Color(if selected {
                    shell.selected
                } else {
                    shell.background
                })),
                text_color: shell.ink,
                border: iced::Border {
                    color: if selected || matches!(status, iced_button::Status::Hovered) {
                        shell.accent_ink
                    } else {
                        shell.line
                    },
                    width: 1.0,
                    radius: 8.0.into(),
                },
                ..Default::default()
            }
        })
        .on_press(press)
        .into()
}

/// Render the "You Send" picker modal content.
pub fn send_picker_modal<'a>(
    btc_balance: Amount,
    usdt_balance: u64,
    current: SendAsset,
    bitcoin_unit: BitcoinDisplayUnit,
    mask_amounts: bool,
) -> Element<'a, LiquidSendMessage> {
    let title = text(localized_copy("YOU SEND")).size(H4_SIZE).bold();

    let lbtc_balance = if mask_amounts {
        "••••••".into()
    } else {
        btc_balance.to_formatted_string_with_unit(bitcoin_unit)
    };
    let usdt_balance = if mask_amounts {
        "••••••".into()
    } else {
        format!("{} USDt", format_usdt_display(usdt_balance))
    };

    let lbtc_row = send_picker_action(
        send_asset_action_id(SendAsset::Lbtc),
        asset_network_logo("lbtc", "liquid", 36.0),
        "L-BTC",
        &lbtc_balance,
        "Liquid",
        current == SendAsset::Lbtc,
        LiquidSendMessage::SetSendAsset(SendAsset::Lbtc),
    );

    let usdt_row = send_picker_action(
        send_asset_action_id(SendAsset::Usdt),
        asset_network_logo("usdt", "liquid", 36.0),
        "USDt",
        &usdt_balance,
        "Liquid",
        current == SendAsset::Usdt,
        LiquidSendMessage::SetSendAsset(SendAsset::Usdt),
    );

    Column::new()
        .spacing(16)
        .padding(24)
        .max_width(420)
        .push(title)
        .push(lbtc_row)
        .push(usdt_row)
        .into()
}

/// Render the "They Receive" picker modal content.
pub fn receive_picker_modal<'a>(
    from_asset: SendAsset,
    current_to: SendAsset,
    current_network: ReceiveNetwork,
    cross_asset_supported: bool,
) -> Element<'a, LiquidSendMessage> {
    let title = text(localized_copy("THEY RECEIVE")).size(H4_SIZE).bold();

    let options = ReceiveNetwork::options_for_send_asset(from_asset, cross_asset_supported);

    let mut col = Column::new()
        .spacing(8)
        .padding(24)
        .max_width(420)
        .push(title);

    for (asset, network) in options {
        let is_selected = asset == current_to && network == current_network;
        let (asset_slug, label, network_slug, network_label) = match (asset, network) {
            (SendAsset::Lbtc, ReceiveNetwork::Lightning) => {
                ("btc", "BTC", "lightning", "Lightning")
            }
            (SendAsset::Lbtc, ReceiveNetwork::Liquid) => ("lbtc", "L-BTC", "liquid", "Liquid"),
            (SendAsset::Lbtc, ReceiveNetwork::Bitcoin) => ("btc", "BTC", "bitcoin", "Bitcoin"),
            (SendAsset::Usdt, ReceiveNetwork::Liquid) => ("usdt", "USDt", "liquid", "Liquid"),
            (SendAsset::Usdt, ReceiveNetwork::Ethereum) => ("usdt", "USDt", "ethereum", "Ethereum"),
            (SendAsset::Usdt, ReceiveNetwork::Tron) => ("usdt", "USDt", "tron", "Tron"),
            (SendAsset::Usdt, ReceiveNetwork::Binance) => ("usdt", "USDt", "bsc", "Binance"),
            (SendAsset::Usdt, ReceiveNetwork::Solana) => ("usdt", "USDt", "solana", "Solana"),
            _ => continue,
        };
        let ico: Element<'_, LiquidSendMessage> =
            asset_network_logo(asset_slug, network_slug, 36.0);

        col = col.push(send_picker_action(
            receive_target_action_id(asset, network),
            ico,
            label,
            "",
            network_label,
            is_selected,
            LiquidSendMessage::SetReceiveTarget(asset, network),
        ));
    }

    col.into()
}

pub struct RecentTransaction {
    pub description: String,
    pub time_ago: String,
    pub amount: Amount,
    pub fees_sat: Amount,
    pub fiat_amount: Option<FiatAmount>,
    pub is_incoming: bool,
    pub status: DomainPaymentStatus,
    pub details: DomainPaymentDetails,
    /// When set, the transaction displays this string instead of the BTC amount (e.g. "5.00 USDt").
    pub usdt_display: Option<String>,
}

pub struct AmountInputConfig<'a> {
    pub amount: &'a form::Value<String>,
    pub usdt_amount_input: &'a form::Value<String>,
    pub to_asset: SendAsset,
    pub from_asset: SendAsset,
    pub uri_asset: Option<AssetKind>,
    pub usdt_balance: u64,
    pub comment: String,
    pub has_fiat_converter: bool,
    pub btc_balance: Amount,
    pub description: Option<&'a str>,
    pub lightning_limits: Option<(u64, u64)>,
    pub onchain_limits: Option<(u64, u64)>,
    pub input_type: &'a Option<InputType>,
    pub bitcoin_unit: BitcoinDisplayUnit,
    pub error: Option<&'a str>,
    pub cross_asset_supported: bool,
    pub pay_fees_with_asset: bool,
    pub max_loading: bool,
}

/// "Max" button that shows an animated "." → ".." → "..." while the fee probe is in flight.
fn max_button(
    loading: bool,
) -> iced::Element<'static, LiquidSendMessage, coincube_ui::theme::Theme> {
    use coincube_ui::component::spinner::typing_text_carousel;
    use std::time::Duration;

    let label: iced::Element<'static, LiquidSendMessage, coincube_ui::theme::Theme> = if loading {
        typing_text_carousel("...", true, Duration::from_millis(300), |s| {
            text(s).size(13).color(color::ORANGE)
        })
        .into()
    } else {
        text("Max").size(13).color(color::ORANGE).into()
    };
    iced_button(
        Container::new(label)
            .padding([4, 12])
            .center_x(Length::Shrink),
    )
    .on_press_maybe(if loading {
        None
    } else {
        Some(LiquidSendMessage::PopupMessage(
            view::SendPopupMessage::SendMax,
        ))
    })
    .style(|_, _| iced::widget::button::Style {
        background: Some(Background::Color(iced::Color::TRANSPARENT)),
        text_color: color::ORANGE,
        border: iced::Border {
            color: color::ORANGE,
            width: 1.0,
            radius: 6.0.into(),
        },
        ..Default::default()
    })
    .into()
}

pub fn amount_input_model<'a>(config: AmountInputConfig<'a>) -> Element<'a, LiquidSendMessage> {
    let mut content = Column::new()
        .spacing(20)
        .padding(30)
        .width(Length::Fixed(500.0))
        .align_x(Alignment::Center);

    // Show inline error banner if present
    if let Some(err) = config.error {
        content = content.push(
            container(p1_regular(coincube_ui::locale::notice(err.to_owned())).color(color::WHITE))
                .width(Length::Fill)
                .padding(12)
                .style(|_| {
                    iced::widget::container::Style::default()
                        .background(iced::Color::from_rgb(0.3, 0.05, 0.05))
                        .border(
                            iced::Border::default()
                                .width(1)
                                .color(color::RED)
                                .rounded(8),
                        )
                }),
        );
    }

    // Show balance of the asset being paid from
    let paying_asset = config.from_asset;
    let balance_text = match paying_asset {
        SendAsset::Usdt => format!("{} USDt", format_usdt_display(config.usdt_balance)),
        SendAsset::Lbtc => format!(
            "{} {}",
            if matches!(config.bitcoin_unit, BitcoinDisplayUnit::BTC) {
                config.btc_balance.to_btc().to_string()
            } else {
                config.btc_balance.to_sat().to_string()
            },
            config.bitcoin_unit
        ),
    };

    let header = Row::new()
        .push(iced::widget::Space::new().width(Length::Fill))
        .push(text(localized_copy("BALANCE: ")).size(16))
        .push(text(balance_text).size(16).bold().color(color::ORANGE))
        .width(Length::Fill)
        .align_y(Alignment::Center);

    content = content.push(header);

    // Cross-asset swap indicator and toggle (SideSwap, mainnet only)
    if config.cross_asset_supported
        && (config.uri_asset.is_some() || config.from_asset != config.to_asset)
    {
        let is_cross_asset = config.from_asset != config.to_asset;
        let toggle_label = if is_cross_asset {
            let paying_with = match config.from_asset {
                SendAsset::Lbtc => "L-BTC",
                SendAsset::Usdt => "USDt",
            };
            let receiving = match config.to_asset {
                SendAsset::Lbtc => "L-BTC",
                SendAsset::Usdt => "USDt",
            };
            format!(
                "Paying with {} → Receiver gets {} (swap)",
                paying_with, receiving
            )
        } else {
            let asset_name = match config.to_asset {
                SendAsset::Lbtc => "L-BTC",
                SendAsset::Usdt => "USDt",
            };
            format!("Paying with {}", asset_name)
        };

        let swap_toggle =
            iced_button(
                Container::new(
                    Row::new()
                        .spacing(6)
                        .align_y(Alignment::Center)
                        .push(icon::left_right_icon().size(14).style(|_| {
                            iced::widget::text::Style {
                                color: Some(color::ORANGE),
                            }
                        }))
                        .push(text(toggle_label).size(13).color(color::ORANGE)),
                )
                .padding([4, 12]),
            )
            .on_press(LiquidSendMessage::PopupMessage(
                view::SendPopupMessage::ToggleSendAsset,
            ))
            .style(|_, _| iced::widget::button::Style {
                background: Some(Background::Color(iced::Color::TRANSPARENT)),
                text_color: color::ORANGE,
                border: iced::Border {
                    color: color::ORANGE,
                    width: 1.0,
                    radius: 6.0.into(),
                },
                ..Default::default()
            });

        content = content.push(
            Container::new(swap_toggle)
                .width(Length::Fill)
                .center_x(Length::Fill),
        );
    }

    if let Some(desc) = config.description {
        content = content.push(
            Container::new(text(desc).size(16))
                .padding([10, 20])
                .width(Length::Fill)
                .style(
                    |_theme: &coincube_ui::theme::Theme| iced::widget::container::Style {
                        background: Some(iced::Background::Color(iced::Color::from_rgb(
                            0.15, 0.15, 0.15,
                        ))),
                        border: iced::Border {
                            color: iced::Color::from_rgb(0.6, 0.4, 0.2),
                            width: 2.0,
                            radius: 50.0.into(),
                        },
                        ..Default::default()
                    },
                ),
        );
    }

    // Amount section — branched on the pre-selected asset (no toggle)
    match config.to_asset {
        SendAsset::Usdt => {
            let mut usdt_col = Column::new()
                .spacing(5)
                .push(
                    Row::new()
                        .push(text(localized_copy("Amount (USDt)")).size(16))
                        .push(Space::new().width(Length::Fill))
                        .push(max_button(config.max_loading))
                        .align_y(Alignment::Center),
                )
                .push(
                    iced::widget::text_input("e.g. 1.50", &config.usdt_amount_input.value)
                        .on_input(|v| {
                            LiquidSendMessage::PopupMessage(
                                view::SendPopupMessage::UsdtAmountEdited(v),
                            )
                        })
                        .padding(10),
                );
            if let Some(warn) = config.usdt_amount_input.warning {
                usdt_col = usdt_col.push(text(localized_copy(warn)).size(12).color(color::ORANGE));
            }

            // Fee asset toggle (USDt vs L-BTC) — only for same-asset sends
            if config.from_asset == config.to_asset {
                let fee_label = if config.pay_fees_with_asset {
                    "Pay fees with USDt"
                } else {
                    "Pay fees with L-BTC"
                };
                let fee_toggle = iced_button(
                    Container::new(
                        Row::new()
                            .spacing(6)
                            .align_y(Alignment::Center)
                            .push(icon::left_right_icon().size(14).style(|_| {
                                iced::widget::text::Style {
                                    color: Some(color::ORANGE),
                                }
                            }))
                            .push(text(fee_label).size(13).color(color::ORANGE)),
                    )
                    .padding([4, 12]),
                )
                .on_press(LiquidSendMessage::PopupMessage(
                    view::SendPopupMessage::ToggleFeeAsset,
                ))
                .style(|_, _| iced::widget::button::Style {
                    background: Some(Background::Color(iced::Color::TRANSPARENT)),
                    text_color: color::ORANGE,
                    border: iced::Border {
                        color: color::ORANGE,
                        width: 1.0,
                        radius: 6.0.into(),
                    },
                    ..Default::default()
                });

                usdt_col = usdt_col.push(
                    Container::new(fee_toggle)
                        .width(Length::Fill)
                        .center_x(Length::Fill),
                );
            }

            content = content.push(usdt_col);
        }
        SendAsset::Lbtc => {
            let mut amount_label_section = Column::new().spacing(2);

            let max_btn = max_button(config.max_loading);

            let amount_row = Row::new()
                .spacing(10)
                .push(
                    text(localized_format!(
                        "Amount ({})",
                        "Importe ({})",
                        config.bitcoin_unit
                    ))
                    .size(16),
                )
                .push(iced::widget::Space::new().width(Length::Fill))
                .align_y(Alignment::Center);

            let amount_row = if config.has_fiat_converter {
                amount_row
                    .push(
                        button::transparent(None, "⇄")
                            .on_press(LiquidSendMessage::PopupMessage(
                                view::SendPopupMessage::FiatConvert,
                            ))
                            .width(Length::Shrink),
                    )
                    .push(max_btn)
            } else {
                amount_row.push(max_btn)
            };

            amount_label_section = amount_label_section.push(amount_row);

            let mut amount_input_section = Column::new().spacing(5);
            amount_input_section = amount_input_section.push(
                if matches!(config.bitcoin_unit, BitcoinDisplayUnit::BTC) {
                    form::Form::new_amount_btc(
                        coincube_ui::locale::copy("Enter amount"),
                        config.amount,
                        |v| {
                            LiquidSendMessage::PopupMessage(view::SendPopupMessage::AmountEdited(v))
                        },
                    )
                    .maybe_warning(config.amount.warning.map(coincube_ui::locale::copy))
                    .padding(10)
                } else {
                    form::Form::new_amount_sats(
                        coincube_ui::locale::copy("Enter amount"),
                        config.amount,
                        |v| {
                            LiquidSendMessage::PopupMessage(view::SendPopupMessage::AmountEdited(v))
                        },
                    )
                    .maybe_warning(config.amount.warning.map(coincube_ui::locale::copy))
                    .padding(10)
                },
            );

            if let Some(input_type) = config.input_type {
                if matches!(input_type, InputType::BitcoinAddress { .. }) {
                    if let Some((min_sat, max_sat)) = config.onchain_limits {
                        let min_btc = Amount::from_sat(min_sat);
                        let max_btc = Amount::from_sat(max_sat);
                        amount_input_section = amount_input_section.push(
                            text(localized_format!(
                                "Enter an amount between {} and {} {}",
                                "Introduce un importe entre {} y {} {}",
                                min_btc.to_formatted_string_with_unit(config.bitcoin_unit),
                                max_btc.to_formatted_string_with_unit(config.bitcoin_unit),
                                config.bitcoin_unit,
                            ))
                            .size(12),
                        );
                    }
                } else if let Some((min_sat, max_sat)) = config.lightning_limits {
                    let min_btc = Amount::from_sat(min_sat);
                    let max_btc = Amount::from_sat(max_sat);
                    amount_input_section = amount_input_section.push(
                        text(localized_format!(
                            "Enter an amount between {} and {} {}",
                            "Introduce un importe entre {} y {} {}",
                            min_btc.to_formatted_string_with_unit(config.bitcoin_unit),
                            max_btc.to_formatted_string_with_unit(config.bitcoin_unit),
                            config.bitcoin_unit,
                        ))
                        .size(12),
                    );
                }
            }

            amount_label_section = amount_label_section.push(amount_input_section);
            content = content.push(amount_label_section);
        }
    }

    content = content.push(iced::widget::Space::new().height(Length::Fixed(5.0)));

    let mut comment_section = Column::new().spacing(5);
    comment_section = comment_section.push(text(localized_copy("Comment")).size(16));
    comment_section = comment_section.push(
        iced::widget::text_input("Comment (Optional)", &config.comment)
            .on_input(|v| LiquidSendMessage::PopupMessage(view::SendPopupMessage::CommentEdited(v)))
            .padding(10),
    );

    content = content.push(comment_section);

    // Check that the paying asset has sufficient balance
    let paying_asset = config.from_asset;
    let has_balance = match paying_asset {
        SendAsset::Usdt => config.usdt_balance > 0,
        SendAsset::Lbtc => config.btc_balance.to_sat() > 0,
    };

    let is_next_enabled = has_balance
        && match config.to_asset {
            SendAsset::Usdt
                if matches!(config.input_type, Some(InputType::LiquidAddress { .. })) =>
            {
                config.usdt_amount_input.valid && !config.usdt_amount_input.value.trim().is_empty()
            }
            _ => config.amount.valid && !config.amount.value.trim().is_empty(),
        };

    let next_button = button::primary(None, localized_copy("Next")).width(Length::Fill);
    let next_button = if !is_next_enabled {
        next_button
    } else {
        next_button.on_press(LiquidSendMessage::PopupMessage(
            view::SendPopupMessage::Done,
        ))
    };

    content = content.push(next_button);

    Container::new(content)
        .padding(20)
        .style(coincube_ui::theme::card::simple)
        .into()
}

pub fn fiat_input_model<'a>(
    fiat_input: &'a form::Value<String>,
    currencies: &'a [crate::services::fiat::Currency; 4],
    selected_currency: &'a crate::services::fiat::Currency,
    converters: &'a std::collections::HashMap<crate::services::fiat::Currency, FiatAmountConverter>,
    bitcoin_unit: BitcoinDisplayUnit,
) -> Element<'a, LiquidSendMessage> {
    use coincube_ui::component::amount::DisplayAmount;
    use coincube_ui::icon::cross_icon;

    let mut content = Column::new()
        .spacing(15)
        .padding(30)
        .width(Length::Fixed(500.0))
        .align_x(Alignment::Center);

    let header = Row::new()
        .push(
            text(localized_copy("Select Fiat Currency:"))
                .size(20)
                .bold(),
        )
        .push(iced::widget::Space::new().width(Length::Fill))
        .push(
            button::transparent(Some(cross_icon()), "")
                .on_press(LiquidSendMessage::PopupMessage(
                    view::SendPopupMessage::FiatClose,
                ))
                .width(Length::Shrink),
        )
        .width(Length::Fill)
        .align_y(Alignment::Center);

    content = content.push(header);

    let mut currency_row = Row::new().spacing(10).align_y(Alignment::Center);

    for currency in currencies.iter() {
        let is_selected = currency == selected_currency;
        let currency_str = &currency.to_static_str();

        let capsule = button::primary(None, currency_str)
            .on_press(LiquidSendMessage::PopupMessage(
                view::SendPopupMessage::FiatCurrencySelected(*currency),
            ))
            .width(Length::Shrink)
            .style(move |_theme, status| {
                let bg_color = if is_selected {
                    iced::Color::from_rgb(1.0, 0.647, 0.0)
                } else {
                    iced::Color::from_rgb(0.15, 0.15, 0.15)
                };

                let text_color = if is_selected {
                    iced::Color::BLACK
                } else {
                    iced::Color::WHITE
                };

                let base_style = iced::widget::button::Style {
                    background: Some(iced::Background::Color(bg_color)),
                    text_color,
                    border: iced::Border {
                        radius: 8.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                };

                match status {
                    iced::widget::button::Status::Hovered => iced::widget::button::Style {
                        background: Some(iced::Background::Color(iced::Color {
                            a: 0.8,
                            ..bg_color
                        })),
                        ..base_style
                    },
                    _ => base_style,
                }
            });

        currency_row = currency_row.push(capsule);
    }

    content = content.push(
        Container::new(currency_row)
            .width(Length::Fill)
            .align_x(Alignment::Center),
    );

    content = content.push(
        text(localized_format!(
            "Amount in {}",
            "Importe en {}",
            selected_currency
        ))
        .size(16)
        .width(Length::Fill),
    );

    content = content.push(
        form::Form::new_amount_numeric(&format!("{} amount", selected_currency), fiat_input, |v| {
            LiquidSendMessage::PopupMessage(view::SendPopupMessage::FiatInputEdited(v))
        })
        .maybe_warning(fiat_input.warning.map(coincube_ui::locale::copy))
        .padding(10),
    );

    let (btc_amount_str, rate_str) = if let Some(converter) = converters.get(selected_currency) {
        let default_string = format!(
            "{} {}",
            if matches!(bitcoin_unit, BitcoinDisplayUnit::BTC) {
                "0.00000000".to_string()
            } else {
                "0".to_string()
            },
            bitcoin_unit
        );
        let btc_amount = if !fiat_input.value.is_empty() {
            if let Ok(fiat_amount) = FiatAmount::from_str_in(&fiat_input.value, *selected_currency)
            {
                if let Ok(btc_amt) = converter.convert_to_btc(&fiat_amount) {
                    format!(
                        "{} {}",
                        if matches!(bitcoin_unit, BitcoinDisplayUnit::BTC) {
                            btc_amt.to_btc().to_string()
                        } else {
                            btc_amt.to_sat().to_string()
                        },
                        bitcoin_unit
                    )
                } else {
                    default_string
                }
            } else {
                default_string
            }
        } else {
            default_string
        };

        let amount = if matches!(bitcoin_unit, BitcoinDisplayUnit::BTC) {
            "1"
        } else {
            "1000"
        };

        let fiat_value = if matches!(bitcoin_unit, BitcoinDisplayUnit::BTC) {
            converter.to_fiat_amount().to_formatted_string()
        } else {
            (converter.price_per_btc() / 100000.0f64).to_string()
        };

        let rate = format!(
            "{} {} = {} {}",
            amount, bitcoin_unit, fiat_value, selected_currency
        );

        (btc_amount, rate)
    } else {
        ("Loading...".to_string(), "Fetching rate...".to_string())
    };

    let btc_conversion_section = Column::new()
        .spacing(2)
        .align_x(Alignment::Center)
        .push(text(btc_amount_str).size(18).bold())
        .push(
            text(rate_str)
                .size(14)
                .color(iced::Color::from_rgb(0.7, 0.7, 0.7)),
        );

    content = content.push(
        Container::new(btc_conversion_section)
            .width(Length::Fill)
            .align_x(Alignment::Center),
    );

    content = content.push(iced::widget::Space::new().height(Length::Fixed(5.0)));

    let done_button = button::primary(None, CopyText::Done.get()).width(Length::Fill);
    let done_button = if !fiat_input.valid || fiat_input.value.is_empty() {
        done_button
    } else {
        done_button.on_press(LiquidSendMessage::PopupMessage(
            view::SendPopupMessage::FiatDone,
        ))
    };

    content = content.push(done_button);

    Container::new(content)
        .padding(20)
        .style(coincube_ui::theme::card::simple)
        .into()
}

/// The confirmation screen, rendered **only** from the prepared intent.
///
/// It used to take the amount, assets, comment and destination as loose
/// arguments read from the live screen state, with the prepared responses passed
/// alongside. Those fields keep changing while a prepare is in flight, so the
/// screen could describe a payment that was not the one about to execute. Taking
/// the intent — the same value `ConfirmSend` executes — removes the possibility.
pub fn final_check_page<'a>(
    intent: &'a PreparedIntent,
    chain: crate::chain::ChainId,
    fiat_converter: Option<&FiatAmountConverter>,
    is_sending: bool,
    bitcoin_unit: BitcoinDisplayUnit,
) -> Element<'a, LiquidSendMessage> {
    let amount = intent.amount();
    let to_asset = intent.to_asset();
    let from_asset = intent.from_asset();
    let usdt_send_amount = intent.usdt_amount_display();
    let comment = intent.comment().unwrap_or("").to_string();
    let description = intent.description();
    let header = Row::new()
        .push(
            button::secondary(Some(icon::previous_icon()), CopyText::Back.get())
                .width(Length::Fixed(150.0))
                .on_press(LiquidSendMessage::PopupMessage(
                    view::SendPopupMessage::Close,
                )),
        )
        .push(Space::new().width(Length::Fill))
        .width(Length::Fill)
        .padding([0, 40])
        .align_y(Alignment::Center);

    let network = crate::chain::canonical_network_name(chain);
    let destination = match intent.payment() {
        PreparedPayment::Onchain { .. } => network.to_owned(),
        PreparedPayment::Regular { response, .. } => match &response.destination {
            breez_sdk_liquid::prelude::SendDestination::LiquidAddress { .. } => "Liquid".to_owned(),
            breez_sdk_liquid::prelude::SendDestination::Bolt11 { .. }
            | breez_sdk_liquid::prelude::SendDestination::Bolt12 { .. } => {
                CopyText::LightningNetwork
                    .get()
                    .replacen("{network}", network, 1)
            }
        },
    };
    let mut content = Column::new()
        .spacing(25)
        .padding(40)
        .width(Length::Fill)
        .max_width(600)
        .align_x(Alignment::Center)
        .push(p2_regular(CopyText::SourceLiquidNetwork.get()))
        .push(p2_regular(CopyText::DestinationNetwork.get().replacen(
            "{network}",
            &destination,
            1,
        )));

    if let Some(desc) = description {
        content = content.push(
            Container::new(text(desc.to_string()).size(22).bold())
                .width(Length::Fill)
                .align_x(Alignment::Center),
        );
    }

    // Cross-asset swap indicator
    if from_asset != to_asset {
        let paying_with = match from_asset {
            SendAsset::Lbtc => "L-BTC",
            SendAsset::Usdt => "USDt",
        };
        let receiving = match to_asset {
            SendAsset::Lbtc => "L-BTC",
            SendAsset::Usdt => "USDt",
        };
        content = content.push(
            Container::new(
                text(localized_format!(
                    "Cross-asset swap: paying with {} → receiver gets {}",
                    "Intercambio entre activos: pagas {} → el destinatario recibe {}",
                    paying_with,
                    receiving
                ))
                .size(14)
                .color(color::ORANGE),
            )
            .padding([6, 12])
            .width(Length::Fill)
            .center_x(Length::Fill),
        );
    }

    content = content.push(Space::new().height(Length::Fixed(2.0)));

    // Fee display: for USDt sends the SDK may quote the fee in USDt (asset fees)
    // or in L-BTC. The intent answers both, from the response it actually holds
    // — no guessing from the input type about which prepare applies.
    let usdt_asset_fees: Option<f64> = intent.asset_fees();
    let fees_sat = intent.fees_sat();
    let fees_amount = Amount::from_sat(fees_sat);

    if to_asset == SendAsset::Usdt {
        // USDt send: show USDt amount prominently, fees separately
        content = content.push(
            Container::new(
                text(format!("{} USDt", usdt_send_amount))
                    .size(38)
                    .bold()
                    .color(color::ORANGE),
            )
            .width(Length::Fill)
            .align_x(Alignment::Center),
        );

        content = content.push(Space::new().height(Length::Fixed(10.0)));

        let mut details_box = Column::new().spacing(15).width(Length::Fill).padding(20);

        details_box = details_box.push(
            Row::new()
                .push(text(localized_copy("Send amount:")).size(16))
                .push(Space::new().width(Length::Fill))
                .push(text(format!("{} USDt", usdt_send_amount)).size(16).bold())
                .width(Length::Fill)
                .align_y(Alignment::Center),
        );

        details_box = details_box.push(
            Container::new(Space::new().height(Length::Fixed(1.0)))
                .width(Length::Fill)
                .style(
                    |_theme: &coincube_ui::theme::Theme| iced::widget::container::Style {
                        background: Some(iced::Background::Color(color::GREY_3)),
                        ..Default::default()
                    },
                ),
        );

        if let Some(asset_fee) = usdt_asset_fees {
            // Fees paid in USDt — convert f64 to base units for consistent formatting
            let fee_base = (asset_fee
                * 10_u64.pow(crate::app::breez_liquid::assets::USDT_PRECISION as u32) as f64)
                .ceil() as u64;
            details_box = details_box.push(
                Row::new()
                    .push(text(localized_copy("Fees (USDt):")).size(16))
                    .push(Space::new().width(Length::Fill))
                    .push(
                        text(format!(
                            "{} USDt",
                            crate::app::breez_liquid::assets::format_usdt_display(fee_base)
                        ))
                        .size(16)
                        .bold(),
                    )
                    .width(Length::Fill)
                    .align_y(Alignment::Center),
            );
        } else {
            // Fees paid in L-BTC
            details_box = details_box.push(
                Row::new()
                    .push(text(localized_copy("Fees (L-BTC):")).size(16))
                    .push(Space::new().width(Length::Fill))
                    .push(
                        text(format!(
                            "{} {}",
                            if matches!(bitcoin_unit, BitcoinDisplayUnit::BTC) {
                                fees_amount.to_btc().to_string()
                            } else {
                                fees_amount.to_sat().to_string()
                            },
                            bitcoin_unit
                        ))
                        .size(16)
                        .bold(),
                    )
                    .width(Length::Fill)
                    .align_y(Alignment::Center),
            );
        }

        if !comment.is_empty() {
            details_box = details_box.push(
                Container::new(Space::new().height(Length::Fixed(1.0)))
                    .width(Length::Fill)
                    .style(
                        |_theme: &coincube_ui::theme::Theme| iced::widget::container::Style {
                            background: Some(iced::Background::Color(color::GREY_3)),
                            ..Default::default()
                        },
                    ),
            );
            details_box = details_box.push(
                Row::new()
                    .push(text(localized_copy("Comment:")).size(16))
                    .push(Space::new().width(Length::Fill))
                    .push(text(comment.clone()).size(16).bold())
                    .width(Length::Fill)
                    .align_y(Alignment::Center),
            );
        }

        content = content.push(Container::new(details_box).width(Length::Fill).style(
            |_theme: &coincube_ui::theme::Theme| iced::widget::container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgb(
                    0.15, 0.15, 0.15,
                ))),
                border: iced::Border {
                    radius: 12.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            },
        ));

        content = content.push(Space::new().height(Length::Fixed(30.0)));

        let send_button = button::primary(None, localized_copy("Send")).width(Length::Fill);
        content = content.push(if is_sending {
            send_button
        } else {
            send_button.on_press(LiquidSendMessage::ConfirmSend)
        });

        if is_sending {
            content = content.push(loading_indicator(None))
        }

        return Column::new()
            .push(header)
            .push(
                Container::new(content)
                    .width(Length::Fill)
                    .align_x(Alignment::Center),
            )
            .width(Length::Fill)
            .into();
    }

    let total_amount = Amount::from_sat(intent.total_sat());

    content = content.push(
        Container::new(
            text(format!(
                "{} {}",
                if matches!(bitcoin_unit, BitcoinDisplayUnit::BTC) {
                    amount.to_btc().to_string()
                } else {
                    amount.to_sat().to_string()
                },
                bitcoin_unit
            ))
            .size(38)
            .bold()
            .color(color::ORANGE),
        )
        .width(Length::Fill)
        .align_x(Alignment::Center),
    );

    if let Some(converter) = fiat_converter {
        let fiat_amount = converter.convert(amount);
        content = content.push(
            Container::new(fiat_amount.to_text().size(18).color(color::GREY_3))
                .width(Length::Fill)
                .align_x(Alignment::Center),
        );
    }

    content = content.push(Space::new().height(Length::Fixed(10.0)));

    let mut details_box = Column::new().spacing(15).width(Length::Fill).padding(20);

    details_box = details_box.push(
        Row::new()
            .push(text(localized_copy("Amount:")).size(16))
            .push(Space::new().width(Length::Fill))
            .push(
                text(format!(
                    "{} {}",
                    if matches!(bitcoin_unit, BitcoinDisplayUnit::BTC) {
                        amount.to_btc().to_string()
                    } else {
                        amount.to_sat().to_string()
                    },
                    bitcoin_unit
                ))
                .size(16)
                .bold(),
            )
            .width(Length::Fill)
            .align_y(Alignment::Center),
    );

    details_box = details_box.push(
        Container::new(Space::new().height(Length::Fixed(1.0)))
            .width(Length::Fill)
            .style(
                |_theme: &coincube_ui::theme::Theme| iced::widget::container::Style {
                    background: Some(iced::Background::Color(color::GREY_3)),
                    ..Default::default()
                },
            ),
    );

    details_box = details_box.push(
        Row::new()
            .push(text(localized_copy("Fees:")).size(16))
            .push(Space::new().width(Length::Fill))
            .push(
                text(format!(
                    "{} {}",
                    if matches!(bitcoin_unit, BitcoinDisplayUnit::BTC) {
                        fees_amount.to_btc().to_string()
                    } else {
                        fees_amount.to_sat().to_string()
                    },
                    bitcoin_unit
                ))
                .size(16)
                .bold(),
            )
            .width(Length::Fill)
            .align_y(Alignment::Center),
    );

    details_box = details_box.push(
        Container::new(Space::new().height(Length::Fixed(1.0)))
            .width(Length::Fill)
            .style(
                |_theme: &coincube_ui::theme::Theme| iced::widget::container::Style {
                    background: Some(iced::Background::Color(color::GREY_3)),
                    ..Default::default()
                },
            ),
    );

    details_box = details_box.push(
        Row::new()
            .push(text(localized_copy("Total:")).size(18).bold())
            .push(Space::new().width(Length::Fill))
            .push(
                text(format!(
                    "{} {}",
                    if matches!(bitcoin_unit, BitcoinDisplayUnit::BTC) {
                        total_amount.to_btc().to_string()
                    } else {
                        total_amount.to_sat().to_string()
                    },
                    bitcoin_unit
                ))
                .size(18)
                .bold()
                .color(color::ORANGE),
            )
            .width(Length::Fill)
            .align_y(Alignment::Center),
    );

    if !comment.is_empty() {
        details_box = details_box.push(
            Container::new(Space::new().height(Length::Fixed(1.0)))
                .width(Length::Fill)
                .style(
                    |_theme: &coincube_ui::theme::Theme| iced::widget::container::Style {
                        background: Some(iced::Background::Color(color::GREY_3)),
                        ..Default::default()
                    },
                ),
        );

        details_box = details_box.push(
            Row::new()
                .push(text(localized_copy("Comment:")).size(16))
                .push(Space::new().width(Length::Fill))
                .push(text(comment).size(16).bold())
                .width(Length::Fill)
                .align_y(Alignment::Center),
        );
    }

    content = content.push(Container::new(details_box).width(Length::Fill).style(
        |_theme: &coincube_ui::theme::Theme| iced::widget::container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgb(
                0.15, 0.15, 0.15,
            ))),
            border: iced::Border {
                radius: 12.0.into(),
                ..Default::default()
            },
            ..Default::default()
        },
    ));

    content = content.push(Space::new().height(Length::Fixed(30.0)));

    let send_button = button::primary(None, localized_copy("Send")).width(Length::Fill);
    content = content.push(if is_sending {
        send_button
    } else {
        send_button.on_press(LiquidSendMessage::ConfirmSend)
    });

    if is_sending {
        content = content.push(loading_indicator(None))
    }

    Column::new()
        .push(header)
        .push(
            Container::new(content)
                .width(Length::Fill)
                .align_x(Alignment::Center),
        )
        .width(Length::Fill)
        .into()
}

// NOTE: The old send_asset_toggle function has been removed.
// Asset selection now uses the two-card "You Send / They Receive" layout
// with picker modals (send_picker_modal / receive_picker_modal above).
//
// The following dead code is the remainder of the old toggle — delete after
// confirming the new layout works.
#[allow(dead_code)]
fn _old_send_asset_toggle(_current_asset: SendAsset) -> Element<'static, LiquidSendMessage> {
    let lbtc_active = _current_asset == SendAsset::Lbtc;
    let usdt_active = _current_asset == SendAsset::Usdt;

    let lbtc_button = {
        let ico = icon::bitcoin_icon()
            .size(18)
            .style(move |_theme: &theme::Theme| iced::widget::text::Style {
                color: Some(if lbtc_active {
                    color::ORANGE
                } else {
                    color::GREY_2
                }),
            });

        let label =
            text("L-BTC")
                .size(16)
                .style(move |_theme: &theme::Theme| iced::widget::text::Style {
                    color: Some(if lbtc_active {
                        color::ORANGE
                    } else {
                        color::GREY_2
                    }),
                });

        let button_content = Container::new(
            Row::new()
                .spacing(8)
                .align_y(iced::alignment::Vertical::Center)
                .push(ico)
                .push(label),
        )
        .width(Length::Fill)
        .align_x(iced::alignment::Horizontal::Center);

        Container::new(
            iced_button(Container::new(button_content).padding([10, 30]))
                .style(move |_theme: &theme::Theme, _status| iced_button::Style {
                    background: Some(Background::Color(iced::Color::TRANSPARENT)),
                    text_color: if lbtc_active {
                        color::WHITE
                    } else {
                        color::GREY_2
                    },
                    border: iced::Border {
                        radius: 6.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .on_press(LiquidSendMessage::PresetAsset(SendAsset::Lbtc)),
        )
        .style(move |_theme: &theme::Theme| container::Style {
            background: Some(Background::Color(if lbtc_active {
                iced::color!(0x161716)
            } else {
                color::TRANSPARENT
            })),
            border: iced::Border {
                radius: 6.0.into(),
                color: if lbtc_active {
                    color::ORANGE
                } else {
                    color::TRANSPARENT
                },
                width: if lbtc_active { 0.7 } else { 0.0 },
            },
            ..Default::default()
        })
    };

    let usdt_button = {
        let ico = icon::usd_icon()
            .size(18)
            .style(move |_theme: &theme::Theme| iced::widget::text::Style {
                color: Some(if usdt_active {
                    color::ORANGE
                } else {
                    color::GREY_2
                }),
            });

        let label =
            text("USDt")
                .size(16)
                .style(move |_theme: &theme::Theme| iced::widget::text::Style {
                    color: Some(if usdt_active {
                        color::ORANGE
                    } else {
                        color::GREY_2
                    }),
                });

        let button_content = Container::new(
            Row::new()
                .spacing(8)
                .align_y(iced::alignment::Vertical::Center)
                .push(ico)
                .push(label),
        )
        .width(Length::Fill)
        .align_x(iced::alignment::Horizontal::Center);

        Container::new(
            iced_button(Container::new(button_content).padding([10, 30]))
                .style(move |_theme: &theme::Theme, _status| iced_button::Style {
                    background: Some(Background::Color(iced::Color::TRANSPARENT)),
                    text_color: if usdt_active {
                        color::WHITE
                    } else {
                        color::GREY_2
                    },
                    border: iced::Border {
                        radius: 6.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .on_press(LiquidSendMessage::PresetAsset(SendAsset::Usdt)),
        )
        .style(move |_theme: &theme::Theme| container::Style {
            background: Some(Background::Color(if usdt_active {
                iced::color!(0x161716)
            } else {
                color::TRANSPARENT
            })),
            border: iced::Border {
                radius: 6.0.into(),
                color: if usdt_active {
                    color::ORANGE
                } else {
                    color::TRANSPARENT
                },
                width: if usdt_active { 0.7 } else { 0.0 },
            },
            ..Default::default()
        })
    };

    Container::new(Row::new().push(lbtc_button).push(usdt_button))
        .padding(4)
        .max_width(800.0)
        .style(|_theme: &theme::Theme| container::Style {
            background: Some(Background::Color(iced::color!(0x202020))),
            border: iced::Border {
                color: iced::color!(0x202020),
                radius: 10.0.into(),
                width: 2.0,
            },
            ..Default::default()
        })
        .into()
}

#[cfg(test)]
mod privacy_tests {
    use super::*;
    use crate::app::cache::Cache;
    use crate::app::view::shared::landing::capture;

    #[test]
    fn breadcrumb_navigates_to_liquid_overview_without_resetting_the_draft() {
        let cache = Cache::default();
        for pointer in [false, true] {
            let messages = capture::activate(
                send_page(
                    Column::new().into(),
                    &cache,
                    Amount::ZERO,
                    0,
                    SendAsset::Lbtc,
                ),
                iced::advanced::widget::Id::new(BREADCRUMB_ACTION_ID),
                (1024, 768),
                pointer,
            );
            assert!(messages.iter().any(|message| matches!(
                message,
                Message::Menu(Menu::Liquid(crate::app::menu::LiquidSubMenu::Overview))
            )));
            assert!(!messages.iter().any(|message| matches!(
                message,
                Message::LiquidSend(LiquidSendMessage::BackToHome)
            )));
        }
    }

    #[test]
    fn master_asset_reselection_uses_current_asset_for_keyboard_and_pointer() {
        let input = form::Value::<String>::default();
        let input_type = None;
        for asset in [SendAsset::Lbtc, SendAsset::Usdt] {
            let id = if asset == SendAsset::Lbtc {
                "liquid-send-kind-lbtc"
            } else {
                "liquid-send-kind-usdt"
            };
            for pointer in [false, true] {
                let messages = capture::activate(
                    liquid_send_view(
                        Amount::ZERO,
                        0,
                        asset,
                        asset,
                        ReceiveNetwork::Liquid,
                        None,
                        &[],
                        &input,
                        &input_type,
                        BitcoinDisplayUnit::Sats,
                        "",
                        false,
                        false,
                    ),
                    iced::advanced::widget::Id::new(id),
                    (1024, 768),
                    pointer,
                );
                assert!(messages.iter().any(|message| matches!(
                    message,
                    LiquidSendMessage::SelectMasterAsset(selected) if *selected == asset
                )));
            }
        }
    }

    #[test]
    fn send_details_fit_compact_and_wide_themes_with_keyboard_continue() {
        let input = form::Value {
            value: "synthetic-destination".to_owned(),
            warning: None,
            valid: true,
        };
        let input_type = None;
        let render = |unit| {
            liquid_send_view(
                Amount::from_sat(123_456),
                0,
                SendAsset::Lbtc,
                SendAsset::Usdt,
                ReceiveNetwork::Ethereum,
                None,
                &[],
                &input,
                &input_type,
                unit,
                "",
                false,
                false,
            )
        };
        for unit in [BitcoinDisplayUnit::Sats, BitcoinDisplayUnit::BTC] {
            for (width, height) in [(1024, 768), (1440, 900)] {
                for (appearance, theme) in [
                    ("dark", theme::Theme::dark()),
                    ("light", theme::Theme::light()),
                ] {
                    let mounted = capture::inspect(
                        match unit {
                            BitcoinDisplayUnit::Sats => "liquid-send-sats-details",
                            BitcoinDisplayUnit::BTC => "liquid-send-btc-details",
                        },
                        render(unit),
                        (width, height),
                        appearance,
                        theme,
                    );
                    assert!(mounted.contains("Payment details"));
                    assert!(mounted.contains("Destination"));
                    assert!(mounted.contains("Continue to amount"));
                    assert!(mounted.contains(&format!(
                        "Balance: {} {}",
                        Amount::from_sat(123_456).to_formatted_string_with_unit(unit),
                        unit.label()
                    )));
                    assert!(mounted.has_focus(iced::advanced::widget::Id::new(CONTINUE_ACTION_ID)));
                    let cache = Cache {
                        cube_name: "Everyday".to_owned(),
                        network: coincube_core::miniscript::bitcoin::Network::Bitcoin,
                        bitcoin_unit: unit,
                        ..Default::default()
                    };
                    let payment_page = capture::inspect_wide_content(
                        "liquid-send-payment-page",
                        send_page(
                            render(unit).map(Message::LiquidSend),
                            &cache,
                            Amount::from_sat(123_456),
                            0,
                            SendAsset::Lbtc,
                        ),
                        (width, height),
                        appearance,
                        theme,
                        width - 248 - 48,
                    );
                    for label in ["‹ Liquid", "Send", "Payment details", "Available in Liquid"] {
                        assert!(payment_page.contains(label));
                    }
                    for id in ["liquid-send-kind-lbtc", "liquid-send-kind-usdt"] {
                        assert!(payment_page.has_focus(iced::advanced::widget::Id::new(id)));
                    }
                    if width == 1440 {
                        let x = |label| {
                            mounted
                                .text
                                .iter()
                                .find(|(value, _)| value == label)
                                .expect("send heading in capture")
                                .1
                                .x
                        };
                        // Left-anchored: the bounded column starts at the
                        // view's left edge; the Cube shell adds the gutter.
                        assert!(
                            x("Last transactions") >= 0.0 && x("Last transactions") < 1.0,
                            "Last transactions at x = {}",
                            x("Last transactions")
                        );
                        assert!(
                            (x("Payment details") - x("Last transactions") - 24.0).abs() <= 4.0
                        );
                    }
                }
            }
        }
        let messages = capture::activate(
            render(BitcoinDisplayUnit::Sats),
            iced::advanced::widget::Id::new(CONTINUE_ACTION_ID),
            (1024, 768),
            false,
        );
        assert!(messages
            .iter()
            .any(|message| matches!(message, LiquidSendMessage::Send)));
    }

    #[test]
    fn route_and_picker_actions_support_keyboard_selection() {
        let input = form::Value::<String>::default();
        let input_type = None;
        let route_view = || {
            liquid_send_view(
                Amount::from_sat(123_456),
                0,
                SendAsset::Lbtc,
                SendAsset::Lbtc,
                ReceiveNetwork::Lightning,
                None,
                &[],
                &input,
                &input_type,
                BitcoinDisplayUnit::Sats,
                "",
                false,
                false,
            )
        };
        let mounted = capture::inspect(
            "liquid-send-routes",
            route_view(),
            (1024, 768),
            "dark",
            theme::Theme::dark(),
        );
        for id in [SEND_ROUTE_ACTION_ID, RECEIVE_ROUTE_ACTION_ID] {
            assert!(mounted.has_focus(iced::advanced::widget::Id::new(id)));
        }
        let open_source = capture::activate(
            route_view(),
            iced::advanced::widget::Id::new(SEND_ROUTE_ACTION_ID),
            (1024, 768),
            false,
        );
        assert!(open_source
            .iter()
            .any(|message| matches!(message, LiquidSendMessage::OpenSendPicker)));
        let open_destination = capture::activate_with_key(
            route_view(),
            iced::advanced::widget::Id::new(RECEIVE_ROUTE_ACTION_ID),
            (1024, 768),
            false,
            iced::keyboard::key::Named::Space,
        );
        assert!(open_destination
            .iter()
            .any(|message| matches!(message, LiquidSendMessage::OpenReceivePicker)));

        let source_picker = || {
            send_picker_modal(
                Amount::from_sat(123_456),
                0,
                SendAsset::Lbtc,
                BitcoinDisplayUnit::Sats,
                false,
            )
        };
        let source_mounted = capture::inspect(
            "liquid-send-source-picker",
            source_picker(),
            (1024, 768),
            "dark",
            theme::Theme::dark(),
        );
        for asset in [SendAsset::Lbtc, SendAsset::Usdt] {
            assert!(source_mounted
                .has_focus(iced::advanced::widget::Id::new(send_asset_action_id(asset))));
        }
        let select_source = capture::activate_with_key(
            source_picker(),
            iced::advanced::widget::Id::new(send_asset_action_id(SendAsset::Usdt)),
            (1024, 768),
            false,
            iced::keyboard::key::Named::Space,
        );
        assert!(select_source
            .iter()
            .any(|message| matches!(message, LiquidSendMessage::SetSendAsset(SendAsset::Usdt))));

        let target_picker = || {
            receive_picker_modal(
                SendAsset::Usdt,
                SendAsset::Usdt,
                ReceiveNetwork::Liquid,
                true,
            )
        };
        let target_mounted = capture::inspect(
            "liquid-send-target-picker",
            target_picker(),
            (1024, 768),
            "dark",
            theme::Theme::dark(),
        );
        let ethereum = receive_target_action_id(SendAsset::Usdt, ReceiveNetwork::Ethereum);
        assert!(target_mounted.has_focus(iced::advanced::widget::Id::new(ethereum)));
        let select_target = capture::activate(
            target_picker(),
            iced::advanced::widget::Id::new(ethereum),
            (1024, 768),
            false,
        );
        assert!(select_target.iter().any(|message| matches!(
            message,
            LiquidSendMessage::SetReceiveTarget(SendAsset::Usdt, ReceiveNetwork::Ethereum)
        )));
    }

    #[test]
    fn send_passive_balances_and_recent_rows_follow_global_mask() {
        let input = form::Value::<String>::default();
        let input_type = None;
        let rows = vec![
            RecentTransaction {
                description: "Bitcoin payment".into(),
                time_ago: "just now".into(),
                amount: Amount::from_sat(98_765_432),
                fees_sat: Amount::ZERO,
                fiat_amount: Some(
                    FiatAmount::new(3456.78, crate::services::fiat::Currency::USD).unwrap(),
                ),
                is_incoming: true,
                status: DomainPaymentStatus::Complete,
                details: DomainPaymentDetails::Lightning {
                    swap_id: None,
                    description: "Bitcoin payment".into(),
                    payer_note: None,
                },
                usdt_display: None,
            },
            RecentTransaction {
                description: "Token payment".into(),
                time_ago: "just now".into(),
                amount: Amount::ZERO,
                fees_sat: Amount::ZERO,
                fiat_amount: None,
                is_incoming: false,
                status: DomainPaymentStatus::Complete,
                details: DomainPaymentDetails::LiquidAsset {
                    asset_id: "usdt".into(),
                    asset_info: None,
                    description: "Token payment".into(),
                    payer_note: None,
                },
                usdt_display: Some("2468.13 USDt".into()),
            },
        ];
        let render = |mask_amounts, from_asset| {
            liquid_send_view(
                Amount::from_sat(12_345_678),
                135_791_113,
                from_asset,
                SendAsset::Lbtc,
                ReceiveNetwork::Lightning,
                None,
                &rows,
                &input,
                &input_type,
                BitcoinDisplayUnit::Sats,
                "usdt",
                true,
                mask_amounts,
            )
        };
        let hidden = capture::inspect(
            "liquid-send-hidden",
            render(true, SendAsset::Lbtc),
            (1440, 900),
            "dark",
            theme::Theme::dark(),
        );
        assert!(hidden.contains("••••••"));
        for sentinel in ["12,345,678", "98,765,432", "2468.13", "3,456.78"] {
            assert!(
                !hidden.contains(sentinel),
                "hidden Liquid send amount {}",
                sentinel
            );
        }
        let shown = capture::inspect(
            "liquid-send-shown",
            render(false, SendAsset::Lbtc),
            (1440, 900),
            "dark",
            theme::Theme::dark(),
        );
        assert!(shown.contains("12,345,678"));
        assert!(shown.contains("98,765,432"));
        assert!(shown.contains("2468.13"));
        assert!(shown.contains("3,456.78"));
        let hidden_usdt = capture::inspect(
            "liquid-send-usdt-hidden",
            render(true, SendAsset::Usdt),
            (1440, 900),
            "dark",
            theme::Theme::dark(),
        );
        let usdt_balance = format!("{} USDt", format_usdt_display(135_791_113));
        assert!(!hidden_usdt.contains(&usdt_balance));
        let shown_usdt = capture::inspect(
            "liquid-send-usdt-shown",
            render(false, SendAsset::Usdt),
            (1440, 900),
            "dark",
            theme::Theme::dark(),
        );
        assert!(shown_usdt.contains(&usdt_balance));

        let picker_hidden = capture::inspect(
            "liquid-send-picker-hidden",
            send_picker_modal(
                Amount::from_sat(12_345_678),
                135_791_113,
                SendAsset::Lbtc,
                BitcoinDisplayUnit::Sats,
                true,
            ),
            (1440, 900),
            "dark",
            theme::Theme::dark(),
        );
        assert!(!picker_hidden.contains("12,345,678"));
        assert!(!picker_hidden.contains("1.35 USDt"));
        let picker_shown = capture::inspect(
            "liquid-send-picker-shown",
            send_picker_modal(
                Amount::from_sat(12_345_678),
                135_791_113,
                SendAsset::Lbtc,
                BitcoinDisplayUnit::Sats,
                false,
            ),
            (1440, 900),
            "dark",
            theme::Theme::dark(),
        );
        assert!(picker_shown.contains("12,345,678"));
        assert!(picker_shown.contains(&usdt_balance));
    }
}
