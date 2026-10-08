//! Single source of the map item geometry and camera math. The map panel
//! module (`component/panels/map`) must re-export or reuse these, never
//! redefine them.
use std::{collections::HashSet, time::Duration};

use iced::{mouse, time::Instant, Point, Rectangle, Size, Transformation, Vector};

use crate::widget::graph_view::{AnchorSide, ItemId, Shape, Side, Target};

/// Grid unit.
pub const U: f32 = 12.0;
pub const INPUT_COLUMN_WIDTH: f32 = 192.0;
pub const MIDDLE_COLUMN_WIDTH: f32 = 252.0;
pub const OUTPUT_COLUMN_WIDTH: f32 = 252.0;
pub const BLOCK_WIDTH: f32 = INPUT_COLUMN_WIDTH + MIDDLE_COLUMN_WIDTH + OUTPUT_COLUMN_WIDTH;
pub const SLOT_HEIGHT: f32 = 48.0;
pub const BLOCK_MIN_HEIGHT: f32 = 156.0;
pub const LEAF_WIDTH: f32 = 216.0;
pub const LEAF_HEIGHT: f32 = 36.0;

pub const MIN_ZOOM: f32 = 0.1;
pub const MAX_ZOOM: f32 = 2.5;
pub const FIT_PADDING: f32 = 72.0;
pub const FIT_MAX_ZOOM: f32 = 1.1;
pub const FOCUS_ZOOM: f32 = 1.0;
pub const ZOOM_STEP: f32 = 1.25;
pub const WHEEL_ZOOM_RATE: f32 = 0.0015;
/// Pixels per wheel line, to match the browser pixel convention of the spec.
pub const WHEEL_LINE_PX: f32 = 100.0;
/// Screen px a pressed pointer may move before it counts as a drag.
pub const CLICK_THRESHOLD: f32 = 5.0;

pub const FRAME_PADDING: f32 = 24.0;
pub const FRAME_RADIUS: f32 = 20.0;
pub const FRAME_WIDTH: f32 = 2.0;
pub const GRID_MIN_SCREEN_STEP: f32 = 10.0;
pub const GRID_DOT_RADIUS: f32 = 1.3;
pub const EDGE_MIN_DX: f32 = 50.0;
pub const BEZIER_SAMPLES: usize = 24;
pub const COIN_EDGE_WIDTH: f32 = 2.0;
pub const COIN_EDGE_ACTIVE_WIDTH: f32 = 3.0;
pub const COIN_EDGE_OPACITY: f32 = 0.5;
pub const COUNTERPARTY_EDGE_WIDTH: f32 = 1.5;
pub const COUNTERPARTY_EDGE_ACTIVE_WIDTH: f32 = 2.5;
pub const COUNTERPARTY_DASH: [f32; 2] = [6.0, 5.0];
pub const EDGE_DIMMED_OPACITY: f32 = 0.15;
pub const MARKER_STUB: f32 = 12.0;
pub const MARKER_STUB_HEIGHT: f32 = 2.0;
pub const MARKER_RING: f32 = 8.0;
pub const MARKER_RING_WIDTH: f32 = 2.0;
/// Half of the 14 px edge hit stroke, in screen px.
pub const EDGE_HIT_HALF_WIDTH: f32 = 7.0;
pub const DOUBLE_CLICK: Duration = Duration::from_millis(400);
pub const AREA_OPACITY: f32 = 0.7;
pub const AREA_BORDER_WIDTH: f32 = 1.0;

impl Shape {
    pub fn size(self) -> Size {
        match self {
            Shape::Block { inputs, outputs } => Size::new(
                BLOCK_WIDTH,
                BLOCK_MIN_HEIGHT.max(inputs.max(outputs) as f32 * SLOT_HEIGHT),
            ),
            Shape::Leaf => Size::new(LEAF_WIDTH, LEAF_HEIGHT),
        }
    }
}

/// A graph point `g` is shown at the widget-relative point `g * zoom + offset`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub offset: Vector,
    pub zoom: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            offset: Vector::ZERO,
            zoom: 1.0,
        }
    }
}

impl Camera {
    pub fn to_screen(self, graph: Point) -> Point {
        Point::new(
            graph.x * self.zoom + self.offset.x,
            graph.y * self.zoom + self.offset.y,
        )
    }

    pub fn to_graph(self, screen: Point) -> Point {
        Point::new(
            (screen.x - self.offset.x) / self.zoom,
            (screen.y - self.offset.y) / self.zoom,
        )
    }

