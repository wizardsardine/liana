pub mod about;
pub mod general;
mod install_stats;
pub mod local_signing;
pub mod recovery_kit;

use std::sync::Arc;

use iced::Task;

use coincube_ui::widget::Element;

use about::AboutSettingsState;
use general::GeneralSettingsState;
use install_stats::InstallStatsState;
use local_signing::LocalSigningState;

use crate::{
    app::{
        cache::Cache,
        menu::Menu,
        message::Message,
        settings::fiat::PriceSetting,
        state::State,
        view::{self},
        wallet::Wallet,
    },
    daemon::Daemon,
};

pub struct SettingsState {
    setting: Option<Box<dyn State>>,
    cube_id: String,
    current_price_setting: PriceSetting,
    current_unit_setting: crate::app::settings::unit::UnitSetting,
    /// Cube Recovery Kit wizard + cached status. Lives on the outer
    /// wrapper rather than `GeneralSettingsState` so `App::update` can
    /// reach it without downcasting through `Box<dyn State>`. The
    /// Recovery-Kit card is rendered inside the General section's
    /// view, which reads this field through a parameter.
    pub recovery_kit: recovery_kit::RecoveryKit,
    /// Cached wallet handed in via [`State::reload`]. Threaded into
    /// the section sub-states' own `reload(daemon, wallet)` so they
    /// can wire up wallet-dependent state on construction —
    /// specifically, the local-signer panel needs the wallet's
    /// fingerprint to enable the Pair button. App-level
    /// `set_current_panel` calls our `reload` before dispatching the
    /// section message so this is populated by the time the section
    /// is constructed.
    wallet: Option<Arc<Wallet>>,
}

impl SettingsState {
    pub fn new(
        cube_id: String,
        price_setting: PriceSetting,
        unit_setting: crate::app::settings::unit::UnitSetting,
    ) -> Self {
        Self {
            setting: None,
            cube_id,
            current_price_setting: price_setting,
            current_unit_setting: unit_setting,
            recovery_kit: recovery_kit::RecoveryKit::new(),
            wallet: None,
        }
    }
}

impl State for SettingsState {
    fn update(
        &mut self,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        match &message {
            Message::View(view::Message::Settings(view::SettingsMessage::GeneralSection)) => {
                self.setting = Some(
                    GeneralSettingsState::new(
                        self.cube_id.clone(),
                        self.current_price_setting.clone(),
                        self.current_unit_setting.clone(),
                        &cache.datadir_path,
                    )
                    .into(),
                );
                let reload_task = self
                    .setting
                    .as_mut()
                    .map(|s| s.reload(daemon, None))
                    .unwrap_or_else(Task::none);
                // Kick the Recovery-Kit status fetch so the card has
                // fresh copy by the time the user looks at it. The
                // handler is App-level (it needs the authenticated
                // client); we just drop a message onto the queue.
                let load_status = Task::done(Message::View(view::Message::Settings(
                    view::SettingsMessage::RecoveryKit(view::RecoveryKitMessage::LoadStatus),
                )));
                Task::batch([reload_task, load_status])
            }
            Message::View(view::Message::Settings(view::SettingsMessage::AboutSection)) => {
                self.setting = Some(AboutSettingsState::default().into());
                self.setting
                    .as_mut()
                    .map(|s| s.reload(daemon, None))
                    .unwrap_or_else(Task::none)
            }
            Message::View(view::Message::Settings(view::SettingsMessage::InstallStatsSection)) => {
                self.setting = Some(InstallStatsState::default().into());
                self.setting
                    .as_mut()
                    .map(|s| s.reload(daemon, None))
                    .unwrap_or_else(Task::none)
            }
            Message::View(view::Message::Settings(view::SettingsMessage::LocalSigningSection)) => {
                self.setting = Some(LocalSigningState::default().into());
                // Hand the cached wallet down so LocalSigningState
                // can populate `wallet_fingerprint` and enable the
                // Pair button. App-level `set_current_panel` calls
                // SettingsState::reload before dispatching this
                // message, so `self.wallet` reflects the currently
                // loaded wallet by the time we get here.
                let wallet = self.wallet.clone();
                self.setting
                    .as_mut()
                    .map(|s| s.reload(daemon, wallet))
                    .unwrap_or_else(Task::none)
            }
            Message::SettingsSaved => {
                // Update tracked price and unit settings when saved
                if let Ok(settings) = crate::app::settings::Settings::from_file(
                    &cache.datadir_path.network_directory(cache.network),
                ) {
                    if let Some(cube) = settings.cubes.iter().find(|c| c.id == self.cube_id) {
                        self.current_unit_setting = cube.unit_setting.clone();
                        if let Some(price_setting) = cube.fiat_price.clone() {
                            self.current_price_setting = price_setting;
                        }
                    }
                }
                self.setting
                    .as_mut()
                    .map(|s| s.update(daemon, cache, message))
                    .unwrap_or_else(Task::none)
            }
            _ => self
                .setting
                .as_mut()
                .map(|s| s.update(daemon, cache, message))
                .unwrap_or_else(Task::none),
        }
    }

    fn subscription(&self) -> iced::Subscription<Message> {
        if let Some(setting) = &self.setting {
            setting.subscription()
        } else {
            iced::Subscription::none()
        }
    }

    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        use iced::widget::Column;
        // Recovery-Kit wizard takes over the entire settings page
        // when active, the same way `BackupSeedState != None` does.
        if !matches!(self.recovery_kit.flow, recovery_kit::RecoveryKitState::None) {
            if let Some(wizard) = crate::app::view::settings::recovery_kit::dispatch(
                &self.recovery_kit.flow,
                &self.recovery_kit.pin,
            ) {
                return crate::app::view::dashboard(
                    menu,
                    cache,
                    Column::new().spacing(20).push(wizard),
                );
            }
        }
        if let Some(setting) = &self.setting {
            // Reach into the concrete `GeneralSettingsState` (when
            // that's the active section) so its view can receive the
            // Recovery-Kit status cached on this wrapper. The rest of
            // the sections don't need it and go through the plain
            // `State::view` path.
            if let Some(general) = setting
                .as_any()
                .and_then(|a| a.downcast_ref::<GeneralSettingsState>())
            {
                return general.view_with_recovery_kit(menu, cache, &self.recovery_kit);
            }
            setting.view(menu, cache)
        } else {
            // No setting installed yet — the tertiary rail's click would
            // normally auto-dispatch the matching section. Render the
            // dashboard frame only so the rails stay visible while we
            // (effectively) wait a frame for that dispatch.
            crate::app::view::dashboard(menu, cache, iced::widget::Space::new())
        }
    }

    fn reload(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        wallet: Option<Arc<Wallet>>,
    ) -> Task<Message> {
        // Cache the wallet so the next section dispatch (see the
        // `LocalSigningSection` arm in `update`) can thread it into
        // the section's `reload(daemon, wallet)`.
        self.wallet = wallet;
        self.setting = None;
        Task::none()
    }
}

impl From<SettingsState> for Box<dyn State> {
    fn from(s: SettingsState) -> Box<dyn State> {
        Box::new(s)
    }
}
