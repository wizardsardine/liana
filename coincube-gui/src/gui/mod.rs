use coincube_ui::locale::copy as localized_copy;
use iced::{
    event::{self, Event},
    keyboard,
    widget::pane_grid,
    Size, Subscription, Task,
};
use iced_runtime::window;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};
use tracing::{error, info};
use tracing_subscriber::filter::LevelFilter;
extern crate serde;
extern crate serde_json;

use coincube_core::miniscript::bitcoin;
use coincube_ui::widget::{action, Element};

mod cache;
pub mod pane;
pub mod tab;
mod worldview;

use crate::{
    app::{
        cache::{FiatPrice, FiatPriceRequest},
        message::{FiatMessage as AppFiatMessage, Message as AppMessage},
        settings::global::{GlobalSettings, PreferencesReceipt, WindowConfig},
        settings::preferences::Zoom,
        state::preferences::{
            BalancePrivacyChrome, BeginSaveError, Operation, PreferencesNotice, PreferencesOwner,
            PreferencesPageState, SaveOutcome, SaveRequest,
        },
        view::{
            connect::{
                ConnectSheetMessage, ConnectSheetPage, ConnectSheetSection, CLOSE_ID,
                CONTENT_SCROLL_ID,
            },
            preferences as preferences_view,
        },
    },
    dir::CoincubeDirectory,
    gui::cache::GlobalCache,
    home,
    logger::setup_logger,
    services::fiat::{
        api::{ListCurrenciesResult, PriceApi, PriceApiError},
        Currency, PriceClient, PriceSource,
    },
};

/// A Show receipt may have gone stale while the GUI was being constructed (or
/// between later refreshes). Recheck the established choice at render time;
/// any read failure is fail-closed until the ordered refresh accepts a receipt.
fn privacy_render_needs_cover(
    preferences: &PreferencesOwner,
    settings_path: &std::path::Path,
) -> bool {
    use crate::app::state::preferences::BalancePrivacy;
    preferences.privacy() == BalancePrivacy::Shown
        && !matches!(
            GlobalSettings::try_current_balances_hidden(settings_path),
            Ok(Some(false))
        )
}

#[cfg(test)]
mod seed_export_duress_gui_tests {
    use super::*;
    use iced::futures::StreamExt;

    #[test]
    fn stale_app_callback_carries_verified_duress_after_switch() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _enter = runtime.enter();
        let _guard = crate::app::session::test_guard();
        crate::app::session::close();
        let path = std::env::temp_dir().join(format!("seed-export-gui-{}", uuid::Uuid::new_v4()));
        let (app, _) = crate::app::shell_tests::bitcoin_app(&path);
        let root = app.datadir().clone();
        let cube_id = app.cube_settings().id.clone();
        let chain = app.cube_settings().network;
        let app_generation = app.cache().app_generation;
        let (home, startup) = home::Home::new_for_chain(root.clone(), None);
        drop(startup);
        let mut gui = GUI::reviewer_app_home_gui(app, home);
        let pane_id = gui.focus.unwrap();
        let old_generation = gui.panes.get(pane_id).unwrap().tabs[0]
            .app_generation_token()
            .unwrap();
        // The first App message registers this exact App/Cube/root origin.
        drop(
            gui.panes.get_mut(pane_id).unwrap().tabs[0]
                .update(tab::Message::UnlockMotionTick(Instant::now())),
        );
        let proof = crate::services::unlock::throttle::verified_seed_export_duress_for_test(
            path.clone(),
            cube_id,
            chain,
            app_generation,
        );
        let (release, delayed) = tokio::sync::oneshot::channel();
        let pending = Task::perform(async move { delayed.await.unwrap() }, move |proof| {
            Message::Pane(
                pane_id,
                pane::Message::tab(
                    1,
                    tab::Message::AppAsync(
                        old_generation,
                        Box::new(tab::Message::Run(AppMessage::View(
                            crate::app::view::Message::DuressActivateLocal(proof),
                        ))),
                    ),
                ),
            )
        });
        let (home, startup) = home::Home::new_for_chain(root.clone(), None);
        drop(startup);
        gui.panes.get_mut(pane_id).unwrap().tabs[0].state = tab::State::Home(home);
        release.send(proof).unwrap();
        let delivered = runtime.block_on(async {
            let mut stream = iced_runtime::task::into_stream(pending).unwrap();
            while let Some(action) = stream.next().await {
                if let iced_runtime::Action::Output(message) = action {
                    return message;
                }
            }
            panic!("pending duress task did not deliver")
        });
        drop(gui.update(delivered));
        assert!(matches!(
            gui.panes.get(pane_id).unwrap().tabs[0].state,
            tab::State::DuressActive(_)
        ));
        assert!(!crate::app::session::is_open());
        drop(gui);
        std::fs::remove_dir_all(path).ok();
    }

    #[test]
    fn verified_duress_hides_foreground_seed_reveal_before_wipe_task_runs() {
        let result = std::thread::Builder::new()
            .name("seed-export-immediate-global-seal".into())
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let _enter = runtime.enter();
                let _guard = crate::app::session::test_guard();
                let path = std::env::temp_dir()
                    .join(format!("seed-export-global-seal-{}", uuid::Uuid::new_v4()));
                let (source, _) = crate::app::shell_tests::bitcoin_app(&path);
                let root = source.datadir().clone();
                let proof = crate::services::unlock::throttle::verified_seed_export_duress_for_test(
                    path.clone(),
                    source.cube_settings().id.clone(),
                    source.cube_settings().network,
                    source.cache().app_generation,
                );
                let (home, startup) = home::Home::new_for_chain(root, None);
                drop(startup);
                let (mut gui, pane_id) = GUI::reviewer_split_app_home_gui(source, home);
                let sibling_pane = gui.focus.unwrap();
                let generation = gui.panes.get(pane_id).unwrap().tabs[0]
                    .app_generation_token()
                    .unwrap();
                drop(
                    gui.panes.get_mut(pane_id).unwrap().tabs[0]
                        .update(tab::Message::UnlockMotionTick(Instant::now())),
                );

                let (mut revealing, _) = crate::app::shell_tests::bitcoin_app(&path);
                revealing.reviewer_show_seed_phrase_for_test();
                let pane = gui.panes.get_mut(sibling_pane).unwrap();
                pane.tabs.push(tab::Tab::new(3, tab::State::App(revealing)));
                pane.focused_tab = 1;
                crate::app::session::open(
                    "foreground-seed-reveal",
                    zeroize::Zeroizing::new("2468".into()),
                );

                // Hold the returned Task: local wipe and DuressActivated are
                // delayed. The foreground reveal must already be gone.
                let pending_activation = gui.update(Message::Pane(
                    pane_id,
                    pane::Message::tab(
                        1,
                        tab::Message::AppAsync(
                            generation,
                            Box::new(tab::Message::Run(AppMessage::View(
                                crate::app::view::Message::DuressActivateLocal(proof),
                            ))),
                        ),
                    ),
                ));
                let pane = gui.panes.get(pane_id).unwrap();
                assert_eq!(pane.tabs.len(), 1, "seed reveal sibling was retired");
                assert_eq!(pane.focused_tab, 0);
                assert!(matches!(pane.tabs[0].state, tab::State::DuressActive(_)));
                assert_eq!(gui.focus, Some(pane_id));
                assert_eq!(gui.panes.iter().count(), 1, "sibling pane was retired");
                assert!(!crate::app::session::is_open());
                drop(pending_activation);
                drop(gui);
                std::fs::remove_dir_all(path).ok();
            })
            .unwrap()
            .join();
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }

    #[test]
    fn unlock_duress_hides_foreground_seed_reveal_before_wipe_task_runs() {
        let result = std::thread::Builder::new()
            .name("unlock-duress-immediate-global-seal".into())
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let _enter = runtime.enter();
                let _guard = crate::app::session::test_guard();
                let path = std::env::temp_dir().join(format!(
                    "unlock-duress-global-seal-{}",
                    uuid::Uuid::new_v4()
                ));
                let (source, _) = crate::app::shell_tests::bitcoin_app(&path);
                let root = source.datadir().clone();
                let (home, startup) = home::Home::new_for_chain(root.clone(), None);
                drop(startup);
                let (mut gui, pane_id) = GUI::reviewer_split_app_home_gui(source, home);
                let sibling_pane = gui.focus.unwrap();
                let pin = crate::pin_entry::PinEntry::new(
                    crate::app::settings::CubeSettings::new(
                        "Locked Cube".into(),
                        crate::chain::ChainId::Bitcoin,
                    ),
                    path.clone(),
                    crate::pin_entry::PinEntrySuccess::LoadApp {
                        datadir: root,
                        config: crate::app::Config::new(false),
                        network: bitcoin::Network::Bitcoin,
                        internal_bitcoind: None,
                        backup: None,
                        wallet_settings: None,
                        connect_client: None,
                    },
                    None,
                );
                gui.panes.get_mut(pane_id).unwrap().tabs[0].state = tab::State::PinEntry(pin);

                let (mut revealing, _) = crate::app::shell_tests::bitcoin_app(&path);
                revealing.reviewer_show_seed_phrase_for_test();
                let pane = gui.panes.get_mut(sibling_pane).unwrap();
                pane.tabs.push(tab::Tab::new(3, tab::State::App(revealing)));
                pane.focused_tab = 1;
                crate::app::session::open(
                    "foreground-seed-reveal",
                    zeroize::Zeroizing::new("2468".into()),
                );

                let pending_activation = gui.update(Message::Pane(
                    pane_id,
                    pane::Message::tab(
                        1,
                        tab::Message::PinEntry(crate::pin_entry::Message::DuressDetected {
                            account_id: None,
                        }),
                    ),
                ));
                let pane = gui.panes.get(pane_id).unwrap();
                assert_eq!(pane.tabs.len(), 1);
                assert_eq!(pane.focused_tab, 0);
                assert!(matches!(pane.tabs[0].state, tab::State::DuressActive(_)));
                assert_eq!(gui.focus, Some(pane_id));
                assert_eq!(gui.panes.iter().count(), 1);
                assert!(!crate::app::session::is_open());
                drop(pending_activation);
                drop(gui);
                std::fs::remove_dir_all(path).ok();
            })
            .unwrap()
            .join();
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }

    #[test]
    fn delayed_classifier_duress_survives_back_and_same_tab_reopen() {
        let result = std::thread::Builder::new()
            .name("delayed-unlock-duress-origin".into())
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let _enter = runtime.enter();
                let _guard = crate::app::session::test_guard();
                for reopen in [false, true] {
                    crate::app::session::close();
                    let path = std::env::temp_dir().join(format!(
                        "delayed-unlock-duress-{}-{}",
                        reopen,
                        uuid::Uuid::new_v4()
                    ));
                    let (source, _) = crate::app::shell_tests::bitcoin_app(&path);
                    let cube = source.cube_settings().clone();
                    let root = source.datadir().clone();
                    let (home, startup) = home::Home::new_for_chain(root.clone(), None);
                    drop(startup);
                    let (mut gui, pane_id) = GUI::reviewer_split_app_home_gui(source, home);
                    let sibling_pane = gui.focus.unwrap();
                    let pin = crate::pin_entry::PinEntry::new(
                        cube.clone(),
                        path.clone(),
                        crate::pin_entry::PinEntrySuccess::LoadApp {
                            datadir: root.clone(),
                            config: crate::app::Config::new(false),
                            network: bitcoin::Network::Bitcoin,
                            internal_bitcoind: None,
                            backup: None,
                            wallet_settings: None,
                            connect_client: None,
                        },
                        None,
                    );
                    let old_generation = pin.reviewer_trial_generation();
                    gui.panes.get_mut(pane_id).unwrap().tabs[0].state = tab::State::PinEntry(pin);

                    let (mut revealing, _) = crate::app::shell_tests::bitcoin_app(&path);
                    revealing.reviewer_show_seed_phrase_for_test();
                    let pane = gui.panes.get_mut(sibling_pane).unwrap();
                    pane.tabs.push(tab::Tab::new(3, tab::State::App(revealing)));
                    pane.focused_tab = 1;
                    crate::app::session::open(
                        "foreground-seed-reveal",
                        zeroize::Zeroizing::new("2468".into()),
                    );

                    let proof = crate::pin_entry::VerifiedUnlockDuress::for_test(
                        path.clone(),
                        cube.id.clone(),
                        cube.network,
                    );
                    let (release, delayed) = tokio::sync::oneshot::channel();
                    let pending_classifier =
                        Task::perform(async move { delayed.await.unwrap() }, move |proof| {
                            Message::Pane(
                                pane_id,
                                pane::Message::tab(
                                    1,
                                    tab::Message::PinEntry(crate::pin_entry::Message::Classified(
                                        old_generation,
                                        Ok(crate::pin_entry::Verdict::Duress(proof)),
                                    )),
                                ),
                            )
                        });
                    drop(gui.update(Message::Pane(
                        pane_id,
                        pane::Message::tab(
                            1,
                            tab::Message::PinEntry(crate::pin_entry::Message::Back),
                        ),
                    )));
                    assert!(matches!(
                        gui.panes.get(pane_id).unwrap().tabs[0].state,
                        tab::State::Home(_)
                    ));

                    if reopen {
                        let mut new_cube = cube.clone();
                        new_cube.id = "reopened-different-cube".into();
                        let mut reopened = crate::pin_entry::PinEntry::new(
                            new_cube,
                            path.clone(),
                            crate::pin_entry::PinEntrySuccess::LoadApp {
                                datadir: root,
                                config: crate::app::Config::new(false),
                                network: bitcoin::Network::Bitcoin,
                                internal_bitcoind: None,
                                backup: None,
                                wallet_settings: None,
                                connect_client: None,
                            },
                            None,
                        );
                        assert_ne!(reopened.reviewer_trial_generation(), old_generation);
                        for (index, digit) in "1234".chars().enumerate() {
                            drop(reopened.update(crate::pin_entry::Message::PinInput(
                                crate::pin_input::Message::DigitChanged(index, digit.to_string()),
                            )));
                        }
                        drop(reopened.update(crate::pin_entry::Message::Submit));
                        assert!(reopened.is_loading());
                        gui.panes.get_mut(pane_id).unwrap().tabs[0].state =
                            tab::State::PinEntry(reopened);
                    }

                    // An old ordinary Unlock cannot advance the new PinEntry
                    // or write a signer into the foreground Cube's session.
                    drop(gui.update(Message::Pane(
                        pane_id,
                        pane::Message::tab(
                            1,
                            tab::Message::PinEntry(crate::pin_entry::Message::Classified(
                                old_generation,
                                Ok(crate::pin_entry::Verdict::Unlock),
                            )),
                        ),
                    )));
                    if reopen {
                        let tab::State::PinEntry(pin) =
                            &gui.panes.get(pane_id).unwrap().tabs[0].state
                        else {
                            panic!("old ordinary verdict changed the new PIN entry")
                        };
                        assert!(pin.is_loading());
                    }
                    assert_eq!(
                        crate::app::session::current_cube_id().as_deref(),
                        Some("foreground-seed-reveal")
                    );

                    release.send(proof).unwrap();
                    let delivered = runtime.block_on(async {
                        let mut stream =
                            iced_runtime::task::into_stream(pending_classifier).unwrap();
                        while let Some(action) = stream.next().await {
                            if let iced_runtime::Action::Output(message) = action {
                                return message;
                            }
                        }
                        panic!("held classifier did not deliver")
                    });
                    let pending_activation = gui.update(delivered);
                    let pane = gui.panes.get(pane_id).unwrap();
                    assert_eq!(pane.tabs.len(), 1);
                    assert!(matches!(pane.tabs[0].state, tab::State::DuressActive(_)));
                    assert_eq!(gui.focus, Some(pane_id));
                    assert_eq!(gui.panes.iter().count(), 1);
                    assert!(!crate::app::session::is_open());
                    drop(pending_activation);
                    drop(gui);
                    std::fs::remove_dir_all(path).ok();
                }
            })
            .unwrap()
            .join();
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }

    #[test]
    fn closed_origin_tab_routes_only_same_root_verified_duress_to_surviving_home() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _enter = runtime.enter();
        let _guard = crate::app::session::test_guard();
        let path = std::env::temp_dir().join(format!("seed-export-close-{}", uuid::Uuid::new_v4()));
        let (app, _) = crate::app::shell_tests::bitcoin_app(&path);
        let root = app.datadir().clone();
        let cube_id = app.cube_settings().id.clone();
        let chain = app.cube_settings().network;
        let app_generation = app.cache().app_generation;
        let (home, startup) = home::Home::new_for_chain(root, None);
        drop(startup);
        let mut gui = GUI::reviewer_app_home_gui(app, home);
        let pane_id = gui.focus.unwrap();
        let generation = gui.panes.get(pane_id).unwrap().tabs[0]
            .app_generation_token()
            .unwrap();
        let deliver = move |proof| {
            Message::Pane(
                pane_id,
                pane::Message::tab(
                    1,
                    tab::Message::AppAsync(
                        generation,
                        Box::new(tab::Message::Run(AppMessage::View(
                            crate::app::view::Message::DuressActivateLocal(proof),
                        ))),
                    ),
                ),
            )
        };
        let proof = crate::services::unlock::throttle::verified_seed_export_duress_for_test(
            path.clone(),
            cube_id.clone(),
            chain,
            app_generation,
        );
        let (release, delayed) = tokio::sync::oneshot::channel();
        let pending = Task::perform(async move { delayed.await.unwrap() }, deliver);
        gui.panes.get_mut(pane_id).unwrap().close_tab(0);
        assert_eq!(gui.panes.get(pane_id).unwrap().tabs.len(), 1);
        let unrelated = crate::services::unlock::throttle::verified_seed_export_duress_for_test(
            std::env::temp_dir().join("unrelated-root"),
            cube_id,
            chain,
            app_generation,
        );
        drop(gui.update(deliver(unrelated)));
        assert!(matches!(
            gui.panes.get(pane_id).unwrap().tabs[0].state,
            tab::State::Home(_)
        ));
        assert_eq!(
            gui.panes.get(pane_id).unwrap().tabs.len(),
            1,
            "a wrong-root proof must not retire an unrelated surface"
        );
        release.send(proof).unwrap();
        let delivered = runtime.block_on(async {
            let mut stream = iced_runtime::task::into_stream(pending).unwrap();
            while let Some(action) = stream.next().await {
                if let iced_runtime::Action::Output(message) = action {
                    return message;
                }
            }
            panic!("pending duress task did not deliver")
        });
        drop(gui.update(delivered));
        assert!(matches!(
            gui.panes.get(pane_id).unwrap().tabs[0].state,
            tab::State::DuressActive(_)
        ));
        assert!(!crate::app::session::is_open());
        drop(gui);
        std::fs::remove_dir_all(path).ok();
    }

    #[test]
    fn verified_duress_from_last_closed_tab_installs_emergency_target() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _enter = runtime.enter();
        let _guard = crate::app::session::test_guard();
        let path =
            std::env::temp_dir().join(format!("seed-export-no-tab-{}", uuid::Uuid::new_v4()));
        let (app, _) = crate::app::shell_tests::bitcoin_app(&path);
        let root = app.datadir().clone();
        let proof = crate::services::unlock::throttle::verified_seed_export_duress_for_test(
            path.clone(),
            app.cube_settings().id.clone(),
            app.cube_settings().network,
            app.cache().app_generation,
        );
        let (home, startup) = home::Home::new_for_chain(root, None);
        drop(startup);
        let mut gui = GUI::reviewer_app_home_gui(app, home);
        let pane_id = gui.focus.unwrap();
        let generation = gui.panes.get(pane_id).unwrap().tabs[0]
            .app_generation_token()
            .unwrap();
        let (release, delayed) = tokio::sync::oneshot::channel();
        let pending = Task::perform(async move { delayed.await.unwrap() }, move |proof| {
            Message::Pane(
                pane_id,
                pane::Message::tab(
                    1,
                    tab::Message::AppAsync(
                        generation,
                        Box::new(tab::Message::Run(AppMessage::View(
                            crate::app::view::Message::DuressActivateLocal(proof),
                        ))),
                    ),
                ),
            )
        });
        let (release_wrong, delayed_wrong) = tokio::sync::oneshot::channel::<()>();
        let pending_wrong = Task::perform(async move { delayed_wrong.await.unwrap() }, move |()| {
            Message::Pane(
                pane_id,
                pane::Message::tab(
                    1,
                    tab::Message::AppAsync(
                        generation,
                        Box::new(tab::Message::Run(AppMessage::View(
                            crate::app::view::Message::Settings(
                                crate::app::view::SettingsMessage::BackupMasterSeed(
                                    crate::app::view::BackupWalletMessage::PinVerifiedAttempt(
                                        crate::app::cache::AppGeneration::next(),
                                        Err("Incorrect PIN".into()),
                                    ),
                                ),
                            ),
                        ))),
                    ),
                ),
            )
        });
        drop(gui.update(Message::Pane(
            pane_id,
            pane::Message::View(pane::ViewMessage::CloseTab(1)),
        )));
        gui.panes.get_mut(pane_id).unwrap().tabs[0].mark_seed_export_trial_pending_for_test();
        gui.panes.get_mut(pane_id).unwrap().tabs[0].mark_seed_export_trial_pending_for_test();
        drop(gui.update(Message::Pane(
            pane_id,
            pane::Message::View(pane::ViewMessage::CloseTab(0)),
        )));
        let pane = gui
            .panes
            .get(pane_id)
            .expect("PIN trial keeps window alive");
        assert_eq!(pane.tabs.len(), 1);
        assert_eq!(pane.tabs[0].id, usize::MAX);
        release_wrong.send(()).unwrap();
        let wrong = runtime.block_on(async {
            let mut stream = iced_runtime::task::into_stream(pending_wrong).unwrap();
            while let Some(action) = stream.next().await {
                if let iced_runtime::Action::Output(message) = action {
                    return message;
                }
            }
            panic!("first pending PIN task did not deliver")
        });
        drop(gui.update(wrong));
        assert_eq!(
            gui.panes.get(pane_id).unwrap().tabs[0].id,
            usize::MAX,
            "one wrong result must not close the window before another trial's Duress verdict"
        );
        release.send(proof).unwrap();
        let delivered = runtime.block_on(async {
            let mut stream = iced_runtime::task::into_stream(pending).unwrap();
            while let Some(action) = stream.next().await {
                if let iced_runtime::Action::Output(message) = action {
                    return message;
                }
            }
            panic!("pending duress task did not deliver")
        });
        drop(gui.update(delivered));
        let pane = gui.panes.get(pane_id).unwrap();
        assert_eq!(pane.tabs.len(), 1);
        assert!(matches!(pane.tabs[0].state, tab::State::DuressActive(_)));
        assert!(!crate::app::session::is_open());
        drop(gui);
        std::fs::remove_dir_all(path).ok();
    }

    #[test]
    fn ordinary_pin_result_releases_last_tab_window_hold() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _enter = runtime.enter();
        let _guard = crate::app::session::test_guard();
        let path =
            std::env::temp_dir().join(format!("seed-export-close-wrong-{}", uuid::Uuid::new_v4()));
        let (app, _) = crate::app::shell_tests::bitcoin_app(&path);
        let (home, startup) = home::Home::new_for_chain(app.datadir().clone(), None);
        drop(startup);
        let mut gui = GUI::reviewer_app_home_gui(app, home);
        let pane_id = gui.focus.unwrap();
        let generation = gui.panes.get(pane_id).unwrap().tabs[0]
            .app_generation_token()
            .unwrap();
        drop(gui.update(Message::Pane(
            pane_id,
            pane::Message::View(pane::ViewMessage::CloseTab(1)),
        )));
        gui.panes.get_mut(pane_id).unwrap().tabs[0].mark_seed_export_trial_pending_for_test();
        drop(gui.update(Message::Pane(
            pane_id,
            pane::Message::View(pane::ViewMessage::CloseTab(0)),
        )));
        assert_eq!(gui.panes.get(pane_id).unwrap().tabs[0].id, usize::MAX);
        let stale = Message::Pane(
            pane_id,
            pane::Message::tab(
                1,
                tab::Message::AppAsync(
                    generation,
                    Box::new(tab::Message::Run(AppMessage::View(
                        crate::app::view::Message::Settings(
                            crate::app::view::SettingsMessage::BackupMasterSeed(
                                crate::app::view::BackupWalletMessage::PinVerifiedAttempt(
                                    crate::app::cache::AppGeneration::next(),
                                    Err("Incorrect PIN".into()),
                                ),
                            ),
                        ),
                    ))),
                ),
            ),
        );
        drop(gui.update(stale));
        assert!(gui
            .panes
            .get(pane_id)
            .is_none_or(|pane| pane.tabs.is_empty()));
        drop(gui);
        std::fs::remove_dir_all(path).ok();
    }
}

use iced::window::Id;

/// The eye's message must retain the generation emitted by the App view.
/// A bare `Run` has no provenance and cannot change a device-wide setting.
fn balance_privacy_eye_change(
    tab: Option<&tab::Tab>,
    message: &tab::Message,
) -> Option<(crate::app::settings::preferences::PreferenceChange, bool)> {
    let run_change = |message: &tab::Message| match message {
        tab::Message::Run(AppMessage::View(crate::app::view::Message::ChangeDevicePreference(
            change,
        ))) => Some(*change),
        _ => None,
    };
    match message {
        tab::Message::AppAsync(generation, inner) => run_change(inner).map(|change| {
            let current = tab.is_some_and(|tab| {
                tab.accepts_app_generation(*generation)
                    && matches!(&tab.state, tab::State::App(app) if !app.cube_settings().network.is_blake2b())
            });
            (change, current)
        }),
        tab::Message::ForkAsync(generation, inner) => run_change(inner).map(|change| {
            let current = tab.is_some_and(|tab| {
                tab.accepts_fork_generation(*generation)
                    && matches!(&tab.state, tab::State::App(app) if app.cube_settings().network.is_blake2b())
            });
            (change, current)
        }),
        other => run_change(other).map(|change| (change, false)),
    }
}

/// The Launcher's dock may only choose where network activity is read from,
/// and only while the tab that sent it still shows the Launcher. It shows no
/// balances, so it never changes balance privacy; a queued pick from a tab
/// that has since opened a Cube is dropped.
fn launcher_change_is_current(
    tab: Option<&tab::Tab>,
    change: &crate::app::settings::preferences::PreferenceChange,
) -> bool {
    matches!(
        change,
        crate::app::settings::preferences::PreferenceChange::NetworkActivitySource(_)
    ) && tab.is_some_and(|tab| matches!(tab.state, tab::State::Home(_)))
}

/// Home reload also reconciles fork and Split flow availability. Only feature
/// edits may ask it to run that reconciliation.
fn needs_global_settings_reload(
    before: Option<crate::app::settings::preferences::DevicePreferences>,
    after: Option<crate::app::settings::preferences::DevicePreferences>,
) -> bool {
    match (before, after) {
        (Some(before), Some(after)) => {
            before.developer_mode != after.developer_mode
                || before.bitcoin_blake2b_beta != after.bitcoin_blake2b_beta
        }
        (None, Some(_)) => true,
        _ => false,
    }
}

fn launcher_presentation_changed(
    before: Option<crate::app::settings::preferences::DevicePreferences>,
    after: Option<crate::app::settings::preferences::DevicePreferences>,
) -> bool {
    match (before, after) {
        (Some(before), Some(after)) => {
            before.launcher_style != after.launcher_style
                || before.motion != after.motion
                || before.network_activity != after.network_activity
                || before.network_activity_source != after.network_activity_source
        }
        (None, Some(_)) => true,
        _ => false,
    }
}

#[cfg(test)]
mod balance_privacy_eye_origin_tests {
    use super::*;
    use crate::app::{self, settings::preferences::PreferenceChange, view};

