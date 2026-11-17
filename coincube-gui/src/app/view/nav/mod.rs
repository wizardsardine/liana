//! Cube navigation. [`shell`] paints the single text sidebar and top bar on
//! every live Cube screen. The older rail modules still provide route/item
//! inventories while screen families migrate; their rail renderers are not
//! mounted by the current dashboard or P2P panels.

pub mod items;
pub mod primary;
pub mod secondary;
pub mod shell;
pub mod split;

pub mod cube;
pub mod marketplace;
pub mod spark;
pub mod tertiary;
pub mod vault;

pub use items::SubItem;

use crate::app::{menu::Menu, view::Message};
use coincube_ui::{
    color,
    component::text,
    icon::cube_icon,
    image::{tenshu_wordmark, theme_toggle_button},
    theme,
    widget::{Button, Column, ColumnExt, Element, Row},
};
use iced::{
    widget::{column, container, row, scrollable, text::Wrapping},
    Alignment, Length,
};

/// Width of the primary + secondary rails. The tertiary rail is an
/// overlay on the content area, so the sidebar column itself is always
/// this width. Used for the wordmark / identity / toggle bands and for
/// the toast offset.
pub const SIDEBAR_BASE_WIDTH: f32 = primary::RAIL_WIDTH + secondary::RAIL_WIDTH;

/// Height of the logo header band that caps both rails. Tall enough for the
/// stacked Tenshu lockup (mark ~2× the label + the "Tenshu" label).
const WORDMARK_BAND_HEIGHT: f32 = 60.0;
/// Label font size for the Tenshu lockup in the compact two-rail header.
const WORDMARK_SIZE: f32 = 14.0;
/// Fixed height for the identity block (avatar + cube name + LN
/// address). Fixed so the tertiary overlay's top offset is deterministic.
const IDENTITY_BLOCK_HEIGHT: f32 = 130.0;

/// Vertical distance from the top of the sidebar column to the top of
/// the rails row (primary + secondary). The tertiary overlay uses this
/// to position itself so its items align horizontally with the rails.
pub const TERTIARY_TOP_OFFSET: f32 = WORDMARK_BAND_HEIGHT + IDENTITY_BLOCK_HEIGHT;

/// Composed sidebar: wordmark · identity block · two rails · theme toggle.
///
/// Layout (top to bottom):
/// ```text
/// [ Tenshu logo (mark + label)    ]  full 144px
/// [ avatar + cube name + LN addr  ]  full 144px — pushes both rails down
/// [ primary (72px) | secondary (72px) ]  fills the remaining height
/// [ dark/light toggle             ]  full 144px
/// ```
///
/// The two rails sit inside a single scrollable that takes all the height
/// left between the identity block and the toggle. When the rails are
/// taller than that (short windows, many submenu items) they scroll
/// together rather than pushing the toggle off-screen or being clipped.
/// The rails themselves are content-sized and unstyled; the surrounding
/// container paints the shared `sidebar_primary` background so it covers
/// the whole band even when the rails are shorter than the viewport.
pub fn sidebar<'a>(menu: &Menu, ctx: &NavContext<'a>) -> Element<'a, Message> {
    // Base sidebar is always 144px wide (primary + secondary rails only).
    // The tertiary rail lives as an overlay on the content area — see
    // [`tertiary_rail`] and `dashboard_with_info` — so content never
    // shifts horizontally when the tertiary rail slides out.
    let rails_row: Row<Message> = row![primary::rail(menu, ctx), secondary::rail(menu, ctx)]
        .width(Length::Fixed(SIDEBAR_BASE_WIDTH));
    let rails = container(
        scrollable(rails_row)
            .width(Length::Fixed(SIDEBAR_BASE_WIDTH))
            .height(Length::Fill),
    )
    .width(Length::Fixed(SIDEBAR_BASE_WIDTH))
    .height(Length::Fill)
    .style(theme::container::sidebar_primary);

    let wordmark = container(tenshu_wordmark(WORDMARK_SIZE))
        .width(Length::Fixed(SIDEBAR_BASE_WIDTH))
        .height(Length::Fixed(WORDMARK_BAND_HEIGHT))
        .center_x(Length::Fixed(SIDEBAR_BASE_WIDTH))
        .center_y(Length::Fixed(WORDMARK_BAND_HEIGHT))
        .style(theme::container::sidebar_primary);

    let identity = identity_block(ctx);
    let toggle = theme_toggle_row(ctx);

    let col: Column<Message> = column![wordmark, identity, rails, toggle]
        .push_maybe(
            ctx.connect_authenticated
                .then(|| coincube_ui::component::session_status::indicator(false)),
        )
        .width(Length::Fixed(SIDEBAR_BASE_WIDTH))
        .height(Length::Fill)
        .align_x(Alignment::Start);
    container(col)
        .width(Length::Fixed(SIDEBAR_BASE_WIDTH))
        .height(Length::Fill)
        .style(theme::container::sidebar_primary)
        .into()
}

