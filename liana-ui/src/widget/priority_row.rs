//! A row that narrows or hides its least important children, by measured width, when it lacks
//! room. See [`crate::widget::priority`].

use std::cmp::Reverse;

use iced::{
    advanced::{
        layout::{Limits, Node},
        mouse, overlay, renderer,
        widget::{tree, Operation, Tree, Widget},
        Clipboard, Layout, Shell,
    },
    alignment::Vertical,
    Alignment, Event, Length, Padding, Pixels, Point, Rectangle, Size, Vector,
};

use crate::{
    theme::Theme,
    widget::{
        priority::{Mode, PriorityState},
        Element,
    },
};

type Renderer = iced::Renderer;

/// Creates a [`crate::widget::PriorityRow`] with the given children, like iced's `row!`.
#[macro_export]
macro_rules! priority_row {
    () => (
        $crate::widget::PriorityRow::new()
    );
    ($($x:expr),+ $(,)?) => (
        $crate::widget::PriorityRow::with_children([$($crate::widget::Element::from($x)),+])
    );
}

pub struct PriorityRow<'a, Message> {
    spacing: f32,
    padding: Padding,
    width: Length,
    height: Length,
    align: Alignment,
    children: Vec<Element<'a, Message>>,
}

#[derive(Debug, Default)]
struct State {
    hidden: Vec<bool>,
}

/// What a child needs to be laid out, as measured before allocating the row width.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Slot {
    natural: f32,
    min: f32,
    /// `None` for a child that is never degraded.
    priority: Option<PriorityState>,
}

impl<'a, Message> PriorityRow<'a, Message> {
    pub fn new() -> Self {
        Self {
            spacing: 0.0,
            padding: Padding::ZERO,
            width: Length::Shrink,
            height: Length::Shrink,
            align: Alignment::Start,
            children: Vec::new(),
        }
    }

    pub fn with_children(children: impl IntoIterator<Item = Element<'a, Message>>) -> Self {
        Self::new().extend(children)
    }

    pub fn spacing(mut self, amount: impl Into<Pixels>) -> Self {
        self.spacing = amount.into().0;
        self
    }

    pub fn padding(mut self, padding: impl Into<Padding>) -> Self {
        self.padding = padding.into();
        self
    }

    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }

    pub fn align_y(mut self, align: impl Into<Vertical>) -> Self {
        self.align = Alignment::from(align.into());
        self
    }

    pub fn push(mut self, child: impl Into<Element<'a, Message>>) -> Self {
        let child = child.into();
        let child_size = child.as_widget().size_hint();

        if !child_size.is_void() {
            self.width = self.width.enclose(child_size.width);
            self.height = self.height.enclose(child_size.height);
            self.children.push(child);
        }

        self
    }

    pub fn extend(self, children: impl IntoIterator<Item = Element<'a, Message>>) -> Self {
        children.into_iter().fold(self, Self::push)
    }
}

