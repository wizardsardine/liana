use std::{
    collections::{HashMap, HashSet},
    hash::{Hash, Hasher},
    mem,
};

use chrono::{DateTime, Utc};
use iced::{
    advanced::widget::Id,
    widget::{column, lazy, stack},
    Color, Length, Point,
};
use liana::{
    label::Label,
    miniscript::bitcoin::{Address, Amount, OutPoint, SignedAmount},
};
use liana_ui::{
    component::{
        form,
        list::DeviceStatus,
        modal::{self, modal_view, ModalWidth},
        panels::map::{
            block::{block, BlockState, SlotKind, SlotReorder, SlotState, SlotView},
            header::map_header,
            leaf::{self, LeafState},
            modals::{
                coin_action_bar, import_wallet_modal_body, label_modal_body, reuse_modal_body,
                shortcuts_modal_body, tag_popover, wallets_modal_body, ExternalRow, LabelSubject,
                TxDirection, WalletRow,
            },
            overlays::{coin_selection_bar, empty_state, loading_state, tag_status_bar},
            wallet_color,
        },
        text::format_date,
    },
    theme::Theme,
    widget::{
        graph_view::{
            Anchor, AnchorSide, Edge, EdgeKind, GraphItem, GraphView, Handle, ItemId, Lane, Shape,
            Side,
        },
        text_input, Container, Element,
    },
};

use crate::{
    app::{
        state::map::{
            coin_ui::CoinUi,
            display::{label_key, DisplayState},
            display_row,
            edit::live_column,
            external::ExternalWallet,
            graph::{InputSlot, LeafKind, OutputSlot, SlotRef, TxGraph},
            import::ImportForm,
            selection::TagHighlight,
            wallets::{OtherWallet, WalletKey, WalletStatus, WalletStore},
            LabelTarget, LiveReorder, Orders, Toggles,
        },
        view::{
            label::{label_field, LabelSize},
            MapMessage, Message,
        },
    },
    daemon::model::TransactionKind,
    hw::HardwareWallet,
    t,
    view::hw::unusable_device_entry,
};

/// What a block shows, owned so `lazy` rebuilds it only when it changes.
struct BlockDisplay {
    label: Label,
    time: Option<DateTime<Utc>>,
    net: SignedAmount,
    fee: Option<Amount>,
    inputs: Vec<SlotView>,
    outputs: Vec<SlotView>,
    state: BlockState,
    group_member: bool,
    reorder: Option<SlotReorder>,
    tint: Option<Color>,
    wallet: Option<String>,
}

impl Hash for BlockDisplay {
    fn hash<H: Hasher>(&self, state: &mut H) {
        mem::discriminant(&self.label).hash(state);
        self.label.value().hash(state);
        self.time.hash(state);
        self.net.hash(state);
        self.fee.hash(state);
        self.inputs.hash(state);
        self.outputs.hash(state);
        self.state.hash(state);
        self.group_member.hash(state);
        self.tint.map(Color::into_rgba8).hash(state);
        self.wallet.hash(state);
        if let Some(reorder) = &self.reorder {
            reorder.side.hash(state);
            reorder.index.hash(state);
            reorder.offset_y.to_bits().hash(state);
        }
    }
}

#[derive(Hash)]
struct LeafDisplay {
    kind: leaf::LeafKind,
    label: Option<String>,
    id: String,
    reuse_count: Option<usize>,
    state: LeafState,
    group_member: bool,
}

