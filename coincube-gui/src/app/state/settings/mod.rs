mod about;
mod general;

use std::sync::Arc;

use iced::Task;

use coincube_ui::widget::Element;

use about::AboutSettingsState;
use general::GeneralSettingsState;

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
        tracing::info!("SettingsState: Received message: {:?}", message);
        match &message {
            Message::View(view::Message::Settings(view::SettingsMessage::GeneralSection)) => {
                self.setting = Some(
                    GeneralSettingsState::new(
                        self.cube_id.clone(),
                        self.current_price_setting.clone(),
                        self.current_unit_setting.clone(),
                    )
                    .into(),
                );
                self.setting
                    .as_mut()
                    .map(|s| s.reload(daemon, None))
                    .unwrap_or_else(Task::none)
            }
            Message::View(view::Message::Settings(view::SettingsMessage::AboutSection)) => {
                self.setting = Some(AboutSettingsState::default().into());
                self.setting
                    .as_mut()
                    .map(|s| s.reload(daemon, None))
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
        if let Some(setting) = &self.setting {
            setting.view(menu, cache)
        } else {
            crate::app::view::settings::list(menu, cache)
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
