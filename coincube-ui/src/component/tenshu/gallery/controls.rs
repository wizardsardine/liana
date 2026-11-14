//! Behaviour tests and captures for the Tenshu form controls: toggles,
//! segmented choices, text and search fields, and setting rows.
//!
//! Set `TENSHU_GALLERY_DIR` to write `tenshu-controls-*` (a Settings and Send
//! composition at both window sizes) and `tenshu-control-states-*` (a state
//! matrix per theme). All data is synthetic.

use std::collections::HashSet;

use iced::advanced::widget::operation::{focusable, scrollable};
use iced::advanced::widget::Id;
use iced::keyboard::{self, key};
use iced::widget::button::Status;
use iced::widget::text_input;
use iced::{alignment, mouse, Background, Event, Length, Point, Rectangle, Size};

use super::{
    blit, near, sample, save, shell, themes, CellState, Mounted, Msg, HEADER_HEIGHT, LABEL_WIDTH,
    STATES, WINDOWS,
};
use crate::component::tenshu::*;
use crate::icon;
use crate::theme::tenshu::{self as tokens, control as control_style, size, space};
use crate::theme::Theme;
use crate::widget::{Column, Container, Element, Row};

fn toggled(name: &'static str) -> impl FnOnce(bool) -> Msg {
    move |value| Msg::Toggled(name, value)
}

fn selected(name: &'static str) -> impl Fn(u8) -> Msg {
    move |value| Msg::Selected(name, value)
}

fn input(name: &'static str) -> impl Fn(String) -> Msg {
    move |value| Msg::Input(name, value)
}

fn column_of(controls: Vec<Element<'static, Msg>>) -> Element<'static, Msg> {
    controls
        .into_iter()
        .fold(Column::new().spacing(12).padding(12), |column, control| {
            column.push(control)
        })
        .into()
}

fn ids(mounted: &mut Mounted) -> Vec<Id> {
    mounted
        .collect()
        .focusables
        .into_iter()
        .map(|(id, _)| id)
        .collect()
}

fn bounds_of(mounted: &mut Mounted, id: &'static str) -> Rectangle {
    mounted
        .collect()
        .focusables
        .into_iter()
        .find(|(found, _)| *found == Id::new(id))
        .unwrap_or_else(|| panic!("{id} is not a focus stop"))
        .1
}

fn text_bounds(mounted: &mut Mounted, text: &str) -> Rectangle {
    mounted
        .collect()
        .texts
        .into_iter()
        .find(|(_, found)| found == text)
        .unwrap_or_else(|| panic!("missing text {text:?}"))
        .0
}

/// Apply a focus intent the way an owner does: whole-tree focus.
fn apply(mounted: &mut Mounted, messages: &[Msg]) {
    for message in messages {
        if let Msg::Focus(id) = message {
            mounted.run(focusable::focus::<()>(id.clone()));
        }
    }
}

fn typed(text: &str) -> Event {
    let key = keyboard::Key::Character(text.into());
    Event::Keyboard(keyboard::Event::KeyPressed {
        key: key.clone(),
        modified_key: key,
        physical_key: key::Physical::Unidentified(key::NativeCode::Unidentified),
        location: keyboard::Location::Standard,
        modifiers: keyboard::Modifiers::empty(),
        text: Some(text.into()),
        repeat: false,
    })
}

fn units(selected_unit: Option<u8>, enabled: bool) -> Element<'static, Msg> {
    segmented(
        [
            segment("unit-btc", 0u8, "BTC"),
            segment("unit-sats", 1, "Sats"),
            segment("unit-msat", 2, "msat").disabled(),
            segment("unit-fiat", 3, "Fiat"),
        ],
        selected_unit,
        Msg::Focus,
    )
    .size(SegmentSize::Compact)
    .on_select_maybe(enabled.then(|| selected("unit")))
    .into()
}

// ----- Toggles ---------------------------------------------------------------

#[test]
fn toggles_emit_the_opposite_value_once_per_pointer_or_key_press() {
    for is_on in [false, true] {
        let controls = vec![
            toggle("pill", is_on, Msg::Focus)
                .on_toggle(toggled("pill"))
                .into(),
            toggle("switch", is_on, Msg::Focus)
                .switch()
                .on_toggle(toggled("switch"))
                .into(),
        ];
        let mut mounted = Mounted::new(column_of(controls), Size::new(320.0, 200.0));
        assert_eq!(ids(&mut mounted), vec![Id::new("pill"), Id::new("switch")]);
        for name in ["pill", "switch"] {
            let id = Id::new(name);
            let bounds = bounds_of(&mut mounted, name);
            assert_eq!(
                mounted.click(bounds.center()),
                vec![Msg::Focus(id.clone()), Msg::Toggled(name, !is_on)],
                "{name} (on: {is_on}): a click requests focus, then the opposite value"
            );
            mounted.focus(name);
            assert_eq!(mounted.focused(), Some(id));
            assert_eq!(
                mounted.key(key::Named::Space, true, false),
                vec![Msg::Toggled(name, !is_on)],
                "{name}: Space toggles once"
            );
            assert!(
                mounted.key(key::Named::Space, true, true).is_empty(),
                "{name}: a held Space does not repeat"
            );
            assert!(mounted.key(key::Named::Space, false, false).is_empty());
            assert_eq!(
                mounted.key(key::Named::Enter, true, false),
                vec![Msg::Toggled(name, !is_on)],
                "{name}: Enter toggles once"
            );
            assert!(mounted.key(key::Named::Enter, false, false).is_empty());
        }
        let texts = mounted.texts();
        assert!(
            texts.contains(&if is_on { "On" } else { "Off" }.to_string()),
            "the pill names its state in words: {texts:?}"
        );
    }
}

