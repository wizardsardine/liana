//! The page every wallet shows once a payment completes.
//!
//! The receipt leads: status, amount, where the money went, the rows a user
//! might need later (fee, total, ID), then Done. The worldview artwork, its
//! quote and the worldview line frame the receipt from the right, or sit
//! below it when the window is too narrow for both.
//!
//! Only Done, Enter and Esc close the page. Clicking the art or empty space
//! does nothing, so a stray click never throws the confirmation away.

use std::borrow::Cow;

use iced::advanced::layout;
use iced::keyboard::{self, key};
use iced::{Alignment, Length, Size};

use super::quote_display::{self as qd, Quote, QuoteDisplayProps};
use super::tenshu::arranged::arranged;
use super::tenshu::scope::KeyScope;
use super::{button, separation};
use crate::font;
use crate::icon;
use crate::locale::{copy as localized_copy, Text as CopyText};
use crate::theme::tenshu::{text as tone_text, Tone};
use crate::widget::{Column, Container, Element, Row, Text};

/// The artwork's side in the two-column layout.
const ART_SIDE: u16 = 300;
/// The receipt never narrows below this to keep the art beside it.
const RECEIPT_MIN_WIDTH: f32 = 420.0;
const COLUMN_GAP: f32 = 56.0;
const STACK_GAP: f32 = 36.0;
/// Widest the frame gets when it sits under the receipt.
const STACKED_FRAME_MAX: f32 = 460.0;
const AMOUNT_SIZE: f32 = 44.0;
const UNIT_SIZE: f32 = 20.0;
const LABEL_WIDTH: f32 = 150.0;

/// What became of the payment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Money arrived in this wallet.
    Received,
    /// Money left and is final (or as final as the rail gets).
    Sent,
    /// Broadcast, not yet confirmed. Activity carries the confirmation.
    Confirming,
}

impl Outcome {
    fn headline(self) -> &'static str {
        match self {
            Outcome::Received => CopyText::PaymentReceived.get(),
            Outcome::Sent => CopyText::PaymentSent.get(),
            Outcome::Confirming => CopyText::SentAwaitingConfirmation.get(),
        }
    }

    fn tone(self) -> Tone {
        match self {
            Outcome::Received | Outcome::Sent => Tone::Good,
            Outcome::Confirming => Tone::Warn,
        }
    }
}

/// One line of the receipt: a muted label and its value.
#[derive(Debug, Clone)]
pub struct ReceiptRow<'a, M> {
    label: Cow<'a, str>,
    value: Cow<'a, str>,
    note: Option<Cow<'a, str>>,
    mono: bool,
    copy: Option<M>,
    copy_tip: Option<Cow<'a, str>>,
}

impl<'a, M> ReceiptRow<'a, M> {
    pub fn new(label: impl Into<Cow<'a, str>>, value: impl Into<Cow<'a, str>>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            note: None,
            mono: false,
            copy: None,
            copy_tip: None,
        }
    }

    /// An identifier: set in Plex Mono.
    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }

    /// A caption under the value.
    pub fn note(mut self, note: impl Into<Cow<'a, str>>) -> Self {
        self.note = Some(note.into());
        self
    }

    /// A copy button after the value, sending `message`. Its tooltip reads
    /// "Copy <label>" unless [`Self::copy_tip`] names it.
    pub fn on_copy(mut self, message: M) -> Self {
        self.copy = Some(message);
        self
    }

    /// The copy button's tooltip, for a label that doesn't name what is
    /// copied (such as "To").
    pub fn copy_tip(mut self, tip: impl Into<Cow<'a, str>>) -> Self {
        self.copy_tip = Some(tip.into());
        self
    }
}

/// Build with [`payment_result`], then convert into an [`Element`].
pub struct PaymentResult<'a, M> {
    outcome: Outcome,
    headline: Option<Cow<'a, str>>,
    amount: Cow<'a, str>,
    detail: Option<Cow<'a, str>>,
    rows: Vec<ReceiptRow<'a, M>>,
    context: String,
    quote: &'a Quote,
    image: &'a iced::widget::image::Handle,
    voice: Option<&'a str>,
    on_done: M,
    on_activity: Option<M>,
}

