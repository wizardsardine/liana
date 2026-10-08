use std::collections::{HashMap, HashSet};

use iced::Color;
use liana::miniscript::bitcoin::OutPoint;
use liana_ui::color;

use crate::{app::state::map::history::Change, t};

/// Index in the registry.
pub type TagId = usize;

const DEFAULT_COLORS: [Color; 4] = [
    color::SUCCESS_GREEN,
    color::AMBER,
    color::BLUE,
    color::BUSINESS_BLUE,
];
const NEW_COLORS: [Color; 4] = [
    color::ORANGE,
    color::FINGERPRINT_TEXT,
    color::SOFT_BLUE,
    color::GREY_2,
];

pub fn tag_color(id: TagId) -> Color {
    match DEFAULT_COLORS.get(id) {
        Some(color) => *color,
        None => NEW_COLORS[(id - DEFAULT_COLORS.len()) % NEW_COLORS.len()],
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tag {
    pub name: String,
    pub color: Color,
}

/// Tags, frozen coins and map selection, in memory only.
#[derive(Debug, Clone)]
pub struct CoinUi {
    tags: Vec<Tag>,
    coin_tags: HashMap<OutPoint, Vec<TagId>>,
    frozen: HashSet<OutPoint>,
    selected: HashSet<OutPoint>,
}

impl Default for CoinUi {
    fn default() -> Self {
        let names = [
            t!("map-tag-savings"),
            t!("map-tag-spending"),
            t!("map-tag-business"),
            t!("map-tag-non-kyc"),
        ];
        let tags = names
            .iter()
            .enumerate()
            .map(|(id, name)| Tag {
                name: name.to_string(),
                color: tag_color(id),
            })
            .collect();
        Self {
            tags,
            coin_tags: HashMap::new(),
            frozen: HashSet::new(),
            selected: HashSet::new(),
        }
    }
}

impl CoinUi {
    pub fn tags(&self) -> &[Tag] {
        &self.tags
    }

    pub fn tag(&self, id: TagId) -> Option<&Tag> {
        self.tags.get(id)
    }

    pub fn find_tag(&self, name: &str) -> Option<TagId> {
        let name = name.trim().to_lowercase();
        self.tags
            .iter()
            .position(|tag| tag.name.to_lowercase() == name)
    }

    /// In the order added.
    pub fn coin_tags(&self, coin: &OutPoint) -> &[TagId] {
        self.coin_tags.get(coin).map_or(&[], Vec::as_slice)
    }

    pub fn coins_with_tag(&self, tag: TagId) -> Vec<OutPoint> {
        let mut coins: Vec<OutPoint> = self
            .coin_tags
            .iter()
            .filter(|(_, tags)| tags.contains(&tag))
            .map(|(coin, _)| *coin)
            .collect();
        coins.sort();
        coins
    }

    pub fn is_frozen(&self, coin: &OutPoint) -> bool {
        self.frozen.contains(coin)
    }

    pub fn is_selected(&self, coin: &OutPoint) -> bool {
        self.selected.contains(coin)
    }

    pub fn selected(&self) -> &HashSet<OutPoint> {
        &self.selected
    }

    /// `None` while the coin is frozen.
    pub fn toggle_selected(&mut self, coin: OutPoint) -> Option<Change> {
        if self.is_frozen(&coin) {
            return None;
        }
        let change = Change::Select {
            coin,
            selected: !self.is_selected(&coin),
        };
        self.apply(&change);
        Some(change)
    }

    pub fn toggle_frozen(&mut self, coin: OutPoint) -> Change {
        let frozen = !self.is_frozen(&coin);
        let change = Change::Freeze {
            coin,
            frozen,
            deselected: frozen && self.is_selected(&coin),
        };
        self.apply(&change);
        change
    }

    pub fn toggle_tag(&mut self, coin: OutPoint, tag: TagId) -> Change {
        let change = Change::Tag {
            coin,
            tag,
            added: !self.coin_tags(&coin).contains(&tag),
        };
        self.apply(&change);
        change
    }

    /// Reuses the tag named `name` (case insensitive) or creates it.
    pub fn add_tag_by_name(&mut self, coin: OutPoint, name: &str) -> Option<Change> {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        let change = match self.find_tag(name) {
            Some(tag) if self.coin_tags(&coin).contains(&tag) => return None,
            Some(tag) => Change::Tag {
                coin,
                tag,
                added: true,
            },
            None => Change::CreateTag {
                tag: self.tags.len(),
                name: name.to_string(),
                coin: Some(coin),
                created: true,
            },
        };
        self.apply(&change);
        Some(change)
    }

    /// Applies the coin changes, returns `false` for the layout and label ones.
    pub fn apply(&mut self, change: &Change) -> bool {
        match change {
            Change::Select { coin, selected } => self.set_selected(*coin, *selected),
            Change::Freeze {
                coin,
                frozen,
                deselected,
            } => {
                if *frozen {
                    self.frozen.insert(*coin);
                } else {
                    self.frozen.remove(coin);
                }
                if *deselected {
                    self.set_selected(*coin, !*frozen);
                }
            }
            Change::Tag { coin, tag, added } => self.set_tag(*coin, *tag, *added),
            Change::CreateTag {
                tag,
                name,
                coin,
                created,
            } => {
                if *created {
                    debug_assert_eq!(*tag, self.tags.len());
                    self.tags.push(Tag {
                        name: name.clone(),
                        color: tag_color(*tag),
                    });
                } else if *tag + 1 == self.tags.len() {
                    self.tags.pop();
                }
                if let Some(coin) = coin {
                    self.set_tag(*coin, *tag, *created);
                }
            }
            Change::Move(_)
            | Change::Reorder { .. }
            | Change::Layout { .. }
            | Change::Label { .. } => return false,
        }
        true
    }

    fn set_selected(&mut self, coin: OutPoint, selected: bool) {
        if selected {
            self.selected.insert(coin);
        } else {
            self.selected.remove(&coin);
        }
    }

    fn set_tag(&mut self, coin: OutPoint, tag: TagId, added: bool) {
        let tags = self.coin_tags.entry(coin).or_default();
        if added {
            if !tags.contains(&tag) {
                tags.push(tag);
            }
        } else {
            tags.retain(|t| *t != tag);
        }
        if tags.is_empty() {
            self.coin_tags.remove(&coin);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::map::fixture;

    struct Coins {
        u1: OutPoint,
        u2: OutPoint,
        u3: OutPoint,
        s1: OutPoint,
    }

    fn coins() -> Coins {
        let ids = fixture::sample_wallet().ids;
        Coins {
            u1: OutPoint::new(ids.unconfirmed, 1),
            u2: OutPoint::new(ids.batch, 13),
            u3: OutPoint::new(ids.rent[3], 0),
            s1: OutPoint::new(ids.salary, 0),
        }
    }

    #[test]
    fn default_tags() {
        let ui = CoinUi::default();
        let expected = [
            (t!("map-tag-savings"), color::SUCCESS_GREEN),
            (t!("map-tag-spending"), color::AMBER),
            (t!("map-tag-business"), color::BLUE),
            (t!("map-tag-non-kyc"), color::BUSINESS_BLUE),
        ];
        assert_eq!(ui.tags().len(), 4);
        for (tag, (name, color)) in ui.tags().iter().zip(expected) {
            assert_eq!(tag.name, name);
            assert_eq!(tag.color, color);
        }
    }

    #[test]
    fn new_tag_colors_cycle() {
        let mut ui = CoinUi::default();
        let coin = coins().u1;
        for name in ["a", "b", "c", "d", "e"] {
            ui.add_tag_by_name(coin, name);
        }
        let colors: Vec<Color> = ui.tags()[4..].iter().map(|tag| tag.color).collect();
        assert_eq!(
            colors,
            [
                color::ORANGE,
                color::FINGERPRINT_TEXT,
                color::SOFT_BLUE,
                color::GREY_2,
                color::ORANGE
            ]
        );
    }

    #[test]
    fn find_tag_case_insensitive() {
        let ui = CoinUi::default();
        assert_eq!(ui.find_tag("savings"), Some(0));
        assert_eq!(ui.find_tag(" SAVINGS "), Some(0));
    }

    #[test]
    fn add_existing_tag_by_name_does_not_create() {
        let mut ui = CoinUi::default();
        let u1 = coins().u1;
        assert_eq!(
            ui.add_tag_by_name(u1, "spending"),
            Some(Change::Tag {
                coin: u1,
                tag: 1,
                added: true
            })
        );
        assert_eq!(ui.tags().len(), 4);
        assert_eq!(ui.add_tag_by_name(u1, "spending"), None);
    }

    #[test]
    fn two_tags_and_coins_with_tag() {
        let mut ui = CoinUi::default();
        let c = coins();
        ui.add_tag_by_name(c.u1, "Savings");
        ui.add_tag_by_name(c.u1, "Spending");
        ui.add_tag_by_name(c.s1, "Savings");
        assert_eq!(ui.coin_tags(&c.u1), [0, 1]);
        let with_savings = ui.coins_with_tag(0);
        assert_eq!(with_savings.len(), 2);
        assert!(with_savings.contains(&c.u1) && with_savings.contains(&c.s1));
    }

    #[test]
    fn freeze_removes_from_selection() {
        let mut ui = CoinUi::default();
        let u2 = coins().u2;
        ui.toggle_selected(u2);
        let change = ui.toggle_frozen(u2);
        assert_eq!(
            change,
            Change::Freeze {
                coin: u2,
                frozen: true,
                deselected: true
            }
        );
        assert!(ui.is_frozen(&u2) && !ui.is_selected(&u2));
        assert_eq!(ui.toggle_selected(u2), None);
    }

    #[test]
    fn undo_freeze_restores_selection() {
        let mut ui = CoinUi::default();
        let u2 = coins().u2;
        ui.toggle_selected(u2);
        let change = ui.toggle_frozen(u2);
        assert!(ui.apply(&change.inverse()));
        assert!(!ui.is_frozen(&u2) && ui.is_selected(&u2));
    }

    #[test]
    fn create_tag_undo_removes_it() {
        let mut ui = CoinUi::default();
        let u3 = coins().u3;
        let change = ui.add_tag_by_name(u3, "Rent").unwrap();
        assert_eq!(ui.coin_tags(&u3), [4]);
        assert_eq!(ui.tags().len(), 5);
        ui.apply(&change.inverse());
        assert!(ui.coin_tags(&u3).is_empty());
        assert_eq!(ui.tags().len(), 4);
    }

    #[test]
    fn apply_ignores_layout_changes() {
        let mut ui = CoinUi::default();
        assert!(!ui.apply(&Change::Move(vec![])));
    }
}
