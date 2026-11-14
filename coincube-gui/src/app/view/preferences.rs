//! Device-wide App Preferences sheet. The GUI supplies the last accepted
//! snapshot, observed platform inputs, and real diagnostic capabilities.
//! This leaf emits typed intents; it never persists or changes a preference.
//!
//! Geometry follows the App preferences artboard
//! (`design-review/canvas/P-AppPrefs.dc.html`): the 1040px Connect sheet
//! surface, a header over a rule, a 232px section list with its own rule, and
//! content inset 28/40px with a 700px reading width. Every row carries the
//! "This device" tag in its own trailing column.

use coincube_ui::locale::copy as localized_copy;
use coincube_ui::{
    component::tenshu::{
        segment, segmented, setting_row, text as tenshu_text, toggle, ActionButton, ActionKind,
        SegmentSize, Tone,
    },
    font,
    icon::*,
    theme,
    widget::{action::Action as FocusableAction, Column, Element, Row},
};
use iced::{advanced::widget::Id, widget::container, Alignment, Length};

use crate::app::{
    settings::preferences::{
        ArtworkStyle, DevicePreferences, EffectivePreferences, Language, LauncherStyle,
        MotionPreference, NetworkActivitySource, PreferenceChange, ThemePreference, WorldviewMode,
        Zoom,
    },
    state::preferences::{BalancePrivacy, PreferencesPageState, Section},
};
use coincube_ui::locale::Text as CopyText;
use coincube_ui::theme::palette::ThemeMode;

fn language_row<'a>(selected: Language, enabled: bool) -> Element<'a, Message> {
    setting_row(
        CopyText::Language.get(),
        segmented(
            [
                segment(
                    "tenshu.preferences.language.english",
                    Language::English,
                    "English",
                ),
                segment(
                    "tenshu.preferences.language.spanish",
                    Language::Spanish,
                    "Español",
                ),
            ],
            Some(selected),
            Message::FocusAction,
        )
        .size(SegmentSize::Compact)
        .on_select_maybe(
            enabled.then_some(|language| Message::Change(PreferenceChange::Language(language))),
        ),
    )
    .tag(CopyText::ThisDevice.get())
    .trailing_tag()
    .description(CopyText::LanguageDescription.get())
    .into()
}
/// Bitcoin Worldview spans the reading width instead of a two-column row:
/// the label, a choice per mode with its tagline, then what the selected
/// mode changes (network names and example wording), so the choice is made
/// on facts rather than on a tagline alone.
fn worldview_row<'a>(saved: DevicePreferences, enabled: bool) -> Element<'a, Message> {
    use coincube_ui::theme::tenshu::{control, space, type_size};
    use iced::widget::Space;

    let selected = saved.worldview;
    let muted = |content: String, size: f32| {
        tenshu_text::body(content)
            .size(size)
            .style(theme::text::secondary)
    };
    let tag = container(tenshu_text::caption(CopyText::ThisDevice.get()))
        .padding([0.0, 6.0])
        .style(control::tag);
    let heading = Row::new()
        .spacing(24)
        .align_y(Alignment::Start)
        .push(
            Column::new()
                .spacing(2)
                .width(Length::Fill)
                .push(tenshu_text::label(CopyText::BitcoinWorldview.get()))
                .push(muted(
                    CopyText::WorldviewDescription.get().to_owned(),
                    type_size::CHIP,
                )),
        )
        .push(
            container(tag)
                .width(Length::Fixed(TAG_COLUMN))
                .align_x(iced::alignment::Horizontal::Right),
        );

    let modes = [
        (
            WorldviewMode::Samurai,
            "Samurai",
            CopyText::TheWayOfTheBitcoin.get(),
            "tenshu.preferences.worldview.samurai",
        ),
        (
            WorldviewMode::Suitcoiner,
            "Suitcoiner",
            CopyText::TheMarketHasSpoken.get(),
            "tenshu.preferences.worldview.suitcoiner",
        ),
        (
            WorldviewMode::Luke,
            "Luke",
            CopyText::NoSpamShallPass.get(),
            "tenshu.preferences.worldview.luke",
        ),
    ];
    let mode_name = modes
        .iter()
        .find(|(mode, ..)| *mode == selected)
        .map_or("Samurai", |(_, name, ..)| *name);
    let choices = IntoIterator::into_iter(modes).fold(
        Row::new().spacing(space::S),
        |row, (mode, name, tagline, id)| {
            let is_selected = mode == selected;
            let title = Row::new()
                .spacing(space::S)
                .align_y(Alignment::Center)
                .push(
                    tenshu_text::label(name)
                        .font(font::SEMIBOLD)
                        .width(Length::Fill),
                )
                .push(is_selected.then(|| {
                    check_circle_icon()
                        .size(16)
                        .style(theme::tenshu::text::accent)
                }));
            let tagline = tenshu_text::body(tagline)
                .size(type_size::CHIP)
                .style(if is_selected {
                    theme::text::primary
                } else {
                    theme::text::secondary
                });
            row.push(
                FocusableAction::new(
                    id,
                    Column::new().spacing(2).push(title).push(tagline),
                    Message::FocusAction,
                )
                .width(Length::Fill)
                .padding([10, 14])
                .style(move |theme, status| control::choice_chip(theme, status, is_selected))
                .on_press_maybe(
                    enabled.then_some(Message::Change(PreferenceChange::Worldview(mode))),
                ),
            )
        },
    );

    let section_title = |content: String| {
        tenshu_text::body(content)
            .size(type_size::CHIP)
            .font(font::SEMIBOLD)
            .style(theme::text::secondary)
    };

    // Names come from the same registry the passive labels use; the
    // BLAKE2b row appears only where that network is switched on.
    let mut chains = vec![crate::chain::ChainId::Bitcoin];
    if saved.bitcoin_blake2b_beta {
        chains.push(crate::chain::ChainId::BitcoinBlake2b);
    }
    let names = chains.into_iter().enumerate().fold(
        Column::new()
            .spacing(space::M)
            .push(section_title(coincube_ui::localized_format!(
                "Network names in {mode_name}",
                "Nombres de red en {mode_name}"
            ))),
        |column, (index, chain)| {
            let display = crate::chain::worldview_chain_label(chain, selected);
            let unchanged =
                display == crate::chain::worldview_chain_label(chain, WorldviewMode::Samurai);
            let name = Row::new()
                .spacing(space::S)
                .align_y(Alignment::Center)
                .push(
                    tenshu_text::body(display)
                        .size(17)
                        .font(font::SEMIBOLD)
                        .style(if unchanged {
                            theme::text::primary
                        } else {
                            theme::tenshu::text::accent
                        }),
                )
                .push(unchanged.then(|| {
                    tenshu_text::caption(coincube_ui::localized_format!("Unchanged", "Sin cambios"))
                }));
            column.push((index > 0).then(horizontal_rule)).push(
                Column::new()
                    .spacing(2)
                    .push(tenshu_text::caption(crate::chain::canonical_network_name(
                        chain,
                    )))
                    .push(name),
            )
        },
    );

    use coincube_ui::worldview_copy::VoiceLine;
    let profile = VoiceLine::Profile.for_mode(selected).unwrap_or_default();
    let mut wording = Column::new()
        .spacing(space::XS)
        .push(section_title(coincube_ui::localized_format!(
            "Example wording",
            "Texto de ejemplo"
        )))
        .push(Space::new().height(space::XS))
        .push(tenshu_text::body(profile).size(15).font(font::MEDIUM))
        .push(
            VoiceLine::Consensus
                .for_mode(selected)
                .map(|line| muted(line.to_owned(), type_size::CHIP)),
        );
    let line = |voice: VoiceLine| voice.for_mode(selected).unwrap_or_default().to_owned();
    let examples: [(String, String); 4] = [
        (
            coincube_ui::localized_format!("Welcome", "Bienvenida"),
            line(VoiceLine::Welcome),
        ),
        (
            coincube_ui::localized_format!("Empty Vault", "Vault vacío"),
            line(VoiceLine::EmptySha256Vault),
        ),
        (
            coincube_ui::localized_format!("Payment sent", "Pago enviado"),
            line(VoiceLine::PaymentSent),
        ),
        (
            localized_copy("Syncing").to_owned(),
            line(VoiceLine::Syncing),
        ),
    ];
    wording = wording
        .push(Space::new().height(space::S))
        .push(horizontal_rule());
    let wording = IntoIterator::into_iter(examples).fold(
        wording.push(Space::new().height(space::XS)),
        |column, (place, text)| {
            column.push(
                Row::new()
                    .spacing(space::M)
                    .push(muted(place, type_size::CHIP).width(Length::Fixed(112.0)))
                    .push(
                        tenshu_text::body(text)
                            .size(type_size::CHIP)
                            .width(Length::Fill)
                            .style(theme::text::primary),
                    ),
            )
        },
    );

    let panel = container(
        Row::new()
            .push(
                container(names)
                    .padding([16, 18])
                    .width(Length::FillPortion(5)),
            )
            .push(vertical_rule())
            .push(
                container(wording)
                    .padding([16, 18])
                    .width(Length::FillPortion(6)),
            )
            .height(Length::Shrink),
    )
    .width(Length::Fill)
    .style(worldview_panel_style);

    let footnote = Row::new()
        .spacing(space::S)
        .push(tooltip_icon().size(14).style(theme::text::secondary))
        .push(tenshu_text::caption(coincube_ui::localized_format!(
            "Signing, addresses, fees, errors and recovery steps always use plain wording and the technical network name, such as {name}.",
            "La firma, las direcciones, las comisiones, los errores y los pasos de recuperación siempre usan un lenguaje sencillo y el nombre técnico de la red, como {name}.",
            name = crate::chain::canonical_network_name(crate::chain::ChainId::Bitcoin)
        )));

    Column::new()
        .width(Length::Fill)
        .push(
            Column::new()
                .spacing(space::M)
                .padding([18.0, 0.0])
                .push(heading)
                .push(choices)
                .push(panel)
                .push(footnote),
        )
        .push(horizontal_rule())
        .into()
}

