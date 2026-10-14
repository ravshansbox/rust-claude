use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

#[derive(Default, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct Usage {
    pub input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub output: u64,
    pub duration_ms: u64,
}

impl Usage {
    pub(super) fn add(&mut self, other: Self) {
        self.input += other.input;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
        self.output += other.output;
        self.duration_ms += other.duration_ms;
    }

    pub(super) fn tokens(self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write
    }

    /// Stores a reply's usage with that of its retried attempts nested
    /// under `retried`, which only totals count: the context size and speed
    /// come from the attempt that produced the reply.
    pub(super) fn to_json_with_retried(self, retried: Self) -> Value {
        let mut value = json!(self);
        if retried.tokens() > 0 {
            value["retried"] = json!(retried);
        }
        value
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
        Self::deserialize(value).unwrap_or_default()
    }
}

#[derive(Default, Clone, Copy)]
pub struct Stats {
    pub usage: Usage,
    pub tokens_per_second: Option<f64>,
    pub context_tokens: Option<u64>,
    pub context_window: u64,
    pub quota: Quota,
    /// Background commands running or waiting to be answered.
    pub background: usize,
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
        let header = |window: &str, field: &str| {
            headers
                .get(format!("anthropic-ratelimit-unified-{window}-{field}"))
                .and_then(|value| value.to_str().ok())
        };
        let remaining = |window: &str| {
            header(window, "utilization")
                .and_then(|value| value.parse::<f64>().ok())
                .map(|utilisation| (1.0 - utilisation) * 100.0)
        };
        let reset =
            |window: &str| header(window, "reset").and_then(|value| value.parse::<u64>().ok());
        self.five_hour_remaining = remaining("5h").or(self.five_hour_remaining);
        self.seven_day_remaining = remaining("7d").or(self.seven_day_remaining);
        self.five_hour_reset = reset("5h").or(self.five_hour_reset);
        self.seven_day_reset = reset("7d").or(self.seven_day_reset);
    }
}

fn parse_timestamp(text: &str) -> Option<u64> {
    let time = OffsetDateTime::parse(text, &Rfc3339).ok()?;
    u64::try_from(time.unix_timestamp()).ok()
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

/// Tokens used by replies, and by subagents, whose tokens are kept with the
/// tool results they produced.
pub(super) fn total_usage(messages: &[Value]) -> Usage {
    let mut total = Usage::default();
    for message in messages {
        total.add(Usage::from_json(&message["usage"]));
        total.add(Usage::from_json(&message["usage"]["retried"]));
    }
    total
}

pub(super) async fn fetch_quota(
    http: reqwest::Client,
    url: String,
    token: String,
) -> Result<Quota> {
    let response = http
        .get(url)
        .bearer_auth(token)
        .header("anthropic-beta", super::OAUTH_BETA)
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
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
mod tests;
