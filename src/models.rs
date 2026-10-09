const DEFAULT_MAX_OUTPUT: u64 = 8192;

struct Model {
    id: &'static str,
    context_window: u64,
    max_output: u64,
}

const MODELS: &[Model] = &[
    Model {
        id: "claude-fable-5",
        context_window: 1_000_000,
        max_output: 128_000,
    },
    Model {
        id: "claude-fable-5-1",
        context_window: 1_000_000,
        max_output: 128_000,
    },
    Model {
        id: "claude-haiku-4-5",
        context_window: 200_000,
        max_output: 64_000,
    },
    Model {
        id: "claude-haiku-4-5-20251001",
        context_window: 200_000,
        max_output: 64_000,
    },
    Model {
        id: "claude-haiku-5-5",
        context_window: 1_000_000,
        max_output: 128_000,
    },
    Model {
        id: "claude-opus-4-5",
        context_window: 200_000,
        max_output: 64_000,
    },
    Model {
        id: "claude-opus-4-5-20251101",
        context_window: 200_000,
        max_output: 64_000,
    },
    Model {
        id: "claude-opus-4-6",
        context_window: 1_000_000,
        max_output: 128_000,
    },
    Model {
        id: "claude-opus-4-7",
        context_window: 1_000_000,
        max_output: 128_000,
    },
    Model {
        id: "claude-opus-4-8",
        context_window: 1_000_000,
        max_output: 128_000,
    },
    Model {
        id: "claude-opus-5",
        context_window: 1_000_000,
        max_output: 128_000,
    },
    Model {
        id: "claude-opus-5-5",
        context_window: 1_000_000,
        max_output: 128_000,
    },
    Model {
        id: "claude-sonnet-4-5",
        context_window: 200_000,
        max_output: 64_000,
    },
    Model {
        id: "claude-sonnet-4-5-20250929",
        context_window: 200_000,
        max_output: 64_000,
    },
    Model {
        id: "claude-sonnet-4-6",
        context_window: 1_000_000,
        max_output: 128_000,
    },
    Model {
        id: "claude-sonnet-5",
        context_window: 1_000_000,
        max_output: 128_000,
    },
    Model {
        id: "claude-sonnet-5-5",
        context_window: 1_000_000,
        max_output: 128_000,
    },
];

pub fn context_window(model: &str) -> u64 {
    MODELS
        .iter()
        .find(|candidate| candidate.id == model)
        .map_or(0, |model| model.context_window)
}

pub fn max_output(model: &str) -> u64 {
    MODELS
        .iter()
        .find(|candidate| candidate.id == model)
        .map_or(DEFAULT_MAX_OUTPUT, |model| model.max_output)
}