#[allow(clippy::too_many_arguments)]
pub fn map_view<'a>(
    graph: Option<&'a TxGraph>,
    layout: &HashMap<ItemId, Point>,
    orders: &Orders,
    coin_ui: &CoinUi,
    display: Option<&DisplayState>,
    selected: &HashSet<ItemId>,
    tag_highlight: Option<&TagHighlight>,
    toggles: Toggles,
    command_held: bool,
    can_undo: bool,
    can_redo: bool,
    align_count: usize,
    reorder: Option<LiveReorder>,
    zoom: f32,
    loading: bool,
    graph_id: &Id,
    other_wallets: bool,
    wallet_colors: &HashMap<WalletKey, Color>,
    wallet_names: &HashMap<WalletKey, String>,
    lanes: Vec<Lane>,
    handles: Vec<Handle>,
) -> Element<'a, Message> {
    let unspent = graph.map(TxGraph::unspent_coins).unwrap_or_default();
    let unspent_total: Amount = unspent.iter().map(|(_, amount)| *amount).sum();
    let enabled = graph.is_some_and(|graph| !graph.is_empty());
    let totals = graph.filter(|_| enabled).map(|graph| {
        let coins = graph
            .txs()
            .iter()
            .flat_map(|tx| &tx.outputs)
            .filter(|slot| matches!(slot, OutputSlot::OurCoin { .. }))
            .count();
        (graph.txs().len(), coins, unspent.len())
    });
    let header = map_header(
        zoom,
        enabled,
        can_undo,
        can_redo,
        toggles.area || command_held,
        toggles.unspent,
        toggles.snap,
        toggles.lanes,
        toggles.clusters,
        unspent.len(),
        &unspent_total,
        align_count,
        other_wallets,
        totals,
        |action| Message::Map(MapMessage::Header(action)),
    );

    let canvas: Element<'a, Message> =
        match graph {
            _ if loading => Container::new(loading_state()).center(Length::Fill).into(),
            // With a wallet hidden, the lane handles stay to show it again.
            Some(graph) if graph.is_empty() && handles.iter().all(|handle| handle.displayed) => {
                Container::new(empty_state()).center(Length::Fill).into()
            }
            None => Container::new(iced::widget::Space::new())
                .center(Length::Fill)
                .into(),
            Some(graph) => {
                let display = display.expect("the display state is computed with the graph");
                let order_of = |slot: SlotRef| {
                    let txid = &graph.txs()[slot.tx].history().txid;
                    orders
                        .get(txid)
                        .and_then(|(inputs, outputs)| match slot.side {
                            Side::Input => inputs.as_deref(),
                            Side::Output => outputs.as_deref(),
                        })
                };
                let anchor = |slot: SlotRef| Anchor {
                    item: graph.tx_item(slot.tx),
                    side: match slot.side {
                        Side::Input => AnchorSide::Input,
                        Side::Output => AnchorSide::Output,
                    },
                    row: display_row(order_of(slot), slot.index),
                };
                let position = |id: ItemId| layout.get(&id).copied().unwrap_or_default();
                let color_of = |wallet: &WalletKey| wallet_colors.get(wallet).copied();
                let slot_view = |kind: SlotKind,
                                 amount: Option<Amount>,
                                 coin: Option<(&WalletKey, &OutPoint)>,
                                 state: SlotState| {
                    let (wallet_color, coin) = match coin {
                        Some((wallet, coin)) => (color_of(wallet), Some(coin)),
                        None => (None, None),
                    };
                    let tags = coin.map(|coin| {
                        coin_ui
                            .coin_tags(coin)
                            .iter()
                            .filter_map(|id| coin_ui.tag(*id))
                            .map(|tag| tag.name.clone())
                            .collect()
                    });
                    SlotView {
                        kind,
                        amount,
                        tags: tags.unwrap_or_default(),
                        frozen: coin.is_some_and(|coin| coin_ui.is_frozen(coin)),
                        selected_for_spending: coin.is_some_and(|coin| coin_ui.is_selected(coin)),
                        state,
                        wallet_color,
                    }
                };

                let mut items = Vec::new();
                let mut markers = Vec::new();
                for (index, tx) in graph.txs().iter().enumerate() {
                    let id = graph.tx_item(index);
                    let (input_order, output_order) = orders
                        .get(&tx.history().txid)
                        .map(|(inputs, outputs)| (inputs.as_deref(), outputs.as_deref()))
                        .unwrap_or_default();
                    let true_indexes = |order: Option<&[u32]>, len: usize| -> Vec<usize> {
                        order.map_or_else(
                            || (0..len).collect(),
                            |order| order.iter().map(|&i| i as usize).collect(),
                        )
                    };
                    let slot_state = |side: Side, slot_index: usize| {
                        display
                            .slots
                            .get(&SlotRef {
                                tx: index,
                                side,
                                index: slot_index,
                            })
                            .copied()
                            .unwrap_or(SlotState::Default)
                    };
                    let inputs: Vec<SlotView> = true_indexes(input_order, tx.inputs.len())
                        .into_iter()
                        .map(|i| match &tx.inputs[i] {
                            InputSlot::OurCoin {
                                wallet,
                                outpoint,
                                amount,
                            } => slot_view(
                                SlotKind::SpendsOurCoin,
                                Some(*amount),
                                Some((wallet, outpoint)),
                                slot_state(Side::Input, i),
                            ),
                            InputSlot::CounterpartyCoin { .. } => slot_view(
                                SlotKind::SpendsCounterpartyCoin,
                                None,
                                None,
                                slot_state(Side::Input, i),
                            ),
                        })
                        .collect();
                    let outputs: Vec<SlotView> = true_indexes(output_order, tx.outputs.len())
                        .into_iter()
                        .map(|i| match &tx.outputs[i] {
                            OutputSlot::OurCoin {
                                wallet,
                                outpoint,
                                amount,
                            } => {
                                let kind = if graph.is_unspent(outpoint) {
                                    SlotKind::OurCoinUnspent
                                } else {
                                    SlotKind::OurCoinSpent
                                };
                                slot_view(
                                    kind,
                                    Some(*amount),
                                    Some((wallet, outpoint)),
                                    slot_state(Side::Output, i),
                                )
                            }
                            OutputSlot::Payment { amount, .. } => slot_view(
                                SlotKind::Payment,
                                Some(*amount),
                                None,
                                slot_state(Side::Output, i),
                            ),
                            OutputSlot::CounterpartyOutput { amount, .. } => slot_view(
                                SlotKind::CounterpartyOutput,
                                Some(*amount),
                                None,
                                slot_state(Side::Output, i),
                            ),
                        })
                        .collect();
                    markers.extend(tx.outputs.iter().enumerate().filter_map(
                        |(i, slot)| match slot {
                            OutputSlot::OurCoin { outpoint, .. } if graph.is_unspent(outpoint) => {
                                Some(Anchor {
                                    item: id,
                                    side: AnchorSide::Output,
                                    row: display_row(output_order, i),
                                })
                            }
                            _ => None,
                        },
                    ));

                    let (inputs, outputs, slot_reorder) = match reorder.filter(|r| r.item == id) {
                        Some(r) => {
                            let slot_reorder = SlotReorder {
                                side: r.side,
                                index: r.to,
                                offset_y: r.offset_y,
                            };
                            match r.side {
                                Side::Input => (
                                    live_column(&inputs, r.from, r.to),
                                    outputs,
                                    Some(slot_reorder),
                                ),
                                Side::Output => (
                                    inputs,
                                    live_column(&outputs, r.from, r.to),
                                    Some(slot_reorder),
                                ),
                            }
                        }
                        None => (inputs, outputs, None),
                    };
                    let shape = Shape::Block {
                        inputs: inputs.len(),
                        outputs: outputs.len(),
                    };
                    let display = BlockDisplay {
                        label: graph.tx_label(index),
                        time: tx.history().datetime(),
                        net: tx.net,
                        fee: tx.fee,
                        inputs,
                        outputs,
                        state: display.blocks[index].0,
                        group_member: display.blocks[index].1,
                        reorder: slot_reorder,
                        tint: color_of(tx.primary()),
                        wallet: wallet_names.get(tx.primary()).cloned(),
                    };
                    let content = lazy(display, |d| {
                        block(
                            &d.label,
                            d.time,
                            d.net,
                            d.fee,
                            d.inputs.clone(),
                            d.outputs.clone(),
                            d.reorder,
                            d.state,
                            d.group_member,
                            d.tint,
                            d.wallet.as_deref(),
                        )
                    });
                    items.push(GraphItem {
                        id,
                        position: position(id),
                        shape,
                        content: content.into(),
                    });
                }
                for (index, leaf) in graph.leaves().iter().enumerate() {
                    let id = graph.leaf_item(index);
                    let (kind, id_text) = match (leaf.kind, &leaf.address) {
                        (LeafKind::CounterpartyCoin, _) | (_, None) => {
                            let kind = match leaf.kind {
                                LeafKind::CounterpartyCoin => leaf::LeafKind::Coin,
                                _ => leaf::LeafKind::Address,
                            };
                            (kind, leaf.outpoint.to_string())
                        }
                        (_, Some(address)) => (leaf::LeafKind::Address, address.to_string()),
                    };
                    let display = LeafDisplay {
                        kind,
                        label: graph.leaf_label(index).value().map(str::to_string),
                        id: id_text,
                        reuse_count: leaf
                            .address
                            .as_ref()
                            .filter(|_| leaf.reused)
                            .map(|address| graph.leaves_on_address(address).len()),
                        state: display.leaves[index].0,
                        group_member: display.leaves[index].1,
                    };
                    let content = lazy(display, |d| {
                        leaf::leaf(
                            d.kind,
                            d.label.as_deref(),
                            &d.id,
                            d.reuse_count,
                            d.state,
                            d.group_member,
                        )
                    });
                    items.push(GraphItem {
                        id,
                        position: position(id),
                        shape: Shape::Leaf,
                        content: content.into(),
                    });
                }

                let coin_edges = graph
                    .coin_edges()
                    .iter()
                    .enumerate()
                    .map(|(index, edge)| Edge {
                        from: anchor(edge.from),
                        to: anchor(edge.to),
                        kind: EdgeKind::Coin,
                        active: display.coin_edges[index],
                        color: graph.slot_wallet(edge.from).and_then(color_of),
                    });
                let leaf_edges = graph.leaves().iter().enumerate().map(|(index, leaf)| {
                    let slot = anchor(leaf.slot());
                    let item = graph.leaf_item(index);
                    let (from, to) = match leaf.kind {
                        LeafKind::CounterpartyCoin => (
                            Anchor {
                                item,
                                side: AnchorSide::LeafRight,
                                row: 0,
                            },
                            slot,
                        ),
                        LeafKind::Payment | LeafKind::CounterpartyOutput => (
                            slot,
                            Anchor {
                                item,
                                side: AnchorSide::LeafLeft,
                                row: 0,
                            },
                        ),
                    };
                    Edge {
                        from,
                        to,
                        kind: EdgeKind::Counterparty,
                        active: display.leaf_edges[index],
                        color: None,
                    }
                });
                let edges: Vec<Edge> = coin_edges.chain(leaf_edges).collect();

                let wheel_slot = tag_highlight.map(|tag| {
                    let slot = tag.slot();
                    (
                        graph.tx_item(slot.tx),
                        slot.side,
                        display_row(order_of(slot), slot.index),
                    )
                });
                let graph_view = GraphView::new(graph_id.clone(), items, edges)
                    .markers(markers)
                    .selected(selected)
                    .frame(display.frame)
                    .area_mode(toggles.area)
                    .dim_edges(toggles.unspent)
                    .wheel_slot(wheel_slot)
                    .snap(toggles.snap)
                    .grid(toggles.snap)
                    .lanes(lanes)
                    .lanes_enabled(toggles.lanes)
                    .handles(handles)
                    .on_event(|event| Message::Map(MapMessage::Graph(event)));
                let selection_bar = (!coin_ui.selected().is_empty()).then(|| {
                    let total: Amount = coin_ui
                        .selected()
                        .iter()
                        .filter_map(|coin| graph.coin(coin))
                        .map(|coin| coin.amount)
                        .sum();
                    let bar = coin_selection_bar(
                        coin_ui.selected().len(),
                        &total,
                        Message::Map(MapMessage::ClearCoinSelection),
                    );
                    let bar: Element<'a, Message> = Container::new(bar)
                        .center_x(Length::Fill)
                        .align_bottom(Length::Fill)
                        .padding([16, 0])
                        .into();
                    bar
                });
                let tag_bar = tag_highlight.filter(|tag| tag.has_many()).and_then(|tag| {
                    let (position, count) = tag.position();
                    let info = coin_ui.tag(tag.active_tag())?;
                    let bar = tag_status_bar(&info.name, info.color, position - 1, count);
                    let bar: Element<'a, Message> = Container::new(bar)
                        .center_x(Length::Fill)
                        .padding([16, 0])
                        .into();
                    Some(bar)
                });
                stack![graph_view]
                    .extend(tag_bar)
                    .extend(selection_bar)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into()
            }
        };

    column![header, canvas]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// Label modal of `target` (spec 12.1), `None` when the target is not in the graph.
