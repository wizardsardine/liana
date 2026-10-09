use std::collections::{HashMap, VecDeque};

use iced::{Point, Vector};
use liana::miniscript::bitcoin::{OutPoint, Txid};
use liana_ui::widget::graph_view::Side;
use lianad::commands::GraphItem;

use crate::{
    app::{settings::WalletId, state::map::coin_ui::TagId},
    daemon::model::LabelItem,
};

pub const HISTORY_LIMIT: usize = 100;

/// Display order of one column, `None` is the true order.
pub type Order = Option<Vec<u32>>;

/// Positions and slot display orders (inputs, outputs) of the whole map.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LayoutState {
    pub positions: HashMap<GraphItem, Point>,
    pub orders: HashMap<Txid, (Order, Order)>,
}

/// A recorded action. Items are keyed by `GraphItem`, stable across map reloads.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    /// Item, position before, position after.
    Move(Vec<(GraphItem, Point, Point)>),
    /// Another wallet moved as a whole.
    Offset {
        wallet: WalletId,
        before: Vector,
        after: Vector,
    },
    Reorder {
        tx: Txid,
        side: Side,
        before: Order,
        after: Order,
    },
    Layout {
        before: LayoutState,
        after: LayoutState,
    },
    Label {
        item: LabelItem,
        before: Option<String>,
        after: Option<String>,
    },
    Select {
        coin: OutPoint,
        selected: bool,
    },
    /// `deselected`: freezing also removed the coin from the selection.
    Freeze {
        coin: OutPoint,
        frozen: bool,
        deselected: bool,
    },
    Tag {
        coin: OutPoint,
        tag: TagId,
        added: bool,
    },
    CreateTag {
        tag: TagId,
        name: String,
        coin: Option<OutPoint>,
        created: bool,
    },
}

impl Change {
    pub fn inverse(&self) -> Change {
        match self.clone() {
            Change::Move(moves) => Change::Move(
                moves
                    .into_iter()
                    .map(|(item, before, after)| (item, after, before))
                    .collect(),
            ),
            Change::Offset {
                wallet,
                before,
                after,
            } => Change::Offset {
                wallet,
                before: after,
                after: before,
            },
            Change::Reorder {
                tx,
                side,
                before,
                after,
            } => Change::Reorder {
                tx,
                side,
                before: after,
                after: before,
            },
            Change::Layout { before, after } => Change::Layout {
                before: after,
                after: before,
            },
            Change::Label {
                item,
                before,
                after,
            } => Change::Label {
                item,
                before: after,
                after: before,
            },
            Change::Select { coin, selected } => Change::Select {
                coin,
                selected: !selected,
            },
            Change::Freeze {
                coin,
                frozen,
                deselected,
            } => Change::Freeze {
                coin,
                frozen: !frozen,
                deselected,
            },
            Change::Tag { coin, tag, added } => Change::Tag {
                coin,
                tag,
                added: !added,
            },
            Change::CreateTag {
                tag,
                name,
                coin,
                created,
            } => Change::CreateTag {
                tag,
                name,
                coin,
                created: !created,
            },
        }
    }
}

#[derive(Debug, Default)]
pub struct History {
    undo: VecDeque<Change>,
    redo: Vec<Change>,
}

impl History {
    pub fn record(&mut self, change: Change) {
        self.undo.push_back(change);
        if self.undo.len() > HISTORY_LIMIT {
            self.undo.pop_front();
        }
        self.redo.clear();
    }

    /// Returns the change to apply to go back.
    pub fn undo(&mut self) -> Option<Change> {
        let change = self.undo.pop_back()?;
        let inverse = change.inverse();
        self.redo.push(change);
        Some(inverse)
    }

    /// Returns the change to apply again.
    pub fn redo(&mut self) -> Option<Change> {
        let change = self.redo.pop()?;
        self.undo.push_back(change.clone());
        Some(change)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::map::fixture;

    fn select(n: u8, selected: bool) -> Change {
        Change::Select {
            coin: fixture::foreign(n),
            selected,
        }
    }

    #[test]
    fn undo_returns_inverse_and_redo_the_change() {
        let mut history = History::default();
        history.record(select(1, true));
        assert_eq!(history.undo(), Some(select(1, false)));
        assert!(history.can_redo());
        assert_eq!(history.redo(), Some(select(1, true)));
        assert!(history.can_undo());
    }

    #[test]
    fn new_change_clears_redo() {
        let mut history = History::default();
        history.record(select(1, true));
        history.undo();
        history.record(select(2, true));
        assert!(!history.can_redo());
    }

    #[test]
    fn history_limited_to_100() {
        let mut history = History::default();
        for n in 0..=HISTORY_LIMIT as u8 {
            history.record(select(n, true));
        }
        let mut last = None;
        for _ in 0..HISTORY_LIMIT {
            last = history.undo();
            assert!(last.is_some());
        }
        assert_eq!(last, Some(select(1, false)));
        assert_eq!(history.undo(), None);
    }

    #[test]
    fn undo_on_empty_is_none() {
        let mut history = History::default();
        assert_eq!(history.undo(), None);
        assert_eq!(history.redo(), None);
    }

    #[test]
    fn inverse_is_involution() {
        let item = GraphItem::Tx(fixture::foreign(1).txid);
        let layout = LayoutState {
            positions: HashMap::from([(item, Point::new(1.0, 2.0))]),
            orders: HashMap::new(),
        };
        let moved = Change::Move(vec![(item, Point::new(0.0, 0.0), Point::new(5.0, 6.0))]);
        let changes = [
            moved.clone(),
            Change::Offset {
                wallet: WalletId::new("b".to_string(), None),
                before: Vector::new(0.0, 120.0),
                after: Vector::new(24.0, 240.0),
            },
            Change::Reorder {
                tx: fixture::foreign(1).txid,
                side: Side::Output,
                before: None,
                after: Some(vec![1, 0]),
            },
            Change::Layout {
                before: layout,
                after: LayoutState::default(),
            },
            Change::Label {
                item: LabelItem::OutPoint(fixture::foreign(1)),
                before: None,
                after: Some("a".to_string()),
            },
            select(1, true),
            Change::Freeze {
                coin: fixture::foreign(1),
                frozen: true,
                deselected: true,
            },
            Change::Tag {
                coin: fixture::foreign(1),
                tag: 2,
                added: true,
            },
            Change::CreateTag {
                tag: 4,
                name: "Rent".to_string(),
                coin: None,
                created: true,
            },
        ];
        for change in changes {
            assert_eq!(change.inverse().inverse(), change);
        }
        assert_eq!(
            moved.inverse(),
            Change::Move(vec![(item, Point::new(5.0, 6.0), Point::new(0.0, 0.0))])
        );
    }
}
