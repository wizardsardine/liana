use super::*;
use crate::app::view::nav::split::ShellSplit;
use iced::advanced::{
    layout,
    renderer::Headless,
    widget::{operation, operation::focusable, operation::Focusable, Id, Operation, Tree},
    Layout,
};
use iced::{keyboard, mouse, Event, Rectangle, Size, Vector};

fn fixture_context() -> NavContext<'static> {
    static STATUS: ConnectionStatus = ConnectionStatus::Inactive;
    NavContext {
        has_vault: true,
        has_imported_bitcoin: false,
        has_p2p: false,
        network: ChainId::Bitcoin,
        p2p_test_coordinator: false,
        marketplace_flags: features::MarketplaceServerFlags::OFF,
        cube_name: "Synthetic QA Cube",
        // Wallet names in the labels; the row IDs stay `…nav.Spark` and
        // `…nav.Vault`.
        spark_alias: Some("Daily spending"),
        vault_alias: Some("Family savings"),
        lightning_address: None,
        avatar: None,
        theme_mode: coincube_ui::theme::palette::ThemeMode::Dark,
        connect_authenticated: false,
        connect_email: None,
        connect_stream_status: &STATUS,
        connect: Default::default(),
        btcb2_server_enabled: false,
        btcb2_already_claimed: false,
    }
}

/// The wallet rows read `Spark · ‹nickname›` and `Vault · ‹alias›`, fall back
/// to the plain wallet name when the name is unset or blank, and keep one
/// focus ID whatever the label. The Cube's own name differs from both
/// wallet names, so a swapped or misrouted label can't pass.
#[test]
fn wallet_rows_name_each_wallet_and_keep_their_ids() {
    use crate::app::view::shared::landing::capture;

    // The texts drawn inside the focus target `id`.
    fn row_text(inspection: &capture::Inspection, id: &'static str) -> Vec<String> {
        let (_, row) = inspection
            .focus
            .iter()
            .find(|(focus, _)| *focus == Id::new(id))
            .unwrap_or_else(|| panic!("no {} row", id));
        inspection
            .text
            .iter()
            .filter(|(_, bounds)| row.contains(bounds.center()))
            .map(|(value, _)| value.clone())
            .collect()
    }

    for (spark, vault, spark_label, vault_label) in [
        (
            Some("Daily spending"),
            Some("Family savings"),
            "Spark · Daily spending",
            "Vault · Family savings",
        ),
        (None, None, "Spark", "Vault"),
        (Some("   "), Some(""), "Spark", "Vault"),
        (
            Some(" Daily\nspending "),
            None,
            "Spark · Daily spending",
            "Vault",
        ),
        (
            None,
            Some("A very long Vault alias indeed"),
            "Spark",
            "Vault · A very long Vaul…",
        ),
    ] {
        let mut ctx = fixture_context();
        ctx.spark_alias = spark;
        ctx.vault_alias = vault;
        for menu in [
            Menu::Cube(CubeSubMenu::Overview),
            Menu::Spark(SparkSubMenu::Overview),
            Menu::Vault(VaultSubMenu::Overview),
        ] {
            let inspection = capture::inspect_wide_content(
                "sidebar-wallet-names",
                sidebar(&menu, &ctx, ShellState::default()),
                (1440, 900),
                "dark",
                coincube_ui::theme::Theme::dark(),
                FULL_WIDTH as u32,
            );
            assert_eq!(
                row_text(&inspection, "tenshu.shell.nav.Spark"),
                [spark_label],
                "{spark:?} on {menu:?}"
            );
            assert_eq!(
                row_text(&inspection, "tenshu.shell.nav.Vault"),
                [vault_label],
                "{vault:?} on {menu:?}"
            );
            assert!(!inspection.contains(&format!("· {}", ctx.cube_name)));
        }
    }
}

#[test]
fn activity_is_cube_scoped_even_without_a_wallet_backend() {
    let mut ctx = fixture_context();
    ctx.has_vault = false;
    ctx.network = ChainId::Testnet;
    assert_eq!(
        activity_route(&ctx),
        Some(Menu::Cube(CubeSubMenu::Activity))
    );

    ctx.network = ChainId::Regtest;
    assert_eq!(
        activity_route(&ctx),
        Some(Menu::Cube(CubeSubMenu::Activity))
    );

    ctx.network = ChainId::Bitcoin;
    assert_eq!(
        activity_route(&ctx),
        Some(Menu::Cube(CubeSubMenu::Activity))
    );
    ctx.has_imported_bitcoin = true;
    assert_eq!(
        activity_route(&ctx),
        Some(Menu::Cube(CubeSubMenu::Activity))
    );

    ctx.has_imported_bitcoin = false;
    ctx.network = ChainId::BitcoinBlake2b;
    assert_eq!(
        activity_route(&ctx),
        Some(Menu::Cube(CubeSubMenu::Activity))
    );
}

fn shell_tree<'a>(ctx: &NavContext<'a>, state: ShellState) -> Element<'a, Message> {
    let menu = Menu::Cube(CubeSubMenu::Overview);
    iced::widget::column![
        top_bar(ctx),
        ShellSplit::new(
            sidebar(&menu, ctx, state),
            container(text("Synthetic content")).into()
        ),
    ]
    .into()
}

fn shell_with_input<'a>(
    ctx: &NavContext<'a>,
    menu: &Menu,
    state: ShellState,
) -> Element<'a, Message> {
    iced::widget::column![
        top_bar(ctx),
        ShellSplit::new(
            sidebar(menu, ctx, state),
            container(
                iced::widget::text_input("Synthetic content input", "")
                    .id("content-input")
                    .on_input(Message::Clipboard)
                    .on_submit(Message::Reload),
            )
            .padding(24)
            .into(),
        ),
    ]
    .into()
}

#[derive(Default)]
struct FocusSnapshot {
    bounds: Vec<(Id, Rectangle)>,
    focused: Vec<Id>,
}

impl Operation for FocusSnapshot {
    fn traverse(&mut self, f: &mut dyn FnMut(&mut dyn Operation)) {
        f(self);
    }

    fn focusable(&mut self, id: Option<&Id>, bounds: Rectangle, state: &mut dyn Focusable) {
        if let Some(id) = id {
            self.bounds.push((id.clone(), bounds));
            if state.is_focused() {
                self.focused.push(id.clone());
            }
        }
    }
}

