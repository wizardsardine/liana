//! Decorative artwork selected at draw time from the active Tenshu theme.

use std::sync::LazyLock;

use iced::advanced::{layout, renderer, widget::Tree, Layout, Widget};
use iced::widget::image::{Handle, Image};
use iced::{mouse, ContentFit, Length, Rectangle, Size};

use crate::theme::{ArtworkStyle, Theme, WorldviewMode};
use crate::widget::Element;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Opening,
    EmptyHistory,
    Sent,
    Received,
    Restored,
    KitProtection,
    KitRemoval,
    Pin,
    Mascot,
}

macro_rules! embedded {
    ($($part:literal),+) => {{
        static HANDLE: LazyLock<Option<Handle>> = LazyLock::new(|| {
            decode_png(include_bytes!(concat!(
                "../static/images/worldview/",
                $($part,)+
                ".png"
            )))
        });
        HANDLE.as_ref()
    }};
}

// WGPU can measure decoded handles on first use. Encoded PNG handles enter
// its asynchronous load queue and can briefly fall back to the Samurai image.
fn decode_png(bytes: &[u8]) -> Option<Handle> {
    let rgba = ::image::load_from_memory_with_format(bytes, ::image::ImageFormat::Png)
        .ok()?
        .into_rgba8();
    Some(Handle::from_rgba(
        rgba.width(),
        rgba.height(),
        rgba.into_raw(),
    ))
}

fn alternate(mode: WorldviewMode, slot: Slot) -> Option<&'static Handle> {
    use Slot::*;
    use WorldviewMode::*;

    match (mode, slot) {
        (Samurai, _) => None,
        (Suitcoiner, Opening) => embedded!("suitcoiner-loading"),
        (Suitcoiner, EmptyHistory) => embedded!("suitcoiner-empty"),
        (Suitcoiner, Sent) => embedded!("suitcoiner-sent"),
        (Suitcoiner, Received) => embedded!("suitcoiner-received"),
        (Suitcoiner, Restored) => embedded!("suitcoiner-restored"),
        (Suitcoiner, KitProtection) => embedded!("suitcoiner-protected"),
        (Suitcoiner, KitRemoval) => embedded!("suitcoiner-withdrawn"),
        (Suitcoiner, Pin) => embedded!("suitcoiner-pin"),
        (Suitcoiner, Mascot) => embedded!("suitcoiner-mascot"),
        (Luke, Opening) => embedded!("luke-loading"),
        (Luke, EmptyHistory) => embedded!("luke-empty"),
        (Luke, Sent) => embedded!("luke-sent"),
        (Luke, Received) => embedded!("luke-received"),
        (Luke, Restored) => embedded!("luke-restored"),
        (Luke, KitProtection) => embedded!("luke-protected"),
        (Luke, KitRemoval) => embedded!("luke-withdrawn"),
        (Luke, Pin) => embedded!("luke-pin"),
        (Luke, Mascot) => embedded!("luke-mascot"),
    }
}

/// Existing transaction contexts select an illustration without changing
/// the payment rail or the text shown on the celebration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PaymentArt {
    BitcoinSend,
    BitcoinReceive,
    LightningSend,
    LightningReceive,
    NoteSend,
    NoteReceive,
    SparkSend,
    SparkReceive,
}

impl PaymentArt {
    pub(crate) fn from_context(context: &str) -> Option<Self> {
        Some(match context {
            "bitcoin-send" => Self::BitcoinSend,
            "bitcoin-receive" => Self::BitcoinReceive,
            "lightning-send" => Self::LightningSend,
            "lightning-receive" => Self::LightningReceive,
            "note-send" => Self::NoteSend,
            "note-receive" => Self::NoteReceive,
            "spark-send" => Self::SparkSend,
            "spark-receive" => Self::SparkReceive,
            _ => return None,
        })
    }
}

