use std::sync::Arc;

use coincube_core::miniscript::bitcoin::Network;
use coincube_ui::widget::Element;
use iced::Task;

use crate::app::cache::Cache;
use crate::app::error::Error;
use crate::app::menu::Menu;
use crate::app::message::{FiatMessage, Message};
use crate::app::settings::fiat::PriceSetting;
use crate::app::settings::unit::UnitSetting;
use crate::app::settings::update_settings_file;
use crate::app::state::State;
use crate::app::view;
use crate::app::wallet::Wallet;
use crate::daemon::Daemon;
use crate::dir::CoincubeDirectory;
use crate::services::fiat::currency::Currency;

async fn update_price_setting(
    data_dir: CoincubeDirectory,
    network: Network,
    cube_id: String,
    new_price_setting: PriceSetting,
) -> Result<(), Error> {
    let network_dir = data_dir.network_directory(network);
    let mut cube_found = false;
    let result = update_settings_file(&network_dir, |mut settings| {
        if let Some(cube) = settings.cubes.iter_mut().find(|c| c.id == cube_id) {
            cube.fiat_price = Some(new_price_setting);
            cube_found = true;
        } else {
            tracing::error!(
                "Cube not found with id: {} - cannot save price setting",
                cube_id
            );
            tracing::error!(
                "Available cubes: {:?}",
                settings.cubes.iter().map(|c| &c.id).collect::<Vec<_>>()
            );
        }
        // Always return Some to prevent file deletion
        Some(settings)
    })
    .await;

    match result {
        Ok(()) if cube_found => Ok(()),
        Ok(()) => Err(Error::Unexpected(
            "Cube not found in settings file".to_string(),
        )),
        Err(e) => {
            tracing::error!("Failed to save price setting: {:?}", e);
            Err(Error::Unexpected(format!(
                "Failed to update settings: {}",
                e
            )))
        }
    }
}

async fn update_unit_setting(
    data_dir: CoincubeDirectory,
    network: Network,
    cube_id: String,
    new_unit_setting: UnitSetting,
) -> Result<(), Error> {
    let network_dir = data_dir.network_directory(network);
    let mut cube_found = false;
    let result = update_settings_file(&network_dir, |mut settings| {
        if let Some(cube) = settings.cubes.iter_mut().find(|c| c.id == cube_id) {
            cube.unit_setting = new_unit_setting;
            cube_found = true;
        } else {
            tracing::error!(
                "Cube not found with id: {} - cannot save unit setting",
                cube_id
            );
            tracing::error!(
                "Available cubes: {:?}",
                settings.cubes.iter().map(|c| &c.id).collect::<Vec<_>>()
            );
        }
        // Always return Some to prevent file deletion
        Some(settings)
    })
    .await;

    match result {
        Ok(()) if cube_found => Ok(()),
        Ok(()) => Err(Error::Unexpected(
            "Cube not found in settings file".to_string(),
        )),
        Err(e) => {
            tracing::error!("Failed to save unit setting: {:?}", e);
            Err(Error::Unexpected(format!(
                "Failed to update settings: {}",
                e
            )))
        }
    }
}

pub struct GeneralSettingsState {
    cube_id: String,
    new_price_setting: PriceSetting,
    new_unit_setting: UnitSetting,
    currencies: Vec<Currency>,
    error: Option<Error>,
}

impl From<GeneralSettingsState> for Box<dyn State> {
    fn from(s: GeneralSettingsState) -> Box<dyn State> {
        Box::new(s)
    }
}

impl GeneralSettingsState {
    pub fn new(cube_id: String, price_setting: PriceSetting, unit_setting: UnitSetting) -> Self {
        Self {
            cube_id,
            new_price_setting: price_setting,
            new_unit_setting: unit_setting,
            currencies: Vec::new(),
            error: None,
        }
    }
}

