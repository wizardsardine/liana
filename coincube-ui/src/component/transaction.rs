use crate::{
    component::{amount, amount::BitcoinDisplayUnit, badge, text},
    theme,
    widget::{Column, Container, Element, Row},
};
use bitcoin::Amount;
use iced::{widget::button, Alignment, Length};

use chrono::{DateTime, Local, Utc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionDirection {
    Incoming,
    Outgoing,
    SelfTransfer,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TransactionBadge {
    Unconfirmed,
    Batch,
    Recovery,
}

pub struct TransactionListItem<'a, T> {
    direction: TransactionDirection,
    label: Option<String>,
    timestamp: Option<DateTime<Utc>>,
    time_ago: Option<String>,
    badges: Vec<TransactionBadge>,
    amount: &'a Amount,
    bitcoin_unit: BitcoinDisplayUnit,
    fiat_amount: Option<String>,
    amount_override: Option<String>,
    custom_status: Option<Element<'static, T>>,
    /// Custom icon element replacing the default direction badge.
    custom_icon: Option<Element<'static, T>>,
    /// Whether to show the direction badge (receive/spend/cycle arrow).
    show_direction_badge: bool,
}

impl<'a, T> TransactionListItem<'a, T> {
    pub fn new(
        direction: TransactionDirection,
        amount: &'a Amount,
        bitcoin_unit: BitcoinDisplayUnit,
    ) -> Self {
        Self {
            direction,
            label: None,
            timestamp: None,
            time_ago: None,
            badges: Vec::new(),
            amount,
            bitcoin_unit,
            fiat_amount: None,
            amount_override: None,
            custom_status: None,
            custom_icon: None,
            show_direction_badge: true,
        }
    }

    pub fn with_label(mut self, label: String) -> Self {
        self.label = Some(label);
        self
    }

    pub fn with_timestamp(mut self, timestamp: DateTime<Utc>) -> Self {
        self.timestamp = Some(timestamp);
        self
    }

    pub fn with_time_ago(mut self, time_ago: String) -> Self {
        self.time_ago = Some(time_ago);
        self
    }

    pub fn with_badge(mut self, badge: TransactionBadge) -> Self {
        self.badges.push(badge);
        self
    }

    pub fn with_badges(mut self, badges: Vec<TransactionBadge>) -> Self {
        self.badges = badges;
        self
    }

    pub fn with_fiat_amount(mut self, fiat_amount: String) -> Self {
        self.fiat_amount = Some(fiat_amount);
        self
    }

    /// Replace the primary amount display with a plain string (e.g. "5.00 USDt").
    pub fn with_amount_override(mut self, s: String) -> Self {
        self.amount_override = Some(s);
        self
    }

    pub fn with_custom_status(mut self, status: Element<'static, T>) -> Self {
        self.custom_status = Some(status);
        self
    }

    /// Replace the default direction badge with a custom icon element.
    pub fn with_custom_icon(mut self, icon: Element<'static, T>) -> Self {
        self.custom_icon = Some(icon);
        self
    }

    /// Show or hide the direction badge (receive/spend/cycle arrow).
    pub fn with_show_direction_badge(mut self, show: bool) -> Self {
        self.show_direction_badge = show;
        self
    }

    pub fn view(self, on_press: T) -> Container<'static, T>
    where
        T: Clone + 'static,
    {
        self.build_view(Some(on_press))
    }

    pub fn view_readonly(self) -> Container<'static, T>
    where
        T: Clone + 'static,
    {
        self.build_view(None)
    }

    fn build_view(self, on_press: Option<T>) -> Container<'static, T>
    where
        T: Clone + 'static,
    {
        let mut info_column = Column::new().spacing(5);

        if let Some(label) = self.label {
            info_column = info_column.push(text::p1_regular(label));
        }

        // An unconfirmed payment has no timestamp, so surface its badge here —
        // left-justified in the slot where the date normally sits — rather than
        // over on the right next to the amount, which reads as more intuitive.
        // The self-transfer pill joins it there, leaving the amount slot free
        // to show what was actually moved.
        let has_unconfirmed = self.badges.contains(&TransactionBadge::Unconfirmed);
        let is_self_transfer = self.direction == TransactionDirection::SelfTransfer;

        let mut status_row = Row::new().spacing(8).align_y(Alignment::Center);
        let mut has_status = false;
        if let Some(timestamp) = self.timestamp {
            status_row = status_row.push(
                text::p2_regular({
                    let local = timestamp.with_timezone(&Local);
                    local
                        .format(&crate::locale::date_pattern("%b. %d, %Y - %T", &local))
                        .to_string()
                })
                .style(theme::text::secondary),
            );
            has_status = true;
        } else if let Some(time_ago) = self.time_ago {
            status_row = status_row.push(text::p2_regular(time_ago).style(theme::text::secondary));
            has_status = true;
        } else if has_unconfirmed {
            status_row = status_row.push(badge::unconfirmed());
            has_status = true;
        }
        if let Some(status) = self.custom_status {
            status_row = status_row.push(status);
            has_status = true;
        }
        if is_self_transfer {
            status_row = status_row.push(badge::self_transfer());
            has_status = true;
        }
        if has_status {
            info_column = info_column.push(status_row);
        }

        let left_side = transaction_leading_row(
            self.direction,
            self.custom_icon,
            self.show_direction_badge,
            info_column,
        );

        let mut content_row = Row::new()
            .align_y(Alignment::Center)
            .spacing(20)
            .push(left_side);

        for badge_type in self.badges {
            let badge_elem = match badge_type {
                // Rendered left-justified in the info column above (where the
                // date would be), not on the right by the amount.
                TransactionBadge::Unconfirmed => continue,
                TransactionBadge::Batch => badge::batch(),
                TransactionBadge::Recovery => badge::recovery(),
            };
            content_row = content_row.push(badge_elem);
        }

        let amount_column = transaction_amount(
            self.direction,
            self.bitcoin_unit,
            self.amount,
            &self.amount_override,
            self.fiat_amount,
        );

        content_row = content_row.push(amount_column);

        if let Some(on_press) = on_press {
            Container::new(
                button(content_row.padding(10))
                    .on_press(on_press)
                    .style(theme::button::transparent_border),
            )
            .style(theme::card::simple)
        } else {
            Container::new(content_row.padding(10)).style(theme::card::simple)
        }
    }
}

fn transaction_leading_row<T: Clone + 'static>(
    direction: TransactionDirection,
    custom_icon: Option<Element<'static, T>>,
    show_direction_badge: bool,
    info_column: Column<'static, T>,
) -> Row<'static, T> {
    let mut left_side = Row::new().spacing(10).align_y(Alignment::Center);

    if let Some(custom_icon) = custom_icon {
        left_side = left_side.push(custom_icon);
    }
    if show_direction_badge {
        let direction_badge = match direction {
            TransactionDirection::Incoming => badge::receive(),
            TransactionDirection::Outgoing => badge::spend(),
            TransactionDirection::SelfTransfer => badge::cycle(),
        };
        left_side = left_side.push(direction_badge);
    }

    left_side = left_side.push(info_column).width(Length::Fill);
    left_side
}

