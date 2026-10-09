use std::hash::{Hash, Hasher};

use chrono::{DateTime, Utc};
use iced::{
    border::{Dash, Radius},
    widget::{column, container, pin, row, tooltip::Position, Space},
    Alignment, Background, Border, Color, Length,
};
use liana::label::Label;
use liana_i18n::t;

use crate::{
    color,
    component::{
        amount::{self, Amount, DisplayAmount, SignedAmount},
        label::display_label_unsnapped,
        panels::map::{
            scrim, separator, BLOCK_WIDTH, INPUT_COLUMN_WIDTH, MIDDLE_COLUMN_WIDTH,
            OUTPUT_COLUMN_WIDTH, SLOT_HEIGHT,
        },
        pill,
        text::{format_date, new},
        tooltip::tooltip_unsnapped,
    },
    icon, theme,
    theme::{
        card::{CARD_RADIUS, CARD_SHADOW, CARD_SHADOW_HOVER},
        Theme,
    },
    widget::{
        graph_view::{Shape, Side},
        Container, Element, Outline, SpaceExt, Stack,
    },
};

/// Single line at 220 px in b4_medium leaves room for about 18 chars plus the info icon.
const BLOCK_LABEL_MAX_CHARS: usize = 18;

const BLOCK_DIMMED_OPACITY: f32 = 0.45;

/// What a slot holds, for its tooltip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SlotKind {
    SpendsOurCoin,
    SpendsCounterpartyCoin,
    OurCoinSpent,
    OurCoinUnspent,
    CounterpartyOutput,
    Payment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SlotState {
    Default,
    /// Hovered, or end of a hovered edge.
    Hover,
    /// Selection or "Show on map" target.
    Highlighted,
    TagSibling,
    Reordering,
    /// Unspent coin while "Highlight unspent coins" is on.
    Unspent,
    /// Any other slot while "Highlight unspent coins" is on.
    Dimmed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SlotView {
    pub kind: SlotKind,
    /// `None` for a counterparty coin: "Unknown amount".
    pub amount: Option<Amount>,
    /// Tag names; the tag indicator shows when not empty.
    pub tags: Vec<String>,
    pub frozen: bool,
    pub selected_for_spending: bool,
    pub state: SlotState,
    /// Side bar of a coin owned by another wallet.
    pub wallet_color: Option<Color>,
}

impl Hash for SlotView {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.kind.hash(state);
        self.amount.hash(state);
        self.tags.hash(state);
        self.frozen.hash(state);
        self.selected_for_spending.hash(state);
        self.state.hash(state);
        self.wallet_color.map(Color::into_rgba8).hash(state);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlockState {
    Default,
    Hover,
    Selected,
    Dragging,
    Dimmed,
}

/// Live slot drag: the slot at `index` (live display order) follows the pointer, its top at `offset_y` from the column top.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SlotReorder {
    pub side: Side,
    pub index: usize,
    pub offset_y: f32,
}

fn accent(theme: &Theme) -> Color {
    theme.colors.general.accent
}

/// `tint`: border of a transaction of another wallet.
#[allow(clippy::too_many_arguments)]
pub fn block<'a, M: 'a>(
    label: &Label,
    time: Option<DateTime<Utc>>,
    net: SignedAmount,
    fee: Option<Amount>,
    inputs: Vec<SlotView>,
    outputs: Vec<SlotView>,
    reorder: Option<SlotReorder>,
    state: BlockState,
    group_member: bool,
    tint: Option<Color>,
) -> Element<'a, M> {
    let height = Shape::Block {
        inputs: inputs.len(),
        outputs: outputs.len(),
    }
    .size()
    .height;
    let unconfirmed = time.is_none();

    let inputs = slot_column(
        inputs,
        Side::Input,
        INPUT_COLUMN_WIDTH - 1.0,
        height,
        reorder,
    );
    let outputs = slot_column(outputs, Side::Output, OUTPUT_COLUMN_WIDTH, height, reorder);

    let label = display_label_unsnapped(label, new::B4_MEDIUM_SPEC, Some(BLOCK_LABEL_MAX_CHARS));
    let date: Element<'a, M> = match time {
        Some(time) => new::caption(format_date(time))
            .style(theme::text::secondary)
            .into(),
        None => pill::unconfirmed_compact().into(),
    };
    let net = amount::signed_amount(&net, new::B4_MEDIUM_SPEC);
    let fee = match fee {
        Some(fee) => t!("map-block-fee", amount = fee.to_formatted_string()),
        None => t!("map-block-fee-unknown"),
    };
    let fee = new::small_caption(fee).style(theme::text::tertiary);
    let selected = state == BlockState::Selected;
    let middle = Container::new(column![label, date, net, fee].spacing(6))
        .padding([14, 16])
        .width(MIDDLE_COLUMN_WIDTH - 1.0)
        .height(Length::Fill)
        .style(move |_| container::Style {
            background: selected.then_some(Background::Color(color::TRANSPARENT_GREEN)),
            ..Default::default()
        });

    let separator = || separator(1, Length::Fill);
    let card = Container::new(row![inputs, separator(), middle, separator(), outputs])
        .width(BLOCK_WIDTH)
        .height(height)
        .style(|theme: &Theme| container::Style {
            border: Border {
                radius: CARD_RADIUS.into(),
                ..Default::default()
            },
            shadow: CARD_SHADOW,
            ..theme::card::simple(theme)
        });

    let opacity_layer = match state {
        BlockState::Dragging => Some(scrim(
            |theme| theme.colors.general.background,
            0.06,
            CARD_RADIUS.into(),
        )),
        BlockState::Dimmed => Some(scrim(
            |theme| theme.colors.general.background,
            1.0 - BLOCK_DIMMED_OPACITY,
            CARD_RADIUS.into(),
        )),
        _ => None,
    };
    let layers = [Some(card.into()), opacity_layer.map(Element::from)]
        .into_iter()
        .flatten();
    let stack = Stack::with_children(layers)
        .width(BLOCK_WIDTH)
        .height(height);

    let accented = matches!(
        state,
        BlockState::Hover | BlockState::Dragging | BlockState::Selected
    );
    let plain_border = tint
        .or(unconfirmed.then_some(color::GREY_4))
        .map(|color| match state {
            BlockState::Dimmed => Color {
                a: BLOCK_DIMMED_OPACITY,
                ..color
            },
            _ => color,
        });
    let mut outlined = Outline::new(stack);
    if accented || plain_border.is_some() {
        outlined = outlined.outline(0.0, move |theme| {
            let color = match plain_border {
                Some(color) if !accented => color,
                _ => accent(theme),
            };
            let border = Border {
                color,
                width: 1.0,
                radius: CARD_RADIUS.into(),
                ..Default::default()
            };
            if unconfirmed {
                border.dashes(Dash::new(4.0, 3.0))
            } else {
                border
            }
        });
    }
    if selected {
        outlined = outlined.outline(1.0, |theme| Border {
            color: accent(theme),
            width: 1.0,
            radius: (CARD_RADIUS + 1.0).into(),
            ..Default::default()
        });
    }
    if group_member {
        outlined = outlined.outline(4.0, |theme| Border {
            color: accent(theme),
            width: 2.0,
            radius: (CARD_RADIUS + 4.0).into(),
            ..Default::default()
        });
    }
    outlined.into()
}

