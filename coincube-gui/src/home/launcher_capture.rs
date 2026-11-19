//! Offscreen frames of the real Launcher view through the production wgpu
//! renderer, for side-by-side review against the prototype. Opt-in only:
//! `LAUNCHER_CAPTURE_DIR=<dir> cargo test -p coincube-gui --lib
//! launcher_capture -- --ignored`. All Cubes are synthetic fixtures.

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use coincube_core::miniscript::bitcoin::Network;
use coincube_ui::{font, theme, widget::Element};
use iced::{
    advanced::{layout, mouse, renderer::Headless, widget::Tree, Layout, Renderer as _, Shell},
    Rectangle, Size,
};

use super::{
    launcher_catalog::LauncherMode, launcher_presenter, launcher_scene::Motion, CubeCatalogKey,
    Home, State,
};
use crate::{app::settings::CubeSettings, chain::ChainId, dir::CoincubeDirectory};

pub(crate) fn renderer() -> Option<iced::Renderer> {
    for bytes in font::load() {
        iced::advanced::graphics::text::font_system()
            .write()
            .unwrap()
            .load_font(bytes);
    }
    iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
        font::REGULAR,
        iced::Pixels(14.0),
        Some("wgpu"),
    ))
}

pub(crate) fn save<M>(
    renderer: &mut iced::Renderer,
    element: Element<'_, M>,
    size: Size,
    theme: &theme::Theme,
    path: PathBuf,
) {
    save_scrolled(renderer, element, size, theme, path, None);
}

/// As `save`, after snapping the scrollable `scroll` to its end.
fn save_scrolled<M>(
    renderer: &mut iced::Renderer,
    mut element: Element<'_, M>,
    size: Size,
    theme: &theme::Theme,
    path: PathBuf,
    scroll: Option<iced::advanced::widget::Id>,
) {
    let mut tree = Tree::new(element.as_widget());
    let node =
        element
            .as_widget_mut()
            .layout(&mut tree, renderer, &layout::Limits::new(Size::ZERO, size));
    if let Some(id) = scroll {
        let mut snap = iced::advanced::widget::operation::scrollable::snap_to(
            id,
            iced::widget::scrollable::RelativeOffset {
                x: None,
                y: Some(1.0),
            },
        );
        element
            .as_widget_mut()
            .operate(&mut tree, Layout::new(&node), renderer, &mut snap);
    }
    let viewport = Rectangle::with_size(size);
    // Frames of different sizes share the renderer; clip to this one.
    renderer.reset(viewport);
    let mut messages = Vec::new();
    element.as_widget_mut().update(
        &mut tree,
        &iced::Event::Window(iced::window::Event::RedrawRequested(Instant::now())),
        Layout::new(&node),
        mouse::Cursor::Unavailable,
        renderer,
        &mut iced::advanced::clipboard::Null,
        &mut Shell::new(&mut messages),
        &viewport,
    );
    element.as_widget().draw(
        &tree,
        renderer,
        theme,
        &iced::advanced::renderer::Style {
            text_color: theme.colors.shell.ink,
        },
        Layout::new(&node),
        mouse::Cursor::Unavailable,
        &viewport,
    );
    let pixels = renderer.screenshot(
        Size::new(size.width as u32, size.height as u32),
        1.0,
        theme.colors.shell.background,
    );
    renderer.reset(viewport);
    image::save_buffer(
        path,
        &pixels,
        size.width as u32,
        size.height as u32,
        image::ColorType::Rgba8,
    )
    .expect("save capture");
}

fn home(light: bool) -> Home {
    let mut home = Home::new(
        CoincubeDirectory::new(PathBuf::new()),
        Some(Network::Bitcoin),
    )
    .0;
    home.network = ChainId::Bitcoin;
    let names = [
        "Everyday",
        "Archive",
        "Household 2024",
        "Travel fund",
        "Studio",
        "Lab",
    ];
    let mut cubes: Vec<CubeSettings> = names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            CubeSettings::new_with_raw_id(format!("cube-{i}"), (*name).into(), ChainId::Bitcoin)
        })
        .collect();
    cubes[0].set_vault(crate::app::settings::VaultIdentity {
        wallet_id: crate::app::settings::WalletId::new("fixture".into(), Some(1)),
        fingerprint: Some("12345678".into()),
    });
    cubes[2].backed_up = true;
    cubes[2].set_vault(crate::app::settings::VaultIdentity {
        wallet_id: crate::app::settings::WalletId::new("fixture2".into(), Some(1)),
        fingerprint: Some("87654321".into()),
    });
    home.state = State::Cubes {
        cubes,
        create_cube: false,
        source: ChainId::Bitcoin,
    };
    home.advisory_notice = None;
    home.catalog.mode = LauncherMode::Cubes;
    home.theme_mode = if light {
        coincube_ui::theme::palette::ThemeMode::Light
    } else {
        coincube_ui::theme::palette::ThemeMode::Dark
    };
    home
}

