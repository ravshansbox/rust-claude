use serde_json::{Value, json};

const DEFAULT_MAX_OUTPUT: u64 = 8192;

struct Model {
    id: &'static str,
    context_window: u64,
    max_output: u64,
    thinking_off: Option<&'static str>,
}

const MODELS: &[Model] = &[
    Model {
        id: "claude-fable-5-1",
        context_window: 1_000_000,
        max_output: 128_000,
        thinking_off: None,
    },
    Model {
        id: "claude-opus-5-5",
        context_window: 1_000_000,
        max_output: 128_000,
        thinking_off: None,
    },
    Model {
        id: "claude-sonnet-5-5",
        context_window: 1_000_000,
        max_output: 128_000,
        thinking_off: Some("between_tools"),
    },
    Model {
        id: "claude-haiku-5-5",
        context_window: 1_000_000,
        max_output: 128_000,
        thinking_off: Some("disabled"),
    },
];

fn find(model: &str) -> Option<&'static Model> {
    MODELS.iter().find(|candidate| candidate.id == model)
}

pub const DEFAULT: &str = "claude-opus-5-5";

pub fn known() -> impl Iterator<Item = &'static str> {
    MODELS.iter().map(|model| model.id)
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
        .position(|candidate| candidate.id == model)
        .map_or(0, |index| (index + 1) % MODELS.len());
    MODELS[index].id
}

pub fn context_window(model: &str) -> u64 {
    find(model).map_or(0, |model| model.context_window)
}

pub fn max_output(model: &str) -> u64 {
    find(model).map_or(DEFAULT_MAX_OUTPUT, |model| model.max_output)
}

const EFFORT_LEVELS: [&str; 6] = ["off", "low", "medium", "high", "xhigh", "max"];

fn thinking_off(model: &str) -> Option<&'static str> {
    find(model).and_then(|model| model.thinking_off)
}

pub fn effort_levels(model: &str) -> &'static [&'static str] {
    if thinking_off(model).is_some() {
        &EFFORT_LEVELS
    } else {
        &EFFORT_LEVELS[1..]
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
