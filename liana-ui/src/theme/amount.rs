use super::Theme;
use iced::widget::text::Style;

pub fn sats(theme: &Theme, blink: bool) -> Style {
    let col = if blink {
        theme.colors.price.blink_sats
    } else {
        theme.colors.price.sats
    };
    Style { color: Some(col) }
}

pub fn zeroes(theme: &Theme, blink: bool) -> Style {
    let col = if blink {
        theme.colors.price.blink_zeroes
    } else {
        theme.colors.price.zeroes
    };
    Style { color: Some(col) }
}
