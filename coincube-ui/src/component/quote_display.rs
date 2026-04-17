use std::collections::VecDeque;
use std::sync::LazyLock;

use iced::widget::image;
use iced::{Alignment, Length};
use rand::seq::SliceRandom;
use serde::Deserialize;

use crate::component::text::{CAPTION_SIZE, P2_SIZE};
use crate::widget::{Column, Container};
use crate::{color, font, theme};

// ---------------------------------------------------------------------------
// Quote data types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct Quote {
    pub id: String,
    pub text: String,
    pub author: String,
    pub source: String,
    pub category: String,
    pub length: String,
    pub contexts: Vec<String>,
}

#[derive(Deserialize)]
struct QuotesFile {
    quotes: Vec<Quote>,
}

// ---------------------------------------------------------------------------
// Embedded quote data
// ---------------------------------------------------------------------------

const QUOTES_JSON: &str = include_str!("../../static/loading-quotes.json");

static ALL_QUOTES: LazyLock<Vec<Quote>> = LazyLock::new(|| {
    serde_json::from_str::<QuotesFile>(QUOTES_JSON)
        .expect("loading-quotes.json must be valid")
        .quotes
});

// ---------------------------------------------------------------------------
// Embedded images (context defaults)
// ---------------------------------------------------------------------------

const IMG_CUBE_ACTIVE: &[u8] = include_bytes!("../../static/images/cube/cube-active.png");
const IMG_CUBE_SEALED: &[u8] = include_bytes!("../../static/images/cube/cube-sealed.png");
const IMG_KAGE: &[u8] = include_bytes!("../../static/images/kage/kage.png");
const IMG_KAGE_SEATED: &[u8] = include_bytes!("../../static/images/kage/kage-seated.png");
const IMG_KAGE_FRONT: &[u8] = include_bytes!("../../static/images/kage/kage-front.png");
const IMG_KAGE_BUST: &[u8] = include_bytes!("../../static/images/kage/kage-bust.png");
const IMG_KAGE_P2P_SUCCESS: &[u8] = include_bytes!("../../static/images/kage/kage-p2p-success.png");
const IMG_KAGE_LIGHTNING_SEND: &[u8] =
    include_bytes!("../../static/images/kage/kage-lightning-send.png");
const IMG_KAGE_LIGHTNING_RECEIVE: &[u8] =
    include_bytes!("../../static/images/kage/kage-lightning-receive.png");
const IMG_KAGE_NOTE_SEND: &[u8] = include_bytes!("../../static/images/kage/kage-note-send.png");
const IMG_KAGE_NOTE_RECEIVE: &[u8] =
    include_bytes!("../../static/images/kage/kage-note-receive.png");
const IMG_KAGE_SPARK_SEND: &[u8] = include_bytes!("../../static/images/kage/kage-spark-send.png");
const IMG_KAGE_SPARK_RECEIVE: &[u8] =
    include_bytes!("../../static/images/kage/kage-spark-receive.png");
const IMG_KAGE_LIQUID_SEND: &[u8] =
    include_bytes!("../../static/images/kage/kage-liquid-send.png");
const IMG_KAGE_LIQUID_RECEIVE: &[u8] =
    include_bytes!("../../static/images/kage/kage-liquid-receive.png");
const IMG_KAGE_BITCOIN_SEND: &[u8] =
    include_bytes!("../../static/images/kage/kage-bitcoin-send.png");
const IMG_KAGE_BITCOIN_RECEIVE: &[u8] =
    include_bytes!("../../static/images/kage/kage-bitcoin-receive.png");

