//! Firmware advisories for signing devices.
//!
//! An *advisory* is a security notice about a device's firmware that we
//! surface next to the device wherever it appears — the vault device list,
//! the installer key picker, the xpub file-import flow. It is purely
//! informational:
//!
//! * **It never gates anything.** Connecting, registering, signing and
//!   importing behave exactly as they do without an advisory. Everything in
//!   this module is a pure, total function over data we already read from the
//!   device; no I/O, no `Result`, nothing that can fail a device path.
//! * **An unreadable firmware version is never a clean bill of health.** A
//!   device that won't say what it is running gets the strongest tier the
//!   advisory has.
//! * **Whether patched firmware ends the matter is the advisory's own call.**
//!   Some flaws firmware cannot undo: a seed *generated* by a broken RNG stays
//!   broken after the update that fixes the RNG, so the Coldcard advisory keeps
//!   speaking to an up-to-date device via [`Advisory::residual`]. Others the
//!   update genuinely closes — the BitBox advisory sets `residual: None` and
//!   disappears from a device on fixed firmware, because a badge that can never
//!   be cleared is a badge nobody reads.
//!
//! The table below is the wire shape a future Connect-served advisory feed
//! would fill in; today it is a `const` with one row per vendor incident.
//!
//! ## Model lines and version strings
//!
//! `async_hwi::parse_version` normalises Coldcard's non-semver suffixes
//! (`X` for the Mk4 Edge branch, `QX` for the Q Edge branch) away before
//! parsing, and a plain `Q` suffix (Coldcard Q mainline, e.g. `1.5.0Q`)
//! fails the semver regex outright — `get_version()` then yields `None`.
//! So the numeric version alone is what we get, and the *major* is what
//! separates the product lines in practice:
//!
//! | Line          | Firmware track |
//! |---------------|----------------|
//! | Coldcard Q    | `1.x`          |
//! | Mk2 / Mk3     | `4.x`          |
//! | Mk4 / Mk5     | `5.x`          |
//! | Edge (Mk4/Q)  | `6.x`          |
//!
//! Each affected range is therefore expressed as a half-open interval whose
//! bounds share a major, which keeps the lines from bleeding into each other.
//! A version we cannot read or cannot compare is treated as affected.
//!
//! The BitBox02 needs none of that: it reports plain semver on a single `9.x`
//! track, so its advisory is one row.

use async_hwi::{DeviceKind, Version};

/// Stable identifier for the July 2026 Coldcard RNG advisory. Doubles as the
/// dismissal key (see [`crate::app::settings::global::GlobalSettings`]) and,
/// once a feed exists, as the feed's row id.
pub const COLDCARD_RNG_2026_07: &str = "CC-2026-07-RNG";

/// Stable identifier for the August 2026 BitBox02 firmware advisory. Same
/// double duty as [`COLDCARD_RNG_2026_07`].
pub const BITBOX_FIRMWARE_2026_08: &str = "BB-2026-08-FIRMWARE";

/// How urgent the advice is for one device.
///
/// Ordered weakest → strongest so `max` picks the stronger of two tiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AdvisoryTier {
    /// Firmware is at or past the fix, or is on a track the advisory does not
    /// name — so there is nothing left to *update*. Whether the advisory still
    /// has something to say here is per-advisory: see [`Advisory::residual`].
    Residual,
    /// Firmware is inside (or below) an affected range — or we could not read
    /// it at all. The device is running something the vendor has since fixed.
    Affected,
}

/// One product line's affected firmware window, half-open: `[introduced, patched)`.
#[derive(Debug, Clone)]
pub struct AffectedRange {
    /// Human name of the product line, e.g. `"Mk4/Mk5"`. Shown in the UI so a
    /// user with several Coldcards can tell which one is being talked about.
    pub line: &'static str,
    /// First affected version, inclusive.
    pub introduced: Version,
    /// First *fixed* version, exclusive. Anything below it on this line is
    /// affected.
    pub patched: Version,
}

/// A security advisory for one device kind.
#[derive(Debug, Clone)]
pub struct Advisory {
    /// Stable id, e.g. [`COLDCARD_RNG_2026_07`].
    pub id: &'static str,
    pub device_kind: DeviceKind,
    /// Affected-version matrix, one row per product line.
    pub affected: &'static [AffectedRange],
    /// Where the guide for this incident lives.
    pub url: &'static str,
    /// Short line shown next to the badge.
    pub headline: &'static str,
    /// Label for the link to [`Advisory::url`]. Named per advisory because
    /// what the guide asks of the user differs: the Coldcard incident needs a
    /// key rotated, the BitBox one needs a firmware update and nothing more.
    pub guide_label: &'static str,
    /// Body for [`AdvisoryTier::Affected`].
    pub affected_copy: &'static str,
    /// Body for [`AdvisoryTier::Residual`], or `None` when firmware at or past
    /// the fix genuinely ends the exposure — the advisory then stops showing
    /// on that device altogether, rather than lingering as a badge with no
    /// remaining instruction behind it.
    pub residual: Option<&'static str>,
    /// Body used where no firmware version is knowable at all — an xpub
    /// parsed out of a file carries no firmware information.
    pub file_import: &'static str,
    /// Copy for the one-time, app-wide incident notice. Addressed to everyone,
    /// because a descriptor key doesn't record which device it came from, so
    /// the notice can't be targeted at the people holding the affected
    /// hardware — it has to let them recognise themselves in it.
    pub notice: &'static str,
}

