use std::num::ParseFloatError;

pub use bitcoin::Amount;
use iced::{
    widget::{row, Space},
    Alignment,
};

use crate::{component::text::*, theme::amount, widget::*};

pub trait DisplayAmount {
    fn to_formatted_string(&self) -> String;
}

impl DisplayAmount for Amount {
    fn to_formatted_string(&self) -> String {
        format_f64_as_string(self.to_btc(), " ", 8, true)
    }
}

/// Amount with default size and colors.
pub fn amount<'a, T: 'a>(a: &Amount) -> Row<'a, T> {
    amount_with_font(a, crate::component::text::legacy::P1_REGULAR_SPEC)
}

/// Amount with default colors.
pub fn amount_with_font<'a, T: 'a>(a: &Amount, font: TextSpec) -> Row<'a, T> {
    render_amount(a.to_formatted_string(), font, false)
}

/// Amount with the given size and colors.
///
/// `color_before` is the color to use before the first non-zero
/// value in `a`.
///
/// `color_after` is the color to use from the first non-zero
/// value in `a` onwards. If `None`, the default theme value
/// will be used.
pub fn amount_with_font_blink<'a, T: 'a>(a: &Amount, font: TextSpec) -> Row<'a, T> {
    render_amount(a.to_formatted_string(), font, true)
}

//
// Helpers
//

/// Formats an f64 as a string with a custom separator and number of decimals,
/// padding the decimal part with zeros if needed.
/// If `sep_decimals` is true, the separator is also applied to the decimal part,
/// grouping from the right.
pub fn format_f64_as_string(
    value: f64,
    sep: &str,
    num_decimals: usize,
    sep_decimals: bool,
) -> String {
    // Format with the requested number of decimals.
    let amount = format!("{value:.num_decimals$}");

    // Split into integer and fractional parts.
    let (integer, fraction) = match amount.split_once('.') {
        Some((i, f)) => (i, f),
        None => (amount.as_str(), ""), // num_decimals must be 0
    };

    let integer = format_amount_number_part(integer, sep);

    if num_decimals > 0 {
        let fraction = if sep_decimals {
            format_amount_number_part(fraction, sep)
        } else {
            fraction.to_string()
        };
        format!("{integer}.{fraction}")
    } else {
        integer
    }
}

// Format a "part" of a number string with spaces to fit display requirements.
// Currently using French formatting rules so digits are space-separated in groups
// of three, starting from the right side. Incidentally, this works for both the
// integer portion of the number as well as the fraction part.
// Ex:
//   1000 => 1 000
//   100000 => 100 000
fn format_amount_number_part(s: &str, sep: &str) -> String {
    let mut part = s
        .chars()
        .collect::<Vec<_>>()
        .rchunks(3)
        .map(|c| c.iter().collect::<String>())
        .collect::<Vec<_>>();
    part.reverse();

    part.join(sep)
}

// Helper functions split a string at the first occurrence of a non-zero integer (where
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
fn render_amount<'a, T: 'a>(amount: String, font: TextSpec, blink: bool) -> Row<'a, T> {
    let size = font.size.unwrap_or(P1_SIZE);
    let spacing = if size > P1_SIZE { 10 } else { 5 };

    let (zeroes, after) = match split_at_first_non_zero(amount) {
        Some((b, a)) => (b, a),
        None => (String::from("0.00 000 000"), String::from("")),
    };

    let sats = apply(after, font).style(move |theme| amount::sats(theme, blink));
    let zeroes = apply(zeroes, font).style(move |theme| amount::zeroes(theme, blink));
    let btc = apply("BTC", font).style(move |theme| amount::zeroes(theme, blink));
    row![zeroes, sats, Space::with_width(spacing), btc].align_y(iced::Alignment::Center)
}

macro_rules! currency_enum {
    ($name:ident { $($variant:ident),* $(,)? }) => {
        #[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Default)]
        pub enum $name {
            #[default]
            $($variant,)*
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self {
                    $(Self::$variant => write!(f, stringify!($variant)),)*
                }
            }
        }

        impl std::str::FromStr for $name {
            type Err = String;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s.to_uppercase().as_str() {
                    $(stringify!($variant) => Ok(Self::$variant),)*
                    _ => Err("Invalid currency".to_string()),
                }
            }
        }

    };
}

