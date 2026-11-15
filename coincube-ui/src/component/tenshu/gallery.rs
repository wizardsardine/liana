//! Headless gallery and behaviour tests for the Tenshu components.
//!
//! Every capture is real Iced output from the tiny-skia renderer. Set
//! `TENSHU_GALLERY_DIR` to write them as PNGs: an Overview composition and a
//! component sheet in both themes at 1024×768 and 1440×900, plus a state
//! matrix (active, hover, pressed, disabled, focused) per theme. All data is
//! synthetic.

use std::collections::HashSet;
use std::sync::Once;

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer::{self as core_renderer, Headless};
use iced::advanced::widget::operation::{self, focusable, scrollable, Focusable, Outcome};
use iced::advanced::widget::{Id, Operation, Tree};
use iced::advanced::{clipboard, Shell};
use iced::keyboard::{self, key};
use iced::widget::button::Status;
use iced::{
    alignment, mouse, Background, Border, Color, Event, Length, Point, Rectangle, Size, Vector,
};

use super::*;
use crate::font;
use crate::icon;
use crate::image;
use crate::theme::tenshu::button::{self as button_style, ActionShape};
use crate::theme::tenshu::{self as tokens, size, space};
use crate::theme::Theme;
use crate::widget::{Column, Container, Element, Renderer, Row, Scrollable};

mod choices;
mod controls;

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Focus(Id),
    Pressed(&'static str),
    Toggled(&'static str, bool),
    Selected(&'static str, u8),
    Input(&'static str, String),
    Submitted(&'static str),
    Cleared(&'static str),
}

const WINDOWS: [(u32, u32); 2] = [(1024, 768), (1440, 900)];

fn themes() -> [(&'static str, Theme); 2] {
    [("dark", Theme::dark()), ("light", Theme::light())]
}

fn load_fonts() {
    static FONTS: Once = Once::new();
    FONTS.call_once(|| {
        for bytes in font::load() {
            iced::advanced::graphics::text::font_system()
                .write()
                .unwrap()
                .load_font(bytes);
        }
    });
}

/// A laid-out element with its own headless renderer.
struct Mounted {
    element: Element<'static, Msg>,
    tree: Tree,
    node: layout::Node,
    renderer: Renderer,
    size: Size,
}

impl Mounted {
    fn new(element: impl Into<Element<'static, Msg>>, size: Size) -> Self {
        load_fonts();
        let renderer = iced::futures::executor::block_on(<Renderer as Headless>::new(
            font::REGULAR,
            14.into(),
            Some("tiny-skia"),
        ))
        .unwrap();
        let mut element = element.into();
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(Size::ZERO, size),
        );
        Self {
            element,
            tree,
            node,
            renderer,
            size,
        }
    }

    /// Replace the element in place, as a rebuilt view does each frame.
    fn rebuild(&mut self, element: impl Into<Element<'static, Msg>>) {
        self.element = element.into();
        self.tree.diff(self.element.as_widget());
        self.node = self.element.as_widget_mut().layout(
            &mut self.tree,
            &self.renderer,
            &layout::Limits::new(Size::ZERO, self.size),
        );
    }

    fn run<T: 'static>(&mut self, operation: impl Operation<T> + 'static) -> Option<T> {
        let mut operation: Box<dyn Operation<T>> = Box::new(operation);
        loop {
            self.element.as_widget_mut().operate(
                &mut self.tree,
                Layout::new(&self.node),
                &self.renderer,
                &mut operation::black_box(operation.as_mut()),
            );
            match operation.finish() {
                Outcome::None => return None,
                Outcome::Some(result) => return Some(result),
                Outcome::Chain(next) => operation = next,
            }
        }
    }

    fn collect(&mut self) -> Collected {
        let mut collected = Collected::default();
        self.element.as_widget_mut().operate(
            &mut self.tree,
            Layout::new(&self.node),
            &self.renderer,
            &mut collected,
        );
        collected
    }

    fn texts(&mut self) -> Vec<String> {
        self.collect()
            .texts
            .into_iter()
            .map(|(_, text)| text)
            .collect()
    }

    fn focus(&mut self, id: &'static str) {
        self.run(focusable::focus::<()>(Id::new(id)));
    }

    /// Focus as a keyboard user arrives: after a key press, so the ring
    /// shows even if an earlier cell on this thread was clicked.
    fn keyboard_focus(&mut self, id: &'static str) {
        let tab = keyboard::Key::Named(keyboard::key::Named::Tab);
        self.event(
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: tab.clone(),
                modified_key: tab,
                physical_key: keyboard::key::Physical::Unidentified(
                    keyboard::key::NativeCode::Unidentified,
                ),
                location: keyboard::Location::Standard,
                modifiers: keyboard::Modifiers::empty(),
                text: None,
                repeat: false,
            }),
            mouse::Cursor::Unavailable,
        );
        self.focus(id);
    }

    fn focused(&mut self) -> Option<Id> {
        self.run(focusable::find_focused())
    }

    fn event(&mut self, event: Event, cursor: mouse::Cursor) -> Vec<Msg> {
        let mut messages = Vec::new();
        self.element.as_widget_mut().update(
            &mut self.tree,
            &event,
            Layout::new(&self.node),
            cursor,
            &self.renderer,
            &mut clipboard::Null,
            &mut Shell::new(&mut messages),
            &Rectangle::with_size(self.size),
        );
        messages
    }

    fn click(&mut self, at: Point) -> Vec<Msg> {
        let cursor = mouse::Cursor::Available(at);
        let mut messages = self.event(
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            cursor,
        );
        messages.extend(self.event(
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
            cursor,
        ));
        messages
    }

    fn key(&mut self, named: key::Named, pressed: bool, repeat: bool) -> Vec<Msg> {
        self.event(
            key_event(named, pressed, repeat),
            mouse::Cursor::Unavailable,
        )
    }

    /// Iced buttons resolve hover and pressed styles on a redraw request.
    fn redraw(&mut self, cursor: mouse::Cursor) {
        self.event(
            Event::Window(iced::window::Event::RedrawRequested(
                iced::time::Instant::now(),
            )),
            cursor,
        );
    }

    fn pixels(&mut self, theme: &Theme, cursor: mouse::Cursor) -> Vec<u8> {
        use iced::advanced::Renderer as _;
        self.redraw(cursor);
        // Start each capture from an empty frame, as Iced's compositor does;
        // otherwise a second capture of one mount keeps the first's layers.
        self.renderer.reset(Rectangle::with_size(self.size));
        self.element.as_widget().draw(
            &self.tree,
            &mut self.renderer,
            theme,
            &core_renderer::Style {
                text_color: theme.colors.shell.ink,
            },
            Layout::new(&self.node),
            cursor,
            &Rectangle::with_size(self.size),
        );
        self.renderer.screenshot(
            Size::new(self.size.width as u32, self.size.height as u32),
            1.0,
            theme.colors.shell.background,
        )
    }
}

/// Every focusable ID and every text fragment in tree order.
#[derive(Default)]
struct Collected {
    focusables: Vec<(Id, Rectangle)>,
    texts: Vec<(Rectangle, String)>,
    scrollables: Vec<(Id, Rectangle, Rectangle, Vector)>,
}

impl Operation for Collected {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }

    fn focusable(&mut self, id: Option<&Id>, bounds: Rectangle, _state: &mut dyn Focusable) {
        if let Some(id) = id {
            self.focusables.push((id.clone(), bounds));
        }
    }

    fn text(&mut self, _id: Option<&Id>, bounds: Rectangle, text: &str) {
        self.texts.push((bounds, text.to_string()));
    }

    fn scrollable(
        &mut self,
        id: Option<&Id>,
        bounds: Rectangle,
        content_bounds: Rectangle,
        translation: Vector,
        _state: &mut dyn scrollable::Scrollable,
    ) {
        if let Some(id) = id {
            self.scrollables
                .push((id.clone(), bounds, content_bounds, translation));
        }
    }
}

fn key_event(named: key::Named, pressed: bool, repeat: bool) -> Event {
    let key = keyboard::Key::Named(named);
    let physical_key = key::Physical::Unidentified(key::NativeCode::Unidentified);
    if pressed {
        Event::Keyboard(keyboard::Event::KeyPressed {
            key: key.clone(),
            modified_key: key,
            physical_key,
            location: keyboard::Location::Standard,
            modifiers: keyboard::Modifiers::empty(),
            text: None,
            repeat,
        })
    } else {
        Event::Keyboard(keyboard::Event::KeyReleased {
            key: key.clone(),
            modified_key: key,
            physical_key,
            location: keyboard::Location::Standard,
            modifiers: keyboard::Modifiers::empty(),
        })
    }
}

fn press(name: &'static str, enabled: bool) -> Option<Msg> {
    enabled.then_some(Msg::Pressed(name))
}

/// Every actionable control in the API, enabled, with a stable ID each.
fn every_action(enabled: bool) -> Vec<(&'static str, Element<'static, Msg>)> {
    use ActionKind::*;
    let pill = |id: &'static str, label: &'static str, kind| -> Element<'static, Msg> {
        ActionButton::new(id, label, kind, Msg::Focus)
            .on_press_maybe(press(id, enabled))
            .into()
    };
    let compact = |id: &'static str, label: &'static str, kind| -> Element<'static, Msg> {
        ActionButton::new(id, label, kind, Msg::Focus)
            .compact()
            .on_press_maybe(press(id, enabled))
            .into()
    };
    vec![
        ("send", pill("send", "Send", Primary)),
        ("receive", pill("receive", "Receive", Secondary)),
        ("recovery", pill("recovery", "Recovery path", Quiet)),
        ("remove", pill("remove", "Remove device", Destructive)),
        ("save", compact("save", "Save changes", Primary)),
        ("discard", compact("discard", "Discard", Secondary)),
        ("delete", compact("delete", "Delete Cube", Destructive)),
        (
            "eye",
            eye_action(
                "eye",
                AmountVisibility::Shown,
                Msg::Focus,
                press("eye", enabled),
            ),
        ),
        (
            "transfer",
            transfer_action("transfer", Msg::Focus, press("transfer", enabled)).into(),
        ),
        (
            "account",
            chip_action(
                "account",
                "alex@example.test",
                Msg::Focus,
                press("account", enabled),
            )
            .into(),
        ),
    ]
}