pub fn label_modal<'a>(
    graph: &'a TxGraph,
    target: LabelTarget,
    editing: &'a HashMap<String, form::Value<String>>,
    coin_ui: &CoinUi,
    tag_filter: Option<&str>,
    tag_input_id: &text_input::Id,
) -> Option<Element<'a, Message>> {
    let key = label_key(graph, &target)?;
    let (label, subject, coin) = match target {
        LabelTarget::Tx(index) => {
            let tx = graph.txs().get(index)?;
            let direction = if tx.kind == TransactionKind::SendToSelf {
                TxDirection::Moved
            } else if tx.net.is_negative() {
                TxDirection::Sent
            } else {
                TxDirection::Received
            };
            let subject = LabelSubject::Transaction {
                amount: tx.net.unsigned_abs(),
                direction,
                time: tx.history().datetime(),
            };
            (graph.tx_label(index), subject, None)
        }
        LabelTarget::Slot(slot) => {
            let tx = graph.txs().get(slot.tx)?;
            let subject = match slot.side {
                Side::Input => match tx.inputs.get(slot.index)? {
                    InputSlot::OurCoin { amount, .. } => LabelSubject::Amount(*amount),
                    InputSlot::CounterpartyCoin { .. } => LabelSubject::UnknownAmount,
                },
                Side::Output => match tx.outputs.get(slot.index)? {
                    OutputSlot::OurCoin { amount, .. }
                    | OutputSlot::Payment { amount, .. }
                    | OutputSlot::CounterpartyOutput { amount, .. } => {
                        LabelSubject::Amount(*amount)
                    }
                },
            };
            (graph.slot_label(slot), subject, graph.slot_coin(slot))
        }
        LabelTarget::Leaf(index) => (
            graph.address_label(index),
            LabelSubject::Leaf(key.clone()),
            None,
        ),
    };
    let label = label_field(
        vec![key.clone()],
        editing.get(&key),
        &label,
        LabelSize::Display,
    );
    let tags = coin
        .iter()
        .flat_map(|coin| coin_ui.coin_tags(coin))
        .filter_map(|id| coin_ui.tag(*id))
        .map(|tag| (tag.name.clone(), tag.color))
        .collect();
    let action_bar = coin.filter(|coin| graph.is_unspent(coin)).map(|coin| {
        let popover = tag_filter.map(|filter| {
            let registry = coin_ui
                .tags()
                .iter()
                .enumerate()
                .map(|(id, tag)| {
                    (
                        tag.name.clone(),
                        tag.color,
                        coin_ui.coin_tags(&coin).contains(&id),
                    )
                })
                .collect();
            tag_popover(
                tag_input_id.clone(),
                filter,
                |text| Message::Map(MapMessage::TagFilterEdited(text)),
                Message::Map(MapMessage::TagCreate),
                registry,
                |id| Message::Map(MapMessage::TagToggled(id)),
                Message::Map(MapMessage::TagCreate),
            )
        });
        coin_action_bar(
            coin_ui.is_selected(&coin),
            coin_ui.is_frozen(&coin),
            tag_filter.is_some(),
            popover,
            Message::Map(MapMessage::ToggleCoinSelected),
            Message::Map(MapMessage::ToggleFrozen),
            Message::Map(MapMessage::ToggleTagPopover),
        )
    });
    let body = label_modal_body(label, subject, tags, action_bar);
    Some(modal_view(
        None::<String>,
        None,
        Some(Message::Map(MapMessage::CloseModal)),
        ModalWidth::S,
        body,
    ))
}