currency_enum!(Currency {
    USD, // macro sets first variant as the default
    AED,
    AMD,
    ARS,
    AUD,
    BAM,
    BDT,
    BHD,
    BMD,
    BRL,
    CAD,
    CHF,
    CLP,
    CNY,
    COP,
    CRC,
    CZK,
    DKK,
    DOP,
    EUR,
    GBP,
    GEL,
    GTQ,
    HKD,
    HNL,
    HUF,
    IDR,
    ILS,
    INR,
    JPY,
    KES,
    KRW,
    KWD,
    LKR,
    LBP,
    MMK,
    MXN,
    MYR,
    NGN,
    NOK,
    NZD,
    PEN,
    PHP,
    PKR,
    PLN,
    RON,
    RUB,
    SAR,
    SEK,
    SGD,
    SVC,
    THB,
    TRY,
    TWD,
    UAH,
    VEF,
    VND,
    ZAR,
    ZMW,
});

impl Currency {
    /// Returns the number of decimals required for the minor unit.
    pub fn decimals(&self) -> usize {
        match self {
            Currency::CLP | Currency::JPY | Currency::KRW | Currency::VND => 0,
            Currency::BHD | Currency::KWD => 3,
            _ => 2,
        }
    }
}

/// A non-negative fiat amount with a specific currency.
#[derive(Debug, Clone, Copy)]
pub struct FiatAmount {
    amount: f64,
    currency: Currency,
}

#[derive(Debug, Clone)]
pub enum AmountError {
    Negative,
    ParseError(String),
}

impl std::fmt::Display for AmountError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Negative => write!(f, "Amount must be non-negative"),
            Self::ParseError(e) => write!(f, "Parse error: {e}"),
        }
    }
}

impl FiatAmount {
    pub fn new(amount: f64, currency: Currency) -> Result<Self, AmountError> {
        if amount < 0.0 {
            return Err(AmountError::Negative);
        }
        Ok(Self { amount, currency })
    }

    /// Parse a fiat amount from a string in the given currency.
    pub fn from_str_in(s: &str, currency: Currency) -> Result<Self, AmountError> {
        let amount: f64 = s
            .trim()
            .parse()
            .map_err(|e: ParseFloatError| AmountError::ParseError(e.to_string()))?;
        Self::new(amount, currency)
    }

    pub fn amount(&self) -> f64 {
        self.amount
    }

    pub fn currency(&self) -> Currency {
        self.currency
    }

    /// Format a fiat amount as a string with required decimal places for currency and no thousands separator.
    pub fn to_rounded_string(&self) -> String {
        format_f64_as_string(self.amount, "", self.currency().decimals(), false)
    }

    /// Format a fiat amount as a string with a tilde (~) prefix to indicate approximation.
    pub fn to_display_string(&self) -> String {
        self.to_display_string_approx(true)
    }

    /// Format a fiat amount as a string, with a tilde (~) prefix only when
    /// `approximate` (an exact, user-known price shows no `~`).
    pub fn to_display_string_approx(&self, approximate: bool) -> String {
        let prefix = if approximate { "~" } else { "" };
        format!("{prefix}{} {}", self.to_formatted_string(), self.currency())
    }
}

// Format a fiat amount as a string with required decimal places for currency and a comma as the thousands separator.
impl DisplayAmount for FiatAmount {
    fn to_formatted_string(&self) -> String {
        format_f64_as_string(self.amount, ",", self.currency().decimals(), false)
    }
}

/// Size preset for [`amount_with_fiat`]: picks the BTC-amount and fiat text specs.
#[derive(Debug, Clone, Copy)]
pub enum AmountSize {
    S,
    M,
    L,
}

impl AmountSize {
    fn amount_spec(self) -> TextSpec {
        match self {
            AmountSize::S => new::CAPTION_SPEC,
            AmountSize::M => new::H2_SEMI_SPEC,
            AmountSize::L => new::D2_SPEC,
        }
    }

    fn fiat_spec(self) -> TextSpec {
        match self {
            AmountSize::S => new::CAPTION_SPEC,
            AmountSize::M => new::H3_SPEC,
            AmountSize::L => new::H1_SPEC,
        }
    }
}

/// A BTC amount with an optional fiat value beside it and an optional trailing
/// element after the fiat (e.g. a price-source tooltip), in a consistent format.
///
/// `to_fiat` converts the amount to fiat (typically `|a| converter.convert(a)`);
/// when `None`, only the BTC amount is shown. The fiat value is rendered in the
/// amount colors with a `~` approximation prefix, to match the BTC amount.
pub fn amount_with_fiat_tooltip<'a, M: 'a, F: Fn(Amount) -> FiatAmount>(
    a: &Amount,
    to_fiat: Option<F>,
    size: AmountSize,
    approximate: bool,
    tooltip: Option<Element<'a, M>>,
) -> Element<'a, M> {
    let btc = amount_with_font(a, size.amount_spec());
    let fiat = to_fiat.map(|to_fiat| {
        apply(
            to_fiat(*a).to_display_string_approx(approximate),
            size.fiat_spec(),
        )
        .style(|t| amount::zeroes(t, false))
    });
    row![btc, fiat, tooltip]
        .spacing(10)
        .align_y(Alignment::Center)
        .wrap()
        .into()
}