impl<Message> PriorityRow<'_, Message> {
    /// Lays out visible children at their granted width, fill children share what is left.
    fn layout_granted(
        &mut self,
        trees: &mut [Tree],
        renderer: &Renderer,
        limits: &Limits,
        granted: &[Option<f32>],
    ) -> Vec<Node> {
        let fill_sum: u16 = self
            .children
            .iter()
            .map(|child| fill_factor(child, limits))
            .sum();
        let mut nodes: Vec<Node> = self
            .children
            .iter_mut()
            .zip(trees.iter_mut())
            .zip(granted)
            .map(|((child, tree), granted)| match granted {
                Some(width) if fill_factor(child, limits) == 0 => {
                    child
                        .as_widget_mut()
                        .layout(tree, renderer, &child_limits(limits, 0.0, *width))
                }
                _ => Node::new(Size::ZERO),
            })
            .collect();
        // A cut text can end up narrower than granted, fill children take the actual leftover.
        let laid: Vec<Option<f32>> = nodes
            .iter()
            .zip(granted)
            .map(|(node, granted)| granted.map(|_| node.size().width))
            .collect();
        let remaining = (limits.max().width - used_width(&laid, self.spacing)).max(0.0);
        for ((child, tree), (node, granted)) in self
            .children
            .iter_mut()
            .zip(trees)
            .zip(nodes.iter_mut().zip(granted))
        {
            let fill = fill_factor(child, limits);
            if granted.is_some() && fill > 0 {
                let width = remaining * fill as f32 / fill_sum as f32;
                let min = if width.is_finite() { width } else { 0.0 };
                *node =
                    child
                        .as_widget_mut()
                        .layout(tree, renderer, &child_limits(limits, min, width));
            }
        }
        nodes
    }

    /// Places visible nodes left to right, returns the width they take.
    fn place(&self, nodes: &mut [Node], granted: &[Option<f32>], cross: f32) -> f32 {
        let mut x = self.padding.left;
        let visible = nodes
            .iter_mut()
            .zip(granted)
            .filter_map(|(node, granted)| granted.map(|_| node));
        for (i, node) in visible.enumerate() {
            if i > 0 {
                x += self.spacing;
            }
            node.move_to_mut(Point::new(x, self.padding.top));
            node.align_mut(Alignment::Start, self.align, Size::new(0.0, cross));
            x += node.size().width;
        }
        x - self.padding.left
    }
}

impl<Message> Default for PriorityRow<'_, Message> {
    fn default() -> Self {
        Self::new()
    }
}

/// Width granted to each child, `None` when hidden.
///
/// When the naturals do not fit, prioritized children give way one at a time, highest rank
/// first and rightmost first among equal ranks, until the row fits: a truncated child shrinks by
/// the overflow if that keeps it at or above its minimum, otherwise it is hidden like a hidden
/// child. A hidden child also gives back its spacing.
fn allocate(slots: &[Slot], available: f32, spacing: f32) -> Vec<Option<f32>> {
    let mut granted: Vec<Option<f32>> = slots.iter().map(|slot| Some(slot.natural)).collect();
    let mut degradable: Vec<(usize, PriorityState)> = slots
        .iter()
        .enumerate()
        .filter_map(|(i, slot)| slot.priority.map(|priority| (i, priority)))
        .collect();
    degradable.sort_by_key(|(i, priority)| Reverse((priority.rank, *i)));

    for (i, priority) in degradable {
        let overflow = used_width(&granted, spacing) - available;
        if overflow <= 0.0 {
            break;
        }
        let narrowed = slots[i].natural - overflow;
        granted[i] = match priority.mode {
            Mode::Truncate if narrowed >= slots[i].min => Some(narrowed),
            Mode::Truncate | Mode::Hide => None,
        };
    }
    granted
}

fn used_width(granted: &[Option<f32>], spacing: f32) -> f32 {
    let visible: Vec<f32> = granted.iter().flatten().copied().collect();
    visible.iter().sum::<f32>() + spacing * visible.len().saturating_sub(1) as f32
}

fn priority_of<Message>(child: &Element<'_, Message>) -> Option<PriorityState> {
    let widget = child.as_widget();
    (widget.tag() == tree::Tag::of::<PriorityState>())
        .then(|| *widget.state().downcast_ref::<PriorityState>())
}

/// Fill factor of a child on the main axis, zero when the row compresses fill children.
fn fill_factor<Message>(child: &Element<'_, Message>, limits: &Limits) -> u16 {
    if limits.compression().width {
        0
    } else {
        child.as_widget().size().width.fill_factor()
    }
}

fn child_limits(limits: &Limits, min_width: f32, max_width: f32) -> Limits {
    Limits::with_compression(
        Size::new(min_width, 0.0),
        Size::new(max_width, limits.max().height),
        Size::new(limits.compression().width, false),
    )
}

