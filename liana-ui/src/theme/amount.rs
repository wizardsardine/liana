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

pub fn fiat_price(theme: &Theme) -> Style {
    Style {
        color: Some(theme.colors.price.zeroes),
    }
}

pub fn spend(theme: &Theme) -> Style {
    Style {
        color: Some(theme.colors.price.send),
    }
}

pub fn receive(theme: &Theme) -> Style {
    Style {
        color: Some(theme.colors.price.receive),
    }
}

pub fn refresh(theme: &Theme) -> Style {
    Style {
        color: Some(theme.colors.price.refresh),
    }
}