fn capture_appearance(light: bool) -> (&'static str, theme::Theme) {
    if light {
        ("light", theme::Theme::light())
    } else {
        ("dark", theme::Theme::dark())
    }
}

fn key(i: usize) -> CubeCatalogKey {
    CubeCatalogKey {
        chain: ChainId::Bitcoin,
        cube_id: format!("cube-{i}"),
    }
}

/// The floor of twelve Cubes at the minimum window, a 1280px laptop window
/// and a 2000px desktop window: four, four and six Cubes a page, each row
/// centered on the page arrows' line.
#[test]
#[ignore = "writes PNG frames; needs a GPU adapter"]
fn launcher_capture_sizes() {
    let Ok(directory) = std::env::var("LAUNCHER_CAPTURE_DIR") else {
        return;
    };
    let directory = PathBuf::from(directory);
    std::fs::create_dir_all(&directory).unwrap();
    let mut renderer = renderer().expect("wgpu headless renderer");
    for light in [false, true] {
        let (appearance, palette) = capture_appearance(light);
        let mut home = home(light);
        if let State::Cubes { cubes, .. } = &mut home.state {
            for i in cubes.len()..12 {
                cubes.push(CubeSettings::new_with_raw_id(
                    format!("cube-{i}"),
                    format!("Cube {}", i + 1),
                    ChainId::Bitcoin,
                ));
            }
            // Every backup badge: "?" (no seed recorded), "!", and none.
            for (i, cube) in cubes.iter_mut().enumerate() {
                if i % 3 != 0 {
                    cube.master_signer_fingerprint = Some([1, 2, 3, i as u8].into());
                    cube.backed_up = i % 3 == 2;
                }
            }
        }
        // The second Cube on the page is at the top of its hover float.
        let floating = super::launcher_catalog::project(&home).locals[1]
            .key
            .clone();
        home.catalog.hover(
            floating,
            true,
            std::time::Instant::now() - super::launcher_scene::BOB_PERIOD / 2,
        );
        for (width, height) in [
            (1024.0, 768.0),
            (1280.0, 800.0),
            (2000.0, 1083.0),
            (2000.0, 1250.0),
        ] {
            save(
                &mut renderer,
                super::launcher_catalog::launcher_view(&home),
                Size::new(width, height),
                &palette,
                directory.join(format!("{appearance}-13-floor-{width}x{height}.png")),
            );
        }
        home.catalog.key.mode = super::launcher_catalog::KeyMode::Pinned;
        save(
            &mut renderer,
            super::launcher_catalog::launcher_view(&home),
            Size::new(1440.0, 900.0),
            &palette,
            directory.join(format!("{appearance}-14-floor-key-1440x900.png")),
        );
    }
}

fn ghost(uuid: &str, name: &str, kit: bool) -> super::RemoteCube {
    super::RemoteCube {
        id: 9,
        uuid: uuid.into(),
        name: name.into(),
        network: crate::app::settings::network_to_api_string(ChainId::Bitcoin),
        has_recovery_kit: kit,
        has_encrypted_seed: kit,
        phone_recoverable: false,
        phone_full_cube: false,
        encryption_pubkey: None,
        has_vault: Some(true),
    }
}

