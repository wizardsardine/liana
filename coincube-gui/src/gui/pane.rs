use coincube_ui::{
    color,
    component::text::*,
    icon::{cross_icon, plus_icon},
    theme,
    widget::*,
};
use iced::{Length, Subscription, Task};
use iced_aw::ContextMenu;

use crate::{app, gui::Config};

use super::tab;

#[derive(Debug)]
pub enum Message {
    Tab(usize, Box<tab::Message>),
    View(ViewMessage),
    StartClaimInstaller(usize),
    CopyInstallFinished {
        tab: usize,
        owner: app::cache::AppGeneration,
        created: bool,
    },
}

#[cfg(test)]
mod launcher_delete_tests {
    use super::*;
    use crate::{chain::ChainId, dir::CoincubeDirectory, home};

    /// The GUI's lock path asks the Launcher for the Cube's delete
    /// confirmation on the Cube's network, not the one it is listing.
    #[test]
    fn focus_launcher_to_delete_names_the_cubes_network() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _enter = runtime.enter();
        let path = std::env::temp_dir().join(format!("launcher-delete-{}", uuid::Uuid::new_v4()));
        let root = CoincubeDirectory::new(path.clone());
        let (home, startup) = home::Home::new_for_chain(root.clone(), Some(ChainId::Signet));
        drop(startup);
        let mut pane = Pane::new_with_tab(tab::State::Home(home));
        let task = pane.focus_launcher_to_delete(
            "unfinished".into(),
            ChainId::Bitcoin,
            &Config::new(root, None),
        );
        let mut asked = None;
        if let Some(stream) = iced_runtime::task::into_stream(task) {
            use iced_runtime::futures::futures::StreamExt;
            let mut stream = Box::pin(stream);
            let _ = runtime.block_on(tokio::time::timeout(
                std::time::Duration::from_secs(10),
                async {
                    while let Some(action) = stream.next().await {
                        if let iced_runtime::Action::Output(Message::Tab(_, inner)) = action {
                            if let tab::Message::Launch(home::Message::View(
                                home::ViewMessage::DeleteCube(
                                    home::DeleteCubeMessage::ShowModalFor { cube_id, chain },
                                ),
                            )) = *inner
                            {
                                asked = Some((cube_id, chain));
                                break;
                            }
                        }
                    }
                },
            ));
        }
        assert_eq!(asked, Some(("unfinished".to_string(), ChainId::Bitcoin)));
        drop(pane);
        std::fs::remove_dir_all(path).ok();
    }
}

#[cfg(test)]
mod connect_notice_tests {
    use super::*;
    use crate::{app::SignedOutBy, dir::CoincubeDirectory, home};

    #[test]
    fn a_new_tab_shows_the_signed_out_notice_its_siblings_show() {
        let _guard = app::session::test_guard();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _enter = runtime.enter();
        let path = std::env::temp_dir().join(format!("connect-notice-{}", uuid::Uuid::new_v4()));
        let root = CoincubeDirectory::new(path.clone());
        let (first, startup) = home::Home::new_for_chain(root.clone(), None);
        drop(startup);
        let mut pane = Pane::new_with_tab(tab::State::Home(first));
        pane.set_connect_signed_out_by(Some(SignedOutBy::SessionEnded));

        drop(pane.add_tab(&Config::new(root.clone(), None)));
        let added = pane.tabs.last().unwrap();
        assert_eq!(added.connect_signed_out_by, Some(SignedOutBy::SessionEnded));
        if let tab::State::Home(home) = &added.state {
            assert_eq!(
                home.connect_account.signed_out_by,
                Some(SignedOutBy::SessionEnded)
            );
        }

        pane.set_connect_signed_out_by(None);
        drop(pane.add_tab(&Config::new(root, None)));
        assert_eq!(pane.tabs.last().unwrap().connect_signed_out_by, None);
        drop(pane);
        std::fs::remove_dir_all(path).ok();
    }
}