/// The inset well under the Worldview choices: rail fill, 1px line, 10px radius.
fn worldview_panel_style(theme: &theme::Theme) -> container::Style {
    let shell = &theme.colors.shell;
    container::Style {
        background: Some(iced::Background::Color(shell.rail)),
        border: iced::Border {
            color: shell.line,
            width: 1.0,
            radius: coincube_ui::theme::tenshu::radius::MENU.into(),
        },
        ..container::Style::default()
    }
}

fn artwork_row<'a>(saved: DevicePreferences, enabled: bool) -> Element<'a, Message> {
    use coincube_ui::{illustration, theme::tenshu::control};

    let mode = saved.worldview;
    let (mode_name, existing_name, scene_name) = match mode {
        WorldviewMode::Samurai => ("Samurai", "Kage", "Mountain Watch"),
        WorldviewMode::Suitcoiner => ("Suitcoiner", "Gilded Wall Street", "Gilded Exchange"),
        WorldviewMode::Luke => ("Luke", "Purist’s Woodcut", "The Stone Crossing"),
    };
    let mut options = [
        (
            ArtworkStyle::Existing,
            existing_name,
            localized_copy("Existing set"),
            "tenshu.preferences.artwork.existing",
        ),
        (
            ArtworkStyle::Scene,
            scene_name,
            localized_copy("Full scenes"),
            "tenshu.preferences.artwork.scene",
        ),
    ];
    if ArtworkStyle::default_for(mode) == ArtworkStyle::Scene {
        options.swap(0, 1);
    }
    let choices = IntoIterator::into_iter(options).fold(
        Row::new().spacing(8),
        |row, (style, name, label, id)| {
            let selected = saved.artwork.get(mode) == style;
            let thumbnail = illustration::artwork_thumbnail(mode, style).map(|image| {
                iced::widget::image(image)
                    .width(Length::Fixed(88.0))
                    .height(Length::Fixed(88.0))
                    .content_fit(iced::ContentFit::Contain)
            });
            let caption = Row::new()
                .spacing(4)
                .align_y(Alignment::Center)
                .push(selected.then(|| check_circle_icon().size(12)))
                .push(tenshu_text::caption(name).font(font::MEDIUM));
            let content = Column::new()
                .spacing(4)
                .align_x(Alignment::Center)
                .width(Length::Fill)
                .push(thumbnail)
                .push(caption)
                .push(tenshu_text::caption(label).style(theme::text::secondary));
            row.push(
                FocusableAction::new(id, content, Message::FocusAction)
                    .width(Length::Fixed(139.0))
                    .padding([7, 5])
                    .style(move |theme, status| control::choice_chip(theme, status, selected))
                    .on_press_maybe(
                        enabled.then_some(Message::Change(PreferenceChange::Artwork(mode, style))),
                    ),
            )
        },
    );
    setting_row(localized_copy("Artwork"), choices)
        .tag(CopyText::ThisDevice.get())
        .trailing_tag()
        .description(coincube_ui::localized_format!(
            "Choose the artwork set for {mode_name}.",
            "Elige la colección de ilustraciones de {mode_name}."
        ))
        .note(
            localized_copy("Each worldview remembers its own choice."),
            Tone::Muted,
        )
        .into()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Close,
    RetryLoad,
    RefreshWorldview,
    PublishWorldview,
    RevokeWorldview,
    OpenLogs,
    CopySupportSummary,
    RepairRegistration,
    OpenDownloadStats,
    OpenLicenses,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    FocusAction(Id),
    SelectSection(Section),
    Change(PreferenceChange),
    ToggleComponentVersions,
    ToggleRegistrationRepair,
    Action(Action),
}

