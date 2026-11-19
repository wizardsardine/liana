use coincube_ui::locale::copy as localized_copy;
mod message;

pub mod activity;

pub mod buysell;
pub mod connect;
pub mod duress;
pub mod global_home;
pub mod nav;
pub mod p2p;
pub mod preferences;
pub mod settings;
pub mod shared;
pub mod spark;
pub mod wallet_header;

pub mod vault;

pub use message::*;
pub use spark::{
    SparkOverviewMessage, SparkOverviewView, SparkReceiveMessage, SparkReceiveView,
    SparkSendMessage, SparkSendView, SparkSettingsMessage, SparkSettingsStatus, SparkSettingsView,
    SparkStatus, SparkTransactionsMessage, SparkTransactionsStatus, SparkTransactionsView,
};
pub use vault::fiat::FiatAmountConverter;
pub use vault::warning::warn;

use iced::{
    widget::{container, row, scrollable, Space},
    Alignment, Length,
};

use coincube_ui::component::tenshu::{
    gutter_for_window,
    page::{PAGE_BOTTOM, PAGE_TOP},
    page_head::BODY_TOP,
    PageHead, PageWidth,
};
use coincube_ui::component::text::Text;
use coincube_ui::{
    color,
    component::{button, text, text::*},
    icon::cross_icon,
    theme,
    widget::*,
};

use crate::app::{
    cache::{Cache, SparkNotice},
    menu::Menu,
};

/// Wraps `content` in the shared balance card style used across wallet overview and send screens
/// (themed card background, orange border, rounded corners).
pub fn balance_header_card<'a, Msg: 'a>(content: impl Into<Element<'a, Msg>>) -> Element<'a, Msg> {
    container(content)
        .padding(20)
        .width(Length::Fill)
        .style(theme::container::balance_header)
        .into()
}

/// The user-facing sentence for a [`SparkNotice`]. Split out so the copy
/// is testable without rendering.
pub fn spark_notice_text(notice: &SparkNotice) -> String {
    match notice {
        SparkNotice::StableBalancePaused { reason } => format!(
            "Stable Balance was paused: converting bitcoin to USDB failed {} times in a row \
             ({reason}). Your funds are unchanged. You can turn it back on in Spark settings \
             once the issue clears.",
            coincube_spark_protocol::STABLE_BALANCE_PAUSE_THRESHOLD
        ),
        SparkNotice::StableBalanceOffWithHolding => "This Spark wallet holds USDB from when \
             Stable Balance was on, but the setting is off on this device — it doesn't travel \
             with your seed. Review it in Spark settings."
            .to_string(),
    }
}

/// A warning strip for a Spark Stable Balance condition, rendered by the
/// `dashboard` wrapper when `cache.spark_notice` is set.
/// "Spark Settings" routes to the page with the
/// toggle; × dismisses for the session.
pub fn spark_notice_banner(notice: &SparkNotice) -> Element<'_, Message> {
    let body = container(
        row![
            coincube_ui::icon::warning_icon().color(color::BLACK),
            // `Fill`, not the default `Shrink`: a shrink text is laid out
            // first with the whole row available, wraps into all of it, and
            // pushes the buttons off the edge. A fill child is sized after
            // the fixed ones, so it wraps within what's actually left.
            text::p2_regular(spark_notice_text(notice))
                .color(color::BLACK)
                .width(Length::Fill),
            button::secondary(None, "Spark Settings")
                .padding([6, 14])
                .width(Length::Fixed(140.0))
                .on_press(Message::Menu(Menu::Spark(
                    crate::app::menu::SparkSubMenu::Settings(None),
                ))),
            iced::widget::Button::new(
                cross_icon()
                    .align_x(Alignment::Center)
                    .align_y(Alignment::Center),
            )
            .padding([8, 10])
            .style(theme::button::secondary)
            .on_press(Message::DismissSparkNotice),
        ]
        .spacing(10)
        .align_y(Alignment::Center),
    )
    .padding([8, 16])
    .width(Length::Fill)
    .style(theme::notification::warning);

    page(body, PageWidth::Reading).vertical(8.0, 8.0).into()
}

/// A Cube page with no head of its own. Settings and the Vault's Tools still
/// get theirs from the route (see [`dashboard_with_info_layout`]).
pub fn dashboard<'a, T: Into<Element<'a, Message>>>(
    menu: &'a Menu,
    cache: &'a Cache,
    content: T,
) -> Element<'a, Message> {
    dashboard_with_info(
        menu,
        cache,
        content,
        &cache.cube_name,
        cache.avatar_handle.as_ref(),
        cache.lightning_address.as_deref(),
    )
}

/// A Cube page under a pinned [`page_head`]: the head stays put and only
/// `content` scrolls (HANDOFF "v175 delta", "Page head").
pub fn dashboard_headed<'a, T: Into<Element<'a, Message>>>(
    menu: &'a Menu,
    cache: &'a Cache,
    head: PageHead<'a, Message>,
    content: T,
) -> Element<'a, Message> {
    dashboard_with_info_layout(
        menu,
        cache,
        Some(head),
        content,
        &cache.cube_name,
        cache.avatar_handle.as_ref(),
        cache.lightning_address.as_deref(),
        PageWidth::Reading,
    )
}

/// The Cube's coin, the mark before a page head's Cube line (`.idl > img`).
pub fn cube_coin<'a>(cache: &Cache) -> Element<'a, Message> {
    let light = matches!(
        cache.theme_mode,
        coincube_ui::theme::palette::ThemeMode::Light
    );
    iced::widget::Svg::new(iced::widget::svg::Handle::from_memory(
        crate::home::launcher_coin::widget_source(cache.chain(), 16.0, 1.0, light).into_bytes(),
    ))
    .width(Length::Fixed(16.0))
    .height(Length::Fixed(16.0))
    .into()
}

/// "‹name› · ‹network›", the line under a page title.
pub fn network_line(cache: &Cache, name: &str) -> String {
    format!("{name} · {}", nav::shell::network_label(cache.chain()))
}

/// The Cube's name as the side panel shows it, or "Cube" while it has none.
/// Names are saved without a length limit and may hold line breaks, and a
/// pinned head doesn't scroll, so heads get the same one-line, 16-character
/// cut.
pub fn cube_display_name(cache: &Cache) -> String {
    if cache.cube_name.is_empty() {
        "Cube".to_owned()
    } else {
        nav::shell::short_cube_name(&cache.cube_name)
    }
}

/// A pinned page head: `title`, then the Cube's coin and "‹Cube› · ‹network›".
/// Add the page's actions and its back target with the [`PageHead`] builders.
pub fn page_head<'a>(
    cache: &Cache,
    title: impl iced::widget::text::IntoFragment<'a>,
) -> PageHead<'a, Message> {
    coincube_ui::component::tenshu::page_head(title, Message::FocusAction).context(
        cube_coin(cache),
        network_line(cache, &cube_display_name(cache)),
    )
}

