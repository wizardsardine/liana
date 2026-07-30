use iced::widget::{pick_list, tooltip, Column, Row, Space, Toggler};
use iced::{Alignment, Length};

use coincube_ui::color;
use coincube_ui::component::text::*;
use coincube_ui::component::{button, card, tooltip_custom};
use coincube_ui::icon;
use coincube_ui::theme;
use coincube_ui::widget::{ColumnExt, Element};

use crate::app::cache;
use crate::app::menu::Menu;
use crate::app::settings::display::DisplayMode;
use crate::app::settings::fiat::PriceSetting;
use crate::app::settings::unit::{BitcoinDisplayUnit, UnitSetting};
use crate::app::state::settings::general::SettingsSection;
use crate::app::state::settings::recovery_alerts::RecoveryAlerts;
use crate::app::state::settings::recovery_kit::RecoveryKit;
use crate::app::view::dashboard;
use crate::app::view::message::*;
use crate::services::coincube::RecoveryKitStatus;
use crate::services::fiat::{Currency, ALL_PRICE_SOURCES};
use crate::services::inheritance::EscrowTier;

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
    backup_state: &'a crate::app::state::settings::general::BackupSeedState,
    backup_pin: &'a crate::pin_input::PinInput,
    backup_mnemonic: Option<&'a [String]>,
    recovery_kit: Option<&'a RecoveryKit>,
    recovery_alerts: Option<&'a RecoveryAlerts>,
) -> Element<'a, Message> {
    match section {
        SettingsSection::General => general_section(
            menu,
            cache,
            new_price_setting,
            new_unit_setting,
            currencies_list,
            show_direction_badges,
        ),
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

/// General tab: app/display preferences only.
fn general_section<'a>(
    menu: &'a Menu,
    cache: &'a cache::Cache,
    new_price_setting: &'a PriceSetting,
    new_unit_setting: &'a UnitSetting,
    currencies_list: &'a [Currency],
    show_direction_badges: bool,
) -> Element<'a, Message> {
    let col = Column::new()
        .spacing(20)
        .push(super::header("General", SettingsMessage::GeneralSection))
        .push(network_row(cache.network))
        .push(bitcoin_display_unit(new_unit_setting))
        .push(display_mode_toggle(cache.display_mode))
        .push(direction_badges_toggle(show_direction_badges))
        .push(fiat_price(new_price_setting, currencies_list));

    dashboard(menu, cache, col)
}

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
        if let Some(wizard) = super::backup::dispatch(backup_state, backup_pin, backup_mnemonic) {
            return dashboard(menu, cache, Column::new().spacing(20).push(wizard));
        }
    }

    let mut col = Column::new()
        .spacing(20)
        .push(super::header("Recovery", SettingsMessage::RecoverySection))
        .push(backup_master_seed_card(cache.current_cube_backed_up));

    // Connect-hosted Recovery Kit card. Render only when the outer
    // SettingsState had a `RecoveryKit` on hand — i.e. when the downcasting
    // wrapper invoked `view_with_recovery_kit`. Falling back to no-card when
    // `None` keeps the trait-based `view` callers harmless.
    if let Some(rk) = recovery_kit {
        if !cache.connect_authenticated {
            // The Recovery Kit lives in the Connect account, so it needs sign-in
            // first. Reuse the shared Connect sign-in prompt (same card + "Sign
            // In" → `OpenConnectSignIn` button used by Avatar / Members) rather
            // than a "Create Recovery Kit" CTA that would only dead-end.
            col = col.push(crate::app::view::connect::sign_in_prompt::sign_in_prompt(
                "back up your Cube Recovery Kit",
            ));
        } else {
            // W12 drift, per method (PR 3): compare the live descriptor
            // fingerprint (refreshed every App tick) against each method's
            // last-backed-up fingerprint (persisted per-method on `CubeSettings`).
            // A method's descriptor presence comes from the server status via
            // `backup_overview`; the positive-evidence-only rule keeps a missing
            // slot from firing that method's banner.
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
            col = col.push(recovery_kit_card(
                cache.current_cube_is_passkey,
                cache.has_vault,
                rk.status.as_ref(),
                rk.status_loading,
                drift,
            ));
        }
    }

    // Vault Recovery Alerts card (Estate Notifications — PR 2). Same threading
    // discipline as the Recovery Kit card above.
    if let Some(ra) = recovery_alerts {
        col = col.push(recovery_alerts_card(ra));
    }

    dashboard(menu, cache, col)
}

