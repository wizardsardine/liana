//! A card whose clickable row folds the details card below it.
use iced::{
    advanced::{
        layout, mouse, overlay, renderer,
        widget::{tree, Operation, Tree, Widget},
        Clipboard, Layout, Shell,
    },
    border::Radius,
    Element, Event, Length, Padding, Point, Rectangle, Shadow, Size, Vector,
};

use crate::{
    component::button::{ListEntryAccent, LIST_ENTRY_ACCENT_WIDTH},
    icon,
    theme::{card::CARD_SHADOW_HOVER, Theme},
};

type Renderer = iced::Renderer;

/// Space left between the header card and the details card when unfolded.
const CARD_GAP: f32 = 2.0;
/// Space left between the clickable row and its chevron.
const CHEVRON_SPACING: f32 = 10.0;
/// Space left between the visible part and the clickable row.
const VISIBLE_SPACING: f32 = 10.0;

/// The children of the card, in the order their trees are kept.
#[derive(Clone, Copy)]
enum Child {
    Visible,
    Clickable,
    Foldable,
    ChevronFolded,
    ChevronUnfolded,
}

impl Child {
    const ALL: [Child; 5] = [
        Child::Visible,
        Child::Clickable,
        Child::Foldable,
        Child::ChevronFolded,
        Child::ChevronUnfolded,
    ];

    fn chevron(expanded: bool) -> Child {
        if expanded {
            Child::ChevronUnfolded
        } else {
            Child::ChevronFolded
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct FoldableState {
    expanded: bool,
}

pub struct FoldableCard<'a, Message> {
    visible: Option<Element<'a, Message, Theme, Renderer>>,
    clickable: Element<'a, Message, Theme, Renderer>,
    foldable: Option<Element<'a, Message, Theme, Renderer>>,
    chevron_folded: Element<'a, Message, Theme, Renderer>,
    chevron_unfolded: Element<'a, Message, Theme, Renderer>,
    accent: Option<ListEntryAccent>,
    expanded: Option<bool>,
    on_toggle: Option<Box<dyn Fn() -> Message + 'a>>,
    padding: Padding,
    width: Length,
}

impl<'a, Message: 'a> FoldableCard<'a, Message> {
    /// `visible` sits on top, `clickable` toggles `foldable`; without `foldable` there is nothing
    /// to toggle and no chevron.
    pub fn new(
        visible: Option<Element<'a, Message, Theme, Renderer>>,
        clickable: impl Into<Element<'a, Message, Theme, Renderer>>,
        foldable: Option<Element<'a, Message, Theme, Renderer>>,
    ) -> Self {
        Self {
            visible,
            clickable: clickable.into(),
            foldable,
            chevron_folded: icon::collapsed_icon().into(),
            chevron_unfolded: icon::collapse_icon().into(),
            accent: None,
            expanded: None,
            on_toggle: None,
            padding: Padding::ZERO,
            width: Length::Fill,
        }
    }

    /// Draws a stripe of this colour along the left edge of the cards.
    pub fn accent(mut self, accent: ListEntryAccent) -> Self {
        self.accent = Some(accent);
        self
    }

    /// Lets the caller own the folded state instead of the card.
    pub fn expanded(mut self, expanded: bool) -> Self {
        self.expanded = Some(expanded);
        self
    }

    /// Publishes this message on click instead of folding the card itself.
    pub fn on_toggle(mut self, on_toggle: impl Fn() -> Message + 'a) -> Self {
        self.on_toggle = Some(Box::new(on_toggle));
        self
    }

    pub fn padding<P: Into<Padding>>(mut self, padding: P) -> Self {
        self.padding = padding.into();
        self
    }

    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    fn child(&self, child: Child) -> Option<&Element<'a, Message, Theme, Renderer>> {
        match child {
            Child::Visible => self.visible.as_ref(),
            Child::Clickable => Some(&self.clickable),
            Child::Foldable => self.foldable.as_ref(),
            Child::ChevronFolded => Some(&self.chevron_folded),
            Child::ChevronUnfolded => Some(&self.chevron_unfolded),
        }
    }

    fn child_mut(&mut self, child: Child) -> Option<&mut Element<'a, Message, Theme, Renderer>> {
        match child {
            Child::Visible => self.visible.as_mut(),
            Child::Clickable => Some(&mut self.clickable),
            Child::Foldable => self.foldable.as_mut(),
            Child::ChevronFolded => Some(&mut self.chevron_folded),
            Child::ChevronUnfolded => Some(&mut self.chevron_unfolded),
        }
    }

    fn is_expanded(&self, tree: &Tree) -> bool {
        self.foldable.is_some() && tree.state.downcast_ref::<FoldableState>().expanded
    }
}