/// Tertiary rail for the current route, to be overlaid on the left
/// edge of the content area by `dashboard_with_info`. Returns `None`
/// when the active route has no third level.
pub fn tertiary_rail<'a>(menu: &Menu, ctx: &NavContext<'a>) -> Option<Element<'a, Message>> {
    tertiary::rail(menu, ctx)
}

/// Full-width identity header — avatar + cube name + optional Lightning
/// Address. Rendered above both rails so the top of each rail is pushed
/// down equally (the first secondary-rail item lines up with the `Home`
/// / `Cube` item on the primary rail).
fn identity_block<'a>(ctx: &NavContext<'a>) -> Element<'a, Message> {
    const AVATAR_SIZE: f32 = 56.0;

    let avatar: Element<'a, Message> = if let Some(handle) = ctx.avatar {
        iced::widget::image(handle.clone())
            .width(Length::Fixed(AVATAR_SIZE))
            .height(Length::Fixed(AVATAR_SIZE))
            .into()
    } else {
        container(cube_icon().size(28).color(color::GREY_3))
            .width(Length::Fixed(AVATAR_SIZE))
            .height(Length::Fixed(AVATAR_SIZE))
            .center_x(Length::Fixed(AVATAR_SIZE))
            .center_y(Length::Fixed(AVATAR_SIZE))
            .style(
                |t: &coincube_ui::theme::Theme| iced::widget::container::Style {
                    background: Some(iced::Background::Color(t.colors.cards.simple.background)),
                    border: iced::Border {
                        radius: (AVATAR_SIZE / 2.0).into(),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .into()
    };

    // The avatar is managed on Cube → Settings → Avatar, so clicking it
    // jumps straight there (rather than Cube → Overview, where there's
    // nothing to do with it).
    let avatar_button: Element<'a, Message> = Button::new(avatar)
        .style(theme::button::transparent)
        .on_press(Message::Menu(Menu::Cube(
            crate::app::menu::CubeSubMenu::Settings(crate::app::menu::CubeSettingsOption::Avatar),
        )))
        .into();

    let mut col: Column<Message> = Column::new()
        .padding([12, 8])
        .spacing(4)
        .align_x(Alignment::Center)
        .width(Length::Fixed(SIDEBAR_BASE_WIDTH))
        .push(avatar_button);

    if !ctx.cube_name.is_empty() {
        let name = text::p2_bold(ctx.cube_name)
            .style(theme::text::primary)
            .align_x(Alignment::Center);
        col = col.push(name);
    }

    if let Some(addr) = ctx.lightning_address {
        let display_addr = if addr.contains('@') {
            addr.to_string()
        } else {
            format!("{}@coincube.io", addr)
        };
        let label = text::caption(display_addr.clone())
            .color(color::GREY_3)
            .width(Length::Fill)
            .wrapping(Wrapping::WordOrGlyph);
        let btn: Button<Message> = Button::new(label)
            .width(Length::Fill)
            .style(theme::button::transparent)
            .on_press(Message::Clipboard(display_addr));

        col = col.push(btn);
    }

    container(col)
        .width(Length::Fixed(SIDEBAR_BASE_WIDTH))
        .height(Length::Fixed(IDENTITY_BLOCK_HEIGHT))
        .style(theme::container::sidebar_primary)
        .into()
}

/// Dark/light toggle anchored at the bottom of the sidebar, spanning
/// both rails. Mirrors the placement in the pre-refactor sidebar.
fn theme_toggle_row<'a>(ctx: &NavContext<'a>) -> Element<'a, Message> {
    let toggle = theme_toggle_button(ctx.theme_mode, Message::ToggleTheme);
    container(toggle)
        .width(Length::Fixed(SIDEBAR_BASE_WIDTH))
        .padding(iced::Padding {
            top: 4.0,
            right: 8.0,
            bottom: 16.0,
            left: 8.0,
        })
        .style(theme::container::sidebar_primary)
        .into()
}

