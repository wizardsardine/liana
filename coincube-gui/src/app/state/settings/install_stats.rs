use std::sync::Arc;

use coincube_ui::widget::Element;
use iced::Task;

use crate::app::cache::Cache;
use crate::app::menu::Menu;
use crate::app::message::{InstallStatsMessage, Message};
use crate::app::state::State;
use crate::app::view;
use crate::app::wallet::Wallet;
use crate::daemon::Daemon;
use crate::services::coincube::{CoincubeClient, DownloadStats, StatsPeriod, TimeseriesPoint};

pub struct InstallStatsState {
    client: CoincubeClient,
    pub download_stats: Option<DownloadStats>,
    pub today_count: Option<u32>,
    pub timeseries: Option<Vec<TimeseriesPoint>>,
    pub period: StatsPeriod,
    pub loading: bool,
    pub error: Option<String>,
}

impl Default for InstallStatsState {
    fn default() -> Self {
        Self {
            client: CoincubeClient::new(),
            download_stats: None,
            today_count: None,
            timeseries: None,
            period: StatsPeriod::Week,
            loading: false,
            error: None,
        }
    }
}

impl InstallStatsState {
    fn fetch_all(&self) -> Task<Message> {
        let client = self.client.clone();
        let client2 = self.client.clone();
        let client3 = self.client.clone();
        let period = self.period;

        Task::batch(vec![
            Task::perform(
                async move {
                    client
                        .fetch_download_stats()
                        .await
                        .map_err(|e| e.to_string())
                },
                |res| Message::InstallStats(InstallStatsMessage::DownloadStatsLoaded(res)),
            ),
            Task::perform(
                async move {
                    client2
                        .fetch_today_stats()
                        .await
                        .map(|s| s.count)
                        .map_err(|e| e.to_string())
                },
                |res| Message::InstallStats(InstallStatsMessage::TodayStatsLoaded(res)),
            ),
            Task::perform(
                async move {
                    client3
                        .fetch_timeseries(period)
                        .await
                        .map(|r| r.points)
                        .map_err(|e| e.to_string())
                },
                |res| Message::InstallStats(InstallStatsMessage::TimeseriesLoaded(res)),
            ),
        ])
    }

    fn fetch_timeseries(&self) -> Task<Message> {
        let client = self.client.clone();
        let period = self.period;
        Task::perform(
            async move {
                client
                    .fetch_timeseries(period)
                    .await
                    .map(|r| r.points)
                    .map_err(|e| e.to_string())
            },
            |res| Message::InstallStats(InstallStatsMessage::TimeseriesLoaded(res)),
        )
    }
}

impl From<InstallStatsState> for Box<dyn State> {
    fn from(s: InstallStatsState) -> Box<dyn State> {
        Box::new(s)
    }
}

impl State for InstallStatsState {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        crate::app::view::settings::install_stats::install_stats_section(
            menu,
            cache,
            self.download_stats.as_ref(),
            self.today_count,
            self.timeseries.as_deref(),
            self.period,
            self.loading,
            self.error.as_deref(),
        )
    }

    fn update(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        _cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        match message {
            Message::InstallStats(InstallStatsMessage::DownloadStatsLoaded(res)) => match res {
                Ok(stats) => self.download_stats = Some(stats),
                Err(e) => self.error = Some(e),
            },
            Message::InstallStats(InstallStatsMessage::TodayStatsLoaded(res)) => match res {
                Ok(count) => self.today_count = Some(count),
                Err(e) => self.error = Some(e),
            },
            Message::InstallStats(InstallStatsMessage::TimeseriesLoaded(res)) => {
                self.loading = false;
                match res {
                    Ok(points) => self.timeseries = Some(points),
                    Err(e) => self.error = Some(e),
                }
            }
            Message::View(view::Message::Settings(view::SettingsMessage::InstallStats(
                view::InstallStatsViewMessage::PeriodChanged(period),
            ))) => {
                if self.period != period {
                    self.period = period;
                    self.timeseries = None;
                    self.loading = true;
                    return self.fetch_timeseries();
                }
            }
            Message::View(view::Message::Settings(view::SettingsMessage::InstallStats(
                view::InstallStatsViewMessage::Refresh,
            ))) => {
                self.download_stats = None;
                self.today_count = None;
                self.timeseries = None;
                self.loading = true;
                return self.fetch_all();
            }
            _ => {}
        }
        Task::none()
    }

    fn reload(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        _wallet: Option<Arc<Wallet>>,
    ) -> Task<Message> {
        self.loading = true;
        self.fetch_all()
    }
}
