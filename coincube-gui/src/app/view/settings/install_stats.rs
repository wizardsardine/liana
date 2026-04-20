use iced::widget::canvas::{self, Frame, Geometry, Path, Stroke};
use iced::widget::{Column, Row, Space};
use iced::{Alignment, Color, Length, Point, Rectangle};

use coincube_ui::component::{card, text::*};
use coincube_ui::theme;
use coincube_ui::widget::{Button, Element, ProgressBar};

use crate::app::cache;
use crate::app::menu::Menu;
use crate::app::view::dashboard;
use crate::app::view::message::*;
use crate::services::coincube::{DownloadStats, StatsPeriod, TimeseriesPoint};

const DOWNLOAD_GOAL: u32 = 1_000_000;

#[allow(clippy::too_many_arguments)]
pub fn install_stats_section<'a>(
    menu: &'a Menu,
    cache: &'a cache::Cache,
    download_stats: Option<&'a DownloadStats>,
    today_count: Option<u32>,
    timeseries: Option<&'a [TimeseriesPoint]>,
    period: StatsPeriod,
    loading: bool,
    error: Option<&'a str>,
) -> Element<'a, Message> {
    let refresh_msg = Message::Settings(SettingsMessage::InstallStats(
        InstallStatsViewMessage::Refresh,
    ));

    let header_row = Row::new()
        .align_y(Alignment::Center)
        .push(super::header(
            "Download Stats",
            SettingsMessage::InstallStatsSection,
        ))
        .push(Space::new().width(Length::Fill))
        .push(
            Button::new(text("Refresh").size(13).bold())
                .padding([6, 16])
                .style(theme::button::transparent_border)
                .on_press_maybe(if loading {
                    None
                } else {
                    Some(refresh_msg.clone())
                }),
        );

    let mut col = Column::new()
        .spacing(20)
        .push(header_row)
        .width(Length::Fill);

    if let Some(err) = error {
        col = col.push(
            Row::new()
                .spacing(10)
                .align_y(Alignment::Center)
                .push(card::warning(err.to_string()).width(Length::Fill))
                .push(
                    Button::new(text("Retry").size(13).bold())
                        .padding([6, 16])
                        .style(theme::button::transparent_border)
                        .on_press(refresh_msg),
                ),
        );
    }

    col = col.push(downloads_card(download_stats, today_count, loading));
    col = col.push(chart_card(timeseries, period, loading));

    dashboard(menu, cache, col)
}

fn downloads_card<'a>(
    stats: Option<&'a DownloadStats>,
    today_count: Option<u32>,
    loading: bool,
) -> Element<'a, Message> {
    let today_label = match today_count {
        Some(n) => format!("+{} downloads today", n),
        None if loading => "Loading…".to_string(),
        None => "—".to_string(),
    };

    let inner = Column::new().spacing(8).push(
        text("TOTAL DOWNLOADS")
            .size(11)
            .style(theme::text::secondary),
    );

    let inner = if let Some(s) = stats {
        let total = s.total;
        let raw_pct = if DOWNLOAD_GOAL > 0 {
            total as f64 / DOWNLOAD_GOAL as f64
        } else {
            0.0
        };
        let bar_pct = (raw_pct as f32).clamp(0.0, 1.0);
        inner
            .push(text(format_count(total)).size(38).bold())
            .push(
                text(format!("of {} downloads", format_count(DOWNLOAD_GOAL)))
                    .size(13)
                    .style(theme::text::secondary),
            )
            .push(Space::new().height(Length::Fixed(4.0)))
            .push(ProgressBar::new(0.0..=1.0, bar_pct))
            .push(
                Row::new().push(Space::new().width(Length::Fill)).push(
                    text(format!("{:.2}% of goal", raw_pct * 100.0))
                        .size(12)
                        .style(theme::text::secondary),
                ),
            )
    } else {
        inner.push(
            text(if loading { "Loading…" } else { "—" })
                .size(38)
                .bold()
                .style(theme::text::secondary),
        )
    };

    let today_row = Row::new()
        .align_y(Alignment::Center)
        .push(Space::new().width(Length::Fill))
        .push(text(today_label).size(14).style(theme::text::secondary))
        .push(Space::new().width(Length::Fill));

    card::simple(Column::new().spacing(10).push(inner).push(today_row))
        .width(Length::Fill)
        .into()
}

fn chart_card<'a>(
    timeseries: Option<&'a [TimeseriesPoint]>,
    period: StatsPeriod,
    loading: bool,
) -> Element<'a, Message> {
    let chart_area: Element<'a, Message> = if loading && timeseries.is_none() {
        Row::new()
            .push(Space::new().width(Length::Fill))
            .push(text("Loading…").size(13).style(theme::text::secondary))
            .push(Space::new().width(Length::Fill))
            .height(Length::Fixed(160.0))
            .into()
    } else if let Some(points) = timeseries {
        if points.is_empty() {
            Row::new()
                .push(Space::new().width(Length::Fill))
                .push(text("No data").size(13).style(theme::text::secondary))
                .push(Space::new().width(Length::Fill))
                .height(Length::Fixed(160.0))
                .into()
        } else {
            let chart = LineChart {
                points: points.to_vec(),
            };
            canvas::Canvas::new(chart)
                .width(Length::Fill)
                .height(Length::Fixed(160.0))
                .into()
        }
    } else {
        Space::new().height(Length::Fixed(160.0)).into()
    };

    let period_buttons = Row::new()
        .spacing(8)
        .push(period_btn(StatsPeriod::Day, period))
        .push(period_btn(StatsPeriod::Week, period))
        .push(period_btn(StatsPeriod::Month, period))
        .push(period_btn(StatsPeriod::Year, period));

    card::simple(
        Column::new()
            .spacing(12)
            .push(chart_area)
            .push(period_buttons),
    )
    .width(Length::Fill)
    .into()
}