/// Returns the default `(image_bytes, image_height_px)` for a given context key.
pub fn default_image_for_context(context: &str) -> (&'static [u8], u16) {
    match context {
        "loading" | "syncing" => (IMG_CUBE_ACTIVE, 160),
        "first-launch" => (IMG_KAGE, 400),
        "empty-wallet" => (IMG_KAGE_SEATED, 240),
        "transaction-sent" => (IMG_KAGE_FRONT, 240),
        "transaction-received" => (IMG_KAGE_P2P_SUCCESS, 240),
        "transaction-confirm" => (IMG_CUBE_SEALED, 160),
        "backup-reminder" => (IMG_KAGE_FRONT, 240),
        "error" => (IMG_KAGE_BUST, 160),
        "idle" => (IMG_KAGE_FRONT, 240),
        "balance-update" => (IMG_CUBE_ACTIVE, 120),
        // Network-specific celebration images
        "lightning-send" => (IMG_KAGE_LIGHTNING_SEND, 240),
        "lightning-receive" => (IMG_KAGE_LIGHTNING_RECEIVE, 240),
        "note-send" => (IMG_KAGE_NOTE_SEND, 240),
        "note-receive" => (IMG_KAGE_NOTE_RECEIVE, 240),
        "spark-send" => (IMG_KAGE_SPARK_SEND, 240),
        "spark-receive" => (IMG_KAGE_SPARK_RECEIVE, 240),
        "liquid-send" => (IMG_KAGE_LIQUID_SEND, 240),
        "liquid-receive" => (IMG_KAGE_LIQUID_RECEIVE, 240),
        "bitcoin-send" => (IMG_KAGE_BITCOIN_SEND, 240),
        "bitcoin-receive" => (IMG_KAGE_BITCOIN_RECEIVE, 240),
        _ => (IMG_KAGE_SEATED, 240),
    }
}

// ---------------------------------------------------------------------------
// QuoteProvider — stateful quote selector with recency tracking
// ---------------------------------------------------------------------------

const RECENCY_WINDOW: usize = 5;

#[derive(Debug, Clone, Default)]
pub struct QuoteProvider {
    recent_ids: VecDeque<String>,
}

/// Pick a single random quote for a context without recency tracking.
/// Use this for one-shot selections (init, celebrations). For repeated
/// selections during a session (e.g. loading screen rotation), use
/// [`QuoteProvider`] to avoid showing the same quote back-to-back.
pub fn random_quote(context: &str) -> Quote {
    QuoteProvider::new().select(context)
}

impl QuoteProvider {
    pub fn new() -> Self {
        Self::default()
    }

    /// Select a quote for the given context key.
    ///
    /// Filters by context, excludes recently shown quotes, prefers short
    /// quotes for brief-duration states, then picks uniformly at random.
    pub fn select(&mut self, context: &str) -> Quote {
        let prefer_short = matches!(
            context,
            "transaction-sent"
                | "transaction-received"
                | "balance-update"
                | "error"
                | "lightning-send"
                | "lightning-receive"
                | "note-send"
                | "note-receive"
                | "spark-send"
                | "spark-receive"
                | "liquid-send"
                | "liquid-receive"
                | "bitcoin-send"
                | "bitcoin-receive"
        );

        // For network-specific contexts, fall back to the generic
        // transaction-sent / transaction-received context when no
        // quotes explicitly list the specific context.
        let fallback_context = if context.ends_with("-send") {
            Some("transaction-sent")
        } else if context.ends_with("-receive") {
            Some("transaction-received")
        } else {
            None
        };

        let candidates: Vec<&Quote> = ALL_QUOTES
            .iter()
            .filter(|q| {
                q.contexts.iter().any(|c| c == context)
                    || fallback_context
                        .map(|fb| q.contexts.iter().any(|c| c == fb))
                        .unwrap_or(false)
            })
            .filter(|q| !self.recent_ids.contains(&q.id))
            .collect();

        // Fall back to full context pool if recency excludes everything
        let pool = if candidates.is_empty() {
            ALL_QUOTES
                .iter()
                .filter(|q| {
                    q.contexts.iter().any(|c| c == context)
                        || fallback_context
                            .map(|fb| q.contexts.iter().any(|c| c == fb))
                            .unwrap_or(false)
                })
                .collect::<Vec<_>>()
        } else {
            candidates
        };

        // Prefer short quotes for brief-duration states
        let final_pool = if prefer_short {
            let short: Vec<_> = pool
                .iter()
                .filter(|q| q.length == "short")
                .copied()
                .collect();
            if short.is_empty() {
                pool
            } else {
                short
            }
        } else {
            pool
        };

        let mut rng = rand::thread_rng();
        let chosen = final_pool
            .choose(&mut rng)
            .copied()
            .unwrap_or_else(|| ALL_QUOTES.first().expect("quotes must not be empty"));

        // Track recency
        self.recent_ids.push_back(chosen.id.clone());
        if self.recent_ids.len() > RECENCY_WINDOW {
            self.recent_ids.pop_front();
        }

        chosen.clone()
    }
}