/// Ambient data both rails need while rendering.
pub struct NavContext<'a> {
    pub has_vault: bool,
    pub has_imported_bitcoin: bool,
    pub has_p2p: bool,
    /// Active chain identity. Drives per-feature availability gating
    /// (Spark on the primary rail, Buy/Sell + P2P in Marketplace)
    /// via [`crate::app::features`].
    pub network: crate::chain::ChainId,
    /// Whether a non-mainnet Mostro coordinator is configured. Only
    /// affects P2P on test networks (mainnet P2P is always available).
    pub p2p_test_coordinator: bool,
    /// Server-controlled Marketplace availability (from `/connect/features`).
    /// Gates whether the Marketplace top-level nav and its Buy/Sell + P2P
    /// sub-items are shown at all — on top of the per-network gate. Fail-closed
    /// [`MarketplaceServerFlags::OFF`](crate::app::features::MarketplaceServerFlags::OFF)
    /// until features load, hiding the section while the API stance is unknown.
    pub marketplace_flags: crate::app::features::MarketplaceServerFlags,
    pub cube_name: &'a str,
    /// The wallet names after "Spark" and "Vault" in the side panel. See
    /// [`crate::app::cache::WalletNames`].
    pub spark_alias: Option<&'a str>,
    pub vault_alias: Option<&'a str>,
    pub lightning_address: Option<&'a str>,
    pub avatar: Option<&'a iced::widget::image::Handle>,
    pub theme_mode: coincube_ui::theme::palette::ThemeMode,
    pub connect_authenticated: bool,
    pub connect_email: Option<&'a str>,
    /// Latest realtime-stream health for the small status dot rendered
    /// under the cube name in [`identity_block`]. `Inactive` renders
    /// no dot (e.g. local-daemon installs without Connect).
    pub connect_stream_status: &'a crate::app::ConnectionStatus,
    /// The Connect control's chrome: its panel, signed-out notice, light
    /// phase and the Vault line.
    pub connect: ConnectChrome<'a>,
    /// Account-scoped Bitcoin Blake2b grant, mirrored from `/connect/features`
    /// into [`crate::app::cache::Cache::btcb2_server_enabled`].
    pub btcb2_server_enabled: bool,
    /// Whether a Bitcoin Blake2b Cube already reuses this Cube's descriptor.
    pub btcb2_already_claimed: bool,
}

/// What the Connect control and the sidebar's Vault item need beyond the
/// account and stream status.
#[derive(Debug, Clone, Copy, Default)]
pub struct ConnectChrome<'a> {
    pub panel_open: bool,
    pub signed_out_by: Option<crate::app::SignedOutBy>,
    /// The pulsing light is in its dim phase.
    pub light_dim: bool,
    /// The Vault's alert while its recent polls fail.
    pub vault_alert: Option<&'a crate::app::VaultAlert>,
}

impl<'a> ConnectChrome<'a> {
    pub fn from_cache(cache: &'a crate::app::cache::Cache) -> Self {
        Self {
            panel_open: cache.connect_panel_open,
            signed_out_by: cache.connect_signed_out_by,
            light_dim: cache.connect_light_dim && !cache.reduce_motion,
            vault_alert: cache.vault_alert.as_ref(),
        }
    }
}

