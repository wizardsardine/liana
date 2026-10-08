use iced::{
    border::Dash,
    widget::{container, row, tooltip::Position},
    Alignment, Background, Border, Color,
};
use liana_i18n::t;

use crate::{
    color,
    component::{
        panels::map::{scrim, LEAF_HEIGHT, LEAF_WIDTH},
        text::{new, short_string, truncate},
        tooltip::tooltip_unsnapped,
    },
    icon, theme,
    theme::{
        card::{CARD_RADIUS, CARD_SHADOW_HOVER},
        Theme,
    },
    widget::{Container, Element, Outline, Stack},
};

/// Single line at 168 px usable in b5_medium.
const LEAF_LABEL_MAX_CHARS: usize = 18;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LeafKind {
    /// Payment or counterparty output: an address.
    Address,
    /// Counterparty coin spent by one of our transactions.
    Coin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LeafState {
    Default,
    Hover,
    Selected,
    Sibling,
    Dragging,
    /// Any leaf while "Highlight unspent coins" is on.
    Dimmed,
}

fn accent(theme: &Theme) -> Color {
    theme.colors.general.accent
}

/// `label` is the effective label, `id` the full address or outpoint shown without one.
/// `reuse_count: Some(n)` flags an address appearing in `n` outputs.
pub fn leaf<'a, M: 'a>(
    kind: LeafKind,
    label: Option<&str>,
    id: &str,
    reuse_count: Option<usize>,
    state: LeafState,
    group_member: bool,
) -> Element<'a, M> {
    let reused = reuse_count.is_some();
    let icon = match (reused, kind) {
        (true, _) => icon::exclamation_circle_fill_icon().style(theme::text::error),
        (false, LeafKind::Address) => icon::person_icon().style(theme::text::tertiary),
        (false, LeafKind::Coin) => icon::bitcoin_icon().style(theme::text::tertiary),
    }
    .size(16);
    let text: Element<'a, M> = match label {
        Some(label) => new::b5_medium(truncate(label, LEAF_LABEL_MAX_CHARS))
            .style(theme::text::primary)
            .into(),
        None => new::caption(short_string(id, 20))
            .style(theme::text::secondary)
            .into(),
    };
    let content = row![icon, text].spacing(8).align_y(Alignment::Center);

    let card = Container::new(content)
        .width(LEAF_WIDTH)
        .height(LEAF_HEIGHT)
        .padding([0, 12])
        .align_y(Alignment::Center)
        .style(move |theme: &Theme| {
            let background = match state {
                LeafState::Selected | LeafState::Sibling => color::TRANSPARENT_GREEN,
                LeafState::Hover if reused => color::GREY_5,
                _ => theme.colors.general.background,
            };
            let border = match (reused, state) {
                (true, _) => (theme.colors.text.error, 2.0),
                (false, LeafState::Hover | LeafState::Selected | LeafState::Dragging) => {
                    (accent(theme), 1.0)
                }
                _ => (theme.colors.text.border, 1.0),
            };
            container::Style {
                background: Some(Background::Color(background)),
                border: Border {
                    color: border.0,
                    width: border.1,
                    radius: CARD_RADIUS.into(),
                    ..Default::default()
                },
                shadow: if state == LeafState::Dragging {
                    CARD_SHADOW_HOVER
                } else {
                    Default::default()
                },
                ..theme::card::simple(theme)
            }
        });

    let dimmed = (state == LeafState::Dimmed).then(|| {
        scrim(
            |theme| theme.colors.general.background,
            1.0 - 0.35,
            CARD_RADIUS.into(),
        )
    });
    let layers = [Some(card.into()), dimmed.map(Element::from)]
        .into_iter()
        .flatten();
    let stack = Stack::with_children(layers)
        .width(LEAF_WIDTH)
        .height(LEAF_HEIGHT);

    let mut outlined = Outline::new(stack);
    if group_member {
        outlined = outlined.outline(3.0, |theme| Border {
            color: accent(theme),
            width: 2.0,
            radius: (CARD_RADIUS + 3.0).into(),
            ..Default::default()
        });
    } else if state == LeafState::Sibling {
        outlined = outlined.outline(3.0, |theme| {
            Border {
                color: accent(theme),
                width: 2.0,
                radius: (CARD_RADIUS + 3.0).into(),
                ..Default::default()
            }
            .dashes(Dash::new(6.0, 4.0))
        });
    }

    match reuse_count {
        Some(count) => tooltip_unsnapped(
            new::caption(t!("map-leaf-reuse-tooltip", count = count)),
            outlined,
            Position::Top,
        )
        .into(),
        None => outlined.into(),
    }
}
