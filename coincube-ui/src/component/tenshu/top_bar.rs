//! The one top bar (`.top`) shared by the Launcher, the unlock face and every
//! open Cube: 64px, the rail colour, a 24px gutter, the brand on the left and
//! a right group of `[eye, chips…, App preferences]` in that order.
//!
//! The bar holds no state. Callers pass ready controls, their stable ids and
//! their existing intents; the Launcher's filters go in [`TopBar::lead`].

use std::borrow::Cow;
use std::cell::Cell;
use std::rc::Rc;

use iced::advanced::layout::{self, Layout};
use iced::advanced::overlay;
use iced::advanced::renderer;
use iced::advanced::widget::{tree, Id, Operation, Tree, Widget};
use iced::advanced::{Clipboard, Shell};
use iced::widget::{button, container, svg, text, tooltip, Space};
use iced::{mouse, Alignment, Background, Border, Color, Event, Length, Rectangle, Size, Vector};

use crate::font;
use crate::theme::Theme;
use crate::widget::action::Action;
use crate::widget::{Container, Element, Renderer, Row};

/// `.top { height: 64px }`.
pub const HEIGHT: f32 = 64.0;
/// `.top { padding: 0 24px }`.
pub const GUTTER: f32 = 24.0;
/// `.top { gap: 16px }`. Wider than two 4px focus rings, so a focused
/// control's ring never reaches its neighbour.
pub const GAP: f32 = 16.0;
/// `.tbtn.eye { width: 32px }` and its height.
pub const ICON_BUTTON: f32 = 32.0;
/// `.tbtn svg { width: 16px }`.
const GLYPH: f32 = 16.0;
/// `.brand svg { width: 20px }`.
const BRAND_MARK: f32 = 20.0;
/// `.tbtn { border-radius: 6px }`, also the brand's.
const RADIUS: f32 = 6.0;

/// The accessible name of the App preferences control on every bar.
pub const APP_PREFERENCES: &str = "App preferences";

/// `.brand`'s cube.
pub const CUBE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="1.5" stroke-linejoin="round"><path d="M12 3l8 4.5v9L12 21l-8-4.5v-9z"/><path d="M4 7.5l8 4.5 8-4.5M12 12v9"/></svg>"##;
/// The eye while balances are shown.
pub const EYE_OPEN: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12z"/><circle cx="12" cy="12" r="3"/></svg>"##;
/// The eye while balances are hidden or not yet known.
pub const EYE_SHUT: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M9.9 5.8A9.6 9.6 0 0 1 12 5.5c6 0 9.5 6.5 9.5 6.5a16 16 0 0 1-2.6 3.4M6.4 7.4C3.9 9.1 2.5 12 2.5 12S6 18.5 12 18.5c1.8 0 3.3-.5 4.6-1.2M9.9 9.9a3 3 0 0 0 4.2 4.2M4 4l16 16"/></svg>"##;
/// App preferences.
pub const SLIDERS: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="1.5" stroke-linecap="round"><path d="M4 7h10M18 7h2M4 17h4M12 17h8"/><circle cx="16" cy="7" r="2"/><circle cx="10" cy="17" r="2"/></svg>"##;

/// A bar under construction. Convert it into an [`Element`].
pub struct TopBar<'a, M> {
    brand: Element<'a, M>,
    lead: Vec<Element<'a, M>>,
    eye: Option<Element<'a, M>>,
    chips: Vec<Element<'a, M>>,
    preferences: Element<'a, M>,
}

/// A bar with the brand on the left and App preferences last on the right.
pub fn top_bar<'a, M: 'a>(
    brand: impl Into<Element<'a, M>>,
    preferences: impl Into<Element<'a, M>>,
) -> TopBar<'a, M> {
    TopBar {
        brand: brand.into(),
        lead: Vec::new(),
        eye: None,
        chips: Vec::new(),
        preferences: preferences.into(),
    }
}

