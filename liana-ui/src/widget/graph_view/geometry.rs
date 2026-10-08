//! Single source of the map item geometry and camera math. The map panel
//! module (`component/panels/map`) must re-export or reuse these, never
//! redefine them.
use iced::{mouse, Point, Rectangle, Size, Transformation, Vector};

use crate::widget::graph_view::Shape;

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
}
