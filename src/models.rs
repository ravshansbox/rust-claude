use serde_json::{Value, json};
use std::cmp::Reverse;

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
    match thinking_off(model) {
        Some(_) => &EFFORT_LEVELS,
        None => &EFFORT_LEVELS[1..],
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
    use super::{context_window, effort_levels, max_output, thinking_settings};
    use serde_json::json;

    const MODELS: [&str; 4] = [
        "claude-fable-5-1",
        "claude-opus-5-5",
        "claude-sonnet-5-5",
        "claude-haiku-5-5",
    ];

    #[test]
    fn knows_the_latest_model_in_each_class() {
        for model in MODELS {
            assert_eq!(context_window(model), 1_000_000, "{model}");
            assert_eq!(max_output(model), 128_000, "{model}");
        }
    }

    #[test]
    fn uses_adaptive_thinking_with_the_level_as_effort() {
        for model in MODELS.into_iter().chain(["claude-x"]) {
            for level in ["low", "medium", "high", "xhigh", "max"] {
                assert_eq!(
                    thinking_settings(model, level),
                    json!({
                        "thinking": { "type": "adaptive", "display": "summarized" },
                        "output_config": { "effort": level },
                    }),
                    "{model}"
                );
            }
        }
    }

    #[test]
    fn offers_off_only_on_models_that_can_turn_thinking_off() {
        let levels = ["low", "medium", "high", "xhigh", "max"];
        let with_off = ["off", "low", "medium", "high", "xhigh", "max"];
        assert_eq!(effort_levels("claude-fable-5-1"), levels);
        assert_eq!(effort_levels("claude-opus-5-5"), levels);
        assert_eq!(effort_levels("claude-sonnet-5-5"), with_off);
        assert_eq!(effort_levels("claude-haiku-5-5"), with_off);
        assert_eq!(effort_levels("claude-x"), levels);
    }

    #[test]
    fn turns_thinking_off_without_an_effort() {
        assert_eq!(
            thinking_settings("claude-sonnet-5-5", "off"),
            json!({ "thinking": { "type": "between_tools" } })
        );
        assert_eq!(
            thinking_settings("claude-haiku-5-5", "off"),
            json!({ "thinking": { "type": "disabled" } })
        );
    }
}