impl<'a, M: 'a> TopBar<'a, M> {
    /// An item after the brand, such as the Launcher's network filters.
    /// `None` adds nothing.
    pub fn lead(mut self, item: impl Into<Element<'a, M>>) -> Self {
        self.lead.push(item.into());
        self
    }

    /// The balances eye, first in the right group. `None` while privacy is
    /// unavailable.
    pub fn eye(mut self, eye: impl Into<Element<'a, M>>) -> Self {
        self.eye = Some(eye.into());
        self
    }

    /// A status chip, after the eye, in the order given.
    pub fn chip(mut self, chip: impl Into<Element<'a, M>>) -> Self {
        self.chips.push(chip.into());
        self
    }
}

impl<'a, M: 'a> From<TopBar<'a, M>> for Element<'a, M> {
    fn from(bar: TopBar<'a, M>) -> Self {
        let TopBar {
            brand,
            lead,
            eye,
            chips,
            preferences,
        } = bar;
        let mut row = Row::new()
            .spacing(GAP)
            .align_y(Alignment::Center)
            .height(Length::Fill)
            .push(brand);
        for item in lead {
            row = row.push(item);
        }
        row = row.push(Space::new().width(Length::Fill)).push(eye);
        for chip in chips {
            row = row.push(chip);
        }
        Container::new(row.push(preferences))
            .height(Length::Fixed(HEIGHT))
            .padding([0.0, GUTTER])
            .width(Length::Fill)
            .style(rail)
            .into()
    }
}

fn rail(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(theme.colors.shell.rail)),
        text_color: Some(theme.colors.shell.ink),
        ..Default::default()
    }
}

/// A stroke glyph tinted like CSS `currentColor`.
fn mark<'a, M: 'a>(
    source: &'static str,
    size: f32,
    colour: impl Fn(&Theme) -> Color + 'a,
) -> Element<'a, M> {
    svg(svg::Handle::from_memory(source.as_bytes()))
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .style(move |theme: &Theme, _| svg::Style {
            color: Some(colour(theme)),
        })
        .into()
}

fn ink(theme: &Theme, muted: bool) -> Color {
    if muted {
        theme.colors.shell.muted
    } else {
        theme.colors.shell.ink
    }
}

/// `.brand` word size: the COINCUBE wordmark beside the 20px cube.
pub const BRAND_WORD: f32 = 15.0;

fn brand_row<'a, M: 'a>(mark: Element<'a, M>, word: Element<'a, M>) -> Row<'a, M> {
    Row::new()
        .spacing(8)
        .align_y(Alignment::Center)
        .push(mark)
        .push(word)
}

/// The brand word: the two-tone COINCUBE wordmark, or the same letters in
/// one muted ink under a Cube's unlock face. Both set "COINCUBE" in Space
/// Grotesk Bold at [`BRAND_WORD`], so the brand keeps one width either way.
fn brand_word<'a, M: 'a>(muted: bool) -> Element<'a, M> {
    if muted {
        text("COINCUBE")
            .size(BRAND_WORD)
            .font(font::SPACE_GROTESK_BOLD)
            .style(move |theme: &Theme| text::Style {
                color: Some(ink(theme, true)),
            })
            .into()
    } else {
        crate::image::coincube_wordmark(BRAND_WORD).into()
    }
}

/// `.brand` where it is not a control: the cube and the COINCUBE wordmark,
/// muted under a Cube's unlock face (`.quiet`).
pub fn brand_mark<'a, M: 'a>(muted: bool) -> Element<'a, M> {
    brand_row(
        mark(CUBE, BRAND_MARK, move |theme| ink(theme, muted)),
        brand_word(muted),
    )
    .into()
}

