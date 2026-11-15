//! The Connect control, drawn the same way in an open Cube's top bar and on
//! the Launcher: the account glyph with a status light, then
//! `Connect · {email}`, `Connect · Offline`, `Connect · Checking…` or
//! `Connect · Sign in`. Inside a Cube the light reports live updates and a
//! chevron opens its panel; on the Launcher it reports the account and the
//! control opens the Connect sheet.

use coincube_ui::{
    component::tenshu::top_bar as bar,
    font, theme,
    widget::{action::Action, Element},
};
use iced::{
    advanced::widget::Id,
    widget::{container, row, text, Space},
    Alignment, Length,
};

use crate::app::{view::shared::glyph, ConnectLight};

/// `.chip { border-radius: 8px }`.
pub const CHIP_RADIUS: f32 = 8.0;
/// `.chip { height: 32px }`.
pub const CHIP_HEIGHT: f32 = 32.0;

/// A chip's press target: the shell's hover fill, kept inside the pill.
pub fn chip_button(
    theme: &theme::Theme,
    status: iced::widget::button::Status,
) -> iced::widget::button::Style {
    let mut style = theme::button::shell_nav(theme, status);
    style.border.radius = CHIP_RADIUS.into();
    style
}

/// `.chip`: the shell's chip surface with the design's 8px corners.
pub fn chip_style(t: &theme::Theme) -> iced::widget::container::Style {
    let mut style = theme::container::shell_chip(t);
    style.border.radius = CHIP_RADIUS.into();
    style
}

/// The light colours (`--alert`, `--amber`): stronger than the text tones,
/// so a 9px dot reads on the rail.
fn light_color(t: &theme::Theme, light: ConnectLight) -> iced::Color {
    let dark = t.mode == coincube_ui::theme::palette::ThemeMode::Dark;
    match light {
        ConnectLight::On | ConnectLight::Connecting => t.colors.shell.good,
        ConnectLight::Reconnecting => {
            if dark {
                iced::color!(0xf3b465)
            } else {
                iced::color!(0xc27400)
            }
        }
        ConnectLight::NeedsYou => {
            if dark {
                iced::color!(0xf0604d)
            } else {
                iced::color!(0xb3261e)
            }
        }
        ConnectLight::Offline => t.colors.shell.muted,
    }
}

/// The status light: a 9px dot ringed in the rail colour, or a grey ring
/// when offline. `dim` is the pulse's low phase.
fn status_light<'a, M: 'static>(light: ConnectLight, dim: bool) -> Element<'a, M> {
    let pulses = matches!(light, ConnectLight::Connecting | ConnectLight::Reconnecting);
    let alpha = if pulses && dim { 0.3 } else { 1.0 };
    container(Space::new())
        .width(Length::Fixed(9.0))
        .height(Length::Fixed(9.0))
        .style(move |t: &theme::Theme| {
            let colour = light_color(t, light);
            let (background, border) = if light == ConnectLight::Offline {
                (
                    t.colors.shell.rail,
                    iced::Border {
                        color: colour,
                        width: 1.5,
                        radius: 4.5.into(),
                    },
                )
            } else {
                (
                    iced::Color { a: alpha, ..colour },
                    iced::Border {
                        color: t.colors.shell.rail,
                        width: 1.5,
                        radius: 4.5.into(),
                    },
                )
            };
            iced::widget::container::Style {
                background: Some(background.into()),
                border,
                ..Default::default()
            }
        })
        .into()
}

/// The most characters of an account shown after `Connect ·`. The chip caps
/// at 300px; after its padding, the glyph, `Connect ·`, the gaps and a
/// chevron, about 150px is left, which holds ~22 characters at 13px.
const ACCOUNT_MAX_CHARS: usize = 22;
/// Of those, the account's opening characters kept before the `…`; the rest
/// (less the `…`) come from its end, so both the name and the domain read.
const ACCOUNT_HEAD_CHARS: usize = 10;

/// The account as drawn in the chip: unchanged when it fits, otherwise
/// middle-elided (`olawale494…cock.edu.ng`). Counts characters, not bytes,
/// so a non-ASCII address never splits inside one. The control's
/// accessible label and the Connect panel keep the full address.
pub fn chip_account(account: &str) -> String {
    let chars: Vec<char> = account.chars().collect();
    if chars.len() <= ACCOUNT_MAX_CHARS {
        return account.to_owned();
    }
    let tail = ACCOUNT_MAX_CHARS - ACCOUNT_HEAD_CHARS - 1;
    let mut elided: String = chars[..ACCOUNT_HEAD_CHARS].iter().collect();
    elided.push('…');
    elided.extend(&chars[chars.len() - tail..]);
    elided
}