fn focus_snapshot(
    element: &mut Element<'_, Message>,
    tree: &mut Tree,
    node: &layout::Node,
    renderer: &iced::Renderer,
) -> FocusSnapshot {
    let mut snapshot = FocusSnapshot::default();
    element
        .as_widget_mut()
        .operate(tree, Layout::new(node), renderer, &mut snapshot);
    snapshot
}

/// The shared bar's right group: the eye, the account and connection chips,
/// then App preferences, in both visual and Tab order.
#[test]
fn top_bar_right_group_is_eye_then_chips_then_app_preferences() {
    load_fonts();
    let renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
        coincube_ui::font::REGULAR,
        14.into(),
        Some("tiny-skia"),
    ))
    .unwrap();
    let ctx = fixture_context();
    let chrome = BalancePrivacyChrome {
        privacy: BalancePrivacy::Shown,
        busy: false,
        interactive: true,
    };
    let mut element = top_bar_with_privacy(&ctx, Some(chrome));
    let mut tree = Tree::new(element.as_widget());
    let node = element.as_widget_mut().layout(
        &mut tree,
        &renderer,
        &layout::Limits::new(Size::ZERO, Size::new(1024.0, TOP_BAR_HEIGHT)),
    );
    let order = [BALANCE_PRIVACY_ID, CONNECTION_ID, APP_PREFERENCES_ID].map(Id::new);
    let bounds = focus_snapshot(&mut element, &mut tree, &node, &renderer).bounds;
    let at = |target: &Id| bounds.iter().find(|(id, _)| id == target).unwrap().1;
    let x = |target: &Id| at(target).x;
    // The eye and App preferences are the shared 32px icon controls.
    for icon in [&order[0], &order[2]] {
        assert_eq!((at(icon).width, at(icon).height), (32.0, 32.0));
    }
    // The Connect control is the design's 32px `.chip`: 30px inside its
    // 1px border.
    assert_eq!(at(&order[1]).height, 30.0);
    assert!(
        order.windows(2).all(|pair| x(&pair[0]) < x(&pair[1])),
        "right group order: {:?}",
        bounds
    );

    operate(
        &mut element,
        &mut tree,
        &node,
        &renderer,
        focusable::focus::<()>(order[0].clone()),
    );
    for next in &order[1..] {
        operate(
            &mut element,
            &mut tree,
            &node,
            &renderer,
            focusable::focus_next::<()>(),
        );
        assert_eq!(
            focus_snapshot(&mut element, &mut tree, &node, &renderer).focused,
            vec![next.clone()]
        );
    }
}

#[derive(Default)]
struct MiddleEdgeSnapshot {
    viewport: Option<Rectangle>,
    content: Option<Rectangle>,
    first: Option<Rectangle>,
    last: Option<Rectangle>,
}

impl Operation for MiddleEdgeSnapshot {
    fn traverse(&mut self, f: &mut dyn FnMut(&mut dyn Operation)) {
        f(self);
    }

    fn scrollable(
        &mut self,
        id: Option<&Id>,
        bounds: Rectangle,
        content_bounds: Rectangle,
        _: Vector,
        _: &mut dyn operation::Scrollable,
    ) {
        if id == Some(&Id::new(MIDDLE_SCROLL_ID)) {
            self.viewport = Some(bounds);
            self.content = Some(content_bounds);
        }
    }

    fn focusable(&mut self, id: Option<&Id>, bounds: Rectangle, _: &mut dyn Focusable) {
        match id {
            Some(id) if id == &Id::new("tenshu.shell.nav.Spark") => self.first = Some(bounds),
            // The fixture's last middle row: Vault (no Liquid or Marketplace).
            Some(id) if id == &Id::new("tenshu.shell.nav.Vault") => self.last = Some(bounds),
            _ => {}
        }
    }
}

fn load_fonts() {
    for bytes in coincube_ui::font::load() {
        iced::advanced::graphics::text::font_system()
            .write()
            .unwrap()
            .load_font(bytes);
    }
}

fn operate<T: 'static>(
    element: &mut Element<'_, Message>,
    tree: &mut Tree,
    node: &layout::Node,
    renderer: &iced::Renderer,
    operation: impl Operation<T> + 'static,
) -> Option<T> {
    let mut operation: Box<dyn Operation<T>> = Box::new(operation);
    loop {
        element.as_widget_mut().operate(
            tree,
            Layout::new(node),
            renderer,
            &mut iced::advanced::widget::operation::black_box(operation.as_mut()),
        );
        match operation.finish() {
            iced::advanced::widget::operation::Outcome::None => return None,
            iced::advanced::widget::operation::Outcome::Some(result) => return Some(result),
            iced::advanced::widget::operation::Outcome::Chain(next) => operation = next,
        }
    }
}

fn send(
    element: &mut Element<'_, Message>,
    tree: &mut Tree,
    node: &layout::Node,
    renderer: &iced::Renderer,
    event: Event,
    cursor: mouse::Cursor,
    size: Size,
) -> Vec<Message> {
    test_send(element, tree, node, renderer, &event, cursor, size)
}

fn key(named: keyboard::key::Named, repeat: bool, pressed: bool) -> Event {
    let key = keyboard::Key::Named(named);
    let physical_key =
        keyboard::key::Physical::Unidentified(keyboard::key::NativeCode::Unidentified);
    let location = keyboard::Location::Standard;
    let modifiers = keyboard::Modifiers::empty();
    if pressed {
        Event::Keyboard(keyboard::Event::KeyPressed {
            key: key.clone(),
            modified_key: key,
            physical_key,
            location,
            modifiers,
            text: None,
            repeat,
        })
    } else {
        Event::Keyboard(keyboard::Event::KeyReleased {
            key: key.clone(),
            modified_key: key,
            physical_key,
            location,
            modifiers,
        })
    }
}