/// `.brand` as the way back to the Launcher. Hover turns the whole mark the
/// accent ink (`.brand:hover`).
pub fn brand_action<'a, M: Clone + 'static>(
    id: impl Into<Id>,
    focus_intent: fn(Id) -> M,
    on_press: M,
) -> Element<'a, M> {
    // An SVG cannot inherit the button's text colour, and its own hover
    // status covers only the cube. The button's style runs just before its
    // content draws, so it records the hover for the cube to read.
    let hovered = Rc::new(Cell::new(false));
    let cube_hovered = hovered.clone();
    let cube = mark(CUBE, BRAND_MARK, move |theme| {
        if cube_hovered.get() {
            theme.colors.shell.accent_ink
        } else {
            theme.colors.shell.ink
        }
    });
    Action::new(id, brand_row(cube, brand_word(false)), focus_intent)
        .padding(0.0)
        .style(move |theme: &Theme, status| {
            let hover = matches!(status, button::Status::Hovered | button::Status::Pressed);
            hovered.set(hover);
            button::Style {
                background: None,
                text_color: if hover {
                    theme.colors.shell.accent_ink
                } else {
                    theme.colors.shell.ink
                },
                border: Border {
                    radius: RADIUS.into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        })
        .focus_radius(RADIUS)
        .on_press(on_press)
        .into()
}

/// `.tbtn`: a 32px bar control, borderless at rest, with the line border on
/// hover. `pressed` (`aria-pressed`, an open sheet) adds the selected fill
/// and the accent border.
pub fn button_style(theme: &Theme, status: button::Status, pressed: bool) -> button::Style {
    let shell = theme.colors.shell;
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: pressed.then_some(Background::Color(shell.selected)),
        text_color: shell.ink,
        border: Border {
            color: if pressed {
                shell.accent
            } else if hovered {
                shell.line
            } else {
                Color::TRANSPARENT
            },
            width: 1.0,
            radius: RADIUS.into(),
        },
        ..Default::default()
    }
}

/// A 32×32 icon-only bar control (`.tbtn.eye`). Its label is the tooltip and
/// its accessible name. Build it, then convert it into an [`Element`].
pub struct IconAction<'a, M> {
    id: Id,
    glyph: &'static str,
    label: Cow<'a, str>,
    focus_intent: fn(Id) -> M,
    on_press: Option<M>,
    pressed: bool,
    muted: bool,
}

/// An icon-only control. Without an intent it is inert and leaves the Tab
/// order, as every [`Action`] does.
pub fn icon_action<'a, M>(
    id: impl Into<Id>,
    glyph: &'static str,
    label: impl Into<Cow<'a, str>>,
    focus_intent: fn(Id) -> M,
) -> IconAction<'a, M> {
    IconAction {
        id: id.into(),
        glyph,
        label: localized(label.into()),
        focus_intent,
        on_press: None,
        pressed: false,
        muted: false,
    }
}

/// The SLIDERS control that opens App preferences.
pub fn preferences<'a, M>(
    id: impl Into<Id>,
    focus_intent: fn(Id) -> M,
    on_press: M,
) -> IconAction<'a, M> {
    icon_action(id, SLIDERS, APP_PREFERENCES, focus_intent).on_press(on_press)
}

impl<'a, M> IconAction<'a, M> {
    pub fn on_press(mut self, message: M) -> Self {
        self.on_press = Some(message);
        self
    }

    pub fn on_press_maybe(mut self, message: Option<M>) -> Self {
        self.on_press = message;
        self
    }

    /// Draw the pressed state: the balances are hidden, or the control's
    /// sheet is open.
    pub fn pressed(mut self, pressed: bool) -> Self {
        self.pressed = pressed;
        self
    }

    /// Mute the glyph, as `.quiet` does under a Cube's unlock face.
    pub fn muted(mut self, muted: bool) -> Self {
        self.muted = muted;
        self
    }
}