/// A finished payment. `amount` is the display string the wallet already
/// formats (number and unit, or the privacy mask); `context` picks the art.
pub fn payment_result<'a, M>(
    outcome: Outcome,
    amount: impl Into<Cow<'a, str>>,
    context: &str,
    quote: &'a Quote,
    image: &'a iced::widget::image::Handle,
    on_done: M,
) -> PaymentResult<'a, M> {
    PaymentResult {
        outcome,
        headline: None,
        amount: amount.into(),
        detail: None,
        rows: Vec::new(),
        context: context.to_string(),
        quote,
        image,
        voice: None,
        on_done,
        on_activity: None,
    }
}

impl<'a, M> PaymentResult<'a, M> {
    /// Replaces the outcome's own status line (a transfer, a cancellation).
    pub fn headline(mut self, headline: impl Into<Cow<'a, str>>) -> Self {
        self.headline = Some(headline.into());
        self
    }

    /// The line under the amount: where the money went or came from.
    pub fn detail(mut self, detail: impl Into<Cow<'a, str>>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn row(mut self, row: ReceiptRow<'a, M>) -> Self {
        self.rows.push(row);
        self
    }

    pub fn rows(mut self, rows: impl IntoIterator<Item = ReceiptRow<'a, M>>) -> Self {
        self.rows.extend(rows);
        self
    }

    /// The worldview's line under the quote. `None` hides it (for example, a
    /// Vault self-transfer).
    pub fn voice(mut self, voice: Option<&'a str>) -> Self {
        self.voice = voice;
        self
    }

    /// Adds View in Activity.
    pub fn on_activity(mut self, message: M) -> Self {
        self.on_activity = Some(message);
        self
    }

    pub fn on_activity_maybe(mut self, message: Option<M>) -> Self {
        self.on_activity = message;
        self
    }
}

/// A long identifier as its first and last eight characters, for a receipt
/// row whose copy button carries the full value.
pub fn short_id(id: &str) -> String {
    let chars: Vec<char> = id.chars().collect();
    if chars.len() <= 24 {
        return id.to_string();
    }
    let head: String = chars[..8].iter().collect();
    let tail: String = chars[chars.len() - 8..].iter().collect();
    format!("{head} … {tail}")
}

/// Splits "2,000 sats" into ("2,000", Some("sats")) so the unit can sit
/// beside the number in a smaller, muted face. Anything else stays whole.
fn split_amount(amount: &str) -> (&str, Option<&str>) {
    match amount.trim().rsplit_once(' ') {
        Some((number, unit))
            if !unit.is_empty()
                && unit.chars().all(char::is_alphabetic)
                && number.chars().any(|c| c.is_ascii_digit()) =>
        {
            (number, Some(unit))
        }
        _ => (amount.trim(), None),
    }
}

/// The figure as shown: received amounts carry "+", sent ones no sign.
/// A masked amount is never signed.
fn signed_figure(outcome: Outcome, number: &str) -> String {
    let bare = number.trim_start_matches(['+', '-', '\u{2212}']);
    let masked = !bare.chars().any(|c| c.is_ascii_digit());
    match outcome {
        Outcome::Received if !masked => format!("+{bare}"),
        _ => bare.to_string(),
    }
}

fn status_icon<'a>(outcome: Outcome) -> Text<'a> {
    match outcome {
        Outcome::Received | Outcome::Sent => icon::check_circle_icon(),
        Outcome::Confirming => icon::clock_icon(),
    }
}

fn receipt_row<'a, M: Clone + 'a>(row: ReceiptRow<'a, M>) -> Element<'a, M> {
    let value: Text<'a> = if row.mono {
        super::tenshu::mono(row.value)
    } else {
        super::tenshu::body(row.value)
    };
    let mut value_col = Column::new().spacing(2).width(Length::Fill).push(value);
    if let Some(note) = row.note {
        value_col = value_col.push(super::tenshu::caption(note));
    }
    let mut line = Row::new()
        .spacing(16)
        .align_y(Alignment::Center)
        .push(
            Container::new(super::tenshu::body(row.label.clone()).style(tone_text::muted))
                .width(Length::Fixed(LABEL_WIDTH)),
        )
        .push(value_col);
    if let Some(copy) = row.copy {
        // The square copy button Activity's detail panel uses; the owner
        // confirms the copy with a toast.
        let tip = row.copy_tip.unwrap_or_else(|| {
            Cow::Owned(crate::localized_format!("Copy {}", "Copiar {}", row.label))
        });
        line = line.push(super::tenshu::copy_icon(tip, copy));
    }
    Container::new(line)
        .padding([11, 0])
        .width(Length::Fill)
        .into()
}