fn period_btn(p: StatsPeriod, current: StatsPeriod) -> Element<'static, Message> {
    let is_active = p == current;
    Button::new(text(p.label()).size(13).bold().style(if is_active {
        theme::text::primary
    } else {
        theme::text::secondary
    }))
    .padding([6, 16])
    .style(if is_active {
        theme::button::primary
    } else {
        theme::button::transparent_border
    })
    .on_press(Message::Settings(SettingsMessage::InstallStats(
        InstallStatsViewMessage::PeriodChanged(p),
    )))
    .into()
}

fn format_count(n: u32) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(' ');
        }
        result.push(ch);
    }
    result.chars().rev().collect()
}

struct LineChart {
    points: Vec<TimeseriesPoint>,
}

impl canvas::Program<Message, coincube_ui::theme::Theme> for LineChart {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        theme: &coincube_ui::theme::Theme,
        bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<Geometry<iced::Renderer>> {
        let mut frame = Frame::new(renderer, bounds.size());

        if self.points.is_empty() {
            return vec![frame.into_geometry()];
        }

        let is_light = theme.mode == coincube_ui::theme::palette::ThemeMode::Light;

        // Theme-aware colors for the chart
        let grid_color = if is_light {
            Color::from_rgba(0.0, 0.0, 0.0, 0.08)
        } else {
            Color::from_rgba(1.0, 1.0, 1.0, 0.07)
        };
        let label_color = if is_light {
            Color::from_rgba(0.0, 0.0, 0.0, 0.45)
        } else {
            Color::from_rgba(1.0, 1.0, 1.0, 0.4)
        };
        let line_color = if is_light {
            Color::from_rgb(0.97, 0.57, 0.10) // orange
        } else {
            Color::WHITE
        };
        let fill_color = if is_light {
            Color::from_rgba(0.97, 0.57, 0.10, 0.12)
        } else {
            Color::from_rgba(1.0, 1.0, 1.0, 0.08)
        };
        let dot_color = line_color;

        let pad_left = 38.0_f32;
        let pad_right = 10.0_f32;
        let pad_top = 10.0_f32;
        let pad_bottom = 24.0_f32;

        let w = bounds.width - pad_left - pad_right;
        let h = bounds.height - pad_top - pad_bottom;

        let min_val = self.points.iter().map(|p| p.count).min().unwrap_or(0);
        let max_val = self.points.iter().map(|p| p.count).max().unwrap_or(1);
        let range = (max_val - min_val).max(1);

        let n = self.points.len();

        let map_x = |i: usize| pad_left + (i as f32 / (n - 1).max(1) as f32) * w;
        let map_y = |v: u32| pad_top + h - ((v.saturating_sub(min_val)) as f32 / range as f32) * h;

        let pts: Vec<Point> = self
            .points
            .iter()
            .enumerate()
            .map(|(i, p)| Point::new(map_x(i), map_y(p.count)))
            .collect();

        for step in 0..=4 {
            let y = pad_top + (step as f32 / 4.0) * h;
            let grid_line = Path::line(Point::new(pad_left, y), Point::new(pad_left + w, y));
            frame.stroke(
                &grid_line,
                Stroke::default().with_color(grid_color).with_width(1.0),
            );

            let val = max_val.saturating_sub((step as u64 * range as u64 / 4) as u32);
            let label = format_count(val);
            frame.fill_text(canvas::Text {
                content: label,
                position: Point::new(0.0, y - 6.0),
                color: label_color,
                size: iced::Pixels(9.0),
                ..canvas::Text::default()
            });
        }

        let fill_path = Path::new(|builder| {
            builder.move_to(Point::new(pts[0].x, pad_top + h));
            builder.line_to(pts[0]);
            for &pt in &pts[1..] {
                builder.line_to(pt);
            }
            builder.line_to(Point::new(pts[n - 1].x, pad_top + h));
            builder.close();
        });
        frame.fill(&fill_path, fill_color);

        let line_path = Path::new(|builder| {
            builder.move_to(pts[0]);
            for &pt in &pts[1..] {
                builder.line_to(pt);
            }
        });
        frame.stroke(
            &line_path,
            Stroke::default().with_color(line_color).with_width(2.0),
        );

        let last = pts[n - 1];
        let dot = Path::circle(last, 5.0);
        frame.fill(&dot, dot_color);

        let label_step = ((n as f32 / 7.0).ceil() as usize).max(1);
        for (i, p) in self.points.iter().enumerate() {
            if i % label_step != 0 && i != n - 1 {
                continue;
            }
            let x = map_x(i);
            let label = short_date_label(&p.date);
            let text_x = (x - 10.0).max(0.0);
            frame.fill_text(canvas::Text {
                content: label,
                position: Point::new(text_x, pad_top + h + 8.0),
                color: label_color,
                size: iced::Pixels(9.0),
                ..canvas::Text::default()
            });
        }

        vec![frame.into_geometry()]
    }
}

fn short_date_label(date: &str) -> String {
    // date is "YYYY-MM-DD"; show "MM/DD"
    if date.len() >= 10 {
        format!("{}/{}", &date[5..7], &date[8..10])
    } else {
        date.to_string()
    }
}
