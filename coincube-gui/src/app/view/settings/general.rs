use coincube_ui::locale::copy as localized_copy;
use coincube_ui::localized_format;
use iced::widget::{mouse_area, Column, Container, Row, Space};
use iced::{Alignment, Length};

use coincube_ui::component::tenshu::{
    segment, segmented, select, setting_row, toggle, SegmentSize, Tone,
};
use coincube_ui::component::text::*;
use coincube_ui::theme;
use coincube_ui::widget::Element;

use crate::app::cache;
use crate::app::menu::{CubeSettingsOption, CubeSubMenu, Menu, SettingsOption, VaultSubMenu};
use crate::app::settings::display::DisplayMode;
use crate::app::settings::fiat::PriceSetting;
use crate::app::settings::unit::{BitcoinDisplayUnit, UnitSetting};
use crate::app::state::settings::general::SettingsSection;
use crate::app::state::settings::recovery_alerts::RecoveryAlerts;
use crate::app::state::settings::recovery_kit::RecoveryKit;
use crate::app::view::dashboard;
use crate::app::view::message::*;
use crate::app::view::settings::page;
use crate::services::coincube::{RecoveryKitStatus, VaultMemberRole};
use crate::services::fiat::{Currency, ALL_PRICE_SOURCES};
use crate::services::inheritance::EscrowTier;

pub const RECOVERY_ALERTS_VIEW_PLANS_ID: &str = "tenshu.settings.recovery-alerts.view-plans";
pub const ESCROW_VIEW_PLANS_ID: &str = "tenshu.settings.escrow.view-plans";

/// Render whichever settings sub-tab is active. The General and Recovery tabs
/// share `GeneralSettingsState`; this dispatches on the section discriminator.
#[allow(clippy::too_many_arguments)]
pub fn settings_content_section<'a>(
    section: SettingsSection,
    menu: &'a Menu,
    cache: &'a cache::Cache,
    new_price_setting: &'a PriceSetting,
    new_unit_setting: &'a UnitSetting,
    currencies_list: &'a [Currency],
    show_direction_badges: bool,
    saved: bool,
    backup_state: &'a crate::app::state::settings::general::BackupSeedState,
    backup_pin: &'a crate::pin_input::PinInput,
    backup_mnemonic: Option<&'a [String]>,
    recovery_kit: Option<&'a RecoveryKit>,
    recovery_alerts: Option<&'a RecoveryAlerts>,
    name_input: Option<&'a str>,
    confirm_checks_off: bool,
) -> Element<'a, Message> {
    match section {
        SettingsSection::General => general_section(
            menu,
            cache,
            new_price_setting,
            new_unit_setting,
            currencies_list,
            show_direction_badges,
            saved,
            name_input,
        ),
        SettingsSection::PaymentChecks => {
            payment_checks_section(menu, cache, saved, confirm_checks_off)
        }
        SettingsSection::DataExports => data_exports_section(menu, cache),
        SettingsSection::Recovery => recovery_section(
            menu,
            cache,
            backup_state,
            backup_pin,
            backup_mnemonic,
            recovery_kit,
            recovery_alerts,
        ),
    }
}

/// Cube display preferences. Each control keeps its existing persistence path:
/// Cube amount settings save per Cube, while direction badges save per device.
#[allow(clippy::too_many_arguments)]
fn general_section<'a>(
    menu: &'a Menu,
    cache: &'a cache::Cache,
    new_price_setting: &'a PriceSetting,
    new_unit_setting: &'a UnitSetting,
    currencies_list: &'a [Currency],
    show_direction_badges: bool,
    saved: bool,
    name_input: Option<&'a str>,
) -> Element<'a, Message> {
    dashboard(
        menu,
        cache,
        display_content(
            cache,
            new_price_setting,
            new_unit_setting,
            currencies_list,
            show_direction_badges,
            saved,
            name_input,
            None,
        ),
    )
}

pub const CUBE_NAME_ID: &str = "tenshu.settings.cube-name";
pub const AVATAR_ACTION_ID: &str = "tenshu.settings.avatar";

/// The Display & appearance groups (`#h-amt`, `#h-id`). `avatar_flow` is the
/// avatar set-up or regeneration flow, shown inline under the Avatar row
/// while it is open.
#[allow(clippy::too_many_arguments)]
pub(crate) fn display_content<'a>(
    cache: &'a cache::Cache,
    new_price_setting: &'a PriceSetting,
    new_unit_setting: &'a UnitSetting,
    currencies_list: &'a [Currency],
    show_direction_badges: bool,
    saved: bool,
    name_input: Option<&'a str>,
    avatar_flow: Option<Element<'a, Message>>,
) -> Element<'a, Message> {
    let amounts = Column::new()
        .push(page::rule())
        .push(amounts_and_currency(
            new_unit_setting,
            cache.display_mode,
            cache.fiat_toggle_allowed(),
            new_price_setting,
            currencies_list,
            cache.fiat_chain.is_blake2b(),
            show_direction_badges,
        ))
        .push(page::saved_line(saved));

    let name = name_input.unwrap_or(cache.cube_name.as_str());
    let name_row = page::row(localized_copy("Cube name"))
        .description(localized_copy(
            "Saved when you press Return or leave the field.",
        ))
        .control(
            Container::new(
                coincube_ui::component::tenshu::text_field(
                    CUBE_NAME_ID,
                    "Cube name",
                    name,
                    Message::FocusAction,
                )
                .hide_label()
                .size(coincube_ui::component::tenshu::FieldSize::Compact)
                .width(Length::Fixed(280.0))
                .on_input(|value| SettingsMessage::CubeNameEdited(value).into())
                .on_submit(SettingsMessage::CubeNameSubmitted(name.to_owned()).into()),
            )
            .width(Length::Fixed(280.0)),
        );

    let flow_open = avatar_flow.is_some();
    let mut identity = Column::new()
        .width(Length::Fill)
        .push(page::rows([name_row.into(), avatar_row(cache, flow_open)]));
    if let Some(flow) = avatar_flow {
        identity = identity.push(
            Container::new(flow)
                .padding(iced::Padding::default().top(16.0))
                .width(Length::Fill),
        );
    }

    Column::new()
        .width(Length::Fill)
        .push(page::group(
            localized_copy("Amounts & currency"),
            None,
            amounts,
        ))
        .push(page::group(localized_copy("Cube identity"), None, identity))
        .into()
}

/// The Avatar row: the 48px tile, how many regenerations remain and the
/// action that opens the avatar flow inline.
fn avatar_row<'a>(cache: &'a cache::Cache, flow_open: bool) -> Element<'a, Message> {
    let tile: Element<'a, Message> = match cache.avatar_handle.as_ref() {
        Some(handle) => Container::new(
            iced::widget::image(handle.clone())
                .width(Length::Fixed(48.0))
                .height(Length::Fixed(48.0))
                .border_radius(12.0),
        )
        .into(),
        None => Container::new(text("No avatar").size(11).style(theme::text::secondary))
            .width(Length::Fixed(48.0))
            .height(Length::Fixed(48.0))
            .center_x(Length::Fixed(48.0))
            .center_y(Length::Fixed(48.0))
            .style(|t: &theme::Theme| iced::widget::container::Style {
                border: iced::Border {
                    color: t.colors.shell.muted,
                    width: 1.0,
                    radius: 12.0.into(),
                },
                ..Default::default()
            })
            .into(),
    };
    let signed_in = cache.connect_authenticated;
    let detail = if !signed_in {
        "Needs Connect sign-in.".to_owned()
    } else if cache.avatar_handle.is_none() {
        "Not set up yet.".to_owned()
    } else {
        match cache.avatar_regenerations_remaining {
            Some(n) if n >= 0 => {
                localized_format!("Regenerations left: {}", "Regeneraciones restantes: {}", n)
            }
            _ => "Generated from your Lightning Address.".to_owned(),
        }
    };
    let (label, target) = if flow_open {
        (
            "Close",
            Menu::Cube(CubeSubMenu::Settings(CubeSettingsOption::General)),
        )
    } else if cache.avatar_handle.is_some() {
        (
            "Regenerate…",
            Menu::Cube(CubeSubMenu::Settings(CubeSettingsOption::Avatar)),
        )
    } else {
        (
            "Set up…",
            Menu::Cube(CubeSubMenu::Settings(CubeSettingsOption::Avatar)),
        )
    };
    page::row("Avatar")
        .leading(tile)
        .description(detail)
        .control(page::button(
            AVATAR_ACTION_ID,
            label,
            signed_in.then_some(Message::Menu(target)),
        ))
        .into()
}

fn payment_checks_section<'a>(
    menu: &'a Menu,
    cache: &'a cache::Cache,
    saved: bool,
    confirming_off: bool,
) -> Element<'a, Message> {
    let enabled = crate::services::branta::enabled();
    let mut rows: Vec<Element<'a, Message>> = vec![page::row("Check payment requests")
        .control(
            toggle(CHECKS_TOGGLE_ID, enabled, Message::FocusAction).on_toggle(move |on| {
                if on {
                    SettingsMessage::ToggleRecipientIdentityChecks(true).into()
                } else {
                    SettingsMessage::AskRecipientChecksOff.into()
                }
            }),
        )
        .into()];
    if confirming_off && enabled {
        rows.push(
            Column::new()
                .push(page::note(
                    Tone::Warn,
                    Row::new()
                        .spacing(4)
                        .push(
                            text(localized_copy("Turn off recipient checks?"))
                                .size(13)
                                .font(coincube_ui::font::SEMIBOLD),
                        )
                        .push(text(localized_copy("Sends will skip the identity check.")).size(13))
                        .wrap(),
                    Some(
                        Row::new()
                            .spacing(8)
                            .push(page::primary(
                                CHECKS_OFF_CONFIRM_ID,
                                localized_copy("Turn off"),
                                Some(SettingsMessage::ToggleRecipientIdentityChecks(false).into()),
                            ))
                            .push(page::button(
                                CHECKS_OFF_CANCEL_ID,
                                localized_copy("Cancel"),
                                Some(SettingsMessage::CancelRecipientChecksOff.into()),
                            ))
                            .into(),
                    ),
                ))
                .push(Space::new().height(Length::Fixed(12.0)))
                .into(),
        );
    }
    rows.push(
        page::row(localized_copy("Strict mode"))
            .description(localized_copy(
                "Strict mode does not share the plaintext destination or payment amount. \
                 Branta can still observe lookup timing and your network connection.",
            ))
            .line(page::link(
                BRANTA_PRIVACY_ID,
                localized_copy("Branta privacy information"),
                Message::OpenUrl("https://branta.pro/your-data".into()),
            ))
            .control(page::status(page::Status::Ok, localized_copy("Always on")))
            .top()
            .into(),
    );
    let content = Column::new()
        .width(Length::Fill)
        .push(page::group(
            localized_copy("Recipient checks"),
            Some("Before you send, Tenshu asks Branta whether it can identify the recipient. This applies to every Cube on this device.".into()),
            page::rows(rows),
        ))
        .push(page::caption(
            "A check never guarantees who the recipient is. Recipients Branta can\u{2019}t identify show as \u{201c}Not identified\u{201d}.",
        ))
        .push(page::saved_line(saved));
    dashboard(menu, cache, content)
}

pub const CHECKS_TOGGLE_ID: &str = "tenshu.settings.checks.toggle";
pub const CHECKS_OFF_CONFIRM_ID: &str = "tenshu.settings.checks.off";
pub const CHECKS_OFF_CANCEL_ID: &str = "tenshu.settings.checks.off-cancel";
pub const BRANTA_PRIVACY_ID: &str = "tenshu.settings.checks.branta-privacy";

fn recovery_kit_route() -> Menu {
    Menu::Cube(CubeSubMenu::Settings(CubeSettingsOption::Recovery))
}

/// Imported Bitcoin has a Vault scope, but it has no `VaultSettingsState` or
/// `ImportExportSettingsState`. Keep its route unavailable until it has a
/// dedicated data export flow.
fn vault_data_route(cache: &cache::Cache) -> Option<Menu> {
    (cache.has_vault && !cache.has_imported_bitcoin).then_some(Menu::Vault(VaultSubMenu::Settings(
        Some(SettingsOption::ImportExport),
    )))
}