/// A BTC amount with an optional fiat value beside it, in a consistent format.
/// The fiat is shown as an approximation (`~`). See [`amount_with_fiat_tooltip`]
/// for the variant with an exact/approximate flag and a trailing tooltip.
pub fn amount_with_fiat<'a, M: 'a, F: Fn(Amount) -> FiatAmount>(
    a: &Amount,
    to_fiat: Option<F>,
    size: AmountSize,
) -> Element<'a, M> {
    amount_with_fiat_tooltip(a, to_fiat, size, true, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_fiat_amount() {
        // Try with negative amounts.
        for amt in &[-1000.0, -10.5, -0.1] {
            let result = FiatAmount::new(*amt, Currency::USD);
            assert!(result.is_err());
            assert!(matches!(result.unwrap_err(), AmountError::Negative));
        }

        // Check non-negative amounts work.
        for amt in &[-0.0, 0.0, 0.1, 27.12] {
            let result = FiatAmount::new(*amt, Currency::USD);
            assert!(result.is_ok());
        }
    }

    #[test]
    fn test_amount_as_str() {
        assert_eq!(
            "0.00 799 800",
            bitcoin::Amount::from_btc(0.00799800)
                .unwrap()
                .to_formatted_string()
        );
        assert_eq!(
            "1 000.00 799 800",
            bitcoin::Amount::from_btc(1000.00799800)
                .unwrap()
                .to_formatted_string()
        );
        assert_eq!(
            "1 000.00 000 000",
            bitcoin::Amount::from_btc(1000.0)
                .unwrap()
                .to_formatted_string()
        );
        assert_eq!(
            "0.00 012 340",
            bitcoin::Amount::from_btc(0.00012340)
                .unwrap()
                .to_formatted_string()
        )
    }

    #[test]
    fn test_format_f64_as_string() {
        assert_eq!(
            format_f64_as_string(1234567.12345678, " ", 8, false),
            "1 234 567.12345678"
        );
        assert_eq!(
            format_f64_as_string(1234567.12345678, " ", 8, true),
            "1 234 567.12 345 678"
        );

        assert_eq!(
            format_f64_as_string(1234567.12345678, ",", 2, false),
            "1,234,567.12"
        );
        assert_eq!(
            format_f64_as_string(1234567.12345678, ",", 2, true),
            "1,234,567.12"
        );

        assert_eq!(
            format_f64_as_string(1234567.12345678, ",", 4, false),
            "1,234,567.1235"
        );
        assert_eq!(
            format_f64_as_string(1234567.12345678, ",", 4, true),
            "1,234,567.1,235"
        );

        assert_eq!(format_f64_as_string(0.000132, " ", 8, false), "0.00013200");
        assert_eq!(format_f64_as_string(0.000132, " ", 8, true), "0.00 013 200");

        assert_eq!(format_f64_as_string(0.0, " ", 8, false), "0.00000000");
        assert_eq!(format_f64_as_string(0.0, " ", 8, true), "0.00 000 000");

        assert_eq!(
            format_f64_as_string(1000.00799800, " ", 8, false),
            "1 000.00799800"
        );
        assert_eq!(
            format_f64_as_string(1000.00799800, " ", 8, true),
            "1 000.00 799 800"
        );

        assert_eq!(
            format_f64_as_string(1000.0, " ", 8, false),
            "1 000.00000000"
        );
        assert_eq!(
            format_f64_as_string(1000.0, " ", 8, true),
            "1 000.00 000 000"
        );

        assert_eq!(format_f64_as_string(1234567.0, " ", 0, false), "1 234 567");
        assert_eq!(format_f64_as_string(1234567.0, " ", 0, true), "1 234 567");

        assert_eq!(format_f64_as_string(1234567.0, ",", 0, false), "1,234,567");
        assert_eq!(format_f64_as_string(1234567.0, ",", 0, true), "1,234,567");

        assert_eq!(format_f64_as_string(0.0, " ", 0, false), "0");
        assert_eq!(format_f64_as_string(0.0, " ", 0, true), "0");

        assert_eq!(format_f64_as_string(0.0, ",", 0, false), "0");
        assert_eq!(format_f64_as_string(0.0, ",", 0, true), "0");
    }
}
