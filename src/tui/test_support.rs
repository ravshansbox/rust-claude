use super::App;
use crate::agent::{Quota, Stats, Usage};

pub(super) fn new_app() -> App {
    let stats = Stats {
        usage: Usage::default(),
        cache_hit_rate: None,
        tokens_per_second: None,
        context_tokens: 0,
        context_window: 0,
        quota: Quota::default(),
    };
    App::new("model", "medium", stats)
}