/// Values are populated from real owners. Missing fields render as unavailable;
/// neither a stream connection nor a repair result is inferred by this view.
#[derive(Debug, Default, Clone, Copy)]
pub struct Diagnostics<'a> {
    pub stream_status: Option<&'a str>,
    pub registration_status: Option<&'a str>,
    pub can_repair_registration: bool,
    pub can_open_logs: bool,
    /// Already sanitized by the caller's explicit support-summary allowlist;
    /// displayed for inspection before the user chooses Copy.
    pub support_summary_preview: Option<&'a str>,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct About<'a> {
    pub build_provenance: Option<&'a str>,
    pub daemon_version: Option<&'a str>,
    pub spark_bridge_version: Option<&'a str>,
    pub can_open_download_stats: bool,
    pub can_open_licenses: bool,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct WorldviewSharing {
    pub status: &'static str,
    pub shared_selection: Option<(&'static str, &'static str)>,
    pub can_refresh: bool,
    pub can_publish: bool,
    pub can_revoke: bool,
}

fn worldview_sharing_row<'a>(sharing: &WorldviewSharing) -> Element<'a, Message> {
    let actions = [
        (
            "refresh",
            localized_copy("Refresh"),
            Action::RefreshWorldview,
            sharing.can_refresh,
        ),
        (
            "publish",
            localized_copy("Share saved selection"),
            Action::PublishWorldview,
            sharing.can_publish,
        ),
        (
            "revoke",
            localized_copy("Stop sharing"),
            Action::RevokeWorldview,
            sharing.can_revoke,
        ),
    ];
    let controls = IntoIterator::into_iter(actions).fold(
        Row::new().spacing(8),
        |row, (id, label, action, enabled)| {
            row.push(
                ActionButton::new(
                    format!("tenshu.preferences.worldview-sharing.{id}"),
                    label,
                    ActionKind::Secondary,
                    Message::FocusAction,
                )
                .compact()
                .on_press_maybe(enabled.then_some(Message::Action(action))),
            )
        },
    );
    let status = if let Some((mode, art)) = sharing.shared_selection {
        format!("{} · {mode} · {art}", sharing.status)
    } else {
        sharing.status.to_owned()
    };
    let content = Column::new()
        .spacing(8)
        .push(tenshu_text::label(localized_copy("Share with Keychain")))
        .push(
            tenshu_text::body(localized_copy("Once sharing is enabled, saved Worldview and artwork changes automatically update Keychain on your Connect account."))
                .size(13)
                .style(theme::tenshu::text::muted),
        )
        .push(tenshu_text::caption(status).style(theme::tenshu::text::muted))
        .push(controls)
        .width(Length::Fill);
    Column::new()
        .push(container(content).padding([14, 0]))
        .push(horizontal_rule())
        .width(Length::Fill)
        .into()
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ViewModel<'a> {
    /// None means no accepted load yet. Controls stay inert and balances must
    /// remain suppressed by the GUI until an accepted snapshot is available.
    pub saved: Option<DevicePreferences>,
    pub effective: Option<EffectivePreferences>,
    pub system_theme: Option<ThemeMode>,
    pub system_reduces_motion: Option<bool>,
    pub cubes_renderer_available: bool,
    pub saving: bool,
    /// Effective session privacy from the GUI owner. Never infer it from
    /// `saved.hide_balances`: a local Hide can mask before its save succeeds.
    pub balance_privacy: BalancePrivacy,
    /// Mirrors `BalancePrivacyChrome::interactive`: explicit saves disable
    /// controls; a periodic read keeps them usable and queues a click.
    pub balance_privacy_interactive: bool,
    /// The last save committed but its durability could not be confirmed.
    /// It is applied, not rolled back; the notice is neutral.
    pub durability_unconfirmed: bool,
    /// True while an initial load or explicit retry is in flight.
    pub loading: bool,
    pub load_error: Option<&'a str>,
    pub save_error: Option<&'a str>,
    pub worldview_changed: Option<WorldviewMode>,
    pub worldview_sharing: WorldviewSharing,
    pub diagnostics: Diagnostics<'a>,
    pub about: About<'a>,
    /// The largest zoom this window's screen holds; None until it is known.
    pub max_zoom: Option<Zoom>,
}

pub const CONTENT_SCROLL_ID: &str = "tenshu.preferences.content-scroll";

/// Where every App preferences setting is stored (`.scope`).
/// Width of the trailing tag column (`.row` third column).
const TAG_COLUMN: f32 = 92.0;
/// Reading width of a section (`.sec`).
const READING_WIDTH: f32 = 700.0;

pub fn view<'a>(page: &'a PreferencesPageState, model: ViewModel<'a>) -> Element<'a, Message> {
    // `.shead`: 24px title over a 13px muted line, the 36px Close on the right.
    let header = container(
        Row::new()
            .push(
                Column::new()
                    .push(
                        iced::widget::text(CopyText::AppPreferences.get())
                            .size(24)
                            .line_height(iced::widget::text::LineHeight::Relative(1.25))
                            .font(font::SEMIBOLD)
                            .style(theme::tenshu::text::ink),
                    )
                    .push(
                        tenshu_text::body(CopyText::PreferencesScopeDescription.get())
                            .size(13)
                            .style(theme::tenshu::text::muted),
                    )
                    .spacing(2)
                    .width(Length::Fill),
            )
            .push(
                FocusableAction::new(
                    "tenshu.preferences.close",
                    centred(
                        Row::new()
                            .push(cross_icon().size(14).style(theme::tenshu::text::ink))
                            .push(tenshu_text::label(CopyText::Close.get()))
                            .spacing(6)
                            .align_y(Alignment::Center),
                    ),
                    Message::FocusAction,
                )
                .height(Length::Fixed(36.0))
                .padding(iced::Padding {
                    left: 10.0,
                    right: 12.0,
                    ..iced::Padding::ZERO
                })
                .style(|theme, status| {
                    theme::tenshu::button::action(
                        theme,
                        status,
                        ActionKind::Secondary,
                        theme::tenshu::button::ActionShape::Compact,
                    )
                })
                .on_press(Message::Action(Action::Close)),
            )
            .spacing(16)
            .align_y(Alignment::Center),
    )
    .padding(iced::Padding {
        top: 18.0,
        right: 24.0,
        bottom: 18.0,
        left: 28.0,
    })
    .width(Length::Fill);

    // `.nav`: the six sections, then the note pinned to the bottom.
    let sections = Section::ALL.map(|section| {
        let icon = match section {
            Section::Appearance => circle_half_icon(),
            Section::Launcher => cube_outline_icon(),
            Section::BalancesAndMotion => eye_outline_icon(),
            Section::Developer => code_slash_icon(),
            Section::Diagnostics => activity_icon(),
            Section::About => tooltip_icon(),
        };
        segment(section.action_id(), section, section.label()).icon(icon)
    });
    let navigation = Column::new()
        .push(
            coincube_ui::component::tenshu::nav_list(
                sections,
                Some(page.section),
                Message::FocusAction,
            )
            .on_select(Message::SelectSection),
        )
        .push(iced::widget::Space::new().height(Length::Fill))
        .push(
            container(tenshu_text::caption(CopyText::PreferencesScopeNote.get())).padding([0, 12]),
        )
        .height(Length::Fill);

    let body = Row::new()
        .push(
            container(navigation)
                .padding([16, 12])
                .width(Length::Fixed(232.0))
                .height(Length::Fill),
        )
        .push(vertical_rule())
        .push(
            iced::widget::scrollable(
                container(container(section_content(page, model)).max_width(READING_WIDTH))
                    .padding(iced::Padding {
                        top: 28.0,
                        right: 40.0,
                        bottom: 40.0,
                        left: 40.0,
                    })
                    .width(Length::Fill),
            )
            .id(CONTENT_SCROLL_ID)
            .width(Length::Fill)
            .height(Length::Fill),
        )
        .height(Length::Fill);

    container(
        Column::new()
            .push(header)
            .push(horizontal_rule())
            .push(body)
            .height(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .clip(true)
    .style(sheet_style)
    .into()
}

fn section_content<'a>(
    page: &'a PreferencesPageState,
    model: ViewModel<'a>,
) -> Element<'a, Message> {
    let mut content = Column::new()
        .push(
            container(tenshu_text::heading(page.section.label()))
                .padding(iced::Padding::default().bottom(16)),
        )
        .width(Length::Fill);
    let mut notices: Vec<(String, Tone)> = Vec::new();
    let mut notice = |line: String, tone: Tone| notices.push((line, tone));
    if let Some(error) = model.load_error {
        notice(
            coincube_ui::localized_format!(
                "Device preferences could not be loaded: {error}",
                "No se pudieron cargar las preferencias del dispositivo: {error}"
            ),
            Tone::Bad,
        );
    }
    if let Some(error) = model.save_error {
        let status = if model.balance_privacy == BalancePrivacy::HiddenLocally {
            CopyText::CouldNotSaveAmountsRemainHidden.get()
        } else {
            CopyText::CouldNotSaveYourPreviousSettings.get()
        };
        notice(format!("{status} {error}"), Tone::Bad);
    }
    if model.saving {
        notice(
            CopyText::SavingDevicePreferences.get().to_string(),
            Tone::Muted,
        );
    }
    if model.durability_unconfirmed {
        notice(
            CopyText::SavedDurabilityCouldNotBeConfirmed
                .get()
                .to_string(),
            Tone::Muted,
        );
    }
    if page.section == Section::Appearance {
        if let Some(line) = model
            .worldview_changed
            .filter(|mode| model.saved.is_some_and(|saved| saved.worldview == *mode))
            .and_then(|mode| coincube_ui::worldview_copy::VoiceLine::ModeChanged.for_mode(mode))
        {
            notice(line.to_owned(), Tone::Muted);
        }
    }
    let idle = !model.saving && !model.loading;
    let Some(saved) = model.saved else {
        let status = if model.load_error.is_some() {
            localized_copy(
                "Device preferences are unavailable. Retry the load before changing settings. \
             Amounts stay hidden until you choose.",
            )
        } else {
            CopyText::LoadingDevicePreferencesAmountsRemainHidden.get()
        };
        notice(status.to_string(), Tone::Muted);
        content = content.push(notice_column(notices));
        if model.load_error.is_some() {
            content = content.push(
                Column::new()
                    .push(
                        ActionButton::new(
                            "tenshu.preferences.retry-load",
                            CopyText::RetryLoad.get(),
                            ActionKind::Primary,
                            Message::FocusAction,
                        )
                        .compact()
                        .on_press_maybe(idle.then_some(Message::Action(Action::RetryLoad))),
                    )
                    .push(explicit_privacy_choice(
                        idle && model.balance_privacy_interactive,
                    ))
                    .spacing(12)
                    .padding(iced::Padding::default().top(12)),
            );
        }
        return content.into();
    };
    if !notices.is_empty() {
        content = content
            .push(container(notice_column(notices)).padding(iced::Padding::default().bottom(16)));
    }
    let enabled = idle;
    let mut rows: Vec<Element<'a, Message>> = Vec::new();
    match page.section {
        Section::Appearance => {
            let current = match model.system_theme {
                Some(ThemeMode::Light) => CopyText::SystemIsCurrentlyLight.get(),
                Some(ThemeMode::Dark) => CopyText::SystemIsCurrentlyDark.get(),
                None => CopyText::SystemThemeFallback.get(),
            };
            rows.push(
                setting_row(
                    CopyText::Theme.get(),
                    segmented(
                        [
                            segment(
                                "tenshu.preferences.theme.system",
                                ThemePreference::System,
                                CopyText::System.get(),
                            )
                            .icon(display_icon()),
                            segment(
                                "tenshu.preferences.theme.light",
                                ThemePreference::Light,
                                CopyText::Light.get(),
                            )
                            .icon(sun_icon()),
                            segment(
                                "tenshu.preferences.theme.dark",
                                ThemePreference::Dark,
                                CopyText::Dark.get(),
                            )
                            .icon(moon_icon()),
                        ],
                        Some(saved.theme),
                        Message::FocusAction,
                    )
                    .size(SegmentSize::Compact)
                    .on_select_maybe(enabled.then_some(
                        |value: ThemePreference| Message::Change(PreferenceChange::Theme(value)),
                    )),
                )
                .tag(CopyText::ThisDevice.get())
                .trailing_tag()
                .description(CopyText::ColorsForTheLauncherAndEvery.get())
                .note(current, Tone::Muted)
                .into(),
            );
            rows.push(language_row(saved.language, enabled));
            rows.push(worldview_row(saved, enabled));
            rows.push(artwork_row(saved, enabled));
            rows.push(worldview_sharing_row(&model.worldview_sharing));
            let max_zoom = model.max_zoom.unwrap_or(Zoom::LARGEST);
            let (zoom_selected, zoom_note) = zoom_choice(saved.zoom, max_zoom);
            rows.push(
                setting_row(
                    CopyText::Zoom.get(),
                    segmented(
                        Zoom::STEPS.map(|zoom| {
                            segment(
                                format!("tenshu.preferences.zoom.{}", zoom.percent()),
                                zoom,
                                format!("{}%", zoom.percent()),
                            )
                            .enabled(zoom <= max_zoom)
                        }),
                        Some(zoom_selected),
                        Message::FocusAction,
                    )
                    .size(SegmentSize::Compact)
                    .on_select_maybe(
                        enabled.then_some(|value: Zoom| {
                            Message::Change(PreferenceChange::Zoom(value))
                        }),
                    ),
                )
                .tag(CopyText::ThisDevice.get())
                .trailing_tag()
                .description(CopyText::ZoomDescription.get())
                .note(zoom_note, Tone::Muted)
                .into(),
            );
        }
        Section::Launcher => {
            let status = if !model.cubes_renderer_available
                && saved.launcher_style == LauncherStyle::Cubes
            {
                CopyText::ShowingListNowTheCubesView.get()
            } else if model
                .effective
                .is_some_and(|value| value.launcher_style == LauncherStyle::List)
                && saved.launcher_style == LauncherStyle::Cubes
            {
                CopyText::ShowingListNowReducedMotionIs.get()
            } else if saved.launcher_style == LauncherStyle::List {
                CopyText::ShowingList.get()
            } else {
                CopyText::ShowingCubes.get()
            };
            rows.push(
                setting_row(
                    CopyText::LauncherStyle.get(),
                    segmented(
                        [
                            segment(
                                "tenshu.preferences.launcher.cubes",
                                LauncherStyle::Cubes,
                                "Cubes",
                            )
                            .icon(cube_outline_icon()),
                            segment(
                                "tenshu.preferences.launcher.list",
                                LauncherStyle::List,
                                CopyText::List.get(),
                            )
                            .icon(list_ul_icon()),
                        ],
                        Some(saved.launcher_style),
                        Message::FocusAction,
                    )
                    .size(SegmentSize::Compact)
                    .on_select_maybe(enabled.then_some(
                        |value: LauncherStyle| {
                            Message::Change(PreferenceChange::LauncherStyle(value))
                        },
                    )),
                )
                .tag(CopyText::ThisDevice.get())
                .trailing_tag()
                .description(CopyText::ListIsUsedAutomaticallyUnderReduced.get())
                .note(status, Tone::Muted)
                .into(),
            );
            rows.push(
                setting_row(
                    CopyText::NetworkActivity.get(),
                    toggle(
                        "tenshu.preferences.launcher.network-activity",
                        saved.network_activity,
                        Message::FocusAction,
                    )
                    .switch()
                    .on_toggle_maybe(enabled.then_some(|shown: bool| {
                        Message::Change(PreferenceChange::NetworkActivity(shown))
                    })),
                )
                .tag(CopyText::ThisDevice.get())
                .trailing_tag()
                .description(CopyText::RecentBlocksTheMempoolFeeRate.get())
                .into(),
            );
            if saved.network_activity {
                rows.push(
                    setting_row(
                        CopyText::ActivitySource.get(),
                        segmented(
                            [
                                segment(
                                    "tenshu.preferences.launcher.activity.automatic",
                                    NetworkActivitySource::Automatic,
                                    CopyText::Automatic.get(),
                                ),
                                segment(
                                    "tenshu.preferences.launcher.activity.local",
                                    NetworkActivitySource::LocalNode,
                                    CopyText::LocalNode.get(),
                                ),
                                segment(
                                    "tenshu.preferences.launcher.activity.connect",
                                    NetworkActivitySource::Connect,
                                    "Connect",
                                ),
                            ],
                            Some(saved.network_activity_source),
                            Message::FocusAction,
                        )
                        .size(SegmentSize::Compact)
                        .on_select_maybe(enabled.then_some(
                            |value: NetworkActivitySource| {
                                Message::Change(PreferenceChange::NetworkActivitySource(value))
                            },
                        )),
                    )
                    .tag(CopyText::ThisDevice.get())
                    .trailing_tag()
                    .description(CopyText::AutomaticReadsALocalNodeThat.get())
                    .into(),
                );
            }
        }
        Section::BalancesAndMotion => {
            let privacy_enabled = enabled && model.balance_privacy_interactive;
            let system_status = match model.system_reduces_motion {
                Some(true) => CopyText::SystemIsCurrentlyReducedMotion.get(),
                Some(false) => CopyText::SystemIsCurrentlyFullMotion.get(),
                None => CopyText::SystemMotionPreferenceUnavailable.get(),
            };
            let control: Element<'a, Message> = match model.balance_privacy {
                BalancePrivacy::Unresolved => explicit_privacy_choice(privacy_enabled),
                BalancePrivacy::HiddenLocally => retry_hide_controls(privacy_enabled),
                privacy => {
                    let change = privacy.toggle_change();
                    toggle(
                        "tenshu.preferences.hide-balances",
                        privacy.masks_amounts(),
                        Message::FocusAction,
                    )
                    .switch()
                    .on_toggle_maybe(
                        privacy_enabled.then_some(move |_: bool| Message::Change(change)),
                    )
                    .into()
                }
            };
            let mut hide = setting_row(CopyText::HideBalances.get(), control)
                .tag(CopyText::ThisDevice.get())
                .trailing_tag()
                .description(CopyText::AmountsShowAsInEveryCube.get());
            if !model.balance_privacy_interactive {
                hide = hide.note(
                    CopyText::BalanceVisibilityChangesAreTemporarilyUnavailable.get(),
                    Tone::Muted,
                );
            }
            rows.push(hide.into());
            rows.push(
                setting_row(
                    CopyText::Motion.get(),
                    segmented(
                        [
                            segment(
                                "tenshu.preferences.motion.system",
                                MotionPreference::FollowSystem,
                                CopyText::FollowSystem.get(),
                            ),
                            segment(
                                "tenshu.preferences.motion.reduced",
                                MotionPreference::Reduced,
                                CopyText::Reduced.get(),
                            ),
                        ],
                        Some(saved.motion),
                        Message::FocusAction,
                    )
                    .size(SegmentSize::Compact)
                    .on_select_maybe(enabled.then_some(
                        |value: MotionPreference| Message::Change(PreferenceChange::Motion(value)),
                    )),
                )
                .tag(CopyText::ThisDevice.get())
                .trailing_tag()
                .description(localized_copy(
                    "Reduced removes the Cube turn and other movement. Security checks never \
                     wait for animation.",
                ))
                .note(system_status, Tone::Muted)
                .into(),
            );
        }
        Section::Developer => {
            rows.push(
                setting_row(
                    CopyText::DeveloperMode.get(),
                    toggle(
                        "tenshu.preferences.developer-mode",
                        saved.developer_mode,
                        Message::FocusAction,
                    )
                    .switch()
                    .on_toggle_maybe(
                        enabled.then_some(|on: bool| {
                            Message::Change(PreferenceChange::DeveloperMode(on))
                        }),
                    ),
                )
                .tag(CopyText::ThisDevice.get())
                .trailing_tag()
                .description(CopyText::ShowTestNetworksInTheCubes.get())
                .into(),
            );
            rows.push(
                setting_row(
                    "Bitcoin Blake2b",
                    toggle(
                        "tenshu.preferences.bitcoin-blake2b-beta",
                        saved.bitcoin_blake2b_beta,
                        Message::FocusAction,
                    )
                    .switch()
                    .on_toggle_maybe(enabled.then_some(|on: bool| {
                        Message::Change(PreferenceChange::BitcoinBlake2bBeta(on))
                    })),
                )
                .tag(CopyText::ThisDevice.get())
                .trailing_tag()
                .description(localized_copy(
                    "Show the Bitcoin Blake2b network in the Launcher, when creating Cubes and in \
                     Vault tools.",
                ))
                .into(),
            );
        }
        Section::Diagnostics => {
            let diagnostics = model.diagnostics;
            let mut logs = Column::new().width(Length::Fill).push(
                setting_row(
                    CopyText::SanitizedLogs.get(),
                    Row::new()
                        .push(
                            ActionButton::new(
                                "tenshu.preferences.open-logs",
                                CopyText::OpenFolder.get(),
                                ActionKind::Secondary,
                                Message::FocusAction,
                            )
                            .compact()
                            .icon(folder_icon())
                            .on_press_maybe(
                                diagnostics
                                    .can_open_logs
                                    .then_some(Message::Action(Action::OpenLogs)),
                            ),
                        )
                        .push(
                            ActionButton::new(
                                "tenshu.preferences.copy-support-summary",
                                CopyText::CopySummary.get(),
                                ActionKind::Secondary,
                                Message::FocusAction,
                            )
                            .compact()
                            .icon(copy_icon())
                            .on_press_maybe(
                                diagnostics
                                    .support_summary_preview
                                    .map(|_| Message::Action(Action::CopySupportSummary)),
                            ),
                        )
                        .spacing(8),
                )
                .tag(CopyText::ThisDevice.get())
                .trailing_tag()
                .description(CopyText::LogsAreSanitizedBeforeTheyRe.get())
                .without_divider(),
            );
            // The allowlisted summary stays inspectable before Copy.
            if let Some(preview) = diagnostics.support_summary_preview {
                logs = logs.push(
                    container(status_text(preview, Tone::Neutral))
                        .padding([12, 16])
                        .width(Length::Fill)
                        .style(review_style),
                );
                logs = logs.push(iced::widget::Space::new().height(Length::Fixed(14.0)));
            }
            rows.push(logs.push(horizontal_rule()).into());

            let mut repair = Column::new().width(Length::Fill).push(
                setting_row(
                    CopyText::RepairDeviceRegistration.get(),
                    ActionButton::new(
                        "tenshu.preferences.repair-details",
                        CopyText::Repair.get(),
                        ActionKind::Secondary,
                        Message::FocusAction,
                    )
                    .compact()
                    .on_press(Message::ToggleRegistrationRepair),
                )
                .tag(CopyText::ThisDevice.get())
                .trailing_tag()
                .description(CopyText::ForWhenConnectNoLongerRecognises.get())
                .without_divider(),
            );
            if page.registration_repair_expanded {
                repair = repair
                    .push(
                        container(
                            Column::new()
                                .push(
                                    tenshu_text::body(
                                        CopyText::ReRegisterThisDeviceWithConnect.get(),
                                    )
                                    .font(font::SEMIBOLD)
                                    .style(theme::tenshu::text::ink),
                                )
                                .push(
                                    Column::new()
                                        .push(bullet(CopyText::ReRegistersThisDeviceWithYour.get()))
                                        .push(bullet(
                                            CopyText::RequiresSigningInToConnectFirst.get(),
                                        ))
                                        .push(bullet(CopyText::DoesNotChangeAnyCubeWallet.get()))
                                        .spacing(2)
                                        .padding(iced::Padding {
                                            top: 6.0,
                                            bottom: 12.0,
                                            ..iced::Padding::ZERO
                                        }),
                                )
                                .push(
                                    Row::new()
                                        .push(
                                            ActionButton::new(
                                                "tenshu.preferences.repair-registration",
                                                CopyText::SignInAndRepair.get(),
                                                ActionKind::Primary,
                                                Message::FocusAction,
                                            )
                                            .compact()
                                            .on_press_maybe(
                                                diagnostics.can_repair_registration.then_some(
                                                    Message::Action(Action::RepairRegistration),
                                                ),
                                            ),
                                        )
                                        .push(
                                            ActionButton::new(
                                                "tenshu.preferences.repair-cancel",
                                                CopyText::Cancel.get(),
                                                ActionKind::Secondary,
                                                Message::FocusAction,
                                            )
                                            .compact()
                                            .on_press(Message::ToggleRegistrationRepair),
                                        )
                                        .spacing(12)
                                        .align_y(Alignment::Center),
                                ),
                        )
                        .padding([14, 16])
                        .width(Length::Fill)
                        .style(review_style),
                    )
                    .push(iced::widget::Space::new().height(Length::Fixed(14.0)));
            }
            rows.push(repair.push(horizontal_rule()).into());

            let stream: Element<'a, Message> = match diagnostics.stream_status {
                Some(status) => status_text(status, Tone::Neutral).into(),
                None => status_line(dash_circle_icon(), CopyText::Unavailable.get(), Tone::Muted),
            };
            rows.push(
                setting_row(CopyText::StreamStatus.get(), stream)
                    .tag(CopyText::ThisDevice.get())
                    .trailing_tag()
                    .description(CopyText::LiveUpdatesFromConnectToThis.get())
                    .into(),
            );
        }
        Section::About => {
            let about = model.about;
            rows.push(
                setting_row(
                    CopyText::TenshuVersion.get(),
                    value_text(crate::VERSION.to_string()),
                )
                .tag(CopyText::ThisDevice.get())
                .trailing_tag()
                .description(CopyText::InstalledOnThisDevice.get())
                .into(),
            );
            rows.push(
                setting_row(
                    CopyText::BuildProvenance.get(),
                    value_text(
                        about
                            .build_provenance
                            .unwrap_or(CopyText::Unavailable.get()),
                    ),
                )
                .tag(CopyText::ThisDevice.get())
                .trailing_tag()
                .description(CopyText::WhichBuildThisIsAndWhether.get())
                .into(),
            );
            rows.push(component_versions_row(page.component_versions_expanded));
            if page.component_versions_expanded {
                rows.push(sub_row(
                    CopyText::WalletDaemon.get(),
                    about.daemon_version.unwrap_or(CopyText::NotLoaded.get()),
                ));
                rows.push(sub_row(
                    CopyText::SparkBridge.get(),
                    about
                        .spark_bridge_version
                        .unwrap_or(CopyText::NotLoaded.get()),
                ));
            }
            rows.push(
                setting_row(
                    CopyText::DownloadStats.get(),
                    link_action(
                        "tenshu.preferences.open-download-stats",
                        CopyText::ViewDownloadStats.get(),
                        about
                            .can_open_download_stats
                            .then_some(Message::Action(Action::OpenDownloadStats)),
                    ),
                )
                .tag(CopyText::ThisDevice.get())
                .trailing_tag()
                .description(localized_copy("Public download counts for Tenshu releases"))
                .into(),
            );
            rows.push(
                setting_row(
                    localized_copy("Open-source licenses"),
                    link_action(
                        "tenshu.preferences.open-licenses",
                        localized_copy("View licenses"),
                        about
                            .can_open_licenses
                            .then_some(Message::Action(Action::OpenLicenses)),
                    ),
                )
                .tag(CopyText::ThisDevice.get())
                .trailing_tag()
                .description(localized_copy(
                    "Licenses for the software Tenshu is built on",
                ))
                .into(),
            );
        }
    }
    // `.rows`: a rule above the first row; each row draws the rule under it.
    content
        .push(
            rows.into_iter()
                .fold(Column::new().push(horizontal_rule()), |list, row| {
                    list.push(row)
                }),
        )
        .into()
}

