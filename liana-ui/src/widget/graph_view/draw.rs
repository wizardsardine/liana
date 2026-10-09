//! Layers of the graph view that are not child elements.
use iced::{
    advanced::{renderer, Renderer as _},
    border,
    widget::canvas::{self, LineDash, Path, Stroke},
    Border, Color, Point, Rectangle, Size,
};

use crate::{
    theme::palette::Graph,
    widget::{
        graph_view::{
            geometry::{
                frame_rect, grid_step, Camera, AREA_BORDER_WIDTH, AREA_OPACITY,
                COIN_EDGE_ACTIVE_WIDTH, COIN_EDGE_OPACITY, COIN_EDGE_WIDTH, COUNTERPARTY_DASH,
                COUNTERPARTY_EDGE_ACTIVE_WIDTH, COUNTERPARTY_EDGE_WIDTH, EDGE_DIMMED_OPACITY,
                FRAME_RADIUS, FRAME_WIDTH, GRID_DOT_RADIUS, MARKER_RING, MARKER_RING_WIDTH,
                MARKER_STUB, MARKER_STUB_HEIGHT,
            },
            EdgeKind,
        },
        Renderer,
    },
};

/// Dots at every `grid_step` graph px, drawn in screen space.
pub fn grid(
    renderer: &mut Renderer,
    palette: &Graph,
    bounds: Rectangle,
    clip: Rectangle,
    camera: Camera,
) {
    let step = grid_step(camera.zoom);
    let top_left = camera.to_graph(Point::new(clip.x - bounds.x, clip.y - bounds.y));
    let bottom_right = camera.to_graph(Point::new(
        clip.x + clip.width - bounds.x,
        clip.y + clip.height - bounds.y,
    ));
    let mut x = (top_left.x / step).floor() * step;
    while x <= bottom_right.x {
        let mut y = (top_left.y / step).floor() * step;
        while y <= bottom_right.y {
            let screen = camera.to_screen(Point::new(x, y));
            renderer.fill_quad(
                renderer::Quad {
                    bounds: Rectangle::new(
                        Point::new(
                            bounds.x + screen.x - GRID_DOT_RADIUS,
                            bounds.y + screen.y - GRID_DOT_RADIUS,
                        ),
                        Size::new(2.0 * GRID_DOT_RADIUS, 2.0 * GRID_DOT_RADIUS),
                    ),
                    border: border::rounded(GRID_DOT_RADIUS),
                    ..renderer::Quad::default()
                },
                palette.grid_dot,
            );
            y += step;
        }
        x += step;
    }
}

/// Dashed frame around `members` (graph px), constant width on screen.
pub fn frame(
    renderer: &mut Renderer,
    palette: &Graph,
    bounds: Rectangle,
    camera: Camera,
    members: Rectangle,
) {
    let rect = frame_rect(members);
    let top_left = camera.to_screen(rect.position());
    let bottom_right = camera.to_screen(Point::new(rect.x + rect.width, rect.y + rect.height));
    renderer.fill_quad(
        renderer::Quad {
            bounds: Rectangle::new(
                Point::new(bounds.x + top_left.x, bounds.y + top_left.y),
                Size::new(bottom_right.x - top_left.x, bottom_right.y - top_left.y),
            ),
            border: Border::default()
                .color(palette.frame)
                .width(FRAME_WIDTH)
                .rounded(FRAME_RADIUS * camera.zoom)
                .dashed(),
            ..renderer::Quad::default()
        },
        Color::TRANSPARENT,
    );
}

/// Strokes screen-space curves, active ones last so they sit on top.
pub fn edges(
    frame: &mut canvas::Frame,
    palette: &Graph,
    curves: &[(EdgeKind, bool, Option<Color>, [Point; 4])],
    dim: bool,
) {
    for active in [false, true] {
        for (kind, _, color, c) in curves.iter().filter(|(_, a, _, _)| *a == active) {
            let path = Path::new(|b| {
                b.move_to(c[0]);
                b.bezier_curve_to(c[1], c[2], c[3]);
            });
            let (color, width, dashed) = match (kind, active) {
                (EdgeKind::Coin, false) => (
                    color
                        .unwrap_or(palette.edge_coin)
                        .scale_alpha(COIN_EDGE_OPACITY),
                    COIN_EDGE_WIDTH,
                    false,
                ),
                (EdgeKind::Coin, true) => (
                    color.unwrap_or(palette.edge_coin),
                    COIN_EDGE_ACTIVE_WIDTH,
                    false,
                ),
                (EdgeKind::Counterparty, false) => (
                    color.unwrap_or(palette.edge_counterparty),
                    COUNTERPARTY_EDGE_WIDTH,
                    true,
                ),
                (EdgeKind::Counterparty, true) => (
                    color.unwrap_or(palette.edge_counterparty_active),
                    COUNTERPARTY_EDGE_ACTIVE_WIDTH,
                    true,
                ),
            };
            let color = if dim {
                color.scale_alpha(EDGE_DIMMED_OPACITY)
            } else {
                color
            };
            let segments: &[f32] = if dashed { &COUNTERPARTY_DASH } else { &[] };
            frame.stroke(
                &path,
                Stroke {
                    line_dash: LineDash {
                        segments,
                        offset: 0,
                    },
                    ..Stroke::default().with_color(color).with_width(width)
                },
            );
        }
    }
}

/// Unspent markers at output anchors (graph px), drawn under the camera
/// transformation so they scale with the map.
pub fn markers(renderer: &mut Renderer, palette: &Graph, points: &[Point]) {
    for p in points {
        renderer.fill_quad(
            renderer::Quad {
                bounds: Rectangle::new(
                    Point::new(p.x, p.y - MARKER_STUB_HEIGHT / 2.0),
                    Size::new(MARKER_STUB, MARKER_STUB_HEIGHT),
                ),
                ..renderer::Quad::default()
            },
            palette.marker,
        );
        renderer.fill_quad(
            renderer::Quad {
                bounds: Rectangle::new(
                    Point::new(p.x + MARKER_STUB, p.y - MARKER_RING / 2.0),
                    Size::new(MARKER_RING, MARKER_RING),
                ),
                border: Border::default()
                    .color(palette.marker)
                    .width(MARKER_RING_WIDTH)
                    .rounded(MARKER_RING / 2.0),
                ..renderer::Quad::default()
            },
            palette.marker_fill,
        );
    }
}

/// Selection rectangle in absolute screen px: solid when picking items fully
/// inside, dashed when picking every item touched.
pub fn area(renderer: &mut Renderer, palette: &Graph, rect: Rectangle, crossing: bool) {
    let (color, fill) = if crossing {
        (palette.area_crossing, palette.area_crossing_fill)
    } else {
        (palette.area_inside, palette.area_inside_fill)
    };
    let border = Border::default().color(color).width(AREA_BORDER_WIDTH);
    renderer.fill_quad(
        renderer::Quad {
            bounds: rect,
            border: if crossing { border.dashed() } else { border },
            ..renderer::Quad::default()
        },
        fill.scale_alpha(AREA_OPACITY),
    );
}