fn rail_art(
    mode: WorldviewMode,
    artwork: ArtworkStyle,
    payment: PaymentArt,
) -> Option<&'static Handle> {
    macro_rules! collection {
        ($mode:literal, $style:literal) => {
            match payment {
                PaymentArt::BitcoinSend => embedded!($mode, $style, "-bitcoin-send"),
                PaymentArt::BitcoinReceive => embedded!($mode, $style, "-bitcoin-receive"),
                PaymentArt::LightningSend => embedded!($mode, $style, "-lightning-send"),
                PaymentArt::LightningReceive => embedded!($mode, $style, "-lightning-receive"),
                PaymentArt::NoteSend => embedded!($mode, $style, "-note-send"),
                PaymentArt::NoteReceive => embedded!($mode, $style, "-note-receive"),
                PaymentArt::SparkSend => embedded!($mode, $style, "-spark-send"),
                PaymentArt::SparkReceive => embedded!($mode, $style, "-spark-receive"),
            }
        };
    }

    match (mode, artwork) {
        (WorldviewMode::Samurai, ArtworkStyle::Existing) => None,
        (WorldviewMode::Suitcoiner, ArtworkStyle::Existing) => collection!("suitcoiner", ""),
        (WorldviewMode::Luke, ArtworkStyle::Existing) => collection!("luke", ""),
        (WorldviewMode::Samurai, ArtworkStyle::Scene) => collection!("samurai", "-scene"),
        (WorldviewMode::Suitcoiner, ArtworkStyle::Scene) => collection!("suitcoiner", "-scene"),
        (WorldviewMode::Luke, ArtworkStyle::Scene) => collection!("luke", "-scene"),
    }
}

fn scene(mode: WorldviewMode, slot: Slot) -> Option<&'static Handle> {
    macro_rules! collection {
        ($mode:literal) => {
            match slot {
                Slot::Opening | Slot::EmptyHistory => embedded!($mode, "-scene"),
                Slot::Sent => embedded!($mode, "-scene-sent"),
                Slot::Received => embedded!($mode, "-scene-received"),
                Slot::Restored => embedded!($mode, "-scene-restored"),
                Slot::KitProtection => embedded!($mode, "-scene-protected"),
                Slot::KitRemoval => embedded!($mode, "-scene-withdrawn"),
                Slot::Pin => embedded!($mode, "-scene-pin"),
                Slot::Mascot => embedded!($mode, "-scene-mascot"),
            }
        };
    }

    match mode {
        WorldviewMode::Samurai => collection!("samurai"),
        WorldviewMode::Suitcoiner => collection!("suitcoiner"),
        WorldviewMode::Luke => collection!("luke"),
    }
}

/// Whether the selected Restore art replaces the original Samurai backdrop.
pub fn has_full_scene(theme: &Theme, slot: Slot) -> bool {
    theme.artwork() == ArtworkStyle::Scene && scene(theme.worldview(), slot).is_some()
}

/// Thumbnail for an Appearance option. Use the existing set's empty-history
/// image so the picker and activity preview share the same artwork.
pub fn artwork_thumbnail(mode: WorldviewMode, style: ArtworkStyle) -> Option<Handle> {
    static KAGE: LazyLock<Option<Handle>> =
        LazyLock::new(|| decode_png(include_bytes!("../static/images/kage/kage-seated.png")));
    selected(mode, style, Slot::EmptyHistory)
        .map(|(handle, _)| handle)
        .or_else(|| KAGE.as_ref())
        .cloned()
}

/// Artwork on the live Launcher opening face, fading with the other content.
pub fn opening_image<'a, M: 'a>(side: f32, opacity: f32) -> Option<Element<'a, M>> {
    static CUBE: LazyLock<Option<Handle>> =
        LazyLock::new(|| decode_png(include_bytes!("../static/images/cube/cube-active.png")));
    Some(Element::new(Illustration {
        slot: Some(Slot::Opening),
        incumbent: CUBE.as_ref()?.clone(),
        width: Length::Fixed(side),
        height: Length::Fixed(side),
        fit: ContentFit::ScaleDown,
        opacity,
        payment: None,
        existing_only: false,
    }))
}

/// The Overview's worldview art beside the Total balance, `side` square:
/// the mode's scene in a rounded, hairline frame under
/// [`ArtworkStyle::Scene`], else its existing cutout contained at the
/// bottom right with no frame (Kage for Samurai). It uses the empty-history
/// slot's set, the same art as the Appearance thumbnail. Decorative: it has
/// no label and stays while balances are hidden.
pub fn overview_art<'a, M: 'a>(side: f32) -> Element<'a, M> {
    Element::new(OverviewArt { side })
}