#[cfg(test)]
mod copy_owner_tests {
    use super::*;
    use crate::{dir::CoincubeDirectory, home};

    fn home_tab(id: usize, root: &CoincubeDirectory) -> tab::Tab {
        let (home, startup) = home::Home::new_for_chain(root.clone(), None);
        drop(startup);
        tab::Tab::new(id, tab::State::Home(home))
    }

    fn copy_tab(id: usize, root: &CoincubeDirectory, owner: app::cache::AppGeneration) -> tab::Tab {
        let mut tab = home_tab(id, root);
        tab.set_copy_owner(owner);
        tab
    }

    #[test]
    fn closing_source_app_closes_only_its_copy_installer_tab() {
        let _guard = app::session::test_guard();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _enter = runtime.enter();
        let path = std::env::temp_dir().join(format!("copy-close-{}", uuid::Uuid::new_v4()));
        let root = CoincubeDirectory::new(path.clone());
        let (source, _) = app::shell_tests::bitcoin_app(&path);
        let owner = source.cache().app_generation;
        let mut pane = Pane::new_with_tab(tab::State::App(source));
        pane.tabs.push(copy_tab(2, &root, owner));
        pane.tabs.push(home_tab(3, &root));
        assert_eq!(pane.tabs[1].copy_owner(), Some(owner));
        assert_eq!(pane.tabs[2].copy_owner(), None);

        pane.close_tab(0);
        assert_eq!(pane.tabs.len(), 1);
        assert_eq!(pane.tabs[0].id, 3, "unrelated Home tab survives");
        drop(pane);
        std::fs::remove_dir_all(path).ok();
    }

    #[test]
    fn a_finished_copy_install_focuses_only_the_live_matching_source_app() {
        let _guard = app::session::test_guard();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _enter = runtime.enter();
        let path = std::env::temp_dir().join(format!("copy-finish-owner-{}", uuid::Uuid::new_v4()));
        let root = CoincubeDirectory::new(path.clone());
        let (unrelated, _) = app::shell_tests::bitcoin_app(&path);
        let (source, _) = app::shell_tests::bitcoin_app(&path);
        let owner = source.cache().app_generation;
        assert_ne!(owner, unrelated.cache().app_generation);
        let mut pane = Pane::new_with_tab(tab::State::App(unrelated));
        pane.tabs.push(tab::Tab::new(2, tab::State::App(source)));
        pane.tabs.push(copy_tab(3, &root, owner));
        pane.focused_tab = 0;
        let cfg = Config::new(root.clone(), None);
        let finished = |tab, owner, created| Message::CopyInstallFinished {
            tab,
            owner,
            created,
        };

        drop(pane.update(finished(3, owner, false), &cfg));
        assert_eq!(pane.focused_tab, 1, "return to exact owner, not first App");
        assert_eq!(pane.tabs.len(), 2, "the installer tab is closed");

        pane.tabs.push(copy_tab(4, &root, owner));
        pane.focused_tab = 0;
        drop(pane.update(finished(4, owner, true), &cfg));
        assert_eq!(
            pane.focused_tab, 1,
            "a created copy returns to its source too"
        );
        assert_eq!(pane.tabs.len(), 2);

        let stale = app::cache::AppGeneration::next();
        pane.tabs.push(copy_tab(5, &root, stale));
        pane.focused_tab = 0;
        drop(pane.update(finished(5, stale, false), &cfg));
        assert_eq!(pane.focused_tab, 0, "stale owner cannot select an App");

        pane.close_tab(1);
        pane.tabs.push(home_tab(6, &root));
        pane.tabs.push(copy_tab(7, &root, owner));
        pane.focused_tab = 1;
        drop(pane.update(finished(7, owner, false), &cfg));
        assert_eq!(
            pane.focused_tab, 1,
            "closed source cannot select unrelated App"
        );
        drop(pane);
        std::fs::remove_dir_all(path).ok();
    }
}