fn column_of(controls: Vec<(&'static str, Element<'static, Msg>)>) -> Element<'static, Msg> {
    controls
        .into_iter()
        .fold(
            Column::new().spacing(12).padding(12),
            |column, (_, control)| column.push(control),
        )
        .into()
}

#[test]
fn every_action_emits_its_intent_once_per_pointer_or_key_press() {
    let names: Vec<_> = every_action(true)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    let mut mounted = Mounted::new(column_of(every_action(true)), Size::new(420.0, 640.0));
    let focusables = mounted.collect().focusables;
    assert_eq!(
        focusables
            .iter()
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>(),
        names.iter().map(|name| Id::new(name)).collect::<Vec<_>>(),
        "every enabled action is a focus stop, in visual order"
    );

    for (name, (id, bounds)) in names.iter().copied().zip(focusables) {
        let messages = mounted.click(bounds.center());
        assert_eq!(
            messages,
            vec![Msg::Focus(id.clone()), Msg::Pressed(name)],
            "{name}: a click requests whole-tree focus, then emits once"
        );

        mounted.focus(name);
        assert_eq!(mounted.focused(), Some(id));
        assert_eq!(
            mounted.key(key::Named::Enter, true, false),
            vec![Msg::Pressed(name)],
            "{name}: Enter activates once"
        );
        assert!(
            mounted.key(key::Named::Enter, true, true).is_empty(),
            "{name}: a held Enter does not repeat"
        );
        assert!(mounted.key(key::Named::Enter, false, false).is_empty());
        assert_eq!(
            mounted.key(key::Named::Space, true, false),
            vec![Msg::Pressed(name)],
            "{name}: Space activates once"
        );
        assert!(mounted.key(key::Named::Space, false, false).is_empty());
    }
}