/// A wallet page's head: "Spark · ‹nickname›" or "Vault · ‹alias›" as the
/// side panel names it, plain "Spark" or "Vault" without a name.
pub fn wallet_page_head<'a>(
    cache: &Cache,
    wallet: &str,
    name: Option<&str>,
) -> PageHead<'a, Message> {
    page_head(cache, nav::shell::wallet_label(wallet, name))
}

/// The head of a page inside a wallet (Send, Receive, its activity, a
/// transaction): `title`, then the coin and "‹wallet› · ‹Cube› · ‹network›",
/// so the page still says which wallet it belongs to (the H-Send boards).
pub fn wallet_sub_page_head<'a>(
    cache: &Cache,
    wallet: &str,
    name: Option<&str>,
    title: impl iced::widget::text::IntoFragment<'a>,
) -> PageHead<'a, Message> {
    coincube_ui::component::tenshu::page_head(title, Message::FocusAction).context(
        cube_coin(cache),
        format!(
            "{} · {}",
            nav::shell::wallet_label(wallet, name),
            network_line(cache, &cube_display_name(cache))
        ),
    )
}

/// [`wallet_sub_page_head`] for a Spark page.
pub fn spark_page_head<'a>(
    cache: &Cache,
    title: impl iced::widget::text::IntoFragment<'a>,
) -> PageHead<'a, Message> {
    wallet_sub_page_head(cache, "Spark", cache.wallet_names.spark.as_deref(), title)
}

/// [`wallet_sub_page_head`] for a Vault page.
pub fn vault_page_head<'a>(
    cache: &Cache,
    title: impl iced::widget::text::IntoFragment<'a>,
) -> PageHead<'a, Message> {
    wallet_sub_page_head(cache, "Vault", cache.wallet_names.vault.as_deref(), title)
}

/// Activity fills the content area edge to edge (`.main`) so its header and
/// filters stay put while the list and the detail pane scroll on their own;
/// `view::activity` puts them on the same centred page column as [`page`].
pub fn dashboard_activity<'a, T: Into<Element<'a, Message>>>(
    menu: &'a Menu,
    cache: &'a Cache,
    content: T,
) -> Element<'a, Message> {
    let content_column: Element<'_, Message> = Column::new()
        .push(warn(None))
        .push_maybe(cache.spark_notice.as_ref().map(spark_notice_banner))
        .push(
            Container::new(content)
                .width(Length::Fill)
                .height(Length::Fill)
                .style(|theme: &theme::Theme| iced::widget::container::Style {
                    background: Some(iced::Background::Color(theme.colors.shell.background)),
                    ..Default::default()
                }),
        )
        .width(Length::Fill)
        .into();
    dashboard_shell_with_info(
        menu,
        cache,
        content_column,
        &cache.cube_name,
        cache.avatar_handle.as_ref(),
        cache.lightning_address.as_deref(),
    )
}

/// Send and Receive: a reading page. Their 680px form and 240px context rail
/// sit together from the left edge at 1440px and wrap at the 1024px minimum
/// window size.
pub fn dashboard_payment<'a, T: Into<Element<'a, Message>>>(
    menu: &'a Menu,
    cache: &'a Cache,
    head: Option<PageHead<'a, Message>>,
    content: T,
) -> Element<'a, Message> {
    dashboard_with_info_layout(
        menu,
        cache,
        head,
        content,
        &cache.cube_name,
        cache.avatar_handle.as_ref(),
        cache.lightning_address.as_deref(),
        PageWidth::Reading,
    )
}

/// The Cube shell's page gutter for a content pane `pane_width` wide.
///
/// The canvas rule is `clamp(24px, 3vw, 48px)` of the *window*; the pane is
/// the window minus the side menu, whose width itself depends on the window
/// (`nav::split::sidebar_width_for_window`), so add the menu back first.
pub fn page_gutter(pane_width: f32) -> f32 {
    let with_full_menu = pane_width + nav::shell::FULL_WIDTH;
    let window = if nav::split::sidebar_width_for_window(with_full_menu) == nav::shell::FULL_WIDTH {
        with_full_menu
    } else {
        pane_width + nav::shell::COMPACT_WIDTH
    };
    gutter_for_window(window)
}

/// Every Cube screen's page container (HANDOFF "Page layout"): one column at
/// most `PAGE_MAX` wide, centred, at least [`page_gutter`] from each side.
pub fn page<'a, T: Into<Element<'a, Message>>>(
    content: T,
    width: PageWidth,
) -> coincube_ui::component::tenshu::Page<'a, Message> {
    coincube_ui::component::tenshu::page(content, width, page_gutter)
}

pub fn dashboard_with_info<'a, T: Into<Element<'a, Message>>>(
    menu: &'a Menu,
    cache: &'a Cache,
    content: T,
    cube_name: &'a str,
    avatar_handle: Option<&'a iced::widget::image::Handle>,
    lightning_address: Option<&'a str>,
) -> Element<'a, Message> {
    dashboard_with_info_layout(
        menu,
        cache,
        None,
        content,
        cube_name,
        avatar_handle,
        lightning_address,
        PageWidth::Reading,
    )
}

/// The pinned head over a page's scrollable, or an empty slot. The slot is
/// always there, so a head that comes and goes (a Send that turns into its
/// celebration) never moves the scrollable in the tree and resets its offset.
fn head_slot<'a>(head: Option<PageHead<'a, Message>>) -> Element<'a, Message> {
    head.map(|head| head.view(page_gutter))
        .unwrap_or_else(|| Space::new().into())
}

/// The page body's top padding: tighter under a head (`.main .pg`).
fn body_top(headed: bool) -> f32 {
    if headed {
        BODY_TOP
    } else {
        PAGE_TOP
    }
}

/// A full-page replacement for the route's own page: the received-payment
/// celebration, or the placeholder for a feature this network lacks. It has
/// no head, and takes none of the route's chrome either (Settings' scope
/// switch and tabs, the Vault's Tools head and tabs): those belong to the
/// page it replaces.
pub fn replacement_page<'a, T: Into<Element<'a, Message>>>(
    menu: &'a Menu,
    cache: &'a Cache,
    content: T,
) -> Element<'a, Message> {
    dashboard_layout(
        menu,
        cache,
        None,
        false,
        content,
        &cache.cube_name,
        cache.avatar_handle.as_ref(),
        cache.lightning_address.as_deref(),
        PageWidth::Reading,
    )
}

/// `head` is the page's own; without one, Settings and the Vault's Tools
/// take theirs from the route. Settings carries its scope switch and tabs in
/// the head, and the Tools head its tabs (plan W6).
#[allow(clippy::too_many_arguments)]
fn dashboard_with_info_layout<'a, T: Into<Element<'a, Message>>>(
    menu: &'a Menu,
    cache: &'a Cache,
    head: Option<PageHead<'a, Message>>,
    content: T,
    cube_name: &'a str,
    avatar_handle: Option<&'a iced::widget::image::Handle>,
    lightning_address: Option<&'a str>,
    width: PageWidth,
) -> Element<'a, Message> {
    dashboard_layout(
        menu,
        cache,
        head,
        true,
        content,
        cube_name,
        avatar_handle,
        lightning_address,
        width,
    )
}

