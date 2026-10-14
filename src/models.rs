use serde_json::{Value, json};

const DEFAULT_MAX_OUTPUT: u64 = 8192;
const CONTEXT_WINDOW: u64 = 1_000_000;
const MAX_OUTPUT: u64 = 128_000;

const MODELS: &[(&str, Option<&str>)] = &[
    ("claude-fable-5-1", None),
    ("claude-opus-5-5", None),
    ("claude-sonnet-5-5", Some("between_tools")),
    ("claude-haiku-5-5", Some("disabled")),
];

fn find(model: &str) -> Option<&'static (&'static str, Option<&'static str>)> {
    MODELS.iter().find(|(id, _)| *id == model)
}

pub const DEFAULT: &str = "claude-opus-5-5";

pub fn known() -> impl Iterator<Item = &'static str> {
    MODELS.iter().map(|(id, _)| *id)
}

pub fn is_known(model: &str) -> bool {
    find(model).is_some()
}

pub fn unknown(model: &str) -> String {
    format!(
        "unknown model: {model} (options: {})",
        known().collect::<Vec<_>>().join(", ")
    )
}

pub fn next_known(model: &str) -> &'static str {
    let index = MODELS
        .iter()
        .position(|(id, _)| *id == model)
        .map_or(0, |index| (index + 1) % MODELS.len());
    MODELS[index].0
}

pub fn context_window(model: &str) -> u64 {
    if is_known(model) { CONTEXT_WINDOW } else { 0 }
}

pub fn max_output(model: &str) -> u64 {
    if is_known(model) {
        MAX_OUTPUT
    } else {
        DEFAULT_MAX_OUTPUT
    }
}

const EFFORT_LEVELS: [&str; 6] = ["off", "low", "medium", "high", "xhigh", "max"];

fn thinking_off(model: &str) -> Option<&'static str> {
    find(model).and_then(|(_, off)| *off)
}

pub fn effort_levels(model: &str) -> &'static [&'static str] {
    if thinking_off(model).is_some() {
        &EFFORT_LEVELS
    } else {
        &EFFORT_LEVELS[1..]
    }
}

pub const DEFAULT_EFFORT: &str = "medium";

pub fn find_effort(model: &str, name: &str) -> Option<&'static str> {
    effort_levels(model)
        .iter()
        .copied()
        .find(|level| *level == name)
}

/// `effort`, or the default when `model` does not support it.
pub fn supported_effort(model: &str, effort: &str) -> &'static str {
    find_effort(model, effort).unwrap_or(DEFAULT_EFFORT)
}

/// The saved model, or the default with a warning when it is not listed.
pub fn choose(saved: Option<String>) -> (String, Option<String>) {
    match saved {
        Some(model) if !is_known(&model) => (
            DEFAULT.into(),
            Some(format!("{}, using {DEFAULT}", unknown(&model))),
        ),
        saved => (saved.unwrap_or_else(|| DEFAULT.into()), None),
    }
}

/// The named effort level, or the default with a warning when it is unknown.
pub fn choose_effort(model: &str, name: &str) -> (&'static str, Option<String>) {
    match find_effort(model, name) {
        Some(level) => (level, None),
        None if name == "off" => (DEFAULT_EFFORT, None),
        None => (
            DEFAULT_EFFORT,
            Some(format!(
                "unknown effort level: {name} (options: {}), using {DEFAULT_EFFORT}",
                effort_levels(model).join(", ")
            )),
        ),
    }
}

pub fn thinking_settings(model: &str, level: &str) -> Value {
    if let Some(kind) = thinking_off(model).filter(|_| level == "off") {
        return json!({ "thinking": { "type": kind } });
    }
    json!({
        "thinking": { "type": "adaptive", "display": "summarized" },
        "output_config": { "effort": level },
    })
}

#[cfg(test)]
mod tests;