impl Message {
    pub fn tab(id: usize, message: tab::Message) -> Self {
        Self::Tab(id, Box::new(message))
    }
}

#[derive(Debug, Clone)]
pub enum ViewMessage {
    FocusTab(usize),
    CloseTab(usize),
    SplitTab(usize),
    AddTab,
    ToggleTheme,
    GlobalSettingsChanged,
    /// Bubbled from a Cube tab whose Connect-requiring feature page
    /// emitted [`crate::app::view::Message::OpenConnectSignIn`]. The
    /// handler focuses an existing Home tab and routes it to its
    /// Connect overview so the user can sign in.
    OpenConnectSignIn,
    /// Return to the Launcher while the originating Cube tab stays open.
    OpenLauncher,
    /// Bubbled from a tab whose paid-feature locked card emitted
    /// [`crate::app::view::Message::OpenPlanBilling`]. The handler focuses an
    /// existing Home tab and routes it to Connect → Plan & Billing.
    OpenPlanBilling,
    /// Bubbled from the Home tab on its auth-success edge. `GUI::update`
    /// broadcasts a `ConnectAccountMessage::Init` to every open Cube tab, in
    /// every pane, so those panels re-check the session and swap out of
    /// their "Sign in" prompts without waiting for the user to interact.
    ConnectSignedIn,
}

pub struct Pane {
    pub tabs: Vec<tab::Tab>,

    // this is an index in the tabs array
    pub focused_tab: usize,

    // used to generate tabs ids.
    tabs_created: usize,

    // current theme mode — applied to new tabs on creation
    theme_mode: coincube_ui::theme::palette::ThemeMode,
    balance_privacy: Option<crate::app::state::preferences::BalancePrivacyChrome>,
    // the signed-out-by-Connect notice — applied to new tabs on creation
    connect_signed_out_by: Option<crate::app::SignedOutBy>,
}

impl Pane {
    fn show_app_error(&self, source_tab: usize, error: String) -> Task<Message> {
        self.tabs
            .iter()
            .find(|tab| tab.id == source_tab)
            .map_or_else(Task::none, |tab| {
                tab.emit_app_task(Task::done(app::Message::View(
                    app::view::Message::ShowError(error),
                )))
                .map(move |message| Message::tab(source_tab, message))
            })
    }