/// A Cube page under `head`, and with the route's chrome when `route_chrome`
/// (see [`dashboard_with_info_layout`] and [`replacement_page`]).
#[allow(clippy::too_many_arguments)]
fn dashboard_layout<'a, T: Into<Element<'a, Message>>>(
    menu: &'a Menu,
    cache: &'a Cache,
    head: Option<PageHead<'a, Message>>,
    route_chrome: bool,
    content: T,
    cube_name: &'a str,
    avatar_handle: Option<&'a iced::widget::image::Handle>,
    lightning_address: Option<&'a str>,
    width: PageWidth,
) -> Element<'a, Message> {
    // A Vault tool's own head whose rule holds its steps (Recovery) leaves no
    // room there for the Tools tabs, which then stay in the body.
    let own_steps = head.as_ref().is_some_and(PageHead::has_rule);
    let head = if route_chrome {
        head.map(|head| vault::tools::with_back(menu, cache, head))
            .or_else(|| settings::frame::page_head(menu, cache))
            .or_else(|| vault::tools::page_head(menu, cache))
    } else {
        head
    };
    let body = page(
        Column::new()
            .push_maybe(
                route_chrome
                    .then(|| vault::tools::in_content_navigation(menu, cache, own_steps))
                    .flatten(),
            )
            .push(content.into())
            .width(Length::Fill),
        width,
    )
    .vertical(body_top(head.is_some()), PAGE_BOTTOM);
    let content_column: Element<'_, Message> = Column::new()
        .push(warn(None))
        .push_maybe(cache.spark_notice.as_ref().map(spark_notice_banner))
        .push(head_slot(head))
        .push(
            Container::new(scrollable(body).on_scroll(|w| Message::Scroll(w.absolute_offset().y)))
                .width(Length::Fill)
                .style(theme::container::background)
                .height(Length::Fill),
        )
        .width(Length::Fill)
        .into();

    dashboard_shell_with_info(
        menu,
        cache,
        content_column,
        cube_name,
        avatar_handle,
        lightning_address,
    )
}

/// Iced invokes responsive builders during layout, after the root view has
/// returned. Carry the window's presentation preferences into that build.
pub(crate) fn responsive<'a, M: 'a>(
    build: impl Fn(iced::Size) -> Element<'a, M> + 'a,
) -> iced::widget::Responsive<'a, M, theme::Theme, iced::Renderer> {
    let language = coincube_ui::locale::language();
    let mode = coincube_ui::worldview_copy::mode();
    iced::widget::responsive(move |size| {
        coincube_ui::locale::render_in(language, || {
            coincube_ui::worldview_copy::render_in(mode, || build(size))
        })
    })
}

const WALLET_PAGE_SCROLL_ID: &str = "tenshu.wallet.page.scroll";

/// A reading page with Activity's detail pane (`.det`) for a selected row:
/// beside the page at the content's right edge when both fit, else over the
/// page from that edge. `content` and `detail` are rebuilt per layout width.
///
/// The pinned `head` spans the page beside the pane, not the pane itself.
pub fn dashboard_with_detail<'a>(
    menu: &'a Menu,
    cache: &'a Cache,
    head: impl Fn() -> PageHead<'a, Message> + 'a,
    content: impl Fn() -> Element<'a, Message> + 'a,
    detail: impl Fn() -> Option<Element<'a, Message>> + 'a,
) -> Element<'a, Message> {
    let body = responsive(move |size: iced::Size| {
        let page_body = Column::new()
            .push(head_slot(Some(head())))
            .push(
                Container::new(
                    scrollable(
                        page(
                            Column::new().push(content()).width(Length::Fill),
                            PageWidth::Reading,
                        )
                        .vertical(BODY_TOP, PAGE_BOTTOM),
                    )
                    .id(iced::advanced::widget::Id::from(WALLET_PAGE_SCROLL_ID))
                    .on_scroll(|w| Message::Scroll(w.absolute_offset().y)),
                )
                .width(Length::Fill)
                .style(theme::container::background)
                .height(Length::Fill),
            )
            .width(Length::Fill)
            .height(Length::Fill);
        // The page keeps one place in the tree, pane or not, beside or over:
        // otherwise Iced drops its scroll offset when a row is selected.
        let detail = detail();
        let beside = activity::wallet_detail_beside(size.width);
        let mut split = Row::new().push(page_body).height(Length::Fill);
        let mut over: Element<'a, Message> = Space::new().into();
        match detail {
            Some(detail) if beside => {
                split = split
                    .push(iced::widget::rule::vertical(1))
                    .push(activity::detail_column(detail));
            }
            Some(detail) => {
                over = row![
                    Space::new().width(Length::Fill),
                    iced::widget::opaque(row![
                        iced::widget::rule::vertical(1),
                        activity::detail_column(detail)
                    ])
                ]
                .into();
            }
            None => {}
        }
        iced::widget::Stack::new().push(split).push(over).into()
    });
    let content_column: Element<'_, Message> = Column::new()
        .push(warn(None))
        .push_maybe(cache.spark_notice.as_ref().map(spark_notice_banner))
        .push(body)
        .width(Length::Fill)
        .into();
    dashboard_shell_with_info(
        menu,
        cache,
        content_column,
        &cache.cube_name,
        cache.avatar_handle.as_ref(),
        cache.lightning_address.as_deref(),
    )
}

/// Use the same shell for panels with their own scroll handling, such as P2P
/// chat. `body` owns its content and warnings; the shell owns all navigation.
///
/// The shell adds no page head. P2P order details pins its own above its
/// scrollable. The chats have none (plan L3 exception): each lays out its own
/// header, message list and composer to the pane's full height, and a head
/// above it would take that height from the messages.
pub fn dashboard_shell<'a, T: Into<Element<'a, Message>>>(
    menu: &'a Menu,
    cache: &'a Cache,
    body: T,
) -> Element<'a, Message> {
    dashboard_shell_with_info(
        menu,
        cache,
        body.into(),
        &cache.cube_name,
        cache.avatar_handle.as_ref(),
        cache.lightning_address.as_deref(),
    )
}

fn dashboard_shell_with_info<'a>(
    menu: &'a Menu,
    cache: &'a Cache,
    content_column: Element<'a, Message>,
    cube_name: &'a str,
    avatar_handle: Option<&'a iced::widget::image::Handle>,
    lightning_address: Option<&'a str>,
) -> Element<'a, Message> {
    // Passkey Cubes used to be exempt from this banner, because there was no
    // way for one to back its seed up. There is now (**I11**), so an
    // un-backed-up passkey Cube gets the same nudge as any other — it is the
    // Cube that most needs it.
    let show_backup_warning = !cache.current_cube_backed_up;
    let nav_ctx = nav::NavContext {
        cube_name,
        lightning_address,
        avatar: avatar_handle,
        ..nav::NavContext::from_cache(cache)
    };

    let shell_state = nav::shell::ShellState {
        switcher_open: cache.cube_switcher_open,
        connect_open: cache.connect_panel_open,
        lock_confirmation: cache.cube_lock_confirmation,
        backup_attention: show_backup_warning,
    };
    Column::new()
        .push(nav::shell::top_bar_with_privacy(
            &nav_ctx,
            cache.balance_privacy,
        ))
        .push(nav::split::ShellSplit::new(
            nav::shell::sidebar(menu, &nav_ctx, shell_state),
            content_column,
        ))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