    #[test]
    fn first_render_rechecks_a_show_receipt_after_another_writer_hides() {
        let root =
            std::env::temp_dir().join(format!("privacy-first-render-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join(crate::app::settings::global::DEFAULT_FILE_NAME);
        let shown =
            GlobalSettings::save_preference(&path, PreferenceChange::HideBalances(false)).unwrap();
        let owner = PreferencesOwner::new(path.clone(), &Ok(shown));
        let shown_bytes = std::fs::read(&path).unwrap();
        assert!(!privacy_render_needs_cover(&owner, &path));
        assert_eq!(std::fs::read(&path).unwrap(), shown_bytes);

        let other_path = path.clone();
        std::thread::spawn(move || {
            GlobalSettings::save_preference(&other_path, PreferenceChange::HideBalances(true))
                .unwrap();
        })
        .join()
        .unwrap();
        assert_eq!(
            owner.privacy(),
            crate::app::state::preferences::BalancePrivacy::Shown
        );
        assert!(privacy_render_needs_cover(&owner, &path));

        std::fs::write(&path, br#"{"hide_balances":false}"#).unwrap();
        assert!(privacy_render_needs_cover(&owner, &path));

        std::fs::write(&path, b"{malformed").unwrap();
        assert!(privacy_render_needs_cover(&owner, &path));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn render_covers_amounts_without_waiting_for_a_settings_writer() {
        use fs4::fs_std::FileExt;
        use std::sync::mpsc;

        let root =
            std::env::temp_dir().join(format!("privacy-render-lock-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join(crate::app::settings::global::DEFAULT_FILE_NAME);
        let shown =
            GlobalSettings::save_preference(&path, PreferenceChange::HideBalances(false)).unwrap();
        let owner = PreferencesOwner::new(path.clone(), &Ok(shown));
        let lock = std::fs::File::open(path.with_extension("lock")).unwrap();
        FileExt::lock_exclusive(&lock).unwrap();

        let (sender, receiver) = mpsc::channel();
        let render = std::thread::spawn(move || {
            sender
                .send(privacy_render_needs_cover(&owner, &path))
                .unwrap();
        });
        let covered_while_locked = receiver.recv_timeout(Duration::from_secs(2));
        FileExt::unlock(&lock).unwrap();
        render.join().unwrap();
        assert!(covered_while_locked.unwrap());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn queued_eye_event_cannot_change_device_privacy_after_app_closes() {
        let _guard = app::session::test_guard();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _enter = runtime.enter();
        let path =
            std::env::temp_dir().join(format!("privacy-eye-origin-{}", uuid::Uuid::new_v4()));
        let (app, _) = app::shell_tests::bitcoin_app(&path);
        let mut tab = tab::Tab::new(1, tab::State::App(app));
        let generation = tab.app_generation_token().unwrap();
        let change = PreferenceChange::HideBalances(false);
        let source = PreferenceChange::NetworkActivitySource(
            crate::app::settings::preferences::NetworkActivitySource::Connect,
        );
        assert!(
            !launcher_change_is_current(Some(&tab), &source),
            "a Launcher dock pick cannot write once an App owns the tab"
        );
        let eye = || {
            tab::Message::AppAsync(
                generation,
                Box::new(tab::Message::Run(AppMessage::View(
                    view::Message::ChangeDevicePreference(change),
                ))),
            )
        };

        assert_eq!(
            balance_privacy_eye_change(Some(&tab), &eye()),
            Some((change, true))
        );
        assert_eq!(
            balance_privacy_eye_change(
                Some(&tab),
                &tab::Message::Run(AppMessage::View(view::Message::ChangeDevicePreference(
                    change
                )))
            ),
            Some((change, false)),
            "a bare Run event has no App generation"
        );
        drop(tab.update(tab::Message::LockCubeToLauncher));
        assert!(matches!(tab.state, tab::State::Home(_)));
        assert!(
            !launcher_change_is_current(None, &source),
            "a Launcher dock pick for a closed tab is dropped"
        );
        assert!(launcher_change_is_current(Some(&tab), &source));
        assert!(
            !launcher_change_is_current(Some(&tab), &change),
            "the Launcher has no eye and never writes balance privacy"
        );
        assert_eq!(
            balance_privacy_eye_change(Some(&tab), &eye()),
            Some((change, false))
        );
        std::fs::remove_dir_all(path).ok();
    }
}

/// "Zoom 125%", centred in the top bar's empty middle. Not interactive, so
/// the stack still hands every click and hover to the screen beneath.
fn zoom_pill<'a>(zoom: Zoom) -> Element<'a, Message> {
    use coincube_ui::{component::text, theme};
    let top_bar = crate::app::view::nav::shell::TOP_BAR_HEIGHT;
    iced::widget::container(
        iced::widget::container(text::p2_regular(format!("Zoom {}%", zoom.percent())))
            .padding([6, 14])
            .style(|theme: &theme::Theme| iced::widget::container::Style {
                background: Some(iced::Background::Color(theme.colors.shell.panel)),
                text_color: Some(theme.colors.shell.ink),
                border: iced::Border {
                    color: theme.colors.shell.line,
                    width: 1.0,
                    radius: 999.0.into(),
                },
                ..Default::default()
            }),
    )
    .width(iced::Length::Fill)
    .height(iced::Length::Fixed(top_bar))
    .center_x(iced::Length::Fill)
    .center_y(iced::Length::Fixed(top_bar))
    .into()
}

/// Exact provider/account context to revalidate on the next local PIN attempt.
/// Holding this candidate grants neither seed access nor daemon authority.
#[derive(Clone)]
struct ForkUnlockCandidate {
    root: CoincubeDirectory,
    client: crate::services::coincube::CoincubeClient,
    user_id: u32,
}

pub struct GUI {
    panes: pane_grid::State<pane::Pane>,
    focus: Option<pane_grid::Pane>,
    config: Config,
    window_id: Option<Id>,
    window_init: Option<bool>,
    window_config: Option<WindowConfig>,
    global_cache: GlobalCache,
    theme_mode: coincube_ui::theme::palette::ThemeMode,
    system_theme: Option<coincube_ui::theme::palette::ThemeMode>,
    system_reduces_motion: Option<bool>,
    preferences_sheet: Option<PreferencesSheet>,
    connect_sheet: Option<ConnectSheet>,
    preferences: PreferencesOwner,
    zoom: ZoomState,
    worldview_sharing: worldview::Sharing,
}

/// How long the zoom pill stays up after a shortcut. The 1 Hz tick clears
/// it, so it shows for one to two seconds.
const ZOOM_NOTICE: Duration = Duration::from_secs(1);

/// Zoom applied ahead of its save. The preferences lane refuses work while
/// it is busy, and pressing Cmd + twice quickly must still land on the
/// second step, so the GUI holds the newest choice until a save confirms it.
#[derive(Debug, Default)]
struct ZoomState {
    pending: Option<Zoom>,
    notice_until: Option<Instant>,
    /// The window's screen width in points at 100%, once iced reports it.
    /// Caps the zoom at [`Zoom::largest_for_screen`].
    screen_width: Option<f32>,
    /// The zoom the window's minimum size was last set for.
    min_size_for: Option<Zoom>,
}

/// The minimum window height, in screen points, set in `main.rs`. Only the
/// width grows with the zoom: a shorter layout scrolls, a narrower one breaks.
const MIN_WINDOW_HEIGHT: f32 = 768.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoomStep {
    In,
    Out,
    ActualSize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Key {
    Tab(bool),
    Escape,
    Zoom(ZoomStep),
}

#[derive(Debug)]
pub enum Message {
    CtrlC,
    Tick,
    FontLoaded(Result<(), iced::font::Error>),
    Pane(pane_grid::Pane, pane::Message),
    KeyPressed(Key),
    Event(iced::Event),

    Clicked(pane_grid::Pane),
    Dragged(pane_grid::DragEvent),
    Resized(pane_grid::ResizeEvent),
    Window(Option<Id>),
    WindowSize(Size),
    /// The window's screen size in zoomed units, and the zoom it was asked at.
    ScreenSize(Option<Size>, Zoom),
    /// The window moved, maybe to another screen.
    WindowMoved,

    Fiat(FiatMessage),
    ToggleTheme,
    SystemTheme(iced::theme::Mode),
    PreferencesSheet(preferences_view::Message),
    Worldview(
        OverlayOrigin,
        (u32, u64, u64),
        uuid::Uuid,
        Result<
            crate::services::coincube::worldview::Snapshot,
            crate::services::coincube::worldview::Error,
        >,
    ),
    ConnectSheet(ConnectSheetMessage),
    Preferences(SaveRequest, Result<PreferencesReceipt, String>),
}

#[derive(Debug)]
pub enum FiatMessage {
    GetPriceResult(FiatPrice),
    /// Result of a request for the list of available currencies for a given source.
    ///
    /// The pane and tab that requested the list are included to be able to return the result.
    ListCurrenciesResult(
        pane_grid::Pane,
        usize, // tab id
        PriceSource,
        Instant,
        Result<ListCurrenciesResult, PriceApiError>,
    ),
}

impl From<FiatMessage> for Message {
    fn from(value: FiatMessage) -> Self {
        Self::Fiat(value)
    }
}

impl From<Result<(), iced::font::Error>> for Message {
    fn from(value: Result<(), iced::font::Error>) -> Self {
        Self::FontLoaded(value)
    }
}

async fn ctrl_c() -> Result<(), ()> {
    if let Err(e) = tokio::signal::ctrl_c().await {
        error!("{}", e);
    }
    info!("Signal received, exiting");
    Ok(())
}

/// Whether an event means a human did something.
///
/// This is the idle auto-lock's only source of truth for "someone is at the
/// machine". Window events are deliberately excluded: a resize or a focus
/// change can be produced by the OS, another application, or a display
/// reconfiguration with nobody present.
fn is_user_input(event: &Event) -> bool {
    matches!(
        event,
        Event::Keyboard(_) | Event::Mouse(_) | Event::Touch(_)
    )
}

/// Only keys the focused widget declined belong to the GUI shell. Captured
/// Escape remains with text fields and modal content.
fn ignored_shell_key(event: &Event, status: event::Status) -> Option<Key> {
    if !matches!(status, event::Status::Ignored) {
        return None;
    }
    match event {
        Event::Keyboard(keyboard::Event::KeyPressed {
            key: keyboard::Key::Named(keyboard::key::Named::Tab),
            modifiers,
            ..
        }) => Some(Key::Tab(modifiers.shift())),
        Event::Keyboard(keyboard::Event::KeyPressed {
            key: keyboard::Key::Named(keyboard::key::Named::Escape),
            repeat: false,
            ..
        }) => Some(Key::Escape),
        Event::Keyboard(keyboard::Event::KeyPressed {
            key: keyboard::Key::Character(key),
            modifiers,
            ..
        }) if modifiers.command() && !modifiers.alt() => match key.as_str() {
            // Shift turns = into + on most layouts; both zoom in.
            "=" | "+" => Some(Key::Zoom(ZoomStep::In)),
            "-" | "_" => Some(Key::Zoom(ZoomStep::Out)),
            "0" => Some(Key::Zoom(ZoomStep::ActualSize)),
            _ => None,
        },
        _ => None,
    }
}

fn connect_account_message(
    message: &tab::Message,
) -> Option<&crate::app::view::ConnectAccountMessage> {
    match message {
        tab::Message::Launch(home::Message::View(home::ViewMessage::ConnectAccount(message)))
        | tab::Message::Run(AppMessage::View(crate::app::view::Message::ConnectAccount(message))) => {
            Some(message)
        }
        _ => None,
    }
}

/// Guarded App tasks bypass Pane's ordinary task-output mapper. Preserve its
/// navigation bubbles after checking the generation in `GUI::update`.
fn route_guarded_tab_result(
    pane_id: pane_grid::Pane,
    tab_id: usize,
    message: tab::Message,
) -> Message {
    let view = match message {
        tab::Message::OpenLauncher => pane::ViewMessage::OpenLauncher,
        tab::Message::OpenConnectSignIn => pane::ViewMessage::OpenConnectSignIn,
        tab::Message::OpenPlanBilling => pane::ViewMessage::OpenPlanBilling,
        tab::Message::ToggleTheme => pane::ViewMessage::ToggleTheme,
        tab::Message::OpenAppPreferences(source) => {
            return Message::Pane(
                pane_id,
                pane::Message::tab(tab_id, tab::Message::OpenAppPreferences(source)),
            );
        }
        tab::Message::GlobalSettingsChanged => pane::ViewMessage::GlobalSettingsChanged,
        tab::Message::ConnectSignedIn => pane::ViewMessage::ConnectSignedIn,
        message => return Message::Pane(pane_id, pane::Message::tab(tab_id, message)),
    };
    Message::Pane(pane_id, pane::Message::View(view))
}

fn is_seed_export_trial_result(message: &tab::Message) -> bool {
    matches!(
        message,
        tab::Message::Run(AppMessage::View(crate::app::view::Message::Settings(
            crate::app::view::SettingsMessage::BackupMasterSeed(
                crate::app::view::BackupWalletMessage::PinVerifiedAttempt(..)
            ) | crate::app::view::SettingsMessage::RecoveryKit(
                crate::app::view::RecoveryKitMessage::PinVerifiedAttempt(..)
            ) | crate::app::view::SettingsMessage::RecoveryAlerts(
                crate::app::view::RecoveryAlertsMessage::FullCubePinReady(..)
                    | crate::app::view::RecoveryAlertsMessage::FullCubePinFailed(..)
            )
        )))
    )
}

#[cfg(test)]
fn unresolved_test_preferences() -> PreferencesOwner {
    PreferencesOwner::new(
        std::path::PathBuf::from("unresolved-test-fixture")
            .join(crate::app::settings::global::DEFAULT_FILE_NAME),
        &Err(crate::app::settings::global::PrivacyInitError::Settings(
            crate::app::settings::SettingsError::Unexpected(
                "not loaded by this fixture".to_string(),
            ),
        )),
    )
}

impl GUI {
    fn finish_closed_seed_export_trial(
        &mut self,
        pane_id: pane_grid::Pane,
        origin_tab: usize,
    ) -> Task<Message> {
        let Some(pane) = self.panes.get_mut(pane_id) else {
            return Task::none();
        };
        let Some(index) = pane.tabs.iter().position(|tab| tab.id == usize::MAX) else {
            return Task::none();
        };
        if pane.tabs[index].finish_closed_seed_export_trial(origin_tab) != Some(true) {
            return Task::none();
        }
        pane.close_tab(index);
        if pane.tabs.is_empty() {
            self.panes.close(pane_id);
            if self.focus == Some(pane_id) {
                self.focus = None;
            }
        }
        if !self.panes.iter().any(|(_, pane)| !pane.tabs.is_empty()) {
            iced::window::latest().and_then(iced::window::close)
        } else {
            Task::none()
        }
    }

    /// A verified marker is an emergency after the originating settings tab
    /// closes as well. The proof is private to the locked PIN classifier; the
    /// GUI data directory is the final owner boundary before local activation.
    fn route_verified_seed_export_duress(
        &mut self,
        pane_id: pane_grid::Pane,
        tab_id: usize,
        proof: crate::services::unlock::throttle::VerifiedSeedExportDuress,
    ) -> Task<Message> {
        let (root, cube_id, chain, _app) = proof.origin();
        if cube_id.is_empty()
            || !crate::chain::same_path(root, self.config.coincube_directory.path())
        {
            return Task::none();
        }
        if self
            .panes
            .get(pane_id)
            .is_some_and(|pane| pane.tabs.iter().any(|tab| tab.id == tab_id))
        {
            let was_duress = self.panes.get(pane_id).is_some_and(|pane| {
                pane.tabs
                    .iter()
                    .any(|tab| tab.id == tab_id && matches!(tab.state, tab::State::DuressActive(_)))
            });
            let task = self.update(Message::Pane(
                pane_id,
                pane::Message::tab(tab_id, tab::Message::LocalDuress(proof)),
            ));
            if !was_duress {
                self.seal_verified_seed_export_duress_surface(pane_id, tab_id);
            }
            return task;
        }
        let (target_pane, target_tab) = self.emergency_duress_target(pane_id, tab_id, root, chain);
        let was_duress = self.panes.get(target_pane).is_some_and(|pane| {
            pane.tabs
                .iter()
                .any(|tab| tab.id == target_tab && matches!(tab.state, tab::State::DuressActive(_)))
        });
        let task = self
            .panes
            .get_mut(target_pane)
            .unwrap()
            .tabs
            .iter_mut()
            .find(|tab| tab.id == target_tab)
            .unwrap()
            .activate_unhosted_seed_export_duress(proof);
        if !was_duress {
            self.seal_verified_seed_export_duress_surface(target_pane, target_tab);
        }
        task.map(move |message| Message::Pane(target_pane, pane::Message::tab(target_tab, message)))
    }

    fn route_verified_unlock_duress(
        &mut self,
        pane_id: pane_grid::Pane,
        tab_id: usize,
        proof: crate::pin_entry::VerifiedUnlockDuress,
    ) -> Task<Message> {
        let (root, cube_id, chain) = proof.origin();
        if cube_id.is_empty()
            || !crate::chain::same_path(root, self.config.coincube_directory.path())
        {
            return Task::none();
        }
        let (target_pane, target_tab) = self.emergency_duress_target(pane_id, tab_id, root, chain);
        let was_duress = self.panes.get(target_pane).is_some_and(|pane| {
            pane.tabs
                .iter()
                .any(|tab| tab.id == target_tab && matches!(tab.state, tab::State::DuressActive(_)))
        });
        let task = self
            .panes
            .get_mut(target_pane)
            .unwrap()
            .tabs
            .iter_mut()
            .find(|tab| tab.id == target_tab)
            .unwrap()
            .activate_verified_unlock_duress(proof);
        if !was_duress {
            self.seal_verified_seed_export_duress_surface(target_pane, target_tab);
        }
        task.map(move |message| Message::Pane(target_pane, pane::Message::tab(target_tab, message)))
    }

    fn emergency_duress_target(
        &mut self,
        pane_id: pane_grid::Pane,
        tab_id: usize,
        root: &std::path::Path,
        chain: crate::chain::ChainId,
    ) -> (pane_grid::Pane, usize) {
        if self
            .panes
            .get(pane_id)
            .is_some_and(|pane| pane.tabs.iter().any(|tab| tab.id == tab_id))
        {
            return (pane_id, tab_id);
        }
        let mut target = self
            .panes
            .iter()
            .find_map(|(id, pane)| pane.tabs.first().map(|tab| (*id, tab.id)));
        if target.is_none() {
            // Pane::remove_tab can leave a live GUI with no tabs. Build an
            // emergency-only target from the verified proof's root; no App or
            // signer is reconstructed just to deliver the wipe completion.
            let datadir = CoincubeDirectory::new(root.to_path_buf());
            let state = tab::State::DuressActive(
                crate::app::view::duress::active_screen::DuressActiveScreen::with_context(
                    datadir,
                    Some(chain.bitcoin_network()),
                ),
            );
            let other_pane = self.panes.iter().next().map(|(id, _)| *id);
            if let Some(pane) = self.panes.get_mut(pane_id) {
                let emergency_id = usize::MAX;
                pane.tabs.push(tab::Tab::new(emergency_id, state));
                pane.focused_tab = pane.tabs.len() - 1;
                target = Some((pane_id, emergency_id));
            } else if let Some(other_id) = other_pane {
                let pane = self.panes.get_mut(other_id).expect("pane still exists");
                let emergency_id = usize::MAX;
                pane.tabs.push(tab::Tab::new(emergency_id, state));
                pane.focused_tab = pane.tabs.len() - 1;
                target = Some((other_id, emergency_id));
            } else {
                let (panes, new_pane) = pane_grid::State::new(pane::Pane::new_with_tab(state));
                self.panes = panes;
                self.focus = Some(new_pane);
                target = Some((new_pane, 1));
            }
        }
        target.expect("emergency target was installed")
    }

    /// Admit the private proof across the entire window immediately. The
    /// orchestrator may wait on disk or network work, so its completion cannot
    /// be the event that hides a foreground seed reveal in another pane.
    /// A refused or replayed proof never changes its target to DuressActive.
    fn seal_verified_seed_export_duress_surface(
        &mut self,
        pane_id: pane_grid::Pane,
        tab_id: usize,
    ) {
        let admitted = self.panes.get(pane_id).is_some_and(|pane| {
            pane.tabs
                .iter()
                .any(|tab| tab.id == tab_id && matches!(tab.state, tab::State::DuressActive(_)))
        });
        if !admitted {
            return;
        }
        let mut empty_panes = Vec::new();
        for (&other_id, pane) in self.panes.iter_mut() {
            pane.close_tabs_matching(|tab| other_id != pane_id || tab.id != tab_id);
            if other_id != pane_id && pane.tabs.is_empty() {
                empty_panes.push(other_id);
            }
        }
        for empty in empty_panes {
            self.panes.close(empty);
        }
        if let Some(pane) = self.panes.get_mut(pane_id) {
            pane.focused_tab = pane
                .tabs
                .iter()
                .position(|tab| tab.id == tab_id)
                .unwrap_or(0);
        }
        self.focus = Some(pane_id);
        self.preferences_sheet = None;
        self.connect_sheet = None;
        self.publish_preferences_sheet();
        crate::app::session::close();
    }

    /// The process owns one secret session and idle clock. A new public Cube
    /// admission must retire every old secret-bearing surface before its
    /// constructor or unlock callback can run, even when no App tab remains.
    fn retire_public_owners(&mut self) {
        for (_, pane) in self.panes.iter_mut() {
            pane.close_tabs_matching(|tab| {
                matches!(
                    tab.state,
                    tab::State::App(_)
                        | tab::State::Loader(_)
                        | tab::State::Installer(_)
                        | tab::State::PinEntry(_)
                        | tab::State::PasskeyUnlock(_)
                )
            });
        }
        crate::app::session::close();
    }

    #[cfg(test)]
    fn reviewer_unwrap_guarded_message(mut message: &tab::Message) -> &tab::Message {
        while let tab::Message::AppAsync(_, inner) | tab::Message::ForkAsync(_, inner) = message {
            message = inner;
        }
        message
    }

    pub fn title(&self) -> String {
        match cfg!(debug_assertions) {
            true => format!("Tenshu v{} (development)", env!("CARGO_PKG_VERSION")),
            false => format!("Tenshu v{}", env!("CARGO_PKG_VERSION")),
        }
    }

    pub fn new(config: Config, log_level: Option<LevelFilter>) -> (GUI, Task<Message>) {
        // Establish the global privacy receipt before constructing the first pane.
        // A failed or malformed read remains unresolved and masked.
        let privacy = GlobalSettings::load_or_initialize_preferences(&config.coincube_directory);
        let preferences =
            PreferencesOwner::new(GlobalSettings::path(&config.coincube_directory), &privacy);
        let log_level = log_level.unwrap_or(LevelFilter::INFO);
        // Record the resolved (possibly `--datadir`-overridden) data directory
        // process-wide so helpers without a datadir in hand — e.g. the Connect
        // panel's duress fingerprint — use the real path, not the OS default.
        CoincubeDirectory::set_active(config.coincube_directory.clone());

        if let Err(e) = setup_logger(log_level, &config.coincube_directory) {
            eprintln!("Error while setting up logger: {}", e);
        }

        let mut cmds = vec![
            window::oldest().map(Message::Window),
            Task::perform(ctrl_c(), |_| Message::CtrlC),
            iced::system::theme().map(Message::SystemTheme),
        ];
        let (mut pane, cmd) = pane::Pane::new(&config);
        pane.set_balance_privacy(Some(Self::privacy_chrome_for(&preferences)));
        let system_reduces_motion = crate::app::settings::system_accessibility::reduces_motion();
        for tab in &mut pane.tabs {
            tab.set_system_reduces_motion(system_reduces_motion);
        }
        let (mut panes, focused_pane) = pane_grid::State::new(pane);
        cmds.push(cmd.map(move |msg| Message::Pane(focused_pane, msg)));
        let global_settings_path = GlobalSettings::path(&config.coincube_directory);
        crate::services::branta::set_enabled(GlobalSettings::load_recipient_identity_checks(
            &global_settings_path,
        ));
        let window_config = GlobalSettings::load_window_config(&global_settings_path);
        let window_init = window_config.is_some().then_some(true);
        let theme_mode = GlobalSettings::load_theme_mode(&global_settings_path);
        // Propagate persisted theme mode to all pane tabs
        for (_, pane) in panes.iter_mut() {
            pane.set_theme_mode(theme_mode);
        }
        (
            Self {
                panes,
                focus: Some(focused_pane),
                config,
                window_id: None,
                window_init,
                window_config,
                global_cache: GlobalCache::default(),
                theme_mode,
                system_theme: None,
                system_reduces_motion,
                preferences_sheet: None,
                connect_sheet: None,
                preferences,
                zoom: ZoomState::default(),
                worldview_sharing: worldview::Sharing::default(),
            },
            Task::batch(cmds),
        )
    }

    fn privacy_chrome_for(preferences: &PreferencesOwner) -> BalancePrivacyChrome {
        let user_change_busy = (preferences.is_busy() && !preferences.is_background_refreshing())
            || preferences.has_queued_changes();
        BalancePrivacyChrome {
            privacy: preferences.privacy(),
            busy: user_change_busy,
            interactive: preferences.accepted().is_some() && !user_change_busy,
        }
    }

    /// Mark the Launcher under an open App preferences sheet so its top-bar
    /// entry draws as current; every other Launcher draws it at rest.
    fn publish_preferences_sheet(&mut self) {
        let origin = self
            .preferences_sheet
            .as_ref()
            .map(|sheet| (sheet.origin.pane, sheet.origin.tab_id));
        for (pane_id, pane) in self.panes.iter_mut() {
            for tab in &mut pane.tabs {
                if let tab::State::Home(home) = &mut tab.state {
                    home.app_preferences_open = origin == Some((*pane_id, tab.id));
                }
            }
        }
    }

    fn publish_balance_privacy(&mut self) {
        let chrome = Some(Self::privacy_chrome_for(&self.preferences));
        for (_, pane) in self.panes.iter_mut() {
            pane.set_balance_privacy(chrome);
        }
    }

    /// A single resolved device theme drives the window and every retained tab.
    /// A System choice follows the latest theme reported by iced; until that
    /// arrives it uses the model's documented dark fallback.
    fn publish_appearance(&mut self) {
        let Some(accepted) = self.preferences.accepted() else {
            return;
        };
        let resolved = accepted
            .preferences
            .resolve(self.system_theme, self.system_reduces_motion);
        self.theme_mode = resolved.theme_mode;
        for (_, pane) in self.panes.iter_mut() {
            pane.set_theme_mode(self.theme_mode);
            pane.set_reduce_motion(resolved.reduce_motion);
        }
    }

    fn reload_all_global_settings(&mut self) -> Task<Message> {
        let root = &self.config.coincube_directory;
        let mut tasks = Vec::new();
        for (&pane_id, pane) in self.panes.iter_mut() {
            for tab in &mut pane.tabs {
                let tab_id = tab.id;
                tasks.push(tab.reload_global_settings(root).map(move |message| {
                    Message::Pane(pane_id, pane::Message::tab(tab_id, message))
                }));
            }
        }
        Task::batch(tasks)
    }

    fn refresh_all_launcher_modes(&mut self) {
        for (_, pane) in self.panes.iter_mut() {
            for tab in &mut pane.tabs {
                if let tab::State::Home(home) = &mut tab.state {
                    home.refresh_launcher_mode();
                }
            }
        }
    }

    fn preferences_view_model(&self) -> preferences_view::ViewModel<'_> {
        let accepted = self.preferences.accepted();
        let notice = self.preferences.notice();
        let user_change_busy = (self.preferences.is_busy()
            && !self.preferences.is_background_refreshing())
            || self.preferences.has_queued_changes();
        let pending_operation = self.preferences.pending_operation();
        preferences_view::ViewModel {
            saved: accepted.map(|value| value.preferences),
            effective: accepted.map(|value| {
                value
                    .preferences
                    .resolve(self.system_theme, self.system_reduces_motion)
            }),
            system_theme: self.system_theme,
            system_reduces_motion: self.system_reduces_motion,
            cubes_renderer_available: true,
            saving: matches!(pending_operation, Some(Operation::Save(_)))
                || self.preferences.has_queued_changes(),
            balance_privacy: self.preferences.privacy(),
            balance_privacy_interactive: !user_change_busy,
            durability_unconfirmed: matches!(notice, PreferencesNotice::SavedUnconfirmed),
            loading: user_change_busy && matches!(pending_operation, Some(Operation::Initialize)),
            load_error: matches!(notice, PreferencesNotice::LoadFailed)
                .then_some("The device settings could not be loaded."),
            save_error: matches!(notice, PreferencesNotice::SaveFailed)
                .then_some("Please try again."),
            worldview_changed: self
                .preferences_sheet
                .as_ref()
                .and_then(|sheet| sheet.worldview_changed),
            worldview_sharing: self.worldview_view(),
            diagnostics: preferences_view::Diagnostics::default(),
            about: preferences_view::About::default(),
            max_zoom: self.zoom.screen_width.map(Zoom::largest_for_screen),
        }
    }

    fn close_preferences_sheet(&mut self) -> Task<Message> {
        let Some(sheet) = self.preferences_sheet.take() else {
            return Task::none();
        };
        self.publish_preferences_sheet();
        if self.overlay_origin_is_current(&sheet.origin) {
            if let Some(focus) = sheet.origin.focus {
                return iced_runtime::task::widget(
                    iced::advanced::widget::operation::focusable::focus(focus),
                );
            }
        }
        Task::none()
    }

    fn close_connect_sheet(&mut self) -> Task<Message> {
        let Some(sheet) = self.connect_sheet.take() else {
            return Task::none();
        };
        if matches!(
            sheet.page,
            ConnectSheetPage::Sections(ConnectSheetSection::Duress)
        ) {
            self.scrub_connect_sheet_duress(&sheet.origin);
        }
        if self.overlay_origin_is_current(&sheet.origin) {
            if let Some(focus) = self.connect_return_focus(&sheet.origin) {
                return iced_runtime::task::widget(
                    iced::advanced::widget::operation::focusable::focus(focus),
                );
            }
        }
        Task::none()
    }

    /// Shows or clears the signed-out-by-Connect notice in every tab.
    fn relay_connect_signed_out_by(&mut self, reason: Option<crate::app::SignedOutBy>) {
        for (_, pane) in self.panes.iter_mut() {
            pane.set_connect_signed_out_by(reason);
        }
    }

    fn connect_return_focus(&self, origin: &OverlayOrigin) -> Option<iced::advanced::widget::Id> {
        let saved = origin.focus.as_ref()?;
        let tab = self
            .panes
            .get(origin.pane)?
            .tabs
            .iter()
            .find(|tab| tab.id == origin.tab_id)?;
        match &tab.state {
            // The Launcher renders one of two Connect controls in each mode.
            // Authentication may have replaced the control while the sheet was open.
            tab::State::Home(home) => home.connect_sheet_return_id(saved),
            // An open Cube has one Connect control in every state, so a
            // sign-in or sign-out while the sheet was open keeps its id.
            tab::State::App(_)
                if *saved
                    == iced::advanced::widget::Id::new(
                        crate::app::view::nav::shell::CONNECTION_ID,
                    ) =>
            {
                Some(saved.clone())
            }
            _ => Some(saved.clone()),
        }
    }

    fn open_connect_sheet(
        &mut self,
        section: ConnectSheetSection,
        focus: Option<iced::advanced::widget::Id>,
    ) -> Task<Message> {
        let Some(origin) = self.overlay_origin(focus) else {
            return Task::none();
        };
        let Some(account) = self.connect_panel(&origin) else {
            return Task::none();
        };
        let section = if section == ConnectSheetSection::Duress
            && !(account.is_authenticated() && account.show_duress())
        {
            ConnectSheetSection::Account
        } else {
            section
        };
        let initialize = self.connect_panel(&origin).is_some_and(|account| {
            matches!(
                &account.step,
                crate::app::state::connect::ConnectFlowStep::CheckingSession
            )
        });
        self.preferences_sheet = None;
        self.publish_preferences_sheet();
        self.connect_sheet = Some(ConnectSheet {
            origin: origin.clone(),
            page: ConnectSheetPage::Sections(section),
        });
        let focus =
            iced_runtime::task::widget(iced::advanced::widget::operation::focusable::focus(
                iced::advanced::widget::Id::new(CLOSE_ID),
            ));
        let load = self.load_connect_section(&origin, section);
        if !initialize {
            return Task::batch([focus, load]);
        }
        let inner = match origin.source {
            tab::PreferencesSource::Home => tab::Message::Launch(home::Message::View(
                home::ViewMessage::ConnectAccount(crate::app::view::ConnectAccountMessage::Init),
            )),
            tab::PreferencesSource::App(_) => {
                tab::Message::Run(AppMessage::View(crate::app::view::Message::ConnectAccount(
                    crate::app::view::ConnectAccountMessage::Init,
                )))
            }
        };
        Task::batch([
            focus,
            load,
            Task::done(Message::Pane(
                origin.pane,
                pane::Message::tab(origin.tab_id, inner),
            )),
        ])
    }

    fn open_connect_sheet_from(
        &mut self,
        pane_id: pane_grid::Pane,
        tab_id: usize,
        section: ConnectSheetSection,
        focus: Option<iced::advanced::widget::Id>,
    ) -> Task<Message> {
        let Some(origin) = self.overlay_origin(None) else {
            return Task::none();
        };
        if origin.pane != pane_id || origin.tab_id != tab_id {
            return Task::none();
        }
        self.open_connect_sheet(section, focus)
    }

    fn connect_sheet_message(&mut self, message: ConnectSheetMessage) -> Task<Message> {
        let Some(sheet) = &self.connect_sheet else {
            return Task::none();
        };
        if !self.overlay_origin_is_current(&sheet.origin) {
            if let Some(sheet) = self.connect_sheet.take() {
                if matches!(
                    sheet.page,
                    ConnectSheetPage::Sections(ConnectSheetSection::Duress)
                ) {
                    self.scrub_connect_sheet_duress(&sheet.origin);
                }
            }
            return Task::none();
        }
        match message {
            ConnectSheetMessage::FocusAction(id) => {
                iced_runtime::task::widget(iced::advanced::widget::operation::focusable::focus(id))
            }
            ConnectSheetMessage::Close => self.close_connect_sheet(),
            // Not now: the red light clears in every tab and the sheet closes.
            ConnectSheetMessage::DismissSignOut => {
                self.relay_connect_signed_out_by(None);
                self.close_connect_sheet()
            }
            ConnectSheetMessage::SelectSection(section) => {
                let origin = sheet.origin.clone();
                let was_duress = matches!(
                    sheet.page,
                    ConnectSheetPage::Sections(ConnectSheetSection::Duress)
                );
                let section = if section == ConnectSheetSection::Duress
                    && !self
                        .connect_panel(&origin)
                        .is_some_and(|account| account.is_authenticated() && account.show_duress())
                {
                    ConnectSheetSection::Account
                } else {
                    section
                };
                if was_duress && section != ConnectSheetSection::Duress {
                    self.scrub_connect_sheet_duress(&origin);
                }
                if let Some(sheet) = &mut self.connect_sheet {
                    sheet.page = ConnectSheetPage::Sections(section);
                }
                self.load_connect_section(&origin, section)
            }
            ConnectSheetMessage::OpenAuthentication { create_account } => {
                if matches!(
                    sheet.page,
                    ConnectSheetPage::Sections(ConnectSheetSection::Duress)
                ) {
                    let origin = sheet.origin.clone();
                    self.scrub_connect_sheet_duress(&origin);
                }
                if let Some(sheet) = &mut self.connect_sheet {
                    sheet.page = ConnectSheetPage::Authentication;
                }
                let update =
                    self.connect_sheet_message(ConnectSheetMessage::Account(if create_account {
                        crate::app::view::ConnectAccountMessage::CreateAccount
                    } else {
                        crate::app::view::ConnectAccountMessage::Init
                    }));
                Task::batch([
                    update,
                    iced_runtime::task::widget(
                        iced::advanced::widget::operation::focusable::focus(
                            iced::advanced::widget::Id::new(
                                crate::app::view::connect::EMAIL_INPUT_ID,
                            ),
                        ),
                    ),
                ])
            }
            ConnectSheetMessage::Account(message) => {
                let origin = sheet.origin.clone();
                let plan_navigation = matches!(
                    &message,
                    crate::app::view::ConnectAccountMessage::OpenPlanBilling
                        | crate::app::view::ConnectAccountMessage::ComparePlans(_)
                        | crate::app::view::ConnectAccountMessage::RenewCurrentPlan
                );
                if plan_navigation
                    && matches!(
                        sheet.page,
                        ConnectSheetPage::Sections(ConnectSheetSection::Duress)
                    )
                {
                    self.scrub_connect_sheet_duress(&origin);
                }
                let inner = match origin.source {
                    tab::PreferencesSource::Home => {
                        tab::Message::Launch(home::Message::View(if plan_navigation {
                            home::ViewMessage::ConnectSheetAccount(message)
                        } else {
                            home::ViewMessage::ConnectAccount(message)
                        }))
                    }
                    tab::PreferencesSource::App(_) => tab::Message::Run(AppMessage::View(
                        crate::app::view::Message::ConnectAccount(message),
                    )),
                };
                let task = self.update(Message::Pane(
                    origin.pane,
                    pane::Message::tab(origin.tab_id, inner),
                ));
                if plan_navigation && self.overlay_origin_is_current(&origin) {
                    if let Some(sheet) = &mut self.connect_sheet {
                        sheet.page = ConnectSheetPage::Sections(ConnectSheetSection::PlanBilling);
                    }
                }
                task
            }
        }
    }

    fn scrub_connect_sheet_duress(&mut self, origin: &OverlayOrigin) {
        let Some(tab) = self
            .panes
            .get_mut(origin.pane)
            .and_then(|pane| pane.tabs.iter_mut().find(|tab| tab.id == origin.tab_id))
        else {
            return;
        };
        match &mut tab.state {
            tab::State::Home(home) => home.connect_account.scrub_duress_dialogs(),
            tab::State::App(app) => app.connect_sheet_account_mut().scrub_duress_dialogs(),
            _ => {}
        }
    }

    /// Feature, plan, and auth completions arrive through ordinary Pane
    /// messages as well as sheet actions. Reconcile the retained sheet route
    /// after every dispatch so a revoked Duress surface cannot reappear if a
    /// later response turns its gate back on.
    fn reconcile_connect_sheet_duress(&mut self) {
        let Some(sheet) = self.connect_sheet.as_ref() else {
            return;
        };
        if !matches!(
            sheet.page,
            ConnectSheetPage::Sections(ConnectSheetSection::Duress)
        ) {
            return;
        }
        let origin = sheet.origin.clone();
        if self
            .connect_panel(&origin)
            .is_some_and(|account| account.is_authenticated() && account.show_duress())
        {
            return;
        }
        if let Some(sheet) = &mut self.connect_sheet {
            sheet.page = ConnectSheetPage::Sections(ConnectSheetSection::Account);
        }
        self.scrub_connect_sheet_duress(&origin);
        if let Some(tab) = self
            .panes
            .get_mut(origin.pane)
            .and_then(|pane| pane.tabs.iter_mut().find(|tab| tab.id == origin.tab_id))
        {
            let account = match &mut tab.state {
                tab::State::Home(home) => &mut home.connect_account,
                tab::State::App(app) => app.connect_sheet_account_mut(),
                _ => return,
            };
            account.active_sub = crate::app::menu::ConnectSubMenu::Overview;
        }
    }

    fn load_connect_section(
        &mut self,
        origin: &OverlayOrigin,
        section: ConnectSheetSection,
    ) -> Task<Message> {
        use crate::app::{menu::ConnectSubMenu, state::connect::account::load_security_data};
        let Some(tab) = self
            .panes
            .get_mut(origin.pane)
            .and_then(|pane| pane.tabs.iter_mut().find(|tab| tab.id == origin.tab_id))
        else {
            return Task::none();
        };
        let (task, app) = {
            let (account, app) = match &mut tab.state {
                tab::State::Home(home) => (&mut home.connect_account, false),
                tab::State::App(app) => (app.connect_sheet_account_mut(), true),
                _ => return Task::none(),
            };
            let section = if section == ConnectSheetSection::Duress
                && !(account.is_authenticated() && account.show_duress())
            {
                ConnectSheetSection::Account
            } else {
                section
            };
            account.active_sub = match section {
                ConnectSheetSection::Account | ConnectSheetSection::RecoveryServices => {
                    ConnectSubMenu::Overview
                }
                ConnectSheetSection::PlanBilling => ConnectSubMenu::PlanBilling,
                ConnectSheetSection::Contacts => ConnectSubMenu::Contacts,
                ConnectSheetSection::DevicesSecurity => ConnectSubMenu::Security,
                ConnectSheetSection::Duress => ConnectSubMenu::Duress,
            };
            if !account.is_authenticated() {
                return Task::none();
            }
            let task = match section {
                ConnectSheetSection::Account | ConnectSheetSection::RecoveryServices => {
                    account.reload_overview()
                }
                ConnectSheetSection::PlanBilling => account.reload_plan_billing(),
                ConnectSheetSection::Contacts => account.reload_contacts(),
                ConnectSheetSection::DevicesSecurity => {
                    load_security_data(&account.client, account.session_generation())
                }
                ConnectSheetSection::Duress => Task::batch([
                    account.reload_duress_contacts(),
                    account.reload_duress_cubes(),
                    account.reload_duress_state(),
                ]),
            };
            (task, app)
        };
        let task = if app {
            tab.emit_app_task(task)
        } else {
            task.map(|message| match message {
                AppMessage::View(crate::app::view::Message::ConnectAccount(message)) => {
                    tab::Message::Launch(home::Message::View(home::ViewMessage::ConnectAccount(
                        message,
                    )))
                }
                _ => {
                    log::warn!("Unexpected account section load result in Launcher");
                    tab::Message::Launch(home::Message::View(home::ViewMessage::Check))
                }
            })
        };
        let pane_id = origin.pane;
        let tab_id = origin.tab_id;
        task.map(move |message| Message::Pane(pane_id, pane::Message::tab(tab_id, message)))
    }

    fn preferences_sheet_message(&mut self, message: preferences_view::Message) -> Task<Message> {
        if !self
            .preferences_sheet
            .as_ref()
            .is_some_and(|sheet| self.overlay_origin_is_current(&sheet.origin))
        {
            return Task::none();
        }
        use preferences_view::{Action, Message as ViewMessage};
        match message {
            ViewMessage::FocusAction(id) => {
                iced_runtime::task::widget(iced::advanced::widget::operation::focusable::focus(id))
            }
            ViewMessage::SelectSection(section) => {
                if let Some(sheet) = &mut self.preferences_sheet {
                    sheet.page.select(section);
                }
                Task::none()
            }
            ViewMessage::Change(change) => self.change_preference(change),
            ViewMessage::ToggleComponentVersions => {
                if let Some(sheet) = &mut self.preferences_sheet {
                    sheet.page.toggle_component_versions();
                }
                Task::none()
            }
            ViewMessage::ToggleRegistrationRepair => {
                if let Some(sheet) = &mut self.preferences_sheet {
                    sheet.page.toggle_registration_repair();
                }
                Task::none()
            }
            ViewMessage::Action(Action::RefreshWorldview) => {
                self.worldview_action(worldview::Action::Refresh)
            }
            ViewMessage::Action(Action::PublishWorldview) => {
                self.worldview_action(worldview::Action::Publish)
            }
            ViewMessage::Action(Action::RevokeWorldview) => {
                self.worldview_action(worldview::Action::Revoke)
            }
            ViewMessage::Action(Action::Close) => self.close_preferences_sheet(),
            ViewMessage::Action(Action::RetryLoad) => self.retry_preferences(),
            // These remain disabled until their real diagnostic owners supply
            // an explicit capability to the sheet's view model.
            ViewMessage::Action(_) => Task::none(),
        }
    }

    /// Retries `load_or_initialize_preferences` on the preferences lane.
    /// While the lane is busy this is refused, not queued.
    pub fn retry_preferences(&mut self) -> Task<Message> {
        match self.preferences.begin_initialize() {
            Ok(request) => {
                self.publish_balance_privacy();
                self.run_preferences(request)
            }
            Err(_) => Task::none(),
        }
    }

    /// One explicit device-preference change on the same lane. An unresolved
    /// or busy privacy authority cannot originate a GUI preference change.
    pub fn change_preference(
        &mut self,
        change: crate::app::settings::preferences::PreferenceChange,
    ) -> Task<Message> {
        if let crate::app::settings::preferences::PreferenceChange::Zoom(zoom) = change {
            // The row disables steps over the cap; refuse one that arrives anyway.
            if zoom > self.max_zoom() {
                return Task::none();
            }
            self.zoom.pending = Some(zoom);
            return self.save_pending_zoom();
        }
        if matches!(
            change,
            crate::app::settings::preferences::PreferenceChange::Worldview(_)
        ) {
            if let Some(sheet) = &mut self.preferences_sheet {
                sheet.worldview_changed = None;
            }
        }
        match self.preferences.begin_save(change) {
            Ok(request) => {
                self.worldview_record_save(&request);
                self.publish_balance_privacy();
                self.run_preferences(request)
            }
            Err(BeginSaveError::Queued) => {
                self.worldview_queue_save(change);
                self.publish_balance_privacy();
                Task::none()
            }
            Err(_) => Task::none(),
        }
    }

    /// The zoom in effect: the newest unsaved choice, else the saved one,
    /// capped at what this screen holds. A 150% saved on a large monitor reads
    /// as 125% on a laptop, and returns on the monitor without a new save.
    fn current_zoom(&self) -> Zoom {
        self.zoom
            .pending
            .or_else(|| {
                self.preferences
                    .accepted()
                    .map(|accepted| accepted.preferences.zoom)
            })
            .unwrap_or_default()
            .min(self.max_zoom())
    }

    /// The largest zoom this window's screen can lay out at full width.
    /// Uncapped until iced reports the screen.
    fn max_zoom(&self) -> Zoom {
        self.zoom
            .screen_width
            .map_or(Zoom::LARGEST, Zoom::largest_for_screen)
    }

    /// Asks iced for the window's screen, tagged with the zoom its answer is
    /// scaled by.
    fn query_screen_size(&self) -> Task<Message> {
        let Some(id) = self.window_id else {
            return Task::none();
        };
        let zoom = self.current_zoom();
        window::monitor_size(id).map(move |size| Message::ScreenSize(size, zoom))
    }

    /// Keeps the minimum window at [`Zoom::LAYOUT_MIN_WIDTH`] laid-out points
    /// wide, so a capped zoom never lays a page out narrower than its design.
    /// Waits for the screen size: before it, the zoom is not yet capped and
    /// `main.rs`'s minimum stands.
    fn sync_min_window_size(&mut self) -> Task<Message> {
        let (Some(id), Some(_)) = (self.window_id, self.zoom.screen_width) else {
            return Task::none();
        };
        let zoom = self.current_zoom();
        if self.zoom.min_size_for == Some(zoom) {
            return Task::none();
        }
        self.zoom.min_size_for = Some(zoom);
        window::set_min_size(id, Some(Self::min_window_size(zoom)))
    }

    /// The minimum window in iced's zoomed units: never under 1024 x 768
    /// screen points, and at least 1024 laid-out points wide above 100%.
    fn min_window_size(zoom: Zoom) -> Size {
        Size::new(
            Zoom::LAYOUT_MIN_WIDTH * zoom.max(Zoom::ACTUAL_SIZE).factor(),
            MIN_WINDOW_HEIGHT,
        ) * (1.0 / zoom.factor())
    }

    /// A size at 100% in the zoomed units iced's window actions take.
    fn zoomed(&self, size: Size) -> Size {
        size * (1.0 / self.current_zoom().factor())
    }

    /// Saves the pending zoom once the lane is free. Called again after every
    /// preferences completion, so a refused request is retried, not dropped.
    fn save_pending_zoom(&mut self) -> Task<Message> {
        use crate::app::settings::preferences::PreferenceChange;
        let Some(zoom) = self.zoom.pending else {
            return Task::none();
        };
        if self
            .preferences
            .accepted()
            .is_some_and(|accepted| accepted.preferences.zoom == zoom)
        {
            self.zoom.pending = None;
            return Task::none();
        }
        if self.preferences.is_busy() || self.preferences.has_queued_changes() {
            return Task::none();
        }
        match self.preferences.begin_save(PreferenceChange::Zoom(zoom)) {
            Ok(request) => {
                // The lane is now held: the balance toggle must show it busy,
                // or a Hide clicked during this save is refused unseen.
                self.publish_balance_privacy();
                self.run_preferences(request)
            }
            Err(_) => Task::none(),
        }
    }

    fn change_balance_privacy(
        &mut self,
        change: crate::app::settings::preferences::PreferenceChange,
    ) -> Task<Message> {
        match self.preferences.begin_save(change) {
            Ok(request) => {
                self.publish_balance_privacy();
                self.run_preferences(request)
            }
            Err(BeginSaveError::Queued) => {
                self.publish_balance_privacy();
                Task::none()
            }
            Err(_) => Task::none(),
        }
    }

    fn run_preferences(&self, request: SaveRequest) -> Task<Message> {
        let root = self.config.coincube_directory.clone();
        let work = request.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || match work.operation {
                    Operation::Initialize => GlobalSettings::load_or_initialize_preferences(&root)
                        .map_err(|error| error.to_string()),
                    Operation::Save(change) => GlobalSettings::save_preference(&work.root, change)
                        .map_err(|error| error.to_string()),
                })
                .await
                .unwrap_or_else(|_| Err("The preferences task did not finish.".to_string()))
            },
            move |result| Message::Preferences(request, result),
        )
    }

    /// Messages, errors and dialog titles composed while handling a message
    /// use the window's language, like the view. Text that is saved, sent or
    /// compared must stay English: build it in `Language::English` or keep it
    /// a code and localize it at display.
    pub fn update(&mut self, message: Message) -> Task<Message> {
        let (language, worldview) = (self.language(), self.worldview());
        coincube_ui::locale::render_in(language, || {
            coincube_ui::worldview_copy::render_in(worldview, || self.update_in_language(message))
        })
    }

    fn update_in_language(&mut self, message: Message) -> Task<Message> {
        if self
            .preferences_sheet
            .as_ref()
            .is_some_and(|sheet| !self.overlay_origin_is_current(&sheet.origin))
        {
            self.preferences_sheet = None;
            self.publish_preferences_sheet();
        }
        if self
            .connect_sheet
            .as_ref()
            .is_some_and(|sheet| !self.overlay_origin_is_current(&sheet.origin))
        {
            if let Some(sheet) = self.connect_sheet.take() {
                if matches!(
                    sheet.page,
                    ConnectSheetPage::Sections(ConnectSheetSection::Duress)
                ) {
                    self.scrub_connect_sheet_duress(&sheet.origin);
                }
            }
        }
        // A completed classifier is an emergency even if Back closed its
        // PinEntry or a different Cube reopened at the same tab id. The proof
        // was minted by the captured trial, and carries no seed into Iced.
        if matches!(
            &message,
            Message::Pane(_, pane::Message::Tab(_, inner))
                if matches!(
                    inner.as_ref(),
                    tab::Message::PinEntry(
                        crate::pin_entry::Message::Classified(_, Ok(crate::pin_entry::Verdict::Duress(_)))
                            | crate::pin_entry::Message::ForkClassified(_, Ok(crate::pin_entry::Verdict::Duress(_)))
                    )
                )
        ) {
            let Message::Pane(pane_id, pane::Message::Tab(tab_id, inner)) = message else {
                unreachable!()
            };
            let tab::Message::PinEntry(classified) = *inner else {
                unreachable!()
            };
            let proof = match classified {
                crate::pin_entry::Message::Classified(
                    _,
                    Ok(crate::pin_entry::Verdict::Duress(proof)),
                )
                | crate::pin_entry::Message::ForkClassified(
                    _,
                    Ok(crate::pin_entry::Verdict::Duress(proof)),
                ) => proof,
                _ => unreachable!(),
            };
            return self.route_verified_unlock_duress(pane_id, tab_id, proof);
        }
        // Intercept before stripping AppAsync/ForkAsync so a queued eye press
        // from a closed or replaced Cube cannot mutate device preferences.
        if let Message::Pane(pane_id, pane::Message::Tab(tab_id, inner)) = &message {
            let tab = self
                .panes
                .get(*pane_id)
                .and_then(|pane| pane.tabs.iter().find(|tab| tab.id == *tab_id));
            if let Some((change, current)) = balance_privacy_eye_change(tab, inner) {
                return if current {
                    self.change_balance_privacy(change)
                } else {
                    Task::none()
                };
            }
            if let tab::Message::HomeDevicePreference(change) = inner.as_ref() {
                return if launcher_change_is_current(tab, change) {
                    self.change_preference(*change)
                } else {
                    Task::none()
                };
            }
        }
        // Validate session-tagged App callbacks before inspecting global auth
        // actions. A queued event from an old App cannot affect a reopened one.
        let message = match message {
            Message::Pane(pane_id, pane::Message::Tab(tab_id, tab_message))
                if matches!(tab_message.as_ref(), tab::Message::AppAsync(..)) =>
            {
                let tab::Message::AppAsync(generation, message) = *tab_message else {
                    unreachable!();
                };
                if let tab::Message::Run(AppMessage::View(
                    crate::app::view::Message::DuressActivateLocal(_),
                )) = message.as_ref()
                {
                    let tab::Message::Run(AppMessage::View(
                        crate::app::view::Message::DuressActivateLocal(proof),
                    )) = *message
                    else {
                        unreachable!()
                    };
                    return self.route_verified_seed_export_duress(pane_id, tab_id, proof);
                }
                let current = self
                    .panes
                    .get(pane_id)
                    .and_then(|pane| pane.tabs.iter().find(|tab| tab.id == tab_id))
                    .is_some_and(|tab| tab.accepts_app_generation(generation));
                if !current {
                    let closed_trial_finished = is_seed_export_trial_result(message.as_ref());
                    tab::discard_closed_loader_result(*message);
                    return if closed_trial_finished {
                        if let Some(tab) = self
                            .panes
                            .get_mut(pane_id)
                            .and_then(|pane| pane.tabs.iter_mut().find(|tab| tab.id == tab_id))
                        {
                            tab.finish_stale_seed_export_trial();
                            Task::none()
                        } else {
                            self.finish_closed_seed_export_trial(pane_id, tab_id)
                        }
                    } else {
                        Task::none()
                    };
                }
                route_guarded_tab_result(pane_id, tab_id, *message)
            }
            Message::Pane(pane_id, pane::Message::Tab(tab_id, tab_message))
                if matches!(tab_message.as_ref(), tab::Message::ForkAsync(..)) =>
            {
                let tab::Message::ForkAsync(generation, message) = *tab_message else {
                    unreachable!();
                };
                if let tab::Message::Run(AppMessage::View(
                    crate::app::view::Message::DuressActivateLocal(_),
                )) = message.as_ref()
                {
                    let tab::Message::Run(AppMessage::View(
                        crate::app::view::Message::DuressActivateLocal(proof),
                    )) = *message
                    else {
                        unreachable!()
                    };
                    return self.route_verified_seed_export_duress(pane_id, tab_id, proof);
                }
                let current = self
                    .panes
                    .get(pane_id)
                    .and_then(|pane| pane.tabs.iter().find(|tab| tab.id == tab_id))
                    .is_some_and(|tab| tab.accepts_fork_generation(generation));
                if !current {
                    let closed_trial_finished = is_seed_export_trial_result(message.as_ref());
                    tab::discard_closed_loader_result(*message);
                    return if closed_trial_finished {
                        if let Some(tab) = self
                            .panes
                            .get_mut(pane_id)
                            .and_then(|pane| pane.tabs.iter_mut().find(|tab| tab.id == tab_id))
                        {
                            tab.finish_stale_seed_export_trial();
                            Task::none()
                        } else {
                            self.finish_closed_seed_export_trial(pane_id, tab_id)
                        }
                    } else {
                        Task::none()
                    };
                }
                route_guarded_tab_result(pane_id, tab_id, *message)
            }
            Message::Pane(pane_id, pane::Message::Tab(tab_id, tab_message))
                if !self
                    .panes
                    .get(pane_id)
                    .is_some_and(|pane| pane.tabs.iter().any(|tab| tab.id == tab_id)) =>
            {
                tab::discard_closed_loader_result(*tab_message);
                return Task::none();
            }
            message => message,
        };
        // Every hosted fork PIN starts with a completed, scoped account
        // candidate. A cold Home can borrow one unambiguous verified sibling;
        // an account still being checked cannot bypass its gate that way.
        let opening_pin = match &message {
            Message::Pane(pane_id, pane::Message::Tab(tab_id, inner)) => match inner.as_ref() {
                tab::Message::Launch(
                    home::Message::Run(root, _, chain, _)
                    | home::Message::RunFromFloor(root, _, chain, _, _)
                    | home::Message::RunToRecovery(root, _, chain, _),
                ) if chain.is_blake2b() => {
                    self.panes
                        .get(*pane_id)
                        .and_then(|pane| pane.tabs.iter().find(|tab| tab.id == *tab_id))
                        .and_then(|tab| match &tab.state {
                            tab::State::Home(home) => {
                                let candidate = if home.connect_account.is_authenticated() {
                                    self.verified_unlock_origin(*pane_id, *tab_id)
                                } else if home.connect_account.authenticated_client().is_none() {
                                    self.unique_unlock_candidate(
                                        root,
                                        &home.connect_account.client.base_url,
                                    )
                                } else {
                                    // A retained token while this Home's account
                                    // gate is checking or closed is not admission.
                                    None
                                };
                                candidate.map(|candidate| (*pane_id, *tab_id, candidate))
                            }
                            _ => None,
                        })
                }
                _ => None,
            },
            _ => None,
        };
        // Shared-client restore reaches readiness without SetSession. Capture
        // its exact current origin before Pane converts this to an originless intent.
        let ready_unlock = match &message {
            Message::Pane(pane_id, pane::Message::Tab(tab_id, inner))
                if matches!(
                    inner.as_ref(),
                    tab::Message::Launch(home::Message::ConnectSignedInBubble)
                        | tab::Message::ConnectSignedIn
                ) =>
            {
                self.verified_unlock_origin(*pane_id, *tab_id)
            }
            _ => None,
        };
        // Reuse a sibling session when opening Home or a Bitcoin Cube. Separate
        // keyring refreshes race token rotation and can leave older Cubes with
        // an unauthenticated account panel. Each adopter still checks /user.
        let session_init = match &message {
            Message::Pane(pane_id, pane::Message::Tab(tab_id, inner))
                if matches!(
                    inner.as_ref(),
                    tab::Message::Launch(home::Message::View(home::ViewMessage::ConnectAccount(
                        crate::app::view::ConnectAccountMessage::Init
                    ))) | tab::Message::Run(AppMessage::View(
                        crate::app::view::Message::ConnectAccount(
                            crate::app::view::ConnectAccountMessage::Init
                        )
                    ))
                ) =>
            {
                Some((*pane_id, *tab_id))
            }
            _ => None,
        };
        if let Some((origin_pane, origin_tab)) = session_init {
            let active_client = self.panes.iter().find_map(|(&pane_id, pane)| {
                pane.tabs.iter().find_map(|tab| {
                    if pane_id == origin_pane && tab.id == origin_tab {
                        return None;
                    }
                    (match &tab.state {
                        tab::State::App(app) => app.shareable_connect_client(),
                        tab::State::Home(home) => home.connect_account.authenticated_client(),
                        _ => None,
                    })
                    .filter(|client| client.token().is_some_and(|token| !token.is_empty()))
                })
            });
            if let Some(client) = active_client {
                if let Some(tab) = self
                    .panes
                    .get_mut(origin_pane)
                    .and_then(|pane| pane.tabs.iter_mut().find(|tab| tab.id == origin_tab))
                {
                    let adopted = match &mut tab.state {
                        tab::State::Home(home)
                            if home.connect_account.can_adopt_shared_session() =>
                        {
                            Some(home.adopt_connect_client(client).map(tab::Message::Launch))
                        }
                        tab::State::App(app) if app.can_adopt_shared_connect_session() => {
                            let task = app.adopt_shared_connect_client(client);
                            Some(tab.emit_app_task(task))
                        }
                        _ => None,
                    };
                    if let Some(task) = adopted {
                        return task.map(move |message| {
                            Message::Pane(origin_pane, pane::Message::tab(origin_tab, message))
                        });
                    }
                }
            }
        }
        // The signed-out-by-Connect notice is one notice for the whole app
        // (L3): whichever tab learns it, or dismisses it, every tab follows.
        if let Message::Pane(_, pane::Message::Tab(_, inner)) = &message {
            let relay = match inner.as_ref() {
                tab::Message::Run(AppMessage::ConnectSignedOutBy(reason))
                | tab::Message::Launch(home::Message::ConnectSignedOutBy(reason)) => {
                    Some(Some(*reason))
                }
                tab::Message::Run(AppMessage::View(
                    crate::app::view::Message::DismissConnectSignOut,
                ))
                | tab::Message::Launch(home::Message::View(
                    home::ViewMessage::DismissConnectSignOut,
                )) => Some(None),
                _ => None,
            };
            if let Some(reason) = relay {
                self.relay_connect_signed_out_by(reason);
            }
        }
        // A log-out or a sign-in from any tab reaches every tab, with its kind
        // and where it came from (`Tab::invalidate_fork_session`). A
        // `SetSession` counts as a sign-in only if the tab it came from will
        // accept it: its account panel drops a completion of an operation
        // begun before that tab's last auth invalidation
        // (`ConnectAccountPanel::stale_completion`), and a completion the
        // panel is about to drop must not tell the other tabs anything — this
        // is the first reader of the variant, ahead of that dispatch.
        let auth_change = match &message {
            Message::Pane(pane_id, pane::Message::Tab(tab_id, inner)) => {
                match connect_account_message(inner) {
                    Some(msg) => match msg {
                        crate::app::view::ConnectAccountMessage::LogOut => {
                            Some((tab::AuthChange::LogOut, *pane_id, *tab_id))
                        }
                        crate::app::view::ConnectAccountMessage::SetSession(login, epoch)
                        | crate::app::view::ConnectAccountMessage::OtpVerified(login, epoch, _)
                        | crate::app::view::ConnectAccountMessage::AuthCommitAccepted {
                            login,
                            epoch,
                            refreshed: false,
                        } => {
                            let origin_is_home = self
                                .panes
                                .get(*pane_id)
                                .and_then(|pane| pane.tabs.iter().find(|tab| tab.id == *tab_id))
                                .is_some_and(|tab| matches!(&tab.state, tab::State::Home(_)));
                            let eligible = !matches!(
                                msg,
                                crate::app::view::ConnectAccountMessage::OtpVerified(..)
                            ) || origin_is_home;
                            let origin_epoch = self
                                .panes
                                .get(*pane_id)
                                .and_then(|pane| pane.tabs.iter().find(|tab| tab.id == *tab_id))
                                .and_then(|tab| match &tab.state {
                                    tab::State::App(app) => Some(app.connect_auth_epoch()),
                                    tab::State::Home(home) => {
                                        Some(home.connect_account.auth_epoch())
                                    }
                                    _ => None,
                                });
                            let same_fork_session = self
                        .panes
                        .get(*pane_id)
                        .and_then(|pane| pane.tabs.iter().find(|tab| tab.id == *tab_id))
                        .is_some_and(|tab| {
                            matches!(&tab.state, tab::State::App(app) if app.matches_admitted_connect_session(login))
                        });
                            match origin_epoch {
                                Some(current)
                                    if *epoch >= current && !same_fork_session && eligible =>
                                {
                                    Some((
                                        tab::AuthChange::SignIn {
                                            user_id: login.user.id,
                                            token: zeroize::Zeroizing::new(login.token.clone()),
                                        },
                                        *pane_id,
                                        *tab_id,
                                    ))
                                }
                                // Stale for its own tab (dropped there on dispatch), or
                                // no account panel to judge it: not a sign-in.
                                _ => None,
                            }
                        }
                        _ => None,
                    },
                    None => None,
                }
            }
            _ => None,
        };
        let auth_from_fork_app = if auth_change.is_some() {
            match &message {
                Message::Pane(pane_id, pane::Message::Tab(tab_id, _)) => self
                    .panes
                    .get(*pane_id)
                    .and_then(|pane| pane.tabs.iter().find(|tab| tab.id == *tab_id))
                    .is_some_and(
                        |tab| matches!(&tab.state, tab::State::App(app) if app.requires_connect()),
                    ),
                _ => false,
            }
        } else {
            false
        };
        let completed_login = match &message {
            Message::Pane(_, pane::Message::Tab(_, inner)) => {
                match connect_account_message(inner) {
                    Some(crate::app::view::ConnectAccountMessage::SetSession(login, _))
                    | Some(crate::app::view::ConnectAccountMessage::OtpVerified(login, _, _))
                    | Some(crate::app::view::ConnectAccountMessage::AuthCommitAccepted {
                        login,
                        refreshed: false,
                        ..
                    }) => Some(login.clone()),
                    _ => None,
                }
            }
            _ => None,
        };
        let accepted_before = auth_change.as_ref().and_then(|(_, pane_id, tab_id)| {
            self.panes
                .get(*pane_id)
                .and_then(|pane| pane.tabs.iter().find(|tab| tab.id == *tab_id))
                .and_then(|tab| match &tab.state {
                    tab::State::App(app) => {
                        Some(app.connect_sheet_account().accepted_auth_revision())
                    }
                    tab::State::Home(home) => Some(home.connect_account.accepted_auth_revision()),
                    _ => None,
                })
        });
        let mut auth_tasks = Vec::new();
        // The old fork App is gone. Apply its auth operation to the replacement
        // Home so logout clears saved login state instead of being discarded.
        let message = if auth_from_fork_app {
            match message {
                Message::Pane(pane_id, pane::Message::Tab(tab_id, inner)) => match *inner {
                    tab::Message::Run(AppMessage::View(
                        crate::app::view::Message::ConnectAccount(msg),
                    )) => Message::Pane(
                        pane_id,
                        pane::Message::tab(
                            tab_id,
                            tab::Message::Launch(home::Message::View(
                                home::ViewMessage::ConnectAccount(msg),
                            )),
                        ),
                    ),
                    other => Message::Pane(pane_id, pane::Message::tab(tab_id, other)),
                },
                message => message,
            }
        } else {
            message
        };
        // Both the scoped seed-export proof and the existing Cube-unlock PIN
        // verdict enter Duress before their async wipe task runs. The unlock
        // verdict has no private proof object, so its live PinEntry and exact
        // data root are the GUI admission boundary for the global surface seal.
        let unlock_duress_message = matches!(
            &message,
            Message::Pane(_, pane::Message::Tab(_, inner))
                if matches!(
                    inner.as_ref(),
                    tab::Message::PinEntry(crate::pin_entry::Message::DuressDetected { .. })
                )
        );
        let unlock_duress_target = match &message {
            Message::Pane(pane_id, pane::Message::Tab(tab_id, inner))
                if matches!(
                    inner.as_ref(),
                    tab::Message::PinEntry(crate::pin_entry::Message::DuressDetected { .. })
                ) =>
            {
                self.panes
                    .get(*pane_id)
                    .and_then(|pane| pane.tabs.iter().find(|tab| tab.id == *tab_id))
                    .and_then(|tab| match &tab.state {
                        tab::State::PinEntry(pin) => {
                            let crate::pin_entry::PinEntrySuccess::LoadApp { datadir, .. } =
                                &pin.on_success;
                            crate::chain::same_path(
                                datadir.path(),
                                self.config.coincube_directory.path(),
                            )
                            .then_some((*pane_id, *tab_id))
                        }
                        _ => None,
                    })
            }
            _ => None,
        };
        if unlock_duress_message && unlock_duress_target.is_none() {
            return Task::none();
        }
        // Dispatch may return early (notably for in-App account messages).
        // Keep those returns inside the helper so invalidation tasks always run.
        let routed = self.update_message(message);
        self.reconcile_connect_sheet_duress();
        if let Some((change, origin_pane, origin_tab)) = auth_change {
            let accepted = match &change {
                tab::AuthChange::SignIn { .. } => accepted_before.is_some_and(|before| {
                    self.panes
                        .get(origin_pane)
                        .and_then(|pane| pane.tabs.iter().find(|tab| tab.id == origin_tab))
                        .and_then(|tab| match &tab.state {
                            tab::State::App(app) => {
                                Some(app.connect_sheet_account().accepted_auth_revision())
                            }
                            tab::State::Home(home) => {
                                Some(home.connect_account.accepted_auth_revision())
                            }
                            _ => None,
                        })
                        .is_some_and(|after| after != before)
                }),
                tab::AuthChange::LogOut => true,
            };
            if accepted && matches!(change, tab::AuthChange::SignIn { .. }) {
                // A sign-in anywhere ends the signed-out notice everywhere.
                self.relay_connect_signed_out_by(None);
            }
            if accepted {
                for (&pane_id, pane) in self.panes.iter_mut() {
                    for tab in &mut pane.tabs {
                        if completed_login.as_ref().is_some_and(|login| {
                            matches!(&tab.state, tab::State::App(app) if app.matches_admitted_connect_session(login))
                        }) {
                            continue;
                        }
                        let tab_id = tab.id;
                        let originated = pane_id == origin_pane && tab_id == origin_tab;
                        auth_tasks.push(
                            tab.invalidate_fork_session(change.clone(), originated).map(
                                move |msg| Message::Pane(pane_id, pane::Message::tab(tab_id, msg)),
                            ),
                        );
                    }
                }
            }
        }
        if let Some((pane_id, tab_id)) = unlock_duress_target {
            self.seal_verified_seed_export_duress_surface(pane_id, tab_id);
        }
        auth_tasks.push(routed);
        if let Some(candidate) = ready_unlock {
            for (_, pane) in self.panes.iter_mut() {
                for tab in &mut pane.tabs {
                    if let tab::State::PinEntry(pin) = &mut tab.state {
                        pin.adopt_connect_candidate(
                            &candidate.root,
                            candidate.client.clone(),
                            candidate.user_id,
                        );
                    }
                }
            }
        }

        if let Some((pane_id, tab_id, candidate)) = opening_pin {
            if let Some(tab::Tab {
                state: tab::State::PinEntry(pin),
                ..
            }) = self
                .panes
                .get_mut(pane_id)
                .and_then(|pane| pane.tabs.iter_mut().find(|tab| tab.id == tab_id))
            {
                pin.adopt_connect_candidate(&candidate.root, candidate.client, candidate.user_id);
            }
        }

        self.refresh_sidebar_session_status();
        auth_tasks.push(self.sync_min_window_size());
        Task::batch(auth_tasks)
    }

    fn verified_unlock_origin(
        &self,
        pane_id: pane_grid::Pane,
        tab_id: usize,
    ) -> Option<ForkUnlockCandidate> {
        let tab = self
            .panes
            .get(pane_id)?
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)?;
        match &tab.state {
            tab::State::Home(home) if home.connect_account.is_authenticated() => {
                Some(ForkUnlockCandidate {
                    root: home.datadir_path.clone(),
                    user_id: home.connect_account.user.as_ref()?.id,
                    client: home.connect_account.authenticated_client()?,
                })
            }
            tab::State::App(app) => {
                app.shareable_connect_identity()
                    .map(|(user_id, client)| ForkUnlockCandidate {
                        root: app.datadir().clone(),
                        user_id,
                        client,
                    })
            }
            _ => None,
        }
    }

    /// A cold Home may borrow only one unambiguous completed identity.
    /// Conflicting account or token snapshots require a new account check.
    fn unique_unlock_candidate(
        &self,
        root: &CoincubeDirectory,
        provider: &str,
    ) -> Option<ForkUnlockCandidate> {
        let mut selected: Option<ForkUnlockCandidate> = None;
        for (_, pane) in self.panes.iter() {
            for tab in &pane.tabs {
                let identity = match &tab.state {
                    tab::State::Home(home) if home.connect_account.is_authenticated() => {
                        home.connect_account.user.as_ref().and_then(|user| {
                            home.connect_account
                                .authenticated_client()
                                .map(|client| (home.datadir_path.clone(), user.id, client))
                        })
                    }
                    tab::State::App(app) => app
                        .shareable_connect_identity()
                        .map(|(user_id, client)| (app.datadir().clone(), user_id, client)),
                    _ => None,
                };
                let Some((source_root, user_id, client)) = identity else {
                    continue;
                };
                if !crate::chain::same_path(root.path(), source_root.path())
                    || provider.trim_end_matches('/') != client.base_url.trim_end_matches('/')
                    || client.token().is_none_or(|token| token.trim().is_empty())
                {
                    continue;
                }
                if let Some(previous) = &selected {
                    if previous.user_id != user_id || previous.client.token() != client.token() {
                        return None;
                    }
                } else {
                    selected = Some(ForkUnlockCandidate {
                        root: source_root,
                        client,
                        user_id,
                    });
                }
            }
        }
        selected
    }

    // The footer describes signing in to the app, not the individual Cube's
    // service readiness. Keep it separate from operation/claim authorization.
    fn refresh_sidebar_session_status(&mut self) {
        let signed_in = self.panes.iter().any(|(_, pane)| {
            pane.tabs.iter().any(|tab| match &tab.state {
                tab::State::App(app) => {
                    app.cache().connect_authenticated && app.shareable_connect_client().is_some()
                }
                tab::State::Home(home) => home.connect_account.is_authenticated(),
                _ => false,
            })
        });
        for (_, pane) in self.panes.iter_mut() {
            for tab in &mut pane.tabs {
                if let tab::State::App(app) = &mut tab.state {
                    app.cache_mut().sidebar_connect_signed_in = signed_in;
                }
            }
        }
    }

    fn update_message(&mut self, message: Message) -> Task<Message> {
        match message {
            // we get this message only once at startup
            Message::Window(id) => {
                self.window_id = id;
                let screen = self.query_screen_size();
                // Common case: if there is an already saved screen size we reuse it
                let size = if let (Some(id), Some(WindowConfig { width, height })) =
                    (id, &self.window_config)
                {
                    window::resize(
                        id,
                        self.zoomed(Size {
                            width: *width,
                            height: *height,
                        }),
                    )
                // Initial startup: we maximize the screen in order to know the max usable screen area
                } else if let Some(id) = &self.window_id {
                    window::maximize(*id, true)
                } else {
                    Task::none()
                };
                Task::batch([screen, size])
            }
            Message::ScreenSize(size, zoom) => {
                // Iced measures the answer at the zoom in effect when it runs
                // the query, after the rest of its batch. If the zoom changed
                // since it was asked, the tag no longer scales it: ask again.
                if zoom != self.current_zoom() {
                    return self.query_screen_size();
                }
                if let Some(size) = size {
                    // Rounded: f32 scaling puts a 1536-point screen at 1535.9999.
                    self.zoom.screen_width = Some((size.width * zoom.factor()).round());
                }
                Task::none()
            }
            Message::WindowSize(monitor_size) => {
                // Iced reports the size in zoomed units; record it at 100% so
                // a zoom change between launches keeps the window's size.
                let monitor_size = monitor_size * self.current_zoom().factor();
                // A resize can come from a move to another screen.
                let screen = self.query_screen_size();
                let cloned_cfg = self.window_config.clone();
                let resized = match (cloned_cfg, &self.window_init, &self.window_id) {
                    // no previous screen size recorded && window maximized
                    (None, Some(false), Some(id)) => {
                        self.window_init = Some(true);
                        let mut batch = vec![window::maximize(*id, false)];
                        let new_size = if monitor_size.height >= 1200.0 {
                            let size = Size {
                                width: 1200.0,
                                height: 950.0,
                            };
                            batch.push(window::resize(*id, self.zoomed(size)));
                            size
                        } else {
                            batch.push(window::resize(
                                *id,
                                self.zoomed(iced::window::Settings::default().size),
                            ));
                            iced::window::Settings::default().size
                        };
                        self.window_config = Some(WindowConfig {
                            width: new_size.width,
                            height: new_size.height,
                        });
                        Task::batch(batch)
                    }
                    // we already have a record of the last window size and we update it
                    (Some(WindowConfig { width, height }), _, _) => {
                        if width != monitor_size.width || height != monitor_size.height {
                            if let Some(cfg) = &mut self.window_config {
                                cfg.width = monitor_size.width;
                                cfg.height = monitor_size.height;
                            }
                        }
                        Task::none()
                    }
                    // we ignore the first notification about initial window size it will always be
                    // the default one
                    _ => {
                        if self.window_init.is_none() {
                            self.window_init = Some(false);
                        }
                        Task::none()
                    }
                };
                Task::batch([screen, resized])
            }
            Message::WindowMoved => self.query_screen_size(),
            Message::CtrlC
            | Message::Event(iced::Event::Window(iced::window::Event::CloseRequested)) => {
                for (_, pane) in self.panes.iter_mut() {
                    pane.stop();
                }
                if let Some(window_config) = &self.window_config {
                    let path = GlobalSettings::path(&self.config.coincube_directory);
                    if let Err(e) = GlobalSettings::update_window_config(&path, window_config) {
                        tracing::error!("Failed to update the window config: {e}");
                    }
                }
                iced::window::latest().and_then(iced::window::close)
            }
            Message::KeyPressed(Key::Tab(shift)) => {
                if self.preferences_sheet.is_some() {
                    return if shift {
                        action::focus_previous_visible(preferences_view::CONTENT_SCROLL_ID)
                    } else {
                        action::focus_next_visible(preferences_view::CONTENT_SCROLL_ID)
                    };
                }
                if self.connect_sheet.is_some() {
                    return if shift {
                        action::focus_previous_visible(CONTENT_SCROLL_ID)
                    } else {
                        action::focus_next_visible(CONTENT_SCROLL_ID)
                    };
                }
                log::debug!("Tab pressed!");
                let focus = if shift {
                    action::focus_previous_visible(crate::app::view::nav::shell::MIDDLE_SCROLL_ID)
                } else {
                    action::focus_next_visible(crate::app::view::nav::shell::MIDDLE_SCROLL_ID)
                };
                // The Border Wallet grid has its own clipped viewport. Revealing
                // focus there is a no-op for every other screen.
                let focus = focus.chain(action::reveal_focused(
                    crate::installer::step::descriptor::editor::border_wallet_wizard::BORDER_WALLET_GRID_SCROLL_ID,
                ));
                // The Create form scrolls inside its card while its footer stays
                // fixed. Reveal keyboard focus for controls below that viewport.
                let focus = focus.chain(action::reveal_focused(crate::home::CREATE_BODY_SCROLL_ID));
                #[cfg(feature = "mcp-proposals")]
                let focus = focus.chain(action::reveal_focused(
                    crate::installer::proposal_handoff::SCROLL_ID,
                ));
                // The RBF warning has its own clipped descendant list. Tab can
                // reach every copy action, including those below its viewport.
                focus.chain(action::reveal_focused(
                    crate::app::view::vault::transactions::RBF_DESCENDANTS_SCROLL_ID,
                ))
            }
            Message::KeyPressed(Key::Zoom(step)) => {
                let current = self.current_zoom();
                let next = match step {
                    ZoomStep::In => current.zoomed_in().filter(|next| *next <= self.max_zoom()),
                    ZoomStep::Out => current.zoomed_out(),
                    ZoomStep::ActualSize => Some(Zoom::ACTUAL_SIZE),
                };
                // The pill also shows at the first and last step, so a press
                // that changes nothing still says where the zoom is.
                self.zoom.notice_until = Some(Instant::now() + ZOOM_NOTICE);
                match next.filter(|next| *next != current) {
                    Some(next) => {
                        self.zoom.pending = Some(next);
                        self.save_pending_zoom()
                    }
                    None => Task::none(),
                }
            }
            Message::KeyPressed(Key::Escape) => {
                if self.preferences_sheet.is_some() {
                    return self.close_preferences_sheet();
                }
                if self.connect_sheet.is_some() {
                    return self.close_connect_sheet();
                }
                let Some((pane_id, tab_id, generation, intent)) = self.focus.and_then(|pane_id| {
                    let pane = self.panes.get(pane_id)?;
                    let tab = pane.tabs.get(pane.focused_tab)?;
                    let tab::State::App(app) = &tab.state else {
                        return None;
                    };
                    Some((
                        pane_id,
                        tab.id,
                        tab.app_generation_token()?,
                        app.shell_escape_intent()?,
                    ))
                }) else {
                    return Task::none();
                };
                self.update(Message::Pane(
                    pane_id,
                    pane::Message::tab(
                        tab_id,
                        tab::Message::AppAsync(
                            generation,
                            Box::new(tab::Message::Run(AppMessage::View(intent))),
                        ),
                    ),
                ))
            }
            Message::Fiat(FiatMessage::GetPriceResult(price)) => {
                if price.request.origin.is_some() {
                    return Task::none();
                }
                if self
                    .global_cache
                    .pending_fiat_price_request(price.source(), price.currency())
                    != Some(&price.request)
                {
                    tracing::debug!(
                        "Ignoring fiat price result for {} from {} as it is not the last request",
                        price.currency(),
                        price.source(),
                    );
                    return Task::none();
                }
                match price.res.as_ref() {
                    Ok(res) => {
                        tracing::debug!(
                            "Fiat price request for {} from {} completed successfully: {:?}",
                            price.currency(),
                            price.source(),
                            res,
                        );
                    }
                    Err(e) => {
                        tracing::error!(
                            "Fiat price request for {} from {} returned error: {}",
                            price.currency(),
                            price.source(),
                            e
                        );
                    }
                }
                // Update the cache with the result even if there was an error.
                self.global_cache
                    .remove_fiat_price_request(price.source(), price.currency());
                self.global_cache.insert_fiat_price(price);
                Task::none()
            }
            Message::Fiat(FiatMessage::ListCurrenciesResult(
                pane_id,
                tab_id,
                source,
                instant,
                res,
            )) => {
                if let Ok(list) = res.as_ref() {
                    self.global_cache
                        .insert_currencies(source, instant, list.currencies.clone());
                }
                // Return the result to the tab even if there was an error.
                if let Some(pane) = self.panes.get_mut(pane_id) {
                    pane.update_tab_with_app_msg(
                        tab_id,
                        AppFiatMessage::ListCurrenciesResult(source, res),
                        &self.config,
                    )
                    .map(move |msg| Message::Pane(pane_id, msg))
                } else {
                    Task::none()
                }
            }
            Message::Pane(_, pane::Message::View(pane::ViewMessage::GlobalSettingsChanged)) => {
                self.reload_all_global_settings()
            }
            // A Home sign-in re-checks the Connect session of every open Cube,
            // in every pane: a split-off Cube is still an open Cube. Each Init
            // re-enters `update`, so it may adopt the verified sibling client.
            Message::Pane(_, pane::Message::View(pane::ViewMessage::ConnectSignedIn)) => {
                let mut tasks = Vec::new();
                for (&pane_id, pane) in self.panes.iter() {
                    for tab in &pane.tabs {
                        if matches!(&tab.state, tab::State::App(_)) {
                            let tab_id = tab.id;
                            tasks.push(
                                tab.emit_app_task(Task::done(AppMessage::View(
                                    crate::app::view::Message::ConnectAccount(
                                        crate::app::view::ConnectAccountMessage::Init,
                                    ),
                                )))
                                .map(move |message| {
                                    Message::Pane(pane_id, pane::Message::tab(tab_id, message))
                                }),
                            );
                        }
                    }
                }
                Task::batch(tasks)
            }
            // Fork tabs wrap task outputs with their session generation.
            // After validation/unwrapping, global intents can arrive directly
            // as tab messages rather than through Pane's task-output mapper.
            Message::Pane(pane_id, pane::Message::Tab(_, inner))
                if matches!(inner.as_ref(), tab::Message::ConnectSignedIn) =>
            {
                self.update(Message::Pane(
                    pane_id,
                    pane::Message::View(pane::ViewMessage::ConnectSignedIn),
                ))
            }
            Message::Pane(_, pane::Message::Tab(_, inner))
                if matches!(inner.as_ref(), tab::Message::ToggleTheme) =>
            {
                self.update(Message::ToggleTheme)
            }
            Message::Pane(_, pane::Message::View(pane::ViewMessage::ToggleTheme)) => {
                self.update(Message::ToggleTheme)
            }
            Message::Pane(pane_id, pane::Message::Tab(tab_id, inner))
                if matches!(
                    inner.as_ref(),
                    tab::Message::Run(AppMessage::View(
                        crate::app::view::Message::OpenConnectSignIn
                    ))
                ) && self.connect_overlay_eligible(pane_id) =>
            {
                let refresh = self.overlay_origin(None).is_some_and(|origin| {
                    origin.pane == pane_id
                        && origin.tab_id == tab_id
                        && self.connect_panel(&origin).is_some_and(|account| {
                            !account.is_authenticated() && !account.is_checking_session()
                        })
                });
                // The Connect control opened it, directly or from its panel.
                let source_id = crate::app::view::nav::shell::CONNECTION_ID;
                let open = self.open_connect_sheet_from(
                    pane_id,
                    tab_id,
                    ConnectSheetSection::Account,
                    Some(iced::advanced::widget::Id::new(source_id)),
                );
                if refresh {
                    Task::batch([
                        open,
                        Task::done(Message::Pane(
                            pane_id,
                            pane::Message::tab(
                                tab_id,
                                tab::Message::Run(AppMessage::View(
                                    crate::app::view::Message::ConnectAccount(
                                        crate::app::view::ConnectAccountMessage::Init,
                                    ),
                                )),
                            ),
                        )),
                    ])
                } else {
                    open
                }
            }
            Message::Pane(pane_id, pane::Message::Tab(tab_id, inner))
                if matches!(
                    inner.as_ref(),
                    tab::Message::Run(AppMessage::View(
                        crate::app::view::Message::OpenPlanBilling
                            | crate::app::view::Message::OpenPlanBillingFrom(_)
                    ))
                ) && self.connect_overlay_eligible(pane_id) =>
            {
                let focus = match inner.as_ref() {
                    tab::Message::Run(AppMessage::View(
                        crate::app::view::Message::OpenPlanBillingFrom(id),
                    )) => Some(iced::advanced::widget::Id::new(id)),
                    _ => None,
                };
                self.open_connect_sheet_from(
                    pane_id,
                    tab_id,
                    ConnectSheetSection::PlanBilling,
                    focus,
                )
            }
            Message::Pane(pane_id, pane::Message::Tab(tab_id, inner))
                if matches!(
                    inner.as_ref(),
                    tab::Message::Launch(home::Message::View(home::ViewMessage::GoToSection(
                        home::HomeSection::Connect(
                            crate::app::menu::ConnectSubMenu::Overview
                                | crate::app::menu::ConnectSubMenu::PlanBilling
                                | crate::app::menu::ConnectSubMenu::Security
                                | crate::app::menu::ConnectSubMenu::Duress
                                | crate::app::menu::ConnectSubMenu::Contacts
                        )
                    )))
                ) =>
            {
                let tab::Message::Launch(home::Message::View(home::ViewMessage::GoToSection(
                    home::HomeSection::Connect(sub),
                ))) = *inner
                else {
                    unreachable!();
                };
                let Some(origin_focus) = self
                    .panes
                    .get(pane_id)
                    .and_then(|pane| pane.tabs.iter().find(|tab| tab.id == tab_id))
                    .and_then(|tab| match &tab.state {
                        tab::State::Home(home) => Some(home.connect_sheet_origin_id()),
                        _ => None,
                    })
                else {
                    return Task::none();
                };
                let section = match sub {
                    crate::app::menu::ConnectSubMenu::Overview => ConnectSheetSection::Account,
                    crate::app::menu::ConnectSubMenu::PlanBilling => {
                        ConnectSheetSection::PlanBilling
                    }
                    crate::app::menu::ConnectSubMenu::Security => {
                        ConnectSheetSection::DevicesSecurity
                    }
                    crate::app::menu::ConnectSubMenu::Contacts => ConnectSheetSection::Contacts,
                    crate::app::menu::ConnectSubMenu::Duress => ConnectSheetSection::Duress,
                };
                let signed_out_launcher = section == ConnectSheetSection::Account
                    && self.overlay_origin(None).is_some_and(|origin| {
                        origin.pane == pane_id
                            && origin.tab_id == tab_id
                            && matches!(origin.source, tab::PreferencesSource::Home)
                            && self
                                .connect_panel(&origin)
                                .is_some_and(|account| !account.is_authenticated())
                    });
                let open =
                    self.open_connect_sheet_from(pane_id, tab_id, section, Some(origin_focus));
                if signed_out_launcher {
                    if let Some(sheet) = &mut self.connect_sheet {
                        sheet.page = ConnectSheetPage::Authentication;
                    }
                    Task::batch([
                        open,
                        iced_runtime::task::widget(
                            iced::advanced::widget::operation::focusable::focus(
                                iced::advanced::widget::Id::new(
                                    crate::app::view::connect::EMAIL_INPUT_ID,
                                ),
                            ),
                        ),
                    ])
                } else {
                    open
                }
            }
            Message::Pane(pane_id, pane::Message::View(pane::ViewMessage::OpenConnectSignIn))
                if self.connect_overlay_eligible(pane_id) =>
            {
                self.open_connect_sheet(
                    ConnectSheetSection::Account,
                    Some(iced::advanced::widget::Id::new("tenshu.connect.sign-in")),
                )
            }
            Message::Pane(pane_id, pane::Message::View(pane::ViewMessage::OpenPlanBilling))
                if self.connect_overlay_eligible(pane_id) =>
            {
                self.open_connect_sheet(ConnectSheetSection::PlanBilling, None)
            }
            Message::Pane(pane_id, pane::Message::Tab(tab_id, inner))
                if matches!(inner.as_ref(), tab::Message::OpenAppPreferences(_)) =>
            {
                let tab::Message::OpenAppPreferences(source) = *inner else {
                    unreachable!();
                };
                let current_source = self.panes.get(pane_id).and_then(|pane| {
                    pane.tabs
                        .iter()
                        .find(|tab| tab.id == tab_id)
                        .and_then(tab::Tab::preferences_source)
                });
                if current_source != Some(source) {
                    return Task::none();
                }
                let focus = Some(iced::advanced::widget::Id::new(match source {
                    tab::PreferencesSource::Home => "tenshu.launcher.catalog:control:settings",
                    tab::PreferencesSource::App(_) => {
                        crate::app::view::nav::shell::APP_PREFERENCES_ID
                    }
                }));
                let Some(origin) = self.overlay_origin(focus) else {
                    return Task::none();
                };
                if origin.pane != pane_id || origin.tab_id != tab_id || origin.source != source {
                    return Task::none();
                }
                self.preferences_sheet = Some(PreferencesSheet {
                    origin,
                    page: PreferencesPageState::default(),
                    worldview_changed: None,
                });
                self.connect_sheet = None;
                self.publish_preferences_sheet();
                Task::batch([
                    iced_runtime::task::widget(
                        iced::advanced::widget::operation::focusable::focus(
                            iced::advanced::widget::Id::new("tenshu.preferences.close"),
                        ),
                    ),
                    self.worldview_action(worldview::Action::Refresh),
                ])
            }
            Message::Pane(pane_id, pane::Message::Tab(tab_id, inner))
                if matches!(
                    inner.as_ref(),
                    tab::Message::LockCubeToLauncher
                        | tab::Message::LockCube
                        | tab::Message::LockCubeToDelete
                ) =>
            {
                let delete = matches!(inner.as_ref(), tab::Message::LockCubeToDelete);
                let to_launcher = delete
                    || matches!(inner.as_ref(), tab::Message::LockCubeToLauncher)
                    || self.panes.get(pane_id).is_some_and(|pane| {
                        let owner =
                            pane.tabs.iter().find(|tab| tab.id == tab_id).and_then(
                                |tab| match &tab.state {
                                    tab::State::App(app) => Some(app.cache().app_generation),
                                    _ => None,
                                },
                            );
                        matches!(
                            pane.tabs.get(pane.focused_tab).map(|tab| &tab.state),
                            Some(tab::State::Home(_))
                        ) || owner.is_some_and(|owner| {
                            pane.tabs.iter().any(|tab| tab.copy_owner() == Some(owner))
                        })
                    });
                let identity = self
                    .panes
                    .get(pane_id)
                    .and_then(|pane| pane.tabs.iter().find(|tab| tab.id == tab_id))
                    .and_then(|tab| match &tab.state {
                        tab::State::App(app) => Some((
                            app.cache().app_generation,
                            app.datadir().clone(),
                            app.cube_settings().network,
                            app.cube_settings().id.clone(),
                        )),
                        _ => None,
                    });
                let Some((owner, root, chain, cube_id)) = identity else {
                    return Task::none();
                };
                // Locking must not wait on daemon shutdowns: each poller first
                // finishes its in-flight network request. Those daemons stop on
                // their own threads; only the watch-only daemon outlives this
                // update, while the signer and wallet clients go with the tabs.
                let mut empty_panes = Vec::new();
                for (&other_id, pane) in self.panes.iter_mut() {
                    pane.close_tabs_matching_in_background(|sibling| {
                        !(other_id == pane_id && sibling.id == tab_id)
                            && (sibling.copy_owner() == Some(owner)
                                || sibling.matches_cube_identity(&root, chain, &cube_id)
                                || matches!(
                                    sibling.state,
                                    tab::State::App(_)
                                        | tab::State::Loader(_)
                                        | tab::State::Installer(_)
                                        | tab::State::PinEntry(_)
                                        | tab::State::PasskeyUnlock(_)
                                ))
                    });
                    if pane.tabs.is_empty() {
                        empty_panes.push(other_id);
                    }
                }
                for empty in empty_panes {
                    self.panes.close(empty);
                    if self.focus == Some(empty) {
                        self.focus = Some(pane_id);
                    }
                }
                if let Some(pane) = self.panes.get_mut(pane_id) {
                    if let Some(index) = pane.tabs.iter().position(|tab| tab.id == tab_id) {
                        if matches!(pane.tabs[index].state, tab::State::App(_)) {
                            if !to_launcher {
                                crate::app::session::close();
                                self.focus = Some(pane_id);
                                return pane
                                    .update(pane::Message::tab(tab_id, *inner), &self.config)
                                    .map(move |message| Message::Pane(pane_id, message));
                            }
                            pane.close_tab_in_background(index);
                            crate::app::session::close(); // zeroizes signer/PIN
                            self.focus = Some(pane_id);
                            let task = if delete {
                                pane.focus_launcher_to_delete(cube_id, chain, &self.config)
                            } else {
                                pane.focus_launcher(&self.config)
                            };
                            // Lock makes the Launcher the primary tab even if
                            // a legacy layout retained an unrelated Cube.
                            if pane.focused_tab > 0 {
                                let launcher = pane.tabs.remove(pane.focused_tab);
                                pane.tabs.insert(0, launcher);
                                pane.focused_tab = 0;
                            }
                            return task.map(move |message| Message::Pane(pane_id, message));
                        }
                    }
                }
                Task::none()
            }
            Message::Pane(pane_id, pane::Message::Tab(tab_id, inner))
                if matches!(inner.as_ref(), tab::Message::DuressActivated { .. })
                    || matches!(
                        inner.as_ref(),
                        tab::Message::Run(AppMessage::View(
                            crate::app::view::Message::DuressLockRemote
                        ))
                    ) =>
            {
                let remote = matches!(
                    inner.as_ref(),
                    tab::Message::Run(AppMessage::View(
                        crate::app::view::Message::DuressLockRemote
                    ))
                );
                let result = {
                    let Some(pane) = self.panes.get_mut(pane_id) else {
                        return Task::none();
                    };
                    let Some(index) = pane.tabs.iter().position(|tab| tab.id == tab_id) else {
                        return Task::none();
                    };
                    if remote && !matches!(pane.tabs[index].state, tab::State::App(_)) {
                        return Task::none();
                    }
                    pane.tabs[index].stop();
                    pane.update(pane::Message::Tab(tab_id, inner), &self.config)
                };
                // The wipe is complete: do not leave a Launcher or another
                // Cube surface visible above the cryptic duress state.
                for (&other_id, other_pane) in self.panes.iter_mut() {
                    other_pane.close_tabs_matching(|tab| other_id != pane_id || tab.id != tab_id);
                }
                crate::app::session::close();
                if let Some(pane) = self.panes.get_mut(pane_id) {
                    pane.focused_tab = pane
                        .tabs
                        .iter()
                        .position(|tab| tab.id == tab_id)
                        .unwrap_or(0);
                }
                self.focus = Some(pane_id);
                result.map(move |message| Message::Pane(pane_id, message))
            }
            Message::Pane(_, pane::Message::View(pane::ViewMessage::SplitTab(_))) => Task::none(),
            Message::Pane(pane_id, pane::Message::View(pane::ViewMessage::CloseTab(i))) => {
                if let Some(pane) = self.panes.get_mut(pane_id) {
                    if pane.tabs.get(i).is_some_and(|tab| tab.id == usize::MAX) {
                        return Task::none();
                    }
                    let pending_seed_trials = pane
                        .tabs
                        .get(i)
                        .map_or(0, tab::Tab::pending_seed_export_trials);
                    let closed_tab_id = pane.tabs.get(i).map(|tab| tab.id);
                    let _ = pane
                        .update(
                            pane::Message::View(pane::ViewMessage::CloseTab(i)),
                            &self.config,
                        )
                        .map(move |msg| Message::Pane(pane_id, msg));
                    if pane.tabs.is_empty() && pending_seed_trials > 0 {
                        // Closing the final tab must not close the Iced runtime
                        // before a submitted PIN can classify as Duress. Keep
                        // a neutral tab alive; a non-duress result closes it.
                        let (home, startup) = home::Home::new(
                            self.config.coincube_directory.clone(),
                            self.config.network,
                        );
                        drop(startup);
                        let mut placeholder = tab::Tab::new(usize::MAX, tab::State::Home(home));
                        placeholder.hold_closed_seed_export_trials(
                            closed_tab_id.expect("closed tab existed"),
                            pending_seed_trials,
                        );
                        pane.tabs.push(placeholder);
                        pane.focused_tab = 0;
                        self.focus = Some(pane_id);
                    }
                    if pane.tabs.is_empty() {
                        self.panes.close(pane_id);
                        if self.focus == Some(pane_id) {
                            self.focus = None;
                        }
                    }
                }
                if !self.panes.iter().any(|(_, p)| !p.tabs.is_empty()) {
                    return iced::window::latest().and_then(iced::window::close);
                }
                Task::none()
            }
            // In case of cube deletion, remove any tab where the cube/wallet is currently running.
            Message::Pane(p, pane::Message::Tab(t, inner))
                if matches!(inner.as_ref(), tab::Message::Launch(_)) =>
            {
                let tab::Message::Launch(msg) = *inner else {
                    unreachable!();
                };
                let mut tasks = Vec::new();
                let public_exit = matches!(
                    msg,
                    home::Message::Run(..)
                        | home::Message::RunFromFloor(..)
                        | home::Message::RunToRecovery(..)
                        | home::Message::Install(..)
                );
                if public_exit {
                    let admission = self
                        .panes
                        .get(p)
                        .and_then(|pane| pane.tabs.iter().find(|tab| tab.id == t))
                        .map_or_else(
                            || Err("The Launcher destination closed.".to_string()),
                            |tab| tab.validate_home_exit(&msg),
                        );
                    if let Err(error) = admission {
                        if let Some(tab::State::Home(home)) = self
                            .panes
                            .get_mut(p)
                            .and_then(|pane| pane.tabs.iter_mut().find(|tab| tab.id == t))
                            .map(|tab| &mut tab.state)
                        {
                            home.set_error(error);
                        }
                        return Task::none();
                    }
                }
                if let Some((root, chain, cube)) = match &msg {
                    home::Message::Run(root, _, chain, cube)
                    | home::Message::RunFromFloor(root, _, chain, cube, _)
                    | home::Message::RunToRecovery(root, _, chain, cube) => {
                        Some((root, chain, cube))
                    }
                    _ => None,
                } {
                    // Opening the same Cube from the Launcher resumes its live
                    // App. In particular this must not ask for its PIN again.
                    let secret_owners = self
                        .panes
                        .iter()
                        .flat_map(|(_, pane)| pane.tabs.iter())
                        .filter(|tab| {
                            matches!(
                                tab.state,
                                tab::State::App(_)
                                    | tab::State::Loader(_)
                                    | tab::State::Installer(_)
                                    | tab::State::PinEntry(_)
                                    | tab::State::PasskeyUnlock(_)
                            )
                        })
                        .count();
                    // A retained App can resume only while it is the sole
                    // public owner of the exact live process session. Legacy
                    // duplicate tabs, or an App outliving its session, go
                    // through the normal close-then-unlock admission instead.
                    let existing = (secret_owners == 1
                        && crate::app::session::current_cube_id().as_deref()
                            == Some(cube.id.as_str()))
                    .then(|| {
                        self.panes.iter().find_map(|(&pane_id, pane)| {
                            pane.tabs
                                .iter()
                                .enumerate()
                                .find_map(|(index, tab)| match &tab.state {
                                    tab::State::App(app)
                                        if app.datadir() == root
                                            && app.cube_settings().network == *chain
                                            && app.cube_settings().id == cube.id =>
                                    {
                                        Some((pane_id, index))
                                    }
                                    _ => None,
                                })
                        })
                    })
                    .flatten();
                    if let Some((pane_id, index)) = existing {
                        self.focus = Some(pane_id);
                        if let Some(pane) = self.panes.get_mut(pane_id) {
                            if matches!(&msg, home::Message::RunToRecovery(..)) {
                                let tab_id = pane.tabs[index].id;
                                let focus = pane
                                    .update(
                                        pane::Message::View(pane::ViewMessage::FocusTab(index)),
                                        &self.config,
                                    )
                                    .map(move |message| Message::Pane(pane_id, message));
                                let recovery = pane.tabs[index]
                                    .emit_app_task(Task::done(AppMessage::View(
                                        crate::app::view::Message::Menu(
                                            crate::app::menu::Menu::Cube(
                                                crate::app::menu::CubeSubMenu::Settings(
                                                    crate::app::menu::CubeSettingsOption::Recovery,
                                                ),
                                            ),
                                        ),
                                    )))
                                    .map(move |message| {
                                        Message::Pane(pane_id, pane::Message::tab(tab_id, message))
                                    });
                                return Task::batch([focus, recovery]);
                            }
                            return pane
                                .update(
                                    pane::Message::View(pane::ViewMessage::FocusTab(index)),
                                    &self.config,
                                )
                                .map(move |message| Message::Pane(pane_id, message));
                        }
                    }
                }
                if public_exit {
                    self.retire_public_owners();
                }
                if let home::Message::View(home::ViewMessage::DeleteCube(
                    home::DeleteCubeMessage::Confirm(..),
                )) = &msg
                {
                    self.retire_public_owners();
                    // When a cube is deleted, close all App and Loader tabs since they won't be valid anymore
                    let mut panes_to_close = Vec::<pane_grid::Pane>::new();
                    for (id, pane) in self.panes.iter_mut() {
                        pane.close_tabs_matching(|tab| {
                            matches!(tab.state, tab::State::App(_) | tab::State::Loader(_))
                        });
                        if pane.tabs.is_empty() {
                            panes_to_close.push(*id);
                        }
                    }
                    for id in panes_to_close {
                        self.panes.close(id);
                    }
                    for (&id, pane) in self.panes.iter() {
                        for tab in &pane.tabs {
                            if let tab::State::Home(l) = &tab.state {
                                let tab_id = tab.id;
                                tasks.push(l.reload().map(move |msg| {
                                    Message::Pane(
                                        id,
                                        pane::Message::tab(tab_id, tab::Message::Launch(msg)),
                                    )
                                }));
                            }
                        }
                    }
                }

                if let Some(pane) = self.panes.get_mut(p) {
                    tasks.push(
                        pane.update(
                            pane::Message::tab(t, tab::Message::Launch(msg)),
                            &self.config,
                        )
                        .map(move |msg| Message::Pane(p, msg)),
                    );
                }

                Task::batch(tasks)
            }
            Message::Pane(i, msg) => {
                match msg {
                    // Handle ListCurrencies requests separately.
                    pane::Message::Tab(tab_id, inner)
                        if matches!(
                            inner.as_ref(),
                            tab::Message::Run(AppMessage::Fiat(AppFiatMessage::ListCurrencies(_)))
                        ) =>
                    {
                        let tab::Message::Run(AppMessage::Fiat(AppFiatMessage::ListCurrencies(
                            source,
                        ))) = *inner
                        else {
                            tracing::error!("Unexpected message type after unboxing");
                            return Task::none();
                        };
                        // BTCB2 uses only Connect pricing. Currency preferences do not
                        // authorize a Bitcoin/aggregator currency-list request either.
                        let btcb2 = self
                            .panes
                            .get(i)
                            .and_then(|pane| pane.tabs.iter().find(|tab| tab.id == tab_id))
                            .and_then(|tab| tab.cube_settings())
                            .is_some_and(|cube| cube.network.is_blake2b());
                        if btcb2 {
                            if let Some(pane) = self.panes.get_mut(i) {
                                return pane
                                    .update_tab_with_app_msg(
                                        tab_id,
                                        AppFiatMessage::ListCurrenciesResult(
                                            source,
                                            Ok(ListCurrenciesResult {
                                                currencies: Currency::ALL.to_vec(),
                                            }),
                                        ),
                                        &self.config,
                                    )
                                    .map(move |msg| Message::Pane(i, msg));
                            }
                            return Task::none();
                        }
                        // If we already have a fresh list of currencies for this source, return it directly to the tab.
                        if let Some(fresh_list) = self.global_cache.fresh_currencies(source) {
                            tracing::debug!("Using cached currencies list for {}", source,);
                            if let Some(pane) = self.panes.get_mut(i) {
                                return pane
                                    .update_tab_with_app_msg(
                                        tab_id,
                                        AppFiatMessage::ListCurrenciesResult(
                                            source,
                                            Ok(ListCurrenciesResult {
                                                currencies: fresh_list.clone(),
                                            }),
                                        ),
                                        &self.config,
                                    )
                                    .map(move |msg| Message::Pane(i, msg));
                            }
                        } else {
                            tracing::debug!("Requesting list of currencies from {}", source);
                            return Task::perform(
                                async move {
                                    let client = PriceClient::default_from_source(source);
                                    (
                                        tab_id,
                                        source,
                                        Instant::now(),
                                        client.list_currencies().await,
                                    )
                                },
                                move |(tab_id, source, now, res)| {
                                    FiatMessage::ListCurrenciesResult(i, tab_id, source, now, res)
                                        .into()
                                },
                            );
                        }
                    }
                    _ => {
                        if let Some(pane) = self.panes.get_mut(i) {
                            return pane
                                .update(msg, &self.config)
                                .map(move |msg| Message::Pane(i, msg));
                        }
                    }
                }
                Task::none()
            }
            Message::Clicked(pane) => {
                self.focus = Some(pane);
                Task::none()
            }
            Message::Resized(_) | Message::Dragged(_) => Task::none(),
            Message::Preferences(request, result) => {
                let before = self
                    .preferences
                    .accepted()
                    .map(|accepted| accepted.preferences);
                let outcome = self.preferences.complete(&request, result);
                let applied = matches!(outcome, SaveOutcome::Applied(_));
                let failed = matches!(outcome, SaveOutcome::Failed(_));
                self.publish_balance_privacy();
                let mut tasks = Vec::new();
                if applied {
                    let after = self
                        .preferences
                        .accepted()
                        .map(|accepted| accepted.preferences);
                    if before.map(|prefs| prefs.theme) != after.map(|prefs| prefs.theme) {
                        self.publish_appearance();
                    }
                    if let (
                        Operation::Save(
                            crate::app::settings::preferences::PreferenceChange::Worldview(mode),
                        ),
                        Some(sheet),
                    ) = (request.operation, &mut self.preferences_sheet)
                    {
                        if before.is_some_and(|prefs| prefs.worldview != mode)
                            && after.is_some_and(|prefs| prefs.worldview == mode)
                        {
                            sheet.worldview_changed = Some(mode);
                        }
                    }
                    if launcher_presentation_changed(before, after) {
                        self.refresh_all_launcher_modes();
                    }
                    if needs_global_settings_reload(before, after) {
                        tasks.push(self.reload_all_global_settings());
                    }
                }
                self.worldview_saved(&request, applied, before);
                if let Some(next) = self.preferences.begin_next_queued_save() {
                    self.worldview_start_queued_save(&next);
                    self.publish_balance_privacy();
                    tasks.push(self.run_preferences(next));
                }
                // A zoom that failed to save goes back to the saved one, like
                // every other choice ("Your previous settings remain active").
                // A newer step pressed during the save is kept and saved next.
                if let Operation::Save(crate::app::settings::preferences::PreferenceChange::Zoom(
                    zoom,
                )) = request.operation
                {
                    if failed && self.zoom.pending == Some(zoom) {
                        self.zoom.pending = None;
                    }
                }
                tasks.push(self.save_pending_zoom());
                tasks.push(self.worldview_automatic());
                Task::batch(tasks)
            }
            Message::ToggleTheme => {
                use crate::app::settings::preferences::{PreferenceChange, ThemePreference};
                use coincube_ui::theme::palette::ThemeMode;
                let next = match self.theme_mode {
                    ThemeMode::Dark => ThemePreference::Light,
                    ThemeMode::Light => ThemePreference::Dark,
                };
                self.change_preference(PreferenceChange::Theme(next))
            }
            Message::SystemTheme(mode) => {
                self.system_theme = match mode {
                    iced::theme::Mode::Light => Some(coincube_ui::theme::palette::ThemeMode::Light),
                    iced::theme::Mode::Dark => Some(coincube_ui::theme::palette::ThemeMode::Dark),
                    iced::theme::Mode::None => None,
                };
                self.publish_appearance();
                Task::none()
            }
            Message::Worldview(origin, context, request, result) => {
                self.worldview_complete(origin, context, request, result)
            }
            Message::PreferencesSheet(message) => self.preferences_sheet_message(message),
            Message::ConnectSheet(message) => self.connect_sheet_message(message),
            Message::Tick => {
                if self
                    .zoom
                    .notice_until
                    .is_some_and(|until| Instant::now() >= until)
                {
                    self.zoom.notice_until = None;
                }
                let system_reduces_motion =
                    crate::app::settings::system_accessibility::reduces_motion();
                self.system_reduces_motion = system_reduces_motion;
                for (_, pane) in self.panes.iter_mut() {
                    for tab in &mut pane.tabs {
                        tab.set_system_reduces_motion(system_reduces_motion);
                    }
                }
                let privacy_refresh = self
                    .preferences
                    .begin_refresh_if_due()
                    .map(|request| self.run_preferences(request));
                if privacy_refresh.is_some() {
                    self.publish_balance_privacy();
                }
                let mut tasks = vec![];
                if let Some(refresh) = privacy_refresh {
                    tasks.push(refresh);
                }

                // These are the required (source, currency) pairs for which the global price is stale.
                let mut stale_pairs = HashSet::<(PriceSource, Currency)>::new();
                // These are the tabs that need the cached global price.
                let mut need_cached = HashMap::<(pane_grid::Pane, usize), FiatPrice>::new();
                // Tabs that need a fresh USD price for USDt→sats conversion.
                let mut need_usd_cached: Vec<(pane_grid::Pane, usize, FiatPrice)> = Vec::new();
                for (&pane_id, pane) in self.panes.iter() {
                    for tab in pane.tabs.iter() {
                        // BTCB2 has an authenticated, generation-bound poll in App.
                        // Never insert its requests into the shared Bitcoin cache.
                        if tab
                            .cube_settings()
                            .is_some_and(|cube| cube.network.is_blake2b())
                        {
                            continue;
                        }
                        let fiat_sett = tab.cube_settings().and_then(|cs| cs.fiat_price.as_ref());

                        // When fiat display is enabled, fetch the user's selected currency.
                        if let Some(sett) = fiat_sett.filter(|s| s.is_enabled) {
                            if let Some(fresh_price) = self
                                .global_cache
                                .fresh_fiat_price(sett.source, sett.currency)
                            {
                                if !tab.cache().and_then(|c| c.fiat_price.as_ref()).is_some_and(
                                    |tab_price| tab_price.request == fresh_price.request,
                                ) {
                                    need_cached.insert((pane_id, tab.id), fresh_price.clone());
                                }
                            } else if self
                                .global_cache
                                .pending_fiat_price_request(sett.source, sett.currency)
                                .is_none()
                            {
                                stale_pairs.insert((sett.source, sett.currency));
                            }
                        }

                        // Always fetch BTC/USD for USDt→sats conversion, even when
                        // fiat display is disabled. Use the configured source if
                        // available, otherwise fall back to the default source.
                        let usd_source = fiat_sett
                            .map(|s| s.source)
                            .unwrap_or(PriceSource::default());
                        if let Some(fresh_usd) = self
                            .global_cache
                            .fresh_fiat_price(usd_source, Currency::USD)
                        {
                            if tab.cache().and_then(|c| c.btc_usd_price).is_none() {
                                need_usd_cached.push((pane_id, tab.id, fresh_usd.clone()));
                            }
                        } else if self
                            .global_cache
                            .pending_fiat_price_request(usd_source, Currency::USD)
                            .is_none()
                        {
                            stale_pairs.insert((usd_source, Currency::USD));
                        }
                    }
                }
                for (source, currency) in stale_pairs {
                    let request = FiatPriceRequest::new(source, currency);
                    // Store request immediately to avoid multiple requests for the same pair.
                    self.global_cache.insert_fiat_price_request(request);
                    tasks.push(Task::perform(request.send_default(), |res| {
                        FiatMessage::GetPriceResult(res).into()
                    }));
                }
                for ((pane_id, tab_id), global_price) in need_cached {
                    if let Some(pane) = self.panes.get_mut(pane_id) {
                        // Return the cached global price to the tab.
                        tracing::debug!(
                            "Updating tab {} in pane {:?} with cached fiat price {:?}",
                            tab_id,
                            pane_id,
                            global_price
                        );
                        tasks.push(
                            pane.update_tab_with_app_msg(
                                tab_id,
                                AppFiatMessage::GetPriceResult(global_price),
                                &self.config,
                            )
                            .map(move |msg| Message::Pane(pane_id, msg)),
                        );
                    }
                }
                // Distribute fresh USD prices for USDt→sats conversion.
                for (pane_id, tab_id, usd_price) in need_usd_cached {
                    if let Some(pane) = self.panes.get_mut(pane_id) {
                        tasks.push(
                            pane.update_tab_with_app_msg(
                                tab_id,
                                AppFiatMessage::GetPriceResult(usd_price),
                                &self.config,
                            )
                            .map(move |msg| Message::Pane(pane_id, msg)),
                        );
                    }
                }
                tasks.extend(
                    self.panes
                        .iter_mut()
                        .map(|(&id, p)| p.on_tick().map(move |msg| Message::Pane(id, msg))),
                );

                Task::batch(tasks)
            }
            _ => Task::none(),
        }
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let mut vec = vec![
            iced::time::every(Duration::from_secs(1)).map(|_| Message::Tick),
            iced::system::theme_changes().map(Message::SystemTheme),
            iced::event::listen_with(|event, status, _| {
                // The idle auto-lock's only source of truth for "someone is at
                // the machine". It must come from real input: message traffic
                // does not work, because the 1 Hz tick spawns follow-up
                // messages continuously and would defer the lock forever.
                //
                // Done as a side effect rather than by emitting a message —
                // cursor movement alone would flood the queue at hundreds of
                // messages a second for a value nothing else reads.
                if is_user_input(&event) {
                    crate::app::session::touch();
                }
                if let Some(key) = ignored_shell_key(&event, status) {
                    return Some(Message::KeyPressed(key));
                }
                match (&event, status) {
                    (
                        iced::Event::Window(iced::window::Event::CloseRequested),
                        event::Status::Ignored,
                    ) => Some(Message::Event(event)),
                    (iced::Event::Window(iced::window::Event::Resized(size)), _) => {
                        Some(Message::WindowSize(*size))
                    }
                    (iced::Event::Window(iced::window::Event::Moved(_)), _) => {
                        Some(Message::WindowMoved)
                    }
                    _ => None,
                }
            }),
        ];
        let visible_pane = self
            .panes
            .iter()
            .find(|(id, _)| Some(**id) == self.focus)
            .or_else(|| self.panes.iter().next())
            .map(|(id, _)| *id);
        for (id, pane) in self.panes.iter() {
            vec.push(
                pane.subscription(Some(*id) == visible_pane)
                    .with(*id)
                    .map(|(id, msg)| Message::Pane(id, msg)),
            );
        }
        Subscription::batch(vec)
    }

    /// The screen with the zoom pill layered on top. The stack is always
    /// there, pill or not, so showing the pill keeps every widget's state.
    pub fn view(&self) -> Element<Message> {
        let pill: Element<Message> = match self.zoom.notice_until {
            Some(until) if Instant::now() < until => zoom_pill(self.current_zoom()),
            _ => iced::widget::Space::new().into(),
        };
        iced::widget::stack![self.screen(), pill].into()
    }

    fn screen(&self) -> Element<Message> {
        coincube_ui::locale::render_in(self.language(), || {
            coincube_ui::worldview_copy::render_in(self.worldview(), || self.localized_screen())
        })
    }

    fn localized_screen(&self) -> Element<Message> {
        if privacy_render_needs_cover(
            &self.preferences,
            &GlobalSettings::path(&self.config.coincube_directory),
        ) {
            return iced::widget::container(iced::widget::text(localized_copy(
                "Updating balance privacy…",
            )))
            .width(iced::Length::Fill)
            .height(iced::Length::Fill)
            .into();
        }
        let (&id, pane) = self
            .panes
            .iter()
            .find(|(id, _)| Some(**id) == self.focus)
            .or_else(|| self.panes.iter().next())
            .expect("GUI always owns its primary pane");
        let background = pane
            .view(self.worldview())
            .map(move |message| Message::Pane(id, message));
        if let Some(sheet) = &self.preferences_sheet {
            if self.overlay_origin_is_current(&sheet.origin) {
                let foreground = preferences_view::view(&sheet.page, self.preferences_view_model())
                    .map(Message::PreferencesSheet);
                // P-AppPrefs keeps the Launcher's top bar above the sheet;
                // over an open Cube (P-AppPrefs-InCube) the sheet covers it.
                return compose_overlay(
                    background,
                    foreground,
                    matches!(sheet.origin.source, tab::PreferencesSource::Home),
                );
            }
        }
        if let Some(sheet) = &self.connect_sheet {
            if self.overlay_origin_is_current(&sheet.origin) {
                if let Some(account) = self.connect_panel(&sheet.origin) {
                    let foreground = crate::app::view::connect::connect_sheet(account, sheet.page)
                        .map(Message::ConnectSheet);
                    // P-Connect keeps the Launcher's top bar above the sheet;
                    // over an open Cube (P-Connect-InCube) the sheet covers it.
                    return compose_overlay(
                        background,
                        foreground,
                        matches!(sheet.origin.source, tab::PreferencesSource::Home),
                    );
                }
            }
        }
        background
    }

    pub fn scale_factor(&self) -> f32 {
        self.current_zoom().factor()
    }

    fn language(&self) -> coincube_ui::locale::Language {
        self.preferences
            .accepted()
            .map(|accepted| accepted.preferences.language)
            .unwrap_or_default()
    }

    fn worldview(&self) -> coincube_ui::theme::WorldviewMode {
        self.preferences
            .accepted()
            .map(|accepted| accepted.preferences.worldview)
            .unwrap_or_default()
    }

    pub fn theme(&self) -> coincube_ui::theme::Theme {
        let mode = self.worldview();
        let theme = coincube_ui::theme::Theme::from_mode(self.theme_mode).with_worldview(mode);
        if let Some(accepted) = self.preferences.accepted() {
            theme.with_artwork(accepted.preferences.artwork.get(mode))
        } else {
            theme
        }
    }
}

#[derive(Clone)]
pub struct Config {
    pub coincube_directory: CoincubeDirectory,
    network: Option<bitcoin::Network>,
}

impl Config {
    pub fn new(coincube_directory: CoincubeDirectory, network: Option<bitcoin::Network>) -> Self {
        Self {
            coincube_directory,
            network,
        }
    }
}

/// Where an overlay (App Preferences, Connect) was opened from: the exact
/// retained pane and tab, the App generation when that tab is a Cube, and the
/// control to refocus when the overlay closes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayOrigin {
    pub pane: pane_grid::Pane,
    pub tab_id: usize,
    pub source: tab::PreferencesSource,
    pub focus: Option<iced::advanced::widget::Id>,
}

struct PreferencesSheet {
    origin: OverlayOrigin,
    page: PreferencesPageState,
    worldview_changed: Option<coincube_ui::theme::WorldviewMode>,
}

struct ConnectSheet {
    origin: OverlayOrigin,
    page: ConnectSheetPage,
}

impl GUI {
    fn connect_overlay_eligible(&self, pane_id: pane_grid::Pane) -> bool {
        self.visible_pane().is_some_and(|(id, pane)| {
            id == pane_id
                && pane.tabs.get(pane.focused_tab).is_some_and(|tab| {
                    matches!(&tab.state, tab::State::Home(_) | tab::State::App(_))
                })
        })
    }

    fn connect_panel(
        &self,
        origin: &OverlayOrigin,
    ) -> Option<&crate::app::state::connect::ConnectAccountPanel> {
        let tab = self
            .panes
            .get(origin.pane)?
            .tabs
            .iter()
            .find(|tab| tab.id == origin.tab_id)?;
        match &tab.state {
            tab::State::Home(home) => Some(&home.connect_account),
            tab::State::App(app) => Some(app.connect_sheet_account()),
            _ => None,
        }
    }
    /// The pane `view` renders: the focused one, else the first.
    fn visible_pane(&self) -> Option<(pane_grid::Pane, &pane::Pane)> {
        self.panes
            .iter()
            .find(|(id, _)| Some(**id) == self.focus)
            .or_else(|| self.panes.iter().next())
            .map(|(id, pane)| (*id, pane))
    }

    /// Records what is visible now as an overlay origin.
    pub fn overlay_origin(
        &self,
        focus: Option<iced::advanced::widget::Id>,
    ) -> Option<OverlayOrigin> {
        let (pane, state) = self.visible_pane()?;
        let tab = state.tabs.get(state.focused_tab)?;
        Some(OverlayOrigin {
            pane,
            tab_id: tab.id,
            source: tab.preferences_source()?,
            focus,
        })
    }

    /// Whether `origin` still names the same visible state. Lock, sign-out,
    /// deletion, duress or a replaced Cube change the visible tab or its App
    /// generation, so closing the overlay must not restore into it.
    pub fn overlay_origin_is_current(&self, origin: &OverlayOrigin) -> bool {
        self.visible_pane()
            .filter(|(pane, _)| *pane == origin.pane)
            .and_then(|(_, state)| state.tabs.get(state.focused_tab))
            .is_some_and(|tab| {
                tab.id == origin.tab_id && tab.preferences_source() == Some(origin.source)
            })
    }
}

/// The real retained background, drawn but unreachable while an overlay owns
/// input: it receives no pointer or keyboard event and no focus or other
/// operation, so Tab cannot leave the sheet and background controls cannot
/// activate. Its own overlays (tooltips, menus) are not shown and its
/// animations pause. Nothing is cloned or screenshotted.
struct InertBackground<'a, M> {
    content: Element<'a, M>,
}

impl<'a, M> iced::advanced::Widget<M, coincube_ui::theme::Theme, iced::Renderer>
    for InertBackground<'a, M>
{
    fn size(&self) -> Size<iced::Length> {
        self.content.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        renderer: &iced::Renderer,
        limits: &iced::advanced::layout::Limits,
    ) -> iced::advanced::layout::Node {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }

    fn draw(
        &self,
        tree: &iced::advanced::widget::Tree,
        renderer: &mut iced::Renderer,
        theme: &coincube_ui::theme::Theme,
        style: &iced::advanced::renderer::Style,
        layout: iced::advanced::Layout<'_>,
        _cursor: iced::mouse::Cursor,
        viewport: &iced::Rectangle,
    ) {
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            iced::mouse::Cursor::Unavailable,
            viewport,
        );
    }