/// Connect-only Cubes as ghosts: after two local Cubes, as a new device's
/// whole row, and each kind's details face.
#[test]
#[ignore = "writes PNG frames; needs a GPU adapter"]
fn launcher_capture_ghosts() {
    let Ok(directory) = std::env::var("LAUNCHER_CAPTURE_DIR") else {
        return;
    };
    let directory = PathBuf::from(directory);
    std::fs::create_dir_all(&directory).unwrap();
    let mut renderer = renderer().expect("wgpu headless renderer");
    let size = Size::new(1440.0, 900.0);
    for light in [false, true] {
        let appearance = if light { "light" } else { "dark" };
        let palette = if light {
            theme::Theme::light()
        } else {
            theme::Theme::dark()
        };
        let mut home = home(light);
        if let State::Cubes { cubes, .. } = &mut home.state {
            cubes.truncate(2);
        }
        home.remote_cubes = vec![
            ghost("ghost-kit", "Passkey", true),
            ghost("ghost-nokit", "Old laptop", false),
        ];
        let mut shot = |home: &Home, name: &str| {
            save(
                &mut renderer,
                super::launcher_catalog::launcher_view(home),
                size,
                &palette,
                directory.join(format!("{appearance}-ghosts-{name}.png")),
            );
        };
        shot(&home, "1-mixed-row");
        home.catalog.key.mode = super::launcher_catalog::KeyMode::Pinned;
        shot(&home, "1-key");
        home.catalog.key.mode = super::launcher_catalog::KeyMode::Closed;
        for uuid in ["ghost-kit", "ghost-nokit"] {
            home.catalog.selected = Some(CubeCatalogKey {
                chain: ChainId::Bitcoin,
                cube_id: uuid.into(),
            });
            shot(&home, &format!("2-face-{uuid}"));
        }
        home.catalog.selected = None;
        home.state = State::NoCube { create_cube: false };
        shot(&home, "3-new-device");
    }
}

#[test]
#[ignore = "writes PNG frames; needs a GPU adapter"]
fn launcher_capture() {
    let Ok(directory) = std::env::var("LAUNCHER_CAPTURE_DIR") else {
        return;
    };
    let directory = PathBuf::from(directory);
    std::fs::create_dir_all(&directory).unwrap();
    let mut renderer = renderer().expect("wgpu headless renderer");
    let size = Size::new(1440.0, 900.0);
    for light in [false, true] {
        let (appearance, palette) = capture_appearance(light);
        let shot = |home: &Home, name: &str, renderer: &mut iced::Renderer| {
            save(
                renderer,
                super::launcher_catalog::launcher_view(home),
                size,
                &palette,
                directory.join(format!("{appearance}-{name}.png")),
            );
        };
        let mut home = home(light);
        // The prototype captures open the second Cube (dark) and the fourth
        // Cube (light); match them so the frames compare one to one.
        let pick = if light { 3 } else { 1 };
        let other = if light { 1 } else { 3 };
        shot(&home, "01-floor", &mut renderer);
        home.catalog.key.mode = super::launcher_catalog::KeyMode::Pinned;
        shot(&home, "01-floor-key", &mut renderer);
        home.catalog.key.mode = super::launcher_catalog::KeyMode::Closed;
        home.catalog.network_menu = true;
        shot(&home, "01-floor-menu", &mut renderer);
        home.catalog.network_menu = false;
        // Every network at once (Developer mode), as on the canvas: All /
        // Live / Test chips, the Network menu with counts, a filtered count
        // with Clear filters, and the empty state when nothing matches.
        {
            use super::launcher_catalog::NetworkFilter;
            let mut all = self::home(light);
            all.developer_mode = true;
            all.displayed_networks = vec![
                ChainId::Bitcoin,
                ChainId::Testnet4,
                ChainId::Signet,
                ChainId::Regtest,
            ];
            let test_cube = |id: &str, name: &str, chain| {
                CubeSettings::new_with_raw_id(id.into(), name.into(), chain)
            };
            all.network_catalog_requested =
                vec![ChainId::Testnet4, ChainId::Signet, ChainId::Regtest];
            all.network_catalog = vec![
                (
                    ChainId::Testnet4,
                    vec![test_cube("t-1", "Lab bench", ChainId::Testnet4)],
                ),
                (
                    ChainId::Signet,
                    vec![test_cube("s-1", "Signet demo", ChainId::Signet)],
                ),
                (
                    ChainId::Regtest,
                    vec![test_cube("r-1", "Regtest", ChainId::Regtest)],
                ),
            ];
            shot(&all, "12-filters-all", &mut renderer);
            all.catalog.network_menu = true;
            shot(&all, "12-filters-menu", &mut renderer);
            all.catalog.network_menu = false;
            all.catalog.filter = NetworkFilter::Live;
            shot(&all, "12-filters-live", &mut renderer);
            all.catalog.filter = NetworkFilter::Test;
            shot(&all, "12-filters-test", &mut renderer);
            all.displayed_networks.push(ChainId::BitcoinBlake2bTestnet4);
            all.catalog.filter = NetworkFilter::Network(ChainId::BitcoinBlake2bTestnet4);
            shot(&all, "12-filters-none", &mut renderer);
        }
        capture_motion_frames(&mut home, pick, other, &mut renderer, &shot);

        // A 30fps sequence for the recording: spin in, hold, hand off, return.
        if std::env::var("LAUNCHER_CAPTURE_SEQUENCE").is_ok() {
            capture_motion_sequence(&mut home, pick, other, &mut renderer, &shot);
        }

        // A short window scrolls the stage above the frame top: the GPU
        // picture must stay under the live details face and labels.
        let scroll = iced::advanced::widget::Id::new("launcher.capture.scroll");
        home.catalog.selected = Some(key(pick));
        home.catalog.motion = None;
        save_scrolled(
            &mut renderer,
            iced::widget::scrollable(launcher_presenter::floor_body(&home))
                .id(scroll.clone())
                .height(iced::Length::Fill)
                .into(),
            Size::new(1440.0, 560.0),
            &palette,
            directory.join(format!("{appearance}-10-details-scrolled.png")),
            Some(scroll),
        );
        home.catalog.selected = None;

        capture_unlock_frames(
            &mut home,
            pick,
            &mut renderer,
            size,
            &palette,
            &directory,
            appearance,
        );
    }
}