fn receipt<'a, M: Clone + 'a>(
    outcome: Outcome,
    headline: Cow<'a, str>,
    amount: &str,
    detail: Option<Cow<'a, str>>,
    rows: Vec<ReceiptRow<'a, M>>,
    on_done: M,
    on_activity: Option<M>,
) -> Element<'a, M> {
    let tone = outcome.tone();
    let status = Row::new()
        .spacing(8)
        .align_y(Alignment::Center)
        .push(
            status_icon(outcome)
                .size(20)
                .style(move |t| tone_text::tone(t, tone)),
        )
        .push(
            iced::widget::text(headline)
                .size(15)
                .font(font::SEMIBOLD)
                .style(move |t| tone_text::tone(t, tone)),
        );

    let (number, unit) = split_amount(amount);
    let figure_tone = if outcome == Outcome::Received {
        Tone::Good
    } else {
        Tone::Neutral
    };
    let mut figure = Row::new().spacing(10).align_y(Alignment::End).push(
        iced::widget::text(signed_figure(outcome, number))
            .size(AMOUNT_SIZE)
            .font(font::SEMIBOLD)
            .line_height(iced::widget::text::LineHeight::Relative(1.1))
            .style(move |t| tone_text::tone(t, figure_tone)),
    );
    if let Some(unit) = unit {
        figure = figure.push(
            Container::new(
                iced::widget::text(unit.to_string())
                    .size(UNIT_SIZE)
                    .font(font::MEDIUM)
                    .style(tone_text::muted),
            )
            .padding(iced::Padding::ZERO.bottom(6)),
        );
    }

    let mut col = Column::new()
        .width(Length::Fill)
        .push(status)
        .push(Container::new(figure).padding(iced::Padding::ZERO.top(14)));
    if let Some(detail) = detail {
        col = col
            .push(Container::new(super::tenshu::body(detail)).padding(iced::Padding::ZERO.top(12)));
    }

    if !rows.is_empty() {
        let mut list = Column::new().width(Length::Fill).push(separation());
        for row in rows {
            list = list.push(receipt_row(row)).push(separation());
        }
        col = col.push(Container::new(list).padding(iced::Padding::ZERO.top(24)));
    }

    let mut actions = Row::new().spacing(12).align_y(Alignment::Center).push(
        button::primary(None, localized_copy("Done"))
            .height(Length::Fixed(44.0))
            .width(Length::Fixed(132.0))
            .on_press(on_done),
    );
    if let Some(activity) = on_activity {
        actions = actions.push(
            button::secondary(None, localized_copy("View in Activity"))
                .height(Length::Fixed(44.0))
                .width(Length::Shrink)
                .padding([0, 20])
                .on_press(activity),
        );
    }
    actions = actions.push(super::tenshu::caption(localized_copy(
        "Enter or Esc closes this",
    )));
    col.push(Container::new(actions).padding(iced::Padding::ZERO.top(28)))
        .into()
}

fn frame<'a, M: 'a>(
    context: &str,
    quote: &'a Quote,
    image: &'a iced::widget::image::Handle,
    outcome: Outcome,
    voice: Option<&'a str>,
) -> Element<'a, M> {
    let slot = match outcome {
        Outcome::Received => crate::illustration::Slot::Received,
        Outcome::Sent | Outcome::Confirming => crate::illustration::Slot::Sent,
    };
    let props = QuoteDisplayProps::new(context, quote, image)
        .illustration_slot(slot)
        .image_size(ART_SIDE);
    let mut col = Column::new()
        .width(Length::Fill)
        .push(qd::artwork(&props))
        .push(
            Container::new(
                super::tenshu::body(quote.localized_text())
                    .size(15)
                    .width(Length::Fill),
            )
            .padding(iced::Padding::ZERO.top(20)),
        );
    if let Some(attribution) = qd::format_attribution(quote) {
        col = col.push(
            Container::new(super::tenshu::caption(attribution).size(13))
                .padding(iced::Padding::ZERO.top(8)),
        );
    }
    if let Some(voice) = voice {
        col = col.push(
            Column::new()
                .padding(iced::Padding::ZERO.top(16))
                .spacing(14)
                .push(separation())
                .push(
                    super::tenshu::body(voice)
                        .font(font::ITALIC)
                        .style(tone_text::muted),
                ),
        );
    }
    col.into()
}

