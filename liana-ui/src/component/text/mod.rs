pub mod legacy;
pub mod new;

pub use legacy::*;

use crate::{font, theme::Theme};
use iced::advanced::text::Shaping;
use iced::Font;
use std::fmt::Display;

/// Per-helper typography spec: the font and (optionally) size that a text
/// helper applies. This is useful for the debugger being able to
/// display font spec without hradcoding it in the debug view.
#[derive(Debug, Clone, Copy)]
pub struct TextSpec {
    pub size: Option<u32>,
    pub font: Font,
}

/// Build a text widget from a [`TextSpec`].
pub fn apply<'a>(content: impl Display, spec: TextSpec) -> iced::widget::Text<'a, Theme> {
    let mut t = iced::widget::text!("{}", content)
        .shaping(Shaping::Advanced)
        .font(spec.font);
    if let Some(s) = spec.size {
        t = t.size(s);
    }
    t
}

/// Declare a batch of typography roles. For each `name, SPEC, font [, size]`
/// row, emits the `*_SPEC` constant and a matching helper fn that builds
/// an iced text widget from it. The trailing `size` is optional; when
/// omitted the spec's `size` is `None` (caller picks the size).
macro_rules! text_roles {
    ($($name:ident, $spec:ident, $font:expr $(, $size:expr)?);* $(;)?) => {
        $(
            pub const $spec: $crate::component::text::TextSpec =
                $crate::component::text::TextSpec {
                    size: $crate::component::text::__opt_size!($($size)?),
                    font: $font,
                };

            pub fn $name<'a>(
                content: impl ::std::fmt::Display,
            ) -> ::iced::widget::Text<'a, $crate::theme::Theme> {
                $crate::component::text::apply(content, $spec)
            }
        )*
    };
}

#[doc(hidden)]
macro_rules! __opt_size {
    () => {
        None
    };
    ($size:expr) => {
        Some($size)
    };
}

pub(crate) use __opt_size;
pub(crate) use text_roles;

pub trait Text {
    fn bold(self) -> Self;
    fn small(self) -> Self;
}

impl Text for iced::widget::Text<'_, Theme> {
    fn bold(self) -> Self {
        self.font(font::BOLD)
    }
    fn small(self) -> Self {
        self.size(legacy::P1_SIZE)
    }
}

pub fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().to_string() + chars.as_str(),
    }
}

pub fn truncate(str: &str, len: usize) -> String {
    let str = str.to_string();
    if str.len() <= len {
        return str;
    }
    if len < 3 {
        let mut str = str;
        while str.len() > len {
            str.pop();
        }
        return str;
    }
    let budget = len - 3;
    let mut str = str;
    while str.len() > budget {
        str.pop();
    }
    str.push_str("...");
    str
}

const SHORT_MARKER: &str = "[...]";

pub fn short_string(str: &str, len: usize) -> String {
    if str.len() <= len {
        return str.to_string();
    }
    shorten_middle(str, len)
}

pub fn short_email(str: &str, len: usize) -> String {
    if str.matches('@').count() == 1 {
        if let Some((local, domain)) = str.split_once('@') {
            let tail_len = 1 + domain.len();
            if len > tail_len + SHORT_MARKER.len() {
                let local_budget = len - tail_len;
                return format!("{}@{domain}", shorten_middle(local, local_budget));
            }
        }
    }
    shorten_middle(str, len)
}

fn shorten_middle(str: &str, len: usize) -> String {
    if str.len() <= len {
        return str.to_string();
    }
    if len <= SHORT_MARKER.len() {
        let mut out = String::from(str);
        while out.len() > len {
            out.pop();
        }
        return out;
    }
    let budget = len - SHORT_MARKER.len();
    let head_budget = budget.div_ceil(2);
    let tail_budget = budget - head_budget;

    let mut head_end = head_budget;
    while !str.is_char_boundary(head_end) {
        head_end -= 1;
    }

    let mut tail_start = str.len() - tail_budget;
    while !str.is_char_boundary(tail_start) {
        tail_start += 1;
    }

    format!("{}{SHORT_MARKER}{}", &str[..head_end], &str[tail_start..])
}