fn data_exports_section<'a>(menu: &'a Menu, cache: &'a cache::Cache) -> Element<'a, Message> {
    let vault_detail = if cache.has_imported_bitcoin {
        "Imported Bitcoin data exports are unavailable in this version."
    } else if cache.has_vault {
        "Exported per Vault."
    } else {
        "No Vault in this Cube."
    };

    let rows = page::rows([
        page::row(localized_copy("Recovery Kit"))
            .description(localized_copy("Managed in Backup & protection."))
            .control(page::button(
                DATA_BACKUP_ID,
                localized_copy("Open Backup & protection"),
                Some(Message::Menu(recovery_kit_route())),
            ))
            .into(),
        page::row(localized_copy("Vault files and records"))
            .description(vault_detail)
            .control(page::button(
                DATA_VAULT_ID,
                localized_copy("Open Vault data"),
                vault_data_route(cache).map(Message::Menu),
            ))
            .into(),
    ]);
    // No heading: it would only repeat the tab name.
    dashboard(
        menu,
        cache,
        Column::new()
            .width(Length::Fill)
            .push(Space::new().height(Length::Fixed(page::GROUP_GAP)))
            .push(rows),
    )
}

pub const DATA_BACKUP_ID: &str = "tenshu.settings.data.backup";
pub const DATA_VAULT_ID: &str = "tenshu.settings.data.vault";

/// Recovery tab: local paper backup + Connect Recovery Kit + Vault Recovery
/// Alerts. Hosts the local-backup wizard page takeover (unchanged from when it
/// lived on General).
#[allow(clippy::too_many_arguments)]
fn recovery_section<'a>(
    menu: &'a Menu,
    cache: &'a cache::Cache,
    backup_state: &'a crate::app::state::settings::general::BackupSeedState,
    backup_pin: &'a crate::pin_input::PinInput,
    backup_mnemonic: Option<&'a [String]>,
    recovery_kit: Option<&'a RecoveryKit>,
    recovery_alerts: Option<&'a RecoveryAlerts>,
) -> Element<'a, Message> {
    use crate::app::state::settings::general::BackupSeedState;

    // When the local backup flow is active, take over the entire settings page
    // with the wizard view. This keeps the multi-step PIN → reveal flow focused.
    if !matches!(backup_state, BackupSeedState::None) {
        if let Some(wizard) =
            super::backup::dispatch(backup_state, backup_pin, backup_mnemonic, cache.chain())
        {
            return dashboard(menu, cache, Column::new().spacing(20).push(wizard));
        }
    }

    dashboard(
        menu,
        cache,
        recovery_overview(cache, recovery_kit, recovery_alerts),
    )
}

/// The overview body is separate from the shell so synthetic captures exercise
/// the production rows without mounting a second scrolling dashboard.
fn recovery_overview<'a>(
    cache: &'a cache::Cache,
    recovery_kit: Option<&'a RecoveryKit>,
    recovery_alerts: Option<&'a RecoveryAlerts>,
) -> Element<'a, Message> {
    let backed_up = cache.current_cube_backed_up;
    let seed_start: Message = SettingsMessage::BackupMasterSeed(BackupWalletMessage::Start).into();
    let seed_row = page::row(localized_copy("Seed phrase"))
        .accessory(if backed_up {
            page::status(page::Status::Ok, localized_copy("Confirmed"))
        } else {
            page::status(page::Status::Warn, localized_copy("Not confirmed"))
        })
        .description(localized_copy("12 words, written down offline."))
        .control(if backed_up {
            page::button(
                SEED_REVEAL_ID,
                localized_copy("Reveal seed phrase…"),
                Some(seed_start),
            )
        } else {
            page::primary(
                SEED_REVEAL_ID,
                localized_copy("Confirm seed phrase"),
                Some(seed_start),
            )
        });

    let mut methods = vec![seed_row.into()];
    if let Some(rk) = recovery_kit {
        methods.push(recovery_kit_row(cache, rk));
    }

    let mut col = Column::new().width(Length::Fill);
    if let Some(notice) = cache.unswept_recovery_notice() {
        col = col.push(page::note(
            Tone::Muted,
            page::muted(localized_copy(notice)),
            None,
        ));
    }
    col = col.push(page::group(
        localized_copy("Backup methods"),
        None,
        page::rows(methods),
    ));

    let path_row = if cache.has_vault {
        page::row(localized_copy("Recovery path"))
            .description(localized_copy(
                "Review the Vault's recovery spending path and timelocks in its tools.",
            ))
            .control(page::button(
                RECOVERY_TOOLS_ID,
                localized_copy("Open recovery tools"),
                Some(Message::Menu(Menu::Vault(VaultSubMenu::Recovery))),
            ))
    } else {
        page::row(localized_copy("Recovery path"))
            .description(localized_copy("No Vault in this Cube."))
            .control(page::status(
                page::Status::Off,
                localized_copy("Not applicable"),
            ))
    };
    let mut path_rows = vec![path_row.into()];
    let mut alerts_controls = None;
    if let Some(ra) = recovery_alerts {
        path_rows.push(recovery_alerts_row(ra, cache.has_vault));
        if ra.managing && cache.has_vault && ra.entitled && !ra.no_vault && ra.status.is_some() {
            let spark = crate::app::features::spark(cache.chain()).is_available();
            alerts_controls = Some(recovery_alerts_card(ra, spark));
        }
    }
    let mut path = Column::new()
        .width(Length::Fill)
        .push(page::rows(path_rows));
    if let Some(controls) = alerts_controls {
        path = path.push(page::disclosure_body([controls]));
    }
    col = col.push(page::group(
        localized_copy("Vault recovery path"),
        Some(localized_copy("Spending, not backup. A backup never unlocks it.").into()),
        path,
    ));

    col.into()
}

pub const SEED_REVEAL_ID: &str = "tenshu.settings.backup.seed";
pub const KIT_PRIMARY_ID: &str = "tenshu.settings.backup.kit";
pub const KIT_REMOVE_ID: &str = "tenshu.settings.backup.kit-remove";
pub const RECOVERY_TOOLS_ID: &str = "tenshu.settings.backup.recovery-tools";
pub const ALERTS_MANAGE_ID: &str = "tenshu.settings.backup.alerts";

/// The Recovery Kit row (`#recovery-kit`): its status beside the label, what
/// protects it and what it holds below, and the action for its state.
fn recovery_kit_row<'a>(cache: &'a cache::Cache, rk: &'a RecoveryKit) -> Element<'a, Message> {
    let row = page::row(localized_copy("Recovery Kit")).description(localized_copy(
        "Encrypted copy kept with your Connect account.",
    ));
    if !cache.connect_authenticated && !cache.restorable_connect_session {
        return row
            .accessory(page::status(
                page::Status::Unknown,
                localized_copy("Sign in required"),
            ))
            .control(page::button(
                KIT_PRIMARY_ID,
                localized_copy("Sign in"),
                Some(Message::OpenConnectSignIn),
            ))
            .into();
    }
    if let Some((label, action)) = unregistered_cube_action(cache) {
        return row
            .accessory(page::status(
                page::Status::Unknown,
                localized_copy("Reconnect required"),
            ))
            .control(page::button(KIT_PRIMARY_ID, label, Some(action)))
            .into();
    }
    let overview = backup_overview(rk.status.as_ref());
    let drift = descriptor_drift(
        cache.current_descriptor_fingerprint.as_deref(),
        overview.password.map(|m| m.descriptor).unwrap_or(false),
        cache
            .recovery_kit_last_backed_up_descriptor_fingerprint
            .as_deref(),
        overview.keychain.map(|m| m.descriptor).unwrap_or(false),
        cache
            .recovery_kit_last_backed_up_keychain_descriptor_fingerprint
            .as_deref(),
    );
    recovery_kit_card(
        cache.has_vault || cache.has_imported_bitcoin,
        rk.status.as_ref(),
        rk.status_loading,
        rk.status_stale,
        drift,
        cache.has_imported_bitcoin && cache.current_descriptor_fingerprint.is_none(),
        row,
    )
}

/// The Recovery alerts row: eligibility and state beside the label, with
/// "Manage alerts" opening the alerts controls beneath the rows.
fn recovery_alerts_row<'a>(ra: &'a RecoveryAlerts, has_vault: bool) -> Element<'a, Message> {
    let manage = |label: &'static str| {
        page::button(
            ALERTS_MANAGE_ID,
            if ra.managing { "Close" } else { label },
            Some(SettingsMessage::RecoveryAlerts(RecoveryAlertsMessage::ToggleManage).into()),
        )
    };
    let row = page::row(localized_copy("Recovery alerts"));
    let row = if !has_vault || ra.no_vault {
        row.accessory(page::status(
            page::Status::Off,
            localized_copy("Not applicable"),
        ))
        .description(localized_copy(
            "Needs a Vault with a recovery path, registered with Connect.",
        ))
    } else if !ra.entitled {
        row.accessory(page::status(
            page::Status::Off,
            localized_copy("Not included"),
        ))
        .description(localized_copy("Not in your plan. Verified by Connect."))
        .control(page::button(
            RECOVERY_ALERTS_VIEW_PLANS_ID,
            localized_copy("Compare plans"),
            Some(Message::OpenPlanBillingFrom(RECOVERY_ALERTS_VIEW_PLANS_ID)),
        ))
    } else if ra.loading && ra.status.is_none() {
        row.accessory(page::status(
            page::Status::Busy,
            localized_copy("Checking…"),
        ))
        .description(localized_copy("Still checking. No plan change implied."))
    } else if ra.status.is_none() {
        row.accessory(page::status(
            page::Status::Unknown,
            localized_copy("Eligibility unknown"),
        ))
        .description(localized_copy("Sign in to Connect to check eligibility."))
    } else if ra.alerts_on() {
        row.accessory(page::status(page::Status::Ok, localized_copy("On")))
            .description(localized_copy(
                "Alerts your recovery contacts when the path opens.",
            ))
            .control(manage("Manage alerts"))
    } else {
        row.accessory(page::status(page::Status::Off, localized_copy("Off")))
            .description(localized_copy(
                "Your keyholders are not alerted when the path opens.",
            ))
            .control(manage("Manage alerts"))
    };
    row.into()
}

/// Refresh this Cube tab's Connect panel directly. The shell's sign-in intent
/// opens an account-wide sheet and may navigate to Launcher instead.
fn unregistered_cube_action(cache: &cache::Cache) -> Option<(&'static str, Message)> {
    if cache.current_cube_server_id.is_some() {
        None
    } else if cache.connect_authenticated {
        Some((
            "Retry Cube connection",
            Message::ConnectCube(ConnectCubeMessage::RetryRegistration),
        ))
    } else if cache.restorable_connect_session {
        Some((
            "Restore Connect session",
            Message::ConnectAccount(ConnectAccountMessage::Init),
        ))
    } else {
        None
    }
}