impl Advisory {
    /// The tiered body copy, or `None` where this advisory has nothing left to
    /// say at `tier` — the one case being [`AdvisoryTier::Residual`] on an
    /// advisory whose fix is complete.
    pub fn body(&self, tier: AdvisoryTier) -> Option<&'static str> {
        match tier {
            AdvisoryTier::Affected => Some(self.affected_copy),
            AdvisoryTier::Residual => self.residual,
        }
    }
}

/// An advisory matched against a specific device.
///
/// Only ever constructed where there is copy to show, so every getter returns
/// something renderable.
#[derive(Debug, Clone)]
pub struct AdvisoryHit {
    pub advisory: &'static Advisory,
    pub tier: AdvisoryTier,
    /// Which product line matched, when the version placed the device on one.
    /// `None` when the version was unreadable or on no named line.
    pub line: Option<&'static str>,
    /// The tier's body copy, resolved at construction.
    copy: &'static str,
}

impl AdvisoryHit {
    pub fn id(&self) -> &'static str {
        self.advisory.id
    }

    pub fn headline(&self) -> &'static str {
        self.advisory.headline
    }

    pub fn body(&self) -> &'static str {
        self.copy
    }

    pub fn guide_label(&self) -> &'static str {
        self.advisory.guide_label
    }

    pub fn url(&self) -> &'static str {
        self.advisory.url
    }
}

const fn v(major: u32, minor: u32, patch: u32) -> Version {
    Version {
        major,
        minor,
        patch,
        prerelease: None,
    }
}

// Firmware windows from Coinkite's advisory of 2026-07-30 (updated 2026-08-01),
// "COLDCARD Security Advisory: seed generation":
//
//   Mk2/Mk3        4.0.1 – 4.1.9 inclusive     fixed in 4.2.0
//   Mk4/Mk5        below 5.6.0                 fixed in 5.6.0
//   Mk4/Mk5 Edge   below 6.6.0X                fixed in 6.6.0X
//   Q              below 1.5.0Q                fixed in 1.5.0Q
//   Q Edge         below 6.6.0QX               fixed in 6.6.0QX
//
// The two Edge tracks share the 6.x numbering once `async_hwi::parse_version`
// has stripped the `X` / `QX` suffix, so they are one row here.
const COLDCARD_RNG_RANGES: &[AffectedRange] = &[
    // Coldcard Q — 1.x track.
    AffectedRange {
        line: "Q",
        introduced: v(1, 0, 0),
        patched: v(1, 5, 0),
    },
    // Mk2 / Mk3 — 4.x track. Coinkite's advisory names 4.0.1 as the first
    // affected release; Block's engineering analysis of the same bug puts it at
    // 4.0.0. We take the wider bound — the only firmware this changes the
    // wording for is 4.0.0 itself, and over-warning is the safe direction.
    AffectedRange {
        line: "Mk2/Mk3",
        introduced: v(4, 0, 0),
        patched: v(4, 2, 0),
    },
    // Mk4 / Mk5 — 5.x track.
    AffectedRange {
        line: "Mk4/Mk5",
        introduced: v(5, 0, 0),
        patched: v(5, 6, 0),
    },
    // Edge (Mk4/Q experimental branch) — 6.x track.
    AffectedRange {
        line: "Edge",
        introduced: v(6, 0, 0),
        patched: v(6, 6, 0),
    },
];

// Copy discipline: verifiable facts from the vendor advisory only, the multisig
// reassurance stated once, and a concrete instruction. No speculation about
// what an attacker could do, and no "you are safe" on any tier.
//
// Two facts from the advisory materially change what a user has to do, so they
// appear on every surface: seeds created with at least 50 independent private
// dice rolls are not at risk from this issue, and a strong unique BIP-39
// passphrase is an independent barrier — but it does not repair the seed.
/// Coldcard [`AdvisoryTier::Residual`] copy. Pulled out of the struct literal
/// only so the `Option` around it stays readable.
const COLDCARD_RNG_RESIDUAL: &str = "This Coldcard reports firmware outside every range named by \
     Coinkite's advisory of 30 July 2026. That is not an all-clear: updating firmware does not \
     repair a seed that already exists, so if this device's seed was created on affected \
     firmware, generate a new seed on the device and rotate this key out of your Cube. A single \
     key in a multisig Cube cannot move funds on its own, so you have time to do this properly. \
     Seeds created with 50 or more of your own dice rolls are not affected.";

