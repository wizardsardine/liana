use iced::{
    widget::{column, row, rule, stack, Space},
    Alignment, Length, Padding,
};
use std::fmt::Display;

use bitcoin::Network;

use crate::{
    component, icon, image,
    spacing::{HSpacing, VSpacing},
    theme,
    widget::*,
    Variant,
};

use super::{
    button::{btn_breadcrumb_previous, btn_share_xpubs},
    list, pick_list, pill, scrollable, text,
};

const USERBAR_HEIGHT: u32 = 44;
const NAV_LEFT_SLOT_WIDTH: u32 = 200;
const MAX_SCROLL_HEIGHT: u32 = 500;
const HEADER_HEIGHT: u32 = 88;
const CONTENT_TOP_SPACING: u32 = 72;
const SCREEN_INTRO_SUB_WIDTH: u32 = 620;

pub enum LayoutContent<'a, M> {
    Scrollable(Element<'a, M>),
    ScrollableList {
        header: Option<Element<'a, M>>,
        list: Element<'a, M>,
        pinned: Option<Element<'a, M>>,
        footer: Option<Element<'a, M>>,
    },
}

pub struct LayoutConfig<'a, M> {
    pub variant: Variant,
    pub network: Network,
    pub email: Option<&'a str>,
    pub is_ws_admin: bool,
    pub nav_bar: NavBar<'a, M>,
    pub content_width: f32,
}

pub enum NavBar<'a, M> {
    Steps {
        progress: (usize, usize),
        breadcrumb: Vec<String>,
        previous_message: Option<M>,
    },
    StepTitle {
        progress: (usize, usize),
        title: &'a str,
        previous_message: Option<M>,
    },
    Launcher {
        previous_message: Option<M>,
        share_xpubs_message: Option<M>,
        networks: &'a [Network],
        selected_network: Network,
        on_network_selected: fn(Network) -> M,
    },
}

pub fn screen_intro<'a, M: 'a>(
    title: impl Display + 'a,
    sub: Option<Element<'a, M>>,
    extra_spacing: bool,
) -> Element<'a, M> {
    let spacing = if extra_spacing { 60 } else { 15 };
    column![text::new::d3(title), sub]
        .align_x(Alignment::Center)
        .spacing(spacing)
        .into()
}

pub fn intro_description<'a, M: 'a>(value: &'a str) -> Element<'a, M> {
    Container::new(text::new::caption(value).style(theme::text::secondary))
        .width(Length::Shrink)
        .max_width(SCREEN_INTRO_SUB_WIDTH)
        .align_x(Alignment::Center)
        .into()
}

pub fn intro_prompt<'a, M: 'a>(prompt: &'a str, accent: Option<&'a str>) -> Element<'a, M> {
    let accent = accent.map(|accent| text::new::h3_semi(accent).style(theme::text::accent));
    Container::new(row![text::new::h3_semi(prompt), accent].wrap())
        .center_x(Length::Fill)
        .into()
}

fn breadcrumb_header<'a, M: 'a>(segments: &[String]) -> Element<'a, M> {
    let mut row = row![].spacing(HSpacing::M).align_y(Alignment::Center);

    for (i, segment) in segments.iter().enumerate() {
        if i > 0 {
            row = row.push(list::breadcrumb_chevron());
        }
        row = row.push(if i + 1 == segments.len() {
            text::new::h3_semi(segment.clone()).style(theme::text::primary)
        } else {
            text::new::h3(segment.clone()).style(theme::text::muted)
        });
    }

    row.wrap().into()
}

fn thin_separator<'a, M: 'a>() -> Container<'a, M> {
    Container::new(rule::horizontal(1).style(|t: &theme::Theme| {
        if t.is_business() {
            theme::rule::separator(t)
        } else {
            theme::rule::transparent(t)
        }
    }))
}