#[test]
fn disabled_actions_are_inert_and_skipped_by_tab() {
    let disabled = every_action(false);
    let count = disabled.len();
    let mut controls: Vec<(&'static str, Element<'static, Msg>)> = vec![(
        "first",
        ActionButton::new("first", "First", ActionKind::Primary, Msg::Focus)
            .on_press(Msg::Pressed("first"))
            .into(),
    )];
    controls.extend(disabled);
    controls.push((
        "last",
        ActionButton::new("last", "Last", ActionKind::Secondary, Msg::Focus)
            .on_press(Msg::Pressed("last"))
            .into(),
    ));
    let mut mounted = Mounted::new(column_of(controls), Size::new(420.0, 760.0));

    assert_eq!(
        mounted
            .collect()
            .focusables
            .into_iter()
            .map(|(id, _)| id)
            .collect::<Vec<_>>(),
        vec![Id::new("first"), Id::new("last")],
        "disabled actions register no focus stop"
    );
    mounted.run(focusable::focus_next::<()>());
    assert_eq!(mounted.focused(), Some(Id::new("first")));
    mounted.run(focusable::focus_next::<()>());
    assert_eq!(mounted.focused(), Some(Id::new("last")));

    for index in 1..=count {
        let bounds = mounted.node.children()[index].bounds();
        assert!(
            mounted.click(bounds.center()).is_empty(),
            "disabled control {index} must not emit focus or press intents"
        );
        assert!(mounted.key(key::Named::Enter, true, false).is_empty());
        assert!(mounted.key(key::Named::Enter, false, false).is_empty());
    }
}

const AMOUNTS: [&str; 6] = [
    "123,456",
    "0.00123456",
    "$41.70",
    "41.70",
    "5.00 USDt",
    "20.74 USD",
];

fn synthetic_values() -> Vec<BalanceValue<'static>> {
    vec![
        BalanceValue::Known {
            primary: Figure::new("$41.70"),
            secondary: Some(Figure::new("123,456").unit("SATS")),
        },
        BalanceValue::Known {
            primary: Figure::new("0.00123456").unit("BTC"),
            secondary: Some(Figure::new("20.74 USD")),
        },
        BalanceValue::Known {
            primary: Figure::new("5.00 USDt"),
            secondary: None,
        },
        BalanceValue::Zero {
            primary: Figure::new("$0.00"),
            secondary: Some(Figure::new("0").unit("SATS")),
        },
        BalanceValue::Pending {
            label: "Receiving 123,456 SATS".into(),
        },
        BalanceValue::Unavailable {
            reason: "Last known 0.00123456 BTC".into(),
        },
        BalanceValue::Masked,
    ]
}

fn coin() -> Element<'static, Msg> {
    image::btc_logo()
        .width(Length::Fixed(size::COIN))
        .height(Length::Fixed(size::COIN))
        .into()
}

fn send_receive(prefix: &'static str) -> Vec<Element<'static, Msg>> {
    vec![
        ActionButton::new(
            Id::from(format!("{prefix}-send")),
            "Send",
            ActionKind::Primary,
            Msg::Focus,
        )
        .on_press(Msg::Pressed("send"))
        .into(),
        ActionButton::new(
            Id::from(format!("{prefix}-receive")),
            "Receive",
            ActionKind::Secondary,
            Msg::Focus,
        )
        .on_press(Msg::Pressed("receive"))
        .into(),
    ]
}

#[test]
fn hidden_and_unresolved_amounts_never_enter_the_tree() {
    for visibility in [AmountVisibility::Hidden, AmountVisibility::Unresolved] {
        for tier in [BalanceTier::Hero, BalanceTier::Card, BalanceTier::Row] {
            for value in synthetic_values() {
                let shown = format!("{value:?}");
                let mut mounted =
                    Mounted::new(balance(value, tier, visibility), Size::new(640.0, 200.0));
                let texts = mounted.texts();
                assert!(!texts.is_empty());
                for text in &texts {
                    assert_eq!(
                        text, MASK,
                        "{visibility:?} {tier:?} drew {text:?} for {shown}; only the mask may appear"
                    );
                }
            }
        }

        let mut row = Mounted::new(
            asset_row(
                coin(),
                "BTC",
                synthetic_values().remove(1),
                visibility,
                send_receive("row"),
            ),
            Size::new(760.0, 80.0),
        );
        let texts = row.texts();
        assert!(texts.contains(&"BTC".to_string()));
        assert!(texts.contains(&MASK.to_string()));
        assert!(texts.contains(&"Send".to_string()) && texts.contains(&"Receive".to_string()));
        for text in &texts {
            assert!(
                !text.chars().any(|c| c.is_ascii_digit()),
                "{visibility:?} asset row leaked {text:?}"
            );
        }
    }
}

#[test]
fn rebuilding_with_hidden_visibility_removes_shown_amounts() {
    let value = || synthetic_values().remove(0);
    let mut mounted = Mounted::new(
        balance(value(), BalanceTier::Hero, AmountVisibility::Shown),
        Size::new(640.0, 200.0),
    );
    let texts = mounted.texts().join(" ");
    assert!(texts.contains("$41.70") && texts.contains("123,456"));

    mounted.rebuild(balance(
        value(),
        BalanceTier::Hero,
        AmountVisibility::Hidden,
    ));
    let texts = mounted.texts();
    assert!(texts.iter().all(|text| text == MASK), "{texts:?}");
    assert!(AMOUNTS
        .iter()
        .all(|amount| texts.iter().all(|text| !text.contains(amount))));
}

#[test]
fn shown_balances_render_each_state_in_the_artboard_grammar() {
    let shown = AmountVisibility::Shown;
    let texts =
        |value, tier| Mounted::new(balance(value, tier, shown), Size::new(640.0, 200.0)).texts();
    let values = synthetic_values();

    assert_eq!(
        texts(values[0].clone(), BalanceTier::Hero),
        vec!["$41.70", "123,456", "SATS"]
    );
    assert_eq!(
        texts(values[1].clone(), BalanceTier::Row),
        vec!["0.00123456", "BTC", "20.74 USD"]
    );
    assert_eq!(
        texts(values[3].clone(), BalanceTier::Card),
        vec!["$0.00", "0", "SATS"]
    );
    let pending = texts(values[4].clone(), BalanceTier::Card);
    assert_eq!(pending[0], "—");
    assert!(pending.contains(&"Receiving 123,456 SATS".to_string()));
    let unavailable = texts(values[5].clone(), BalanceTier::Row);
    assert_eq!(unavailable[0], "—");
    assert!(unavailable.contains(&"Last known 0.00123456 BTC".to_string()));
    assert_eq!(texts(BalanceValue::Masked, BalanceTier::Row), vec![MASK]);
}

#[test]
fn eye_glyph_reflects_visibility_and_holds_no_state() {
    let glyph = |text: crate::widget::Text<'static>| {
        Mounted::new(text, Size::new(40.0, 40.0)).texts().remove(0)
    };
    let open = glyph(icon::eye_icon());
    let slashed = glyph(icon::eye_slash_icon());
    assert_ne!(open, slashed);
    for (visibility, expected) in [
        (AmountVisibility::Shown, &open),
        (AmountVisibility::Hidden, &slashed),
        (AmountVisibility::Unresolved, &slashed),
    ] {
        let mut mounted = Mounted::new(
            eye_action("eye", visibility, Msg::Focus, Some(Msg::Pressed("eye"))),
            Size::new(48.0, 48.0),
        );
        assert_eq!(&mounted.texts().remove(0), expected, "{visibility:?}");
        let bounds = mounted.collect().focusables[0].1;
        mounted.click(bounds.center());
        assert_eq!(
            &mounted.texts().remove(0),
            expected,
            "{visibility:?}: pressing emits the intent but never flips the glyph locally"
        );
    }
    assert_eq!(
        AmountVisibility::from_hidden_preference(Some(false)),
        AmountVisibility::Shown
    );
    assert_eq!(
        AmountVisibility::from_hidden_preference(Some(true)),
        AmountVisibility::Hidden
    );
    assert_eq!(
        AmountVisibility::from_hidden_preference(None),
        AmountVisibility::Unresolved
    );
}