    /// Maps the absolute layout point `bounds.position() + g` to
    /// `bounds.position() + to_screen(g)`.
    pub fn transformation(self, bounds: Rectangle) -> Transformation {
        Transformation::translate(bounds.x + self.offset.x, bounds.y + self.offset.y)
            * Transformation::scale(self.zoom)
            * Transformation::translate(-bounds.x, -bounds.y)
    }

    /// Zooms by `factor` keeping the graph point under the widget-relative
    /// `anchor` in place.
    pub fn zoom_around(self, anchor: Point, factor: f32) -> Camera {
        let zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let graph = self.to_graph(anchor);
        Camera {
            offset: Vector::new(anchor.x - graph.x * zoom, anchor.y - graph.y * zoom),
            zoom,
        }
    }

    pub fn fit(content: Rectangle, size: Size) -> Camera {
        let width = content.width + 2.0 * FIT_PADDING;
        let height = content.height + 2.0 * FIT_PADDING;
        let zoom = (size.width / width)
            .min(size.height / height)
            .min(FIT_MAX_ZOOM)
            .clamp(MIN_ZOOM, MAX_ZOOM);
        Camera::centered_on(content.center(), zoom, size)
    }

    pub fn centered_on(center: Point, zoom: f32, size: Size) -> Camera {
        Camera {
            offset: Vector::new(
                size.width / 2.0 - center.x * zoom,
                size.height / 2.0 - center.y * zoom,
            ),
            zoom,
        }
    }
}

/// Where an edge attaches, in graph px. `row` is fractional so a slot being
/// reordered can sit between two rows.
pub fn anchor_point(side: AnchorSide, row: f32, position: Point, shape: Shape) -> Point {
    let slot_y = position.y + row * SLOT_HEIGHT + SLOT_HEIGHT / 2.0;
    let right = position.x + shape.size().width;
    match side {
        AnchorSide::Input => Point::new(position.x, slot_y),
        AnchorSide::Output => Point::new(right, slot_y),
        AnchorSide::LeafLeft => Point::new(position.x, position.y + LEAF_HEIGHT / 2.0),
        AnchorSide::LeafRight => Point::new(right, position.y + LEAF_HEIGHT / 2.0),
    }
}

/// Bezier control polygon of an edge.
pub fn edge_curve(from: Point, to: Point) -> [Point; 4] {
    let dx = EDGE_MIN_DX.max((to.x - from.x).abs() / 2.0);
    [
        from,
        Point::new(from.x + dx, from.y),
        Point::new(to.x - dx, to.y),
        to,
    ]
}

pub fn bezier_point(curve: &[Point; 4], t: f32) -> Point {
    let u = 1.0 - t;
    let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    Point::new(
        a * curve[0].x + b * curve[1].x + c * curve[2].x + d * curve[3].x,
        a * curve[0].y + b * curve[1].y + c * curve[2].y + d * curve[3].y,
    )
}

/// Dot grid step in graph px. Doubles past the spec's single doubling when the
/// step is still too tight, otherwise a far zoom-out would draw hundreds of
/// thousands of dots.
pub fn grid_step(zoom: f32) -> f32 {
    let mut step = U;
    while step * zoom < GRID_MIN_SCREEN_STEP {
        step *= 2.0;
    }
    step
}

pub fn frame_rect(members: Rectangle) -> Rectangle {
    members.expand(FRAME_PADDING)
}

/// Vertical wheel delta in px, positive when scrolling down (iced reports
/// positive `y` when scrolling up).
pub fn wheel_delta(delta: mouse::ScrollDelta) -> f32 {
    match delta {
        mouse::ScrollDelta::Lines { y, .. } => -y * WHEEL_LINE_PX,
        mouse::ScrollDelta::Pixels { y, .. } => -y,
    }
}