/// Vault Recovery Alerts card. Two independent controls backed by two server
/// records: the **Recovery alerts** toggle (monitoring; not Estate-gated) and
/// the **What keyholders can recover** escrow selector (Estate-gated). The
/// keyholder list lives under the alerts toggle (it's who alerts notify). See
/// `PLAN-recovery-alerts-cleanup.md` PR 2.
fn recovery_alerts_card<'a>(ra: &'a RecoveryAlerts) -> Element<'a, Message> {
    let mut body = Column::new()
        .spacing(10)
        .push(text("Vault Recovery Alerts").bold())
        .push(
            text(
                "Let COINCUBE watch the chain and alert your keyholders when a recovery path \
                 for this Vault opens.",
            )
            .size(13),
        );

    // Locked affordance when the account can't use recovery alerts at all.
    if !ra.entitled {
        body = body
            .push(
                text(
                    "Recovery alerts aren't available on your current plan. Upgrade your Connect \
                     plan to alert your keyholders when this Vault's recovery window opens.",
                )
                .size(13),
            )
            .push(Space::new().height(Length::Fixed(12.0)))
            .push(
                button::primary(None, "View plans")
                    .width(Length::Fixed(160.0))
                    .on_press(Message::OpenPlanBilling),
            );
        return card::simple(body).width(Length::Fill).into();
    }

    // No Connect vault to monitor yet.
    if ra.no_vault {
        body = body.push(
            text("Create a Vault and register it with Connect to enable recovery alerts.").size(13),
        );
        return card::simple(body).width(Length::Fill).into();
    }

    if ra.loading && ra.status.is_none() {
        body = body.push(text("Loading\u{2026}").size(13));
        return card::simple(body).width(Length::Fill).into();
    }

    let alerts_on = ra.alerts_on();
    let busy = ra.submitting;

    // ── Control 1: the Recovery alerts toggle (monitoring on/off) ──────────
    body = body.push(Space::new().height(Length::Fixed(4.0)));
    body = body.push(
        Row::new()
            .spacing(20)
            .align_y(Alignment::Center)
            .push(text("Recovery alerts").bold())
            .push(Space::new().width(Length::Fill))
            .push(
                Toggler::new(alerts_on)
                    .on_toggle(|on| {
                        SettingsMessage::RecoveryAlerts(RecoveryAlertsMessage::ToggleAlerts(on))
                            .into()
                    })
                    .width(50)
                    .style(theme::toggler::orange),
            ),
    );
    // The C4 disclosure — stated plainly, no euphemisms.
    body = body.push(
        text(
            "When on, COINCUBE learns only the block height at which this Vault's recovery window \
             opens and that this desktop checked in — never your addresses or balances.",
        )
        .size(13),
    );

    // Residual nudge (PR 3): with keyholders present but alerts off, they'd
    // never be told when the recovery window opens. Not a re-prompt — the
    // one-time consent card is durable — just a standing warning on the card.
    if !alerts_on && !ra.keyholders.is_empty() {
        body = body.push(
            text(
                "Your keyholders will NOT be alerted when this Vault's recovery window opens. \
                 Turn on Recovery alerts to fix this.",
            )
            .size(13)
            .style(theme::text::error),
        );
    }

    // Inline confirm for turning alerts off. Never silent: when a recovery kit
    // is escrowed, the confirm discloses that it will be deleted too (escrow
    // can't outlive alerts).
    if ra.confirming_alerts_off {
        body = body.push(Space::new().height(Length::Fixed(6.0)));
        let warning = if ra.has_escrow() {
            "Turn off recovery alerts? COINCUBE will stop watching for this Vault's recovery \
             window. This also deletes the encrypted recovery kit currently stored for your \
             keyholders — they'll no longer be able to recover this Vault."
        } else {
            "Turn off recovery alerts? COINCUBE will stop watching for this Vault's recovery \
             window, so your keyholders won't be alerted when it opens."
        };
        body = body.push(text(warning).size(13));
        body = body.push(
            Row::new()
                .spacing(8)
                .push(
                    button::primary(None, "Turn off")
                        .padding([8, 14])
                        .on_press_maybe(
                            (!busy).then_some(
                                SettingsMessage::RecoveryAlerts(
                                    RecoveryAlertsMessage::ConfirmAlertsOff,
                                )
                                .into(),
                            ),
                        ),
                )
                .push(button::secondary(None, "Cancel").padding([8, 14]).on_press(
                    SettingsMessage::RecoveryAlerts(RecoveryAlertsMessage::CancelAlertsOff).into(),
                )),
        );
    }

    // The keyholder list belongs to alerts (who'd be notified), so it shows
    // whenever alerts are on.
    if alerts_on {
        body = body.push(Space::new().height(Length::Fixed(4.0)));
        body = body.push(text("Keyholders who'd be notified").bold().size(14));
        if ra.keyholders.is_empty() {
            body = body.push(
                text("No keyholders on this Cube yet — add keyholders so someone is alerted.")
                    .size(13),
            );
        } else {
            let mut who = Column::new().spacing(2);
            for email in &ra.keyholders {
                who = who.push(text(email.as_str()).size(13));
            }
            body = body.push(who);
        }
    }

    // ── Control 2: the escrow selector (what keyholders can recover) ───────
    body = body.push(Space::new().height(Length::Fixed(10.0)));
    body = body.push(text("What keyholders can recover").bold().size(14));
    body = body.push(escrow_selector(ra, alerts_on, busy));

    if let Some(err) = ra.error.as_deref() {
        body = body.push(text(err).size(13).style(theme::text::error));
    }

    card::simple(body).width(Length::Fill).into()
}

