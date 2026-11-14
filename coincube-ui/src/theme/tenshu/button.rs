//! Button styles for Tenshu actions.
//!
//! Pill actions follow `.pill.fill` / `.pill.out`, compact buttons `.btn`,
//! quiet links `.rlink`, chips `.chip` and icon actions `.eye` in the v113
//! artboards. Hover uses the shell `hover` overlay; pressed stacks it twice
//! (or darkens an accent fill). Disabled controls fade to `DISABLED_ALPHA`.

use iced::widget::button::{Status, Style};
use iced::{Background, Border, Color};

use super::{accent_pressed, faded, over, radius, Theme, ACCENT_HOVER, DISABLED_ALPHA, ON_ACCENT};

/// What an action does, which sets its emphasis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    /// The one main action of a row or screen, such as Send.
    Primary,
    /// A peer of the primary action, such as Receive or Transfer.
    Secondary,
    /// A borderless text action, such as Recovery path.
    Quiet,
    /// An action that removes or revokes something.
    Destructive,
}

/// The geometry family of an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionShape {
    /// 36px pill (`.pill`).
    Pill,
    /// 32px button (`.btn`).
    Compact,
    /// 32px chip (`.chip`). The kind does not change its treatment.
    Chip,
    /// 32px square icon-only action (`.eye`). The kind does not change its treatment.
    Icon,
}

impl ActionShape {
    /// Corner radius for this shape and kind; the focus ring follows it.
    pub fn radius(self, kind: ActionKind) -> f32 {
        match (self, kind) {
            (ActionShape::Pill | ActionShape::Compact, ActionKind::Quiet) => radius::LINK,
            (ActionShape::Pill, _) => radius::PILL,
            (ActionShape::Compact, _) | (ActionShape::Icon, _) => radius::CONTROL,
            (ActionShape::Chip, _) => radius::CHIP,
        }
    }
}

/// Style of an enabled or disabled action in the given pointer state.
pub fn action(theme: &Theme, status: Status, kind: ActionKind, shape: ActionShape) -> Style {
    let shell = &theme.colors.shell;
    let radius = shape.radius(kind);
    // (fill, border, ink). A `None` fill shows the surface behind the action.
    let (fill, border, ink) = match (shape, kind) {
        (ActionShape::Icon, _) => (None, None, shell.muted),
        (ActionShape::Chip, _) => (None, Some(shell.line), shell.ink),
        (_, ActionKind::Primary) => (Some(shell.accent), Some(shell.accent), ON_ACCENT),
        (_, ActionKind::Quiet) => (None, None, shell.accent_ink),
        (ActionShape::Pill, ActionKind::Secondary) => {
            (None, Some(shell.accent_ink), shell.accent_ink)
        }
        (ActionShape::Compact, ActionKind::Secondary) => {
            (Some(shell.panel), Some(shell.line), shell.ink)
        }
        (ActionShape::Pill, ActionKind::Destructive) => (None, Some(shell.bad), shell.bad),
        (ActionShape::Compact, ActionKind::Destructive) => {
            (Some(shell.panel), Some(shell.bad), shell.bad)
        }
    };

    let (fill, ink) = pointer_colors(theme, status, kind, shape, fill, ink);

    let fade = |color: Color| {
        if status == Status::Disabled {
            faded(color, DISABLED_ALPHA)
        } else {
            color
        }
    };

    Style {
        background: fill.map(|color| Background::Color(fade(color))),
        text_color: fade(ink),
        border: Border {
            color: border.map(fade).unwrap_or(Color::TRANSPARENT),
            width: if border.is_some() { 1.0 } else { 0.0 },
            radius: radius.into(),
        },
        ..Style::default()
    }
}

fn pointer_colors(
    theme: &Theme,
    status: Status,
    kind: ActionKind,
    shape: ActionShape,
    fill: Option<Color>,
    ink: Color,
) -> (Option<Color>, Color) {
    let shell = &theme.colors.shell;
    match status {
        Status::Active | Status::Disabled => (fill, ink),
        Status::Hovered | Status::Pressed => {
            let pressed = status == Status::Pressed;
            let ink = if shape == ActionShape::Icon {
                shell.ink
            } else {
                ink
            };
            let fill = fill_for_pointer(theme, kind, shape, fill, pressed);
            (fill, ink)
        }
    }
}

fn fill_for_pointer(
    theme: &Theme,
    kind: ActionKind,
    shape: ActionShape,
    fill: Option<Color>,
    pressed: bool,
) -> Option<Color> {
    let shell = &theme.colors.shell;
    match (kind, shape, fill) {
        (ActionKind::Primary, ActionShape::Pill | ActionShape::Compact, _) => Some(if pressed {
            accent_pressed(theme)
        } else {
            ACCENT_HOVER
        }),
        (_, _, Some(base)) => {
            let once = over(base, shell.hover);
            Some(if pressed {
                over(once, shell.hover)
            } else {
                once
            })
        }
        (_, _, None) => Some(if pressed {
            stacked(shell.hover)
        } else {
            shell.hover
        }),
    }
}

/// Two hover layers over an unknown surface, kept translucent.
fn stacked(hover: Color) -> Color {
    let alpha = 1.0 - (1.0 - hover.a) * (1.0 - hover.a);
    Color { a: alpha, ..hover }
}
