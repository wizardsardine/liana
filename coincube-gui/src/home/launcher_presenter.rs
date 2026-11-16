//! v113 Launcher List and Opening presentation. The catalog, unlock and
//! loader remain the authorities for rows, actions and startup state.

use crate::app::view::responsive;
use coincube_ui::locale::copy as localized_copy;
use coincube_ui::locale::Text as CopyText;
use coincube_ui::localized_format;
use iced::{
    advanced::widget::Id,
    widget::{button as iced_button, container, rule, text::Wrapping, Space, Stack},
    Alignment, Length, Size,
};

use coincube_ui::{
    component::{
        quote_display::Quote,
        tenshu::{self, top_bar, ActionButton, ActionKind},
    },
    icon, theme,
    widget::{action::Action, text_input, Column, ColumnExt, Container, Element, Row, TextInput},
};

use super::{
    launcher_catalog::{self, CatalogMessage, Projection},
    launcher_chrome, launcher_coin, launcher_gpu, launcher_scene, CubeCatalogKey,
    DeleteCubeMessage, Home, ViewMessage,
};
use crate::{
    chain::{ChainId, ChainIdExt},
    loader,
};
use launcher_scene::BackupMark;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LauncherScene {
    pub query: String,
    /// These counts are unfiltered; search never changes quota facts.
    pub local_total: usize,
    pub remote_total: usize,
    /// Selected identity, not a row number that could move on refresh.
    pub selected: Option<CubeCatalogKey>,
}

/// Project only catalog facts that the new scene may reuse. The production
/// catalog still reconciles keys against current state and owns every
/// launch, restore and delete intent.
pub(super) fn project_catalog(catalog: &Projection<'_>) -> LauncherScene {
    LauncherScene {
        query: catalog.query.clone(),
        local_total: catalog.local_total,
        remote_total: catalog.remote_total,
        selected: catalog
            .selected
            .and_then(|index| catalog.locals.get(index).map(|row| row.key.clone())),
    }
}

const DETAILS_WIDTH: f32 = 440.0;
const PANEL_GAP: f32 = 24.0;
const LIST_MIN_WIDTH: f32 = 560.0;
const TABLE_MIN_VIEW_WIDTH: f32 = 1100.0;
const TABLE_MIN_LIST_WIDTH: f32 = 840.0;
const AVATAR_WIDTH: f32 = 44.0;
const NETWORK_WIDTH: f32 = 145.0;
const WALLETS_WIDTH: f32 = 160.0;
const BACKUP_WIDTH: f32 = 145.0;
/// The face's term column; the details snapshot sets its values at the same
/// x (28px margin + this + the 12px gap).
pub(super) const FACE_TERM_WIDTH: f32 = 84.0;
const ACTION_WIDTH: f32 = 72.0;
const CELL_GAP: f32 = 14.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WalletFaceMark {
    Vault,
    ImportedBitcoin,
    None,
}

pub(super) fn wallet_face_mark(cube: &crate::app::settings::CubeSettings) -> WalletFaceMark {
    if cube.vault_wallet_id.is_some() {
        WalletFaceMark::Vault
    } else if cube.has_imported_bitcoin() {
        // An import operation may exist before its wallet is installed.
        WalletFaceMark::ImportedBitcoin
    } else {
        WalletFaceMark::None
    }
}

// The generated avatar is not available in this pre-unlock catalog. Keep a
// stable, readable initial alongside the name in both the floor and details.
fn catalog_avatar(name: &str) -> Container<'static, ViewMessage> {
    catalog_avatar_any(name)
}

fn catalog_initial(name: &str) -> String {
    name.chars()
        .find(|ch| ch.is_alphanumeric())
        .map(|ch| ch.to_uppercase().collect::<String>())
        .unwrap_or_else(|| "?".into())
}

fn catalog_avatar_any<M: 'static>(name: &str) -> Container<'static, M> {
    Container::new(tenshu::label(catalog_initial(name)))
        .center_x(Length::Fixed(34.0))
        .center_y(Length::Fixed(34.0))
        .style(|theme: &theme::Theme| container::Style {
            background: Some(iced::Background::Color(theme.colors.shell.panel)),
            border: iced::Border {
                color: theme.colors.shell.line,
                width: 1.0,
                radius: 17.0.into(),
            },
            text_color: Some(theme.colors.shell.ink),
            ..Default::default()
        })
}

/// Standalone unlock screens use the saved appearance until the tab's live
/// resolved theme is passed by the GUI shell.
pub(crate) fn saved_unlock_theme(
    datadir_root: &std::path::Path,
) -> coincube_ui::theme::palette::ThemeMode {
    use crate::app::settings::global::GlobalSettings;
    let root = crate::dir::CoincubeDirectory::new(datadir_root.to_path_buf());
    GlobalSettings::load_theme_mode(&GlobalSettings::path(&root))
}
fn focus(id: Id) -> ViewMessage {
    ViewMessage::Catalog(CatalogMessage::Focus(id))
}

fn search_id() -> text_input::Id {
    text_input::Id::new("tenshu.launcher.catalog:control:search")
}

fn clear_id() -> Id {
    Id::from("tenshu.launcher.catalog:control:clear-query")
}

fn clear_empty_id() -> Id {
    Id::from("tenshu.launcher.catalog:control:clear-query-empty")
}

fn selected_style(
    selected: bool,
) -> impl Fn(&theme::Theme, iced_button::Status) -> iced_button::Style {
    move |theme, status| {
        let shell = theme.colors.shell;
        iced_button::Style {
            background: Some(iced::Background::Color(if selected {
                shell.selected
            } else if matches!(
                status,
                iced_button::Status::Hovered | iced_button::Status::Pressed
            ) {
                shell.hover
            } else {
                iced::Color::TRANSPARENT
            })),
            text_color: shell.ink,
            border: iced::Border {
                radius: 6.0.into(),
                ..Default::default()
            },
            ..Default::default()
        }
    }
}

/// `.hit`: the label and hit area laid over each Cube. The prototype draws no
/// box on hover or selection; the Cube itself moves.
fn floor_style() -> impl Fn(&theme::Theme, iced_button::Status) -> iced_button::Style {
    |theme, _status| iced_button::Style {
        background: None,
        text_color: theme.colors.shell.ink,
        ..Default::default()
    }
}

/// The wallets a Cube holds, read without unlocking it, in the order the
/// Cube's sidebar lists them. Spark comes with every Cube on a network it
/// supports. Liquid follows the same sunset gate as the sidebar
/// ([`crate::app::features::LiquidGate`]), which hides it for everyone since
/// Tenshu v175; the gate still records this Cube's persisted grant and a
/// Liquid wallet already on this device (`liquid_on_disk`).
pub(super) fn wallet_kinds(
    cube: &crate::app::settings::CubeSettings,
    liquid_on_disk: bool,
) -> Vec<&'static str> {
    use crate::app::{features, settings::BitcoinImportStage};
    let mut kinds = Vec::new();
    if features::spark(cube.network).is_available() {
        kinds.push("Spark");
    }
    if cube.vault_wallet_id.is_some() {
        kinds.push("Vault");
    }
    if let Some(import) = &cube.bitcoin_import {
        // A pending import is an intent, not an installed wallet.
        kinds.push(match &import.stage {
            BitcoinImportStage::Ready { .. } => "Bitcoin",
            _ => "Bitcoin (importing)",
        });
    }
    let gate = features::LiquidGate {
        server_enabled: cube.liquid_granted == Some(true),
        local_state_exists: liquid_on_disk,
    };
    if features::liquid_wallet_usable(cube.network, gate) {
        kinds.push("Liquid");
    }
    kinds
}

pub(super) fn local_wallets(
    cube: &crate::app::settings::CubeSettings,
    liquid_on_disk: bool,
) -> String {
    let kinds = wallet_kinds(cube, liquid_on_disk);
    if kinds.is_empty() {
        "No wallets yet".into()
    } else {
        kinds.join(", ")
    }
}

/// Matches the Cube's own "Backup not confirmed" warning: `backed_up` is set
/// once the user confirms the seed phrase backup. The Recovery Kit is a
/// Connect fact and shows on the Recovery Kit line. A Cube with no recorded master
/// seed (created before it was tracked, or elsewhere) may hold no seed on
/// this device at all, so its status is unknown rather than a warning.
pub(super) fn backup_mark(cube: &crate::app::settings::CubeSettings) -> BackupMark {
    if cube.backed_up {
        BackupMark::Confirmed
    } else if cube.master_signer_fingerprint.is_none() {
        BackupMark::Unknown
    } else {
        BackupMark::NotConfirmed
    }
}

pub(super) fn backup(cube: &crate::app::settings::CubeSettings) -> (&'static str, tenshu::Tone) {
    match backup_mark(cube) {
        BackupMark::Confirmed => (CopyText::Confirmed.get(), tenshu::Tone::Good),
        BackupMark::NotConfirmed => ("Not confirmed", tenshu::Tone::Warn),
        BackupMark::Unknown => ("Status unknown", tenshu::Tone::Muted),
    }
}

/// A Cube is open while this process holds its unlocked session.
pub(super) fn lock_state(open: bool) -> &'static str {
    if open {
        "Open"
    } else {
        "Locked"
    }
}

pub(super) fn unlock_prompt(cube: &crate::app::settings::CubeSettings) -> &'static str {
    if cube.is_passkey_cube() {
        "Passkey prompt"
    } else {
        "PIN prompt"
    }
}

fn text_tone(tone: tenshu::Tone) -> impl Fn(&theme::Theme) -> iced::widget::text::Style {
    move |theme| theme::tenshu::text::tone(theme, tone)
}

fn network_coin(
    chain: ChainId,
    size: f32,
    light: bool,
) -> iced::widget::Svg<'static, theme::Theme> {
    let border = if size == 18.0 { 1.0 } else { 1.5 };
    iced::widget::Svg::new(iced::widget::svg::Handle::from_memory(
        launcher_coin::widget_source(chain, size, border, light).into_bytes(),
    ))
    .width(Length::Fixed(size))
    .height(Length::Fixed(size))
}

/// `.pnet`: the 18px coin and the network's full name at 14px on a 20px
/// line, under the name on both the details and the unlock face.
fn face_network<'a, M: 'a>(chain: ChainId, light: bool) -> Element<'a, M> {
    Row::new()
        .spacing(6)
        .align_y(Alignment::Center)
        .height(Length::Fixed(20.0))
        .push(network_coin(chain, 18.0, light))
        .push(face_words(
            launcher_chrome::network_name(chain),
            14.0,
            coincube_ui::font::REGULAR,
            false,
        ))
        .into()
}

fn network_label<'a>(
    chain: ChainId,
    label: &'static str,
    size: f32,
    light: bool,
) -> Element<'a, ViewMessage> {
    Row::new()
        .spacing(6)
        .align_y(Alignment::Center)
        .push(network_coin(chain, size, light))
        .push(tenshu::caption(label).wrapping(Wrapping::WordOrGlyph))
        .into()
}

fn local_facts<'a>(
    row: &launcher_catalog::LocalRow<'_>,
    table: bool,
    light: bool,
    worldview: crate::app::settings::preferences::WorldviewMode,
) -> Element<'a, ViewMessage> {
    let network = crate::chain::worldview_chain_label(row.key.chain, worldview);
    let (backup_text, backup_tone) = backup(row.cube);
    let name =
        Column::new()
            .spacing(2)
            .width(Length::Fill)
            .push(
                tenshu::label(row.name.clone())
                    .width(Length::Fill)
                    .wrapping(Wrapping::WordOrGlyph),
            )
            .push(tenshu::caption(lock_state(row.open)))
            .push(row.sync_warning.as_ref().map(|warning| {
                tenshu::caption(warning.clone()).style(text_tone(tenshu::Tone::Warn))
            }))
            .push(row.connect_off.map(|_| launcher_catalog::connect_off_tag()));
    if table {
        Row::new()
            .spacing(CELL_GAP)
            .align_y(Alignment::Center)
            .push(Container::new(icon::cube_icon()).width(Length::Fixed(AVATAR_WIDTH)))
            .push(name)
            .push(
                Container::new(network_label(row.key.chain, network, 16.0, light))
                    .width(Length::Fixed(NETWORK_WIDTH)),
            )
            .push(
                tenshu::caption(row.wallets.clone())
                    .width(Length::Fixed(WALLETS_WIDTH))
                    .wrapping(Wrapping::WordOrGlyph),
            )
            .push(
                tenshu::body(backup_text)
                    .style(text_tone(backup_tone))
                    .width(Length::Fixed(BACKUP_WIDTH))
                    .wrapping(Wrapping::WordOrGlyph),
            )
            .into()
    } else {
        Column::new()
            .spacing(6)
            .width(Length::Fill)
            .push(
                Row::new()
                    .spacing(12)
                    .align_y(Alignment::Center)
                    .push(icon::cube_icon())
                    .push(name),
            )
            .push(network_label(row.key.chain, network, 16.0, light))
            .push(
                tenshu::caption(format!(
                    "Wallets: {} · Backup: {}",
                    row.wallets, backup_text
                ))
                .wrapping(Wrapping::WordOrGlyph),
            )
            .into()
    }
}

fn local_row<'a>(
    row: &launcher_catalog::LocalRow<'_>,
    selected: bool,
    table: bool,
    light: bool,
    worldview: crate::app::settings::preferences::WorldviewMode,
) -> Element<'a, ViewMessage> {
    let select = Action::new(
        launcher_catalog::select_id(&row.key),
        local_facts(row, table, light, worldview),
        focus,
    )
    .width(Length::Fill)
    .padding([12, 12])
    .style(selected_style(selected))
    .on_press_maybe(
        row.actionable
            .then(|| ViewMessage::Catalog(CatalogMessage::Select(row.key.clone()))),
    );
    let open: Element<'a, ViewMessage> = ActionButton::new(
        launcher_catalog::open_id(&row.key),
        localized_copy("Open"),
        ActionKind::Quiet,
        focus,
    )
    .compact()
    .on_press_maybe(
        row.actionable
            .then(|| ViewMessage::RunCatalog(row.key.clone())),
    )
    .into();
    Row::new()
        .spacing(8)
        .align_y(Alignment::Center)
        .push(select)
        .push(Container::new(open).width(if table {
            Length::Fixed(ACTION_WIDTH)
        } else {
            Length::Shrink
        }))
        .into()
}

fn detail_line<'a>(
    label: &'static str,
    value: Element<'a, ViewMessage>,
) -> Element<'a, ViewMessage> {
    Row::new()
        .spacing(16)
        .width(Length::Fill)
        .push(tenshu::caption(label).width(Length::Fixed(88.0)))
        .push(value)
        .into()
}

fn details<'a>(row: &launcher_catalog::LocalRow<'_>, light: bool) -> Element<'a, ViewMessage> {
    let (backup_text, backup_tone) = backup(row.cube);
    let mut content = Column::new()
        .spacing(14)
        .push(
            Row::new()
                .spacing(12)
                .align_y(Alignment::Center)
                .push(catalog_avatar(&row.name))
                .push(
                    tenshu::section(row.name.clone())
                        .width(Length::Fill)
                        .wrapping(Wrapping::WordOrGlyph),
                )
                .push(
                    ActionButton::new(
                        launcher_catalog::close_details_id(),
                        CopyText::Back.get(),
                        ActionKind::Quiet,
                        focus,
                    )
                    .compact()
                    .on_press(ViewMessage::Catalog(CatalogMessage::ClearSelection)),
                ),
        )
        .push(detail_line(
            CopyText::Network.get(),
            network_label(
                row.key.chain,
                crate::chain::canonical_network_name(row.key.chain),
                18.0,
                light,
            ),
        ))
        .push(detail_line(
            localized_copy("Lock"),
            tenshu::body(lock_state(row.open)).into(),
        ))
        .push(detail_line(
            localized_copy("Seed phrase"),
            tenshu::body(backup_text)
                .style(text_tone(backup_tone))
                .into(),
        ))
        .push(detail_line(
            "Vault",
            tenshu::body(row.vault.clone())
                .wrapping(Wrapping::WordOrGlyph)
                .into(),
        ))
        .push(detail_line(
            localized_copy("Wallets"),
            tenshu::body(row.wallets.clone()).into(),
        ))
        .push(detail_line(
            localized_copy("Recovery Kit"),
            tenshu::body(row.connect.1.clone())
                .wrapping(Wrapping::WordOrGlyph)
                .into(),
        ))
        .push(row.connect_off.map(|note| {
            detail_line(
                "Connect",
                tenshu::body(localized_copy(note))
                    .style(text_tone(tenshu::Tone::Muted))
                    .wrapping(Wrapping::WordOrGlyph)
                    .into(),
            )
        }));
    content = content.push(detail_line(
        localized_copy("Next step"),
        tenshu::body(unlock_prompt(row.cube)).into(),
    ));
    content = content.push(tenshu::caption(localized_copy(
        "The next screen checks access before this Cube opens.",
    )));
    content = content.push(
        Row::new()
            .spacing(8)
            .push(
                ActionButton::new(
                    launcher_catalog::details_open_id(&row.key),
                    localized_copy("Open"),
                    ActionKind::Primary,
                    focus,
                )
                .compact()
                .on_press(ViewMessage::RunCatalog(row.key.clone())),
            )
            .push(
                ActionButton::new(
                    launcher_catalog::rename_id(&row.key),
                    "Rename",
                    ActionKind::Quiet,
                    focus,
                )
                .compact()
                .on_press(ViewMessage::RenameCatalog(row.key.clone())),
            )
            .push(
                ActionButton::new(
                    launcher_catalog::delete_id(&row.key),
                    localized_copy("Delete…"),
                    ActionKind::Destructive,
                    focus,
                )
                .compact()
                .on_press(ViewMessage::DeleteCatalog(row.key.clone())),
            ),
    );
    tenshu::card(content).max_width(DETAILS_WIDTH).into()
}

/// The floor face has a fixed 420px rectangle. Keep its content deliberately
/// shorter than the List details card so actions stay on the visible face.
/// `.fc` text: sizes and tones straight from the face's CSS.
pub(crate) fn face_words<'a>(
    value: impl Into<String>,
    size: f32,
    weight: iced::Font,
    muted: bool,
) -> iced::widget::Text<'a, theme::Theme> {
    iced::widget::text(value.into())
        .size(size)
        .font(weight)
        .wrapping(Wrapping::WordOrGlyph)
        .style(move |theme: &theme::Theme| iced::widget::text::Style {
            color: Some(if muted {
                theme.colors.shell.muted
            } else {
                theme.colors.shell.ink
            }),
        })
}

/// `.pdl div`: an 84px muted term, wide enough for "Recovery Kit", a 12px
/// gap, then the value on a 22px line.
fn face_fact<'a>(label: &'static str, value: Element<'a, ViewMessage>) -> Element<'a, ViewMessage> {
    Row::new()
        .spacing(12)
        .width(Length::Fill)
        .push(
            Container::new(face_words(label, 13.0, coincube_ui::font::REGULAR, true))
                .width(Length::Fixed(FACE_TERM_WIDTH))
                .height(Length::Fixed(22.0))
                .center_y(Length::Fixed(22.0)),
        )
        .push(Container::new(value).center_y(Length::Fixed(22.0)))
        .into()
}

/// "No Cubes match these filters", the hidden count and Clear filters.
fn filtered_out<'a>(hidden: usize) -> Element<'a, ViewMessage> {
    use coincube_ui::font;
    Column::new()
        .spacing(8)
        .align_x(Alignment::Center)
        .push(face_words(
            localized_copy("No Cubes match these filters"),
            20.0,
            font::SEMIBOLD,
            false,
        ))
        .push(
            Row::new()
                .spacing(4)
                .align_y(Alignment::Center)
                .push(face_words(
                    format!(
                        "{} {} hidden.",
                        hidden,
                        if hidden == 1 { "Cube" } else { "Cubes" }
                    ),
                    14.0,
                    font::REGULAR,
                    true,
                ))
                .push(
                    Action::new(
                        launcher_catalog::clear_filters_empty_id(),
                        face_link_label(localized_copy("Clear filters")),
                        focus,
                    )
                    .padding([2.0, 4.0])
                    .style(face_link_style)
                    .on_press(ViewMessage::Catalog(CatalogMessage::Filter(
                        launcher_catalog::NetworkFilter::All,
                    ))),
                ),
        )
        .into()
}

/// `.linkbtn`: ink text with a 1px accent underline 3px below it. A Stack
/// takes its first layer's size, so the underline spans exactly the label.
pub(crate) fn face_link_label<'a, M: 'a>(label: &'static str) -> Element<'a, M> {
    Stack::new()
        .push(
            Column::new()
                .push(face_words(label, 14.0, coincube_ui::font::REGULAR, false))
                .push(Space::new().height(Length::Fixed(3.0))),
        )
        .push(
            Column::new()
                .height(Length::Fill)
                .push(Space::new().height(Length::Fill))
                .push(
                    Container::new(Space::new())
                        .width(Length::Fill)
                        .height(Length::Fixed(1.0))
                        .style(|theme: &theme::Theme| container::Style {
                            background: Some(iced::Background::Color(theme.colors.shell.accent)),
                            ..Default::default()
                        }),
                ),
        )
        .into()
}

/// `.linkbtn` has no box of its own.
pub(crate) fn face_link_style(
    _theme: &theme::Theme,
    _status: iced_button::Status,
) -> iced_button::Style {
    iced_button::Style::default()
}

fn face_link<'a>(id: Id, label: &'static str, message: ViewMessage) -> Element<'a, ViewMessage> {
    Action::new(id, face_link_label(localized_copy(label)), focus)
        .padding([2.0, 4.0])
        .style(face_link_style)
        .on_press(message)
        .into()
}