#[test]
fn geometry_matches_the_v113_artboards() {
    let bounds = |element: Element<'static, Msg>| {
        Mounted::new(Container::new(element).padding(10), Size::new(800.0, 200.0))
            .node
            .children()[0]
            .bounds()
    };
    let enabled = Some(Msg::Pressed("x"));
    let pill = bounds(
        ActionButton::new("p", "Send", ActionKind::Primary, Msg::Focus)
            .on_press_maybe(enabled.clone())
            .into(),
    );
    assert_eq!(pill.height, size::PILL_HEIGHT);
    let compact = bounds(
        ActionButton::new("c", "Discard", ActionKind::Secondary, Msg::Focus)
            .compact()
            .on_press_maybe(enabled.clone())
            .into(),
    );
    assert_eq!(compact.height, size::BUTTON_HEIGHT);
    let chip = bounds(chip_action("k", "alex@example.test", Msg::Focus, enabled.clone()).into());
    assert_eq!(chip.height, size::CHIP_HEIGHT);
    let status = bounds(status_chip(None, "Connected", Tone::Good).into());
    assert_eq!(status.height, size::CHIP_HEIGHT);
    let eye = bounds(eye_action(
        "e",
        AmountVisibility::Shown,
        Msg::Focus,
        enabled,
    ));
    assert_eq!(
        (eye.width, eye.height),
        (size::ICON_ACTION, size::ICON_ACTION)
    );
    let row = bounds(asset_row(
        coin(),
        "BTC",
        synthetic_values().remove(1),
        AmountVisibility::Shown,
        send_receive("geo"),
    ));
    assert_eq!(row.height, size::ASSET_ROW_HEIGHT);

    let mounted = Mounted::new(
        card(Column::new().push(text::body("Spark"))),
        Size::new(400.0, 200.0),
    );
    let content = mounted.node.children()[0].bounds();
    assert_eq!((content.x, content.y), (space::L, space::L));

    let theme = Theme::dark();
    for (shape, kind, radius) in [
        (ActionShape::Pill, ActionKind::Primary, tokens::radius::PILL),
        (ActionShape::Pill, ActionKind::Quiet, tokens::radius::LINK),
        (
            ActionShape::Compact,
            ActionKind::Secondary,
            tokens::radius::CONTROL,
        ),
        (
            ActionShape::Chip,
            ActionKind::Secondary,
            tokens::radius::CHIP,
        ),
        (
            ActionShape::Icon,
            ActionKind::Secondary,
            tokens::radius::CONTROL,
        ),
    ] {
        let style = button_style::action(&theme, Status::Active, kind, shape);
        assert_eq!(style.border.radius.top_left, radius, "{shape:?} {kind:?}");
    }
    assert_eq!(
        tokens::surface::card(&theme).border.radius.top_left,
        tokens::radius::CARD
    );
    assert_eq!(
        tokens::surface::panel(&theme).border.radius.top_left,
        tokens::radius::MENU
    );
}

#[test]
fn every_button_shape_is_square_cornered() {
    // Canvas v122: buttons use `.btn`'s 8px corner at most; no pills.
    let theme = Theme::dark();
    for shape in [
        ActionShape::Pill,
        ActionShape::Compact,
        ActionShape::Chip,
        ActionShape::Icon,
    ] {
        for kind in [
            ActionKind::Primary,
            ActionKind::Secondary,
            ActionKind::Quiet,
            ActionKind::Destructive,
        ] {
            for status in [
                Status::Active,
                Status::Hovered,
                Status::Pressed,
                Status::Disabled,
            ] {
                let corner = button_style::action(&theme, status, kind, shape)
                    .border
                    .radius
                    .top_left;
                assert!(
                    corner <= tokens::radius::CONTROL,
                    "{:?} {:?} {:?} has a {}px corner",
                    shape,
                    kind,
                    status,
                    corner
                );
            }
        }
    }
    let chip = tokens::chip::chip(&theme).border.radius.top_left;
    assert!(chip <= tokens::radius::CONTROL, "status chip {}px", chip);
    let choice = tokens::control::choice_chip(&theme, Status::Active, false)
        .border
        .radius
        .top_left;
    assert!(
        choice <= tokens::radius::CONTROL,
        "choice chip {}px",
        choice
    );
}

#[test]
fn plex_digits_keep_amount_widths_stable() {
    let width = |value: &'static str| {
        let mut mounted = Mounted::new(
            balance(
                BalanceValue::Known {
                    primary: Figure::new(value).unit("SATS"),
                    secondary: None,
                },
                BalanceTier::Row,
                AmountVisibility::Shown,
            ),
            Size::new(400.0, 40.0),
        );
        mounted
            .collect()
            .texts
            .into_iter()
            .find(|(_, text)| text == value)
            .unwrap()
            .0
            .width
    };
    assert!((width("111,111") - width("888,888")).abs() < 0.5);
}

