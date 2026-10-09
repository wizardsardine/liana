//! Layers of the graph view that are not child elements.
use iced::{
    advanced::{
        renderer,
        text::{Alignment, Shaping},
        Renderer as _,
    },
    alignment::Vertical,
    border,
    widget::canvas::{self, LineDash, Path, Stroke},
    Border, Color, Pixels, Point, Rectangle, Size,
};

use crate::{
    component::text::{new::SMALL_CAPTION_SPEC, truncate},
    icon,
    theme::{palette::Graph, Theme},
    widget::{
        graph_view::{
            geometry::{
                frame_rect, grid_step, Camera, AREA_BORDER_WIDTH, AREA_OPACITY,
                COIN_EDGE_ACTIVE_WIDTH, COIN_EDGE_OPACITY, COIN_EDGE_WIDTH, COUNTERPARTY_DASH,
                COUNTERPARTY_EDGE_ACTIVE_WIDTH, COUNTERPARTY_EDGE_WIDTH, EDGE_DIMMED_OPACITY,
                FRAME_RADIUS, FRAME_WIDTH, GRID_DOT_RADIUS, HANDLE_CHECKBOX_AREA,
                HANDLE_CHECKBOX_SIZE, HANDLE_DOT_RADIUS, HANDLE_DROP_LINE_WIDTH, HANDLE_GRIP_SIZE,
                HANDLE_GRIP_WIDTH, HANDLE_INSET, HANDLE_NAME_MAX_CHARS, HANDLE_NAME_X,
                HANDLE_RADIUS, HANDLE_WIDTH, LANE_BAND_OPACITY, LANE_SEPARATOR_WIDTH,
                LANE_STRIP_WIDTH, MARKER_RING, MARKER_RING_WIDTH, MARKER_STUB, MARKER_STUB_HEIGHT,
                SPACE_LINE_WIDTH,
            },
            EdgeKind, Handle, Lane,
        },
        Renderer,
    },
};

const HIDDEN_HANDLE_OPACITY: f32 = 0.5;
const CHECKBOX_RADIUS: f32 = 3.0;
const CHECKBOX_STROKE_WIDTH: f32 = 1.5;
const HANDLE_BORDER_WIDTH: f32 = 1.0;

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

/// Space drag preview in screen space: an accent line along `x` (graph px).
pub fn space(renderer: &mut Renderer, theme: &Theme, bounds: Rectangle, camera: Camera, x: f32) {
    let line = bounds.x + camera.to_screen(Point::new(x, 0.0)).x;
    renderer.fill_quad(
        renderer::Quad {
            bounds: Rectangle::new(
                Point::new(line - SPACE_LINE_WIDTH / 2.0, bounds.y),
                Size::new(SPACE_LINE_WIDTH, bounds.height),
            ),
            ..renderer::Quad::default()
        },
        theme.colors.general.accent,
    );
}

/// Lane bands in screen space: every other band tinted, a separator under each
/// band but the last and the lane color along the left edge.
pub fn lanes(
    renderer: &mut Renderer,
    theme: &Theme,
    bounds: Rectangle,
    camera: Camera,
    lanes: &[Lane],
) {
    for (index, lane) in lanes.iter().enumerate() {
        let top = bounds.y + camera.to_screen(Point::new(0.0, lane.top)).y;
        let height = lane.height * camera.zoom;
        if index % 2 == 1 {
            renderer.fill_quad(
                renderer::Quad {
                    bounds: Rectangle::new(
                        Point::new(bounds.x, top),
                        Size::new(bounds.width, height),
                    ),
                    ..renderer::Quad::default()
                },
                theme
                    .colors
                    .cards
                    .simple
                    .background
                    .scale_alpha(LANE_BAND_OPACITY),
            );
        }
        renderer.fill_quad(
            renderer::Quad {
                bounds: Rectangle::new(
                    Point::new(bounds.x, top),
                    Size::new(LANE_STRIP_WIDTH, height),
                ),
                ..renderer::Quad::default()
            },
            lane.color,
        );
        if index + 1 < lanes.len() {
            renderer.fill_quad(
                renderer::Quad {
                    bounds: Rectangle::new(
                        Point::new(bounds.x, top + height - LANE_SEPARATOR_WIDTH / 2.0),
                        Size::new(bounds.width, LANE_SEPARATOR_WIDTH),
                    ),
                    ..renderer::Quad::default()
                },
                theme.colors.text.border,
            );
        }
    }
}