#[test]
fn shell_actions_follow_visual_order_and_dispatch_keyboard_once() {
    load_fonts();
    let renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
        coincube_ui::font::REGULAR,
        14.into(),
        Some("tiny-skia"),
    ))
    .unwrap();
    let ctx = fixture_context();
    let mut element = shell_tree(&ctx, ShellState::default());
    let mut tree = Tree::new(element.as_widget());
    let size = Size::new(1024.0, 768.0);
    let node = element.as_widget_mut().layout(
        &mut tree,
        &renderer,
        &layout::Limits::new(Size::ZERO, size),
    );
    for expected in [
        LAUNCHER_RETURN_ID,
        CONNECTION_ID,
        APP_PREFERENCES_ID,
        CUBE_SWITCHER_ID,
    ] {
        operate(
            &mut element,
            &mut tree,
            &node,
            &renderer,
            focusable::focus_next::<()>(),
        );
        assert_eq!(
            operate(
                &mut element,
                &mut tree,
                &node,
                &renderer,
                focusable::find_focused(),
            ),
            Some(Id::new(expected))
        );
    }
    assert!(matches!(
        send(
            &mut element,
            &mut tree,
            &node,
            &renderer,
            key(keyboard::key::Named::Enter, false, true),
            mouse::Cursor::Unavailable,
            size
        )
        .as_slice(),
        [Message::ToggleCubeSwitcher]
    ));
    assert!(send(
        &mut element,
        &mut tree,
        &node,
        &renderer,
        key(keyboard::key::Named::Enter, true, true),
        mouse::Cursor::Unavailable,
        size
    )
    .is_empty());
    send(
        &mut element,
        &mut tree,
        &node,
        &renderer,
        key(keyboard::key::Named::Enter, false, false),
        mouse::Cursor::Unavailable,
        size,
    );
    operate(
        &mut element,
        &mut tree,
        &node,
        &renderer,
        focusable::focus_previous::<()>(),
    );
    assert_eq!(
        operate(
            &mut element,
            &mut tree,
            &node,
            &renderer,
            focusable::find_focused()
        ),
        Some(Id::new(APP_PREFERENCES_ID))
    );
    for expected in [CONNECTION_ID] {
        operate(
            &mut element,
            &mut tree,
            &node,
            &renderer,
            focusable::focus_previous::<()>(),
        );
        assert_eq!(
            operate(
                &mut element,
                &mut tree,
                &node,
                &renderer,
                focusable::find_focused()
            ),
            Some(Id::new(expected))
        );
    }
    assert!(matches!(
        send(
            &mut element,
            &mut tree,
            &node,
            &renderer,
            key(keyboard::key::Named::Space, false, true),
            mouse::Cursor::Unavailable,
            size
        )
        .as_slice(),
        [Message::OpenConnectSignIn]
    ));
}

#[test]
fn signed_in_connect_control_opens_its_panel() {
    load_fonts();
    let renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
        coincube_ui::font::REGULAR,
        14.into(),
        Some("tiny-skia"),
    ))
    .unwrap();
    let mut ctx = fixture_context();
    ctx.connect_authenticated = true;
    ctx.connect_email = Some("shell@example.invalid");
    let mut element = shell_tree(&ctx, ShellState::default());
    let mut tree = Tree::new(element.as_widget());
    let size = Size::new(1024.0, 768.0);
    let node = element.as_widget_mut().layout(
        &mut tree,
        &renderer,
        &layout::Limits::new(Size::ZERO, size),
    );
    for _ in 0..2 {
        operate(
            &mut element,
            &mut tree,
            &node,
            &renderer,
            focusable::focus_next::<()>(),
        );
    }
    assert_eq!(
        operate(
            &mut element,
            &mut tree,
            &node,
            &renderer,
            focusable::find_focused(),
        ),
        Some(Id::new(CONNECTION_ID))
    );
    assert!(matches!(
        send(
            &mut element,
            &mut tree,
            &node,
            &renderer,
            key(keyboard::key::Named::Enter, false, true),
            mouse::Cursor::Unavailable,
            size,
        )
        .as_slice(),
        [Message::ToggleConnectPanel]
    ));
}

/// The Connect control is the second control (after the brand) in every
/// state, under one id, and opens its panel while signed in.
#[test]
fn connection_chip_routes_to_cube_connections_in_every_state() {
    load_fonts();
    let renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
        coincube_ui::font::REGULAR,
        14.into(),
        Some("tiny-skia"),
    ))
    .unwrap();
    static FAILED: ConnectionStatus = ConnectionStatus::RenewFailed;
    static CONNECTING: ConnectionStatus = ConnectionStatus::Connecting;
    static CONNECTED: ConnectionStatus = ConnectionStatus::Connected;
    static OFFLINE: ConnectionStatus = ConnectionStatus::Offline;
    static INACTIVE: ConnectionStatus = ConnectionStatus::Inactive;
    let size = Size::new(1024.0, 768.0);
    // Focus after the brand: the Connect control is the second shell control.
    let second_focus = |status: &'static ConnectionStatus, has_vault, has_imported_bitcoin| {
        let mut ctx = fixture_context();
        ctx.connect_authenticated = true;
        ctx.connect_email = Some("shell@example.invalid");
        ctx.connect_stream_status = status;
        ctx.has_vault = has_vault;
        ctx.has_imported_bitcoin = has_imported_bitcoin;
        let mut element = shell_tree(&ctx, ShellState::default());
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(Size::ZERO, size),
        );
        for _ in 0..2 {
            operate(
                &mut element,
                &mut tree,
                &node,
                &renderer,
                focusable::focus_next::<()>(),
            );
        }
        let focused = operate(
            &mut element,
            &mut tree,
            &node,
            &renderer,
            focusable::find_focused(),
        );
        let sent = send(
            &mut element,
            &mut tree,
            &node,
            &renderer,
            key(keyboard::key::Named::Enter, false, true),
            mouse::Cursor::Unavailable,
            size,
        );
        (focused, sent)
    };

    for status in [&FAILED, &CONNECTING, &CONNECTED, &OFFLINE, &INACTIVE] {
        for (has_vault, has_imported_bitcoin) in [(true, false), (false, true), (false, false)] {
            let (focused, sent) = second_focus(status, has_vault, has_imported_bitcoin);
            assert_eq!(focused, Some(Id::new(CONNECTION_ID)));
            assert!(
                matches!(sent.as_slice(), [Message::ToggleConnectPanel]),
                "{:?} vault={} imported={}: {:?}",
                status,
                has_vault,
                has_imported_bitcoin,
                sent
            );
        }
    }
}