const COLDCARD_RNG: Advisory = Advisory {
    id: COLDCARD_RNG_2026_07,
    device_kind: DeviceKind::Coldcard,
    affected: COLDCARD_RNG_RANGES,
    url: "https://coincube.io/advisories/2026-07-coldcard-rng",
    headline: "Coldcard firmware advisory",
    guide_label: "Read the rotation guide",
    // Patched firmware never clears this one: the flaw was in seed generation,
    // and an update cannot regenerate a seed that already exists.
    residual: Some(COLDCARD_RNG_RESIDUAL),
    affected_copy: "This Coldcard reports firmware covered by Coinkite's advisory of \
                        30 July 2026: seeds generated on it may have far less randomness \
                        than intended. Update the device firmware, then generate a new seed \
                        on the device and rotate this key out of your Cube. A single key in \
                        a multisig Cube cannot move funds on its own, so you have time to do \
                        this properly. Seeds created with 50 or more of your own dice rolls \
                        are not affected.",
    file_import: "This key was exported from a Coldcard. A file carries no firmware \
                  information, so Coincube cannot tell which firmware generated the seed \
                  behind it. If that seed was created on firmware covered by Coinkite's \
                  advisory of 30 July 2026 — and without 50 or more of your own dice rolls \
                  — generate a new seed on the device and rotate this key out of your Cube. \
                  A single key in a multisig Cube cannot move funds on its own, so you have \
                  time to do this properly.",
    notice: "On 30 July 2026 Coinkite published a security advisory: on affected Coldcard \
             firmware, seeds were generated with far less randomness than intended. Seeds \
             made on Mk2 and Mk3 firmware 4.0.1–4.1.9 are the worst case; Mk4, Mk5 and Q \
             seeds made before the fix have roughly 72 bits of entropy instead of 128. If \
             any key in one of your Cubes lives on a Coldcard, this concerns you.\n\n\
             A single key in a multisig Cube cannot move funds on its own, so you have time \
             to do this properly. When you can: update the device firmware, generate a new \
             seed on the device, rotate that key out of your Cube, and move the funds to the \
             new descriptor. Coincube flags every connected Coldcard with the same notice, \
             so you can come back to this from the device list.\n\n\
             Two exceptions from the advisory: a seed created with 50 or more of your own \
             dice rolls is not affected, and a strong unique BIP-39 passphrase is a separate \
             barrier — though it does not repair the seed underneath it. Any single-sig \
             wallet on the same device is not protected by your Cube's multisig and should \
             be dealt with first.\n\n\
             Nothing about how Coincube connects to, signs with, or imports from a Coldcard \
             has changed.",
};

// BitBox published firmware 9.26.5 ("Dixence") on 17 August 2026, closing three
// issues:
//
//   * a bootloader flaw (firmware through 9.26.1) that let an attacker who had
//     already phished the user into running a fake BitBoxApp install malicious
//     firmware on a genuine device — fixed back in 9.26.2 ("Oeschinen"); the
//     BitBox02 Nova was never affected;
//   * a memory-corruption flaw (through 9.26.4) allowing arbitrary code
//     execution on an *uninitialised* BitBox02 Multi attached to a malicious
//     host — the Bitcoin-only edition is not affected;
//   * a Silent Payments implementation flaw (9.21.0 through 9.26.4) that could
//     lock funds to an unintended address.
//
// BitBox reports no evidence any of them was exploited, and states plainly that
// existing wallet seeds are unaffected.
//
// One range, not three. We cannot tell a Multi from a Bitcoin-only edition or a
// Nova from a BitBox02 through `async_hwi` — every one of them arrives as
// `DeviceKind::BitBox02` — so splitting the window per issue would mean
// guessing which issues reach the device in front of the user. Anything below
// 9.26.5 gets one instruction that settles all three.
const BITBOX_FIRMWARE_RANGES: &[AffectedRange] = &[AffectedRange {
    line: "BitBox02",
    // Below the oldest firmware Coincube works with at all (9.15.0), so this
    // bound is really "every version we can encounter".
    introduced: v(9, 0, 0),
    patched: v(9, 26, 5),
}];

