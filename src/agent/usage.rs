use anyhow::{Result, bail};
use serde_json::{Value, json};

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";

#[derive(Default, Clone, Copy)]
pub struct Usage {
    pub input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub output: u64,
    pub duration_ms: u64,
}

impl Usage {
    pub(super) fn add(&mut self, other: Usage) {
        self.input += other.input;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
        self.output += other.output;
        self.duration_ms += other.duration_ms;
    }

    pub(super) fn to_json(self) -> Value {
        json!({
            "input": self.input,
            "output": self.output,
            "cache_read": self.cache_read,
            "cache_write": self.cache_write,
            "duration_ms": self.duration_ms,
        })
    }

    pub(super) fn update_from_response(&mut self, value: &Value) {
        let fields = [
            (&mut self.input, "input_tokens"),
            (&mut self.output, "output_tokens"),
            (&mut self.cache_read, "cache_read_input_tokens"),
            (&mut self.cache_write, "cache_creation_input_tokens"),
        ];
        for (field, key) in fields {
            if let Some(count) = value[key].as_u64() {
                *field = count;
            }
        }
    }

    pub(super) fn from_json(value: &Value) -> Self {
        Self {
            input: value["input"].as_u64().unwrap_or(0),
            output: value["output"].as_u64().unwrap_or(0),
            cache_read: value["cache_read"].as_u64().unwrap_or(0),
            cache_write: value["cache_write"].as_u64().unwrap_or(0),
            duration_ms: value["duration_ms"].as_u64().unwrap_or(0),
        }
    }
}

pub struct Stats {
    pub usage: Usage,
    pub cache_hit_rate: Option<f64>,
    pub tokens_per_second: Option<f64>,
    pub context_tokens: u64,
    pub context_window: u64,
    pub quota: Quota,
}

#[derive(Default, Clone, Copy)]
pub struct Quota {
    pub five_hour_remaining: Option<f64>,
    pub seven_day_remaining: Option<f64>,
    pub five_hour_reset: Option<u64>,
    pub seven_day_reset: Option<u64>,
}

impl Quota {
    pub(super) fn update_from_headers(&mut self, headers: &reqwest::header::HeaderMap) {
        let remaining = |window: &str| {
            headers
                .get(format!("anthropic-ratelimit-unified-{window}-utilization"))
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<f64>().ok())
                .map(|utilisation| (1.0 - utilisation) * 100.0)
        };
        let reset = |window: &str| {
            headers
                .get(format!("anthropic-ratelimit-unified-{window}-reset"))
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
        };
        self.five_hour_remaining = remaining("5h").or(self.five_hour_remaining);
        self.seven_day_remaining = remaining("7d").or(self.seven_day_remaining);
        self.five_hour_reset = reset("5h").or(self.five_hour_reset);
        self.seven_day_reset = reset("7d").or(self.seven_day_reset);
    }
}

fn parse_timestamp(text: &str) -> Option<u64> {
    let number = |range: std::ops::Range<usize>| text.get(range)?.parse::<i64>().ok();
    let year = number(0..4)?;
    let month = number(5..7)?;
    let day = number(8..10)?;
    let hour = number(11..13)?;
    let minute = number(14..16)?;
    let second = number(17..19)?;
    let zone = &text[text.get(19..)?.find(['Z', '+', '-'])? + 19..];
    let offset = match zone.as_bytes().first()? {
        b'Z' => 0,
        sign => {
            let hours: i64 = zone.get(1..3)?.parse().ok()?;
            let minutes: i64 = zone.get(4..6)?.parse().ok()?;
            let offset = hours * 3_600 + minutes * 60;
            if *sign == b'-' { -offset } else { offset }
        }
    };
    let shifted_year = if month <= 2 { year - 1 } else { year };
    let era = shifted_year.div_euclid(400);
    let year_of_era = shifted_year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    u64::try_from(days * 86_400 + hour * 3_600 + minute * 60 + second - offset).ok()
}

pub(super) fn cache_hit_rate(messages: &[Value]) -> Option<f64> {
    messages
        .iter()
        .rfind(|message| message["role"] == "assistant" && message.get("stop_reason").is_none())
        .map(|message| Usage::from_json(&message["usage"]))
        .and_then(|usage| {
            let prompt_tokens = usage.input + usage.cache_read + usage.cache_write;
            (prompt_tokens > 0).then(|| usage.cache_read as f64 / prompt_tokens as f64 * 100.0)
        })
}

pub(super) fn tokens_per_second(messages: &[Value]) -> Option<f64> {
    let mut timed = Usage::default();
    for message in messages
        .iter()
        .filter(|message| message["role"] == "assistant")
    {
        let usage = Usage::from_json(&message["usage"]);
        if usage.duration_ms > 0 {
            timed.add(usage);
        }
    }
    (timed.duration_ms > 0).then(|| timed.output as f64 * 1_000.0 / timed.duration_ms as f64)
}

pub(super) fn total_usage(messages: &[Value]) -> Usage {
    let mut total = Usage::default();
    for message in messages
        .iter()
        .filter(|message| message["role"] == "assistant")
    {
        total.add(Usage::from_json(&message["usage"]));
    }
    total
}

pub(super) async fn fetch_quota(http: reqwest::Client, token: String) -> Result<Quota> {
    let response = http
        .get(USAGE_URL)
        .bearer_auth(token)
        .header("anthropic-beta", "oauth-2025-04-20")
        .send()
        .await?;
    if !response.status().is_success() {
        let status = response.status();
        bail!("{status}: {}", response.text().await?);
    }
    let body: Value = response.json().await?;
    let remaining = |window: &str| {
        body[window]["utilization"]
            .as_f64()
            .map(|utilisation| 100.0 - utilisation)
    };
    let reset = |window: &str| body[window]["resets_at"].as_str().and_then(parse_timestamp);
    Ok(Quota {
        five_hour_remaining: remaining("five_hour"),
        seven_day_remaining: remaining("seven_day"),
        five_hour_reset: reset("five_hour"),
        seven_day_reset: reset("seven_day"),
    })
}

#[cfg(test)]
mod tests {
    use super::{Quota, cache_hit_rate, parse_timestamp, tokens_per_second};
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
    fn ignores_discarded_usage_in_cache_hit_rate() {
        let messages = vec![
            json!({ "role": "user", "content": "hello" }),
            json!({
                "role": "assistant",
                "content": [{ "type": "text", "text": "hi" }],
                "usage": { "input": 10, "output": 5, "cache_read": 90, "cache_write": 0 },
            }),
            json!({
                "role": "assistant",
                "stop_reason": "aborted",
                "content": [],
                "usage": { "input": 999 },
            }),
        ];
        assert_eq!(cache_hit_rate(&messages), Some(90.0));
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
}
