use chrono::{DateTime, Utc};
use iced::{
    widget::{column, row, Space},
    Alignment, Color, Length,
};
use liana::label::Label;
use liana_i18n::t;

use crate::{
    component::{
        amount::{amount_with_fiat, amount_with_font, Amount, AmountSize, FiatAmount},
        button::{self, EntryWidth},
        label::{display_label, LABEL_DISPLAY_MAX_CHARS},
        panels::map::separator,
        pick_list::PICK_LIST_PADDING,
        pill::{self, Segment, SegmentTone},
        scrollable,
        text::{command_key, format_date, new, short_string},
    },
    icon, theme,
    widget::{text_input::Id, Button, Column, Container, Element, Row, SpaceExt, TextInput},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxDirection {
    Sent,
    Received,
    Moved,
}

/// What the label modal is about, for the lines under the label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LabelSubject {
    /// Absolute net amount, direction and date (`None`: unconfirmed).
    Transaction {
        amount: Amount,
        direction: TxDirection,
        time: Option<DateTime<Utc>>,
    },
    /// One of our coins, or an output that is not ours.
    Amount(Amount),
    /// Counterparty coin spent by an input.
    UnknownAmount,
    /// Leaf: full address or outpoint, shown shortened.
    Leaf(String),
}

/// Body of the label modal. `label` is the caller's label editor.
pub fn label_modal_body<'a, M: 'a>(
    label: Element<'a, M>,
    subject: LabelSubject,
    tags: Vec<(String, Color)>,
    action_bar: Option<Element<'a, M>>,
) -> Element<'a, M> {
    let details: Element<'a, M> = match subject {
        LabelSubject::Transaction {
            amount,
            direction,
            time,
        } => {
            let date = time
                .map(format_date)
                .unwrap_or_else(|| t!("pill-unconfirmed"));
            let caption = match direction {
                TxDirection::Sent => t!("map-caption-sent", date = date),
                TxDirection::Received => t!("map-caption-received", date = date),
                TxDirection::Moved => t!("map-caption-moved", date = date),
            };
            column![
                amount_with_fiat(&amount, None::<fn(Amount) -> FiatAmount>, AmountSize::M),
                new::caption(caption).style(theme::text::secondary),
            ]
            .spacing(10)
            .into()
        }
        LabelSubject::Amount(amount) => {
            amount_with_fiat(&amount, None::<fn(Amount) -> FiatAmount>, AmountSize::M)
        }
        LabelSubject::UnknownAmount => new::caption(t!("map-unknown-amount"))
            .style(theme::text::tertiary)
            .into(),
        LabelSubject::Leaf(id) => new::caption(short_string(&id, 30))
            .style(theme::text::secondary)
            .into(),
    };
    let top = column![label, details].spacing(10);
    let chips = (!tags.is_empty()).then(|| {
        Row::with_children(
            tags.into_iter()
                .map(|(name, color)| pill::tag_chip(name, color).into()),
        )
        .spacing(8)
        .wrap()
    });
    column![top, chips, action_bar]
        .spacing(20)
        .width(Length::Fill)
        .into()
}

/// Select / Freeze / + pill under a divider, with the tag popover above when open.
#[allow(clippy::too_many_arguments)]
pub fn coin_action_bar<'a, M: Clone + 'a>(
    selected: bool,
    frozen: bool,
    tags_open: bool,
    popover: Option<Element<'a, M>>,
    on_select: M,
    on_freeze: M,
    on_tags: M,
) -> Element<'a, M> {
    let segments = vec![
        Segment {
            icon: icon::check_square_fill_icon(),
            label: Some(t!("btn-select")),
            tooltip: Some(if frozen {
                t!("map-select-frozen-tooltip")
            } else {
                t!("map-select-tooltip")
            }),
            tone: SegmentTone::Accent,
            on: selected,
            msg: (!frozen).then_some(on_select),
        },
        Segment {
            icon: icon::snow_icon(),
            label: Some(t!("map-freeze")),
            tooltip: Some(t!("map-freeze-tooltip")),
            tone: SegmentTone::Cold,
            on: frozen,
            msg: Some(on_freeze),
        },
        Segment {
            icon: icon::plus_icon(),
            label: None,
            tooltip: Some(t!("map-add-tag-tooltip")),
            tone: SegmentTone::Accent,
            on: tags_open,
            msg: Some(on_tags),
        },
    ];
    let popover_row = popover.map(|popover| {
        Container::new(Column::new().push(popover).push(Space::with_height(10)))
            .center_x(Length::Fill)
    });
    let pill_row = Container::new(pill::segmented_pill(segments)).center_x(Length::Fill);
    column![
        popover_row,
        separator(Length::Fill, 1),
        Space::with_height(18),
        pill_row,
    ]
    .into()
}