/// Address reuse modal (spec 12.4), `None` when the address has no leaf.
pub fn reuse_modal<'a>(graph: &TxGraph, address: &Address) -> Option<Element<'a, Message>> {
    let leaves = graph.leaves_on_address(address);
    let address_label = graph.address_label(*leaves.first()?);
    let outputs = leaves
        .iter()
        .filter_map(|&index| {
            let leaf = &graph.leaves()[index];
            let tx = &graph.txs()[leaf.tx];
            let amount = match tx.outputs.get(leaf.index)? {
                OutputSlot::OurCoin { amount, .. }
                | OutputSlot::Payment { amount, .. }
                | OutputSlot::CounterpartyOutput { amount, .. } => *amount,
            };
            Some((
                graph.tx_label(leaf.tx),
                tx.history().datetime(),
                amount,
                Message::Map(MapMessage::ReuseRowSelected(graph.leaf_item(index))),
            ))
        })
        .collect();
    let body = reuse_modal_body(
        &address_label,
        &address.to_string(),
        outputs,
        Message::Map(MapMessage::CloseModal),
    );
    Some(modal_view(None::<String>, None, None, ModalWidth::M, body))
}

/// Other wallets modal: a checkbox per wallet of `wallets` and `externals`, ticked when it is
/// in `loaded`. `switching`: a wallet is being added or removed, the checkboxes wait for it.
pub fn wallets_modal<'a>(
    wallets: &[OtherWallet],
    externals: &[ExternalWallet],
    loaded: &HashMap<WalletKey, WalletStore>,
    rescanning: &HashSet<String>,
    switching: bool,
) -> Element<'a, Message> {
    let theme = Theme::default();
    let rows = wallets
        .iter()
        .map(|wallet| WalletRow {
            name: wallet.name.clone(),
            color: wallet_color(&theme, &wallet.checksum),
            checked: loaded.contains_key(&WalletKey::Other(wallet.id.clone())),
            enabled: wallet.status == WalletStatus::Available,
            note: match wallet.status {
                WalletStatus::Available => None,
                WalletStatus::Outdated => Some(t!("map-wallets-outdated")),
                WalletStatus::Unavailable => Some(t!("map-wallets-unavailable")),
            },
            on_toggle: (!switching).then(|| {
                Message::Map(MapMessage::WalletToggled(WalletKey::Other(
                    wallet.id.clone(),
                )))
            }),
        })
        .collect();
    let externals = externals
        .iter()
        .map(|wallet| {
            let key = WalletKey::External(wallet.id.clone());
            let scanning = rescanning.contains(&wallet.id);
            ExternalRow {
                name: wallet.name.clone(),
                color: wallet_color(&theme, wallet.checksum()),
                checked: loaded.contains_key(&key),
                last_scan: wallet
                    .last_scan
                    .and_then(|time| DateTime::from_timestamp(i64::from(time), 0))
                    .map(|time| t!("map-external-last-scan", date = format_date(time))),
                scanning,
                on_toggle: (!switching).then(|| Message::Map(MapMessage::WalletToggled(key))),
                on_rescan: (!scanning)
                    .then(|| Message::Map(MapMessage::ExternalRescan(wallet.id.clone()))),
                on_remove: (!switching && !scanning)
                    .then(|| Message::Map(MapMessage::ExternalRemove(wallet.id.clone()))),
            }
        })
        .collect();
    let body = wallets_modal_body(
        rows,
        externals,
        Message::Map(MapMessage::ImportWallet),
        Message::Map(MapMessage::CloseModal),
    );
    modal_view(None::<String>, None, None, ModalWidth::M, body)
}