#[test]
fn toggle_draws_its_state_in_the_thumb_position_and_fill() {
    for (name, theme) in themes() {
        let shell = theme.colors.shell;
        for is_on in [false, true] {
            assert_toggle_render(name, &theme, shell, is_on);
        }
    }
}

// ----- Segmented choice ------------------------------------------------------

#[test]
fn segmented_choice_selects_by_pointer_keys_and_arrows() {
    let mut mounted = Mounted::new(units(Some(0), true), Size::new(420.0, 80.0));
    assert_eq!(
        ids(&mut mounted),
        ["unit-btc", "unit-sats", "unit-fiat"].map(Id::new).to_vec(),
        "every enabled segment is a Tab stop; the disabled one is not"
    );

    for (name, value) in [("unit-btc", 0u8), ("unit-sats", 1), ("unit-fiat", 3)] {
        let bounds = bounds_of(&mut mounted, name);
        assert_eq!(
            mounted.click(bounds.center()),
            vec![Msg::Focus(Id::new(name)), Msg::Selected("unit", value)],
            "{name}: a click requests focus and selects once"
        );
        mounted.focus(name);
        assert_eq!(
            mounted.key(key::Named::Enter, true, false),
            vec![Msg::Selected("unit", value)]
        );
        assert!(mounted.key(key::Named::Enter, false, false).is_empty());
    }

    let msat = mounted.node.children()[0].children()[2].bounds();
    assert!(
        mounted.click(msat.center()).is_empty(),
        "the disabled segment is inert"
    );

    // Arrows move the selection and focus to the next enabled segment,
    // skipping the disabled one and wrapping at both ends.
    mounted.focus("unit-sats");
    let messages = mounted.key(key::Named::ArrowRight, true, false);
    assert_eq!(
        messages,
        vec![Msg::Focus(Id::new("unit-fiat")), Msg::Selected("unit", 3)]
    );
    apply(&mut mounted, &messages);
    assert_eq!(mounted.focused(), Some(Id::new("unit-fiat")));
    assert!(
        mounted.key(key::Named::ArrowRight, true, true).is_empty(),
        "a held arrow does not repeat"
    );
    assert_eq!(
        mounted.key(key::Named::ArrowDown, true, false),
        vec![Msg::Focus(Id::new("unit-btc")), Msg::Selected("unit", 0)],
        "arrows wrap past the end"
    );
    mounted.focus("unit-btc");
    let messages = mounted.key(key::Named::ArrowLeft, true, false);
    assert_eq!(
        messages,
        vec![Msg::Focus(Id::new("unit-fiat")), Msg::Selected("unit", 3)],
        "arrows wrap past the start"
    );
    apply(&mut mounted, &messages);
    assert_eq!(
        mounted.key(key::Named::ArrowUp, true, false),
        vec![Msg::Focus(Id::new("unit-sats")), Msg::Selected("unit", 1)],
        "Up steps back past the disabled segment"
    );

    mounted.run(focusable::unfocus::<()>());
    assert!(
        mounted.key(key::Named::ArrowRight, true, false).is_empty(),
        "arrows do nothing unless a segment has focus"
    );
}

#[test]
fn selected_segment_is_drawn_with_the_selected_fill() {
    for (name, theme) in themes() {
        let shell = theme.colors.shell;
        let mut mounted = Mounted::new(
            Container::new(units(Some(1), true)).padding(8),
            Size::new(320.0, 60.0),
        );
        let pixels = mounted.pixels(&theme, mouse::Cursor::Unavailable);
        let sats = bounds_of(&mut mounted, "unit-sats");
        let btc = bounds_of(&mut mounted, "unit-btc");
        let fill = sample(&pixels, 320, sats.x + 3.0, sats.y + 3.0);
        let rest = sample(&pixels, 320, btc.x + 3.0, btc.y + 3.0);
        assert!(near(fill, shell.selected), "{name}: selected fill {fill:?}");
        // The selected segment's outline is the accent. Its left edge may sit
        // between pixels, so one of the columns around it is solid.
        let outline: Vec<_> = (-1..=1)
            .map(|dx| sample(&pixels, 320, sats.x + dx as f32 + 0.5, sats.center_y()))
            .collect();
        // Anti-aliased, the stroke still sits far closer to the accent than
        // the selected fill does.
        let accent = shell.accent.into_rgba8();
        let distance = |pixel: [u8; 3]| {
            pixel
                .iter()
                .zip(accent)
                .map(|(a, b)| (f32::from(*a) - f32::from(b)).powi(2))
                .sum::<f32>()
                .sqrt()
        };
        let nearest = outline
            .iter()
            .map(|pixel| distance(*pixel))
            .fold(f32::MAX, f32::min);
        assert!(
            nearest < 0.5 * distance(fill),
            "{name}: selected outline is not drawn in the accent: {outline:?}"
        );
        for status in [Status::Active, Status::Hovered, Status::Pressed] {
            let style = control_style::segment(&theme, status, true, 6.0);
            assert_eq!(
                (style.border.color, style.border.width),
                (shell.accent, 1.0),
                "{name}: selected {status:?} outline"
            );
            let style = control_style::segment(&theme, status, false, 6.0);
            assert_eq!(style.border.width, 0.0, "{name}: unselected {status:?}");
        }
        let unselected = sample(&pixels, 320, btc.x + 0.5, btc.center_y());
        assert!(
            near(unselected, shell.background),
            "{name}: unselected segments have no outline {unselected:?}"
        );
        assert!(near(rest, shell.background), "{name}: resting {rest:?}");
    }
}

