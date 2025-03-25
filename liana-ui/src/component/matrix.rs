use crate::theme::Theme;
use iced::mouse;
use iced::widget::canvas;
use iced::{Color, Font, Point, Rectangle, Renderer};

use std::cell::RefCell;

#[derive(Default)]
pub struct Matrix {
    tick: usize,
}

impl Matrix {
    pub fn tick(&mut self) {
        self.tick = (self.tick + 1) % CACHES_LEN;
    }
}

const CACHES_LEN: usize = 30;

impl<Message> canvas::Program<Message, Theme> for Matrix {
    type State = RefCell<Vec<canvas::Cache>>;

    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        use rand::distributions::Distribution;
        use rand::Rng;

        const CELL_SIZE: f32 = 10.0;

        let mut caches = state.borrow_mut();

        if caches.is_empty() {
            let group = canvas::Group::unique();

            caches.resize_with(CACHES_LEN, || canvas::Cache::with_group(group));
        }

        vec![
            caches[self.tick % caches.len()].draw(renderer, bounds.size(), |frame| {
                frame.fill_rectangle(Point::ORIGIN, frame.size(), Color::BLACK);

                let mut rng = rand::thread_rng();
                let rows = (frame.height() / CELL_SIZE).ceil() as usize;
                let columns = (frame.width() / CELL_SIZE).ceil() as usize;

                for row in 0..rows {
                    for column in 0..columns {
                        let position =
                            Point::new(column as f32 * CELL_SIZE, row as f32 * CELL_SIZE);

                        let alphas = [0.05, 0.1, 0.2, 0.5];
                        let weights = [10, 4, 2, 1];
                        let distribution = rand::distributions::WeightedIndex::new(weights)
                            .expect("Create distribution");

                        frame.fill_text(canvas::Text {
                            content: rng.gen_range('!'..'z').to_string(),
                            position,
                            color: Color {
                                a: alphas[distribution.sample(&mut rng)],
                                g: 1.0,
                                ..Color::BLACK
                            },
                            size: CELL_SIZE.into(),
                            font: Font::MONOSPACE,
                            ..canvas::Text::default()
                        });
                    }
                }
            }),
        ]
    }
}