/// `.btn.primary`, hovering to `#ffa54d`; disabled at `opacity: .45`.
pub(crate) fn face_primary_style(
    theme: &theme::Theme,
    status: iced_button::Status,
) -> iced_button::Style {
    let fill = match status {
        iced_button::Status::Hovered | iced_button::Status::Pressed => iced::color!(0xffa54d),
        _ => theme.colors.shell.accent,
    };
    let alpha = if matches!(status, iced_button::Status::Disabled) {
        0.45
    } else {
        1.0
    };
    let fade = |c: iced::Color| iced::Color {
        a: c.a * alpha,
        ..c
    };
    iced_button::Style {
        background: Some(iced::Background::Color(fade(fill))),
        text_color: fade(iced::color!(0x201407)),
        border: iced::Border {
            color: fade(fill),
            width: 1.0,
            radius: 8.0.into(),
        },
        ..Default::default()
    }
}

/// A `.btn` label: 14px medium in the button's own text colour.
pub(crate) fn face_button_label<'a, M: 'a>(label: &'static str) -> Element<'a, M> {
    Container::new(
        iced::widget::text(label)
            .size(14)
            .font(coincube_ui::font::MEDIUM),
    )
    .center_y(Length::Fill)
    .into()
}

/// The unlock face below its heading: the fields, then `.pnote` and the
/// Cancel link pinned to the bottom, as the prototype's unlock `.fc`.
pub(crate) fn unlock_face_body<'a, M: 'a>(
    top: Element<'a, M>,
    cancel: Element<'a, M>,
) -> Element<'a, M> {
    Column::new()
        .height(Length::Fill)
        .push(top)
        .push(Space::new().height(Length::Fill))
        .push(cancel)
        .into()
}

/// `.btn.primary`: 36px, 16px sides, 14px medium on the accent fill.
fn face_primary<'a>(id: Id, label: &'static str, message: ViewMessage) -> Element<'a, ViewMessage> {
    Action::new(id, face_button_label(label), focus)
        .height(Length::Fixed(36.0))
        .padding([0.0, 16.0])
        .style(face_primary_style)
        .on_press(message)
        .into()
}

/// The details face, laid out as the prototype's `.fc` section: `.pn`,
/// `.pnet`, the `.pdl` facts, then `.pact` and `.pfoot` at the bottom.
fn floor_details<'a>(
    row: &launcher_catalog::LocalRow<'_>,
    light: bool,
) -> Element<'a, ViewMessage> {
    use coincube_ui::font;
    let (backup_text, backup_tone) = backup(row.cube);
    let avatar = Container::new(face_words(
        catalog_initial(&row.name),
        14.0,
        font::SEMIBOLD,
        false,
    ))
    .center_x(Length::Fixed(32.0))
    .center_y(Length::Fixed(32.0))
    .style(|theme: &theme::Theme| container::Style {
        background: Some(iced::Background::Color(theme.colors.shell.panel)),
        border: iced::Border {
            radius: 16.0.into(),
            ..Default::default()
        },
        ..Default::default()
    });
    let facts = Column::new()
        .spacing(10)
        .push(face_fact(
            "Lock",
            face_words(lock_state(row.open), 14.0, font::REGULAR, false).into(),
        ))
        .push(face_fact(
            "Seed phrase",
            face_words(backup_text, 14.0, font::REGULAR, false)
                .style(text_tone(backup_tone))
                .into(),
        ))
        .push(face_fact(
            "Recovery Kit",
            face_words(row.connect.1.clone(), 14.0, font::REGULAR, false).into(),
        ))
        .push(face_fact(
            "Wallets",
            face_words(row.wallets.clone(), 14.0, font::REGULAR, false).into(),
        ))
        // The fixed face has room for a short line only; the List details
        // carry the full explanation.
        .push(row.connect_off.map(|_| {
            face_fact(
                "Connect",
                face_words(
                    localized_copy("Off · over plan limit"),
                    14.0,
                    font::REGULAR,
                    true,
                )
                .into(),
            )
        }));
    let actions = Row::new()
        .spacing(16)
        .align_y(Alignment::Center)
        .push(face_primary(
            launcher_catalog::details_open_id(&row.key),
            "Open",
            ViewMessage::RunFloorCatalog(row.key.clone()),
        ))
        .push(face_link(
            launcher_catalog::close_details_id(),
            CopyText::Back.get(),
            ViewMessage::Catalog(CatalogMessage::ClearSelection),
        ))
        .push(face_link(
            launcher_catalog::delete_id(&row.key),
            "Delete…",
            ViewMessage::DeleteCatalog(row.key.clone()),
        ));
    Container::new(
        Column::new()
            .push(
                Row::new()
                    .spacing(10)
                    .align_y(Alignment::Center)
                    .push(avatar)
                    .push(
                        face_words(row.name.clone(), 20.0, font::SEMIBOLD, false)
                            .width(Length::Fill),
                    ),
            )
            .push(Space::new().height(Length::Fixed(8.0)))
            .push(face_network(row.key.chain, light))
            .push(Space::new().height(Length::Fixed(20.0)))
            .push(rule::horizontal(1))
            .push(Space::new().height(Length::Fixed(16.0)))
            .push(facts)
            .push(Space::new().height(Length::Fill))
            .push(actions)
            .push(Space::new().height(Length::Fixed(10.0)))
            .push(face_words(
                localized_format!(
                    "Next step · {}",
                    "Siguiente paso · {}",
                    unlock_prompt(row.cube)
                ),
                12.0,
                font::REGULAR,
                true,
            )),
    )
    .width(Length::Fixed(420.0))
    .height(Length::Fixed(420.0))
    .padding(28)
    .into()
}

/// `.btn`: the outlined 36px button, hovering to `--hover`.
fn face_secondary_style(theme: &theme::Theme, status: iced_button::Status) -> iced_button::Style {
    let shell = theme.colors.shell;
    iced_button::Style {
        background: matches!(
            status,
            iced_button::Status::Hovered | iced_button::Status::Pressed
        )
        .then_some(iced::Background::Color(shell.hover)),
        text_color: shell.ink,
        border: iced::Border {
            color: shell.line,
            width: 1.0,
            radius: 8.0.into(),
        },
        ..Default::default()
    }
}

/// `.av.ghost`: a dashed ring holding the cloud.
pub(super) fn ghost_avatar<'a, M: 'a>(size: f32) -> Element<'a, M> {
    let ring = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 32 32\"><circle cx=\"16\" cy=\"16\" r=\"15.5\" fill=\"none\" stroke=\"#fff\" stroke-dasharray=\"3 2\"/><g transform=\"translate(8 8) scale(.6667)\"><path d=\"{}\" fill=\"none\" stroke=\"#fff\" stroke-width=\"1.6\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/></g></svg>",
        launcher_scene::CLOUD
    );
    iced::widget::svg(iced::widget::svg::Handle::from_memory(ring.into_bytes()))
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .style(|theme: &theme::Theme, _| iced::widget::svg::Style {
            color: Some(theme.colors.shell.muted),
        })
        .into()
}

/// The Recovery fact on a ghost's face, and the note under the facts.
fn ghost_copy(cube: &super::RemoteCube) -> (&'static str, tenshu::Tone, &'static str) {
    let restore = launcher_catalog::remote_restore(cube);
    match restore.action {
        Some((ViewMessage::RestoreWithPhone { .. }, _)) => (
            restore.status,
            tenshu::Tone::Good,
            localized_copy("Restore brings this Cube onto this device with your phone. You'll set a PIN for it here."),
        ),
        Some(_) => (
            restore.status,
            tenshu::Tone::Good,
            localized_copy("Restore brings this Cube onto this device from its Recovery Kit. You'll set a PIN for it here."),
        ),
        None if cube.has_recovery_kit => (
            localized_copy("Recovery Kit has no Master Seed"),
            tenshu::Tone::Warn,
            localized_copy("Its Recovery Kit holds no Master Seed, so it can't be restored from here. Restore it on the device where it was backed up, or import it with its recovery phrase."),
        ),
        None => (
            localized_copy("No Recovery Kit in Connect"),
            tenshu::Tone::Warn,
            localized_copy("Connect has this Cube's registration but not its keys, so it can't be restored from here. If another device still has this Cube, open it there and add a Recovery Kit, or import it with its recovery phrase."),
        ),
    }
}

/// G-Ghost-Details: the one face a ghost turns to. Restore when Connect can
/// restore it here, else the recovery phrase; Back; and Delete from Connect,
/// which removes only the registration.
fn ghost_details<'a>(
    cube: &super::RemoteCube,
    network: ChainId,
    light: bool,
) -> Element<'a, ViewMessage> {
    use coincube_ui::font;
    let key = launcher_catalog::ghost_key(cube);
    let (backup_text, backup_tone, note) = ghost_copy(cube);
    let facts = Column::new()
        .spacing(10)
        .push(face_fact(
            "Where",
            face_words(
                localized_copy("In Connect, not on this device"),
                14.0,
                font::REGULAR,
                true,
            )
            .into(),
        ))
        .push(face_fact(
            "Recovery",
            face_words(backup_text, 14.0, font::REGULAR, false)
                .style(text_tone(backup_tone))
                .into(),
        ));
    // The primary action's ID is the details Open ID, so the face's arrival
    // focuses it as it focuses Open on a local Cube.
    let primary: Element<'a, ViewMessage> = match launcher_catalog::remote_restore(cube).action {
        Some((message, _)) => face_primary(
            launcher_catalog::details_open_id(&key),
            "Restore to this device",
            message,
        ),
        None => Action::new(
            launcher_catalog::details_open_id(&key),
            face_button_label("Import with recovery phrase…"),
            focus,
        )
        .height(Length::Fixed(36.0))
        .padding([0.0, 16.0])
        .style(face_secondary_style)
        .on_press(ViewMessage::ImportGhostWithPhrase(cube.uuid.clone()))
        .into(),
    };
    let actions = Row::new()
        .spacing(14)
        .align_y(Alignment::Center)
        .push(primary)
        .push(face_link(
            launcher_catalog::close_details_id(),
            CopyText::Back.get(),
            ViewMessage::Catalog(CatalogMessage::ClearSelection),
        ));
    // Three actions overrun the 364px face, so Delete takes its own line,
    // next to what it removes.
    let delete = Row::new()
        .spacing(8)
        .align_y(Alignment::Center)
        .push(face_link(
            launcher_catalog::remote_delete_id(&cube.uuid),
            "Delete from Connect…",
            ViewMessage::DeleteCube(DeleteCubeMessage::ShowRemoteModal(cube.uuid.clone())),
        ))
        .push(face_words(
            localized_copy("Removes the registration only; no funds are touched."),
            12.0,
            font::REGULAR,
            true,
        ));
    Container::new(
        Column::new()
            .push(
                Row::new()
                    .spacing(10)
                    .align_y(Alignment::Center)
                    .push(ghost_avatar(32.0))
                    .push(
                        face_words(
                            super::catalog_display_name(&cube.name),
                            20.0,
                            font::SEMIBOLD,
                            false,
                        )
                        .width(Length::Fill),
                    ),
            )
            .push(Space::new().height(Length::Fixed(8.0)))
            .push(
                Row::new()
                    .spacing(6)
                    .align_y(Alignment::Center)
                    .height(Length::Fixed(20.0))
                    .push(network_coin(network, 18.0, light).opacity(0.6_f32))
                    .push(face_words(
                        super::launcher_chrome::network_name(network),
                        14.0,
                        font::REGULAR,
                        false,
                    )),
            )
            .push(Space::new().height(Length::Fixed(20.0)))
            .push(rule::horizontal(1))
            .push(Space::new().height(Length::Fixed(16.0)))
            .push(facts)
            .push(Space::new().height(Length::Fixed(14.0)))
            .push(face_words(note, 13.0, font::REGULAR, true))
            .push(Space::new().height(Length::Fill))
            .push(actions)
            .push(Space::new().height(Length::Fixed(12.0)))
            .push(delete),
    )
    .width(Length::Fixed(420.0))
    .height(Length::Fixed(420.0))
    .padding(28)
    .into()
}

/// A ghost's slot: the dashed cloud avatar, its muted name and its Connect
/// state, over a hit area that turns it to its details face.
fn ghost_slot<'a>(
    cube: &super::RemoteCube,
    trouble: Option<crate::services::connect::grpc::trouble::Trouble>,
    layout: &launcher_scene::Layout,
    active: bool,
    closes_face: bool,
) -> Element<'a, ViewMessage> {
    let key = launcher_catalog::ghost_key(cube);
    let label = Row::new()
        .spacing(8)
        .align_y(Alignment::Center)
        .push(ghost_avatar(32.0))
        .push(
            Column::new()
                .spacing(2)
                .push(
                    iced::widget::text(super::catalog_display_name(&cube.name))
                        .size(14)
                        .font(coincube_ui::font::MEDIUM)
                        .wrapping(Wrapping::WordOrGlyph)
                        .style(|theme: &theme::Theme| iced::widget::text::Style {
                            color: Some(theme.colors.shell.muted),
                        }),
                )
                .push({
                    // `.st.warn`: nothing restores it here.
                    let warn =
                        launcher_catalog::ghost_kind(cube) == launcher_scene::Ghost::Unrestorable;
                    iced::widget::text(launcher_catalog::ghost_status(cube, trouble))
                        .size(13)
                        .style(move |theme: &theme::Theme| iced::widget::text::Style {
                            color: Some(if warn {
                                theme.colors.shell.warn
                            } else {
                                theme.colors.shell.muted
                            }),
                        })
                }),
        );
    let content: Element<'a, ViewMessage> = if active {
        Space::new().width(Length::Fill).height(Length::Fill).into()
    } else {
        Column::new()
            .width(Length::Fill)
            .height(Length::Fill)
            .push(Space::new().height(Length::Fill))
            .push(
                Container::new(label)
                    .width(Length::Fill)
                    .center_x(Length::Fill),
            )
            .into()
    };
    let slot = Action::new(launcher_catalog::select_id(&key), content, focus)
        .width(Length::Fixed(layout.slot()))
        .height(Length::Fixed(layout.slot_height()))
        .padding(iced::Padding {
            top: 0.0,
            right: 4.0,
            bottom: 10.0,
            left: 4.0,
        })
        .style(floor_style())
        .on_press(ViewMessage::Catalog(if closes_face {
            CatalogMessage::ClearSelection
        } else {
            CatalogMessage::SelectFloor(key.clone())
        }));
    iced::widget::mouse_area(slot)
        .on_enter(ViewMessage::Catalog(CatalogMessage::CubeHover(
            key.clone(),
            true,
        )))
        .on_exit(ViewMessage::Catalog(CatalogMessage::CubeHover(key, false)))
        .into()
}

fn remote_row<'a>(cube: &super::RemoteCube) -> Element<'a, ViewMessage> {
    let restore = launcher_catalog::remote_restore(cube);
    Row::new()
        .spacing(12)
        .align_y(Alignment::Center)
        .push(icon::cloud_arrow_down_icon())
        .push(
            Column::new()
                .spacing(4)
                .width(Length::Fill)
                .push(
                    tenshu::label(super::catalog_display_name(&cube.name))
                        .wrapping(Wrapping::WordOrGlyph),
                )
                .push(tenshu::caption(restore.status).wrapping(Wrapping::WordOrGlyph)),
        )
        .push(restore.action.map(|(message, label)| {
            ActionButton::new(
                launcher_catalog::restore_id(&cube.uuid),
                label,
                ActionKind::Secondary,
                focus,
            )
            .compact()
            .on_press(message)
        }))
        .push(
            ActionButton::new(
                launcher_catalog::remote_delete_id(&cube.uuid),
                localized_copy("Delete…"),
                ActionKind::Destructive,
                focus,
            )
            .compact()
            .on_press(ViewMessage::DeleteCube(DeleteCubeMessage::ShowRemoteModal(
                cube.uuid.clone(),
            ))),
        )
        .into()
}

fn section_heading<'a>(label: &'static str, count: usize) -> Element<'a, ViewMessage> {
    Row::new()
        .spacing(8)
        .align_y(Alignment::Center)
        .push(tenshu::label(label))
        .push(tenshu::caption(count.to_string()))
        .into()
}

fn table_header<'a>() -> Element<'a, ViewMessage> {
    Row::new()
        .spacing(CELL_GAP)
        .align_y(Alignment::Center)
        .push(Space::new().width(Length::Fixed(AVATAR_WIDTH)))
        .push(tenshu::caption("Cube").width(Length::Fill))
        .push(tenshu::caption(CopyText::Network.get()).width(Length::Fixed(NETWORK_WIDTH)))
        .push(tenshu::caption(localized_copy("Wallets")).width(Length::Fixed(WALLETS_WIDTH)))
        .push(tenshu::caption(localized_copy("Seed phrase")).width(Length::Fixed(BACKUP_WIDTH)))
        .push(Space::new().width(Length::Fixed(ACTION_WIDTH)))
        .into()
}

fn list_layout<'a>(
    projection: &Projection<'_>,
    scene: &LauncherScene,
    width: f32,
    light: bool,
    worldview: crate::app::settings::preferences::WorldviewMode,
) -> Element<'a, ViewMessage> {
    let list_width =
        if scene.selected.is_some() && width >= LIST_MIN_WIDTH + DETAILS_WIDTH + PANEL_GAP {
            width - DETAILS_WIDTH - PANEL_GAP
        } else {
            width
        };
    let table = width >= TABLE_MIN_VIEW_WIDTH && list_width >= TABLE_MIN_LIST_WIDTH;
    let search = Row::new()
        .spacing(8)
        .align_y(Alignment::Center)
        .push(
            TextInput::new(localized_copy("Search Cubes by name"), &scene.query)
                .id(search_id())
                .on_input(|query| ViewMessage::Catalog(CatalogMessage::Query(query)))
                .padding(10)
                .width(Length::Fill),
        )
        .push((!scene.query.is_empty()).then(|| {
            ActionButton::new(
                clear_id(),
                localized_copy("Clear"),
                ActionKind::Quiet,
                focus,
            )
            .compact()
            .on_press(ViewMessage::Catalog(CatalogMessage::ClearQuery))
        }));
    let mut list = Column::new().spacing(8).width(Length::Fill).push(search);
    if projection.no_matches() && projection.query.trim().is_empty() {
        list = list.push(tenshu::card(filtered_out(projection.hidden)));
    } else if projection.no_matches() {
        list = list.push(tenshu::card(
            Column::new()
                .spacing(8)
                .push(tenshu::body(localized_format!(
                    "No Cubes match “{}”.",
                    "No Cubes coinciden con “{}”.",
                    scene.query.trim()
                )))
                .push(
                    ActionButton::new(
                        clear_empty_id(),
                        localized_copy("Clear search"),
                        ActionKind::Quiet,
                        focus,
                    )
                    .compact()
                    .on_press(ViewMessage::Catalog(CatalogMessage::ClearQuery)),
                ),
        ));
    }
    if !projection.locals.is_empty() {
        list = list.push(section_heading("On this device", projection.locals.len()));
        if table {
            list = list.push(table_header());
        }
        for row in &projection.locals {
            list = list.push(local_row(
                row,
                scene.selected.as_ref() == Some(&row.key),
                table,
                light,
                worldview,
            ));
        }
    }
    if !projection.remotes.is_empty() {
        list = list.push(section_heading(
            "Available in Connect",
            projection.remotes.len(),
        ));
        for cube in projection.remotes.iter().copied() {
            list = list.push(remote_row(cube));
        }
    }
    if scene.local_total + scene.remote_total == 0 && !projection.no_matches() {
        list = list.push(tenshu::card(tenshu::body(localized_copy(
            "No Cubes on this network.",
        ))));
    }
    let details = scene
        .selected
        .as_ref()
        .and_then(|key| projection.locals.iter().find(|row| &row.key == key))
        .map(|row| details(row, light));
    match details {
        None => list.into(),
        Some(panel) if width >= LIST_MIN_WIDTH + DETAILS_WIDTH + PANEL_GAP => Row::new()
            .spacing(PANEL_GAP)
            .align_y(Alignment::Start)
            .push(list)
            .push(Container::new(panel).width(Length::Fixed(DETAILS_WIDTH)))
            .into(),
        Some(panel) => Column::new()
            .spacing(16)
            .width(Length::Fill)
            .push(panel)
            .push(list)
            .into(),
    }
}

#[cfg(test)]
pub(super) fn list_body(home: &Home) -> Element<'_, ViewMessage> {
    list_body_with_worldview(home, Default::default())
}

/// The Launcher List body, with aliases confined to passive row captions.
pub(super) fn list_body_with_worldview(
    home: &Home,
    worldview: crate::app::settings::preferences::WorldviewMode,
) -> Element<'_, ViewMessage> {
    let projection = launcher_catalog::project(home);
    let scene = project_catalog(&projection);
    let light = matches!(
        home.theme_mode,
        coincube_ui::theme::palette::ThemeMode::Light
    );
    Column::new()
        .spacing(8)
        .push(
            responsive(move |size: Size| {
                list_layout(&projection, &scene, size.width, light, worldview)
            })
            .height(Length::Shrink),
        )
        .into()
}