/// Vault Recovery Alerts card. Two independent controls backed by two server
/// records: the **Recovery alerts** toggle (monitoring; not Estate-gated) and
/// the **What keyholders can recover** escrow selector (Estate-gated). The
/// keyholder list lives under the alerts toggle (it's who alerts notify). See
/// `PLAN-recovery-alerts-cleanup.md` PR 2.
/// `spark`: whether this Cube's network has Spark, for the Full Cube copy.
fn recovery_alerts_card<'a>(ra: &'a RecoveryAlerts, spark: bool) -> Element<'a, Message> {
    let alerts_on = ra.alerts_on();
    let busy = ra.submitting;
    let mut body = Column::new().spacing(0).width(Length::Fill);

    // ── Control 1: the Recovery alerts toggle (monitoring on/off) ──────────
    body = body.push(
        page::row(localized_copy("Recovery alerts"))
            .description(localized_copy(
                "Let COINCUBE watch the chain and alert your keyholders when a recovery path \
                 for this Vault opens.",
            ))
            .control(
                toggle(ALERTS_TOGGLE_ID, alerts_on, Message::FocusAction).on_toggle_maybe(
                    (!busy).then_some(|on| {
                        SettingsMessage::RecoveryAlerts(RecoveryAlertsMessage::ToggleAlerts(on))
                            .into()
                    }),
                ),
            ),
    );
    // The C4 disclosure — stated plainly, no euphemisms.
    if alerts_on {
        body = body.push(page::caption(localized_copy(
            "When on, COINCUBE learns only the block height at which this Vault's recovery \
             window opens and that this desktop checked in — never your addresses or balances.",
        )));
    }

    // Residual nudge (PR 3): with keyholders present but alerts off, they'd
    // never be told when the recovery window opens.
    if !alerts_on && !ra.recipients.notified.is_empty() {
        body = body.push(page::note(
            Tone::Warn,
            page::muted(localized_copy(
                "Your keyholders will NOT be alerted when this Vault's recovery window opens. \
                 Turn on Recovery alerts to fix this.",
            )),
            None,
        ));
    }

    // Inline confirm for turning alerts off. Never silent: when a recovery kit
    // is escrowed, the confirm discloses that it will be deleted too (escrow
    // can't outlive alerts).
    if ra.confirming_alerts_off {
        let warning = if ra.has_escrow() {
            "Turn off recovery alerts? COINCUBE will stop watching for this Vault's recovery \
             window. This also deletes the encrypted recovery kit currently stored for your \
             keyholders — they'll no longer be able to recover this Vault."
        } else {
            "Turn off recovery alerts? COINCUBE will stop watching for this Vault's recovery \
             window, so your keyholders won't be alerted when it opens."
        };
        body = body.push(page::note(
            Tone::Bad,
            text(localized_copy(warning)).size(13),
            Some(
                Row::new()
                    .spacing(8)
                    .push(page::primary(
                        ALERTS_OFF_CONFIRM_ID,
                        localized_copy("Turn off"),
                        (!busy).then_some(
                            SettingsMessage::RecoveryAlerts(
                                RecoveryAlertsMessage::ConfirmAlertsOff,
                            )
                            .into(),
                        ),
                    ))
                    .push(page::button(
                        ALERTS_OFF_CANCEL_ID,
                        localized_copy("Cancel"),
                        Some(
                            SettingsMessage::RecoveryAlerts(RecoveryAlertsMessage::CancelAlertsOff)
                                .into(),
                        ),
                    ))
                    .into(),
            ),
        ));
    }

    // The keyholder list belongs to alerts (who'd be notified).
    if alerts_on {
        let mut who = Column::new().spacing(2).width(Length::Fill);
        if ra.recipients.notified.is_empty() {
            // Deliberately does *not* say "add keyholders": a Vault's
            // keyholders are its signers, fixed when the descriptor is sealed.
            who = who.push(page::muted(localized_copy(
                "No one would be alerted for this Vault. Alert recipients are this Vault's \
                 keyholders and beneficiaries; its keyholders are fixed when the Vault is \
                 built, so they can't be added to an existing Vault.",
            )));
        } else {
            for r in &ra.recipients.notified {
                // Tag beneficiaries so a mixed list stays honest under a
                // heading that says "keyholders". Keyholders read plain.
                let line = match r.role {
                    VaultMemberRole::Keyholder => r.email.clone(),
                    role => format!("{} — {}", r.email, role),
                };
                who = who.push(text(line).size(13));
            }
        }
        // Members who hold a notifiable role but have no address on file.
        if ra.recipients.unreachable > 0 {
            let n = ra.recipients.unreachable;
            let (noun, verb) = if n == 1 {
                ("member", "has")
            } else {
                ("members", "have")
            };
            who = who.push(page::muted(localized_format!(
                "{} {} of this Vault {} no email on file, so COINCUBE can't alert them.", "{} {} de este Vault {} no email en el archivo, por lo que COINCUBE no puede alertarlos.",
                n, noun, verb,
            )));
        }
        body = body.push(
            page::row(localized_copy("Keyholders who'd be notified"))
                .line(who)
                .top(),
        );
    }

    // ── Control 2: the escrow selector (what keyholders can recover) ───────
    body = body.push(
        page::row(localized_copy("What keyholders can recover"))
            .line(escrow_selector(ra, alerts_on, busy, spark))
            .top(),
    );

    if let Some(err) = ra.error.as_deref() {
        body = body.push(page::caption(coincube_ui::locale::notice(err.to_owned())));
    }

    body.into()
}

pub const ALERTS_TOGGLE_ID: &str = "tenshu.settings.alerts.toggle";
pub const ALERTS_OFF_CONFIRM_ID: &str = "tenshu.settings.alerts.off";
pub const ALERTS_OFF_CANCEL_ID: &str = "tenshu.settings.alerts.off-cancel";
pub const ESCROW_PIN_ID: &str = "tenshu.settings.escrow.pin";
pub const ESCROW_CONFIRM_ID: &str = "tenshu.settings.escrow.confirm";
pub const ESCROW_CANCEL_ID: &str = "tenshu.settings.escrow.cancel";

/// The "What keyholders can recover" escrow selector — Estate-gated (locked
/// affordance when the account lacks the `inheritanceEscrow` entitlement). The
/// tier is server-derived; picking Vault only / Full Cube auto-enables alerts.
fn escrow_selector<'a>(
    ra: &'a RecoveryAlerts,
    alerts_on: bool,
    busy: bool,
    spark: bool,
) -> Element<'a, Message> {
    // Locked affordance when the account can't escrow a recovery kit.
    if !ra.escrow_entitled {
        return page::note(
            Tone::Muted,
            Column::new()
                .spacing(2)
                .push(
                    text(localized_copy(
                        "An encrypted recovery kit for your keyholders is part of the Estate plan.",
                    ))
                    .size(13),
                )
                .push(page::muted(localized_copy(
                    "Recovery alerts above still work on your current plan.",
                ))),
            Some(page::button(
                ESCROW_VIEW_PLANS_ID,
                localized_copy("View plans"),
                Some(Message::OpenPlanBillingFrom(ESCROW_VIEW_PLANS_ID)),
            )),
        );
    }

    // `None` = escrow is on but this device can't tell which tier (older API
    // that doesn't report `escrowedArtifacts`). While collecting the PIN for a
    // Full-Cube enrolment, highlight Full Cube to match the copy below.
    let escrow_tier = ra.escrow_tier();
    let display_tier = if ra.awaiting_pin {
        Some(EscrowTier::FullCube)
    } else {
        escrow_tier
    };

    // Every tier stays pressable unless a change is in flight; re-selecting
    // the known current tier is a no-op in `SelectEscrow`.
    let selector = segmented(
        [
            segment(
                "tenshu.settings.escrow.off",
                EscrowTier::Off,
                localized_copy("Nothing"),
            ),
            segment(
                "tenshu.settings.escrow.vault",
                EscrowTier::VaultOnly,
                localized_copy("Vault only"),
            ),
            segment(
                "tenshu.settings.escrow.full",
                EscrowTier::FullCube,
                localized_copy("Full Cube"),
            ),
        ],
        display_tier,
        Message::FocusAction,
    )
    .size(SegmentSize::Compact)
    .plain()
    .on_select_maybe((!busy).then_some(|tier| {
        SettingsMessage::RecoveryAlerts(RecoveryAlertsMessage::SelectEscrow(tier)).into()
    }));

    let mut col = Column::new()
        .spacing(6)
        .push(Space::new().height(Length::Fixed(4.0)))
        .push(selector)
        .push(Container::new(text(escrow_copy(display_tier, spark)).size(13)).max_width(560));

    // Auto-enable disclosure: picking a kit while alerts are off turns them on.
    if !alerts_on {
        col = col.push(page::muted(localized_copy(
            "Choosing Vault only or Full Cube also turns Recovery alerts on.",
        )));
    }

    // Full-Cube re-confirms the PIN before exporting the seed into escrow.
    if ra.awaiting_pin {
        col = col
            .push(Space::new().height(Length::Fixed(8.0)))
            .push(
                text(localized_copy(
                    "Enter your PIN to include this Cube's seed in the encrypted recovery kit.",
                ))
                .size(13)
                .font(coincube_ui::font::SEMIBOLD),
            )
            .push(
                coincube_ui::component::tenshu::text_field(
                    ESCROW_PIN_ID,
                    "PIN",
                    ra.pin.as_str(),
                    Message::FocusAction,
                )
                .hide_label()
                .placeholder("PIN")
                .secure(true)
                .size(coincube_ui::component::tenshu::FieldSize::Compact)
                .width(Length::Fixed(200.0))
                .on_input(|s| {
                    SettingsMessage::RecoveryAlerts(RecoveryAlertsMessage::EscrowPinChanged(
                        s.into(),
                    ))
                    .into()
                })
                .on_submit(
                    SettingsMessage::RecoveryAlerts(RecoveryAlertsMessage::ConfirmFullCube).into(),
                ),
            )
            .push(
                Row::new()
                    .spacing(8)
                    .push(page::primary(
                        ESCROW_CONFIRM_ID,
                        localized_copy("Confirm"),
                        (!busy).then_some(
                            SettingsMessage::RecoveryAlerts(RecoveryAlertsMessage::ConfirmFullCube)
                                .into(),
                        ),
                    ))
                    .push(page::button(
                        ESCROW_CANCEL_ID,
                        localized_copy("Cancel"),
                        Some(
                            SettingsMessage::RecoveryAlerts(RecoveryAlertsMessage::CancelFullCube)
                                .into(),
                        ),
                    )),
            );
    }

    col.into()
}

/// Plain-language trade-off for each escrow tier (no euphemisms — the
/// self-custody trust model demands it; see the ECIES decision record).
/// Everything is encrypted to the keyholders' own keys: COINCUBE can read
/// neither the descriptor nor the seed. `spark`: whether this Cube's network
/// has Spark, so Full Cube names only the wallets a restore can bring back.
fn escrow_copy(tier: Option<EscrowTier>, spark: bool) -> &'static str {
    match tier {
        // Escrow is on, but an older API doesn't report which tier — state that
        // honestly rather than assert (and possibly mis-state) descriptor-only
        // vs seed escrow.
        None => localized_copy(
            "A recovery kit is stored, but this device can't tell which kind (this server doesn't \
             report it). Reselect Vault only or Full Cube to confirm or change it, or Nothing to \
             remove it.",
        ),
        Some(EscrowTier::Off) => localized_copy(
            "Nothing — alerts only: your keyholders are told when the recovery window opens, but \
             no encrypted recovery kit is stored, so they can't recover this Vault themselves.",
        ),
        Some(EscrowTier::VaultOnly) => localized_copy(
            "Vault only: an encrypted copy of this Vault's descriptor is sealed to each \
             keyholder's own key — only they can open it, never COINCUBE. When the recovery \
             window opens, a keyholder recovers the watch-only Vault and sweeps the funds. The \
             seed is never escrowed.",
        ),
        Some(EscrowTier::FullCube) if spark => localized_copy(
            "Full Cube: seals this Cube's seed AND descriptor to each keyholder's own key. A \
             keyholder can restore the entire Cube — Spark and Vault. COINCUBE can never \
             read either; only the keyholder's key can. You'll re-confirm your PIN to include the \
             seed.",
        ),
        Some(EscrowTier::FullCube) => localized_copy(
            "Full Cube: seals this Cube's seed AND descriptor to each keyholder's own key. A \
             keyholder can restore the entire Cube, including its Vault. COINCUBE can never \
             read either; only the keyholder's key can. You'll re-confirm your PIN to include the \
             seed.",
        ),
    }
}

/// Which halves a single backup method holds. A method is "enabled" (in use)
/// when it holds at least one half; whether it's *complete* depends on the
/// Cube's shape (see [`method_complete`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MethodBackup {
    pub(crate) seed: bool,
    pub(crate) descriptor: bool,
}

/// One source of truth for "what's backed up, by which method" — the master
/// §definitions distilled into a per-method view the card (and, later, the
/// duress-vault-gate `CubeBackupCompleteness`) both read. `password` /
/// `keychain` are `Some` iff that method holds any restorable material (a
/// recipient without envelopes never counts — master F1). `last_updated` is
/// the later of the password kit's and the phone envelope's timestamps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BackupOverview {
    /// Password-encrypted kit. `Some` ⇔ any password blob present.
    pub(crate) password: Option<MethodBackup>,
    /// Owner-keychain ("phone") envelopes. `Some` ⇔ any envelope kind present
    /// (`has_envelope`; never `has_recipient` alone — master F1).
    pub(crate) keychain: Option<MethodBackup>,
    /// `max(kit.updated_at, owner_self.updated_at)`, compared by parsed instant.
    pub(crate) last_updated: Option<String>,
}

impl BackupOverview {
    /// At least one method holds restorable material.
    pub(crate) fn any_enabled(&self) -> bool {
        self.password.is_some() || self.keychain.is_some()
    }
}

