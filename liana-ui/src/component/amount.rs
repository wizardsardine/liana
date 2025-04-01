pub use bitcoin::Amount;
use iced::Color;

use crate::{color, component::text::*, widget::*};

/// Amount with default size and colors.
pub fn amount<'a, T: 'a>(a: &Amount) -> Row<'a, T> {
    amount_with_size(a, P1_SIZE)
}

/// Amount with default colors.
pub fn amount_with_size<'a, T: 'a>(a: &Amount, size: u16) -> Row<'a, T> {
    amount_with_size_and_colors(a, size, color::GREEN, None)
}

/// Amount with the given size and colors.
///
/// `color_before` is the color to use before the first non-zero
/// value in `a`.
///
/// `color_after` is the color to use from the first non-zero
/// value in `a` onwards. If `None`, the default theme value
/// will be used.
pub fn amount_with_size_and_colors<'a, T: 'a>(
    a: &Amount,
    size: u16,
    color_before: Color,
    color_after: Option<Color>,
) -> Row<'a, T> {
    render_amount(amount_as_string(*a), size, color_before, color_after)
}

pub fn unconfirmed_amount_with_size<'a, T: 'a>(a: &Amount, size: u16) -> Row<'a, T> {
    render_unconfirmed_amount(amount_as_string(*a), size)
}

//
// Helpers
//

// Format a BTC amount as a string for display.
pub fn amount_as_string(a: Amount) -> String {
    let amount = a.to_btc().to_string();

    // Reformat the integer portion of the amount with space separation.
    let (integer, fraction) = match amount.split_once('.') {
        Some((i, f)) => (i, f),
        None => (amount.as_str(), "00000000"),
    };

    let integer = format_amount_number_part(integer);
    let fraction = format_amount_number_part(&format!("{:0<8}", fraction));

    format!("{integer}.{fraction}")
}

// Format a "part" of a number string with spaces to fit display requirements.
// Currently using French formatting rules so digits are space-separated in groups
// of three, starting from the right side. Incidentally, this works for both the
// integer portion of the number as well as the fraction part.
// Ex:
//   1000 => 1 000
//   100000 => 100 000
fn format_amount_number_part(s: &str) -> String {
    let mut part = s
        .chars()
        .collect::<Vec<_>>()
        .rchunks(3)
        .map(|c| c.iter().collect::<String>())
        .collect::<Vec<_>>();
    part.reverse();

    part.join(" ")
}

// Helper functions split a string at the first occurence of a non-zero integer (where
// the amount starts).
fn split_at_first_non_zero(s: String) -> Option<(String, String)> {
    for (index, c) in s.char_indices() {
        if c.is_ascii_digit() && c != '0' {
            let (before, after) = s.split_at(index);
            return Some((before.to_string(), after.to_string()));
        }
    }
    None
}

// Build the rendering elements for displaying a Bitcoin amount.
// The text should be bolded beginning where the BTC amount is non-zero.
fn render_amount<'a, T: 'a>(
    amount: String,
    size: u16,
    color_before: Color,
    color_after: Option<Color>,
) -> Row<'a, T> {
    let spacing = if size > P1_SIZE { 10 } else { 5 };

    let (before, after) = match split_at_first_non_zero(amount) {
        Some((b, a)) => (b, a),
        None => (String::from("0.00 000 000"), String::from("")),
    };

    let mut child_after = text(after).size(size).bold();
    if let Some(color_after) = color_after {
        child_after = child_after.color(color_after);
    }
    let row = Row::new()
        .push(text(before).size(size).color(color_before))
        .push(child_after);

    Row::with_children(vec![
        row.into(),
        text("BTC").size(size).color(color_before).into(),
    ])
    .spacing(spacing)
    .align_y(iced::Alignment::Center)
}

// Build the rendering elements for displaying a Bitcoin amount.
fn render_unconfirmed_amount<'a, T: 'a>(amount: String, size: u16) -> Row<'a, T> {
    let spacing = if size > P1_SIZE { 10 } else { 5 };

    Row::with_children(vec![
        text(amount).size(size).color(color::GREEN).into(),
        text("BTC").size(size).color(color::GREEN).into(),
    ])
    .spacing(spacing)
    .align_y(iced::Alignment::Center)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_amount_as_str() {
        assert_eq!(
            "0.00 799 800",
            amount_as_string(bitcoin::Amount::from_btc(0.00799800).unwrap())
        );
        assert_eq!(
            "1 000.00 799 800",
            amount_as_string(bitcoin::Amount::from_btc(1000.00799800).unwrap())
        );
        assert_eq!(
            "1 000.00 000 000",
            amount_as_string(bitcoin::Amount::from_btc(1000.0).unwrap())
        );
        assert_eq!(
            "0.00 012 340",
            amount_as_string(bitcoin::Amount::from_btc(0.00012340).unwrap())
        )
    }
}