// Copy discipline as for the Coldcard row: vendor-verifiable facts, one
// instruction. The instruction is *update the firmware* and stops there —
// BitBox states that existing wallet seeds are unaffected, so telling a user to
// rotate a key would invent work that the vendor's own advisory does not ask
// for, and would drown the one action that does matter.
const BITBOX_FIRMWARE: Advisory = Advisory {
    id: BITBOX_FIRMWARE_2026_08,
    device_kind: DeviceKind::BitBox02,
    affected: BITBOX_FIRMWARE_RANGES,
    url: "https://coincube.io/advisories/2026-08-bitbox-firmware",
    headline: "BitBox02 firmware advisory",
    guide_label: "Read the update guide",
    // Unlike the Coldcard incident, the update is the whole remedy: nothing
    // here reaches a seed, so a device on 9.26.5 or later has nothing left to
    // act on and shows no advisory at all.
    residual: None,
    affected_copy: "This BitBox02 reports firmware below 9.26.5, the release BitBox published \
                    on 17 August 2026. It closes a memory-corruption issue that could run \
                    attacker code on an uninitialised device, a Silent Payments issue that \
                    could lock funds to an unintended address, and — for devices still below \
                    9.26.2 — a bootloader issue that could be used to install malicious \
                    firmware after a successful phishing attack. None of this weakens the seed \
                    already on the device, so for almost everyone the fix is simply to update \
                    the firmware from the BitBoxApp — this notice then goes away. The exception \
                    is a device that was actually attacked rather than merely vulnerable, where \
                    the key should be replaced too; the guide covers when that applies. \
                    Download the app only from bitbox.swiss/download or the link inside the app \
                    you already have.",
    // Unreachable today: `export::import_xpub` only recognises Coldcard's two
    // export formats, so no file import ever names a BitBox. Written anyway so
    // the row is complete the day one is added.
    file_import: "This key was exported from a BitBox02. A file carries no firmware \
                  information, so Coincube cannot tell which version the device is running. \
                  If it is below 9.26.5 — the release BitBox published on 17 August 2026 — \
                  update it from the BitBoxApp. None of the issues that release fixes weakens \
                  the seed the device generated, so updating is normally the whole remedy; a \
                  device that was actually attacked is the exception the guide covers.",
    notice: "On 17 August 2026 BitBox released firmware 9.26.5 for the BitBox02, fixing three \
             security issues: a memory-corruption issue that could run attacker code on a \
             device that has not been set up yet, a Silent Payments issue that could lock \
             funds to an unintended address, and — for devices below 9.26.2 — a bootloader \
             issue that could be used to install malicious firmware on a genuine device after \
             a successful phishing attack. BitBox has no evidence any of them was exploited. \
             If any key in one of your Cubes lives on a BitBox02, this concerns you.\n\n\
             None of this weakens the seed already on your device, so unlike the Coldcard \
             advisory there is normally nothing to rotate and no funds to move: plug the \
             device in, open the BitBoxApp, and update it from Manage device. Coincube flags \
             every connected BitBox02 below 9.26.5 with the same notice, and the flag clears \
             once the device reports the new firmware.\n\n\
             The exception is a device that was actually attacked rather than merely \
             vulnerable — phished into counterfeit firmware, or set up while attached to a \
             machine you should not have trusted. Code running on a BitBox can reach the seed \
             it holds, and no later update undoes that, so such a device needs its key \
             replaced as well. The guide covers how to tell and what to do.\n\n\
             Get the BitBoxApp only from bitbox.swiss/download or from the update link inside \
             the app you already have. Announcements like this one attract phishing, and \
             BitBox will never ask you for your recovery words — no genuine update ever needs \
             them typed anywhere but on the device itself.\n\n\
             Nothing about how Coincube connects to, signs with, or imports from a BitBox02 \
             has changed.",
};

/// Every advisory we ship, in the order their one-time notices are owed. The
/// shape is the feed's.
pub const ADVISORIES: &[Advisory] = &[COLDCARD_RNG, BITBOX_FIRMWARE];

/// The advisory covering `kind`, if any.
pub fn advisory_for_kind(kind: &DeviceKind) -> Option<&'static Advisory> {
    ADVISORIES.iter().find(|a| a.device_kind == *kind)
}

/// Match a connected device against the advisory table.
///
/// Total and pure: every input produces an answer, and a device kind we have
/// no advisory for yields `None`. A `None` or uncomparable version resolves to
/// the strongest tier rather than to "no advisory" — an unknown firmware is
/// not a clean bill of health.
///
/// The one way a *matched* device comes back `None` is [`AdvisoryTier::Residual`]
/// on an advisory whose fix is complete (`residual: None`): the update closed
/// it, so there is nothing to show.
pub fn evaluate(kind: &DeviceKind, version: Option<&Version>) -> Option<AdvisoryHit> {
    let advisory = advisory_for_kind(kind)?;
    let (tier, line) = match version {
        // Firmware unreadable (a locked device, a `get_version` failure, or a
        // version string async_hwi can't parse — Coldcard Q mainline `1.5.0Q`
        // among them). Assume the worst.
        None => (AdvisoryTier::Affected, None),
        Some(version) => match affected_range(advisory, version) {
            Some(range) => (AdvisoryTier::Affected, Some(range.line)),
            None => (AdvisoryTier::Residual, line_for(advisory, version)),
        },
    };
    Some(AdvisoryHit {
        advisory,
        tier,
        line,
        copy: advisory.body(tier)?,
    })
}

/// The advisory copy for a key that arrived from a file rather than from a
/// connected device — same advisory, strongest tier, no firmware to name.
pub fn evaluate_file_import(kind: &DeviceKind) -> Option<&'static Advisory> {
    advisory_for_kind(kind)
}

/// The affected range `version` falls into, if any.
fn affected_range(
    advisory: &'static Advisory,
    version: &Version,
) -> Option<&'static AffectedRange> {
    advisory
        .affected
        .iter()
        .find(|range| at_least(version, &range.introduced) && below(version, &range.patched))
}