    fn open_claim_installer(&mut self, source_tab: usize) -> Task<Message> {
        let owner = self
            .tabs
            .iter()
            .find(|tab| tab.id == source_tab)
            .and_then(|tab| {
                let tab::State::App(app) = &tab.state else {
                    return None;
                };
                Some(app.cache().app_generation)
            });
        if let Some(index) = self
            .tabs
            .iter()
            .position(|tab| tab.copy_owner() == owner && owner.is_some())
        {
            self.focused_tab = index;
            return Task::none();
        }
        let prepared = self
            .tabs
            .iter()
            .find(|tab| tab.id == source_tab)
            .and_then(|tab| {
                let tab::State::App(app) = &tab.state else {
                    return None;
                };
                Some((|| {
                    let cube = app.cube_settings().clone();
                    let wallet = app
                        .wallet()
                        .ok_or_else(|| "This Cube has no Vault to claim.".to_string())?;
                    let descriptor = wallet.inheritance_descriptor().ok_or_else(|| {
                        "This imported Bitcoin wallet does not have a Vault claim policy."
                            .to_string()
                    })?;
                    let fingerprint = cube.master_signer_fingerprint.ok_or_else(|| {
                        "This Cube's master key isn't available in this session.".to_string()
                    })?;
                    let signer = app::session::unlocked_signer(&cube.id, fingerprint)
                        .ok_or_else(|| "Unlock this Cube again to start a claim.".to_string())?;
                    let source = crate::installer::ClaimSource {
                        cube: cube.clone(),
                        descriptor: descriptor.clone(),
                        signer: std::sync::Arc::new(crate::signer::Signer::new(signer)),
                    };
                    let root = app.datadir().clone();
                    let (installer, task) = crate::installer::Installer::try_new_for_chain(
                        root.clone(),
                        crate::chain::ChainId::BitcoinBlake2b,
                        crate::installer::UserFlow::ClaimBlake2b {
                            from_cube: Box::new(source),
                        },
                        true,
                        None,
                        app.breez_client(),
                        app.cube_signer(),
                        app.spark_backend(),
                        crate::app::settings::global::GlobalSettings::load_developer_mode(
                            &crate::app::settings::global::GlobalSettings::path(app.datadir()),
                        ),
                        app.authenticated_coincube_client(),
                    )
                    .map_err(|error| error.to_string())?;
                    let owner = app.cache().app_generation;
                    Ok((installer, task, owner))
                })())
            });
        let Some(prepared) = prepared else {
            return Task::none();
        };
        let (installer, task, owner) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => return self.show_app_error(source_tab, error),
        };
        self.tabs_created += 1;
        let id = self.tabs_created;
        let mut tab = tab::Tab::new(id, tab::State::Installer(Box::new(installer)));
        tab.set_copy_owner(owner);
        tab.set_theme_mode(self.theme_mode);
        tab.set_balance_privacy(self.balance_privacy);
        tab.set_connect_signed_out_by(self.connect_signed_out_by);
        self.tabs.push(tab);
        self.focused_tab = self.tabs.len() - 1;
        task.map(move |message| Message::tab(id, tab::Message::Install(message)))
    }

    pub fn new(cfg: &Config) -> (Self, Task<Message>) {
        let (state, task) = tab::State::new(cfg.coincube_directory.clone(), cfg.network);
        (
            Self {
                tabs: vec![tab::Tab::new(1, state)],
                focused_tab: 0,
                tabs_created: 1,
                theme_mode: coincube_ui::theme::palette::ThemeMode::default(),
                balance_privacy: None,
                connect_signed_out_by: None,
            },
            task.map(|msg| Message::tab(1, msg)),
        )
    }

    pub fn new_with_tab(s: tab::State) -> Self {
        Self {
            tabs: vec![tab::Tab::new(1, s)],
            focused_tab: 0,
            tabs_created: 1,
            theme_mode: coincube_ui::theme::palette::ThemeMode::default(),
            balance_privacy: None,
            connect_signed_out_by: None,
        }
    }

    fn add_tab(&mut self, cfg: &Config) -> Task<Message> {
        let (state, task) = tab::State::new(cfg.coincube_directory.clone(), cfg.network);
        self.tabs_created += 1;
        let id = self.tabs_created;
        let mut tab = tab::Tab::new(id, state);
        tab.set_theme_mode(self.theme_mode);
        tab.set_balance_privacy(self.balance_privacy);
        tab.set_connect_signed_out_by(self.connect_signed_out_by);
        self.tabs.push(tab);
        self.focused_tab = self.tabs.len() - 1;
        task.map(move |msg| Message::tab(id, msg))
    }

    /// Focus an existing Home tab (or spawn one) and route it to `nav`. Shared
    /// by the OpenConnectSignIn / OpenPlanBilling handlers, which differ only in
    /// the Connect sub-section `nav` targets.
    fn focus_home_tab_with(&mut self, nav: crate::home::Message, cfg: &Config) -> Task<Message> {
        let home_tab = self
            .tabs
            .iter()
            .enumerate()
            .find(|(_, t)| matches!(&t.state, tab::State::Home(_)))
            .map(|(i, t)| (i, t.id));
        if let Some((idx, tab_id)) = home_tab {
            self.focused_tab = idx;
            return Task::done(Message::tab(tab_id, tab::Message::Launch(nav)));
        }
        // No Home tab open — spawn one and queue the navigation against its id
        // so it lands on the target Connect section rather than the default view.
        let add_task = self.add_tab(cfg);
        let new_tab_id = self.tabs.last().map(|t| t.id);
        if let Some(tab_id) = new_tab_id {
            return Task::batch([
                add_task,
                Task::done(Message::tab(tab_id, tab::Message::Launch(nav))),
            ]);
        }
        add_task
    }

    pub(crate) fn focus_launcher(&mut self, cfg: &Config) -> Task<Message> {
        self.focus_home_tab_with(
            crate::home::Message::View(crate::home::ViewMessage::GoToSection(
                crate::home::HomeSection::Cubes,
            )),
            cfg,
        )
    }

    /// [`Self::focus_launcher`], then the Launcher's delete confirmation for
    /// `cube_id` on `chain`: an import abandoned before it finished leaves a
    /// Cube the user may not want.
    pub(crate) fn focus_launcher_to_delete(
        &mut self,
        cube_id: String,
        chain: crate::chain::ChainId,
        cfg: &Config,
    ) -> Task<Message> {
        let focus = self.focus_launcher(cfg);
        let Some(tab_id) = self
            .tabs
            .iter()
            .find(|t| matches!(&t.state, tab::State::Home(_)))
            .map(|t| t.id)
        else {
            return focus;
        };
        Task::batch([
            focus,
            Task::done(Message::tab(
                tab_id,
                tab::Message::Launch(crate::home::Message::View(
                    crate::home::ViewMessage::DeleteCube(
                        crate::home::DeleteCubeMessage::ShowModalFor { cube_id, chain },
                    ),
                )),
            )),
        ])
    }

    pub fn close_tab(&mut self, i: usize) {
        self.close_tab_with(i, false);
    }

    /// [`Self::close_tab`] for locking: daemons stop on their own threads
    /// (see [`tab::Tab::stop_in_background`]).
    pub(crate) fn close_tab_in_background(&mut self, i: usize) {
        self.close_tab_with(i, true);
    }

    fn close_tab_with(&mut self, i: usize, in_background: bool) {
        if let Some(mut tab) = self.remove_tab(i) {
            let owner = match &tab.state {
                tab::State::App(app) => Some(app.cache().app_generation),
                _ => None,
            };
            if in_background {
                tab.stop_in_background();
            } else {
                tab.stop();
            }
            if let Some(owner) = owner {
                self.close_tabs_matching_with(|tab| tab.copy_owner() == Some(owner), in_background);
            }
        }
    }

    /// `close_tab` may recursively remove an App's paired Claim tabs. Resolve
    /// each selected tab by stable ID after every close instead of retaining
    /// indices into a vector whose lower entries may have disappeared.
    pub(crate) fn close_tabs_matching(&mut self, predicate: impl Fn(&tab::Tab) -> bool) -> bool {
        self.close_tabs_matching_with(predicate, false)
    }

    /// [`Self::close_tabs_matching`] for locking.
    pub(crate) fn close_tabs_matching_in_background(
        &mut self,
        predicate: impl Fn(&tab::Tab) -> bool,
    ) -> bool {
        self.close_tabs_matching_with(predicate, true)
    }

    fn close_tabs_matching_with(
        &mut self,
        predicate: impl Fn(&tab::Tab) -> bool,
        in_background: bool,
    ) -> bool {
        let selected: Vec<usize> = self
            .tabs
            .iter()
            .filter(|tab| predicate(tab))
            .map(|tab| tab.id)
            .collect();
        let mut closed = false;
        for id in selected {
            if let Some(index) = self.tabs.iter().position(|tab| tab.id == id) {
                self.close_tab_with(index, in_background);
                closed = true;
            }
        }
        closed
    }

    pub fn remove_tab(&mut self, i: usize) -> Option<tab::Tab> {
        if i >= self.tabs.len() {
            return None;
        }
        let tab = self.tabs.remove(i);

        if self.focused_tab >= i {
            self.focused_tab = self.focused_tab.saturating_sub(1);
        }

        Some(tab)
    }

    pub fn add_tabs(&mut self, tabs: Vec<tab::Tab>, focused_tab: usize) {
        for tab in tabs {
            self.tabs_created += 1;
            let id = self.tabs_created;
            let mut new_tab = tab::Tab::new(id, tab.state);
            new_tab.set_theme_mode(self.theme_mode);
            new_tab.set_balance_privacy(self.balance_privacy);
            new_tab.set_connect_signed_out_by(self.connect_signed_out_by);
            self.tabs.push(new_tab);
        }
        if self.focused_tab + focused_tab + 1 < self.tabs.len() {
            self.focused_tab += focused_tab + 1;
        }
    }

    pub fn set_theme_mode(&mut self, mode: coincube_ui::theme::palette::ThemeMode) {
        self.theme_mode = mode;
        for tab in &mut self.tabs {
            tab.set_theme_mode(mode);
        }
    }

    pub fn set_reduce_motion(&mut self, reduce: bool) {
        for tab in &mut self.tabs {
            tab.set_reduce_motion(reduce);
        }
    }

    pub fn set_connect_signed_out_by(&mut self, reason: Option<crate::app::SignedOutBy>) {
        self.connect_signed_out_by = reason;
        for tab in &mut self.tabs {
            tab.set_connect_signed_out_by(reason);
        }
    }

    pub fn set_balance_privacy(
        &mut self,
        chrome: Option<crate::app::state::preferences::BalancePrivacyChrome>,
    ) {
        self.balance_privacy = chrome;
        for tab in &mut self.tabs {
            tab.set_balance_privacy(chrome);
        }
    }

    pub fn on_tick(&mut self) -> Task<Message> {
        Task::batch(self.tabs.iter_mut().map(|t| {
            let id = t.id;
            t.on_tick().map(move |msg| Message::tab(id, msg))
        }))
    }

    /// Helper to update a tab with an app message.
    pub fn update_tab_with_app_msg(
        &mut self,
        tab_id: usize,
        app_msg: impl Into<app::Message>,
        cfg: &Config,
    ) -> Task<Message> {
        self.update(Message::tab(tab_id, tab::Message::Run(app_msg.into())), cfg)
    }

    pub fn update(&mut self, message: Message, cfg: &Config) -> Task<Message> {
        match message {
            Message::StartClaimInstaller(source_tab) => {
                return self.open_claim_installer(source_tab);
            }
            Message::CopyInstallFinished {
                tab: temporary_tab,
                owner,
                created,
            } => {
                if let Some(index) = self.tabs.iter().position(|tab| tab.id == temporary_tab) {
                    self.close_tab(index);
                }
                let Some(index) = self.tabs.iter().position(|tab| {
                    matches!(&tab.state, tab::State::App(app)
                        if app.cache().app_generation == owner)
                }) else {
                    return Task::none();
                };
                self.focused_tab = index;
                if !created {
                    return Task::none();
                }
                // Back on Tools › Split coins, which re-reads whether the copy
                // exists, so "Create copy" is checked off.
                let source = self.tabs[index].id;
                return self.update_tab_with_app_msg(
                    source,
                    app::Message::View(app::view::Message::Menu(app::menu::Menu::Vault(
                        app::menu::VaultSubMenu::SplitCoins,
                    ))),
                    cfg,
                );
            }
            Message::Tab(id, msg) => {
                let owner = self.tabs.iter().find(|tab| tab.id == id).and_then(|tab| {
                    let tab::State::App(app) = &tab.state else {
                        return None;
                    };
                    Some(app.cache().app_generation)
                });
                let task = self
                    .tabs
                    .iter_mut()
                    .find(|tab| tab.id == id)
                    .map(|tab| tab.update(*msg));
                if let Some(owner) = owner {
                    let retained = self.tabs.iter().any(|tab| {
                        tab.id == id
                            && matches!(&tab.state, tab::State::App(app) if app.cache().app_generation == owner)
                    });
                    if !retained {
                        self.close_tabs_matching(|tab| tab.copy_owner() == Some(owner));
                    }
                }
                return task
                    .map(|task| {
                        task.then(move |msg| match msg {
                            // Bubble ToggleTheme up to pane level as a ViewMessage
                            tab::Message::GlobalSettingsChanged => {
                                Task::done(Message::View(ViewMessage::GlobalSettingsChanged))
                            }
                            tab::Message::ToggleTheme => {
                                Task::done(Message::View(ViewMessage::ToggleTheme))
                            }
                            tab::Message::StartClaimInstaller => {
                                Task::done(Message::StartClaimInstaller(id))
                            }
                            tab::Message::CopyInstallFinished { owner, created } => {
                                Task::done(Message::CopyInstallFinished {
                                    tab: id,
                                    owner,
                                    created,
                                })
                            }
                            tab::Message::OpenConnectSignIn => {
                                Task::done(Message::View(ViewMessage::OpenConnectSignIn))
                            }
                            tab::Message::OpenLauncher => {
                                Task::done(Message::View(ViewMessage::OpenLauncher))
                            }
                            tab::Message::OpenPlanBilling => {
                                Task::done(Message::View(ViewMessage::OpenPlanBilling))
                            }
                            tab::Message::ConnectSignedIn => {
                                Task::done(Message::View(ViewMessage::ConnectSignedIn))
                            }
                            other => Task::done(Message::tab(id, other)),
                        })
                    })
                    .unwrap_or(Task::none());
            }
            Message::View(ViewMessage::FocusTab(i)) => {
                if i < self.tabs.len() {
                    self.focused_tab = i;
                    let tab = &self.tabs[i];
                    if let tab::State::App(app) = &tab.state {
                        if app.can_adopt_shared_connect_session() {
                            let id = tab.id;
                            return tab
                                .emit_app_task(Task::done(app::Message::View(
                                    app::view::Message::ConnectAccount(
                                        app::view::ConnectAccountMessage::Init,
                                    ),
                                )))
                                .map(move |message| Message::tab(id, message));
                        }
                    }
                    if let tab::State::Home(home) = &tab.state {
                        let id = tab.id;
                        return home
                            .on_focus()
                            .map(move |message| Message::tab(id, tab::Message::Launch(message)));
                    }
                }
            }
            // The public tab strip is gone. Home is created only by the
            // explicit Launcher/Account/Plan routes above.
            Message::View(ViewMessage::AddTab) => {}
            Message::View(ViewMessage::CloseTab(i)) => {
                self.close_tab(i);
            }
            // handle by the pane grid update.
            Message::View(ViewMessage::SplitTab(_)) => {}
            // handled at the GUI level
            // handled at the GUI level, which re-inits every pane's Cube tabs
            Message::View(
                ViewMessage::ToggleTheme
                | ViewMessage::GlobalSettingsChanged
                | ViewMessage::ConnectSignedIn,
            ) => {}
            Message::View(ViewMessage::OpenConnectSignIn) => {
                // Focus/spawn a Home tab and land on Connect → Overview so the
                // user reaches the login form.
                let nav = crate::home::Message::View(crate::home::ViewMessage::GoToSection(
                    crate::home::HomeSection::Connect(crate::app::menu::ConnectSubMenu::Overview),
                ));
                return self.focus_home_tab_with(nav, cfg);
            }
            Message::View(ViewMessage::OpenLauncher) => {
                for tab in &mut self.tabs {
                    if let tab::State::App(app) = &mut tab.state {
                        app.dismiss_shell_disclosures();
                    }
                }
                return self.focus_launcher(cfg);
            }
            Message::View(ViewMessage::OpenPlanBilling) => {
                // Focus/spawn a Home tab and land on Connect → Plan & Billing,
                // for a paid-feature "View plans" CTA outside the Connect page
                // (e.g. Settings → Vault Recovery Alerts).
                let nav = crate::home::Message::View(crate::home::ViewMessage::GoToSection(
                    crate::home::HomeSection::Connect(
                        crate::app::menu::ConnectSubMenu::PlanBilling,
                    ),
                ));
                return self.focus_home_tab_with(nav, cfg);
            }
        }

        Task::none()
    }

    pub fn subscription(&self, foreground: bool) -> Subscription<Message> {
        let subs: Vec<Subscription<Message>> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(index, t)| {
                t.subscription(foreground && index == self.focused_tab)
                    .with(t.id)
                    .map(|(id, msg)| Message::tab(id, msg))
            })
            .collect();
        Subscription::batch(subs)
    }

    pub fn stop(&mut self) {
        self.tabs.iter_mut().for_each(|t| t.stop());
    }

    pub fn tabs_menu_view(&self) -> Element<Message> {
        let mut menu = Row::new().spacing(3);
        let tabs_len = self.tabs.len();
        for (i, tab) in self.tabs.iter().enumerate() {
            let title = tab.title();
            let title_row = if title.len() < 20 {
                Row::new().push(p1_regular(title)).push(p1_regular(
                    "                     "[..21 - title.len()].to_string(),
                ))
            } else {
                Row::new()
                    .push(p1_regular(&title[..17]))
                    .push(p1_regular("..."))
            };

            let tab_content: Element<ViewMessage> = if tabs_len > 1 {
                // Show close button when more than one tab
                Row::new()
                    .align_y(iced::Alignment::Center)
                    .push(
                        Button::new(title_row)
                            .style(if i == self.focused_tab {
                                theme::button::tab_liquid
                            } else {
                                theme::button::tab
                            })
                            .on_press(ViewMessage::FocusTab(i)),
                    )
                    .push(
                        Button::new(
                            iced::widget::Container::new(cross_icon().size(14))
                                .center_x(iced::Length::Fill)
                                .center_y(iced::Length::Fill),
                        )
                        .width(iced::Length::Fixed(28.0))
                        .height(iced::Length::Fixed(28.0))
                        .style(|_: &coincube_ui::theme::Theme, status| {
                            iced::widget::button::Style {
                                background: Some(iced::Background::Color(color::TRANSPARENT)),
                                text_color: match status {
                                    iced::widget::button::Status::Hovered => color::ORANGE,
                                    _ => color::GREY_3,
                                },
                                ..Default::default()
                            }
                        })
                        .padding(0)
                        .on_press(ViewMessage::CloseTab(i)),
                    )
                    .into()
            } else {
                // Single tab — no close button
                Button::new(title_row)
                    .style(if i == self.focused_tab {
                        theme::button::tab_liquid
                    } else {
                        theme::button::tab
                    })
                    .on_press(ViewMessage::FocusTab(i))
                    .into()
            };

            menu = menu.push(ContextMenu::new(tab_content, move || {
                Column::new()
                    .push(
                        Button::new(p1_regular("Close"))
                            .style(theme::button::secondary)
                            .on_press(ViewMessage::CloseTab(i))
                            .width(100),
                    )
                    .push(if tabs_len > 1 {
                        Some(
                            Button::new(p1_regular("Split"))
                                .style(theme::button::secondary)
                                .on_press(ViewMessage::SplitTab(i))
                                .width(100),
                        )
                    } else {
                        None
                    })
                    .into()
            }));
        }
        menu = menu.push(
            Button::new(plus_icon())
                .style(theme::button::tab)
                .on_press(ViewMessage::AddTab),
        );

        let menu: Element<ViewMessage> = menu.wrap().into();
        menu.map(Message::View)
    }

    pub(crate) fn view(
        &self,
        worldview: crate::app::settings::preferences::WorldviewMode,
    ) -> Element<Message> {
        Container::new(if let Some(t) = self.tabs.get(self.focused_tab) {
            let id = t.id;
            t.view(worldview).map(move |msg| Message::tab(id, msg))
        } else {
            Row::new().into()
        })
        .style(theme::container::background)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }
}