pub fn modal<'a, T: Into<Element<'a, Message>>, F: Into<Element<'a, Message>>>(
    is_previous: bool,
    content: T,
    fixed_footer: Option<F>,
) -> Element<'a, Message> {
    Column::new()
        .push(
            Container::new(
                Row::new()
                    .push(if is_previous {
                        Column::new()
                            .push(
                                button::secondary(None, "< Back")
                                    .width(Length::Fixed(150.0))
                                    .on_press(Message::Previous),
                            )
                            .width(Length::Fill)
                    } else {
                        Column::new().width(Length::Fill)
                    })
                    .align_y(iced::Alignment::Center)
                    .push(
                        button::secondary(Some(cross_icon()), localized_copy("Close"))
                            .on_press(Message::Close),
                    ),
            )
            .padding(10)
            .style(theme::container::background),
        )
        .push(modal_section(Container::new(scrollable(content))))
        .push(fixed_footer)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn modal_section<'a, T: 'a>(menu: Container<'a, T>) -> Container<'a, T> {
    Container::new(menu.max_width(1500))
        .style(theme::container::background)
        .center_x(Length::Fill)
        .width(Length::Fill)
        .height(Length::Fill)
}

pub fn placeholder<'a, T: Into<Element<'a, Message>>>(
    icon: T,
    title: &'a str,
    subtitle: &'a str,
) -> Element<'a, Message> {
    let content = Column::new()
        .push(icon)
        .push(text(title).style(theme::text::secondary).bold())
        .push(
            text(subtitle)
                .size(P2_SIZE)
                .style(theme::text::secondary)
                .align_x(Alignment::Center),
        )
        .spacing(16)
        .align_x(Alignment::Center);

    Container::new(content)
        .width(Length::Fill)
        .padding(60)
        .center_x(Length::Fill)
        .style(|t| container::Style {
            background: Some(iced::Background::Color(t.colors.cards.simple.background)),
            border: iced::Border {
                radius: 20.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}

/// Full-panel placeholder for a route whose feature isn't available — e.g. a
/// network-disabled rail item reached by a restored or deep-linked route. Mirrors [`placeholder`] but owns its copy so it can be built
/// from a transient reason string (see
/// [`crate::app::features::route_availability`]).
pub fn feature_unavailable_panel(reason: String) -> Element<'static, Message> {
    let content = Column::new()
        .push(
            coincube_ui::icon::plug_icon()
                .size(48)
                .style(theme::text::secondary),
        )
        .push(
            text(localized_copy("Not available on this network"))
                .style(theme::text::secondary)
                .bold(),
        )
        .push(
            text(reason)
                .size(P2_SIZE)
                .style(theme::text::secondary)
                .align_x(Alignment::Center),
        )
        .spacing(16)
        .align_x(Alignment::Center);

    Container::new(content)
        .width(Length::Fill)
        .padding(60)
        .center_x(Length::Fill)
        .style(|t| container::Style {
            background: Some(iced::Background::Color(t.colors.cards.simple.background)),
            border: iced::Border {
                radius: 20.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}

/// Loading-state card mirroring [`placeholder`]'s layout, but with the
/// app's pulsing-dots `loading_indicator` (the same one shown while a Cube
/// loads) below the title instead of a subtitle. Shown while an initial
/// fetch is in flight so panels don't briefly flash an empty
/// "No transactions" state before data arrives.
pub fn loading_placeholder<'a, T: Into<Element<'a, Message>>>(
    icon: T,
    title: &'a str,
) -> Element<'a, Message> {
    let content = Column::new()
        .push(icon)
        .push(text(title).style(theme::text::secondary).bold())
        .push(crate::loading::loading_indicator::<Message>(None))
        .spacing(16)
        .align_x(Alignment::Center);

    Container::new(content)
        .width(Length::Fill)
        .padding(60)
        .center_x(Length::Fill)
        .style(|t| container::Style {
            background: Some(iced::Background::Color(t.colors.cards.simple.background)),
            border: iced::Border {
                radius: 20.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}

/// Fixed toast width so stacked toasts line up; height follows the copy.
const TOAST_WIDTH: f32 = 420.0;
/// Square dismiss button and icon tile inside a toast.
const TOAST_CONTROL: f32 = 28.0;
/// Glyph size of the toast icon and the dismiss cross.
const TOAST_GLYPH: f32 = 16.0;

/// One toast: a tone icon tile, the message, and a dismiss button when
/// `on_dismiss` is set. Styles resolve against the live theme at draw time,
/// so the toast follows light and dark mode.
fn toast_card<'a>(
    tone: coincube_ui::theme::notification::ToastTone,
    message: String,
    on_dismiss: Option<Message>,
) -> Container<'a, Message> {
    use coincube_ui::component::tenshu::text as tenshu_text;
    use coincube_ui::icon;
    use coincube_ui::theme::notification::{self, ToastTone};
    use coincube_ui::theme::tenshu::button::{action, ActionKind, ActionShape};

    let glyph = match tone {
        ToastTone::Info => icon::tooltip_icon(),
        ToastTone::Success => icon::check_circle_icon(),
        ToastTone::Warning => icon::warning_icon(),
        ToastTone::Error => icon::x_circle_icon(),
        ToastTone::Debug => icon::code_slash_icon(),
    }
    .size(TOAST_GLYPH)
    .style(
        move |t: &coincube_ui::theme::Theme| iced::widget::text::Style {
            color: Some(tone.color(t)),
        },
    );

    let tile = container(glyph)
        .width(TOAST_CONTROL)
        .height(TOAST_CONTROL)
        .center_x(TOAST_CONTROL)
        .center_y(TOAST_CONTROL)
        .style(move |t| notification::toast_icon_tile(t, tone));

    // `Word` wrapping never breaks inside a word, so a txid, address or URL
    // would run past the card; fall back to breaking between glyphs.
    let message = if tone == ToastTone::Debug {
        tenshu_text::mono(message)
    } else {
        tenshu_text::label(message)
    }
    .wrapping(iced::widget::text::Wrapping::WordOrGlyph);

    let mut content = iced::widget::Row::new()
        .spacing(12)
        .align_y(Alignment::Start)
        .push(tile)
        .push(
            container(message)
                .width(Length::Fill)
                .padding(iced::Padding::ZERO.top(4).bottom(4)),
        );

    if let Some(on_dismiss) = on_dismiss {
        // Dismiss on press, not on release: the inner `mouse_area` publishes
        // and captures the press before the Button sees it, so the Button
        // keeps its hover styling but never holds a pressed state. A press
        // can't then carry over to another toast when a new one shifts the
        // capped stack and iced re-pairs card state by slot.
        let cross = iced::widget::mouse_area(
            icon::cross_icon()
                .size(TOAST_GLYPH)
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(Alignment::Center)
                .align_y(Alignment::Center),
        )
        .on_press(on_dismiss.clone());
        content = content.push(
            iced::widget::Button::new(cross)
                .padding(0)
                .width(TOAST_CONTROL)
                .height(TOAST_CONTROL)
                .style(|t, status| action(t, status, ActionKind::Quiet, ActionShape::Icon))
                .on_press(on_dismiss),
        );
    }

    container(content)
        .width(TOAST_WIDTH)
        .padding(iced::Padding {
            top: 10.0,
            right: 8.0,
            bottom: 10.0,
            left: 12.0,
        })
        .style(move |t| notification::toast(t, tone))
        .clip(true)
}

/// Most toasts shown at once, so a burst can't grow the stack up the window.
/// Older toasts wait out of view with their expiry clocks stopped; as shown
/// ones leave, the newest waiting toast comes on screen first, and a burst
/// beyond the waiting backlog drops its oldest (`App::arm_toast_timers`).
pub(crate) const TOAST_MAX_VISIBLE: usize = 3;

/// The newest `TOAST_MAX_VISIBLE` toasts, oldest first so the newest sits
/// nearest the bottom edge.
fn visible_toasts<T>(iter: impl Iterator<Item = T>) -> impl Iterator<Item = T> {
    let all: Vec<T> = iter.collect();
    let hidden = all.len().saturating_sub(TOAST_MAX_VISIBLE);
    all.into_iter().skip(hidden)
}

pub fn toast_overlay<'a, I: Iterator<Item = (usize, log::Level, String)>>(
    iter: I,
) -> coincube_ui::widget::Element<'a, Message> {
    let centered = iced::widget::row![
        // Offset notifications past the full-width Cube shell.
        iced::widget::Space::new().width(nav::shell::FULL_WIDTH),
        // center toasts horizontally
        iced::widget::Space::new().width(iced::Length::Fill),
        // Keyed by toast id so card state follows its toast when the number
        // shown changes. iced only matches keys when the count changes; a
        // capped shift that keeps three cards on screen re-pairs state by
        // slot, which is why the dismiss cross acts on press (`toast_card`).
        iced::widget::keyed_column(visible_toasts(iter).map(|(id, level, content)| {
            // `opaque` captures mouse presses on the card. It doesn't capture
            // other events, but it reports an `Idle` interaction over the
            // card, so the overlay's `Stack` hands the layers below a
            // levitated cursor with no position: hover and the wheel over a
            // toast don't act on the screen underneath (scrollables and
            // `mouse_area` ignore a cursor that isn't over them). Global
            // `event::listen` subscriptions still see every event. Only the
            // cards are wrapped: the rest of the overlay stays click-through.
            let card = iced::widget::opaque(toast_card(
                level.into(),
                coincube_ui::locale::notice(content),
                Some(Message::DismissToast(id)),
            ));
            (id, card)
        }))
        .spacing(8),
        iced::widget::Space::new().width(iced::Length::Fill),
    ];

    // full screen positioning: bottom-centre, 24px above the edge
    let column = iced::widget::column![
        iced::widget::Space::new().height(iced::Length::Fill),
        centered,
        iced::widget::Space::new().height(24),
    ];

    container(column)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .into()
}

#[cfg(test)]
mod toast_overlay_tests {
    use super::{toast_overlay, visible_toasts, Message, TOAST_MAX_VISIBLE};
    use coincube_ui::{font, theme, widget::Renderer};
    use iced::advanced::{
        renderer::Headless,
        widget::{operation, Id, Operation},
    };
    use iced::{mouse, Event, Point, Rectangle, Size};
    use iced_runtime::user_interface::{Cache as UiCache, UserInterface};

    /// Marks a press that reached the screen under the toasts.
    const UNDERLAY: usize = usize::MAX;
    const WINDOW: Size = Size::new(1440.0, 900.0);

    #[derive(Default)]
    struct TextProbe(Vec<(String, Rectangle)>);
    impl Operation for TextProbe {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            operate(self);
        }
        fn text(&mut self, _: Option<&Id>, bounds: Rectangle, value: &str) {
            self.0.push((value.to_owned(), bounds));
        }
    }

    fn toasts(n: usize) -> Vec<(usize, log::Level, String)> {
        (0..n)
            .map(|i| (i, log::Level::Info, format!("Toast number {i}")))
            .collect()
    }

    fn renderer() -> Renderer {
        for bytes in font::load() {
            iced::advanced::graphics::text::font_system()
                .write()
                .unwrap()
                .load_font(bytes);
        }
        iced::futures::executor::block_on(<Renderer as Headless>::new(
            font::REGULAR,
            16.into(),
            Some("tiny-skia"),
        ))
        .unwrap()
    }

    /// The overlay stacked over a full-window clickable screen, as `App::view`
    /// mounts it.
    fn mounted<'a>(
        list: Vec<(usize, log::Level, String)>,
        renderer: &mut Renderer,
    ) -> UserInterface<'a, Message, theme::Theme, Renderer> {
        remounted(list, UiCache::new(), renderer)
    }

    /// Rebuilds over a previous frame's cache, as iced does between updates,
    /// so widget state carries across.
    fn remounted<'a>(
        list: Vec<(usize, log::Level, String)>,
        cache: UiCache,
        renderer: &mut Renderer,
    ) -> UserInterface<'a, Message, theme::Theme, Renderer> {
        let underlay = iced::widget::mouse_area(
            iced::widget::Space::new()
                .width(iced::Length::Fill)
                .height(iced::Length::Fill),
        )
        .on_press(Message::DismissToast(UNDERLAY))
        .on_scroll(|_| Message::DismissToast(UNDERLAY));
        let stack: coincube_ui::widget::Element<'a, Message> = iced::widget::Stack::new()
            .push(underlay)
            .push(toast_overlay(list.into_iter()))
            .into();
        UserInterface::build(stack, WINDOW, cache, renderer)
    }

    fn texts(
        ui: &mut UserInterface<'_, Message, theme::Theme, Renderer>,
        renderer: &Renderer,
    ) -> Vec<(String, Rectangle)> {
        let mut probe = TextProbe::default();
        ui.operate(renderer, &mut operation::black_box(&mut probe));
        probe.0
    }

    fn click(
        ui: &mut UserInterface<'_, Message, theme::Theme, Renderer>,
        renderer: &mut Renderer,
        at: Point,
    ) -> Vec<Message> {
        send(
            ui,
            renderer,
            at,
            &[
                mouse::Event::CursorMoved { position: at },
                mouse::Event::ButtonPressed(mouse::Button::Left),
                mouse::Event::ButtonReleased(mouse::Button::Left),
            ],
        )
    }

    fn send(
        ui: &mut UserInterface<'_, Message, theme::Theme, Renderer>,
        renderer: &mut Renderer,
        at: Point,
        events: &[mouse::Event],
    ) -> Vec<Message> {
        let mut messages = Vec::new();
        let cursor = mouse::Cursor::Available(at);
        for event in events.iter().cloned().map(Event::Mouse) {
            ui.update(
                &[event],
                cursor,
                renderer,
                &mut iced::advanced::clipboard::Null,
                &mut messages,
            );
        }
        messages
    }

    fn reached_underlay(messages: &[Message]) -> bool {
        messages
            .iter()
            .any(|m| matches!(m, Message::DismissToast(UNDERLAY)))
    }

    /// Bounds of the dismiss cross: the glyph drawn right after the message.
    fn cross_of(
        ui: &mut UserInterface<'_, Message, theme::Theme, Renderer>,
        renderer: &Renderer,
        message: &str,
    ) -> Rectangle {
        let all = texts(ui, renderer);
        let at = all
            .iter()
            .position(|(text, _)| text == message)
            .expect("mounted toast message");
        let cross = all.get(at + 1).expect("mounted dismiss cross").1;
        assert!(cross.x > all[at].1.x, "the cross sits right of the message");
        cross
    }

    fn list(ids: &[usize]) -> Vec<(usize, log::Level, String)> {
        ids.iter()
            .map(|&i| (i, log::Level::Info, format!("Toast number {i}")))
            .collect()
    }

    /// The capped stack re-pairs card state by slot when a new toast shifts
    /// three shown toasts. A dismiss must act on press so a held press can't
    /// land on whichever toast moves into its slot.
    #[test]
    fn a_held_dismiss_cannot_move_to_another_toast() {
        let mut renderer = renderer();
        let mut ui = mounted(list(&[4, 5, 6]), &mut renderer);
        let cross = cross_of(&mut ui, &renderer, "Toast number 5");

        let pressed = send(
            &mut ui,
            &mut renderer,
            cross.center(),
            &[
                mouse::Event::CursorMoved {
                    position: cross.center(),
                },
                mouse::Event::ButtonPressed(mouse::Button::Left),
            ],
        );
        assert!(
            pressed
                .iter()
                .any(|m| matches!(m, Message::DismissToast(5))),
            "the press itself dismisses its own toast"
        );

        // Toast 7 arrives before the release (as if 5 were still shown).
        let cache = ui.into_cache();
        let mut ui = remounted(list(&[5, 6, 7]), cache, &mut renderer);
        let released = send(
            &mut ui,
            &mut renderer,
            cross.center(),
            &[mouse::Event::ButtonReleased(mouse::Button::Left)],
        );
        assert!(
            !released
                .iter()
                .any(|m| matches!(m, Message::DismissToast(_))),
            "a release must not dismiss the toast that moved into the slot"
        );
    }

    #[test]
    fn only_the_newest_toasts_are_shown() {
        let shown: Vec<usize> = visible_toasts(toasts(5).into_iter())
            .map(|(id, ..)| id)
            .collect();
        assert_eq!(shown, vec![2, 3, 4], "newest last, nearest the bottom edge");
        assert_eq!(visible_toasts(toasts(2).into_iter()).count(), 2);
        assert_eq!(visible_toasts(toasts(0).into_iter()).count(), 0);

        let mut renderer = renderer();
        let mut ui = mounted(toasts(TOAST_MAX_VISIBLE + 4), &mut renderer);
        let mounted: Vec<String> = texts(&mut ui, &renderer)
            .into_iter()
            .map(|(text, _)| text)
            .filter(|text| text.starts_with("Toast number"))
            .collect();
        assert_eq!(mounted.len(), TOAST_MAX_VISIBLE, "{mounted:?}");
        assert!(!mounted.iter().any(|t| t == "Toast number 0"));
    }

    #[test]
    fn a_press_on_a_toast_does_not_reach_the_screen_below() {
        let mut renderer = renderer();
        let mut ui = mounted(toasts(1), &mut renderer);
        let message = texts(&mut ui, &renderer)
            .into_iter()
            .find(|(text, _)| text == "Toast number 0")
            .expect("mounted toast message")
            .1;

        let on_toast = click(&mut ui, &mut renderer, message.center());
        assert!(
            !reached_underlay(&on_toast),
            "a press on the toast reached the screen below"
        );

        // The dismiss cross is the glyph after the message. Inside `opaque` it
        // still gets the press first and dismisses its own toast.
        let cross = cross_of(&mut ui, &renderer, "Toast number 0");
        let on_cross = click(&mut ui, &mut renderer, cross.center());
        assert!(
            on_cross
                .iter()
                .any(|m| matches!(m, Message::DismissToast(0))),
            "the dismiss button must still dismiss its toast"
        );
        assert!(
            !reached_underlay(&on_cross),
            "a press on the dismiss button reached the screen below"
        );

        // The wheel over a toast doesn't scroll the screen below either.
        let wheel = mouse::Event::WheelScrolled {
            delta: mouse::ScrollDelta::Lines { x: 0.0, y: -3.0 },
        };
        let scrolled = send(
            &mut ui,
            &mut renderer,
            message.center(),
            &[
                mouse::Event::CursorMoved {
                    position: message.center(),
                },
                wheel,
            ],
        );
        assert!(
            !reached_underlay(&scrolled),
            "the wheel over a toast reached the screen below"
        );
        let beside_wheel = Point::new(WINDOW.width - 20.0, 20.0);
        let scrolled_beside = send(
            &mut ui,
            &mut renderer,
            beside_wheel,
            &[
                mouse::Event::CursorMoved {
                    position: beside_wheel,
                },
                wheel,
            ],
        );
        assert!(
            reached_underlay(&scrolled_beside),
            "the wheel beside the toasts must reach the screen below"
        );

        // The overlay outside the cards stays click-through.
        let beside = click(
            &mut ui,
            &mut renderer,
            Point::new(WINDOW.width - 20.0, 20.0),
        );
        assert!(
            reached_underlay(&beside),
            "a press beside the toasts must reach the screen below"
        );
    }
}

