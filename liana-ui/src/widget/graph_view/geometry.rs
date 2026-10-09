//! Single source of the map item geometry and camera math. The map panel
//! module (`component/panels/map`) must re-export or reuse these, never
//! redefine them.
use std::{collections::HashSet, ops::Range, time::Duration};

use iced::{mouse, time::Instant, Point, Rectangle, Size, Transformation, Vector};

use crate::widget::graph_view::{AnchorSide, HandleSlot, ItemId, Lane, Shape, Side, Target};

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

pub const MIN_ZOOM: f32 = 0.02;
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
/// Accumulated wheel delta that cycles the tag highlight by one step.
pub const TAG_WHEEL_STEP: f32 = 40.0;
pub const FOCUS_DURATION: Duration = Duration::from_millis(520);

pub const LANE_BAND_OPACITY: f32 = 0.5;
pub const LANE_SEPARATOR_WIDTH: f32 = 1.0;
/// Lane color strip along the left edge of its band, in screen px.
pub const LANE_STRIP_WIDTH: f32 = 3.0;
/// Screen px each side of a lane bottom edge that grab it for a resize.
pub const LANE_EDGE_GRAB: f32 = 3.0;
pub const SPACE_LINE_WIDTH: f32 = 2.0;
pub const HANDLE_COLUMN_WIDTH: f32 = 180.0;
/// Horizontal space between the handles and the column edges.
pub const HANDLE_INSET: f32 = 8.0;
pub const HANDLE_WIDTH: f32 = HANDLE_COLUMN_WIDTH - 2.0 * HANDLE_INSET;
pub const HANDLE_HEIGHT: f32 = 28.0;
pub const HANDLE_GAP: f32 = 4.0;
/// Space between a handle stack and the top or bottom of the view.
pub const HANDLE_MARGIN: f32 = 8.0;
pub const HANDLE_RADIUS: f32 = 6.0;
pub const HANDLE_GRIP_WIDTH: f32 = 22.0;
pub const HANDLE_GRIP_SIZE: f32 = 14.0;
pub const HANDLE_DOT_RADIUS: f32 = 4.0;
pub const HANDLE_NAME_X: f32 = 38.0;
/// About the width left between the color dot and the checkbox at the small caption size.
pub const HANDLE_NAME_MAX_CHARS: usize = 14;
/// Right part of a handle that toggles it on click.
pub const HANDLE_CHECKBOX_AREA: f32 = 30.0;
pub const HANDLE_CHECKBOX_SIZE: f32 = 14.0;
pub const HANDLE_DROP_LINE_WIDTH: f32 = 2.0;

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

    /// Same zoom centered on `target`, `None` when `target` is already fully in view.
    pub fn pan_to(self, target: Rectangle, size: Size) -> Option<Camera> {
        let visible = Rectangle::new(self.to_graph(Point::ORIGIN), size * (1.0 / self.zoom));
        (!target.is_within(&visible)).then(|| Camera::centered_on(target.center(), self.zoom, size))
    }
}

/// Display row a slot dragged to `offset_y` (top, from the column top) lands on.
pub fn reorder_target(offset_y: f32, rows: usize) -> usize {
    ((offset_y / SLOT_HEIGHT).round().max(0.0) as usize).min(rows.saturating_sub(1))
}

/// Live row of the slot at `row` while the slot at `from` is dragged to
/// `offset_y` and would land on `to`.
pub fn reordered_row(row: usize, from: usize, to: usize, offset_y: f32) -> f32 {
    if row == from {
        offset_y / SLOT_HEIGHT
    } else if from < to && row > from && row <= to {
        row as f32 - 1.0
    } else if to < from && row >= to && row < from {
        row as f32 + 1.0
    } else {
        row as f32
    }
}