/// Longer Connect labels fit beside the account and preferences controls at
/// the minimum desktop width, in both appearances.
#[test]
fn connection_chip_fits_the_shell_in_every_state() {
    use crate::app::view::shared::landing::capture;
    static STATES: [ConnectionStatus; 7] = [
        ConnectionStatus::Connected,
        ConnectionStatus::Connecting,
        ConnectionStatus::Reconnecting,
        ConnectionStatus::Renewing,
        ConnectionStatus::Offline,
        ConnectionStatus::RenewFailed,
        ConnectionStatus::Inactive,
    ];
    for (appearance, palette) in [
        ("dark", theme::Theme::dark()),
        ("light", theme::Theme::light()),
    ] {
        for (index, status) in STATES.iter().enumerate() {
            let mut ctx = fixture_context();
            ctx.connect_authenticated = true;
            ctx.connect_stream_status = status;
            for width in [1024, 1440] {
                let inspection = capture::inspect_full_window(
                    &format!("connect-badge-{index}"),
                    top_bar_with_privacy(
                        &ctx,
                        Some(BalancePrivacyChrome {
                            privacy: BalancePrivacy::Shown,
                            busy: false,
                            interactive: true,
                        }),
                    ),
                    (width, 768),
                    appearance,
                    palette,
                );
                assert!(inspection.contains("Connect"));
                let bounds = |id: &'static str| {
                    inspection
                        .focus
                        .iter()
                        .find(|(actual, _)| *actual == Id::new(id))
                        .unwrap()
                        .1
                };
                let connection = bounds(CONNECTION_ID);
                let preferences = bounds(APP_PREFERENCES_ID);
                assert!(connection.x + connection.width <= preferences.x);
                assert!(connection.height <= TOP_BAR_HEIGHT);
            }
        }
    }
}

/// The chip keeps one id, so a status change while it has focus keeps focus
/// on it.
#[test]
fn connection_chip_keeps_focus_across_a_status_change() {
    load_fonts();
    let renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
        coincube_ui::font::REGULAR,
        14.into(),
        Some("tiny-skia"),
    ))
    .unwrap();
    static CONNECTING: ConnectionStatus = ConnectionStatus::Connecting;
    static CONNECTED: ConnectionStatus = ConnectionStatus::Connected;
    let size = Size::new(1024.0, 768.0);
    let mut ctx = fixture_context();
    ctx.connect_stream_status = &CONNECTING;
    let mut element = shell_tree(&ctx, ShellState::default());
    let mut tree = Tree::new(element.as_widget());
    let node = element.as_widget_mut().layout(
        &mut tree,
        &renderer,
        &layout::Limits::new(Size::ZERO, size),
    );
    operate(
        &mut element,
        &mut tree,
        &node,
        &renderer,
        focusable::focus::<()>(Id::new(CONNECTION_ID)),
    );

    ctx.connect_stream_status = &CONNECTED;
    let mut element = shell_tree(&ctx, ShellState::default());
    tree.diff(element.as_widget());
    let node = element.as_widget_mut().layout(
        &mut tree,
        &renderer,
        &layout::Limits::new(Size::ZERO, size),
    );
    assert_eq!(
        focus_snapshot(&mut element, &mut tree, &node, &renderer).focused,
        vec![Id::new(CONNECTION_ID)]
    );
}

#[test]
fn shell_escape_prefers_review_then_switcher() {
    let all = ShellState {
        switcher_open: true,
        connect_open: false,
        lock_confirmation: true,
        backup_attention: false,
    };
    assert!(matches!(
        escape_intent(ShellState {
            connect_open: true,
            ..all
        }),
        Some(Message::CloseConnectPanel)
    ));
    assert!(matches!(escape_intent(all), Some(Message::CancelCubeLock)));
    assert!(matches!(
        escape_intent(ShellState {
            lock_confirmation: false,
            ..all
        }),
        Some(Message::ToggleCubeSwitcher)
    ));
    assert!(escape_intent(ShellState::default()).is_none());
}

#[test]
fn pointer_focus_transfers_across_production_shell_split_and_retained_route() {
    load_fonts();
    let renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
        coincube_ui::font::REGULAR,
        14.into(),
        Some("tiny-skia"),
    ))
    .unwrap();
    let ctx = fixture_context();
    let mut menu = Menu::Cube(CubeSubMenu::Overview);
    let mut element = shell_with_input(&ctx, &menu, ShellState::default());
    let mut tree = Tree::new(element.as_widget());
    let size = Size::new(1024.0, 768.0);
    let mut node = element.as_widget_mut().layout(
        &mut tree,
        &renderer,
        &layout::Limits::new(Size::ZERO, size),
    );
    operate(
        &mut element,
        &mut tree,
        &node,
        &renderer,
        focusable::focus::<()>(Id::new("content-input")),
    );
    assert_eq!(
        focus_snapshot(&mut element, &mut tree, &node, &renderer).focused,
        vec![Id::new("content-input")]
    );

    // Backup routes to a different panel while the fixture retains its content
    // input. It has no unrelated disclosure focus task to mask the pointer bug.
    let target = Id::new("tenshu.shell.nav.Backup & protection");
    let bounds = focus_snapshot(&mut element, &mut tree, &node, &renderer)
        .bounds
        .into_iter()
        .find(|(id, _)| id == &target)
        .unwrap()
        .1;
    let cursor = mouse::Cursor::Available(bounds.center());
    let focus_intents = send(
        &mut element,
        &mut tree,
        &node,
        &renderer,
        Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
        cursor,
        size,
    );
    assert!(matches!(focus_intents.as_slice(), [Message::FocusAction(id)] if id == &target));

    // The production App arm returns this exact whole-tree focus operation.
    // Apply it after rebuilding, as the runtime does for a Task.
    element = shell_with_input(&ctx, &menu, ShellState::default());
    tree.diff(element.as_widget());
    node = element.as_widget_mut().layout(
        &mut tree,
        &renderer,
        &layout::Limits::new(Size::ZERO, size),
    );
    operate(
        &mut element,
        &mut tree,
        &node,
        &renderer,
        focusable::focus::<()>(target.clone()),
    );
    assert_eq!(
        focus_snapshot(&mut element, &mut tree, &node, &renderer).focused,
        vec![target.clone()]
    );

    let route = send(
        &mut element,
        &mut tree,
        &node,
        &renderer,
        Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
        cursor,
        size,
    );
    menu = Menu::Cube(CubeSubMenu::Settings(CubeSettingsOption::Recovery));
    assert!(matches!(route.as_slice(), [Message::Menu(to)] if to == &menu));
    element = shell_with_input(&ctx, &menu, ShellState::default());
    tree.diff(element.as_widget());
    node = element.as_widget_mut().layout(
        &mut tree,
        &renderer,
        &layout::Limits::new(Size::ZERO, size),
    );
    assert_eq!(
        focus_snapshot(&mut element, &mut tree, &node, &renderer).focused,
        vec![target.clone()]
    );

    operate(
        &mut element,
        &mut tree,
        &node,
        &renderer,
        focusable::focus_next::<()>(),
    );
    assert_eq!(
        focus_snapshot(&mut element, &mut tree, &node, &renderer).focused,
        vec![Id::new("tenshu.shell.nav.Spark")]
    );
    operate(
        &mut element,
        &mut tree,
        &node,
        &renderer,
        focusable::focus_previous::<()>(),
    );
    assert_eq!(
        focus_snapshot(&mut element, &mut tree, &node, &renderer).focused,
        vec![target]
    );

    // Clicking the retained content input transfers focus in the reverse
    // direction; neither Enter nor later Tab can activate the old shell row.
    let input_bounds = focus_snapshot(&mut element, &mut tree, &node, &renderer)
        .bounds
        .into_iter()
        .find(|(id, _)| id == &Id::new("content-input"))
        .unwrap()
        .1;
    let input_cursor = mouse::Cursor::Available(input_bounds.center());
    send(
        &mut element,
        &mut tree,
        &node,
        &renderer,
        Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
        input_cursor,
        size,
    );
    send(
        &mut element,
        &mut tree,
        &node,
        &renderer,
        Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
        input_cursor,
        size,
    );
    assert_eq!(
        focus_snapshot(&mut element, &mut tree, &node, &renderer).focused,
        vec![Id::new("content-input")]
    );
    assert!(matches!(
        send(
            &mut element,
            &mut tree,
            &node,
            &renderer,
            key(keyboard::key::Named::Enter, false, true),
            mouse::Cursor::Unavailable,
            size
        )
        .as_slice(),
        [Message::Reload]
    ));
    operate(
        &mut element,
        &mut tree,
        &node,
        &renderer,
        focusable::focus_previous::<()>(),
    );
    assert_eq!(
        focus_snapshot(&mut element, &mut tree, &node, &renderer)
            .focused
            .len(),
        1
    );
}