#[cfg(test)]
mod unavailable_panel_tests {
    use super::feature_unavailable_panel;
    use crate::app::{
        features::{self, MarketplaceServerFlags},
        menu::{Menu, SparkSubMenu},
        view::shared::landing::capture,
    };
    use coincube_core::miniscript::bitcoin::Network;
    use coincube_ui::theme;

    #[test]
    fn an_unsupported_network_gets_the_network_panel_in_both_themes() {
        let reason = features::route_availability(
            &Menu::Spark(SparkSubMenu::Overview),
            Network::Signet,
            false,
            MarketplaceServerFlags::OFF,
        )
        .reason()
        .expect("route is unavailable")
        .to_owned();
        assert_eq!(reason, "Spark isn't available on Signet.");
        for (appearance, palette) in [
            ("dark", theme::Theme::dark()),
            ("light", theme::Theme::light()),
        ] {
            for outer in [(1024, 768), (1440, 900)] {
                let mounted = capture::inspect(
                    "spark-network-unavailable",
                    feature_unavailable_panel(reason.clone()),
                    outer,
                    appearance,
                    palette,
                );
                assert!(mounted.contains("Not available on this network"));
                assert!(mounted.contains(&reason));
            }
        }
    }
}

#[cfg(test)]
mod page_tests {
    use super::{nav::split::sidebar_width_for_window, page_gutter};
    use coincube_ui::component::tenshu::gutter_for_window;