fn identity_bar<'a, M: 'static + 'a + Clone>(
    variant: Variant,
    network: Network,
    is_ws_admin: bool,
    email: Option<&'a str>,
) -> Element<'a, M> {
    const IDENTITY_BAR_HEIGHT: u32 = 28;

    let identity_theme = if network == Network::Bitcoin {
        theme::container::top_bar
    } else {
        theme::banner::network
    };

    let logo_width = match variant {
        Variant::Liana => 170,
        Variant::LianaBusiness => 200,
    };
    let logo = match variant {
        Variant::Liana => image::liana_wallet_logo(),
        Variant::LianaBusiness => image::liana_business_logo(),
    }
    .width(logo_width)
    .height(IDENTITY_BAR_HEIGHT);
    let logo: Element<'a, M> = if variant == Variant::Liana && network != Network::Bitcoin {
        Container::new(logo)
            .padding([0, 20])
            .height(USERBAR_HEIGHT)
            .align_y(Alignment::Center)
            .style(theme::container::top_bar)
            .into()
    } else {
        Container::new(logo)
            .padding([0, 20])
            .height(USERBAR_HEIGHT)
            .align_y(Alignment::Center)
            .into()
    };

    let ws_admin_pill = is_ws_admin.then_some(pill::ws_admin());
    let user = email.map(|e| {
        let mut icon = icon::person_icon().size(16).style(theme::text::tertiary);
        let e = component::text::short_email(e, 35);
        let mut mail = text::new::caption(e).style(theme::text::accent);
        if network != Network::Bitcoin {
            icon = icon.style(theme::text::network_banner);
            mail = mail.style(theme::text::network_banner);
        }
        row![icon, mail].spacing(HSpacing::ML)
    });
    let user = row![
        Space::fill_width(),
        ws_admin_pill,
        user,
        Space::with_width(20)
    ]
    .height(Length::Fill)
    .align_y(Alignment::Center);
    let identity_bar = row![logo, Space::fill_width()]
        .spacing(HSpacing::ML)
        .align_y(Alignment::Center)
        .height(USERBAR_HEIGHT)
        .width(Length::Fill);

    let content = if network == Network::Bitcoin {
        stack![identity_bar, user]
    } else {
        stack![
            identity_bar,
            Container::new(row![
                Space::with_width(logo_width + 30),
                Container::new(network_warning(network))
                    .style(identity_theme)
                    .padding(Padding {
                        top: 0.0,
                        right: 0.0,
                        bottom: 0.0,
                        left: 50.0,
                    })
                    .width(Length::Fill)
                    .center_y(Length::Fill),
            ]),
            user
        ]
    };

    Container::new(content)
        .style(theme::container::top_bar)
        .into()
}

fn network_warning<'a, M: 'a>(network: Network) -> Element<'a, M> {
    row![
        icon::warning_icon(),
        text::new::caption("THIS IS A "),
        text::new::b5_bold(match network {
            Network::Signet => "SIGNET WALLET",
            Network::Testnet => "TESTNET WALLET",
            Network::Testnet4 => "TESTNET4 WALLET",
            Network::Regtest => "REGTEST WALLET",
            Network::Bitcoin => unreachable!(),
            _ => "NON-MAINNET WALLET",
        }),
        text::new::caption(", COINS HAVE "),
        text::new::b5_bold("NO VALUE"),
    ]
    .align_y(Alignment::Center)
    .into()
}

fn step_dots<'a, M: 'a>((step, total): (usize, usize)) -> Element<'a, M> {
    let mut dots = row![].spacing(HSpacing::XS).align_y(Alignment::Center);
    for i in 0..total {
        let filled = i < step;
        let width = if i + 1 == step { 20.0 } else { 8.0 };
        dots = dots.push(
            Container::new(Space::new())
                .width(width)
                .height(8)
                .style(if filled {
                    theme::container::step_dot_filled
                } else {
                    theme::container::step_dot_track
                }),
        );
    }
    dots.push(Space::with_width(HSpacing::XS))
        .push(
            text::new::small_caption(format!("{step}/{total}"))
                .style(|theme: &theme::Theme| theme::text::custom(theme.colors.general.accent)),
        )
        .into()
}

fn step_nav<'a, M: Clone + 'a>(
    header: impl Into<Element<'a, M>>,
    msg: Option<M>,
    progress: (usize, usize),
) -> Element<'a, M> {
    let progress = if progress.1 > 0 {
        step_dots(progress)
    } else {
        row![].into()
    };

    row![
        previous_slot(msg),
        Container::new(header).width(Length::Fill),
        progress,
        Space::with_width(20),
    ]
    .align_y(Alignment::Center)
    .height(HEADER_HEIGHT)
    .into()
}