/// The "What keyholders can recover" escrow selector — Estate-gated (locked
/// affordance when the account lacks the `inheritanceEscrow` entitlement). The
/// tier is server-derived; picking Vault only / Full Cube auto-enables alerts.
fn escrow_selector<'a>(
    ra: &'a RecoveryAlerts,
    alerts_on: bool,
    busy: bool,
) -> Element<'a, Message> {
    // Locked affordance when the account can't escrow a recovery kit.
    if !ra.escrow_entitled {
        return Column::new()
            .spacing(10)
            .push(
                text(
                    "An encrypted recovery kit for your keyholders is part of the Estate plan. \
                     Recovery alerts above still work on your current plan.",
                )
                .size(13),
            )
            .push(
                button::primary(None, "View plans")
                    .width(Length::Fixed(160.0))
                    .on_press(Message::OpenPlanBilling),
            )
            .into();
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

    let selector = Row::new()
        .spacing(8)
        .push(escrow_button(
            "Nothing",
            EscrowTier::Off,
            display_tier,
            busy,
        ))
        .push(escrow_button(
            "Vault only",
            EscrowTier::VaultOnly,
            display_tier,
            busy,
        ))
        .push(escrow_button(
            "Full Cube",
            EscrowTier::FullCube,
            display_tier,
            busy,
        ));

    let mut col = Column::new()
        .spacing(10)
        .push(selector)
        .push(text(escrow_copy(display_tier)).size(13));

    // Auto-enable disclosure: picking a kit while alerts are off turns them on.
    if !alerts_on {
        col = col.push(
            text("Choosing Vault only or Full Cube also turns Recovery alerts on.")
                .size(13)
                .style(theme::text::secondary),
        );
    }

    // Full-Cube re-confirms the PIN before exporting the seed into escrow.
    if ra.awaiting_pin {
        col = col.push(Space::new().height(Length::Fixed(6.0)));
        col = col.push(
            text("Enter your PIN to include this Cube's seed in the encrypted recovery kit.")
                .size(13),
        );
        col = col.push(
            iced::widget::text_input("PIN", ra.pin.as_str())
                .secure(true)
                .padding(8)
                .on_input(|s| {
                    SettingsMessage::RecoveryAlerts(RecoveryAlertsMessage::EscrowPinChanged(
                        s.into(),
                    ))
                    .into()
                })
                .on_submit(
                    SettingsMessage::RecoveryAlerts(RecoveryAlertsMessage::ConfirmFullCube).into(),
                ),
        );
        col = col.push(
            Row::new()
                .spacing(8)
                .push(
                    button::primary(None, "Confirm")
                        .padding([8, 14])
                        .on_press_maybe(
                            (!busy).then_some(
                                SettingsMessage::RecoveryAlerts(
                                    RecoveryAlertsMessage::ConfirmFullCube,
                                )
                                .into(),
                            ),
                        ),
                )
                .push(button::secondary(None, "Cancel").padding([8, 14]).on_press(
                    SettingsMessage::RecoveryAlerts(RecoveryAlertsMessage::CancelFullCube).into(),
                )),
        );
    }

    col.into()
}