impl<'a, M: Clone + 'static> From<IconAction<'a, M>> for Element<'a, M> {
    fn from(action: IconAction<'a, M>) -> Self {
        let IconAction {
            id,
            glyph,
            label,
            focus_intent,
            on_press,
            pressed,
            muted,
        } = action;
        let muted = muted || on_press.is_none();
        let content = Container::new(mark(glyph, GLYPH, move |theme| ink(theme, muted)))
            .center_x(Length::Fill)
            .center_y(Length::Fill);
        let control = Action::new(id, content, focus_intent)
            .width(Length::Fixed(ICON_BUTTON))
            .height(Length::Fixed(ICON_BUTTON))
            .padding(0.0)
            .style(move |theme, status| button_style(theme, status, pressed))
            .focus_radius(RADIUS)
            .on_press_maybe(on_press);
        tooltip(
            AccessibleLabel::new(control, label.clone()),
            tip(label),
            tooltip::Position::Bottom,
        )
        .into()
    }
}

/// An icon-only control drawn at rest, for a bar that takes no input (the
/// Launcher's bar behind a Cube's unlock face). It keeps its accessible name.
pub fn resting_icon<'a, M: 'a>(
    glyph: &'static str,
    label: impl Into<Cow<'a, str>>,
) -> Element<'a, M> {
    let icon = Container::new(mark(glyph, GLYPH, |theme| ink(theme, true)))
        .width(Length::Fixed(ICON_BUTTON))
        .height(Length::Fixed(ICON_BUTTON))
        .center_x(Length::Fixed(ICON_BUTTON))
        .center_y(Length::Fixed(ICON_BUTTON));
    AccessibleLabel::new(icon, localized(label.into())).into()
}

/// An icon's accessible name and tooltip in the accepted language. Callers
/// pass the English name, which tests and focus logic match on.
fn localized(label: Cow<'_, str>) -> Cow<'_, str> {
    let spanish = crate::locale::copy(&label);
    if spanish != label.as_ref() {
        return Cow::Owned(spanish.to_owned());
    }
    label
}

fn tip<'a, M: 'a>(label: Cow<'a, str>) -> Element<'a, M> {
    Container::new(text(label).size(12).style(|theme: &Theme| text::Style {
        color: Some(theme.colors.shell.muted),
    }))
    .padding([6, 10])
    .style(crate::theme::container::shell_menu)
    .into()
}

/// Gives a control `label` as its accessible name (see [`AccessibleLabel`]),
/// for a control whose visible text doesn't say everything, such as one
/// whose state is shown by a light.
pub fn accessible<'a, M: 'a>(
    content: impl Into<Element<'a, M>>,
    label: impl Into<Cow<'a, str>>,
) -> Element<'a, M> {
    AccessibleLabel::new(content, label.into()).into()
}

/// The accessible label of an icon-only control: exposed to widget
/// operations as text, as an `aria-label` is, and never drawn.
///
/// Iced has no accessibility tree, and a tooltip's text is not part of its
/// operations. This reports `label` over the control's bounds and otherwise
/// is the control: same tree, size, layout, focus, input and drawing.
struct AccessibleLabel<'a, M> {
    content: Element<'a, M>,
    label: Cow<'a, str>,
}

impl<'a, M> AccessibleLabel<'a, M> {
    fn new(content: impl Into<Element<'a, M>>, label: Cow<'a, str>) -> Self {
        Self {
            content: content.into(),
            label,
        }
    }
}

impl<M> Widget<M, Theme, Renderer> for AccessibleLabel<'_, M> {
    fn tag(&self) -> tree::Tag {
        self.content.as_widget().tag()
    }

    fn state(&self) -> tree::State {
        self.content.as_widget().state()
    }

    fn children(&self) -> Vec<Tree> {
        self.content.as_widget().children()
    }

    fn diff(&self, tree: &mut Tree) {
        self.content.as_widget().diff(tree);
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content.as_widget_mut().layout(tree, renderer, limits)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        operation.text(None, layout.bounds(), &self.label);
        self.content
            .as_widget_mut()
            .operate(tree, layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        viewport: &Rectangle,
    ) {
        self.content.as_widget_mut().update(
            tree, event, layout, cursor, renderer, clipboard, shell, viewport,
        );
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
        self.content
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.content
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, M, Theme, Renderer>> {
        self.content
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation)
    }
}

