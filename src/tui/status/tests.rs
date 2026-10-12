use super::*;
use crate::agent::{Quota, Usage};

fn stats_with(quota: Quota) -> Stats {
    Stats {
        quota,
        ..Default::default()
    }
}

#[test]
fn joins_output_speed_to_stats() {
    let stats = Stats {
        usage: Usage {
            input: 4,
            output: 338,
            cache_read: 6_000,
            cache_write: 88,
            duration_ms: 0,
        },
        tokens_per_second: Some(42.0),
        ..stats_with(Quota::default())
    };
    assert_eq!(format_stats(&stats), "R6k W88 42t/s");
}

#[test]
fn labels_single_quota_window() {
    let five_hour = Quota {
        five_hour_remaining: Some(95.0),
        ..Quota::default()
    };
    assert_eq!(format_quota(&stats_with(five_hour)), "5h 95%");
    let seven_day = Quota {
        seven_day_remaining: Some(81.0),
        ..Quota::default()
    };
    assert_eq!(format_quota(&stats_with(seven_day)), "7d 81%");
    let both = Quota {
        five_hour_remaining: Some(95.0),
        seven_day_remaining: Some(81.0),
        ..Quota::default()
    };
    assert_eq!(format_quota(&stats_with(both)), "95% · 81%");
}

#[test]
fn formats_durations() {
    assert_eq!(format_duration(0), "<1m");
    assert_eq!(format_duration(59), "<1m");
    assert_eq!(format_duration(45 * 60), "45m");
    assert_eq!(format_duration(2 * 3_600), "2h");
    assert_eq!(format_duration(3_600 + 13 * 60), "1h13m");
    assert_eq!(format_duration(2 * 3_600 + 13 * 60), "2h");
    assert_eq!(format_duration(2 * 3_600 + 40 * 60), "3h");
    assert_eq!(format_duration(23 * 3_600 + 40 * 60), "1d");
    assert_eq!(format_duration(86_400 + 4 * 3_600 + 5 * 60), "1d4h");
    assert_eq!(format_duration(3 * 86_400), "3d");
    assert_eq!(format_duration(3 * 86_400 + 4 * 3_600 + 5 * 60), "3d");
    assert_eq!(format_duration(3 * 86_400 + 14 * 3_600), "4d");
}

#[test]
fn shows_context_tokens_without_a_percentage_for_an_unknown_window() {
    let mut stats = stats_with(Quota::default());
    stats.context_tokens = Some(12_000);
    assert_eq!(format_context(&stats), "12k");
    stats.context_window = 200_000;
    assert_eq!(format_context(&stats), "6%/200k");
}

#[test]
fn hides_context_until_a_reply_counts_it() {
    let mut stats = stats_with(Quota::default());
    stats.context_window = 200_000;
    assert_eq!(format_context(&stats), "");
}

#[test]
fn formats_tokens() {
    assert_eq!(format_tokens(0), "0");
    assert_eq!(format_tokens(999), "999");
    assert_eq!(format_tokens(1_000), "1k");
    assert_eq!(format_tokens(2_500), "3k");
    assert_eq!(format_tokens(12_345), "12k");
    assert_eq!(format_tokens(999_499), "999k");
    assert_eq!(format_tokens(999_500), "1m");
    assert_eq!(format_tokens(1_500_000), "2m");
    assert_eq!(format_tokens(12_345_678), "12m");
}
