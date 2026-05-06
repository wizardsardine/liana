use iced::{widget::container::Style, Background, Border};

use super::{card::CARD_SHADOW, palette::ContainerPalette, styles};

fn builder(palette: &ContainerPalette) -> Style {
    Style {
        background: Some(Background::Color(palette.background)),
        text_color: palette.text,
        border: Border {
            radius: 25.0.into(),
            width: 2.0,
            color: palette.border.unwrap_or_default(),
        },
        ..Default::default()
    }
}

#[rustfmt::skip]
styles!(
    builder,
    pills,
    [
        simple,
        success,
        warning,
        soft_warning,
        internal,
        external,
        safety_net,
    ]
);

pub fn fingerprint(theme: &crate::theme::Theme) -> ::iced::widget::container::Style {
    let palette = &theme.colors.pills.fingerprint;
    Style {
        background: Some(Background::Color(palette.background)),
        text_color: palette.text,
        border: Border {
            radius: 25.0.into(),
            width: 2.0,
            color: palette.border.unwrap_or_default(),
        },
        shadow: CARD_SHADOW,
        ..Default::default()
    }
}