impl<'a, M: 'a> From<AccessibleLabel<'a, M>> for Element<'a, M> {
    fn from(label: AccessibleLabel<'a, M>) -> Self {
        Self::new(label)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::advanced::renderer::Headless;
    use iced::advanced::widget::operation::{focusable, Focusable, Outcome};

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Message {
        Focus(Id),
        Brand,
        Eye,
        Chip(u8),
        Preferences,
    }

    const WIDTH: f32 = 1024.0;

    fn renderer() -> Renderer {
        for bytes in font::load() {
            iced::advanced::graphics::text::font_system()
                .write()
                .unwrap()
                .load_font(bytes);
        }
        iced::futures::executor::block_on(<Renderer as Headless>::new(
            font::REGULAR,
            14.into(),
            Some("tiny-skia"),
        ))
        .unwrap()
    }

    fn chip(index: u8) -> Element<'static, Message> {
        Action::new(
            format!("chip.{index}"),
            text(format!("Chip {index}")),
            Message::Focus,
        )
        .on_press(Message::Chip(index))
        .into()
    }

    fn bar() -> Element<'static, Message> {
        top_bar(
            brand_action("brand", Message::Focus, Message::Brand),
            preferences("preferences", Message::Focus, Message::Preferences),
        )
        .lead(text("Lead"))
        .lead(None::<Element<'static, Message>>)
        .eye(icon_action("eye", EYE_OPEN, "Hide balances", Message::Focus).on_press(Message::Eye))
        .chip(chip(1))
        .chip(chip(2))
        .into()
    }

    #[derive(Default)]
    struct Probe {
        focus: Vec<(Id, Rectangle)>,
        text: Vec<(String, Rectangle)>,
        focused: Option<Id>,
    }

    impl Operation for Probe {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            operate(self);
        }

        fn focusable(&mut self, id: Option<&Id>, bounds: Rectangle, state: &mut dyn Focusable) {
            if let Some(id) = id {
                self.focus.push((id.clone(), bounds));
                if state.is_focused() {
                    self.focused = Some(id.clone());
                }
            }
        }