    #[test]
    fn page_gutter_is_the_window_clamp_seen_from_the_pane() {
        // 1440 window: 248px menu, 1192px pane, 43.2px gutter.
        assert_eq!(page_gutter(1192.0), gutter_for_window(1440.0));
        // 1024 minimum window: compact 208px menu.
        assert_eq!(page_gutter(816.0), gutter_for_window(1024.0));
        // From 1600 the gutter stops growing.
        assert_eq!(page_gutter(2560.0 - 248.0), 48.0);
    }

    #[test]
    fn every_window_width_gets_its_own_gutter_within_a_pixel_and_a_bit() {
        // The menu narrows at 1100, so windows 1060..=1100 (compact menu) and
        // 1100..=1140 (full menu) share pane widths; the gutter is then read
        // from the full-menu window, at most 40px * 3% = 1.2px off.
        for window in 1024..=2560 {
            let window = window as f32;
            let pane = window - sidebar_width_for_window(window);
            let off = (page_gutter(pane) - gutter_for_window(window)).abs();
            assert!(off <= 1.21, "window {}: off by {}", window, off);
        }
    }
}

#[cfg(test)]
mod page_head_tests {
    use super::{
        dashboard, dashboard_headed, page_head, spark_page_head, vault_page_head, wallet_page_head,
        Message,
    };
    use crate::app::cache::{Cache, WalletNames};
    use crate::app::menu::{CubeSettingsOption, CubeSubMenu, Menu, VaultSubMenu};
    use coincube_ui::component::tenshu::PageHead;
    use coincube_ui::widget::{Column, Element};
    use iced::advanced::widget::operation::{Focusable, Scrollable};
    use iced::advanced::widget::{Id, Operation, Tree};
    use iced::advanced::{layout, renderer::Headless, Layout};
    use iced::{mouse, Point, Rectangle, Size, Vector};