pub fn wheel_zoom_factor(delta: f32) -> f32 {
    (-delta * WHEEL_ZOOM_RATE).exp()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemHit {
    Body,
    Slot(Side, usize),
}

/// What `point` is on inside the item at `position`, all in graph px.
pub fn hit_item(point: Point, position: Point, shape: Shape) -> Option<ItemHit> {
    if !Rectangle::new(position, shape.size()).contains(point) {
        return None;
    }
    let Shape::Block { inputs, outputs } = shape else {
        return Some(ItemHit::Body);
    };
    let (lx, ly) = (point.x - position.x, point.y - position.y);
    let row = (ly / SLOT_HEIGHT) as usize;
    if lx < INPUT_COLUMN_WIDTH && row < inputs {
        Some(ItemHit::Slot(Side::Input, row))
    } else if lx >= BLOCK_WIDTH - OUTPUT_COLUMN_WIDTH && row < outputs {
        Some(ItemHit::Slot(Side::Output, row))
    } else {
        Some(ItemHit::Body)
    }
}

fn segment_distance(p: Point, a: Point, b: Point) -> f32 {
    let ab = b - a;
    let len_sq = ab.x * ab.x + ab.y * ab.y;
    let t = if len_sq == 0.0 {
        0.0
    } else {
        (((p.x - a.x) * ab.x + (p.y - a.y) * ab.y) / len_sq).clamp(0.0, 1.0)
    };
    p.distance(Point::new(a.x + t * ab.x, a.y + t * ab.y))
}

/// Distance from `point` to the sampled polyline of the bezier `curve`.
pub fn distance_to_curve(point: Point, curve: &[Point; 4]) -> f32 {
    let samples: Vec<Point> = (0..=BEZIER_SAMPLES)
        .map(|i| bezier_point(curve, i as f32 / BEZIER_SAMPLES as f32))
        .collect();
    samples
        .windows(2)
        .map(|w| segment_distance(point, w[0], w[1]))
        .fold(f32::INFINITY, f32::min)
}

pub fn rect_from_points(a: Point, b: Point) -> Rectangle {
    Rectangle::new(
        Point::new(a.x.min(b.x), a.y.min(b.y)),
        Size::new((a.x - b.x).abs(), (a.y - b.y).abs()),
    )
}

/// Items fully inside `rect`, or touched by it when `crossing`.
pub fn area_pick(
    rect: Rectangle,
    crossing: bool,
    items: impl IntoIterator<Item = (ItemId, Rectangle)>,
) -> Vec<ItemId> {
    items
        .into_iter()
        .filter(|(_, item)| {
            if crossing {
                rect.intersects(item)
            } else {
                item.is_within(&rect)
            }
        })
        .map(|(id, _)| id)
        .collect()
}

pub fn snap_to_grid(point: Point) -> Point {
    Point::new((point.x / U).round() * U, (point.y / U).round() * U)
}

/// The whole selection when `id` is one of two or more selected items.
pub fn drag_set(id: ItemId, selected: &HashSet<ItemId>) -> Vec<ItemId> {
    if selected.len() >= 2 && selected.contains(&id) {
        let mut items: Vec<ItemId> = selected.iter().copied().collect();
        items.sort();
        items
    } else {
        vec![id]
    }
}

#[derive(Debug, Default)]
pub struct ClickTracker {
    last: Option<(Target, Instant)>,
}

impl ClickTracker {
    /// True when the previous click was on the same target within
    /// `DOUBLE_CLICK`. A detected double click starts over.
    pub fn register(&mut self, target: Target, at: Instant) -> bool {
        let double = self
            .last
            .is_some_and(|(last, time)| last == target && at.duration_since(time) <= DOUBLE_CLICK);
        self.last = if double { None } else { Some((target, at)) };
        double
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    fn close_point(a: Point, b: Point) -> bool {
        close(a.x, b.x) && close(a.y, b.y)
    }

    #[test]
    fn camera_round_trip() {
        let camera = Camera {
            offset: Vector::new(13.0, -7.0),
            zoom: 0.37,
        };
        for p in [
            Point::ORIGIN,
            Point::new(100.0, 250.0),
            Point::new(-40.0, 3.5),
        ] {
            assert!(close_point(camera.to_graph(camera.to_screen(p)), p));
        }
    }

    #[test]
    fn transformation_matches_to_screen() {
        let bounds = Rectangle::new(Point::new(100.0, 50.0), Size::new(800.0, 600.0));
        let camera = Camera {
            offset: Vector::new(13.0, -7.0),
            zoom: 0.37,
        };
        let transformation = camera.transformation(bounds);
        for g in [Point::ORIGIN, Point::new(120.0, 80.0)] {
            let absolute = Point::new(bounds.x + g.x, bounds.y + g.y);
            let expected = camera.to_screen(g);
            let expected = Point::new(bounds.x + expected.x, bounds.y + expected.y);
            let mapped = absolute * transformation;
            assert!(close_point(mapped, expected));
            assert!(close_point(mapped * transformation.inverse(), absolute));
        }
    }

    #[test]
    fn zoom_around_keeps_anchor() {
        let camera = Camera {
            offset: Vector::new(30.0, 10.0),
            zoom: 0.8,
        };
        let anchor = Point::new(200.0, 150.0);
        let zoomed = camera.zoom_around(anchor, 1.7);
        assert!(close_point(
            zoomed.to_graph(anchor),
            camera.to_graph(anchor)
        ));
    }

    #[test]
    fn zoom_is_clamped() {
        let camera = Camera::default();
        assert_eq!(camera.zoom_around(Point::ORIGIN, 1000.0).zoom, MAX_ZOOM);
        assert_eq!(camera.zoom_around(Point::ORIGIN, 0.0001).zoom, MIN_ZOOM);
    }

    #[test]
    fn fit_centers_and_caps() {
        let size = Size::new(1000.0, 800.0);
        let content = Rectangle::new(Point::new(40.0, 60.0), Size::new(100.0, 100.0));
        let camera = Camera::fit(content, size);
        assert!(close(camera.zoom, FIT_MAX_ZOOM));
        assert!(close_point(
            camera.to_screen(content.center()),
            Point::new(500.0, 400.0)
        ));

        let wide = Rectangle::new(Point::ORIGIN, Size::new(8000.0, 500.0));
        let camera = Camera::fit(wide, size);
        assert!(close(camera.zoom, 1000.0 / (8000.0 + 2.0 * FIT_PADDING)));
    }

    #[test]
    fn centered_on_focus() {
        let size = Size::new(900.0, 500.0);
        let center = Point::new(321.0, -45.0);
        let camera = Camera::centered_on(center, FOCUS_ZOOM, size);
        assert_eq!(camera.zoom, 1.0);
        assert!(close_point(
            camera.to_screen(center),
            Point::new(450.0, 250.0)
        ));
    }

    #[test]
    fn wheel_direction() {
        let up = wheel_delta(mouse::ScrollDelta::Lines { x: 0.0, y: 1.0 });
        assert!(up < 0.0);
        assert!(wheel_zoom_factor(up) > 1.0);

        let down = wheel_delta(mouse::ScrollDelta::Pixels { x: 0.0, y: -30.0 });
        assert_eq!(down, 30.0);
        assert!(wheel_zoom_factor(down) < 1.0);
    }

    #[test]
    fn anchor_points() {
        let block = Shape::Block {
            inputs: 3,
            outputs: 5,
        };
        let position = Point::new(120.0, 240.0);
        assert_eq!(
            anchor_point(AnchorSide::Input, 2.0, position, block),
            Point::new(120.0, 240.0 + 2.0 * 48.0 + 24.0)
        );
        assert_eq!(
            anchor_point(AnchorSide::Output, 0.0, position, block),
            Point::new(120.0 + 696.0, 264.0)
        );
        assert_eq!(
            anchor_point(AnchorSide::LeafLeft, 0.0, Point::ORIGIN, Shape::Leaf),
            Point::new(0.0, 18.0)
        );
        assert_eq!(
            anchor_point(AnchorSide::LeafRight, 0.0, Point::ORIGIN, Shape::Leaf),
            Point::new(216.0, 18.0)
        );
    }

    #[test]
    fn edge_curve_min_dx() {
        let c = edge_curve(Point::ORIGIN, Point::new(40.0, 100.0));
        assert_eq!(c[1], Point::new(50.0, 0.0));
        assert_eq!(c[2], Point::new(-10.0, 100.0));
        let c = edge_curve(Point::ORIGIN, Point::new(400.0, 0.0));
        assert_eq!(c[1], Point::new(200.0, 0.0));
        assert_eq!(c[2], Point::new(200.0, 0.0));
    }

    #[test]
    fn bezier_endpoints() {
        let c = edge_curve(Point::new(10.0, 20.0), Point::new(300.0, 90.0));
        assert!(close_point(bezier_point(&c, 0.0), c[0]));
        assert!(close_point(bezier_point(&c, 1.0), c[3]));
        let flat = edge_curve(Point::ORIGIN, Point::new(400.0, 0.0));
        assert!(close(bezier_point(&flat, 0.5).y, 0.0));
    }

    #[test]
    fn grid_step_adapts() {
        assert_eq!(grid_step(1.0), 12.0);
        assert_eq!(grid_step(0.5), 24.0);
        assert!(grid_step(0.1) * 0.1 >= GRID_MIN_SCREEN_STEP);
    }

    #[test]
    fn frame_rect_padding() {
        let members = Rectangle::new(Point::ORIGIN, Size::new(100.0, 50.0));
        assert_eq!(
            frame_rect(members),
            Rectangle::new(Point::new(-24.0, -24.0), Size::new(148.0, 98.0))
        );
    }

    #[test]
    fn shape_sizes() {
        assert_eq!(
            Shape::Block {
                inputs: 3,
                outputs: 5
            }
            .size(),
            Size::new(696.0, 240.0)
        );
        assert_eq!(
            Shape::Block {
                inputs: 1,
                outputs: 1
            }
            .size(),
            Size::new(696.0, 156.0)
        );
        assert_eq!(Shape::Leaf.size(), Size::new(216.0, 36.0));
    }

    #[test]
    fn hit_item_regions() {
        let block = Shape::Block {
            inputs: 2,
            outputs: 3,
        };
        let origin = Point::ORIGIN;
        let hit = |x, y| hit_item(Point::new(x, y), origin, block);
        assert_eq!(hit(10.0, 10.0), Some(ItemHit::Slot(Side::Input, 0)));
        assert_eq!(hit(10.0, 60.0), Some(ItemHit::Slot(Side::Input, 1)));
        assert_eq!(hit(10.0, 100.0), Some(ItemHit::Body));
        assert_eq!(hit(300.0, 10.0), Some(ItemHit::Body));
        assert_eq!(hit(600.0, 130.0), Some(ItemHit::Slot(Side::Output, 2)));
        assert_eq!(hit(700.0, 10.0), None);
        assert_eq!(
            hit_item(Point::new(10.0, 10.0), origin, Shape::Leaf),
            Some(ItemHit::Body)
        );
    }

    #[test]
    fn curve_distance() {
        let curve = edge_curve(Point::ORIGIN, Point::new(400.0, 0.0));
        assert!(distance_to_curve(Point::new(100.0, 0.0), &curve) < 0.5);
        assert!((distance_to_curve(Point::new(200.0, 10.0), &curve) - 10.0).abs() < 0.5);
        assert!(distance_to_curve(Point::new(200.0, 100.0), &curve) > EDGE_HIT_HALF_WIDTH);
    }

    #[test]
    fn area_pick_inside_vs_crossing() {
        let rect = Rectangle::new(Point::ORIGIN, Size::new(100.0, 100.0));
        let size = Size::new(20.0, 20.0);
        let items = [
            (ItemId(1), Rectangle::new(Point::new(10.0, 10.0), size)),
            (ItemId(2), Rectangle::new(Point::new(90.0, 10.0), size)),
            (ItemId(3), Rectangle::new(Point::new(200.0, 10.0), size)),
        ];
        assert_eq!(area_pick(rect, false, items), vec![ItemId(1)]);
        assert_eq!(area_pick(rect, true, items), vec![ItemId(1), ItemId(2)]);
    }

    #[test]
    fn rect_from_points_normalizes() {
        assert_eq!(
            rect_from_points(Point::new(10.0, 50.0), Point::new(-5.0, 20.0)),
            Rectangle::new(Point::new(-5.0, 20.0), Size::new(15.0, 30.0))
        );
    }

    #[test]
    fn snap_rounds_to_grid() {
        assert_eq!(snap_to_grid(Point::new(5.9, 6.1)), Point::new(0.0, 12.0));
        assert_eq!(
            snap_to_grid(Point::new(-7.0, 18.0)),
            Point::new(-12.0, 24.0)
        );
    }

    #[test]
    fn drag_set_rules() {
        let one: HashSet<ItemId> = [ItemId(2)].into();
        let three: HashSet<ItemId> = [ItemId(3), ItemId(1), ItemId(2)].into();
        assert_eq!(drag_set(ItemId(9), &three), vec![ItemId(9)]);
        assert_eq!(drag_set(ItemId(2), &one), vec![ItemId(2)]);
        assert_eq!(
            drag_set(ItemId(3), &three),
            vec![ItemId(1), ItemId(2), ItemId(3)]
        );
    }

    #[test]
    fn double_click() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let a = Target::Frame;
        let b = Target::Edge(0);

        let mut clicks = ClickTracker::default();
        assert!(!clicks.register(a, t0));
        assert!(clicks.register(a, t0 + ms(400)));
        assert!(!clicks.register(a, t0 + ms(500)));

        let mut clicks = ClickTracker::default();
        clicks.register(a, t0);
        assert!(!clicks.register(a, t0 + ms(401)));

        let mut clicks = ClickTracker::default();
        clicks.register(a, t0);
        assert!(!clicks.register(b, t0 + ms(100)));
    }
}
