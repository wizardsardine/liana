use crate::{
    component::{amount, amount::BitcoinDisplayUnit, badge, text},
    theme,
    widget::*,
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

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum TransactionType {
    #[default]
    Bitcoin,
    Lightning,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TransactionBadge {
    Unconfirmed,
    Batch,
    Recovery,
}

pub struct TransactionListItem<'a, T> {
    direction: TransactionDirection,
    transaction_type: Option<TransactionType>,
    label: Option<String>,
    timestamp: Option<DateTime<Utc>>,
    time_ago: Option<String>,
    badges: Vec<TransactionBadge>,
    amount: &'a Amount,
    bitcoin_unit: BitcoinDisplayUnit,
    fiat_amount: Option<String>,
    amount_override: Option<String>,
    custom_status: Option<Element<'static, T>>,
    /// Custom icon element replacing the default type+direction badges.
    custom_icon: Option<Element<'static, T>>,
}

impl<'a, T> TransactionListItem<'a, T> {
    pub fn new(
        direction: TransactionDirection,
        amount: &'a Amount,
        bitcoin_unit: BitcoinDisplayUnit,
    ) -> Self {
        Self {
            direction,
            transaction_type: None,
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
        }
    }

    pub fn with_type(mut self, transaction_type: TransactionType) -> Self {
        self.transaction_type = Some(transaction_type);
        self
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

    /// Replace the default type+direction badges with a custom icon element.
    pub fn with_custom_icon(mut self, icon: Element<'static, T>) -> Self {
        self.custom_icon = Some(icon);
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
        let direction_badge = match self.direction {
            TransactionDirection::Incoming => badge::receive(),
            TransactionDirection::Outgoing => badge::spend(),
            TransactionDirection::SelfTransfer => badge::cycle(),
        };

        let type_badge = self.transaction_type.map(|t| match t {
            TransactionType::Lightning => badge::lightning(),
            TransactionType::Bitcoin => badge::bitcoin(),
        });

        let mut info_column = Column::new().spacing(5);

        if let Some(label) = self.label {
            info_column = info_column.push(text::p1_regular(label));
        }

        if let Some(timestamp) = self.timestamp {
            info_column = info_column.push(
                text::p2_regular(
                    timestamp
                        .with_timezone(&Local)
                        .format("%b. %d, %Y - %T")
                        .to_string(),
                )
                .style(theme::text::secondary),
            );
        } else if let Some(time_ago) = self.time_ago {
            let mut time_row = Row::new().spacing(8);
            time_row = time_row.push(text::p2_regular(time_ago).style(theme::text::secondary));
            if let Some(status) = self.custom_status {
                time_row = time_row.push(status);
            }
            info_column = info_column.push(time_row);
        }

        let mut left_side = Row::new().spacing(10).align_y(Alignment::Center);

        if let Some(custom_icon) = self.custom_icon {
            left_side = left_side.push(custom_icon);
        } else {
            if let Some(type_badge) = type_badge {
                left_side = left_side.push(type_badge);
            }
            left_side = left_side.push(direction_badge);
        }

        left_side = left_side.push(info_column).width(Length::Fill);

        let mut content_row = Row::new()
            .align_y(Alignment::Center)
            .spacing(20)
            .push(left_side);

        for badge_type in self.badges {
            let badge_elem = match badge_type {
                TransactionBadge::Unconfirmed => badge::unconfirmed(),
                TransactionBadge::Batch => badge::batch(),
                TransactionBadge::Recovery => badge::recovery(),
            };
            content_row = content_row.push(badge_elem);
        }

        let mut amount_column = Column::new().align_x(Alignment::End).spacing(5);

        if self.direction == TransactionDirection::SelfTransfer {
            amount_column = amount_column.push(text::p1_regular("Self-transfer"));
        } else {
            let (amount_sign, sign_style): (&str, fn(&theme::Theme) -> iced::widget::text::Style) =
                match self.direction {
                    TransactionDirection::Incoming => ("+", theme::text::incoming),
                    TransactionDirection::Outgoing => ("-", theme::text::outgoing),
                    TransactionDirection::SelfTransfer => ("", theme::text::default),
                };

            if let Some(ref override_str) = self.amount_override {
                // Only color the sign (+/-), not the amount text — consistent with BTC rendering
                amount_column = amount_column.push(
                    Row::new()
                        .spacing(5)
                        .push(text::p1_regular(amount_sign).style(sign_style))
                        .push(text::p1_bold(override_str.clone()))
                        .align_y(Alignment::Center),
                );
            } else {
                amount_column = amount_column.push(
                    Row::new()
                        .spacing(5)
                        .push(text::p1_regular(amount_sign).style(sign_style))
                        .push(amount::amount_with_unit(self.amount, self.bitcoin_unit))
                        .align_y(Alignment::Center),
                );
            }
        }

        if let Some(fiat) = self.fiat_amount {
            amount_column =
                amount_column.push(text::p2_regular(fiat).style(theme::text::secondary));
        }

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