fn slot_column<'a, M: 'a>(
    slots: Vec<SlotView>,
    side: Side,
    width: f32,
    height: f32,
    reorder: Option<SlotReorder>,
) -> Element<'a, M> {
    let len = slots.len();
    let reorder = reorder.filter(|r| r.side == side && r.index < len);
    let mut pinned = None;
    let mut rows: Vec<Element<'a, M>> = Vec::with_capacity(len);
    for (index, mut view) in slots.into_iter().enumerate() {
        let top = if index == 0 { CARD_RADIUS } else { 0.0 };
        let bottom = if (index + 1) as f32 * SLOT_HEIGHT >= height {
            CARD_RADIUS
        } else {
            0.0
        };
        let radius = match side {
            Side::Input => Radius {
                top_left: top,
                bottom_left: bottom,
                ..Default::default()
            },
            Side::Output => Radius {
                top_right: top,
                bottom_right: bottom,
                ..Default::default()
            },
        };
        match reorder {
            Some(r) if r.index == index => {
                view.state = SlotState::Reordering;
                pinned = Some((slot(view, side, radius), r.offset_y));
                rows.push(Space::with_height(SLOT_HEIGHT).into());
            }
            _ => rows.push(slot(view, side, radius)),
        }
    }
    let column = Container::new(column(rows))
        .width(width)
        .height(height)
        .clip(true);
    match pinned {
        Some((dragged, offset_y)) => {
            let max_y = (len - 1) as f32 * SLOT_HEIGHT;
            let dragged = pin(dragged).y(offset_y.clamp(0.0, max_y));
            Container::new(Stack::with_children([column.into(), dragged.into()]))
                .width(width)
                .height(height)
                .clip(true)
                .into()
        }
        None => column.into(),
    }
}