/// Distill a `RecoveryKitStatus` into the per-method [`BackupOverview`]. Absent
/// status (not loaded yet) → every method `None`. Keychain presence keys off
/// `has_envelope()` (kinds non-empty), never a bare recipient (master F1).
pub(crate) fn backup_overview(status: Option<&RecoveryKitStatus>) -> BackupOverview {
    let Some(s) = status else {
        return BackupOverview {
            password: None,
            keychain: None,
            last_updated: None,
        };
    };
    let password =
        (s.has_encrypted_seed || s.has_encrypted_wallet_descriptor).then_some(MethodBackup {
            seed: s.has_encrypted_seed,
            descriptor: s.has_encrypted_wallet_descriptor,
        });
    let keychain = s
        .owner_self
        .as_ref()
        .filter(|o| o.has_envelope())
        .map(|o| MethodBackup {
            seed: o.envelope_kinds.iter().any(|k| k == "seed"),
            descriptor: o.envelope_kinds.iter().any(|k| k == "descriptor"),
        });
    let owner_updated = s.owner_self.as_ref().and_then(|o| o.updated_at.as_deref());
    let last_updated = later_timestamp(s.updated_at.as_deref(), owner_updated);
    BackupOverview {
        password,
        keychain,
        last_updated,
    }
}

/// User-facing names of the halves a method holds, in a fixed order (seed
/// first). Empty for a method holding nothing, which `backup_overview` never
/// produces — a method with no halves is `None`, not `Some(empty)`.
///
/// The wording matches the rest of this card ("Add Master Seed Phrase", "Your
/// password backup is missing the Wallet Descriptor") rather than the
/// Cube/Vault shorthand used on the Home list, so a user reading the card top
/// to bottom sees one name per artifact.
pub(crate) fn half_labels(m: &MethodBackup) -> Vec<&'static str> {
    let mut out = Vec::with_capacity(2);
    if m.seed {
        out.push("Master Seed Phrase");
    }
    if m.descriptor {
        out.push("Wallet Descriptor");
    }
    out
}

/// What the card should say is *inside* the backup, as opposed to which method
/// protects it. Method pills alone left the most important question unanswered
/// — a kit reading "Password" tells you nothing about whether your Master Seed
/// Phrase, your Wallet Descriptor, or both are actually recoverable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BackupContents {
    /// Nothing backed up by any method — the card is in its Create state and
    /// shows no pills at all.
    Nothing,
    /// Every enabled method holds the same halves, so one row says it for all
    /// of them. The common case, and always true once the card reads "backed
    /// up" on a Cube whose methods are complete for the same shape.
    Shared(Vec<&'static str>),
    /// The enabled methods hold *different* halves — e.g. a password kit with
    /// both halves alongside a phone envelope carrying only the descriptor.
    /// Listed per method, because a shared row would either overstate the
    /// weaker method or understate the stronger one.
    PerMethod(Vec<(&'static str, Vec<&'static str>)>),
}

/// Distill the per-method [`BackupOverview`] into what the card renders under
/// its method pills. Method order is fixed (password first) so the rows don't
/// reshuffle between renders.
pub(crate) fn backup_contents(overview: &BackupOverview) -> BackupContents {
    let mut methods: Vec<(&'static str, &MethodBackup)> = Vec::with_capacity(2);
    if let Some(m) = overview.password.as_ref() {
        methods.push(("Password", m));
    }
    if let Some(m) = overview.keychain.as_ref() {
        methods.push(("Keychain", m));
    }

    match methods.as_slice() {
        [] => BackupContents::Nothing,
        [(_, only)] => BackupContents::Shared(half_labels(only)),
        [(_, a), (_, b)] if a == b => BackupContents::Shared(half_labels(a)),
        rest => BackupContents::PerMethod(
            rest.iter()
                .map(|&(label, m)| (label, half_labels(m)))
                .collect(),
        ),
    }
}

/// Whether a method independently holds everything the Cube's shape requires
/// (master §definitions: completeness is per-method):
///   * with a Vault — seed **and** descriptor;
///   * without one — seed alone.
///
/// There is no passkey arm any more. It used to read "descriptor only, the seed
/// is unextractable on-device", and since **I11** that premise is false: a
/// passkey Cube's Kit carries the encrypted master seed, re-derived from a
/// WebAuthn assertion at Kit time. Keeping a passkey arm here would put this
/// card and `cube_backup_completeness` — which this function is supposed to
/// mirror — back into disagreement, with the card claiming "backed up" for a
/// Cube the duress gate would block.
pub(crate) fn method_complete(m: &MethodBackup, has_vault: bool) -> bool {
    if has_vault {
        m.seed && m.descriptor
    } else {
        m.seed
    }
}

/// The later of two RFC 3339 timestamps, compared by parsed instant so
/// mixed-precision strings sort correctly. Falls back to whichever side parses
/// when only one does, and to the first present value when neither parses.
fn later_timestamp(a: Option<&str>, b: Option<&str>) -> Option<String> {
    match (a, b) {
        (Some(a), Some(b)) => {
            let pa = chrono::DateTime::parse_from_rfc3339(a).ok();
            let pb = chrono::DateTime::parse_from_rfc3339(b).ok();
            match (pa, pb) {
                (Some(pa), Some(pb)) => Some(if pa >= pb { a } else { b }.to_string()),
                (Some(_), None) => Some(a.to_string()),
                (None, Some(_)) => Some(b.to_string()),
                // Neither parses — deterministic fall back to the first (password).
                (None, None) => Some(a.to_string()),
            }
        }
        (Some(a), None) => Some(a.to_string()),
        (None, Some(b)) => Some(b.to_string()),
        (None, None) => None,
    }
}

/// Which Recovery-Kit backup methods are actually in place, from the status.
/// Returns `(password_present, keychain_present)` — presence, not completeness,
/// so a partial method still shows its pill. Thin wrapper over
/// [`backup_overview`]; shared by the Settings card and the protection-choice
/// wizard screen.
pub(crate) fn backup_methods_present(status: Option<&RecoveryKitStatus>) -> (bool, bool) {
    let o = backup_overview(status);
    (o.password.is_some(), o.keychain.is_some())
}

/// Format an RFC 3339 timestamp as the API returns it (e.g.
/// `2026-05-08T14:36:50Z`) into a friendlier `8 May 2026, 14:36 UTC`. Falls
/// back to the raw string if it can't be parsed.
fn format_backup_time(raw: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(raw)
        .map(|dt| {
            let utc = dt.with_timezone(&chrono::Utc);
            utc.format(&coincube_ui::locale::date_pattern(
                "%-d %b %Y, %H:%M UTC",
                &utc,
            ))
            .to_string()
        })
        .unwrap_or_else(|_| raw.to_string())
}

/// The computed textual state of the Recovery-Kit card — title, subtitle, and
/// primary CTA — factored out of [`recovery_kit_card`] so the per-method state
/// matrix (plan §PR1) is unit-testable without standing up iced Elements.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CardState {
    title: &'static str,
    subtitle: String,
    primary_label: &'static str,
    primary_mode: RecoveryKitMode,
}

/// The "Create" (nothing backed up yet) card state.
///
/// One state for both shapes. The passkey variant used to say the seed "is
/// protected by your passkey and isn't included in the Recovery Kit"; that
/// claim is gone with **I11**, and its removal matters more than the wording
/// change suggests — a passkey user who read it and skipped the Kit was left
/// with recovery that depended entirely on iCloud Keychain being on, which we
/// can neither require nor detect.
fn create_state() -> CardState {
    CardState {
        title: "Back up your Cube Recovery Kit",
        subtitle: "Back up your Master Seed Phrase and Wallet Descriptor to your Connect \
                   account so you can restore your Cube if you lose this device."
            .to_string(),
        primary_label: "Create Recovery Kit",
        primary_mode: RecoveryKitMode::Create,
    }
}

/// A sentence naming the missing half of an incomplete enabled method (`label`
/// is "password" / "phone"), or `None` when the method is complete for the
/// shape. An enabled method holds at least one half, so for the vault shape the
/// gap is whichever half is absent; for the seed-only (vaultless) shape it
/// names that required half.
fn method_gap_sentence(m: &MethodBackup, has_vault: bool, label: &str) -> Option<String> {
    if method_complete(m, has_vault) {
        return None;
    }
    let missing_half = if m.seed {
        // A vault method that already holds the seed is missing the descriptor.
        "Wallet Descriptor"
    } else {
        // No seed present — the seed is what's missing, in either shape.
        "Master Seed Phrase"
    };
    Some(localized_format!(
        "Your {} backup is missing the {}.",
        "A tu copia con {} le falta: {}.",
        localized_copy(label),
        localized_copy(missing_half)
    ))
}

/// The CTA that closes the *password* method's gap, when the password method is
/// enabled and incomplete. `None` when password is absent or already complete —
/// the gap is then on the keychain method, closed by re-sealing via `Rotate`.
fn password_gap_cta(
    password: &Option<MethodBackup>,
    has_vault: bool,
) -> Option<(&'static str, RecoveryKitMode)> {
    let pw = password.as_ref()?;
    if method_complete(pw, has_vault) {
        return None;
    }
    if pw.seed && !pw.descriptor {
        // Seed backed up, descriptor missing → the existing AddDescriptor branch.
        Some(("Add Wallet Descriptor", RecoveryKitMode::AddDescriptor))
    } else {
        // Descriptor present but seed missing (or vaultless seed-shape with no
        // seed) → the existing AddSeed branch.
        Some(("Add Master Seed Phrase", RecoveryKitMode::AddSeed))
    }
}

/// Derive the card's copy + primary CTA from the per-method [`BackupOverview`]
/// and the Cube's shape. The Cube reads "backed up" iff ≥1 method is enabled
/// and every enabled method is complete for the shape (master F6); any
/// enabled-but-incomplete method drops to the "Finish backing up" state,
/// naming each gap (password first) with the CTA pointed at the first gap.
/// `status_present` distinguishes "loaded, no methods" (Create) from "not
/// loaded yet" (pending or unavailable service status).
fn recovery_kit_card_state(
    overview: &BackupOverview,
    has_vault: bool,
    loading: bool,
    status_present: bool,
) -> CardState {
    // The outer view reaches this card only with an authenticated session.
    // Missing status is pending or failed, not proof that no kit exists.
    if !status_present {
        return CardState {
            title: "Cube Recovery Kit",
            subtitle: if loading {
                "Checking your Connect account…".to_string()
            } else {
                "Recovery Kit status is unavailable. Refresh before making changes.".to_string()
            },
            primary_label: "Create Recovery Kit",
            primary_mode: RecoveryKitMode::Create,
        };
    }

    // No method enabled → Create.
    if !overview.any_enabled() {
        return create_state();
    }

    // Per-method completeness. A disabled method vacuously satisfies the
    // conjunction, so `unwrap_or(true)`.
    let pw_complete = overview
        .password
        .as_ref()
        .map(|m| method_complete(m, has_vault))
        .unwrap_or(true);
    let kc_complete = overview
        .keychain
        .as_ref()
        .map(|m| method_complete(m, has_vault))
        .unwrap_or(true);

    if pw_complete && kc_complete {
        // Every enabled method complete → backed up.
        let title = "Recovery Kit backed up";
        let subtitle = localized_format!(
            "Last updated {}.",
            "Última actualización de {}.",
            overview
                .last_updated
                .as_deref()
                .map(format_backup_time)
                .unwrap_or_else(|| "—".to_string())
        );
        return CardState {
            title,
            subtitle,
            primary_label: "Update",
            primary_mode: RecoveryKitMode::Rotate,
        };
    }

    // At least one enabled method is incomplete → "Finish backing up", listing
    // each gap in deterministic order (password first).
    let mut gaps: Vec<String> = Vec::new();
    if let Some(pw) = &overview.password {
        if let Some(s) = method_gap_sentence(pw, has_vault, "password") {
            gaps.push(s);
        }
    }
    if let Some(kc) = &overview.keychain {
        if let Some(s) = method_gap_sentence(kc, has_vault, "phone") {
            gaps.push(s);
        }
    }

    // The CTA closes the first gap (password first); a keychain-only gap
    // re-seals via Rotate (the protection-choice → phone re-seal fills whatever
    // the tier requires in one pass).
    let (primary_label, primary_mode) = password_gap_cta(&overview.password, has_vault)
        .unwrap_or(("Finish backing up", RecoveryKitMode::Rotate));

    CardState {
        title: "Finish backing up your Recovery Kit",
        subtitle: gaps.join(" "),
        primary_label,
        primary_mode,
    }
}