/// An escrow-tier option button. Highlighted (primary) when it's the active
/// tier (or while collecting the PIN for a Full-Cube enrolment); disabled while
/// a change is in flight.
fn escrow_button<'a>(
    label: &'static str,
    this: EscrowTier,
    active: Option<EscrowTier>,
    busy: bool,
) -> Element<'a, Message> {
    // `active == None` (tier on but unknown on this device) highlights nothing,
    // leaving every tier pressable so the owner can confirm/change it.
    let is_active = active == Some(this);
    let on_press = (!busy && !is_active).then_some(
        SettingsMessage::RecoveryAlerts(RecoveryAlertsMessage::SelectEscrow(this)).into(),
    );
    if is_active {
        button::primary(None, label)
            .padding([8, 14])
            .on_press_maybe(None)
            .into()
    } else {
        button::secondary(None, label)
            .padding([8, 14])
            .on_press_maybe(on_press)
            .into()
    }
}

/// Plain-language trade-off for each escrow tier (no euphemisms — the
/// self-custody trust model demands it; see the ECIES decision record).
/// Everything is encrypted to the keyholders' own keys: COINCUBE can read
/// neither the descriptor nor the seed.
fn escrow_copy(tier: Option<EscrowTier>) -> &'static str {
    match tier {
        // Escrow is on, but an older API doesn't report which tier — state that
        // honestly rather than assert (and possibly mis-state) descriptor-only
        // vs seed escrow.
        None => {
            "A recovery kit is stored, but this device can't tell which kind (this server doesn't \
             report it). Reselect Vault only or Full Cube to confirm or change it, or Nothing to \
             remove it."
        }
        Some(EscrowTier::Off) => {
            "Nothing — alerts only: your keyholders are told when the recovery window opens, but \
             no encrypted recovery kit is stored, so they can't recover this Vault themselves."
        }
        Some(EscrowTier::VaultOnly) => {
            "Vault only: an encrypted copy of this Vault's descriptor is sealed to each \
             keyholder's own key — only they can open it, never COINCUBE. When the recovery \
             window opens, a keyholder recovers the watch-only Vault and sweeps the funds. The \
             seed is never escrowed."
        }
        Some(EscrowTier::FullCube) => {
            "Full Cube: seals this Cube's seed AND descriptor to each keyholder's own key. A \
             keyholder can restore the entire Cube — Liquid, Spark and Vault. COINCUBE can never \
             read either; only the keyholder's key can. You'll re-confirm your PIN to include the \
             seed."
        }
    }
}

/// The "Backup Master Seed Phrase" card shown on the normal General
/// Settings page. Shows a different label depending on whether the
/// current cube has already been backed up.
fn backup_master_seed_card<'a>(backed_up: bool) -> Element<'a, Message> {
    let (title, subtitle, button_label) = if backed_up {
        (
            "Master Seed Phrase Backed Up",
            "You've already recorded your recovery phrase. You can view it again if needed.",
            "View Again",
        )
    } else {
        (
            "Backup Master Seed Phrase",
            "Write down your 12-word recovery phrase as a backup. This is the only way to recover your Cube if you forget your PIN and do not have a Recovery Kit.",
            "Start Backup",
        )
    };
    card::simple(
        Row::new()
            .spacing(20)
            .align_y(Alignment::Center)
            .push(
                Column::new()
                    .spacing(4)
                    .width(Length::Fill)
                    .push(text(title).bold())
                    .push(text(subtitle).size(14)),
            )
            .push(
                button::secondary(None, button_label)
                    .padding([8, 16])
                    .width(Length::Fixed(160.0))
                    .on_press(SettingsMessage::BackupMasterSeed(BackupWalletMessage::Start).into()),
            ),
    )
    .width(Length::Fill)
    .into()
}

/// Small rounded "pill" badge naming a Recovery-Kit backup method in use
/// (e.g. "Password", "Keychain"). Subtle translucent-orange fill + orange
/// border, but the label uses the **theme's primary text colour** (not orange)
/// so it stays high-contrast in both dark and light mode — orange-on-orange was
/// unreadable, especially on the light warm-paper background.
pub(crate) fn backup_pill<'a>(label: &'static str) -> Element<'a, Message> {
    iced::widget::container(text(label).size(12).bold())
        .padding([3, 10])
        .style(|t: &theme::Theme| iced::widget::container::Style {
            text_color: Some(t.colors.text.primary),
            background: Some(iced::Background::Color(color::TRANSPARENT_ORANGE)),
            border: iced::Border {
                radius: 10.0.into(),
                width: 1.0,
                color: color::ORANGE,
            },
            ..Default::default()
        })
        .into()
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

