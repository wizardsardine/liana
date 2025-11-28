use iced::{
    widget::{pick_list, Space},
    Alignment, Length,
};

use liana::miniscript::bitcoin::{self, Network};
use liana_ui::{
    color,
    component::{
        button,
        text::{self, text},
    },
    icon::*,
    theme,
    widget::*,
};

use crate::app::{
    self,
    view::{BuySellMessage, Message as ViewMessage},
};

#[derive(Debug, Clone)]
pub enum BuyOrSell {
    Sell,
    Buy { address: LabelledAddress },
}

#[derive(Debug, Clone, PartialEq)]
pub struct LabelledAddress {
    pub address: bitcoin::Address,
    pub index: bitcoin::bip32::ChildNumber,
    pub label: Option<String>,
}

impl std::fmt::Display for LabelledAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.label {
            Some(l) => write!(f, "{}: {}", l, self.address),
            None => std::fmt::Display::fmt(&self.address, f),
        }
    }
}

pub enum BuySellFlowState {
    /// Detecting user's location via IP geolocation, true if geolocation failed and the user is manually prompted
    DetectingLocation(bool),
    /// Nigeria, Kenya and South Africa, ie Mavapay supported countries
    Mavapay(super::flow_state::MavapayState),
    /// Renders an interface to either generate a new address for bitcoin deposit, or skip to selling BTC
    Initialization {
        buy_or_sell_selected: Option<bool>,
        buy_or_sell: Option<BuyOrSell>, // `sell` mode always has an address generated included
        // mavapay credentials restored from the OS keyring
        mavapay_credentials: Option<(String, Vec<u8>)>,
    },
    /// A webview is currently active, and is rendered instead of a buysell UI
    WebviewRenderer { active: iced_wry::IcedWebview },
}

pub struct BuySellPanel {
    // Runtime state - determines which flow is active
    pub flow_state: BuySellFlowState,
    pub wallet: std::sync::Arc<crate::app::wallet::Wallet>,
    pub modal: app::state::vault::receive::Modal,

    // Common fields (always present)
    pub error: Option<String>,
    pub network: Network,

    // services used by several buysell providers
    pub coincube_client: crate::services::coincube::CoincubeClient,
    pub detected_country: Option<crate::services::coincube::Country>,
    pub webview_manager: iced_wry::IcedWebviewManager,
}

impl BuySellPanel {
    pub fn new(
        network: bitcoin::Network,
        wallet: std::sync::Arc<crate::app::wallet::Wallet>,
    ) -> Self {
        Self {
            // Start in detecting location state
            flow_state: BuySellFlowState::DetectingLocation(false),
            error: None,
            wallet,
            network,
            modal: app::state::vault::receive::Modal::None,
            // API state
            coincube_client: crate::services::coincube::CoincubeClient::new(),
            detected_country: None,
            webview_manager: iced_wry::IcedWebviewManager::new(),
        }
    }