/// Adds `delta` to `accumulated`; returns the remainder and the whole steps.
pub fn accumulate_wheel(accumulated: f32, delta: f32) -> (f32, i32) {
    let total = accumulated + delta;
    let steps = (total / TAG_WHEEL_STEP).trunc() as i32;
    (total - steps as f32 * TAG_WHEEL_STEP, steps)
}

/// Interpolates the graph point at the widget center and the zoom, which keeps
/// the motion straight on screen.
pub fn interpolate_camera(from: Camera, to: Camera, t: f32, size: Size) -> Camera {
    let center = Point::new(size.width / 2.0, size.height / 2.0);
    let (a, b) = (from.to_graph(center), to.to_graph(center));
    let lerp = |x: f32, y: f32| x + (y - x) * t;
    Camera::centered_on(
        Point::new(lerp(a.x, b.x), lerp(a.y, b.y)),
        lerp(from.zoom, to.zoom),
        size,
    )
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

/// Vertical move allowed to items dragged by `dy`: each item in `spans` stays
/// inside its lane band. An item already outside its band may stay where it is
/// or move toward the band, never further away.
pub fn clamp_to_lanes<'a>(dy: f32, spans: impl IntoIterator<Item = (Rectangle, &'a Lane)>) -> f32 {
    let (mut lowest, mut highest) = (f32::NEG_INFINITY, f32::INFINITY);
    for (item, lane) in spans {
        lowest = lowest.max((lane.top - item.y).min(0.0));
        highest = highest.min((lane.top + lane.height - item.y - item.height).max(0.0));
    }
    dy.clamp(lowest, highest)
}

/// Where a dragged lane lands, graph px.
#[derive(Debug, Clone, PartialEq)]
pub struct LaneDrop {
    /// Lane it goes before, `None` for after all of them.
    pub before: Option<u64>,
    /// Drop indicator, `None` when the lane stays in its slot.
    pub line: Option<f32>,
}

/// Drop of the lane `id` of `lanes` (stacked in order) with the pointer at `y`.
pub fn lane_drop(lanes: &[Lane], id: u64, y: f32) -> LaneDrop {
    let spans: Vec<(u64, Range<f32>)> = lanes
        .iter()
        .map(|lane| (lane.id, lane.top..lane.top + lane.height))
        .collect();
    span_drop(&spans, id, y)
}

/// Drop of `id` among `spans` (stacked in order) with the pointer at `y`: it takes the slot
/// of the span under the pointer, the first one above them all, the last one below. One not
/// among them goes before the span under the pointer, after them all below.
fn span_drop(spans: &[(u64, Range<f32>)], id: u64, y: f32) -> LaneDrop {
    let from = spans.iter().position(|(other, _)| *other == id);
    let above = spans.iter().filter(|(_, span)| span.end <= y).count();
    let under = above.min(spans.len().saturating_sub(1));
    let after = |index: usize| spans.get(index + 1).map(|(next, _)| *next);
    match from {
        Some(from) if under > from => LaneDrop {
            before: after(under),
            line: Some(spans[under].1.end),
        },
        Some(from) if under < from => LaneDrop {
            before: Some(spans[under].0),
            line: Some(spans[under].1.start),
        },
        Some(from) => LaneDrop {
            before: after(from),
            line: None,
        },
        None => match spans.get(above) {
            Some((next, span)) => LaneDrop {
                before: Some(*next),
                line: Some(span.start),
            },
            None => LaneDrop {
                before: None,
                line: spans.last().map(|(_, span)| span.end),
            },
        },
    }
}

/// Lane whose bottom edge is within `LANE_EDGE_GRAB` of `y` (widget px).
pub fn lane_edge(lanes: &[Lane], camera: Camera, y: f32) -> Option<u64> {
    lanes
        .iter()
        .find(|lane| {
            let bottom = camera.to_screen(Point::new(0.0, lane.top + lane.height)).y;
            (y - bottom).abs() <= LANE_EDGE_GRAB
        })
        .map(|lane| lane.id)
}