/// What follows `Connect ·`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectDetail<'a> {
    /// The signed-in account.
    Account(&'a str),
    /// No internet connection.
    Offline,
    /// The session is being checked.
    Checking,
    /// Signed out: the call to action, in accent ink.
    SignIn,
}

/// One Connect control.
pub struct ConnectControl<'a> {
    /// One id in every state, so a status change while it has focus keeps
    /// focus on it.
    pub id: Id,
    pub detail: ConnectDetail<'a>,
    pub light: Option<ConnectLight>,
    /// The pulsing light's low phase (never set under reduced motion).
    pub light_dim: bool,
    /// A chevron for a control that opens a panel.
    pub chevron: bool,
    /// The panel is open.
    pub open: bool,
    /// The accessible name: the account plus what the light means.
    pub label: String,
}

pub fn connect_control<'a, M: Clone + 'static>(
    control: ConnectControl<'a>,
    on_press: Option<M>,
    focus: impl FnOnce(Id) -> M,
) -> Element<'a, M> {
    let mut avatar = iced::widget::Stack::new()
        .width(Length::Fixed(20.0))
        .height(Length::Fixed(20.0))
        .push(
            container(glyph::glyph(glyph::USER, 16.0, glyph::muted))
                .center_x(Length::Fill)
                .center_y(Length::Fill),
        );
    if let Some(light) = control.light {
        avatar = avatar.push(
            container(status_light(light, control.light_dim))
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(iced::alignment::Horizontal::Right)
                .align_y(iced::alignment::Vertical::Top),
        );
    }
    let sign_in = control.detail == ConnectDetail::SignIn;
    let detail = match control.detail {
        ConnectDetail::Account(email) => chip_account(email),
        ConnectDetail::Offline => "Offline".to_owned(),
        ConnectDetail::Checking => "Checking…".to_owned(),
        ConnectDetail::SignIn => "Sign in".to_owned(),
    };
    let detail_style = move |t: &theme::Theme| iced::widget::text::Style {
        color: Some(if sign_in {
            t.colors.shell.accent_ink
        } else {
            t.colors.shell.muted
        }),
    };
    let mut content = row![
        avatar,
        text("Connect").size(13).font(font::SEMIBOLD),
        text("·")
            .size(13)
            .style(|t: &theme::Theme| iced::widget::text::Style {
                color: Some(t.colors.shell.muted),
            }),
        // Clipped to the width the row gives it: if a platform's font or
        // display scale still draws the account wider than the chip, it is
        // cut at the chip's edge instead of painting over the next control.
        container(
            text(detail)
                .size(13)
                .font(if sign_in {
                    font::SEMIBOLD
                } else {
                    font::REGULAR
                })
                .wrapping(text::Wrapping::None)
                .style(detail_style),
        )
        .clip(true),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    if control.chevron {
        content = content.push(glyph::glyph(glyph::CHEVRON_DOWN, 16.0, glyph::muted));
    }
    let open = control.open;
    let widget = container(
        Action::new(
            control.id,
            container(content).center_y(Length::Fixed(CHIP_HEIGHT - 2.0)),
            focus,
        )
        .padding([0, 10])
        .style(chip_button)
        .focus_radius(CHIP_RADIUS)
        .on_press_maybe(on_press),
    )
    .max_width(300)
    .style(move |t: &theme::Theme| {
        let mut style = chip_style(t);
        if open {
            style.background = Some(t.colors.shell.selected.into());
            style.border.color = t.colors.shell.selected;
        }
        style
    });
    bar::accessible(widget, control.label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chip_account_keeps_short_accounts() {
        assert_eq!(chip_account("alex@example.test"), "alex@example.test");
        // Exactly at the limit: nothing to gain from eliding.
        let at_limit = "a".repeat(ACCOUNT_MAX_CHARS);
        assert_eq!(chip_account(&at_limit), at_limit);
    }

    #[test]
    fn chip_account_middle_elides_long_accounts() {
        let shown = chip_account("olawale4947@student.babcock.edu.ng");
        assert_eq!(shown, "olawale494…cock.edu.ng");
        assert_eq!(shown.chars().count(), ACCOUNT_MAX_CHARS);
    }

    #[test]
    fn chip_account_counts_characters_not_bytes() {
        // Multi-byte characters on both sides of the cut must not panic or
        // split, and the result still holds the character budget.
        let account = "ñandú.pérez.álvarez@universidad.ejemplo.es";
        let shown = chip_account(account);
        assert_eq!(shown.chars().count(), ACCOUNT_MAX_CHARS);
        assert!(shown.starts_with("ñandú.pére"));
        assert!(shown.ends_with("ejemplo.es"));
    }
}