impl<'a> NavContext<'a> {
    /// The navigation facts the App cache holds. Identity fields (name,
    /// Lightning address, avatar) are left empty for the caller to fill.
    pub fn from_cache(cache: &'a crate::app::cache::Cache) -> Self {
        NavContext {
            has_vault: cache.has_vault,
            has_imported_bitcoin: cache.has_imported_bitcoin,
            has_p2p: cache.has_p2p,
            network: cache.chain(),
            p2p_test_coordinator: cache.p2p_test_coordinator,
            marketplace_flags: cache.marketplace_flags,
            btcb2_server_enabled: cache.btcb2_server_enabled,
            btcb2_already_claimed: cache.btcb2_already_claimed,
            cube_name: "",
            spark_alias: cache.wallet_names.spark.as_deref(),
            vault_alias: cache.wallet_names.vault.as_deref(),
            lightning_address: None,
            avatar: None,
            theme_mode: cache.theme_mode,
            connect_authenticated: cache.sidebar_connect_signed_in,
            connect_email: cache.connect_email.as_deref(),
            connect_stream_status: &cache.connect_stream_status,
            connect: ConnectChrome::from_cache(cache),
        }
    }

    /// This Cube as a candidate Bitcoin Blake2b claim source.
    pub fn claim_source_cube(&self) -> crate::app::features::ClaimSourceCube {
        crate::app::features::ClaimSourceCube {
            chain: self.network,
            has_vault: self.has_vault,
            server_enabled: self.btcb2_server_enabled,
            already_claimed: self.btcb2_already_claimed,
        }
    }
}

#[cfg(test)]
mod shell_tests {
    use super::*;
    use iced::advanced::{
        layout,
        renderer::Headless,
        widget::{Id, Operation, Tree},
        Layout,
    };

    #[derive(Default)]
    struct Labels(Vec<(String, iced::Rectangle)>);

    impl Operation for Labels {
        fn traverse(&mut self, f: &mut dyn FnMut(&mut dyn Operation)) {
            f(self);
        }

        fn text(&mut self, _: Option<&Id>, bounds: iced::Rectangle, value: &str) {
            self.0.push((value.to_owned(), bounds));
        }
    }

    #[derive(Default)]
    struct ControlBounds(Vec<(Id, iced::Rectangle)>);

    impl Operation for ControlBounds {
        fn traverse(&mut self, f: &mut dyn FnMut(&mut dyn Operation)) {
            f(self);
        }

        fn container(&mut self, id: Option<&Id>, bounds: iced::Rectangle) {
            if let Some(id) = id {
                self.0.push((id.clone(), bounds));
            }
        }
    }

    fn visible_control(
        bounds: &ControlBounds,
        id: &'static str,
        viewport: iced::Size,
    ) -> iced::Rectangle {
        let rect = bounds
            .0
            .iter()
            .find(|(candidate, _)| candidate == &Id::new(id))
            .unwrap_or_else(|| panic!("missing control {}", id))
            .1;
        assert!(
            rect.width > 0.0
                && rect.height > 0.0
                && rect.x >= 0.0
                && rect.y >= 0.0
                && rect.x + rect.width <= viewport.width
                && rect.y + rect.height <= viewport.height,
            "{} outside compact viewport: {:?}",
            id,
            rect
        );
        rect
    }

    fn load_fonts() {
        for bytes in [
            coincube_ui::font::REGULAR_BYTES,
            coincube_ui::font::MEDIUM_BYTES,
            coincube_ui::font::SEMIBOLD_BYTES,
            coincube_ui::font::BOOTSTRAP_ICONS_BYTE,
        ] {
            iced::advanced::graphics::text::font_system()
                .write()
                .unwrap()
                .load_font(std::borrow::Cow::Borrowed(bytes));
        }
    }