fn capture_motion_frames(
    home: &mut Home,
    pick: usize,
    other: usize,
    renderer: &mut iced::Renderer,
    shot: &impl Fn(&Home, &str, &mut iced::Renderer),
) {
    // Browse -> details on the second Cube: every 82ms of the 820ms turn.
    home.catalog.selected = Some(key(pick));
    for step in 0..=10u64 {
        home.catalog.motion = Some(Motion::at(
            None,
            Instant::now() - Duration::from_millis(step * 82),
        ));
        shot(home, &format!("02-spin-{:04}ms", step * 82), renderer);
    }
    home.catalog.motion = None;
    shot(home, "03-details", renderer);
    // Handoff to another Cube: 160ms delay, then its turn.
    home.catalog.selected = Some(key(other));
    for ms in [0u64, 80, 160, 400, 980] {
        home.catalog.motion = Some(Motion::at(
            Some(key(pick)),
            Instant::now() - Duration::from_millis(ms),
        ));
        shot(home, &format!("04-handoff-{ms:04}ms"), renderer);
    }
    home.catalog.motion = None;
    // Back to browse.
    home.catalog.selected = None;
    for ms in [0u64, 250, 820] {
        home.catalog.motion = Some(Motion::returning_at(
            key(other),
            Instant::now() - Duration::from_millis(ms),
        ));
        shot(home, &format!("05-return-{ms:04}ms"), renderer);
    }
    home.catalog.motion = None;
    home.catalog.page = 1;
    shot(home, "06-floor-page2", renderer);
    home.catalog.page = 0;
}

fn capture_motion_sequence(
    home: &mut Home,
    pick: usize,
    other: usize,
    renderer: &mut iced::Renderer,
    shot: &impl Fn(&Home, &str, &mut iced::Renderer),
) {
    let mut frame = 0;
    let mut seq = |home: &Home, renderer: &mut iced::Renderer| {
        shot(home, &format!("seq-{frame:04}"), renderer);
        frame += 1;
    };
    home.catalog.selected = None;
    home.catalog.motion = None;
    for _ in 0..10 {
        seq(home, renderer);
    }
    home.catalog.selected = Some(key(pick));
    for ms in (0..=840u64).step_by(33) {
        home.catalog.motion = Some(Motion::at(None, Instant::now() - Duration::from_millis(ms)));
        seq(home, renderer);
    }
    home.catalog.motion = None;
    for _ in 0..20 {
        seq(home, renderer);
    }
    home.catalog.selected = Some(key(other));
    for ms in (0..=1000u64).step_by(33) {
        home.catalog.motion = Some(Motion::at(
            Some(key(pick)),
            Instant::now() - Duration::from_millis(ms),
        ));
        seq(home, renderer);
    }
    home.catalog.motion = None;
    for _ in 0..20 {
        seq(home, renderer);
    }
    home.catalog.selected = None;
    for ms in (0..=840u64).step_by(33) {
        home.catalog.motion = Some(Motion::returning_at(
            key(other),
            Instant::now() - Duration::from_millis(ms),
        ));
        seq(home, renderer);
    }
    home.catalog.motion = None;
    for _ in 0..10 {
        seq(home, renderer);
    }
}

