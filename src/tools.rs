use serde_json::{Value, json};

pub mod agent;
pub mod background;
mod bash;
mod display;
mod edit;
mod files;
mod read;
pub mod sources;
#[cfg(test)]
pub(crate) mod test_support;
pub mod web_fetch;
pub mod web_search;
mod write;

pub use bash::{ProcessGroup, new_session};
pub use display::{ReadGroup, diff, note, summary};
use files::{failed, regular_file, replace_file};

const MAX_OUTPUT: usize = 20_000;

pub fn truncate(mut text: String) -> String {
    if text.len() > MAX_OUTPUT {
        text.truncate(text.floor_char_boundary(MAX_OUTPUT));
        text.push_str("\n… output truncated");
    }
    text
}

pub fn definitions() -> Value {
    json!([
        bash::definition(),
        read::definition(),
        write::definition(),
        edit::definition(),
        agent::definition(),
        background::stop_definition(),
        web_fetch::definition(),
        web_search::definition()
    ])
}

fn tool(name: &str, description: &str, properties: &Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "input_schema": { "type": "object", "properties": properties, "required": required }
    })
}

pub(crate) fn plural(count: usize, noun: &str) -> String {
    match count {
        1 => format!("1 {noun}"),
        _ => format!("{count} {noun}s"),
    }
}

fn argument<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
    input[key]
        .as_str()
        .ok_or_else(|| format!("missing argument: {key}"))
}

fn number(input: &Value, key: &str, min: u64) -> Result<Option<u64>, String> {
    match &input[key] {
        Value::Null => Ok(None),
        value if !value.is_i64() && !value.is_u64() => Err(format!("{key} must be a whole number")),
        value => value
            .as_u64()
            .filter(|number| *number >= min)
            .map(Some)
            .ok_or_else(|| format!("{key} must be at least {min}")),
    }
}

pub async fn call(name: &str, input: &Value) -> Result<String, String> {
    match name {
        "bash" => bash::run(input).await,
        "read" => read::run(input).await,
        "write" => write::run(input).await,
        "edit" => edit::run(input).await,
        _ => Err(format!("unknown tool: {name}")),
    }
}

#[cfg(test)]
mod tests;
