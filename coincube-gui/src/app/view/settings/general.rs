use iced::widget::{pick_list, Column, Row, Space, Toggler};
use iced::{Alignment, Length};

use coincube_ui::component::text::*;
use coincube_ui::component::{button, card};
use coincube_ui::theme;
use coincube_ui::widget::{ColumnExt, Element};

use crate::app::cache;
use crate::app::menu::Menu;
use crate::app::settings::display::DisplayMode;
use crate::app::settings::fiat::PriceSetting;
use crate::app::settings::unit::{BitcoinDisplayUnit, UnitSetting};
use crate::app::state::settings::recovery_kit::RecoveryKit;
use crate::app::view::dashboard;
use crate::app::view::message::*;
use crate::services::coincube::RecoveryKitStatus;
use crate::services::fiat::{Currency, ALL_PRICE_SOURCES};

#[allow(clippy::too_many_arguments)]
pub fn general_section<'a>(
    menu: &'a Menu,
    cache: &'a cache::Cache,
    new_price_setting: &'a PriceSetting,
    new_unit_setting: &'a UnitSetting,
    currencies_list: &'a [Currency],
    developer_mode: bool,
    show_direction_badges: bool,
    backup_state: &'a crate::app::state::settings::general::BackupSeedState,
    backup_pin: &'a crate::pin_input::PinInput,
    backup_mnemonic: Option<&'a [String]>,
    recovery_kit: Option<&'a RecoveryKit>,
) -> Element<'a, Message> {
    use crate::app::state::settings::general::BackupSeedState;

    // When the backup flow is active, take over the entire settings page
    // with the wizard view. This matches the UX the old Liquid Settings
    // backup used and keeps the multi-step flow focused.
    if !matches!(backup_state, BackupSeedState::None) {
        if let Some(wizard) = super::backup::dispatch(backup_state, backup_pin, backup_mnemonic) {
            return dashboard(menu, cache, Column::new().spacing(20).push(wizard));
        }
    }

    // Normal settings rendering.
    let mut col = Column::new()
        .spacing(20)
        .push(super::header("General", SettingsMessage::GeneralSection))
        .push(network_row(cache.network))
        .push(bitcoin_display_unit(new_unit_setting))
        .push(display_mode_toggle(cache.display_mode))
        .push(direction_badges_toggle(show_direction_badges))
        .push(fiat_price(new_price_setting, currencies_list))
        .push(backup_master_seed_card(cache.current_cube_backed_up));

    // Connect-hosted Recovery Kit card. Render only when the outer
    // SettingsState had a `RecoveryKit` on hand — i.e. when the
    // downcasting wrapper invoked `view_with_recovery_kit`. Falling
    // back to no-card when `None` keeps the trait-based `view`
    // callers harmless.
    if let Some(rk) = recovery_kit {
        // W12 drift: compute on the fly by comparing the cached live
        // fingerprint (refreshed every App tick) with the last-backed-up
        // fingerprint (persisted on `CubeSettings`). Only meaningful
        // when a descriptor has actually been backed up — otherwise
        // the card's "incomplete" copy already prompts the user.
        let server_has_descriptor = rk
            .status
            .as_ref()
            .map(|s| s.has_encrypted_wallet_descriptor)
            .unwrap_or(false);
        let drift = descriptor_drift(
            server_has_descriptor,
            cache.current_descriptor_fingerprint.as_deref(),
            cache
                .recovery_kit_last_backed_up_descriptor_fingerprint
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

    if developer_mode {
        col = col.push(toast_testing());
    }

    dashboard(menu, cache, col)
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
            "Write down your 12-word recovery phrase as a backup. This is the only way to recover your Cube if you forget your PIN.",
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
    drift: bool,
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

    let (title, subtitle, primary_label, primary_mode) = if is_passkey {
        // Passkey variant — descriptor-only. Two states.
        match status {
            Some(s) if s.has_encrypted_wallet_descriptor => (
                "Wallet Descriptor backed up",
                format!("Last updated {}.", s.updated_at.as_deref().unwrap_or("—")),
                "Update",
                RecoveryKitMode::Rotate,
            ),
            _ => (
                "Back up your Wallet Descriptor",
                "Your Master Seed Phrase is protected by your passkey and isn't included \
                 in the Recovery Kit — we back up the Wallet Descriptor only."
                    .to_string(),
                "Create Recovery Kit",
                RecoveryKitMode::Create,
            ),
        }
    } else {
        // Mnemonic variant — full four-state matrix per plan §6.3.
        match status {
            Some(s) if !s.has_recovery_kit => (
                "Back up your Cube Recovery Kit",
                "Back up your Master Seed Phrase and Wallet Descriptor to your Connect \
                 account so you can restore your Cube if you lose this device."
                    .to_string(),
                "Create Recovery Kit",
                RecoveryKitMode::Create,
            ),
            Some(s) if s.has_encrypted_seed && !s.has_encrypted_wallet_descriptor && has_vault => (
                "Finish backing up your Recovery Kit",
                "Your Master Seed Phrase is backed up, but your Wallet Descriptor isn't."
                    .to_string(),
                "Add Wallet Descriptor",
                RecoveryKitMode::AddDescriptor,
            ),
            Some(s) if !s.has_encrypted_seed && s.has_encrypted_wallet_descriptor => (
                "Finish backing up your Recovery Kit",
                "Your Wallet Descriptor is backed up, but your Master Seed Phrase isn't."
                    .to_string(),
                "Add Master Seed Phrase",
                RecoveryKitMode::AddSeed,
            ),
            Some(s) => (
                "Recovery Kit backed up",
                format!("Last updated {}.", s.updated_at.as_deref().unwrap_or("—")),
                "Update",
                RecoveryKitMode::Rotate,
            ),
            None => (
                "Cube Recovery Kit",
                if loading {
                    "Checking your Connect account…".to_string()
                } else {
                    "Sign in to Connect to back up your Cube Recovery Kit.".to_string()
                },
                "Create Recovery Kit",
                RecoveryKitMode::Create,
            ),
        }
    };

    // Drift overrides the "complete" state: primary CTA becomes
    // "Update now" and the subtitle swaps to the drift warning.
    let (subtitle, primary_label, primary_mode) = if drift {
        (
            "Your Wallet Descriptor changed since your last backup — update now.".to_string(),
            "Update now",
            RecoveryKitMode::Rotate,
        )
    } else {
        (subtitle, primary_label, primary_mode)
    };

    // Render with Remove button when a kit exists.
    let has_kit = status.map(|s| s.has_recovery_kit).unwrap_or(false);
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

    let mut body = Column::new()
        .spacing(4)
        .width(Length::Fill)
        .push(text(title).bold())
        .push(text(subtitle).size(14));
    if drift {
        body = body.push(
            text("⚠ Descriptor out of sync with your Connect backup.")
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

fn toast_testing<'a>() -> Element<'a, Message> {
    let btn = |label: &'static str, level: log::Level| {
        iced::widget::Button::new(text(label).bold())
            .padding([8, 16])
            .style(theme::button::secondary)
            .on_press(SettingsMessage::TestToast(level).into())
    };

    card::simple(
        Column::new()
            .spacing(15)
            .push(text("Toast Testing").bold())
            .push(
                Row::new()
                    .spacing(10)
                    .push(btn("Error", log::Level::Error))
                    .push(btn("Warn", log::Level::Warn))
                    .push(btn("Info", log::Level::Info))
                    .push(btn("Debug", log::Level::Debug))
                    .push(btn("Trace", log::Level::Trace)),
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

/// Decide whether the Recovery-Kit card should flag "descriptor
/// drift". Split out of `general_section` so the branch table is
/// testable without standing up a full `Cache`.
///
/// - `server_has_descriptor`: the Connect-side `RecoveryKitStatus`
///   reports a descriptor half is backed up.
/// - `live`: SHA-256 of what the live wallet would currently upload
///   (`None` when no Vault is loaded).
/// - `cached`: SHA-256 of what this device last uploaded
///   (`None` when the local cache was cleared, the kit was made from
///   a different install, or the backup happened before the cache
///   field existed).
fn descriptor_drift(server_has_descriptor: bool, live: Option<&str>, cached: Option<&str>) -> bool {
    if !server_has_descriptor {
        return false;
    }
    match (live, cached) {
        // Both present — direct comparison.
        (Some(live), Some(cached)) => live != cached,
        // Live wallet descriptor loaded but no local fingerprint to
        // compare against (cache cleared, restored from a different
        // install, descriptor uploaded from another device). The
        // server says a descriptor is backed up; we can't verify it
        // matches what this device would now upload, so nudge the
        // user to resync.
        (Some(_), None) => true,
        // Any case where the live fingerprint isn't computable (no
        // Vault loaded yet) can't produce a meaningful comparison;
        // avoid a false positive.
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_drift_when_server_has_no_descriptor() {
        // Card's "incomplete" copy already covers this case; drift
        // must not double up the signal.
        assert!(!descriptor_drift(false, Some("a"), Some("b")));
        assert!(!descriptor_drift(false, Some("a"), None));
        assert!(!descriptor_drift(false, None, Some("b")));
    }

    #[test]
    fn drift_when_live_and_cached_differ() {
        assert!(descriptor_drift(true, Some("a"), Some("b")));
    }

    #[test]
    fn no_drift_when_live_and_cached_match() {
        assert!(!descriptor_drift(true, Some("a"), Some("a")));
    }

    #[test]
    fn drift_when_cached_missing_but_live_present() {
        // Regression: previously swallowed by `_ => false`. Server
        // reports a descriptor, live wallet is loaded, but we have
        // no local cache to compare against — the conservative call
        // is to flag drift so the user resyncs.
        assert!(descriptor_drift(true, Some("a"), None));
    }

    #[test]
    fn no_drift_when_live_missing() {
        // No Vault loaded yet — can't compute a comparison.
        // `server_has_descriptor` being true here means another
        // device backed up the descriptor; we can't usefully flag
        // drift until this device has a wallet to diff against.
        assert!(!descriptor_drift(true, None, Some("b")));
        assert!(!descriptor_drift(true, None, None));
    }
}
