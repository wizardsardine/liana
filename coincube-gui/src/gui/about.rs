//! App preferences › About: the component versions and the public download
//! stats, loaded while the sheet is open. Every request carries an id; a
//! result whose id is no longer the one this sheet waits for is dropped, so a
//! closed sheet, a superseded period or an older refresh never lands.

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use coincube_ui::locale::{copy as localized_copy, Text as CopyText};
use iced::Task;

use crate::app::view::preferences::{About, DownloadStatsView};
use crate::daemon::Daemon;
use crate::services::coincube::{CoincubeClient, DownloadStats, StatsPeriod, TimeseriesPoint};

static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);

fn next_request() -> u64 {
    NEXT_REQUEST.fetch_add(1, Ordering::Relaxed)
}

#[derive(Debug)]
pub enum AboutMessage {
    DaemonVersion(u64, Result<String, String>),
    BridgeVersion(u64, Result<String, String>),
    DownloadStats(u64, Result<DownloadStats, String>),
    Today(u64, Result<u32, String>),
    Timeseries(u64, Result<Vec<TimeseriesPoint>, String>),
}

#[derive(Default)]
pub struct AboutState {
    versions_request: Option<u64>,
    daemon_version: Option<String>,
    bridge_version: Option<String>,
    stats: StatsState,
}

struct StatsState {
    /// Id of the totals/today batch in flight or last accepted.
    request: Option<u64>,
    timeseries_request: Option<u64>,
    pending: u8,
    period: StatsPeriod,
    stats: Option<DownloadStats>,
    today: Option<u32>,
    timeseries: Option<Vec<TimeseriesPoint>>,
    error: Option<String>,
}

impl Default for StatsState {
    fn default() -> Self {
        Self {
            request: None,
            timeseries_request: None,
            pending: 0,
            period: StatsPeriod::Week,
            stats: None,
            today: None,
            timeseries: None,
            error: None,
        }
    }
}

impl AboutState {
    /// Checks the component versions once per sheet. `daemon` is the Vault
    /// daemon of the Cube the sheet was opened from; without one, the version
    /// shown is the daemon built into this app.
    pub fn load_versions(
        &mut self,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
    ) -> Task<AboutMessage> {
        if self.versions_request.is_some() {
            return Task::none();
        }
        let id = next_request();
        self.versions_request = Some(id);
        let daemon_task = match daemon {
            Some(daemon) => Task::perform(
                async move {
                    daemon
                        .get_info()
                        .await
                        .map(|info| info.version)
                        .map_err(|e| e.to_string())
                },
                move |result| AboutMessage::DaemonVersion(id, result),
            ),
            None => Task::done(AboutMessage::DaemonVersion(
                id,
                Ok(format!(
                    "{} · {}",
                    coincubed::VERSION,
                    localized_copy("built in")
                )),
            )),
        };
        let bridge_task = Task::perform(
            async {
                crate::app::breez_spark::client::bridge_version()
                    .await
                    .map_err(|e| e.to_string())
            },
            move |result| AboutMessage::BridgeVersion(id, result),
        );
        Task::batch([daemon_task, bridge_task])
    }

    /// (Re)loads the totals, today's count and the current period's series.
    pub fn refresh_stats(&mut self) -> Task<AboutMessage> {
        let id = next_request();
        let stats = &mut self.stats;
        stats.request = Some(id);
        stats.pending = 2;
        stats.stats = None;
        stats.today = None;
        stats.error = None;
        let client = CoincubeClient::new();
        let today_client = client.clone();
        Task::batch([
            Task::perform(
                async move {
                    client
                        .fetch_download_stats()
                        .await
                        .map_err(|e| e.to_string())
                },
                move |result| AboutMessage::DownloadStats(id, result),
            ),
            Task::perform(
                async move {
                    today_client
                        .fetch_today_stats()
                        .await
                        .map(|today| today.count)
                        .map_err(|e| e.to_string())
                },
                move |result| AboutMessage::Today(id, result),
            ),
            self.load_timeseries(),
        ])
    }

    pub fn select_period(&mut self, period: StatsPeriod) -> Task<AboutMessage> {
        if self.stats.period == period && self.stats.timeseries_request.is_some() {
            return Task::none();
        }
        self.stats.period = period;
        self.stats.error = None;
        self.load_timeseries()
    }