fn capture_unlock_frames(
    home: &mut Home,
    pick: usize,
    renderer: &mut iced::Renderer,
    size: Size,
    palette: &theme::Theme,
    directory: &Path,
    appearance: &str,
) {
    // Details -> unlock on the same floor, through the real unlock stage.
    home.catalog.selected = Some(key(pick));
    home.catalog.open_from_floor = true;
    let cube = if let State::Cubes { cubes, .. } = &home.state {
        cubes[pick].clone()
    } else {
        unreachable!()
    };
    let context = home.launcher_scene_context(&cube).expect("floor context");
    let mode = home.theme_mode;
    let steps: Vec<f32> = if std::env::var("LAUNCHER_CAPTURE_SEQUENCE").is_ok() {
        (0..=25)
            .map(|i| super::launcher_scene::ease(i as f32 / 25.0))
            .collect()
    } else {
        vec![0.0, 0.25, 0.5, 0.75]
    };
    // The real PIN and passkey screens, with synthetic Cubes only.
    let success = || crate::pin_entry::PinEntrySuccess::LoadApp {
        datadir: crate::dir::CoincubeDirectory::new(Default::default()),
        config: crate::app::Config::new(false),
        network: cube.network.bitcoin_network(),
        internal_bitcoind: None,
        backup: None,
        wallet_settings: None,
        connect_client: None,
    };
    let entry = crate::pin_entry::PinEntry::new(cube.clone(), Default::default(), success(), None);
    for progress in steps {
        let element = launcher_presenter::unlock_turn_with_floor_controls(
            &cube,
            mode,
            &context,
            Some(progress),
            entry.view_with_theme_and_floor(mode, Some(&context)),
        );
        save(
            renderer,
            element,
            size,
            palette,
            directory.join(format!(
                "{appearance}-07-unlock-turn-{:04}.png",
                (progress * 1000.0) as u32
            )),
        );
    }
    save(
        renderer,
        entry.view_with_theme_and_floor(mode, Some(&context)),
        size,
        palette,
        directory.join(format!("{appearance}-08-unlock.png")),
    );
    // The unlock screens without a Launcher page behind them.
    save(
        renderer,
        entry.view_with_theme(mode),
        size,
        palette,
        directory.join(format!("{appearance}-09-unlock-lone.png")),
    );
    let passkey =
        crate::passkey_unlock::PasskeyUnlock::new(cube.clone(), Default::default(), success());
    save(
        renderer,
        passkey.view_with_theme_and_floor(mode, Some(&context)),
        size,
        palette,
        directory.join(format!("{appearance}-11-unlock-passkey.png")),
    );
}