/// Kage (`kage/kage.png`): Samurai's existing Overview art.
fn kage() -> Option<&'static Handle> {
    static KAGE: LazyLock<Option<Handle>> = LazyLock::new(|| {
        decode_png(crate::component::quote_display::default_image_for_context("first-launch").0)
    });
    KAGE.as_ref()
}

/// The Overview art for `mode` and `style`, and whether it is a full scene.
pub fn overview_choice(
    mode: WorldviewMode,
    style: ArtworkStyle,
) -> Option<(&'static Handle, bool)> {
    selected(mode, style, Slot::EmptyHistory).or_else(|| kage().map(|handle| (handle, false)))
}

/// `.art.scene { border-radius: 10px }`.
const OVERVIEW_ART_RADIUS: f32 = 10.0;

struct OverviewArt {
    side: f32,
}

impl<M> Widget<M, Theme, iced::Renderer> for OverviewArt {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fixed(self.side), Length::Fixed(self.side))
    }

    fn layout(
        &mut self,
        _tree: &mut Tree,
        _renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::Node::new(limits.resolve(
            Length::Fixed(self.side),
            Length::Fixed(self.side),
            Size::ZERO,
        ))
    }

    fn draw(
        &self,
        _tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        use iced::advanced::image::Renderer as _;
        use iced::advanced::Renderer as _;

        let bounds = layout.bounds();
        if bounds.intersection(viewport).is_none() {
            return;
        }
        let Some((handle, scene, size)) = overview_choice(theme.worldview(), theme.artwork())
            .and_then(|(handle, scene)| {
                renderer
                    .measure_image(handle)
                    .map(|size| (handle, scene, size))
            })
        else {
            return;
        };
        let natural = Size::new(size.width as f32, size.height as f32);
        let (fitted, radius) = if scene {
            (
                ContentFit::Cover.fit(natural, bounds.size()),
                OVERVIEW_ART_RADIUS,
            )
        } else {
            (ContentFit::Contain.fit(natural, bounds.size()), 0.0)
        };
        // A scene is centred and cropped to the frame; a cutout stands in the
        // bottom-right corner, its feet on the row's baseline.
        let drawn = if scene {
            Rectangle::new(
                iced::Point::new(
                    bounds.center_x() - fitted.width / 2.0,
                    bounds.center_y() - fitted.height / 2.0,
                ),
                fitted,
            )
        } else {
            Rectangle::new(
                iced::Point::new(
                    bounds.x + bounds.width - fitted.width,
                    bounds.y + bounds.height - fitted.height,
                ),
                fitted,
            )
        };
        renderer.draw_image(
            iced::advanced::image::Image {
                handle: handle.clone(),
                filter_method: iced::advanced::image::FilterMethod::Linear,
                rotation: iced::Radians(0.0),
                border_radius: radius.into(),
                opacity: 1.0,
                snap: true,
            },
            drawn,
            bounds,
        );
        if scene {
            renderer.fill_quad(
                renderer::Quad {
                    bounds,
                    border: iced::Border {
                        color: theme.colors.shell.line,
                        width: 1.0,
                        radius: OVERVIEW_ART_RADIUS.into(),
                    },
                    ..renderer::Quad::default()
                },
                iced::Color::TRANSPARENT,
            );
        }
    }
}

fn selected(
    mode: WorldviewMode,
    style: ArtworkStyle,
    slot: Slot,
) -> Option<(&'static Handle, bool)> {
    let full_scene = if style == ArtworkStyle::Scene {
        scene(mode, slot)
    } else {
        None
    };
    full_scene
        .map(|handle| (handle, true))
        .or_else(|| alternate(mode, slot).map(|handle| (handle, false)))
}

/// Keep the illustration row stable across styles. The scene slots reserve
/// a square while existing artwork draws in its original, centered bounds.
pub fn image<'a, M: 'a>(
    slot: Option<Slot>,
    incumbent: Handle,
    width: Length,
    height: Length,
    fit: ContentFit,
) -> Element<'a, M> {
    Element::new(Illustration::new(slot, incumbent, width, height, fit))
}