// ----- Disabled controls ----------------------------------------------------

#[test]
fn disabled_controls_are_inert_and_skipped_by_tab() {
    let controls = vec![
        ActionButton::new("first", "First", ActionKind::Primary, Msg::Focus)
            .on_press(Msg::Pressed("first"))
            .into(),
        toggle("pill-off", true, Msg::Focus)
            .on_toggle_maybe(None::<fn(bool) -> Msg>)
            .into(),
        toggle("switch-off", false, Msg::Focus)
            .switch()
            .on_toggle_maybe(None::<fn(bool) -> Msg>)
            .into(),
        units(Some(0), false),
        text_field("name-off", "Cube name", "Everyday", Msg::Focus).into(),
        search_field("search-off", "Search label or id", "roof", Msg::Focus)
            .on_clear("search-off-clear", Msg::Cleared("search-off"))
            .into(),
        ActionButton::new("last", "Last", ActionKind::Secondary, Msg::Focus)
            .on_press(Msg::Pressed("last"))
            .into(),
    ];
    let mut mounted = Mounted::new(column_of(controls), Size::new(420.0, 600.0));
    assert_eq!(
        ids(&mut mounted),
        vec![Id::new("first"), Id::new("last")],
        "disabled toggles, segments and fields register no focus stop"
    );
    mounted.run(focusable::focus_next::<()>());
    mounted.run(focusable::focus_next::<()>());
    assert_eq!(mounted.focused(), Some(Id::new("last")));

    mounted.run(focusable::focus::<()>(Id::new("name-off")));
    assert_ne!(
        mounted.focused(),
        Some(Id::new("name-off")),
        "whole-tree focus cannot land on a disabled field"
    );
    let children: Vec<Rectangle> = mounted
        .node
        .children()
        .iter()
        .map(|node| node.bounds())
        .collect();
    for (index, bounds) in children.iter().enumerate().take(6).skip(1) {
        for point in [
            bounds.center(),
            Point::new(bounds.x + 8.0, bounds.y + 8.0),
            Point::new(bounds.x + bounds.width - 8.0, bounds.center_y()),
        ] {
            let messages = mounted.click(point);
            assert!(
                messages.is_empty(),
                "disabled control {index} emitted {messages:?}"
            );
        }
        assert!(mounted.key(key::Named::Enter, true, false).is_empty());
        assert!(mounted.key(key::Named::Space, true, false).is_empty());
        assert!(mounted.key(key::Named::Escape, true, false).is_empty());
    }
    let texts = mounted.texts();
    assert!(
        texts.contains(&"Cube name".to_string()),
        "a disabled field keeps its label"
    );
}

// ----- Text and search fields -----------------------------------------------

#[test]
fn text_field_label_focuses_the_input_which_takes_text_and_submit() {
    let field = || {
        column_of(vec![
            text_field("dest", "Destination", "", Msg::Focus)
                .placeholder("Lightning invoice, Lightning address or Bitcoin address")
                .on_input(input("dest"))
                .on_submit(Msg::Submitted("dest"))
                .into(),
            text_field("amount", "Amount", "21", Msg::Focus)
                .suffix("sats")
                .hint("Available 25,053 SATS")
                .on_input(input("amount"))
                .into(),
        ])
    };
    let mut mounted = Mounted::new(field(), Size::new(520.0, 300.0));
    assert_eq!(ids(&mut mounted), vec![Id::new("dest"), Id::new("amount")]);

    let label = text_bounds(&mut mounted, "Destination");
    let messages = mounted.click(label.center());
    assert_eq!(
        messages,
        vec![Msg::Focus(Id::new("dest"))],
        "pressing the label requests focus for its input"
    );
    apply(&mut mounted, &messages);
    assert_eq!(mounted.focused(), Some(Id::new("dest")));
    assert_eq!(
        mounted.event(typed("l"), mouse::Cursor::Unavailable),
        vec![Msg::Input("dest", "l".into())]
    );
    assert_eq!(
        mounted.key(key::Named::Enter, true, false),
        vec![Msg::Submitted("dest")]
    );

    mounted.run(focusable::focus_next::<()>());
    assert_eq!(
        mounted.focused(),
        Some(Id::new("amount")),
        "Tab moves from field to field in visual order"
    );
    for text in ["sats", "Available 25,053 SATS", "Amount"] {
        assert!(mounted.texts().contains(&text.to_string()), "{text}");
    }

    // A label accessory is its own stop before the input; the label text
    // still focuses the input.
    let mut with_accessory = Mounted::new(
        column_of(vec![text_field("amt", "Amount", "", Msg::Focus)
            .label_accessory(
                segmented(
                    [
                        segment("amt-sats", 0u8, "sats"),
                        segment("amt-btc", 1, "BTC"),
                    ],
                    Some(0),
                    Msg::Focus,
                )
                .size(SegmentSize::Small)
                .on_select(selected("amt-unit")),
            )
            .on_input(input("amt"))
            .into()]),
        Size::new(420.0, 160.0),
    );
    assert_eq!(
        ids(&mut with_accessory),
        ["amt-sats", "amt-btc", "amt"].map(Id::new).to_vec()
    );
    let label = text_bounds(&mut with_accessory, "Amount");
    assert_eq!(
        with_accessory.click(label.center()),
        vec![Msg::Focus(Id::new("amt"))]
    );
    let btc = bounds_of(&mut with_accessory, "amt-btc");
    assert_eq!(
        with_accessory.click(btc.center()),
        vec![Msg::Focus(Id::new("amt-btc")), Msg::Selected("amt-unit", 1)]
    );

    // A suffix is drawn over the input but never blocks it.
    let amount = bounds_of(&mut mounted, "amount");
    mounted.run(focusable::unfocus::<()>());
    let messages = mounted.click(Point::new(
        amount.x + amount.width - 20.0,
        amount.center_y(),
    ));
    assert!(messages.is_empty(), "{messages:?}");
    assert_eq!(mounted.focused(), Some(Id::new("amount")));
}