/// Cube Recovery Kit card — rendered below the local paper-phrase
/// backup card. Shows copy + a primary action that drives the
/// `RecoveryKitMessage` flow. States mirror the plan §6.3 matrix.
///
/// - `has_vault`: gates the "complete" copy — a seed-only kit on a
///   vaultless cube is already "complete" from the user's perspective, so
///   the CTA becomes "Update" rather than "Add Wallet Descriptor".
///
/// The Cube's passkey shape is no longer an input. It used to select a reduced
/// descriptor-only variant, and to **suppress the card entirely** on a passkey
/// Cube with no Vault ("nothing to back up — create a Vault to enable
/// Recovery-Kit backup"). That suppression was the worst of it: the Cube most
/// in need of a Kit was the one shown no way to make one. Since **I11** a
/// passkey Cube backs up its master seed like any other, Vault or no Vault.
fn recovery_kit_card<'a>(
    has_vault: bool,
    status: Option<&RecoveryKitStatus>,
    loading: bool,
    stale: bool,
    drift: DescriptorDrift,
    imported_state_unknown: bool,
    row: page::PageRow<'a, Message>,
) -> Element<'a, Message> {
    // One source of truth for what's backed up, by which method (master
    // §definitions). Drives the row state, the method lines and Remove.
    let overview = backup_overview(status);
    let CardState {
        title,
        subtitle,
        primary_label,
        primary_mode,
    } = recovery_kit_card_state(&overview, has_vault, loading, status.is_some());
    let has_kit = overview.any_enabled();

    // The row's status word, from the same state the card copy used.
    let (kind, word) = if loading {
        (page::Status::Busy, "Checking…")
    } else if status.is_none() {
        (page::Status::Unknown, "Status unavailable")
    } else if stale {
        (page::Status::Unknown, "Last known state")
    } else if drift.any() {
        (page::Status::Warn, "Out of date")
    } else if !has_kit {
        (page::Status::Off, "Not set up")
    } else if title == "Recovery Kit backed up" {
        (page::Status::Ok, "Backed up")
    } else {
        (page::Status::Warn, "Incomplete")
    };

    // Drift overrides the "complete" state: the action becomes "Update now"
    // and the detail names the stale method(s) (per-method drift, PR 3).
    let (detail, primary_label, primary_mode) = if loading {
        (None, primary_label, primary_mode)
    } else if stale && status.is_some() {
        (
            Some(localized_copy("Showing the last known Recovery Kit state. Reconnect and refresh before changing it.").to_string()),
            primary_label,
            primary_mode,
        )
    } else if imported_state_unknown {
        (
            Some(localized_copy("Connect and refresh this Bitcoin wallet before checking or updating its Recovery Kit.").to_string()),
            primary_label,
            primary_mode,
        )
    } else if drift.any() {
        (
            Some(drift_subtitle(&drift).to_string()),
            "Update now",
            RecoveryKitMode::Rotate,
        )
    } else if status.is_none() || !has_kit {
        (None, primary_label, primary_mode)
    } else {
        (Some(subtitle), primary_label, primary_mode)
    };

    let mut row = row.accessory(page::status(kind, localized_copy(word)));
    if let Some(detail) = detail {
        row = row.line(page::muted(detail));
    }
    let (password_present, keychain_present) =
        (overview.password.is_some(), overview.keychain.is_some());
    if password_present || keychain_present {
        let mut methods: Vec<&str> = Vec::new();
        if password_present {
            methods.push("Password");
        }
        if keychain_present {
            methods.push("Keychain");
        }
        row = row.line(page::muted(localized_format!(
            "Protected by: {}",
            "Protegido por: {}",
            methods
                .iter()
                .map(|method| localized_copy(method))
                .collect::<Vec<_>>()
                .join(" · ")
        )));
    }
    // What's actually *in* the backup, not only how it is protected.
    match backup_contents(&overview) {
        BackupContents::Nothing => {}
        BackupContents::Shared(halves) => {
            row = row.line(page::muted(localized_format!(
                "Includes: {}",
                "Incluye: {}",
                halves
                    .iter()
                    .map(|half| localized_copy(half))
                    .collect::<Vec<_>>()
                    .join(" · ")
            )));
        }
        BackupContents::PerMethod(per_method) => {
            for (method, halves) in per_method {
                row = row.line(page::muted(localized_format!(
                    "{} holds: {}",
                    "{} contiene: {}",
                    localized_copy(method),
                    halves
                        .iter()
                        .map(|half| localized_copy(half))
                        .collect::<Vec<_>>()
                        .join(" · ")
                )));
            }
        }
    }
    if drift.any() {
        row = row.line(
            text(localized_copy(drift_warning_line(&drift)))
                .size(12)
                .style(coincube_ui::theme::text::warning),
        );
    }

    if status.is_none() || stale {
        row = row.control(page::button(
            KIT_PRIMARY_ID,
            localized_copy("Refresh status"),
            (!loading)
                .then_some(SettingsMessage::RecoveryKit(RecoveryKitMessage::LoadStatus).into()),
        ));
    } else {
        let label = match primary_mode {
            RecoveryKitMode::Create => "Set up Recovery Kit…",
            _ => primary_label,
        };
        let press = (!imported_state_unknown && !loading).then_some(
            SettingsMessage::RecoveryKit(RecoveryKitMessage::Start(primary_mode)).into(),
        );
        row = row.control(if drift.any() || word == "Incomplete" {
            page::primary(KIT_PRIMARY_ID, localized_copy(label), press)
        } else {
            page::button(KIT_PRIMARY_ID, localized_copy(label), press)
        });
    }
    if has_kit && !stale && !loading {
        row = row.control(page::button(
            KIT_REMOVE_ID,
            localized_copy("Remove…"),
            Some(SettingsMessage::RecoveryKit(RecoveryKitMessage::Remove).into()),
        ));
    }
    row.top().into()
}

fn amounts_and_currency<'a>(
    new_unit_setting: &'a UnitSetting,
    display_mode: DisplayMode,
    display_mode_allowed: bool,
    new_price_setting: &'a PriceSetting,
    currencies_list: &'a [Currency],
    btcb2: bool,
    show_direction_badges: bool,
) -> Element<'a, Message> {
    Column::new()
        .spacing(0)
        .width(Length::Fill)
        .push(bitcoin_display_unit(new_unit_setting))
        .push(display_mode_choice(display_mode, display_mode_allowed))
        .push(fiat_price(new_price_setting, currencies_list, btcb2))
        .push(direction_badges_toggle(show_direction_badges))
        .into()
}

fn direction_badges_toggle<'a>(show: bool) -> Element<'a, Message> {
    setting_row(
        localized_copy("Direction badges"),
        toggle("settings-direction-badges", show, Message::FocusAction)
            .on_toggle(|value| SettingsMessage::ToggleDirectionBadges(value).into()),
    )
    .tag(localized_copy("This device"))
    .description(localized_copy("In and Out badges in Activity."))
    .into()
}

/// The Settings artboard places a select's label in the left column. The
/// reusable select draws its own clickable label above the field, so keep the
/// same focus behavior here while retaining the two-column setting row.
fn labeled_select_row<'a>(
    label: &'static str,
    description: Option<&'static str>,
    select_id: &'static str,
    enabled: bool,
    control: Element<'a, Message>,
) -> Element<'a, Message> {
    let label_text = coincube_ui::component::tenshu::label(label);
    let label: Element<'a, Message> = if enabled {
        mouse_area(label_text)
            .on_press(Message::FocusAction(iced::advanced::widget::Id::new(
                select_id,
            )))
            .into()
    } else {
        label_text.into()
    };
    let mut left = Column::new().spacing(2).width(Length::Fill).push(label);
    if let Some(description) = description {
        left = left.push(text(description).size(13).style(theme::text::secondary));
    }
    let body = Row::new()
        .spacing(24)
        .align_y(Alignment::Center)
        .push(left)
        .push(Container::new(control).align_x(iced::alignment::Horizontal::Right));
    Column::new()
        .width(Length::Fill)
        .push(Container::new(body).padding([12, 0]))
        .push(
            Container::new(Space::new())
                .width(Length::Fill)
                .height(Length::Fixed(1.0))
                .style(theme::tenshu::control::divider),
        )
        .into()
}

fn display_mode_choice<'a>(current: DisplayMode, allowed: bool) -> Element<'a, Message> {
    setting_row(
        localized_copy("Primary amount"),
        segmented(
            [
                segment(
                    "settings-primary-bitcoin",
                    DisplayMode::BitcoinNative,
                    "Bitcoin",
                ),
                segment("settings-primary-fiat", DisplayMode::FiatNative, "Fiat"),
            ],
            Some(current),
            Message::FocusAction,
        )
        .size(SegmentSize::Compact)
        .plain()
        .on_select_maybe(allowed.then_some({
            move |selected| {
                if selected == current {
                    Message::FocusAction(iced::advanced::widget::Id::new(match selected {
                        DisplayMode::BitcoinNative => "settings-primary-bitcoin",
                        DisplayMode::FiatNative => "settings-primary-fiat",
                    }))
                } else {
                    Message::FlipDisplayMode
                }
            }
        })),
    )
    .description(localized_copy("Shown large."))
    .into()
}

pub fn bitcoin_display_unit<'a>(new_unit_setting: &'a UnitSetting) -> Element<'a, Message> {
    let current = new_unit_setting.display_unit;
    setting_row(
        localized_copy("Bitcoin unit"),
        segmented(
            [
                segment("settings-unit-btc", BitcoinDisplayUnit::BTC, "BTC"),
                segment("settings-unit-sats", BitcoinDisplayUnit::Sats, "Sats"),
            ],
            Some(current),
            Message::FocusAction,
        )
        .size(SegmentSize::Compact)
        .plain()
        .on_select(move |selected| {
            if selected == current {
                Message::FocusAction(iced::advanced::widget::Id::new(match selected {
                    BitcoinDisplayUnit::BTC => "settings-unit-btc",
                    BitcoinDisplayUnit::Sats => "settings-unit-sats",
                }))
            } else {
                SettingsMessage::DisplayUnitChanged(selected).into()
            }
        }),
    )
    .into()
}

pub fn fiat_price<'a>(
    new_price_setting: &'a PriceSetting,
    currencies_list: &'a [Currency],
    btcb2: bool,
) -> Element<'a, Message> {
    let enabled = new_price_setting.is_enabled;
    let source = if btcb2 {
        crate::services::fiat::PriceSource::Coincube
    } else {
        new_price_setting.source
    };
    let sources = if btcb2 {
        &ALL_PRICE_SOURCES[..1]
    } else {
        &ALL_PRICE_SOURCES[..]
    };
    let mut rows = Column::new()
        .spacing(0)
        .width(Length::Fill)
        .push(setting_row(
            localized_copy("Fiat estimates"),
            toggle("settings-fiat-estimates", enabled, Message::FocusAction)
                .on_toggle(|value| FiatMessage::Enable(value).into()),
        )
        .description(localized_copy("Approximate values beside Bitcoin amounts."))
        .note(
            "Fiat price data is provided by third-party services. Availability and accuracy are not guaranteed.",
            Tone::Muted,
        ))
        .push(labeled_select_row(
            "Currency",
            None,
            "settings-fiat-currency",
            enabled,
            select(
                "settings-fiat-currency",
                "Currency",
                currencies_list.iter().copied().map(|currency| (currency, currency.to_string())),
                Some(new_price_setting.currency),
                Message::FocusAction,
            )
            .hide_label()
            .width(Length::Fixed(232.0))
            .on_select_maybe(enabled.then_some(|currency| FiatMessage::CurrencyEdited(currency).into()))
            .into(),
        ))
        .push(labeled_select_row(
            "Exchange-rate source",
            None,
            "settings-fiat-source",
            enabled,
            select(
                "settings-fiat-source",
                "Exchange-rate source",
                sources.iter().copied().map(|price_source| (price_source, price_source.to_string())),
                Some(source),
                Message::FocusAction,
            )
            .hide_label()
            .width(Length::Fixed(232.0))
            .on_select_maybe(enabled.then_some(|price_source| FiatMessage::SourceEdited(price_source).into()))
            .into(),
        ));
    if let Some(attribution) = source.attribution().filter(|_| enabled) {
        rows = rows.push(
            Row::new()
                .push(Space::new().width(Length::Fill))
                .push(text(attribution).size(12).style(theme::text::secondary)),
        );
    }
    rows.into()
}

/// Per-method descriptor drift verdict (per-method drift, PR 3). Each enabled
/// method's cached descriptor fingerprint is compared to the live descriptor
/// independently, so re-sealing one method clears only that method's warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct DescriptorDrift {
    pub(crate) password: bool,
    pub(crate) keychain: bool,
}

impl DescriptorDrift {
    /// Any enabled method's descriptor is out of sync with the live Vault.
    pub(crate) fn any(&self) -> bool {
        self.password || self.keychain
    }
}