fn assert_action_state_contrast(
    name: &str,
    theme: &Theme,
    shell: crate::theme::palette::ShellPalette,
    shape: ActionShape,
    kind: ActionKind,
) {
    for status in [Status::Active, Status::Hovered, Status::Pressed] {
        let style = button_style::action(theme, status, kind, shape);
        for surface in [shell.background, shell.panel] {
            let fill = match style.background {
                Some(Background::Color(color)) => tokens::over(surface, color),
                _ => surface,
            };
            let ratio = tokens::contrast(style.text_color, fill);
            assert!(
                ratio >= 4.5,
                "{name}: {shape:?} {kind:?} {status:?} label is {ratio:.2}:1"
            );
        }
    }
}

#[test]
fn text_and_focus_colours_meet_wcag_aa() {
    use ActionKind::*;
    for (name, theme) in themes() {
        let shell = theme.colors.shell;
        for surface in [shell.background, shell.panel] {
            for (role, ink) in [
                ("ink", shell.ink),
                ("muted", shell.muted),
                ("accent ink", shell.accent_ink),
                ("good", shell.good),
                ("warn", shell.warn),
                ("bad", shell.bad),
            ] {
                let ratio = tokens::contrast(ink, surface);
                assert!(ratio >= 4.5, "{name}: {role} text is {ratio:.2}:1");
            }
            let ring = tokens::contrast(shell.focus, surface);
            assert!(ring >= 3.0, "{name}: focus ring is {ring:.2}:1");
        }

        // Every enabled action state, on a card and on the window background.
        for shape in [
            ActionShape::Pill,
            ActionShape::Compact,
            ActionShape::Chip,
            ActionShape::Icon,
        ] {
            for kind in [Primary, Secondary, Quiet, Destructive] {
                assert_action_state_contrast(name, &theme, shell, shape, kind);
            }
        }
    }
}

// ----- Captures ---------------------------------------------------------

fn save(name: &str, pixels: &[u8], width: u32, height: u32) {
    if let Ok(directory) = std::env::var("TENSHU_GALLERY_DIR") {
        ::image::save_buffer(
            std::path::Path::new(&directory).join(format!("{name}.png")),
            pixels,
            width,
            height,
            ::image::ColorType::Rgba8,
        )
        .unwrap();
    }
}

fn sample(pixels: &[u8], width: u32, x: f32, y: f32) -> [u8; 3] {
    let i = ((y as u32 * width + x as u32) * 4) as usize;
    [pixels[i], pixels[i + 1], pixels[i + 2]]
}