    fn children(&self) -> Vec<iced::advanced::widget::Tree> {
        vec![iced::advanced::widget::Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut iced::advanced::widget::Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }
}

/// Pure overlay composition: `background` is the real view of the retained
/// origin, rendered inert under a scrim; `sheet` owns all input and focus.
pub fn compose_overlay<'a, M: 'a>(
    background: Element<'a, M>,
    sheet: Element<'a, M>,
    preserve_top_bar: bool,
) -> Element<'a, M> {
    let overlay = iced::widget::container(
        iced::widget::container(sheet)
            .max_width(1040)
            .width(iced::Length::Fill)
            .height(iced::Length::Fill)
            .center_x(iced::Length::Fill),
    )
    .width(iced::Length::Fill)
    .height(iced::Length::Fill)
    .center_x(iced::Length::Fill)
    // `.sheet`: 28px above and below, over the stage or the whole window.
    .padding([28, 24])
    .style(
        |theme: &coincube_ui::theme::Theme| iced::widget::container::Style {
            background: Some(iced::Background::Color(theme.colors.shell.shadow)),
            ..Default::default()
        },
    );
    if preserve_top_bar {
        return iced::widget::stack![
            Element::new(InertBackground {
                content: background
            }),
            iced::widget::Column::new()
                .push(iced::widget::Space::new().height(64))
                .push(overlay),
        ]
        .into();
    }
    iced::widget::stack![
        Element::new(InertBackground {
            content: background
        }),
        overlay,
    ]
    .into()
}