/// The product line `version` belongs to, whether or not it is affected —
/// a patched 5.6.1 is still an "Mk4/Mk5". Matching on the major is what
/// `AffectedRange` already encodes; see the module docs.
fn line_for(advisory: &'static Advisory, version: &Version) -> Option<&'static str> {
    advisory
        .affected
        .iter()
        .find(|range| range.patched.major == version.major)
        .map(|range| range.line)
}

/// `version >= bound`, with an uncomparable pair (two prerelease strings that
/// `async_hwi::Version` declines to order) counting as "yes" — every
/// indeterminate comparison must widen the affected window, never narrow it.
fn at_least(version: &Version, bound: &Version) -> bool {
    match version.partial_cmp(bound) {
        Some(ordering) => ordering.is_ge(),
        None => true,
    }
}

/// `version < bound`, indeterminate counting as "yes" — see [`at_least`].
fn below(version: &Version, bound: &Version) -> bool {
    match version.partial_cmp(bound) {
        Some(ordering) => ordering.is_lt(),
        None => true,
    }
}

/// Which advisory badges the user has collapsed, keyed by device fingerprint
/// and advisory id.
///
/// Dismissing only collapses the detail panel; the badge itself stays on the
/// device row forever, so a dismissed advisory is still visible at a glance.
///
/// The set lives in `global_settings.json` (app-wide, like the theme and
/// window size — an advisory is about a *device*, not about one Cube), and is
/// mirrored here in-process because the device-row renderers are leaf view
/// functions that receive a single [`crate::hw::HardwareWallet`] and have no
/// route to the settings file. Uninitialised — in tests, and before
/// [`init`] runs — nothing reads as dismissed, so the advisory shows.
pub mod dismissals {
    use coincube_core::miniscript::bitcoin::bip32::Fingerprint;
    use std::collections::HashSet;
    use std::path::PathBuf;
    use std::sync::{OnceLock, RwLock};

    use crate::app::settings::global::GlobalSettings;

    struct Store {
        /// Path to `global_settings.json`; `None` until [`init`] runs.
        path: Option<PathBuf>,
        keys: HashSet<String>,
    }

    fn store() -> &'static RwLock<Store> {
        static STORE: OnceLock<RwLock<Store>> = OnceLock::new();
        STORE.get_or_init(|| {
            RwLock::new(Store {
                path: None,
                keys: HashSet::new(),
            })
        })
    }

    /// Load the persisted dismissals. Called once at startup with the path to
    /// `global_settings.json`.
    pub fn init(path: PathBuf) {
        let keys = GlobalSettings::load_dismissed_hw_advisories(&path)
            .into_iter()
            .collect();
        let mut store = store().write().expect("poisoned");
        store.keys = keys;
        store.path = Some(path);
    }

    fn key(fingerprint: &Fingerprint, advisory_id: &str) -> String {
        format!("{fingerprint}:{advisory_id}")
    }

    pub fn is_dismissed(fingerprint: &Fingerprint, advisory_id: &str) -> bool {
        store()
            .read()
            .expect("poisoned")
            .keys
            .contains(&key(fingerprint, advisory_id))
    }

    /// Record a dismissal in memory and persist it. A failed write is logged
    /// and otherwise ignored: the worst case is the panel coming back on the
    /// next launch, which is the safe direction for a security advisory.
    pub fn dismiss(fingerprint: &Fingerprint, advisory_id: &str) {
        let key = key(fingerprint, advisory_id);
        let path = {
            let mut store = store().write().expect("poisoned");
            store.keys.insert(key.clone());
            store.path.clone()
        };
        match path {
            Some(path) => {
                if let Err(e) = GlobalSettings::dismiss_hw_advisory(&path, key) {
                    tracing::error!("Failed to persist advisory dismissal: {e}");
                }
            }
            None => tracing::warn!("Advisory dismissal not persisted: store not initialised"),
        }
    }
}

/// Rendering glue shared by every surface that lists signing devices — the
/// vault device lists and the installer's two key pickers. Each of those uses
/// its own message type, so the section is generic over it; the copy, the
/// dismissal lookup and the badge chrome are decided here, once.
pub mod view {
    use coincube_core::miniscript::bitcoin::bip32::Fingerprint;
    use coincube_ui::{component::hw, widget::Element};

    use super::{dismissals, evaluate, AdvisoryHit};
    use crate::hw::HardwareWallet;

    const BADGE: &str = "Firmware advisory";
    const BADGE_TIP: &str = "This device is covered by a firmware advisory. It stays fully \
                             usable — expand the notice for what to do.";

    /// The advisory covering a connected device, if any.
    ///
    /// A locked device hasn't reported its firmware yet, which resolves to the
    /// strongest tier rather than to silence — same as a device whose
    /// `get_version` failed.
    pub fn hit(hw: &HardwareWallet) -> Option<AdvisoryHit> {
        let version = match hw {
            HardwareWallet::Supported { version, .. }
            | HardwareWallet::Unsupported { version, .. } => version.as_ref(),
            HardwareWallet::Locked { .. } => None,
        };
        evaluate(hw.kind(), version)
    }

    /// Badge plus collapsible detail panel, to be rendered under the device
    /// row. `on_dismiss` is what offers the dismiss control; pass `None` on
    /// surfaces that don't own one.
    pub fn section<'a, T: 'a + Clone>(
        hit: &AdvisoryHit,
        fingerprint: Option<Fingerprint>,
        on_guide: T,
        on_dismiss: Option<T>,
    ) -> Element<'a, T> {
        let dismissed =
            fingerprint.is_some_and(|fingerprint| dismissals::is_dismissed(&fingerprint, hit.id()));
        hw::advisory_section(
            BADGE,
            BADGE_TIP,
            hit.headline(),
            hit.line,
            hit.body(),
            hit.guide_label(),
            Some(on_guide),
            on_dismiss,
            dismissed,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tier(version: Option<Version>) -> AdvisoryTier {
        evaluate(&DeviceKind::Coldcard, version.as_ref())
            .expect("coldcard is in the table")
            .tier
    }

    fn line(version: Version) -> Option<&'static str> {
        evaluate(&DeviceKind::Coldcard, Some(&version))
            .expect("coldcard is in the table")
            .line
    }