// ---------------------------------------------------------------------------
// QuoteDisplay — stateless display component
// ---------------------------------------------------------------------------

/// Create a cached `image::Handle` for a given context key.
///
/// Call this once when entering a context and store the result. Do NOT call
/// on every render — that causes the image to flicker because Iced treats
/// each new `Handle` as a different image.
pub fn image_handle_for_context(context: &str) -> image::Handle {
    let (bytes, _) = default_image_for_context(context);
    image::Handle::from_bytes(bytes)
}

/// Props for the [`quote_display`] component.
pub struct QuoteDisplayProps<'a> {
    pub quote: &'a Quote,
    pub image_handle: &'a image::Handle,
    pub image_size: u16,
    pub show_quote: bool,
    pub show_separator: bool,
}

impl<'a> QuoteDisplayProps<'a> {
    pub fn new(context: &str, quote: &'a Quote, image_handle: &'a image::Handle) -> Self {
        let (_, default_size) = default_image_for_context(context);
        Self {
            quote,
            image_handle,
            image_size: default_size,
            show_quote: true,
            show_separator: true,
        }
    }

    pub fn image_size(mut self, size: u16) -> Self {
        self.image_size = size;
        self
    }

    pub fn show_quote(mut self, show: bool) -> Self {
        self.show_quote = show;
        self
    }

    pub fn show_separator(mut self, show: bool) -> Self {
        self.show_separator = show;
        self
    }
}

/// Renders an image paired with a contextual quote.
///
/// ```text
/// ┌─────────────────────────────┐
/// │        [Image]              │
/// ├─────────────────────────────┤  ← 1px separator
/// │  "Quote text here..."       │
/// │  — Author, Source           │
/// └─────────────────────────────┘
/// ```
pub fn display<'a, M: 'a>(props: &QuoteDisplayProps<'a>) -> Container<'a, M> {
    // Image — uses a pre-built handle to avoid flicker from recreating on every render
    let img = iced::widget::image(props.image_handle.clone())
        .height(Length::Fixed(props.image_size as f32))
        .content_fit(iced::ContentFit::ScaleDown);

    let img_container = Container::new(img).center_x(Length::Fill);

    let mut col = Column::new().align_x(Alignment::Center).push(img_container);

    // Separator (1px line, same pattern as component::separation)
    if props.show_separator && props.show_quote {
        col = col.push(
            Container::new(
                Container::new(Column::new().push(iced::widget::space()))
                    .style(theme::container::border)
                    .height(Length::Fixed(1.0))
                    .width(Length::Fill),
            )
            .padding(16),
        );
    }

    // Quote + attribution
    if props.show_quote {
        let quote_text = iced::widget::text(&props.quote.text)
            .size(P2_SIZE)
            .font(font::REGULAR)
            .style(theme::text::secondary)
            .shaping(iced::advanced::text::Shaping::Advanced)
            .align_x(Alignment::Center)
            .width(Length::Fill);

        let mut quote_col = Column::new()
            .align_x(Alignment::Center)
            .spacing(6)
            .padding(24)
            .push(quote_text);

        if let Some(attribution) = format_attribution(props.quote) {
            let attr_text = iced::widget::text(attribution)
                .size(CAPTION_SIZE)
                .font(font::ITALIC)
                .color(color::GREY_3)
                .shaping(iced::advanced::text::Shaping::Advanced)
                .align_x(Alignment::Center)
                .width(Length::Fill);

            quote_col = quote_col.push(attr_text);
        }

        col = col.push(quote_col);
    }

    Container::new(col).center_x(Length::Fill).max_width(448)
}

/// Format the attribution line for a quote.
///
/// - `scripture`: em-dash + author (which contains the verse reference)
/// - All others: em-dash + author + source
fn format_attribution(quote: &Quote) -> Option<String> {
    match quote.category.as_str() {
        "scripture" => Some(format!("\u{2014} {}", quote.author)),
        _ => Some(format!("\u{2014} {}, {}", quote.author, quote.source)),
    }
}
