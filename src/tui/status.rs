use crate::agent::Stats;
use std::time::SystemTime;

pub(super) fn format_stats(stats: &Stats) -> String {
    let usage = &stats.usage;
    let mut parts: Vec<String> = [
        ("↑", usage.input),
        ("↓", usage.output),
        ("R", usage.cache_read),
        ("W", usage.cache_write),
    ]
    .into_iter()
    .filter(|(_, count)| *count > 0)
    .map(|(label, count)| format!("{label}{}", format_tokens(count)))
    .collect();
    if let Some(rate) = stats.cache_hit_rate
        && (usage.cache_read > 0 || usage.cache_write > 0)
    {
        parts.push(format!("CH{rate:.0}%"));
    }
    let mut sections = vec![parts.join(" ")];
    if let Some(rate) = stats.tokens_per_second {
        sections.push(format!("{rate:.0} tps"));
    }
    sections.retain(|section| !section.is_empty());
    sections.join(" · ")
}

pub(super) fn format_quota(stats: &Stats) -> String {
    [
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
    .filter_map(|(label, remaining, reset)| {
        remaining.map(|remaining| {
            let reset = reset
                .map(|reset| format!(" {}", time_until(reset)))
                .unwrap_or_default();
            format!("{label} {remaining:.0}%{reset}")
        })
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
    let percent = if stats.context_window > 0 {
        stats.context_tokens as f64 / stats.context_window as f64 * 100.0
    } else {
        0.0
    };
    format!("{percent:.0}%/{}", format_tokens(stats.context_window))
}

fn format_tokens(count: u64) -> String {
    let count = count as f64;
    if count < 1_000.0 {
        count.to_string()
    } else if (count / 1_000.0).round() < 1_000.0 {
        format!("{}k", (count / 1_000.0).round())
    } else {
        format!("{}M", (count / 1_000_000.0).round())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_durations() {
        assert_eq!(format_duration(0), "<1m");
        assert_eq!(format_duration(59), "<1m");
        assert_eq!(format_duration(45 * 60), "45m");
        assert_eq!(format_duration(2 * 3_600), "2h");
        assert_eq!(format_duration(2 * 3_600 + 13 * 60), "2h13m");
        assert_eq!(format_duration(3 * 86_400), "3d");
        assert_eq!(format_duration(3 * 86_400 + 4 * 3_600 + 5 * 60), "3d4h");
    }

    #[test]
    fn formats_tokens() {
        assert_eq!(format_tokens(0), "0");
        assert_eq!(format_tokens(999), "999");
        assert_eq!(format_tokens(1_000), "1k");
        assert_eq!(format_tokens(2_500), "3k");
        assert_eq!(format_tokens(12_345), "12k");
        assert_eq!(format_tokens(999_499), "999k");
        assert_eq!(format_tokens(999_500), "1M");
        assert_eq!(format_tokens(1_500_000), "2M");
        assert_eq!(format_tokens(12_345_678), "12M");
    }
}
