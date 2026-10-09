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
        checkbox::checkbox_button_maybe,
        form::{Form, Value},
        label::{display_label, LABEL_DISPLAY_MAX_CHARS},
        panels::map::separator,
        pick_list::PICK_LIST_PADDING,
        pill::{self, Segment, SegmentTone},
        scrollable, spinner,
        tab::tab_header,
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

/// One wallet of the other wallets modal.
#[derive(Debug, Clone)]
pub struct WalletRow<M> {
    pub name: String,
    pub color: Color,
    pub checked: bool,
    pub enabled: bool,
    /// Why the wallet is disabled, e.g. its database is outdated.
    pub note: Option<String>,
    pub on_toggle: Option<M>,
}

/// One imported wallet of the other wallets modal.
#[derive(Debug, Clone)]
pub struct ExternalRow<M> {
    pub name: String,
    pub color: Color,
    pub checked: bool,
    /// Formatted date of the last scan, e.g. "Scanned Jun 5, 2026".
    pub last_scan: Option<String>,
    /// Shows a spinner in place of the rescan button.
    pub scanning: bool,
    pub on_toggle: Option<M>,
    pub on_rescan: Option<M>,
    pub on_remove: Option<M>,
}

/// Other wallets modal body with its own title row: a checkbox per wallet adds it to the map,
/// then the imported wallets and the import button.
pub fn wallets_modal_body<'a, M: Clone + 'a>(
    rows: Vec<WalletRow<M>>,
    externals: Vec<ExternalRow<M>>,
    on_import: M,
    on_close: M,
) -> Element<'a, M> {
    let title = row![
        new::b1_bold(t!("map-wallets-title")),
        Space::fill_width(),
        button::btn_modal_close(Some(on_close)),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    let description = new::caption(t!("map-wallets-description")).style(theme::text::secondary);
    let list: Element<'a, M> = if rows.is_empty() {
        new::b5_medium(t!("map-wallets-empty"))
            .style(theme::text::tertiary)
            .into()
    } else {
        Column::with_children(rows.into_iter().map(|wallet| {
            let check =
                checkbox_button_maybe(wallet.checked, wallet.on_toggle.filter(|_| wallet.enabled));
            let note = wallet
                .note
                .map(|note| new::caption(note).style(theme::text::tertiary));
            let name = column![new::b5_medium(wallet.name), note].spacing(2);
            row![check, pill::tag_dot(wallet.color, 10), name]
                .spacing(10)
                .align_y(Alignment::Center)
                .into()
        }))
        .spacing(12)
        .into()
    };
    let external_title =
        new::caption(t!("map-external-wallets-title")).style(theme::text::tertiary);
    let external_list: Element<'a, M> = if externals.is_empty() {
        new::b5_medium(t!("map-external-wallets-empty"))
            .style(theme::text::tertiary)
            .into()
    } else {
        Column::with_children(externals.into_iter().map(|wallet| {
            let check = checkbox_button_maybe(wallet.checked, wallet.on_toggle);
            let last_scan = wallet
                .last_scan
                .map(|scan| new::caption(scan).style(theme::text::tertiary));
            let name = column![new::b5_medium(wallet.name), last_scan].spacing(2);
            let rescan: Element<'a, M> = if wallet.scanning {
                spinner::spinner()
            } else {
                button::btn_rescan(wallet.on_rescan).into()
            };
            row![
                check,
                pill::tag_dot(wallet.color, 10),
                name,
                Space::fill_width(),
                rescan,
                button::btn_remove(wallet.on_remove),
            ]
            .spacing(10)
            .align_y(Alignment::Center)
            .into()
        }))
        .spacing(12)
        .into()
    };
    let import = button::btn_import_wallet(Some(on_import));
    column![
        title,
        description,
        list,
        separator(Length::Fill, 1),
        external_title,
        external_list,
        import,
    ]
    .spacing(15)
    .into()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportMode {
    Descriptor,
    SigningDevice,
}

/// Import form of an external wallet. `electrum`: asked only when the current wallet
/// has no Electrum server. `devices`: the device entries built by the caller.
#[allow(clippy::too_many_arguments)]
pub fn import_wallet_modal_body<'a, M: Clone + 'static>(
    mode: ImportMode,
    on_mode: impl Fn(ImportMode) -> M,
    name: &Value<String>,
    on_name: impl Fn(String) -> M + 'static,
    descriptor: &Value<String>,
    on_descriptor: impl Fn(String) -> M + 'static,
    account: &Value<String>,
    on_account: impl Fn(String) -> M + 'static,
    devices: Vec<Element<'a, M>>,
    electrum: Option<&Value<String>>,
    on_electrum: impl Fn(String) -> M + 'static,
    error: Option<String>,
    scanning: bool,
    can_import: bool,
    on_cancel: M,
    on_import: M,
) -> Element<'a, M> {
    let title = new::b1_bold(t!("map-import-title"));
    let tabs = tab_header(
        &[
            (ImportMode::Descriptor, t!("map-import-descriptor"), None),
            (
                ImportMode::SigningDevice,
                t!("map-import-signing-device"),
                None,
            ),
        ],
        &mode,
        |mode| on_mode(*mode),
    );
    let name_placeholder = t!("map-import-name-placeholder");
    let name = if scanning {
        Form::new_disabled(name_placeholder, name)
    } else {
        Form::new(name_placeholder, name, on_name)
    };
    let source: Element<'a, M> = match mode {
        ImportMode::Descriptor => {
            let placeholder = t!("map-import-descriptor-placeholder");
            if scanning {
                Form::new_disabled(placeholder, descriptor).into()
            } else {
                Form::new(placeholder, descriptor, on_descriptor).into()
            }
        }
        ImportMode::SigningDevice => {
            let placeholder = t!("map-import-account-placeholder");
            let account = if scanning {
                Form::new_disabled(placeholder, account)
            } else {
                Form::new(placeholder, account, on_account)
            }
            .label(t!("map-import-account"));
            let devices: Element<'a, M> = if devices.is_empty() {
                new::caption(t!("map-import-no-device"))
                    .style(theme::text::tertiary)
                    .into()
            } else {
                Column::with_children(devices).spacing(8).into()
            };
            column![account, devices].spacing(15).into()
        }
    };
    let electrum = electrum.map(|electrum| {
        let placeholder = t!("map-import-electrum-placeholder");
        if scanning {
            Form::new_disabled(placeholder, electrum)
        } else {
            Form::new(placeholder, electrum, on_electrum)
        }
        .label(t!("map-import-electrum"))
    });
    let error = error.map(|error| new::caption(error).style(theme::text::error));
    let progress = scanning.then(|| {
        column![
            spinner::spinner(),
            new::caption(t!("map-import-scanning")).style(theme::text::secondary),
        ]
        .spacing(10)
        .align_x(Alignment::Center)
        .width(Length::Fill)
    });
    let footer = row![
        Space::fill_width(),
        button::btn_cancel(Some(on_cancel)),
        button::btn_import((can_import && !scanning).then_some(on_import)),
    ]
    .spacing(10);
    column![title, tabs, name, source, electrum, error, progress, footer]
        .spacing(15)
        .width(Length::Fill)
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
                    t!("map-shortcut-select-wallet"),
                    vec![ctrl.clone(), t!("map-key-alt"), t!("map-key-click")],
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
                    t!("map-shortcut-space"),
                    vec![t!("map-key-space"), t!("map-key-drag")],
                ),
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
