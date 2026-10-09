use std::collections::HashMap;

use iced::{Point, Rectangle, Size};
use liana_ui::widget::graph_view::{
    geometry::{INPUT_COLUMN_WIDTH, MIDDLE_COLUMN_WIDTH, OUTPUT_COLUMN_WIDTH, SLOT_HEIGHT},
    ItemId, Shape, Side,
};

use crate::app::{
    menu::MapFocus,
    state::map::{
        display_row,
        graph::{OutputSlot, SlotRef, TxGraph},
        Orders,
    },
};

/// Slots and leaf highlighted by "Show on map" (spec 10.2).
#[derive(Debug, Clone, PartialEq)]
pub struct ShowOnMap {
    /// The output slot, plus the input slot spending it (our coin).
    pub slots: Vec<SlotRef>,
    /// Leaf index of a payment or counterparty output.
    pub leaf: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FocusLanding {
    /// Graph coordinates of the target.
    pub rect: Rectangle,
    pub select: Option<ItemId>,
    pub highlight: Option<ShowOnMap>,
}

/// `None` when the target is not on the map anymore.
pub fn resolve_focus(
    graph: &TxGraph,
    layout: &HashMap<ItemId, Point>,
    orders: &Orders,
    focus: &MapFocus,
) -> Option<FocusLanding> {
    match focus {
        MapFocus::Tx(txid) => {
            let tx = graph.tx_index(txid)?;
            let id = graph.tx_item(tx);
            let map_tx = &graph.txs()[tx];
            let size = Shape::Block {
                inputs: map_tx.inputs.len(),
                outputs: map_tx.outputs.len(),
            }
            .size();
            Some(FocusLanding {
                rect: Rectangle::new(*layout.get(&id)?, size),
                select: Some(id),
                highlight: None,
            })
        }
        MapFocus::Coin(outpoint) => {
            let tx = graph.tx_index(&outpoint.txid)?;
            let index = outpoint.vout as usize;
            let map_tx = &graph.txs()[tx];
            let output = map_tx.outputs.get(index)?;
            let slot = SlotRef {
                tx,
                side: Side::Output,
                index,
            };
            let order = orders
                .get(&outpoint.txid)
                .and_then(|(_, outputs)| outputs.as_deref());
            let block = layout.get(&graph.tx_item(tx))?;
            let rect = Rectangle::new(
                Point::new(
                    block.x + INPUT_COLUMN_WIDTH + MIDDLE_COLUMN_WIDTH,
                    block.y + display_row(order, index) as f32 * SLOT_HEIGHT,
                ),
                Size::new(OUTPUT_COLUMN_WIDTH, SLOT_HEIGHT),
            );
            let (slots, leaf) = match output {
                OutputSlot::OurCoin { .. } => {
                    let mut slots = vec![slot];
                    slots.extend(graph.spending_input(outpoint));
                    (slots, None)
                }
                OutputSlot::Payment { leaf, .. } | OutputSlot::CounterpartyOutput { leaf, .. } => {
                    (vec![slot], Some(*leaf))
                }
            };
            Some(FocusLanding {
                rect,
                select: None,
                highlight: Some(ShowOnMap { slots, leaf }),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use iced::Rectangle;
    use liana::miniscript::bitcoin::OutPoint;
    use liana_ui::widget::graph_view::{
        geometry::{INPUT_COLUMN_WIDTH, MIDDLE_COLUMN_WIDTH, SLOT_HEIGHT},
        Side,
    };

    use crate::app::{
        menu::MapFocus,
        state::map::{
            fixture,
            focus::{resolve_focus, FocusLanding},
            graph::{OutputSlot, SlotRef},
            layout,
            wallets::WalletKey,
            Orders,
        },
    };

    fn output(tx: usize, index: usize) -> SlotRef {
        SlotRef {
            tx,
            side: Side::Output,
            index,
        }
    }

    fn input(tx: usize, index: usize) -> SlotRef {
        SlotRef {
            tx,
            side: Side::Input,
            index,
        }
    }

    fn landing(
        txid: liana::miniscript::bitcoin::Txid,
        vout: u32,
        orders: &Orders,
    ) -> Option<FocusLanding> {
        let graph = fixture::graph();
        let positions = layout::reset(&graph, &WalletKey::Current);
        resolve_focus(
            &graph,
            &positions,
            orders,
            &MapFocus::Coin(OutPoint::new(txid, vout)),
        )
    }

    #[test]
    fn focus_tx_selects_its_block() {
        let fixture = fixture::sample_wallet();
        let graph = fixture::graph();
        let positions = layout::reset(&graph, &WalletKey::Current);
        let tx = graph.tx_index(&fixture.ids.salary).unwrap();
        let id = graph.tx_item(tx);
        let landing = resolve_focus(
            &graph,
            &positions,
            &Orders::new(),
            &MapFocus::Tx(fixture.ids.salary),
        )
        .unwrap();
        assert_eq!(
            landing.rect,
            Rectangle::new(positions[&id], layout::item_size(&graph, id))
        );
        assert_eq!(landing.select, Some(id));
        assert_eq!(landing.highlight, None);
    }

    #[test]
    fn focus_own_spent_coin_highlights_spending_input() {
        let ids = fixture::sample_wallet().ids;
        let graph = fixture::graph();
        let from = graph.tx_index(&ids.salary).unwrap();
        let to = graph.tx_index(&ids.rent[0]).unwrap();
        let highlight = landing(ids.salary, 0, &Orders::new())
            .unwrap()
            .highlight
            .unwrap();
        assert_eq!(highlight.slots, vec![output(from, 0), input(to, 0)]);
        assert_eq!(highlight.leaf, None);
    }

    #[test]
    fn focus_own_unspent_coin_highlights_only_its_slot() {
        let ids = fixture::sample_wallet().ids;
        let graph = fixture::graph();
        let tx = graph.tx_index(&ids.unconfirmed).unwrap();
        let highlight = landing(ids.unconfirmed, 1, &Orders::new())
            .unwrap()
            .highlight
            .unwrap();
        assert_eq!(highlight.slots, vec![output(tx, 1)]);
        assert_eq!(highlight.leaf, None);
    }

    #[test]
    fn focus_payment_highlights_its_leaf() {
        let ids = fixture::sample_wallet().ids;
        let graph = fixture::graph();
        let tx = graph.tx_index(&ids.rent[0]).unwrap();
        let OutputSlot::Payment { leaf, .. } = graph.txs()[tx].outputs[0] else {
            panic!("rent output 0 is a payment");
        };
        let landing = landing(ids.rent[0], 0, &Orders::new()).unwrap();
        let highlight = landing.highlight.unwrap();
        assert_eq!(highlight.slots, vec![output(tx, 0)]);
        assert_eq!(highlight.leaf, Some(leaf));
        assert_eq!(landing.select, None);
    }

    #[test]
    fn focus_uses_display_row() {
        let ids = fixture::sample_wallet().ids;
        let graph = fixture::graph();
        let tx = graph.tx_index(&ids.incoming_four).unwrap();
        let positions = layout::reset(&graph, &WalletKey::Current);
        let block = positions[&graph.tx_item(tx)];
        let orders = Orders::from([(ids.incoming_four, (None, Some(vec![2, 0, 1, 3, 4])))]);
        let landing = landing(ids.incoming_four, 2, &orders).unwrap();
        assert_eq!(landing.rect.y, block.y);
        assert_eq!(
            landing.rect.x,
            block.x + INPUT_COLUMN_WIDTH + MIDDLE_COLUMN_WIDTH
        );
        let landing = resolve_focus(
            &graph,
            &positions,
            &orders,
            &MapFocus::Coin(OutPoint::new(ids.incoming_four, 0)),
        )
        .unwrap();
        assert_eq!(landing.rect.y, block.y + SLOT_HEIGHT);
    }

    #[test]
    fn focus_unknown_tx_is_none() {
        let graph = fixture::graph();
        let positions = layout::reset(&graph, &WalletKey::Current);
        let foreign = fixture::foreign(99);
        assert_eq!(
            resolve_focus(
                &graph,
                &positions,
                &Orders::new(),
                &MapFocus::Tx(foreign.txid)
            ),
            None
        );
        assert_eq!(landing(foreign.txid, 0, &Orders::new()), None);
    }
}