#[test]
fn production_lock_review_rebuild_focuses_cancel_then_restores_lock_trigger() {
    load_fonts();
    let renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
        coincube_ui::font::REGULAR,
        14.into(),
        Some("tiny-skia"),
    ))
    .unwrap();
    let ctx = fixture_context();
    let menu = Menu::Cube(CubeSubMenu::Overview);
    let mut state = ShellState {
        switcher_open: true,
        ..Default::default()
    };
    let mut element = shell_with_input(&ctx, &menu, state);
    let mut tree = Tree::new(element.as_widget());
    let size = Size::new(1024.0, 768.0);
    let mut node = element.as_widget_mut().layout(
        &mut tree,
        &renderer,
        &layout::Limits::new(Size::ZERO, size),
    );
    operate(
        &mut element,
        &mut tree,
        &node,
        &renderer,
        focusable::focus::<()>(Id::new(CUBE_LOCK_ID)),
    );
    assert!(matches!(
        send(
            &mut element,
            &mut tree,
            &node,
            &renderer,
            key(keyboard::key::Named::Enter, false, true),
            mouse::Cursor::Unavailable,
            size
        )
        .as_slice(),
        [Message::AskCubeLock]
    ));

    state.lock_confirmation = true;
    element = shell_with_input(&ctx, &menu, state);
    tree.diff(element.as_widget());
    node = element.as_widget_mut().layout(
        &mut tree,
        &renderer,
        &layout::Limits::new(Size::ZERO, size),
    );
    operate(
        &mut element,
        &mut tree,
        &node,
        &renderer,
        focusable::focus::<()>(Id::new(CUBE_LOCK_CANCEL_ID)),
    );
    assert_eq!(
        focus_snapshot(&mut element, &mut tree, &node, &renderer).focused,
        vec![Id::new(CUBE_LOCK_CANCEL_ID)]
    );
    assert!(matches!(
        escape_intent(state),
        Some(Message::CancelCubeLock)
    ));

    state.lock_confirmation = false;
    element = shell_with_input(&ctx, &menu, state);
    tree.diff(element.as_widget());
    node = element.as_widget_mut().layout(
        &mut tree,
        &renderer,
        &layout::Limits::new(Size::ZERO, size),
    );
    operate(
        &mut element,
        &mut tree,
        &node,
        &renderer,
        focusable::focus::<()>(Id::new(CUBE_LOCK_ID)),
    );
    assert_eq!(
        focus_snapshot(&mut element, &mut tree, &node, &renderer).focused,
        vec![Id::new(CUBE_LOCK_ID)]
    );
    assert!(matches!(
        escape_intent(state),
        Some(Message::ToggleCubeSwitcher)
    ));
    state.switcher_open = false;
    element = shell_with_input(&ctx, &menu, state);
    tree.diff(element.as_widget());
    node = element.as_widget_mut().layout(
        &mut tree,
        &renderer,
        &layout::Limits::new(Size::ZERO, size),
    );
    operate(
        &mut element,
        &mut tree,
        &node,
        &renderer,
        focusable::focus::<()>(Id::new(CUBE_SWITCHER_ID)),
    );
    assert_eq!(
        focus_snapshot(&mut element, &mut tree, &node, &renderer).focused,
        vec![Id::new(CUBE_SWITCHER_ID)]
    );
    assert!(escape_intent(state).is_none());
}