    fn parse(s: &str) -> Version {
        async_hwi::parse_version(s).expect("parseable")
    }

    fn bitbox(version: Option<Version>) -> Option<AdvisoryHit> {
        evaluate(&DeviceKind::BitBox02, version.as_ref())
    }

    #[test]
    fn kinds_with_no_advisory_are_never_flagged() {
        for kind in [
            DeviceKind::Ledger,
            DeviceKind::LedgerSimulator,
            DeviceKind::Jade,
            DeviceKind::Specter,
            DeviceKind::SpecterSimulator,
        ] {
            assert!(evaluate(&kind, None).is_none(), "{} should be clean", kind);
            assert!(evaluate(&kind, Some(&v(1, 0, 0))).is_none());
        }
    }

    #[test]
    fn unknown_version_takes_the_strongest_tier() {
        assert_eq!(tier(None), AdvisoryTier::Affected);
    }

    #[test]
    fn unparseable_version_strings_yield_no_version_at_all() {
        // Coldcard Q mainline ("1.5.0Q") is not semver and async_hwi's parser
        // only strips the Edge `X`/`QX` suffixes, so the device surfaces with
        // `version: None` — which is the strongest tier, above.
        assert!(async_hwi::parse_version("1.5.0Q").is_err());
        assert!(async_hwi::parse_version("garbage").is_err());
    }

    #[test]
    fn mk2_mk3_boundary() {
        assert_eq!(tier(Some(v(4, 0, 0))), AdvisoryTier::Affected);
        assert_eq!(tier(Some(v(4, 1, 9))), AdvisoryTier::Affected);
        assert_eq!(tier(Some(v(4, 2, 0))), AdvisoryTier::Residual);
    }

    #[test]
    fn mk4_mk5_boundary() {
        assert_eq!(tier(Some(v(5, 5, 9))), AdvisoryTier::Affected);
        assert_eq!(tier(Some(v(5, 6, 0))), AdvisoryTier::Residual);
        assert_eq!(tier(Some(v(5, 6, 1))), AdvisoryTier::Residual);
    }

    #[test]
    fn q_boundary() {
        assert_eq!(tier(Some(v(1, 4, 9))), AdvisoryTier::Affected);
        assert_eq!(tier(Some(v(1, 5, 0))), AdvisoryTier::Residual);
    }

    #[test]
    fn edge_boundary_from_device_strings() {
        // Mk4 Edge and Q Edge suffixes are stripped by the parser.
        assert_eq!(tier(Some(parse("6.5.9X"))), AdvisoryTier::Affected);
        assert_eq!(tier(Some(parse("6.6.0X"))), AdvisoryTier::Residual);
        assert_eq!(tier(Some(parse("6.6.0QX"))), AdvisoryTier::Residual);
        assert_eq!(tier(Some(parse("6.2.1"))), AdvisoryTier::Affected);
    }

    #[test]
    fn a_version_below_every_named_range_is_still_flagged() {
        // 3.x predates the named windows: not "affected", but never cleared.
        assert_eq!(tier(Some(v(3, 0, 0))), AdvisoryTier::Residual);
    }

    #[test]
    fn product_line_is_named_on_both_tiers() {
        assert_eq!(line(v(5, 5, 0)), Some("Mk4/Mk5"));
        assert_eq!(line(v(5, 6, 0)), Some("Mk4/Mk5"));
        assert_eq!(line(v(4, 1, 0)), Some("Mk2/Mk3"));
        assert_eq!(line(v(1, 5, 1)), Some("Q"));
        assert_eq!(line(parse("6.6.0X")), Some("Edge"));
        // A track the advisory does not name at all.
        assert_eq!(line(v(3, 0, 0)), None);
    }

    #[test]
    fn prerelease_firmware_is_placed_by_its_numbers() {
        // No shipped bound carries a prerelease, so a prerelease version still
        // orders determinately against every range: 5.5.0-rc1 sits inside the
        // Mk4/Mk5 window, 5.9.0-rc1 sits past it.
        let pre = |minor| Version {
            major: 5,
            minor,
            patch: 0,
            prerelease: Some("rc1".to_string()),
        };
        assert_eq!(tier(Some(pre(5))), AdvisoryTier::Affected);
        assert_eq!(tier(Some(pre(9))), AdvisoryTier::Residual);
    }

    #[test]
    fn uncomparable_versions_widen_the_affected_window() {
        // `async_hwi::Version` refuses to order two prereleases against each
        // other. A future advisory row with a prerelease bound must not let
        // that indeterminacy clear a device, so both comparison helpers say
        // "yes" when they cannot tell.
        let pre = |s: &str| Version {
            major: 5,
            minor: 0,
            patch: 0,
            prerelease: Some(s.to_string()),
        };
        assert!(pre("rc2").partial_cmp(&pre("rc1")).is_none());
        assert!(at_least(&pre("rc2"), &pre("rc1")));
        assert!(below(&pre("rc2"), &pre("rc1")));
    }

