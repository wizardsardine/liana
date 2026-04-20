//! Reusable horizontal tab bar for in-content sub-navigation.
//!
//! Used when a panel has sub-sub-navigation that doesn't belong in the
//! left nav rails (e.g. P2P Overview/My Trades/Chat/Create Order/Settings,
//! or per-wallet Settings sub-pages). See
//! `plans/PLAN-two-rail-left-nav-redesign.md` §12.

use crate::{
    component::text,
    theme,
    widget::{Button, Element, Row, Text},
};
use iced::{widget::row, Alignment, Length};

pub struct Tab<'a, Message> {
    pub label: &'a str,
    pub icon: Option<Text<'a>>,
    pub active: bool,
    pub on_select: Message,
}

/// Horizontal tab bar. Active tab uses the "pressed" tab style (orange
/// accent); inactive tabs use the flat tab style. Caller decides which
/// tab is active by setting `Tab::active`.
pub fn bar<'a, Message: Clone + 'a>(tabs: Vec<Tab<'a, Message>>) -> Element<'a, Message> {
    let mut row_widget: Row<Message> = row![].spacing(2).align_y(Alignment::Center);

    for tab in tabs {
        let mut label_row: Row<Message> = row![].spacing(8).align_y(Alignment::Center);
        if let Some(icon) = tab.icon {
            label_row = label_row.push(icon);
        }
        label_row = label_row.push(text::p2_regular(tab.label));

        let btn: Button<Message> = Button::new(label_row)
            .padding([8, 16])
            .style(if tab.active {
                theme::button::tab_liquid
            } else {
                theme::button::tab
            })
            .on_press(tab.on_select);

        row_widget = row_widget.push(btn);
    }

    row_widget.width(Length::Fill).into()
}
