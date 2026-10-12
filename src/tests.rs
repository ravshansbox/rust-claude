use super::{choose_effort, choose_model};

#[test]
fn chooses_an_effort_level_the_model_supports() {
    assert_eq!(choose_effort("claude-haiku-5-5", "off"), ("off", None));
    assert_eq!(choose_effort("claude-opus-5-5", "high"), ("high", None));
    assert_eq!(choose_effort("claude-opus-5-5", "off"), ("medium", None));
    assert_eq!(
        choose_effort("claude-opus-5-5", "loud"),
        (
            "medium",
            Some(
                "unknown effort level: loud (options: low, medium, high, xhigh, max), using medium"
                    .into()
            )
        )
    );
}

#[test]
fn uses_the_default_model_when_the_saved_one_is_not_in_the_list() {
    assert_eq!(
        choose_model(Some("claude-haiku-5-5".into())),
        ("claude-haiku-5-5".into(), None)
    );
    assert_eq!(choose_model(None), ("claude-opus-5-5".into(), None));
    assert_eq!(
        choose_model(Some("claude-opus-4-5".into())),
        (
            "claude-opus-5-5".into(),
            Some(
                "unknown model: claude-opus-4-5 (options: claude-fable-5-1, claude-opus-5-5, claude-sonnet-5-5, claude-haiku-5-5), using claude-opus-5-5"
                    .into()
            )
        )
    );
}
