use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    mem,
};

use chrono::{DateTime, Utc};
use iced::{
    advanced::widget::Id,
    widget::{column, lazy, stack},
    Length, Point,
};
use liana::{
    label::Label,
    miniscript::bitcoin::{Amount, OutPoint, SignedAmount},
};
use liana_ui::{
    component::panels::map::{
        block::{block, BlockState, SlotKind, SlotState, SlotView},
        header::map_header,
        leaf::{self, LeafState},
        overlays::{empty_state, legend, loading_state},
    },
    widget::{
        graph_view::{
            Anchor, AnchorSide, Edge, EdgeKind, GraphItem, GraphView, ItemId, Shape, Side,
        },
        Container, Element,
    },
};

use crate::app::{
    state::map::{
        coin_ui::CoinUi,
        display_row,
        graph::{InputSlot, LeafKind, OutputSlot, SlotRef, TxGraph},
        Orders,
    },
    view::{MapMessage, Message},
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
    zoom: f32,
    loading: bool,
    graph_id: &Id,
) -> Element<'a, Message> {
    let unspent = graph.map(TxGraph::unspent_coins).unwrap_or_default();
    let unspent_total: Amount = unspent.iter().map(|(_, amount)| *amount).sum();
    let enabled = graph.is_some_and(|graph| !graph.is_empty());
    let header = map_header(
        zoom,
        enabled,
        false,
        false,
        false,
        false,
        false,
        unspent.len(),
        &unspent_total,
        0,
        |action| Message::Map(MapMessage::Header(action)),
    );

    let canvas: Element<'a, Message> = match graph {
        _ if loading => Container::new(loading_state()).center(Length::Fill).into(),
        Some(graph) if graph.is_empty() => {
            Container::new(empty_state()).center(Length::Fill).into()
        }
        None => Container::new(iced::widget::Space::new())
            .center(Length::Fill)
            .into(),
        Some(graph) => {
            let order_of = |slot: SlotRef| {
                let txid = &graph.txs()[slot.tx].history.txid;
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
            let slot_view = |kind: SlotKind, amount: Option<Amount>, coin: Option<&OutPoint>| {
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
                    state: SlotState::Default,
                }
            };

            let mut items = Vec::new();
            let mut markers = Vec::new();
            for (index, tx) in graph.txs().iter().enumerate() {
                let id = graph.tx_item(index);
                let (input_order, output_order) = orders
                    .get(&tx.history.txid)
                    .map(|(inputs, outputs)| (inputs.as_deref(), outputs.as_deref()))
                    .unwrap_or_default();
                let true_indexes = |order: Option<&[u32]>, len: usize| -> Vec<usize> {
                    order.map_or_else(
                        || (0..len).collect(),
                        |order| order.iter().map(|&i| i as usize).collect(),
                    )
                };
                let inputs: Vec<SlotView> = true_indexes(input_order, tx.inputs.len())
                    .into_iter()
                    .map(|i| match &tx.inputs[i] {
                        InputSlot::OurCoin { outpoint, amount } => {
                            slot_view(SlotKind::SpendsOurCoin, Some(*amount), Some(outpoint))
                        }
                        InputSlot::CounterpartyCoin { .. } => {
                            slot_view(SlotKind::SpendsCounterpartyCoin, None, None)
                        }
                    })
                    .collect();
                let outputs: Vec<SlotView> = true_indexes(output_order, tx.outputs.len())
                    .into_iter()
                    .map(|i| match &tx.outputs[i] {
                        OutputSlot::OurCoin { outpoint, amount } => {
                            let kind = if graph.is_unspent(outpoint) {
                                SlotKind::OurCoinUnspent
                            } else {
                                SlotKind::OurCoinSpent
                            };
                            slot_view(kind, Some(*amount), Some(outpoint))
                        }
                        OutputSlot::Payment { amount, .. } => {
                            slot_view(SlotKind::Payment, Some(*amount), None)
                        }
                        OutputSlot::CounterpartyOutput { amount, .. } => {
                            slot_view(SlotKind::CounterpartyOutput, Some(*amount), None)
                        }
                    })
                    .collect();
                markers.extend(
                    tx.outputs
                        .iter()
                        .enumerate()
                        .filter_map(|(i, slot)| match slot {
                            OutputSlot::OurCoin { outpoint, .. } if graph.is_unspent(outpoint) => {
                                Some(Anchor {
                                    item: id,
                                    side: AnchorSide::Output,
                                    row: display_row(output_order, i),
                                })
                            }
                            _ => None,
                        }),
                );

                let shape = Shape::Block {
                    inputs: inputs.len(),
                    outputs: outputs.len(),
                };
                let display = BlockDisplay {
                    label: graph.tx_label(index),
                    time: tx.history.datetime(),
                    net: tx.net,
                    fee: tx.fee,
                    inputs,
                    outputs,
                    state: BlockState::Default,
                    group_member: false,
                };
                let content = lazy(display, |d| {
                    block(
                        &d.label,
                        d.time,
                        d.net,
                        d.fee,
                        d.inputs.clone(),
                        d.outputs.clone(),
                        None,
                        d.state,
                        d.group_member,
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
                    state: LeafState::Default,
                    group_member: false,
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

            let coin_edges = graph.coin_edges().iter().map(|edge| Edge {
                from: anchor(edge.from),
                to: anchor(edge.to),
                kind: EdgeKind::Coin,
                active: false,
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
                    active: false,
                }
            });
            let edges: Vec<Edge> = coin_edges.chain(leaf_edges).collect();

            let graph_view = GraphView::new(graph_id.clone(), items, edges)
                .markers(markers)
                .grid(false)
                .on_event(|event| Message::Map(MapMessage::Graph(event)));
            let legend = Container::new(legend()).padding(16);
            stack![graph_view, legend]
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