#[test]
fn middle_scroll_reserves_full_outer_ring_at_both_content_edges() {
    load_fonts();
    let renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
        coincube_ui::font::REGULAR,
        14.into(),
        Some("tiny-skia"),
    ))
    .unwrap();
    let ctx = fixture_context();
    for (width, height) in [(1024.0, 768.0), (1440.0, 900.0)] {
        let mut element = shell_tree(&ctx, ShellState::default());
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(Size::ZERO, Size::new(width, height)),
        );
        let mut edges = MiddleEdgeSnapshot::default();
        element
            .as_widget_mut()
            .operate(&mut tree, Layout::new(&node), &renderer, &mut edges);
        let viewport = edges.viewport.expect("production middle scroll viewport");
        let content = edges.content.expect("production middle scroll content");
        let first = edges.first.expect("first enabled middle action");
        let last = edges.last.expect("last enabled middle action");
        assert!(
            first.y - viewport.y >= 4.0,
            "first ring clips at {width}x{height}",
            width = width,
            height = height
        );
        assert!(
            content.y + content.height - last.y - last.height >= 4.0,
            "last ring clips at maximum scroll for {width}x{height}",
            width = width,
            height = height
        );
        for action in [first, last] {
            assert!(
                action.x - viewport.x >= 4.0
                    && viewport.x + viewport.width - action.x - action.width >= 4.0,
                "middle action's horizontal ring clips at {width}x{height}",
                width = width,
                height = height
            );
        }
    }
}

