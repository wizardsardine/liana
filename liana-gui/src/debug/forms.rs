//! Gallery of every `liana_ui::component::form::Form` constructor and
//! relevant builder modifier.
//!
//! Forms borrow their `Value<String>` so we keep one [`std::sync::LazyLock`]
//! per sample value at module scope — that gives us `&'static Value<String>`
//! references the `Form<'static, …>` constructors require.
//!
//! Inputs receive `|_| ()` as the on-change callback. Since the values are
//! immutable (stored in static `LazyLock`s, not stateful), typing in the
//! field shows focus/cursor behavior but doesn't update the displayed text;
//! events route through `Message::DebugNoOp` like every other debug-overlay
//! interaction.

use std::sync::LazyLock;

use iced::{Alignment, Length};
use liana_ui::{
    component::{
        form::{Form, Value},
        modal, text,
    },
    theme,
    widget::*,
};

use crate::debug::{debug_chrome, DebugMessage, DebugPageEntry};

pub static ENTRY: DebugPageEntry = DebugPageEntry { view };

const ROW_SPACING: f32 = 30.0;

// ----- Sample values --------------------------------------------------------

static V_EMPTY: LazyLock<Value<String>> = LazyLock::new(Value::default);

static V_HELLO: LazyLock<Value<String>> = LazyLock::new(|| Value {
    value: "Hello world".to_string(),
    warning: None,
    valid: true,
});

static V_INVALID: LazyLock<Value<String>> = LazyLock::new(|| Value {
    value: "bad input".to_string(),
    warning: Some("This value is invalid"),
    valid: false,
});

static V_BTC: LazyLock<Value<String>> = LazyLock::new(|| Value {
    value: "0.00100000".to_string(),
    warning: None,
    valid: true,
});

// ----- Layout helpers ------------------------------------------------------

fn entry(
    path: &'static str,
    widget: Element<'static, DebugMessage>,
) -> Column<'static, DebugMessage> {
    Column::new().spacing(8).push(text::p1_regular(path)).push(
        Container::new(widget)
            .width(Length::Fixed(modal::BTN_W as f32))
            .style(theme::card::border),
    )
}

/// Build a debug page from a list of `(code-path, widget)` pairs. Each widget
/// is converted to [`Element`] internally, so callers only pass the raw
/// constructed widget — no `.into()` per row.
fn build_page<W>(
    title: &'static str,
    rows: Vec<(&'static str, W)>,
) -> Element<'static, DebugMessage>
where
    W: Into<Element<'static, DebugMessage>>,
{
    let mid = rows.len().div_ceil(2);
    let mut iter = rows.into_iter().map(|(p, w)| entry(p, w.into()));
    let left = (&mut iter)
        .take(mid)
        .fold(Column::new().spacing(ROW_SPACING), Column::push);
    let right = iter.fold(Column::new().spacing(ROW_SPACING), Column::push);
    let body = Row::new()
        .spacing(40)
        .align_y(Alignment::Start)
        .push(left)
        .push(right);
    debug_chrome(title, body)
}

// ----- View ----------------------------------------------------------------

fn view() -> Element<'static, DebugMessage> {
    let on_change = |_: String| ();

    #[rustfmt::skip]
    let rows = vec![
        ("Form::new(\"placeholder\", &Value::default(), |_| ())", Form::new("placeholder", &V_EMPTY, on_change)),
        ("Form::new(\"placeholder\", <hello>, |_| ())",           Form::new("placeholder", &V_HELLO, on_change)),
        ("Form::new(\"placeholder\", <invalid>, |_| ())",         Form::new("placeholder", &V_INVALID, on_change)),
        ("Form::new_disabled(\"placeholder\", <hello>)",          Form::new_disabled("placeholder", &V_HELLO)),
        ("Form::new_disabled(\"placeholder\", <empty>)",          Form::new_disabled("placeholder", &V_EMPTY)),
        ("Form::new_trimmed(\"placeholder\", <hello>, |_| ())",   Form::new_trimmed("placeholder", &V_HELLO, on_change)),
        ("Form::new_amount_btc(\"0.0\", <btc>, |_| ())",          Form::new_amount_btc("0.0", &V_BTC, on_change)),
        ("Form::new(...).padding(20)",                            Form::new("placeholder", &V_EMPTY, on_change).padding(20)),
        ("Form::new(...).size(28)",                               Form::new("placeholder", &V_HELLO, on_change).size(28)),
        ("Form::new(...).warning(\"...\")",                       Form::new("placeholder", &V_INVALID, on_change).warning("Explicit warning")),
        ("Form::new(...).on_submit(())",                          Form::new("placeholder", &V_HELLO, on_change).on_submit(())),
    ];

    build_page("Forms", rows)
}