    const WINDOW: Size = Size::new(1024.0, 768.0);
    const TITLE: &str = "A long page";

    /// The text and scrollables of a laid-out page, as an [`Operation`] sees
    /// them. Also used by the P2P order details test.
    #[derive(Default)]
    pub(crate) struct Probe {
        text: Vec<(String, Rectangle)>,
        scrollables: Vec<(Rectangle, Vector)>,
    }

    impl Operation for Probe {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            operate(self);
        }

        fn text(&mut self, _id: Option<&Id>, bounds: Rectangle, value: &str) {
            self.text.push((value.to_owned(), bounds));
        }

        fn focusable(&mut self, _id: Option<&Id>, _bounds: Rectangle, _: &mut dyn Focusable) {}

        fn scrollable(
            &mut self,
            _id: Option<&Id>,
            bounds: Rectangle,
            _content_bounds: Rectangle,
            translation: Vector,
            _state: &mut dyn Scrollable,
        ) {
            self.scrollables.push((bounds, translation));
        }
    }

    impl Probe {
        pub(crate) fn text(&self, value: &str) -> Rectangle {
            self.text
                .iter()
                .find(|(text, _)| text == value)
                .unwrap_or_else(|| panic!("no {:?}", value))
                .1
        }

        /// The scrollable that holds `inner`, and how far it has scrolled.
        pub(crate) fn scroller_of(&self, inner: Rectangle) -> (Rectangle, Vector) {
            *self
                .scrollables
                .iter()
                .filter(|(bounds, _)| bounds.x <= inner.x && inner.x < bounds.x + bounds.width)
                .max_by(|(a, _), (b, _)| a.width.partial_cmp(&b.width).unwrap())
                .expect("a scrollable around the page body")
        }
    }

    pub(crate) fn renderer() -> iced::Renderer {
        for bytes in coincube_ui::font::load() {
            iced::advanced::graphics::text::font_system()
                .write()
                .unwrap()
                .load_font(bytes);
        }
        iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
            coincube_ui::font::REGULAR,
            iced::Pixels(14.0),
            Some("tiny-skia"),
        ))
        .unwrap()
    }

    fn long_page<'a>(menu: &'a Menu, cache: &'a Cache) -> Element<'a, Message> {
        let rows = (0..80).fold(Column::new().spacing(12), |column, row| {
            column.push(iced::widget::text(format!("Row {}", row)).size(14))
        });
        dashboard_headed(menu, cache, page_head(cache, TITLE), rows)
    }

    /// Lays the page out in a 1024×768 window as a fresh view, as every frame
    /// does, and probes it.
    pub(crate) fn frame(
        tree: &mut Tree,
        renderer: &iced::Renderer,
        element: &mut Element<'_, Message>,
    ) -> (layout::Node, Probe) {
        tree.diff(element.as_widget());
        let node = element.as_widget_mut().layout(
            tree,
            renderer,
            &layout::Limits::new(Size::ZERO, WINDOW),
        );
        let mut probe = Probe::default();
        element
            .as_widget_mut()
            .operate(tree, Layout::new(&node), renderer, &mut probe);
        (node, probe)
    }

    /// Turns the mouse wheel `dy` pixels down the page with the cursor at
    /// `at`, and returns the messages that publishes.
    pub(crate) fn wheel(
        tree: &mut Tree,
        renderer: &iced::Renderer,
        element: &mut Element<'_, Message>,
        node: &layout::Node,
        at: Point,
        dy: f32,
    ) -> Vec<Message> {
        let mut messages = Vec::new();
        element.as_widget_mut().update(
            tree,
            &iced::Event::Mouse(mouse::Event::WheelScrolled {
                delta: mouse::ScrollDelta::Pixels { x: 0.0, y: -dy },
            }),
            Layout::new(node),
            mouse::Cursor::Available(at),
            renderer,
            &mut iced::advanced::clipboard::Null,
            &mut iced::advanced::Shell::new(&mut messages),
            &Rectangle::with_size(WINDOW),
        );
        messages
    }

    #[test]
    fn the_head_stays_put_while_a_long_page_scrolls_under_it() {
        let renderer = renderer();
        let cache = Cache {
            cube_name: "Everyday".into(),
            ..Cache::default()
        };
        let menu = Menu::Cube(CubeSubMenu::Overview);
        let mut element = long_page(&menu, &cache);
        let mut tree = Tree::new(element.as_widget());
        let (node, before) = frame(&mut tree, &renderer, &mut element);
        let title = before.text(TITLE);
        let line = before.text("Everyday · Bitcoin mainnet");
        let first_row = before.text("Row 0");
        let (viewport, offset) = before.scroller_of(first_row);
        assert_eq!(offset, Vector::ZERO);
        assert!(
            viewport.y >= line.y + line.height,
            "only the body scrolls; the head sits above its viewport: {:?} {:?}",
            line,
            viewport
        );

        let messages = wheel(
            &mut tree,
            &renderer,
            &mut element,
            &node,
            first_row.center(),
            400.0,
        );
        let scrolled = messages
            .iter()
            .find_map(|message| match message {
                Message::Scroll(y) => Some(*y),
                _ => None,
            })
            .expect("the body reports its scroll");
        assert!(scrolled > 0.0, "{}", scrolled);

        // The next frame is a new view over the same tree.
        let mut element = long_page(&menu, &cache);
        let (_, after) = frame(&mut tree, &renderer, &mut element);
        assert_eq!(after.text(TITLE), title, "the title does not move");
        assert_eq!(
            after.text("Everyday · Bitcoin mainnet"),
            line,
            "the Cube line does not move"
        );
        let (_, offset) = after.scroller_of(after.text("Row 0"));
        assert_eq!(offset.y, scrolled, "the body moved by the reported offset");
        // Content draws at its layout position less the translation.
        let shown_first_row = after.text("Row 0").y - offset.y;
        assert!(
            shown_first_row < viewport.y,
            "the first row scrolled out under the head: {} vs {:?}",
            shown_first_row,
            viewport
        );
    }

    #[test]
    fn wallet_heads_name_the_wallet_as_the_side_panel_does() {
        let renderer = renderer();
        let menu = Menu::Cube(CubeSubMenu::Overview);
        let named = Cache {
            cube_name: "Everyday".into(),
            wallet_names: WalletNames {
                spark: Some("Daily spending".into()),
                vault: Some("Family savings".into()),
            },
            ..Cache::default()
        };
        // A blank name reads as no name, as in the side panel.
        let unnamed = Cache {
            cube_name: "Everyday".into(),
            wallet_names: WalletNames {
                spark: None,
                vault: Some("  ".into()),
            },
            ..Cache::default()
        };
        type Head = fn(&Cache) -> PageHead<'_, Message>;
        let cases: [(&Cache, Head, &str, &str); 6] = [
            // H-Send, H-Send-Vault, H-Receive: "‹w.name› · ‹Cube› · ‹network›".
            (
                &named,
                |cache| vault_page_head(cache, "Send"),
                "Send",
                "Vault · Family savings · Everyday · Bitcoin mainnet",
            ),
            (
                &named,
                |cache| spark_page_head(cache, "Receive"),
                "Receive",
                "Spark · Daily spending · Everyday · Bitcoin mainnet",
            ),
            (
                &unnamed,
                |cache| vault_page_head(cache, "Send"),
                "Send",
                "Vault · Everyday · Bitcoin mainnet",
            ),
            (
                &unnamed,
                |cache| spark_page_head(cache, "Receive"),
                "Receive",
                "Spark · Everyday · Bitcoin mainnet",
            ),
            // A wallet's own page is titled with its name.
            (
                &named,
                |cache| wallet_page_head(cache, "Vault", cache.wallet_names.vault.as_deref()),
                "Vault · Family savings",
                "Everyday · Bitcoin mainnet",
            ),
            (
                &named,
                |cache| wallet_page_head(cache, "Spark", cache.wallet_names.spark.as_deref()),
                "Spark · Daily spending",
                "Everyday · Bitcoin mainnet",
            ),
        ];
        for (cache, head, title, line) in cases {
            let mut element = dashboard_headed(&menu, cache, head(cache), Column::new());
            let mut tree = Tree::new(element.as_widget());
            let (_, probe) = frame(&mut tree, &renderer, &mut element);
            probe.text(title);
            probe.text(line);
        }
    }

    fn rows<'a>() -> Column<'a, Message> {
        (0..4).fold(Column::new(), |column, row| {
            column.push(iced::widget::text(format!("Row {}", row)))
        })
    }

    #[test]
    fn heads_cut_a_long_cube_name_as_the_side_panel_does() {
        let renderer = renderer();
        let named = |name: &str| Cache {
            cube_name: name.into(),
            ..Cache::default()
        };
        // Saved names have no length limit and may hold line breaks; the
        // head doesn't scroll, so it shows the side panel's 16-character cut.
        let long = named("A very long Cube name\nthat goes on and on, line after line");
        let short = named("Everyday");
        let overview = Menu::Cube(CubeSubMenu::Overview);
        let settings = Menu::Cube(CubeSubMenu::Settings(CubeSettingsOption::General));
        let tools = Menu::Vault(VaultSubMenu::Recovery);
        let send = Menu::Vault(VaultSubMenu::Send);
        type Page = for<'a> fn(&'a Menu, &'a Cache) -> Element<'a, Message>;
        let cases: [(&Menu, Page, &str); 4] = [
            (
                &overview,
                |menu, cache| dashboard_headed(menu, cache, page_head(cache, "Overview"), rows()),
                "A very long Cube… · Bitcoin mainnet",
            ),
            (
                &settings,
                |menu, cache| dashboard(menu, cache, rows()),
                "Cube · A very long Cube… · Bitcoin mainnet",
            ),
            (
                &tools,
                |menu, cache| dashboard(menu, cache, rows()),
                "A very long Cube… · Bitcoin mainnet",
            ),
            (
                &send,
                |menu, cache| dashboard_headed(menu, cache, vault_page_head(cache, "Send"), rows()),
                "Vault · A very long Cube… · Bitcoin mainnet",
            ),
        ];
        for (menu, page, line) in cases {
            let viewport = |cache: &Cache, line: Option<&str>| {
                let mut element = page(menu, cache);
                let mut tree = Tree::new(element.as_widget());
                let (_, probe) = frame(&mut tree, &renderer, &mut element);
                if let Some(line) = line {
                    probe.text(line);
                }
                probe.scroller_of(probe.text("Row 0")).0
            };
            // The head is as tall as it is with a short name.
            assert_eq!(
                viewport(&long, Some(line)).y,
                viewport(&short, None).y,
                "{:?}",
                menu
            );
        }
    }

    #[test]
    fn spanish_dashboard_detail_survives_deferred_layout() {
        use coincube_ui::locale::{language, render_in, Language, Text};
        let renderer = renderer();
        let cache = Cache::default();
        let menu = Menu::Vault(VaultSubMenu::Overview);
        let mut element = render_in(Language::Spanish, || {
            super::dashboard_with_detail(
                &menu,
                &cache,
                || vault_page_head(&cache, "Vault"),
                || iced::widget::text(Text::Review.get()).into(),
                || Some(iced::widget::text(Text::TransactionReplacement.get()).into()),
            )
        });
        assert_eq!(language(), Language::English);
        let mut tree = Tree::new(element.as_widget());
        let (_, probe) = frame(&mut tree, &renderer, &mut element);
        probe.text("Revisión");
        probe.text("Reemplazo de transacción");
        assert_eq!(language(), Language::English);
    }

    #[test]
    fn a_vault_tool_step_keeps_back_to_vault_under_its_own_title() {
        let renderer = renderer();
        let cache = Cache::default();
        // Recovery's compose and review steps pass their own head on the
        // Recovery tool route; the Tools head's Back to Vault stays.
        for (menu, back) in [
            (Menu::Vault(VaultSubMenu::Recovery), true),
            (Menu::Vault(VaultSubMenu::Send), false),
        ] {
            let mut element =
                dashboard_headed(&menu, &cache, vault_page_head(&cache, "Recovery"), rows());
            let mut tree = Tree::new(element.as_widget());
            let (_, probe) = frame(&mut tree, &renderer, &mut element);
            probe.text("Recovery");
            assert_eq!(
                probe.text.iter().any(|(text, _)| text == "Back to Vault"),
                back,
                "{:?}",
                menu
            );
        }
    }
}