    fn load_timeseries(&mut self) -> Task<AboutMessage> {
        let id = next_request();
        let period = self.stats.period;
        self.stats.timeseries_request = Some(id);
        self.stats.timeseries = None;
        let client = CoincubeClient::new();
        Task::perform(
            async move {
                client
                    .fetch_timeseries(period)
                    .await
                    .map(|response| response.points)
                    .map_err(|e| e.to_string())
            },
            move |result| AboutMessage::Timeseries(id, result),
        )
    }

    pub fn apply(&mut self, message: AboutMessage) {
        let unavailable = || CopyText::Unavailable.get().to_string();
        match message {
            AboutMessage::DaemonVersion(id, result) if self.versions_request == Some(id) => {
                self.daemon_version = Some(result.unwrap_or_else(|e| {
                    tracing::warn!("Wallet daemon version unavailable: {e}");
                    unavailable()
                }));
            }
            AboutMessage::BridgeVersion(id, result) if self.versions_request == Some(id) => {
                self.bridge_version = Some(result.unwrap_or_else(|e| {
                    tracing::warn!("Spark bridge version unavailable: {e}");
                    unavailable()
                }));
            }
            AboutMessage::DownloadStats(id, result) if self.stats.request == Some(id) => {
                self.stats.pending = self.stats.pending.saturating_sub(1);
                match result {
                    Ok(stats) => self.stats.stats = Some(stats),
                    Err(e) => self.stats.error = Some(e),
                }
            }
            AboutMessage::Today(id, result) if self.stats.request == Some(id) => {
                self.stats.pending = self.stats.pending.saturating_sub(1);
                match result {
                    Ok(today) => self.stats.today = Some(today),
                    Err(e) => self.stats.error = Some(e),
                }
            }
            AboutMessage::Timeseries(id, result) if self.stats.timeseries_request == Some(id) => {
                match result {
                    Ok(points) => self.stats.timeseries = Some(points),
                    Err(e) => {
                        self.stats.timeseries = Some(Vec::new());
                        self.stats.error = Some(e);
                    }
                }
            }
            _ => {}
        }
    }

    pub fn view(&self) -> About<'_> {
        let stats = &self.stats;
        let timeseries_loading = stats.timeseries_request.is_some() && stats.timeseries.is_none();
        About {
            build_provenance: None,
            daemon_version: self.daemon_version.as_deref(),
            spark_bridge_version: self.bridge_version.as_deref(),
            can_open_download_stats: true,
            can_open_licenses: true,
            download_stats: DownloadStatsView {
                stats: stats.stats.as_ref(),
                today: stats.today,
                timeseries: stats.timeseries.as_deref(),
                period: stats.period,
                loading: stats.pending > 0 || timeseries_loading,
                error: stats.error.as_deref(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_superseded_result_is_dropped() {
        let mut about = AboutState::default();
        let _ = about.refresh_stats();
        let stale = about.stats.request.unwrap();
        let _ = about.refresh_stats();
        about.apply(AboutMessage::Today(stale, Ok(7)));
        assert_eq!(about.view().download_stats.today, None);
        assert!(about.view().download_stats.loading);

        let current = about.stats.request.unwrap();
        about.apply(AboutMessage::Today(current, Ok(9)));
        assert_eq!(about.view().download_stats.today, Some(9));
    }

    #[test]
    fn a_failed_version_check_reads_unavailable() {
        let mut about = AboutState::default();
        let _ = about.load_versions(None);
        let id = about.versions_request.unwrap();
        about.apply(AboutMessage::BridgeVersion(id, Err("missing".into())));
        assert_eq!(
            about.view().spark_bridge_version,
            Some(CopyText::Unavailable.get())
        );
        assert_eq!(about.view().daemon_version, None, "still checking");
    }

    #[test]
    fn changing_period_reloads_only_the_series() {
        let mut about = AboutState::default();
        let _ = about.refresh_stats();
        let batch = about.stats.request;
        let _ = about.select_period(StatsPeriod::Month);
        assert_eq!(about.stats.request, batch);
        assert_eq!(about.view().download_stats.period, StatsPeriod::Month);
    }
}