/// Tag picker: filter input, registry rows and a create row.
#[allow(clippy::too_many_arguments)]
pub fn tag_popover<'a, M: Clone + 'a>(
    input_id: Id,
    filter: &str,
    on_filter: impl Fn(String) -> M + 'a,
    on_submit: M,
    tags: Vec<(String, Color, bool)>,
    on_toggle: impl Fn(usize) -> M + 'a,
    on_create: M,
) -> Element<'a, M> {
    let input = TextInput::new(t!("map-tag-filter-placeholder"), filter)
        .id(input_id)
        .on_input(on_filter)
        .on_submit(on_submit)
        .style(theme::text_input::fee)
        .padding(10)
        .size(16);
    let needle = filter.trim().to_lowercase();
    let exists = tags
        .iter()
        .any(|(name, _, _)| name.to_lowercase() == needle);
    let rows = tags
        .into_iter()
        .enumerate()
        .filter(|(_, (name, _, _))| name.to_lowercase().contains(&needle))
        .map(|(i, (name, color, has))| {
            let check: Element<'a, M> = if has {
                icon::check_icon()
                    .size(14)
                    .style(theme::text::accent)
                    .into()
            } else {
                Space::with_width(14).into()
            };
            let content = row![check, pill::tag_dot(color, 12), new::b5_medium(name)]
                .spacing(10)
                .align_y(Alignment::Center);
            option_button(content.into(), on_toggle(i))
        });
    let create = (!needle.is_empty() && !exists).then(|| {
        let content = row![
            icon::plus_icon().size(15),
            new::b5_medium(t!("map-tag-create", name = filter.trim())),
        ]
        .spacing(10)
        .align_y(Alignment::Center);
        option_button(content.into(), on_create)
    });
    let list = Column::with_children(rows).push(create);
    Container::new(column![input, separator(Length::Fill, 1), list])
        .width(260)
        .style(theme::pick_list::menu_container)
        .into()
}

