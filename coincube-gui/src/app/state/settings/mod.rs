pub mod about;
pub mod connections;
pub mod general;
mod install_stats;
pub mod keychain_backfill;
pub mod local_signing;
pub mod recovery_alerts;
pub mod recovery_kit;

use std::sync::Arc;

use iced::Task;

use coincube_ui::widget::Element;

use about::AboutSettingsState;
use connections::ConnectionsSettingsState;
use general::{GeneralSettingsState, SettingsSection};
use install_stats::InstallStatsState;

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
    /// Vault Recovery Alerts card (Estate Notifications — PR 2). Held here
    /// for the same reason as `recovery_kit`: the App-level handler injects
    /// the authenticated client, cube id, wallet, and keyholders.
    pub recovery_alerts: recovery_alerts::RecoveryAlerts,
}

impl SettingsState {
    #[cfg(test)]
    pub(crate) fn reviewer_show_seed_phrase_for_test(
        &mut self,
        datadir: &crate::dir::CoincubeDirectory,
    ) {
        let mut recovery = GeneralSettingsState::new(
            self.cube_id.clone(),
            SettingsSection::Recovery,
            self.current_price_setting.clone(),
            self.current_unit_setting.clone(),
            datadir,
        );
        recovery.backup_state = general::BackupSeedState::RecoveryPhrase;
        recovery.backup_mnemonic = Some(zeroize::Zeroizing::new(vec!["synthetic".into(); 12]));
        self.setting = Some(recovery.into());
    }

    /// Load the active Cube's preferences from its authenticated chain directory.
    pub fn from_directory(cube_id: String, directory: &crate::dir::NetworkDirectory) -> Self {
        let (price, unit) = crate::app::settings::Settings::from_file(directory)
            .ok()
            .and_then(|s| s.cubes.into_iter().find(|c| c.id == cube_id))
            .map(|c| (c.fiat_price.unwrap_or_default(), c.unit_setting))
            .unwrap_or_default();
        Self::new(cube_id, price, unit)
    }

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
            recovery_alerts: recovery_alerts::RecoveryAlerts::new(),
        }
    }
}

impl SettingsState {
    /// The Display & appearance (or another general) section, when open.
    pub fn general(&self) -> Option<&GeneralSettingsState> {
        self.setting
            .as_ref()
            .and_then(|s| s.as_any())
            .and_then(|a| a.downcast_ref::<GeneralSettingsState>())
    }

    /// Leaving Display & appearance for another section saves a Cube name
    /// that was typed but not yet submitted, as leaving the field does.
    fn save_pending_name(&self, cache: &Cache) -> Task<Message> {
        match self
            .general()
            .and_then(|g| g.pending_cube_name(&cache.cube_name))
        {
            Some(name) => Task::done(Message::View(view::Message::Settings(
                view::SettingsMessage::CubeNameSubmitted(name),
            ))),
            None => Task::none(),
        }
    }

