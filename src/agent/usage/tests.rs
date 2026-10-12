use super::{Quota, Usage, parse_timestamp, tokens_per_second, total_usage};
use serde_json::json;

#[test]
fn keeps_quota_when_headers_are_missing() {
    let mut quota = Quota {
        five_hour_remaining: Some(40.0),
        seven_day_remaining: Some(70.0),
        five_hour_reset: Some(100),
        seven_day_reset: Some(200),
    };
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "anthropic-ratelimit-unified-5h-utilization",
        "0.25".parse().unwrap(),
    );
    quota.update_from_headers(&headers);
    assert_eq!(quota.five_hour_remaining, Some(75.0));
    assert_eq!(quota.seven_day_remaining, Some(70.0));
    assert_eq!(quota.five_hour_reset, Some(100));
    assert_eq!(quota.seven_day_reset, Some(200));
}

#[test]
fn parses_timestamps() {
    assert_eq!(
        parse_timestamp("2026-10-09T04:39:59.870710+00:00"),
        Some(1_791_520_799)
    );
    assert_eq!(parse_timestamp("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(parse_timestamp("1970-01-01T01:00:00+01:00"), Some(0));
    assert_eq!(
        parse_timestamp("2024-02-29T00:00:00-00:30"),
        Some(1_709_166_600)
    );
}

#[test]
fn stores_usage_and_reads_a_bad_one_as_zero() {
    let stored =
        json!({ "input": 1, "output": 2, "cache_read": 3, "cache_write": 4, "duration_ms": 5 });
    let usage = total_usage(&[
        json!({ "usage": stored }),
        json!({ "usage": { "input": 7, "output": "x" } }),
    ]);
    assert_eq!(usage.to_json_with_retried(Usage::default()), stored);
}

#[test]
fn averages_tokens_per_second_over_timed_replies() {
    let messages = vec![
        json!({ "role": "assistant", "content": [], "usage": { "output": 500 } }),
        json!({ "role": "assistant", "content": [], "usage": { "output": 100, "duration_ms": 1_000 } }),
        json!({ "role": "assistant", "content": [], "usage": { "output": 200, "duration_ms": 3_000 } }),
    ];
    assert_eq!(tokens_per_second(&messages), Some(75.0));
    assert_eq!(tokens_per_second(&messages[..1]), None);
}