/// O-Launcher's opening: the tumble, the settled loading face as the edge
/// brightens, a stalled stage with Cancel, the Flash finale, and the List
/// view's full-screen frame.
#[test]
#[ignore = "writes PNG frames; needs a GPU adapter"]
fn launcher_capture_opening() {
    use super::launcher_opening::{Opening, Stage};
    let Ok(directory) = std::env::var("LAUNCHER_CAPTURE_DIR") else {
        return;
    };
    let directory = PathBuf::from(directory);
    std::fs::create_dir_all(&directory).unwrap();
    let mut renderer = renderer().expect("wgpu headless renderer");
    let size = Size::new(1440.0, 900.0);
    let quote = coincube_ui::component::quote_display::Quote {
        id: "capture".into(),
        text: "Trusted third parties are security holes.".into(),
        author: "Nick Szabo".into(),
        source: "Trusted Third Parties Are Security Holes, 2001".into(),
        category: String::new(),
        length: String::new(),
        contexts: Vec::new(),
    };
    for light in [false, true] {
        let (appearance, palette) = capture_appearance(light);
        let mode = if light {
            coincube_ui::theme::palette::ThemeMode::Light
        } else {
            coincube_ui::theme::palette::ThemeMode::Dark
        };
        let mut home = home(light);
        home.catalog.selected = Some(key(1));
        let cube = match &home.state {
            State::Cubes { cubes, .. } => cubes[1].clone(),
            _ => unreachable!(),
        };
        let floor = home
            .launcher_scene_context(&cube)
            .expect("the selected Cube's floor");
        let t0 = Instant::now();
        let mut shot = |opening: &Opening, now: Instant, stage: Stage, name: &str| {
            let element = opening_capture_element(&cube, &quote, opening, now, stage, mode);
            save(
                &mut renderer,
                element,
                size,
                &palette,
                directory.join(format!("{appearance}-opening-{name}.png")),
            );
        };
        let ms = Duration::from_millis;
        let mut opening = Opening::new(Some(floor.clone()), false, Stage::Connecting, t0);
        shot(&opening, t0 + ms(300), Stage::Connecting, "1-tumbling");
        shot(&opening, t0 + ms(1300), Stage::Connecting, "2-connecting");
        opening.set_stage(Stage::Syncing, t0 + ms(3000));
        shot(&opening, t0 + ms(4500), Stage::Syncing, "3-syncing");
        shot(
            &opening,
            t0 + ms(3000) + super::launcher_opening::STALL,
            Stage::Syncing,
            "4-stalled",
        );
        opening.set_stage(Stage::Ready, t0 + ms(30_000));
        shot(&opening, t0 + ms(30_300), Stage::Ready, "5-ready");
        shot(&opening, t0 + ms(31_000), Stage::Ready, "6-finale-rush");
        shot(&opening, t0 + ms(31_400), Stage::Ready, "7-finale-wash");
        let list = Opening::new(None, false, Stage::Syncing, t0);
        shot(&list, t0 + ms(2400), Stage::Syncing, "8-list-frame");
        let reduced = Opening::new(None, true, Stage::StartingDaemon, t0);
        shot(
            &reduced,
            t0 + ms(100),
            Stage::StartingDaemon,
            "9-reduced-frame",
        );
    }
}

fn opening_capture_element<'a>(
    cube: &'a CubeSettings,
    quote: &'a coincube_ui::component::quote_display::Quote,
    opening: &'a super::launcher_opening::Opening,
    now: Instant,
    stage: super::launcher_opening::Stage,
    mode: coincube_ui::theme::palette::ThemeMode,
) -> Element<'a, ()> {
    let face = launcher_presenter::OpeningFace {
        cube,
        quote,
        outgoing: None,
        stage,
        stalled: opening.stalled(now),
        cancel: || (),
        without_spark: || (),
    };
    let base: Element<'_, ()> = match opening.floor() {
        Some(floor) => launcher_presenter::opening_on_floor(floor, face, mode, opening, now),
        None => launcher_presenter::opening_full_screen(face, mode, opening, now),
    };
    let wash = opening.wash(now);
    if wash > 0.0 {
        iced::widget::Stack::new()
            .push(base)
            .push(launcher_presenter::opening_wash(cube.network, wash))
            .into()
    } else {
        base
    }
}

/// The dock's plan and Network activity menus, a full plan, and Import or
/// restore's import path and Overview.
#[test]
#[ignore = "writes PNG frames; needs a GPU adapter"]
fn launcher_capture_dock_and_restore() {
    let Ok(directory) = std::env::var("LAUNCHER_CAPTURE_DIR") else {
        return;
    };
    let directory = PathBuf::from(directory);
    std::fs::create_dir_all(&directory).unwrap();
    let mut renderer = renderer().expect("wgpu headless renderer");
    let size = Size::new(1440.0, 900.0);
    for light in [false, true] {
        let appearance = if light { "light" } else { "dark" };
        let palette = if light {
            theme::Theme::light()
        } else {
            theme::Theme::dark()
        };
        let mut full = home(light);
        full.server_cube_limit = Some(6);
        let mut home = home(light);
        home.server_cube_limit = Some(7);
        home.displayed_networks = vec![ChainId::Bitcoin, ChainId::BitcoinBlake2b];
        home.catalog.activity.show_for_test(
            ChainId::Bitcoin,
            Ok(super::launcher_activity::sample_snapshot()),
        );
        save(
            &mut renderer,
            super::launcher_catalog::launcher_view(&home),
            size,
            &palette,
            directory.join(format!("{appearance}-dock.png")),
        );
        home.catalog.plan_menu = true;
        save(
            &mut renderer,
            super::launcher_catalog::launcher_view(&home),
            size,
            &palette,
            directory.join(format!("{appearance}-dock-plan.png")),
        );
        home.catalog.plan_menu = false;
        home.catalog.activity.menu = true;
        save(
            &mut renderer,
            super::launcher_catalog::launcher_view(&home),
            size,
            &palette,
            directory.join(format!("{appearance}-dock-activity.png")),
        );
        home.catalog.activity.menu = false;
        save(
            &mut renderer,
            super::launcher_catalog::launcher_view(&full),
            size,
            &palette,
            directory.join(format!("{appearance}-dock-full.png")),
        );
        let _ = home.update(super::Message::View(super::ViewMessage::ShowImportBitcoin));
        save(
            &mut renderer,
            home.create_journey_view(),
            size,
            &palette,
            directory.join(format!("{appearance}-restore-import.png")),
        );
        let _ = home.update(super::Message::View(super::ViewMessage::ShowRestoreHub));
        save(
            &mut renderer,
            home.create_journey_view(),
            size,
            &palette,
            directory.join(format!("{appearance}-restore-overview.png")),
        );
    }
}