#[test]
fn field_error_is_shown_in_text_and_border() {
    for (name, theme) in themes() {
        let shell = theme.colors.shell;
        let mut mounted = Mounted::new(
            Container::new(
                text_field("dest", "Destination", "bc1q-not-valid", Msg::Focus)
                    .error("This is not a valid Bitcoin address.")
                    .on_input(input("dest")),
            )
            .padding(8),
            Size::new(420.0, 120.0),
        );
        assert!(mounted
            .texts()
            .contains(&"This is not a valid Bitcoin address.".to_string()));
        let field = bounds_of(&mut mounted, "dest");
        let pixels = mounted.pixels(&theme, mouse::Cursor::Unavailable);
        // The field's left edge falls on a whole pixel; its top may not.
        let border = sample(&pixels, 420, field.x + 0.5, field.center_y());
        assert!(near(border, shell.bad), "{name}: error border {border:?}");

        mounted.focus("dest");
        let pixels = mounted.pixels(&theme, mouse::Cursor::Unavailable);
        let ring = sample(&pixels, 420, field.center_x(), field.y + 1.0);
        assert!(near(ring, shell.focus), "{name}: focus border {ring:?}");
    }
}

fn search(value: &'static str) -> Element<'static, Msg> {
    column_of(vec![search_field(
        "q",
        "Search label or id",
        value,
        Msg::Focus,
    )
    .placeholder("Label, txid, payment hash, swap ID")
    .on_input(input("q"))
    .on_clear("q-clear", Msg::Cleared("q"))
    .into()])
}

#[test]
fn search_field_clears_by_action_or_escape_and_keeps_focus_in_the_input() {
    let mut mounted = Mounted::new(search("roof"), Size::new(360.0, 120.0));
    assert_eq!(
        ids(&mut mounted),
        vec![Id::new("q"), Id::new("q-clear")],
        "Clear follows the input in Tab order while there is a value"
    );

    mounted.focus("q");
    assert_eq!(
        mounted.key(key::Named::Escape, true, false),
        vec![Msg::Cleared("q")],
        "Escape clears a non-empty search"
    );
    mounted.rebuild(search(""));
    assert_eq!(
        mounted.focused(),
        Some(Id::new("q")),
        "focus stays in the input after it empties"
    );
    assert_eq!(
        ids(&mut mounted),
        vec![Id::new("q")],
        "no Clear without a value"
    );
    assert!(
        mounted.key(key::Named::Escape, true, false).is_empty(),
        "Escape on an empty search emits nothing"
    );
    // Iced's own Escape behaviour then applies: the input gives up focus.
    mounted.focus("q");
    assert_eq!(
        mounted.event(typed("r"), mouse::Cursor::Unavailable),
        vec![Msg::Input("q", "r".into())]
    );

    mounted.rebuild(search("roof"));
    let clear = bounds_of(&mut mounted, "q-clear");
    let messages = mounted.click(clear.center());
    assert_eq!(
        messages,
        vec![Msg::Focus(Id::new("q")), Msg::Cleared("q")],
        "pressing Clear hands focus to the input and clears once"
    );
    // Keyboard activation of Clear must not strand focus on the Clear action,
    // which leaves the tree with the value.
    for named in [key::Named::Enter, key::Named::Space] {
        mounted.rebuild(search("roof"));
        mounted.focus("q-clear");
        assert_eq!(mounted.focused(), Some(Id::new("q-clear")));
        let messages = mounted.key(named, true, false);
        assert_eq!(
            messages,
            vec![Msg::Focus(Id::new("q")), Msg::Cleared("q")],
            "{named:?} on Clear hands focus to the input and clears once"
        );
        assert!(mounted.key(named, true, true).is_empty(), "{named:?} held");
        assert!(mounted.key(named, false, false).is_empty());
        apply(&mut mounted, &messages);
        mounted.rebuild(search(""));
        assert_eq!(
            mounted.focused(),
            Some(Id::new("q")),
            "{named:?}: focus is in the emptied input"
        );
        assert_eq!(
            mounted.event(typed("r"), mouse::Cursor::Unavailable),
            vec![Msg::Input("q", "r".into())],
            "{named:?}: typing continues in the input"
        );
    }
}

// ----- Geometry and contrast ------------------------------------------------

#[test]
fn control_geometry_matches_the_v113_artboards() {
    let bounds = |element: Element<'static, Msg>, id: &'static str| {
        let mut mounted =
            Mounted::new(Container::new(element).padding(10), Size::new(800.0, 200.0));
        bounds_of(&mut mounted, id)
    };
    let pill = bounds(
        toggle("t", true, Msg::Focus).on_toggle(toggled("t")).into(),
        "t",
    );
    assert_eq!(pill.height, size::TOGGLE_HEIGHT);
    let switch = bounds(
        toggle("s", true, Msg::Focus)
            .switch()
            .on_toggle(toggled("s"))
            .into(),
        "s",
    );
    assert_eq!(
        (switch.width, switch.height),
        (size::SWITCH_WIDTH, size::SWITCH_HEIGHT)
    );
    for (segment_size, height) in [
        (SegmentSize::Regular, size::SEGMENT_HEIGHT),
        (SegmentSize::Compact, size::SEGMENT_COMPACT_HEIGHT),
        (SegmentSize::Small, size::SEGMENT_SMALL_HEIGHT),
    ] {
        let group = segmented([segment("a", 0u8, "Bitcoin")], Some(0), Msg::Focus)
            .size(segment_size)
            .on_select(selected("g"));
        assert_eq!(bounds(group.into(), "a").height, height, "{segment_size:?}");
    }
    for (field_size, height) in [
        (FieldSize::Regular, size::FIELD_HEIGHT),
        (FieldSize::Compact, size::FIELD_COMPACT_HEIGHT),
    ] {
        let field = text_field("f", "Destination", "", Msg::Focus)
            .size(field_size)
            .on_input(input("f"));
        assert_eq!(bounds(field.into(), "f").height, height, "{field_size:?}");
        let search = search_field("q", "Search", "x", Msg::Focus)
            .size(field_size)
            .on_input(input("q"))
            .on_clear("c", Msg::Cleared("q"));
        assert_eq!(bounds(search.into(), "q").height, height, "{field_size:?}");
    }
}

#[test]
fn control_text_and_state_colours_meet_wcag() {
    for (name, theme) in themes() {
        let shell = theme.colors.shell;
        assert_control_text_contrast(name, &theme, shell);

        // Thumbs against their tracks (non-text contrast, 3:1).
        for (track, thumb, what) in [
            (shell.line, shell.ink, "pill off"),
            (shell.accent, tokens::ON_ACCENT, "pill/switch on"),
            (shell.background, shell.muted, "switch off"),
        ] {
            let ratio = tokens::contrast(track, thumb);
            assert!(ratio >= 3.0, "{name}: {what} thumb {ratio:.2}:1");
        }
        let ratio = tokens::contrast(shell.muted, shell.background);
        assert!(ratio >= 3.0, "{name}: switch outline {ratio:.2}:1");

        // Fields: value and placeholder text, focus and error borders.
        let style = |status| control_style::field(&theme, status, 8.0, false);
        let active = style(text_input::Status::Active);
        let Background::Color(field_fill) = active.background else {
            unreachable!()
        };
        for (role, ink, minimum) in [
            ("value", active.value, 4.5),
            ("placeholder", active.placeholder, 4.5),
            (
                "focus border",
                style(text_input::Status::Focused { is_hovered: false })
                    .border
                    .color,
                3.0,
            ),
            (
                "error border",
                control_style::field(&theme, text_input::Status::Active, 8.0, true)
                    .border
                    .color,
                3.0,
            ),
        ] {
            let ratio = tokens::contrast(ink, field_fill);
            assert!(ratio >= minimum, "{name}: field {role} {ratio:.2}:1");
        }
    }
}

// ----- State matrix ----------------------------------------------------------

const CONTROL_CELL: (u32, u32) = (230, 92);

#[derive(Debug, Clone, Copy)]
enum ControlVariant {
    Pill(bool),
    Switch(bool),
    Segment,
    Field,
    Search,
}

const CONTROL_VARIANTS: [(&str, ControlVariant); 7] = [
    ("Toggle off", ControlVariant::Pill(false)),
    ("Toggle on", ControlVariant::Pill(true)),
    ("Switch off", ControlVariant::Switch(false)),
    ("Switch on", ControlVariant::Switch(true)),
    ("Segment", ControlVariant::Segment),
    ("Text field", ControlVariant::Field),
    ("Search field", ControlVariant::Search),
];

fn control_cell(variant: ControlVariant, enabled: bool) -> Element<'static, Msg> {
    match variant {
        ControlVariant::Pill(is_on) => toggle("cell", is_on, Msg::Focus)
            .on_toggle_maybe(enabled.then(|| toggled("cell")))
            .into(),
        ControlVariant::Switch(is_on) => toggle("cell", is_on, Msg::Focus)
            .switch()
            .on_toggle_maybe(enabled.then(|| toggled("cell")))
            .into(),
        ControlVariant::Segment => segmented(
            [
                segment("cell", 0u8, "Light"),
                segment("cell-dark", 1, "Dark"),
            ],
            Some(1),
            Msg::Focus,
        )
        .size(SegmentSize::Compact)
        .on_select_maybe(enabled.then(|| selected("cell")))
        .into(),
        ControlVariant::Field => text_field("cell", "Cube name", "Everyday", Msg::Focus)
            .size(FieldSize::Compact)
            .width(Length::Fixed(200.0))
            .on_input_maybe(enabled.then(|| input("cell")))
            .into(),
        ControlVariant::Search => search_field("cell", "Search", "", Msg::Focus)
            .placeholder("Label or txid")
            .width(Length::Fixed(200.0))
            .on_input_maybe(enabled.then(|| input("cell")))
            .into(),
    }
}