impl State for GeneralSettingsState {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        crate::app::view::settings::general::general_section(
            menu,
            cache,
            &self.new_price_setting,
            &self.new_unit_setting,
            &self.currencies,
            None, // Errors now shown via global toast
        )
    }

    fn reload(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        _wallet: Option<Arc<Wallet>>,
    ) -> iced::Task<Message> {
        if self.new_price_setting.is_enabled {
            let source = self.new_price_setting.source;
            return Task::perform(async move { source }, |source| {
                FiatMessage::ListCurrencies(source).into()
            });
        }
        Task::none()
    }

    fn update(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        match message {
            Message::Fiat(FiatMessage::SaveChanges) => {
                if self.error.is_none() {
                    tracing::info!(
                        "Saving cube fiat price setting: {:?}",
                        self.new_price_setting
                    );
                    let price_setting = self.new_price_setting.clone();
                    let network = cache.network;
                    let datadir_path = cache.datadir_path.clone();
                    let cube_id = self.cube_id.clone();
                    return Task::perform(
                        async move {
                            update_price_setting(datadir_path, network, cube_id, price_setting)
                                .await
                        },
                        |res| match res {
                            Ok(()) => Message::SettingsSaved,
                            Err(e) => Message::SettingsSaveFailed(e),
                        },
                    );
                }
                Task::none()
            }
            Message::SettingsSaved => {
                tracing::info!("GeneralSettingsState: SettingsSaved received");
                self.error = None;
                // Reload unit setting from disk to sync toggle state with what was saved
                let network_dir = cache.datadir_path.network_directory(cache.network);
                tracing::info!(
                    "GeneralSettingsState: Loading settings from {:?}",
                    network_dir.path()
                );
                if let Ok(settings) = crate::app::settings::Settings::from_file(&network_dir) {
                    tracing::info!(
                        "GeneralSettingsState: Loaded settings, searching for cube_id: {}",
                        self.cube_id
                    );
                    tracing::info!(
                        "GeneralSettingsState: Available cubes: {:?}",
                        settings.cubes.iter().map(|c| &c.id).collect::<Vec<_>>()
                    );
                    if let Some(cube) = settings.cubes.iter().find(|c| c.id == self.cube_id) {
                        tracing::info!(
                            "GeneralSettingsState: Found cube, reloading unit_setting: {:?}",
                            cube.unit_setting.display_unit
                        );
                        self.new_unit_setting = cube.unit_setting.clone();
                        tracing::info!(
                            "GeneralSettingsState: new_unit_setting now set to: {:?}",
                            self.new_unit_setting.display_unit
                        );
                    } else {
                        tracing::warn!(
                            "GeneralSettingsState: Cube not found with id: {}",
                            self.cube_id
                        );
                    }
                } else {
                    tracing::error!("GeneralSettingsState: Failed to load settings from disk");
                }
                Task::none()
            }
            Message::SettingsSaveFailed(e) => {
                let err_msg = e.to_string();
                self.error = Some(e);
                // Show error in global toast
                let toast_task = Task::done(Message::View(view::Message::ShowError(err_msg)));
                // Reload settings from disk to revert toggle state to persisted value
                let network_dir = cache.datadir_path.network_directory(cache.network);
                if let Ok(settings) = crate::app::settings::Settings::from_file(&network_dir) {
                    if let Some(cube) = settings.cubes.iter().find(|c| c.id == self.cube_id) {
                        tracing::info!(
                            "Reverting unit_setting to persisted value after save failure: {:?}",
                            cube.unit_setting.display_unit
                        );
                        self.new_unit_setting = cube.unit_setting.clone();
                        self.new_price_setting = cube.fiat_price.clone().unwrap_or_default();
                    } else {
                        tracing::warn!(
                            "Could not revert settings: Cube not found with id: {}",
                            self.cube_id
                        );
                    }
                } else {
                    tracing::error!("Could not revert settings: Failed to load settings from disk");
                }
                toast_task
            }
            Message::Fiat(FiatMessage::ValidateCurrencySetting) => {
                self.error = None;
                if !self.currencies.contains(&self.new_price_setting.currency) {
                    if self.currencies.contains(&Currency::default()) {
                        self.new_price_setting.currency = Currency::default();
                    } else if let Some(curr) = self.currencies.first() {
                        self.new_price_setting.currency = *curr;
                    } else {
                        let err = Error::Unexpected(
                            "No available currencies in the list.".to_string(),
                        );
                        let err_msg = err.to_string();
                        self.error = Some(err);
                        return Task::done(Message::View(view::Message::ShowError(err_msg)));
                    }
                }
                Task::perform(async move {}, |_| FiatMessage::SaveChanges.into())
            }
            Message::Fiat(FiatMessage::ListCurrenciesResult(source, res)) => {
                if self.new_price_setting.source != source {
                    return Task::none();
                }
                match res {
                    Ok(list) => {
                        self.error = None;
                        self.currencies = list.currencies;
                        Task::perform(async move {}, |_| {
                            FiatMessage::ValidateCurrencySetting.into()
                        })
                    }
                    Err(e) => {
                        let err: Error = e.into();
                        let err_msg = err.to_string();
                        self.error = Some(err);
                        Task::done(Message::View(view::Message::ShowError(err_msg)))
                    }
                }
            }
            Message::View(view::Message::Settings(view::SettingsMessage::Fiat(msg))) => {
                match msg {
                    view::FiatMessage::Enable(is_enabled) => {
                        self.new_price_setting.is_enabled = is_enabled;
                        if self.new_price_setting.is_enabled {
                            let source = self.new_price_setting.source;
                            return Task::perform(async move { source }, |source| {
                                FiatMessage::ListCurrencies(source).into()
                            });
                        } else {
                            return Task::perform(async move {}, |_| {
                                FiatMessage::SaveChanges.into()
                            });
                        }
                    }
                    view::FiatMessage::SourceEdited(source) => {
                        self.new_price_setting.source = source;
                        if self.new_price_setting.is_enabled {
                            let source = self.new_price_setting.source;
                            return Task::perform(async move { source }, |source| {
                                FiatMessage::ListCurrencies(source).into()
                            });
                        }
                    }
                    view::FiatMessage::CurrencyEdited(currency) => {
                        self.new_price_setting.currency = currency;
                        return Task::perform(async move {}, |_| {
                            FiatMessage::ValidateCurrencySetting.into()
                        });
                    }
                }
                Task::none()
            }
            Message::View(view::Message::Settings(view::SettingsMessage::DisplayUnitChanged(
                unit,
            ))) => {
                tracing::info!("GeneralSettingsState: DisplayUnitChanged({:?})", unit);
                self.new_unit_setting.display_unit = unit;
                tracing::info!(
                    "GeneralSettingsState: Updated new_unit_setting to {:?}",
                    self.new_unit_setting.display_unit
                );
                let cube_id = self.cube_id.clone();
                let unit_setting = self.new_unit_setting.clone();
                let network = cache.network;
                let datadir_path = cache.datadir_path.clone();

                // Save to disk - cache update will happen in App::update after this returns
                #[allow(clippy::let_and_return)]
                return Task::perform(
                    async move {
                        tracing::info!(
                            "Saving unit_setting to disk: {:?}",
                            unit_setting.display_unit
                        );
                        update_unit_setting(datadir_path, network, cube_id, unit_setting).await
                    },
                    |res| match res {
                        Ok(()) => {
                            tracing::info!("Unit setting saved successfully");
                            Message::SettingsSaved
                        }
                        Err(e) => {
                            tracing::error!("Unit setting save failed: {:?}", e);
                            Message::SettingsSaveFailed(e)
                        }
                    },
                );
            }
            _ => Task::none(),
        }
    }
}