    #[test]
    fn every_shipped_advisory_is_renderable() {
        for advisory in ADVISORIES {
            assert!(!advisory.id.is_empty());
            assert!(advisory.url.starts_with("https://"));
            assert!(!advisory.headline.is_empty());
            assert!(!advisory.guide_label.is_empty());
            assert!(!advisory.notice.is_empty());
            assert!(!advisory.file_import.is_empty());
            // Whatever copy a tier offers at all must be non-empty: a hit is
            // only built where `body` returned `Some`, so an empty string would
            // render an advisory panel with nothing in it.
            for tier in [AdvisoryTier::Affected, AdvisoryTier::Residual] {
                assert!(advisory.body(tier).is_none_or(|copy| !copy.is_empty()));
            }
            // Every advisory says something on the strongest tier — otherwise
            // it could never fire.
            assert!(advisory.body(AdvisoryTier::Affected).is_some());
        }
    }

    #[test]
    fn advisory_ids_are_unique() {
        // Ids key both dismissals and the seen-notice list, so a collision
        // would silence one advisory the first time the other is acknowledged.
        for (i, a) in ADVISORIES.iter().enumerate() {
            for b in &ADVISORIES[i + 1..] {
                assert_ne!(a.id, b.id);
                // One advisory per kind: `advisory_for_kind` takes the first
                // match, so a second row for the same device would be dead.
                assert_ne!(a.device_kind, b.device_kind);
            }
        }
    }

    #[test]
    fn coldcard_hits_carry_the_coldcard_id() {
        let hit = evaluate(&DeviceKind::Coldcard, None).expect("hit");
        assert_eq!(hit.id(), COLDCARD_RNG_2026_07);
    }

    #[test]
    fn file_import_copy_is_offered_only_for_kinds_we_have_an_advisory_for() {
        for kind in [DeviceKind::Coldcard, DeviceKind::BitBox02] {
            let advisory = evaluate_file_import(&kind).expect("advisory");
            assert!(!advisory.file_import.is_empty());
        }
        assert!(evaluate_file_import(&DeviceKind::Ledger).is_none());
    }

    /// The BitBox row. Its defining difference from the Coldcard row: the
    /// firmware update *is* the whole remedy, so a device past the fix stops
    /// being flagged instead of dropping to a weaker tier.
    mod bitbox {
        use super::*;

        #[test]
        fn firmware_below_the_fix_is_flagged() {
            for version in [v(9, 15, 0), v(9, 21, 0), v(9, 26, 1), v(9, 26, 4)] {
                let hit = bitbox(Some(version.clone())).expect("flagged");
                assert_eq!(hit.tier, AdvisoryTier::Affected);
                assert_eq!(hit.id(), BITBOX_FIRMWARE_2026_08);
                assert_eq!(hit.line, Some("BitBox02"));
            }
        }

        #[test]
        fn the_fix_clears_the_advisory_outright() {
            // Unlike the Coldcard rows, which never reach a cleared state.
            for version in [v(9, 26, 5), v(9, 26, 6), v(9, 27, 0), v(10, 0, 0)] {
                assert!(
                    bitbox(Some(version.clone())).is_none(),
                    "{:?} should be clear",
                    version
                );
            }
        }

        #[test]
        fn unreadable_firmware_is_still_flagged() {
            // A BitBox02 that hasn't been unlocked yet reports no version. It
            // gets the notice until it says otherwise — same safe direction as
            // everywhere else, and self-correcting once the device unlocks.
            let hit = bitbox(None).expect("flagged");
            assert_eq!(hit.tier, AdvisoryTier::Affected);
            assert_eq!(hit.line, None);
        }

        #[test]
        fn the_copy_asks_for_an_update_and_not_a_rotation() {
            let hit = bitbox(Some(v(9, 26, 4))).expect("flagged");
            assert!(hit.body().contains("9.26.5"));
            assert!(hit.body().contains("update the firmware"));
            // None of these issues weakens a seed by itself, so the copy must
            // not send every keyholder off to rotate a key the way the Coldcard
            // copy does. It must still leave room for the device that was
            // actually exploited, where the key does need replacing — hence
            // "exception", not silence.
            assert!(!hit.body().contains("rotate this key out of your Cube"));
            assert!(hit.body().contains("exception"));
            assert!(hit.guide_label().contains("update"));
        }
    }

    /// The view-model side: which rows get a badge, and with what copy.
    mod row {
        use super::*;
        use crate::hw::{HardwareWallet, UnsupportedReason};

        fn coldcard_row(version: Option<Version>) -> HardwareWallet {
            HardwareWallet::Unsupported {
                id: "coldcard-test".to_string(),
                kind: DeviceKind::Coldcard,
                version,
                reason: UnsupportedReason::Version {
                    minimal_supported_version: "Edge firmware v6.2.1",
                    note: None,
                },
            }
        }

