use coincube_ui::{color, theme};
use iced::{
    widget::{text, Container, Row},
    Alignment, Element, Length,
};
use iced_anim::AnimationBuilder;
use std::time::{SystemTime, UNIX_EPOCH};

const DOT_SIZE: f32 = 20.0;
const MIN_OPACITY: f32 = 0.3;
const MAX_OPACITY: f32 = 1.0;
const DOT_SPACING: f32 = 8.0;
const TEXT_SIZE: f32 = 18.0;
const TEXT_SPACING: f32 = 12.0;
const ANIMATION_SPEED: f64 = 3.0;

pub fn loading_indicator<'a, Message: 'a + Clone>(
    message: Option<&'a str>,
) -> Element<'a, Message, theme::Theme, iced::Renderer> {
    use iced_anim::spring::Motion;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();

    let phase = ((now as f64 * ANIMATION_SPEED / 1000.0) % 3.0) as f32;

    let dot1_intensity = 1.0 - (phase - 0.0).abs().min(1.0);
    let dot2_intensity = 1.0 - (phase - 1.0).abs().min(1.0);
    let dot3_intensity = 1.0 - (phase - 2.0).abs().min(1.0);

    let mut content = Row::new().spacing(TEXT_SPACING).align_y(Alignment::Center);

    if let Some(message_text) = message {
        if !message_text.is_empty() {
            content = content.push(
                text(message_text)
                    .size(TEXT_SIZE)
                    .style(theme::text::primary),
            );
        }
    }

    let create_dot = |intensity: f32| {
        AnimationBuilder::new(intensity, move |animated_val| {
            let opacity = MIN_OPACITY + (animated_val * (MAX_OPACITY - MIN_OPACITY));

            text("●")
                .size(DOT_SIZE)
                .style(move |_theme: &theme::Theme| iced::widget::text::Style {
                    color: Some(iced::Color {
                        a: opacity,
                        ..color::ORANGE
                    }),
                })
                .into()
        })
        .animation(Motion::SMOOTH)
        .animates_layout(false)
    };

    content = content.push(
        Row::new()
            .spacing(DOT_SPACING)
            .align_y(Alignment::Center)
            .push(create_dot(dot1_intensity))
            .push(create_dot(dot2_intensity))
            .push(create_dot(dot3_intensity)),
    );

    Container::new(content)
        .width(Length::Fill)
        .center_x(Length::Fill)
        .into()
}