/// Per-method drift verdicts for the Recovery-Kit card. Each method is judged by
/// the same positive-evidence-only rule as [`method_descriptor_drift`]:
/// `{password,keychain}_present` is whether that method has a descriptor on the
/// server; `*_slot` is that method's last locally-cached upload fingerprint;
/// `live` is what the wallet would currently upload.
fn descriptor_drift(
    live: Option<&str>,
    password_present: bool,
    password_slot: Option<&str>,
    keychain_present: bool,
    keychain_slot: Option<&str>,
) -> DescriptorDrift {
    DescriptorDrift {
        password: method_descriptor_drift(password_present, live, password_slot),
        keychain: method_descriptor_drift(keychain_present, live, keychain_slot),
    }
}

/// Whether a single backup method's descriptor is out of sync with the live
/// Vault. Split out of `general_section` so the branch table is testable without
/// standing up a full `Cache`, and shared by both methods (PR 3).
///
/// - `present`: this method reports a descriptor backed up on Connect
///   (password: `has_encrypted_wallet_descriptor`; keychain:
///   `envelope_kinds ∋ "descriptor"`).
/// - `live`: SHA-256 of what the live wallet would currently upload
///   (`None` when no Vault is loaded).
/// - `cached`: SHA-256 of what this device last backed up for this method
///   (`None` when the local slot was cleared, the kit was made from a different
///   install, or the backup predates the slot field).
fn method_descriptor_drift(present: bool, live: Option<&str>, cached: Option<&str>) -> bool {
    if !present {
        return false;
    }
    match (live, cached) {
        // Both present — direct comparison. This is the only branch
        // with positive evidence of drift; everything else is
        // absence-of-evidence and must not fire the banner.
        (Some(live), Some(cached)) => live != cached,
        // Live descriptor known, no cached hash. This happens for:
        // kit restored onto this device (we never re-uploaded so
        // never populated the cache), kit uploaded from a different
        // device, and installs that pre-date the cache field. In
        // all three, the server's descriptor-present flag already
        // tells us a backup exists — firing the banner
        // here produces a *permanent* false positive that stays up
        // until the user manually re-uploads, training them to
        // ignore the signal entirely (banner blindness). The
        // always-visible "Update" CTA on the card is the right
        // affordance for users who want to force a refresh; the
        // banner is reserved for confirmed drift.
        (Some(_), None) => false,
        // No live fingerprint (no Vault loaded yet) → can't compare.
        _ => false,
    }
}

/// The card subtitle when a descriptor has drifted — names the stale method(s)
/// (per-method drift, PR 3). Empty string when nothing drifted (callers guard
/// with [`DescriptorDrift::any`]).
fn drift_subtitle(d: &DescriptorDrift) -> &'static str {
    match (d.password, d.keychain) {
        (true, true) => localized_copy(
            "Your Wallet Descriptor changed since your last backup — update both copies now.",
        ),
        (true, false) => {
            localized_copy("Your password backup's Wallet Descriptor is out of date — update now.")
        }
        (false, true) => {
            localized_copy("Your phone backup's Wallet Descriptor is out of date — update now.")
        }
        (false, false) => "",
    }
}

