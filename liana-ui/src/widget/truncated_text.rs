//! Single line text cut with an ellipsis to the width it is given.

use iced::{
    advanced::{
        layout, mouse, overlay, renderer,
        text::{Renderer as _, Wrapping},
        widget::{tree, Operation, Tree, Widget},
        Clipboard, Layout, Shell,
    },
    widget::{text::Style, tooltip::Position},
    Event, Length, Pixels, Rectangle, Size, Vector,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::{
    component::{
        text::{self, TextSpec},
        tooltip::{help_text, tooltip_custom},
    },
    theme::Theme,
    widget::Element,
};

type Renderer = iced::Renderer;

const ELLIPSIS: &str = "…";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ellipsis {
    End,
    /// Keeps the head and the tail, for emails, addresses and txids.
    Middle,
}

pub struct TruncatedText<'a, Message> {
    content: String,
    spec: TextSpec,
    style: Option<fn(&Theme) -> Style>,
    ellipsis: Ellipsis,
    tooltip: bool,
    /// Full content until layout rebuilds it with the cut content.
    shown: Element<'a, Message>,
}

#[derive(Debug, Default)]
struct State {
    cut: bool,
}

impl<'a, Message: 'a> TruncatedText<'a, Message> {
    pub fn new(content: impl Into<String>, spec: TextSpec) -> Self {
        let content = content.into();
        Self {
            shown: text::apply(&content, spec).wrapping(Wrapping::None).into(),
            content,
            spec,
            style: None,
            ellipsis: Ellipsis::End,
            tooltip: false,
        }
    }

    pub fn style(mut self, style: fn(&Theme) -> Style) -> Self {
        self.style = Some(style);
        self.shown = self.render(&self.content);
        self
    }

    pub fn middle(mut self) -> Self {
        self.ellipsis = Ellipsis::Middle;
        self
    }

    /// Show the full text on hover when it was cut.
    pub fn tooltip(mut self) -> Self {
        self.tooltip = true;
        self.shown = self.render(&self.content);
        self
    }

    fn render(&self, shown: &str) -> Element<'a, Message> {
        let text = text::apply(shown, self.spec).wrapping(Wrapping::None);
        let text = match self.style {
            Some(style) => text.style(style),
            None => text,
        };
        if self.tooltip {
            tooltip_custom(help_text(&self.content), text, Position::Top).into()
        } else {
            text.into()
        }
    }
}

/// `content` cut to fit `max_width` with an ellipsis, `None` when it fits whole. Falls back to the
/// ellipsis alone when nothing else fits.
fn cut(
    content: &str,
    ellipsis: Ellipsis,
    max_width: f32,
    measure: impl Fn(&str) -> f32,
) -> Option<String> {
    if measure(content) <= max_width {
        return None;
    }
    let graphemes: Vec<&str> = content.graphemes(true).collect();
    let mut fitting = 0;
    let mut too_many = graphemes.len();
    while too_many - fitting > 1 {
        let kept = (fitting + too_many) / 2;
        if measure(&ellipsize(&graphemes, kept, ellipsis)) <= max_width {
            fitting = kept;
        } else {
            too_many = kept;
        }
    }
    Some(ellipsize(&graphemes, fitting, ellipsis))
}

/// Keeps `kept` graphemes around the ellipsis.
fn ellipsize(graphemes: &[&str], kept: usize, ellipsis: Ellipsis) -> String {
    let (head, tail) = match ellipsis {
        Ellipsis::End => (kept, 0),
        Ellipsis::Middle => (kept.div_ceil(2), kept / 2),
    };
    let mut shown = graphemes[..head].concat();
    shown.push_str(ELLIPSIS);
    shown.push_str(&graphemes[graphemes.len() - tail..].concat());
    shown
}

impl<'a, Message: 'a> Widget<Message, Theme, Renderer> for TruncatedText<'a, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.shown)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.shown));
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Shrink, Length::Shrink)
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let size = self
            .spec
            .size
            .map(Pixels::from)
            .unwrap_or_else(|| renderer.default_size());
        let cut = cut(&self.content, self.ellipsis, limits.max().width, |s| {
            text::width(s, self.spec.font, size)
        });
        tree.state.downcast_mut::<State>().cut = cut.is_some();
        self.shown = self.render(cut.as_deref().unwrap_or(&self.content));
        self.shown
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
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
        self.shown.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
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
        self.shown.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.shown.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.shown
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        if !tree.state.downcast_ref::<State>().cut {
            return None;
        }
        self.shown.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

impl<'a, Message: 'a> From<TruncatedText<'a, Message>> for Element<'a, Message> {
    fn from(text: TruncatedText<'a, Message>) -> Self {
        Element::new(text)
    }
}

#[cfg(test)]
mod tests {
    use unicode_segmentation::UnicodeSegmentation;

    use crate::widget::truncated_text::{cut, Ellipsis};

    fn ten_per_grapheme(s: &str) -> f32 {
        s.graphemes(true).count() as f32 * 10.0
    }

    #[test]
    fn exact_fit_is_not_cut() {
        assert_eq!(cut("abcde", Ellipsis::End, 50.0, ten_per_grapheme), None);
    }

    #[test]
    fn end() {
        assert_eq!(
            cut("abcdefghij", Ellipsis::End, 50.0, ten_per_grapheme),
            Some("abcd…".to_string())
        );
    }

    #[test]
    fn middle() {
        assert_eq!(
            cut("abcdefghij", Ellipsis::Middle, 50.0, ten_per_grapheme),
            Some("ab…ij".to_string())
        );
        assert_eq!(
            cut("abcdefghij", Ellipsis::Middle, 60.0, ten_per_grapheme),
            Some("abc…ij".to_string())
        );
    }

    #[test]
    fn multi_byte_graphemes() {
        assert_eq!(
            cut("ééééé", Ellipsis::End, 30.0, ten_per_grapheme),
            Some("éé…".to_string())
        );
        assert_eq!(
            cut("ééééé", Ellipsis::Middle, 40.0, ten_per_grapheme),
            Some("éé…é".to_string())
        );
    }

    #[test]
    fn combining_marks_kept_whole() {
        assert_eq!(
            cut(
                "e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}",
                Ellipsis::End,
                30.0,
                ten_per_grapheme
            ),
            Some("e\u{301}e\u{301}…".to_string())
        );
    }

    #[test]
    fn only_ellipsis_fits() {
        assert_eq!(
            cut("abc", Ellipsis::End, 10.0, ten_per_grapheme),
            Some("…".to_string())
        );
    }

    #[test]
    fn nothing_fits() {
        assert_eq!(
            cut("abc", Ellipsis::End, 0.0, ten_per_grapheme),
            Some("…".to_string())
        );
        assert_eq!(
            cut("abc", Ellipsis::Middle, 0.0, ten_per_grapheme),
            Some("…".to_string())
        );
    }
}