pub(crate) fn quote_image<'a, M: 'a>(
    slot: Option<Slot>,
    incumbent: Handle,
    height: u16,
    payment: Option<PaymentArt>,
    existing_only: bool,
) -> Element<'a, M> {
    let mut art = Illustration::new(
        slot,
        incumbent,
        Length::Shrink,
        Length::Fixed(f32::from(height)),
        ContentFit::ScaleDown,
    );
    art.payment = payment;
    art.existing_only = existing_only;
    Element::new(art)
}

struct Illustration {
    slot: Option<Slot>,
    incumbent: Handle,
    width: Length,
    height: Length,
    fit: ContentFit,
    opacity: f32,
    payment: Option<PaymentArt>,
    existing_only: bool,
}

impl Illustration {
    fn new(
        slot: Option<Slot>,
        incumbent: Handle,
        width: Length,
        height: Length,
        fit: ContentFit,
    ) -> Self {
        Self {
            slot,
            incumbent,
            width,
            height,
            fit,
            opacity: 1.0,
            payment: None,
            existing_only: false,
        }
    }

    fn choice(&self, theme: &Theme) -> Option<(&'static Handle, bool)> {
        let artwork = if self.existing_only {
            ArtworkStyle::Existing
        } else {
            theme.artwork()
        };
        self.payment
            .and_then(|payment| rail_art(theme.worldview(), artwork, payment))
            .map(|handle| (handle, artwork == ArtworkStyle::Scene))
            .or_else(|| {
                self.slot
                    .and_then(|slot| selected(theme.worldview(), artwork, slot))
            })
    }

    fn image(&self, handle: &Handle) -> Image {
        Image::new(handle.clone())
            .width(self.width)
            .height(self.height)
            .content_fit(self.fit)
            .opacity(self.opacity)
    }
}

