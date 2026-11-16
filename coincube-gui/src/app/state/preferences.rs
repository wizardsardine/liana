//! Pure interaction state for installation-wide App Preferences.
//!
//! The GUI owns the accepted `DevicePreferences` snapshot and passes it to the
//! view. This module never reads or writes settings and must outlive an App tab
//! when it owns an in-flight save. Hide masks immediately in this session;
//! only a successful explicit Show can clear that mask.

use coincube_ui::locale::Text as CopyText;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::app::settings::preferences::{DevicePreferences, PreferenceChange};

/// Device-wide balance privacy as presented to the user. An accepted Show
/// receipt permits visibility unless this session has since requested Hide.
/// `DevicePreferences::default()` and an unresolved or failed load grant none.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum BalancePrivacy {
    /// No accepted privacy choice yet. Amounts stay masked.
    #[default]
    Unresolved,
    Hidden,
    /// A local Hide is active while the last accepted receipt still says Show.
    /// The next eye action retries persisting Hide, never reveals amounts.
    HiddenLocally,
    Shown,
}

impl BalancePrivacy {
    pub const fn from_balances_hidden(balances_hidden: Option<bool>) -> Self {
        match balances_hidden {
            Some(false) => Self::Shown,
            Some(true) => Self::Hidden,
            None => Self::Unresolved,
        }
    }

    /// Whether passive amounts must be masked in this state.
    pub const fn masks_amounts(self) -> bool {
        !matches!(self, Self::Shown)
    }

    /// The explicit change the eye offers. A local Hide that is not reflected
    /// by the accepted receipt retries Hide instead of undoing the mask.
    pub const fn toggle_change(self) -> PreferenceChange {
        PreferenceChange::HideBalances(matches!(self, Self::Shown | Self::HiddenLocally))
    }
}

/// Read-only input for the shared top-bar eye. Chrome never consults disk.
/// `interactive` is true only with accepted privacy choices and no explicit
/// change pending or queued. A read-only background refresh stays interactive.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BalancePrivacyChrome {
    pub privacy: BalancePrivacy,
    /// An explicit load or change is in flight, or a user change is queued.
    /// Read-only background refreshes alone do not set this flag.
    pub busy: bool,
    pub interactive: bool,
}

impl BalancePrivacyChrome {
    /// The typed intent the eye may emit now, if any.
    pub const fn intent(self) -> Option<PreferenceChange> {
        if self.interactive && !self.busy {
            Some(self.privacy.toggle_change())
        } else {
            None
        }
    }
}

/// What the GUI may publish from one accepted receipt: the snapshot, its
/// privacy provenance and whether a committed write's durability is unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcceptedPreferences {
    pub preferences: DevicePreferences,
    pub privacy: BalancePrivacy,
    pub durability_unconfirmed: bool,
}

impl From<&crate::app::settings::global::PreferencesReceipt> for AcceptedPreferences {
    fn from(receipt: &crate::app::settings::global::PreferencesReceipt) -> Self {
        use crate::app::settings::global::{Durability, Persistence};
        Self {
            preferences: receipt.preferences(),
            privacy: BalancePrivacy::from_balances_hidden(receipt.balances_hidden()),
            durability_unconfirmed: matches!(
                receipt.persistence(),
                Persistence::Committed(Durability::Unconfirmed(_))
            ),
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    #[default]
    Appearance,
    Launcher,
    BalancesAndMotion,
    Developer,
    Diagnostics,
    About,
}

impl Section {
    pub const ALL: [Self; 6] = [
        Self::Appearance,
        Self::Launcher,
        Self::BalancesAndMotion,
        Self::Developer,
        Self::Diagnostics,
        Self::About,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Appearance => CopyText::Appearance.get(),
            Self::Launcher => CopyText::Launcher.get(),
            Self::BalancesAndMotion => CopyText::BalancesMotion.get(),
            Self::Developer => CopyText::Developer.get(),
            Self::Diagnostics => CopyText::Diagnostics.get(),
            Self::About => CopyText::About.get(),
        }
    }

    pub const fn action_id(self) -> &'static str {
        match self {
            Self::Appearance => "tenshu.preferences.section.appearance",
            Self::Launcher => "tenshu.preferences.section.launcher",
            Self::BalancesAndMotion => "tenshu.preferences.section.balances-motion",
            Self::Developer => "tenshu.preferences.section.developer",
            Self::Diagnostics => "tenshu.preferences.section.diagnostics",
            Self::About => "tenshu.preferences.section.about",
        }
    }
}

/// What About shows: its rows, or one of the pages its links open.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum AboutPage {
    #[default]
    Main,
    DownloadStats,
    Licenses,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PreferencesPageState {
    pub section: Section,
    pub about_page: AboutPage,
    pub component_versions_expanded: bool,
    pub registration_repair_expanded: bool,
}

impl PreferencesPageState {
    /// Choosing a section (About included) shows its main page.
    pub fn select(&mut self, section: Section) {
        self.section = section;
        self.about_page = AboutPage::Main;
    }