/// Where a click folds or unfolds the card: below the visible part, across the whole card.
fn clickable_zone(card: Rectangle, header: Rectangle, visible: Rectangle) -> Rectangle {
    let top = visible.y + visible.height;
    Rectangle {
        x: card.x,
        y: top,
        width: card.width,
        height: header.y + header.height - top,
    }
}

impl<'a, Message: 'a> Widget<Message, Theme, Renderer> for FoldableCard<'a, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<FoldableState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(FoldableState {
            expanded: self.expanded.unwrap_or(false),
        })
    }

    fn children(&self) -> Vec<Tree> {
        Child::ALL
            .iter()
            .map(|&child| {
                self.child(child)
                    .map_or_else(Tree::empty, |el| Tree::new(el))
            })
            .collect()
    }

    fn diff(&self, tree: &mut Tree) {
        if let Some(expanded) = self.expanded {
            tree.state.downcast_mut::<FoldableState>().expanded = expanded;
        }
        for child in Child::ALL {
            let slot = &mut tree.children[child as usize];
            match self.child(child) {
                Some(element) => slot.diff(element),
                None => *slot = Tree::empty(),
            }
        }
    }

    fn size(&self) -> Size<Length> {
        Size::new(self.width, Length::Shrink)
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let expanded = self.is_expanded(tree);
        let padding = self.padding;
        let limits = limits.width(self.width);
        let inner_limits = limits.shrink(padding);

        let visible_node = self.visible.as_mut().map(|visible| {
            visible.as_widget_mut().layout(
                &mut tree.children[Child::Visible as usize],
                renderer,
                &inner_limits,
            )
        });
        let visible_size = visible_node.as_ref().map_or(Size::ZERO, |node| node.size());
        let row_top = match visible_node {
            Some(_) => padding.top + visible_size.height + VISIBLE_SPACING,
            None => padding.top,
        };

        let chevron = Child::chevron(expanded);
        let chevron_node = if self.foldable.is_some() {
            self.child_mut(chevron).map(|element| {
                element.as_widget_mut().layout(
                    &mut tree.children[chevron as usize],
                    renderer,
                    &inner_limits,
                )
            })
        } else {
            None
        };
        let chevron_size = chevron_node.as_ref().map_or(Size::ZERO, |node| node.size());
        let chevron_room = match chevron_node {
            Some(_) => chevron_size.width + CHEVRON_SPACING,
            None => 0.0,
        };

        let clickable_node = self.clickable.as_widget_mut().layout(
            &mut tree.children[Child::Clickable as usize],
            renderer,
            &inner_limits.shrink(Padding {
                right: chevron_room,
                ..Padding::ZERO
            }),
        );
        let clickable_size = clickable_node.size();
        let row_height = clickable_size.height.max(chevron_size.height);

        let content_width = visible_size.width.max(clickable_size.width + chevron_room);
        let header_width = (content_width + padding.x()).min(limits.max().width);
        let header_height = row_top + row_height + padding.bottom;

        // Absent parts keep an empty node so the header children stay at fixed positions.
        let visible_node = visible_node.map_or_else(
            || layout::Node::new(Size::ZERO),
            |node| node.move_to(Point::new(padding.left, padding.top)),
        );
        let clickable_node = clickable_node.move_to(Point::new(padding.left, row_top));
        let chevron_node = chevron_node.map_or_else(
            || layout::Node::new(Size::ZERO),
            |node| {
                node.move_to(Point::new(
                    header_width - padding.right - chevron_size.width,
                    row_top + (row_height - chevron_size.height) / 2.0,
                ))
            },
        );
        let header_node = layout::Node::with_children(
            Size::new(header_width, header_height),
            vec![visible_node, clickable_node, chevron_node],
        );

        let foldable_node = if expanded {
            self.foldable.as_mut().map(|foldable| {
                foldable.as_widget_mut().layout(
                    &mut tree.children[Child::Foldable as usize],
                    renderer,
                    &limits,
                )
            })
        } else {
            None
        };

        let Some(foldable_node) = foldable_node else {
            return layout::Node::with_children(
                Size::new(header_width, header_height),
                vec![header_node],
            );
        };

        let foldable_size = foldable_node.size();
        let total_height = header_height + CARD_GAP + foldable_size.height;
        let total_width = header_width
            .max(foldable_size.width)
            .min(limits.max().width);

        layout::Node::with_children(
            Size::new(total_width, total_height),
            vec![
                header_node,
                foldable_node.move_to(Point::new(0.0, header_height + CARD_GAP)),
            ],
        )
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
        let mut children = layout.children();
        let Some(header_layout) = children.next() else {
            return;
        };
        let foldable_layout = children.next();
        let mut header_children = header_layout.children();
        let (Some(visible_layout), Some(clickable_layout)) =
            (header_children.next(), header_children.next())
        else {
            return;
        };

        if self.foldable.is_some() {
            if let Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) = event {
                let zone = clickable_zone(
                    layout.bounds(),
                    header_layout.bounds(),
                    visible_layout.bounds(),
                );
                if cursor.is_over(zone) {
                    match &self.on_toggle {
                        Some(on_toggle) => shell.publish(on_toggle()),
                        None => {
                            let state = tree.state.downcast_mut::<FoldableState>();
                            state.expanded = !state.expanded;
                        }
                    }
                    shell.invalidate_layout();
                    shell.request_redraw();
                    shell.capture_event();
                    return;
                }
            }
        }

        let expanded = self.is_expanded(tree);
        let [visible_tree, clickable_tree, foldable_tree, _, _] = tree.children.as_mut_slice()
        else {
            return;
        };

        if let Some(visible) = &mut self.visible {
            visible.as_widget_mut().update(
                visible_tree,
                event,
                visible_layout,
                cursor,
                renderer,
                clipboard,
                shell,
                viewport,
            );
        }
        self.clickable.as_widget_mut().update(
            clickable_tree,
            event,
            clickable_layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
        if let (true, Some(foldable), Some(foldable_layout)) =
            (expanded, &mut self.foldable, foldable_layout)
        {
            foldable.as_widget_mut().update(
                foldable_tree,
                event,
                foldable_layout,
                cursor,
                renderer,
                clipboard,
                shell,
                viewport,
            );
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        defaults: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let expanded = self.is_expanded(tree);
        let bounds = layout.bounds();
        let mut children = layout.children();
        let Some(header_layout) = children.next() else {
            return;
        };
        let foldable_layout = children.next();
        let mut header_children = header_layout.children();
        let (Some(visible_layout), Some(clickable_layout), Some(chevron_layout)) = (
            header_children.next(),
            header_children.next(),
            header_children.next(),
        ) else {
            return;
        };

        let style = crate::theme::card::button_simple(theme);
        let radius = style.border.radius;
        // Unfolded, the two cards face each other with their joined corners squared off.
        let header_radius = if expanded {
            Radius {
                bottom_right: 0.0,
                bottom_left: 0.0,
                ..radius
            }
        } else {
            radius
        };

        let hovered = self.foldable.is_some()
            && cursor.is_over(clickable_zone(
                bounds,
                header_layout.bounds(),
                visible_layout.bounds(),
            ));
        let shadow = if hovered {
            CARD_SHADOW_HOVER
        } else {
            style.shadow
        };

        if let Some(background) = style.background {
            let draw_card = |renderer: &mut Renderer, bounds: Rectangle, radius: Radius| {
                // With an accent, a card of its colour sits behind and carries the shadow, and
                // the card is inset on the left so the accent shows as a stripe.
                let (bounds, shadow) = match self.accent {
                    Some(accent) => {
                        let stripe = renderer::Quad {
                            bounds: Rectangle {
                                y: bounds.y + 1.0,
                                width: bounds.width - 1.0,
                                height: bounds.height - 2.0,
                                ..bounds
                            },
                            border: iced::Border {
                                radius,
                                ..Default::default()
                            },
                            shadow,
                            snap: style.snap,
                        };
                        renderer::Renderer::fill_quad(renderer, stripe, accent(theme));
                        let inset = Rectangle {
                            x: bounds.x + LIST_ENTRY_ACCENT_WIDTH,
                            width: bounds.width - LIST_ENTRY_ACCENT_WIDTH,
                            ..bounds
                        };
                        (inset, Shadow::default())
                    }
                    None => (bounds, shadow),
                };
                let card = renderer::Quad {
                    bounds,
                    border: iced::Border {
                        radius,
                        ..style.border
                    },
                    shadow,
                    snap: style.snap,
                };
                renderer::Renderer::fill_quad(renderer, card, background);
            };

            let header_card = Rectangle {
                width: bounds.width,
                ..header_layout.bounds()
            };
            draw_card(renderer, header_card, header_radius);

            if let Some(foldable_layout) = foldable_layout {
                let details_card = Rectangle {
                    x: bounds.x,
                    width: bounds.width,
                    ..foldable_layout.bounds()
                };
                let details_radius = Radius {
                    top_left: 0.0,
                    top_right: 0.0,
                    ..radius
                };
                draw_card(renderer, details_card, details_radius);
            }
        }

        if let Some(visible) = &self.visible {
            visible.as_widget().draw(
                &tree.children[Child::Visible as usize],
                renderer,
                theme,
                defaults,
                visible_layout,
                cursor,
                viewport,
            );
        }
        self.clickable.as_widget().draw(
            &tree.children[Child::Clickable as usize],
            renderer,
            theme,
            defaults,
            clickable_layout,
            cursor,
            viewport,
        );
        if self.foldable.is_some() {
            let chevron = Child::chevron(expanded);
            if let Some(element) = self.child(chevron) {
                element.as_widget().draw(
                    &tree.children[chevron as usize],
                    renderer,
                    theme,
                    defaults,
                    chevron_layout,
                    cursor,
                    viewport,
                );
            }
        }
        if let (true, Some(foldable), Some(foldable_layout)) =
            (expanded, &self.foldable, foldable_layout)
        {
            foldable.as_widget().draw(
                &tree.children[Child::Foldable as usize],
                renderer,
                theme,
                defaults,
                foldable_layout,
                cursor,
                viewport,
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
        let expanded = self.is_expanded(tree);
        let mut children = layout.children();
        let Some(header_layout) = children.next() else {
            return mouse::Interaction::default();
        };
        let foldable_layout = children.next();
        let mut header_children = header_layout.children();
        let (Some(visible_layout), Some(clickable_layout)) =
            (header_children.next(), header_children.next())
        else {
            return mouse::Interaction::default();
        };

        let zone = clickable_zone(
            layout.bounds(),
            header_layout.bounds(),
            visible_layout.bounds(),
        );
        if self.foldable.is_some() && cursor.is_over(zone) {
            return mouse::Interaction::Pointer;
        }

        let visible = self.visible.as_ref().map(|visible| {
            visible.as_widget().mouse_interaction(
                &tree.children[Child::Visible as usize],
                visible_layout,
                cursor,
                viewport,
                renderer,
            )
        });
        let clickable = self.clickable.as_widget().mouse_interaction(
            &tree.children[Child::Clickable as usize],
            clickable_layout,
            cursor,
            viewport,
            renderer,
        );
        let foldable = match (expanded, &self.foldable, foldable_layout) {
            (true, Some(foldable), Some(foldable_layout)) => {
                Some(foldable.as_widget().mouse_interaction(
                    &tree.children[Child::Foldable as usize],
                    foldable_layout,
                    cursor,
                    viewport,
                    renderer,
                ))
            }
            _ => None,
        };

        [visible, Some(clickable), foldable]
            .into_iter()
            .flatten()
            .max()
            .unwrap_or_default()
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        let expanded = self.is_expanded(tree);
        let mut children = layout.children();
        let Some(header_layout) = children.next() else {
            return;
        };
        let foldable_layout = children.next();
        let mut header_children = header_layout.children();
        let (Some(visible_layout), Some(clickable_layout)) =
            (header_children.next(), header_children.next())
        else {
            return;
        };
        let [visible_tree, clickable_tree, foldable_tree, _, _] = tree.children.as_mut_slice()
        else {
            return;
        };

        operation.container(None, layout.bounds());
        operation.traverse(&mut |operation| {
            if let Some(visible) = &mut self.visible {
                visible
                    .as_widget_mut()
                    .operate(visible_tree, visible_layout, renderer, operation);
            }
            self.clickable.as_widget_mut().operate(
                clickable_tree,
                clickable_layout,
                renderer,
                operation,
            );
            if let (true, Some(foldable), Some(foldable_layout)) =
                (expanded, &mut self.foldable, foldable_layout)
            {
                foldable.as_widget_mut().operate(
                    foldable_tree,
                    foldable_layout,
                    renderer,
                    operation,
                );
            }
        });
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        let expanded = self.is_expanded(tree);
        let mut children = layout.children();
        let header_layout = children.next()?;
        let foldable_layout = children.next();
        let mut header_children = header_layout.children();
        let visible_layout = header_children.next()?;
        let clickable_layout = header_children.next()?;
        let [visible_tree, clickable_tree, foldable_tree, _, _] = tree.children.as_mut_slice()
        else {
            return None;
        };

        let mut overlays = Vec::new();
        if let Some(visible) = &mut self.visible {
            overlays.extend(visible.as_widget_mut().overlay(
                visible_tree,
                visible_layout,
                renderer,
                viewport,
                translation,
            ));
        }
        overlays.extend(self.clickable.as_widget_mut().overlay(
            clickable_tree,
            clickable_layout,
            renderer,
            viewport,
            translation,
        ));
        if let (true, Some(foldable), Some(foldable_layout)) =
            (expanded, &mut self.foldable, foldable_layout)
        {
            overlays.extend(foldable.as_widget_mut().overlay(
                foldable_tree,
                foldable_layout,
                renderer,
                viewport,
                translation,
            ));
        }

        (!overlays.is_empty()).then(|| overlay::Group::with_children(overlays).overlay())
    }
}

impl<'a, Message: 'a> From<FoldableCard<'a, Message>> for Element<'a, Message, Theme, Renderer> {
    fn from(card: FoldableCard<'a, Message>) -> Self {
        Element::new(card)
    }
}
