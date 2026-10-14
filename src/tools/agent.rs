use super::{argument, plural, tool};
use serde_json::{Value, json};

pub const NAME: &str = "agent";

pub fn definition() -> Value {
    tool(
        NAME,
        "Start subagents to work on tasks on their own. Each subagent starts a new conversation with the same instructions and tools as you, except this one, and returns only its final reply. Give each task a prompt with all the context it needs. Mode \"parallel\" runs the tasks at the same time. Mode \"sequential\" runs them one after another, and adds each task's final reply to the next task's prompt.",
        &json!({
            "tasks": {
                "type": "array",
                "minItems": 1,
                "items": {
                    "type": "object",
                    "properties": {
                        "description": {
                            "type": "string",
                            "description": "A short label for the task, in 3 to 5 words"
                        },
                        "prompt": {
                            "type": "string",
                            "description": "The full task for the subagent"
                        }
                    },
                    "required": ["description", "prompt"]
                }
            },
            "mode": { "type": "string", "enum": ["sequential", "parallel"] }
        }),
        &["tasks", "mode"],
    )
}

pub fn summary(input: &Value) -> String {
    let mode = input["mode"].as_str().unwrap_or_default();
    let count = input["tasks"].as_array().map_or(0, Vec::len);
    format!("{mode}, {}", plural(count, "task"))
}

pub fn tool_calls(count: usize) -> String {
    plural(count, "tool call")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Sequential,
    Parallel,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Task {
    pub description: String,
    pub prompt: String,
}

pub fn parse(input: &Value) -> Result<(Mode, Vec<Task>), String> {
    let mode = match argument(input, "mode")? {
        "sequential" => Mode::Sequential,
        "parallel" => Mode::Parallel,
        other => return Err(format!("unknown mode: {other}")),
    };
    let tasks = input["tasks"]
        .as_array()
        .ok_or("missing argument: tasks")?
        .iter()
        .map(|task| {
            Ok(Task {
                description: argument(task, "description")?.to_string(),
                prompt: argument(task, "prompt")?.to_string(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    if tasks.is_empty() {
        return Err("tasks must not be empty".into());
    }
    Ok((mode, tasks))
}
