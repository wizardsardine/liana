//! The canvas's 18px stroke icons (`.ic`: 1.5 stroke, round caps and joins),
//! drawn as SVG and tinted like `currentColor`. The bootstrap font has no
//! exact match for several of them, so the shell and Activity draw these.

use coincube_ui::{theme, widget::Element};
use iced::{widget::svg, Color, Length};

macro_rules! icon {
    ($name:ident, $body:literal) => {
        pub const $name: &str = concat!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 18 18" fill="none" stroke="#fff" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">"##,
            $body,
            "</svg>"
        );
    };
}

icon!(
    GRID,
    r#"<rect x="2.5" y="2.5" width="5" height="5" rx="1"/><rect x="10.5" y="2.5" width="5" height="5" rx="1"/><rect x="2.5" y="10.5" width="5" height="5" rx="1"/><rect x="10.5" y="10.5" width="5" height="5" rx="1"/>"#
);
icon!(
    CLOCK,
    r#"<circle cx="9" cy="9" r="6.5"/><path d="M9 5.5V9l2.5 1.5"/>"#
);
icon!(
    SHIELD_CHECK,
    r#"<path d="M9 2 3.5 4v4.5c0 3.4 2.3 6.1 5.5 7.5 3.2-1.4 5.5-4.1 5.5-7.5V4z"/><path d="m6.5 9 1.8 1.8 3.5-3.5"/>"#
);
icon!(BOLT, r#"<path d="M10 2 4 10h4.5L8 16l6-8H9.5z"/>"#);
icon!(
    USER,
    r#"<circle cx="9" cy="6.25" r="3"/><path d="M3.25 15.5c.85-2.85 3.1-4.25 5.75-4.25s4.9 1.4 5.75 4.25"/>"#
);
icon!(CHEVRON_DOWN, r#"<path d="m5 7 4 4 4-4"/>"#);
icon!(CHEVRON_RIGHT, r#"<path d="m7 5 4 4-4 4"/>"#);
icon!(
    BUSY,
    r#"<path d="M15 9a6 6 0 1 1-1.76-4.24"/><path d="M15 3v3.5h-3.5"/>"#
);
/// Spark's asterisk from `coincube-ui/static/logos/spark_badge.svg`. A filled
/// brand mark, not a stroke icon, fitted to about 15px of the 18px cell.
pub const SPARK: &str = concat!(
    r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 18 18"><path transform="translate(1.5 1.75) scale(0.2229)" fill="#fff" fill-rule="evenodd" d="M39.7159 25.248L40.8727 0.570312H26.4219L27.5787 25.2483L4.46555 16.5221L0 30.2656L23.8282 36.7915L8.38717 56.0763L20.0781 64.5703L33.6483 43.9245L47.2179 64.5695L58.9089 56.0755L43.4679 36.7909L67.2937 30.2657L62.8281 16.5221L39.7159 25.248Z"/>"##,
    "</svg>"
);
// Spark's stroked asterisk (`.ic` at stroke 2): the canvas's Spark mark for
// navigation and wallet rows. Coin network badges keep the filled `SPARK`.
icon!(
    SPARK_STROKE,
    r#"<path stroke-width="2" d="M9 2.5v13M3.37 5.75l11.26 6.5M3.37 12.25l11.26-6.5"/>"#
);
icon!(
    SHIELD,
    r#"<path d="M9 2 3.5 4v4.5c0 3.4 2.3 6.1 5.5 7.5 3.2-1.4 5.5-4.1 5.5-7.5V4z"/>"#
);
icon!(
    DROPLET,
    r#"<path d="M9 2.5c2.6 3.2 4.5 5.8 4.5 8a4.5 4.5 0 0 1-9 0c0-2.2 1.9-4.8 4.5-8z"/>"#
);
icon!(
    GEAR,
    r#"<circle cx="9" cy="9" r="2.25"/><circle cx="9" cy="9" r="5"/><path d="M9 2v2M9 14v2M2 9h2M14 9h2M4.05 4.05l1.41 1.41M12.54 12.54l1.41 1.41M4.05 13.95l1.41-1.41M12.54 5.46l1.41-1.41"/>"#
);
icon!(
    WARNING,
    r#"<path d="M9 2.5 16 15H2z"/><path d="M9 7.5v3M9 12.75v.01"/>"#
);
icon!(CHEVRON_UP, r#"<path d="m5 11 4-4 4 4"/>"#);
icon!(RECEIVED, r#"<path d="M13 5 5 13M5 7.5V13h5.5"/>"#);
icon!(SENT, r#"<path d="M5 13 13 5M7.5 5H13v5.5"/>"#);
icon!(
    TRANSFER,
    r#"<path d="M3 6.5h11.5M11.5 3.5l3 3-3 3M15 11.5H3.5M6.5 8.5l-3 3 3 3"/>"#
);
icon!(
    CHECK_CIRCLE,
    r#"<circle cx="9" cy="9" r="6.5"/><path d="m6.25 9.25 1.9 1.9 3.6-3.9"/>"#
);
icon!(
    PENDING,
    r#"<circle cx="9" cy="9" r="6.5"/><path d="M9 5.5V9l2 1.5"/>"#
);
icon!(
    X_CIRCLE,
    r#"<circle cx="9" cy="9" r="6.5"/><path d="m6.75 6.75 4.5 4.5M11.25 6.75l-4.5 4.5"/>"#
);
icon!(
    SIGNING,
    r#"<path d="M11.5 3.5l3 3-8 8h-3v-3z"/><path d="M10 5l3 3"/>"#
);
icon!(
    LOCK,
    r#"<rect x="3.5" y="8" width="11" height="7.5" rx="1.5"/><path d="M6 8V5.75a3 3 0 0 1 6 0V8"/>"#
);
icon!(CLOSE, r#"<path d="m4.5 4.5 9 9M13.5 4.5l-9 9"/>"#);

/// One icon at `size` px, tinted by `color` (the canvas's `currentColor`).
pub fn glyph<'a, M: 'a>(
    source: &'static str,
    size: f32,
    color: fn(&theme::Theme) -> Color,
) -> Element<'a, M> {
    svg(svg::Handle::from_memory(source.as_bytes()))
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .style(move |theme: &theme::Theme, _| svg::Style {
            color: Some(color(theme)),
        })
        .into()
}

pub fn muted(theme: &theme::Theme) -> Color {
    theme.colors.shell.muted
}

pub fn ink(theme: &theme::Theme) -> Color {
    theme.colors.shell.ink
}

pub fn accent(theme: &theme::Theme) -> Color {
    theme.colors.shell.accent_ink
}

pub fn good(theme: &theme::Theme) -> Color {
    theme.colors.shell.good
}

pub fn warn(theme: &theme::Theme) -> Color {
    theme.colors.shell.warn
}

pub fn bad(theme: &theme::Theme) -> Color {
    theme.colors.shell.bad
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_is_a_complete_stroke_svg() {
        for source in [
            GRID,
            CLOCK,
            SHIELD_CHECK,
            BOLT,
            SPARK_STROKE,
            SHIELD,
            DROPLET,
            GEAR,
            WARNING,
            CHEVRON_DOWN,
            CHEVRON_UP,
            RECEIVED,
            SENT,
            TRANSFER,
            CHECK_CIRCLE,
            PENDING,
            X_CIRCLE,
            SIGNING,
            LOCK,
            CLOSE,
            coincube_ui::component::tenshu::copy_icon::COPY_GLYPH,
        ] {
            assert!(source.starts_with("<svg") && source.ends_with("</svg>"));
            assert!(source.contains(r#"viewBox="0 0 18 18""#));
            assert!(resvg::usvg::Tree::from_str(source, &resvg::usvg::Options::default()).is_ok());
        }
    }

    #[test]
    fn spark_is_the_badge_asterisk() {
        let badge = include_str!("../../../../../coincube-ui/static/logos/spark_badge.svg");
        let path = SPARK
            .split(" d=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        assert!(
            badge.contains(path),
            "{}",
            "the Spark glyph drifted from the badge"
        );
        assert!(SPARK.contains(r#"viewBox="0 0 18 18""#));
        assert!(resvg::usvg::Tree::from_str(SPARK, &resvg::usvg::Options::default()).is_ok());
    }

    #[test]
    fn spark_stroke_is_the_canvas_asterisk() {
        // The side panel's Spark row and the Activity wallet cell on the v175
        // boards.
        let path =
            r#"<path stroke-width="2" d="M9 2.5v13M3.37 5.75l11.26 6.5M3.37 12.25l11.26-6.5"/>"#;
        for board in [
            include_str!("../../../../../design-review/canvas/P-Overview.dc.html"),
            include_str!("../../../../../design-review/canvas/P-Activity.dc.html"),
        ] {
            assert!(board.contains(path), "the board's Spark mark changed");
        }
        assert!(
            SPARK_STROKE.contains(path),
            "the stroked Spark glyph drifted from the boards"
        );
    }
}