fn previous_slot<'a, M: Clone + 'a>(msg: Option<M>) -> Container<'a, M> {
    let content: Element<'a, M> = if msg.is_some() {
        btn_breadcrumb_previous(msg).into()
    } else {
        Space::new().into()
    };

    Container::new(content).center_x(NAV_LEFT_SLOT_WIDTH)
}

fn launcher_nav<'a, M: Clone + 'a>(
    previous_message: Option<M>,
    share_xpubs_message: Option<M>,
    networks: &'a [Network],
    selected_network: Network,
    on_network_selected: fn(Network) -> M,
) -> Element<'a, M> {
    let network_picker =
        pick_list::pick_list(networks, Some(selected_network), on_network_selected).padding(10);

    row![
        previous_slot(previous_message),
        Space::fill_width(),
        btn_share_xpubs(share_xpubs_message),
        network_picker,
        Space::with_width(20),
    ]
    .spacing(20)
    .align_y(Alignment::Center)
    .height(HEADER_HEIGHT)
    .into()
}

fn nav_bar<'a, M: Clone + 'a>(nav_bar: NavBar<'a, M>) -> Element<'a, M> {
    match nav_bar {
        NavBar::Steps {
            progress,
            breadcrumb,
            previous_message,
        } => step_nav(breadcrumb_header(&breadcrumb), previous_message, progress),
        NavBar::StepTitle {
            progress,
            title,
            previous_message,
        } => step_nav(text::new::h3_semi(title), previous_message, progress),
        NavBar::Launcher {
            previous_message,
            share_xpubs_message,
            networks,
            selected_network,
            on_network_selected,
        } => launcher_nav(
            previous_message,
            share_xpubs_message,
            networks,
            selected_network,
            on_network_selected,
        ),
    }
}

pub fn layout_inner<'a, M: 'static + Clone + 'a>(
    config: LayoutConfig<'a, M>,
    content: LayoutContent<'a, M>,
) -> Element<'a, M> {
    let identity_bar = identity_bar(
        config.variant,
        config.network,
        config.is_ws_admin,
        config.email,
    );
    let top = column![identity_bar, thin_separator(), nav_bar(config.nav_bar),].width(Length::Fill);

    let body: Element<'a, M> = match content {
        LayoutContent::Scrollable(inner) => {
            let inner = Container::new(inner)
                .width(Length::Fill)
                .max_width(config.content_width);
            let content_body = Container::new(column![
                Space::with_height(CONTENT_TOP_SPACING as f32),
                Container::new(inner).center_x(Length::Fill),
            ])
            .width(Length::Fill);

            scrollable::vertical_floating(content_body)
                .height(Length::Fill)
                .width(Length::Fill)
                .into()
        }
        LayoutContent::ScrollableList {
            header,
            list,
            pinned,
            footer,
        } => {
            let header = Container::new(header).center_x(Length::Fill);
            let list_body = Container::new(scrollable::vertical(list))
                .max_height(MAX_SCROLL_HEIGHT)
                .max_width(config.content_width);
            let footer = footer.map(|f| {
                column![
                    thin_separator(),
                    Space::with_height(VSpacing::S),
                    f,
                    Space::with_height(VSpacing::S),
                ]
            });

            let body = column![header, list_body, pinned, Space::fill_height(), footer,]
                .spacing(VSpacing::M)
                .max_width(config.content_width);

            Container::new(body)
                .center_x(Length::Fill)
                .height(Length::Fill)
                .into()
        }
    };

    let content = column![top, body].width(Length::Fill).height(Length::Fill);

    Container::new(content)
        .center_x(Length::Fill)
        .height(Length::Fill)
        .style(theme::container::background)
        .into()
}

pub fn layout<'a, M: 'static + Clone + 'a>(
    config: LayoutConfig<'a, M>,
    content: impl Into<Element<'a, M>>,
) -> Element<'a, M> {
    layout_inner(config, LayoutContent::Scrollable(content.into()))
}

pub fn layout_with_scrollable_list<'a, M: 'static + Clone + 'a>(
    config: LayoutConfig<'a, M>,
    header_content: Option<Element<'a, M>>,
    list_content: impl Into<Element<'a, M>>,
    pinned_content: Option<Element<'a, M>>,
    footer_content: Option<Element<'a, M>>,
) -> Element<'a, M> {
    layout_inner(
        config,
        LayoutContent::ScrollableList {
            header: header_content,
            list: list_content.into(),
            pinned: pinned_content,
            footer: footer_content,
        },
    )
}