/// Height of a lane `height` tall resized by `dy`, never below `min`.
pub fn resized_height(height: f32, dy: f32, min: f32) -> f32 {
    (height + dy).max(min)
}

/// `lanes` with the lane `id` `height` tall, the lanes below it moved along.
pub fn resized_lanes(lanes: &[Lane], id: u64, height: f32) -> Vec<Lane> {
    let mut shift = 0.0;
    lanes
        .iter()
        .map(|lane| {
            let mut lane = lane.clone();
            lane.top += shift;
            if lane.id == id {
                shift = height - lane.height;
                lane.height = height;
            }
            lane
        })
        .collect()
}

/// Room a space removal at `x` has: the gap between `x` and the right edge of the items
/// starting left of it, `None` without such items.
pub fn space_room(rects: impl IntoIterator<Item = Rectangle>, x: f32) -> Option<f32> {
    rects
        .into_iter()
        .filter(|rect| rect.x < x)
        .map(|rect| (x - rect.x - rect.width).max(0.0))
        .reduce(f32::min)
}

/// Shift of a space drag by `dx`: a removal takes at most `room`. With `snap`, on the grid
/// without going past `room`.
pub fn space_shift(dx: f32, room: Option<f32>, snap: bool) -> f32 {
    let dx = room.map_or(dx, |room| dx.max(-room));
    if !snap {
        return dx;
    }
    let snapped = (dx / U).round() * U;
    if room.is_some_and(|room| snapped < -room) {
        snapped + U
    } else {
        snapped
    }
}

/// Items a space shift at `x` moves: the ones whose left edge is at or right of it.
pub fn space_items(rects: impl IntoIterator<Item = (ItemId, Rectangle)>, x: f32) -> Vec<ItemId> {
    rects
        .into_iter()
        .filter(|(_, rect)| rect.x >= x)
        .map(|(id, _)| id)
        .collect()
}

/// Top of the handle of a band spanning `top` to `bottom` (screen px): the band
/// top, held at the view top while the band is scrolled past it.
pub fn lane_handle_top(top: f32, bottom: f32) -> f32 {
    top.max(0.0).min(bottom - HANDLE_HEIGHT).max(top)
}

/// Rect of a handle whose top is at `top`, in widget px.
pub fn handle_rect(top: f32) -> Rectangle {
    Rectangle::new(
        Point::new(HANDLE_INSET, top),
        Size::new(HANDLE_WIDTH, HANDLE_HEIGHT),
    )
}

/// Tops of `count` handles stacked down from `top`.
pub fn handle_stack(top: f32, count: usize) -> Vec<f32> {
    (0..count)
        .map(|index| top + index as f32 * (HANDLE_HEIGHT + HANDLE_GAP))
        .collect()
}

/// Top of a stack of `count` handles resting on the bottom of a view `height` px tall.
pub fn bottom_stack_top(height: f32, count: usize) -> f32 {
    height - HANDLE_MARGIN - count as f32 * (HANDLE_HEIGHT + HANDLE_GAP) + HANDLE_GAP
}

/// Index a handle dropped with its center at `y` takes among the handles of
/// its group: the number of the other ones whose center is above it.
pub fn handle_drop_index(y: f32, others: impl IntoIterator<Item = f32>) -> usize {
    others.into_iter().filter(|center| *center < y).count()
}

/// Where a dragged handle lands, in widget px.
#[derive(Debug, Clone, PartialEq)]
pub struct HandleDrop {
    /// Handle it goes before, `None` for after all of them.
    pub before: Option<u64>,
    /// Drop indicator, `None` when the handle stays on its lane or the group
    /// has no other handle.
    pub line: Option<f32>,
}