    pub fn open_about_page(&mut self, page: AboutPage) {
        self.section = Section::About;
        self.about_page = page;
    }

    pub fn toggle_component_versions(&mut self) {
        self.component_versions_expanded = !self.component_versions_expanded;
    }

    pub fn toggle_registration_repair(&mut self) {
        self.registration_repair_expanded = !self.registration_repair_expanded;
    }
}

/// What one request on the ordered preferences lane does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    /// Initial load or explicit retry:
    /// `GlobalSettings::load_or_initialize_preferences(root_directory)`.
    Initialize,
    /// One explicit change: `GlobalSettings::save_preference(&request.root, change)`.
    Save(PreferenceChange),
}

/// Identity travels with the async result. The GUI supplies a new
/// `root_generation` whenever it replaces an installation root. A result from
/// an old root, generation or operation cannot be accepted by the coordinator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveRequest {
    /// Exact settings file path from `GlobalSettings::path`, not its parent directory.
    pub root: PathBuf,
    pub root_generation: u64,
    pub request_generation: u64,
    pub operation: Operation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BeginSaveError {
    Busy,
    /// A user change was retained behind an in-flight background refresh.
    Queued,
    GenerationExhausted,
    /// [`PreferencesOwner`] only: no choices are accepted yet, so only an
    /// explicit Show/Hide balances choice may be saved.
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveOutcome<T = DevicePreferences> {
    /// GUI must apply this returned, persisted result to all live mirrors.
    /// Only this variant may publish choices, provenance or a durability notice.
    Applied(T),
    /// GUI retains its last accepted snapshot and displays this retryable error.
    Failed(String),
    /// Foreign, old, or duplicate callback: no state or snapshot change.
    Ignored,
}

#[derive(Debug, Clone)]
pub struct SaveCoordinator {
    root: PathBuf,
    root_generation: u64,
    next_request_generation: u64,
    pending: Option<SaveRequest>,
    error: Option<String>,
}

impl SaveCoordinator {
    pub fn new(root: PathBuf, root_generation: u64) -> Self {
        Self {
            root,
            root_generation,
            next_request_generation: 0,
            pending: None,
            error: None,
        }
    }

    pub fn is_saving(&self) -> bool {
        self.pending.is_some()
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn begin(&mut self, change: PreferenceChange) -> Result<SaveRequest, BeginSaveError> {
        self.begin_operation(Operation::Save(change))
    }

    /// Starts an initial load or retry on the same ordered lane as saves, so
    /// a retry and a save can never race or be silently reordered.
    pub fn begin_initialize(&mut self) -> Result<SaveRequest, BeginSaveError> {
        self.begin_operation(Operation::Initialize)
    }

    fn begin_operation(&mut self, operation: Operation) -> Result<SaveRequest, BeginSaveError> {
        if self.pending.is_some() {
            return Err(BeginSaveError::Busy);
        }
        let next = self
            .next_request_generation
            .checked_add(1)
            .ok_or(BeginSaveError::GenerationExhausted)?;
        self.next_request_generation = next;
        self.error = None;
        let request = SaveRequest {
            root: self.root.clone(),
            root_generation: self.root_generation,
            request_generation: next,
            operation,
        };
        self.pending = Some(request.clone());
        Ok(request)
    }

    /// Does not own the persisted snapshot. The integration layer runs the
    /// request's `Operation` off the UI thread and passes its exact result
    /// here. It must keep this coordinator alive across route changes,
    /// including a return to Launcher.
    pub fn complete(
        &mut self,
        request: &SaveRequest,
        result: Result<DevicePreferences, String>,
    ) -> SaveOutcome {
        self.complete_with(request, result)
    }

    /// The same identity checks, carrying an accepted payload intact (for
    /// example a `PreferencesReceipt`). A foreign, stale or duplicate
    /// completion is `Ignored` and changes nothing.
    pub fn complete_with<T>(
        &mut self,
        request: &SaveRequest,
        result: Result<T, String>,
    ) -> SaveOutcome<T> {
        if self.pending.as_ref() != Some(request)
            || request.root != self.root
            || request.root_generation != self.root_generation
        {
            return SaveOutcome::Ignored;
        }
        self.pending = None;
        match result {
            Ok(snapshot) => {
                self.error = None;
                SaveOutcome::Applied(snapshot)
            }
            Err(error) => {
                self.error = Some(error.clone());
                SaveOutcome::Failed(error)
            }
        }
    }
}

/// What the user may be told about the preferences lane. Deliberately
/// neutral: never a path, an OS error or a raw durability warning.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum PreferencesNotice {
    #[default]
    None,
    /// No choices could be loaded or initialized. Amounts stay masked; offer
    /// a load retry.
    LoadFailed,
    /// The last save was not committed. The accepted receipt is unchanged;
    /// a local Hide still masks immediately.
    SaveFailed,
    /// Saved; durability could not be confirmed. Not a rollback.
    SavedUnconfirmed,
}

/// The GUI's one device-preferences owner. Built from the startup receipt
/// before any pane, Home or App exists, and kept for the whole process across
/// page changes, lock and sign-out. Its single lane serializes retries and
/// saves; while it is busy every new request is refused, never reordered.
///
/// It performs no I/O. The GUI runs each request off the UI thread and passes
/// the exact result to [`Self::complete`]. A Hide request masks immediately;
/// only a successful explicit Show receipt releases that session mask.
#[derive(Debug, Clone)]
pub struct PreferencesOwner {
    lane: SaveCoordinator,
    accepted: Option<AcceptedPreferences>,
    last_refresh: Instant,
    background_refresh: Option<u64>,
    queued_changes: VecDeque<PreferenceChange>,
    /// A failed reread cannot leave a previously accepted Show visible.
    refresh_unresolved: bool,
    /// Session safety latch. A failed Hide or a later reload of persisted Show
    /// must not undo a user's Hide click.
    session_hide: bool,
    notice: PreferencesNotice,
}

impl PreferencesOwner {
    /// The process has one installation root, so one root generation.
    const ROOT_GENERATION: u64 = 1;

    /// `settings_path` is `GlobalSettings::path(root)` for the startup root.
    pub fn new(
        settings_path: PathBuf,
        startup: &Result<
            crate::app::settings::global::PreferencesReceipt,
            crate::app::settings::global::PrivacyInitError,
        >,
    ) -> Self {
        let mut owner = Self {
            lane: SaveCoordinator::new(settings_path, Self::ROOT_GENERATION),
            accepted: None,
            last_refresh: Instant::now(),
            background_refresh: None,
            queued_changes: VecDeque::new(),
            refresh_unresolved: false,
            session_hide: false,
            notice: PreferencesNotice::None,
        };
        match startup {
            Ok(receipt) => owner.accept(receipt),
            Err(_) => owner.notice = PreferencesNotice::LoadFailed,
        }
        owner
    }

    /// Masked unless the latest accepted receipt established Show and no local
    /// Hide is active. The accepted receipt itself is never overwritten here.
    pub fn privacy(&self) -> BalancePrivacy {
        if self.session_hide
            && (self.refresh_unresolved
                || !self
                    .accepted
                    .is_some_and(|accepted| accepted.privacy == BalancePrivacy::Hidden))
        {
            return BalancePrivacy::HiddenLocally;
        }
        if self.refresh_unresolved {
            return BalancePrivacy::Unresolved;
        }
        match self.accepted.map(|accepted| accepted.privacy) {
            Some(privacy) => privacy,
            None => BalancePrivacy::Unresolved,
        }
    }

    /// The latest accepted choices, if any receipt has been accepted.
    pub fn accepted(&self) -> Option<&AcceptedPreferences> {
        self.accepted.as_ref()
    }

    pub fn notice(&self) -> PreferencesNotice {
        self.notice
    }

    pub fn is_busy(&self) -> bool {
        self.lane.is_saving()
    }

    /// A read-only periodic refresh must not look like a user-initiated save.
    pub fn is_background_refreshing(&self) -> bool {
        self.background_refresh.is_some()
    }

    pub fn has_queued_changes(&self) -> bool {
        !self.queued_changes.is_empty()
    }

    pub fn pending_operation(&self) -> Option<Operation> {
        self.lane.pending.as_ref().map(|request| request.operation)
    }

    /// A retry of `load_or_initialize_preferences` on the shared lane.
    pub fn begin_initialize(&mut self) -> Result<SaveRequest, BeginSaveError> {
        let request = self.lane.begin_initialize()?;
        self.last_refresh = Instant::now();
        Ok(request)
    }

    /// Reread the strict global choice periodically so another process's
    /// Hide reaches every mounted amount presenter without route changes.
    pub fn begin_refresh_if_due(&mut self) -> Option<SaveRequest> {
        if self.last_refresh.elapsed() < Duration::from_secs(1) {
            return None;
        }
        let request = self.begin_initialize().ok()?;
        self.background_refresh = Some(request.request_generation);
        Some(request)
    }

    /// One explicit change. Before any choices are accepted only an explicit
    /// Show/Hide balances choice is offered; it still goes through the strict
    /// save and cannot repair a malformed file.
    pub fn begin_save(&mut self, change: PreferenceChange) -> Result<SaveRequest, BeginSaveError> {
        if self.accepted.is_none() && !matches!(change, PreferenceChange::HideBalances(_)) {
            return Err(BeginSaveError::Unavailable);
        }
        if self.is_background_refreshing() {
            self.queued_changes.push_back(change);
            if matches!(change, PreferenceChange::HideBalances(true)) {
                self.session_hide = true;
            }
            return Err(BeginSaveError::Queued);
        }
        let request = self.lane.begin(change)?;
        if matches!(change, PreferenceChange::HideBalances(true)) {
            self.session_hide = true;
        }
        Ok(request)
    }

    /// Applies one completion. A foreign, stale or duplicate completion is
    /// `Ignored` and changes neither the choices nor the notice.
    pub fn complete(
        &mut self,
        request: &SaveRequest,
        result: Result<crate::app::settings::global::PreferencesReceipt, String>,
    ) -> SaveOutcome<AcceptedPreferences> {
        let outcome = self.lane.complete_with(request, result);
        if !matches!(&outcome, SaveOutcome::Ignored)
            && self.background_refresh == Some(request.request_generation)
        {
            self.background_refresh = None;
        }
        match outcome {
            SaveOutcome::Applied(receipt) => {
                self.accept(&receipt);
                if matches!(
                    request.operation,
                    Operation::Save(PreferenceChange::HideBalances(false))
                ) && self
                    .accepted
                    .is_some_and(|accepted| accepted.privacy == BalancePrivacy::Shown)
                {
                    self.session_hide = false;
                }
                SaveOutcome::Applied(AcceptedPreferences::from(&receipt))
            }
            SaveOutcome::Failed(error) => {
                if matches!(request.operation, Operation::Initialize) {
                    self.refresh_unresolved = true;
                }
                self.notice = match request.operation {
                    Operation::Initialize => PreferencesNotice::LoadFailed,
                    Operation::Save(_) => PreferencesNotice::SaveFailed,
                };
                SaveOutcome::Failed(error)
            }
            SaveOutcome::Ignored => SaveOutcome::Ignored,
        }
    }

    fn accept(&mut self, receipt: &crate::app::settings::global::PreferencesReceipt) {
        self.refresh_unresolved = false;
        let accepted = AcceptedPreferences::from(receipt);
        self.notice = if accepted.durability_unconfirmed {
            PreferencesNotice::SavedUnconfirmed
        } else {
            PreferencesNotice::None
        };
        self.accepted = Some(accepted);
    }

    /// Resume one user action after the periodic read has completed.
    pub fn begin_next_queued_save(&mut self) -> Option<SaveRequest> {
        if self.is_busy() {
            return None;
        }
        let change = *self.queued_changes.front()?;
        match self.begin_save(change) {
            Ok(request) => {
                self.queued_changes.pop_front();
                Some(request)
            }
            Err(_) => {
                // Keep the intent for a later retry or owner inspection.
                self.notice = PreferencesNotice::SaveFailed;
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_cross_process_refresh_masks_a_previously_shown_choice() {
        use crate::app::settings::global::GlobalSettings;

        let root = std::env::temp_dir().join(format!("privacy-refresh-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join(crate::app::settings::global::DEFAULT_FILE_NAME);
        let shown =
            GlobalSettings::save_preference(&path, PreferenceChange::HideBalances(false)).unwrap();
        let mut owner = PreferencesOwner::new(path.clone(), &Ok(shown));
        assert_eq!(owner.privacy(), BalancePrivacy::Shown);

        let failed = owner.begin_initialize().unwrap();
        assert!(matches!(
            owner.complete(&failed, Err("synthetic reread failure".into())),
            SaveOutcome::Failed(_)
        ));
        assert_eq!(owner.privacy(), BalancePrivacy::Unresolved);

        let hidden =
            GlobalSettings::save_preference(&path, PreferenceChange::HideBalances(true)).unwrap();
        let refreshed = owner.begin_initialize().unwrap();
        assert!(matches!(
            owner.complete(&refreshed, Ok(hidden)),
            SaveOutcome::Applied(_)
        ));
        assert_eq!(owner.privacy(), BalancePrivacy::Hidden);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_hide_remains_a_retry_hide_after_a_failed_refresh() {
        use crate::app::settings::global::GlobalSettings;

        let root = std::env::temp_dir().join(format!("privacy-hide-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join(crate::app::settings::global::DEFAULT_FILE_NAME);
        let shown =
            GlobalSettings::save_preference(&path, PreferenceChange::HideBalances(false)).unwrap();
        let mut owner = PreferencesOwner::new(path, &Ok(shown));
        let hide = owner
            .begin_save(PreferenceChange::HideBalances(true))
            .unwrap();
        assert!(matches!(
            owner.complete(&hide, Err("synthetic save failure".into())),
            SaveOutcome::Failed(_)
        ));
        assert_eq!(owner.privacy(), BalancePrivacy::HiddenLocally);
        assert_eq!(
            owner.privacy().toggle_change(),
            PreferenceChange::HideBalances(true)
        );

        let refresh = owner.begin_initialize().unwrap();
        assert!(matches!(
            owner.complete(&refresh, Err("synthetic reread failure".into())),
            SaveOutcome::Failed(_)
        ));
        assert_eq!(owner.privacy(), BalancePrivacy::HiddenLocally);
        assert_eq!(
            owner.privacy().toggle_change(),
            PreferenceChange::HideBalances(true)
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn only_an_accepted_false_shows_balances() {
        assert_eq!(BalancePrivacy::default(), BalancePrivacy::Unresolved);
        assert!(BalancePrivacy::default().masks_amounts());
        assert!(BalancePrivacy::from_balances_hidden(None).masks_amounts());
        assert!(BalancePrivacy::from_balances_hidden(Some(true)).masks_amounts());
        assert!(!BalancePrivacy::from_balances_hidden(Some(false)).masks_amounts());
        // A default snapshot says hide_balances == false. It is not authority.
        assert!(!DevicePreferences::default().hide_balances);
        assert_eq!(
            BalancePrivacy::from_balances_hidden(None),
            BalancePrivacy::Unresolved
        );
    }

    #[test]
    fn toggle_offers_the_opposite_explicit_choice() {
        assert_eq!(
            BalancePrivacy::Shown.toggle_change(),
            PreferenceChange::HideBalances(true)
        );
        assert_eq!(
            BalancePrivacy::Hidden.toggle_change(),
            PreferenceChange::HideBalances(false)
        );
        assert_eq!(
            BalancePrivacy::HiddenLocally.toggle_change(),
            PreferenceChange::HideBalances(true)
        );
        assert_eq!(
            BalancePrivacy::Unresolved.toggle_change(),
            PreferenceChange::HideBalances(false)
        );
    }

    #[test]
    fn chrome_emits_only_when_interactive_and_idle() {
        for privacy in [
            BalancePrivacy::Unresolved,
            BalancePrivacy::Hidden,
            BalancePrivacy::HiddenLocally,
            BalancePrivacy::Shown,
        ] {
            let idle = BalancePrivacyChrome {
                privacy,
                busy: false,
                interactive: true,
            };
            assert_eq!(idle.intent(), Some(privacy.toggle_change()));
            assert_eq!(BalancePrivacyChrome { busy: true, ..idle }.intent(), None);
            assert_eq!(
                BalancePrivacyChrome {
                    interactive: false,
                    ..idle
                }
                .intent(),
                None
            );
        }
        assert_eq!(BalancePrivacyChrome::default().intent(), None);
    }

    #[test]
    fn save_is_serial_and_only_current_root_result_can_apply() {
        let mut coordinator =
            SaveCoordinator::new(PathBuf::from("/installation-a/settings.json"), 7);
        let first = coordinator
            .begin(PreferenceChange::HideBalances(true))
            .unwrap();
        assert_eq!(
            coordinator.begin(PreferenceChange::DeveloperMode(true)),
            Err(BeginSaveError::Busy)
        );
        let foreign = SaveRequest {
            root: PathBuf::from("/installation-b/settings.json"),
            ..first.clone()
        };
        assert_eq!(
            coordinator.complete(&foreign, Ok(DevicePreferences::default())),
            SaveOutcome::Ignored
        );
        assert!(coordinator.is_saving());
        let accepted = DevicePreferences {
            hide_balances: true,
            ..DevicePreferences::default()
        };
        assert_eq!(
            coordinator.complete(&first, Ok(accepted)),
            SaveOutcome::Applied(accepted)
        );
        assert!(!coordinator.is_saving());
        assert_eq!(
            coordinator.complete(&first, Ok(DevicePreferences::default())),
            SaveOutcome::Ignored
        );
        let second = coordinator
            .begin(PreferenceChange::DeveloperMode(true))
            .unwrap();
        assert!(second.request_generation > first.request_generation);
    }

    #[test]
    fn failed_save_keeps_snapshot_external_and_can_retry() {
        let mut coordinator = SaveCoordinator::new(PathBuf::from("/installation/settings.json"), 1);
        let request = coordinator
            .begin(PreferenceChange::Theme(
                crate::app::settings::preferences::ThemePreference::Light,
            ))
            .unwrap();
        assert_eq!(
            coordinator.complete(&request, Err("permission denied".into())),
            SaveOutcome::Failed("permission denied".into())
        );
        assert_eq!(coordinator.error(), Some("permission denied"));
        assert!(!coordinator.is_saving());
        assert!(coordinator
            .begin(PreferenceChange::Theme(
                crate::app::settings::preferences::ThemePreference::Light
            ))
            .is_ok());
        assert_eq!(coordinator.error(), None);
    }

    #[test]
    fn altered_identity_or_change_cannot_complete_pending_save() {
        let mut coordinator = SaveCoordinator::new(PathBuf::from("/installation/settings.json"), 7);
        let current = coordinator
            .begin(PreferenceChange::DeveloperMode(true))
            .unwrap();
        for stale in [
            SaveRequest {
                root_generation: 8,
                ..current.clone()
            },
            SaveRequest {
                request_generation: 2,
                ..current.clone()
            },
            SaveRequest {
                operation: Operation::Save(PreferenceChange::DeveloperMode(false)),
                ..current.clone()
            },
        ] {
            assert_eq!(
                coordinator.complete(&stale, Err("stale error".into())),
                SaveOutcome::Ignored
            );
            assert!(coordinator.is_saving());
            assert_eq!(coordinator.error(), None);
        }
        assert_eq!(
            coordinator.complete(&current, Err("disk full".into())),
            SaveOutcome::Failed("disk full".into())
        );
        let newer = coordinator
            .begin(PreferenceChange::DeveloperMode(true))
            .unwrap();
        assert_eq!(
            coordinator.complete(&current, Err("old failure".into())),
            SaveOutcome::Ignored
        );
        assert_eq!(coordinator.error(), None);
        assert!(coordinator.is_saving());
        assert_eq!(newer.request_generation, current.request_generation + 1);
    }

    #[test]
    fn exhausted_generation_never_creates_a_request_or_erases_error() {
        let mut coordinator = SaveCoordinator::new(PathBuf::from("/installation/settings.json"), 1);
        coordinator.next_request_generation = u64::MAX - 1;
        let last = coordinator
            .begin(PreferenceChange::HideBalances(true))
            .unwrap();
        assert_eq!(last.request_generation, u64::MAX);
        assert_eq!(
            coordinator.complete(&last, Err("write denied".into())),
            SaveOutcome::Failed("write denied".into())
        );
        assert_eq!(
            coordinator.begin(PreferenceChange::HideBalances(false)),
            Err(BeginSaveError::GenerationExhausted)
        );
        assert_eq!(coordinator.error(), Some("write denied"));
        assert!(!coordinator.is_saving());
    }

    #[test]
    fn initialize_and_save_share_one_ordered_lane() {
        let mut coordinator = SaveCoordinator::new(PathBuf::from("/installation/settings.json"), 3);
        let init = coordinator.begin_initialize().unwrap();
        assert_eq!(init.operation, Operation::Initialize);
        assert_eq!(
            coordinator.begin(PreferenceChange::HideBalances(false)),
            Err(BeginSaveError::Busy)
        );
        // A Save completion cannot complete the pending Initialize.
        let wrong_operation = SaveRequest {
            operation: Operation::Save(PreferenceChange::HideBalances(false)),
            ..init.clone()
        };
        assert_eq!(
            coordinator.complete_with(&wrong_operation, Ok("receipt")),
            SaveOutcome::Ignored
        );
        assert!(coordinator.is_saving());
        assert_eq!(
            coordinator.complete_with(&init, Ok("receipt")),
            SaveOutcome::Applied("receipt")
        );
        let save = coordinator
            .begin(PreferenceChange::HideBalances(false))
            .unwrap();
        assert!(save.request_generation > init.request_generation);
        assert_eq!(
            coordinator.begin_initialize(),
            Err(BeginSaveError::Busy),
            "a retry cannot overtake a save"
        );
        assert_eq!(
            coordinator.complete_with::<&str>(&save, Err("not committed".into())),
            SaveOutcome::Failed("not committed".into())
        );
        assert!(coordinator.begin_initialize().is_ok());
    }

    /// A temporary existing installation root for owner tests.
    struct OwnerRoot(PathBuf);

    impl OwnerRoot {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "r04b-preferences-owner-{tag}-{}",
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn root(&self) -> crate::dir::CoincubeDirectory {
            crate::dir::CoincubeDirectory::new(self.0.clone())
        }
        fn settings(&self) -> PathBuf {
            crate::app::settings::global::GlobalSettings::path(&self.root())
        }
        /// Runs a request exactly as the GUI's worker does.
        fn run(
            &self,
            request: &SaveRequest,
        ) -> Result<crate::app::settings::global::PreferencesReceipt, String> {
            use crate::app::settings::global::GlobalSettings;
            match request.operation {
                Operation::Initialize => {
                    GlobalSettings::load_or_initialize_preferences(&self.root())
                        .map_err(|e| e.to_string())
                }
                Operation::Save(change) => GlobalSettings::save_preference(&request.root, change)
                    .map_err(|e| e.to_string()),
            }
        }
    }

    impl Drop for OwnerRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn owner_masks_until_a_receipt_and_offers_only_an_explicit_choice() {
        let root = OwnerRoot::new("unresolved");
        std::fs::write(root.settings(), b"{").unwrap();
        let startup = crate::app::settings::global::GlobalSettings::load_or_initialize_preferences(
            &root.root(),
        );
        assert!(startup.is_err());
        let mut owner = PreferencesOwner::new(root.settings(), &startup);
        assert_eq!(owner.privacy(), BalancePrivacy::Unresolved);
        assert!(owner.privacy().masks_amounts());
        assert_eq!(owner.accepted(), None);
        assert_eq!(owner.notice(), PreferencesNotice::LoadFailed);
        // Defaults are never a fallback: nothing but an explicit choice.
        assert_eq!(
            owner.begin_save(PreferenceChange::DeveloperMode(true)),
            Err(BeginSaveError::Unavailable)
        );
        assert!(!owner.is_busy());
        // The explicit save is still strict: a malformed file is refused.
        let show = owner
            .begin_save(PreferenceChange::HideBalances(false))
            .unwrap();
        assert!(owner.is_busy());
        let result = root.run(&show);
        assert!(result.is_err());
        assert!(matches!(
            owner.complete(&show, result),
            SaveOutcome::Failed(_)
        ));
        assert_eq!(owner.privacy(), BalancePrivacy::Unresolved);
        assert_eq!(owner.notice(), PreferencesNotice::SaveFailed);
        assert_eq!(std::fs::read(root.settings()).unwrap(), b"{".to_vec());
    }

    #[test]
    fn owner_publishes_only_current_receipts_from_one_lane() {
        let root = OwnerRoot::new("lane");
        let startup = crate::app::settings::global::GlobalSettings::load_or_initialize_preferences(
            &root.root(),
        );
        let mut owner = PreferencesOwner::new(root.settings(), &startup);
        assert_eq!(
            owner.privacy(),
            BalancePrivacy::Hidden,
            "existing root hides once"
        );
        assert_eq!(owner.notice(), PreferencesNotice::None);

        let show = owner
            .begin_save(PreferenceChange::HideBalances(false))
            .unwrap();
        // One lane: neither a retry nor a second save can overtake it.
        assert_eq!(owner.begin_initialize(), Err(BeginSaveError::Busy));
        assert_eq!(
            owner.begin_save(PreferenceChange::HideBalances(true)),
            Err(BeginSaveError::Busy)
        );
        let shown = root.run(&show);
        // A completion for another settings file is ignored.
        let foreign = SaveRequest {
            root: PathBuf::from("/elsewhere/global_settings.json"),
            ..show.clone()
        };
        assert_eq!(
            owner.complete(&foreign, shown.clone()),
            SaveOutcome::Ignored
        );
        assert_eq!(owner.privacy(), BalancePrivacy::Hidden);
        assert!(matches!(
            owner.complete(&show, shown.clone()),
            SaveOutcome::Applied(AcceptedPreferences {
                privacy: BalancePrivacy::Shown,
                ..
            })
        ));
        assert_eq!(owner.privacy(), BalancePrivacy::Shown);
        // The duplicate of an applied completion changes nothing.
        assert_eq!(owner.complete(&show, shown), SaveOutcome::Ignored);

        // Hide takes effect before the write. A precommit failure cannot
        // undo it, even though the accepted receipt still says Show.
        let hide = owner
            .begin_save(PreferenceChange::HideBalances(true))
            .unwrap();
        assert_eq!(owner.privacy(), BalancePrivacy::HiddenLocally);
        assert_eq!(owner.accepted().unwrap().privacy, BalancePrivacy::Shown);
        assert!(matches!(
            owner.complete(&hide, Err("not committed".into())),
            SaveOutcome::Failed(_)
        ));
        assert_eq!(owner.privacy(), BalancePrivacy::HiddenLocally);
        assert_eq!(owner.notice(), PreferencesNotice::SaveFailed);

        // A retry re-reads persisted Show but cannot clear the session mask.
        let retry = owner.begin_initialize().unwrap();
        let reread = root.run(&retry);
        assert!(matches!(
            owner.complete(&retry, reread),
            SaveOutcome::Applied(_)
        ));
        assert_eq!(owner.privacy(), BalancePrivacy::HiddenLocally);
        assert_eq!(owner.accepted().unwrap().privacy, BalancePrivacy::Shown);
        assert_eq!(owner.notice(), PreferencesNotice::None);
        // A failed explicit Show is still masked. Only its successful receipt
        // can release the session Hide latch.
        let failed_show = owner
            .begin_save(PreferenceChange::HideBalances(false))
            .unwrap();
        assert_eq!(owner.privacy(), BalancePrivacy::HiddenLocally);
        assert!(matches!(
            owner.complete(&failed_show, Err("not committed".into())),
            SaveOutcome::Failed(_)
        ));
        assert_eq!(owner.privacy(), BalancePrivacy::HiddenLocally);
        let show_again = owner
            .begin_save(PreferenceChange::HideBalances(false))
            .unwrap();
        let shown_again = root.run(&show_again);
        assert!(matches!(
            owner.complete(&show_again, shown_again),
            SaveOutcome::Applied(_)
        ));
        assert_eq!(owner.privacy(), BalancePrivacy::Shown);
        // Ordinary changes are available once choices are accepted.
        assert!(owner
            .begin_save(PreferenceChange::DeveloperMode(true))
            .is_ok());
    }

    #[test]
    fn queued_hide_masks_immediately_and_saves_after_background_refresh() {
        use crate::app::settings::global::GlobalSettings;

        let root = OwnerRoot::new("queued-hide");
        GlobalSettings::load_or_initialize_preferences(&root.root()).unwrap();
        let shown = GlobalSettings::save_preference(
            &root.settings(),
            PreferenceChange::HideBalances(false),
        )
        .unwrap();
        let mut owner = PreferencesOwner::new(root.settings(), &Ok(shown));
        assert_eq!(owner.privacy(), BalancePrivacy::Shown);

        owner.last_refresh = Instant::now() - Duration::from_secs(2);
        let refresh = owner.begin_refresh_if_due().unwrap();
        assert!(owner.is_background_refreshing());
        assert_eq!(
            owner.begin_save(PreferenceChange::HideBalances(true)),
            Err(BeginSaveError::Queued)
        );
        assert_eq!(owner.privacy(), BalancePrivacy::HiddenLocally);
        assert!(matches!(
            owner.complete(&refresh, root.run(&refresh)),
            SaveOutcome::Applied(_)
        ));
        assert!(!owner.is_background_refreshing());
        assert_eq!(owner.privacy(), BalancePrivacy::HiddenLocally);
        let hide = owner.begin_next_queued_save().unwrap();
        assert_eq!(
            hide.operation,
            Operation::Save(PreferenceChange::HideBalances(true))
        );
        assert!(matches!(
            owner.complete(&hide, root.run(&hide)),
            SaveOutcome::Applied(_)
        ));
        assert_eq!(owner.privacy(), BalancePrivacy::Hidden);
    }

    #[test]
    fn queued_show_waits_for_external_hide_receipt() {
        use crate::app::settings::global::GlobalSettings;

        let root = OwnerRoot::new("queued-show");
        GlobalSettings::load_or_initialize_preferences(&root.root()).unwrap();
        let shown = GlobalSettings::save_preference(
            &root.settings(),
            PreferenceChange::HideBalances(false),
        )
        .unwrap();
        let mut owner = PreferencesOwner::new(root.settings(), &Ok(shown));

        owner.last_refresh = Instant::now() - Duration::from_secs(2);
        let refresh = owner.begin_refresh_if_due().unwrap();
        assert_eq!(
            owner.begin_save(PreferenceChange::HideBalances(false)),
            Err(BeginSaveError::Queued)
        );
        GlobalSettings::save_preference(&root.settings(), PreferenceChange::HideBalances(true))
            .unwrap();
        assert!(matches!(
            owner.complete(&refresh, root.run(&refresh)),
            SaveOutcome::Applied(_)
        ));
        assert_eq!(owner.privacy(), BalancePrivacy::Hidden);
        let show = owner.begin_next_queued_save().unwrap();
        assert_eq!(
            show.operation,
            Operation::Save(PreferenceChange::HideBalances(false))
        );
        assert_eq!(owner.privacy(), BalancePrivacy::Hidden);
        assert!(matches!(
            owner.complete(&show, root.run(&show)),
            SaveOutcome::Applied(_)
        ));
        assert_eq!(owner.privacy(), BalancePrivacy::Shown);
    }

    #[test]
    fn queued_change_is_retained_when_a_new_request_cannot_start() {
        use crate::app::settings::global::GlobalSettings;

        let root = OwnerRoot::new("queued-exhaustion");
        let startup = GlobalSettings::load_or_initialize_preferences(&root.root()).unwrap();
        let mut owner = PreferencesOwner::new(root.settings(), &Ok(startup));
        owner.last_refresh = Instant::now() - Duration::from_secs(2);
        let refresh = owner.begin_refresh_if_due().unwrap();
        assert_eq!(
            owner.begin_save(PreferenceChange::DeveloperMode(true)),
            Err(BeginSaveError::Queued)
        );
        owner.lane.next_request_generation = u64::MAX;
        assert!(matches!(
            owner.complete(&refresh, root.run(&refresh)),
            SaveOutcome::Applied(_)
        ));
        assert!(owner.begin_next_queued_save().is_none());
        assert_eq!(owner.queued_changes.len(), 1);
        assert_eq!(owner.notice(), PreferencesNotice::SaveFailed);
    }

    /// Real receipts from a temporary installation root, not fabricated
    /// provenance. Only an explicit Show yields `Shown`.
    #[test]
    fn accepted_preferences_follow_real_receipts() {
        use crate::app::settings::global::GlobalSettings;
        let path = std::env::temp_dir().join(format!(
            "r04b-accepted-preferences-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&path).unwrap();
        let root = crate::dir::CoincubeDirectory::new(path.clone());
        let initialized = GlobalSettings::load_or_initialize_preferences(&root)
            .expect("an existing root initializes hidden");
        let accepted = AcceptedPreferences::from(&initialized);
        assert_eq!(accepted.privacy, BalancePrivacy::Hidden);
        assert!(accepted.privacy.masks_amounts());
        let shown = GlobalSettings::save_preference(
            &GlobalSettings::path(&root),
            PreferenceChange::HideBalances(false),
        )
        .expect("explicit Show commits");
        assert_eq!(
            AcceptedPreferences::from(&shown).privacy,
            BalancePrivacy::Shown
        );
        let hidden = GlobalSettings::save_preference(
            &GlobalSettings::path(&root),
            PreferenceChange::HideBalances(true),
        )
        .expect("explicit Hide commits");
        assert_eq!(
            AcceptedPreferences::from(&hidden).privacy,
            BalancePrivacy::Hidden
        );
        std::fs::remove_dir_all(path).ok();
    }
}