/// `.cb`: a fixed slot with the name and state at its foot and a hit area
/// over the whole Cube. The 3D Cube itself is painted by the GPU stage below.
fn floor_slot<'a>(
    row: &launcher_catalog::LocalRow<'_>,
    layout: &launcher_scene::Layout,
    active: bool,
    dimmed: bool,
    closes_face: bool,
) -> Element<'a, ViewMessage> {
    let label = Container::new(floor_label(
        &row.name,
        if row.actionable {
            unlock_prompt(row.cube)
        } else {
            localized_copy("Unavailable on this network")
        },
        dimmed,
    ))
    .width(Length::Fill)
    .center_x(Length::Fill);
    // `.cb.active .lbl { visibility: hidden }`.
    let content: Element<'a, ViewMessage> = if active {
        Space::new().width(Length::Fill).height(Length::Fill).into()
    } else {
        Column::new()
            .width(Length::Fill)
            .height(Length::Fill)
            .push(Space::new().height(Length::Fill))
            .push(label)
            .into()
    };
    let slot = Action::new(launcher_catalog::select_id(&row.key), content, focus)
        .width(Length::Fixed(layout.slot()))
        .height(Length::Fixed(layout.slot_height()))
        .padding(iced::Padding {
            top: 0.0,
            right: 4.0,
            bottom: 10.0,
            left: 4.0,
        })
        .style(floor_style())
        .on_press_maybe(if closes_face {
            Some(ViewMessage::Catalog(CatalogMessage::ClearSelection))
        } else {
            row.actionable
                .then(|| ViewMessage::Catalog(CatalogMessage::SelectFloor(row.key.clone())))
        });
    // A resting Cube floats while the pointer is over its slot.
    iced::widget::mouse_area(slot)
        .on_enter(ViewMessage::Catalog(CatalogMessage::CubeHover(
            row.key.clone(),
            true,
        )))
        .on_exit(ViewMessage::Catalog(CatalogMessage::CubeHover(
            row.key.clone(),
            false,
        )))
        .into()
}

/// `.lbl`: the 32px avatar, then the name (14px semibold, `.cb.dim .nm`
/// muted at weight 500) over the 13px muted state line.
fn floor_label<'a, M: 'static>(name: &str, state: &'static str, dimmed: bool) -> Row<'a, M> {
    let initial = name
        .chars()
        .find(|ch| ch.is_alphanumeric())
        .map(|ch| ch.to_uppercase().collect::<String>())
        .unwrap_or_else(|| "?".into());
    let avatar = Container::new(
        iced::widget::text(initial)
            .size(13)
            .font(coincube_ui::font::SEMIBOLD),
    )
    .center_x(Length::Fixed(32.0))
    .center_y(Length::Fixed(32.0))
    .style(|theme: &theme::Theme| container::Style {
        background: Some(iced::Background::Color(theme.colors.shell.panel)),
        border: iced::Border {
            radius: 16.0.into(),
            ..Default::default()
        },
        text_color: Some(theme.colors.shell.ink),
        ..Default::default()
    });
    let name = iced::widget::text(name.to_owned())
        .size(14)
        .font(if dimmed {
            coincube_ui::font::MEDIUM
        } else {
            coincube_ui::font::SEMIBOLD
        })
        .wrapping(Wrapping::WordOrGlyph)
        .style(move |theme: &theme::Theme| iced::widget::text::Style {
            color: Some(if dimmed {
                theme.colors.shell.muted
            } else {
                theme.colors.shell.ink
            }),
        });
    let state = iced::widget::text(state)
        .size(13)
        .style(|theme: &theme::Theme| iced::widget::text::Style {
            color: Some(theme.colors.shell.muted),
        });
    Row::new()
        .spacing(8)
        .align_y(Alignment::Center)
        .push(avatar)
        .push(Column::new().spacing(2).push(name).push(state))
}

fn face_text<'r>(row: &'r launcher_catalog::LocalRow<'_>) -> launcher_scene::FaceText<'r> {
    launcher_scene::FaceText {
        name: &row.name,
        lock: lock_state(row.open),
        backup: backup(row.cube).0,
        connect: &row.connect.1,
        wallets: &row.wallets,
        passkey: row.cube.is_passkey_cube(),
    }
}

/// `.pgdots { bottom: 16px }`.
const PAGER_BOTTOM: f32 = 16.0;

/// `.pgbtn svg`: a 22px chevron, white on dark and ink on light.
fn arrow_chevron<'a, M: 'a>(previous: bool, enabled: bool) -> Element<'a, M> {
    const LEFT: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 18 18" fill="none" stroke="#fff" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="M11 4 6 9l5 5"/></svg>"##;
    const RIGHT: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 18 18" fill="none" stroke="#fff" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="m7 4 5 5-5 5"/></svg>"##;
    // `.pgbtn[disabled] { opacity: .3 }`. Iced's SVG tint drops alpha, so
    // the disabled chevron carries its opacity in the source.
    let source = if previous { LEFT } else { RIGHT };
    let source = if enabled {
        source.to_string()
    } else {
        source.replace("stroke-width=", "stroke-opacity=\".3\" stroke-width=")
    };
    Container::new(
        iced::widget::svg(iced::widget::svg::Handle::from_memory(source.into_bytes()))
            .width(Length::Fixed(22.0))
            .height(Length::Fixed(22.0))
            .style(move |theme: &theme::Theme, _| iced::widget::svg::Style {
                color: Some(if is_light(theme) {
                    theme.colors.shell.ink
                } else {
                    iced::Color::WHITE
                }),
            }),
    )
    .center_x(Length::Fill)
    .center_y(Length::Fill)
    .into()
}

/// `.pgbtn`: a 44px square-cornered button with a muted ring and a soft drop shadow.
fn arrow_style(theme: &theme::Theme, enabled: bool, hovered: bool) -> iced_button::Style {
    let shell = theme.colors.shell;
    let alpha = if enabled { 1.0 } else { 0.3 };
    let fade = |c: iced::Color| iced::Color {
        a: c.a * alpha,
        ..c
    };
    iced_button::Style {
        background: Some(iced::Background::Color(fade(if enabled && hovered {
            shell.hover
        } else {
            shell.panel
        }))),
        text_color: shell.ink,
        border: iced::Border {
            color: fade(shell.muted),
            width: 1.0,
            radius: 8.0.into(),
        },
        // `box-shadow: 0 6px 18px var(--shadow)`.
        shadow: iced::Shadow {
            color: fade(if is_light(theme) {
                iced::Color::from_rgba8(70, 52, 30, 0.3)
            } else {
                iced::Color::from_rgba8(0, 0, 0, 0.6)
            }),
            offset: iced::Vector::new(0.0, 6.0),
            blur_radius: 18.0,
        },
        ..Default::default()
    }
}

fn page_arrow<'a>(previous: bool, target: Option<usize>) -> Element<'a, ViewMessage> {
    let id = if previous {
        "tenshu.launcher.page.previous"
    } else {
        "tenshu.launcher.page.next"
    };
    let enabled = target.is_some();
    Action::new(Id::from(id), arrow_chevron(previous, enabled), focus)
        .width(Length::Fixed(44.0))
        .height(Length::Fixed(44.0))
        .style(move |theme: &theme::Theme, status| {
            arrow_style(
                theme,
                enabled,
                matches!(status, iced_button::Status::Hovered),
            )
        })
        .on_press_maybe(target.map(|page| ViewMessage::Catalog(CatalogMessage::Page(page))))
        .into()
}

/// An arrow drawn at rest, for the unlock screen that takes no paging.
fn resting_arrow<'a, M: 'a>(previous: bool, enabled: bool) -> Element<'a, M> {
    Container::new(arrow_chevron(previous, enabled))
        .width(Length::Fixed(44.0))
        .height(Length::Fixed(44.0))
        .style(move |theme: &theme::Theme| {
            let style = arrow_style(theme, enabled, false);
            container::Style {
                background: style.background,
                border: style.border,
                shadow: style.shadow,
                ..Default::default()
            }
        })
        .into()
}

/// `.pgdots .pgtx`: 18px after the last dot (`gap: 8px; margin-left: 10px`),
/// less the 4px hit padding around each dot.
fn page_caption<'a, M: 'a>(page: usize, pages: usize) -> Element<'a, M> {
    Row::new()
        .push(Space::new().width(Length::Fixed(14.0)))
        .push(tenshu::caption(localized_format!(
            "Page {} of {} · swipe, scroll sideways or use the arrow keys", "Página {} de {} · deslizamiento, desplazamiento laterales o utilizar las teclas de flecha",
            page + 1,
            pages
        )))
        .into()
}

fn dot_style(active: bool) -> impl Fn(&theme::Theme) -> container::Style {
    move |theme: &theme::Theme| container::Style {
        background: Some(iced::Background::Color(if active {
            theme.colors.shell.accent
        } else {
            theme.colors.shell.line
        })),
        border: iced::Border {
            radius: 4.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// The paging arrows and dots over a stage, live on the floor and at rest
/// under the unlock face.
fn paging_layers<'a, M: 'a>(
    stage: Stack<'a, M, theme::Theme>,
    stage_height: f32,
    arrow_line: f32,
    arrows: [Element<'a, M>; 2],
    dots: Vec<Element<'a, M>>,
    caption: Element<'a, M>,
) -> Stack<'a, M, theme::Theme> {
    let [previous, next] = arrows;
    let mut pager = Row::new().spacing(0).align_y(Alignment::Center);
    for dot in dots {
        pager = pager.push(dot);
    }
    stage
        .push(
            // `.pgbtn`: 44px arrows centered on the resting Cubes' line.
            Container::new(
                Row::new()
                    .width(Length::Fill)
                    .align_y(Alignment::Center)
                    .push(previous)
                    .push(Space::new().width(Length::Fill))
                    .push(next),
            )
            .width(Length::Fill)
            .height(Length::Fixed(stage_height))
            .padding(iced::Padding {
                top: (arrow_line - 22.0).max(0.0),
                right: 18.0,
                bottom: 0.0,
                left: 18.0,
            }),
        )
        .push(
            Container::new(
                Column::new()
                    .height(Length::Fill)
                    .push(Space::new().height(Length::Fill))
                    .push(pager.push(caption))
                    .push(Space::new().height(Length::Fixed(PAGER_BOTTOM))),
            )
            .width(Length::Fill)
            .height(Length::Fixed(stage_height))
            .center_x(Length::Fill),
        )
}

fn is_light(theme: &theme::Theme) -> bool {
    theme.colors.shell.background.r > 0.5
}

/// `.pgdots .dot`: 8px dots 8px apart; the hit area pads around each.
/// A page dot at rest, padded like the live one.
fn resting_dot<'a, M: 'a>(active: bool) -> Element<'a, M> {
    Container::new(
        Container::new(Space::new())
            .width(Length::Fixed(8.0))
            .height(Length::Fixed(8.0))
            .style(dot_style(active)),
    )
    .padding(4)
    .into()
}

fn page_dot<'a>(index: usize, active: bool) -> Element<'a, ViewMessage> {
    let dot = Container::new(Space::new())
        .width(Length::Fixed(8.0))
        .height(Length::Fixed(8.0))
        .style(dot_style(active));
    Action::new(
        Id::from(format!("tenshu.launcher.page.dot.{index}")),
        dot,
        focus,
    )
    .padding(4)
    .style(|_theme: &theme::Theme, _status| iced_button::Style::default())
    .on_press(ViewMessage::Catalog(CatalogMessage::Page(index)))
    .into()
}

/// The stage fills the body between the bars, as `.stage { flex: 1 }`
/// does, so its midline (the Cubes' and the arrows' line) is the body's.
/// The Layout scales the Cubes down to fit a short body; only a body
/// shorter than the minimum window's (768px less both bars) scrolls.
const MIN_STAGE_HEIGHT: f32 = 600.0;

fn stage_height(available: f32) -> f32 {
    if available.is_finite() {
        available.max(MIN_STAGE_HEIGHT)
    } else {
        launcher_scene::DESIGN_STAGE_HEIGHT
    }
}

pub(super) fn empty_create_id() -> Id {
    Id::new("tenshu.launcher.empty:create")
}

/// A real stage with a clickable wireframe placeholder, separate from the
/// Connect restore ghosts and from the user's Cube inventory.
fn empty_floor<'a>(home: &'a Home, width: f32, available: f32) -> Element<'a, ViewMessage> {
    let height = stage_height(available);
    let layout = launcher_scene::Layout::new(width, height);
    let light = matches!(
        home.theme_mode,
        coincube_ui::theme::palette::ThemeMode::Light
    );
    let at_limit = launcher_catalog::project(home).at_limit();
    let key = launcher_catalog::create_placeholder_key();
    // The dashed Cube floats under the pointer as a resting Cube does; the
    // plus and the words stay put (Robert, 2026-11-03).
    let now = std::time::Instant::now();
    let lift = home.catalog.lift(&key, now);
    let words = Column::new()
        .push(face_words(
            localized_copy("Create Cube"),
            20.0,
            coincube_ui::font::SEMIBOLD,
            false,
        ))
        .push(face_words(
            if at_limit {
                localized_copy("Cube limit reached")
            } else {
                localized_copy("Click to get started")
            },
            14.0,
            coincube_ui::font::REGULAR,
            true,
        ));
    let welcome = if at_limit {
        None
    } else {
        coincube_ui::worldview_copy::VoiceLine::Welcome.get()
    };
    let words = words
        .push_maybe(welcome.map(|line| face_words(line, 14.0, coincube_ui::font::REGULAR, true)));
    // The plus sits beside the words, below the Cube, so nothing covers
    // the wireframe (Robert, 2026-11-03).
    let content = Column::new()
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .push(Space::new().height(Length::Fill))
        .push(
            Row::new()
                .spacing(16)
                .align_y(Alignment::Center)
                .push(words)
                .push(
                    Container::new(icon::plus_icon().size(28))
                        .center_x(Length::Fixed(44.0))
                        .center_y(Length::Fixed(44.0))
                        .style(move |theme: &theme::Theme| container::Style {
                            background: Some(iced::Background::Color(if at_limit {
                                theme.colors.shell.line
                            } else {
                                theme.colors.shell.accent
                            })),
                            text_color: Some(if at_limit {
                                theme.colors.shell.muted
                            } else {
                                theme::tenshu::ON_ACCENT
                            }),
                            border: iced::Border {
                                radius: 22.0.into(),
                                ..Default::default()
                            },
                            ..Default::default()
                        }),
                ),
        );
    let create = Action::new(empty_create_id(), content, focus)
        .width(Length::Fixed(layout.slot()))
        .height(Length::Fixed(layout.slot_height()))
        .padding([0.0, 4.0])
        .style(floor_style())
        .on_press_maybe((!at_limit).then_some(ViewMessage::ShowCreateCube(true)));
    // At the limit nothing opens, so nothing floats to invite a click.
    let create = iced::widget::mouse_area(create);
    let create = if at_limit {
        create
    } else {
        create
            .on_enter(ViewMessage::Catalog(CatalogMessage::CubeHover(
                key.clone(),
                true,
            )))
            .on_exit(ViewMessage::Catalog(CatalogMessage::CubeHover(key, false)))
    };
    // Network activity fills the sky above the placeholder as it does above
    // a one-Cube floor, and the placeholder floats into it the same way.
    let band = home.activity_band(light).and_then(|band| {
        let (_, sky, _, _) = launcher_scene::bounds(
            &layout,
            layout.rest_center(0, 1),
            launcher_scene::Pose::REST,
        );
        let sky = sky - launcher_scene::BOB_HEIGHT * layout.scale;
        super::launcher_activity::band_layer(&band, sky, now)
    });
    Stack::new()
        .width(Length::Fill)
        .height(Length::Fixed(height))
        .push(launcher_gpu::stage(
            launcher_gpu::StageData::create_placeholder(&layout, light, lift),
        ))
        .push(band)
        .push(
            Column::new()
                .width(Length::Fill)
                .push(Space::new().height(Length::Fixed(layout.rows_top())))
                .push(
                    Container::new(create)
                        .width(Length::Fill)
                        .center_x(Length::Fill),
                ),
        )
        .into()
}

#[allow(clippy::too_many_arguments)]
fn floor_layout<'a>(
    projection: &Projection<'_>,
    page: usize,
    width: f32,
    available: f32,
    light: bool,
    motion: Option<&launcher_scene::Motion>,
    hover: &[launcher_catalog::FloorHover],
    activity: Option<&super::launcher_activity::Band>,
) -> Element<'a, ViewMessage> {
    debug_assert!(projection.floor_len() > 0);
    use launcher_catalog::FloorEntry;
    let hover = |key: &CubeCatalogKey, now| {
        hover
            .iter()
            .find(|hover| &hover.key == key)
            .map_or(0.0, |hover| hover.lift(now))
    };
    let stage_height = stage_height(available);
    let layout = launcher_scene::Layout::new(width, stage_height);
    let per_page = launcher_catalog::floor_page_size(width);
    let pages = launcher_catalog::floor_pages(projection.floor_len(), per_page);
    let page = page.min(pages - 1);
    let entries = projection.floor_entries();
    let selected = projection
        .floor_selected()
        .and_then(|index| entries.get(index).copied());
    let selected_key = selected.map(|entry| entry.key());
    let selected_key = selected_key.as_ref();
    let visible: Vec<_> = entries
        .iter()
        .copied()
        .skip(page * per_page)
        .take(per_page)
        .map(|entry| (entry.key(), entry))
        .collect();
    let now = std::time::Instant::now();
    let pose = |key: &CubeCatalogKey| {
        let pose = match motion {
            Some(motion) => motion.pose(key, selected_key, now),
            None => launcher_scene::settled_pose(key, selected_key),
        };
        // Only a browsing floor floats; a selection settles every Cube.
        if selected_key.is_none() {
            pose.lifted(hover(key, now))
        } else {
            pose
        }
    };
    // Paint order follows the DOM and z-index: peers, `.cb.prev`, `.cb.lead`.
    let previous = motion.and_then(|m| m.previous().or(m.returning_key()));
    let rank = |key: &CubeCatalogKey| {
        if Some(key) == selected_key {
            2
        } else if Some(key) == previous {
            1
        } else {
            0
        }
    };
    let mut order: Vec<usize> = (0..visible.len()).collect();
    order.sort_by_key(|index| rank(&visible[*index].0));
    let count = visible.len();
    let ghost_text: Vec<_> = visible
        .iter()
        .map(|(_, entry)| match entry {
            FloorEntry::Ghost(cube) => Some((
                super::catalog_display_name(&cube.name),
                ghost_copy(cube).0,
                launcher_catalog::ghost_kind(cube) == launcher_scene::Ghost::Restorable,
            )),
            FloorEntry::Local(_) => None,
        })
        .collect();
    let cubes = order.iter().map(|index| {
        let (key, entry) = &visible[*index];
        let (marks, details, ghost_details) = match entry {
            FloorEntry::Local(row) => (launcher_gpu::marks(row.cube), Some(face_text(row)), None),
            FloorEntry::Ghost(cube) => (
                launcher_catalog::ghost_marks(cube),
                None,
                ghost_text[*index]
                    .as_ref()
                    .map(|(name, backup, restorable)| launcher_scene::GhostText {
                        name,
                        backup,
                        restorable: *restorable,
                    }),
            ),
        };
        launcher_gpu::StageCube {
            rest: layout.rest_center(*index, count),
            pose: pose(key),
            marks,
            details,
            ghost_details,
            unlock_name: None,
            glow: None,
        }
    });
    let mut stage = Stack::new()
        .width(Length::Fill)
        .height(Length::Fixed(stage_height))
        .push(
            Container::new(launcher_gpu::stage(launcher_gpu::StageData::new(
                &layout, light, true, cubes,
            )))
            .width(Length::Fill)
            .center_x(Length::Fill),
        );
    // A press on the stage away from an open face closes it, as Back does.
    // It sits under the slots, so a peer's slot still selects that peer.
    if selected_key.is_some_and(|key| pose(key) == launcher_scene::Pose::DETAILS) {
        stage = stage.push(
            iced::widget::mouse_area(Space::new().width(Length::Fill).height(Length::Fill))
                .on_press(ViewMessage::Catalog(CatalogMessage::ClearSelection)),
        );
    }

    // Network activity fills the sky above a browsing floor. An open face
    // rises into that sky, so a selection or a turn clears it.
    if let Some(band) = activity.filter(|_| selected.is_none() && motion.is_none()) {
        let (_, sky, _, _) = launcher_scene::bounds(
            &layout,
            layout.rest_center(0, count),
            launcher_scene::Pose::REST,
        );
        // A hovered Cube bobs up into the sky.
        let sky = sky - launcher_scene::BOB_HEIGHT * layout.scale;
        stage = stage.push(super::launcher_activity::band_layer(band, sky, now));
    }

    // `.cb.lead` and `.cb.prev` paint over their neighbours' labels.
    let covers: Vec<_> = visible
        .iter()
        .enumerate()
        .filter_map(|(index, (key, _))| {
            let pose = pose(key);
            (pose.spin > 0.0)
                .then(|| launcher_scene::bounds(&layout, layout.rest_center(index, count), pose))
        })
        .collect();
    let mut slots = Row::new().spacing(layout.gap()).align_y(Alignment::Start);
    for (index, (key, entry)) in visible.iter().enumerate() {
        let pose = pose(key);
        let covered = covers.iter().any(|cover| {
            launcher_scene::overlaps(*cover, launcher_scene::label_rect(&layout, index, count))
        });
        // The slot the open Cube left is part of the stage: a press there
        // closes the face, as a press beside it does.
        let closes_face = Some(key) == selected_key && pose == launcher_scene::Pose::DETAILS;
        slots = slots.push(match entry {
            FloorEntry::Local(row) => floor_slot(
                row,
                &layout,
                pose.spin > 0.0 || covered,
                pose.dim > 0.5,
                closes_face,
            ),
            FloorEntry::Ghost(cube) => ghost_slot(
                cube,
                projection.connect_trouble,
                &layout,
                pose.spin > 0.0 || covered,
                closes_face,
            ),
        });
    }
    stage = stage.push(
        Column::new()
            .width(Length::Fill)
            .push(Space::new().height(Length::Fixed(layout.rows_top())))
            .push(
                Container::new(slots)
                    .width(Length::Fill)
                    .center_x(Length::Fill),
            ),
    );

    if let (Some(entry), Some(key)) = (selected, selected_key) {
        // The selected Cube becomes the centered face. Its controls arrive
        // once it is square to the camera and remain real Iced widgets.
        if pose(key) == launcher_scene::Pose::DETAILS {
            let face = layout.face();
            let (_, target_y) = layout.target();
            let details = match entry {
                FloorEntry::Local(row) => floor_details(row, light),
                FloorEntry::Ghost(cube) => {
                    ghost_details(cube, launcher_catalog::ghost_key(cube).chain, light)
                }
            };
            stage = stage.push(
                Column::new()
                    .width(Length::Fill)
                    .push(Space::new().height(Length::Fixed(target_y - face / 2.0)))
                    .push(
                        // Clicks on the face never reach the Cubes behind it.
                        Container::new(iced::widget::opaque(details))
                            .width(Length::Fill)
                            .center_x(Length::Fill),
                    ),
            );
        }
    }
    // `.pgbtn` and `.pgdots` stay up under an open face, as in the prototype.
    if pages > 1 {
        stage = paging_layers(
            stage,
            stage_height,
            layout.horizon(),
            [
                page_arrow(true, page.checked_sub(1)),
                page_arrow(false, (page + 1 < pages).then_some(page + 1)),
            ],
            (0..pages)
                .map(|index| page_dot(index, index == page))
                .collect(),
            page_caption(page, pages),
        );
    }
    // `.gintro`: a new device with nothing local yet, but Connect has Cubes.
    // The ghosts stand in the row; this line says what they are.
    if projection.locals.is_empty() && selected.is_none() && motion.is_none() {
        stage = stage.push(
            Column::new()
                .width(Length::Fill)
                .push(Space::new().height(Length::Fixed(GHOST_INTRO_TOP)))
                .push(
                    Container::new(
                        Container::new(
                            face_words(GHOST_INTRO, 14.0, coincube_ui::font::REGULAR, true)
                                .align_x(Alignment::Center),
                        )
                        .max_width(520.0),
                    )
                    .width(Length::Fill)
                    .center_x(Length::Fill),
                ),
        );
    }
    Column::new()
        .spacing(0)
        .width(Length::Fill)
        .push(stage.clip(true))
        .into()
}