fn near(actual: [u8; 3], expected: Color) -> bool {
    let rgba = expected.into_rgba8();
    actual
        .iter()
        .zip(rgba)
        .all(|(actual, expected)| actual.abs_diff(expected) <= 3)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CellState {
    Active,
    Hover,
    Pressed,
    Disabled,
    Focused,
}

const STATES: [(CellState, &str); 5] = [
    (CellState::Active, "Active"),
    (CellState::Hover, "Hover"),
    (CellState::Pressed, "Pressed"),
    (CellState::Disabled, "Disabled"),
    (CellState::Focused, "Focused"),
];

#[derive(Debug, Clone, Copy)]
enum Variant {
    Pill(ActionKind, &'static str),
    Compact(ActionKind, &'static str),
    Chip,
    Eye,
}

const VARIANTS: [(&str, Variant); 9] = [
    ("Primary", Variant::Pill(ActionKind::Primary, "Send")),
    ("Secondary", Variant::Pill(ActionKind::Secondary, "Receive")),
    ("Quiet", Variant::Pill(ActionKind::Quiet, "Recovery path")),
    (
        "Destructive",
        Variant::Pill(ActionKind::Destructive, "Remove device"),
    ),
    (
        "Compact primary",
        Variant::Compact(ActionKind::Primary, "Save changes"),
    ),
    (
        "Compact secondary",
        Variant::Compact(ActionKind::Secondary, "Discard"),
    ),
    (
        "Compact destructive",
        Variant::Compact(ActionKind::Destructive, "Delete Cube"),
    ),
    ("Chip", Variant::Chip),
    ("Eye", Variant::Eye),
];

fn variant_control(variant: Variant, enabled: bool) -> Element<'static, Msg> {
    let on_press = press("cell", enabled);
    match variant {
        Variant::Pill(kind, label) => ActionButton::new("cell", label, kind, Msg::Focus)
            .on_press_maybe(on_press)
            .into(),
        Variant::Compact(kind, label) => ActionButton::new("cell", label, kind, Msg::Focus)
            .compact()
            .on_press_maybe(on_press)
            .into(),
        Variant::Chip => chip_action("cell", "alex@example.test", Msg::Focus, on_press).into(),
        Variant::Eye => eye_action("cell", AmountVisibility::Shown, Msg::Focus, on_press),
    }
}

const CELL: (u32, u32) = (190, 72);
const LABEL_WIDTH: u32 = 180;
const HEADER_HEIGHT: u32 = 32;

/// Render one control in one state; returns its pixels and control bounds.
fn render_cell(variant: Variant, state: CellState, theme: &Theme) -> (Vec<u8>, Rectangle) {
    let control = variant_control(variant, state != CellState::Disabled);
    let mut mounted = Mounted::new(
        Container::new(control)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(alignment::Horizontal::Center)
            .align_y(alignment::Vertical::Center),
        Size::new(CELL.0 as f32, CELL.1 as f32),
    );
    let bounds = mounted.node.children()[0].bounds();
    let over = mouse::Cursor::Available(bounds.center());
    let cursor = match state {
        CellState::Active | CellState::Disabled => mouse::Cursor::Unavailable,
        CellState::Focused => {
            mounted.keyboard_focus("cell");
            mouse::Cursor::Unavailable
        }
        CellState::Hover => over,
        CellState::Pressed => {
            mounted.event(
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
                over,
            );
            over
        }
    };
    (mounted.pixels(theme, cursor), bounds)
}

fn blit(
    target: &mut [u8],
    target_width: u32,
    source: &[u8],
    (width, height): (u32, u32),
    x: u32,
    y: u32,
) {
    for row in 0..height {
        let from = (row * width * 4) as usize;
        let to = (((y + row) * target_width + x) * 4) as usize;
        target[to..to + (width * 4) as usize]
            .copy_from_slice(&source[from..from + (width * 4) as usize]);
    }
}

fn assert_theme_state_matrix(name: &str, theme: &Theme, width: u32, height: u32) {
    let shell = theme.colors.shell;
    // Labels first, then each rendered cell is copied into its slot.
    let header = STATES.iter().fold(
        Row::new().push(iced::widget::Space::new().width(Length::Fixed(LABEL_WIDTH as f32))),
        |row, (_, label)| {
            row.push(
                Container::new(caption(*label))
                    .width(Length::Fixed(CELL.0 as f32))
                    .align_x(alignment::Horizontal::Center),
            )
        },
    );
    let frame = VARIANTS.iter().fold(
        Column::new().push(Container::new(header).height(Length::Fixed(HEADER_HEIGHT as f32))),
        |column, (label, _)| {
            column.push(
                Container::new(body(*label))
                    .padding([0.0, 16.0])
                    .height(Length::Fixed(CELL.1 as f32))
                    .align_y(alignment::Vertical::Center),
            )
        },
    );
    let mut mosaic = Mounted::new(frame, Size::new(width as f32, height as f32))
        .pixels(theme, mouse::Cursor::Unavailable);

    for (row, (label, variant)) in VARIANTS.iter().enumerate() {
        let mut hashes = HashSet::new();
        for (column, (state, _)) in STATES.iter().enumerate() {
            let (pixels, bounds) = render_cell(*variant, *state, theme);
            assert!(
                hashes.insert(pixels.clone()),
                "{name}: {label} {state:?} looks identical to another state"
            );
            if matches!(variant, Variant::Pill(ActionKind::Primary, _)) {
                let fill = sample(&pixels, CELL.0, bounds.x + 6.0, bounds.center_y());
                let expected = match state {
                    CellState::Active | CellState::Focused => shell.accent,
                    CellState::Hover => tokens::ACCENT_HOVER,
                    CellState::Pressed => tokens::accent_pressed(theme),
                    CellState::Disabled => tokens::over(
                        shell.background,
                        tokens::faded(shell.accent, tokens::DISABLED_ALPHA),
                    ),
                };
                assert!(
                    near(fill, expected),
                    "{name}: Primary {state:?} fill {fill:?}"
                );
            }
            if *state == CellState::Focused {
                let ring = sample(&pixels, CELL.0, bounds.center_x(), bounds.y - 3.0);
                assert!(
                    near(ring, shell.focus),
                    "{name}: {label} focus ring {ring:?}"
                );
            }
            blit(
                &mut mosaic,
                width,
                &pixels,
                CELL,
                LABEL_WIDTH + column as u32 * CELL.0,
                HEADER_HEIGHT + row as u32 * CELL.1,
            );
        }
    }
    save(&format!("tenshu-states-{name}"), &mosaic, width, height);
}

#[test]
fn state_matrix_shows_distinct_pointer_disabled_and_focus_states() {
    let width = LABEL_WIDTH + CELL.0 * STATES.len() as u32;
    let height = HEADER_HEIGHT + CELL.1 * VARIANTS.len() as u32;
    for (name, theme) in themes() {
        assert_theme_state_matrix(name, &theme, width, height);
    }
}

fn rail() -> Element<'static, Msg> {
    Container::new(Column::new())
        .width(Length::Fixed(248.0))
        .height(Length::Fill)
        .style(rail_style)
        .into()
}

fn rail_style(theme: &Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(Background::Color(theme.colors.shell.rail)),
        border: Border {
            color: theme.colors.shell.line,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    }
}

fn wallet_card(
    glyph: crate::widget::Text<'static>,
    name: &'static str,
    total: BalanceValue<'static>,
    rows: Vec<Element<'static, Msg>>,
    footer: Option<Element<'static, Msg>>,
    visibility: AmountVisibility,
) -> Element<'static, Msg> {
    let header = Row::new()
        .spacing(8)
        .align_y(iced::Alignment::Center)
        .push(glyph.size(size::ICON_SMALL).style(tokens::text::muted))
        .push(body(name).style(tokens::text::muted));
    let mut content = Column::new()
        .push(header)
        .push(
            Container::new(balance(total, BalanceTier::Card, visibility))
                .padding(iced::Padding::default().top(8.0)),
        )
        .push(iced::widget::Space::new().height(Length::Fixed(6.0)));
    for row in rows {
        content = content.push(row);
    }
    if let Some(footer) = footer {
        content = content.push(iced::widget::rule::horizontal(1)).push(footer);
    }
    card(content).into()
}

fn known(primary: &'static str, sats: &'static str) -> BalanceValue<'static> {
    BalanceValue::Known {
        primary: Figure::new(primary),
        secondary: Some(Figure::new(sats).unit("SATS")),
    }
}

/// An asset amount: `value unit`, then the fiat estimate when there is one.
fn holding(
    value: &'static str,
    unit: &'static str,
    fiat: Option<&'static str>,
) -> BalanceValue<'static> {
    let primary = Figure::new(value).unit(unit);
    let secondary = fiat.map(Figure::new);
    if value == "0" {
        BalanceValue::Zero { primary, secondary }
    } else {
        BalanceValue::Known { primary, secondary }
    }
}

fn asset(
    coin: Element<'static, Msg>,
    ticker: &'static str,
    value: BalanceValue<'static>,
    prefix: &'static str,
    visibility: AmountVisibility,
) -> Element<'static, Msg> {
    asset_row(coin, ticker, value, visibility, send_receive(prefix))
}