fn measure<Message>(
    child: &mut Element<'_, Message>,
    tree: &mut Tree,
    renderer: &Renderer,
    limits: &Limits,
) -> Slot {
    if fill_factor(child, limits) > 0 {
        return Slot {
            natural: 0.0,
            min: 0.0,
            priority: None,
        };
    }
    let priority = priority_of(child);
    let mut width_within = |max_width| {
        child
            .as_widget_mut()
            .layout(tree, renderer, &child_limits(limits, 0.0, max_width))
            .size()
            .width
    };
    let natural = width_within(f32::INFINITY);
    let min = match priority {
        Some(PriorityState {
            mode: Mode::Truncate,
            ..
        }) => width_within(0.0),
        _ => natural,
    };
    Slot {
        natural,
        min,
        priority,
    }
}

impl<'a, Message> Widget<Message, Theme, Renderer> for PriorityRow<'a, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn children(&self) -> Vec<Tree> {
        self.children.iter().map(Tree::new).collect()
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(&self.children);
    }

    fn size(&self) -> Size<Length> {
        Size::new(self.width, self.height)
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &Limits) -> Node {
        let limits = limits
            .width(self.width)
            .height(self.height)
            .shrink(self.padding);
        let slots: Vec<Slot> = self
            .children
            .iter_mut()
            .zip(&mut tree.children)
            .map(|(child, tree)| measure(child, tree, renderer, &limits))
            .collect();
        let granted = allocate(&slots, limits.max().width, self.spacing);

        let mut nodes = self.layout_granted(&mut tree.children, renderer, &limits, &granted);
        let cross = nodes.iter().map(|node| node.size().height).fold(
            if limits.compression().height {
                0.0
            } else {
                limits.max().height
            },
            f32::max,
        );
        let content_width = self.place(&mut nodes, &granted, cross);

        tree.state.downcast_mut::<State>().hidden = granted.iter().map(Option::is_none).collect();

        let intrinsic = Size::new(content_width, cross);
        let size = limits.resolve(self.width, self.height, intrinsic);
        Node::with_children(size.expand(self.padding), nodes)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        let Tree {
            state, children, ..
        } = tree;
        let hidden = &state.downcast_ref::<State>().hidden;
        operation.container(None, layout.bounds());
        operation.traverse(&mut |operation| {
            self.children
                .iter_mut()
                .zip(children.iter_mut())
                .zip(layout.children())
                .zip(hidden)
                .filter(|(_, hidden)| !**hidden)
                .for_each(|(((child, tree), layout), _)| {
                    child
                        .as_widget_mut()
                        .operate(tree, layout, renderer, operation);
                });
        });
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let Tree {
            state, children, ..
        } = tree;
        let hidden = &state.downcast_ref::<State>().hidden;
        for (((child, tree), layout), _) in self
            .children
            .iter_mut()
            .zip(children.iter_mut())
            .zip(layout.children())
            .zip(hidden)
            .filter(|(_, hidden)| !**hidden)
        {
            child.as_widget_mut().update(
                tree, event, layout, cursor, renderer, clipboard, shell, viewport,
            );
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        let hidden = &tree.state.downcast_ref::<State>().hidden;
        self.children
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
            .zip(hidden)
            .filter(|(_, hidden)| !**hidden)
            .map(|(((child, tree), layout), _)| {
                child
                    .as_widget()
                    .mouse_interaction(tree, layout, cursor, viewport, renderer)
            })
            .max()
            .unwrap_or_default()
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let hidden = &tree.state.downcast_ref::<State>().hidden;
        for (((child, tree), layout), _) in self
            .children
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
            .zip(hidden)
            .filter(|(((_, _), layout), hidden)| !**hidden && layout.bounds().intersects(viewport))
        {
            child
                .as_widget()
                .draw(tree, renderer, theme, style, layout, cursor, viewport);
        }
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        let Tree {
            state, children, ..
        } = tree;
        let hidden = &state.downcast_ref::<State>().hidden;
        let overlays: Vec<_> = self
            .children
            .iter_mut()
            .zip(children.iter_mut())
            .zip(layout.children())
            .zip(hidden)
            .filter(|(_, hidden)| !**hidden)
            .filter_map(|(((child, tree), layout), _)| {
                child
                    .as_widget_mut()
                    .overlay(tree, layout, renderer, viewport, translation)
            })
            .collect();
        (!overlays.is_empty()).then(|| overlay::Group::with_children(overlays).overlay())
    }
}

