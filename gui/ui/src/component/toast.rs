use std::time::Instant;

use super::theme::Theme;

use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{layout, mouse, overlay, renderer};
use iced::advanced::{Clipboard, Layout, Shell, Widget};
use iced::event::{self, Event};
use iced::{Alignment, Element, Length, Point, Rectangle, Size, Vector};

pub trait Toast {
    fn title(&self) -> &str;
    fn body(&self) -> &str;
}

pub struct Manager<'a, Message, Renderer> {
    content: Element<'a, Message, Renderer>,
    toasts: Vec<Element<'a, Message, Renderer>>,
}

impl<'a, Message> Manager<'a, Message, iced::Renderer<Theme>>
where
    Message: 'a + Clone,
{
    pub fn new(
        content: impl Into<Element<'a, Message, iced::Renderer<Theme>>>,
        toasts: Vec<Element<'a, Message, iced::Renderer<Theme>>>,
    ) -> Self {
        Self {
            content: content.into(),
            toasts: toasts.into_iter().collect(),
        }
    }
}

impl<'a, Message, Renderer> Widget<Message, Renderer> for Manager<'a, Message, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn width(&self) -> Length {
        self.content.as_widget().width()
    }

    fn height(&self) -> Length {
        self.content.as_widget().height()
    }

    fn layout(&self, renderer: &Renderer, limits: &layout::Limits) -> layout::Node {
        self.content.as_widget().layout(renderer, limits)
    }

    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        struct Marker(Vec<Instant>);
        iced::advanced::widget::tree::Tag::of::<Marker>()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        iced::advanced::widget::tree::State::new(Vec::<Option<Instant>>::new())
    }

    fn children(&self) -> Vec<Tree> {
        std::iter::once(Tree::new(&self.content))
            .chain(self.toasts.iter().map(Tree::new))
            .collect()
    }

    fn diff(&self, tree: &mut Tree) {
        let instants = tree.state.downcast_mut::<Vec<Option<Instant>>>();

        // Invalidating removed instants to None allows us to remove
        // them here so that diffing for removed / new toast instants
        // is accurate
        instants.retain(Option::is_some);

        match (instants.len(), self.toasts.len()) {
            (old, new) if old > new => {
                instants.truncate(new);
            }
            (old, new) if old < new => {
                instants.extend(std::iter::repeat(Some(Instant::now())).take(new - old));
            }
            _ => {}
        }

        tree.diff_children(
            &std::iter::once(&self.content)
                .chain(self.toasts.iter())
                .collect::<Vec<_>>(),
        );
    }

    fn operate(
        &self,
        state: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation<Message>,
    ) {
        operation.container(None, layout.bounds(), &mut |operation| {
            self.content
                .as_widget()
                .operate(&mut state.children[0], layout, renderer, operation);
        });
    }

    fn on_event(
        &mut self,
        state: &mut Tree,
        event: Event,
        layout: Layout<'_>,
        cursor_position: iced::mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) -> event::Status {
        self.content.as_widget_mut().on_event(
            &mut state.children[0],
            event,
            layout,
            cursor_position,
            renderer,
            clipboard,
            shell,
            viewport,
        )
    }

    fn draw(
        &self,
        state: &Tree,
        renderer: &mut Renderer,
        theme: &<Renderer as iced::advanced::Renderer>::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor_position: iced::mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content.as_widget().draw(
            &state.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor_position,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        state: &Tree,
        layout: Layout<'_>,
        cursor_position: iced::mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &state.children[0],
            layout,
            cursor_position,
            viewport,
            renderer,
        )
    }

    fn overlay<'b>(
        &'b mut self,
        state: &'b mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
    ) -> Option<overlay::Element<'b, Message, Renderer>> {
        let instants = state.state.downcast_mut::<Vec<Option<Instant>>>();

        let (content_state, toasts_state) = state.children.split_at_mut(1);

        let content = self
            .content
            .as_widget_mut()
            .overlay(&mut content_state[0], layout, renderer);

        let toasts = (!self.toasts.is_empty()).then(|| {
            overlay::Element::new(
                layout.bounds().position(),
                Box::new(Overlay {
                    toasts: &mut self.toasts,
                    state: toasts_state,
                    instants,
                }),
            )
        });
        let overlays = content.into_iter().chain(toasts).collect::<Vec<_>>();

        (!overlays.is_empty()).then(|| overlay::Group::with_children(overlays).overlay())
    }
}