/// `.gintro { top: 118px }` and its copy.
const GHOST_INTRO_TOP: f32 = 118.0;
pub(super) const GHOST_INTRO: &str = "These Cubes are in COINCUBE | Connect. Select one with a Recovery Kit to restore it here, or create a new Cube.";

#[cfg(test)]
pub(super) fn floor_body(home: &Home) -> Element<'_, ViewMessage> {
    floor_body_in(home, f32::INFINITY)
}

#[cfg(test)]
pub(super) fn floor_body_in(home: &Home, available: f32) -> Element<'_, ViewMessage> {
    floor_body_with_worldview(home, available, Default::default())
}

/// The floor in a body `available` pixels tall, so the stage meets the dock.
pub(super) fn floor_body_with_worldview(
    home: &Home,
    available: f32,
    worldview: crate::app::settings::preferences::WorldviewMode,
) -> Element<'_, ViewMessage> {
    let projection = launcher_catalog::project(home);
    if !projection.floor_eligible(home.catalog.mode) {
        return list_body_with_worldview(home, worldview);
    }
    if projection.shows_create_placeholder(home.catalog.mode) {
        return responsive(move |size: Size| empty_floor(home, size.width, available))
            .height(Length::Shrink)
            .into();
    }
    // P-Launcher's `.empty` state: every Cube is filtered out. Placed at
    // `top: 32%` of the stage, where the floor would be.
    if projection.floor_len() == 0 {
        let height = if available.is_finite() {
            available
        } else {
            launcher_scene::DESIGN_STAGE_HEIGHT
        };
        return Container::new(filtered_out(projection.floor_hidden()))
            .width(Length::Fill)
            .height(Length::Fixed(height))
            .padding(iced::Padding {
                top: height * 0.32,
                ..iced::Padding::ZERO
            })
            .center_x(Length::Fill)
            .into();
    }
    let page = home.catalog.page;
    let light = matches!(
        home.theme_mode,
        coincube_ui::theme::palette::ThemeMode::Light
    );
    let motion = home.catalog.motion.clone();
    let hover = home.catalog.hover.clone();
    let activity = home.activity_band(light);
    let scene = project_catalog(&projection);
    let floor = responsive(move |size: Size| {
        if size.width < 840.0 {
            list_layout(&projection, &scene, size.width, light, worldview)
        } else {
            floor_layout(
                &projection,
                page,
                size.width,
                available,
                light,
                motion.as_ref(),
                &hover,
                activity.as_ref(),
            )
        }
    })
    .height(Length::Shrink);
    // The page size follows the laid-out width; report it so keyboard,
    // wheel and dot paging count the same Cubes a page as the screen.
    iced::widget::sensor(floor)
        .on_resize(|size| ViewMessage::Catalog(CatalogMessage::StageWidth(size.width)))
        .into()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpeningScene {
    cube_name: String,
    network: ChainId,
}

/// The loader's Step remains the only source of progress and errors. The
/// scene projects just the safe identity displayed above its existing body.
pub(crate) fn project_opening(name: &str, network: ChainId) -> OpeningScene {
    OpeningScene {
        cube_name: crate::cube_display::name(name),
        network,
    }
}

pub(crate) fn unlock_turn_progress(started: std::time::Instant, now: std::time::Instant) -> f32 {
    launcher_scene::second_turn_progress(started, now)
}

/// A lone Cube turning from its details face to the unlock face, for the
/// unlock screens that have no Launcher page behind them.
pub(crate) fn unlock_turn<'a, M: 'a>(
    cube: &crate::app::settings::CubeSettings,
    mode: coincube_ui::theme::palette::ThemeMode,
    progress: f32,
) -> Element<'a, M> {
    lone_cube(cube, mode, launcher_scene::Pose::unlock(progress))
}

fn lone_cube<'a, M: 'a>(
    cube: &crate::app::settings::CubeSettings,
    mode: coincube_ui::theme::palette::ThemeMode,
    pose: launcher_scene::Pose,
) -> Element<'a, M> {
    let light = matches!(mode, coincube_ui::theme::palette::ThemeMode::Light);
    let cube = cube.clone();
    // No Launcher behind this screen, so no probe of Liquid on disk: the
    // persisted grant alone decides, for the moment the face is in view.
    let wallets = local_wallets(&cube, false);
    responsive(move |size: Size| {
        let layout = launcher_scene::Layout::new(size.width, size.height);
        let name = crate::cube_display::name(&cube.name);
        launcher_gpu::stage(launcher_gpu::StageData::new(
            &layout,
            light,
            false,
            [launcher_gpu::StageCube {
                rest: layout.target(),
                pose,
                marks: launcher_gpu::marks(&cube),
                details: Some(launcher_scene::FaceText {
                    name: &name,
                    lock: lock_state(false),
                    backup: backup(&cube).0,
                    connect: "Checking…",
                    wallets: &wallets,
                    passkey: cube.is_passkey_cube(),
                }),
                ghost_details: None,
                unlock_name: Some(&name),
                glow: None,
            }],
        ))
    })
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

pub(crate) fn unlock_turn_with_controls<'a, M: 'a>(
    cube: &crate::app::settings::CubeSettings,
    mode: coincube_ui::theme::palette::ThemeMode,
    progress: Option<f32>,
    controls: Element<'a, M>,
) -> Element<'a, M> {
    let visual: Element<'a, M> = match progress {
        Some(progress) => unlock_turn(cube, mode, progress),
        None => Space::new().width(Length::Fill).height(Length::Fill).into(),
    };
    Stack::new()
        .width(Length::Fill)
        .height(Length::Fill)
        .push(controls)
        .push(visual)
        .into()
}

/// The resting unlock face shared by the real PIN and passkey screens. Their
/// existing widgets and messages stay inside `controls`; this only supplies
/// the Launcher stage around them.
/// The resting unlock face shared by the real PIN and passkey screens. Their
/// existing widgets and messages stay inside `controls`; this only supplies
/// the Launcher stage around them.
pub(crate) fn unlock_face<'a, M: 'a>(
    cube: &'a crate::app::settings::CubeSettings,
    mode: coincube_ui::theme::palette::ThemeMode,
    controls: impl Fn() -> Element<'a, M> + 'a,
) -> Element<'a, M> {
    // The tab hands off to the established PIN/passkey state immediately.
    // Render that state's live widgets on the second (270°) square face,
    // placed where `lone_cube` draws it: at the scene's target, not the
    // window's center, and at the face's own size.
    let light = matches!(mode, coincube_ui::theme::palette::ThemeMode::Light);
    Stack::new()
        .width(Length::Fill)
        .height(Length::Fill)
        .push(lone_cube(cube, mode, launcher_scene::Pose::unlock(1.0)))
        .push(
            responsive(move |size: Size| {
                let layout = launcher_scene::Layout::new(size.width, size.height);
                let face = layout.face();
                let (_, target_y) = layout.target();
                Column::new()
                    .width(Length::Fill)
                    .push(Space::new().height(Length::Fixed(target_y - face / 2.0)))
                    .push(
                        Container::new(unlock_controls(cube, controls(), face, light))
                            .width(Length::Fill)
                            .center_x(Length::Fill),
                    )
                    .into()
            })
            .width(Length::Fill)
            .height(Length::Fill),
        )
        .into()
}

/// The live PIN or passkey controls laid out inside the unlock face, `side`
/// square: the size the scene draws that face at.
fn unlock_controls<'a, M: 'a>(
    cube: &crate::app::settings::CubeSettings,
    controls: Element<'a, M>,
    side: f32,
    light: bool,
) -> Element<'a, M> {
    // `.pn`: "Open <name>" at 20px semibold across the whole face, then the
    // network on its own `.pnet` line as on the details face. Side by side,
    // a long name left the network a sliver that broke glyph by glyph and
    // pushed the PIN field, Unlock and Cancel off the face.
    let heading = Column::new()
        .push(
            face_words(
                localized_format!("Open {}", "Abrir {}", crate::cube_display::name(&cube.name)),
                20.0,
                coincube_ui::font::SEMIBOLD,
                false,
            )
            .width(Length::Fill),
        )
        .push(Space::new().height(Length::Fixed(8.0)))
        .push(face_network(cube.network, light));
    Container::new(Column::new().push(heading).push(controls))
        .width(Length::Fixed(side))
        .height(Length::Fixed(side))
        .padding(28)
        .into()
}

fn unlock_floor_stage<'a, M: 'a + 'static>(
    context: &launcher_catalog::LauncherSceneContext,
    cube: &crate::app::settings::CubeSettings,
    mode: coincube_ui::theme::palette::ThemeMode,
    size: Size,
    controls: Option<Element<'a, M>>,
    progress: Option<f32>,
) -> Element<'a, M> {
    let layout = floor_stage_layout(size);
    let overlay = match (progress, controls) {
        (None, Some(controls)) => {
            let face = layout.face();
            let (_, target_y) = layout.target();
            Some((
                target_y - face / 2.0,
                iced::widget::opaque(unlock_controls(
                    cube,
                    controls,
                    face,
                    matches!(mode, coincube_ui::theme::palette::ThemeMode::Light),
                )),
            ))
        }
        _ => None,
    };
    floor_stage(
        context,
        cube,
        mode,
        size,
        FloorLead {
            pose: launcher_scene::Pose::unlock(progress.unwrap_or(1.0)),
            peer: launcher_scene::Pose::DIMMED,
            glow: None,
            faces: true,
            paging: true,
            overlay,
        },
    )
}

/// The Launcher body the stage fills: the window less both bars and their
/// 1px rules, so the floor does not move at the handoff.
fn floor_stage_layout(size: Size) -> launcher_scene::Layout {
    launcher_scene::Layout::new(
        size.width,
        stage_height(size.height - 2.0 * launcher_chrome::BAR_HEIGHT - 2.0),
    )
}

/// The Cube on its way into the Cube, over the page it was picked on.
struct FloorLead<'a, M> {
    pose: launcher_scene::Pose,
    peer: launcher_scene::Pose,
    glow: Option<launcher_gpu::Glow>,
    /// The details and unlock face art, while those faces are in view.
    faces: bool,
    /// The page arrows and dots stay up under the unlock face.
    paging: bool,
    /// Live widgets over the lead's face, from this many pixels down.
    overlay: Option<(f32, Element<'a, M>)>,
}

fn floor_stage<'a, M: 'a + 'static>(
    context: &launcher_catalog::LauncherSceneContext,
    cube: &crate::app::settings::CubeSettings,
    mode: coincube_ui::theme::palette::ThemeMode,
    size: Size,
    lead: FloorLead<'a, M>,
) -> Element<'a, M> {
    let light = matches!(mode, coincube_ui::theme::palette::ThemeMode::Light);
    let layout = floor_stage_layout(size);
    let stage_height = layout.height;
    let count = context.peers.len();
    let name = crate::cube_display::name(&cube.name);
    let selected_pose = lead.pose;
    // Peers first, the turning Cube last, as on the Launcher page.
    let mut order: Vec<usize> = (0..count).collect();
    order.sort_by_key(|index| *index == context.selected_slot);
    let cubes = order.into_iter().map(|index| {
        let peer = &context.peers[index];
        let selected = index == context.selected_slot && peer.key == context.selected;
        let faces = selected && lead.faces;
        launcher_gpu::StageCube {
            rest: layout.rest_center(index, count),
            pose: if selected { selected_pose } else { lead.peer },
            marks: peer.marks,
            details: faces.then_some(launcher_scene::FaceText {
                name: &name,
                lock: lock_state(false),
                backup: context.backup,
                connect: &context.connect,
                wallets: &context.wallets,
                passkey: cube.is_passkey_cube(),
            }),
            ghost_details: None,
            unlock_name: faces.then_some(name.as_str()),
            glow: lead.glow.filter(|_| selected),
        }
    });
    let mut stage = Stack::new()
        .width(Length::Fill)
        .height(Length::Fixed(stage_height))
        .push(
            Container::new(launcher_gpu::stage(launcher_gpu::StageData::new(
                &layout, light, true, cubes,
            )))
            .width(Length::Fill)
            .center_x(Length::Fill),
        );
    let mut labels = Row::new().spacing(layout.gap()).align_y(Alignment::Start);
    for (index, peer) in context.peers.iter().enumerate() {
        let selected = index == context.selected_slot && peer.key == context.selected;
        let label: Element<'a, M> = if selected {
            Space::new().into()
        } else {
            floor_label(&peer.name, peer.status, true).into()
        };
        let covered = launcher_scene::overlaps(
            launcher_scene::bounds(
                &layout,
                layout.rest_center(context.selected_slot, count),
                selected_pose,
            ),
            launcher_scene::label_rect(&layout, index, count),
        );
        labels = labels.push(
            Container::new(if covered { Space::new().into() } else { label })
                .width(Length::Fixed(layout.slot()))
                .height(Length::Fixed(layout.slot_height()))
                .padding(iced::Padding {
                    top: 0.0,
                    right: 4.0,
                    bottom: 10.0,
                    left: 4.0,
                })
                .align_x(iced::alignment::Horizontal::Center)
                .align_y(iced::alignment::Vertical::Bottom),
        );
    }
    stage = stage.push(
        Column::new()
            .width(Length::Fill)
            .push(Space::new().height(Length::Fixed(layout.rows_top())))
            .push(
                Container::new(labels)
                    .width(Length::Fill)
                    .center_x(Length::Fill),
            ),
    );
    if let Some((top, overlay)) = lead.overlay {
        stage = stage.push(
            Column::new()
                .width(Length::Fill)
                .push(Space::new().height(Length::Fixed(top)))
                .push(
                    Container::new(overlay)
                        .width(Length::Fill)
                        .center_x(Length::Fill),
                ),
        );
    }
    if lead.paging && context.pages > 1 {
        let page = context.page;
        let pages = context.pages;
        stage = paging_layers(
            stage,
            stage_height,
            layout.horizon(),
            [
                resting_arrow(true, page > 0),
                resting_arrow(false, page + 1 < pages),
            ],
            (0..pages).map(|index| resting_dot(index == page)).collect(),
            page_caption(page, pages),
        );
    }
    // `.top.quiet` and `.bottom.quiet`: the Launcher's own bars, at rest.
    Column::new()
        .width(Length::Fill)
        .height(Length::Fill)
        .push(launcher_chrome::quiet_top_bar(&context.chrome))
        .push(rule::horizontal(1))
        .push(stage.clip(true))
        .push(Space::new().height(Length::Fill))
        .push(rule::horizontal(1))
        .push(launcher_chrome::quiet_dock(&context.chrome))
        .into()
}

pub(crate) fn unlock_face_on_floor<'a, M: 'a + 'static>(
    cube: &'a crate::app::settings::CubeSettings,
    mode: coincube_ui::theme::palette::ThemeMode,
    context: &'a launcher_catalog::LauncherSceneContext,
    controls: impl Fn() -> Element<'a, M> + 'a,
) -> Element<'a, M> {
    responsive(move |size: Size| {
        unlock_floor_stage(context, cube, mode, size, Some(controls()), None)
    })
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

pub(crate) fn unlock_turn_with_floor_controls<'a, M: 'a + 'static>(
    cube: &'a crate::app::settings::CubeSettings,
    mode: coincube_ui::theme::palette::ThemeMode,
    context: &'a launcher_catalog::LauncherSceneContext,
    progress: Option<f32>,
    controls: Element<'a, M>,
) -> Element<'a, M> {
    // Keep the same root and child shape after the turn. Iced diffs widget
    // state by position, so replacing this Stack with bare controls drops the
    // focused PIN field or passkey action at the animation boundary.
    let visual: Element<'a, M> = match progress {
        Some(progress) => responsive(move |size: Size| {
            unlock_floor_stage(context, cube, mode, size, None, Some(progress))
        })
        .width(Length::Fill)
        .height(Length::Fill)
        .into(),
        None => Space::new().width(Length::Fill).height(Length::Fill).into(),
    };
    Stack::new()
        .width(Length::Fill)
        .height(Length::Fill)
        .push(controls)
        .push(visual)
        .into()
}

/// What the loading face says: its heading, the loader's quote, the stage
/// line and, once a stage has held past the wait, Cancel (or, waiting on
/// Spark, going on without it).
pub(crate) struct OpeningFace<'a, M> {
    pub cube: &'a crate::app::settings::CubeSettings,
    pub quote: &'a Quote,
    /// The previous quote while it crossfades into `quote`, and the
    /// crossfade's progress from 0 to 1.
    pub outgoing: Option<(&'a Quote, f32)>,
    pub stage: super::launcher_opening::Stage,
    pub stalled: bool,
    pub cancel: fn() -> M,
    pub without_spark: fn() -> M,
}

#[derive(Debug, Clone, Copy)]
enum Tone {
    Ink,
    Muted,
    Good,
}

/// Face text that fades in with the loading face.
fn opening_words<'a>(
    value: impl Into<String>,
    size: f32,
    weight: iced::Font,
    tone: Tone,
    alpha: f32,
) -> iced::widget::Text<'a, theme::Theme> {
    iced::widget::text(value.into())
        .size(size)
        .font(weight)
        .wrapping(Wrapping::WordOrGlyph)
        .style(move |theme: &theme::Theme| iced::widget::text::Style {
            color: Some(
                match tone {
                    Tone::Ink => theme.colors.shell.ink,
                    Tone::Muted => theme.colors.shell.muted,
                    Tone::Good => theme.colors.shell.good,
                }
                .scale_alpha(alpha),
            ),
        })
}

/// A loading quote and its attribution.
fn opening_quote<'a, M: 'a>(quote: &Quote, alpha: f32, quote_width: f32) -> Column<'a, M> {
    use coincube_ui::font;
    Column::new()
        .align_x(Alignment::Center)
        .push(
            Container::new(
                opening_words(
                    quote.localized_text().to_owned(),
                    22.0,
                    font::REGULAR,
                    Tone::Ink,
                    alpha,
                )
                .line_height(1.5)
                .align_x(iced::alignment::Horizontal::Center),
            )
            .max_width(quote_width),
        )
        .push(Space::new().height(Length::Fixed(12.0)))
        .push(
            opening_words(
                format!("— {}, {}", quote.author, quote.source),
                13.0,
                font::REGULAR,
                Tone::Muted,
                alpha,
            )
            .align_x(iced::alignment::Horizontal::Center),
        )
}