fn option_button<'a, M: Clone + 'a>(content: Element<'a, M>, on_press: M) -> Element<'a, M> {
    Button::new(Container::new(content).padding(PICK_LIST_PADDING))
        .style(theme::pick_list::option)
        .width(Length::Fill)
        .padding(0)
        .on_press(on_press)
        .into()
}

/// Address reuse modal body with its own title row. `outputs`: transaction label,
/// date (`None`: unconfirmed), amount and the message selecting that leaf.
pub fn reuse_modal_body<'a, M: Clone + 'a>(
    address_label: &Label,
    address: &str,
    outputs: Vec<(Label, Option<DateTime<Utc>>, Amount, M)>,
    close: M,
) -> Element<'a, M> {
    let title = row![
        icon::exclamation_circle_fill_icon()
            .size(22)
            .style(theme::text::error),
        new::b1_bold(t!("map-reuse-title")),
        Space::fill_width(),
        button::btn_modal_close(Some(close)),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    let description = new::caption(t!("map-reuse-description", count = outputs.len()))
        .style(theme::text::secondary);
    let address = column![
        display_label(address_label, new::B5_MEDIUM_SPEC, None),
        new::caption(short_string(address, 30)).style(theme::text::secondary),
    ]
    .spacing(4);
    let rows = Column::with_children(outputs.into_iter().map(|(label, time, amount, msg)| {
        let date = time
            .map(format_date)
            .unwrap_or_else(|| t!("pill-unconfirmed"));
        let content = row![
            column![
                display_label(&label, new::B4_MEDIUM_SPEC, Some(LABEL_DISPLAY_MAX_CHARS)),
                new::caption(date).style(theme::text::secondary),
            ]
            .spacing(2),
            Space::fill_width(),
            amount_with_font(&amount, new::CAPTION_SPEC),
            icon::chevron_right(),
        ]
        .spacing(10)
        .align_y(Alignment::Center);
        button::list_entry(content, None, EntryWidth::Fill, Some(msg))
    }))
    .spacing(8);
    column![title, description, address, rows]
        .spacing(15)
        .into()
}

/// Shortcuts help modal body: four groups of rows with key chips.
pub fn shortcuts_modal_body<'a, M: 'a>() -> Element<'a, M> {
    let ctrl = command_key();
    let groups = [
        (
            t!("map-shortcuts-navigate"),
            vec![
                (t!("map-shortcut-pan"), vec![t!("map-key-drag-canvas")]),
                (t!("map-shortcut-zoom"), vec![t!("map-key-wheel")]),
            ],
        ),
        (
            t!("map-shortcuts-select"),
            vec![
                (t!("map-shortcut-select-one"), vec![t!("map-key-click")]),
                (
                    t!("map-shortcut-toggle-one"),
                    vec![ctrl.clone(), t!("map-key-click")],
                ),
                (
                    t!("map-shortcut-select-path"),
                    vec![t!("map-key-shift"), t!("map-key-click")],
                ),
                (
                    t!("map-shortcut-select-chain"),
                    vec![ctrl.clone(), t!("map-key-shift"), t!("map-key-click")],
                ),
                (
                    t!("map-shortcut-area"),
                    vec![t!("map-key-hold", key = ctrl.as_str()), t!("map-key-drag")],
                ),
                (
                    t!("map-shortcut-add-area"),
                    vec![t!("map-key-shift"), t!("map-key-drag")],
                ),
                (t!("map-shortcut-clear"), vec![t!("map-key-esc")]),
            ],
        ),
        (
            t!("map-shortcuts-edit"),
            vec![
                (t!("map-shortcut-move"), vec![t!("map-key-drag")]),
                (
                    t!("map-shortcut-reorder"),
                    vec![t!("map-key-drag-slot"), t!("map-key-up-down")],
                ),
                (
                    t!("map-shortcut-tag-highlight"),
                    vec![t!("map-key-click-slot")],
                ),
                (
                    t!("map-shortcut-edit-slot"),
                    vec![t!("map-key-double-click-slot")],
                ),
                (
                    t!("map-shortcut-edit-leaf"),
                    vec![t!("map-key-double-click-leaf")],
                ),
                (
                    t!("map-shortcut-edit-tx"),
                    vec![t!("map-key-double-click-tx")],
                ),
                (
                    t!("map-shortcut-reuse-highlight"),
                    vec![t!("map-key-click-red-leaf")],
                ),
                (
                    t!("map-shortcut-reuse-list"),
                    vec![t!("map-key-double-click-red-leaf")],
                ),
                (
                    t!("map-shortcut-reuse-select"),
                    vec![ctrl.clone(), t!("map-key-click-red-leaf")],
                ),
                (
                    t!("map-shortcut-switch-tag"),
                    vec![t!("map-key-wheel-slot")],
                ),
                (t!("map-shortcut-undo"), vec![ctrl.clone(), "Z".to_string()]),
                (
                    t!("map-shortcut-redo"),
                    vec![ctrl, t!("map-key-shift"), "Z".to_string()],
                ),
                (t!("map-shortcut-help"), vec!["?".to_string()]),
            ],
        ),
        (
            t!("map-shortcuts-view"),
            vec![(t!("map-shortcut-unspent"), vec!["U".to_string()])],
        ),
    ];
    let groups = Column::with_children(groups.into_iter().map(|(title, rows)| {
        let rows = rows.into_iter().map(|(what, chips)| {
            let keys =
                Row::with_children(chips.into_iter().map(|k| pill::key_chip(k).into())).spacing(6);
            row![new::b5_medium(what), Space::fill_width(), keys]
                .spacing(16)
                .align_y(Alignment::Center)
                .into()
        });
        column![
            new::caption(title).style(theme::text::tertiary),
            Column::with_children(rows).spacing(8),
        ]
        .spacing(8)
        .into()
    }))
    .spacing(18);
    scrollable::vertical(groups).height(Length::Shrink).into()
}