fn svg(logo: iced::widget::Svg<'static, Theme>) -> Element<'static, Msg> {
    logo.width(Length::Fixed(size::COIN))
        .height(Length::Fixed(size::COIN))
        .into()
}

/// The Overview body from `P-Overview`, composed only from this API.
fn overview(visibility: AmountVisibility) -> Element<'static, Msg> {
    let header = Row::new()
        .spacing(6)
        .align_y(iced::Alignment::Center)
        .push(title("Total Balance"))
        .push(eye_action(
            "eye-total",
            visibility,
            Msg::Focus,
            Some(Msg::Pressed("eye")),
        ))
        .push(iced::widget::Space::new().width(Length::Fill))
        .push(caption("Everyday · Bitcoin mainnet"));
    let wallets = Row::new()
        .spacing(6)
        .align_y(iced::Alignment::Center)
        .push(section("Wallets"))
        .push(eye_action(
            "eye-wallets",
            visibility,
            Msg::Focus,
            Some(Msg::Pressed("eye")),
        ));
    let page = Column::new()
        .spacing(12)
        .max_width(904)
        .push(header)
        .push(balance(
            known("$41.70", "50,360"),
            BalanceTier::Hero,
            visibility,
        ))
        .push(Container::new(wallets).padding(iced::Padding::default().top(24.0)))
        .push(wallet_card(
            icon::spark_icon(),
            "Spark · Daily spending",
            known("$20.74", "25,053"),
            vec![asset(
                coin(),
                "BTC",
                holding("25,053", "SATS", Some("20.74 USD")),
                "spark",
                visibility,
            )],
            None,
            visibility,
        ))
        .push(wallet_card(
            icon::droplet_icon(),
            "Liquid · Travel",
            known("$20.95", "25,307"),
            vec![
                asset(
                    svg(image::lbtc_logo()),
                    "L-BTC",
                    holding("12,784", "SATS", Some("10.58 USD")),
                    "lbtc",
                    visibility,
                ),
                asset(
                    svg(image::usdt_logo()),
                    "USDt",
                    holding("10.37", "USDt", None),
                    "usdt",
                    visibility,
                ),
            ],
            None,
            visibility,
        ))
        .push(wallet_card(
            icon::vault_icon(),
            "Vault · Family savings",
            BalanceValue::Zero {
                primary: Figure::new("$0.00"),
                secondary: Some(Figure::new("0").unit("SATS")),
            },
            vec![asset(
                coin(),
                "BTC",
                holding("0", "SATS", Some("0.00 USD")),
                "vault",
                visibility,
            )],
            Some(
                ActionButton::new("recovery", "Recovery path", ActionKind::Quiet, Msg::Focus)
                    .icon(icon::clock_icon())
                    .on_press(Msg::Pressed("recovery"))
                    .into(),
            ),
            visibility,
        ))
        .push(
            Container::new(transfer_action(
                "transfer",
                Msg::Focus,
                Some(Msg::Pressed("transfer")),
            ))
            .width(Length::Fill)
            .align_x(alignment::Horizontal::Center),
        );
    shell(page.into())
}

fn shell(page: Element<'static, Msg>) -> Element<'static, Msg> {
    Container::new(
        Row::new().push(rail()).push(
            Scrollable::new(
                Container::new(page)
                    .width(Length::Fill)
                    .padding([32.0, 48.0])
                    .align_x(alignment::Horizontal::Center),
            )
            .id("gallery-overview")
            .width(Length::Fill)
            .height(Length::Fill),
        ),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .style(tokens::surface::page)
    .into()
}

/// Every component and state on one sheet.
fn sheet() -> Element<'static, Msg> {
    use ActionKind::*;
    let pill =
        |id: &'static str, label: &'static str, kind, enabled: bool| -> Element<'static, Msg> {
            ActionButton::new(id, label, kind, Msg::Focus)
                .on_press_maybe(press(id, enabled))
                .into()
        };
    let compact =
        |id: &'static str, label: &'static str, kind, enabled: bool| -> Element<'static, Msg> {
            ActionButton::new(id, label, kind, Msg::Focus)
                .compact()
                .on_press_maybe(press(id, enabled))
                .into()
        };
    let wrap = |items: Vec<Element<'static, Msg>>| -> Element<'static, Msg> {
        items
            .into_iter()
            .fold(
                Row::new()
                    .spacing(space::S)
                    .align_y(iced::Alignment::Center),
                |row, item| row.push(item),
            )
            .wrap()
            .vertical_spacing(space::S)
            .into()
    };
    let group =
        |heading: &'static str, items: Vec<Element<'static, Msg>>| -> Element<'static, Msg> {
            items
                .into_iter()
                .fold(
                    Column::new().spacing(space::M).push(section(heading)),
                    |column, item| column.push(item),
                )
                .into()
        };

    let actions = group(
        "Actions",
        vec![
            wrap(vec![
                pill("s-send", "Send", Primary, true),
                pill("s-receive", "Receive", Secondary, true),
                pill("s-recovery", "Recovery path", Quiet, true),
                pill("s-remove", "Remove device", Destructive, true),
                transfer_action("s-transfer", Msg::Focus, Some(Msg::Pressed("s-transfer"))).into(),
            ]),
            wrap(vec![
                pill("s-send-off", "Send", Primary, false),
                pill("s-receive-off", "Receive", Secondary, false),
                eye_action(
                    "s-eye",
                    AmountVisibility::Shown,
                    Msg::Focus,
                    Some(Msg::Pressed("s-eye")),
                ),
                eye_action(
                    "s-eye-hidden",
                    AmountVisibility::Hidden,
                    Msg::Focus,
                    Some(Msg::Pressed("s-eye")),
                ),
            ]),
            wrap(vec![
                compact("s-save", "Save changes", Primary, true),
                compact("s-discard", "Discard", Secondary, true),
                compact("s-delete", "Delete Cube", Destructive, true),
                compact("s-save-off", "Save changes", Primary, false),
            ]),
        ],
    );
    let chips = group(
        "Chips and badges",
        vec![
            wrap(vec![
                status_chip(None, "Signed in", Tone::Good)
                    .icon(icon::person_icon())
                    .state_icon(icon::check_circle_icon())
                    .into(),
                status_chip(None, "Connected", Tone::Good)
                    .icon(icon::link_icon())
                    .state_icon(icon::check_circle_icon())
                    .into(),
                status_chip(None, "Reconnect required", Tone::Warn)
                    .state_icon(icon::warning_icon())
                    .into(),
                status_chip(Some("Connect ·"), "Signed out", Tone::Muted).into(),
                chip_action(
                    "s-chip",
                    "alex@example.test",
                    Msg::Focus,
                    Some(Msg::Pressed("s-chip")),
                )
                .icon(icon::person_icon())
                .into(),
            ]),
            wrap(vec![
                status_badge("Confirmed", Tone::Good)
                    .icon(icon::check_circle_icon())
                    .into(),
                status_badge("Pending", Tone::Warn)
                    .icon(icon::clock_icon())
                    .into(),
                status_badge("Failed", Tone::Bad).into(),
                status_badge("1 of 2 signatures", Tone::Warn).into(),
            ]),
        ],
    );
    let balances = group(
        "Balances",
        vec![
            balance(
                known("$41.70", "50,360"),
                BalanceTier::Hero,
                AmountVisibility::Shown,
            ),
            wrap(vec![
                balance(
                    known("$20.74", "25,053"),
                    BalanceTier::Card,
                    AmountVisibility::Shown,
                ),
                iced::widget::Space::new().width(Length::Fixed(24.0)).into(),
                balance(
                    known("$20.74", "25,053"),
                    BalanceTier::Card,
                    AmountVisibility::Hidden,
                ),
                iced::widget::Space::new().width(Length::Fixed(24.0)).into(),
                balance(
                    BalanceValue::Zero {
                        primary: Figure::new("$0.00"),
                        secondary: Some(Figure::new("0").unit("SATS")),
                    },
                    BalanceTier::Card,
                    AmountVisibility::Shown,
                ),
            ]),
            balance(
                BalanceValue::Pending {
                    label: "Loading payment history".into(),
                },
                BalanceTier::Row,
                AmountVisibility::Shown,
            ),
            balance(
                BalanceValue::Unavailable {
                    reason: "Node offline".into(),
                },
                BalanceTier::Row,
                AmountVisibility::Shown,
            ),
            balance(
                known("$41.70", "50,360"),
                BalanceTier::Row,
                AmountVisibility::Unresolved,
            ),
        ],
    );
    let typography = group(
        "Typography",
        vec![
            title("Total Balance").into(),
            body("Body text for settings rows and descriptions.").into(),
            label("BTC").into(),
            caption("Applies to this Cube on Bitcoin mainnet.").into(),
            mono("bc1q…synthetic…p0r7").into(),
        ],
    );
    let surfaces = panel(
        Column::new()
            .spacing(2)
            .padding(4)
            .push(body("All Cubes"))
            .push(caption("Panel: menus, popovers and sheets")),
    );
    let page = Row::new()
        .spacing(space::XL)
        .push(
            Column::new()
                .spacing(space::XL)
                .width(Length::Fill)
                .push(actions)
                .push(chips)
                .push(surfaces),
        )
        .push(
            Column::new()
                .spacing(space::XL)
                .width(Length::Fill)
                .push(balances)
                .push(typography),
        );
    Container::new(page)
        .padding([32.0, 40.0])
        .width(Length::Fill)
        .height(Length::Fill)
        .style(tokens::surface::page)
        .into()
}