    fn switch_section(
        &mut self,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        match &message {
            Message::View(view::Message::Settings(view::SettingsMessage::GeneralSection))
                if self
                    .general()
                    .is_some_and(|g| g.section() == SettingsSection::General) =>
            {
                Task::none()
            }
            Message::View(view::Message::Settings(view::SettingsMessage::GeneralSection)) => {
                self.setting = Some(
                    GeneralSettingsState::new(
                        self.cube_id.clone(),
                        SettingsSection::General,
                        self.current_price_setting.clone(),
                        self.current_unit_setting.clone(),
                        &cache.datadir_path,
                    )
                    .into(),
                );
                self.setting
                    .as_mut()
                    .map(|s| s.reload(daemon, None))
                    .unwrap_or_else(Task::none)
            }
            Message::View(view::Message::Settings(view::SettingsMessage::PaymentChecksSection)) => {
                self.setting = Some(
                    GeneralSettingsState::new(
                        self.cube_id.clone(),
                        SettingsSection::PaymentChecks,
                        self.current_price_setting.clone(),
                        self.current_unit_setting.clone(),
                        &cache.datadir_path,
                    )
                    .into(),
                );
                Task::none()
            }
            Message::View(view::Message::Settings(view::SettingsMessage::DataExportsSection)) => {
                self.setting = Some(
                    GeneralSettingsState::new(
                        self.cube_id.clone(),
                        SettingsSection::DataExports,
                        self.current_price_setting.clone(),
                        self.current_unit_setting.clone(),
                        &cache.datadir_path,
                    )
                    .into(),
                );
                Task::none()
            }
            Message::View(view::Message::Settings(view::SettingsMessage::RecoverySection)) => {
                // Same content state, Recovery face — it hosts the local backup
                // flow plus the Recovery-Kit and Vault-Alerts cards.
                self.setting = Some(
                    GeneralSettingsState::new(
                        self.cube_id.clone(),
                        SettingsSection::Recovery,
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
                // Kick the Recovery-Kit status fetch so the card has a fresh
                // copy by the time the user looks at it. The handler is
                // App-level (it needs the authenticated client); we just drop a
                // message onto the queue.
                let load_status = Task::done(Message::View(view::Message::Settings(
                    view::SettingsMessage::RecoveryKit(view::RecoveryKitMessage::LoadStatus),
                )));
                // Same pattern for the Recovery Alerts card — kick its status
                // fetch so the three-tier selector reflects the server.
                let load_alerts = Task::done(Message::View(view::Message::Settings(
                    view::SettingsMessage::RecoveryAlerts(view::RecoveryAlertsMessage::LoadStatus),
                )));
                Task::batch([reload_task, load_status, load_alerts])
            }
            Message::View(view::Message::Settings(view::SettingsMessage::AboutSection)) => {
                self.setting = Some(AboutSettingsState::default().into());
                self.setting
                    .as_mut()
                    .map(|s| s.reload(daemon, None))
                    .unwrap_or_else(Task::none)
            }
            Message::View(view::Message::Settings(view::SettingsMessage::ConnectionsSection)) => {
                // Re-opening the tab keeps the last Re-register result.
                if !self.setting.as_ref().is_some_and(|s| {
                    s.as_any()
                        .is_some_and(|a| a.is::<ConnectionsSettingsState>())
                }) {
                    self.setting = Some(ConnectionsSettingsState::default().into());
                }
                Task::none()
            }
            Message::View(view::Message::Settings(view::SettingsMessage::InstallStatsSection)) => {
                self.setting = Some(InstallStatsState::default().into());
                self.setting
                    .as_mut()
                    .map(|s| s.reload(daemon, None))
                    .unwrap_or_else(Task::none)
            }
            Message::SettingsSaved => {
                // Update tracked price and unit settings when saved
                if let Ok(settings) = crate::app::settings::Settings::from_file(
                    &cache
                        .datadir_path
                        .network_directory(if cache.fiat_chain.is_blake2b() {
                            cache.fiat_chain
                        } else {
                            cache.network.into()
                        }),
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
}

impl State for SettingsState {
    fn update(
        &mut self,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        if let Message::View(view::Message::Settings(
            view::SettingsMessage::PaymentChecksSection
            | view::SettingsMessage::DataExportsSection
            | view::SettingsMessage::RecoverySection
            | view::SettingsMessage::AboutSection
            | view::SettingsMessage::ConnectionsSection
            | view::SettingsMessage::InstallStatsSection,
        )) = &message
        {
            let save = self.save_pending_name(cache);
            let switch = self.switch_section(daemon, cache, message);
            return Task::batch([save, switch]);
        }
        self.switch_section(daemon, cache, message)
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
                self.recovery_kit.status.as_ref(),
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
                return general.view_with_recovery_kit(
                    menu,
                    cache,
                    &self.recovery_kit,
                    &self.recovery_alerts,
                );
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
        _wallet: Option<Arc<Wallet>>,
    ) -> Task<Message> {
        self.setting = None;
        Task::none()
    }
}

impl From<SettingsState> for Box<dyn State> {
    fn from(s: SettingsState) -> Box<dyn State> {
        Box::new(s)
    }
}

#[cfg(test)]
mod btcb2_price_settings_tests {
    use super::*;
    use crate::{
        app::settings::{self, CubeSettings},
        chain::ChainId,
        dir::CoincubeDirectory,
        services::fiat::{currency::Currency, PriceSource},
    };

    #[test]
    fn app_settings_saved_refreshes_the_current_panel_before_returning_its_task() {
        let _guard = crate::app::session::test_guard();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        let root = tempfile::tempdir().unwrap();
        let (mut app, _) = crate::app::shell_tests::bitcoin_app(root.path());
        app.panels.current = Menu::Cube(crate::app::menu::CubeSubMenu::Settings(
            crate::app::menu::CubeSettingsOption::General,
        ));
        let mut cube = app.cube_settings.clone();
        cube.unit_setting.display_unit = settings::unit::BitcoinDisplayUnit::BTC;
        cube.fiat_price = Some(PriceSetting {
            currency: Currency::EUR,
            ..Default::default()
        });
        let directory = app.cache.datadir_path.network_directory(ChainId::Bitcoin);
        std::fs::create_dir_all(directory.path()).unwrap();
        std::fs::write(
            directory.path().join(settings::SETTINGS_FILE_NAME),
            serde_json::to_vec(&settings::Settings {
                cubes: vec![cube],
                ..Default::default()
            })
            .unwrap(),
        )
        .unwrap();
        app.panels.global_settings.current_unit_setting.display_unit =
            settings::unit::BitcoinDisplayUnit::Sats;
        app.panels.global_settings.current_price_setting.currency = Currency::USD;
        let task = app.update(Message::SettingsSaved);
        // These are panel-owned preferences, not the App cache. No task has
        // been polled: forwarding must already have refreshed them.
        assert_eq!(
            app.panels.global_settings.current_unit_setting.display_unit,
            settings::unit::BitcoinDisplayUnit::BTC
        );
        assert_eq!(
            app.panels.global_settings.current_price_setting.currency,
            Currency::EUR
        );
        drop(task);
    }

    #[tokio::test]
    async fn reopening_and_save_refresh_load_only_the_cubes_chain_preferences() {
        let path =
            std::env::temp_dir().join(format!("coincube-price-reopen-{}", uuid::Uuid::new_v4()));
        let root = CoincubeDirectory::new(path.clone());
        for (chain, currency) in [
            (ChainId::Bitcoin, Currency::USD),
            (ChainId::BitcoinBlake2b, Currency::EUR),
        ] {
            let mut cube = CubeSettings::new_with_raw_id("same-id".into(), "Fixture".into(), chain);
            cube.fiat_price = Some(PriceSetting {
                currency,
                source: PriceSource::Coincube,
                is_enabled: true,
            });
            settings::update_settings_file(&root.network_directory(chain), move |mut s| {
                s.cubes.push(cube);
                Some(s)
            })
            .await
            .unwrap();
        }
        let mut panel = SettingsState::from_directory(
            "same-id".into(),
            &root.network_directory(ChainId::BitcoinBlake2b),
        );
        assert_eq!(panel.current_price_setting.currency, Currency::EUR);
        panel.current_price_setting.currency = Currency::USD;
        let cache = Cache {
            datadir_path: root.clone(),
            fiat_chain: ChainId::BitcoinBlake2b,
            ..Default::default()
        };
        let _ = panel.update(None, &cache, Message::SettingsSaved);
        assert_eq!(panel.current_price_setting.currency, Currency::EUR);
        let bitcoin = SettingsState::from_directory(
            "same-id".into(),
            &root.network_directory(ChainId::Bitcoin),
        );
        assert_eq!(bitcoin.current_price_setting.currency, Currency::USD);
        std::fs::remove_dir_all(path).unwrap();
    }
}