impl<'a, Message: 'a> From<PriorityRow<'a, Message>> for Element<'a, Message> {
    fn from(row: PriorityRow<'a, Message>) -> Self {
        Element::new(row)
    }
}

#[cfg(test)]
mod tests {
    use crate::widget::{
        priority::{Mode, PriorityState},
        priority_row::{allocate, Slot},
    };

    fn fixed(natural: f32) -> Slot {
        Slot {
            natural,
            min: natural,
            priority: None,
        }
    }

    fn hide(natural: f32, rank: u8) -> Slot {
        Slot {
            natural,
            min: natural,
            priority: Some(PriorityState {
                mode: Mode::Hide,
                rank,
            }),
        }
    }

    fn truncate(natural: f32, min: f32, rank: u8) -> Slot {
        Slot {
            natural,
            min,
            priority: Some(PriorityState {
                mode: Mode::Truncate,
                rank,
            }),
        }
    }

    #[test]
    fn all_fits() {
        let slots = [fixed(100.0), hide(50.0, 2), truncate(200.0, 20.0, 1)];
        assert_eq!(
            allocate(&slots, 370.0, 10.0),
            vec![Some(100.0), Some(50.0), Some(200.0)]
        );
    }

    #[test]
    fn highest_rank_hidden_first() {
        let slots = [truncate(200.0, 20.0, 1), hide(50.0, 2), hide(60.0, 3)];
        assert_eq!(
            allocate(&slots, 260.0, 0.0),
            vec![Some(200.0), Some(50.0), None]
        );
    }

    #[test]
    fn rank_one_kept_last() {
        let slots = [hide(100.0, 1), hide(100.0, 2), hide(100.0, 3)];
        assert_eq!(allocate(&slots, 100.0, 0.0), vec![Some(100.0), None, None]);
    }

    #[test]
    fn rightmost_first_on_tie() {
        let slots = [hide(50.0, 2), hide(50.0, 2)];
        assert_eq!(allocate(&slots, 60.0, 0.0), vec![Some(50.0), None]);
    }

    #[test]
    fn truncate_shrinks_by_overflow() {
        let slots = [fixed(100.0), truncate(200.0, 20.0, 1)];
        assert_eq!(
            allocate(&slots, 250.0, 10.0),
            vec![Some(100.0), Some(140.0)]
        );
    }

    #[test]
    fn hide_before_truncating_rank_one() {
        let slots = [truncate(200.0, 20.0, 1), hide(100.0, 2)];
        assert_eq!(allocate(&slots, 150.0, 0.0), vec![Some(150.0), None]);
    }

    #[test]
    fn truncate_below_min_hidden() {
        let slots = [fixed(100.0), truncate(200.0, 150.0, 1)];
        assert_eq!(allocate(&slots, 250.0, 10.0), vec![Some(100.0), None]);
    }

    #[test]
    fn spacing_given_back_on_hide() {
        let slots = [fixed(100.0), hide(50.0, 1), fixed(100.0)];
        assert_eq!(
            allocate(&slots, 220.0, 20.0),
            vec![Some(100.0), None, Some(100.0)]
        );
    }

    #[test]
    fn unwrapped_and_fill_never_degraded() {
        let fill = Slot {
            natural: 0.0,
            min: 0.0,
            priority: None,
        };
        let slots = [fixed(300.0), fill, hide(50.0, 1)];
        assert_eq!(
            allocate(&slots, 200.0, 10.0),
            vec![Some(300.0), Some(0.0), None]
        );
    }

    #[test]
    fn still_overflowing_when_nothing_left() {
        let slots = [fixed(100.0), truncate(100.0, 80.0, 1)];
        assert_eq!(allocate(&slots, 50.0, 0.0), vec![Some(100.0), None]);
    }
}