/// External wallet import modal: the devices of the signing device mode are clickable once the
/// form is ready.
pub fn import_modal(form: &ImportForm) -> Element<'_, Message> {
    let on_device = form.ready() && !form.scanning;
    let devices = form
        .hws
        .list
        .iter()
        .map(|hw| match hw {
            HardwareWallet::Supported {
                kind,
                fingerprint,
                alias,
                ..
            } => modal::device_entry(
                Some(format!("#{fingerprint}")),
                Some(kind),
                alias.as_ref(),
                if form.device == Some(*fingerprint) {
                    DeviceStatus::Processing
                } else {
                    DeviceStatus::None
                },
                on_device.then(|| Message::Map(MapMessage::ImportDevice(*fingerprint))),
            ),
            _ => unusable_device_entry(hw),
        })
        .collect();
    let body = import_wallet_modal_body(
        form.mode,
        |mode| Message::Map(MapMessage::ImportMode(mode)),
        &form.name,
        |name| Message::Map(MapMessage::ImportName(name)),
        &form.descriptor,
        |descriptor| Message::Map(MapMessage::ImportDescriptor(descriptor)),
        &form.account,
        |account| Message::Map(MapMessage::ImportAccount(account)),
        devices,
        form.electrum.as_ref(),
        |electrum| Message::Map(MapMessage::ImportElectrum(electrum)),
        form.error.clone(),
        form.scanning,
        form.can_import(),
        Message::Map(MapMessage::CloseModal),
        Message::Map(MapMessage::ImportConfirm),
    );
    modal_view(None::<String>, None, None, ModalWidth::M, body)
}

/// Shortcuts help modal (spec 12.5).
pub fn shortcuts_modal<'a>() -> Element<'a, Message> {
    modal_view(
        Some(t!("map-shortcuts-title")),
        None,
        Some(Message::Map(MapMessage::CloseModal)),
        ModalWidth::M,
        shortcuts_modal_body(),
    )
}