#[tokio::test]
async fn focused_shell_capture_uses_production_compact_and_full_split() {
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
        for (width, height) in [(1024, 768), (1440, 900)] {
            let mut ctx = fixture_context();
            ctx.theme_mode = mode;
            let review = ShellState {
                switcher_open: true,
                lock_confirmation: true,
                ..ShellState::default()
            };
            for (case, state, target) in [
                ("launcher-return", ShellState::default(), LAUNCHER_RETURN_ID),
                ("switcher", ShellState::default(), CUBE_SWITCHER_ID),
                ("settings", ShellState::default(), SETTINGS_ID),
                ("lock-cancel", review, CUBE_LOCK_CANCEL_ID),
                ("lock-confirm", review, "tenshu.cube.lock.confirm"),
            ] {
                let mut element = shell_tree(&ctx, state);
                let mut renderer = <iced::Renderer as Headless>::new(
                    coincube_ui::font::REGULAR,
                    iced::Pixels(14.0),
                    Some("tiny-skia"),
                )
                .await
                .unwrap();
                let mut tree = Tree::new(element.as_widget());
                let size = Size::new(width as f32, height as f32);
                let node = element.as_widget_mut().layout(
                    &mut tree,
                    &renderer,
                    &layout::Limits::new(Size::ZERO, size),
                );
                // Arrive as a keyboard user does: only keyboard navigation
                // paints the ring (`coincube_ui::widget::action`).
                send(
                    &mut element,
                    &mut tree,
                    &node,
                    &renderer,
                    key(keyboard::key::Named::Tab, false, true),
                    mouse::Cursor::Unavailable,
                    size,
                );
                operate(
                    &mut element,
                    &mut tree,
                    &node,
                    &renderer,
                    focusable::focus::<()>(Id::new(target)),
                );
                let focused = focus_snapshot(&mut element, &mut tree, &node, &renderer);
                assert_eq!(focused.focused, vec![Id::new(target)]);
                let control = focused
                    .bounds
                    .iter()
                    .find(|(id, _)| id == &Id::new(target))
                    .unwrap()
                    .1;
                assert!(
                    control.y >= 4.0,
                    "{case} top focus ring clips at {width}x{height} {mode:?}",
                    case = case,
                    width = width,
                    height = height,
                    mode = mode
                );
                let expected_sidebar = if width == 1024 {
                    COMPACT_WIDTH
                } else {
                    FULL_WIDTH
                };
                assert_eq!(
                    node.children()[1].children()[0].bounds().width,
                    expected_sidebar
                );
                let redraw = Event::Window(iced::window::Event::RedrawRequested(
                    std::time::Instant::now(),
                ));
                assert!(send(
                    &mut element,
                    &mut tree,
                    &node,
                    &renderer,
                    redraw,
                    mouse::Cursor::Unavailable,
                    size
                )
                .is_empty());
                test_draw(&mut element, &mut tree, &node, &mut renderer, &theme, size);
                let pixels = renderer.screenshot(
                    Size::new(width, height),
                    1.0,
                    theme.colors.shell.background,
                );
                let x = (control.x + control.width / 2.0) as u32;
                let y = (control.y - 3.0) as u32;
                let i = ((y * width + x) * 4) as usize;
                let actual = [pixels[i], pixels[i + 1], pixels[i + 2]];
                let focus = theme.colors.shell.focus;
                let expected =
                    [focus.r, focus.g, focus.b].map(|component| (component * 255.0) as u8);
                assert!(
                    actual
                        .iter()
                        .zip(expected)
                        .all(|(actual, expected)| actual.abs_diff(expected) <= 2),
                    "focus ring clipped or wrong color for {case} at {width}x{height} {mode:?}: {actual:?} != {expected:?}",
                    case = case,
                    width = width,
                    height = height,
                    mode = mode,
                    actual = actual,
                    expected = expected
                );
                if let Ok(directory) = std::env::var("SHELL_KEYBOARD_PREVIEW_DIR") {
                    std::fs::create_dir_all(&directory).unwrap();
                    image::save_buffer(
                        std::path::Path::new(&directory).join(format!(
                            "tenshu-keyboard-focus-{case}-{mode:?}-{width}x{height}.png"
                        )),
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
}

#[tokio::test]
async fn selected_successor_does_not_paint_over_focused_nav_ring() {
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
        for (width, height) in [(1024, 768), (1440, 900)] {
            let mut ctx = fixture_context();
            ctx.theme_mode = mode;
            for (case, menu, focused_id) in [
                (
                    "backup-after-activity",
                    Menu::Cube(CubeSubMenu::Settings(CubeSettingsOption::Recovery)),
                    "tenshu.shell.nav.Activity",
                ),
                (
                    "vault-after-spark",
                    Menu::Vault(VaultSubMenu::Overview),
                    "tenshu.shell.nav.Spark",
                ),
            ] {
                let mut element = shell_with_input(&ctx, &menu, ShellState::default());
                let mut renderer = <iced::Renderer as Headless>::new(
                    coincube_ui::font::REGULAR,
                    iced::Pixels(14.0),
                    Some("tiny-skia"),
                )
                .await
                .unwrap();
                let mut tree = Tree::new(element.as_widget());
                let size = Size::new(width as f32, height as f32);
                let node = element.as_widget_mut().layout(
                    &mut tree,
                    &renderer,
                    &layout::Limits::new(Size::ZERO, size),
                );
                // Arrive as a keyboard user does: only keyboard navigation
                // paints the ring (`coincube_ui::widget::action`).
                send(
                    &mut element,
                    &mut tree,
                    &node,
                    &renderer,
                    key(keyboard::key::Named::Tab, false, true),
                    mouse::Cursor::Unavailable,
                    size,
                );
                operate(
                    &mut element,
                    &mut tree,
                    &node,
                    &renderer,
                    focusable::focus::<()>(Id::new(focused_id)),
                );
                let snapshot = focus_snapshot(&mut element, &mut tree, &node, &renderer);
                assert_eq!(snapshot.focused, vec![Id::new(focused_id)]);
                let control = snapshot
                    .bounds
                    .iter()
                    .find(|(id, _)| id == &Id::new(focused_id))
                    .unwrap()
                    .1;
                let redraw = Event::Window(iced::window::Event::RedrawRequested(
                    std::time::Instant::now(),
                ));
                assert!(send(
                    &mut element,
                    &mut tree,
                    &node,
                    &renderer,
                    redraw,
                    mouse::Cursor::Unavailable,
                    size
                )
                .is_empty());
                test_draw(&mut element, &mut tree, &node, &mut renderer, &theme, size);
                let pixels = renderer.screenshot(
                    Size::new(width, height),
                    1.0,
                    theme.colors.shell.background,
                );
                let focus = theme.colors.shell.focus;
                let expected =
                    [focus.r, focus.g, focus.b].map(|component| (component * 255.0) as u8);
                let center_x = (control.x + control.width / 2.0) as u32;
                let center_y = (control.y + control.height / 2.0) as u32;
                let samples = [
                    ("top", center_x, (control.y - 3.0) as u32),
                    (
                        "bottom",
                        center_x,
                        (control.y + control.height + 3.0) as u32,
                    ),
                    ("left", (control.x - 3.0) as u32, center_y),
                    ("right", (control.x + control.width + 3.0) as u32, center_y),
                ];
                for (edge, x, y) in samples {
                    let i = ((y * width + x) * 4) as usize;
                    let actual = [pixels[i], pixels[i + 1], pixels[i + 2]];
                    assert!(
                        actual
                            .iter()
                            .zip(expected)
                            .all(|(actual, expected)| actual.abs_diff(expected) <= 2),
                        "{case} {mode:?} {width}x{height} focused {edge} ring was covered: {actual:?} != {expected:?}",
                        case = case,
                        mode = mode,
                        width = width,
                        height = height,
                        edge = edge,
                        actual = actual,
                        expected = expected
                    );
                }
                if let Ok(directory) = std::env::var("SHELL_KEYBOARD_PREVIEW_DIR") {
                    std::fs::create_dir_all(&directory).unwrap();
                    image::save_buffer(
                        std::path::Path::new(&directory).join(format!(
                            "tenshu-keyboard-adjacent-{case}-{mode:?}-{width}x{height}.png"
                        )),
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
}

/// Focusable rows of the production sidebar on `menu`.
fn sidebar_targets(ctx: &NavContext<'_>, menu: &Menu) -> usize {
    load_fonts();
    let renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
        coincube_ui::font::REGULAR,
        14.into(),
        Some("tiny-skia"),
    ))
    .unwrap();
    let mut element = sidebar(menu, ctx, ShellState::default());
    let mut tree = Tree::new(element.as_widget());
    let node = element.as_widget_mut().layout(
        &mut tree,
        &renderer,
        &layout::Limits::new(Size::ZERO, Size::new(320.0, 4000.0)),
    );
    focus_snapshot(&mut element, &mut tree, &node, &renderer)
        .bounds
        .len()
}

#[test]
fn wallets_have_no_sidebar_sub_menus() {
    // The canvas sidebar lists each wallet once; Send, Receive and history
    // live on the wallet's own page. Only Marketplace keeps children.
    let ctx = fixture_context();
    let baseline = sidebar_targets(&ctx, &Menu::Cube(CubeSubMenu::Overview));
    // No Vault Tools group either: those tools are links on the Vault page.
    let all_ids = {
        load_fonts();
        let renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
            coincube_ui::font::REGULAR,
            14.into(),
            Some("tiny-skia"),
        ))
        .unwrap();
        let menu = Menu::Vault(VaultSubMenu::Overview);
        let mut element = sidebar(&menu, &ctx, ShellState::default());
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(Size::ZERO, Size::new(320.0, 4000.0)),
        );
        focus_snapshot(&mut element, &mut tree, &node, &renderer).bounds
    };
    assert!(
        !all_ids
            .iter()
            .any(|(id, _)| *id == Id::new("tenshu.shell.nav.Tools")),
        "the sidebar still offers Vault Tools"
    );
    for menu in [
        Menu::Spark(SparkSubMenu::Overview),
        Menu::Spark(SparkSubMenu::Send),
        Menu::Spark(SparkSubMenu::Receive),
        Menu::Spark(SparkSubMenu::Transactions(None)),
        Menu::Vault(VaultSubMenu::Overview),
        Menu::Vault(VaultSubMenu::Send),
        Menu::Vault(VaultSubMenu::Receive),
        Menu::Vault(VaultSubMenu::Transactions(None)),
    ] {
        assert_eq!(
            sidebar_targets(&ctx, &menu),
            baseline,
            "{:?} grew the sidebar",
            menu
        );
    }
}

#[test]
fn switcher_menu_covers_the_nav_without_moving_it_and_keeps_tab_order() {
    load_fonts();
    let renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
        coincube_ui::font::REGULAR,
        14.into(),
        Some("tiny-skia"),
    ))
    .unwrap();
    let ctx = fixture_context();
    let size = Size::new(1024.0, 768.0);
    let mount = |state: ShellState| {
        let mut element = shell_tree(&ctx, state);
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(Size::ZERO, size),
        );
        (element, tree, node)
    };
    let (mut closed, mut closed_tree, closed_node) = mount(ShellState::default());
    let shut = focus_snapshot(&mut closed, &mut closed_tree, &closed_node, &renderer);
    let (mut open, mut open_tree, open_node) = mount(ShellState {
        switcher_open: true,
        ..ShellState::default()
    });
    let shown = focus_snapshot(&mut open, &mut open_tree, &open_node, &renderer);

    for (id, bounds) in &shut.bounds {
        let moved = shown
            .bounds
            .iter()
            .find(|(other, _)| other == id)
            .map(|(_, b)| *b);
        assert_eq!(moved, Some(*bounds), "{:?} moved when the menu opened", id);
    }
    let switcher = bounds_of(&shown.bounds, CUBE_SWITCHER_ID);
    let all_cubes = bounds_of(&shown.bounds, "tenshu.launcher.all-cubes");
    let overview = bounds_of(&shown.bounds, "tenshu.shell.nav.Overview");
    assert!(all_cubes.y >= switcher.y + switcher.height);
    assert!(
        all_cubes.intersects(&overview),
        "the menu lies over the first rows: {:?} {:?}",
        all_cubes,
        overview
    );

    // Tab still goes from the switcher into the menu.
    operate(
        &mut open,
        &mut open_tree,
        &open_node,
        &renderer,
        focusable::focus::<()>(Id::new(CUBE_SWITCHER_ID)),
    );
    operate(
        &mut open,
        &mut open_tree,
        &open_node,
        &renderer,
        focusable::focus_next::<()>(),
    );
    assert_eq!(
        operate(
            &mut open,
            &mut open_tree,
            &open_node,
            &renderer,
            focusable::find_focused(),
        ),
        Some(Id::new("tenshu.launcher.all-cubes"))
    );
}