fn render_control(
    variant: ControlVariant,
    state: CellState,
    theme: &Theme,
) -> (Vec<u8>, Rectangle) {
    let size = Size::new(CONTROL_CELL.0 as f32, CONTROL_CELL.1 as f32);
    let mut mounted = Mounted::new(
        Container::new(control_cell(variant, state != CellState::Disabled))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(alignment::Horizontal::Center)
            .align_y(alignment::Vertical::Center),
        size,
    );
    // The control's input or first segment; a disabled cell has no focus stop,
    // so fall back to the enabled layout for sampling.
    let bounds = mounted
        .collect()
        .focusables
        .into_iter()
        .find(|(id, _)| *id == Id::new("cell"))
        .map(|(_, bounds)| bounds)
        .unwrap_or_else(|| {
            let mut enabled = Mounted::new(
                Container::new(control_cell(variant, true))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(alignment::Horizontal::Center)
                    .align_y(alignment::Vertical::Center),
                size,
            );
            bounds_of(&mut enabled, "cell")
        });
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

#[test]
fn control_state_matrix_shows_distinct_states() {
    let width = LABEL_WIDTH + CONTROL_CELL.0 * STATES.len() as u32;
    let height = HEADER_HEIGHT + CONTROL_CELL.1 * CONTROL_VARIANTS.len() as u32;
    for (name, theme) in themes() {
        render_control_theme_matrix(name, &theme, width, height);
    }
}

fn assert_toggle_render(
    name: &str,
    theme: &Theme,
    shell: crate::theme::palette::ShellPalette,
    is_on: bool,
) {
    let mut mounted = Mounted::new(
        Container::new(
            toggle("sw", is_on, Msg::Focus)
                .switch()
                .on_toggle(toggled("sw")),
        )
        .padding(8),
        Size::new(120.0, 40.0),
    );
    let bounds = bounds_of(&mut mounted, "sw");
    assert!(
        mounted
            .texts()
            .contains(&if is_on { "On" } else { "Off" }.to_string()),
        "{name}: the switch names its state in words"
    );
    let pixels = mounted.pixels(theme, mouse::Cursor::Unavailable);
    let left = sample(&pixels, 120, bounds.x + 12.0, bounds.center_y());
    let right = sample(
        &pixels,
        120,
        bounds.x + bounds.width - 12.0,
        bounds.center_y(),
    );
    let thumb = if is_on { right } else { left };
    let track = if is_on { left } else { right };
    let expected_thumb = if is_on {
        tokens::ON_ACCENT
    } else {
        shell.muted
    };
    let expected_track = if is_on {
        shell.accent
    } else {
        shell.background
    };
    assert!(
        near(thumb, expected_thumb),
        "{name} on={is_on}: thumb {thumb:?}"
    );
    assert!(
        near(track, expected_track),
        "{name} on={is_on}: track {track:?}"
    );
}

fn assert_control_text_contrast(
    name: &str,
    theme: &Theme,
    shell: crate::theme::palette::ShellPalette,
) {
    for surface in [shell.background, shell.panel] {
        let on = |style: iced::widget::button::Style| match style.background {
            Some(Background::Color(color)) => tokens::over(surface, color),
            _ => surface,
        };
        for status in [Status::Active, Status::Hovered, Status::Pressed] {
            let pill = control_style::toggle_pill(theme, status);
            let ratio = tokens::contrast(pill.text_color, on(pill));
            assert!(ratio >= 4.5, "{name}: toggle word {status:?} {ratio:.2}:1");
            for is_selected in [false, true] {
                let style = control_style::segment(theme, status, is_selected, 6.0);
                // Segments sit on the window background inside their frame.
                let fill = match style.background {
                    Some(Background::Color(color)) => tokens::over(shell.background, color),
                    _ => shell.background,
                };
                let ratio = tokens::contrast(style.text_color, fill);
                assert!(
                    ratio >= 4.5,
                    "{name}: segment selected={is_selected} {status:?} {ratio:.2}:1"
                );
            }
        }
    }
}

fn render_control_theme_matrix(name: &str, theme: &Theme, width: u32, height: u32) {
    let shell = theme.colors.shell;
    let header = STATES.iter().fold(
        Row::new().push(iced::widget::Space::new().width(Length::Fixed(LABEL_WIDTH as f32))),
        |row, (_, label)| {
            row.push(
                Container::new(caption(*label))
                    .width(Length::Fixed(CONTROL_CELL.0 as f32))
                    .align_x(alignment::Horizontal::Center),
            )
        },
    );
    let frame = CONTROL_VARIANTS.iter().fold(
        Column::new().push(Container::new(header).height(Length::Fixed(HEADER_HEIGHT as f32))),
        |column, (label, _)| {
            column.push(
                Container::new(body(*label))
                    .padding([0.0, 16.0])
                    .height(Length::Fixed(CONTROL_CELL.1 as f32))
                    .align_y(alignment::Vertical::Center),
            )
        },
    );
    let mut mosaic = Mounted::new(frame, Size::new(width as f32, height as f32))
        .pixels(theme, mouse::Cursor::Unavailable);

    for (row, (label, variant)) in CONTROL_VARIANTS.iter().enumerate() {
        let mut hashes = HashSet::new();
        for (column, (state, _)) in STATES.iter().enumerate() {
            let (pixels, bounds) = render_control(*variant, *state, theme);
            // A pressed field is a focused field: the press focuses it.
            let is_text = matches!(variant, ControlVariant::Field | ControlVariant::Search);
            if !(is_text && *state == CellState::Pressed) {
                assert!(
                    hashes.insert(pixels.clone()),
                    "{name}: {label} {state:?} looks identical to another state"
                );
            }
            if *state == CellState::Focused {
                let (x, y) = if is_text {
                    (bounds.center_x(), bounds.y + 1.0)
                } else {
                    (bounds.center_x(), bounds.y - 3.0)
                };
                let ring = sample(&pixels, CONTROL_CELL.0, x, y);
                assert!(near(ring, shell.focus), "{name}: {label} focus {ring:?}");
            }
            blit(
                &mut mosaic,
                width,
                &pixels,
                CONTROL_CELL,
                LABEL_WIDTH + column as u32 * CONTROL_CELL.0,
                HEADER_HEIGHT + row as u32 * CONTROL_CELL.1,
            );
        }
    }
    save(
        &format!("tenshu-control-states-{name}"),
        &mosaic,
        width,
        height,
    );
}

// ----- Composition -----------------------------------------------------------

/// Settings, App preferences and Send rows built only from the components.
fn controls_page() -> Element<'static, Msg> {
    let group =
        |heading: &'static str, rows: Vec<Element<'static, Msg>>| -> Element<'static, Msg> {
            rows.into_iter()
                .fold(
                    Column::new().push(
                        Container::new(section(heading))
                            .padding(iced::Padding::default().bottom(space::S)),
                    ),
                    |column, row| column.push(row),
                )
                .into()
        };
    let amounts = group(
        "Amounts & currency",
        vec![
            setting_row("Bitcoin unit", units(Some(0), true)).into(),
            setting_row(
                "Primary amount",
                segmented(
                    [
                        segment("primary-bitcoin", 0u8, "Bitcoin"),
                        segment("primary-fiat", 1, "Fiat"),
                    ],
                    Some(0),
                    Msg::Focus,
                )
                .size(SegmentSize::Compact)
                .on_select(selected("primary")),
            )
            .description("Shown large.")
            .into(),
            setting_row(
                "Fiat estimates",
                toggle("fiat", true, Msg::Focus).on_toggle(toggled("fiat")),
            )
            .description("Approximate values beside Bitcoin amounts.")
            .into(),
            setting_row(
                "Direction badges",
                toggle("badges", false, Msg::Focus).on_toggle(toggled("badges")),
            )
            .tag("This device")
            .description("In and Out badges in Activity.")
            .into(),
        ],
    );
    let appearance = group(
        "Appearance",
        vec![
            setting_row(
                "Theme",
                segmented(
                    [
                        segment("theme-system", 0u8, "System").icon(icon::display_icon()),
                        segment("theme-light", 1, "Light").icon(icon::sun_icon()),
                        segment("theme-dark", 2, "Dark").icon(icon::moon_icon()),
                    ],
                    Some(2),
                    Msg::Focus,
                )
                .size(SegmentSize::Compact)
                .on_select(selected("theme")),
            )
            .tag("This device")
            .description("Colors for the Launcher and every Cube on this device.")
            .note("System is currently: Dark", Tone::Muted)
            .into(),
            setting_row(
                "Developer mode",
                toggle("developer", false, Msg::Focus)
                    .switch()
                    .on_toggle(toggled("developer")),
            )
            .tag("This device")
            .trailing_tag()
            .description("Shows test networks in the Launcher and in Create.")
            .into(),
            setting_row(
                "Bitcoin Blake2b",
                toggle("blake2b", false, Msg::Focus)
                    .switch()
                    .on_toggle_maybe(None::<fn(bool) -> Msg>),
            )
            .tag("This device")
            .trailing_tag()
            .description("Beta network for Vault creation.")
            .note("Turn on Developer mode first.", Tone::Warn)
            .without_divider()
            .into(),
        ],
    );
    let send = card(
        Column::new()
            .spacing(space::L)
            .push(
                Row::new()
                    .align_y(iced::Alignment::Center)
                    .push(section("Payment details").width(Length::Fill))
                    .push(
                        segmented(
                            [
                                segment("rail-bitcoin", 0u8, "Bitcoin"),
                                segment("rail-stable", 1, "Stablecoin (cross-chain)"),
                            ],
                            Some(0),
                            Msg::Focus,
                        )
                        .on_select(selected("rail")),
                    ),
            )
            .push(
                text_field("send-destination", "Destination", "", Msg::Focus)
                    .placeholder("Lightning invoice, Lightning address or Bitcoin address")
                    .on_input(input("send-destination")),
            )
            .push(
                text_field("send-amount", "Amount", "25,000", Msg::Focus)
                    .label_accessory(
                        segmented(
                            [
                                segment("amount-sats", 0u8, "sats"),
                                segment("amount-btc", 1, "BTC"),
                            ],
                            Some(0),
                            Msg::Focus,
                        )
                        .size(SegmentSize::Small)
                        .on_select(selected("amount-unit")),
                    )
                    .suffix("sats")
                    .hint("Available 25,053 SATS")
                    .on_input(input("send-amount")),
            )
            .push(
                text_field(
                    "send-invalid",
                    "Refund address",
                    "bc1q…synthetic…zz",
                    Msg::Focus,
                )
                .mono()
                .error("This is not a valid Bitcoin address.")
                .on_input(input("send-invalid")),
            ),
    );
    let filters = group(
        "Activity filters",
        vec![Row::new()
            .spacing(space::M)
            .align_y(iced::Alignment::End)
            .push(
                text_field("filter-dates", "Date range", "Oct 15 – Oct 22", Msg::Focus)
                    .size(FieldSize::Compact)
                    .width(Length::Fixed(180.0))
                    .on_input(input("filter-dates")),
            )
            .push(
                search_field("filter-search", "Search label or id", "roof", Msg::Focus)
                    .placeholder("Label, txid, payment hash, swap ID")
                    .width(Length::Fixed(280.0))
                    .on_input(input("filter-search"))
                    .on_clear("filter-search-clear", Msg::Cleared("filter-search")),
            )
            .push(
                text_field("filter-name", "Cube name", "Everyday", Msg::Focus)
                    .size(FieldSize::Compact)
                    .width(Length::Fixed(160.0)),
            )
            .wrap()
            .vertical_spacing(space::M)
            .into()],
    );
    Column::new()
        .spacing(space::XL)
        .max_width(760)
        .push(
            Column::new()
                .spacing(2)
                .push(title("Settings"))
                .push(caption("Cube · Everyday · Bitcoin mainnet")),
        )
        .push(amounts)
        .push(appearance)
        .push(send)
        .push(filters)
        .into()
}

