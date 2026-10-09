pub mod block;
pub mod header;
pub mod leaf;
pub mod modals;
pub mod overlays;

pub use crate::widget::graph_view::geometry::{
    BLOCK_MIN_HEIGHT, BLOCK_WIDTH, INPUT_COLUMN_WIDTH, LEAF_HEIGHT, LEAF_WIDTH,
    MIDDLE_COLUMN_WIDTH, OUTPUT_COLUMN_WIDTH, SLOT_HEIGHT, U,
};

use iced::{
    border::Radius,
    widget::{container, Space},
    Background, Border, Color, Length,
};

use crate::{theme::Theme, widget::Container};

/// Solid `border-subtle` line, e.g. a 1 px column separator.
pub fn separator<'a, M: 'a>(
    width: impl Into<Length>,
    height: impl Into<Length>,
) -> Container<'a, M> {
    Container::new(Space::new())
        .width(width)
        .height(height)
        .style(|theme: &Theme| container::Style {
            background: Some(Background::Color(theme.colors.text.border)),
            ..Default::default()
        })
}

/// Layer drawn over an item to fake an opacity: `background` at `alpha` on top.
pub fn scrim<'a, M: 'a>(
    background: fn(&Theme) -> Color,
    alpha: f32,
    radius: Radius,
) -> Container<'a, M> {
    Container::new(Space::new())
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |theme: &Theme| container::Style {
            background: Some(Background::Color(Color {
                a: alpha,
                ..background(theme)
            })),
            border: Border {
                radius,
                ..Default::default()
            },
            ..Default::default()
        })
}

/// Hue of another wallet, stable for its descriptor checksum across runs and builds.
pub fn wallet_color(theme: &Theme, checksum: &str) -> Color {
    // FNV-1a, 64 bits.
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0100_0000_01b3;
    let hash = checksum.bytes().fold(OFFSET_BASIS, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(PRIME)
    });
    let wallets = &theme.colors.graph.wallets;
    wallets[(hash % wallets.len() as u64) as usize]
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use crate::{component::panels::map::wallet_color, theme::Theme};

    #[test]
    fn wallet_color_is_stable() {
        let theme = Theme::default();
        assert_eq!(
            wallet_color(&theme, "8yjzcvqz"),
            theme.colors.graph.wallets[4]
        );
    }

    #[test]
    fn wallet_colors_spread() {
        let theme = Theme::default();
        let checksums = [
            "8yjzcvqz", "g4z5ruwf", "x0n7y9ks", "qj6p5cna", "5ln3ywty", "cw9h4zjm", "2rfwp5lt",
            "mua4tqxe",
        ];
        let colors: HashSet<[u8; 4]> = checksums
            .iter()
            .map(|checksum| wallet_color(&theme, checksum).into_rgba8())
            .collect();
        assert_eq!(colors.len(), 7);
    }
}