/// Component versions (`.disc` in a `.row`): the chevron and label are the
/// disclosure action; the caption and the tag sit as in any other row.
fn component_versions_row<'a>(expanded: bool) -> Element<'a, Message> {
    let chevron = if expanded {
        collapsed_icon()
    } else {
        chevron_right()
    };
    let disclosure = FocusableAction::new(
        "tenshu.preferences.component-versions",
        Row::new()
            .push(
                chevron
                    .size(16)
                    .width(Length::Fixed(16.0))
                    .style(theme::tenshu::text::ink),
            )
            .push(tenshu_text::label(localized_copy("Component versions")))
            .spacing(6)
            .align_y(Alignment::Center),
        Message::FocusAction,
    )
    .padding(0)
    .style(theme::tenshu::control::disclosure)
    .on_press(Message::ToggleComponentVersions);
    row_frame(
        Column::new()
            .push(disclosure)
            .push(
                tenshu_text::body(localized_copy("Wallet daemon and Spark bridge"))
                    .size(13)
                    .style(theme::tenshu::text::muted),
            )
            .spacing(2)
            .width(Length::Fill)
            .into(),
        None,
        12.0,
        0.0,
    )
}

/// A component version under Component versions (`.row.sub`): 13px, inset
/// 20px, 10px vertical padding.
fn sub_row<'a>(label: &'static str, value: &'a str) -> Element<'a, Message> {
    row_frame(
        tenshu_text::body(label)
            .size(13)
            .style(theme::tenshu::text::ink)
            .width(Length::Fill)
            .into(),
        Some(value_text(value).into()),
        10.0,
        20.0,
    )
}