/// Scroll the gallery so `bounds` sits near the top of the window when it
/// would otherwise be cut off; returns the offset.
fn reveal(mounted: &mut Mounted, bounds: Rectangle, window: Size) -> f32 {
    let wanted = if bounds.y + bounds.height + 40.0 > window.height {
        bounds.y - 120.0
    } else {
        0.0
    };
    mounted.run(scrollable::scroll_to::<()>(
        Id::new("gallery-overview"),
        scrollable::AbsoluteOffset {
            x: None,
            y: Some(wanted),
        },
    ));
    let (_, _, _, translation) = mounted
        .collect()
        .scrollables
        .into_iter()
        .find(|(id, _, _, _)| *id == Id::new("gallery-overview"))
        .expect("the gallery scrolls");
    let offset = translation.y;
    assert!(
        bounds.y - offset >= 0.0 && bounds.y + bounds.height - offset <= window.height,
        "control at {bounds:?} cannot be brought into the window"
    );
    offset
}

#[test]
fn gallery_renders_controls_in_both_themes_and_window_sizes() {
    for (name, theme) in themes() {
        for (width, height) in WINDOWS {
            let window = Size::new(width as f32, height as f32);
            let mut mounted = Mounted::new(shell(controls_page()), window);
            let collected = mounted.collect();
            for expected in [
                "Bitcoin unit",
                "Fiat estimates",
                "On",
                "Off",
                "This device",
                "System is currently: Dark",
                "Turn on Developer mode first.",
                "Destination",
                "sats",
                "Available 25,053 SATS",
                "This is not a valid Bitcoin address.",
                "Search label or id",
            ] {
                assert!(
                    collected.texts.iter().any(|(_, text)| text == expected),
                    "{name} {width}x{height}: missing {expected:?}"
                );
            }
            for (bounds, text) in &collected.texts {
                assert!(
                    bounds.x + bounds.width <= window.width + 0.5,
                    "{name} {width}x{height}: {text:?} overflows the window"
                );
            }
            let focusables: Vec<Id> = collected
                .focusables
                .iter()
                .map(|(id, _)| id.clone())
                .collect();
            for disabled in ["blake2b", "filter-name", "unit-msat"] {
                assert!(
                    !focusables.contains(&Id::new(disabled)),
                    "{name}: {disabled} must not be a focus stop"
                );
            }
            for stop in [
                "theme-dark",
                "developer",
                "send-destination",
                "filter-search-clear",
            ] {
                assert!(focusables.contains(&Id::new(stop)), "{name}: {stop}");
            }
            let position = |id: &'static str| {
                focusables
                    .iter()
                    .position(|found| *found == Id::new(id))
                    .unwrap()
            };
            assert!(
                position("amount-btc") < position("send-amount"),
                "{name}: a label accessory precedes its input in Tab order"
            );

            // The top of the page with a focused segment, then the form with a
            // focused field, scrolled into view where the window is short.
            // Focus an unselected segment, so the capture shows the blue focus
            // ring beside the selected segment's orange outline.
            mounted.keyboard_focus("theme-light");
            let segment = bounds_of(&mut mounted, "theme-light");
            let offset = reveal(&mut mounted, segment, window);
            let pixels = mounted.pixels(&theme, mouse::Cursor::Unavailable);
            let ring = sample(&pixels, width, segment.center_x(), segment.y - offset - 3.0);
            assert!(
                near(ring, theme.colors.shell.focus),
                "{name} {width}x{height}: segment ring {ring:?}"
            );
            save(
                &format!("tenshu-controls-{name}-{width}x{height}"),
                &pixels,
                width,
                height,
            );

            mounted.focus("send-destination");
            let field = bounds_of(&mut mounted, "send-destination");
            let offset = reveal(&mut mounted, field, window);
            let pixels = mounted.pixels(&theme, mouse::Cursor::Unavailable);
            let border = sample(&pixels, width, field.center_x(), field.y - offset + 1.0);
            assert!(
                near(border, theme.colors.shell.focus),
                "{name} {width}x{height}: field focus {border:?}"
            );
            save(
                &format!("tenshu-controls-form-{name}-{width}x{height}"),
                &pixels,
                width,
                height,
            );
        }
    }
}
