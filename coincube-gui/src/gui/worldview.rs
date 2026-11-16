//! Publish accepted local display edits while account sharing remains enabled.

use super::{preferences_view, Message, OverlayOrigin, GUI};
use crate::app::{
    settings::preferences::{DevicePreferences, PreferenceChange},
    state::preferences::{Operation, SaveRequest},
};
use crate::services::coincube::worldview::{Error, Selection, Snapshot};
use coincube_ui::locale::copy;
use iced::Task;
use std::collections::VecDeque;

type Context = (u32, u64, u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Refresh,
    Publish,
    Revoke,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Binding {
    origin: OverlayOrigin,
    context: Context,
}

#[derive(Debug, Clone)]
struct Intent {
    binding: Binding,
    epoch: uuid::Uuid,
}

#[derive(Debug, Clone, Copy)]
enum Request {
    Read,
    AutomaticRead,
    Publish(Selection),
    Revoke,
}

#[derive(Debug, Default)]
pub struct Sharing {
    binding: Option<Binding>,
    // Rebinding must never allow a second request while an old PUT is active.
    in_flight: Option<(Binding, uuid::Uuid)>,
    pending: Option<uuid::Uuid>,
    operation: Option<Request>,
    snapshot: Option<Snapshot>,
    error: Option<Error>,
    desired: Option<Selection>,
    read_ready: bool,
    deferred_refresh: Option<Binding>,
    epoch: uuid::Uuid,
    save: Option<(SaveRequest, Option<Intent>)>,
    queued: VecDeque<(PreferenceChange, Option<Intent>)>,
}

impl Sharing {
    fn cancel_refresh_for(&mut self, account: u32) {
        if self
            .deferred_refresh
            .as_ref()
            .is_some_and(|binding| binding.context.0 == account)
        {
            self.deferred_refresh = None;
        }
    }

    fn invalidate_intents(&mut self) {
        self.epoch = uuid::Uuid::new_v4();
        self.desired = None;
        self.read_ready = false;
    }

    fn bind(&mut self, binding: &Binding) {
        if self.binding.as_ref() != Some(binding) {
            let same_account = self
                .binding
                .as_ref()
                .is_some_and(|current| current.context.0 == binding.context.0);
            self.desired = None;
            self.read_ready = false;
            self.binding = Some(binding.clone());
            self.pending = None;
            self.operation = None;
            self.snapshot = None;
            if !same_account {
                self.error = None;
            }
        }
    }

    fn complete(&mut self, request: uuid::Uuid, result: Result<Snapshot, Error>) -> bool {
        if self.pending != Some(request) {
            return false;
        }
        self.pending = None;
        let operation = self.operation.take();
        match result {
            Ok(snapshot) => {
                self.read_ready = matches!(operation, Some(Request::AutomaticRead));
                if snapshot.selection.is_none() || matches!(operation, Some(Request::Revoke)) {
                    // Also cancel local edits made while revocation was in flight.
                    self.invalidate_intents();
                }
                self.snapshot = Some(snapshot);
                self.error = None;
            }
            Err(error) => {
                // Lost replies and conflicts never authorize an automatic retry.
                self.invalidate_intents();
                self.snapshot = None;
                self.error = Some(error);
            }
        }
        true
    }
}

fn relevant(change: PreferenceChange) -> bool {
    matches!(
        change,
        PreferenceChange::Worldview(_) | PreferenceChange::Artwork(_, _)
    )
}

impl GUI {
    fn worldview_binding(&self) -> Option<Binding> {
        let mut origin = self.overlay_origin(None)?;
        origin.focus = None;
        self.worldview_binding_for(&origin)
    }

    fn worldview_binding_for(&self, origin: &OverlayOrigin) -> Option<Binding> {
        let tab = self
            .panes
            .get(origin.pane)?
            .tabs
            .iter()
            .find(|tab| tab.id == origin.tab_id)?;
        if tab.preferences_source() != Some(origin.source) {
            return None;
        }
        let account = self.connect_panel(origin)?;
        if !account.is_authenticated() {
            return None;
        }
        Some(Binding {
            origin: origin.clone(),
            context: (
                account.user.as_ref()?.id,
                account.session_generation(),
                account.auth_epoch(),
            ),
        })
    }

    #[cfg(test)]
    fn worldview_context(&self) -> Option<Context> {
        self.worldview_binding().map(|binding| binding.context)
    }

    fn worldview_intent(&self) -> Option<Intent> {
        let binding = self.worldview_binding()?;
        Some(Intent {
            binding,
            epoch: self.worldview_sharing.epoch,
        })
    }

    pub(super) fn worldview_record_save(&mut self, request: &SaveRequest) {
        if let Operation::Save(change) = request.operation {
            if relevant(change) {
                let intent = self.worldview_intent();
                self.worldview_sharing.save = Some((request.clone(), intent));
            }
        }
    }

    pub(super) fn worldview_queue_save(&mut self, change: PreferenceChange) {
        if relevant(change) {
            let intent = self.worldview_intent();
            self.worldview_sharing.queued.push_back((change, intent));
        }
    }

    pub(super) fn worldview_start_queued_save(&mut self, request: &SaveRequest) {
        if let Operation::Save(change) = request.operation {
            if relevant(change) {
                let queued = self.worldview_sharing.queued.pop_front();
                let intent = queued
                    .filter(|(expected, _)| *expected == change)
                    .and_then(|(_, intent)| intent);
                self.worldview_sharing.save = Some((request.clone(), intent));
            }
        }
    }

    pub(super) fn worldview_saved(
        &mut self,
        request: &SaveRequest,
        applied: bool,
        before: Option<DevicePreferences>,
    ) {
        if !self
            .worldview_sharing
            .save
            .as_ref()
            .is_some_and(|(pending, _)| pending == request)
        {
            return;
        }
        let (_, intent) = self.worldview_sharing.save.take().unwrap();
        let Some(intent) = intent else {
            return;
        };
        if !applied
            || intent.epoch != self.worldview_sharing.epoch
            || self.worldview_binding_for(&intent.binding.origin).as_ref() != Some(&intent.binding)
        {
            return;
        }
        let Some(after) = self
            .preferences
            .accepted()
            .map(|accepted| accepted.preferences)
        else {
            return;
        };
        let selection = Selection::from(after);
        if before.map(Selection::from) != Some(selection) {
            // An attempted edit cannot displace a previously accepted intent.
            // Rebind only when the new active pair has actually been saved.
            self.worldview_sharing.bind(&intent.binding);
            // Keep reverts too: the cached snapshot can predate an in-flight PUT.
            self.worldview_sharing.desired = Some(selection);
        }
    }

    pub(super) fn worldview_view(&self) -> preferences_view::WorldviewSharing {
        let Some(binding) = self.worldview_binding() else {
            return preferences_view::WorldviewSharing {
                status: copy("Sign in to Connect to share with COINCUBE Mobile."),
                ..Default::default()
            };
        };
        let sharing = &self.worldview_sharing;
        let same = sharing.binding.as_ref() == Some(&binding);
        let pending = sharing.in_flight.is_some();
        let snapshot = same.then_some(sharing.snapshot.as_ref()).flatten();
        let error = same.then_some(sharing.error).flatten();
        let status = if pending {
            copy("Checking or updating the shared selection…")
        } else if let Some(error) = error {
            match error {
            Error::Conflict => copy("The shared selection changed. Refresh, then choose Share or Stop sharing again."),
            Error::Locked => copy("Sharing changes are unavailable while Connect is locked."),
            Error::Unavailable => copy("Couldn’t confirm the shared selection. Refresh before trying again."),
        }
        } else if snapshot.is_some_and(|value| value.selection.is_some()) {
            copy("Shared with Mobile")
        } else if snapshot.is_some() {
            copy("No Worldview selection is shared with COINCUBE Mobile.")
        } else {
            copy("Refresh to check the selection shared with COINCUBE Mobile.")
        };
        let ready = !self.preferences.is_busy()
            && !self.preferences.has_queued_changes()
            && self.preferences.accepted().is_some();
        preferences_view::WorldviewSharing {
            status,
            shared_selection: snapshot
                .and_then(|value| value.selection)
                .map(selection_names),
            can_refresh: !pending
                && !self.preferences.is_busy()
                && !self.preferences.has_queued_changes(),
            can_publish: !pending && ready && snapshot.is_some(),
            can_revoke: !pending && snapshot.is_some_and(|value| value.selection.is_some()),
        }
    }

    pub(super) fn worldview_action(&mut self, action: Action) -> Task<Message> {
        let Some(binding) = self.worldview_binding() else {
            return Task::none();
        };
        let sharing = &self.worldview_sharing;
        let current_read_or_write = sharing.in_flight.as_ref().is_some_and(|(owner, id)| {
            owner == &binding
                && sharing.binding.as_ref() == Some(&binding)
                && sharing.pending == Some(*id)
        });
        if action == Action::Refresh
            && !current_read_or_write
            && (self.preferences.is_busy()
                || self.preferences.has_queued_changes()
                || sharing.in_flight.is_some())
        {
            // Opening Settings during a local read must eventually show status,
            // without canceling an accepted edit or another origin's request.
            self.worldview_sharing.deferred_refresh = Some(binding);
            return Task::none();
        }
        let view = self.worldview_view();
        let operation = match action {
            Action::Refresh if view.can_refresh => Request::Read,
            Action::Publish if view.can_publish => {
                let Some(saved) = self.preferences.accepted() else {
                    return Task::none();
                };
                Request::Publish(Selection::from(saved.preferences))
            }
            Action::Revoke if view.can_revoke => Request::Revoke,
            _ => return Task::none(),
        };
        self.worldview_sharing.bind(&binding);
        self.worldview_sharing.deferred_refresh = None;
        // A manual refresh/revoke cannot resurrect an earlier automatic intent.
        self.worldview_sharing.invalidate_intents();
        self.worldview_start(binding, operation)
    }

    fn worldview_start(&mut self, binding: Binding, operation: Request) -> Task<Message> {
        if self.worldview_sharing.in_flight.is_some() {
            return Task::none();
        }
        let Some(client) = self
            .connect_panel(&binding.origin)
            .and_then(|panel| panel.authenticated_client())
        else {
            return Task::none();
        };
        let previous = self.worldview_sharing.snapshot.clone();
        if matches!(operation, Request::Read | Request::AutomaticRead)
            && self.worldview_sharing.deferred_refresh.as_ref() == Some(&binding)
        {
            self.worldview_sharing.deferred_refresh = None;
        }
        let request = uuid::Uuid::new_v4();
        self.worldview_sharing.pending = Some(request);
        self.worldview_sharing.in_flight = Some((binding.clone(), request));
        self.worldview_sharing.operation = Some(operation);
        self.worldview_sharing.error = None;
        Task::perform(
            async move {
                match operation {
                    Request::Read | Request::AutomaticRead => client.get_worldview().await,
                    Request::Publish(selection) => {
                        client
                            .share_worldview(&previous.ok_or(Error::Unavailable)?, Some(selection))
                            .await
                    }
                    Request::Revoke => {
                        client
                            .share_worldview(&previous.ok_or(Error::Unavailable)?, None)
                            .await
                    }
                }
            },
            move |result| {
                Message::Worldview(binding.origin.clone(), binding.context, request, result)
            },
        )
    }

    fn worldview_deferred_refresh(&mut self) -> Task<Message> {
        let Some(binding) = self.worldview_sharing.deferred_refresh.take() else {
            return Task::none();
        };
        let same_sheet = self.preferences_sheet.as_ref().is_some_and(|sheet| {
            let mut origin = sheet.origin.clone();
            origin.focus = None;
            origin == binding.origin
        });
        if !same_sheet || self.worldview_binding().as_ref() != Some(&binding) {
            return Task::none();
        }
        self.worldview_action(Action::Refresh)
    }

    pub(super) fn worldview_automatic(&mut self) -> Task<Message> {
        if self.preferences.is_busy() || self.preferences.has_queued_changes() {
            // A GET held across local work is no longer fresh enough to write.
            self.worldview_sharing.read_ready = false;
            return Task::none();
        }
        let sharing = &self.worldview_sharing;
        if sharing.in_flight.is_some() {
            return Task::none();
        }
        if sharing.error.is_some() || sharing.desired.is_none() {
            return self.worldview_deferred_refresh();
        }
        let Some(binding) = sharing.binding.clone() else {
            return Task::none();
        };
        if self.worldview_binding_for(&binding.origin).as_ref() != Some(&binding) {
            self.worldview_sharing.invalidate_intents();
            return self.worldview_deferred_refresh();
        }
        if self
            .preferences
            .accepted()
            .map(|saved| Selection::from(saved.preferences))
            != self.worldview_sharing.desired
        {
            self.worldview_sharing.invalidate_intents();
            return self.worldview_deferred_refresh();
        }
        // Re-read before every automatic write. A remotely revoked selection
        // must remain revoked, including after restart or an offline interval.
        if !sharing.read_ready {
            return self.worldview_start(binding, Request::AutomaticRead);
        }
        self.worldview_sharing.read_ready = false;
        let Some(snapshot) = self
            .worldview_sharing
            .snapshot
            .as_ref()
            .filter(|snapshot| snapshot.selection.is_some())
        else {
            self.worldview_sharing.invalidate_intents();
            return self.worldview_deferred_refresh();
        };
        let selection = self.worldview_sharing.desired.take().unwrap();
        if snapshot.selection == Some(selection) {
            return self.worldview_deferred_refresh();
        }
        self.worldview_start(binding, Request::Publish(selection))
    }

    pub(super) fn worldview_complete(
        &mut self,
        origin: OverlayOrigin,
        context: Context,
        request: uuid::Uuid,
        result: Result<Snapshot, Error>,
    ) -> Task<Message> {
        let binding = Binding { origin, context };
        if !self
            .worldview_sharing
            .in_flight
            .as_ref()
            .is_some_and(|(owner, id)| owner == &binding && *id == request)
        {
            return Task::none();
        }
        self.worldview_sharing.in_flight = None;
        if result.is_err() {
            // A queued status request predates this error; it is not explicit
            // recovery consent and must not silently clear the new pause.
            self.worldview_sharing.cancel_refresh_for(binding.context.0);
        }
        if self.worldview_sharing.binding.as_ref() != Some(&binding)
            || self.worldview_sharing.pending != Some(request)
        {
            // A new origin's edit waited for this request. It must read again
            // after success, or pause if the same account's write is uncertain.
            if let Err(error) = result {
                if self
                    .worldview_sharing
                    .binding
                    .as_ref()
                    .is_some_and(|current| current.context.0 == binding.context.0)
                {
                    self.worldview_sharing.invalidate_intents();
                    self.worldview_sharing.snapshot = None;
                    self.worldview_sharing.error = Some(error);
                }
            }
            return self.worldview_automatic();
        }
        if self.worldview_binding_for(&binding.origin).as_ref() != Some(&binding) {
            // A removed/locked origin must not strand its request on return.
            // Never clear a newer request or adopt the departed account's reply.
            if self.worldview_sharing.pending == Some(request) {
                self.worldview_sharing.invalidate_intents();
                self.worldview_sharing.pending = None;
                self.worldview_sharing.operation = None;
                self.worldview_sharing.snapshot = None;
                self.worldview_sharing.error = None;
            }
            return self.worldview_automatic();
        }
        if self.worldview_sharing.complete(request, result) {
            self.worldview_automatic()
        } else {
            Task::none()
        }
    }
}

fn selection_names(selection: Selection) -> (&'static str, &'static str) {
    use crate::app::settings::preferences::{ArtworkStyle::*, WorldviewMode::*};
    match (selection.mode, selection.artwork) {
        (Samurai, Existing) => ("Samurai", "Kage"),
        (Samurai, Scene) => ("Samurai", "Mountain Watch"),
        (Suitcoiner, Existing) => ("Suitcoiner", "Gilded Wall Street"),
        (Suitcoiner, Scene) => ("Suitcoiner", "Gilded Exchange"),
        (Luke, Existing) => ("Luke", "Purist’s Woodcut"),
        (Luke, Scene) => ("Luke", "The Stone Crossing"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_completions_cannot_replace_a_new_request() {
        let mut sharing = Sharing::default();
        let old = uuid::Uuid::new_v4();
        let new = uuid::Uuid::new_v4();
        sharing.pending = Some(new);
        sharing.complete(old, Err(Error::Locked));
        assert_eq!(sharing.pending, Some(new));
        assert_eq!(sharing.error, None);
        sharing.complete(new, Err(Error::Conflict));
        assert_eq!(sharing.pending, None);
        assert_eq!(sharing.error, Some(Error::Conflict));
        assert_eq!(sharing.snapshot, None);
    }
    #[test]
    fn account_replacement_and_duplicate_callbacks_refuse_old_results() {
        let _guard = crate::app::session::test_guard();
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let _enter = runtime.enter();
                let path = std::env::temp_dir()
                    .join(format!("worldview-sharing-{}", uuid::Uuid::new_v4()));
                std::fs::create_dir_all(&path).unwrap();
                let root = crate::dir::CoincubeDirectory::new(path.clone());
                let mut gui = super::super::app_preferences_sheet_tests::launcher_gui(&root);
                let pane_id = gui.focus.unwrap();
                let tab_id = gui.panes.get(pane_id).unwrap().tabs[0].id;
                let super::super::tab::State::Home(home) =
                    &mut gui.panes.get_mut(pane_id).unwrap().tabs[0].state
                else {
                    unreachable!()
                };
                home.connect_account.user = Some(crate::services::coincube::User {
                    id: 7,
                    email: "fixture@example.invalid".into(),
                    email_verified: Some(true),
                });
                home.connect_account.step = crate::app::state::connect::ConnectFlowStep::Dashboard;
                drop(gui.update(Message::Pane(
                    pane_id,
                    super::super::pane::Message::tab(
                        tab_id,
                        super::super::tab::Message::OpenAppPreferences(
                            super::super::tab::PreferencesSource::Home,
                        ),
                    ),
                )));
                let origin = gui.worldview_binding().unwrap().origin;
                let context = gui.worldview_context().unwrap();
                let old = gui.worldview_sharing.pending.unwrap();
                gui.worldview_sharing.snapshot = Some(Snapshot {
                    account_uuid: "82473444-c619-4c95-bc6e-ef2d9f692e8b".into(),
                    revision: 1,
                    selection: Some(Selection {
                        mode: crate::app::settings::preferences::WorldviewMode::Luke,
                        artwork: crate::app::settings::preferences::ArtworkStyle::Scene,
                    }),
                    etag: "\"old-account-selection\"".into(),
                });
                let super::super::tab::State::Home(home) =
                    &mut gui.panes.get_mut(pane_id).unwrap().tabs[0].state
                else {
                    unreachable!()
                };
                home.connect_account.user.as_mut().unwrap().id = 8;
                drop(gui.worldview_complete(origin.clone(), context, old, Err(Error::Locked)));
                assert_eq!(gui.worldview_sharing.error, None);
                assert!(!gui.worldview_view().can_publish);
                assert!(!gui.worldview_view().can_revoke);
                assert_eq!(gui.worldview_view().shared_selection, None);
                drop(gui.worldview_action(Action::Refresh));
                assert_eq!(gui.worldview_view().shared_selection, None);
                assert!(gui.worldview_sharing.snapshot.is_none());
                let active_context = gui.worldview_context().unwrap();
                let active = gui.worldview_sharing.pending.unwrap();
                drop(gui.worldview_complete(
                    origin.clone(),
                    active_context,
                    active,
                    Err(Error::Conflict),
                ));
                assert!(!gui.worldview_view().can_publish);
                assert!(gui.worldview_view().can_refresh);
                drop(gui.close_preferences_sheet());
                drop(gui.worldview_complete(origin, active_context, active, Err(Error::Locked)));
                assert!(gui.preferences_sheet.is_none());
                drop(gui);
                std::fs::remove_dir_all(path).unwrap();
            })
            .unwrap()
            .join()
            .unwrap();
    }

    fn with_gui(test: impl FnOnce(&mut GUI) + Send + 'static) {
        let _guard = crate::app::session::test_guard();
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let _enter = runtime.enter();
                let path =
                    std::env::temp_dir().join(format!("worldview-auto-{}", uuid::Uuid::new_v4()));
                std::fs::create_dir_all(&path).unwrap();
                let root = crate::dir::CoincubeDirectory::new(path.clone());
                let mut gui = super::super::app_preferences_sheet_tests::launcher_gui(&root);
                set_account(&mut gui, 7);
                test(&mut gui);
                drop(gui);
                std::fs::remove_dir_all(path).unwrap();
            })
            .unwrap()
            .join()
            .unwrap();
    }

    fn set_account(gui: &mut GUI, id: u32) {
        let pane = gui.focus.unwrap();
        let super::super::tab::State::Home(home) =
            &mut gui.panes.get_mut(pane).unwrap().tabs[0].state
        else {
            panic!("home");
        };
        home.connect_account.user = Some(crate::services::coincube::User {
            id,
            email: "fixture@example.invalid".into(),
            email_verified: Some(true),
        });
        home.connect_account.step = crate::app::state::connect::ConnectFlowStep::Dashboard;
    }

    fn selected(gui: &GUI) -> Selection {
        Selection::from(gui.preferences.accepted().unwrap().preferences)
    }

    fn snapshot(selection: Option<Selection>) -> Snapshot {
        Snapshot {
            account_uuid: "82473444-c619-4c95-bc6e-ef2d9f692e8b".into(),
            revision: 1,
            selection,
            etag: "\"test-etag\"".into(),
        }
    }

    fn shared(gui: &mut GUI) {
        let binding = gui.worldview_binding().unwrap();
        gui.worldview_sharing.bind(&binding);
        gui.worldview_sharing.snapshot = Some(snapshot(Some(selected(gui))));
    }

    fn finish_save(gui: &mut GUI, request: SaveRequest) {
        let Operation::Save(change) = request.operation else {
            panic!("save");
        };
        let receipt = super::super::GlobalSettings::save_preference(&request.root, change).unwrap();
        drop(gui.update(Message::Preferences(request, Ok(receipt))));
    }

    fn edit(gui: &mut GUI, change: PreferenceChange) {
        drop(gui.change_preference(change));
        let request = gui.worldview_sharing.save.as_ref().unwrap().0.clone();
        finish_save(gui, request);
    }

    fn remote(gui: &mut GUI, result: Result<Snapshot, Error>) {
        let (binding, request) = gui.worldview_sharing.in_flight.clone().unwrap();
        drop(gui.worldview_complete(binding.origin, binding.context, request, result));
    }

    fn another_home(gui: &mut GUI, account: Option<u32>) {
        let pane = gui.focus.unwrap();
        let mut home =
            super::super::home::Home::new_for_chain(gui.config.coincube_directory.clone(), None).0;
        if let Some(id) = account {
            home.connect_account.user = Some(crate::services::coincube::User {
                id,
                email: "fixture@example.invalid".into(),
                email_verified: Some(true),
            });
            home.connect_account.step = crate::app::state::connect::ConnectFlowStep::Dashboard;
        }
        gui.panes
            .get_mut(pane)
            .unwrap()
            .tabs
            .push(super::super::tab::Tab::new(
                2,
                super::super::tab::State::Home(home),
            ));
        gui.panes.get_mut(pane).unwrap().focused_tab = 1;
    }

    fn open_preferences(gui: &mut GUI) {
        let pane = gui.focus.unwrap();
        let state = gui.panes.get(pane).unwrap();
        let tab = state.tabs[state.focused_tab].id;
        drop(gui.update(Message::Pane(
            pane,
            super::super::pane::Message::tab(
                tab,
                super::super::tab::Message::OpenAppPreferences(
                    super::super::tab::PreferencesSource::Home,
                ),
            ),
        )));
    }

    #[test]
    fn automatic_read_waiting_for_a_save_is_rechecked_before_publication() {
        with_gui(|gui| {
            use crate::app::settings::preferences::WorldviewMode::*;
            shared(gui);
            let original = selected(gui);
            edit(gui, PreferenceChange::Worldview(Luke));
            drop(gui.change_preference(PreferenceChange::Worldview(Suitcoiner)));
            let save = gui.worldview_sharing.save.as_ref().unwrap().0.clone();
            remote(gui, Ok(snapshot(Some(original))));
            assert!(gui.worldview_sharing.in_flight.is_none());
            finish_save(gui, save);
            assert!(matches!(
                gui.worldview_sharing.operation,
                Some(Request::AutomaticRead)
            ));
            remote(gui, Ok(snapshot(None))); // Revoked while the save delayed us.
            assert!(gui.worldview_sharing.in_flight.is_none());
            assert!(gui.worldview_sharing.desired.is_none());
        });
    }

    #[test]
    fn opening_preferences_during_background_read_eventually_refreshes_sharing() {
        with_gui(|gui| {
            std::thread::sleep(std::time::Duration::from_millis(1100));
            let refresh = gui.preferences.begin_refresh_if_due().unwrap();
            open_preferences(gui);
            assert!(gui.worldview_sharing.in_flight.is_none());
            let receipt = super::super::GlobalSettings::load_or_initialize_preferences(
                &gui.config.coincube_directory,
            )
            .unwrap();
            drop(gui.update(Message::Preferences(refresh, Ok(receipt))));
            assert!(matches!(
                gui.worldview_sharing.operation,
                Some(Request::Read)
            ));
            remote(gui, Ok(snapshot(None)));
            assert!(gui.worldview_sharing.snapshot.is_some());
            assert!(gui.worldview_sharing.in_flight.is_none());
        });
    }

    #[test]
    fn deferred_status_refresh_is_dropped_after_close_navigation_or_account_change() {
        for cancellation in 0..3 {
            with_gui(move |gui| {
                std::thread::sleep(std::time::Duration::from_millis(1100));
                let refresh = gui.preferences.begin_refresh_if_due().unwrap();
                open_preferences(gui);
                match cancellation {
                    0 => drop(gui.close_preferences_sheet()),
                    1 => another_home(gui, Some(7)),
                    _ => set_account(gui, 8),
                }
                let receipt = super::super::GlobalSettings::load_or_initialize_preferences(
                    &gui.config.coincube_directory,
                )
                .unwrap();
                drop(gui.update(Message::Preferences(refresh, Ok(receipt))));
                assert!(gui.worldview_sharing.in_flight.is_none());
                assert!(gui.worldview_sharing.deferred_refresh.is_none());
            });
        }
    }

    #[test]
    fn deferred_status_refresh_does_not_cancel_an_accepted_queued_edit() {
        with_gui(|gui| {
            use crate::app::settings::preferences::WorldviewMode::*;
            shared(gui);
            let original = selected(gui);
            std::thread::sleep(std::time::Duration::from_millis(1100));
            let refresh = gui.preferences.begin_refresh_if_due().unwrap();
            open_preferences(gui);
            drop(gui.change_preference(PreferenceChange::Worldview(Luke)));
            let receipt = super::super::GlobalSettings::load_or_initialize_preferences(
                &gui.config.coincube_directory,
            )
            .unwrap();
            drop(gui.update(Message::Preferences(refresh, Ok(receipt))));
            let save = gui.worldview_sharing.save.as_ref().unwrap().0.clone();
            finish_save(gui, save);
            assert!(matches!(
                gui.worldview_sharing.operation,
                Some(Request::AutomaticRead)
            ));
            assert!(gui.worldview_sharing.deferred_refresh.is_none());
            remote(gui, Ok(snapshot(Some(original))));
            let latest = selected(gui);
            assert!(
                matches!(gui.worldview_sharing.operation, Some(Request::Publish(value)) if value == latest)
            );
            remote(gui, Ok(snapshot(Some(latest))));
            assert!(gui.worldview_sharing.in_flight.is_none());
        });
    }

    #[test]
    fn another_origins_status_refresh_waits_and_cannot_clear_a_new_same_account_error() {
        for failure in [None, Some(Error::Conflict), Some(Error::Unavailable)] {
            with_gui(move |gui| {
                use crate::app::settings::preferences::WorldviewMode::*;
                shared(gui);
                let original = selected(gui);
                edit(gui, PreferenceChange::Worldview(Luke));
                let first = gui.worldview_sharing.in_flight.clone();
                another_home(gui, Some(7));
                open_preferences(gui);
                assert_eq!(gui.worldview_sharing.in_flight, first);
                assert!(gui.worldview_sharing.deferred_refresh.is_some());
                if failure == Some(Error::Conflict) {
                    remote(gui, Err(Error::Conflict));
                } else {
                    remote(gui, Ok(snapshot(Some(original))));
                    assert!(matches!(
                        gui.worldview_sharing.operation,
                        Some(Request::Publish(_))
                    ));
                    remote(
                        gui,
                        failure.map_or_else(|| Ok(snapshot(Some(selected(gui)))), Err),
                    );
                }
                if let Some(error) = failure {
                    assert_eq!(gui.worldview_sharing.error, Some(error));
                    assert!(gui.worldview_sharing.in_flight.is_none());
                    assert!(gui.worldview_sharing.deferred_refresh.is_none());
                } else {
                    assert!(matches!(
                        gui.worldview_sharing.operation,
                        Some(Request::Read)
                    ));
                    assert_eq!(gui.worldview_sharing.binding, gui.worldview_binding());
                    remote(gui, Ok(snapshot(Some(selected(gui)))));
                    assert!(gui.worldview_sharing.in_flight.is_none());
                    assert!(gui.worldview_view().shared_selection.is_some());
                }
            });
        }
    }

    #[test]
    fn deferred_status_for_another_account_survives_an_unrelated_account_error() {
        with_gui(|gui| {
            use crate::app::settings::preferences::WorldviewMode::*;
            shared(gui);
            edit(gui, PreferenceChange::Worldview(Luke));
            another_home(gui, Some(8));
            open_preferences(gui);
            remote(gui, Err(Error::Unavailable));
            assert!(matches!(
                gui.worldview_sharing.operation,
                Some(Request::Read)
            ));
            assert_eq!(gui.worldview_sharing.binding.as_ref().unwrap().context.0, 8);
            assert!(gui.worldview_sharing.desired.is_none());
            remote(gui, Ok(snapshot(None)));
            assert!(gui.worldview_sharing.in_flight.is_none());
        });
    }

    #[test]
    fn automatically_publishes_all_six_accepted_pairs_without_another_share_click() {
        with_gui(|gui| {
            use crate::app::settings::preferences::{ArtworkStyle::*, WorldviewMode::*};
            edit(gui, PreferenceChange::Worldview(Luke));
            remote(gui, Ok(snapshot(None))); // Sharing was never enabled.
            shared(gui);
            for (mode, artwork) in [
                (Samurai, Existing),
                (Samurai, Scene),
                (Suitcoiner, Existing),
                (Suitcoiner, Scene),
                (Luke, Existing),
                (Luke, Scene),
            ] {
                let previous = gui.worldview_sharing.snapshot.clone().unwrap();
                edit(gui, PreferenceChange::Worldview(mode));
                edit(gui, PreferenceChange::Artwork(mode, artwork));
                let expected = Selection { mode, artwork };
                assert_eq!(gui.worldview_sharing.desired, Some(expected));
                assert!(matches!(
                    gui.worldview_sharing.operation,
                    Some(Request::AutomaticRead)
                ));
                remote(gui, Ok(previous));
                assert!(
                    matches!(gui.worldview_sharing.operation, Some(Request::Publish(value)) if value == expected)
                );
                remote(gui, Ok(snapshot(Some(expected))));
                assert!(gui.worldview_sharing.pending.is_none());
                assert!(gui.worldview_sharing.desired.is_none());
                assert_eq!(
                    gui.worldview_sharing.snapshot.as_ref().unwrap().selection,
                    Some(expected)
                );
            }
        });
    }

    #[test]
    fn no_sharing_noop_inactive_artwork_and_failed_saves_never_publish() {
        with_gui(|gui| {
            use crate::app::settings::preferences::{ArtworkStyle::*, WorldviewMode::*};
            edit(gui, PreferenceChange::Worldview(Samurai));
            assert!(gui.worldview_sharing.pending.is_none());
            edit(gui, PreferenceChange::Artwork(Luke, Existing));
            assert!(gui.worldview_sharing.pending.is_none());
            edit(gui, PreferenceChange::Artwork(Samurai, Scene));
            remote(gui, Ok(snapshot(None)));
            assert!(gui.worldview_sharing.pending.is_none());
            shared(gui);
            drop(gui.change_preference(PreferenceChange::Worldview(Luke)));
            let request = gui.worldview_sharing.save.as_ref().unwrap().0.clone();
            drop(gui.update(Message::Preferences(request, Err("disk full".into()))));
            assert!(gui.worldview_sharing.pending.is_none());
            assert!(gui.worldview_sharing.desired.is_none());
        });
    }

    #[test]
    fn reverting_during_a_write_keeps_the_latest_accepted_pair() {
        with_gui(|gui| {
            use crate::app::settings::preferences::WorldviewMode::*;
            shared(gui);
            let original = selected(gui);
            edit(gui, PreferenceChange::Worldview(Luke));
            let interim = selected(gui);
            remote(gui, Ok(snapshot(Some(original))));
            assert!(
                matches!(gui.worldview_sharing.operation, Some(Request::Publish(value)) if value == interim)
            );
            edit(gui, PreferenceChange::Worldview(Samurai));
            assert_eq!(gui.worldview_sharing.desired, Some(original));
            remote(gui, Ok(snapshot(Some(interim))));
            assert!(matches!(
                gui.worldview_sharing.operation,
                Some(Request::AutomaticRead)
            ));
            remote(gui, Ok(snapshot(Some(interim))));
            assert!(
                matches!(gui.worldview_sharing.operation, Some(Request::Publish(value)) if value == original)
            );
            remote(gui, Ok(snapshot(Some(original))));
            assert!(gui.worldview_sharing.pending.is_none());
        });
    }

    #[test]
    fn stop_sharing_cancels_edits_before_and_during_revocation() {
        with_gui(|gui| {
            use crate::app::settings::preferences::WorldviewMode::*;
            shared(gui);
            drop(gui.change_preference(PreferenceChange::Worldview(Luke)));
            let save = gui.worldview_sharing.save.as_ref().unwrap().0.clone();
            drop(gui.worldview_action(Action::Revoke));
            finish_save(gui, save);
            assert!(gui.worldview_sharing.desired.is_none());
            edit(gui, PreferenceChange::Worldview(Suitcoiner));
            assert!(gui.worldview_sharing.desired.is_some());
            remote(gui, Ok(snapshot(None)));
            assert!(gui.worldview_sharing.desired.is_none());
            assert!(gui.worldview_sharing.pending.is_none());
            edit(gui, PreferenceChange::Worldview(Samurai));
            remote(gui, Ok(snapshot(None)));
            assert!(gui.worldview_sharing.pending.is_none());
        });
    }

    #[test]
    fn account_changed_during_disk_save_cannot_receive_an_old_accounts_edit() {
        with_gui(|gui| {
            use crate::app::settings::preferences::WorldviewMode::*;
            shared(gui);
            drop(gui.change_preference(PreferenceChange::Worldview(Luke)));
            let save = gui.worldview_sharing.save.as_ref().unwrap().0.clone();
            set_account(gui, 8);
            finish_save(gui, save);
            assert!(gui.worldview_sharing.desired.is_none());
            assert!(gui.worldview_sharing.pending.is_none());
            edit(gui, PreferenceChange::Worldview(Suitcoiner));
            assert_eq!(gui.worldview_sharing.binding.as_ref().unwrap().context.0, 8);
            assert!(matches!(
                gui.worldview_sharing.operation,
                Some(Request::AutomaticRead)
            ));
        });
    }

    #[test]
    fn conflict_locked_and_uncertain_replies_require_explicit_recovery() {
        with_gui(|gui| {
            use crate::app::settings::preferences::WorldviewMode::*;
            shared(gui);
            for (mode, error) in [
                (Luke, Error::Conflict),
                (Suitcoiner, Error::Unavailable),
                (Samurai, Error::Locked),
            ] {
                edit(gui, PreferenceChange::Worldview(mode));
                remote(gui, Err(error));
                assert_eq!(gui.worldview_sharing.error, Some(error));
                assert!(gui.worldview_sharing.desired.is_none());
                drop(gui.worldview_action(Action::Refresh));
                remote(gui, Ok(snapshot(Some(selected(gui)))));
                assert!(gui.worldview_sharing.pending.is_none());
            }
        });
    }

    #[test]
    fn background_refresh_keeps_the_original_fifo_save_bindings() {
        with_gui(|gui| {
            use crate::app::settings::preferences::{ArtworkStyle::*, WorldviewMode::*};
            std::thread::sleep(std::time::Duration::from_millis(1100));
            let refresh = gui.preferences.begin_refresh_if_due().unwrap();
            drop(gui.change_preference(PreferenceChange::Worldview(Luke)));
            drop(gui.change_preference(PreferenceChange::Artwork(Luke, Existing)));
            assert_eq!(gui.worldview_sharing.queued.len(), 2);
            set_account(gui, 8);
            let receipt = super::super::GlobalSettings::load_or_initialize_preferences(
                &gui.config.coincube_directory,
            )
            .unwrap();
            drop(gui.update(Message::Preferences(refresh, Ok(receipt))));
            for _ in 0..2 {
                let save = gui.worldview_sharing.save.as_ref().unwrap().0.clone();
                finish_save(gui, save);
                assert!(gui.worldview_sharing.pending.is_none());
                assert!(gui.worldview_sharing.desired.is_none());
            }
            assert!(gui.worldview_sharing.queued.is_empty());
        });
    }

    #[test]
    fn closing_and_reopening_preferences_keeps_an_authorized_publication() {
        with_gui(|gui| {
            use crate::app::settings::preferences::WorldviewMode::*;
            shared(gui);
            let original = selected(gui);
            edit(gui, PreferenceChange::Worldview(Luke));
            let pane = gui.focus.unwrap();
            let tab = gui.panes.get(pane).unwrap().tabs[0].id;
            let open = |gui: &mut GUI| {
                drop(gui.update(Message::Pane(
                    pane,
                    super::super::pane::Message::tab(
                        tab,
                        super::super::tab::Message::OpenAppPreferences(
                            super::super::tab::PreferencesSource::Home,
                        ),
                    ),
                )))
            };
            let request = gui.worldview_sharing.pending;
            open(gui);
            assert_eq!(gui.worldview_sharing.pending, request);
            drop(gui.close_preferences_sheet());
            remote(gui, Ok(snapshot(Some(original))));
            assert!(matches!(
                gui.worldview_sharing.operation,
                Some(Request::Publish(_))
            ));
            let request = gui.worldview_sharing.pending;
            open(gui);
            assert_eq!(gui.worldview_sharing.pending, request);
            drop(gui.close_preferences_sheet());
            remote(gui, Ok(snapshot(Some(selected(gui)))));
            assert!(gui.worldview_sharing.pending.is_none());
            assert!(gui.preferences_sheet.is_none());
        });
    }

    #[test]
    fn navigation_during_save_and_network_updates_keeps_the_original_account() {
        with_gui(|gui| {
            use crate::app::settings::preferences::WorldviewMode::*;
            shared(gui);
            let original = selected(gui);
            drop(gui.change_preference(PreferenceChange::Worldview(Luke)));
            let save = gui.worldview_sharing.save.as_ref().unwrap().0.clone();
            let pane = gui.focus.unwrap();
            let home = super::super::home::Home::new_for_chain(
                gui.config.coincube_directory.clone(),
                None,
            )
            .0;
            gui.panes
                .get_mut(pane)
                .unwrap()
                .tabs
                .push(super::super::tab::Tab::new(
                    2,
                    super::super::tab::State::Home(home),
                ));
            gui.panes.get_mut(pane).unwrap().focused_tab = 1;
            assert!(gui.worldview_binding().is_none()); // The visible tab is signed out.
            finish_save(gui, save);
            remote(gui, Ok(snapshot(Some(original))));
            let luke = selected(gui);
            assert!(
                matches!(gui.worldview_sharing.operation, Some(Request::Publish(value)) if value == luke)
            );
            remote(gui, Ok(snapshot(Some(luke))));
            assert!(gui.worldview_sharing.pending.is_none());
            gui.panes.get_mut(pane).unwrap().focused_tab = 0;
            edit(gui, PreferenceChange::Worldview(Suitcoiner));
            gui.panes.get_mut(pane).unwrap().focused_tab = 1;
            remote(gui, Ok(snapshot(Some(luke))));
            let suitcoiner = selected(gui);
            assert!(
                matches!(gui.worldview_sharing.operation, Some(Request::Publish(value)) if value == suitcoiner)
            );
            remote(gui, Ok(snapshot(Some(suitcoiner))));
            assert!(gui.worldview_sharing.pending.is_none());
            assert_eq!(gui.worldview_sharing.binding.as_ref().unwrap().context.0, 7);
            gui.panes.get_mut(pane).unwrap().focused_tab = 0;
            assert!(gui.worldview_view().can_refresh);
        });
    }

    #[test]
    fn edits_from_another_authenticated_origin_wait_for_the_in_flight_write() {
        for (failure, round_trip) in [
            (None, false),
            (None, true),
            (Some(Error::Unavailable), false),
            (Some(Error::Unavailable), true),
        ] {
            with_gui(move |gui| {
                use crate::app::settings::preferences::WorldviewMode::*;
                shared(gui);
                let original = selected(gui);
                edit(gui, PreferenceChange::Worldview(Luke));
                remote(gui, Ok(snapshot(Some(original))));
                let luke = selected(gui);
                let first = gui.worldview_sharing.in_flight.clone();
                another_home(gui, Some(7));
                edit(gui, PreferenceChange::Worldview(Suitcoiner));
                if round_trip {
                    let pane = gui.focus.unwrap();
                    gui.panes.get_mut(pane).unwrap().focused_tab = 0;
                    edit(gui, PreferenceChange::Worldview(Samurai));
                }
                assert_eq!(gui.worldview_sharing.in_flight, first);
                assert!(gui.worldview_sharing.pending.is_none());
                assert!(!gui.worldview_view().can_refresh);
                assert!(!gui.worldview_view().can_publish);
                assert!(!gui.worldview_view().can_revoke);
                if let Some(error) = failure {
                    remote(gui, Err(error));
                    assert_eq!(gui.worldview_sharing.error, Some(error));
                    assert!(gui.worldview_sharing.desired.is_none());
                    assert!(gui.worldview_sharing.in_flight.is_none());
                    let pane = gui.focus.unwrap();
                    gui.panes.get_mut(pane).unwrap().focused_tab = usize::from(round_trip);
                    edit(gui, PreferenceChange::Worldview(Luke));
                    assert_eq!(gui.worldview_sharing.error, Some(error));
                    assert!(gui.worldview_sharing.in_flight.is_none());
                } else {
                    remote(gui, Ok(snapshot(Some(luke))));
                    assert!(matches!(
                        gui.worldview_sharing.operation,
                        Some(Request::AutomaticRead)
                    ));
                    remote(gui, Ok(snapshot(Some(luke))));
                    let latest = selected(gui);
                    assert!(
                        matches!(gui.worldview_sharing.operation, Some(Request::Publish(value)) if value == latest)
                    );
                    remote(gui, Ok(snapshot(Some(latest))));
                    assert!(gui.worldview_sharing.in_flight.is_none());
                    assert_eq!(
                        gui.worldview_sharing.snapshot.as_ref().unwrap().selection,
                        Some(latest)
                    );
                }
            });
        }
    }

    #[test]
    fn unsuccessful_or_inactive_edits_in_another_origin_preserve_the_accepted_intent() {
        for kind in 0..3 {
            with_gui(move |gui| {
                use crate::app::settings::preferences::{ArtworkStyle::*, WorldviewMode::*};
                shared(gui);
                let original = selected(gui);
                edit(gui, PreferenceChange::Worldview(Luke));
                let binding = gui.worldview_sharing.binding.clone();
                let accepted = selected(gui);
                another_home(gui, Some(7));
                match kind {
                    0 => edit(gui, PreferenceChange::Worldview(Luke)), // No-op.
                    1 => edit(gui, PreferenceChange::Artwork(Samurai, Scene)),
                    _ => {
                        drop(gui.change_preference(PreferenceChange::Worldview(Suitcoiner)));
                        let request = gui.worldview_sharing.save.as_ref().unwrap().0.clone();
                        drop(gui.update(Message::Preferences(request, Err("disk full".into()))));
                    }
                }
                assert_eq!(gui.worldview_sharing.binding, binding);
                assert_eq!(gui.worldview_sharing.desired, Some(accepted));
                remote(gui, Ok(snapshot(Some(original))));
                assert!(
                    matches!(gui.worldview_sharing.operation, Some(Request::Publish(value)) if value == accepted)
                );
                remote(gui, Ok(snapshot(Some(accepted))));
                assert!(gui.worldview_sharing.in_flight.is_none());
            });
        }
    }

    #[test]
    fn departed_origins_release_their_request_without_clearing_a_replacement() {
        with_gui(|gui| {
            use crate::app::settings::preferences::WorldviewMode::*;
            shared(gui);
            edit(gui, PreferenceChange::Worldview(Luke));
            let departed = gui.worldview_sharing.binding.clone().unwrap();
            let old = gui.worldview_sharing.pending.unwrap();
            set_account(gui, 8);
            drop(gui.worldview_complete(
                departed.origin.clone(),
                departed.context,
                old,
                Ok(snapshot(Some(selected(gui)))),
            ));
            assert!(gui.worldview_sharing.pending.is_none());
            assert!(gui.worldview_sharing.snapshot.is_none());
            set_account(gui, 7);
            assert!(gui.worldview_view().can_refresh);
            edit(gui, PreferenceChange::Worldview(Suitcoiner));
            assert!(gui.worldview_sharing.pending.is_some());
            // A replacement binding owns its own request, even if an old
            // callback arrives after navigation/account replacement.
            set_account(gui, 8);
            edit(gui, PreferenceChange::Worldview(Samurai));
            let replacement = gui.worldview_sharing.pending;
            drop(gui.worldview_complete(
                departed.origin,
                departed.context,
                old,
                Err(Error::Locked),
            ));
            assert_eq!(gui.worldview_sharing.pending, replacement);
            assert_eq!(gui.worldview_sharing.error, None);
        });
    }

    #[test]
    fn enabled_account_is_discovered_after_restart_and_only_the_latest_queued_pair_is_sent() {
        with_gui(|gui| {
            use crate::app::settings::preferences::{ArtworkStyle::*, WorldviewMode::*};
            let original = selected(gui);
            std::thread::sleep(std::time::Duration::from_millis(1100));
            let refresh = gui.preferences.begin_refresh_if_due().unwrap();
            for change in [
                PreferenceChange::Worldview(Luke),
                PreferenceChange::Artwork(Luke, Existing),
                PreferenceChange::Artwork(Luke, Scene),
                PreferenceChange::Artwork(Luke, Existing),
            ] {
                drop(gui.change_preference(change));
            }
            let receipt = super::super::GlobalSettings::load_or_initialize_preferences(
                &gui.config.coincube_directory,
            )
            .unwrap();
            drop(gui.update(Message::Preferences(refresh, Ok(receipt))));
            for index in 0..4 {
                let save = gui.worldview_sharing.save.as_ref().unwrap().0.clone();
                finish_save(gui, save);
                if index < 3 {
                    assert!(gui.worldview_sharing.pending.is_none());
                }
            }
            let expected = Selection {
                mode: Luke,
                artwork: Existing,
            };
            assert_eq!(gui.worldview_sharing.desired, Some(expected));
            assert!(matches!(
                gui.worldview_sharing.operation,
                Some(Request::AutomaticRead)
            ));
            remote(gui, Ok(snapshot(Some(original))));
            assert!(
                matches!(gui.worldview_sharing.operation, Some(Request::Publish(value)) if value == expected)
            );
            remote(gui, Ok(snapshot(Some(expected))));
            assert!(gui.worldview_sharing.pending.is_none());
        });
    }
}