fn assert_window_gallery(name: &str, theme: &Theme, width: u32, height: u32) {
    let window = Size::new(width as f32, height as f32);

    let mut mounted = Mounted::new(overview(AmountVisibility::Shown), window);
    let collected = mounted.collect();
    for expected in [
        "Total Balance",
        "$41.70",
        "Spark · Daily spending",
        "L-BTC",
        "USDt",
        "Recovery path",
    ] {
        assert!(
            collected.texts.iter().any(|(_, text)| text == expected),
            "{name} {width}x{height}: overview is missing {expected:?}"
        );
    }
    for (bounds, text) in &collected.texts {
        assert!(
            bounds.x + bounds.width <= window.width + 0.5,
            "{name} {width}x{height}: {text:?} overflows the window"
        );
    }
    if (width, height) == (1024, 768) {
        mounted.run(scrollable::snap_to::<()>(
            Id::new("gallery-overview"),
            scrollable::RelativeOffset::END.into(),
        ));
        let scrolled = mounted.collect();
        let (_, viewport, content, offset) = scrolled
            .scrollables
            .iter()
            .find(|(id, _, _, _)| *id == Id::new("gallery-overview"))
            .expect("Overview must expose a mounted scrollable");
        assert!(content.height > viewport.height && offset.y > 0.0);
        for expected in ["Vault · Family savings", "Transfer"] {
            let bounds = scrolled
                .texts
                .iter()
                .find(|(_, text)| text == expected)
                .unwrap_or_else(|| panic!("missing {expected:?}"))
                .0;
            assert!(
                bounds.y - offset.y >= viewport.y - 0.5
                    && bounds.y + bounds.height - offset.y <= viewport.y + viewport.height + 0.5,
                "{name}: {expected:?} cannot be brought into the viewport"
            );
        }
        mounted.run(scrollable::snap_to::<()>(
            Id::new("gallery-overview"),
            scrollable::RelativeOffset::START.into(),
        ));
    }
    mounted.keyboard_focus("spark-send");
    let send = collected
        .focusables
        .iter()
        .find(|(id, _)| *id == Id::new("spark-send"))
        .unwrap()
        .1;
    let pixels = mounted.pixels(theme, mouse::Cursor::Unavailable);
    let ring = sample(&pixels, width, send.center_x(), send.y - 3.0);
    assert!(
        near(ring, theme.colors.shell.focus),
        "{name}: Send focus ring {ring:?}"
    );
    save(
        &format!("tenshu-overview-{name}-{width}x{height}"),
        &pixels,
        width,
        height,
    );

    let mut hidden = Mounted::new(overview(AmountVisibility::Hidden), window);
    let texts = hidden.texts();
    assert!(
        texts
            .iter()
            .all(|text| !text.chars().any(|c| c.is_ascii_digit())),
        "{name}: hidden overview leaked {texts:?}"
    );
    let pixels = hidden.pixels(theme, mouse::Cursor::Unavailable);
    save(
        &format!("tenshu-overview-hidden-{name}-{width}x{height}"),
        &pixels,
        width,
        height,
    );

    let mut sheet = Mounted::new(sheet(), window);
    let collected = sheet.collect();
    for (bounds, text) in &collected.texts {
        assert!(
            bounds.x + bounds.width <= window.width + 0.5,
            "{name} {width}x{height}: sheet text {text:?} overflows the window"
        );
    }
    let pixels = sheet.pixels(theme, mouse::Cursor::Unavailable);
    save(
        &format!("tenshu-sheet-{name}-{width}x{height}"),
        &pixels,
        width,
        height,
    );
}

#[test]
fn gallery_renders_overview_and_sheet_in_both_themes_and_window_sizes() {
    for (name, theme) in themes() {
        for (width, height) in WINDOWS {
            assert_window_gallery(name, &theme, width, height);
        }
    }
}