/// The inline ⚠ warning line under the pills when a descriptor has drifted —
/// names the stale method(s) (per-method drift, PR 3).
fn drift_warning_line(d: &DescriptorDrift) -> &'static str {
    match (d.password, d.keychain) {
        (true, true) => "⚠ Your password and phone descriptors are out of sync with your Vault.",
        (true, false) => "⚠ Your password descriptor is out of sync with your Vault.",
        (false, true) => "⚠ Your phone descriptor is out of sync with your Vault.",
        (false, false) => "",
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn backup_page_is_spanish() {
        use crate::app::state::preferences::BalancePrivacy;
        use crate::app::view::{nav::shell::capture_tests, shared::landing::capture};
        use coincube_ui::locale::{render_in, Language};
        for confirmed in [false, true] {
            let mut cache = capture_tests::cache(false, BalancePrivacy::Shown);
            cache.current_cube_backed_up = confirmed;
            let view = render_in(Language::Spanish, || recovery_overview(&cache, None, None));
            capture::inspect(
                "backup-es",
                view,
                (1440, 900),
                "dark",
                coincube_ui::theme::Theme::dark(),
            )
            .assert_spanish("Cube › Settings › Backup & protection");
        }
    }

    #[test]
    fn backup_page_has_one_seed_action_in_each_confirmation_state() {
        use crate::app::state::preferences::BalancePrivacy;
        use crate::app::view::{nav::shell::capture_tests, shared::landing::capture};
        for (appearance, palette) in capture_tests::themes() {
            for confirmed in [false, true] {
                let mut cache = capture_tests::cache(appearance == "light", BalancePrivacy::Shown);
                cache.current_cube_backed_up = confirmed;
                let inspection = capture::inspect(
                    if confirmed {
                        "seed-overview-confirmed"
                    } else {
                        "seed-overview-unconfirmed"
                    },
                    recovery_overview(&cache, None, None),
                    (1024, 768),
                    appearance,
                    palette,
                );
                let action = if confirmed {
                    "Reveal seed phrase…"
                } else {
                    "Confirm seed phrase"
                };
                assert_eq!(
                    inspection
                        .text
                        .iter()
                        .filter(|(value, _)| value == action)
                        .count(),
                    1
                );
                assert!(inspection.contains(if confirmed {
                    "Confirmed"
                } else {
                    "Not confirmed"
                }));
                assert!(!inspection.contains("Your seed phrase is confirmed"));
            }
        }
    }

    use super::*;

    #[derive(Default)]
    struct LabelBounds(Vec<(String, iced::Rectangle)>);

    impl iced::advanced::widget::Operation for LabelBounds {
        fn traverse(
            &mut self,
            operate: &mut dyn FnMut(&mut dyn iced::advanced::widget::Operation),
        ) {
            operate(self);
        }

        fn text(
            &mut self,
            _: Option<&iced::advanced::widget::Id>,
            bounds: iced::Rectangle,
            value: &str,
        ) {
            self.0.push((value.to_owned(), bounds));
        }
    }

    #[test]
    fn currency_and_source_labels_focus_only_enabled_selects() {
        use iced::advanced::{layout, renderer::Headless, widget::Tree, Layout};
        use iced::{mouse, Size};

        for font in coincube_ui::font::load() {
            iced::advanced::graphics::text::font_system()
                .write()
                .unwrap()
                .load_font(font);
        }
        let currencies = [Currency::USD, Currency::EUR];
        for enabled in [true, false] {
            let price = PriceSetting {
                is_enabled: enabled,
                ..PriceSetting::default()
            };
            for (label, target) in [
                ("Currency", "settings-fiat-currency"),
                ("Exchange-rate source", "settings-fiat-source"),
            ] {
                let renderer =
                    iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
                        coincube_ui::font::REGULAR,
                        iced::Pixels(16.0),
                        Some("tiny-skia"),
                    ))
                    .unwrap();
                let mut element = fiat_price(&price, &currencies, false);
                let mut tree = Tree::new(element.as_widget());
                let size = Size::new(652.0, 400.0);
                let node = element.as_widget_mut().layout(
                    &mut tree,
                    &renderer,
                    &layout::Limits::new(Size::ZERO, size),
                );
                let mut labels = LabelBounds::default();
                element.as_widget_mut().operate(
                    &mut tree,
                    Layout::new(&node),
                    &renderer,
                    &mut labels,
                );
                let bounds = labels
                    .0
                    .iter()
                    .find_map(|(text, bounds)| (text == label).then_some(*bounds))
                    .expect("visible select label");
                let mut messages = Vec::new();
                for event in [
                    iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
                    iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
                ] {
                    element.as_widget_mut().update(
                        &mut tree,
                        &event,
                        Layout::new(&node),
                        mouse::Cursor::Available(bounds.center()),
                        &renderer,
                        &mut iced::advanced::clipboard::Null,
                        &mut iced::advanced::Shell::new(&mut messages),
                        &iced::Rectangle::with_size(size),
                    );
                }
                assert_eq!(
                    messages.len(),
                    usize::from(enabled),
                    "{} enabled={}",
                    label,
                    enabled,
                );
                if enabled {
                    assert!(matches!(
                        messages.as_slice(),
                        [Message::FocusAction(id)] if id == &iced::advanced::widget::Id::new(target)
                    ));
                }
            }
        }
    }

    /// Settings → General under the pinned head it takes from its route, at
    /// the top and scrolled (Tenshu v175 L3). Opt-in:
    /// `TOP_BAR_CAPTURE_DIR=<dir> cargo test -p coincube-gui --lib
    /// page_head_capture -- --ignored`.
    #[test]
    #[ignore = "writes PNG frames; needs a GPU adapter"]
    fn page_head_capture_settings() {
        use crate::app::state::preferences::BalancePrivacy;
        use crate::app::view::nav::shell::capture_tests;
        let Some(directory) = capture_tests::directory() else {
            return;
        };
        let mut renderer = capture_tests::renderer();
        let menu = Menu::Cube(CubeSubMenu::Settings(CubeSettingsOption::General));
        let price = PriceSetting::default();
        let unit = UnitSetting::default();
        let currencies = [Currency::USD, Currency::EUR];
        for (appearance, palette) in capture_tests::themes() {
            let cache = capture_tests::cache(appearance == "light", BalancePrivacy::Shown);
            capture_tests::save_page_head_frames(
                &mut renderer,
                &directory,
                "settings",
                appearance,
                &palette,
                || general_section(&menu, &cache, &price, &unit, &currencies, true, false, None),
            );
        }
    }

    #[test]
    fn amounts_and_currency_fits_both_themes_and_window_sizes() {
        use crate::app::view::shared::landing::capture;
        use iced::advanced::widget::Id;

        let unit = UnitSetting::default();
        let currencies = [Currency::USD, Currency::EUR];
        for (appearance, theme) in [
            ("dark", theme::Theme::dark()),
            ("light", theme::Theme::light()),
        ] {
            for outer in [(1024, 768), (1440, 900)] {
                for enabled in [true, false] {
                    let price = PriceSetting {
                        is_enabled: enabled,
                        ..PriceSetting::default()
                    };
                    let content: Element<'_, Message> = Column::new()
                        .spacing(20)
                        .push(coincube_ui::component::tenshu::section(
                            "Amounts & currency",
                        ))
                        .push(amounts_and_currency(
                            &unit,
                            DisplayMode::BitcoinNative,
                            true,
                            &price,
                            &currencies,
                            false,
                            true,
                        ))
                        .into();
                    let inspection = capture::inspect(
                        if enabled {
                            "r26-settings-amounts"
                        } else {
                            "r26-settings-amounts-fiat-off"
                        },
                        content,
                        outer,
                        appearance,
                        theme,
                    );
                    for label in [
                        "Bitcoin unit",
                        "Primary amount",
                        "Fiat estimates",
                        "Currency",
                        "Exchange-rate source",
                        "Direction badges",
                        "This device",
                    ] {
                        assert!(
                            inspection.contains(label),
                            "{} {:?}: {}",
                            appearance,
                            outer,
                            label,
                        );
                    }
                    for id in [
                        "settings-unit-btc",
                        "settings-primary-bitcoin",
                        "settings-fiat-estimates",
                        "settings-direction-badges",
                    ] {
                        assert!(
                            inspection.has_focus(Id::new(id)),
                            "{} {:?}: {}",
                            appearance,
                            outer,
                            id,
                        );
                    }
                    for id in ["settings-fiat-currency", "settings-fiat-source"] {
                        assert_eq!(
                            inspection.has_focus(Id::new(id)),
                            enabled,
                            "{} {:?}: {} enablement",
                            appearance,
                            outer,
                            id,
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn unregistered_cube_uses_session_restore_without_wrong_account_handoff() {
        let restored_session = cache::Cache {
            restorable_connect_session: true,
            ..cache::Cache::default()
        };
        let (label, route) = unregistered_cube_action(&restored_session).unwrap();
        assert_eq!(label, "Restore Connect session");
        assert!(matches!(
            route,
            Message::ConnectAccount(ConnectAccountMessage::Init)
        ));

        let authenticated = cache::Cache {
            connect_authenticated: true,
            restorable_connect_session: true,
            ..cache::Cache::default()
        };
        let (label, route) = unregistered_cube_action(&authenticated).unwrap();
        assert_eq!(label, "Retry Cube connection");
        assert!(matches!(
            route,
            Message::ConnectCube(ConnectCubeMessage::RetryRegistration)
        ));

        // A registered Cube has no registration CTA, and a missing session
        // uses the separate sign-in prompt above this card.
        let registered = cache::Cache {
            current_cube_server_id: Some(7),
            ..restored_session.clone()
        };
        assert!(unregistered_cube_action(&registered).is_none());
        assert!(unregistered_cube_action(&cache::Cache::default()).is_none());
    }

    #[test]
    fn cube_data_exports_reuses_only_available_destinations() {
        assert!(matches!(
            recovery_kit_route(),
            Menu::Cube(CubeSubMenu::Settings(CubeSettingsOption::Recovery))
        ));
        let regular = cache::Cache {
            has_vault: true,
            ..cache::Cache::default()
        };
        assert!(matches!(
            vault_data_route(&regular),
            Some(Menu::Vault(VaultSubMenu::Settings(Some(
                SettingsOption::ImportExport
            ))))
        ));
        let no_vault = cache::Cache::default();
        assert!(vault_data_route(&no_vault).is_none());
        let imported = cache::Cache {
            has_vault: false,
            has_imported_bitcoin: true,
            ..cache::Cache::default()
        };
        assert!(vault_data_route(&imported).is_none());
        // A stale cache must still fail closed instead of dispatching into an
        // export panel that imported Bitcoin never constructs.
        let inconsistent = cache::Cache {
            has_vault: true,
            has_imported_bitcoin: true,
            ..cache::Cache::default()
        };
        assert!(vault_data_route(&inconsistent).is_none());
    }

    #[test]
    fn no_drift_when_server_has_no_descriptor() {
        // Card's "incomplete" copy already covers this case; drift
        // must not double up the signal.
        assert!(!method_descriptor_drift(false, Some("a"), Some("b")));
        assert!(!method_descriptor_drift(false, Some("a"), None));
        assert!(!method_descriptor_drift(false, None, Some("b")));
    }

    #[test]
    fn drift_when_live_and_cached_differ() {
        assert!(method_descriptor_drift(true, Some("a"), Some("b")));
    }

    #[test]
    fn no_drift_when_live_and_cached_match() {
        assert!(!method_descriptor_drift(true, Some("a"), Some("a")));
    }

    #[test]
    fn no_drift_when_cached_missing_but_live_present() {
        // The drift banner must fire only on *positive evidence* of
        // drift (cached hash known + different). Absence of a local
        // cached fingerprint is the normal state after a kit is
        // restored to this device, uploaded from another device, or
        // created by a client version predating the cache field.
        // The server's descriptor-present flag is the
        // authoritative signal that a backup exists; firing a
        // permanent banner here would train users to tune it out.
        assert!(!method_descriptor_drift(true, Some("a"), None));
    }

    #[test]
    fn no_drift_when_live_missing() {
        // No Vault loaded yet — can't compute a comparison.
        // `present` being true here means another
        // device backed up the descriptor; we can't usefully flag
        // drift until this device has a wallet to diff against.
        assert!(!method_descriptor_drift(true, None, Some("b")));
        assert!(!method_descriptor_drift(true, None, None));
    }

    // ── Per-method descriptor drift (plan §PR3) ─────────────────────────────
    //
    // Each enabled method is judged independently against the live descriptor,
    // so re-sealing one method clears only that method's warning. The
    // positive-evidence-only rule carries over per slot (a missing slot never
    // fires that method's banner).

    #[test]
    fn drift_only_flags_the_stale_method() {
        // Password fresh (slot matches live), keychain stale (slot differs).
        let d = descriptor_drift(Some("live"), true, Some("live"), true, Some("old"));
        assert_eq!(
            d,
            DescriptorDrift {
                password: false,
                keychain: true
            }
        );
        assert!(d.any());
        // The inverse: password stale, keychain fresh.
        let d = descriptor_drift(Some("live"), true, Some("old"), true, Some("live"));
        assert_eq!(
            d,
            DescriptorDrift {
                password: true,
                keychain: false
            }
        );
    }

    #[test]
    fn drift_flags_both_methods_when_both_stale() {
        let d = descriptor_drift(Some("live"), true, Some("old"), true, Some("older"));
        assert_eq!(
            d,
            DescriptorDrift {
                password: true,
                keychain: true
            }
        );
    }

    #[test]
    fn no_drift_when_both_methods_match_live() {
        let d = descriptor_drift(Some("live"), true, Some("live"), true, Some("live"));
        assert!(!d.any());
    }

    #[test]
    fn drift_ignores_a_disabled_method() {
        // Keychain not present on the server → its slot is irrelevant even if it
        // differs from live; only the enabled password method is judged.
        let d = descriptor_drift(Some("live"), true, Some("old"), false, Some("whatever"));
        assert_eq!(
            d,
            DescriptorDrift {
                password: true,
                keychain: false
            }
        );
    }

    #[test]
    fn restored_install_with_no_slots_never_drifts() {
        // Both methods present on the server, but neither local slot exists
        // (fresh restore / other-device upload / pre-field install) →
        // positive-evidence-only means no banner for either method.
        let d = descriptor_drift(Some("live"), true, None, true, None);
        assert!(!d.any());
    }

    #[test]
    fn no_drift_before_a_vault_loads() {
        // No live fingerprint yet → nothing to compare, either method.
        let d = descriptor_drift(None, true, Some("old"), true, Some("older"));
        assert!(!d.any());
    }

    #[test]
    fn drift_copy_names_the_stale_methods() {
        let phone_only = DescriptorDrift {
            password: false,
            keychain: true,
        };
        assert!(drift_subtitle(&phone_only).contains("phone"));
        assert!(drift_warning_line(&phone_only).contains("phone"));

        let password_only = DescriptorDrift {
            password: true,
            keychain: false,
        };
        assert!(drift_subtitle(&password_only).contains("password"));
        assert!(drift_warning_line(&password_only).contains("password"));

        let both = DescriptorDrift {
            password: true,
            keychain: true,
        };
        assert!(drift_subtitle(&both).contains("both"));
        assert!(
            drift_warning_line(&both).contains("password")
                && drift_warning_line(&both).contains("phone")
        );

        // No drift → empty (callers guard with `any()`).
        let none = DescriptorDrift::default();
        assert!(drift_subtitle(&none).is_empty());
        assert!(drift_warning_line(&none).is_empty());
    }

    // ── Per-method backup overview + card state matrix (plan §PR1) ──────────
    //
    // These pin the single source of truth (`backup_overview`/`method_complete`)
    // and the card copy derived from it. The bug they guard: the old card read
    // only the password-kit halves, so a keychain-only backup showed "Create
    // Recovery Kit" with a contradictory Keychain pill and no timestamp.

    use crate::services::coincube::OwnerSelfRecoverySummary;

    /// Password-kit status with the given halves; `owner` folds in the keychain
    /// (phone) summary. `has_recovery_kit` mirrors the password halves (the
    /// server's password-scoped flag) so these fixtures match real payloads.
    fn status(
        seed: bool,
        descriptor: bool,
        updated_at: Option<&str>,
        owner: Option<OwnerSelfRecoverySummary>,
    ) -> RecoveryKitStatus {
        RecoveryKitStatus {
            has_recovery_kit: seed || descriptor,
            has_encrypted_seed: seed,
            has_encrypted_wallet_descriptor: descriptor,
            encryption_scheme: "aes-256-gcm".into(),
            created_at: None,
            updated_at: updated_at.map(|s| s.to_string()),
            owner_self: owner,
        }
    }

    fn owner(kinds: &[&str], updated_at: Option<&str>) -> OwnerSelfRecoverySummary {
        OwnerSelfRecoverySummary {
            has_recipient: true,
            tier: "full_cube".into(),
            envelope_kinds: kinds.iter().map(|k| k.to_string()).collect(),
            updated_at: updated_at.map(|s| s.to_string()),
        }
    }

    /// Convenience: card state for a mnemonic-with-vault cube.
    fn mnemonic_vault_state(st: &RecoveryKitStatus) -> CardState {
        recovery_kit_card_state(&backup_overview(Some(st)), true, false, true)
    }

    // ---- what's *in* the backup, not just which method protects it ----

    #[test]
    fn contents_name_the_halves_a_vaultless_kit_holds() {
        // The reported gap: a Vault-less Cube's card showed a lone "Password"
        // pill, which says how the kit is protected and nothing about whether
        // the Master Seed Phrase is actually in it.
        let st = status(true, false, Some("2026-08-08T17:35:00Z"), None);
        assert_eq!(
            backup_contents(&backup_overview(Some(&st))),
            BackupContents::Shared(vec!["Master Seed Phrase"])
        );
    }

    #[test]
    fn contents_name_both_halves_when_a_vault_is_backed_up() {
        let st = status(true, true, None, None);
        assert_eq!(
            backup_contents(&backup_overview(Some(&st))),
            BackupContents::Shared(vec!["Master Seed Phrase", "Wallet Descriptor"])
        );
    }

    #[test]
    fn contents_are_shared_when_both_methods_hold_the_same_halves() {
        // Two methods, identical contents — one row states it for both rather
        // than repeating the same pills under each method.
        let st = status(true, true, None, Some(owner(&["seed", "descriptor"], None)));
        assert_eq!(
            backup_contents(&backup_overview(Some(&st))),
            BackupContents::Shared(vec!["Master Seed Phrase", "Wallet Descriptor"])
        );
    }

    #[test]
    fn contents_split_per_method_when_the_methods_differ() {
        // A password kit with both halves next to a descriptor-only phone
        // envelope. A shared row would claim the phone can restore a seed it
        // doesn't hold, so each method states its own contents.
        let st = status(true, true, None, Some(owner(&["descriptor"], None)));
        assert_eq!(
            backup_contents(&backup_overview(Some(&st))),
            BackupContents::PerMethod(vec![
                ("Password", vec!["Master Seed Phrase", "Wallet Descriptor"]),
                ("Keychain", vec!["Wallet Descriptor"]),
            ])
        );
    }

    #[test]
    fn contents_are_nothing_when_no_method_holds_anything() {
        // Never-backed-up (the Create state) and a bare recipient with no
        // envelopes uploaded both render no contents row at all.
        assert_eq!(
            backup_contents(&backup_overview(None)),
            BackupContents::Nothing
        );
        let bare = RecoveryKitStatus {
            owner_self: Some(OwnerSelfRecoverySummary {
                has_recipient: true,
                tier: "full_cube".into(),
                envelope_kinds: Vec::new(),
                updated_at: None,
            }),
            ..status(false, false, None, None)
        };
        assert_eq!(
            backup_contents(&backup_overview(Some(&bare))),
            BackupContents::Nothing
        );
    }

    #[test]
    fn half_labels_are_ordered_seed_first() {
        // Fixed order so the pills don't reshuffle between renders.
        assert_eq!(
            half_labels(&MethodBackup {
                seed: true,
                descriptor: true
            }),
            vec!["Master Seed Phrase", "Wallet Descriptor"]
        );
        assert_eq!(
            half_labels(&MethodBackup {
                seed: false,
                descriptor: true
            }),
            vec!["Wallet Descriptor"]
        );
    }

    // ---- method_complete truth table (all shapes) ----

    #[test]
    fn method_complete_vaultless_needs_seed_only() {
        let seed_only = MethodBackup {
            seed: true,
            descriptor: false,
        };
        let desc_only = MethodBackup {
            seed: false,
            descriptor: true,
        };
        assert!(method_complete(&seed_only, false));
        assert!(!method_complete(&desc_only, false));
    }

    #[test]
    fn method_complete_vault_needs_both() {
        let both = MethodBackup {
            seed: true,
            descriptor: true,
        };
        let seed_only = MethodBackup {
            seed: true,
            descriptor: false,
        };
        let desc_only = MethodBackup {
            seed: false,
            descriptor: true,
        };
        assert!(method_complete(&both, true));
        assert!(!method_complete(&seed_only, true));
        assert!(!method_complete(&desc_only, true));
    }

    /// **I11**, from the card's side. A descriptor-only kit used to read
    /// "Wallet Descriptor backed up" on a passkey Cube; it now reads incomplete
    /// for every shape, because the seed belongs in the Kit.
    ///
    /// The card and `cube_backup_completeness` are supposed to be the same rule
    /// stated twice. This is the row where they used to differ.
    #[test]
    fn a_descriptor_only_kit_is_never_complete() {
        let desc_only = MethodBackup {
            seed: false,
            descriptor: true,
        };
        assert!(!method_complete(&desc_only, true));
        assert!(!method_complete(&desc_only, false));
    }

    // ---- backup_overview presence rules (master F1) ----

    #[test]
    fn overview_absent_status_is_all_none() {
        let o = backup_overview(None);
        assert!(o.password.is_none());
        assert!(o.keychain.is_none());
        assert!(o.last_updated.is_none());
        assert!(!o.any_enabled());
    }

    #[test]
    fn overview_recipient_without_envelopes_never_counts_as_keychain() {
        // A registered recipient with an empty envelope set is NOT a backup
        // (master F1 — presence = restorable material, not a bare recipient).
        let st = status(false, false, None, Some(owner(&[], None)));
        let o = backup_overview(Some(&st));
        assert!(
            o.keychain.is_none(),
            "empty envelope set must not enable keychain"
        );
        assert!(!o.any_enabled());
    }

    #[test]
    fn overview_keychain_reads_envelope_kinds() {
        let st = status(
            false,
            false,
            None,
            Some(owner(&["seed", "descriptor"], None)),
        );
        let o = backup_overview(Some(&st));
        let kc = o.keychain.expect("keychain enabled by envelopes");
        assert!(kc.seed && kc.descriptor);
        assert!(
            o.password.is_none(),
            "no password blobs → no password method"
        );
    }

    #[test]
    fn overview_last_updated_is_the_later_of_the_two() {
        // Keychain sealed after the password kit → keychain timestamp wins.
        let st = status(
            true,
            true,
            Some("2026-05-01T00:00:00Z"),
            Some(owner(&["seed", "descriptor"], Some("2026-06-01T00:00:00Z"))),
        );
        let o = backup_overview(Some(&st));
        assert_eq!(o.last_updated.as_deref(), Some("2026-06-01T00:00:00Z"));
    }

    // ---- password-only regression (master F4): the original four states ----

    #[test]
    fn password_only_no_kit_is_create() {
        let st = status(false, false, None, None);
        let s = mnemonic_vault_state(&st);
        assert_eq!(s.title, "Back up your Cube Recovery Kit");
        assert_eq!(s.primary_mode, RecoveryKitMode::Create);
    }

    #[test]
    fn password_only_seed_only_with_vault_prompts_add_descriptor() {
        let st = status(true, false, Some("2026-05-01T00:00:00Z"), None);
        let s = mnemonic_vault_state(&st);
        assert_eq!(s.title, "Finish backing up your Recovery Kit");
        assert_eq!(s.primary_mode, RecoveryKitMode::AddDescriptor);
        assert_eq!(s.primary_label, "Add Wallet Descriptor");
        assert!(s.subtitle.contains("Wallet Descriptor"));
    }

    #[test]
    fn password_only_descriptor_only_prompts_add_seed() {
        let st = status(false, true, Some("2026-05-01T00:00:00Z"), None);
        let s = mnemonic_vault_state(&st);
        assert_eq!(s.title, "Finish backing up your Recovery Kit");
        assert_eq!(s.primary_mode, RecoveryKitMode::AddSeed);
        assert_eq!(s.primary_label, "Add Master Seed Phrase");
        assert!(s.subtitle.contains("Master Seed Phrase"));
    }

    #[test]
    fn password_only_both_halves_is_backed_up() {
        let st = status(true, true, Some("2026-05-08T14:36:50Z"), None);
        let s = mnemonic_vault_state(&st);
        assert_eq!(s.title, "Recovery Kit backed up");
        assert_eq!(s.primary_mode, RecoveryKitMode::Rotate);
        assert_eq!(s.primary_label, "Update");
        assert!(s.subtitle.contains("Last updated"));
        assert!(s.subtitle.contains("8 May 2026"));
    }

    #[test]
    fn backed_up_state_survives_spanish() {
        // The card reads its status word from `title`, so the title must stay
        // English; translating it once made a backed-up kit read "Incomplete".
        let st = status(true, true, Some("2026-05-08T14:36:50Z"), None);
        let s = coincube_ui::locale::render_in(coincube_ui::locale::Language::Spanish, || {
            mnemonic_vault_state(&st)
        });
        assert_eq!(s.title, "Recovery Kit backed up");
        assert_eq!(s.primary_label, "Update");
        assert!(s.subtitle.starts_with("Última actualización"));
    }

    #[test]
    fn password_only_seed_only_vaultless_is_backed_up() {
        // A seed-only kit on a vaultless cube is already complete.
        let st = status(true, false, Some("2026-05-01T00:00:00Z"), None);
        let s = recovery_kit_card_state(&backup_overview(Some(&st)), false, false, true);
        assert_eq!(s.title, "Recovery Kit backed up");
        assert_eq!(s.primary_mode, RecoveryKitMode::Rotate);
    }

    // ---- keychain-only (the shipped bug) ----

    #[test]
    fn keychain_only_full_cube_is_backed_up_with_pill_and_remove() {
        // Phone-sealed both halves, no password kit. This used to show "Create".
        let st = status(
            false,
            false,
            None,
            Some(owner(&["seed", "descriptor"], Some("2026-06-01T09:00:00Z"))),
        );
        let o = backup_overview(Some(&st));
        let s = mnemonic_vault_state(&st);
        assert_eq!(s.title, "Recovery Kit backed up");
        assert_eq!(s.primary_mode, RecoveryKitMode::Rotate);
        assert!(
            s.subtitle.contains("1 Jun 2026"),
            "timestamp from owner_self: {}",
            s.subtitle
        );
        // Keychain pill shows; password pill does not; Remove is offered.
        assert_eq!(backup_methods_present(Some(&st)), (false, true));
        assert!(o.any_enabled(), "has_kit → Remove button visible");
    }

    #[test]
    fn keychain_descriptor_only_on_vault_cube_misses_the_seed() {
        // Phone sealed only the descriptor; a mnemonic+vault cube needs the seed.
        let st = status(
            false,
            false,
            None,
            Some(owner(&["descriptor"], Some("2026-06-01T00:00:00Z"))),
        );
        let s = mnemonic_vault_state(&st);
        assert_eq!(s.title, "Finish backing up your Recovery Kit");
        assert_eq!(
            s.subtitle, "Your phone backup is missing the Master Seed Phrase.",
            "keychain gap must name the phone method + missing seed"
        );
        // No password method → the CTA re-seals via Rotate.
        assert_eq!(s.primary_mode, RecoveryKitMode::Rotate);
    }

    // ---- cross-method halves: never "backed up" (master F6) ----

    #[test]
    fn cross_method_halves_surface_both_gaps_and_are_never_backed_up() {
        // Password holds the seed only; keychain holds the descriptor only.
        // Neither method is independently complete for a mnemonic+vault cube,
        // so the Cube is NOT backed up (per-method conjunction, not a union).
        let st = status(
            true,
            false,
            Some("2026-05-01T00:00:00Z"),
            Some(owner(&["descriptor"], Some("2026-05-02T00:00:00Z"))),
        );
        let s = mnemonic_vault_state(&st);
        assert_eq!(s.title, "Finish backing up your Recovery Kit");
        // Both gaps listed, password first.
        assert_eq!(
            s.subtitle,
            "Your password backup is missing the Wallet Descriptor. \
             Your phone backup is missing the Master Seed Phrase."
        );
        // CTA closes the first (password) gap.
        assert_eq!(s.primary_mode, RecoveryKitMode::AddDescriptor);
        // Both pills present (presence, not completeness).
        assert_eq!(backup_methods_present(Some(&st)), (true, true));
    }

    // ---- both methods complete ----

    #[test]
    fn both_methods_complete_shows_two_pills_and_later_timestamp() {
        let st = status(
            true,
            true,
            Some("2026-05-01T00:00:00Z"),
            Some(owner(&["seed", "descriptor"], Some("2026-07-01T00:00:00Z"))),
        );
        let o = backup_overview(Some(&st));
        let s = mnemonic_vault_state(&st);
        assert_eq!(s.title, "Recovery Kit backed up");
        assert_eq!(backup_methods_present(Some(&st)), (true, true));
        // Later of the two timestamps (keychain).
        assert_eq!(o.last_updated.as_deref(), Some("2026-07-01T00:00:00Z"));
        assert!(s.subtitle.contains("1 Jul 2026"));
    }

    // ---- older API (owner_self None) → exact prior behavior ----

    #[test]
    fn older_api_without_owner_self_matches_password_only_states() {
        // owner_self absent on older APIs → keychain never enabled; the card
        // behaves exactly as the pre-keychain password-only card did.
        let complete = status(true, true, Some("2026-05-01T00:00:00Z"), None);
        assert_eq!(
            mnemonic_vault_state(&complete).title,
            "Recovery Kit backed up"
        );
        let absent = status(false, false, None, None);
        assert_eq!(
            mnemonic_vault_state(&absent).title,
            "Back up your Cube Recovery Kit"
        );
        assert!(backup_overview(Some(&absent)).keychain.is_none());
    }

    // ---- one card for both shapes (I11) ----

    /// The card no longer branches on shape, so a passkey Cube with a
    /// descriptor-only kit gets the same "finish backing up" treatment a
    /// mnemonic Cube does — where it used to be told it was done.
    #[test]
    fn a_descriptor_only_kit_reads_incomplete() {
        let st = status(false, true, Some("2026-05-01T00:00:00Z"), None);
        let s = recovery_kit_card_state(&backup_overview(Some(&st)), true, false, true);
        assert_eq!(s.title, "Finish backing up your Recovery Kit");
        assert!(s.subtitle.contains("Master Seed Phrase"));
        assert_eq!(s.primary_mode, RecoveryKitMode::AddSeed);
    }

    #[test]
    fn no_kit_is_create() {
        let st = status(false, false, None, None);
        let s = recovery_kit_card_state(&backup_overview(Some(&st)), true, false, true);
        assert_eq!(s.title, "Back up your Cube Recovery Kit");
        assert_eq!(s.primary_mode, RecoveryKitMode::Create);
    }

    // ---- unavailable status never implies a missing kit ----

    #[test]
    fn not_loaded_shows_loading_or_unavailable() {
        let loading = recovery_kit_card_state(&backup_overview(None), true, true, false);
        assert_eq!(loading.title, "Cube Recovery Kit");
        assert!(loading.subtitle.contains("Checking"));
        let idle = recovery_kit_card_state(&backup_overview(None), true, false, false);
        assert!(idle.subtitle.contains("status is unavailable"));
    }

    // ---- later_timestamp comparison ----

    #[test]
    fn later_timestamp_prefers_the_later_instant() {
        assert_eq!(
            later_timestamp(Some("2026-01-01T00:00:00Z"), Some("2026-02-01T00:00:00Z")).as_deref(),
            Some("2026-02-01T00:00:00Z")
        );
        assert_eq!(
            later_timestamp(Some("2026-03-01T00:00:00Z"), Some("2026-02-01T00:00:00Z")).as_deref(),
            Some("2026-03-01T00:00:00Z")
        );
        // Only one side present.
        assert_eq!(
            later_timestamp(Some("2026-03-01T00:00:00Z"), None).as_deref(),
            Some("2026-03-01T00:00:00Z")
        );
        assert_eq!(later_timestamp(None, None), None);
        // One side unparseable → prefer the parseable side.
        assert_eq!(
            later_timestamp(Some("not-a-date"), Some("2026-02-01T00:00:00Z")).as_deref(),
            Some("2026-02-01T00:00:00Z")
        );
    }

    /// Tenshu v175 hides Liquid (master §3 decision 2), so no escrow tier
    /// promises a keyholder a Liquid wallet. Restore "Liquid" in the Full Cube
    /// copy when Liquid reopens.
    #[test]
    fn escrow_copy_names_no_liquid_wallet() {
        assert!(escrow_copy(Some(EscrowTier::FullCube), true)
            .contains("restore the entire Cube — Spark and Vault."));
        for spark in [true, false] {
            for tier in [
                None,
                Some(EscrowTier::Off),
                Some(EscrowTier::VaultOnly),
                Some(EscrowTier::FullCube),
            ] {
                assert!(!escrow_copy(tier, spark).contains("Liquid"), "{:?}", tier);
            }
        }
    }

    /// Spark exists only where `features::spark` says so (Bitcoin and
    /// Regtest), so Full Cube elsewhere promises only the Vault.
    #[test]
    fn full_cube_escrow_copy_names_spark_only_where_spark_exists() {
        let elsewhere = escrow_copy(Some(EscrowTier::FullCube), false);
        assert!(!elsewhere.contains("Spark"), "{}", elsewhere);
        assert!(elsewhere.contains("restore the entire Cube, including its Vault."));
        for tier in [None, Some(EscrowTier::Off), Some(EscrowTier::VaultOnly)] {
            assert_eq!(escrow_copy(tier, true), escrow_copy(tier, false));
        }
    }
}