impl<M> Widget<M, Theme, iced::Renderer> for Illustration {
    fn size(&self) -> Size<Length> {
        Size::new(self.width, self.height)
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let incumbent = <Image as Widget<M, Theme, iced::Renderer>>::layout(
            &mut self.image(&self.incumbent),
            tree,
            renderer,
            limits,
        );
        if self.width != Length::Shrink || self.existing_only || self.slot.is_none() {
            return incumbent;
        }
        let size = incumbent.size();
        let width = size.height.min(limits.max().width).max(size.width);
        layout::Node::with_children(
            Size::new(width, size.height),
            vec![incumbent.move_to(iced::Point::new((width - size.width) / 2.0, 0.0))],
        )
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        use iced::advanced::image::Renderer as _;

        let (handle, full_scene) = self
            .choice(theme)
            .filter(|(handle, _)| renderer.measure_image(handle).is_some())
            .unwrap_or((&self.incumbent, false));
        let layout = if full_scene {
            layout
        } else {
            layout.children().next().unwrap_or(layout)
        };
        <Image as Widget<M, Theme, iced::Renderer>>::draw(
            &self.image(handle),
            tree,
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Overview art: a distinct picture for every worldview and artwork
    /// style, Kage for Samurai's existing set. (tiny-skia ignores an image's
    /// corner radius, so the scene frame's rounding shows only in wgpu
    /// captures.)
    #[test]
    fn overview_art_draws_each_worldview_and_style() {
        use iced::advanced::renderer::Headless;

        let side = 104.0;
        let mut element = overview_art::<()>(side);
        let mut tree = Tree::new(element.as_widget());
        let mut renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
            crate::font::REGULAR,
            iced::Pixels(16.0),
            Some("tiny-skia"),
        ))
        .expect("software renderer");
        let size = Size::new(side, side);
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(Size::ZERO, Size::new(400.0, 400.0)),
        );
        assert_eq!(node.size(), size);
        let viewport = Rectangle::with_size(size);
        let background = iced::Color::from_rgb8(1, 2, 3);
        let mut captures = std::collections::HashSet::new();
        for mode in [
            WorldviewMode::Samurai,
            WorldviewMode::Suitcoiner,
            WorldviewMode::Luke,
        ] {
            for style in [ArtworkStyle::Scene, ArtworkStyle::Existing] {
                let (_, scene) = overview_choice(mode, style).expect("Overview art");
                assert_eq!(scene, style == ArtworkStyle::Scene, "{mode:?} {style:?}");
                let theme = Theme::dark().with_worldview(mode).with_artwork(style);
                element.as_widget().draw(
                    &tree,
                    &mut renderer,
                    &theme,
                    &renderer::Style {
                        text_color: iced::Color::WHITE,
                    },
                    Layout::new(&node),
                    mouse::Cursor::Unavailable,
                    &viewport,
                );
                let pixels = renderer.screenshot(Size::new(104, 104), 1.0, background);
                let blank = [1u8, 2, 3];
                assert!(
                    pixels.chunks_exact(4).any(|pixel| pixel[..3] != blank),
                    "{mode:?} {style:?} draws"
                );
                assert!(
                    captures.insert(pixels),
                    "{mode:?} {style:?} should draw distinct pixels"
                );
                iced::advanced::Renderer::reset(&mut renderer, viewport);
            }
        }
        assert_eq!(
            overview_choice(WorldviewMode::Samurai, ArtworkStyle::Existing)
                .map(|(handle, _)| handle.id()),
            kage().map(Handle::id)
        );
    }

    #[test]
    fn mounted_illustration_draws_the_selected_worldview() {
        use iced::advanced::renderer::Headless;

        let incumbent = Handle::from_rgba(1, 1, vec![255, 255, 255, 255]);
        let mut element = image::<()>(
            Some(Slot::Opening),
            incumbent,
            Length::Fixed(64.0),
            Length::Fixed(64.0),
            ContentFit::Contain,
        );
        let mut tree = Tree::new(element.as_widget());
        let mut renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
            crate::font::REGULAR,
            iced::Pixels(16.0),
            Some("tiny-skia"),
        ))
        .expect("software renderer");
        let size = Size::new(64.0, 64.0);
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(Size::ZERO, size),
        );
        let viewport = Rectangle::with_size(size);
        let mut captures = std::collections::HashSet::new();
        for mode in [
            WorldviewMode::Samurai,
            WorldviewMode::Suitcoiner,
            WorldviewMode::Luke,
        ] {
            let theme = Theme::dark().with_worldview(mode);
            element.as_widget().draw(
                &tree,
                &mut renderer,
                &theme,
                &renderer::Style {
                    text_color: iced::Color::WHITE,
                },
                Layout::new(&node),
                mouse::Cursor::Unavailable,
                &viewport,
            );
            let pixels = renderer.screenshot(Size::new(64, 64), 1.0, iced::Color::BLACK);
            assert!(pixels.chunks_exact(4).any(|pixel| pixel[..3] != [0, 0, 0]));
            assert!(
                captures.insert(pixels),
                "{mode:?} should draw distinct pixels"
            );
            iced::advanced::Renderer::reset(&mut renderer, viewport);
        }
    }

    #[test]
    fn artwork_picker_resolves_full_scenes_for_every_slot() {
        assert!(artwork_thumbnail(WorldviewMode::Samurai, ArtworkStyle::Existing).is_some());
        let mut ids = std::collections::HashSet::new();
        for mode in [
            WorldviewMode::Samurai,
            WorldviewMode::Suitcoiner,
            WorldviewMode::Luke,
        ] {
            let (opening, full_scene) = selected(mode, ArtworkStyle::Scene, Slot::Opening).unwrap();
            assert!(full_scene);
            assert!(ids.insert(opening.id()));
            assert!(matches!(
                opening,
                Handle::Rgba {
                    width: 480,
                    height: 480,
                    ..
                }
            ));
            assert_eq!(
                artwork_thumbnail(mode, ArtworkStyle::Scene).unwrap().id(),
                opening.id()
            );
            for slot in [
                Slot::Opening,
                Slot::EmptyHistory,
                Slot::Sent,
                Slot::Received,
                Slot::Restored,
                Slot::KitProtection,
                Slot::KitRemoval,
                Slot::Pin,
                Slot::Mascot,
            ] {
                let existing = selected(mode, ArtworkStyle::Existing, slot);
                assert_eq!(
                    existing.map(|(handle, _)| handle.id()),
                    alternate(mode, slot).map(Handle::id)
                );
                let (chosen, full_scene) = selected(mode, ArtworkStyle::Scene, slot).unwrap();
                assert!(full_scene);
                if slot == Slot::Opening || slot == Slot::EmptyHistory {
                    assert_eq!(chosen.id(), opening.id());
                } else {
                    assert!(ids.insert(chosen.id()));
                }
            }
        }
    }

    #[test]
    fn artwork_payment_routes_distinguish_rails_in_both_styles() {
        use crate::component::quote_display::{
            default_image_for_context, image_handle_for_context,
        };

        let mut art_contents = std::collections::HashSet::<&[u8]>::new();
        for (context, slot) in [
            ("bitcoin-send", Slot::Sent),
            ("bitcoin-receive", Slot::Received),
            ("lightning-send", Slot::Sent),
            ("lightning-receive", Slot::Received),
            ("note-send", Slot::Sent),
            ("note-receive", Slot::Received),
            ("spark-send", Slot::Sent),
            ("spark-receive", Slot::Received),
        ] {
            let mut art = Illustration::new(
                Some(slot),
                image_handle_for_context(context),
                Length::Shrink,
                Length::Fixed(240.0),
                ContentFit::ScaleDown,
            );
            let rail = PaymentArt::from_context(context).unwrap();
            art.payment = Some(rail);
            let samurai = Theme::dark().with_worldview(WorldviewMode::Samurai);
            assert!(art
                .choice(&samurai.with_artwork(ArtworkStyle::Existing))
                .is_none());
            assert!(art_contents.insert(default_image_for_context(context).0));

            for mode in [WorldviewMode::Suitcoiner, WorldviewMode::Luke] {
                let theme = Theme::dark().with_worldview(mode);
                let (payment, full_scene) = art
                    .choice(&theme.with_artwork(ArtworkStyle::Existing))
                    .unwrap();
                assert!(!full_scene);
                let Handle::Rgba { pixels, .. } = payment else {
                    panic!("{mode:?} {context} must decode before drawing");
                };
                assert!(art_contents.insert(pixels.as_ref()));
            }
            for mode in [
                WorldviewMode::Samurai,
                WorldviewMode::Suitcoiner,
                WorldviewMode::Luke,
            ] {
                let theme = Theme::dark()
                    .with_worldview(mode)
                    .with_artwork(ArtworkStyle::Scene);
                let (handle, full_scene) = art.choice(&theme).unwrap();
                assert!(full_scene);
                assert_ne!(handle.id(), scene(mode, slot).unwrap().id());
                let Handle::Rgba { pixels, .. } = handle else {
                    panic!("{mode:?} {context} scene must decode before drawing");
                };
                assert!(art_contents.insert(pixels.as_ref()));

                art.payment = None;
                assert_eq!(
                    art.choice(&theme).unwrap().0.id(),
                    scene(mode, slot).unwrap().id()
                );
                art.payment = Some(rail);
            }
        }
        assert!(PaymentArt::from_context("transaction-sent").is_none());
    }

    #[test]
    fn corrupt_artwork_has_no_alternate() {
        assert!(decode_png(b"invalid PNG").is_none());
    }

    #[test]
    fn each_worldview_slot_has_its_own_decoded_alternate() {
        let mut ids = std::collections::HashSet::new();
        for slot in [
            Slot::Opening,
            Slot::EmptyHistory,
            Slot::Sent,
            Slot::Received,
            Slot::Restored,
            Slot::KitProtection,
            Slot::KitRemoval,
            Slot::Pin,
            Slot::Mascot,
        ] {
            assert!(alternate(WorldviewMode::Samurai, slot).is_none());
            for mode in [WorldviewMode::Suitcoiner, WorldviewMode::Luke] {
                let handle = alternate(mode, slot).unwrap();
                let Handle::Rgba {
                    width,
                    height,
                    pixels,
                    ..
                } = handle
                else {
                    panic!("Bundled {mode:?} {slot:?} must be measurable on first draw");
                };
                assert!(*width > 0 && *height > 0);
                assert_eq!(pixels.len(), (*width * *height * 4) as usize);
                assert!(ids.insert(handle.id()));
                assert!(std::ptr::eq(handle, alternate(mode, slot).unwrap()));
            }
        }
    }
}