fn transaction_amount<T: Clone + 'static>(
    direction: TransactionDirection,
    bitcoin_unit: BitcoinDisplayUnit,
    amount: &Amount,
    amount_override: &Option<String>,
    fiat_amount: Option<String>,
) -> Column<'static, T> {
    let mut amount_column = Column::new().align_x(Alignment::End).spacing(5);

    // A self-transfer moves value without gaining or losing any, so it
    // carries no sign — just the amount that was moved.
    let (amount_sign, sign_style): (&str, fn(&theme::Theme) -> iced::widget::text::Style) =
        match direction {
            TransactionDirection::Incoming => ("+", theme::text::incoming),
            TransactionDirection::Outgoing => ("-", theme::text::outgoing),
            TransactionDirection::SelfTransfer => ("", theme::text::default),
        };

    let mut amount_row = Row::new().spacing(5).align_y(Alignment::Center);
    if !amount_sign.is_empty() {
        // Only color the sign (+/-), not the amount text — consistent with BTC rendering
        amount_row = amount_row.push(text::p1_regular(amount_sign).style(sign_style));
    }
    amount_column = amount_column.push(if let Some(ref override_str) = *amount_override {
        amount_row.push(text::p1_bold(override_str.clone()))
    } else {
        amount_row.push(amount::amount_with_unit(amount, bitcoin_unit))
    });

    if let Some(fiat) = fiat_amount {
        amount_column = amount_column.push(text::p2_regular(fiat).style(theme::text::secondary));
    }
    amount_column
}
