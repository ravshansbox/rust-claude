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
    let quota = &stats.quota;
    let windows: Vec<_> = [
        ("5h", quota.five_hour_remaining, quota.five_hour_reset),
        ("7d", quota.seven_day_remaining, quota.seven_day_reset),
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
mod tests;