/// The current quote, crossfading from the last one while `outgoing` is set.
fn opening_quotes<'a, M: 'a>(
    face: &OpeningFace<'a, M>,
    alpha: f32,
    quote_width: f32,
) -> Element<'a, M> {
    let Some((last, t)) = face.outgoing else {
        return opening_quote(face.quote, alpha, quote_width).into();
    };
    let incoming = opening_quote(face.quote, alpha * t, quote_width);
    let outgoing = opening_quote(last, alpha * (1.0 - t), quote_width);
    // The longer quote sizes the stack, and the shorter is centred over it,
    // so neither is cut off mid-fade.
    let (base, over) = if last.localized_text().len() > face.quote.localized_text().len() {
        (outgoing, incoming)
    } else {
        (incoming, outgoing)
    };
    Stack::new()
        .push(base)
        .push(Container::new(over).center(Length::Fill))
        .into()
}

/// `.lhead`, `.lquote`, `.lstage` and `.lslow`: the loading face's content.
fn opening_content<'a, M: 'a>(
    face: &OpeningFace<'a, M>,
    light: bool,
    alpha: f32,
    quote_width: f32,
    artwork_size: f32,
) -> (Element<'a, M>, Element<'a, M>) {
    use coincube_ui::font;
    let network = face.cube.network;
    let heading = Row::new()
        .spacing(8)
        .align_y(Alignment::Center)
        .push(opening_words(
            CopyText::Opening.get(),
            14.0,
            font::REGULAR,
            Tone::Muted,
            alpha,
        ))
        .push(opening_words(
            crate::cube_display::name(&face.cube.name),
            14.0,
            font::SEMIBOLD,
            Tone::Ink,
            alpha,
        ))
        .push(opening_words("·", 14.0, font::REGULAR, Tone::Muted, alpha))
        .push(network_coin(network, 18.0, light).opacity(alpha))
        .push(opening_words(
            launcher_chrome::network_name(network),
            14.0,
            font::REGULAR,
            Tone::Muted,
            alpha,
        ));
    let ready = face.stage == super::launcher_opening::Stage::Ready;
    let mut body = Column::new()
        .align_x(Alignment::Center)
        .push(coincube_ui::illustration::opening_image(
            artwork_size,
            alpha,
        ))
        .push(Space::new().height(Length::Fixed(16.0)))
        .push(opening_quotes(face, alpha, quote_width))
        .push(Space::new().height(Length::Fixed(40.0)))
        .push(
            opening_words(
                face.stage.text(),
                15.0,
                if ready { font::MEDIUM } else { font::REGULAR },
                if ready { Tone::Good } else { Tone::Muted },
                alpha,
            )
            .line_height(iced::widget::text::LineHeight::Absolute(22.0.into())),
        );
    use super::launcher_opening::Stage;
    if !face.stalled
        && matches!(
            face.stage,
            Stage::Scanning | Stage::Syncing | Stage::AlmostThere
        )
    {
        if let Some(line) = coincube_ui::worldview_copy::VoiceLine::Syncing.get() {
            body = body.push(opening_words(line, 13.0, font::REGULAR, Tone::Muted, alpha));
        }
    }
    if face.stalled {
        let (note, label, action) = if face.stage == super::launcher_opening::Stage::Spark {
            (
                CopyText::SparkOpeningSlow.get(),
                CopyText::ContinueWithoutSpark.get(),
                face.without_spark,
            )
        } else {
            (
                CopyText::OpeningSlow.get(),
                CopyText::Cancel.get(),
                face.cancel,
            )
        };
        body = body.push(Space::new().height(Length::Fixed(18.0))).push(
            Row::new()
                .spacing(4)
                .align_y(Alignment::Center)
                .push(opening_words(note, 13.0, font::REGULAR, Tone::Muted, alpha))
                .push({
                    // A button's message must be `Clone`; the caller's need
                    // not be, so the press maps through `action`.
                    let press: Element<'a, ()> = iced::widget::button(face_link_label(label))
                        .padding([2.0, 4.0])
                        .style(face_link_style)
                        .on_press(())
                        .into();
                    press.map(move |()| action())
                }),
        );
    }
    (heading.into(), body.into())
}

/// Give the heading its own band. At high zoom the body can scroll
/// independently if a long quote or a stalled stage needs more height.
fn opening_details<'a, M: 'a>(
    heading: Element<'a, M>,
    body: Element<'a, M>,
    top: f32,
    horizontal: f32,
) -> Element<'a, M> {
    let content = Container::new(body)
        .padding([0.0, horizontal])
        .center_x(Length::Fill);
    Column::new()
        .width(Length::Fill)
        .height(Length::Fill)
        .push(Space::new().height(Length::Fixed(top)))
        .push(Container::new(heading).center_x(Length::Fill))
        .push(Space::new().height(Length::Fixed(12.0)))
        .push(
            Container::new(iced::widget::scrollable(content).height(Length::Shrink))
                .center_x(Length::Fill)
                .center_y(Length::Fill),
        )
        .into()
}

fn opening_art_size(height: f32) -> f32 {
    (height - 430.0).clamp(96.0, 160.0)
}

fn opening_face_content<'a, M: 'a>(
    face: &OpeningFace<'a, M>,
    light: bool,
    alpha: f32,
    side: f32,
) -> Element<'a, M> {
    let (heading, body) = opening_content(face, light, alpha, 520.0, opening_art_size(side));
    let k = side / launcher_scene::OPEN_FACE_SIZE;
    Container::new(opening_details(heading, body, 44.0 * k, 72.0 * k))
        .width(Length::Fixed(side))
        .height(Length::Fixed(side))
        .into()
}

/// O-Launcher's loading phase: the unlocked Cube tumbles to its bottom face
/// on the page it was picked from, grows to the loading face and powers on.
pub(crate) fn opening_on_floor<'a, M: 'a + 'static>(
    context: &'a launcher_catalog::LauncherSceneContext,
    face: OpeningFace<'a, M>,
    mode: coincube_ui::theme::palette::ThemeMode,
    opening: &'a super::launcher_opening::Opening,
    now: std::time::Instant,
) -> Element<'a, M> {
    let light = matches!(mode, coincube_ui::theme::palette::ThemeMode::Light);
    responsive(move |size: Size| {
        // At high zoom there is not enough vertical room for the loading face
        // above the floor chrome. Use the full-window presentation in that space.
        if size.height < 620.0 {
            return opening_full_screen(OpeningFace { ..face }, mode, opening, now);
        }
        let layout = floor_stage_layout(size);
        let side = layout.open_face();
        let alpha = opening.content(now);
        let overlay = (alpha > 0.0).then(|| {
            (
                layout.open_target().1 - side / 2.0,
                opening_face_content(&face, light, alpha, side),
            )
        });
        floor_stage(
            context,
            face.cube,
            mode,
            size,
            FloorLead {
                pose: opening.pose(now),
                peer: opening.peer_pose(now),
                glow: Some(opening.glow(launcher_scene::glow(face.cube.network), now)),
                faces: false,
                paging: false,
                overlay,
            },
        )
    })
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// O-List: the same content full screen under the Opening rail, inside the
/// stage's own glowing frame. No tumble, and no finale.
pub(crate) fn opening_full_screen<'a, M: 'a + 'static>(
    face: OpeningFace<'a, M>,
    mode: coincube_ui::theme::palette::ThemeMode,
    opening: &'a super::launcher_opening::Opening,
    now: std::time::Instant,
) -> Element<'a, M> {
    let light = matches!(mode, coincube_ui::theme::palette::ThemeMode::Light);
    let glow = opening.glow(launcher_scene::glow(face.cube.network), now);
    let alpha = opening.content(now);
    let scene = project_opening(&face.cube.name, face.cube.network);
    let stage = responsive(move |size: Size| {
        let (heading, body) =
            opening_content(&face, light, alpha, 640.0, opening_art_size(size.height));
        let top = (size.height * 0.09).min(64.0);
        Stack::new()
            .width(Length::Fill)
            .height(Length::Fill)
            .push(launcher_gpu::stage(launcher_gpu::StageData::frame(
                size.width,
                size.height,
                light,
                glow,
            )))
            .push(opening_details(heading, body, top, 48.0))
            .into()
    })
    .width(Length::Fill)
    .height(Length::Fill);
    Column::new()
        .width(Length::Fill)
        .height(Length::Fill)
        .push(opening_rail(&scene))
        .push(rule::horizontal(1))
        .push(stage)
        .into()
}

/// `.flash.flat`: the whole window washed in the edge colour. It takes the
/// pointer while it shows, so nothing under it is clicked mid-finale.
pub(crate) fn opening_wash<'a, M: 'a>(network: ChainId, alpha: f32) -> Element<'a, M> {
    let [r, g, b] = launcher_scene::glow(network);
    iced::widget::opaque(
        Container::new(Space::new().width(Length::Fill).height(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |_: &theme::Theme| container::Style {
                background: Some(iced::Background::Color(
                    iced::Color::from_rgb8(r, g, b).scale_alpha(alpha),
                )),
                ..Default::default()
            }),
    )
}

/// The widest the Opening rail's "Opening <name>" may grow before it wraps.
const OPENING_RAIL_NAME: f32 = 560.0;

/// `.top.quiet` while a Cube opens: the shared bar, at rest, with the Cube's
/// name after the brand. Nothing on it is a control until the Cube is open.
fn opening_rail<'a, M: 'a>(scene: &OpeningScene) -> Element<'a, M> {
    top_bar::top_bar(
        top_bar::brand_mark(true),
        top_bar::resting_icon(top_bar::SLIDERS, top_bar::APP_PREFERENCES),
    )
    // Capped, so a long name wraps inside the bar instead of pushing the
    // network and the glyph out of it. Not Fill: the bar's spacer would
    // split the row with it and strand the network mid-bar.
    .lead(
        Container::new(
            tenshu::body(
                CopyText::OpeningCube
                    .get()
                    .replacen("{cube_name}", &scene.cube_name, 1),
            )
            .wrapping(Wrapping::WordOrGlyph),
        )
        .max_width(OPENING_RAIL_NAME),
    )
    .lead(tenshu::caption(scene.network.label()))
    .into()
}