/// `.row` built by hand for rows `setting_row` cannot express: content,
/// optional control, the trailing tag, then the rule.
fn row_frame<'a>(
    text: Element<'a, Message>,
    control: Option<Element<'a, Message>>,
    padding_y: f32,
    inset: f32,
) -> Element<'a, Message> {
    let mut row = Row::new().push(text).spacing(24).align_y(Alignment::Center);
    if let Some(control) = control {
        row = row.push(control);
    }
    row = row.push(
        container(scope_tag())
            .width(Length::Fixed(TAG_COLUMN))
            .align_x(iced::alignment::Horizontal::Right),
    );
    Column::new()
        .push(container(row).padding(iced::Padding {
            top: padding_y,
            bottom: padding_y,
            left: inset,
            right: 0.0,
        }))
        .push(horizontal_rule())
        .width(Length::Fill)
        .into()
}

fn scope_tag<'a>() -> Element<'a, Message> {
    container(tenshu_text::caption(CopyText::ThisDevice.get()))
        .padding([0, 6])
        .style(theme::tenshu::control::tag)
        .into()
}

fn notice_column<'a>(notices: Vec<(String, Tone)>) -> Element<'a, Message> {
    notices
        .into_iter()
        .fold(Column::new().spacing(8), |column, (line, tone)| {
            column.push(status_text(line, tone))
        })
        .into()
}

/// A value in the control column (`.val`), 13px ink.
fn value_text<'a>(
    value: impl iced::widget::text::IntoFragment<'a>,
) -> iced::widget::Text<'a, theme::Theme> {
    tenshu_text::body(value)
        .size(13)
        .style(theme::tenshu::text::ink)
}

/// A 13px status or notice line in `tone`.
fn status_text<'a>(
    line: impl iced::widget::text::IntoFragment<'a>,
    tone: Tone,
) -> iced::widget::Text<'a, theme::Theme> {
    tenshu_text::body(line)
        .size(13)
        .style(move |t| theme::tenshu::text::tone(t, tone))
}

/// A status with its glyph (`.stt` + tone).
fn status_line<'a>(
    icon: iced::widget::Text<'a, theme::Theme>,
    label: &'a str,
    tone: Tone,
) -> Element<'a, Message> {
    Row::new()
        .push(
            icon.size(16)
                .width(Length::Fixed(16.0))
                .style(move |t| theme::tenshu::text::tone(t, tone)),
        )
        .push(status_text(label, tone))
        .spacing(6)
        .align_y(Alignment::Center)
        .into()
}

fn bullet<'a>(line: &'a str) -> Element<'a, Message> {
    Row::new()
        .push(
            tenshu_text::body("•")
                .size(13)
                .style(theme::tenshu::text::ink),
        )
        .push(
            tenshu_text::body(line)
                .size(13)
                .style(theme::tenshu::text::ink),
        )
        .spacing(8)
        .padding(iced::Padding::default().left(4))
        .into()
}

/// `.linkbtn`: ink text with an accent underline and a trailing chevron. The
/// underline is drawn by an accent copy of the label beneath the ink one.
fn link_action<'a>(
    id: &'static str,
    label: &'static str,
    message: Option<Message>,
) -> Element<'a, Message> {
    let enabled = message.is_some();
    let ink = move |theme: &theme::Theme| {
        let color = theme.colors.shell.ink;
        iced::widget::text::Style {
            color: Some(if enabled {
                color
            } else {
                theme::tenshu::faded(color, theme::tenshu::DISABLED_ALPHA)
            }),
        }
    };
    // Disabled, the label is one faded, underlined ink line; enabled, the
    // ink label is drawn over an accent copy that contributes the underline.
    let underline = iced::widget::rich_text([iced::widget::span::<(), _>(label).underline(true)])
        .size(13)
        .font(font::REGULAR)
        .wrapping(iced::widget::text::Wrapping::None)
        .style(move |theme: &theme::Theme| {
            let shell = &theme.colors.shell;
            iced::widget::text::Style {
                color: Some(if enabled {
                    shell.accent
                } else {
                    theme::tenshu::faded(shell.ink, theme::tenshu::DISABLED_ALPHA)
                }),
            }
        });
    let label: Element<'a, Message> = if enabled {
        iced::widget::stack![
            underline,
            iced::widget::text(label)
                .size(13)
                .font(font::REGULAR)
                .wrapping(iced::widget::text::Wrapping::None)
                .style(ink)
        ]
        .into()
    } else {
        underline.into()
    };
    FocusableAction::new(
        id,
        Row::new()
            .push(label)
            .push(
                chevron_right()
                    .size(14)
                    .width(Length::Fixed(14.0))
                    .style(ink),
            )
            .spacing(6)
            .align_y(Alignment::Center),
        Message::FocusAction,
    )
    .padding([2, 0])
    .style(|theme, status| {
        theme::tenshu::button::action(
            theme,
            status,
            ActionKind::Quiet,
            theme::tenshu::button::ActionShape::Compact,
        )
    })
    .on_press_maybe(message)
    .into()
}

/// The step the Zoom row shows selected and its note. A saved step over the
/// screen's cap is shown as the one in use, and the note says why.
fn zoom_choice(saved: Zoom, max_zoom: Zoom) -> (Zoom, String) {
    let note = if max_zoom < Zoom::LARGEST {
        format!(
            "{} {}",
            zoom_shortcuts(),
            CopyText::ZoomLimit
                .get()
                .replacen("{percent}", &max_zoom.percent().to_string(), 1)
        )
    } else {
        zoom_shortcuts().to_string()
    };
    (saved.min(max_zoom), note)
}

/// The keys `gui::ignored_shell_key` reads, named for this platform.
fn zoom_shortcuts() -> &'static str {
    if cfg!(target_os = "macos") {
        CopyText::ZoomMacShortcuts.get()
    } else {
        CopyText::ZoomOtherShortcuts.get()
    }
}