        #[test]
        fn a_flagged_row_is_badged_on_every_tier() {
            let affected = view::hit(&coldcard_row(Some(v(5, 5, 0)))).expect("badge");
            assert_eq!(affected.tier, AdvisoryTier::Affected);
            assert_eq!(affected.line, Some("Mk4/Mk5"));

            let patched = view::hit(&coldcard_row(Some(v(5, 6, 0)))).expect("badge");
            assert_eq!(patched.tier, AdvisoryTier::Residual);

            // The two tiers say different things, and neither says "clear".
            assert_ne!(affected.body(), patched.body());
            for hit in [&affected, &patched] {
                assert!(hit.body().contains("rotate"));
            }
        }

        #[test]
        fn a_row_with_no_readable_firmware_is_still_badged() {
            let hit = view::hit(&coldcard_row(None)).expect("badge");
            assert_eq!(hit.tier, AdvisoryTier::Affected);
        }

        #[test]
        fn other_vendors_get_no_badge() {
            let ledger = HardwareWallet::Unsupported {
                id: "ledger-test".to_string(),
                kind: DeviceKind::Ledger,
                version: Some(v(2, 1, 0)),
                reason: UnsupportedReason::WrongNetwork,
            };
            assert!(view::hit(&ledger).is_none());
        }
    }

    mod dismissal {
        use super::*;
        use crate::app::settings::global::GlobalSettings;
        use coincube_core::miniscript::bitcoin::bip32::Fingerprint;

        /// A fresh `global_settings.json` path per test — see the same
        /// helper in `phone_signer::pairing_store`.
        fn fresh_path() -> std::path::PathBuf {
            let mut path = std::env::temp_dir();
            path.push(format!("coincube-test-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&path).expect("mkdir tempdir");
            path.push("global_settings.json");
            path
        }

        #[test]
        fn dismissals_round_trip_through_settings() {
            let path = fresh_path();
            assert!(GlobalSettings::load_dismissed_hw_advisories(&path).is_empty());

            let fingerprint = Fingerprint::from([0xde, 0xad, 0xbe, 0xef]);
            let key = format!("{fingerprint}:{COLDCARD_RNG_2026_07}");
            GlobalSettings::dismiss_hw_advisory(&path, key.clone()).expect("persist");

            assert_eq!(
                GlobalSettings::load_dismissed_hw_advisories(&path),
                vec![key.clone()]
            );

            // Idempotent: dismissing twice doesn't duplicate the entry.
            GlobalSettings::dismiss_hw_advisory(&path, key.clone()).expect("persist");
            assert_eq!(
                GlobalSettings::load_dismissed_hw_advisories(&path),
                vec![key]
            );

            // Unrelated global settings survive the write.
            assert!(!GlobalSettings::load_developer_mode(&path));
        }

        #[test]
        fn notice_seen_flag_round_trips_and_is_per_advisory() {
            let path = fresh_path();
            assert!(!GlobalSettings::advisory_notice_seen(
                &path,
                COLDCARD_RNG_2026_07
            ));

            GlobalSettings::mark_advisory_notice_seen(&path, COLDCARD_RNG_2026_07)
                .expect("persist");
            assert!(GlobalSettings::advisory_notice_seen(
                &path,
                COLDCARD_RNG_2026_07
            ));
            assert!(!GlobalSettings::advisory_notice_seen(
                &path,
                "SOME-OTHER-ID"
            ));

            // Marking twice keeps a single entry, so the notice can never
            // fire again on this install.
            GlobalSettings::mark_advisory_notice_seen(&path, COLDCARD_RNG_2026_07)
                .expect("persist");
            assert!(GlobalSettings::advisory_notice_seen(
                &path,
                COLDCARD_RNG_2026_07
            ));
        }

        #[test]
        fn mirror_reflects_persisted_state() {
            let path = fresh_path();
            let fingerprint = Fingerprint::from([0x01, 0x02, 0x03, 0x04]);

            dismissals::init(path.clone());
            assert!(!dismissals::is_dismissed(
                &fingerprint,
                COLDCARD_RNG_2026_07
            ));

            dismissals::dismiss(&fingerprint, COLDCARD_RNG_2026_07);
            assert!(dismissals::is_dismissed(&fingerprint, COLDCARD_RNG_2026_07));
            // ... and it survives a reload from disk.
            dismissals::init(path);
            assert!(dismissals::is_dismissed(&fingerprint, COLDCARD_RNG_2026_07));

            // Another device with the same advisory is untouched.
            assert!(!dismissals::is_dismissed(
                &Fingerprint::from([0x05, 0x06, 0x07, 0x08]),
                COLDCARD_RNG_2026_07
            ));
        }
    }

    #[test]
    fn affected_ranges_do_not_overlap() {
        for advisory in ADVISORIES {
            for a in advisory.affected {
                assert!(
                    a.introduced.major == a.patched.major,
                    "{} spans majors; the line matching in `line_for` assumes it doesn't",
                    a.line
                );
                assert!(
                    a.introduced < a.patched,
                    "{} has an empty affected window",
                    a.line
                );
                for b in advisory.affected {
                    if a.line != b.line {
                        assert_ne!(a.patched.major, b.patched.major, "{} vs {}", a.line, b.line);
                    }
                }
            }
        }
    }
}
