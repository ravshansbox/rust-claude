use serde_json::{Value, json};

const DEFAULT_MAX_OUTPUT: u64 = 8192;

#[derive(Clone, Copy)]
enum Thinking {
    Adaptive { xhigh: bool },
    Budget { effort: bool },
}

struct Model {
    id: &'static str,
    context_window: u64,
    max_output: u64,
    thinking: Thinking,
}

const MODELS: &[Model] = &[
    Model {
        id: "claude-fable-5",
        context_window: 1_000_000,
        max_output: 128_000,
        thinking: Thinking::Adaptive { xhigh: true },
    },
    Model {
        id: "claude-fable-5-1",
        context_window: 1_000_000,
        max_output: 128_000,
        thinking: Thinking::Adaptive { xhigh: true },
    },
    Model {
        id: "claude-haiku-4-5",
        context_window: 200_000,
        max_output: 64_000,
        thinking: Thinking::Budget { effort: false },
    },
    Model {
        id: "claude-haiku-4-5-20251001",
        context_window: 200_000,
        max_output: 64_000,
        thinking: Thinking::Budget { effort: false },
    },
    Model {
        id: "claude-haiku-5-5",
        context_window: 1_000_000,
        max_output: 128_000,
        thinking: Thinking::Adaptive { xhigh: true },
    },
    Model {
        id: "claude-opus-4-5",
        context_window: 200_000,
        max_output: 64_000,
        thinking: Thinking::Budget { effort: true },
    },
    Model {
        id: "claude-opus-4-5-20251101",
        context_window: 200_000,
        max_output: 64_000,
        thinking: Thinking::Budget { effort: true },
    },
    Model {
        id: "claude-opus-4-6",
        context_window: 1_000_000,
        max_output: 128_000,
        thinking: Thinking::Adaptive { xhigh: false },
    },
    Model {
        id: "claude-opus-4-7",
        context_window: 1_000_000,
        max_output: 128_000,
        thinking: Thinking::Adaptive { xhigh: true },
    },
    Model {
        id: "claude-opus-4-8",
        context_window: 1_000_000,
        max_output: 128_000,
        thinking: Thinking::Adaptive { xhigh: true },
    },
    Model {
        id: "claude-opus-5",
        context_window: 1_000_000,
        max_output: 128_000,
        thinking: Thinking::Adaptive { xhigh: true },
    },
    Model {
        id: "claude-opus-5-5",
        context_window: 1_000_000,
        max_output: 128_000,
        thinking: Thinking::Adaptive { xhigh: true },
    },
    Model {
        id: "claude-sonnet-4-5",
        context_window: 200_000,
        max_output: 64_000,
        thinking: Thinking::Budget { effort: false },
    },
    Model {
        id: "claude-sonnet-4-5-20250929",
        context_window: 200_000,
        max_output: 64_000,
        thinking: Thinking::Budget { effort: false },
    },
    Model {
        id: "claude-sonnet-4-6",
        context_window: 1_000_000,
        max_output: 128_000,
        thinking: Thinking::Adaptive { xhigh: false },
    },
    Model {
        id: "claude-sonnet-5",
        context_window: 1_000_000,
        max_output: 128_000,
        thinking: Thinking::Adaptive { xhigh: true },
    },
    Model {
        id: "claude-sonnet-5-5",
        context_window: 1_000_000,
        max_output: 128_000,
        thinking: Thinking::Adaptive { xhigh: true },
    },
];

fn find(model: &str) -> Option<&'static Model> {
    MODELS.iter().find(|candidate| candidate.id == model)
}

pub fn context_window(model: &str) -> u64 {
    find(model).map_or(0, |model| model.context_window)
}

pub fn max_output(model: &str) -> u64 {
    find(model).map_or(DEFAULT_MAX_OUTPUT, |model| model.max_output)
}

pub fn thinking_settings(model: &str, level: &str) -> Value {
    let thinking = find(model).map_or(Thinking::Adaptive { xhigh: true }, |model| model.thinking);
    let Thinking::Budget { effort } = thinking else {
        let effort = match (thinking, level) {
            (Thinking::Adaptive { xhigh: false }, "xhigh") => "high",
            _ => level,
        };
        return json!({
            "thinking": { "type": "adaptive", "display": "summarized" },
            "output_config": { "effort": effort },
        });
    };
    let budget = match level {
        "low" => 4_000,
        "medium" => 16_000,
        "high" => 32_000,
        _ => max_output(model) - 1,
    };
    let mut settings = json!({ "thinking": { "type": "enabled", "budget_tokens": budget } });
    if effort && matches!(level, "low" | "medium" | "high") {
        settings["output_config"] = json!({ "effort": level });
    }
    settings
}

#[cfg(test)]
mod tests {
    use super::thinking_settings;
    use serde_json::json;

    #[test]
    fn uses_adaptive_thinking_and_effort_on_newer_models() {
        assert_eq!(
            thinking_settings("claude-opus-4-6", "high"),
            json!({
                "thinking": { "type": "adaptive", "display": "summarized" },
                "output_config": { "effort": "high" },
            })
        );
        assert_eq!(
            thinking_settings("claude-unknown", "low")["thinking"]["type"],
            "adaptive"
        );
    }

    #[test]
    fn sends_high_effort_for_xhigh_on_models_without_it() {
        for model in ["claude-opus-4-6", "claude-sonnet-4-6"] {
            let effort = |level| thinking_settings(model, level)["output_config"]["effort"].clone();
            assert_eq!(effort("xhigh"), "high", "{model}");
            assert_eq!(effort("max"), "max", "{model}");
        }
        assert_eq!(
            thinking_settings("claude-opus-4-7", "xhigh")["output_config"]["effort"],
            "xhigh"
        );
    }

    #[test]
    fn uses_thinking_budget_on_older_models() {
        let budgets = [
            ("low", 4_000),
            ("medium", 16_000),
            ("high", 32_000),
            ("xhigh", 63_999),
            ("max", 63_999),
        ];
        for (level, budget) in budgets {
            assert_eq!(
                thinking_settings("claude-haiku-4-5", level),
                json!({ "thinking": { "type": "enabled", "budget_tokens": budget } }),
                "{level}"
            );
        }
    }

    #[test]
    fn adds_supported_effort_on_opus_4_5() {
        assert_eq!(
            thinking_settings("claude-opus-4-5", "medium"),
            json!({
                "thinking": { "type": "enabled", "budget_tokens": 16_000 },
                "output_config": { "effort": "medium" },
            })
        );
        assert_eq!(
            thinking_settings("claude-opus-4-5-20251101", "max"),
            json!({ "thinking": { "type": "enabled", "budget_tokens": 63_999 } })
        );
    }
}
