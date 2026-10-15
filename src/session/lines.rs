use serde_json::{Value, json};

use super::current_folder;

pub(super) fn header() -> Value {
    json!({ "type": "session", "cwd": current_folder() })
}

pub(super) fn is_header(line: &Value) -> bool {
    line["type"] == "session"
}

/// Records that the session drops thinking blocks whose earlier conversation
/// changed.
pub(super) fn drop_mismatched_thinking_line() -> Value {
    json!({ "type": "thinking_block_binding", "prefix_mismatch_behavior": "drop_block" })
}