/// Places the real loader stage under the quiet Opening rail. The loader body
/// owns scan guidance, backend diagnostics and the error action set; rendering
/// it from the same Step prevents this scene from granting a new Retry route.
pub(crate) fn opening_body<'a>(
    scene: OpeningScene,
    step: &'a loader::Step,
    quote: &'a Quote,
    image: &'a iced::widget::image::Handle,
    restricted_claim: bool,
) -> Element<'a, loader::ViewMessage> {
    let rail = opening_rail(&scene);
    let stage = Container::new(loader::view_with_retry(
        step,
        quote,
        image,
        !restricted_claim,
    ))
    .width(Length::Fill)
    .height(Length::Fill)
    .center_x(Length::Fill)
    .center_y(Length::Fill);
    Column::new()
        .width(Length::Fill)
        .height(Length::Fill)
        .push(rail)
        .push(rule::horizontal(1))
        .push(stage)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::view::shared::landing::capture;
    use crate::dir::CoincubeDirectory;
    use coincube_core::miniscript::bitcoin::Network;
    use coincube_ui::font;
    use iced::advanced::{
        layout as widget_layout,
        renderer::Headless,
        widget::{operation::Focusable, Operation, Tree},
        Layout,
    };
    use iced::Rectangle;
    use launcher_catalog::LauncherMode;
    use std::path::PathBuf;

    #[test]
    fn worldview_voice_survives_deferred_opening_layout_and_yields_to_stall_guidance() {
        use super::super::launcher_opening::{Opening, Stage};
        use coincube_ui::{theme::WorldviewMode, worldview_copy};

        let cube = crate::app::settings::CubeSettings::new_with_raw_id(
            "voice-opening".into(),
            "Everyday".into(),
            ChainId::Bitcoin,
        );
        let floor = floor_context(&cube);
        let quote = coincube_ui::component::quote_display::random_quote("loading");
        let now = std::time::Instant::now();
        let opening = Opening::new(Some(floor.clone()), true, Stage::Syncing, now);
        for stalled in [false, true] {
            let view = worldview_copy::render_in(WorldviewMode::Luke, || {
                opening_on_floor(
                    &floor,
                    OpeningFace {
                        cube: &cube,
                        quote: &quote,
                        outgoing: None,
                        stage: Stage::Syncing,
                        stalled,
                        cancel: || (),
                        without_spark: || (),
                    },
                    coincube_ui::theme::palette::ThemeMode::Dark,
                    &opening,
                    now,
                )
            });
            let mounted = capture::inspect_filled(
                "worldview-voice-sync",
                view,
                (1024, 768),
                "dark",
                theme::Theme::dark().with_worldview(WorldviewMode::Luke),
                (1024, 768),
            );
            assert!(mounted.contains(CopyText::SyncingBlockchain.get()));
            assert_eq!(mounted.contains("Consulting the ancient rules…"), !stalled);
        }
    }

    #[test]
    fn spanish_cube_opening_survives_deferred_layout() {
        use crate::home::launcher_opening::{Opening, Stage};
        use coincube_ui::locale::{language, render_in, Language};

        let cube = crate::app::settings::CubeSettings::new_with_raw_id(
            "spanish-opening".into(),
            "Everyday".into(),
            ChainId::Bitcoin,
        );
        let floor = floor_context(&cube);
        let quote = coincube_ui::component::quote_display::random_quote("loading");
        let spanish_quote = render_in(Language::Spanish, || quote.localized_text().to_owned());
        let now = std::time::Instant::now();
        let opening = Opening::new(Some(floor.clone()), true, Stage::Connecting, now);
        let view = render_in(Language::Spanish, || {
            opening_on_floor(
                &floor,
                OpeningFace {
                    cube: &cube,
                    quote: &quote,
                    outgoing: None,
                    stage: Stage::Connecting,
                    stalled: false,
                    cancel: || (),
                    without_spark: || (),
                },
                coincube_ui::theme::palette::ThemeMode::Dark,
                &opening,
                now,
            )
        });
        assert_eq!(language(), Language::English);
        let result = capture::inspect_filled(
            "spanish-cube-opening",
            view,
            (1024, 768),
            "dark",
            theme::Theme::dark(),
            (1024, 768),
        );
        assert!(result.contains("Conectando con el servicio de la cartera"));
        assert!(result.contains(&spanish_quote));
        assert_eq!(language(), Language::English);
    }

    fn floor_context(
        cube: &crate::app::settings::CubeSettings,
    ) -> launcher_catalog::LauncherSceneContext {
        let peer = crate::app::settings::CubeSettings::new_with_raw_id(
            "peer".into(),
            "Travel".into(),
            cube.network,
        );
        let selected = CubeCatalogKey {
            chain: cube.network,
            cube_id: cube.id.clone(),
        };
        let peer_key = CubeCatalogKey {
            chain: peer.network,
            cube_id: peer.id.clone(),
        };
        launcher_catalog::LauncherSceneContext {
            selected: selected.clone(),
            page: 0,
            selected_slot: 0,
            backup: "Not confirmed",
            connect: "None — local only".into(),
            wallets: "Spark".into(),
            peers: vec![
                launcher_catalog::LauncherScenePeer {
                    key: selected,
                    name: "Everyday".into(),
                    status: "PIN prompt",
                    marks: launcher_gpu::marks(cube),
                },
                launcher_catalog::LauncherScenePeer {
                    key: peer_key,
                    name: "Travel".into(),
                    status: "PIN prompt",
                    marks: launcher_gpu::marks(&peer),
                },
                // The 420px face covers the two middle labels, as the
                // prototype's lead Cube does; the last peer stays readable.
                launcher_catalog::LauncherScenePeer {
                    key: CubeCatalogKey {
                        chain: cube.network,
                        cube_id: "studio".into(),
                    },
                    name: "Studio".into(),
                    status: "PIN prompt",
                    marks: launcher_gpu::marks(&peer),
                },
                launcher_catalog::LauncherScenePeer {
                    key: CubeCatalogKey {
                        chain: cube.network,
                        cube_id: "lab".into(),
                    },
                    name: "Lab".into(),
                    status: "PIN prompt",
                    marks: launcher_gpu::marks(&peer),
                },
            ],
            pages: 1,
            chrome: Default::default(),
        }
    }

    #[test]
    fn unlock_floor_keeps_peer_and_live_input_at_compact_and_wide_sizes() {
        let cube = crate::app::settings::CubeSettings::new_with_raw_id(
            "floor".into(),
            "Everyday".into(),
            ChainId::Bitcoin,
        );
        let context = floor_context(&cube);
        let pin = String::new();
        for (width, height, appearance, palette, mode) in [
            (
                1024,
                768,
                "dark",
                theme::Theme::dark(),
                coincube_ui::theme::palette::ThemeMode::Dark,
            ),
            (
                1440,
                900,
                "light",
                theme::Theme::light(),
                coincube_ui::theme::palette::ThemeMode::Light,
            ),
        ] {
            let scene = unlock_face_on_floor(&cube, mode, &context, || {
                iced::widget::text_input("", &pin)
                    .id(crate::pin_input::first_digit_id())
                    .on_input(|value| value)
                    .into()
            });
            // The inspector reserves 100px for an outer chrome that this
            // standalone Launcher scene owns itself.
            let mounted = capture::inspect_full_window(
                "v113-unlock-floor",
                scene,
                (width, height + 100),
                appearance,
                palette,
            );
            assert!(mounted.contains("Lab"));
            assert!(!mounted.contains("Travel"), "label under the face");
            assert!(mounted.contains("PIN prompt"));
            assert!(mounted.contains("Open Everyday"));
            assert!(mounted.has_focus(crate::pin_input::first_digit_id()));
            let face = mounted
                .text
                .iter()
                .find(|(value, _)| value.starts_with("Open Everyday"))
                .map(|(_, bounds)| *bounds)
                .expect("selected unlock face caption");
            // The inspector lays the scene out unbounded, so the stage is the
            // 742px design height at both widths. Caption top = 64px bar + 1px
            // rule + target (the arrows' line) - 210 + 28px padding.
            let target =
                launcher_scene::Layout::new(width as f32, launcher_scene::DESIGN_STAGE_HEIGHT)
                    .target()
                    .1;
            let expected = 64.0 + 1.0 + target - 210.0 + 28.0;
            assert!(
                (face.y - expected).abs() < 22.0,
                "{}px floor face moved from its selected pose: {:?}",
                width,
                face
            );
        }
    }

    #[test]
    fn unlock_turn_keeps_pin_focus_when_art_completes() {
        #[derive(Default)]
        struct FocusTargets {
            ids: Vec<iced::advanced::widget::Id>,
            first_focused: bool,
        }
        impl Operation for FocusTargets {
            fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
                operate(self);
            }
            fn focusable(
                &mut self,
                id: Option<&iced::advanced::widget::Id>,
                _: Rectangle,
                state: &mut dyn Focusable,
            ) {
                if let Some(id) = id {
                    self.ids.push(id.clone());
                    if *id == crate::pin_input::first_digit_id() {
                        self.first_focused = state.is_focused();
                    }
                }
            }
        }
        struct FocusFirst;
        impl Operation for FocusFirst {
            fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
                operate(self);
            }
            fn focusable(
                &mut self,
                id: Option<&iced::advanced::widget::Id>,
                _: Rectangle,
                state: &mut dyn Focusable,
            ) {
                if id == Some(&crate::pin_input::first_digit_id()) {
                    state.focus();
                }
            }
        }

        let cube = crate::app::settings::CubeSettings::new_with_raw_id(
            "unlock-turn".into(),
            "Everyday".into(),
            ChainId::Bitcoin,
        );
        let pin = String::new();
        let controls: Element<'_, String> = iced::widget::text_input("", &pin)
            .id(crate::pin_input::first_digit_id())
            .on_input(|value| value)
            .into();
        let mut element = unlock_turn_with_controls(
            &cube,
            coincube_ui::theme::palette::ThemeMode::Dark,
            Some(0.25),
            controls,
        );
        for bytes in font::load() {
            iced::advanced::graphics::text::font_system()
                .write()
                .unwrap()
                .load_font(bytes);
        }
        let renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
            font::REGULAR,
            iced::Pixels(16.0),
            Some("tiny-skia"),
        ))
        .unwrap();
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &widget_layout::Limits::new(Size::ZERO, Size::new(1000.0, 900.0)),
        );
        element
            .as_widget_mut()
            .operate(&mut tree, Layout::new(&node), &renderer, &mut FocusFirst);
        let mut targets = FocusTargets::default();
        element
            .as_widget_mut()
            .operate(&mut tree, Layout::new(&node), &renderer, &mut targets);
        assert!(targets.ids.contains(&crate::pin_input::first_digit_id()));
        assert!(targets.first_focused);

        drop(element);
        let settled_controls: Element<'_, String> = iced::widget::text_input("", &pin)
            .id(crate::pin_input::first_digit_id())
            .on_input(|value| value)
            .into();
        let mut settled = unlock_turn_with_controls(
            &cube,
            coincube_ui::theme::palette::ThemeMode::Dark,
            None,
            settled_controls,
        );
        tree.diff(settled.as_widget());
        let settled_node = settled.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &widget_layout::Limits::new(Size::ZERO, Size::new(1000.0, 900.0)),
        );
        let mut targets = FocusTargets::default();
        settled.as_widget_mut().operate(
            &mut tree,
            Layout::new(&settled_node),
            &renderer,
            &mut targets,
        );
        assert!(
            targets.first_focused,
            "input focus must survive turn completion"
        );

        let mode = coincube_ui::theme::palette::ThemeMode::Dark;
        let context = floor_context(&cube);
        let floor_controls = unlock_face_on_floor(
            &cube,
            coincube_ui::theme::palette::ThemeMode::Dark,
            &context,
            || {
                iced::widget::text_input("", &pin)
                    .id(crate::pin_input::first_digit_id())
                    .on_input(|value| value)
                    .into()
            },
        );
        let mut floor_turn = unlock_turn_with_floor_controls(
            &cube,
            coincube_ui::theme::palette::ThemeMode::Dark,
            &context,
            Some(0.0),
            floor_controls,
        );
        let mut floor_tree = Tree::new(floor_turn.as_widget());
        let floor_node = floor_turn.as_widget_mut().layout(
            &mut floor_tree,
            &renderer,
            &widget_layout::Limits::new(Size::ZERO, Size::new(1024.0, 768.0)),
        );
        floor_turn.as_widget_mut().operate(
            &mut floor_tree,
            Layout::new(&floor_node),
            &renderer,
            &mut FocusFirst,
        );
        let mut floor_targets = FocusTargets::default();
        floor_turn.as_widget_mut().operate(
            &mut floor_tree,
            Layout::new(&floor_node),
            &renderer,
            &mut floor_targets,
        );
        assert!(floor_targets.first_focused, "PIN focuses at turn start");

        drop(floor_turn);
        let settled_floor_controls = unlock_face_on_floor(&cube, mode, &context, || {
            iced::widget::text_input("", &pin)
                .id(crate::pin_input::first_digit_id())
                .on_input(|value| value)
                .into()
        });
        let mut settled_floor =
            unlock_turn_with_floor_controls(&cube, mode, &context, None, settled_floor_controls);
        floor_tree.diff(settled_floor.as_widget());
        let settled_floor_node = settled_floor.as_widget_mut().layout(
            &mut floor_tree,
            &renderer,
            &widget_layout::Limits::new(Size::ZERO, Size::new(1024.0, 768.0)),
        );
        let mut floor_targets = FocusTargets::default();
        settled_floor.as_widget_mut().operate(
            &mut floor_tree,
            Layout::new(&settled_floor_node),
            &renderer,
            &mut floor_targets,
        );
        assert!(floor_targets.first_focused, "PIN focus survives floor turn");
    }

    #[test]
    fn passkey_begin_action_is_focusable_during_turn() {
        #[derive(Default)]
        struct FocusTargets(Vec<iced::advanced::widget::Id>);
        impl Operation for FocusTargets {
            fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
                operate(self);
            }
            fn focusable(
                &mut self,
                id: Option<&iced::advanced::widget::Id>,
                _: Rectangle,
                _: &mut dyn Focusable,
            ) {
                if let Some(id) = id {
                    self.0.push(id.clone());
                }
            }
        }

        let cube = crate::app::settings::CubeSettings::new_with_raw_id(
            "passkey-turn".into(),
            "Everyday".into(),
            ChainId::Bitcoin,
        );
        let unlock = crate::passkey_unlock::PasskeyUnlock::new(
            cube.clone(),
            PathBuf::new(),
            crate::pin_entry::PinEntrySuccess::LoadApp {
                datadir: CoincubeDirectory::new(PathBuf::new()),
                config: crate::app::Config::new(false),
                network: ChainId::Bitcoin.bitcoin_network(),
                internal_bitcoind: None,
                backup: None,
                wallet_settings: None,
                connect_client: None,
            },
        );
        let mode = coincube_ui::theme::palette::ThemeMode::Dark;
        let mut element =
            unlock_turn_with_controls(&cube, mode, Some(0.25), unlock.view_with_theme(mode));
        for bytes in font::load() {
            iced::advanced::graphics::text::font_system()
                .write()
                .unwrap()
                .load_font(bytes);
        }
        let renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
            font::REGULAR,
            iced::Pixels(16.0),
            Some("tiny-skia"),
        ))
        .unwrap();
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &widget_layout::Limits::new(Size::ZERO, Size::new(1000.0, 900.0)),
        );
        let mut targets = FocusTargets::default();
        element
            .as_widget_mut()
            .operate(&mut tree, Layout::new(&node), &renderer, &mut targets);
        assert!(targets
            .0
            .contains(&crate::passkey_unlock::begin_button_id()));

        struct FocusPasskey;
        impl Operation for FocusPasskey {
            fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
                operate(self);
            }
            fn focusable(
                &mut self,
                id: Option<&iced::advanced::widget::Id>,
                _: Rectangle,
                state: &mut dyn Focusable,
            ) {
                if id == Some(&crate::passkey_unlock::begin_button_id()) {
                    state.focus();
                }
            }
        }
        let context = floor_context(&cube);
        let mut on_floor = unlock_turn_with_floor_controls(
            &cube,
            mode,
            &context,
            Some(0.0),
            unlock.view_with_theme_and_floor(mode, Some(&context)),
        );
        let mut floor_tree = Tree::new(on_floor.as_widget());
        let floor_node = on_floor.as_widget_mut().layout(
            &mut floor_tree,
            &renderer,
            &widget_layout::Limits::new(Size::ZERO, Size::new(1024.0, 768.0)),
        );
        on_floor.as_widget_mut().operate(
            &mut floor_tree,
            Layout::new(&floor_node),
            &renderer,
            &mut FocusPasskey,
        );
        #[derive(Default)]
        struct PasskeyFocused(bool);
        impl Operation for PasskeyFocused {
            fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
                operate(self);
            }
            fn focusable(
                &mut self,
                id: Option<&iced::advanced::widget::Id>,
                _: Rectangle,
                state: &mut dyn Focusable,
            ) {
                if id == Some(&crate::passkey_unlock::begin_button_id()) {
                    self.0 = state.is_focused();
                }
            }
        }
        let mut focused = PasskeyFocused::default();
        on_floor.as_widget_mut().operate(
            &mut floor_tree,
            Layout::new(&floor_node),
            &renderer,
            &mut focused,
        );
        assert!(focused.0, "passkey action focuses at turn start");

        drop(on_floor);
        let mut settled_floor = unlock_turn_with_floor_controls(
            &cube,
            mode,
            &context,
            None,
            unlock.view_with_theme_and_floor(mode, Some(&context)),
        );
        floor_tree.diff(settled_floor.as_widget());
        let settled_floor_node = settled_floor.as_widget_mut().layout(
            &mut floor_tree,
            &renderer,
            &widget_layout::Limits::new(Size::ZERO, Size::new(1024.0, 768.0)),
        );
        let mut focused = PasskeyFocused::default();
        settled_floor.as_widget_mut().operate(
            &mut floor_tree,
            Layout::new(&settled_floor_node),
            &renderer,
            &mut focused,
        );
        assert!(focused.0, "passkey focus survives floor turn");
    }

    fn home_with_cubes() -> Home {
        let mut home = Home::new(
            CoincubeDirectory::new(PathBuf::new()),
            Some(Network::Bitcoin),
        )
        .0;
        home.network = ChainId::Bitcoin;
        home.state = super::super::State::Cubes {
            cubes: vec![
                crate::app::settings::CubeSettings::new_with_raw_id(
                    "first".into(),
                    "Everyday".into(),
                    ChainId::Bitcoin,
                ),
                crate::app::settings::CubeSettings::new_with_raw_id(
                    "second".into(),
                    "Travel".into(),
                    ChainId::Bitcoin,
                ),
            ],
            create_cube: false,
            source: ChainId::Bitcoin,
        };
        home.advisory_notice = None;
        home.catalog.selected = Some(CubeCatalogKey {
            chain: ChainId::Bitcoin,
            cube_id: "second".into(),
        });
        home
    }

    #[test]
    fn imported_bitcoin_face_mark_requires_an_installed_attachment() {
        use crate::app::settings::{
            BitcoinImport, BitcoinImportStage, CubeSettings, ImportedWalletAttachment,
        };
        use coincube_core::{
            miniscript::bitcoin::{
                bip32::{DerivationPath, Xpriv, Xpub},
                secp256k1::Secp256k1,
            },
            wallet_descriptor::ImportedWalletDescriptor,
        };
        use std::str::FromStr;

        let mut cube = CubeSettings::new("Importing".into(), ChainId::Bitcoin);
        assert_eq!(wallet_face_mark(&cube), WalletFaceMark::None);
        cube.bitcoin_import = Some(Box::new(BitcoinImport::pending()));
        assert_eq!(wallet_face_mark(&cube), WalletFaceMark::None);

        let secp = Secp256k1::new();
        let master = Xpriv::new_master(Network::Bitcoin, &[29; 32]).unwrap();
        let account = master
            .derive_priv(&secp, &DerivationPath::from_str("m/84h/0h/0h").unwrap())
            .unwrap();
        let xpub = Xpub::from_priv(&secp, &account);
        let key = format!("[{}/84h/0h/0h]{xpub}", master.fingerprint(&secp));
        let descriptor = ImportedWalletDescriptor::parse(
            &format!("wpkh({key}/0/*)"),
            Some(&format!("wpkh({key}/1/*)")),
            ChainId::Bitcoin,
        )
        .unwrap();
        let wallet = ImportedWalletAttachment::new(descriptor, ChainId::Bitcoin, 123, 200).unwrap();

        cube.bitcoin_import.as_mut().unwrap().stage = BitcoinImportStage::Installing {
            wallet: wallet.clone(),
        };
        assert_eq!(wallet_face_mark(&cube), WalletFaceMark::None);
        cube.bitcoin_import.as_mut().unwrap().stage = BitcoinImportStage::Syncing {
            wallet: wallet.clone(),
        };
        assert_eq!(wallet_face_mark(&cube), WalletFaceMark::ImportedBitcoin);
        cube.bitcoin_import.as_mut().unwrap().stage = BitcoinImportStage::Ready { wallet };
        assert_eq!(wallet_face_mark(&cube), WalletFaceMark::ImportedBitcoin);
    }

    #[test]
    fn cube_face_shows_unlock_route_without_inventing_backup_status() {
        use launcher_scene::{rest_face_svg, Face};
        let mut cube = crate::app::settings::CubeSettings::new_with_raw_id(
            "fixture".into(),
            "Fixture".into(),
            ChainId::Bitcoin,
        );
        let lock = "M7 11h10v9H7z";
        let attention = "M12 7.5v5.5";
        let question = "M9.6 9.6a2.4";
        // No master seed recorded for this Cube: this device cannot tell.
        let unknown_backup = rest_face_svg(Face::Right, launcher_gpu::marks(&cube), false).unwrap();
        assert_eq!(unlock_prompt(&cube), "PIN prompt");
        assert_eq!(backup(&cube), ("Status unknown", tenshu::Tone::Muted));
        assert!(unknown_backup.contains(lock));
        assert!(unknown_backup.contains(question) && !unknown_backup.contains(attention));

        cube.master_signer_fingerprint = Some([1, 2, 3, 4].into());
        let unconfirmed = rest_face_svg(Face::Right, launcher_gpu::marks(&cube), false).unwrap();
        assert_eq!(backup(&cube), ("Not confirmed", tenshu::Tone::Warn));
        assert!(unconfirmed.contains(attention) && !unconfirmed.contains(question));
        // A Recovery Kit is not the seed phrase backup the Cube asks for.
        cube.recovery_kit_password_backed_up = true;
        assert_eq!(launcher_gpu::marks(&cube).backup, BackupMark::NotConfirmed);

        cube.backed_up = true;
        let recorded_backup = rest_face_svg(Face::Right, launcher_gpu::marks(&cube), true).unwrap();
        assert!(!recorded_backup.contains(attention) && !recorded_backup.contains(question));
        assert!(recorded_backup.contains("#202020"), "light ink");

        cube.passkey_metadata = Some(crate::app::settings::PasskeyMetadata {
            credential_id: "fixture".into(),
            rp_id: "example.test".into(),
            created_at: 0,
            label: None,
        });
        let passkey = rest_face_svg(Face::Right, launcher_gpu::marks(&cube), false).unwrap();
        assert_eq!(unlock_prompt(&cube), "Passkey prompt");
        assert!(!passkey.contains(lock));
    }

    #[test]
    fn floor_pages_local_cubes_then_their_ghosts() {
        let mut home = home_with_cubes();
        home.catalog.selected = None;
        if let super::super::State::Cubes { cubes, .. } = &mut home.state {
            cubes[0].set_vault(crate::app::settings::VaultIdentity {
                wallet_id: crate::app::settings::WalletId::new("fixture".into(), Some(1)),
                fingerprint: Some("12345678".into()),
            });
            cubes[0].backed_up = true;
            cubes[1].recovery_kit_password_backed_up = true;
            for index in 3..=9 {
                cubes.push(crate::app::settings::CubeSettings::new_with_raw_id(
                    format!("cube-{index}"),
                    format!("Cube {index}"),
                    ChainId::Bitcoin,
                ));
            }
        }
        home.remote_cubes.push(super::super::RemoteCube {
            id: 7,
            uuid: "restore-me".into(),
            name: "Connect backup".into(),
            network: crate::app::settings::network_to_api_string(ChainId::Bitcoin),
            has_recovery_kit: true,
            has_encrypted_seed: true,
            phone_recoverable: false,
            phone_full_cube: false,
            encryption_pubkey: None,
            has_vault: None,
        });
        for (page, visible, hidden) in [
            (0, "Everyday", "Cube 5"),
            (1, "Cube 5", "Everyday"),
            (2, "Cube 9", "Cube 5"),
        ] {
            home.catalog.page = page;
            let scenario = format!("v113-launcher-floor-page{}", page + 1);
            for (size, appearance, palette) in [
                ((1024, 768), "dark", theme::Theme::dark()),
                ((1440, 900), "light", theme::Theme::light()),
            ] {
                home.theme_mode = if appearance == "light" {
                    coincube_ui::theme::palette::ThemeMode::Light
                } else {
                    coincube_ui::theme::palette::ThemeMode::Dark
                };
                let mounted = capture::inspect_full_window(
                    &scenario,
                    floor_body(&home),
                    size,
                    appearance,
                    palette,
                );
                assert!(mounted.contains(visible));
                assert!(!mounted.contains(hidden));
                assert!(mounted.contains(&format!("Page {} of 3", page + 1)));
                // The Connect-only Cube stands after the local ones, on the
                // last page, as a ghost the floor can select.
                let ghost = launcher_catalog::select_id(&launcher_catalog::ghost_key(
                    &home.remote_cubes[0],
                ));
                assert_eq!(mounted.has_focus(ghost), page == 2);
                assert_eq!(mounted.contains("In Connect · Restore"), page == 2);
                assert!(!mounted.contains("Available in Connect"));
            }
        }
    }

    fn ghost_fixture(uuid: &str, name: &str, kit: bool) -> super::super::RemoteCube {
        super::super::RemoteCube {
            id: 9,
            uuid: uuid.into(),
            name: name.into(),
            network: crate::app::settings::network_to_api_string(ChainId::Bitcoin),
            has_recovery_kit: kit,
            has_encrypted_seed: kit,
            phone_recoverable: false,
            phone_full_cube: false,
            encryption_pubkey: None,
            has_vault: None,
        }
    }

    #[test]
    fn a_new_device_stands_its_ghosts_on_the_floor_with_the_intro_line() {
        let mut home = home_with_cubes();
        home.state = super::super::State::NoCube { create_cube: false };
        home.catalog.selected = None;
        home.catalog.mode = LauncherMode::Cubes;
        home.remote_cubes = vec![
            ghost_fixture("kit", "Passkey", true),
            ghost_fixture("nokit", "Old laptop", false),
        ];
        for (size, appearance, palette) in [
            ((1024, 768), "dark", theme::Theme::dark()),
            ((1440, 900), "light", theme::Theme::light()),
        ] {
            let mounted = capture::inspect_full_window(
                "ghost-floor-new-device",
                floor_body(&home),
                size,
                appearance,
                palette,
            );
            assert!(mounted.contains(GHOST_INTRO));
            assert!(mounted.contains("In Connect · Restore"));
            assert!(mounted.contains("In Connect · No Recovery Kit"));
            assert!(!mounted.contains("Available in Connect"));
            for cube in &home.remote_cubes {
                let key = launcher_catalog::ghost_key(cube);
                assert!(mounted.has_focus(launcher_catalog::select_id(&key)));
            }
        }
        // With a local Cube on the row, the intro line has nothing to explain.
        home.state = home_with_cubes().state;
        home.catalog.selected = None;
        let mounted = capture::inspect_full_window(
            "ghost-floor-with-locals",
            floor_body(&home),
            (1440, 900),
            "dark",
            theme::Theme::dark(),
        );
        assert!(!mounted.contains(GHOST_INTRO));
        assert!(mounted.contains("Everyday") && mounted.contains("Old laptop"));
    }

    #[test]
    fn a_selected_ghost_shows_restore_or_the_recovery_phrase_on_its_face() {
        let mut home = home_with_cubes();
        home.catalog.mode = LauncherMode::Cubes;
        home.remote_cubes = vec![
            ghost_fixture("kit", "Passkey", true),
            ghost_fixture("nokit", "Old laptop", false),
        ];
        let peer = CubeCatalogKey {
            chain: ChainId::Bitcoin,
            cube_id: "first".into(),
        };
        for (uuid, primary, absent, backup) in [
            (
                "kit",
                "Restore to this device",
                "Import with recovery phrase…",
                "Recovery Kit available",
            ),
            (
                "nokit",
                "Import with recovery phrase…",
                "Restore to this device",
                "No Recovery Kit in Connect",
            ),
        ] {
            let cube = home.remote_cubes.iter().find(|c| c.uuid == uuid).unwrap();
            let key = launcher_catalog::ghost_key(cube);
            home.catalog.selected = Some(key.clone());
            for (size, appearance, palette) in [
                ((1024, 768), "dark", theme::Theme::dark()),
                ((1440, 900), "light", theme::Theme::light()),
            ] {
                let mounted = capture::inspect_full_window(
                    "ghost-face",
                    floor_body(&home),
                    size,
                    appearance,
                    palette,
                );
                assert!(mounted.contains(primary), "{}: {}", uuid, primary);
                assert!(!mounted.contains(absent), "{}: {}", uuid, absent);
                assert!(mounted.contains(backup));
                assert!(mounted.contains("In Connect, not on this device"));
                assert!(mounted.contains("Delete from Connect…"));
                assert!(mounted.has_focus(launcher_catalog::details_open_id(&key)));
                assert!(mounted.has_focus(launcher_catalog::remote_delete_id(uuid)));
                assert!(mounted.has_focus(launcher_catalog::close_details_id()));
                // The local Cubes stay reachable as dimmed peers.
                assert!(mounted.has_focus(launcher_catalog::select_id(&peer)));
                // A ghost has no Open, rename or local delete.
                assert!(!mounted.contains("Next step"));
            }
        }
    }

    #[test]
    fn selected_floor_keeps_a_real_cube_and_its_peers_reachable() {
        let mut home = home_with_cubes();
        home.catalog.mode = LauncherMode::Cubes;
        if let super::super::State::Cubes { cubes, .. } = &mut home.state {
            cubes[1].passkey_metadata = Some(crate::app::settings::PasskeyMetadata {
                credential_id: "fixture".into(),
                rp_id: "example.test".into(),
                created_at: 0,
                label: None,
            });
        }
        if let super::super::State::Cubes { cubes, .. } = &mut home.state {
            for name in ["Studio", "Lab"] {
                cubes.push(crate::app::settings::CubeSettings::new_with_raw_id(
                    name.to_lowercase(),
                    name.into(),
                    ChainId::Bitcoin,
                ));
            }
        }
        let selected = CubeCatalogKey {
            chain: ChainId::Bitcoin,
            cube_id: "second".into(),
        };
        let other = CubeCatalogKey {
            chain: ChainId::Bitcoin,
            cube_id: "first".into(),
        };
        for (size, appearance, palette) in [
            ((1024, 768), "dark", theme::Theme::dark()),
            ((1440, 900), "light", theme::Theme::light()),
        ] {
            home.theme_mode = if appearance == "light" {
                coincube_ui::theme::palette::ThemeMode::Light
            } else {
                coincube_ui::theme::palette::ThemeMode::Dark
            };
            let mounted = capture::inspect_full_window(
                "v113-selected-cube-face",
                floor_body(&home),
                size,
                appearance,
                palette,
            );
            assert!(mounted.contains("Travel"));
            assert!(mounted.contains("Everyday"));
            // `.pnet`: the coin and the network's full name.
            assert!(mounted.contains("Bitcoin mainnet"));
            assert!(mounted.contains("Next step"));
            assert!(mounted.contains("Passkey prompt"));
            // The dimmed peer retains its own state instead of becoming a
            // generic "Select Cube" label while Travel's face is open.
            assert!(mounted.contains("PIN prompt"));
            assert!(!mounted.contains("Select Cube"));
            assert!(mounted.has_focus(launcher_catalog::details_open_id(&selected)));
            assert!(mounted.has_focus(launcher_catalog::select_id(&other)));
            assert!(mounted.has_focus(launcher_catalog::close_details_id()));
        }
    }

    #[test]
    fn real_pin_and_passkey_faces_follow_the_prototype_and_keep_focus_targets() {
        let cube = crate::app::settings::CubeSettings::new_with_raw_id(
            "everyday".into(),
            "Everyday".into(),
            ChainId::Bitcoin,
        );
        let context = floor_context(&cube);
        let success = || crate::pin_entry::PinEntrySuccess::LoadApp {
            datadir: crate::dir::CoincubeDirectory::new(Default::default()),
            config: crate::app::Config::new(false),
            network: cube.network.bitcoin_network(),
            internal_bitcoind: None,
            backup: None,
            wallet_settings: None,
            connect_client: None,
        };
        let mode = coincube_ui::theme::palette::ThemeMode::Dark;
        let entry = crate::pin_entry::PinEntry::new(cube.clone(), PathBuf::new(), success(), None);
        let pin = capture::inspect_full_window(
            "unlock-pin-face",
            entry.view_with_theme_and_floor(mode, Some(&context)),
            (1440, 900),
            "dark",
            theme::Theme::dark(),
        );
        for text in [
            "Open Everyday",
            "Bitcoin mainnet",
            "PIN",
            "Enter this Cube's PIN.",
            "Unlock",
            "Cancel",
        ] {
            assert!(pin.contains(text), "missing {:?}", text);
        }
        assert!(!pin.contains("Network checks"));
        // The old full-screen controls are gone from the face.
        assert!(!pin.contains("Submit"));
        assert!(!pin.contains("Enter PIN for"));
        assert!(pin.has_focus(crate::pin_input::first_digit_id()));
        // The quiet Launcher bars stand behind the face.
        assert!(pin.contains("App preferences"));
        assert!(pin.contains("Create Cube"));

        let passkey =
            crate::passkey_unlock::PasskeyUnlock::new(cube.clone(), PathBuf::new(), success());
        let passkey = capture::inspect_full_window(
            "unlock-passkey-face",
            passkey.view_with_theme_and_floor(mode, Some(&context)),
            (1440, 900),
            "dark",
            theme::Theme::dark(),
        );
        assert!(passkey.contains("Unlock with passkey"));
        assert!(passkey.contains("Cancel"));
        assert!(passkey.has_focus(crate::passkey_unlock::begin_button_id()));
    }

    #[test]
    fn pin_face_controls_stay_inside_the_drawn_face_at_every_window_height() {
        let cube = crate::app::settings::CubeSettings::new_with_raw_id(
            "family".into(),
            "Family".into(),
            ChainId::Bitcoin,
        );
        let context = floor_context(&cube);
        let entry = crate::pin_entry::PinEntry::new(
            cube.clone(),
            PathBuf::new(),
            crate::pin_entry::PinEntrySuccess::LoadApp {
                datadir: crate::dir::CoincubeDirectory::new(Default::default()),
                config: crate::app::Config::new(false),
                network: cube.network.bitcoin_network(),
                internal_bitcoind: None,
                backup: None,
                wallet_settings: None,
                connect_client: None,
            },
            None,
        );
        let mode = coincube_ui::theme::palette::ThemeMode::Light;
        let bounds = |mounted: &capture::Inspection, needle: &str| {
            mounted
                .text
                .iter()
                .find_map(|(value, bounds)| (value == needle).then_some(*bounds))
                .unwrap_or_else(|| panic!("missing {:?}", needle))
        };
        // A locked Cube reopens without the Launcher behind it; the floor
        // variant stands over the Launcher body between its two bars.
        for size in [(1024, 768), (1300, 1316), (1440, 900), (1920, 1200)] {
            for on_floor in [false, true] {
                let (w, h) = (size.0 as f32, size.1 as f32);
                let (layout, offset) = if on_floor {
                    (
                        floor_stage_layout(Size::new(w, h)),
                        launcher_chrome::BAR_HEIGHT + 1.0,
                    )
                } else {
                    (launcher_scene::Layout::new(w, h), 0.0)
                };
                let face = layout.face();
                let (target_x, target_y) = layout.target();
                let top = offset + target_y - face / 2.0;
                let left = target_x - face / 2.0;
                // Laid out at the window's own height: a scroll's unbounded
                // height would hide where the stage puts the face.
                let mounted = capture::inspect_filled(
                    "unlock-pin-face-fit",
                    entry.view_with_theme_and_floor(mode, on_floor.then_some(&context)),
                    size,
                    "light",
                    theme::Theme::light(),
                    size,
                );
                let heading = bounds(&mounted, "Open Family");
                let cancel = bounds(&mounted, "Cancel");
                let at = format!("{:?} floor={}", size, on_floor);
                assert!(
                    (heading.y - top - 28.0).abs() < 1.0,
                    "{}: heading at {} for a face from {}",
                    at,
                    heading.y,
                    top
                );
                assert!(
                    (heading.x - left - 28.0).abs() < 1.0,
                    "{}: heading at x {} for a face from {}",
                    at,
                    heading.x,
                    left
                );
                assert!(
                    cancel.y + cancel.height <= top + face - 28.0 + 1.0,
                    "{}: Cancel ends at {} past the face's padding at {}",
                    at,
                    cancel.y + cancel.height,
                    top + face - 28.0
                );
            }
        }
    }

    /// A long name beside a long network once left the network a sliver of
    /// the heading row: it broke glyph by glyph down the face and took every
    /// pixel the PIN field, Unlock and Cancel needed, so a Bitcoin Blake2b
    /// unlock had no live control left and the window seemed frozen.
    #[test]
    fn long_names_keep_the_network_on_its_own_line_and_the_pin_controls_on_the_face() {
        let bounds = |mounted: &capture::Inspection, needle: &str| {
            mounted
                .text
                .iter()
                .find_map(|(value, bounds)| (value == needle).then_some(*bounds))
                .unwrap_or_else(|| panic!("missing {:?}", needle))
        };
        for chain in [
            ChainId::BitcoinBlake2b,
            ChainId::BitcoinBlake2bTestnet4,
            ChainId::Bitcoin,
        ] {
            let cube = crate::app::settings::CubeSettings::new_with_raw_id(
                "long".into(),
                "My Vault Bitcoin Blake2b Testnet4 wallet".into(),
                chain,
            );
            let context = floor_context(&cube);
            let entry = crate::pin_entry::PinEntry::new(
                cube.clone(),
                PathBuf::new(),
                crate::pin_entry::PinEntrySuccess::LoadApp {
                    datadir: crate::dir::CoincubeDirectory::new(Default::default()),
                    config: crate::app::Config::new(false),
                    network: cube.network.bitcoin_network(),
                    internal_bitcoind: None,
                    backup: None,
                    wallet_settings: None,
                    connect_client: None,
                },
                None,
            );
            for size in [(1024, 768), (1440, 900)] {
                for on_floor in [false, true] {
                    let (w, h) = (size.0 as f32, size.1 as f32);
                    let (layout, offset) = if on_floor {
                        (
                            floor_stage_layout(Size::new(w, h)),
                            launcher_chrome::BAR_HEIGHT + 1.0,
                        )
                    } else {
                        (launcher_scene::Layout::new(w, h), 0.0)
                    };
                    let face = layout.face();
                    let (target_x, target_y) = layout.target();
                    let top = offset + target_y - face / 2.0;
                    let left = target_x - face / 2.0;
                    let mounted = capture::inspect_filled(
                        "unlock-pin-face-long-name",
                        entry.view_with_theme_and_floor(
                            coincube_ui::theme::palette::ThemeMode::Dark,
                            on_floor.then_some(&context),
                        ),
                        size,
                        "dark",
                        theme::Theme::dark(),
                        size,
                    );
                    let at = format!("{:?} {:?} floor={}", chain, size, on_floor);
                    let inside = |rect: Rectangle| {
                        rect.height > 0.0
                            && rect.y >= top - 1.0
                            && rect.y + rect.height <= top + face - 28.0 + 1.0
                    };
                    let digit = mounted
                        .focus
                        .iter()
                        .find_map(|(id, rect)| {
                            (id == &crate::pin_input::first_digit_id()).then_some(*rect)
                        })
                        .unwrap_or_else(|| panic!("{}: no PIN digit", at));
                    assert!(inside(digit), "{}: PIN digit at {:?}", at, digit);
                    let cancel = bounds(&mounted, "Cancel");
                    assert!(inside(cancel), "{}: Cancel at {:?}", at, cancel);
                    let unlock = bounds(&mounted, "Unlock");
                    assert!(inside(unlock), "{}: Unlock at {:?}", at, unlock);

                    let heading = mounted
                        .text
                        .iter()
                        .find_map(|(value, rect)| value.starts_with("Open ").then_some(*rect))
                        .unwrap_or_else(|| panic!("{}: no heading", at));
                    let network = bounds(&mounted, launcher_chrome::network_name(chain));
                    // Its own line, under the whole name and never beside it.
                    assert!(
                        network.y >= heading.y + heading.height - 1.0,
                        "{}: network at {:?} under a heading at {:?}",
                        at,
                        network,
                        heading
                    );
                    assert!(
                        network.x >= left + 28.0 && network.x < left + 28.0 + 40.0,
                        "{}: network starts at x {} for a face from {}",
                        at,
                        network.x,
                        left
                    );
                    // One 14px line, not a column of glyphs.
                    assert!(
                        network.height < 24.0,
                        "{}: network wraps to {:?}",
                        at,
                        network
                    );
                    assert!(
                        digit.y >= network.y + network.height,
                        "{}: PIN digit at {:?} overlaps the network at {:?}",
                        at,
                        digit,
                        network
                    );
                }
            }
        }
    }

    #[test]
    fn unlock_cube_keeps_network_and_persisted_face_marks_in_both_themes() {
        use launcher_scene::{rest_face_svg, Face};
        let mut cube = crate::app::settings::CubeSettings::new_with_raw_id(
            "marked".into(),
            "Marked".into(),
            ChainId::Bitcoin,
        );
        cube.backed_up = true;
        cube.set_vault(crate::app::settings::VaultIdentity {
            wallet_id: crate::app::settings::WalletId::new("fixture".into(), Some(1)),
            fingerprint: Some("12345678".into()),
        });
        let marks = launcher_gpu::marks(&cube);
        assert!(marks.vault && marks.backup == BackupMark::Confirmed && !marks.passkey);
        for light in [false, true] {
            let top = rest_face_svg(Face::Top, marks, light).unwrap();
            assert!(
                top.contains("data:image/png;base64,"),
                "Bitcoin coin missing"
            );
            assert!(rest_face_svg(Face::Left, marks, light)
                .unwrap()
                .contains("M12 3l7 3v5.5"));
            assert!(rest_face_svg(Face::Right, marks, light)
                .unwrap()
                .contains("M7 11h10v9H7z"));
            // The same marks paint the resting floor and every motion frame:
            // the stage reads them from one persisted record.
            assert_eq!(
                launcher_scene::edge(cube.network, light)[..3],
                [243, 130, 29]
            );
        }
        cube.network = ChainId::Testnet4;
        let testnet = launcher_gpu::marks(&cube);
        assert_eq!(
            launcher_scene::edge(testnet.network, false)[..3],
            [185, 187, 181]
        );
        assert!(rest_face_svg(Face::Top, testnet, false)
            .unwrap()
            .contains("stroke-dasharray=\"2.5 2.5\""));
        for (appearance, mode, palette) in [
            (
                "dark",
                coincube_ui::theme::palette::ThemeMode::Dark,
                theme::Theme::dark(),
            ),
            (
                "light",
                coincube_ui::theme::palette::ThemeMode::Light,
                theme::Theme::light(),
            ),
        ] {
            let mounted = capture::inspect_full_window::<ViewMessage>(
                "v113-unlock-face-identity",
                unlock_face(&cube, mode, || tenshu::body("Unlock controls").into()),
                (1024, 768),
                appearance,
                palette,
            );
            assert!(mounted.contains("Marked"));
            assert!(mounted.contains(cube.network.label()));
            assert!(mounted.contains("Unlock controls"));
        }
    }

    #[test]
    fn four_cube_floor_stays_inside_stage_across_wide_breakpoint() {
        let mut home = home_with_cubes();
        home.catalog.mode = LauncherMode::Cubes;
        home.catalog.selected = None;
        if let super::super::State::Cubes { cubes, .. } = &mut home.state {
            for index in 3..=4 {
                cubes.push(crate::app::settings::CubeSettings::new_with_raw_id(
                    format!("cube-{index}"),
                    format!("Cube {index}"),
                    ChainId::Bitcoin,
                ));
            }
        }
        let ids = ["first", "second", "cube-3", "cube-4"];
        for width in [1100, 1160, 1200] {
            for (appearance, palette) in [
                ("dark", theme::Theme::dark()),
                ("light", theme::Theme::light()),
            ] {
                home.theme_mode = if appearance == "light" {
                    coincube_ui::theme::palette::ThemeMode::Light
                } else {
                    coincube_ui::theme::palette::ThemeMode::Dark
                };
                let mounted = capture::inspect_full_window(
                    &format!("v113-floor-breakpoint-{width}"),
                    floor_body(&home),
                    (width, 900),
                    appearance,
                    palette,
                );
                let cubes: Vec<_> = ids
                    .iter()
                    .map(|id| {
                        let key = CubeCatalogKey {
                            chain: ChainId::Bitcoin,
                            cube_id: (*id).into(),
                        };
                        mounted
                            .focus
                            .iter()
                            .find(|(actual, _)| actual == &launcher_catalog::select_id(&key))
                            .map(|(_, bounds)| *bounds)
                            .expect("each Cube has a selectable floor target")
                    })
                    .collect();
                assert!(cubes[0].x >= -0.5, "left Cube clipped at {}", width);
                assert!(
                    cubes[3].x + cubes[3].width <= width as f32 + 0.5,
                    "right Cube clipped at {}",
                    width
                );
                assert!(cubes
                    .windows(2)
                    .all(|pair| { pair[0].x + pair[0].width <= pair[1].x + 0.5 }));
            }
        }
    }

    fn home_with_floor(count: usize) -> Home {
        let mut home = home_with_cubes();
        home.catalog.mode = LauncherMode::Cubes;
        home.catalog.selected = None;
        if let super::super::State::Cubes { cubes, .. } = &mut home.state {
            for index in cubes.len()..count {
                cubes.push(crate::app::settings::CubeSettings::new_with_raw_id(
                    format!("cube-{index}"),
                    format!("Cube {index}"),
                    ChainId::Bitcoin,
                ));
            }
        }
        home
    }

    fn floor_key(index: usize) -> CubeCatalogKey {
        CubeCatalogKey {
            chain: ChainId::Bitcoin,
            cube_id: match index {
                0 => "first".into(),
                1 => "second".into(),
                index => format!("cube-{index}"),
            },
        }
    }

    #[test]
    fn floor_page_size_switches_to_six_at_the_wide_breakpoint() {
        use launcher_catalog::{floor_page_size, FLOOR_WIDE_MIN_WIDTH};
        // 1024 is the minimum window; 1440 and 1680 keep the four-up board.
        for width in [
            840.0,
            1024.0,
            1280.0,
            1440.0,
            1680.0,
            FLOOR_WIDE_MIN_WIDTH - 1.0,
        ] {
            assert_eq!(floor_page_size(width), 4, "at {}", width);
        }
        for width in [FLOOR_WIDE_MIN_WIDTH, 2000.0, 2560.0, 3840.0] {
            assert_eq!(floor_page_size(width), 6, "at {}", width);
        }
    }

    #[test]
    fn floor_pages_and_anchor_page_follow_the_page_size() {
        use launcher_catalog::{floor_page_keeping, floor_pages};
        assert_eq!(floor_pages(0, 4), 1);
        assert_eq!(floor_pages(1, 6), 1);
        assert_eq!(floor_pages(4, 4), 1);
        assert_eq!(floor_pages(5, 4), 2);
        assert_eq!(floor_pages(12, 4), 3);
        assert_eq!(floor_pages(12, 6), 2);
        assert_eq!(floor_pages(13, 6), 3);
        // A Cube's page at each size, and never past the last page.
        assert_eq!(floor_page_keeping(0, 6, 12), 0);
        assert_eq!(floor_page_keeping(5, 6, 12), 0);
        assert_eq!(floor_page_keeping(6, 6, 12), 1);
        assert_eq!(floor_page_keeping(8, 4, 12), 2);
        assert_eq!(floor_page_keeping(11, 6, 12), 1);
        assert_eq!(floor_page_keeping(40, 6, 12), 1);
    }

    #[test]
    fn stage_width_sets_page_size_for_keyboard_and_dot_paging() {
        let mut home = home_with_floor(12);
        assert_eq!(home.catalog.page_size(), 4, "four until measured");
        let _ = home.update_catalog(CatalogMessage::StageWidth(2000.0));
        assert_eq!(home.catalog.page_size(), 6);
        // Arrow keys: two pages of six, and no third.
        let _ = home.update_catalog(CatalogMessage::Step(1));
        assert_eq!(home.catalog.page, 1);
        let _ = home.update_catalog(CatalogMessage::Step(1));
        assert_eq!(home.catalog.page, 1);
        let _ = home.update_catalog(CatalogMessage::Step(-1));
        assert_eq!(home.catalog.page, 0);
        // A dot past the last six-up page is refused.
        let _ = home.update_catalog(CatalogMessage::Page(2));
        assert_eq!(home.catalog.page, 0);
        let _ = home.update_catalog(CatalogMessage::StageWidth(1280.0));
        assert_eq!(home.catalog.page_size(), 4);
        let _ = home.update_catalog(CatalogMessage::Page(2));
        assert_eq!(home.catalog.page, 2);
    }

    #[test]
    fn network_activity_fills_the_sky_of_a_browsing_floor_only() {
        use super::super::launcher_activity::{sample_snapshot, ActivityMessage, Unavailable};
        let mut home = home_with_floor(4);
        home.catalog
            .activity
            .show_for_test(ChainId::Bitcoin, Ok(sample_snapshot()));
        let inspect = |home: &Home, name: &str| {
            capture::inspect_full_window(
                name,
                floor_body_in(home, 1100.0),
                (1440, 1250),
                "dark",
                theme::Theme::dark(),
            )
        };
        let browsing = inspect(&home, "launcher-activity-browse");
        for fact in [
            "BLOCK",
            "#968,412",
            "worth of txs waiting to confirm",
            "to make the next block",
            "HASHRATE",
            "NEXT HALVING",
            "in 81,588 blocks",
        ] {
            assert!(browsing.contains(fact), "missing {:?}", fact);
        }
        // The dock names the control; its menu gives the block and its
        // source, and offers the others.
        let launcher = |home: &Home, name: &str| {
            capture::inspect_filled(
                name,
                super::super::launcher_catalog::launcher_view(home),
                (1440, 900),
                "dark",
                theme::Theme::dark(),
                (1440, 900),
            )
        };
        let dock = launcher(&home, "launcher-activity-dock");
        // The light theme reads the same facts.
        let light = capture::inspect_filled(
            "launcher-activity-light",
            super::super::launcher_catalog::launcher_view(&home),
            (1440, 900),
            "light",
            theme::Theme::light(),
            (1440, 900),
        );
        assert!(light.contains("NEXT HALVING"));
        assert!(dock.contains("Network activity"));
        assert!(!dock.contains("Block 968,412"));
        assert!(!dock.contains("Turn off in App preferences"));
        // The narrowest window still fits the selector beside the limit text.
        let narrow = capture::inspect_filled(
            "launcher-activity-dock-narrow",
            super::super::launcher_catalog::launcher_view(&home),
            (1024, 768),
            "dark",
            theme::Theme::dark(),
            (1024, 768),
        );
        assert!(narrow.contains("Network activity"));
        assert!(narrow.contains("Key"));
        let _ = home.update_catalog(CatalogMessage::Activity(ActivityMessage::Menu(true)));
        let menu = launcher(&home, "launcher-activity-menu");
        for item in [
            "Network activity · Bitcoin mainnet",
            "Block 968,412 · COINCUBE | Connect",
            "Automatic",
            "Local node",
            "Turn off in App preferences",
        ] {
            assert!(menu.contains(item), "missing {:?}", item);
        }
        // An opened face rises into the sky, so a selection clears it.
        let _ = home.update_catalog(CatalogMessage::SelectFloor(floor_key(0)));
        assert!(!inspect(&home, "launcher-activity-selected").contains("NEXT HALVING"));
        let _ = home.update_catalog(CatalogMessage::ClearSelection);
        home.catalog.motion = None;
        // A source that cannot answer leaves the sky empty and says why.
        home.catalog
            .activity
            .show_for_test(ChainId::Bitcoin, Err(Unavailable::LocalNodeNotRunning));
        assert!(!inspect(&home, "launcher-activity-unavailable").contains("NEXT HALVING"));
        assert!(launcher(&home, "launcher-activity-why").contains("Local node didn't answer"));
        // Turned off in App preferences: no band and no dock control.
        home.catalog
            .activity
            .show_for_test(ChainId::Bitcoin, Ok(sample_snapshot()));
        home.catalog.activity.enabled = false;
        assert!(!inspect(&home, "launcher-activity-off").contains("NEXT HALVING"));
        assert!(!launcher(&home, "launcher-activity-off-dock").contains("COINCUBE | Connect"));
    }

    /// With no Cubes the floor holds only the Create placeholder, and the
    /// sky above it still shows Network activity.
    #[test]
    fn network_activity_fills_the_sky_above_the_create_placeholder() {
        use super::super::launcher_activity::sample_snapshot;
        let mut home = home_with_floor(0);
        home.state = super::super::State::NoCube { create_cube: false };
        assert!(launcher_catalog::project(&home).shows_create_placeholder(home.catalog.mode));
        let inspect = |home: &Home, name: &str| {
            capture::inspect_full_window(
                name,
                floor_body_in(home, 1100.0),
                (1440, 1250),
                "dark",
                theme::Theme::dark(),
            )
        };
        assert!(!inspect(&home, "launcher-activity-empty-unread").contains("NEXT HALVING"));
        home.catalog
            .activity
            .show_for_test(ChainId::Bitcoin, Ok(sample_snapshot()));
        let empty = inspect(&home, "launcher-activity-empty");
        for fact in ["Create Cube", "BLOCK", "#968,412", "NEXT HALVING"] {
            assert!(empty.contains(fact), "missing {:?}", fact);
        }
        home.catalog.activity.enabled = false;
        assert!(!inspect(&home, "launcher-activity-empty-off").contains("NEXT HALVING"));
    }

    #[test]
    fn network_activity_follows_the_network_filter() {
        use super::super::launcher_activity::sample_snapshot;
        let mut home = home_with_floor(4);
        // Bitcoin's read does not stand in for another network's.
        home.catalog
            .activity
            .show_for_test(ChainId::Bitcoin, Ok(sample_snapshot()));
        home.catalog.filter =
            super::super::launcher_catalog::NetworkFilter::Network(ChainId::BitcoinBlake2b);
        assert_eq!(home.activity_chain(), ChainId::BitcoinBlake2b);
        assert!(home
            .catalog
            .activity
            .current(ChainId::BitcoinBlake2b)
            .is_none());
        assert!(home.catalog.activity.current(ChainId::Bitcoin).is_some());
    }

    #[test]
    fn the_activity_menu_switches_between_bitcoin_and_blake2b() {
        use super::super::launcher_activity::{sample_snapshot, ActivityMessage};
        use super::super::launcher_catalog::NetworkFilter;
        let menu = |home: &Home, name: &str| {
            capture::inspect_filled(
                name,
                super::super::launcher_catalog::launcher_view(home),
                (1440, 900),
                "dark",
                theme::Theme::dark(),
                (1440, 900),
            )
        };
        let mut home = home_with_floor(4);
        home.catalog
            .activity
            .show_for_test(ChainId::Bitcoin, Ok(sample_snapshot()));
        let _ = home.update_catalog(CatalogMessage::Activity(ActivityMessage::Menu(true)));
        // Without Bitcoin Blake2b there is nothing to switch to.
        assert!(!home.activity_blake2b_offered());
        assert!(!menu(&home, "launcher-activity-no-b2b").contains("Bitcoin Blake2b"));
        let _ = home.update_catalog(CatalogMessage::Activity(ActivityMessage::Blake2b(true)));
        assert_eq!(home.activity_chain(), ChainId::Bitcoin);

        // Listed: the menu offers both, and the choice moves the activity.
        home.displayed_networks = vec![ChainId::Bitcoin, ChainId::BitcoinBlake2b];
        assert!(home.activity_blake2b_offered());
        assert!(menu(&home, "launcher-activity-b2b").contains("Bitcoin Blake2b"));
        let _ = home.update_catalog(CatalogMessage::Activity(ActivityMessage::Blake2b(true)));
        assert_eq!(home.activity_chain(), ChainId::BitcoinBlake2b);
        assert!(menu(&home, "launcher-activity-b2b-on")
            .contains("Network activity · Bitcoin Blake2b mainnet"));
        let _ = home.update_catalog(CatalogMessage::Activity(ActivityMessage::Blake2b(false)));
        assert_eq!(home.activity_chain(), ChainId::Bitcoin);

        // A filter naming one network wins, and the switch steps aside.
        let _ = home.update_catalog(CatalogMessage::Activity(ActivityMessage::Blake2b(true)));
        home.catalog.filter = NetworkFilter::Network(ChainId::Bitcoin);
        assert!(!home.activity_blake2b_offered());
        assert_eq!(home.activity_chain(), ChainId::Bitcoin);
        // Withdrawn Bitcoin Blake2b falls back to Bitcoin.
        home.catalog.filter = NetworkFilter::All;
        home.displayed_networks = vec![ChainId::Bitcoin];
        assert_eq!(home.activity_chain(), ChainId::Bitcoin);
    }

    #[test]
    fn wheel_pages_at_six_up() {
        let mut home = home_with_floor(12);
        let _ = home.update_catalog(CatalogMessage::StageWidth(2000.0));
        let _ = home.update_catalog(CatalogMessage::Wheel(iced::mouse::ScrollDelta::Pixels {
            x: -80.0,
            y: 0.0,
        }));
        assert_eq!(home.catalog.page, 1);
        let mounted = capture::inspect_full_window(
            "launcher-floor-six-up-page2",
            floor_body_in(&home, 1100.0),
            (2000, 1250),
            "dark",
            theme::Theme::dark(),
        );
        assert!(mounted.contains("Page 2 of 2"));
        assert!(mounted.contains("Cube 11"));
        assert!(!mounted.contains("Cube 5"));
    }

    #[test]
    fn crossing_the_breakpoint_keeps_the_shown_cubes_on_screen() {
        let mut home = home_with_floor(12);
        // Four-up page 3 shows Cubes 8..12; six-up they are on page 2.
        let _ = home.update_catalog(CatalogMessage::Page(2));
        let _ = home.update_catalog(CatalogMessage::StageWidth(2000.0));
        assert_eq!(home.catalog.page, 1);
        // Six-up page 2 starts at Cube 6, which is on four-up page 2.
        let _ = home.update_catalog(CatalogMessage::StageWidth(1440.0));
        assert_eq!(home.catalog.page, 1);
        // A selected Cube stays on screen, with its face, across the change.
        let _ = home.update_catalog(CatalogMessage::SelectFloor(floor_key(5)));
        assert_eq!(home.catalog.page, 1);
        let _ = home.update_catalog(CatalogMessage::StageWidth(2000.0));
        assert_eq!(home.catalog.page, 0);
        assert_eq!(home.catalog.selected, Some(floor_key(5)));
        // An unchanged page size leaves the page alone.
        let _ = home.update_catalog(CatalogMessage::StageWidth(2400.0));
        assert_eq!(home.catalog.page, 0);
    }

    #[test]
    fn floor_row_is_centered_on_the_arrow_line_at_every_size() {
        let home = home_with_floor(12);
        // Page one: Next is live (and focusable); Previous is drawn disabled
        // in the mirror position, 18px in from the left edge.
        let next = Id::from("tenshu.launcher.page.next");
        // (window, body between the bars, Cubes a page)
        for ((width, height), per_page) in [
            ((1024u32, 768u32), 4usize),
            ((1280, 800), 4),
            ((1440, 900), 4),
            ((2000, 1250), 6),
            ((2560, 1440), 6),
        ] {
            let body = height as f32 - 2.0 * launcher_chrome::BAR_HEIGHT - 2.0;
            let mounted = capture::inspect_full_window(
                &format!("launcher-floor-centered-{width}x{height}"),
                floor_body_in(&home, body),
                (width, height),
                "dark",
                theme::Theme::dark(),
            );
            let bounds = |id: &Id| {
                mounted
                    .focus
                    .iter()
                    .find(|(actual, _)| actual == id)
                    .map(|(_, bounds)| *bounds)
                    .unwrap_or_else(|| panic!("{:?} is mounted at {}", id, width))
            };
            let slots: Vec<_> = (0..per_page)
                .map(|index| bounds(&launcher_catalog::select_id(&floor_key(index))))
                .collect();
            assert!(
                mounted
                    .focus
                    .iter()
                    .all(|(id, _)| id != &launcher_catalog::select_id(&floor_key(per_page))),
                "page one holds {} Cubes at {}",
                per_page,
                width
            );
            assert!(mounted.contains(&format!("Page 1 of {}", 12usize.div_ceil(per_page))));
            let layout = launcher_scene::Layout::new(width as f32, stage_height(body));
            // Horizontally: the row's margins match.
            let left = slots[0].x;
            let right = width as f32 - (slots[per_page - 1].x + slots[per_page - 1].width);
            assert!(
                (left - right).abs() <= 1.0,
                "row off center at {}: {} vs {}",
                width,
                left,
                right
            );
            // Vertically: each Cube's center is on the arrows' center line,
            // and the Cube-and-label block is centered on the stage.
            let next = bounds(&next);
            let arrows = next.center_y();
            assert!(
                (arrows - layout.horizon()).abs() <= 1.0,
                "arrows at {}",
                arrows
            );
            let label = launcher_scene::label_rect(&layout, 0, per_page);
            let (_, cube_top, _, _) = launcher_scene::bounds(
                &layout,
                layout.rest_center(0, per_page),
                launcher_scene::Pose::REST,
            );
            let block = (cube_top + label.3) / 2.0;
            assert!(
                (block - stage_height(body) / 2.0).abs() <= 1.0,
                "Cube and label block at {} in a {} stage",
                block,
                stage_height(body)
            );
            assert!(
                slots[0].y + slots[0].height - label.3 >= 9.0,
                "label foot inside its slot"
            );
            for (index, slot) in slots.iter().enumerate() {
                let (_, cube) = layout.rest_center(index, per_page);
                assert!((slot.y - layout.rows_top()).abs() <= 1.0);
                assert!(
                    (cube - arrows).abs() <= 1.0,
                    "Cube {} at {}, arrows at {}, {}x{}",
                    index,
                    cube,
                    arrows,
                    width,
                    height
                );
            }
            // Neither arrow crowds a slot.
            assert!((width as f32 - (next.x + next.width) - 18.0).abs() <= 0.5);
            assert!(
                18.0 + next.width <= slots[0].x,
                "left arrow over row at {}",
                width
            );
            assert!(
                slots[per_page - 1].x + slots[per_page - 1].width <= next.x,
                "right arrow over row at {}",
                width
            );
        }
    }

    #[test]
    fn mounted_list_preserves_focus_and_unknown_facts_in_both_themes_and_sizes() {
        let home = home_with_cubes();
        let key = CubeCatalogKey {
            chain: ChainId::Bitcoin,
            cube_id: "second".into(),
        };
        for size in [(1024, 768), (1440, 900)] {
            for (appearance, palette) in [
                ("dark", theme::Theme::dark()),
                ("light", theme::Theme::light()),
            ] {
                let mounted = capture::inspect(
                    "v113-launcher-list",
                    list_body(&home),
                    size,
                    appearance,
                    palette,
                );
                assert!(mounted.contains("Everyday"));
                assert!(mounted.contains("Travel"));
                assert!(mounted.contains("Locked"));
                assert!(mounted.contains("Status unknown"));
                assert!(!mounted.contains("Balance"));
                assert!(!mounted.contains("Checks passed"));
                assert!(mounted.has_focus(search_id().into()));
                assert!(mounted.has_focus(launcher_catalog::select_id(&key)));
                assert!(mounted.has_focus(launcher_catalog::open_id(&key)));
                assert!(mounted.has_focus(launcher_catalog::details_open_id(&key)));
                assert!(mounted.has_focus(launcher_catalog::close_details_id()));
            }
        }
    }

    #[test]
    fn worldview_alias_in_list_keeps_the_exact_network_in_cube_details() {
        let home = home_with_cubes();
        let mounted = capture::inspect(
            "launcher-list-luke-worldview",
            list_body_with_worldview(
                &home,
                crate::app::settings::preferences::WorldviewMode::Luke,
            ),
            (1024, 768),
            "dark",
            theme::Theme::dark(),
        );
        assert!(mounted.contains("Spamcoin (BTC)"));
        assert!(mounted.contains("Bitcoin SHA-256 · Mainnet"));
        assert!(
            mounted.has_focus(launcher_catalog::open_id(&CubeCatalogKey {
                chain: ChainId::Bitcoin,
                cube_id: "second".into(),
            }))
        );
    }

    #[test]
    fn worldview_create_backdrop_uses_alias_and_sheet_uses_exact_chain() {
        use crate::app::settings::preferences::WorldviewMode;

        let mut home = home_with_cubes();
        let super::super::State::Cubes { create_cube, .. } = &mut home.state else {
            panic!("expected the fixture to have local Cubes");
        };
        *create_cube = true;
        home.catalog.mode = launcher_catalog::LauncherMode::List;
        home.catalog.selected = None;
        assert!(home.showing_create_journey());

        // The inert wrapper deliberately hides background widgets from the
        // inspector's operations. Inspect the backdrop before composition.
        let backdrop = capture::inspect_filled(
            "launcher-create-luke-backdrop",
            launcher_catalog::launcher_backdrop(&home, WorldviewMode::Luke),
            (1440, 900),
            "dark",
            theme::Theme::dark(),
            (1440, 900),
        );
        assert!(backdrop.contains("Spamcoin (BTC)"));
        let sheet = capture::inspect_filled(
            "launcher-create-luke-sheet",
            home.view_with_worldview(WorldviewMode::Luke),
            (1440, 900),
            "dark",
            theme::Theme::dark(),
            (1440, 900),
        );
        assert!(sheet.contains("Bitcoin SHA-256 · Mainnet"));
        assert!(!sheet.contains("Spamcoin (BTC)"));
    }

    #[test]
    fn stale_rows_remain_visible_but_not_keyboard_actionable() {
        let mut home = home_with_cubes();
        if let super::super::State::Cubes { source, .. } = &mut home.state {
            *source = ChainId::Testnet4;
        }
        let key = CubeCatalogKey {
            chain: ChainId::Bitcoin,
            cube_id: "second".into(),
        };
        let mounted = capture::inspect(
            "v113-launcher-stale",
            list_body(&home),
            (1024, 768),
            "dark",
            theme::Theme::dark(),
        );
        assert!(mounted.contains("Travel"));
        assert!(!mounted.has_focus(launcher_catalog::select_id(&key)));
        assert!(!mounted.has_focus(launcher_catalog::open_id(&key)));
    }

    #[test]
    fn search_and_remote_restore_keep_the_catalog_intents() {
        let mut home = home_with_cubes();
        home.remote_cubes.push(super::super::RemoteCube {
            id: 7,
            uuid: "remote-id".into(),
            name: "Travel backup".into(),
            network: crate::app::settings::network_to_api_string(ChainId::Bitcoin),
            has_recovery_kit: true,
            has_encrypted_seed: true,
            phone_recoverable: false,
            phone_full_cube: false,
            encryption_pubkey: None,
            has_vault: None,
        });
        home.catalog.query = "Travel".into();
        let mounted = capture::inspect(
            "v113-launcher-search-remote",
            list_body(&home),
            (1440, 900),
            "dark",
            theme::Theme::dark(),
        );
        assert!(mounted.contains("Travel"));
        assert!(!mounted.contains("Everyday"));
        assert!(mounted.contains("Recovery Kit available"));
        assert!(mounted.has_focus(clear_id()));
        assert!(mounted.has_focus(launcher_catalog::restore_id("remote-id")));
        assert!(mounted.has_focus(launcher_catalog::remote_delete_id("remote-id")));
    }

    #[test]
    fn wide_table_mount_keeps_independent_open_and_details_targets() {
        let home = home_with_cubes();
        let projection = launcher_catalog::project(&home);
        let key = CubeCatalogKey {
            chain: ChainId::Bitcoin,
            cube_id: "second".into(),
        };
        let mounted = capture::inspect(
            "v113-launcher-wide-table",
            list_layout(
                &projection,
                &project_catalog(&projection),
                1400.0,
                true,
                Default::default(),
            ),
            (2000, 900),
            "light",
            theme::Theme::light(),
        );
        assert!(mounted.contains("Network"));
        assert!(mounted.contains("Wallets"));
        assert!(mounted.has_focus(launcher_catalog::select_id(&key)));
        assert!(mounted.has_focus(launcher_catalog::open_id(&key)));
        assert!(mounted.has_focus(launcher_catalog::details_open_id(&key)));
    }

    #[test]
    fn artwork_picker_live_opening_keeps_status_and_stall_actions() {
        use super::super::launcher_opening::{Opening, Stage};
        use coincube_ui::theme::{ArtworkStyle, WorldviewMode};
        use std::time::{Duration, Instant};

        let cube = crate::app::settings::CubeSettings::new_with_raw_id(
            "fixture".into(),
            "Everyday".into(),
            ChainId::Bitcoin,
        );
        let quote = Quote {
            id: "fixture".into(),
            text: "Verify the network before you send. An illustration can change with your worldview, while the network and the next action stay the same.".into(),
            author: "Author".into(),
            source: "Source".into(),
            category: "loading".into(),
            length: "long".into(),
            contexts: Vec::new(),
        };
        let started = Instant::now();
        let now = started + Duration::from_secs(20);
        for height in [768, 512] {
            for floor in [None, Some(floor_context(&cube))] {
                let opening = Opening::new(floor, false, Stage::Spark, started);
                for artwork in [ArtworkStyle::Existing, ArtworkStyle::Scene] {
                    let face = OpeningFace {
                        cube: &cube,
                        quote: &quote,
                        outgoing: None,
                        stage: opening.stage(),
                        stalled: true,
                        cancel: || (),
                        without_spark: || (),
                    };
                    let theme = theme::Theme::dark()
                        .with_worldview(WorldviewMode::Suitcoiner)
                        .with_artwork(artwork);
                    let (layout, element) = match opening.floor() {
                        Some(context) => (
                            "floor",
                            opening_on_floor(context, face, theme.mode, &opening, now),
                        ),
                        None => ("list", opening_full_screen(face, theme.mode, &opening, now)),
                    };
                    let mounted = capture::inspect_filled(
                        &format!("artwork-live-opening-{layout}-{artwork:?}"),
                        element,
                        (1024, height),
                        "dark",
                        theme,
                        (1024, height),
                    );
                    assert!(mounted.contains("Loading your Spark wallet"));
                    let action = mounted
                        .text
                        .iter()
                        .find(|(text, _)| text == "Continue without Spark")
                        .unwrap();
                    assert!(action.1.y + action.1.height <= height as f32);
                }
            }
        }
    }

    #[test]
    fn mounted_opening_keeps_real_loader_status_and_scan_guidance() {
        let quote = Quote {
            id: "fixture".into(),
            text: "A source quote".into(),
            author: "Author".into(),
            source: "Source".into(),
            category: "loading".into(),
            length: "short".into(),
            contexts: Vec::new(),
        };
        let (bytes, _) =
            coincube_ui::component::quote_display::default_image_for_context("loading");
        let image = iced::widget::image::Handle::from_bytes(bytes);
        for (step, expected) in [
            (loader::Step::Connecting, "Connecting to daemon..."),
            (
                loader::Step::FullScan { progress: 0.5 },
                "Scanning the blockchain...",
            ),
        ] {
            let scene = project_opening("Example\n\u{202e} Wallet", ChainId::Bitcoin);
            for (appearance, palette) in [
                ("dark", theme::Theme::dark()),
                ("light", theme::Theme::light()),
            ] {
                let mounted = capture::inspect(
                    "v113-opening",
                    opening_body(scene.clone(), &step, &quote, &image, false),
                    (1024, 768),
                    appearance,
                    palette,
                );
                assert!(mounted.contains("Tenshu"));
                assert!(mounted.contains("Opening Example Wallet"));
                assert!(mounted.contains("Bitcoin"));
                assert!(mounted.contains(expected));
                assert!(mounted.contains("A source quote"));
                assert!(!mounted.contains("Checks passed"));
                if matches!(step, loader::Step::FullScan { .. }) {
                    assert!(mounted.contains("Performing an initial scan"));
                }
            }
        }
    }

    #[test]
    fn mounted_opening_wraps_long_latin_and_cjk_names_at_compact_width() {
        let quote = Quote {
            id: "fixture".into(),
            text: "A source quote".into(),
            author: "Author".into(),
            source: "Source".into(),
            category: "loading".into(),
            length: "short".into(),
            contexts: Vec::new(),
        };
        let (bytes, _) =
            coincube_ui::component::quote_display::default_image_for_context("loading");
        let image = iced::widget::image::Handle::from_bytes(bytes);
        let step = loader::Step::Connecting;
        for raw_name in ["W".repeat(80), "旅".repeat(80)] {
            let scene = project_opening(&raw_name, ChainId::Bitcoin);
            for (appearance, palette) in [
                ("dark", theme::Theme::dark()),
                ("light", theme::Theme::light()),
            ] {
                // `inspect` mounts the actual element and asserts every text
                // bound stays inside its compact viewport.
                let mounted = capture::inspect(
                    "v113-opening-long-name",
                    opening_body(scene.clone(), &step, &quote, &image, false),
                    (1024, 768),
                    appearance,
                    palette,
                );
                assert!(mounted.contains("Opening"));
                assert!(mounted.contains("Connecting to daemon..."));
                assert!(mounted.contains("A source quote"));
            }
        }
    }

    #[test]
    fn catalog_projection_keeps_unfiltered_counts() {
        let catalog = Projection {
            connect_trouble: None,
            locals: Vec::new(),
            remotes: Vec::new(),
            local_total: 3,
            remote_total: 2,
            ghosts: Vec::new(),
            selected: None,
            selected_ghost: None,
            query: "hidden".into(),
            network: ChainId::Bitcoin,
            total_count: 5,
            cube_limit: 10,
            tier: "Free",
            usage: Vec::new(),
            hidden: 0,
            hidden_ghosts: 0,
            filtered_total: 3,
            unread: Vec::new(),
            network_counts: vec![(ChainId::Bitcoin, 3)],
            filter: launcher_catalog::NetworkFilter::All,
        };
        let scene = project_catalog(&catalog);
        assert_eq!((scene.local_total, scene.remote_total), (3, 2));
        assert_eq!(scene.query, "hidden");
        assert!(scene.selected.is_none());
    }
}
