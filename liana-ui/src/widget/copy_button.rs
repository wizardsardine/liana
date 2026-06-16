use iced::{
    advanced::{
        layout, mouse, overlay, renderer,
        widget::{tree, Operation, Tree, Widget},
        Clipboard, Layout, Shell,
    },
    touch,
    widget::button::{Catalog, Status, Style, StyleFn},
    window, Background, Color, Element, Event, Length, Padding, Rectangle, Size, Vector,
};
use iced_core::time::{Duration, Instant};

const DEFAULT_PADDING: Padding = Padding {
    top: 5.0,
    right: 10.0,
    bottom: 5.0,
    left: 10.0,
};
const FEEDBACK_DURATION: Duration = Duration::from_millis(1500);

pub struct BistateButton<'a, Message, Theme, Renderer>
where
    Theme: Catalog,
    Renderer: iced::advanced::Renderer,
{
    copy: Element<'a, Message, Theme, Renderer>,
    check: Element<'a, Message, Theme, Renderer>,
    on_press: Option<Message>,
    width: Length,
    height: Length,
    padding: Padding,
    class: Theme::Class<'a>,
}

impl<'a, Message, Theme, Renderer> BistateButton<'a, Message, Theme, Renderer>
where
    Theme: Catalog,
    Renderer: iced::advanced::Renderer,
{
    pub fn new(
        copy: impl Into<Element<'a, Message, Theme, Renderer>>,
        check: impl Into<Element<'a, Message, Theme, Renderer>>,
    ) -> Self {
        let copy = copy.into();
        let check = check.into();
        let size = copy.as_widget().size_hint();
        Self {
            copy,
            check,
            on_press: None,
            width: size.width.fluid(),
            height: size.height.fluid(),
            padding: DEFAULT_PADDING,
            class: Theme::default(),
        }
    }

    pub fn on_press_maybe(mut self, on_press: Option<Message>) -> Self {
        self.on_press = on_press;
        self
    }

    pub fn style(mut self, style: impl Fn(&Theme, Status) -> Style + 'a) -> Self
    where
        Theme::Class<'a>: From<StyleFn<'a, Theme>>,
    {
        self.class = (Box::new(style) as StyleFn<'a, Theme>).into();
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct State {
    is_pressed: bool,
    copied_until: Option<Instant>,
}

impl State {
    fn icon_index(&self) -> usize {
        usize::from(self.copied_until.is_some())
    }
}

impl<'a, Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for BistateButton<'a, Message, Theme, Renderer>
where
    Message: 'a + Clone,
    Renderer: 'a + iced::advanced::Renderer,
    Theme: Catalog,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.copy), Tree::new(&self.check)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(&[&self.copy, &self.check]);
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: self.width,
            height: self.height,
        }
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let state = tree.state.downcast_ref::<State>();
        let child = state.icon_index();
        let content = if child == 0 {
            &mut self.copy
        } else {
            &mut self.check
        };
        layout::padded(limits, self.width, self.height, self.padding, |limits| {
            content
                .as_widget_mut()
                .layout(&mut tree.children[child], renderer, limits)
        })
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        operation.container(None, layout.bounds());
        let state = tree.state.downcast_ref::<State>();
        let child = state.icon_index();
        let content = if child == 0 {
            &mut self.copy
        } else {
            &mut self.check
        };
        content.as_widget_mut().operate(
            &mut tree.children[child],
            layout.children().next().unwrap(),
            renderer,
            operation,
        );
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
            | Event::Touch(touch::Event::FingerPressed { .. }) => {
                if self.on_press.is_some() && cursor.is_over(layout.bounds()) {
                    let state = tree.state.downcast_mut::<State>();
                    state.is_pressed = true;
                    shell.capture_event();
                    shell.request_redraw();
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
            | Event::Touch(touch::Event::FingerLifted { .. }) => {
                if let Some(on_press) = &self.on_press {
                    let state = tree.state.downcast_mut::<State>();
                    if state.is_pressed {
                        state.is_pressed = false;
                        if cursor.is_over(layout.bounds()) {
                            let copied_until = Instant::now() + FEEDBACK_DURATION;
                            if state.copied_until.is_none() {
                                shell.invalidate_layout();
                            }
                            state.copied_until = Some(copied_until);
                            shell.publish(on_press.clone());
                            shell.request_redraw_at(copied_until);
                        }
                        shell.capture_event();
                        shell.request_redraw();
                    }
                }
            }
            Event::Touch(touch::Event::FingerLost { .. }) => {
                let state = tree.state.downcast_mut::<State>();
                state.is_pressed = false;
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                shell.request_redraw();
            }
            Event::Window(window::Event::RedrawRequested(now)) => {
                let state = tree.state.downcast_mut::<State>();
                if let Some(copied_until) = state.copied_until {
                    if *now >= copied_until {
                        state.copied_until = None;
                        shell.invalidate_layout();
                        shell.request_redraw();
                    } else {
                        shell.request_redraw_at(copied_until);
                    }
                }
            }
            _ => {}
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let is_mouse_over = cursor.is_over(bounds);
        let state = tree.state.downcast_ref::<State>();
        let status = if self.on_press.is_none() {
            Status::Disabled
        } else if is_mouse_over {
            if state.is_pressed {
                Status::Pressed
            } else {
                Status::Hovered
            }
        } else {
            Status::Active
        };

        let style = theme.style(&self.class, status);

        if style.background.is_some() || style.border.width > 0.0 || style.shadow.color.a > 0.0 {
            renderer.fill_quad(
                renderer::Quad {
                    bounds,
                    border: style.border,
                    shadow: style.shadow,
                    snap: style.snap,
                },
                style
                    .background
                    .unwrap_or(Background::Color(Color::TRANSPARENT)),
            );
        }

        let child = state.icon_index();
        let content = if child == 0 { &self.copy } else { &self.check };
        content.as_widget().draw(
            &tree.children[child],
            renderer,
            theme,
            &renderer::Style {
                text_color: style.text_color,
            },
            layout.children().next().unwrap(),
            cursor,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        _tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &Renderer,
    ) -> mouse::Interaction {
        if cursor.is_over(layout.bounds()) && self.on_press.is_some() {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::default()
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
        let state = tree.state.downcast_ref::<State>();
        let child = state.icon_index();
        let content = if child == 0 {
            &mut self.copy
        } else {
            &mut self.check
        };
        content.as_widget_mut().overlay(
            &mut tree.children[child],
            layout.children().next().unwrap(),
            renderer,
            viewport,
            translation,
        )
    }
}

impl<'a, Message, Theme, Renderer> From<BistateButton<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: Clone + 'a,
    Theme: Catalog + 'a,
    Renderer: iced::advanced::Renderer + 'a,
{
    fn from(button: BistateButton<'a, Message, Theme, Renderer>) -> Self {
        Element::new(button)
    }
}