    fn redraw(
        element: &mut coincube_ui::widget::Element<'_, Message>,
        tree: &mut Tree,
        node: &layout::Node,
        renderer: &iced::Renderer,
        size: iced::Size,
    ) {
        let mut messages = Vec::new();
        element.as_widget_mut().update(
            tree,
            &iced::Event::Window(iced::window::Event::RedrawRequested(
                std::time::Instant::now(),
            )),
            Layout::new(node),
            iced::mouse::Cursor::Unavailable,
            renderer,
            &mut iced::advanced::clipboard::Null,
            &mut iced::advanced::Shell::new(&mut messages),
            &iced::Rectangle::with_size(size),
        );
        assert!(messages.is_empty());
    }

    #[tokio::test]
    async fn single_sidebar_has_anchored_actions_at_both_minimum_and_full_sizes() {
        load_fonts();
        for (mode, theme) in [
            (
                coincube_ui::theme::palette::ThemeMode::Dark,
                theme::Theme::dark(),
            ),
            (
                coincube_ui::theme::palette::ThemeMode::Light,
                theme::Theme::light(),
            ),
        ] {
            for (width, height, expected_sidebar) in [(1440, 900, 248.0), (1024, 768, 208.0)] {
                let cache = crate::app::cache::Cache {
                    cube_name: "Everyday".into(),
                    has_vault: true,
                    theme_mode: mode,
                    ..Default::default()
                };
                let menu = Menu::Cube(crate::app::menu::CubeSubMenu::Overview);
                let fixture = iced::widget::column![
                    iced::widget::text("Total balance").size(24),
                    iced::widget::text("Synthetic shell preview").size(14),
                ];
                let mut element = crate::app::view::dashboard(&menu, &cache, fixture);
                let mut renderer = <iced::Renderer as Headless>::new(
                    coincube_ui::font::REGULAR,
                    iced::Pixels(14.0),
                    Some("tiny-skia"),
                )
                .await
                .unwrap();
                let mut tree = Tree::new(element.as_widget());
                let size = iced::Size::new(width as f32, height as f32);
                let node = element.as_widget_mut().layout(
                    &mut tree,
                    &renderer,
                    &layout::Limits::new(iced::Size::ZERO, size),
                );
                let body = &node.children()[1];
                let sidebar = &body.children()[0];
                assert!(
                    (sidebar.bounds().width - expected_sidebar).abs() <= 1.0,
                    "sidebar width at {width}: {}",
                    sidebar.bounds().width
                );
                let mut labels = Labels::default();
                element.as_widget_mut().operate(
                    &mut tree,
                    Layout::new(&node),
                    &renderer,
                    &mut labels,
                );
                for required in [
                    "COIN",
                    "Everyday",
                    "Overview",
                    "Activity",
                    "Backup & protection",
                    "Settings",
                ] {
                    assert!(
                        labels.0.iter().any(|(label, _)| label == required),
                        "missing {required} at {width}",
                        required = required,
                        width = width
                    );
                }
                let settings = labels
                    .0
                    .iter()
                    .find(|(label, _)| label == "Settings")
                    .unwrap()
                    .1;
                assert!(
                    settings.y > height as f32 * 0.70,
                    "Settings must stay anchored at the foot"
                );
                let brand = labels
                    .0
                    .iter()
                    .find(|(label, _)| label == "COIN")
                    .unwrap()
                    .1;
                let mut messages = Vec::new();
                let cursor = iced::mouse::Cursor::Available(brand.center());
                for event in [
                    iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left),
                    iced::mouse::Event::ButtonReleased(iced::mouse::Button::Left),
                ] {
                    element.as_widget_mut().update(
                        &mut tree,
                        &iced::Event::Mouse(event),
                        Layout::new(&node),
                        cursor,
                        &renderer,
                        &mut iced::advanced::clipboard::Null,
                        &mut iced::advanced::Shell::new(&mut messages),
                        &iced::Rectangle::with_size(size),
                    );
                }
                assert!(matches!(
                    messages.as_slice(),
                    [Message::FocusAction(id), Message::OpenLauncher]
                        if id == &Id::new(shell::LAUNCHER_RETURN_ID)
                ));
                if let Ok(directory) = std::env::var("SIDEBAR_PREVIEW_DIR") {
                    redraw(&mut element, &mut tree, &node, &renderer, size);
                    std::fs::create_dir_all(&directory).unwrap();
                    element.as_widget().draw(
                        &tree,
                        &mut renderer,
                        &theme,
                        &iced::advanced::renderer::Style {
                            text_color: theme.colors.shell.ink,
                        },
                        Layout::new(&node),
                        iced::mouse::Cursor::Unavailable,
                        &iced::Rectangle::with_size(size),
                    );
                    let pixels = renderer.screenshot(
                        iced::Size::new(width, height),
                        1.0,
                        theme.colors.shell.background,
                    );
                    image::save_buffer(
                        std::path::Path::new(&directory)
                            .join(format!("tenshu-shell-{mode:?}-{width}x{height}.png")),
                        &pixels,
                        width,
                        height,
                        image::ColorType::Rgba8,
                    )
                    .unwrap();
                }
            }
        }
    }

    #[tokio::test]
    async fn long_name_switcher_and_lock_review_fit_compact_shell() {
        load_fonts();
        for (mode, theme) in [
            (
                coincube_ui::theme::palette::ThemeMode::Dark,
                theme::Theme::dark(),
            ),
            (
                coincube_ui::theme::palette::ThemeMode::Light,
                theme::Theme::light(),
            ),
        ] {
            for (confirmation, state_name) in [(false, "switcher"), (true, "lock-review")] {
                let cache = crate::app::cache::Cache {
                    cube_name:
                        "Family household multi-signature treasury for education and retirement"
                            .into(),
                    has_vault: true,
                    cube_switcher_open: true,
                    cube_lock_confirmation: confirmation,
                    theme_mode: mode,
                    ..Default::default()
                };
                let menu = Menu::Cube(crate::app::menu::CubeSubMenu::Overview);
                let mut element = crate::app::view::dashboard(
                    &menu,
                    &cache,
                    iced::widget::text("Synthetic shell preview"),
                );
                let mut renderer = <iced::Renderer as Headless>::new(
                    coincube_ui::font::REGULAR,
                    iced::Pixels(14.0),
                    Some("tiny-skia"),
                )
                .await
                .unwrap();
                let mut tree = Tree::new(element.as_widget());
                let size = iced::Size::new(1024.0, 768.0);
                let node = element.as_widget_mut().layout(
                    &mut tree,
                    &renderer,
                    &layout::Limits::new(iced::Size::ZERO, size),
                );
                assert_eq!(node.children()[1].children()[0].bounds().width, 208.0);
                let mut labels = Labels::default();
                element.as_widget_mut().operate(
                    &mut tree,
                    Layout::new(&node),
                    &renderer,
                    &mut labels,
                );
                for required in ["All Cubes", "Settings", "Cancel"] {
                    if required == "Cancel" && !confirmation {
                        continue;
                    }
                    assert!(labels.0.iter().any(|(label, _)| label == required));
                }
                assert!(labels.0.iter().any(
                    |(label, _)| label.starts_with("Family household") && label.ends_with('…')
                ));
                if confirmation {
                    assert!(labels.0.iter().any(|(label, _)| label == "Lock"));
                } else {
                    assert!(labels.0.iter().any(|(label, _)| label == "Lock Cube"));
                }
                if let Ok(directory) = std::env::var("SIDEBAR_PREVIEW_DIR") {
                    redraw(&mut element, &mut tree, &node, &renderer, size);
                    std::fs::create_dir_all(&directory).unwrap();
                    shell::test_draw(&mut element, &mut tree, &node, &mut renderer, &theme, size);
                    let pixels = renderer.screenshot(
                        iced::Size::new(1024, 768),
                        1.0,
                        theme.colors.shell.background,
                    );
                    image::save_buffer(
                        std::path::Path::new(&directory)
                            .join(format!("tenshu-shell-{mode:?}-long-name-{state_name}.png")),
                        &pixels,
                        1024,
                        768,
                        image::ColorType::Rgba8,
                    )
                    .unwrap();
                }
            }
        }
    }

    #[tokio::test]
    async fn extreme_names_keep_lock_and_settings_controls_inside_compact_viewport() {
        load_fonts();
        let names = [
            ("ascii", "Family treasury ".repeat(40)),
            ("wide", "家族の長期保管庫💠".repeat(48)),
            ("newlines", "Family\n\nTreasury\r\n".repeat(40)),
        ];
        for (mode, theme) in [
            (
                coincube_ui::theme::palette::ThemeMode::Dark,
                theme::Theme::dark(),
            ),
            (
                coincube_ui::theme::palette::ThemeMode::Light,
                theme::Theme::light(),
            ),
        ] {
            for (case, cube_name) in &names {
                let cache = crate::app::cache::Cache {
                    cube_name: cube_name.clone(),
                    has_vault: true,
                    cube_switcher_open: true,
                    cube_lock_confirmation: true,
                    theme_mode: mode,
                    ..Default::default()
                };
                let menu = Menu::Cube(crate::app::menu::CubeSubMenu::Overview);
                let mut element = crate::app::view::dashboard(
                    &menu,
                    &cache,
                    iced::widget::text("Synthetic shell preview"),
                );
                let mut renderer = <iced::Renderer as Headless>::new(
                    coincube_ui::font::REGULAR,
                    iced::Pixels(14.0),
                    Some("tiny-skia"),
                )
                .await
                .unwrap();
                let mut tree = Tree::new(element.as_widget());
                let size = iced::Size::new(1024.0, 768.0);
                let node = element.as_widget_mut().layout(
                    &mut tree,
                    &renderer,
                    &layout::Limits::new(iced::Size::ZERO, size),
                );
                let mut controls = ControlBounds::default();
                element.as_widget_mut().operate(
                    &mut tree,
                    Layout::new(&node),
                    &renderer,
                    &mut controls,
                );
                for (id, expected) in [
                    ("cube-lock-confirm", "lock"),
                    ("cube-lock-cancel", "cancel"),
                    ("cube-sidebar-settings", "settings"),
                ] {
                    let rect = visible_control(&controls, id, size);
                    let cursor = iced::mouse::Cursor::Available(rect.center());
                    let mut messages = Vec::new();
                    for event in [
                        iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left),
                        iced::mouse::Event::ButtonReleased(iced::mouse::Button::Left),
                    ] {
                        messages.extend(shell::test_send(
                            &mut element,
                            &mut tree,
                            &node,
                            &renderer,
                            &iced::Event::Mouse(event),
                            cursor,
                            size,
                        ));
                    }
                    let activated = matches!(
                        (expected, messages.as_slice()),
                        (
                            "lock",
                            [Message::FocusAction(_), Message::LockCubeToLauncher]
                        ) | ("cancel", [Message::FocusAction(_), Message::CancelCubeLock])
                            | (
                                "settings",
                                [
                                    Message::FocusAction(_),
                                    Message::Menu(Menu::Cube(
                                        crate::app::menu::CubeSubMenu::Settings(
                                            crate::app::menu::CubeSettingsOption::General,
                                        )
                                    ))
                                ],
                            )
                    );
                    assert!(
                        activated,
                        "{} did not activate for {}: {:?}",
                        id, case, messages
                    );
                }
                if let Ok(directory) = std::env::var("SIDEBAR_PREVIEW_DIR") {
                    redraw(&mut element, &mut tree, &node, &renderer, size);
                    std::fs::create_dir_all(&directory).unwrap();
                    shell::test_draw(&mut element, &mut tree, &node, &mut renderer, &theme, size);
                    let pixels = renderer.screenshot(
                        iced::Size::new(1024, 768),
                        1.0,
                        theme.colors.shell.background,
                    );
                    image::save_buffer(
                        std::path::Path::new(&directory)
                            .join(format!("tenshu-shell-{mode:?}-extreme-{case}.png")),
                        &pixels,
                        1024,
                        768,
                        image::ColorType::Rgba8,
                    )
                    .unwrap();
                }
            }
        }
    }
}