/// Drop of the handle `id`, its top at `top` and the pointer at `pointer`. The
/// group under the pointer is the bottom stack from its top edge down, the
/// lanes above it. Over the lanes the handle takes the slot of the lane under
/// the pointer, in the stack it goes among the other handles.
pub fn handle_drop(slots: &[HandleSlot], id: u64, top: f32, pointer: f32) -> HandleDrop {
    let stack_top = slots
        .iter()
        .filter(|slot| slot.lane.is_none())
        .map(|slot| slot.rect.y)
        .reduce(f32::min);
    let on_stack = match stack_top {
        Some(stack_top) => {
            slots.iter().all(|slot| slot.lane.is_none()) || pointer >= stack_top - HANDLE_GAP / 2.0
        }
        None => false,
    };
    if !on_stack {
        let mut spans: Vec<(u64, Range<f32>)> = slots
            .iter()
            .filter_map(|slot| Some((slot.id, slot.lane.clone()?)))
            .collect();
        spans.sort_by(|a, b| a.1.start.total_cmp(&b.1.start));
        let drop = span_drop(&spans, id, pointer);
        return HandleDrop {
            before: drop.before,
            line: drop.line,
        };
    }
    let mut others: Vec<&HandleSlot> = slots
        .iter()
        .filter(|slot| slot.lane.is_none() && slot.id != id)
        .collect();
    others.sort_by(|a, b| a.rect.y.total_cmp(&b.rect.y));
    let center = |top: f32| top + HANDLE_HEIGHT / 2.0;
    let index = handle_drop_index(center(top), others.iter().map(|slot| center(slot.rect.y)));
    let line = match others.get(index) {
        Some(next) => Some(next.rect.y - HANDLE_GAP / 2.0),
        None => others
            .last()
            .map(|last| last.rect.y + HANDLE_HEIGHT + HANDLE_GAP / 2.0),
    };
    HandleDrop {
        before: others.get(index).map(|slot| slot.id),
        line,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandlePart {
    Grip,
    Body,
    Checkbox,
}

/// Part of a handle at `x` px from its left edge.
pub fn handle_part(x: f32) -> HandlePart {
    if x < HANDLE_GRIP_WIDTH {
        HandlePart::Grip
    } else if x >= HANDLE_WIDTH - HANDLE_CHECKBOX_AREA {
        HandlePart::Checkbox
    } else {
        HandlePart::Body
    }
}

#[derive(Debug)]
pub struct ClickTracker<T = Target> {
    last: Option<(T, Instant)>,
}

impl<T> Default for ClickTracker<T> {
    fn default() -> Self {
        Self { last: None }
    }
}

impl<T: Copy + PartialEq> ClickTracker<T> {
    /// True when the previous click was on the same target within
    /// `DOUBLE_CLICK`. A detected double click starts over.
    pub fn register(&mut self, target: T, at: Instant) -> bool {
        let double = self
            .last
            .is_some_and(|(last, time)| last == target && at.duration_since(time) <= DOUBLE_CLICK);
        self.last = if double { None } else { Some((target, at)) };
        double
    }
}

#[cfg(test)]
mod tests {
    use iced::{animation::Easing, Animation};

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
    fn pan_to_keeps_the_zoom() {
        let size = Size::new(900.0, 500.0);
        let camera = Camera::centered_on(Point::ORIGIN, 0.5, size);
        let inside = Rectangle::new(Point::new(-100.0, -100.0), Size::new(200.0, 100.0));
        assert_eq!(camera.pan_to(inside, size), None);

        let across = Rectangle::new(Point::new(800.0, 0.0), Size::new(200.0, 100.0));
        let panned = camera.pan_to(across, size).unwrap();
        assert_eq!(panned.zoom, 0.5);
        assert!(close_point(
            panned.to_screen(across.center()),
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
        assert_eq!(grid_step(MIN_ZOOM), 768.0);
        assert!(grid_step(MIN_ZOOM) * MIN_ZOOM < 2.0 * GRID_MIN_SCREEN_STEP);
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
    fn reorder_target_clamps() {
        assert_eq!(reorder_target(0.0, 5), 0);
        assert_eq!(reorder_target(70.0, 5), 1);
        assert_eq!(reorder_target(1000.0, 5), 4);
        assert_eq!(reorder_target(-10.0, 5), 0);
    }

    #[test]
    fn reordered_rows_follow() {
        assert!(close(reordered_row(1, 1, 3, 130.0), 130.0 / 48.0));
        assert_eq!(reordered_row(2, 1, 3, 130.0), 1.0);
        assert_eq!(reordered_row(3, 1, 3, 130.0), 2.0);
        assert_eq!(reordered_row(4, 1, 3, 130.0), 4.0);
        assert_eq!(reordered_row(0, 1, 3, 130.0), 0.0);
        assert_eq!(reordered_row(1, 3, 1, 60.0), 2.0);
        assert_eq!(reordered_row(2, 3, 1, 60.0), 3.0);
    }

    #[test]
    fn wheel_accumulates() {
        assert_eq!(accumulate_wheel(0.0, 30.0), (30.0, 0));
        assert_eq!(accumulate_wheel(30.0, 15.0), (5.0, 1));
        assert_eq!(accumulate_wheel(0.0, -85.0), (-5.0, -2));
    }

    #[test]
    fn camera_interpolation() {
        let size = Size::new(900.0, 500.0);
        let from = Camera {
            offset: Vector::new(30.0, -20.0),
            zoom: 0.5,
        };
        let to = Camera::centered_on(Point::new(400.0, 300.0), 1.5, size);
        let same = |a: Camera, b: Camera| {
            close(a.zoom, b.zoom) && close(a.offset.x, b.offset.x) && close(a.offset.y, b.offset.y)
        };
        assert!(same(interpolate_camera(from, to, 0.0, size), from));
        assert!(same(interpolate_camera(from, to, 1.0, size), to));
        let center = Point::new(450.0, 250.0);
        let mid = interpolate_camera(from, to, 0.5, size);
        assert!(close(mid.zoom, 1.0));
        let (a, b) = (from.to_graph(center), to.to_graph(center));
        assert!(close_point(
            mid.to_graph(center),
            Point::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0)
        ));
    }

    #[test]
    fn focus_easing() {
        let start = Instant::now();
        let animation = Animation::new(false)
            .easing(Easing::EaseInOutCubic)
            .duration(FOCUS_DURATION)
            .go(true, start);
        let mid = animation.interpolate(0.0, 1.0, start + Duration::from_millis(260));
        assert!((mid - 0.5).abs() < 0.05);
        assert!(!animation.is_animating(start + Duration::from_millis(521)));
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

    fn lane(top: f32, height: f32) -> Lane {
        Lane {
            id: 0,
            top,
            height,
            min_height: 0.0,
            items: Vec::new(),
            color: iced::Color::BLACK,
        }
    }

    /// Lanes 0, 1 and 2, 400, 300 and 200 px tall stacked from 0.
    fn three_lanes() -> Vec<Lane> {
        vec![
            lane(0.0, 400.0),
            Lane {
                id: 1,
                ..lane(400.0, 300.0)
            },
            Lane {
                id: 2,
                ..lane(700.0, 200.0)
            },
        ]
    }

    #[test]
    fn lane_drops_in_the_slot_under_the_pointer() {
        let lanes = three_lanes();
        let drop = |id, y| lane_drop(&lanes, id, y);
        assert_eq!(
            drop(0, 500.0),
            LaneDrop {
                before: Some(2),
                line: Some(700.0),
            }
        );
        assert_eq!(
            drop(0, 800.0),
            LaneDrop {
                before: None,
                line: Some(900.0),
            }
        );
        assert_eq!(
            drop(2, 100.0),
            LaneDrop {
                before: Some(0),
                line: Some(0.0),
            }
        );
        assert_eq!(
            drop(2, 450.0),
            LaneDrop {
                before: Some(1),
                line: Some(400.0),
            }
        );
        // Above or below every lane, the first or the last slot.
        assert_eq!(drop(1, -300.0).before, Some(0));
        assert_eq!(drop(1, 5000.0).before, None);
        assert_eq!(drop(0, 5000.0).line, Some(900.0));
        // In its own slot, the lane stays.
        assert_eq!(
            drop(1, 650.0),
            LaneDrop {
                before: Some(2),
                line: None,
            }
        );
        assert_eq!(
            drop(2, 899.0),
            LaneDrop {
                before: None,
                line: None,
            }
        );
        // A lane not among them goes before the one under the pointer.
        assert_eq!(
            drop(7, 100.0),
            LaneDrop {
                before: Some(0),
                line: Some(0.0),
            }
        );
        assert_eq!(
            drop(7, 5000.0),
            LaneDrop {
                before: None,
                line: Some(900.0),
            }
        );
    }

    #[test]
    fn lane_edges_grab_near_the_bottom() {
        let lanes = three_lanes();
        let camera = Camera {
            offset: Vector::new(0.0, 10.0),
            zoom: 0.5,
        };
        assert_eq!(lane_edge(&lanes, camera, 210.0), Some(0));
        assert_eq!(lane_edge(&lanes, camera, 207.0), Some(0));
        assert_eq!(lane_edge(&lanes, camera, 213.0), Some(0));
        assert_eq!(lane_edge(&lanes, camera, 214.0), None);
        assert_eq!(lane_edge(&lanes, camera, 361.0), Some(1));
        assert_eq!(lane_edge(&lanes, camera, 460.0), Some(2));
        assert_eq!(lane_edge(&lanes, camera, 300.0), None);
    }

    #[test]
    fn space_removal_stops_at_the_items_left() {
        let rect = |x: f32, width: f32| Rectangle::new(Point::new(x, 0.0), Size::new(width, 36.0));
        let rects = [rect(0.0, 696.0), rect(1000.0, 216.0), rect(2000.0, 696.0)];
        assert_eq!(space_room(rects, 1500.0), Some(284.0));
        assert_eq!(space_room(rects, 900.0), Some(204.0));
        // An item across the line leaves no room.
        assert_eq!(space_room(rects, 1100.0), Some(0.0));
        assert_eq!(space_room(rects, 0.0), None);

        assert_eq!(space_shift(-100.0, Some(284.0), false), -100.0);
        assert_eq!(space_shift(-400.0, Some(284.0), false), -284.0);
        assert_eq!(space_shift(-400.0, None, false), -400.0);
        assert_eq!(space_shift(500.0, Some(0.0), false), 500.0);
        assert_eq!(space_shift(-400.0, Some(284.0), true), -276.0);
        assert_eq!(space_shift(-100.0, Some(284.0), true), -96.0);
        assert_eq!(space_shift(-5.0, Some(0.0), true), 0.0);
        assert_eq!(space_shift(130.0, None, true), 132.0);
    }

    #[test]
    fn space_moves_the_items_from_its_line() {
        let rect = |x: f32| Rectangle::new(Point::new(x, 0.0), Size::new(216.0, 36.0));
        let items = [
            (ItemId(0), rect(0.0)),
            (ItemId(1), rect(1400.0)),
            (ItemId(2), rect(1500.0)),
            (ItemId(3), rect(3000.0)),
        ];
        assert_eq!(space_items(items, 1500.0), vec![ItemId(2), ItemId(3)]);
        assert_eq!(
            space_items(items, 1300.0),
            vec![ItemId(1), ItemId(2), ItemId(3)]
        );
        assert_eq!(space_items(items, 5000.0), Vec::<ItemId>::new());
    }

    #[test]
    fn lane_resize_keeps_its_content() {
        assert_eq!(resized_height(300.0, 120.0, 200.0), 420.0);
        assert_eq!(resized_height(300.0, -60.0, 200.0), 240.0);
        assert_eq!(resized_height(300.0, -250.0, 200.0), 200.0);
        assert_eq!(resized_height(300.0, 10_000.0, 200.0), 10_300.0);
    }

    #[test]
    fn resized_lane_moves_the_lanes_below() {
        let resized = resized_lanes(&three_lanes(), 1, 450.0);
        let spans: Vec<(f32, f32)> = resized.iter().map(|lane| (lane.top, lane.height)).collect();
        assert_eq!(spans, vec![(0.0, 400.0), (400.0, 450.0), (850.0, 200.0)]);
        assert_eq!(resized_lanes(&three_lanes(), 9, 450.0), three_lanes());
    }

    #[test]
    fn drag_clamped_to_lanes() {
        let top = lane(0.0, 400.0);
        let bottom = lane(400.0, 300.0);
        let block = |y| Rectangle::new(Point::new(0.0, y), Size::new(696.0, 156.0));

        assert_eq!(clamp_to_lanes(50.0, [(block(100.0), &top)]), 50.0);
        assert_eq!(clamp_to_lanes(500.0, [(block(100.0), &top)]), 144.0);
        assert_eq!(clamp_to_lanes(-500.0, [(block(100.0), &top)]), -100.0);
        assert_eq!(
            clamp_to_lanes(100.0, [(block(100.0), &top), (block(500.0), &bottom)]),
            44.0
        );
        assert_eq!(
            clamp_to_lanes(-200.0, [(block(100.0), &top), (block(500.0), &bottom)]),
            -100.0
        );
        assert_eq!(clamp_to_lanes(37.0, []), 37.0);
    }

    #[test]
    fn drag_outside_lane_moves_back_only() {
        let band = lane(400.0, 300.0);
        let above = Rectangle::new(Point::new(0.0, 100.0), Size::new(216.0, 36.0));
        assert_eq!(clamp_to_lanes(-50.0, [(above, &band)]), 0.0);
        assert_eq!(clamp_to_lanes(350.0, [(above, &band)]), 350.0);
        assert_eq!(clamp_to_lanes(1000.0, [(above, &band)]), 564.0);
        let taller = lane(0.0, 100.0);
        let block = Rectangle::new(Point::ORIGIN, Size::new(696.0, 156.0));
        assert_eq!(clamp_to_lanes(30.0, [(block, &taller)]), 0.0);
        assert_eq!(clamp_to_lanes(-30.0, [(block, &taller)]), 0.0);
    }

    #[test]
    fn lane_handle_sticks_to_view_top() {
        assert_eq!(lane_handle_top(120.0, 400.0), 120.0);
        assert_eq!(lane_handle_top(-50.0, 400.0), 0.0);
        assert_eq!(lane_handle_top(-300.0, 10.0), -18.0);
        assert_eq!(lane_handle_top(100.0, 110.0), 100.0);
    }

    #[test]
    fn handle_stacks() {
        assert_eq!(handle_stack(8.0, 3), vec![8.0, 40.0, 72.0]);
        assert!(handle_stack(8.0, 0).is_empty());
        let top = bottom_stack_top(600.0, 2);
        assert_eq!(top, 532.0);
        assert_eq!(handle_stack(top, 2), vec![532.0, 564.0]);
        assert_eq!(564.0 + HANDLE_HEIGHT, 600.0 - HANDLE_MARGIN);
    }

    #[test]
    fn handle_drop_indices() {
        let others = [14.0, 214.0, 414.0];
        assert_eq!(handle_drop_index(0.0, others), 0);
        assert_eq!(handle_drop_index(100.0, others), 1);
        assert_eq!(handle_drop_index(300.0, others), 2);
        assert_eq!(handle_drop_index(900.0, others), 3);
        assert_eq!(handle_drop_index(50.0, []), 0);
    }

    fn slot(id: u64, top: f32) -> HandleSlot {
        HandleSlot {
            index: id as usize,
            id,
            rect: handle_rect(top),
            lane: None,
        }
    }

    /// Handle `id` on a lane spanning `top` to `bottom`.
    fn lane_slot(id: u64, top: f32, bottom: f32) -> HandleSlot {
        HandleSlot {
            lane: Some(top..bottom),
            ..slot(id, top)
        }
    }

    #[test]
    fn handle_drops_in_the_lane_under_the_pointer() {
        let slots = [
            lane_slot(0, 10.0, 170.0),
            lane_slot(1, 170.0, 330.0),
            lane_slot(2, 330.0, 490.0),
            slot(3, 532.0),
            slot(4, 564.0),
        ];
        let drop = |id, top: f32| handle_drop(&slots, id, top, top + 10.0);
        // Up by one, into the body of the lane above.
        assert_eq!(
            drop(2, 200.0),
            HandleDrop {
                before: Some(1),
                line: Some(170.0),
            }
        );
        // Down by one, into the body of the lane below.
        assert_eq!(
            drop(0, 200.0),
            HandleDrop {
                before: Some(2),
                line: Some(330.0),
            }
        );
        // To the top and to the bottom.
        assert_eq!(
            drop(2, 50.0),
            HandleDrop {
                before: Some(0),
                line: Some(10.0),
            }
        );
        assert_eq!(
            drop(0, 400.0),
            HandleDrop {
                before: None,
                line: Some(490.0),
            }
        );
        assert_eq!(drop(1, -100.0).before, Some(0));
        // On its own lane, the drop is its start target.
        assert_eq!(
            drop(1, 250.0),
            HandleDrop {
                before: Some(2),
                line: None,
            }
        );
        assert_eq!(
            drop(2, 400.0),
            HandleDrop {
                before: None,
                line: None,
            }
        );
        // A hidden handle dropped over the lanes goes before the lane under the pointer.
        assert_eq!(
            drop(3, 200.0),
            HandleDrop {
                before: Some(1),
                line: Some(170.0),
            }
        );
        assert_eq!(
            drop(3, 490.0),
            HandleDrop {
                before: None,
                line: Some(490.0),
            }
        );
    }

    #[test]
    fn handle_drops_in_the_bottom_stack() {
        let slots = [
            lane_slot(0, 10.0, 400.0),
            slot(1, 500.0),
            slot(2, 532.0),
            slot(3, 564.0),
        ];
        let drop = |id, top: f32| handle_drop(&slots, id, top, top + 10.0);
        assert_eq!(
            drop(3, 495.0),
            HandleDrop {
                before: Some(1),
                line: Some(498.0),
            }
        );
        assert_eq!(
            drop(1, 560.0),
            HandleDrop {
                before: Some(3),
                line: Some(562.0),
            }
        );
        assert_eq!(drop(1, 600.0).before, None);
        // A lane handle dropped on the stack goes among the hidden ones.
        assert_eq!(drop(0, 520.0).before, Some(2));
        // Above the stack top edge, the pointer is over the lanes group.
        assert_eq!(
            drop(2, 470.0),
            HandleDrop {
                before: None,
                line: Some(400.0),
            }
        );
    }

    #[test]
    fn handle_drops_without_lanes() {
        let slots = [slot(0, 8.0), slot(1, 40.0), slot(2, 72.0)];
        assert_eq!(handle_drop(&slots, 2, 0.0, 10.0).before, Some(0));
        assert_eq!(handle_drop(&slots, 0, 100.0, 110.0).before, None);
        assert_eq!(handle_drop(&[slot(0, 8.0)], 0, 50.0, 60.0).line, None);
    }

    #[test]
    fn handle_parts() {
        assert_eq!(handle_part(4.0), HandlePart::Grip);
        assert_eq!(handle_part(60.0), HandlePart::Body);
        assert_eq!(handle_part(140.0), HandlePart::Checkbox);
        assert_eq!(handle_part(163.0), HandlePart::Checkbox);
    }
}