/// Shown while no accepted privacy choice exists: amounts are masked, and the
/// user may persist an explicit choice only when the shared balance controls
/// are interactive and no load or save is in flight.
/// Neither button is a fallback default.
fn explicit_privacy_choice(enabled: bool) -> Element<'static, Message> {
    Column::new()
        .spacing(6)
        .push(status_text(
            if enabled {
                localized_copy("Not set yet. Amounts stay hidden until you choose.")
            } else {
                localized_copy(
                    "Not set yet. Amounts stay hidden while visibility changes are unavailable.",
                )
            },
            Tone::Muted,
        ))
        .push(
            Row::new()
                .spacing(8)
                .push(
                    ActionButton::new(
                        "tenshu.preferences.balances.show",
                        localized_copy("Show balances"),
                        ActionKind::Secondary,
                        Message::FocusAction,
                    )
                    .compact()
                    .on_press_maybe(
                        enabled.then_some(Message::Change(PreferenceChange::HideBalances(false))),
                    ),
                )
                .push(
                    ActionButton::new(
                        "tenshu.preferences.balances.keep-hidden",
                        localized_copy("Keep hidden"),
                        ActionKind::Secondary,
                        Message::FocusAction,
                    )
                    .compact()
                    .on_press_maybe(
                        enabled.then_some(Message::Change(PreferenceChange::HideBalances(true))),
                    ),
                ),
        )
        .into()
}

/// Hide already masks this session, but the accepted on-disk choice still
/// says Show. Make Retry Hide the primary action; an explicit Show remains
/// available for a user who chooses to reveal again.
fn retry_hide_controls(enabled: bool) -> Element<'static, Message> {
    Column::new()
        .spacing(6)
        .push(status_text(
            localized_copy("Balances are hidden now. Hide has not been saved on this device."),
            Tone::Muted,
        ))
        .push(
            Row::new()
                .spacing(8)
                .push(
                    ActionButton::new(
                        "tenshu.preferences.balances.retry-hide",
                        "Retry Hide",
                        ActionKind::Primary,
                        Message::FocusAction,
                    )
                    .compact()
                    .on_press_maybe(
                        enabled.then_some(Message::Change(PreferenceChange::HideBalances(true))),
                    ),
                )
                .push(
                    ActionButton::new(
                        "tenshu.preferences.balances.show-explicitly",
                        localized_copy("Show balances"),
                        ActionKind::Secondary,
                        Message::FocusAction,
                    )
                    .compact()
                    .on_press_maybe(
                        enabled.then_some(Message::Change(PreferenceChange::HideBalances(false))),
                    ),
                ),
        )
        .into()
}

/// Centres a control's content on its fixed height.
fn centred<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(content).center_y(Length::Fill).into()
}

fn horizontal_rule<'a>() -> Element<'a, Message> {
    container(iced::widget::Space::new())
        .width(Length::Fill)
        .height(Length::Fixed(1.0))
        .style(theme::tenshu::control::divider)
        .into()
}

fn vertical_rule<'a>() -> Element<'a, Message> {
    container(iced::widget::Space::new())
        .width(Length::Fixed(1.0))
        .height(Length::Fill)
        .style(theme::tenshu::control::divider)
        .into()
}

/// The sheet surface (`.sheet`): panel fill, 1px line, 12px radius and the
/// drop shadow.
fn sheet_style(theme: &theme::Theme) -> container::Style {
    let shell = &theme.colors.shell;
    container::Style {
        text_color: Some(shell.ink),
        background: Some(iced::Background::Color(shell.panel)),
        border: iced::Border {
            color: shell.line,
            width: 1.0,
            radius: 12.0.into(),
        },
        shadow: iced::Shadow {
            color: shell.shadow,
            offset: iced::Vector::new(0.0, 24.0),
            blur_radius: 48.0,
        },
        ..Default::default()
    }
}