fn slot<'a, M: 'a>(slot: SlotView, side: Side, radius: Radius) -> Element<'a, M> {
    let amount: Element<'a, M> = match slot.amount {
        Some(a) if slot.frozen => amount::amount_with_font_alpha(&a, new::CAPTION_SPEC, 0.5).into(),
        Some(a) => amount::amount_with_font(&a, new::CAPTION_SPEC).into(),
        None => new::caption(t!("map-unknown-amount"))
            .style(theme::text::tertiary)
            .into(),
    };
    let tags = (side == Side::Output && !slot.tags.is_empty()).then(|| {
        tooltip_unsnapped(
            new::caption(t!("map-slot-tags", tags = slot.tags.join(", "))),
            icon::tag_fill_icon().size(14).style(theme::text::accent),
            Position::Top,
        )
    });
    let frozen = (side == Side::Output && slot.frozen).then(|| {
        tooltip_unsnapped(
            new::caption(t!("map-slot-frozen")),
            icon::snow_icon().size(14).style(theme::text::accent),
            Position::Top,
        )
    });
    let selected = (side == Side::Output && slot.selected_for_spending).then(|| {
        tooltip_unsnapped(
            new::caption(t!("map-slot-selected")),
            icon::check_square_fill_icon()
                .size(14)
                .style(|theme: &Theme| iced::widget::text::Style {
                    color: Some(accent(theme)),
                }),
            Position::Top,
        )
    });
    let indicators = row![tags, frozen, selected].spacing(8);
    let content = row![amount, Space::fill_width(), indicators]
        .spacing(8)
        .padding([0, 12])
        .align_y(Alignment::Center)
        .height(SLOT_HEIGHT - 1.0);

    let background = match slot.state {
        SlotState::Default | SlotState::Dimmed => None,
        SlotState::Hover | SlotState::Reordering => Some(color::GREY_5),
        SlotState::Highlighted | SlotState::TagSibling | SlotState::Unspent => {
            Some(color::TRANSPARENT_GREEN)
        }
    };
    let shadow = match slot.state {
        SlotState::Reordering => CARD_SHADOW_HOVER,
        _ => Default::default(),
    };
    let body = Container::new(column![content, separator(Length::Fill, 1)])
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_| container::Style {
            background: background.map(Background::Color),
            border: Border {
                radius,
                ..Default::default()
            },
            shadow,
            ..Default::default()
        });

    let dimmed = (slot.state == SlotState::Dimmed).then(|| {
        scrim(
            |theme| theme.colors.cards.simple.background,
            1.0 - 0.35,
            radius,
        )
    });
    let selected_for_spending = slot.selected_for_spending;
    let wallet_color = slot.wallet_color;
    let bar = (selected_for_spending || wallet_color.is_some()).then(|| {
        let bar = Container::new(Space::new())
            .width(3)
            .height(Length::Fill)
            .style(move |theme: &Theme| container::Style {
                background: Some(Background::Color(match wallet_color {
                    Some(color) if !selected_for_spending => color,
                    _ => accent(theme),
                })),
                ..Default::default()
            });
        match side {
            Side::Input => row![Space::fill_width(), bar],
            Side::Output => row![bar],
        }
    });
    let layers = [
        Some(body.into()),
        dimmed.map(Element::from),
        bar.map(Element::from),
    ]
    .into_iter()
    .flatten();
    let stack = Stack::with_children(layers)
        .width(Length::Fill)
        .height(SLOT_HEIGHT);

    let outline = match slot.state {
        SlotState::TagSibling => Some((-3.0, 2.0, true)),
        SlotState::Reordering => Some((0.0, 1.0, false)),
        SlotState::Unspent => Some((0.0, 2.0, false)),
        _ => None,
    };
    let mut outlined = Outline::new(stack);
    if let Some((offset, width, dashed)) = outline {
        outlined = outlined.outline(offset, move |theme| {
            let border = Border {
                color: accent(theme),
                width,
                radius,
                ..Default::default()
            };
            if dashed {
                border.dashes(Dash::new(6.0, 4.0))
            } else {
                border
            }
        });
    }

    let kind = match slot.kind {
        SlotKind::SpendsOurCoin => t!("map-slot-spends-our-coin"),
        SlotKind::SpendsCounterpartyCoin => t!("map-slot-spends-counterparty-coin"),
        SlotKind::OurCoinSpent => t!("map-slot-our-coin-spent"),
        SlotKind::OurCoinUnspent => t!("map-slot-our-coin-unspent"),
        SlotKind::CounterpartyOutput => t!("map-slot-counterparty-output"),
        SlotKind::Payment => t!("map-slot-payment"),
    };
    tooltip_unsnapped(
        new::caption(t!("map-slot-hint", what = kind)),
        outlined,
        Position::Top,
    )
    .width(Length::Fill)
    .height(SLOT_HEIGHT)
    .into()
}