/// Empty Cubes and List preferences through the production GPU renderer.
#[test]
#[ignore = "writes PNG frames; needs a GPU adapter"]
fn launcher_capture_empty_modes() {
    let Ok(directory) = std::env::var("LAUNCHER_CAPTURE_DIR") else {
        return;
    };
    let directory = PathBuf::from(directory);
    std::fs::create_dir_all(&directory).unwrap();
    let mut renderer = renderer().expect("wgpu headless renderer");
    for light in [false, true] {
        let (appearance, palette) = capture_appearance(light);
        let mut home = home(light);
        home.state = State::NoCube { create_cube: false };
        for mode in [LauncherMode::Cubes, LauncherMode::List] {
            home.catalog.mode = mode;
            let name = if mode == LauncherMode::Cubes {
                "cubes"
            } else {
                "list"
            };
            for (width, height) in [(1024, 768), (1440, 900)] {
                save(
                    &mut renderer,
                    home.view(),
                    Size::new(width as f32, height as f32),
                    &palette,
                    directory.join(format!("empty-{name}-{appearance}-{width}x{height}.png")),
                );
            }
        }
        // The placeholder under the pointer, at the top of its float.
        home.catalog.mode = LauncherMode::Cubes;
        let top = std::time::Instant::now() - super::launcher_scene::BOB_PERIOD / 2;
        home.catalog
            .hover(super::launcher_catalog::create_placeholder_key(), true, top);
        save(
            &mut renderer,
            home.view(),
            Size::new(1440.0, 900.0),
            &palette,
            directory.join(format!("empty-cubes-hover-{appearance}-1440x900.png")),
        );
        home.catalog.hover.clear();
    }
}

/// The Create form's PIN step, signed out, at the minimum window and a
/// 1440px window: Enter and Confirm PIN side by side under the name.
#[test]
#[ignore = "writes PNG frames; needs a GPU adapter"]
fn launcher_capture_create_form() {
    let Ok(directory) = std::env::var("LAUNCHER_CAPTURE_DIR") else {
        return;
    };
    let directory = PathBuf::from(directory);
    std::fs::create_dir_all(&directory).unwrap();
    let mut renderer = renderer().expect("wgpu headless renderer");
    for light in [false, true] {
        let appearance = if light { "light" } else { "dark" };
        let palette = if light {
            theme::Theme::light()
        } else {
            theme::Theme::dark()
        };
        let mut home = home(light);
        home.state = State::NoCube { create_cube: true };
        home.passkey_mode = false;
        for (width, height) in [(1024.0, 768.0), (1440.0, 900.0)] {
            save(
                &mut renderer,
                home.create_journey_view(),
                Size::new(width, height),
                &palette,
                directory.join(format!("{appearance}-create-form-{width}.png")),
            );
        }
        // The header's network menu, open over the Cube step.
        home.create_network_menu = true;
        save(
            &mut renderer,
            home.create_journey_view(),
            Size::new(1440.0, 900.0),
            &palette,
            directory.join(format!("{appearance}-create-network-menu.png")),
        );
        home.create_network_menu = false;
        // The 500px Bitcoin-import form, where the two PIN fields stack.
        let _ = home.update(super::Message::View(super::ViewMessage::ShowImportBitcoin));
        home.passkey_mode = false;
        save(
            &mut renderer,
            home.create_journey_view(),
            Size::new(1440.0, 900.0),
            &palette,
            directory.join(format!("{appearance}-create-form-import.png")),
        );
    }
}
