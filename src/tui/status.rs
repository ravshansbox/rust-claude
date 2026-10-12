use crate::agent::Stats;
use std::time::SystemTime;

pub(super) fn format_stats(stats: &Stats) -> String {
    let usage = &stats.usage;
    let mut parts: Vec<String> = [("R", usage.cache_read), ("W", usage.cache_write)]
        .into_iter()
        .filter(|(_, count)| *count > 0)
        .map(|(label, count)| format!("{label}{}", format_tokens(count)))
        .collect();
    if let Some(rate) = stats.tokens_per_second {
        parts.push(format!("{rate:.0}t/s"));
    }
    parts.join(" ")
}

pub(super) fn format_quota(stats: &Stats) -> String {
    let windows: Vec<_> = [
        (
            "5h",
            stats.quota.five_hour_remaining,
            stats.quota.five_hour_reset,
        ),
        (
            "7d",
            stats.quota.seven_day_remaining,
            stats.quota.seven_day_reset,
        ),
    ]
    .into_iter()
    .filter_map(|(label, remaining, reset)| remaining.map(|remaining| (label, remaining, reset)))
    .collect();
    let single = windows.len() == 1;
    windows
        .into_iter()
        .map(|(label, remaining, reset)| {
            let label = if single {
                format!("{label} ")
            } else {
                String::new()
            };
            let reset = reset
                .map(|reset| format!(" {}", time_until(reset)))
                .unwrap_or_default();
            format!("{label}{remaining:.0}%{reset}")
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

fn time_until(reset: u64) -> String {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format_duration(reset.saturating_sub(now))
}

fn format_duration(seconds: u64) -> String {
    let minutes = seconds / 60;
    let (days, hours, minutes) = (minutes / 1_440, minutes / 60 % 24, minutes % 60);
    if days >= 2 || (days == 0 && hours >= 23 && minutes >= 30) {
        return format!("{}d", (seconds + 43_200) / 86_400);
    }
    if days == 0 && hours >= 2 {
        return format!("{}h", (seconds + 1_800) / 3_600);
    }
    match (days, hours, minutes) {
        (0, 0, 0) => "<1m".to_string(),
        (0, 0, minutes) => format!("{minutes}m"),
        (0, hours, 0) => format!("{hours}h"),
        (0, hours, minutes) => format!("{hours}h{minutes}m"),
        (days, 0, _) => format!("{days}d"),
        (days, hours, _) => format!("{days}d{hours}h"),
    }
}

pub(super) fn format_context(stats: &Stats) -> String {
    let Some(tokens) = stats.context_tokens else {
        return String::new();
    };
    if stats.context_window == 0 {
        return format_tokens(tokens);
    }
    let percent = tokens as f64 / stats.context_window as f64 * 100.0;
    format!("{percent:.0}%/{}", format_tokens(stats.context_window))
}

fn format_tokens(count: u64) -> String {
    let count = count as f64;
    let thousands = (count / 1_000.0).round();
    if count < 1_000.0 {
        count.to_string()
    } else if thousands < 1_000.0 {
        format!("{thousands}k")
    } else {
        format!("{}m", (count / 1_000_000.0).round())
    }
}

#[cfg(test)]
mod tests {
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
}
