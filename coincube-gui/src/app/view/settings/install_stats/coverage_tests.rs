use super::*;
use crate::app::view::shared::landing::capture;

fn inspect<M: 'static>(content: Element<'_, M>) -> capture::Inspection {
    capture::inspect_rendered_full_window(
        "download-stats-behavior",
        content,
        (1024, 900),
        "dark",
        theme::Theme::dark(),
    )
}

#[test]
fn unknown_and_loading_totals_are_distinct_from_measured_zero() {
    let missing: Element<'_, ()> = downloads_card(None, None, false);
    let view = inspect(missing);
    assert!(view.contains("—"));
    assert!(!view.contains("0.00%"));
    let loading: Element<'_, ()> = downloads_card(None, None, true);
    assert!(inspect(loading).contains("Loading…"));
    let stats = DownloadStats {
        total: 0,
        breakdown: Default::default(),
        last_updated: String::new(),
    };
    let measured: Element<'_, ()> = downloads_card(Some(&stats), Some(0), false);
    let view = inspect(measured);
    assert!(view.contains("0.00% of goal"));
    assert!(view.contains("+0 downloads today"));
}

#[test]
fn download_goal_keeps_actual_total_even_above_one_hundred_percent() {
    for (total, formatted, percentage) in [
        (12345, "12 345", "1.23%"),
        (1000000, "1 000 000", "100.00%"),
        (1500000, "1 500 000", "150.00%"),
    ] {
        let stats = DownloadStats {
            total,
            breakdown: Default::default(),
            last_updated: String::new(),
        };
        let content: Element<'_, ()> = downloads_card(Some(&stats), Some(42), false);
        let view = inspect(content);
        assert!(view.contains(formatted));
        assert!(view.contains(percentage));
        assert!(view.contains("+42 downloads today"));
    }
}

#[test]
fn period_selector_emits_each_requested_period_without_running_a_fetch() {
    for selected in [
        StatsPeriod::Day,
        StatsPeriod::Week,
        StatsPeriod::Month,
        StatsPeriod::Year,
    ] {
        let render = || chart_card(None, StatsPeriod::Day, false, |period| period);
        let messages = capture::click_text(render(), selected.label(), 0, (1024, 900));
        assert_eq!(messages, [selected]);
    }
    assert!(
        inspect(chart_card(Some(&[]), StatsPeriod::Day, false, |period| {
            period
        }))
        .contains("No data")
    );
    assert!(
        inspect(chart_card(None, StatsPeriod::Day, true, |period| period)).contains("Loading…")
    );
}

#[test]
fn charts_accept_single_zero_and_sparse_series_without_inventing_empty_state() {
    for values in [vec![0], vec![10], vec![0, 5, 0], vec![u32::MAX, 1, 100]] {
        let points = values
            .into_iter()
            .enumerate()
            .map(|(i, count)| TimeseriesPoint {
                date: format!("2026-11-{:02}", i + 1),
                count,
            })
            .collect::<Vec<_>>();
        let view = inspect(chart_card(
            Some(&points),
            StatsPeriod::Month,
            false,
            |period| period,
        ));
        assert!(!view.contains("No data"));
        assert!(!view.contains("Loading…"));
        assert!(view.contains("Month"));
    }
}