        fn text(&mut self, _id: Option<&Id>, bounds: Rectangle, text: &str) {
            self.text.push((text.to_owned(), bounds));
        }
    }

    struct Laid {
        element: Element<'static, Message>,
        tree: Tree,
        node: layout::Node,
        renderer: Renderer,
    }

    impl Laid {
        fn new() -> Self {
            let renderer = renderer();
            let mut element = bar();
            let mut tree = Tree::new(element.as_widget());
            let node = element.as_widget_mut().layout(
                &mut tree,
                &renderer,
                &layout::Limits::new(Size::ZERO, Size::new(WIDTH, 400.0)),
            );
            Self {
                element,
                tree,
                node,
                renderer,
            }
        }

        fn probe(&mut self) -> Probe {
            let mut probe = Probe::default();
            self.element.as_widget_mut().operate(
                &mut self.tree,
                Layout::new(&self.node),
                &self.renderer,
                &mut probe,
            );
            probe
        }

        fn run<T: 'static>(&mut self, operation: impl Operation<T> + 'static) {
            let mut operation: Box<dyn Operation<T>> = Box::new(operation);
            loop {
                self.element.as_widget_mut().operate(
                    &mut self.tree,
                    Layout::new(&self.node),
                    &self.renderer,
                    &mut iced::advanced::widget::operation::black_box(operation.as_mut()),
                );
                match operation.finish() {
                    Outcome::Chain(next) => operation = next,
                    _ => return,
                }
            }
        }

        fn send(&mut self, event: Event, cursor: mouse::Cursor) -> Vec<Message> {
            let mut messages = Vec::new();
            self.element.as_widget_mut().update(
                &mut self.tree,
                &event,
                Layout::new(&self.node),
                cursor,
                &self.renderer,
                &mut iced::advanced::clipboard::Null,
                &mut Shell::new(&mut messages),
                &Rectangle::with_size(Size::new(WIDTH, HEIGHT)),
            );
            messages
        }
    }

    fn bounds(probe: &Probe, id: &'static str) -> Rectangle {
        probe
            .focus
            .iter()
            .find(|(target, _)| target == &Id::new(id))
            .map(|(_, bounds)| *bounds)
            .unwrap_or_else(|| panic!("{id} is focusable"))
    }

    #[test]
    fn the_bar_is_64px_with_a_24px_gutter() {
        let mut laid = Laid::new();
        assert_eq!(laid.node.bounds().height, HEIGHT);
        assert_eq!(laid.node.bounds().width, WIDTH);
        let probe = laid.probe();
        let brand = bounds(&probe, "brand");
        let preferences = bounds(&probe, "preferences");
        assert_eq!(brand.x, GUTTER);
        assert_eq!(preferences.x + preferences.width, WIDTH - GUTTER);
        for (id, control) in &probe.focus {
            // Every control is centred in the bar.
            let middle = control.y + control.height / 2.0;
            assert!((middle - HEIGHT / 2.0).abs() <= 0.5, "{id:?} {control:?}");
        }
        assert_eq!(
            (preferences.width, preferences.height),
            (ICON_BUTTON, ICON_BUTTON)
        );
    }

    #[test]
    fn the_right_group_is_eye_then_chips_then_preferences() {
        let mut laid = Laid::new();
        let probe = laid.probe();
        let order: Vec<Id> = probe.focus.iter().map(|(id, _)| id.clone()).collect();
        let expected = ["brand", "eye", "chip.1", "chip.2", "preferences"];
        assert_eq!(order, expected.map(Id::new).to_vec(), "focus order");
        let xs: Vec<f32> = expected.iter().map(|id| bounds(&probe, id).x).collect();
        assert!(xs.windows(2).all(|pair| pair[0] < pair[1]), "{xs:?}");
        // The lead stays on the left; the right group is packed to the gutter.
        let lead = probe
            .text
            .iter()
            .find(|(value, _)| value == "Lead")
            .unwrap()
            .1;
        assert!(lead.x < bounds(&probe, "eye").x);
        // Neighbours are far enough apart for one control's 4px focus ring.
        for pair in expected[1..].windows(2) {
            let (left, right) = (bounds(&probe, pair[0]), bounds(&probe, pair[1]));
            assert!(right.x - (left.x + left.width) >= 2.0 * 4.0, "{pair:?}");
        }

        laid.run(focusable::focus::<()>(Id::new("eye")));
        for next in ["chip.1", "chip.2", "preferences"] {
            laid.run(focusable::focus_next::<()>());
            assert_eq!(laid.probe().focused, Some(Id::new(next)));
        }
    }

    #[test]
    fn icon_controls_expose_their_label_and_stay_focusable_and_pressable() {
        let mut laid = Laid::new();
        let probe = laid.probe();
        for (id, label) in [("eye", "Hide balances"), ("preferences", APP_PREFERENCES)] {
            let control = bounds(&probe, id);
            let named = probe
                .text
                .iter()
                .find(|(value, _)| value == label)
                .unwrap_or_else(|| panic!("{label:?} is exposed"));
            // Over the control itself, and the control's size is unchanged.
            assert_eq!(named.1, control, "{label}");
            assert_eq!((control.width, control.height), (ICON_BUTTON, ICON_BUTTON));
        }

        let preferences = bounds(&probe, "preferences");
        let cursor = mouse::Cursor::Available(preferences.center());
        laid.send(
            Event::Mouse(mouse::Event::CursorMoved {
                position: preferences.center(),
            }),
            cursor,
        );
        let pressed = laid.send(
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            cursor,
        );
        assert_eq!(pressed, vec![Message::Focus(Id::new("preferences"))]);
        let released = laid.send(
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
            cursor,
        );
        assert_eq!(released, vec![Message::Preferences]);
        assert_eq!(laid.probe().focused, Some(Id::new("preferences")));

        let brand = bounds(&probe, "brand");
        let cursor = mouse::Cursor::Available(brand.center());
        laid.send(
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            cursor,
        );
        let released = laid.send(
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
            cursor,
        );
        assert_eq!(released, vec![Message::Brand]);
    }

    /// `.brand:hover` tints the whole mark. The pointer rests on the word, so
    /// the cube's own SVG hover is idle: only the button's hover can tint it.
    #[test]
    fn hovering_the_brand_word_tints_the_cube_too() {
        let theme = Theme::dark();
        let accent = theme.colors.shell.accent_ink;
        // Warm pixels (red well above blue) are the accent; the ink is grey.
        assert!(accent.r - accent.b > 0.3, "{accent:?}");
        let warm_cube_pixels = |hovered: bool| -> usize {
            let mut renderer = renderer();
            let size = Size::new(120.0, 40.0);
            let mut element: Element<'static, Message> =
                Container::new(brand_action("brand", Message::Focus, Message::Brand))
                    .padding(10)
                    .into();
            let mut tree = Tree::new(element.as_widget());
            let node = element.as_widget_mut().layout(
                &mut tree,
                &renderer,
                &layout::Limits::new(Size::ZERO, size),
            );
            // The cube is the 20px square at the padding; the word follows
            // after an 8px gap.
            let cursor = if hovered {
                mouse::Cursor::Available(iced::Point::new(60.0, 20.0))
            } else {
                mouse::Cursor::Unavailable
            };
            let viewport = Rectangle::with_size(size);
            for event in [
                Event::Mouse(mouse::Event::CursorMoved {
                    position: iced::Point::new(60.0, 20.0),
                }),
                Event::Window(iced::window::Event::RedrawRequested(
                    std::time::Instant::now(),
                )),
            ] {
                element.as_widget_mut().update(
                    &mut tree,
                    &event,
                    Layout::new(&node),
                    cursor,
                    &renderer,
                    &mut iced::advanced::clipboard::Null,
                    &mut Shell::new(&mut Vec::new()),
                    &viewport,
                );
            }
            element.as_widget().draw(
                &tree,
                &mut renderer,
                &theme,
                &renderer::Style {
                    text_color: theme.colors.shell.ink,
                },
                Layout::new(&node),
                cursor,
                &viewport,
            );
            let pixels = iced::advanced::renderer::Headless::screenshot(
                &mut renderer,
                Size::new(120, 40),
                1.0,
                theme.colors.shell.rail,
            );
            let mut warm = 0;
            for y in 10..30 {
                for x in 10..30 {
                    let at = (y * 120 + x) * 4;
                    let (r, b) = (i32::from(pixels[at]), i32::from(pixels[at + 2]));
                    if r - b > 60 {
                        warm += 1;
                    }
                }
            }
            warm
        };
        assert_eq!(warm_cube_pixels(false), 0);
        assert!(warm_cube_pixels(true) > 10);
    }

    #[test]
    fn an_icon_control_without_an_intent_leaves_the_tab_order_but_keeps_its_label() {
        let renderer = renderer();
        let mut element: Element<'static, Message> = Row::new()
            .push(icon_action(
                "eye",
                EYE_SHUT,
                "Saving balance privacy…",
                Message::Focus,
            ))
            .push(resting_icon(SLIDERS, APP_PREFERENCES))
            .into();
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(Size::ZERO, Size::new(200.0, HEIGHT)),
        );
        let mut probe = Probe::default();
        element
            .as_widget_mut()
            .operate(&mut tree, Layout::new(&node), &renderer, &mut probe);
        assert!(probe.focus.is_empty());
        let labels: Vec<&str> = probe.text.iter().map(|(value, _)| value.as_str()).collect();
        assert_eq!(labels, ["Saving balance privacy…", APP_PREFERENCES]);
    }
}