#[cfg(test)]
mod connect_sheet_followup_tests;

#[cfg(test)]
mod app_preferences_sheet_tests {
    use super::*;
    use crate::app::settings::preferences::{LauncherStyle, MotionPreference, PreferenceChange};

    #[test]
    fn settings_restore_intent_stays_in_cube_without_opening_account_sheet() {
        // GUI updates reach teardown paths that close the process-global session.
        let _guard = crate::app::session::test_guard();
        // The full GUI update path overflows libtest's 2 MiB thread on Windows
        // debug builds; give it the headroom the sibling GUI tests use.
        let result = std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let _enter = runtime.enter();
                let path = std::env::temp_dir()
                    .join(format!("settings-restore-route-{}", uuid::Uuid::new_v4()));
                std::fs::create_dir_all(&path).unwrap();
                let root = CoincubeDirectory::new(path.clone());
                let (app, _) = crate::app::shell_tests::bitcoin_app(&path);
                let pane = pane::Pane::new_with_tab(tab::State::App(app));
                let (panes, pane_id) = pane_grid::State::new(pane);
                let mut gui = GUI {
                    panes,
                    focus: Some(pane_id),
                    config: Config::new(root, None),
                    window_id: None,
                    window_init: None,
                    window_config: None,
                    global_cache: GlobalCache::default(),
                    theme_mode: Default::default(),
                    system_theme: None,
                    system_reduces_motion: None,
                    preferences_sheet: None,
                    connect_sheet: None,
                    preferences: unresolved_test_preferences(),
                    zoom: ZoomState::default(),
                    worldview_sharing: worldview::Sharing::default(),
                };
                let tab_id = gui.panes.get(pane_id).unwrap().tabs[0].id;
                drop(gui.update(Message::Pane(
                    pane_id,
                    pane::Message::tab(
                        tab_id,
                        tab::Message::Run(AppMessage::View(
                            crate::app::view::Message::ConnectAccount(
                                crate::app::view::ConnectAccountMessage::Init,
                            ),
                        )),
                    ),
                )));
                let pane = gui.panes.get(pane_id).unwrap();
                assert_eq!(pane.tabs.len(), 1);
                assert!(matches!(pane.tabs[0].state, tab::State::App(_)));
                assert_eq!(gui.focus, Some(pane_id));
                assert!(gui.connect_sheet.is_none());
                drop(gui);
                std::fs::remove_dir_all(path).ok();
            })
            .unwrap()
            .join();
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }

    pub(super) fn launcher_gui(root: &CoincubeDirectory) -> GUI {
        let (home, startup) = home::Home::new_for_chain(root.clone(), None);
        drop(startup);
        let pane = pane::Pane::new_with_tab(tab::State::Home(home));
        let (panes, focus) = pane_grid::State::new(pane);
        let receipt = GlobalSettings::load_or_initialize_preferences(root);
        GUI {
            panes,
            focus: Some(focus),
            config: Config::new(root.clone(), None),
            window_id: None,
            window_init: None,
            window_config: None,
            global_cache: GlobalCache::default(),
            theme_mode: Default::default(),
            system_theme: None,
            system_reduces_motion: None,
            preferences_sheet: None,
            connect_sheet: None,
            preferences: PreferencesOwner::new(GlobalSettings::path(root), &receipt),
            zoom: ZoomState::default(),
            worldview_sharing: worldview::Sharing::default(),
        }
    }

    /// P-AppPrefs draws the Launcher's App preferences entry as current
    /// (`.tbtn.on`) only while the sheet is open over that Launcher.
    #[test]
    fn launcher_entry_shows_open_preferences_sheet() {
        // GUI updates reach teardown paths that close the process-global session.
        let _guard = crate::app::session::test_guard();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _enter = runtime.enter();
        let path = std::env::temp_dir().join(format!(
            "tenshu-preferences-entry-state-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&path).unwrap();
        let root = CoincubeDirectory::new(path.clone());
        GlobalSettings::load_or_initialize_preferences(&root).unwrap();
        let mut gui = launcher_gui(&root);
        let pane_id = gui.focus.unwrap();
        let tab_id = gui.panes.get(pane_id).unwrap().tabs[0].id;
        let open = |gui: &GUI| match &gui.panes.get(pane_id).unwrap().tabs[0].state {
            tab::State::Home(home) => home.app_preferences_open,
            _ => panic!("launcher tab"),
        };
        assert!(!open(&gui));
        let _ = gui.update(Message::Pane(
            pane_id,
            pane::Message::Tab(
                tab_id,
                Box::new(tab::Message::OpenAppPreferences(
                    tab::PreferencesSource::Home,
                )),
            ),
        ));
        assert!(gui.preferences_sheet.is_some());
        assert!(open(&gui));
        let _ = gui.update(Message::PreferencesSheet(
            preferences_view::Message::Action(preferences_view::Action::Close),
        ));
        assert!(gui.preferences_sheet.is_none());
        assert!(!open(&gui));
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn background_refresh_does_not_move_preferences_or_disable_controls() {
        // GUI updates reach teardown paths that close the process-global session.
        let _guard = crate::app::session::test_guard();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _enter = runtime.enter();
        let path = std::env::temp_dir().join(format!(
            "tenshu-preferences-refresh-presentation-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&path).unwrap();
        let root = CoincubeDirectory::new(path.clone());
        GlobalSettings::load_or_initialize_preferences(&root).unwrap();
        GlobalSettings::save_preference(
            &GlobalSettings::path(&root),
            PreferenceChange::HideBalances(false),
        )
        .unwrap();
        let mut gui = launcher_gui(&root);
        fn inspect(
            scenario: &str,
            model: preferences_view::ViewModel<'_>,
        ) -> crate::app::view::shared::landing::capture::Inspection {
            let page = PreferencesPageState::default();
            crate::app::view::shared::landing::capture::inspect_full_window(
                scenario,
                preferences_view::view(&page, model),
                (1024, 768),
                "dark",
                coincube_ui::theme::Theme::dark(),
            )
        }

        let baseline_model = gui.preferences_view_model();
        assert!(!baseline_model.saving);
        assert!(baseline_model.balance_privacy_interactive);
        let baseline_chrome = GUI::privacy_chrome_for(&gui.preferences);
        assert_eq!(
            crate::app::view::nav::shell::balance_privacy_label(baseline_chrome),
            "Hide balances"
        );
        assert!(baseline_chrome.intent().is_some());
        let baseline = inspect("preferences-idle", baseline_model);

        std::thread::sleep(Duration::from_millis(1100));
        let refresh = gui.preferences.begin_refresh_if_due().unwrap();
        assert!(gui.preferences.is_busy());
        assert!(gui.preferences.is_background_refreshing());
        let pending_model = gui.preferences_view_model();
        assert!(!pending_model.saving);
        assert!(pending_model.balance_privacy_interactive);
        let reading_chrome = GUI::privacy_chrome_for(&gui.preferences);
        assert_eq!(reading_chrome, baseline_chrome);
        assert_eq!(
            crate::app::view::nav::shell::balance_privacy_label(reading_chrome),
            "Hide balances"
        );
        let pending = inspect("preferences-background-refresh", pending_model);
        assert_eq!(baseline.text, pending.text);
        assert!(!pending.contains("Saving device preferences"));

        // A click during the read is retained, then becomes the only visible
        // save once the read finishes on the same ordered lane.
        drop(gui.change_preference(PreferenceChange::HideBalances(true)));
        assert!(gui.preferences.is_background_refreshing());
        let queued_model = gui.preferences_view_model();
        assert!(queued_model.saving);
        assert!(!queued_model.balance_privacy_interactive);
        let queued_chrome = GUI::privacy_chrome_for(&gui.preferences);
        assert_eq!(
            crate::app::view::nav::shell::balance_privacy_label(queued_chrome),
            "Saving balance privacy…"
        );
        assert!(queued_chrome.intent().is_none());
        let queued = inspect("preferences-queued-save", queued_model);
        assert!(queued.contains("Saving device preferences"));
        let receipt = GlobalSettings::load_or_initialize_preferences(&root).unwrap();
        let next = gui.update(Message::Preferences(refresh, Ok(receipt)));
        assert!(next.units() > 0);
        assert!(!gui.preferences.is_background_refreshing());
        let saving_model = gui.preferences_view_model();
        assert!(saving_model.saving);
        assert!(!saving_model.balance_privacy_interactive);
        let saving = inspect("preferences-explicit-save", saving_model);
        assert!(saving.contains("Saving device preferences"));

        drop(next);
        gui.preferences = unresolved_test_preferences();
        let _hide = gui
            .preferences
            .begin_save(PreferenceChange::HideBalances(true))
            .unwrap();
        let unresolved_save = gui.preferences_view_model();
        assert!(unresolved_save.saving);
        assert!(!unresolved_save.loading);
        assert!(!unresolved_save.balance_privacy_interactive);
        assert_eq!(
            unresolved_save.balance_privacy,
            crate::app::state::preferences::BalancePrivacy::HiddenLocally
        );
        drop(gui);
        std::fs::remove_dir_all(path).ok();
    }

    fn pin_entry(root: &CoincubeDirectory) -> tab::State {
        let pin = crate::pin_entry::PinEntry::new(
            crate::app::settings::CubeSettings::new(
                "Locked Cube".into(),
                crate::chain::ChainId::Bitcoin,
            ),
            root.path().to_path_buf(),
            crate::pin_entry::PinEntrySuccess::LoadApp {
                datadir: root.clone(),
                config: crate::app::Config::new(false),
                network: bitcoin::Network::Bitcoin,
                internal_bitcoind: None,
                backup: None,
                wallet_settings: None,
                connect_client: None,
            },
            None,
        );
        tab::State::PinEntry(pin)
    }

    #[test]
    fn launcher_sheet_keeps_its_tab_and_routes_a_device_change() {
        // GUI updates reach teardown paths that close the process-global session.
        let _guard = crate::app::session::test_guard();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _enter = runtime.enter();
        let path = std::env::temp_dir().join(format!(
            "tenshu-app-preferences-sheet-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&path).unwrap();
        let root = CoincubeDirectory::new(path.clone());
        let mut gui = launcher_gui(&root);
        let pane_id = gui.focus.unwrap();
        let tab_id = gui.panes.get(pane_id).unwrap().tabs[0].id;

        drop(gui.update(Message::Pane(
            pane_id,
            pane::Message::tab(
                tab_id,
                tab::Message::OpenAppPreferences(tab::PreferencesSource::Home),
            ),
        )));
        assert!(gui.preferences_sheet.is_some());
        assert!(gui.overlay_origin_is_current(&gui.preferences_sheet.as_ref().unwrap().origin));
        assert_eq!(gui.panes.get(pane_id).unwrap().tabs[0].id, tab_id);

        drop(gui.update(Message::PreferencesSheet(
            preferences_view::Message::Change(PreferenceChange::LauncherStyle(LauncherStyle::List)),
        )));
        assert!(gui.preferences.is_busy());

        drop(gui.update(Message::KeyPressed(Key::Escape)));
        assert!(gui.preferences_sheet.is_none());
        assert_eq!(gui.panes.get(pane_id).unwrap().tabs[0].id, tab_id);
        drop(gui);
        std::fs::remove_dir_all(path).ok();
    }

    #[test]
    fn connect_sheet_keeps_launcher_origin_and_closes_on_escape() {
        // GUI updates reach teardown paths that close the process-global session.
        let _guard = crate::app::session::test_guard();
        // Measured at about 2 MiB of stack in debug builds; keep Windows headroom.
        let result = std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let _enter = runtime.enter();
                let path = std::env::temp_dir().join(format!(
                    "tenshu-connect-sheet-origin-{}",
                    uuid::Uuid::new_v4()
                ));
                std::fs::create_dir_all(&path).unwrap();
                let root = CoincubeDirectory::new(path.clone());
                let mut gui = launcher_gui(&root);
                let pane_id = gui.focus.unwrap();
                let tab_id = gui.panes.get(pane_id).unwrap().tabs[0].id;

                drop(gui.update(Message::Pane(
                    pane_id,
                    pane::Message::tab(
                        tab_id,
                        tab::Message::Launch(home::Message::View(home::ViewMessage::GoToSection(
                            home::HomeSection::Connect(
                                crate::app::menu::ConnectSubMenu::PlanBilling,
                            ),
                        ))),
                    ),
                )));
                let sheet = gui.connect_sheet.as_ref().expect("Connect sheet mounted");
                assert_eq!(
                    sheet.page,
                    ConnectSheetPage::Sections(ConnectSheetSection::PlanBilling)
                );
                assert!(gui.overlay_origin_is_current(&sheet.origin));
                assert_eq!(gui.panes.get(pane_id).unwrap().tabs[0].id, tab_id);
                assert!(matches!(&gui.panes.get(pane_id).unwrap().tabs[0].state,
                    tab::State::Home(home) if !matches!(home.active_section,
                        home::HomeSection::Connect(_))));

                drop(
                    gui.update(Message::ConnectSheet(ConnectSheetMessage::SelectSection(
                        ConnectSheetSection::Contacts,
                    ))),
                );
                assert_eq!(
                    gui.connect_sheet.as_ref().unwrap().page,
                    ConnectSheetPage::Sections(ConnectSheetSection::Contacts)
                );
                drop(gui.update(Message::ConnectSheet(
                    ConnectSheetMessage::OpenAuthentication {
                        create_account: true,
                    },
                )));
                assert_eq!(
                    gui.connect_sheet.as_ref().unwrap().page,
                    ConnectSheetPage::Authentication
                );
                assert!(matches!(
                    &gui.panes.get(pane_id).unwrap().tabs[0].state,
                    tab::State::Home(home) if matches!(home.connect_account.step,
                        crate::app::state::connect::ConnectFlowStep::Register { .. })
                ));
                drop(gui.update(Message::KeyPressed(Key::Escape)));
                assert!(gui.connect_sheet.is_none());
                assert_eq!(gui.panes.get(pane_id).unwrap().tabs[0].id, tab_id);
                drop(gui);
                std::fs::remove_dir_all(path).ok();
            })
            .unwrap()
            .join();
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }

    #[test]
    fn stale_connect_sheet_discards_queued_account_action() {
        // GUI updates reach teardown paths that close the process-global session.
        let _guard = crate::app::session::test_guard();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _enter = runtime.enter();
        let path = std::env::temp_dir().join(format!(
            "tenshu-connect-sheet-stale-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&path).unwrap();
        let root = CoincubeDirectory::new(path.clone());
        let mut gui = launcher_gui(&root);
        let pane_id = gui.focus.unwrap();
        let tab_id = gui.panes.get(pane_id).unwrap().tabs[0].id;
        drop(gui.update(Message::Pane(
            pane_id,
            pane::Message::tab(
                tab_id,
                tab::Message::Launch(home::Message::View(home::ViewMessage::GoToSection(
                    home::HomeSection::Connect(crate::app::menu::ConnectSubMenu::Overview),
                ))),
            ),
        )));
        assert!(gui.connect_sheet.is_some());
        gui.panes.get_mut(pane_id).unwrap().tabs[0].state = pin_entry(&root);
        assert_eq!(
            gui.update(Message::ConnectSheet(ConnectSheetMessage::Account(
                crate::app::view::ConnectAccountMessage::LogOut,
            )))
            .units(),
            0,
        );
        assert!(gui.connect_sheet.is_none());
        drop(gui);
        std::fs::remove_dir_all(path).ok();
    }

    #[test]
    fn connect_sheet_plan_cta_and_sections_use_existing_account_loaders() {
        // GUI updates reach teardown paths that close the process-global session.
        let _guard = crate::app::session::test_guard();
        // Measured at about 2 MiB of stack in debug builds; keep Windows headroom.
        let result = std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let _enter = runtime.enter();
                let path = std::env::temp_dir().join(format!(
                    "tenshu-connect-sheet-routing-{}",
                    uuid::Uuid::new_v4()
                ));
                std::fs::create_dir_all(&path).unwrap();
                let root = CoincubeDirectory::new(path.clone());
                let mut gui = launcher_gui(&root);
                let pane_id = gui.focus.unwrap();
                let tab_id = gui.panes.get(pane_id).unwrap().tabs[0].id;
                let tab::State::Home(home) = &mut gui.panes.get_mut(pane_id).unwrap().tabs[0].state
                else {
                    unreachable!()
                };
                home.connect_account.user = Some(crate::services::coincube::User {
                    id: 7,
                    email: "shell@example.invalid".into(),
                    email_verified: Some(true),
                });
                home.connect_account.step = crate::app::state::connect::ConnectFlowStep::Dashboard;

                drop(gui.update(Message::Pane(
                    pane_id,
                    pane::Message::tab(
                        tab_id,
                        tab::Message::Launch(home::Message::View(home::ViewMessage::GoToSection(
                            home::HomeSection::Connect(crate::app::menu::ConnectSubMenu::Overview),
                        ))),
                    ),
                )));
                assert_eq!(
                    gui.connect_sheet.as_ref().unwrap().page,
                    ConnectSheetPage::Sections(ConnectSheetSection::Account)
                );
                drop(
                    gui.update(Message::ConnectSheet(ConnectSheetMessage::Account(
                        crate::app::view::ConnectAccountMessage::OpenPlanBilling,
                    ))),
                );
                assert_eq!(
                    gui.connect_sheet.as_ref().unwrap().page,
                    ConnectSheetPage::Sections(ConnectSheetSection::PlanBilling)
                );
                let tab::State::Home(home) = &gui.panes.get(pane_id).unwrap().tabs[0].state else {
                    unreachable!()
                };
                assert_eq!(
                    home.connect_account.active_sub,
                    crate::app::menu::ConnectSubMenu::PlanBilling
                );

                drop(
                    gui.update(Message::ConnectSheet(ConnectSheetMessage::SelectSection(
                        ConnectSheetSection::Contacts,
                    ))),
                );
                let tab::State::Home(home) = &gui.panes.get(pane_id).unwrap().tabs[0].state else {
                    unreachable!()
                };
                assert!(home.connect_account.contacts_state.loading);
                assert_eq!(
                    home.connect_account.active_sub,
                    crate::app::menu::ConnectSubMenu::Contacts
                );
                assert!(
                    gui.update(Message::ConnectSheet(ConnectSheetMessage::SelectSection(
                        ConnectSheetSection::DevicesSecurity
                    ),))
                        .units()
                        > 0
                );

                drop(gui);
                std::fs::remove_dir_all(path).ok();
            })
            .unwrap()
            .join();
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }

    #[test]
    fn home_overlay_and_queued_intent_expire_when_unlock_replaces_home() {
        // GUI updates reach teardown paths that close the process-global session.
        let _guard = crate::app::session::test_guard();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _enter = runtime.enter();
        let path = std::env::temp_dir().join(format!(
            "tenshu-preferences-stale-home-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&path).unwrap();
        let root = CoincubeDirectory::new(path.clone());
        let mut gui = launcher_gui(&root);
        let pane_id = gui.focus.unwrap();
        let tab_id = gui.panes.get(pane_id).unwrap().tabs[0].id;
        let open = || {
            Message::Pane(
                pane_id,
                pane::Message::tab(
                    tab_id,
                    tab::Message::OpenAppPreferences(tab::PreferencesSource::Home),
                ),
            )
        };

        drop(gui.update(open()));
        let origin = gui.preferences_sheet.as_ref().unwrap().origin.clone();
        gui.panes.get_mut(pane_id).unwrap().tabs[0].state = pin_entry(&root);
        assert!(!gui.overlay_origin_is_current(&origin));
        assert_eq!(
            gui.update(Message::PreferencesSheet(
                preferences_view::Message::Action(preferences_view::Action::Close,)
            ))
            .units(),
            0,
            "a stale close must not focus the Launcher behind PIN entry"
        );
        assert!(gui.preferences_sheet.is_none());
        assert_eq!(gui.update(open()).units(), 0);
        assert!(gui.preferences_sheet.is_none());
        drop(gui);
        std::fs::remove_dir_all(path).ok();
    }

    fn zoom_key(character: &str, modifiers: keyboard::Modifiers) -> Event {
        let key = keyboard::Key::Character(character.into());
        Event::Keyboard(keyboard::Event::KeyPressed {
            key: key.clone(),
            modified_key: key,
            physical_key: keyboard::key::Physical::Code(keyboard::key::Code::Equal),
            location: keyboard::Location::Standard,
            modifiers,
            text: None,
            repeat: false,
        })
    }

    #[test]
    fn command_plus_minus_and_zero_are_zoom_shortcuts() {
        let command = keyboard::Modifiers::COMMAND;
        for (character, step) in [
            ("=", ZoomStep::In),
            ("+", ZoomStep::In),
            ("-", ZoomStep::Out),
            ("0", ZoomStep::ActualSize),
        ] {
            assert_eq!(
                ignored_shell_key(&zoom_key(character, command), event::Status::Ignored),
                Some(Key::Zoom(step)),
                "{character}"
            );
        }
        // Plain typing and a focused widget's own keys are not shortcuts.
        assert_eq!(
            ignored_shell_key(
                &zoom_key("=", keyboard::Modifiers::empty()),
                event::Status::Ignored
            ),
            None
        );
        assert_eq!(
            ignored_shell_key(&zoom_key("=", command), event::Status::Captured),
            None
        );
    }

    #[test]
    fn zoom_applies_at_once_and_saves_the_last_step_once_the_lane_is_free() {
        let _guard = crate::app::session::test_guard();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _enter = runtime.enter();
        let path = std::env::temp_dir().join(format!("tenshu-zoom-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        let root = CoincubeDirectory::new(path.clone());
        let mut gui = launcher_gui(&root);
        assert_eq!(gui.scale_factor(), 1.0);

        // Another save holds the lane, so the shortcut cannot save yet.
        let busy = gui
            .preferences
            .begin_save(PreferenceChange::LauncherStyle(LauncherStyle::List))
            .unwrap();
        drop(gui.update(Message::KeyPressed(Key::Zoom(ZoomStep::In))));
        drop(gui.update(Message::KeyPressed(Key::Zoom(ZoomStep::In))));
        assert_eq!(gui.scale_factor(), 1.25, "both steps apply before any save");
        assert!(gui.zoom.notice_until.is_some(), "the pill shows");
        assert_eq!(
            gui.preferences.pending_operation(),
            Some(Operation::Save(PreferenceChange::LauncherStyle(
                LauncherStyle::List
            )))
        );

        // When that save lands, the newest zoom is saved, not the first step.
        let receipt = GlobalSettings::save_preference(
            &busy.root,
            PreferenceChange::LauncherStyle(LauncherStyle::List),
        )
        .unwrap();
        drop(gui.update(Message::Preferences(busy, Ok(receipt))));
        assert_eq!(
            gui.preferences.pending_operation(),
            Some(Operation::Save(PreferenceChange::Zoom(Zoom::from(125))))
        );
        assert_eq!(gui.scale_factor(), 1.25);
        drop(gui);
        std::fs::remove_dir_all(path).ok();
    }

    /// The dock's source menu saves on the same lane as App preferences.
    /// Nothing else the Launcher sends is a device preference: it has no
    /// balances eye, so a hide-balances change from it is dropped.
    #[test]
    fn the_launcher_dock_saves_only_the_activity_source() {
        use crate::app::settings::preferences::NetworkActivitySource;
        let _guard = crate::app::session::test_guard();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _enter = runtime.enter();
        let path =
            std::env::temp_dir().join(format!("tenshu-dock-source-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        let root = CoincubeDirectory::new(path.clone());
        GlobalSettings::load_or_initialize_preferences(&root).unwrap();
        let mut gui = launcher_gui(&root);
        let pane_id = gui.focus.unwrap();
        let tab_id = gui.panes.get(pane_id).unwrap().tabs[0].id;
        let from_dock = |change| {
            Message::Pane(
                pane_id,
                pane::Message::Tab(tab_id, Box::new(tab::Message::HomeDevicePreference(change))),
            )
        };

        drop(gui.update(from_dock(PreferenceChange::HideBalances(true))));
        assert_eq!(gui.preferences.pending_operation(), None);

        let local = PreferenceChange::NetworkActivitySource(NetworkActivitySource::LocalNode);
        drop(gui.update(from_dock(local)));
        assert_eq!(
            gui.preferences.pending_operation(),
            Some(Operation::Save(local))
        );
        drop(gui);
        std::fs::remove_dir_all(path).ok();
    }

    #[test]
    fn a_zoom_save_shows_the_balance_toggle_busy() {
        let _guard = crate::app::session::test_guard();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _enter = runtime.enter();
        let path = std::env::temp_dir().join(format!("tenshu-zoom-busy-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        let root = CoincubeDirectory::new(path.clone());
        let mut gui = launcher_gui(&root);
        // The Launcher draws no eye, but its tab keeps the chrome for the
        // Cube it opens next.
        let launcher_chrome = |gui: &GUI| {
            let pane = gui.panes.get(gui.focus.unwrap()).unwrap();
            assert!(
                matches!(pane.tabs[0].state, tab::State::Home(_)),
                "the launcher starts as Home"
            );
            pane.tabs[0]
                .balance_privacy
                .expect("the shell publishes privacy chrome")
        };
        gui.publish_balance_privacy();
        assert!(launcher_chrome(&gui).interactive);

        drop(gui.update(Message::KeyPressed(Key::Zoom(ZoomStep::In))));
        assert_eq!(
            gui.preferences.pending_operation(),
            Some(Operation::Save(PreferenceChange::Zoom(Zoom::from(110))))
        );
        let chrome = launcher_chrome(&gui);
        assert!(chrome.busy, "the zoom save holds the lane");
        assert!(!chrome.interactive, "a Hide click cannot be refused unseen");
        drop(gui);
        std::fs::remove_dir_all(path).ok();
    }

    #[test]
    fn a_failed_zoom_save_reverts_only_the_step_it_was_saving() {
        let _guard = crate::app::session::test_guard();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _enter = runtime.enter();
        let path =
            std::env::temp_dir().join(format!("tenshu-zoom-failed-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        let root = CoincubeDirectory::new(path.clone());
        let mut gui = launcher_gui(&root);
        let save_110 = |gui: &mut GUI| {
            gui.zoom.pending = Some(Zoom::from(110));
            gui.preferences
                .begin_save(PreferenceChange::Zoom(Zoom::from(110)))
                .unwrap()
        };

        // The step being saved fails: the zoom goes back to the saved 100%.
        let request = save_110(&mut gui);
        drop(gui.update(Message::Preferences(request, Err("disk full".to_string()))));
        assert_eq!(gui.zoom.pending, None);
        assert_eq!(gui.scale_factor(), 1.0);

        // A newer step pressed during the failed save stays and is saved next.
        let request = save_110(&mut gui);
        drop(gui.update(Message::KeyPressed(Key::Zoom(ZoomStep::In))));
        assert_eq!(gui.scale_factor(), 1.25);
        drop(gui.update(Message::Preferences(request, Err("disk full".to_string()))));
        assert_eq!(gui.scale_factor(), 1.25);
        assert_eq!(
            gui.preferences.pending_operation(),
            Some(Operation::Save(PreferenceChange::Zoom(Zoom::from(125))))
        );
        drop(gui);
        std::fs::remove_dir_all(path).ok();
    }

    #[test]
    fn the_saved_window_size_is_recorded_at_actual_size() {
        let _guard = crate::app::session::test_guard();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _enter = runtime.enter();
        let path =
            std::env::temp_dir().join(format!("tenshu-zoom-window-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        let root = CoincubeDirectory::new(path.clone());
        let mut gui = launcher_gui(&root);
        gui.window_config = Some(WindowConfig {
            width: 1440.0,
            height: 900.0,
        });
        gui.zoom.pending = Some(Zoom::from(125));

        // At 125% iced reports a 1440 x 900 window as 1152 x 720.
        drop(gui.update(Message::WindowSize(Size::new(1152.0, 720.0))));
        let saved = gui.window_config.clone().unwrap();
        assert_eq!((saved.width, saved.height), (1440.0, 900.0));
        // Restoring it asks iced for the same window in zoomed units.
        assert_eq!(
            gui.zoomed(Size::new(saved.width, saved.height)),
            Size::new(1152.0, 720.0)
        );
        drop(gui);
        std::fs::remove_dir_all(path).ok();
    }

    #[test]
    fn zoom_stops_at_the_first_and_last_step() {
        let _guard = crate::app::session::test_guard();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _enter = runtime.enter();
        let path = std::env::temp_dir().join(format!("tenshu-zoom-ends-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        let root = CoincubeDirectory::new(path.clone());
        let mut gui = launcher_gui(&root);
        for _ in 0..10 {
            drop(gui.update(Message::KeyPressed(Key::Zoom(ZoomStep::Out))));
        }
        assert_eq!(gui.scale_factor(), 0.8);
        for _ in 0..10 {
            drop(gui.update(Message::KeyPressed(Key::Zoom(ZoomStep::In))));
        }
        assert_eq!(gui.scale_factor(), 1.5);
        drop(gui.update(Message::KeyPressed(Key::Zoom(ZoomStep::ActualSize))));
        assert_eq!(gui.scale_factor(), 1.0);
        drop(gui);
        std::fs::remove_dir_all(path).ok();
    }

    #[test]
    fn a_laptop_screen_caps_the_zoom_at_what_it_lays_out_at_full_width() {
        let _guard = crate::app::session::test_guard();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _enter = runtime.enter();
        let path = std::env::temp_dir().join(format!("tenshu-zoom-cap-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        let root = CoincubeDirectory::new(path.clone());
        let mut gui = launcher_gui(&root);
        // A 1440-point laptop, reported at 100%.
        drop(gui.update(Message::ScreenSize(
            Some(Size::new(1440.0, 900.0)),
            Zoom::ACTUAL_SIZE,
        )));
        assert_eq!(gui.zoom.screen_width, Some(1440.0));
        for _ in 0..10 {
            drop(gui.update(Message::KeyPressed(Key::Zoom(ZoomStep::In))));
        }
        assert_eq!(gui.scale_factor(), 1.25, "150% would leave 960 points");
        // The Zoom row cannot pick it either.
        drop(gui.change_preference(PreferenceChange::Zoom(Zoom::LARGEST)));
        assert_eq!(gui.current_zoom(), Zoom::from(125));
        assert_eq!(gui.preferences_view_model().max_zoom, Some(Zoom::from(125)));
        drop(gui);
        std::fs::remove_dir_all(path).ok();
    }

    #[test]
    fn a_zoom_saved_on_a_larger_screen_reads_as_the_largest_that_fits() {
        let _guard = crate::app::session::test_guard();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _enter = runtime.enter();
        let path =
            std::env::temp_dir().join(format!("tenshu-zoom-saved-cap-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        let root = CoincubeDirectory::new(path.clone());
        let mut gui = launcher_gui(&root);
        gui.zoom.pending = Some(Zoom::LARGEST);
        assert_eq!(
            gui.scale_factor(),
            1.5,
            "uncapped until the screen is known"
        );
        gui.zoom.screen_width = Some(1440.0);
        assert_eq!(gui.scale_factor(), 1.25);
        gui.zoom.screen_width = Some(2560.0);
        assert_eq!(gui.scale_factor(), 1.5, "back on the monitor, unchanged");
        drop(gui);
        std::fs::remove_dir_all(path).ok();
    }

    #[test]
    fn a_screen_size_asked_at_another_zoom_is_asked_again() {
        let _guard = crate::app::session::test_guard();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _enter = runtime.enter();
        let path = std::env::temp_dir().join(format!("tenshu-zoom-stale-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        let root = CoincubeDirectory::new(path.clone());
        let mut gui = launcher_gui(&root);
        gui.window_id = Some(Id::unique());
        gui.zoom.pending = Some(Zoom::from(125));
        // Asked at 110%, measured at 125% on a 1440-point laptop: 1152 x 1.1
        // would read as 1267 points and cap the zoom at 110%.
        drop(gui.update(Message::ScreenSize(
            Some(Size::new(1152.0, 720.0)),
            Zoom::from(110),
        )));
        assert_eq!(gui.zoom.screen_width, None);
        assert_eq!(gui.current_zoom(), Zoom::from(125));
        // f32 scaling leaves a 1536-point screen a hair short of 150%.
        gui.zoom.pending = Some(Zoom::from(90));
        drop(gui.update(Message::ScreenSize(
            Some(Size::new(1706.6665, 960.0)),
            Zoom::from(90),
        )));
        assert_eq!(gui.zoom.screen_width, Some(1536.0));
        assert_eq!(gui.max_zoom(), Zoom::LARGEST);
        drop(gui);
        std::fs::remove_dir_all(path).ok();
    }

    #[test]
    fn the_minimum_window_lays_pages_out_at_least_1024_points_wide() {
        for percent in [80, 90, 100, 110, 125, 150] {
            let zoom = Zoom::from(percent);
            let laid_out = GUI::min_window_size(zoom);
            let screen = laid_out * zoom.factor();
            assert!(
                laid_out.width >= Zoom::LAYOUT_MIN_WIDTH - 0.01,
                "{}%: {:?}",
                percent,
                laid_out
            );
            // Never smaller on screen than the 1024 x 768 window of 100%.
            assert!(
                screen.width >= 1024.0 - 0.01 && (screen.height - 768.0).abs() < 0.01,
                "{}%: {:?}",
                percent,
                screen
            );
        }
        assert_eq!(
            GUI::min_window_size(Zoom::from(125)) * 1.25,
            Size::new(1280.0, 768.0)
        );
    }

    #[test]
    fn the_minimum_window_follows_the_zoom_once_the_screen_is_known() {
        let _guard = crate::app::session::test_guard();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _enter = runtime.enter();
        let path =
            std::env::temp_dir().join(format!("tenshu-zoom-min-size-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        let root = CoincubeDirectory::new(path.clone());
        let mut gui = launcher_gui(&root);
        gui.window_id = Some(Id::unique());
        drop(gui.update(Message::KeyPressed(Key::Zoom(ZoomStep::In))));
        assert_eq!(gui.zoom.min_size_for, None, "main.rs's minimum stands");
        drop(gui.update(Message::ScreenSize(
            Some(Size::new(1309.0, 818.0)),
            Zoom::from(110),
        )));
        assert_eq!(gui.zoom.min_size_for, Some(Zoom::from(110)));
        drop(gui.update(Message::KeyPressed(Key::Zoom(ZoomStep::In))));
        assert_eq!(gui.zoom.min_size_for, Some(Zoom::from(125)));
        drop(gui);
        std::fs::remove_dir_all(path).ok();
    }

    #[test]
    fn worldview_voice_notice_follows_only_a_changed_accepted_save() {
        use coincube_ui::theme::WorldviewMode;

        let _guard = crate::app::session::test_guard();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _enter = runtime.enter();
        let directory =
            std::env::temp_dir().join(format!("tenshu-worldview-voice-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let root = CoincubeDirectory::new(directory.clone());
        let mut gui = launcher_gui(&root);
        let pane = gui.focus.unwrap();
        let tab = gui.panes.get(pane).unwrap().tabs[0].id;
        drop(gui.update(Message::Pane(
            pane,
            pane::Message::tab(
                tab,
                tab::Message::OpenAppPreferences(tab::PreferencesSource::Home),
            ),
        )));
        assert_eq!(gui.preferences_view_model().worldview_changed, None);

        let change = PreferenceChange::Worldview(WorldviewMode::Luke);
        let failed = gui.preferences.begin_save(change).unwrap();
        drop(gui.update(Message::Preferences(failed, Err("disk full".into()))));
        assert_eq!(gui.preferences_view_model().worldview_changed, None);
        assert_eq!(gui.worldview(), WorldviewMode::Samurai);

        let request = gui.preferences.begin_save(change).unwrap();
        let receipt = GlobalSettings::save_preference(&request.root, change).unwrap();
        drop(gui.update(Message::Preferences(request, Ok(receipt))));
        assert_eq!(
            gui.preferences_view_model().worldview_changed,
            Some(WorldviewMode::Luke)
        );
        assert_eq!(gui.worldview(), WorldviewMode::Luke);
        drop(gui.close_preferences_sheet());
        assert_eq!(gui.preferences_view_model().worldview_changed, None);
        drop(gui);
        std::fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn ordinary_refresh_and_display_edits_preserve_the_home_section() {
        // GUI updates reach teardown paths that close the process-global session.
        let _guard = crate::app::session::test_guard();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _enter = runtime.enter();
        let path = std::env::temp_dir().join(format!(
            "tenshu-preferences-refresh-section-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&path).unwrap();
        let root = CoincubeDirectory::new(path.clone());
        let mut gui = launcher_gui(&root);
        let pane_id = gui.focus.unwrap();
        let tab = &mut gui.panes.get_mut(pane_id).unwrap().tabs[0];
        let tab::State::Home(home) = &mut tab.state else {
            panic!("the launcher starts as Home");
        };
        home.active_section = home::HomeSection::RecoverVault;

        let request = gui.preferences.begin_initialize().unwrap();
        let receipt = GlobalSettings::load_or_initialize_preferences(&root).unwrap();
        drop(gui.update(Message::Preferences(request, Ok(receipt))));
        let tab::State::Home(home) = &gui.panes.get(pane_id).unwrap().tabs[0].state else {
            panic!("refresh replaced Home");
        };
        assert_eq!(home.active_section, home::HomeSection::RecoverVault);

        for change in [
            PreferenceChange::LauncherStyle(LauncherStyle::List),
            PreferenceChange::Motion(MotionPreference::Reduced),
        ] {
            let request = gui.preferences.begin_save(change).unwrap();
            let receipt = GlobalSettings::save_preference(&request.root, change).unwrap();
            drop(gui.update(Message::Preferences(request, Ok(receipt))));
            let tab::State::Home(home) = &gui.panes.get(pane_id).unwrap().tabs[0].state else {
                panic!("display edit replaced Home");
            };
            assert_eq!(home.active_section, home::HomeSection::RecoverVault);
        }
        drop(gui);
        std::fs::remove_dir_all(path).ok();
    }
}

#[cfg(test)]
mod idle_activity_tests {
    use super::*;

    /// The idle auto-lock is only as good as its definition of "activity".
    ///
    /// This previously counted *every non-`Tick` message* as user activity,
    /// which meant it never fired: the 1 Hz tick spawns `UpdateDaemonCache` and
    /// `BitcoindNetStats`, whose results arrive as non-`Tick` messages, so the
    /// timer reset once a second forever. The bug was invisible — the code read
    /// correctly, compiled, and shipped a security control that did nothing.
    #[test]
    fn only_real_input_counts_as_activity() {
        use iced::keyboard;
        use iced::mouse;

        assert!(is_user_input(&Event::Keyboard(
            keyboard::Event::KeyReleased {
                key: keyboard::Key::Named(keyboard::key::Named::Enter),
                modified_key: keyboard::Key::Named(keyboard::key::Named::Enter),
                physical_key: keyboard::key::Physical::Code(keyboard::key::Code::Enter),
                location: keyboard::Location::Standard,
                modifiers: keyboard::Modifiers::empty(),
            }
        )));
        assert!(is_user_input(&Event::Mouse(mouse::Event::ButtonPressed(
            mouse::Button::Left
        ))));
        assert!(is_user_input(&Event::Mouse(mouse::Event::CursorEntered)));

        // Window events are not a person. A resize or a focus change can come
        // from the OS, another app, or a monitor being plugged in — counting
        // them would let an unattended machine defer the lock indefinitely.
        assert!(!is_user_input(&Event::Window(iced::window::Event::Focused)));
        assert!(!is_user_input(&Event::Window(
            iced::window::Event::CloseRequested
        )));
    }
}

#[cfg(test)]
mod fork_auth_dispatch_tests {
    use super::*;
    use crate::{
        installer,
        services::coincube::{LoginResponse, User},
    };
    use coincube_core::chain::ChainId;
    use iced::futures::StreamExt;

    fn installer_state(root: &CoincubeDirectory) -> tab::State {
        let (mut installer, _) = installer::Installer::new(
            root.clone(),
            bitcoin::Network::Bitcoin,
            installer::UserFlow::CreateWallet,
            false,
            None,
            None,
            None,
            false,
            None,
        )
        .expect("a Bitcoin fixture installer");
        installer.context.bitcoin_config.chain = ChainId::BitcoinBlake2b;
        tab::State::Installer(Box::new(installer))
    }

    #[test]
    fn new_home_reuses_connect_client_while_sibling_is_being_admitted() {
        // GUI updates reach teardown paths that close the process-global session.
        let _guard = crate::app::session::test_guard();
        let root_path =
            std::env::temp_dir().join(format!("connect-home-reuse-{}", uuid::Uuid::new_v4()));
        let root = CoincubeDirectory::new(root_path.clone());
        let (mut first, first_startup) = home::Home::new_for_chain(root.clone(), None);
        drop(first_startup);
        let mut client = crate::services::coincube::CoincubeClient::new();
        client.set_token("shared-bearer");
        drop(first.adopt_connect_client(client));
        let (second, second_startup) = home::Home::new_for_chain(root.clone(), None);
        drop(second_startup);
        let mut pane = pane::Pane::new_with_tab(tab::State::Home(first));
        pane.tabs.push(tab::Tab::new(2, tab::State::Home(second)));
        let (panes, pane_id) = pane_grid::State::new(pane);
        let mut gui = GUI {
            panes,
            focus: Some(pane_id),
            config: Config::new(root, None),
            window_id: None,
            window_init: None,
            window_config: None,
            global_cache: GlobalCache::default(),
            theme_mode: Default::default(),
            system_theme: None,
            system_reduces_motion: None,
            preferences_sheet: None,
            connect_sheet: None,
            preferences: unresolved_test_preferences(),
            zoom: ZoomState::default(),
            worldview_sharing: worldview::Sharing::default(),
        };
        drop(gui.update(Message::Pane(
            pane_id,
            pane::Message::tab(
                2,
                tab::Message::Launch(home::Message::View(home::ViewMessage::ConnectAccount(
                    crate::app::view::ConnectAccountMessage::Init,
                ))),
            ),
        )));
        let tab::State::Home(second) = &gui.panes.get(pane_id).unwrap().tabs[1].state else {
            panic!("second tab should remain Home");
        };
        assert_eq!(
            second
                .connect_account
                .authenticated_client()
                .and_then(|client| client.token().map(str::to_owned)),
            Some("shared-bearer".into()),
            "opening Home should use its sibling's session instead of refreshing it"
        );
        let _ = std::fs::remove_dir_all(root_path);
    }

    #[test]
    fn accepted_home_login_reaches_an_already_open_fork_pin() {
        // GUI updates reach teardown paths that close the process-global session.
        let _guard = crate::app::session::test_guard();
        let result = std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let _enter = runtime.enter();
                let root_path = std::env::temp_dir()
                    .join(format!("pin-session-handoff-{}", uuid::Uuid::new_v4()));
                let root = CoincubeDirectory::new(root_path.clone());
                std::fs::create_dir_all(&root_path).unwrap();
                let (mut home, startup) = home::Home::new_for_chain(root.clone(), None);
                drop(startup);
                let mut client =
                    crate::services::coincube::CoincubeClient::for_test("http://127.0.0.1:1");
                client.set_token("old-session");
                home.connect_account.client = client.clone();
                home.connect_account.step =
                    crate::app::state::connect::account::ConnectFlowStep::OtpVerification {
                        email: "fixture@example.invalid".into(),
                        otp: "123456".into(),
                        sending: true,
                        is_signup: false,
                        cooldown: 0,
                    };
                let pin = crate::pin_entry::PinEntry::new(
                    crate::app::settings::CubeSettings::new(
                        "Pending fork".into(),
                        ChainId::BitcoinBlake2b,
                    ),
                    root_path.clone(),
                    crate::pin_entry::PinEntrySuccess::LoadApp {
                        datadir: root.clone(),
                        config: crate::app::Config::new(false),
                        network: bitcoin::Network::Bitcoin,
                        internal_bitcoind: None,
                        backup: None,
                        wallet_settings: None,
                        connect_client: Some(client),
                    },
                    None,
                );
                let mut pane = pane::Pane::new_with_tab(tab::State::Home(home));
                pane.tabs.push(tab::Tab::new(2, tab::State::PinEntry(pin)));
                let (panes, pane_id) = pane_grid::State::new(pane);
                let mut gui = GUI {
                    panes,
                    focus: Some(pane_id),
                    config: Config::new(root, None),
                    window_id: None,
                    window_init: None,
                    window_config: None,
                    global_cache: GlobalCache::default(),
                    theme_mode: Default::default(),
                    system_theme: None,
                    system_reduces_motion: None,
                    preferences_sheet: None,
                    connect_sheet: None,
                    preferences: unresolved_test_preferences(),
                    zoom: ZoomState::default(),
                    worldview_sharing: worldview::Sharing::default(),
                };
                let login = LoginResponse {
                    requires_2fa: false,
                    token: "new-session".into(),
                    refresh_token: "new-refresh".into(),
                    user: User {
                        id: 42,
                        email: "fixture@example.invalid".into(),
                        email_verified: Some(true),
                    },
                };
                drop(gui.update(Message::Pane(
                    pane_id,
                    pane::Message::tab(
                        1,
                        tab::Message::Launch(home::Message::View(
                            home::ViewMessage::ConnectAccount(
                                crate::app::view::ConnectAccountMessage::OtpVerified(
                                    login.clone(),
                                    0,
                                    99, // old OTP attempt, same auth epoch
                                ),
                            ),
                        )),
                    ),
                )));
                let tab::State::PinEntry(pin) = &gui.panes.get(pane_id).unwrap().tabs[1].state
                else {
                    unreachable!()
                };
                let crate::pin_entry::PinEntrySuccess::LoadApp { connect_client, .. } =
                    &pin.on_success;
                assert_eq!(
                    connect_client.as_ref().and_then(|client| client.token()),
                    Some("old-session"),
                    "rejected OTP attempt must not broadcast SignIn to the PIN tab"
                );
                drop(gui.update(Message::Pane(
                    pane_id,
                    pane::Message::tab(
                        1,
                        tab::Message::Launch(home::Message::View(
                            home::ViewMessage::ConnectAccount(
                                crate::app::view::ConnectAccountMessage::OtpVerified(login, 0, 0),
                            ),
                        )),
                    ),
                )));
                let account = |message| {
                    Message::Pane(
                        pane_id,
                        pane::Message::tab(
                            1,
                            tab::Message::Launch(home::Message::View(
                                home::ViewMessage::ConnectAccount(message),
                            )),
                        ),
                    )
                };
                let ready = || {
                    Message::Pane(
                        pane_id,
                        pane::Message::tab(
                            1,
                            tab::Message::Launch(home::Message::ConnectSignedInBubble),
                        ),
                    )
                };
                // A token response is not admission: the account gate has not run.
                drop(gui.update(ready()));
                let tab::State::PinEntry(pin) = &gui.panes.get(pane_id).unwrap().tabs[1].state
                else {
                    unreachable!()
                };
                let crate::pin_entry::PinEntrySuccess::LoadApp { connect_client, .. } =
                    &pin.on_success;
                assert!(
                    connect_client.is_none(),
                    "login token alone grants no PIN candidate"
                );
                drop(gui.update(account(
                    crate::app::view::ConnectAccountMessage::SessionLoaded {
                        user: User {
                            id: 42,
                            email: "fixture@example.invalid".into(),
                            email_verified: Some(true),
                        },
                        plan: None,
                        epoch: 0,
                    },
                )));
                let tab::State::Home(home) = &gui.panes.get(pane_id).unwrap().tabs[0].state else {
                    unreachable!()
                };
                let generation = home.connect_account.session_generation();
                drop(gui.update(ready()));
                let tab::State::PinEntry(pin) = &gui.panes.get(pane_id).unwrap().tabs[1].state
                else {
                    unreachable!()
                };
                let crate::pin_entry::PinEntrySuccess::LoadApp { connect_client, .. } =
                    &pin.on_success;
                assert!(
                    connect_client.is_none(),
                    "checking account grants no PIN candidate"
                );
                for outcome in [
                    crate::services::coincube::DuressCheckOutcome::Unreachable,
                    crate::services::coincube::DuressCheckOutcome::Ok(
                        crate::services::coincube::DuressState {
                            active: true,
                            unlock_at: None,
                            enrolled: false,
                            this_device_registered: false,
                        },
                    ),
                ] {
                    drop(gui.update(account(
                        crate::app::view::ConnectAccountMessage::DuressStateChecked(
                            outcome, generation, 2,
                        ),
                    )));
                    drop(gui.update(ready()));
                    let tab::State::PinEntry(pin) = &gui.panes.get(pane_id).unwrap().tabs[1].state
                    else {
                        unreachable!()
                    };
                    let crate::pin_entry::PinEntrySuccess::LoadApp { connect_client, .. } =
                        &pin.on_success;
                    assert!(
                        connect_client.is_none(),
                        "closed account gate grants no PIN candidate"
                    );
                }
                drop(gui.update(account(
                    crate::app::view::ConnectAccountMessage::DuressStateChecked(
                        crate::services::coincube::DuressCheckOutcome::Ok(
                            crate::services::coincube::DuressState {
                                active: false,
                                unlock_at: None,
                                enrolled: false,
                                this_device_registered: false,
                            },
                        ),
                        generation,
                        0,
                    ),
                )));
                drop(gui.update(ready()));
                let pending = gui
                    .panes
                    .get(pane_id)
                    .unwrap()
                    .tabs
                    .iter()
                    .find(|tab| tab.id == 2)
                    .unwrap();
                let tab::State::PinEntry(pin) = &pending.state else {
                    panic!("unlock must remain at PIN")
                };
                let crate::pin_entry::PinEntrySuccess::LoadApp { connect_client, .. } =
                    &pin.on_success;
                assert_eq!(
                    connect_client.as_ref().and_then(|client| client.token()),
                    Some("new-session"),
                    "a completed Home login must not strand the pending fork PIN signed out"
                );
                assert!(
                    pin.take_fork_signer().is_none(),
                    "session propagation must not unlock the seed"
                );
                let source = gui
                    .panes
                    .get_mut(pane_id)
                    .unwrap()
                    .tabs
                    .iter_mut()
                    .find(|tab| tab.id == 1)
                    .unwrap();
                let tab::State::Home(home) = &mut source.state else {
                    unreachable!()
                };
                home.connect_account.invalidate_auth();
                drop(gui.update(Message::Pane(
                    pane_id,
                    pane::Message::tab(
                        1,
                        tab::Message::Launch(home::Message::View(
                            home::ViewMessage::ConnectAccount(
                                crate::app::view::ConnectAccountMessage::SetSession(
                                    LoginResponse {
                                        requires_2fa: false,
                                        token: "stale-session".into(),
                                        refresh_token: "stale-refresh".into(),
                                        user: User {
                                            id: 99,
                                            email: "stale@example.invalid".into(),
                                            email_verified: Some(true),
                                        },
                                    },
                                    0,
                                ),
                            ),
                        )),
                    ),
                )));
                let pending = &gui.panes.get(pane_id).unwrap().tabs[1];
                let tab::State::PinEntry(pin) = &pending.state else {
                    unreachable!()
                };
                let crate::pin_entry::PinEntrySuccess::LoadApp { connect_client, .. } =
                    &pin.on_success;
                assert_eq!(
                    connect_client.as_ref().and_then(|client| client.token()),
                    Some("new-session"),
                    "stale login must not replace the candidate"
                );
                drop(gui.update(Message::Pane(
                    pane_id,
                    pane::Message::tab(
                        1,
                        tab::Message::Launch(home::Message::View(
                            home::ViewMessage::ConnectAccount(
                                crate::app::view::ConnectAccountMessage::LogOut,
                            ),
                        )),
                    ),
                )));
                let pending = &gui.panes.get(pane_id).unwrap().tabs[1];
                let tab::State::PinEntry(pin) = &pending.state else {
                    unreachable!()
                };
                let crate::pin_entry::PinEntrySuccess::LoadApp { connect_client, .. } =
                    &pin.on_success;
                assert!(
                    connect_client.is_none(),
                    "logout must withdraw the candidate"
                );
                assert!(pin.take_fork_signer().is_none());
                drop(gui);
                std::fs::remove_dir_all(root_path).unwrap();
            })
            .unwrap()
            .join();
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }

    #[test]
    fn home_open_only_adopts_completed_scoped_identity() {
        // GUI updates reach teardown paths that close the process-global session.
        let _guard = crate::app::session::test_guard();
        let result = std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let _enter = runtime.enter();
                let server = httpmock::MockServer::start();
                let user = server.mock(|when, then| {
                    when.method(httpmock::Method::GET).path("/api/v1/user")
                        .header("authorization", "Bearer direct-session");
                    then.status(200).json_body(serde_json::json!({
                        "id":8,"email":"other@example.invalid","email_verified":true
                    }));
                });
                let feature = server.mock(|when, then| {
                    when.method(httpmock::Method::GET).path("/api/v1/connect/features");
                    then.status(200).json_body(serde_json::json!({"data":{"plans":[],"bitcoin_blake2b_enabled":true}}));
                });
                let provider_url = server.base_url();
                for case in [
                    "direct", "checking", "active", "same",
                    "wrong-root",
                    "wrong-provider",
                    "account-conflict",
                    "token-conflict",
                ] {
                    let path = std::env::temp_dir()
                        .join(format!("cold-home-pin-{}", uuid::Uuid::new_v4()));
                    let root = CoincubeDirectory::new(path.clone());
                    std::fs::create_dir_all(&path).unwrap();
                    crate::app::settings::global::GlobalSettings::update_bitcoin_blake2b_beta(
                        &crate::app::settings::global::GlobalSettings::path(&root),
                        true,
                    )
                    .unwrap();
                    let provider = provider_url.as_str();
                    let ready =
                        |source_root: CoincubeDirectory, provider: &str, id: u32, token: &str| {
                            let (mut home, task) = home::Home::new_for_chain(source_root, None);
                            drop(task);
                            home.connect_account.client =
                                crate::services::coincube::CoincubeClient::for_test(provider);
                            home.connect_account.client.set_token(token);
                            home.connect_account.user = Some(User {
                                id,
                                email: "fixture@example.invalid".into(),
                                email_verified: Some(true),
                            });
                            home.connect_account.step =
                                crate::app::state::connect::account::ConnectFlowStep::Dashboard;
                            home
                        };
                    let source_root = if case == "wrong-root" {
                        CoincubeDirectory::new(path.join("other"))
                    } else {
                        root.clone()
                    };
                    let source_provider = if case == "wrong-provider" {
                        "http://127.0.0.1:2"
                    } else {
                        provider
                    };
                    let first = ready(source_root, source_provider, 7, "current-session");
                    let (mut cold, task) =
                        home::Home::new_for_chain(root.clone(), Some(ChainId::BitcoinBlake2b));
                    drop(task);
                    cold.connect_account.client =
                        crate::services::coincube::CoincubeClient::for_test(provider);
                    drop(cold.update(home::Message::GlobalFeatureLoaded(0, true)));
                    if matches!(case, "direct" | "checking" | "active") {
                        cold.connect_account.client.set_token("direct-session");
                        cold.connect_account.user = Some(User {
                            id:7,email:"fixture@example.invalid".into(),email_verified:Some(true),
                        });
                        cold.connect_account.step = match case {
                            "direct" => crate::app::state::connect::account::ConnectFlowStep::Dashboard,
                            "checking" => crate::app::state::connect::account::ConnectFlowStep::CheckingDuress {
                                status: crate::app::state::connect::account::DuressGateStatus::Checking,
                            },
                            _ => crate::app::state::connect::account::ConnectFlowStep::DuressRecovery {
                                unlock_at:None,passphrase:String::new(),submitting:false,cleared:false,
                            },
                        };
                        assert!(cold.connect_account.authenticated_client().is_some(),
                            "raw client presence is not completed account admission");
                    }

                    let mut pane = pane::Pane::new_with_tab(tab::State::Home(first));
                    pane.tabs.push(tab::Tab::new(2, tab::State::Home(cold)));
                    if case.ends_with("conflict") {
                        pane.tabs.push(tab::Tab::new(
                            3,
                            tab::State::Home(ready(
                                root.clone(),
                                provider,
                                if case == "account-conflict" { 8 } else { 7 },
                                "other-session",
                            )),
                        ));
                    }
                    let (panes, pane_id) = pane_grid::State::new(pane);
                    let mut gui = GUI {
                        panes,
                        focus: Some(pane_id),
                        config: Config::new(root.clone(), None),
                        window_id: None,
                        window_init: None,
                        window_config: None,
                        global_cache: GlobalCache::default(),
                        theme_mode: Default::default(),
            system_theme: None,
            system_reduces_motion: None,
            preferences_sheet: None,
            connect_sheet: None,
            preferences: unresolved_test_preferences(),
            zoom: ZoomState::default(),
                worldview_sharing: worldview::Sharing::default(),
                    };
                    let mut cube = crate::app::settings::CubeSettings::new(
                        "Cold fork".into(),
                        ChainId::BitcoinBlake2b,
                    );
                    cube.created_at = 0;
                    drop(gui.update(Message::Pane(
                        pane_id,
                        pane::Message::tab(
                            2,
                            tab::Message::Launch(home::Message::Run(
                                root,
                                crate::app::Config::new(false),
                                ChainId::BitcoinBlake2b,
                                cube,
                            )),
                        ),
                    )));
                    let pending = &mut gui.panes.get_mut(pane_id).unwrap().tabs[1];
                    let tab::State::PinEntry(pin) = &mut pending.state else {
                        panic!("{} must reach PIN", case)
                    };
                    let crate::pin_entry::PinEntrySuccess::LoadApp { connect_client, .. } =
                        &pin.on_success;
                    assert_eq!(
                        connect_client.as_ref().and_then(|client| client.token()),
                        if case == "direct" {
                            Some("direct-session")
                        } else if case == "same" {
                            Some("current-session")
                        } else {
                            None
                        },
                        "{}",
                        case
                    );
                    assert!(pin.take_fork_signer().is_none());
                    if case == "direct" {
                        // Exercise the actual Home -> PIN candidate. A changed
                        // server identity must be refused before feature/decrypt work.
                        assert_eq!(user.hits(), 0);
                        for (index, digit) in "1234".chars().enumerate() {
                            drop(pin.update(crate::pin_entry::Message::PinInput(
                                crate::pin_input::Message::DigitChanged(index,digit.to_string()))));
                        }
                        let task = pin.update(crate::pin_entry::Message::Submit);
                        let messages = runtime.block_on(async {
                            use iced::futures::StreamExt;
                            let mut messages = Vec::new();
                            if let Some(mut stream) = iced_runtime::task::into_stream(task) {
                                while let Some(action) = stream.next().await {
                                    if let iced_runtime::Action::Output(message) = action {
                                        messages.push(message);
                                    }
                                }
                            }
                            messages
                        });
                        for message in messages { drop(pin.update(message)); }
                        user.assert_hits(1);
                        feature.assert_hits(0);
                        assert!(pin.take_fork_signer().is_none());
                    }
                    drop(gui);
                    std::fs::remove_dir_all(path).unwrap();
                }
            })
            .unwrap()
            .join();
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }

    #[test]
    fn shared_home_restore_ready_reaches_pending_pin_without_set_session() {
        // GUI updates reach teardown paths that close the process-global session.
        let _guard = crate::app::session::test_guard();
        let result = std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let _enter = runtime.enter();
                let path = std::env::temp_dir()
                    .join(format!("restored-home-pin-{}", uuid::Uuid::new_v4()));
                let root = CoincubeDirectory::new(path.clone());
                std::fs::create_dir_all(&path).unwrap();
                let provider = "http://127.0.0.1:1";
                let mut client = crate::services::coincube::CoincubeClient::for_test(provider);
                client.set_token("restored-session");
                let (mut home, startup) = home::Home::new_for_chain(root.clone(), None);
                drop(startup);
                drop(home.adopt_connect_client(client));
                let generation = home.connect_account.session_generation();
                let epoch = home.connect_account.auth_epoch();
                let mut pin = crate::pin_entry::PinEntry::new(
                    crate::app::settings::CubeSettings::new(
                        "Pending fork".into(),
                        ChainId::BitcoinBlake2b,
                    ),
                    path.clone(),
                    crate::pin_entry::PinEntrySuccess::LoadApp {
                        datadir: root.clone(),
                        config: crate::app::Config::new(false),
                        network: bitcoin::Network::Bitcoin,
                        internal_bitcoind: None,
                        backup: None,
                        wallet_settings: None,
                        connect_client: None,
                    },
                    None,
                );
                pin.set_connect_provider(provider);
                let mut pane = pane::Pane::new_with_tab(tab::State::Home(home));
                pane.tabs.push(tab::Tab::new(2, tab::State::PinEntry(pin)));
                let (panes, pane_id) = pane_grid::State::new(pane);
                let mut gui = GUI {
                    panes,
                    focus: Some(pane_id),
                    config: Config::new(root, None),
                    window_id: None,
                    window_init: None,
                    window_config: None,
                    global_cache: GlobalCache::default(),
                    theme_mode: Default::default(),
                    system_theme: None,
                    system_reduces_motion: None,
                    preferences_sheet: None,
                    connect_sheet: None,
                    preferences: unresolved_test_preferences(),
                    zoom: ZoomState::default(),
                    worldview_sharing: worldview::Sharing::default(),
                };
                let account = |message| {
                    Message::Pane(
                        pane_id,
                        pane::Message::tab(
                            1,
                            tab::Message::Launch(home::Message::View(
                                home::ViewMessage::ConnectAccount(message),
                            )),
                        ),
                    )
                };
                // Run the real restore completion: AdmittedUserLoaded dispatches
                // SessionLoaded synchronously, which waits for the duress check.
                // Drop its network tasks; this fixture supplies only their typed outcomes.
                drop(gui.update(account(
                    crate::app::view::ConnectAccountMessage::AdmittedUserLoaded {
                        user: Ok(User {
                            id: 7,
                            email: "fixture@example.invalid".into(),
                            email_verified: Some(true),
                        }),
                        generation,
                        epoch,
                    },
                )));
                let tab::State::Home(home) = &gui.panes.get(pane_id).unwrap().tabs[0].state else {
                    unreachable!()
                };
                assert!(!home.connect_account.is_authenticated());
                let generation = home.connect_account.session_generation();
                let ready = || {
                    Message::Pane(
                        pane_id,
                        pane::Message::tab(
                            1,
                            tab::Message::Launch(home::Message::ConnectSignedInBubble),
                        ),
                    )
                };
                drop(gui.update(ready()));
                let tab::State::PinEntry(pin) = &gui.panes.get(pane_id).unwrap().tabs[1].state
                else {
                    unreachable!()
                };
                let crate::pin_entry::PinEntrySuccess::LoadApp { connect_client, .. } =
                    &pin.on_success;
                assert!(
                    connect_client.is_none(),
                    "checking origin grants no candidate"
                );
                drop(gui.update(account(
                    crate::app::view::ConnectAccountMessage::DuressStateChecked(
                        crate::services::coincube::DuressCheckOutcome::Ok(
                            crate::services::coincube::DuressState {
                                active: false,
                                unlock_at: None,
                                enrolled: false,
                                this_device_registered: false,
                            },
                        ),
                        generation,
                        0,
                    ),
                )));
                let tab::State::Home(home) = &gui.panes.get(pane_id).unwrap().tabs[0].state else {
                    unreachable!()
                };
                assert!(home.connect_account.is_authenticated());
                // The exact-origin bubble emitted on that ready edge repairs the PIN.
                drop(gui.update(ready()));
                let tab::State::PinEntry(pin) = &gui.panes.get(pane_id).unwrap().tabs[1].state
                else {
                    unreachable!()
                };
                let crate::pin_entry::PinEntrySuccess::LoadApp { connect_client, .. } =
                    &pin.on_success;
                assert_eq!(
                    connect_client.as_ref().and_then(|client| client.token()),
                    Some("restored-session")
                );
                assert!(pin.take_fork_signer().is_none());
                drop(gui.update(account(crate::app::view::ConnectAccountMessage::LogOut)));
                drop(gui.update(ready()));
                let tab::State::PinEntry(pin) = &gui.panes.get(pane_id).unwrap().tabs[1].state
                else {
                    unreachable!()
                };
                let crate::pin_entry::PinEntrySuccess::LoadApp { connect_client, .. } =
                    &pin.on_success;
                assert!(
                    connect_client.is_none(),
                    "stale queued readiness cannot revive logout"
                );
                drop(gui);
                std::fs::remove_dir_all(path).unwrap();
            })
            .unwrap()
            .join();
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }

    #[test]
    fn app_auth_dispatch_runs_home_startup_for_every_invalidated_installer() {
        // GUI updates reach teardown paths that close the process-global session.
        let _guard = crate::app::session::test_guard();
        // Like the completion fixture, keep large inline GUI states off the
        // default libtest stack without changing the runner configuration.
        let result = std::thread::Builder::new()
            .name("fork-auth-dispatch".into())
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap()
                    .block_on(check_auth_dispatch())
            })
            .unwrap()
            .join();
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }

    async fn check_auth_dispatch() {
        use crate::app::view::ConnectAccountMessage;
        for auth in [
            ConnectAccountMessage::LogOut,
            ConnectAccountMessage::SetSession(
                LoginResponse {
                    requires_2fa: false,
                    token: "synthetic".into(),
                    refresh_token: "synthetic".into(),
                    user: User {
                        id: 0,
                        email: "synthetic@example.invalid".into(),
                        email_verified: None,
                    },
                },
                0,
            ),
        ] {
            let root_path =
                std::env::temp_dir().join(format!("fork-auth-dispatch-{}", uuid::Uuid::new_v4()));
            let root = CoincubeDirectory::new(root_path.clone());
            root.network_directory(ChainId::BitcoinBlake2b)
                .init()
                .unwrap();
            // A sign-in must be accepted by its Home origin before GUI
            // broadcasts it; the logout case still exercises a stale App
            // message after the old fork App was replaced by Home.
            let (home, _) = home::Home::new_for_chain(root.clone(), Some(ChainId::BitcoinBlake2b));
            let mut pane = pane::Pane::new_with_tab(tab::State::Home(home));
            pane.tabs.push(tab::Tab::new(2, installer_state(&root)));
            pane.tabs.push(tab::Tab::new(3, installer_state(&root)));
            let (mut panes, source) = pane_grid::State::new(pane);
            panes
                .split(
                    pane_grid::Axis::Vertical,
                    source,
                    pane::Pane::new_with_tab(installer_state(&root)),
                )
                .unwrap();
            let mut gui = GUI {
                panes,
                focus: Some(source),
                config: Config::new(root.clone(), None),
                window_id: None,
                window_init: None,
                window_config: None,
                global_cache: GlobalCache::default(),
                theme_mode: Default::default(),
                system_theme: None,
                system_reduces_motion: None,
                preferences_sheet: None,
                connect_sheet: None,
                preferences: unresolved_test_preferences(),
                zoom: ZoomState::default(),
                worldview_sharing: worldview::Sharing::default(),
            };
            let account_message = if matches!(&auth, ConnectAccountMessage::SetSession(..)) {
                tab::Message::Launch(home::Message::View(home::ViewMessage::ConnectAccount(auth)))
            } else {
                tab::Message::Run(AppMessage::View(crate::app::view::Message::ConnectAccount(
                    auth,
                )))
            };
            let task = gui.update(Message::Pane(
                source,
                pane::Message::tab(1, account_message),
            ));
            let mut checked = HashSet::new();
            if let Some(mut stream) = iced_runtime::task::into_stream(task) {
                while let Some(action) = stream.next().await {
                    if let iced_runtime::Action::Output(message) = action {
                        if !matches!(&message, Message::Pane(_, pane::Message::Tab(_, inner))
                            if matches!(inner.as_ref(), tab::Message::Launch(home::Message::Checked { .. })))
                        {
                            continue;
                        }
                        if let Message::Pane(pane_id, pane::Message::Tab(tab_id, _)) = &message {
                            checked.insert((*pane_id, *tab_id));
                        }
                        // Apply the real directory-probe completion through the
                        // same GUI seam. Do not execute emitted account Init.
                        let _ = gui.update(message);
                    }
                }
            }
            assert_eq!(
                checked.len(),
                3,
                "every invalidated installer must receive its startup result"
            );
            for (pane_id, tab_id) in checked {
                let tab = gui
                    .panes
                    .get(pane_id)
                    .unwrap()
                    .tabs
                    .iter()
                    .find(|tab| tab.id == tab_id)
                    .unwrap();
                assert!(matches!(&tab.state, tab::State::Home(home) if home.is_checked_for_test()));
            }
            // The focused Launcher dispatch is intercepted by the Connect
            // sheet, while the invalidated installers above still receive
            // their own startup completions.
            drop(gui.update(Message::Pane(
                source,
                pane::Message::View(pane::ViewMessage::OpenConnectSignIn),
            )));
            let sheet = gui.connect_sheet.as_ref().expect("focused Launcher sheet");
            assert_eq!(sheet.origin.pane, source);
            assert_eq!(sheet.origin.tab_id, 1);
            assert!(matches!(sheet.origin.source, tab::PreferencesSource::Home));
            std::fs::remove_dir_all(root_path).unwrap();
        }
    }
}

#[cfg(test)]
mod fork_auth_revocation_tests {
    use super::*;
    use crate::app::{
        self,
        menu::{Menu, VaultSubMenu},
        view::{self, ConnectAccountMessage},
        wallet::Wallet,
    };
    use crate::chain::ChainId;
    use std::sync::Arc;

    async fn outputs<T: Send + 'static>(task: Task<T>) -> Vec<T> {
        use iced::futures::StreamExt;
        match iced_runtime::task::into_stream(task) {
            Some(stream) => {
                stream
                    .filter_map(|action| async move {
                        match action {
                            iced_runtime::Action::Output(msg) => Some(msg),
                            _ => None,
                        }
                    })
                    .collect()
                    .await
            }
            None => Vec::new(),
        }
    }

    #[test]
    fn gui_auth_replacement_drops_open_signer_and_rejects_pending_signatures() {
        // A full GUI + Vault App state in a debug frame exceeds libtest's
        // default thread stack (the same shape as `fork_completion_tests` and
        // `fork_auth_dispatch_tests`), so run the body on a thread with the
        // 8 MiB the real main thread has, without changing the runner.
        let result = std::thread::Builder::new()
            .name("fork-auth-revocation".into())
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap()
                    .block_on(check_auth_replacement())
            })
            .unwrap()
            .join();
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }

    #[allow(clippy::await_holding_lock)] // serialize synthetic process-global PIN sessions
    async fn check_auth_replacement() {
        let _guard = app::session::test_guard();
        for replace in [false, true] {
            let root_path =
                std::env::temp_dir().join(format!("fork-auth-revoke-{}", uuid::Uuid::new_v4()));
            let root = CoincubeDirectory::new(root_path.clone());
            let chain = ChainId::BitcoinBlake2b;
            let fixture = app::state::vault::test_support::unified::fixture();
            let wallet = Arc::new(
                Wallet::new(fixture.descriptor.clone())
                    .with_chain(chain)
                    .with_signer(crate::signer::Signer::new(
                        app::state::vault::test_support::unified::signer(21),
                    )),
            );
            let weak_wallet = Arc::downgrade(&wallet);
            let weak_signer = Arc::downgrade(wallet.signer.as_ref().unwrap());
            let mut client = crate::services::coincube::CoincubeClient::new();
            client.base_url = "http://127.0.0.1:9".into();
            client.set_token("synthetic-login");
            // Build the daemon config as a TOML value tree rather than a
            // formatted document: the unified fixture descriptor carries
            // hardened-derivation apostrophes (`48'/0'`), which a TOML literal
            // string cannot contain, and a value tree needs no quoting rule.
            let table = |entries: &[(&str, String)]| {
                toml::Value::Table(
                    entries
                        .iter()
                        .map(|(k, v)| (k.to_string(), toml::Value::String(v.clone())))
                        .collect(),
                )
            };
            let cfg: coincubed::config::Config = toml::Value::Table(
                vec![
                    (
                        "main_descriptor".to_string(),
                        toml::Value::String(fixture.descriptor.to_string()),
                    ),
                    (
                        "data_directory".to_string(),
                        toml::Value::String(root_path.display().to_string()),
                    ),
                    (
                        "bitcoin_config".to_string(),
                        table(&[("network", chain.api_str().to_string())]),
                    ),
                    (
                        "esplora_config".to_string(),
                        table(&[(
                            "addr",
                            format!("{}/api/v1/esplora/bitcoin-blake2b/mainnet", client.base_url),
                        )]),
                    ),
                ]
                .into_iter()
                .collect(),
            )
            .try_into()
            .unwrap();
            // GUI-only fixture: no running daemon, HTTP, native keystore or SDK.
            let daemon = Arc::new(crate::daemon::embedded::EmbeddedDaemon::unstarted_for_test(
                cfg, None,
            ));
            let cube = app::settings::CubeSettings::new("fixture".into(), chain);
            let cube_id = cube.id.clone();
            let (mut app, startup) = app::App::new_for_chain(
                app::cache::Cache {
                    fiat_chain: chain,
                    network: bitcoin::Network::Bitcoin,
                    ..Default::default()
                },
                wallet,
                None,
                client,
                app::Config::new(false),
                daemon,
                root.clone(),
                cube,
            )
            .unwrap();
            drop(startup);
            drop(app.update(AppMessage::View(view::Message::Menu(Menu::Vault(
                VaultSubMenu::PSBTs(None),
            )))));
            let tx = crate::daemon::model::SpendTx::new(
                None,
                fixture.psbt,
                Vec::new(),
                &fixture.descriptor,
                &bitcoin::secp256k1::Secp256k1::new(),
                bitcoin::Network::Bitcoin,
            );
            drop(app.update(AppMessage::SpendTxs(Ok(vec![tx]))));
            drop(app.update(AppMessage::View(view::Message::Select(0))));
            drop(app.update(AppMessage::View(view::Message::Spend(
                view::SpendTxMessage::Sign,
            ))));
            let pane = pane::Pane::new_with_tab(tab::State::App(app));
            let (panes, pane_id) = pane_grid::State::new(pane);
            let mut gui = GUI {
                panes,
                focus: Some(pane_id),
                config: Config::new(root, None),
                window_id: None,
                window_init: None,
                window_config: None,
                global_cache: GlobalCache::default(),
                theme_mode: Default::default(),
                system_theme: None,
                system_reduces_motion: None,
                preferences_sheet: None,
                connect_sheet: None,
                preferences: unresolved_test_preferences(),
                zoom: ZoomState::default(),
                worldview_sharing: worldview::Sharing::default(),
            };
            let (home, home_startup) =
                home::Home::new_for_chain(gui.config.coincube_directory.clone(), None);
            drop(home_startup);
            gui.panes
                .get_mut(pane_id)
                .unwrap()
                .tabs
                .push(tab::Tab::new(2, tab::State::Home(home)));
            drop(gui.update(Message::Pane(
                pane_id,
                pane::Message::tab(
                    2,
                    tab::Message::Launch(home::Message::View(home::ViewMessage::ConnectAccount(
                        ConnectAccountMessage::Init,
                    ))),
                ),
            )));
            assert!(matches!(
                gui.panes.get(pane_id).unwrap().tabs[0].state,
                tab::State::App(_)
            ));
            let tab::State::Home(home) = &gui.panes.get(pane_id).unwrap().tabs[1].state else {
                unreachable!()
            };
            assert_eq!(home.connect_account.client.token(), Some("synthetic-login"));
            // The sibling above adopted the App's bearer and intentionally
            // refuses an unsolicited SetSession. Use a fresh Home panel as
            // the origin of the replacement login under test.
            let (fresh_home, startup) =
                home::Home::new_for_chain(gui.config.coincube_directory.clone(), None);
            drop(startup);
            gui.panes.get_mut(pane_id).unwrap().tabs[1].state = tab::State::Home(fresh_home);
            app::session::open(cube_id.clone(), zeroize::Zeroizing::new("2468".into()));
            let select = || {
                tab::Message::Run(AppMessage::View(view::Message::Spend(
                    view::SpendTxMessage::SelectMasterSigner,
                )))
            };
            let tab = &mut gui.panes.get_mut(pane_id).unwrap().tabs[0];
            let signed = outputs(tab.update(select())).await;
            assert!(signed.iter().any(|msg| matches!(msg,
                tab::Message::ForkAsync(_, inner) if matches!(inner.as_ref(), tab::Message::Run(AppMessage::Signed(_, Ok(_)))))));
            let pending = tab.update(select());
            let auth_epoch = match &gui.panes.get(pane_id).unwrap().tabs[1].state {
                tab::State::Home(home) => home.connect_account.auth_epoch(),
                _ => unreachable!(),
            };
            let auth = if replace {
                ConnectAccountMessage::SetSession(
                    serde_json::from_value(serde_json::json!({
                        "requires_2fa":false,"token":"replacement","refresh_token":"replacement-refresh",
                        "user":{"id":8,"email":"fixture@example.invalid","email_verified":true}
                    }))
                    .unwrap(),
                    auth_epoch,
                )
            } else {
                ConnectAccountMessage::LogOut
            };
            // Actual GUI auth broadcaster, with the signing picker still open.
            drop(gui.update(Message::Pane(
                pane_id,
                pane::Message::tab(
                    2,
                    tab::Message::Launch(home::Message::View(home::ViewMessage::ConnectAccount(
                        auth,
                    ))),
                ),
            )));
            let tab = &mut gui.panes.get_mut(pane_id).unwrap().tabs[0];
            assert!(
                matches!(tab.state, tab::State::Home(_)),
                "replace={}",
                replace
            );
            assert!(outputs(pending).await.is_empty());
            if replace {
                let tab::State::Home(home) = &tab.state else {
                    unreachable!()
                };
                assert_eq!(
                    home.connect_account.client.token(),
                    Some("replacement"),
                    "the replacement Home must receive the completed login"
                );
            }
            assert!(tab.wallet().is_none());
            assert!(app::session::pin_for(&cube_id).is_none());
            assert!(weak_wallet.upgrade().is_none());
            assert!(weak_signer.upgrade().is_none());
            assert!(outputs(tab.update(select())).await.is_empty());
            for stale in signed {
                assert!(outputs(tab.update(stale)).await.is_empty());
            }
            if root_path.exists() {
                std::fs::remove_dir_all(root_path).unwrap();
            }
        }
        app::session::close();
    }
}

#[cfg(test)]
mod shared_adoption_generation_tests {
    use super::*;
    use crate::{app, loader};
    use iced::futures::StreamExt;
    use std::sync::Arc;

    async fn first_output(task: Task<Message>) -> Message {
        let mut stream =
            iced_runtime::task::into_stream(task).expect("a real shared-client request");
        while let Some(action) = stream.next().await {
            if let iced_runtime::Action::Output(message) = action {
                return message;
            }
        }
        panic!("shared-client Init produced no account result");
    }

    #[test]
    fn held_shared_client_success_or_auth_refusal_cannot_cross_same_tab_reopen() {
        std::thread::Builder::new()
            .name("shared-app-generation".into())
            .stack_size(16 * 1024 * 1024)
            .spawn(run_held_shared_client_reopen)
            .unwrap()
            .join()
            .unwrap();
    }

    fn run_held_shared_client_reopen() {
        let _guard = app::session::test_guard();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _enter = runtime.enter();
        for old_status in [200, 401] {
            let root_path = std::env::temp_dir().join(format!(
                "shared-app-generation-{old_status}-{}",
                uuid::Uuid::new_v4()
            ));
            let root = CoincubeDirectory::new(root_path.clone());
            let old_server = httpmock::MockServer::start();
            let old_request = old_server.mock(|when, then| {
                when.method(httpmock::Method::GET)
                    .path("/api/v1/user")
                    .header("authorization", "Bearer old-session");
                then.status(old_status).json_body(serde_json::json!({
                    "id":11,"email":"old@example.invalid","email_verified":true
                }));
            });
            let current_server = httpmock::MockServer::start();
            let current_request = current_server.mock(|when, then| {
                when.method(httpmock::Method::GET)
                    .path("/api/v1/user")
                    .header("authorization", "Bearer current-session");
                then.status(200).json_body(serde_json::json!({
                    "id":22,"email":"current@example.invalid","email_verified":true
                }));
            });
            let client = |url: String, token: &str| {
                let mut client = crate::services::coincube::CoincubeClient::for_test(url);
                client.set_token(token);
                client
            };
            let (source, wallet) = app::shell_tests::bitcoin_app(&root_path);
            let cache = source.cache().clone();
            let settings = source.cube_settings().clone();
            let daemon = app::shell_tests::fixture_daemon(&source);
            let (mut home, startup) = home::Home::new_for_chain(root.clone(), None);
            drop(startup);
            drop(home.adopt_connect_client(client(old_server.base_url(), "old-session")));
            let mut pane = pane::Pane::new_with_tab(tab::State::App(source));
            pane.tabs.push(tab::Tab::new(2, tab::State::Home(home)));
            pane.focused_tab = 0;
            let (panes, pane_id) = pane_grid::State::new(pane);
            let mut gui = GUI {
                panes,
                focus: Some(pane_id),
                config: Config::new(root.clone(), None),
                window_id: None,
                window_init: None,
                window_config: None,
                global_cache: GlobalCache::default(),
                theme_mode: Default::default(),
                system_theme: None,
                system_reduces_motion: None,
                preferences_sheet: None,
                connect_sheet: None,
                preferences: unresolved_test_preferences(),
                zoom: ZoomState::default(),
                worldview_sharing: worldview::Sharing::default(),
            };
            let init = || {
                Message::Pane(
                    pane_id,
                    pane::Message::tab(
                        1,
                        tab::Message::Run(AppMessage::View(
                            crate::app::view::Message::ConnectAccount(
                                crate::app::view::ConnectAccountMessage::Init,
                            ),
                        )),
                    ),
                )
            };
            let old_generation = gui.panes.get(pane_id).unwrap().tabs[0]
                .app_generation_token()
                .unwrap();
            // FocusTab's deferred Init is produced by Pane, before the App
            // can start any account work. Hold this actual Pane output across
            // the same-ID Lock/reopen transaction as well.
            let held_focus = runtime.block_on(first_output(gui.update(Message::Pane(
                pane_id,
                pane::Message::View(pane::ViewMessage::FocusTab(0)),
            ))));
            let Message::Pane(_, pane::Message::Tab(1, focus_inner)) = &held_focus else {
                panic!("FocusTab did not return the originating App task");
            };
            let tab::Message::AppAsync(focus_generation, focus_result) = focus_inner.as_ref()
            else {
                panic!("FocusTab task was not generation guarded");
            };
            assert_eq!(*focus_generation, old_generation);
            assert!(matches!(
                focus_result.as_ref(),
                tab::Message::Run(AppMessage::View(crate::app::view::Message::ConnectAccount(
                    crate::app::view::ConnectAccountMessage::Init
                )))
            ));
            let old_result = runtime.block_on(first_output(gui.update(init())));
            old_request.assert();
            let Message::Pane(_, pane::Message::Tab(1, old_inner)) = &old_result else {
                panic!("the actual adoption task must route to the originating tab");
            };
            let tab::Message::AppAsync(generation, result) = old_inner.as_ref() else {
                panic!("the actual adoption task must capture its App generation");
            };
            assert_eq!(*generation, old_generation);
            assert!(
                matches!(result.as_ref(), tab::Message::Run(AppMessage::View(
                crate::app::view::Message::ConnectAccount(
                    crate::app::view::ConnectAccountMessage::AdmittedUserLoaded { user, .. }
                )
            )) if (old_status == 200 && user.as_ref().is_ok_and(|u| u.id == 11))
                || (old_status == 401 && user.as_ref().is_err_and(|(_, auth)| *auth)))
            );

            // The ordinary same-tab lock invalidates the old App. A synthetic
            // successful Loader completion admits a new App without PIN,
            // keyring, node, or payment work in this disposable fixture.
            drop(gui.update(Message::Pane(
                pane_id,
                pane::Message::tab(1, tab::Message::LockCube),
            )));
            assert!(matches!(
                gui.panes.get(pane_id).unwrap().tabs[0].state,
                tab::State::Home(_)
            ));
            let breez = Arc::new(app::breez_liquid::BreezClient::disconnected(
                settings.network.bitcoin_network(),
            ));
            let (loader, startup) = loader::Loader::new(
                root.clone(),
                app::Config::new(false),
                settings.network.bitcoin_network(),
                None,
                None,
                None,
                settings.clone(),
                Some(breez.clone()),
                None,
            );
            drop(startup);
            {
                let pane = gui.panes.get_mut(pane_id).unwrap();
                let tab::State::Home(home) = &mut pane.tabs[1].state else {
                    unreachable!()
                };
                home.connect_account
                    .install_shared_client(client(current_server.base_url(), "current-session"));
                pane.tabs[0].state = tab::State::Loader(loader);
            }
            drop(gui.update(Message::Pane(
                pane_id,
                pane::Message::tab(
                    1,
                    tab::Message::Load(loader::Message::BreezLoaded {
                        breez,
                        spark_backend: None,
                        cache,
                        wallet,
                        config: app::Config::new(false),
                        daemon,
                        datadir: root.clone(),
                        bitcoind: None,
                        cube_settings: settings,
                    }),
                ),
            )));
            let current_generation = gui.panes.get(pane_id).unwrap().tabs[0]
                .app_generation_token()
                .unwrap();
            assert_ne!(old_generation, current_generation);
            let current_result = runtime.block_on(first_output(gui.update(init())));
            current_request.assert();
            assert!(matches!(&current_result,
                Message::Pane(_, pane::Message::Tab(1, inner))
                    if matches!(inner.as_ref(), tab::Message::AppAsync(generation, _)
                        if *generation == current_generation)));
            drop(gui.update(current_result));
            assert_eq!(gui.reviewer_app_mut(0).connect_user_id_for_test(), Some(22));
            let menu = gui.reviewer_app_mut(0).current_menu_for_test().clone();
            assert_eq!(gui.update(old_result).units(), 0);
            let app = gui.reviewer_app_mut(0);
            assert_eq!(app.connect_user_id_for_test(), Some(22));
            assert_eq!(app.current_menu_for_test(), &menu);
            assert_eq!(gui.update(held_focus).units(), 0);
            assert_eq!(gui.reviewer_app_mut(0).connect_user_id_for_test(), Some(22));
            drop(gui);
            std::fs::remove_dir_all(root_path).ok();
        }
    }
}

#[cfg(test)]
mod shell_escape_dispatch_tests {
    use super::*;
    use crate::app::{self, view};

    fn escape_event(repeat: bool) -> Event {
        let key = keyboard::Key::Named(keyboard::key::Named::Escape);
        Event::Keyboard(keyboard::Event::KeyPressed {
            key: key.clone(),
            modified_key: key,
            physical_key: keyboard::key::Physical::Code(keyboard::key::Code::Escape),
            location: keyboard::Location::Standard,
            modifiers: keyboard::Modifiers::empty(),
            text: None,
            repeat,
        })
    }

    #[test]
    fn only_ignored_nonrepeating_escape_reaches_the_shell_listener() {
        assert_eq!(
            ignored_shell_key(&escape_event(false), event::Status::Ignored),
            Some(Key::Escape)
        );
        assert_eq!(
            ignored_shell_key(&escape_event(false), event::Status::Captured),
            None
        );
        assert_eq!(
            ignored_shell_key(&escape_event(true), event::Status::Ignored),
            None
        );
    }

    #[test]
    fn signed_in_cube_connect_entry_returns_to_same_cube() {
        std::thread::Builder::new()
            .name("gui-connect-cube-entry".into())
            .stack_size(16 * 1024 * 1024)
            .spawn(|| {
                let _guard = app::session::test_guard();
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let _enter = runtime.enter();
                let path = std::env::temp_dir()
                    .join(format!("cube-connect-sheet-{}", uuid::Uuid::new_v4()));
                std::fs::create_dir_all(&path).unwrap();
                let (mut app, _) = app::shell_tests::bitcoin_app(&path);
                app::shell_tests::signed_in_connect_for_shell(&mut app);
                let (panes, pane_id) =
                    pane_grid::State::new(pane::Pane::new_with_tab(tab::State::App(app)));
                let mut gui = GUI {
                    panes,
                    focus: Some(pane_id),
                    config: Config::new(CoincubeDirectory::new(path.clone()), None),
                    window_id: None,
                    window_init: None,
                    window_config: None,
                    global_cache: GlobalCache::default(),
                    theme_mode: Default::default(),
                    system_theme: None,
                    system_reduces_motion: None,
                    preferences_sheet: None,
                    connect_sheet: None,
                    preferences: unresolved_test_preferences(),
                    zoom: ZoomState::default(),
                    worldview_sharing: worldview::Sharing::default(),
                };
                let tab_id = gui.panes.get(pane_id).unwrap().tabs[0].id;
                drop(gui.view());
                drop(gui.update(Message::Pane(
                    pane_id,
                    pane::Message::tab(
                        tab_id,
                        tab::Message::Run(AppMessage::View(view::Message::OpenConnectSignIn)),
                    ),
                )));
                let sheet = gui.connect_sheet.as_ref().expect("Connect sheet opened");
                assert_eq!(sheet.origin.tab_id, tab_id);
                assert_eq!(sheet.origin.pane, pane_id);
                assert_eq!(
                    sheet.page,
                    ConnectSheetPage::Sections(ConnectSheetSection::Account)
                );
                drop(gui.view());
                drop(gui.update(Message::ConnectSheet(ConnectSheetMessage::Close)));
                assert!(gui.connect_sheet.is_none());
                assert!(matches!(
                    &gui.panes.get(pane_id).unwrap().tabs[0].state,
                    tab::State::App(_)
                ));
                assert_eq!(gui.panes.get(pane_id).unwrap().tabs[0].id, tab_id);
                drop(gui);
                std::fs::remove_dir_all(path).ok();
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn real_gui_escape_dismisses_only_the_focused_app_shell() {
        std::thread::Builder::new()
            .name("gui-shell-escape".into())
            .stack_size(16 * 1024 * 1024)
            .spawn(run_gui_shell_escape)
            .unwrap()
            .join()
            .unwrap();
    }

    fn run_gui_shell_escape() {
        let _guard = app::session::test_guard();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _enter = runtime.enter();
        let path = std::env::temp_dir().join(format!("cube-escape-{}", uuid::Uuid::new_v4()));
        let root = CoincubeDirectory::new(path.clone());
        let (source, _) = app::shell_tests::bitcoin_app(&path);
        let cube_id = source.cube_settings().id.clone();
        let owner = source.cache().app_generation;
        app::session::open_without_pin(cube_id.clone());
        let (home, startup) = home::Home::new_for_chain(root.clone(), None);
        drop(startup);
        let mut pane = pane::Pane::new_with_tab(tab::State::App(source));
        pane.tabs.push(tab::Tab::new(2, tab::State::Home(home)));
        pane.tabs[1].set_copy_owner(owner);
        pane.focused_tab = 0;
        let (panes, pane_id) = pane_grid::State::new(pane);
        let mut gui = GUI {
            panes,
            focus: Some(pane_id),
            config: Config::new(root, None),
            window_id: None,
            window_init: None,
            window_config: None,
            global_cache: GlobalCache::default(),
            theme_mode: Default::default(),
            system_theme: None,
            system_reduces_motion: None,
            preferences_sheet: None,
            connect_sheet: None,
            preferences: unresolved_test_preferences(),
            zoom: ZoomState::default(),
            worldview_sharing: worldview::Sharing::default(),
        };
        let route = |intent| {
            Message::Pane(
                pane_id,
                pane::Message::tab(1, tab::Message::Run(AppMessage::View(intent))),
            )
        };
        for intent in [
            view::Message::ToggleCubeSwitcher,
            view::Message::AskCubeLock,
        ] {
            drop(gui.update(route(intent)));
        }
        let menu = gui.reviewer_app_mut(0).current_menu_for_test().clone();
        let shell = |gui: &mut GUI| {
            let cache = gui.reviewer_app_mut(0).cache();
            (cache.cube_lock_confirmation, cache.cube_switcher_open)
        };
        assert_eq!(shell(&mut gui), (true, true));
        for expected in [(false, true), (false, false)] {
            let focus = gui.update(Message::KeyPressed(Key::Escape));
            assert!(
                focus.units() > 0,
                "dismissal returns its trigger focus task"
            );
            drop(focus);
            assert_eq!(shell(&mut gui), expected);
            assert_eq!(gui.reviewer_app_mut(0).current_menu_for_test(), &menu);
            assert_eq!(
                app::session::current_cube_id().as_deref(),
                Some(cube_id.as_str())
            );
            assert_eq!(
                gui.panes.get(pane_id).unwrap().tabs[1].copy_owner(),
                Some(owner)
            );
        }
        assert_eq!(gui.update(Message::KeyPressed(Key::Escape)).units(), 0);
        {
            let pane = gui.panes.get_mut(pane_id).unwrap();
            let tab::State::App(app) = &mut pane.tabs[0].state else {
                unreachable!()
            };
            app.cache_mut().cube_switcher_open = true;
            pane.focused_tab = 1;
        }
        assert_eq!(gui.update(Message::KeyPressed(Key::Escape)).units(), 0);
        assert!(gui.reviewer_app_mut(0).cache().cube_switcher_open);
        assert_eq!(
            app::session::current_cube_id().as_deref(),
            Some(cube_id.as_str())
        );
        app::session::close();
        drop(gui);
        std::fs::remove_dir_all(path).ok();
    }
}

#[cfg(test)]
mod lock_cube_dispatch_tests {
    use super::*;

    #[test]
    fn admitted_destination_closes_orphaned_process_session() {
        let _guard = crate::app::session::test_guard();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _enter = runtime.enter();
        let path =
            std::env::temp_dir().join(format!("cube-orphaned-session-{}", uuid::Uuid::new_v4()));
        let root = CoincubeDirectory::new(path.clone());
        let (home, startup) = home::Home::new_for_chain(root.clone(), None);
        drop(startup);
        let pane = pane::Pane::new_with_tab(tab::State::Home(home));
        let (panes, pane_id) = pane_grid::State::new(pane);
        let mut gui = GUI {
            panes,
            focus: Some(pane_id),
            config: Config::new(root.clone(), None),
            window_id: None,
            window_init: None,
            window_config: None,
            global_cache: GlobalCache::default(),
            theme_mode: Default::default(),
            system_theme: None,
            system_reduces_motion: None,
            preferences_sheet: None,
            connect_sheet: None,
            preferences: unresolved_test_preferences(),
            zoom: ZoomState::default(),
            worldview_sharing: worldview::Sharing::default(),
        };
        crate::app::session::open("orphaned", zeroize::Zeroizing::new("2468".into()));
        let destination = crate::app::settings::CubeSettings::new_with_raw_id(
            "next-cube".into(),
            "Next Cube".into(),
            crate::chain::ChainId::Bitcoin,
        );
        drop(gui.update(Message::Pane(
            pane_id,
            pane::Message::tab(
                1,
                tab::Message::Launch(home::Message::Run(
                    root,
                    crate::app::Config::new(false),
                    crate::chain::ChainId::Bitcoin,
                    destination,
                )),
            ),
        )));
        assert!(!crate::app::session::is_open());
        drop(gui);
        std::fs::remove_dir_all(path).ok();
    }

    #[test]
    fn refused_public_destination_keeps_the_current_app_and_session() {
        let _guard = crate::app::session::test_guard();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _enter = runtime.enter();
        let path =
            std::env::temp_dir().join(format!("cube-refused-switch-{}", uuid::Uuid::new_v4()));
        let root = CoincubeDirectory::new(path.clone());
        let (source, _) = crate::app::shell_tests::bitcoin_app(&path);
        let source_id = source.cube_settings().id.clone();
        let (home, startup) = home::Home::new_for_chain(root.clone(), None);
        drop(startup);
        let mut gui = GUI::reviewer_app_home_gui(source, home);
        let pane_id = gui.focus.unwrap();
        crate::app::session::open(source_id.clone(), zeroize::Zeroizing::new("2468".into()));

        let mismatched = crate::app::settings::CubeSettings::new_with_raw_id(
            "wrong-chain".into(),
            "Wrong chain".into(),
            crate::chain::ChainId::BitcoinBlake2b,
        );
        drop(gui.update(Message::Pane(
            pane_id,
            pane::Message::tab(
                2,
                tab::Message::Launch(home::Message::Run(
                    root.clone(),
                    crate::app::Config::new(false),
                    crate::chain::ChainId::Bitcoin,
                    mismatched,
                )),
            ),
        )));
        assert!(matches!(
            &gui.panes.get(pane_id).unwrap().tabs[0].state,
            tab::State::App(app) if app.cube_settings().id == source_id
        ));
        assert_eq!(
            crate::app::session::current_cube_id(),
            Some(source_id.clone())
        );
        let tab::State::Home(home) = &gui.panes.get(pane_id).unwrap().tabs[1].state else {
            panic!("the Launcher must report the refusal");
        };
        assert!(home.error().is_some_and(|error| error.contains("folder")));
        // A delayed floor result carries its original generation even after
        // Home has cleared the floor selection. Admission must reject it
        // before the public App and its process session are retired.
        let stale_floor_cube = crate::app::settings::CubeSettings::new_with_raw_id(
            "stale-floor".into(),
            "Stale floor".into(),
            crate::chain::ChainId::Bitcoin,
        );
        drop(gui.update(Message::Pane(
            pane_id,
            pane::Message::tab(
                2,
                tab::Message::Launch(home::Message::RunFromFloor(
                    root,
                    crate::app::Config::new(false),
                    crate::chain::ChainId::Bitcoin,
                    stale_floor_cube,
                    1,
                )),
            ),
        )));
        assert!(matches!(
            &gui.panes.get(pane_id).unwrap().tabs[0].state,
            tab::State::App(app) if app.cube_settings().id == source_id
        ));
        assert_eq!(crate::app::session::current_cube_id(), Some(source_id));
        let tab::State::Home(home) = &gui.panes.get(pane_id).unwrap().tabs[1].state else {
            panic!("a stale floor result must remain in the Launcher");
        };
        assert!(home
            .error()
            .is_some_and(|error| error.contains("changed while opening")));
        drop(gui);
        crate::app::session::close();
        std::fs::remove_dir_all(path).ok();
    }

    #[test]
    fn lock_retires_every_secret_owner_before_clearing_the_shared_session() {
        let _guard = crate::app::session::test_guard();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _enter = runtime.enter();
        let root_path =
            std::env::temp_dir().join(format!("cube-lock-siblings-{}", uuid::Uuid::new_v4()));
        let root = CoincubeDirectory::new(root_path.clone());
        let (first, _) = crate::app::shell_tests::bitcoin_app(&root_path);
        let same_cube_id = first.cube_settings().id.clone();
        let same_settings = first.cube_settings().clone();
        let sibling = crate::app::shell_tests::bitcoin_app_with_id(&root_path, &same_cube_id);
        let same_pane_sibling =
            crate::app::shell_tests::bitcoin_app_with_id(&root_path, &same_cube_id);
        let (unrelated, _) = crate::app::shell_tests::bitcoin_app(&root_path);
        let mut first_pane = pane::Pane::new_with_tab(tab::State::App(first));
        first_pane
            .tabs
            .push(tab::Tab::new(2, tab::State::App(unrelated)));
        first_pane
            .tabs
            .push(tab::Tab::new(3, tab::State::App(same_pane_sibling)));
        let (loading, loading_task) = crate::loader::Loader::new(
            root.clone(),
            crate::app::Config::new(false),
            same_settings.network.bitcoin_network(),
            None,
            None,
            None,
            same_settings.clone(),
            None,
            None,
        );
        drop(loading_task);
        first_pane
            .tabs
            .push(tab::Tab::new(4, tab::State::Loader(loading)));
        let (installing, install_task) = crate::installer::Installer::try_new_for_chain(
            root.clone(),
            same_settings.network,
            crate::installer::UserFlow::CreateWallet,
            true,
            Some(same_settings),
            None,
            None,
            false,
            None,
        )
        .unwrap();
        drop(install_task);
        first_pane.tabs.push(tab::Tab::new(
            5,
            tab::State::Installer(Box::new(installing)),
        ));
        let (mut panes, origin) = pane_grid::State::new(first_pane);
        let (other, _) = panes
            .split(
                pane_grid::Axis::Vertical,
                origin,
                pane::Pane::new_with_tab(tab::State::App(sibling)),
            )
            .unwrap();
        let mut gui = GUI {
            panes,
            focus: Some(origin),
            config: Config::new(root, None),
            window_id: None,
            window_init: None,
            window_config: None,
            global_cache: GlobalCache::default(),
            theme_mode: Default::default(),
            system_theme: None,
            system_reduces_motion: None,
            preferences_sheet: None,
            connect_sheet: None,
            preferences: unresolved_test_preferences(),
            zoom: ZoomState::default(),
            worldview_sharing: worldview::Sharing::default(),
        };
        crate::app::session::open(same_cube_id.clone(), zeroize::Zeroizing::new("2468".into()));
        assert_eq!(crate::app::session::current_cube_id(), Some(same_cube_id));

        drop(gui.update(Message::Pane(
            origin,
            pane::Message::tab(1, tab::Message::LockCubeToLauncher),
        )));
        let pane = gui.panes.get(origin).expect("origin remains on Launcher");
        assert!(matches!(pane.tabs[0].state, tab::State::Home(_)));
        assert_eq!(
            pane.tabs.len(),
            1,
            "no App, Loader or Installer may retain a cleared global session"
        );
        assert!(gui.panes.get(other).is_none(), "empty sibling pane closed");
        assert_eq!(gui.focus, Some(origin));
        assert!(!crate::app::session::is_open());
        drop(gui.update(Message::Pane(
            origin,
            pane::Message::tab(4, tab::Message::Load(crate::loader::Message::None)),
        )));
        assert_eq!(gui.panes.get(origin).unwrap().tabs.len(), 1);

        drop(gui);
        let _ = std::fs::remove_dir_all(root_path);
    }
}

// Gandalf's round-3 reviewer seam (#518 issuecomment-5824706295), adopted:
// a real GUI with two Bitcoin App tabs, its real auth broadcaster and
// originating-tab dispatch. Test-only.
#[cfg(test)]
impl GUI {
    pub(crate) fn reviewer_load_preferences(&mut self) {
        let root = &self.config.coincube_directory;
        let receipt = GlobalSettings::load_or_initialize_preferences(root).unwrap();
        self.preferences = PreferencesOwner::new(GlobalSettings::path(root), &Ok(receipt));
        self.publish_balance_privacy();
        self.publish_appearance();
    }

    pub(crate) fn reviewer_claim_gui(first: crate::app::App, second: crate::app::App) -> Self {
        let root = first.datadir().clone();
        let mut pane = pane::Pane::new_with_tab(tab::State::App(first));
        pane.tabs.push(tab::Tab::new(2, tab::State::App(second)));
        let (panes, pane_id) = pane_grid::State::new(pane);
        Self {
            panes,
            focus: Some(pane_id),
            config: Config::new(root, None),
            window_id: None,
            window_init: None,
            window_config: None,
            global_cache: GlobalCache::default(),
            theme_mode: Default::default(),
            system_theme: None,
            system_reduces_motion: None,
            preferences_sheet: None,
            connect_sheet: None,
            preferences: unresolved_test_preferences(),
            zoom: ZoomState::default(),
            worldview_sharing: worldview::Sharing::default(),
        }
    }
    /// An already-open Bitcoin Cube (tab 1) beside a Home tab (tab 2), in one pane.
    pub(crate) fn reviewer_app_home_gui(app: crate::app::App, home: home::Home) -> Self {
        let root = app.datadir().clone();
        let mut pane = pane::Pane::new_with_tab(tab::State::App(app));
        pane.tabs.push(tab::Tab::new(2, tab::State::Home(home)));
        let (panes, pane_id) = pane_grid::State::new(pane);
        Self {
            panes,
            focus: Some(pane_id),
            config: Config::new(root, None),
            window_id: None,
            window_init: None,
            window_config: None,
            global_cache: GlobalCache::default(),
            theme_mode: Default::default(),
            system_theme: None,
            system_reduces_motion: None,
            preferences_sheet: None,
            connect_sheet: None,
            preferences: unresolved_test_preferences(),
            zoom: ZoomState::default(),
            worldview_sharing: worldview::Sharing::default(),
        }
    }
    /// As [`Self::reviewer_app_home_gui`], with the Cube moved to its own
    /// split pane. Returns the Cube's pane; Home keeps the focused pane.
    pub(crate) fn reviewer_split_app_home_gui(
        app: crate::app::App,
        home: home::Home,
    ) -> (Self, pane_grid::Pane) {
        let mut gui = Self::reviewer_app_home_gui(app, home);
        let home_pane = gui.focus.unwrap();
        let cube_tab = gui.panes.get_mut(home_pane).unwrap().tabs.remove(0);
        let mut cube_pane = pane::Pane::new_with_tab(tab::State::Home(
            home::Home::new_for_chain(gui.config.coincube_directory.clone(), None).0,
        ));
        cube_pane.tabs[0] = cube_tab;
        let (cube_pane_id, _) = gui
            .panes
            .split(pane_grid::Axis::Vertical, home_pane, cube_pane)
            .unwrap();
        (gui, cube_pane_id)
    }
    pub(crate) fn reviewer_app_in(&mut self, pane: pane_grid::Pane) -> &mut crate::app::App {
        let tab::State::App(app) = &mut self.panes.get_mut(pane).unwrap().tabs[0].state else {
            panic!("Bitcoin App closed")
        };
        app
    }
    pub(crate) fn reviewer_account_in(
        &mut self,
        pane: pane_grid::Pane,
        id: usize,
        account: crate::app::view::ConnectAccountMessage,
    ) -> Task<Message> {
        self.update(Message::Pane(
            pane,
            pane::Message::tab(
                id,
                tab::Message::Run(AppMessage::View(crate::app::view::Message::ConnectAccount(
                    account,
                ))),
            ),
        ))
    }
    pub(crate) fn reviewer_home_mut(&mut self, index: usize) -> &mut home::Home {
        let pane = self.panes.get_mut(self.focus.unwrap()).unwrap();
        let tab::State::Home(home) = &mut pane.tabs[index].state else {
            panic!("Home closed")
        };
        home
    }
    pub(crate) fn reviewer_home_account(
        &mut self,
        id: usize,
        account: crate::app::view::ConnectAccountMessage,
    ) -> Task<Message> {
        self.update(Message::Pane(
            self.focus.unwrap(),
            pane::Message::tab(
                id,
                tab::Message::Launch(home::Message::View(home::ViewMessage::ConnectAccount(
                    account,
                ))),
            ),
        ))
    }
    /// Account traffic of any tab plus the Home sign-in bubble and its pane
    /// rebroadcast: the messages that carry a sign-in to other open Cubes.
    pub(crate) fn reviewer_is_session_propagation(message: &Message) -> bool {
        if Self::reviewer_is_auth_commit(message) {
            return true;
        }
        match message {
            Message::Pane(_, pane::Message::View(pane::ViewMessage::ConnectSignedIn)) => true,
            Message::Pane(_, pane::Message::Tab(_, inner)) => matches!(
                Self::reviewer_unwrap_guarded_message(inner),
                tab::Message::Run(AppMessage::View(crate::app::view::Message::ConnectAccount(
                    _
                ))) | tab::Message::Launch(home::Message::View(home::ViewMessage::ConnectAccount(
                    _
                ))) | tab::Message::Launch(home::Message::ConnectSignedInBubble)
                    | tab::Message::ConnectSignedIn
            ),
            _ => false,
        }
    }
    pub(crate) fn reviewer_app_mut(&mut self, index: usize) -> &mut crate::app::App {
        let pane = self.panes.get_mut(self.focus.unwrap()).unwrap();
        let tab::State::App(app) = &mut pane.tabs[index].state else {
            panic!("Bitcoin App closed")
        };
        app
    }
    pub(crate) fn reviewer_theme_toggle(&mut self, tab_id: usize) -> Task<Message> {
        self.update(Message::Pane(
            self.focus.unwrap(),
            pane::Message::tab(
                tab_id,
                tab::Message::Run(AppMessage::View(crate::app::view::Message::ToggleTheme)),
            ),
        ))
    }
    pub(crate) fn reviewer_account(
        &mut self,
        id: usize,
        account: crate::app::view::ConnectAccountMessage,
    ) -> Task<Message> {
        self.update(Message::Pane(
            self.focus.unwrap(),
            pane::Message::tab(
                id,
                tab::Message::Run(AppMessage::View(crate::app::view::Message::ConnectAccount(
                    account,
                ))),
            ),
        ))
    }
    /// Whether a GUI message is a tab's Connect account message of any
    /// kind (a completion, a re-dispatched `LogOut`), so a test can drive
    /// exactly those through the real GUI and leave network bootstrap alone.
    pub(crate) fn reviewer_is_account_message(message: &Message) -> bool {
        matches!(message, Message::Pane(_, pane::Message::Tab(_, inner))
            if matches!(Self::reviewer_unwrap_guarded_message(inner), tab::Message::Run(AppMessage::View(
                crate::app::view::Message::ConnectAccount(_)))))
    }

    #[cfg(test)]
    pub(crate) fn reviewer_is_auth_commit(message: &Message) -> bool {
        matches!(message, Message::Pane(_, pane::Message::Tab(_, inner))
            if matches!(Self::reviewer_unwrap_guarded_message(inner),
                tab::Message::Run(AppMessage::InAppConnectAuthReady { .. }
                    | AppMessage::InAppConnectAuthPersisted { .. }
                    | AppMessage::View(crate::app::view::Message::ConnectAccount(
                        crate::app::view::ConnectAccountMessage::AuthCommitAccepted { .. }
                        | crate::app::view::ConnectAccountMessage::AuthCommitRejected { .. })))))
    }
}

#[cfg(test)]
mod single_open_lifecycle_tests {
    use super::*;
    use crate::{app, chain::ChainId};

    #[test]
    fn accepted_create_import_and_restore_retire_the_old_app_before_installer() {
        let result = std::thread::Builder::new()
            .name("installer-admission-owner".into())
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let _enter = runtime.enter();
                let _guard = app::session::test_guard();
                for flow in [
                    crate::installer::UserFlow::CreateWallet,
                    crate::installer::UserFlow::AddWallet,
                    crate::installer::UserFlow::RestoreFromRecoveryKit { cube_uuid: None },
                ] {
                    let path = std::env::temp_dir()
                        .join(format!("installer-admission-{}", uuid::Uuid::new_v4()));
                    let (source, _) = app::shell_tests::bitcoin_app(&path);
                    let source_id = source.cube_settings().id.clone();
                    let root = source.datadir().clone();
                    let (home, startup) = home::Home::new_for_chain(root.clone(), None);
                    drop(startup);
                    let mut gui = GUI::reviewer_app_home_gui(source, home);
                    let pane_id = gui.focus.unwrap();
                    app::session::open(source_id, zeroize::Zeroizing::new("2468".into()));
                    drop(gui.update(Message::Pane(
                        pane_id,
                        pane::Message::tab(
                            2,
                            tab::Message::Launch(home::Message::Install(
                                root,
                                ChainId::Bitcoin,
                                flow,
                                None,
                            )),
                        ),
                    )));
                    let pane = gui.panes.get(pane_id).unwrap();
                    assert!(pane
                        .tabs
                        .iter()
                        .all(|tab| !matches!(tab.state, tab::State::App(_))));
                    assert!(pane
                        .tabs
                        .iter()
                        .any(|tab| matches!(tab.state, tab::State::Installer(_))));
                    assert!(!app::session::is_open());
                    drop(gui);
                    std::fs::remove_dir_all(path).ok();
                }
            })
            .unwrap()
            .join();
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }

    #[test]
    fn legacy_liana_connect_cube_returns_to_the_launcher_with_its_notice() {
        let result = std::thread::Builder::new()
            .name("remote-login-admission".into())
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let _enter = runtime.enter();
                let _guard = app::session::test_guard();
                let path = std::env::temp_dir()
                    .join(format!("remote-login-admission-{}", uuid::Uuid::new_v4()));
                let (source, wallet) = app::shell_tests::bitcoin_app(&path);
                let source_id = source.cube_settings().id.clone();
                let root = source.datadir().clone();
                let target =
                    app::settings::CubeSettings::new("Remote Cube".into(), ChainId::Bitcoin)
                        .with_vault(app::settings::VaultIdentity::new(
                            wallet.id(),
                            wallet.main_descriptor.as_vault(),
                        ));
                let remote_settings = app::settings::WalletSettings {
                    name: "Remote Vault".into(),
                    alias: None,
                    descriptor_checksum: wallet.descriptor_checksum.clone(),
                    pinned_at: wallet.pinned_at,
                    keys: Vec::new(),
                    hardware_wallets: Vec::new(),
                    remote_backend_auth: Some(app::settings::AuthConfig::new(
                        "test@example.invalid".into(),
                        "remote-wallet".into(),
                    )),
                    start_internal_bitcoind: None,
                    pending_rescan: None,
                    keychain_keys_recorded: false,
                };
                let directory = root.network_directory(ChainId::Bitcoin);
                directory.init().unwrap();
                std::fs::write(
                    directory.path().join(app::settings::SETTINGS_FILE_NAME),
                    serde_json::to_vec(&app::settings::Settings {
                        cubes: vec![target.clone()],
                        wallets: vec![remote_settings.clone()],
                        ..Default::default()
                    })
                    .unwrap(),
                )
                .unwrap();
                let (home, startup) = home::Home::new_for_chain(root.clone(), None);
                drop(startup);
                let mut gui = GUI::reviewer_app_home_gui(source, home);
                let pane_id = gui.focus.unwrap();
                app::session::open(source_id, zeroize::Zeroizing::new("2468".into()));
                drop(gui.update(Message::Pane(
                    pane_id,
                    pane::Message::tab(
                        2,
                        tab::Message::Launch(home::Message::Run(
                            root.clone(),
                            app::Config::new(false),
                            ChainId::Bitcoin,
                            target.clone(),
                        )),
                    ),
                )));
                assert!(!app::session::is_open());
                assert!(matches!(
                    gui.panes.get(pane_id).unwrap().tabs[0].state,
                    tab::State::PinEntry(_)
                ));
                // The public unlock callback is synthetic; the real Tab
                // completion refuses the retired backend from the saved
                // wallet type and says where the wallet still lives.
                drop(gui.update(Message::Pane(
                    pane_id,
                    pane::Message::tab(
                        2,
                        tab::Message::BreezClientLoadedAfterPin {
                            connect_client: None,
                            migration_error: None,
                            enc_key_error: None,
                            breez_client: Err(app::breez_liquid::BreezError::SignerError(
                                "fixture".into(),
                            )),
                            spark_backend: None,
                            config: app::Config::new(false),
                            datadir: root.clone(),
                            network: bitcoin::Network::Bitcoin,
                            cube: target,
                            wallet_settings: Some(remote_settings),
                            internal_bitcoind: None,
                            backup: None,
                        },
                    ),
                )));
                let tab::State::Home(home) = &gui.panes.get(pane_id).unwrap().tabs[0].state else {
                    panic!("a legacy Liana Connect Cube must return to the Launcher");
                };
                let notice = home.error().expect("the retired backend is explained");
                assert!(notice.contains("Liana Connect"));
                assert!(notice.contains("test@example.invalid"));
                drop(gui);
                std::fs::remove_dir_all(path).ok();
            })
            .unwrap()
            .join();
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }

    #[test]
    fn deleting_a_cube_retires_its_live_app_and_process_session_first() {
        let result = std::thread::Builder::new()
            .name("delete-admission-owner".into())
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let _enter = runtime.enter();
                let _guard = app::session::test_guard();
                let path =
                    std::env::temp_dir().join(format!("delete-admission-{}", uuid::Uuid::new_v4()));
                let (source, _) = app::shell_tests::bitcoin_app(&path);
                let cube_id = source.cube_settings().id.clone();
                let (home, startup) = home::Home::new_for_chain(source.datadir().clone(), None);
                drop(startup);
                let mut gui = GUI::reviewer_app_home_gui(source, home);
                let pane_id = gui.focus.unwrap();
                app::session::open(cube_id.clone(), zeroize::Zeroizing::new("2468".into()));
                drop(gui.update(Message::Pane(
                    pane_id,
                    pane::Message::tab(
                        2,
                        tab::Message::Launch(home::Message::View(home::ViewMessage::DeleteCube(
                            home::DeleteCubeMessage::Confirm(cube_id),
                        ))),
                    ),
                )));
                assert!(!app::session::is_open());
                assert!(gui
                    .panes
                    .iter()
                    .flat_map(|(_, pane)| pane.tabs.iter())
                    .all(|tab| !matches!(tab.state, tab::State::App(_))));
                drop(gui);
                std::fs::remove_dir_all(path).ok();
            })
            .unwrap()
            .join();
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }

    #[test]
    fn current_remote_duress_closes_the_retained_home_but_stale_duress_cannot_close_a_new_owner() {
        let result = std::thread::Builder::new()
            .name("remote-duress-owner-barrier".into())
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let _enter = runtime.enter();
                let _guard = app::session::test_guard();
                let path = std::env::temp_dir()
                    .join(format!("remote-duress-owner-{}", uuid::Uuid::new_v4()));
                let (source, _) = app::shell_tests::bitcoin_app(&path);
                let source_id = source.cube_settings().id.clone();
                let (home, startup) = home::Home::new_for_chain(source.datadir().clone(), None);
                drop(startup);
                let mut gui = GUI::reviewer_app_home_gui(source, home);
                let pane_id = gui.focus.unwrap();
                let old_generation = gui.panes.get(pane_id).unwrap().tabs[0]
                    .app_generation_token()
                    .unwrap();
                gui.panes.get_mut(pane_id).unwrap().focused_tab = 1;
                app::session::open(source_id, zeroize::Zeroizing::new("2468".into()));
                drop(gui.update(Message::Pane(
                    pane_id,
                    pane::Message::tab(
                        1,
                        tab::Message::AppAsync(
                            old_generation,
                            Box::new(tab::Message::Run(AppMessage::View(
                                crate::app::view::Message::DuressLockRemote,
                            ))),
                        ),
                    ),
                )));
                let pane = gui.panes.get(pane_id).unwrap();
                assert_eq!(pane.tabs.len(), 1, "the retained Launcher is closed");
                assert!(matches!(pane.tabs[0].state, tab::State::DuressActive(_)));
                assert_eq!(pane.focused_tab, 0);
                assert!(!app::session::is_open());

                let new_path = std::env::temp_dir()
                    .join(format!("remote-duress-new-owner-{}", uuid::Uuid::new_v4()));
                let (new_owner, _) = app::shell_tests::bitcoin_app(&new_path);
                let new_id = new_owner.cube_settings().id.clone();
                // Test-only replacement on the same tab identity models a
                // later admitted owner. The real App→Duress transition already
                // advanced Tab's generation before this stale result arrives.
                gui.panes.get_mut(pane_id).unwrap().tabs[0].state = tab::State::App(new_owner);
                assert_ne!(
                    gui.panes.get(pane_id).unwrap().tabs[0]
                        .app_generation_token()
                        .unwrap(),
                    old_generation
                );
                app::session::open(new_id.clone(), zeroize::Zeroizing::new("2468".into()));
                drop(gui.update(Message::Pane(
                    pane_id,
                    pane::Message::tab(
                        1,
                        tab::Message::AppAsync(
                            old_generation,
                            Box::new(tab::Message::Run(AppMessage::View(
                                crate::app::view::Message::DuressLockRemote,
                            ))),
                        ),
                    ),
                )));
                assert!(matches!(
                    gui.panes.get(pane_id).unwrap().tabs[0].state,
                    tab::State::App(_)
                ));
                assert_eq!(app::session::current_cube_id(), Some(new_id));
                drop(gui);
                app::session::close();
                std::fs::remove_dir_all(path).ok();
                std::fs::remove_dir_all(new_path).ok();
            })
            .unwrap()
            .join();
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }

    #[test]
    fn local_duress_completion_uses_the_same_public_owner_barrier() {
        let result = std::thread::Builder::new()
            .name("local-duress-owner-barrier".into())
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let _enter = runtime.enter();
                let _guard = app::session::test_guard();
                let path = std::env::temp_dir()
                    .join(format!("local-duress-owner-{}", uuid::Uuid::new_v4()));
                let (source, _) = app::shell_tests::bitcoin_app(&path);
                let source_id = source.cube_settings().id.clone();
                let root = source.datadir().clone();
                let (home, startup) = home::Home::new_for_chain(root.clone(), None);
                drop(startup);
                let mut gui = GUI::reviewer_app_home_gui(source, home);
                let pane_id = gui.focus.unwrap();
                gui.panes.get_mut(pane_id).unwrap().focused_tab = 1;
                app::session::open(source_id, zeroize::Zeroizing::new("2468".into()));
                drop(gui.update(Message::Pane(
                    pane_id,
                    pane::Message::tab(
                        1,
                        tab::Message::DuressActivated {
                            datadir: root,
                            network: bitcoin::Network::Bitcoin,
                            queue_pending: false,
                        },
                    ),
                )));
                let pane = gui.panes.get(pane_id).unwrap();
                assert_eq!(pane.tabs.len(), 1);
                assert_eq!(pane.focused_tab, 0);
                assert!(matches!(pane.tabs[0].state, tab::State::DuressActive(_)));
                assert!(!app::session::is_open());
                drop(gui);
                std::fs::remove_dir_all(path).ok();
            })
            .unwrap()
            .join();
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }

    #[test]
    fn launcher_resumes_one_app_and_switch_closes_it_before_unlock() {
        let result = std::thread::Builder::new()
            .name("single-open-lifecycle".into())
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let _enter = runtime.enter();
                let _guard = app::session::test_guard();
                let path = std::env::temp_dir()
                    .join(format!("single-open-lifecycle-{}", uuid::Uuid::new_v4()));
                let (source, _) = app::shell_tests::bitcoin_app(&path);
                let root = source.datadir().clone();
                let source_settings = source.cube_settings().clone();
                let source_id = source_settings.id.clone();
                let generation = source.cache().app_generation;
                let (home, startup) = home::Home::new_for_chain(root.clone(), None);
                drop(startup);
                let mut gui = GUI::reviewer_app_home_gui(source, home);
                let pane_id = gui.focus.unwrap();
                app::session::open(
                    source_id.clone(),
                    zeroize::Zeroizing::new("2468".into()),
                );
                let app_count = |gui: &GUI| {
                    gui.panes
                        .iter()
                        .flat_map(|(_, pane)| pane.tabs.iter())
                        .filter(|tab| matches!(tab.state, tab::State::App(_)))
                        .count()
                };
                assert_eq!(app_count(&gui), 1);

                drop(gui.update(Message::Pane(
                    pane_id,
                    pane::Message::View(pane::ViewMessage::OpenLauncher),
                )));
                assert_eq!(app_count(&gui), 1);
                assert!(matches!(
                    gui.panes.get(pane_id).unwrap().tabs[0].state,
                    tab::State::App(_)
                ));
                assert_eq!(gui.panes.get(pane_id).unwrap().focused_tab, 1);

                drop(gui.update(Message::Pane(
                    pane_id,
                    pane::Message::tab(
                        2,
                        tab::Message::Launch(home::Message::Run(
                            root.clone(),
                            app::Config::new(false),
                            ChainId::Bitcoin,
                            source_settings.clone(),
                        )),
                    ),
                )));
                assert_eq!(app_count(&gui), 1);
                assert_eq!(gui.panes.get(pane_id).unwrap().focused_tab, 0);
                let tab::State::App(resumed) = &gui.panes.get(pane_id).unwrap().tabs[0].state
                else {
                    panic!("same Cube must resume the retained App");
                };
                assert_eq!(resumed.cache().app_generation, generation);

                // The Account Recovery action reuses this exact App without
                // another unlock and dispatches the existing management route.
                drop(gui.update(Message::Pane(
                    pane_id,
                    pane::Message::View(pane::ViewMessage::OpenLauncher),
                )));
                let route = gui.update(Message::Pane(
                    pane_id,
                    pane::Message::tab(
                        2,
                        tab::Message::Launch(home::Message::RunToRecovery(
                            root.clone(),
                            app::Config::new(false),
                            ChainId::Bitcoin,
                            source_settings,
                        )),
                    ),
                ));
                use iced_runtime::futures::futures::StreamExt;
                let recovery_message = runtime.block_on(async {
                    tokio::time::timeout(std::time::Duration::from_secs(2), async {
                        let mut stream = iced_runtime::task::into_stream(route)
                            .expect("Recovery route emits a menu event");
                        while let Some(action) = stream.next().await {
                            if let iced_runtime::Action::Output(message) = action {
                                if matches!(
                                    &message,
                                    Message::Pane(_, pane::Message::Tab(_, inner))
                                        if matches!(GUI::reviewer_unwrap_guarded_message(inner), tab::Message::Run(
                                            AppMessage::View(crate::app::view::Message::Menu(
                                                crate::app::menu::Menu::Cube(
                                                    crate::app::menu::CubeSubMenu::Settings(
                                                        crate::app::menu::CubeSettingsOption::Recovery
                                                    )
                                                )
                                            ))
                                        ))
                                ) {
                                    return Some(message);
                                }
                            }
                        }
                        None
                    })
                    .await
                    .expect("Recovery menu route completed")
                    .expect("Recovery menu event")
                });
                drop(gui.update(recovery_message));
                assert_eq!(app_count(&gui), 1);

                let other = app::settings::CubeSettings::new_with_raw_id(
                    "other".into(),
                    "Other".into(),
                    ChainId::Bitcoin,
                );
                drop(gui.update(Message::Pane(
                    pane_id,
                    pane::Message::tab(
                        2,
                        tab::Message::Launch(home::Message::Run(
                            root,
                            app::Config::new(false),
                            ChainId::Bitcoin,
                            other,
                        )),
                    ),
                )));
                assert_eq!(app_count(&gui), 0, "old App must stop before B unlocks");
                drop(gui);
                std::fs::remove_dir_all(path).ok();
            })
            .unwrap()
            .join();
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }
}