/// Whether a method independently holds everything the Cube's shape requires
/// (master §definitions: completeness is per-method):
///   * passkey — descriptor only (the seed is unextractable on-device);
///   * mnemonic + vault — seed **and** descriptor;
///   * mnemonic, no vault — seed alone.
pub(crate) fn method_complete(m: &MethodBackup, has_vault: bool, is_passkey: bool) -> bool {
    if is_passkey {
        m.descriptor
    } else if has_vault {
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
            dt.with_timezone(&chrono::Utc)
                .format("%-d %b %Y, %H:%M UTC")
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

/// The "Create" (nothing backed up yet) card state, per Cube shape.
fn create_state(is_passkey: bool) -> CardState {
    if is_passkey {
        CardState {
            title: "Back up your Wallet Descriptor",
            subtitle: "Your Master Seed Phrase is protected by your passkey and isn't included \
                       in the Recovery Kit — we back up the Wallet Descriptor only."
                .to_string(),
            primary_label: "Create Recovery Kit",
            primary_mode: RecoveryKitMode::Create,
        }
    } else {
        CardState {
            title: "Back up your Cube Recovery Kit",
            subtitle: "Back up your Master Seed Phrase and Wallet Descriptor to your Connect \
                       account so you can restore your Cube if you lose this device."
                .to_string(),
            primary_label: "Create Recovery Kit",
            primary_mode: RecoveryKitMode::Create,
        }
    }
}

/// A sentence naming the missing half of an incomplete enabled method (`label`
/// is "password" / "phone"), or `None` when the method is complete for the
/// shape. An enabled method holds at least one half, so for the mnemonic+vault
/// shape the gap is whichever half is absent; for the descriptor-only (passkey)
/// and seed-only (vaultless) shapes it names that required half.
fn method_gap_sentence(
    m: &MethodBackup,
    has_vault: bool,
    is_passkey: bool,
    label: &str,
) -> Option<String> {
    if method_complete(m, has_vault, is_passkey) {
        return None;
    }
    let missing_half = if is_passkey || m.seed {
        // Passkey needs the descriptor; a mnemonic+vault method that already
        // holds the seed is missing the descriptor.
        "Wallet Descriptor"
    } else {
        // No seed present — the seed is what's missing (mnemonic, either shape).
        "Master Seed Phrase"
    };
    Some(format!(
        "Your {} backup is missing the {}.",
        label, missing_half
    ))
}

/// The CTA that closes the *password* method's gap, when the password method is
/// enabled and incomplete. `None` when password is absent or already complete —
/// the gap is then on the keychain method, closed by re-sealing via `Rotate`.
fn password_gap_cta(
    password: &Option<MethodBackup>,
    has_vault: bool,
    is_passkey: bool,
) -> Option<(&'static str, RecoveryKitMode)> {
    let pw = password.as_ref()?;
    if method_complete(pw, has_vault, is_passkey) {
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
/// loaded yet" (loading / sign-in copy — mnemonic only, matching prior copy).
fn recovery_kit_card_state(
    overview: &BackupOverview,
    is_passkey: bool,
    has_vault: bool,
    loading: bool,
    status_present: bool,
) -> CardState {
    // Not loaded yet. The passkey card historically shows its Create copy here;
    // the mnemonic card shows a loading / sign-in line. Preserve both.
    if !status_present {
        if is_passkey {
            return create_state(true);
        }
        return CardState {
            title: "Cube Recovery Kit",
            subtitle: if loading {
                "Checking your Connect account…".to_string()
            } else {
                "Sign in to Connect to back up your Cube Recovery Kit.".to_string()
            },
            primary_label: "Create Recovery Kit",
            primary_mode: RecoveryKitMode::Create,
        };
    }

    // No method enabled → Create.
    if !overview.any_enabled() {
        return create_state(is_passkey);
    }

    // Per-method completeness. A disabled method vacuously satisfies the
    // conjunction, so `unwrap_or(true)`.
    let pw_complete = overview
        .password
        .as_ref()
        .map(|m| method_complete(m, has_vault, is_passkey))
        .unwrap_or(true);
    let kc_complete = overview
        .keychain
        .as_ref()
        .map(|m| method_complete(m, has_vault, is_passkey))
        .unwrap_or(true);

    if pw_complete && kc_complete {
        // Every enabled method complete → backed up.
        let title = if is_passkey {
            "Wallet Descriptor backed up"
        } else {
            "Recovery Kit backed up"
        };
        let subtitle = format!(
            "Last updated {}.",
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
        if let Some(s) = method_gap_sentence(pw, has_vault, is_passkey, "password") {
            gaps.push(s);
        }
    }
    if let Some(kc) = &overview.keychain {
        if let Some(s) = method_gap_sentence(kc, has_vault, is_passkey, "phone") {
            gaps.push(s);
        }
    }

    // The CTA closes the first gap (password first); a keychain-only gap
    // re-seals via Rotate (the protection-choice → phone re-seal fills whatever
    // the tier requires in one pass).
    let (primary_label, primary_mode) = password_gap_cta(&overview.password, has_vault, is_passkey)
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
/// - `is_passkey`: when true, the seed is unextractable on-device and
///   only the descriptor can be backed up; the card has a reduced
///   two-state variant and is suppressed entirely on passkey cubes
///   without a Vault (nothing to back up).
/// - `has_vault`: gates the "complete" copy on mnemonic cubes — a
///   seed-only kit on a vaultless cube is already "complete" from the
///   user's perspective, so the CTA becomes "Update" rather than
///   "Add Wallet Descriptor".
fn recovery_kit_card<'a>(
    is_passkey: bool,
    has_vault: bool,
    status: Option<&RecoveryKitStatus>,
    loading: bool,
    drift: DescriptorDrift,
) -> Element<'a, Message> {
    // Passkey + no vault => nothing to back up yet. Render a thin
    // informational card rather than the regular flow.
    if is_passkey && !has_vault {
        return card::simple(
            Column::new()
                .spacing(4)
                .push(text("Back up your Wallet Descriptor").bold())
                .push(
                    text(
                        "Passkey Cubes back up the Wallet Descriptor only — create a Vault \
                         to enable Recovery-Kit backup.",
                    )
                    .size(14),
                ),
        )
        .width(Length::Fill)
        .into();
    }

    // One source of truth for what's backed up, by which method (master
    // §definitions). Drives the card state, the pills, and Remove visibility.
    let overview = backup_overview(status);
    let CardState {
        title,
        subtitle,
        primary_label,
        primary_mode,
    } = recovery_kit_card_state(&overview, is_passkey, has_vault, loading, status.is_some());

    // Drift overrides the "complete" state: primary CTA becomes "Update now"
    // and the subtitle swaps to a drift warning naming the stale method(s)
    // (per-method drift, PR 3). Each method is judged independently, so a
    // keychain-only kit can now drift (unlike PR 1, which kept drift
    // password-scoped); re-sealing one method clears only its warning.
    let (subtitle, primary_label, primary_mode) = if drift.any() {
        (
            drift_subtitle(&drift).to_string(),
            "Update now",
            RecoveryKitMode::Rotate,
        )
    } else {
        (subtitle, primary_label, primary_mode)
    };

    // Render with Remove button when any method is enabled.
    let has_kit = overview.any_enabled();
    let mut actions = Row::new().spacing(10).align_y(Alignment::Center).push(
        button::primary(None, primary_label)
            .padding([8, 16])
            .width(Length::Fixed(220.0))
            .on_press(SettingsMessage::RecoveryKit(RecoveryKitMessage::Start(primary_mode)).into()),
    );
    if has_kit {
        actions = actions.push(
            button::secondary(None, "Remove")
                .padding([8, 16])
                .width(Length::Fixed(120.0))
                .on_press(SettingsMessage::RecoveryKit(RecoveryKitMessage::Remove).into()),
        );
    }

    // Which backup methods are actually in place (presence, not completeness) —
    // drives the method pills. A partial method still shows its pill.
    let (password_present, keychain_present) =
        (overview.password.is_some(), overview.keychain.is_some());

    let mut body = Column::new()
        .spacing(4)
        .width(Length::Fill)
        .push(text(title).bold())
        .push(text(subtitle).size(14));
    if password_present || keychain_present {
        let mut pills = Row::new().spacing(6).align_y(Alignment::Center);
        if password_present {
            pills = pills.push(backup_pill("Password"));
        }
        if keychain_present {
            pills = pills.push(backup_pill("Keychain"));
        }
        body = body
            .push(Space::new().height(Length::Fixed(2.0)))
            .push(pills);
    }
    if drift.any() {
        body = body.push(
            text(drift_warning_line(&drift))
                .size(12)
                .style(coincube_ui::theme::text::warning),
        );
    }

    card::simple(
        Row::new()
            .spacing(20)
            .align_y(Alignment::Center)
            .push(body)
            .push(actions),
    )
    .width(Length::Fill)
    .into()
}

fn network_row<'a>(network: coincube_core::miniscript::bitcoin::Network) -> Element<'a, Message> {
    use coincube_core::miniscript::bitcoin::Network;
    let label = match network {
        Network::Bitcoin => "Mainnet",
        Network::Regtest => "Regtest",
        Network::Testnet => "Testnet",
        Network::Signet => "Signet",
        _ => "Unknown",
    };
    card::simple(
        Row::new()
            .spacing(20)
            .align_y(Alignment::Center)
            .push(text("Network:").bold())
            .push(Space::new().width(Length::Fill))
            .push(text(label)),
    )
    .width(Length::Fill)
    .into()
}

fn direction_badges_toggle<'a>(show: bool) -> Element<'a, Message> {
    card::simple(
        Row::new()
            .spacing(20)
            .align_y(Alignment::Center)
            .push(text("Show direction badges on transactions:").bold())
            .push(Space::new().width(Length::Fill))
            .push(
                Toggler::new(show)
                    .on_toggle(|new_val| SettingsMessage::ToggleDirectionBadges(new_val).into())
                    .width(50)
                    .style(theme::toggler::orange),
            ),
    )
    .width(Length::Fill)
    .into()
}

fn display_mode_toggle<'a>(current: DisplayMode) -> Element<'a, Message> {
    card::simple(
        Row::new()
            .spacing(20)
            .align_y(Alignment::Center)
            .push(text("Primary balance value:").bold())
            .push(Space::new().width(Length::Fill))
            .push(text("Fiat"))
            .push(
                Toggler::new(matches!(current, DisplayMode::BitcoinNative))
                    .on_toggle(|_| Message::FlipDisplayMode)
                    .width(50)
                    .style(theme::toggler::orange),
            )
            .push(text("Bitcoin")),
    )
    .width(Length::Fill)
    .into()
}

pub fn bitcoin_display_unit<'a>(new_unit_setting: &'a UnitSetting) -> Element<'a, Message> {
    card::simple(
        Row::new()
            .spacing(20)
            .align_y(Alignment::Center)
            .push(text("Bitcoin display unit:").bold())
            .push(Space::new().width(Length::Fill))
            .push(text("BTC"))
            .push(
                Toggler::new(matches!(
                    new_unit_setting.display_unit,
                    BitcoinDisplayUnit::Sats
                ))
                .on_toggle(|is_sats| {
                    SettingsMessage::DisplayUnitChanged(if is_sats {
                        BitcoinDisplayUnit::Sats
                    } else {
                        BitcoinDisplayUnit::BTC
                    })
                    .into()
                })
                .width(50)
                .style(theme::toggler::orange),
            )
            .push(text("Sats")),
    )
    .width(Length::Fill)
    .into()
}

pub fn fiat_price<'a>(
    new_price_setting: &'a PriceSetting,
    currencies_list: &'a [Currency],
) -> Element<'a, Message> {
    card::simple(
        Column::new()
            .spacing(20)
            .push(
                Row::new()
                    .spacing(10)
                    .align_y(Alignment::Center)
                    .push(text("Fiat price:").bold())
                    .push(tooltip_custom(
                        "Fiat price data is provided by third-party services. Availability and accuracy are not guaranteed.",
                        icon::warning_icon().color(color::ORANGE),
                        tooltip::Position::Bottom,
                    ))
                    .push(Space::new().width(Length::Fill))
                    .push(
                        Toggler::new(new_price_setting.is_enabled)
                            .on_toggle(|new_selection| FiatMessage::Enable(new_selection).into())
                            .width(50)
                            .style(theme::toggler::orange),
                    ),
            )
            .push_maybe(
                new_price_setting.is_enabled.then_some(
                    Row::new()
                        .spacing(20)
                        .align_y(Alignment::Center)
                        .push(text("Exchange rate source:").bold())
                        .push(Space::new().width(Length::Fill))
                        .push(
                            pick_list(
                                &ALL_PRICE_SOURCES[..],
                                Some(new_price_setting.source),
                                |source| FiatMessage::SourceEdited(source).into(),
                            )
                            .style(theme::pick_list::primary)
                            .padding(10),
                        ),
                ),
            )
            .push_maybe(
                new_price_setting.is_enabled.then_some(
                    Row::new()
                        .spacing(20)
                        .align_y(Alignment::Center)
                        .push(text("Currency:").bold())
                        .push(Space::new().width(Length::Fill))
                        .push(
                            pick_list(
                                currencies_list,
                                Some(new_price_setting.currency),
                                |currency| FiatMessage::CurrencyEdited(currency).into(),
                            )
                            .style(theme::pick_list::primary)
                            .padding(10),
                        ),
                ),
            )
            .push_maybe(
                new_price_setting
                    .source
                    .attribution()
                    .filter(|_| new_price_setting.is_enabled)
                    .map(|s| {
                        Row::new()
                            .spacing(20)
                            .align_y(Alignment::Center)
                            .push(Space::new().width(Length::Fill))
                            .push(text(s))
                    }),
            ),
    )
    .width(Length::Fill)
    .into()
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
        (true, true) => {
            "Your Wallet Descriptor changed since your last backup — update both copies now."
        }
        (true, false) => "Your password backup's Wallet Descriptor is out of date — update now.",
        (false, true) => "Your phone backup's Wallet Descriptor is out of date — update now.",
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
    use super::*;

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
        recovery_kit_card_state(&backup_overview(Some(st)), false, true, false, true)
    }

    // ---- method_complete truth table (all shapes) ----

    #[test]
    fn method_complete_passkey_needs_descriptor_only() {
        let seed_only = MethodBackup {
            seed: true,
            descriptor: false,
        };
        let desc_only = MethodBackup {
            seed: false,
            descriptor: true,
        };
        assert!(!method_complete(&seed_only, true, true));
        assert!(method_complete(&desc_only, true, true));
        // has_vault is irrelevant for passkey.
        assert!(method_complete(&desc_only, false, true));
    }

    #[test]
    fn method_complete_mnemonic_vaultless_needs_seed_only() {
        let seed_only = MethodBackup {
            seed: true,
            descriptor: false,
        };
        let desc_only = MethodBackup {
            seed: false,
            descriptor: true,
        };
        assert!(method_complete(&seed_only, false, false));
        assert!(!method_complete(&desc_only, false, false));
    }

    #[test]
    fn method_complete_mnemonic_vault_needs_both() {
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
        assert!(method_complete(&both, true, false));
        assert!(!method_complete(&seed_only, true, false));
        assert!(!method_complete(&desc_only, true, false));
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
    fn password_only_seed_only_vaultless_is_backed_up() {
        // A seed-only kit on a vaultless cube is already complete.
        let st = status(true, false, Some("2026-05-01T00:00:00Z"), None);
        let s = recovery_kit_card_state(&backup_overview(Some(&st)), false, false, false, true);
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

    // ---- passkey variant (descriptor-only shape) ----

    #[test]
    fn passkey_descriptor_backed_up() {
        let st = status(false, true, Some("2026-05-01T00:00:00Z"), None);
        let s = recovery_kit_card_state(&backup_overview(Some(&st)), true, true, false, true);
        assert_eq!(s.title, "Wallet Descriptor backed up");
        assert_eq!(s.primary_mode, RecoveryKitMode::Rotate);
    }

    #[test]
    fn passkey_no_kit_is_create() {
        let st = status(false, false, None, None);
        let s = recovery_kit_card_state(&backup_overview(Some(&st)), true, true, false, true);
        assert_eq!(s.title, "Back up your Wallet Descriptor");
        assert_eq!(s.primary_mode, RecoveryKitMode::Create);
    }

    // ---- not-loaded-yet (status None) preserves prior copy ----

    #[test]
    fn not_loaded_mnemonic_shows_loading_or_signin() {
        let loading = recovery_kit_card_state(&backup_overview(None), false, true, true, false);
        assert_eq!(loading.title, "Cube Recovery Kit");
        assert!(loading.subtitle.contains("Checking"));
        let idle = recovery_kit_card_state(&backup_overview(None), false, true, false, false);
        assert!(idle.subtitle.contains("Sign in"));
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
}
