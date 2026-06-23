//! Liquid cross-asset Swap view.
//!
//! PR 1 scope: a placeholder screen so the Swap route is testable in
//! isolation from both entry points (Overview button + nav rail). The
//! single-input swap layout, rate chip, and review/confirm sub-view land
//! in later PRs.

use coincube_core::miniscript::bitcoin::Amount;
use coincube_ui::{
    color,
    component::{amount::*, button, text::*},
    icon::arrow_down_up_icon,
    theme,
    widget::Element,
};
use iced::{
    widget::{container, Column, Container, Row},
    Alignment, Background, Length,
};

use crate::app::breez_liquid::assets::format_usdt_display;
use crate::app::state::liquid::send::SendAsset;
use crate::app::view::{FiatAmountConverter, LiquidSwapMessage};

/// Inputs the Swap view needs to render. Grouped in a struct to keep the
/// `State::view` call site readable as the screen grows in later PRs.
pub struct LiquidSwapConfig<'a> {
    pub from_asset: SendAsset,
    pub to_asset: SendAsset,
    pub btc_balance: Amount,
    pub usdt_balance: u64,
    pub fiat_converter: Option<FiatAmountConverter>,
    pub bitcoin_unit: BitcoinDisplayUnit,
    pub error: Option<&'a str>,
}

fn asset_ticker(asset: SendAsset) -> &'static str {
    match asset {
        SendAsset::Lbtc => "L-BTC",
        SendAsset::Usdt => "USDt",
    }
}

/// Render the balance line for a given asset.
fn balance_line<'a>(
    asset: SendAsset,
    btc_balance: Amount,
    usdt_balance: u64,
    bitcoin_unit: BitcoinDisplayUnit,
) -> Element<'a, LiquidSwapMessage> {
    let value: Element<'a, LiquidSwapMessage> = match asset {
        SendAsset::Lbtc => amount_with_size_and_unit(&btc_balance, P2_SIZE, bitcoin_unit).into(),
        SendAsset::Usdt => text(format!("{} USDt", format_usdt_display(usdt_balance)))
            .size(P2_SIZE)
            .into(),
    };
    Row::new()
        .spacing(6)
        .align_y(Alignment::Center)
        .push(text("Balance:").size(P2_SIZE).style(theme::text::secondary))
        .push(value)
        .into()
}

pub fn liquid_swap_view(config: LiquidSwapConfig<'_>) -> Element<'_, LiquidSwapMessage> {
    let LiquidSwapConfig {
        from_asset,
        to_asset,
        btc_balance,
        usdt_balance,
        fiat_converter: _,
        bitcoin_unit,
        error,
    } = config;

    let from_card = Container::new(
        Column::new()
            .spacing(8)
            .push(
                Row::new()
                    .align_y(Alignment::Center)
                    .push(
                        text("Transfer from")
                            .size(P2_SIZE)
                            .style(theme::text::secondary),
                    )
                    .push(iced::widget::Space::new().width(Length::Fill))
                    .push(text(asset_ticker(from_asset)).size(P1_SIZE).bold()),
            )
            .push(balance_line(
                from_asset,
                btc_balance,
                usdt_balance,
                bitcoin_unit,
            )),
    )
    .padding(20)
    .width(Length::Fill)
    .style(|t| container::Style {
        background: Some(Background::Color(t.colors.cards.simple.background)),
        border: iced::Border {
            radius: 20.0.into(),
            ..Default::default()
        },
        ..Default::default()
    });

    let flip = Container::new(arrow_down_up_icon().size(20).color(color::ORANGE))
        .width(Length::Fill)
        .center_x(Length::Fill);

    let to_card = Container::new(
        Column::new()
            .spacing(8)
            .push(
                Row::new()
                    .align_y(Alignment::Center)
                    .push(
                        text("Transfer to")
                            .size(P2_SIZE)
                            .style(theme::text::secondary),
                    )
                    .push(iced::widget::Space::new().width(Length::Fill))
                    .push(text(asset_ticker(to_asset)).size(P1_SIZE).bold()),
            )
            .push(balance_line(
                to_asset,
                btc_balance,
                usdt_balance,
                bitcoin_unit,
            )),
    )
    .padding(20)
    .width(Length::Fill)
    .style(|t| container::Style {
        background: Some(Background::Color(t.colors.cards.simple.background)),
        border: iced::Border {
            radius: 20.0.into(),
            ..Default::default()
        },
        ..Default::default()
    });

    let coming_up = Container::new(
        Column::new()
            .spacing(8)
            .align_x(Alignment::Center)
            .push(
                text("Swap — coming up")
                    .bold()
                    .style(theme::text::secondary),
            )
            .push(
                text("Enter an amount and review your quote here soon.")
                    .size(P2_SIZE)
                    .style(theme::text::secondary)
                    .align_x(Alignment::Center),
            ),
    )
    .width(Length::Fill)
    .padding(30)
    .center_x(Length::Fill);

    let mut content = Column::new()
        .spacing(16)
        .max_width(560)
        .push(h4_bold("Swap"))
        .push(from_card)
        .push(flip)
        .push(to_card)
        .push(coming_up)
        .push(button::primary(None, "Continue").width(Length::Fill));

    if let Some(err) = error {
        content = content.push(
            Container::new(text(err.to_string()).size(14).color(color::RED))
                .padding(10)
                .style(theme::card::invalid)
                .width(Length::Fill),
        );
    }

    Container::new(content)
        .width(Length::Fill)
        .center_x(Length::Fill)
        .into()
}
