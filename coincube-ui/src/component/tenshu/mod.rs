//! Tenshu v113 components for the redesigned presenters.
//!
//! These are presentation contracts. Callers supply typed values, stable IDs,
//! the owner's focus intent and existing intents; the components hold no
//! wallet, balance, privacy or route state. Classic components elsewhere in
//! `component` are unchanged for screens that have not been ported.

pub mod action;
pub mod arranged;
pub mod asset_row;
pub mod balance;
pub mod bleed;
pub mod cards;
pub mod chip;
pub mod copy_field;
pub mod disclosure;
pub mod field;
pub mod fit_row;
pub mod page;
pub mod page_head;
pub(crate) mod scope;
pub mod segmented;
pub mod select;
pub mod setting_row;
pub mod steps;
pub mod surface;
pub mod text;
pub mod toggle;
pub mod top_bar;

#[cfg(test)]
mod gallery;

pub use action::{chip_action, eye_action, icon_action, transfer_action, ActionButton};
pub use arranged::{arranged, Arrange, Arranged};
pub use asset_row::asset_row;
pub use balance::{balance, AmountVisibility, BalanceTier, BalanceValue, Figure, MASK};
pub use bleed::{bleed, Bleed};
pub use cards::{cards, Cards, CARD_GAP, CARD_MIN_WIDTH};
pub use chip::{status_badge, status_chip, StatusBadge, StatusChip};
pub use copy_field::{copy_field, CopyField};
pub use disclosure::{disclosure, Disclosure};
pub use field::{search_field, text_field, FieldSize, TextField};
pub use fit_row::{fit_row, FitRow};
pub use page::{content_frame, gutter_for_window, page, Page, PageWidth, PAGE_MAX};
pub use page_head::{page_head, PageHead};
pub use segmented::{
    choice_chips, nav_list, radio_list, segment, segmented, tabs, Segment, SegmentSize, Segmented,
};
pub use select::{select, Select};
pub use setting_row::{setting_row, SettingRow};
pub use steps::steps;
pub use surface::{card, panel};
pub use text::{body, caption, heading, label, mono, section, title};
pub use toggle::{toggle, Toggle, ToggleForm};
pub use top_bar::{top_bar, TopBar};

pub use crate::theme::tenshu::button::ActionKind;
pub use crate::theme::tenshu::Tone;