/// Lane handle card at `rect` (widget px): grip, color dot, name and a
/// checkbox checked while the lane is displayed.
pub fn handle(frame: &mut canvas::Frame, theme: &Theme, rect: Rectangle, handle: &Handle) {
    let colors = &theme.colors;
    let middle = rect.y + rect.height / 2.0;
    let card = Path::rounded_rectangle(rect.position(), rect.size(), HANDLE_RADIUS.into());
    frame.fill(&card, colors.cards.simple.background);
    frame.stroke(
        &card,
        Stroke::default()
            .with_color(colors.text.border)
            .with_width(HANDLE_BORDER_WIDTH),
    );

    frame.fill_text(canvas::Text {
        content: icon::GRIP_VERTICAL.to_string(),
        position: Point::new(rect.x + HANDLE_GRIP_WIDTH / 2.0, middle),
        color: colors.text.secondary,
        size: Pixels(HANDLE_GRIP_SIZE),
        font: icon::BOOTSTRAP_ICONS,
        align_x: Alignment::Center,
        align_y: Vertical::Center,
        ..canvas::Text::default()
    });

    let (dot, name_color) = if handle.displayed {
        (handle.color, colors.text.primary)
    } else {
        (
            handle.color.scale_alpha(HIDDEN_HANDLE_OPACITY),
            colors.text.secondary,
        )
    };
    frame.fill(
        &Path::circle(
            Point::new(rect.x + HANDLE_GRIP_WIDTH + 2.0 * HANDLE_DOT_RADIUS, middle),
            HANDLE_DOT_RADIUS,
        ),
        dot,
    );

    let mut name = canvas::Text {
        content: truncate(&handle.name, HANDLE_NAME_MAX_CHARS),
        position: Point::new(rect.x + HANDLE_NAME_X, middle),
        color: name_color,
        font: SMALL_CAPTION_SPEC.font,
        align_y: Vertical::Center,
        shaping: Shaping::Advanced,
        ..canvas::Text::default()
    };
    if let Some(size) = SMALL_CAPTION_SPEC.size {
        name.size = Pixels(size as f32);
    }
    frame.fill_text(name);

    let checkbox = Rectangle::new(
        Point::new(
            rect.x + rect.width - (HANDLE_CHECKBOX_AREA + HANDLE_CHECKBOX_SIZE) / 2.0,
            middle - HANDLE_CHECKBOX_SIZE / 2.0,
        ),
        Size::new(HANDLE_CHECKBOX_SIZE, HANDLE_CHECKBOX_SIZE),
    );
    let square =
        Path::rounded_rectangle(checkbox.position(), checkbox.size(), CHECKBOX_RADIUS.into());
    if handle.displayed {
        frame.fill(&square, colors.general.accent);
        let point = |x: f32, y: f32| {
            Point::new(
                checkbox.x + x * checkbox.width,
                checkbox.y + y * checkbox.height,
            )
        };
        let tick = Path::new(|builder| {
            builder.move_to(point(0.25, 0.5));
            builder.line_to(point(0.43, 0.7));
            builder.line_to(point(0.75, 0.3));
        });
        frame.stroke(
            &tick,
            Stroke::default()
                .with_color(colors.general.background)
                .with_width(CHECKBOX_STROKE_WIDTH),
        );
    } else {
        frame.stroke(
            &square,
            Stroke::default()
                .with_color(colors.text.secondary)
                .with_width(CHECKBOX_STROKE_WIDTH),
        );
    }
}

/// Where a dragged handle would land, across the handle column at `y`.
pub fn drop_line(frame: &mut canvas::Frame, theme: &Theme, y: f32) {
    frame.stroke(
        &Path::line(
            Point::new(HANDLE_INSET, y),
            Point::new(HANDLE_INSET + HANDLE_WIDTH, y),
        ),
        Stroke::default()
            .with_color(theme.colors.general.accent)
            .with_width(HANDLE_DROP_LINE_WIDTH),
    );
}