fn bounds_of(bounds: &[(Id, Rectangle)], id: &'static str) -> Rectangle {
    bounds
        .iter()
        .find_map(|(other, b)| (*other == Id::new(id)).then_some(*b))
        .unwrap_or_else(|| panic!("{} has no focus target", id))
}

/// The Connect panel in every state the design table names, with its exact
/// labels. With `R06A_CAPTURE_DIR` set the captures are saved for review.
#[test]
fn connect_panel_says_what_the_state_table_says() {
    use crate::app::view::shared::landing::capture;
    use crate::app::{SignedOutBy, VaultAlert, VaultStall};
    static CONNECTED: ConnectionStatus = ConnectionStatus::Connected;
    static OFFLINE: ConnectionStatus = ConnectionStatus::Offline;
    static RECONNECTING: ConnectionStatus = ConnectionStatus::Reconnecting;
    static RENEWING: ConnectionStatus = ConnectionStatus::Renewing;
    static FAILED: ConnectionStatus = ConnectionStatus::RenewFailed;
    static INACTIVE: ConnectionStatus = ConnectionStatus::Inactive;
    let offline_vault = VaultAlert {
        stall: VaultStall::Offline,
        via_connect: true,
        needs_sign_in: false,
        last_synced: Some("14:02".into()),
        block: 974_600,
    };
    let signed_out_vault = VaultAlert {
        stall: VaultStall::NotUpdating,
        via_connect: true,
        needs_sign_in: true,
        last_synced: Some("09:14".into()),
        block: 974_571,
    };
    struct Case<'a> {
        name: &'static str,
        signed_in: bool,
        status: &'static ConnectionStatus,
        signed_out_by: Option<SignedOutBy>,
        vault: Option<&'a VaultAlert>,
        says: &'static [&'static str],
        never: &'static [&'static str],
    }
    let cases = [
        Case {
            name: "healthy",
            signed_in: true,
            status: &CONNECTED,
            signed_out_by: None,
            vault: None,
            says: &[
                "Signed in to COINCUBE | Connect",
                "Live updates on this computer",
                "On",
                "Connect account",
                "Connections settings",
            ],
            never: &["Vault", "Try again"],
        },
        Case {
            name: "offline",
            signed_in: true,
            status: &OFFLINE,
            signed_out_by: None,
            vault: Some(&offline_vault),
            says: &[
                "Offline",
                "No internet connection.",
                "Family savings",
                "The balance shown is from 14:02.",
            ],
            never: &["Try again"],
        },
        Case {
            name: "unreachable",
            signed_in: true,
            status: &RECONNECTING,
            signed_out_by: None,
            vault: None,
            says: &[
                "Reconnecting…",
                "COINCUBE Desktop can’t reach Connect right now",
            ],
            never: &["Try again", "registration"],
        },
        Case {
            name: "renewing",
            signed_in: true,
            status: &RENEWING,
            signed_out_by: None,
            vault: None,
            says: &["Reconnecting…", "Nothing to do."],
            never: &["Try again", "Device ID"],
        },
        Case {
            name: "failed",
            signed_in: true,
            status: &FAILED,
            signed_out_by: None,
            vault: None,
            says: &[
                "Offline",
                "Couldn’t reconnect this computer",
                "Try again",
                "Connections settings",
            ],
            never: &["Paused", "Re-register"],
        },
        Case {
            name: "signed-out-by-connect",
            signed_in: false,
            status: &INACTIVE,
            signed_out_by: Some(SignedOutBy::ComputerRemoved),
            vault: Some(&signed_out_vault),
            says: &[
                "Signed out of COINCUBE | Connect",
                "This computer was removed from your Connect account",
                "Sign in",
                "Not now",
                "Not updating",
                "which needs you signed in",
            ],
            never: &["unlocked", "Connections settings"],
        },
    ];
    for case in &cases {
        for (appearance, palette) in [
            ("dark", theme::Theme::dark()),
            ("light", theme::Theme::light()),
        ] {
            let mut ctx = fixture_context();
            ctx.connect_authenticated = case.signed_in;
            ctx.connect_email = case.signed_in.then_some("alex@example.test");
            ctx.connect_stream_status = case.status;
            ctx.connect.signed_out_by = case.signed_out_by;
            ctx.connect.vault_alert = case.vault;
            let bar = capture::inspect_filled(
                &format!("connect-control-{}", case.name),
                top_bar_with_privacy(&ctx, None),
                (1024, 65),
                appearance,
                palette,
                (1024, 65),
            );
            assert!(bar.contains("Connect"), "{}", case.name);
            let panel = capture::inspect_full_window(
                &format!("connect-panel-{}", case.name),
                container(connect_panel(&ctx)).padding(16).into(),
                (392, 460),
                appearance,
                palette,
            );
            for label in case.says {
                assert!(panel.contains(label), "{}: {}", case.name, label);
            }
            for label in case.never {
                assert!(!panel.contains(label), "{}: {}", case.name, label);
            }
        }
    }
}