struct Overlay<'a, 'b, Message, Renderer> {
    toasts: &'b mut [Element<'a, Message, Renderer>],
    state: &'b mut [Tree],
    instants: &'b mut [Option<Instant>],
}

impl<'a, 'b, Message, Renderer> overlay::Overlay<Message, Renderer>
    for Overlay<'a, 'b, Message, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn layout(&self, renderer: &Renderer, bounds: Size, position: Point) -> layout::Node {
        let limits = layout::Limits::new(Size::ZERO, bounds)
            .width(Length::Fill)
            .height(Length::Fill);

        layout::flex::resolve(
            layout::flex::Axis::Vertical,
            renderer,
            &limits,
            10.into(),
            10.0,
            Alignment::End,
            self.toasts,
        )
        .translate(Vector::new(position.x, position.y))
    }

    fn on_event(
        &mut self,
        event: Event,
        layout: Layout<'_>,
        cursor_position: iced::mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
    ) -> event::Status {
        let viewport = layout.bounds();
        self.toasts
            .iter_mut()
            .zip(self.state.iter_mut())
            .zip(layout.children())
            .zip(self.instants.iter_mut())
            .map(|(((child, state), layout), instant)| {
                let mut local_messages = vec![];
                let mut local_shell = Shell::new(&mut local_messages);

                let status = child.as_widget_mut().on_event(
                    state,
                    event.clone(),
                    layout,
                    cursor_position,
                    renderer,
                    clipboard,
                    &mut local_shell,
                    &viewport,
                );

                if !local_shell.is_empty() {
                    instant.take();
                }

                shell.merge(local_shell, std::convert::identity);

                status
            })
            .fold(event::Status::Ignored, event::Status::merge)
    }

    fn draw(
        &self,
        renderer: &mut Renderer,
        theme: &<Renderer as iced::advanced::Renderer>::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor_position: iced::mouse::Cursor,
    ) {
        let viewport = layout.bounds();

        for ((child, state), layout) in self
            .toasts
            .iter()
            .zip(self.state.iter())
            .zip(layout.children())
        {
            child.as_widget().draw(
                state,
                renderer,
                theme,
                style,
                layout,
                cursor_position,
                &viewport,
            );
        }
    }

    fn operate(
        &mut self,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn iced::advanced::widget::Operation<Message>,
    ) {
        operation.container(None, layout.bounds(), &mut |operation| {
            self.toasts
                .iter()
                .zip(self.state.iter_mut())
                .zip(layout.children())
                .for_each(|((child, state), layout)| {
                    child
                        .as_widget()
                        .operate(state, layout, renderer, operation);
                })
        });
    }

    fn mouse_interaction(
        &self,
        layout: Layout<'_>,
        cursor_position: iced::mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.toasts
            .iter()
            .zip(self.state.iter())
            .zip(layout.children())
            .map(|((child, state), layout)| {
                child.as_widget().mouse_interaction(
                    state,
                    layout,
                    cursor_position,
                    viewport,
                    renderer,
                )
            })
            .max()
            .unwrap_or_default()
    }

    fn is_over(&self, layout: Layout<'_>, _renderer: &Renderer, cursor_position: Point) -> bool {
        layout
            .children()
            .any(|layout| layout.bounds().contains(cursor_position))
    }
}

impl<'a, Message, Renderer> From<Manager<'a, Message, Renderer>> for Element<'a, Message, Renderer>
where
    Renderer: 'a + iced::advanced::Renderer,
    Message: 'a,
{
    fn from(manager: Manager<'a, Message, Renderer>) -> Self {
        Element::new(manager)
    }
}
