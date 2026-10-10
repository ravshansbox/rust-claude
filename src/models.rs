use serde_json::{Value, json};
use std::cmp::Reverse;

const DEFAULT_MAX_OUTPUT: u64 = 8192;

struct Model {
    id: &'static str,
    context_window: u64,
    max_output: u64,
}

const MODELS: &[Model] = &[
    Model {
        id: "claude-fable-5-1",
        context_window: 1_000_000,
        max_output: 128_000,
    },
    Model {
        id: "claude-opus-5-5",
        context_window: 1_000_000,
        max_output: 128_000,
    },
    Model {
        id: "claude-sonnet-5-5",
        context_window: 1_000_000,
        max_output: 128_000,
    },
    Model {
        id: "claude-haiku-5-5",
        context_window: 1_000_000,
        max_output: 128_000,
    },
];

fn find(model: &str) -> Option<&'static Model> {
    MODELS.iter().find(|candidate| candidate.id == model)
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

pub fn thinking_settings(level: &str) -> Value {
    json!({
        "thinking": { "type": "adaptive", "display": "summarized" },
        "output_config": { "effort": level },
    })
}

const CLASSES: [&str; 4] = ["fable", "opus", "sonnet", "haiku"];

pub fn latest_in_each_class(models: Vec<String>) -> Vec<String> {
    CLASSES
        .iter()
        .filter_map(|class| {
            let prefix = format!("claude-{class}-");
            models
                .iter()
                .filter_map(|model| Some((model, model.strip_prefix(&prefix)?)))
                .max_by_key(|(_, version)| {
                    let parts: Vec<&str> = version.split('-').collect();
                    let dated = parts.iter().any(|part| part.len() == 8);
                    let numbers: Vec<u64> = parts
                        .iter()
                        .filter(|part| part.len() != 8)
                        .filter_map(|part| part.parse().ok())
                        .collect();
                    (numbers, Reverse(dated))
                })
                .map(|(model, _)| model.clone())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{context_window, max_output, thinking_settings};
    use serde_json::json;

    #[test]
    fn knows_only_the_latest_model_in_each_class() {
        for model in [
            "claude-fable-5-1",
            "claude-opus-5-5",
            "claude-sonnet-5-5",
            "claude-haiku-5-5",
        ] {
            assert_eq!(context_window(model), 1_000_000, "{model}");
            assert_eq!(max_output(model), 128_000, "{model}");
        }
        for model in [
            "claude-fable-5",
            "claude-opus-4-6",
            "claude-sonnet-4-5",
            "claude-haiku-4-5-20251001",
        ] {
            assert_eq!(context_window(model), 0, "{model}");
            assert_eq!(max_output(model), 8192, "{model}");
        }
    }

    #[test]
    fn uses_adaptive_thinking_with_the_level_as_effort() {
        for level in ["low", "medium", "high", "xhigh", "max"] {
            assert_eq!(
                thinking_settings(level),
                json!({
                    "thinking": { "type": "adaptive", "display": "summarized" },
                    "output_config": { "effort": level },
                })
            );
        }
    }
}
