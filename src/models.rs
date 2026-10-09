struct Rates {
    input: f64,
    output: f64,
    cache_read: f64,
    cache_write: f64,
}

struct Model {
    id: &'static str,
    rates: Rates,
    tier: Option<(u64, Rates)>,
}

const fn rates(input: f64, output: f64, cache_read: f64, cache_write: f64) -> Rates {
    Rates {
        input,
        output,
        cache_read,
        cache_write,
    }
}

const MODELS: &[Model] = &[
    Model {
        id: "claude-fable-5",
        rates: rates(10.0, 50.0, 1.0, 12.5),
        tier: None,
    },
    Model {
        id: "claude-fable-5-1",
        rates: rates(10.0, 50.0, 0.25, 12.5),
        tier: None,
    },
    Model {
        id: "claude-haiku-4-5",
        rates: rates(1.0, 5.0, 0.1, 1.25),
        tier: None,
    },
    Model {
        id: "claude-haiku-4-5-20251001",
        rates: rates(1.0, 5.0, 0.1, 1.25),
        tier: None,
    },
    Model {
        id: "claude-haiku-5-5",
        rates: rates(0.1, 0.5, 0.01, 0.125),
        tier: Some((100_000, rates(0.5, 2.5, 0.05, 0.625))),
    },
    Model {
        id: "claude-opus-4-5",
        rates: rates(5.0, 25.0, 0.5, 6.25),
        tier: None,
    },
    Model {
        id: "claude-opus-4-5-20251101",
        rates: rates(5.0, 25.0, 0.5, 6.25),
        tier: None,
    },
    Model {
        id: "claude-opus-4-6",
        rates: rates(5.0, 25.0, 0.5, 6.25),
        tier: None,
    },
    Model {
        id: "claude-opus-4-7",
        rates: rates(5.0, 25.0, 0.5, 6.25),
        tier: None,
    },
    Model {
        id: "claude-opus-4-8",
        rates: rates(5.0, 25.0, 0.5, 6.25),
        tier: None,
    },
    Model {
        id: "claude-opus-5",
        rates: rates(5.0, 25.0, 0.5, 6.25),
        tier: None,
    },
    Model {
        id: "claude-opus-5-5",
        rates: rates(4.0, 20.0, 0.2, 5.0),
        tier: None,
    },
    Model {
        id: "claude-sonnet-4-5",
        rates: rates(3.0, 15.0, 0.3, 3.75),
        tier: None,
    },
    Model {
        id: "claude-sonnet-4-5-20250929",
        rates: rates(3.0, 15.0, 0.3, 3.75),
        tier: None,
    },
    Model {
        id: "claude-sonnet-4-6",
        rates: rates(3.0, 15.0, 0.3, 3.75),
        tier: None,
    },
    Model {
        id: "claude-sonnet-5",
        rates: rates(2.0, 10.0, 0.2, 2.5),
        tier: None,
    },
    Model {
        id: "claude-sonnet-5-5",
        rates: rates(2.0, 10.0, 0.2, 2.5),
        tier: None,
    },
];

pub fn cost(model: &str, input: u64, output: u64, cache_read: u64, cache_write: u64) -> f64 {
    let Some(model) = MODELS.iter().find(|candidate| candidate.id == model) else {
        return 0.0;
    };
    let rates = match &model.tier {
        Some((threshold, tier)) if input + cache_read + cache_write > *threshold => tier,
        _ => &model.rates,
    };
    (rates.input * input as f64
        + rates.output * output as f64
        + rates.cache_read * cache_read as f64
        + rates.cache_write * cache_write as f64)
        / 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::cost;

    fn assert_close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
    }

    #[test]
    fn calculates_cost() {
        assert_close(cost("claude-opus-5-5", 1_000_000, 0, 0, 0), 4.0);
        assert_close(
            cost("claude-opus-5-5", 0, 1_000_000, 1_000_000, 1_000_000),
            25.2,
        );
        assert_close(cost("unknown", 1_000_000, 0, 0, 0), 0.0);
    }

    #[test]
    fn uses_tier_above_threshold() {
        assert_close(cost("claude-haiku-5-5", 100_000, 0, 0, 0), 0.01);
        assert_close(cost("claude-haiku-5-5", 100_001, 0, 0, 0), 0.0500005);
    }
}
