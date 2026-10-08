use iced::{
    alignment::Horizontal,
    widget::{column, container, row, svg},
    Alignment, Background, Border, Color, Padding,
};
use liana_i18n::t;

use crate::{
    component::{
        amount::{self, Amount},
        button, pill, spinner,
        text::new,
    },
    icon, image, theme,
    theme::Theme,
    widget::{Container, Element},
};

fn accent_card(theme: &Theme) -> container::Style {
    let card = theme::card::simple(theme);
    container::Style {
        border: Border {
            color: theme.colors.general.accent,
            width: 1.0,
            ..card.border
        },
        ..card
    }
}

pub fn legend<'a, M: 'a>() -> Element<'a, M> {
    let coin_edge = image::legend_coin_edge()
        .width(40)
        .height(12)
        .opacity(0.5)
        .style(theme::svg::accent);
    let counterparty_edge = image::legend_counterparty_edge()
        .width(40)
        .height(12)
        .style(|theme: &Theme, _| svg::Style {
            color: Some(theme.colors.text.muted),
        });
    let unspent_bar = Container::new(iced::widget::Space::new())
        .width(12)
        .height(2)
        .style(|theme: &Theme| container::Style {
            background: Some(Background::Color(theme.colors.general.accent)),
            ..Default::default()
        });
    let unspent_ring = Container::new(iced::widget::Space::new())
        .width(8)
        .height(8)
        .style(|theme: &Theme| container::Style {
            background: Some(Background::Color(theme.colors.general.background)),
            border: Border {
                color: theme.colors.general.accent,
                width: 2.0,
                radius: 4.0.into(),
                ..Default::default()
            },
            ..Default::default()
        });
    let unspent_marker = row![unspent_bar, unspent_ring]
        .width(40)
        .align_y(Alignment::Center);

    let label = |text: String| new::small_caption(text).style(theme::text::secondary);
    let coin = row![coin_edge, label(t!("map-legend-coin"))]
        .spacing(10)
        .align_y(Alignment::Center);
    let counterparty = row![counterparty_edge, label(t!("map-legend-counterparty"))]
        .spacing(10)
        .align_y(Alignment::Center);
    let unspent = row![unspent_marker, label(t!("map-legend-unspent"))]
        .spacing(10)
        .align_y(Alignment::Center);

    Container::new(column![coin, counterparty, unspent].spacing(6))
        .padding([12, 16])
        .style(theme::card::simple)
        .into()
}

pub fn tag_status_bar<'a, M: 'a>(
    name: &str,
    color: Color,
    index: usize,
    count: usize,
) -> Element<'a, M> {
    let status = new::small_caption(t!("map-tag-status", position = index + 1, count = count))
        .style(theme::text::secondary);
    Container::new(
        row![
            pill::tag_dot(color, 8),
            new::b5_medium(name.to_string()),
            status
        ]
        .spacing(10)
        .align_y(Alignment::Center),
    )
    .padding([8, 16])
    .style(accent_card)
    .into()
}

pub fn coin_selection_bar<'a, M: Clone + 'a>(
    count: usize,
    total: &Amount,
    on_clear: M,
) -> Element<'a, M> {
    let check = icon::check_square_fill_icon().style(theme::text::accent);
    let selected = new::b5_medium(t!("map-coins-selected", count = count));
    let total = amount::amount_with_font(total, new::CAPTION_SPEC);
    let clear = button::btn_clear(Some(on_clear));
    Container::new(
        row![check, selected, total, clear]
            .spacing(14)
            .align_y(Alignment::Center),
    )
    .padding(Padding {
        top: 10.0,
        right: 12.0,
        bottom: 10.0,
        left: 18.0,
    })
    .style(accent_card)
    .into()
}

pub fn empty_state<'a, M: 'a>() -> Element<'a, M> {
    let icon = icon::diagram_3_icon().size(32).style(theme::text::tertiary);
    let title = new::b4_medium(t!("map-empty-title"));
    let description = new::caption(t!("map-empty-description"))
        .style(theme::text::secondary)
        .align_x(Horizontal::Center);
    Container::new(
        column![icon, title, description]
            .spacing(10)
            .align_x(Alignment::Center),
    )
    .padding([40, 48])
    .max_width(520)
    .style(theme::card::simple)
    .into()
}

pub fn loading_state<'a, M: 'a>() -> Element<'a, M> {
    let caption = new::caption(t!("map-loading")).style(theme::text::secondary);
    column![spinner::spinner(), caption]
        .spacing(10)
        .align_x(Alignment::Center)
        .into()
}