    pub fn view<'a>(&'a self) -> iced::Element<'a, ViewMessage, liana_ui::theme::Theme> {
        let column = {
            let column = Column::new()
                .push(Space::with_height(60))
                // COINCUBE branding
                .push(
                    Row::new()
                        .push(
                            Row::new()
                                .push(text::h4_bold("COIN").color(color::ORANGE))
                                .push(text::h4_bold("CUBE").color(color::WHITE))
                                .spacing(0),
                        )
                        .push(Space::with_width(Length::Fixed(8.0)))
                        .push(text::h5_regular("BUY/SELL").color(color::GREY_3))
                        .align_y(Alignment::Center),
                )
                // error display
                .push_maybe(self.error.as_ref().map(|err| {
                    Container::new(text(err).size(14).color(color::RED))
                        .padding(10)
                        .style(theme::card::invalid)
                }))
                .push_maybe(
                    self.error
                        .is_some()
                        .then(|| Space::with_height(Length::Fixed(20.0))),
                )
                // render flow state
                .push({
                    let element: iced::Element<ViewMessage, theme::Theme> = match &self.flow_state {
                        BuySellFlowState::DetectingLocation(m) => self.geolocation_ux(*m).into(),
                        BuySellFlowState::Initialization {
                            buy_or_sell_selected,
                            buy_or_sell,
                            ..
                        } => match buy_or_sell.as_ref() {
                            Some(BuyOrSell::Buy { address }) => {
                                self.initialization_ux(*buy_or_sell_selected, Some(&address))
                            }
                            _ => self.initialization_ux(*buy_or_sell_selected, None),
                        }
                        .into(),
                        BuySellFlowState::Mavapay(state) => {
                            let element: iced::Element<BuySellMessage, theme::Theme> =
                                super::mavapay_ui::form(state).into();
                            element.map(|b| ViewMessage::BuySell(b))
                        }
                        BuySellFlowState::WebviewRenderer { active, .. } => {
                            BuySellPanel::webview_ux(self.network, active).into()
                        }
                    };

                    element
                });

            column
                .align_x(Alignment::Center)
                .spacing(7) // Reduced spacing for more compact layout
                .width(Length::Fill)
        };

        Container::new(column)
            .width(Length::Fill)
            .align_y(Alignment::Start)
            .align_x(Alignment::Center)
            .into()
    }

    fn webview_ux<'a>(
        network: liana::miniscript::bitcoin::Network,
        webview: &'a iced_wry::IcedWebview,
    ) -> Column<'a, ViewMessage> {
        iced::widget::column![
            webview.view(Length::Fixed(640.0), Length::Fixed(600.0)),
            // Network display banner
            Space::with_height(Length::Fixed(15.0)),
            {
                let (network_name, network_color) = match network {
                    liana::miniscript::bitcoin::Network::Bitcoin => {
                        ("Bitcoin Mainnet", color::GREEN)
                    }
                    liana::miniscript::bitcoin::Network::Testnet => {
                        ("Bitcoin Testnet", color::ORANGE)
                    }
                    liana::miniscript::bitcoin::Network::Testnet4 => {
                        ("Bitcoin Testnet4", color::ORANGE)
                    }
                    liana::miniscript::bitcoin::Network::Signet => ("Bitcoin Signet", color::BLUE),
                    liana::miniscript::bitcoin::Network::Regtest => ("Bitcoin Regtest", color::RED),
                };

                iced::widget::row![
                    // currently selected bitcoin network display
                    text("Network: ").size(12).color(color::GREY_3),
                    text(network_name).size(12).color(network_color),
                    // render a button that closes the webview
                    Space::with_width(Length::Fixed(20.0)),
                    {
                        button::secondary(Some(arrow_back()), "Start Over")
                            .on_press(ViewMessage::BuySell(BuySellMessage::ResetWidget))
                            .width(iced::Length::Fixed(300.0))
                    }
                ]
                .spacing(5)
                .align_y(Alignment::Center)
            }
        ]
    }

    fn initialization_ux<'a>(
        &'a self,
        buy_or_sell: Option<bool>,
        generated: Option<&'a LabelledAddress>,
    ) -> Column<'a, ViewMessage> {
        use iced::widget::scrollable;
        use liana_ui::component::{
            button, card,
            text::{p2_regular, Text},
        };

        let mut column = Column::new();
        column = match generated.as_ref() {
            Some(addr) => column
                .push(text("Generated Address").size(14).color(color::GREY_3))
                .push({
                    let address_text = addr.to_string();

                    card::simple(
                        Column::new()
                            .push(
                                Container::new(
                                    scrollable(
                                        Column::new()
                                            .push(Space::with_height(Length::Fixed(10.0)))
                                            .push(
                                                p2_regular(&address_text)
                                                    .small()
                                                    .style(theme::text::secondary),
                                            )
                                            // Space between the address and the scrollbar
                                            .push(Space::with_height(Length::Fixed(10.0))),
                                    )
                                    .direction(
                                        scrollable::Direction::Horizontal(
                                            scrollable::Scrollbar::new().width(2).scroller_width(2),
                                        ),
                                    ),
                                )
                                .width(Length::Fill),
                            )
                            .push(
                                Row::new()
                                    .push(
                                        button::secondary(None, "Verify on hardware device")
                                            .on_press(ViewMessage::Select(0)),
                                    )
                                    .push(Space::with_width(Length::Fill))
                                    .push(
                                        Button::new(qr_code_icon().style(theme::text::secondary))
                                            .on_press(ViewMessage::ShowQrCode(0))
                                            .style(theme::button::transparent_border),
                                    )
                                    .push(
                                        Button::new(clipboard_icon().style(theme::text::secondary))
                                            .on_press(ViewMessage::Clipboard(address_text))
                                            .style(theme::button::transparent_border),
                                    )
                                    .align_y(Alignment::Center),
                            )
                            .spacing(10),
                    )
                    .width(Length::Fill)
                })
                .push(
                    button::primary(Some(globe_icon()), "Continue")
                        .on_press_maybe(
                            self.detected_country
                                .is_some()
                                .then_some(ViewMessage::BuySell(BuySellMessage::StartSession)),
                        )
                        .width(iced::Length::Fill),
                ),
            None => column
                .push({
                    Column::new()
                        .push(
                            button::secondary(
                                Some(bitcoin_icon()),
                                "Buy Bitcoin using Fiat Currencies",
                            )
                            .on_press(ViewMessage::BuySell(BuySellMessage::SelectBuyOrSell(true)))
                            .style({
                                move |th, st| match buy_or_sell {
                                    Some(true) => liana_ui::theme::button::primary(th, st),
                                    _ => liana_ui::theme::button::secondary(th, st),
                                }
                            })
                            .padding(30)
                            .width(iced::Length::Fill),
                        )
                        .push(
                            button::secondary(
                                Some(dollar_icon()),
                                "Sell Bitcoin to a Fiat Currency",
                            )
                            .on_press(ViewMessage::BuySell(BuySellMessage::SelectBuyOrSell(false)))
                            .style({
                                move |th, st| match buy_or_sell {
                                    Some(false) => liana_ui::theme::button::primary(th, st),
                                    _ => liana_ui::theme::button::secondary(th, st),
                                }
                            })
                            .padding(30)
                            .width(iced::Length::Fill),
                        )
                        .spacing(15)
                        .padding(5)
                })
                .push(
                    iced::widget::container(Space::with_height(1))
                        .style(|_| {
                            iced::widget::container::background(iced::Background::Color(
                                color::GREY_6,
                            ))
                        })
                        .width(Length::Fill),
                )
                .push_maybe({
                    (matches!(buy_or_sell, Some(true))).then(|| {
                        button::secondary(Some(plus_icon()), "Generate New Address")
                            .on_press(ViewMessage::BuySell(BuySellMessage::CreateNewAddress))
                            .width(iced::Length::Fill)
                    })
                })
                .push_maybe({
                    (matches!(buy_or_sell, Some(false))).then(|| {
                        button::secondary(Some(globe_icon()), "Continue")
                            .on_press_maybe(
                                self.detected_country
                                    .is_some()
                                    .then_some(ViewMessage::BuySell(BuySellMessage::StartSession)),
                            )
                            .width(iced::Length::Fill)
                    })
                }),
        };

        column
            .align_x(Alignment::Center)
            .spacing(12)
            .max_width(640)
            .width(Length::Fill)
    }

    fn geolocation_ux<'a>(&'a self, manual_selection: bool) -> Column<'a, ViewMessage> {
        use liana_ui::component::text;

        match manual_selection {
            true => Column::new()
                .push(
                    pick_list(
                        crate::services::coincube::get_countries(),
                        self.detected_country.as_ref(),
                        |c| ViewMessage::BuySell(BuySellMessage::CountryDetected(Ok(c))),
                    )
                    .padding(10)
                    .placeholder("Select Country: "),
                )
                .align_x(Alignment::Center)
                .width(Length::Fill),
            false => Column::new()
                .push(Space::with_height(Length::Fixed(30.0)))
                .push(text::p1_bold("Detecting your location...").color(color::WHITE))
                .push(Space::with_height(Length::Fixed(20.0)))
                .push(text("Please wait...").size(14).color(color::GREY_3))
                .align_x(Alignment::Center)
                .spacing(10)
                .max_width(500)
                .width(Length::Fill),
        }
    }
}