/// The repair review and summary preview (`.review`): selected fill, 8px radius.
fn review_style(theme: &theme::Theme) -> container::Style {
    container::Style {
        text_color: Some(theme.colors.shell.ink),
        background: Some(iced::Background::Color(theme.colors.shell.selected)),
        border: iced::Border {
            radius: 8.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::advanced::{
        layout,
        renderer::Headless,
        widget::{operation, operation::focusable, Operation, Tree},
        Layout,
    };
    use iced::{keyboard, mouse, Event, Rectangle, Size};

    fn renderer() -> iced::Renderer {
        for bytes in coincube_ui::font::load() {
            iced::advanced::graphics::text::font_system()
                .write()
                .unwrap()
                .load_font(bytes);
        }
        iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
            coincube_ui::font::REGULAR,
            14.into(),
            Some("tiny-skia"),
        ))
        .unwrap()
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
                &mut operation::black_box(operation.as_mut()),
            );
            match operation.finish() {
                operation::Outcome::None => return None,
                operation::Outcome::Some(value) => return Some(value),
                operation::Outcome::Chain(next) => operation = next,
            }
        }
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
    fn a_saved_zoom_over_the_cap_shows_the_step_in_use_and_why() {
        let (selected, note) = zoom_choice(Zoom::LARGEST, Zoom::from(125));
        assert_eq!(selected, Zoom::from(125));
        assert!(note.ends_with("This screen fits up to 125%."), "{}", note);
        let (selected, note) = zoom_choice(Zoom::from(110), Zoom::LARGEST);
        assert_eq!(selected, Zoom::from(110));
        assert_eq!(note, zoom_shortcuts());
    }

    #[test]
    fn zoom_steps_over_the_screen_cap_cannot_be_picked() {
        let model = || ViewModel {
            saved: Some(DevicePreferences::default()),
            max_zoom: Some(Zoom::from(125)),
            ..Default::default()
        };
        let (_, messages) = activate(
            model(),
            Section::Appearance,
            "tenshu.preferences.zoom.150",
            keyboard::key::Named::Enter,
        );
        assert!(messages.is_empty(), "{:?}", messages);
        let (focused, messages) = activate(
            model(),
            Section::Appearance,
            "tenshu.preferences.zoom.125",
            keyboard::key::Named::Enter,
        );
        assert!(focused);
        assert_eq!(
            messages,
            vec![Message::Change(PreferenceChange::Zoom(Zoom::from(125)))]
        );
    }

    fn activate(
        model: ViewModel<'_>,
        section: Section,
        id: &'static str,
        named: keyboard::key::Named,
    ) -> (bool, Vec<Message>) {
        let page = PreferencesPageState {
            section,
            ..Default::default()
        };
        let mut element = view(&page, model);
        let mut tree = Tree::new(element.as_widget());
        let renderer = renderer();
        let size = Size::new(1024.0, 768.0);
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
            focusable::focus::<()>(Id::new(id)),
        );
        let focused = operate(
            &mut element,
            &mut tree,
            &node,
            &renderer,
            focusable::find_focused(),
        ) == Some(Id::new(id));
        let mut messages = Vec::new();
        for event in [
            key(named, false, true),
            key(named, true, true),
            key(named, false, false),
        ] {
            element.as_widget_mut().update(
                &mut tree,
                &event,
                Layout::new(&node),
                mouse::Cursor::Unavailable,
                &renderer,
                &mut iced::advanced::clipboard::Null,
                &mut iced::advanced::Shell::new(&mut messages),
                &Rectangle::with_size(size),
            );
        }
        (focused, messages)
    }

    #[test]
    fn appearance_language_is_keyboard_operable_and_independent_of_worldview() {
        for worldview in [
            WorldviewMode::Samurai,
            WorldviewMode::Suitcoiner,
            WorldviewMode::Luke,
        ] {
            let model = ViewModel {
                saved: Some(DevicePreferences {
                    worldview,
                    ..DevicePreferences::default()
                }),
                ..Default::default()
            };
            let (focused, messages) = activate(
                model,
                Section::Appearance,
                "tenshu.preferences.language.spanish",
                keyboard::key::Named::Enter,
            );
            assert!(focused);
            assert_eq!(
                messages,
                vec![Message::Change(PreferenceChange::Language(
                    Language::Spanish
                ))]
            );
        }
    }

    #[test]
    fn worldview_voice_profiles_and_applied_mode_notice_render_in_appearance() {
        use crate::app::view::shared::landing::capture;

        let page = PreferencesPageState {
            section: Section::Appearance,
            ..Default::default()
        };
        for (mode, consensus, notice) in [
            (
                WorldviewMode::Suitcoiner,
                "Consensus by market capitalization.",
                "Institutional wisdom activated.",
            ),
            (
                WorldviewMode::Luke,
                "Consensus is not a popularity contest.",
                "You have rejected the establishment.",
            ),
        ] {
            let model = ViewModel {
                saved: Some(DevicePreferences {
                    worldview: mode,
                    ..Default::default()
                }),
                worldview_changed: Some(mode),
                ..Default::default()
            };
            let shown = capture::inspect_filled(
                "worldview-voice-appearance",
                view(&page, model),
                (1024, 768),
                "dark",
                theme::Theme::dark(),
                (1024, 768),
            );
            assert!(shown.contains(consensus));
            assert!(shown.contains(notice));
            assert!(
                shown.contains("Your wallets, networks, and security settings remain unchanged.")
            );
        }
    }

    #[test]
    fn worldview_panel_shows_names_and_real_wording_for_the_selected_mode() {
        use crate::app::view::shared::landing::capture;

        let page = PreferencesPageState {
            section: Section::Appearance,
            ..Default::default()
        };
        for (mode, shown_lines, hidden) in [
            (
                WorldviewMode::Samurai,
                vec![
                    "Network names in Samurai",
                    "Tend to what is yours to control.",
                    "Welcome. Begin when you are ready.",
                    "Empty and ready. Fill it with patience.",
                    "Sent with intention.",
                    "Patience is part of the way.",
                    "Unchanged",
                ],
                "No ideological baggage",
            ),
            (
                WorldviewMode::Suitcoiner,
                vec![
                    "Network names in Suitcoiner",
                    "LukeCoin (BTCB2)",
                    "Welcome, institutional-grade HODLer.",
                    "Time to stack some sats.",
                    "The free market thanks you.",
                ],
                "Sent with intention.",
            ),
            (
                WorldviewMode::Luke,
                vec![
                    "Network names in Luke",
                    "Spamcoin (BTC)",
                    "Bitcoin (XBT)",
                    "No Spamcoin detected. Excellent.",
                    "No JPEGs were required.",
                    "Consulting the ancient rules…",
                ],
                "Unchanged",
            ),
        ] {
            let model = ViewModel {
                saved: Some(DevicePreferences {
                    worldview: mode,
                    bitcoin_blake2b_beta: true,
                    ..Default::default()
                }),
                ..Default::default()
            };
            let shown = capture::inspect_filled(
                &format!("worldview-panel-{mode:?}"),
                view(&page, model),
                (1024, 1100),
                "dark",
                theme::Theme::dark(),
                (1024, 1100),
            );
            for line in shown_lines {
                assert!(shown.contains(line), "{:?} is missing {:?}", mode, line);
            }
            assert!(!shown.contains(hidden), "{:?} shows {:?}", mode, hidden);
            assert!(shown.contains("Example wording"));
            assert!(shown.contains("Bitcoin SHA-256 · Mainnet"));
        }

        // Without the BLAKE2b beta the panel names only the network in use.
        let model = ViewModel {
            saved: Some(DevicePreferences {
                worldview: WorldviewMode::Luke,
                ..Default::default()
            }),
            ..Default::default()
        };
        let shown = capture::inspect_filled(
            "worldview-panel-no-blake2b",
            view(&page, model),
            (1024, 1100),
            "dark",
            theme::Theme::dark(),
            (1024, 1100),
        );
        assert!(shown.contains("Spamcoin (BTC)"));
        assert!(!shown.contains("Bitcoin (XBT)"));
    }

    #[test]
    fn spanish_appearance_fits_for_all_three_worldviews() {
        use crate::app::view::shared::landing::capture;
        let page = PreferencesPageState {
            section: Section::Appearance,
            ..Default::default()
        };
        for (worldview, tagline) in [
            (WorldviewMode::Samurai, "El camino de Bitcoin"),
            (WorldviewMode::Suitcoiner, "El mercado ha hablado"),
            (WorldviewMode::Luke, "El spam no pasará"),
        ] {
            for (appearance, theme) in [
                ("dark", theme::Theme::dark()),
                ("light", theme::Theme::light()),
            ] {
                let model = ViewModel {
                    saved: Some(DevicePreferences {
                        worldview,
                        language: Language::Spanish,
                        ..DevicePreferences::default()
                    }),
                    ..Default::default()
                };
                let result = coincube_ui::locale::render_in(Language::Spanish, || {
                    capture::inspect_filled(
                        &format!("spanish-preferences-{worldview:?}"),
                        view(&page, model),
                        (1024, 768),
                        appearance,
                        theme,
                        (1024, 768),
                    )
                });
                assert!(result.contains("Preferencias de la aplicación"));
                assert!(result.contains("Idioma"));
                assert!(result.contains("Español"));
                assert!(result.contains(tagline));
            }
        }
    }

    #[test]
    fn artwork_picker_uses_the_active_worldview_and_fits_appearance() {
        use crate::app::view::shared::landing::capture;
        use coincube_ui::theme::Theme;
        let page = PreferencesPageState {
            section: Section::Appearance,
            ..Default::default()
        };
        for (worldview, existing, scene) in [
            (WorldviewMode::Samurai, "Kage", "Mountain Watch"),
            (
                WorldviewMode::Suitcoiner,
                "Gilded Wall Street",
                "Gilded Exchange",
            ),
            (
                WorldviewMode::Luke,
                "Purist’s Woodcut",
                "The Stone Crossing",
            ),
        ] {
            let model = ViewModel {
                saved: Some(DevicePreferences {
                    worldview,
                    ..Default::default()
                }),
                ..Default::default()
            };
            for (id, style) in [
                (
                    "tenshu.preferences.artwork.existing",
                    ArtworkStyle::Existing,
                ),
                ("tenshu.preferences.artwork.scene", ArtworkStyle::Scene),
            ] {
                let (focused, messages) =
                    activate(model, Section::Appearance, id, keyboard::key::Named::Enter);
                assert!(focused);
                assert_eq!(
                    messages,
                    vec![Message::Change(PreferenceChange::Artwork(worldview, style))]
                );
                let (focused, messages) = activate(
                    ViewModel {
                        saving: true,
                        ..model
                    },
                    Section::Appearance,
                    id,
                    keyboard::key::Named::Space,
                );
                assert!(!focused);
                assert!(messages.is_empty());
            }
            for (appearance, theme) in [("light", Theme::light()), ("dark", Theme::dark())] {
                let mounted = capture::inspect_filled(
                    &format!("artwork-picker-{worldview:?}"),
                    view(&page, model),
                    (1024, 768),
                    appearance,
                    theme.with_worldview(worldview),
                    (1024, 768),
                );
                assert!(mounted.contains(existing));
                assert!(mounted.contains(scene));
            }
        }
    }

    #[test]
    fn appearance_worldview_segments_emit_typed_keyboard_intents() {
        for (id, mode) in [
            (
                "tenshu.preferences.worldview.samurai",
                WorldviewMode::Samurai,
            ),
            (
                "tenshu.preferences.worldview.suitcoiner",
                WorldviewMode::Suitcoiner,
            ),
            ("tenshu.preferences.worldview.luke", WorldviewMode::Luke),
        ] {
            let model = ViewModel {
                saved: Some(DevicePreferences::default()),
                ..Default::default()
            };
            let (focused, messages) =
                activate(model, Section::Appearance, id, keyboard::key::Named::Enter);
            assert!(focused, "{}", id);
            assert_eq!(
                messages,
                vec![Message::Change(PreferenceChange::Worldview(mode))]
            );

            let (focused, messages) = activate(
                ViewModel {
                    saving: true,
                    ..model
                },
                Section::Appearance,
                id,
                keyboard::key::Named::Space,
            );
            assert!(!focused, "{} was focusable while saving", id);
            assert!(messages.is_empty());
        }
    }

    #[test]
    fn rendered_boolean_controls_toggle_both_directions_without_optimistic_state() {
        for (section, id, change) in [
            (
                Section::BalancesAndMotion,
                "tenshu.preferences.hide-balances",
                PreferenceChange::HideBalances(true),
            ),
            (
                Section::Developer,
                "tenshu.preferences.developer-mode",
                PreferenceChange::DeveloperMode(true),
            ),
            (
                Section::Developer,
                "tenshu.preferences.bitcoin-blake2b-beta",
                PreferenceChange::BitcoinBlake2bBeta(true),
            ),
        ] {
            let saved = DevicePreferences::default();
            // Privacy comes from the accepted receipt, not `saved`.
            let privacy = |on: bool| match change {
                PreferenceChange::HideBalances(_) if on => BalancePrivacy::Hidden,
                PreferenceChange::HideBalances(_) => BalancePrivacy::Shown,
                _ => BalancePrivacy::Unresolved,
            };
            let (focused, messages) = activate(
                ViewModel {
                    saved: Some(saved),
                    balance_privacy: privacy(false),
                    balance_privacy_interactive: true,
                    ..Default::default()
                },
                section,
                id,
                keyboard::key::Named::Enter,
            );
            assert!(focused, "{}", id);
            assert_eq!(messages, vec![Message::Change(change)]);
            let enabled = match change {
                PreferenceChange::HideBalances(_) => DevicePreferences {
                    hide_balances: true,
                    ..saved
                },
                PreferenceChange::DeveloperMode(_) => DevicePreferences {
                    developer_mode: true,
                    ..saved
                },
                PreferenceChange::BitcoinBlake2bBeta(_) => DevicePreferences {
                    bitcoin_blake2b_beta: true,
                    ..saved
                },
                _ => unreachable!(),
            };
            let expected_off = match change {
                PreferenceChange::HideBalances(_) => PreferenceChange::HideBalances(false),
                PreferenceChange::DeveloperMode(_) => PreferenceChange::DeveloperMode(false),
                PreferenceChange::BitcoinBlake2bBeta(_) => {
                    PreferenceChange::BitcoinBlake2bBeta(false)
                }
                _ => unreachable!(),
            };
            let (focused, messages) = activate(
                ViewModel {
                    saved: Some(enabled),
                    balance_privacy: privacy(true),
                    balance_privacy_interactive: true,
                    ..Default::default()
                },
                section,
                id,
                keyboard::key::Named::Space,
            );
            assert!(focused, "{}", id);
            assert_eq!(messages, vec![Message::Change(expected_off)]);
            let (focused, messages) = activate(
                ViewModel {
                    saved: Some(enabled),
                    balance_privacy: privacy(true),
                    balance_privacy_interactive: true,
                    saving: true,
                    ..Default::default()
                },
                section,
                id,
                keyboard::key::Named::Enter,
            );
            assert!(!focused, "saving control {} was focusable", id);
            assert!(messages.is_empty());
            // A rejected save leaves the supplied accepted snapshot unchanged.
            let (_, messages) = activate(
                ViewModel {
                    saved: Some(enabled),
                    balance_privacy: privacy(true),
                    balance_privacy_interactive: true,
                    save_error: Some("write denied"),
                    ..Default::default()
                },
                section,
                id,
                keyboard::key::Named::Enter,
            );
            assert_eq!(messages, vec![Message::Change(expected_off)]);
        }
    }

    #[test]
    fn initial_load_error_has_focusable_retry_and_blocks_repeat_while_loading() {
        let model = ViewModel {
            load_error: Some("read denied"),
            ..Default::default()
        };
        let (focused, messages) = activate(
            model,
            Section::About,
            "tenshu.preferences.retry-load",
            keyboard::key::Named::Enter,
        );
        assert!(focused);
        assert_eq!(messages, vec![Message::Action(Action::RetryLoad)]);
        let (focused, messages) = activate(
            ViewModel {
                loading: true,
                ..model
            },
            Section::About,
            "tenshu.preferences.retry-load",
            keyboard::key::Named::Enter,
        );
        assert!(!focused);
        assert!(messages.is_empty());
        // An explicit Show/Keep-hidden save after the failed load is pending:
        // the retained error stays visible but Retry is not a target either.
        let (focused, messages) = activate(
            ViewModel {
                saving: true,
                ..model
            },
            Section::About,
            "tenshu.preferences.retry-load",
            keyboard::key::Named::Enter,
        );
        assert!(
            !focused,
            "Retry focusable while an explicit save is pending"
        );
        assert!(messages.is_empty(), "{:?}", messages);
        let (focused, messages) = activate(
            model,
            Section::About,
            "tenshu.preferences.retry-load",
            keyboard::key::Named::Space,
        );
        assert!(focused, "idle failure Retry must still work");
        assert_eq!(messages, vec![Message::Action(Action::RetryLoad)]);
        let (focused, messages) = activate(
            ViewModel {
                saved: Some(DevicePreferences::default()),
                load_error: None,
                ..model
            },
            Section::About,
            "tenshu.preferences.retry-load",
            keyboard::key::Named::Enter,
        );
        assert!(!focused);
        assert!(messages.is_empty());
    }

    #[test]
    fn unresolved_privacy_offers_only_an_explicit_choice_and_ignores_saved_default() {
        // An accepted snapshot whose privacy marker is unresolved: its
        // `hide_balances == false` must not render as an "Off" toggle.
        let unresolved = ViewModel {
            saved: Some(DevicePreferences::default()),
            balance_privacy: BalancePrivacy::Unresolved,
            balance_privacy_interactive: true,
            ..Default::default()
        };
        let (focused, messages) = activate(
            unresolved,
            Section::BalancesAndMotion,
            "tenshu.preferences.hide-balances",
            keyboard::key::Named::Enter,
        );
        assert!(!focused, "unresolved privacy must not render the toggle");
        assert!(messages.is_empty(), "{:?}", messages);
        for (id, hide) in [
            ("tenshu.preferences.balances.show", false),
            ("tenshu.preferences.balances.keep-hidden", true),
        ] {
            let (focused, messages) = activate(
                unresolved,
                Section::BalancesAndMotion,
                id,
                keyboard::key::Named::Enter,
            );
            assert!(focused, "{}", id);
            assert_eq!(
                messages,
                vec![Message::Change(PreferenceChange::HideBalances(hide))]
            );
            for busy in [
                ViewModel {
                    saving: true,
                    ..unresolved
                },
                ViewModel {
                    loading: true,
                    ..unresolved
                },
            ] {
                let (focused, messages) = activate(
                    busy,
                    Section::BalancesAndMotion,
                    id,
                    keyboard::key::Named::Enter,
                );
                assert!(!focused, "{} must be inert while busy", id);
                assert!(messages.is_empty(), "{:?}", messages);
            }
        }
        // No accepted receipt at all: after a load error the explicit choice
        // sits beside Retry; while loading neither is available.
        let failed = ViewModel {
            load_error: Some("Your privacy setting could not be loaded."),
            balance_privacy_interactive: true,
            ..Default::default()
        };
        let (focused, messages) = activate(
            failed,
            Section::Appearance,
            "tenshu.preferences.balances.show",
            keyboard::key::Named::Space,
        );
        assert!(focused);
        assert_eq!(
            messages,
            vec![Message::Change(PreferenceChange::HideBalances(false))]
        );
        let (focused, messages) = activate(
            ViewModel {
                loading: true,
                ..failed
            },
            Section::Appearance,
            "tenshu.preferences.balances.show",
            keyboard::key::Named::Space,
        );
        assert!(!focused);
        assert!(messages.is_empty(), "{:?}", messages);
    }

    #[test]
    fn privacy_controls_follow_shared_chrome_availability() {
        for privacy in [BalancePrivacy::Hidden, BalancePrivacy::Shown] {
            let model = ViewModel {
                saved: Some(DevicePreferences::default()),
                balance_privacy: privacy,
                // The live shell chrome has `interactive: false` until every
                // passive source amount has been checked.
                balance_privacy_interactive: false,
                ..Default::default()
            };
            let (focused, messages) = activate(
                model,
                Section::BalancesAndMotion,
                "tenshu.preferences.hide-balances",
                keyboard::key::Named::Enter,
            );
            assert!(!focused, "{:?}", privacy);
            assert!(messages.is_empty(), "{:?}: {:?}", privacy, messages);
        }

        for saved in [Some(DevicePreferences::default()), None] {
            for id in [
                "tenshu.preferences.balances.show",
                "tenshu.preferences.balances.keep-hidden",
            ] {
                let model = ViewModel {
                    saved,
                    balance_privacy: BalancePrivacy::Unresolved,
                    load_error: saved.is_none().then_some("read denied"),
                    ..Default::default()
                };
                let (focused, messages) = activate(
                    model,
                    Section::BalancesAndMotion,
                    id,
                    keyboard::key::Named::Space,
                );
                assert!(!focused, "{} with saved={:?}", id, saved);
                assert!(messages.is_empty(), "{}: {:?}", id, messages);
            }
        }
    }

    #[test]
    fn local_hide_presents_retry_and_separate_explicit_show() {
        let model = ViewModel {
            saved: Some(DevicePreferences::default()),
            balance_privacy: BalancePrivacy::HiddenLocally,
            balance_privacy_interactive: true,
            save_error: Some("write denied"),
            ..Default::default()
        };
        let (focused, messages) = activate(
            model,
            Section::BalancesAndMotion,
            "tenshu.preferences.balances.retry-hide",
            keyboard::key::Named::Enter,
        );
        assert!(focused);
        assert_eq!(
            messages,
            vec![Message::Change(PreferenceChange::HideBalances(true))]
        );
        let (focused, messages) = activate(
            model,
            Section::BalancesAndMotion,
            "tenshu.preferences.balances.show-explicitly",
            keyboard::key::Named::Enter,
        );
        assert!(focused);
        assert_eq!(
            messages,
            vec![Message::Change(PreferenceChange::HideBalances(false))]
        );
        let (focused, messages) = activate(
            ViewModel {
                saving: true,
                ..model
            },
            Section::BalancesAndMotion,
            "tenshu.preferences.balances.retry-hide",
            keyboard::key::Named::Enter,
        );
        assert!(!focused);
        assert!(messages.is_empty());

        let page = PreferencesPageState {
            section: Section::BalancesAndMotion,
            ..Default::default()
        };
        for (scenario, presented, has_error) in [
            ("failed-hide", model, true),
            (
                "reloaded-show",
                ViewModel {
                    save_error: None,
                    ..model
                },
                false,
            ),
        ] {
            let mounted = crate::app::view::shared::landing::capture::inspect(
                scenario,
                view(&page, presented),
                (1440, 900),
                "dark",
                coincube_ui::theme::Theme::dark(),
            );
            assert!(mounted.contains("Hide has not been saved on this device."));
            assert!(mounted.contains("Retry Hide"));
            assert_eq!(mounted.contains("Could not save."), has_error);
        }
    }

    #[test]
    fn close_section_and_disclosure_actions_are_keyboard_targets() {
        let model = ViewModel {
            saved: Some(DevicePreferences::default()),
            ..Default::default()
        };
        for (section, id, expected) in [
            (
                Section::Appearance,
                "tenshu.preferences.close",
                Message::Action(Action::Close),
            ),
            (
                Section::Appearance,
                Section::About.action_id(),
                Message::SelectSection(Section::About),
            ),
            (
                Section::Diagnostics,
                "tenshu.preferences.repair-details",
                Message::ToggleRegistrationRepair,
            ),
            (
                Section::About,
                "tenshu.preferences.component-versions",
                Message::ToggleComponentVersions,
            ),
        ] {
            let (focused, messages) = activate(model, section, id, keyboard::key::Named::Space);
            assert!(focused, "{}", id);
            assert_eq!(messages, vec![expected]);
        }
    }
    #[test]
    fn worldview_sharing_actions_follow_the_publisher_state_and_keyboard() {
        let model = ViewModel {
            saved: Some(DevicePreferences::default()),
            worldview_sharing: WorldviewSharing {
                status: "Shared with Keychain",
                shared_selection: Some(("Luke", "The Stone Crossing")),
                can_refresh: true,
                can_publish: true,
                can_revoke: true,
            },
            ..Default::default()
        };
        for (id, expected) in [
            (
                "tenshu.preferences.worldview-sharing.refresh",
                Action::RefreshWorldview,
            ),
            (
                "tenshu.preferences.worldview-sharing.publish",
                Action::PublishWorldview,
            ),
            (
                "tenshu.preferences.worldview-sharing.revoke",
                Action::RevokeWorldview,
            ),
        ] {
            let (focused, messages) =
                activate(model, Section::Appearance, id, keyboard::key::Named::Enter);
            assert!(focused);
            assert_eq!(messages, vec![Message::Action(expected)]);
            let disabled = ViewModel {
                worldview_sharing: WorldviewSharing::default(),
                ..model
            };
            let (_, messages) = activate(
                disabled,
                Section::Appearance,
                id,
                keyboard::key::Named::Enter,
            );
            assert!(messages.is_empty());
        }
    }
    #[test]
    fn worldview_sharing_row_fits_english_and_spanish_in_both_themes() {
        use crate::app::view::shared::landing::capture;
        for language in [Language::English, Language::Spanish] {
            for (appearance, theme) in [
                ("dark", theme::Theme::dark()),
                ("light", theme::Theme::light()),
            ] {
                coincube_ui::locale::render_in(language, || {
                    let sharing = WorldviewSharing {
                        status: localized_copy("Shared with Keychain"),
                        shared_selection: Some(("Suitcoiner", "Gilded Wall Street")),
                        can_refresh: true,
                        can_publish: true,
                        can_revoke: true,
                    };
                    let shown = capture::inspect_filled(
                        &format!("worldview-sharing-{language:?}"),
                        worldview_sharing_row(&sharing),
                        (700, 220),
                        appearance,
                        theme,
                        (700, 220),
                    );
                    assert!(shown.contains(localized_copy("Share saved selection")));
                    assert!(shown.contains("Gilded Wall Street"));
                });
            }
        }
    }
}