impl<'a, M: Clone + 'a> From<PaymentResult<'a, M>> for Element<'a, M> {
    fn from(page: PaymentResult<'a, M>) -> Self {
        let PaymentResult {
            outcome,
            headline,
            amount,
            detail,
            rows,
            context,
            quote,
            image,
            voice,
            on_done,
            on_activity,
        } = page;
        let headline = headline.unwrap_or(Cow::Borrowed(outcome.headline()));
        let lead = receipt(
            outcome,
            headline,
            &amount,
            detail,
            rows,
            on_done.clone(),
            on_activity,
        );
        let side = frame(&context, quote, image, outcome, voice);

        let body = arranged(
            vec![lead, side],
            Size::new(Length::Fill, Length::Shrink),
            |children, trees, renderer, limits| {
                let max = limits.max();
                let width = if max.width.is_finite() {
                    max.width
                } else {
                    RECEIPT_MIN_WIDTH + COLUMN_GAP + f32::from(ART_SIDE)
                };
                let frame_side = f32::from(ART_SIDE);
                let beside = width >= RECEIPT_MIN_WIDTH + COLUMN_GAP + frame_side;
                let (lead_w, frame_w) = if beside {
                    (width - COLUMN_GAP - frame_side, frame_side)
                } else {
                    (width, width.min(STACKED_FRAME_MAX))
                };
                let lay = |i: usize,
                           w: f32,
                           children: &mut [Element<'a, M>],
                           trees: &mut [iced::advanced::widget::Tree]| {
                    let own = layout::Limits::new(Size::ZERO, Size::new(w, f32::INFINITY))
                        .width(Length::Fixed(w));
                    children[i]
                        .as_widget_mut()
                        .layout(&mut trees[i], renderer, &own)
                };
                let lead = lay(0, lead_w, children, trees);
                let side = lay(1, frame_w, children, trees);
                let (lead_h, side_h) = (lead.size().height, side.size().height);
                let (side_at, height) = if beside {
                    ((lead_w + COLUMN_GAP, 0.0), lead_h.max(side_h))
                } else {
                    ((0.0, lead_h + STACK_GAP), lead_h + STACK_GAP + side_h)
                };
                layout::Node::with_children(
                    Size::new(width, height),
                    vec![lead, side.move_to(side_at)],
                )
            },
        );

        let scoped = KeyScope::new(
            Container::new(body)
                .max_width(1000)
                .padding(iced::Padding::ZERO.top(8).bottom(30)),
            Box::new(move |key, _modifiers, focused| match key {
                keyboard::Key::Named(key::Named::Escape) => Some(vec![on_done.clone()]),
                // Enter closes unless a control inside holds focus, which
                // answers Enter itself.
                keyboard::Key::Named(key::Named::Enter) if focused.is_none() => {
                    Some(vec![on_done.clone()])
                }
                _ => None,
            }),
        );
        scoped.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amount_splits_a_trailing_word_unit_only() {
        assert_eq!(split_amount("2,000 sats"), ("2,000", Some("sats")));
        assert_eq!(split_amount("+2,000 SATS"), ("+2,000", Some("SATS")));
        assert_eq!(split_amount("12.50 USDt"), ("12.50", Some("USDt")));
        assert_eq!(split_amount("0.00 012 000"), ("0.00 012 000", None));
        assert_eq!(split_amount("••••••"), ("••••••", None));
    }

    #[test]
    fn long_ids_keep_both_ends() {
        assert_eq!(short_id("abc"), "abc");
        assert_eq!(
            short_id("0123456789abcdef0123456789abcdef"),
            "01234567 … 89abcdef"
        );
    }

    #[test]
    fn only_a_visible_received_amount_is_signed() {
        assert_eq!(signed_figure(Outcome::Received, "2,000"), "+2,000");
        assert_eq!(signed_figure(Outcome::Received, "+2,000"), "+2,000");
        assert_eq!(signed_figure(Outcome::Sent, "2,000"), "2,000");
        assert_eq!(signed_figure(Outcome::Confirming, "-2,000"), "2,000");
        assert_eq!(signed_figure(Outcome::Received, "••••••"), "••••••");
    }
}
