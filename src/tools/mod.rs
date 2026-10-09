use serde_json::{Value, json};

mod bash;
mod display;
mod edit;
mod read;
#[cfg(test)]
mod test_support;
mod write;

pub use bash::ProcessGroup;
pub use display::{ReadGroup, diff, note, summary};

const MAX_OUTPUT: usize = 20_000;

pub fn truncate(mut text: String) -> String {
    if text.len() > MAX_OUTPUT {
        let mut end = MAX_OUTPUT;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str("\n… output truncated");
    }
    text
}

pub fn definitions() -> Value {
    json!([
        bash::definition(),
        read::definition(),
        write::definition(),
        edit::definition()
    ])
}

fn argument<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
    input[key]
        .as_str()
        .ok_or_else(|| format!("missing argument: {key}"))
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
mod tests {
    use super::{MAX_OUTPUT, call};
    use serde_json::json;

    #[tokio::test]
    async fn rejects_invalid_numbers() {
        let cases = [
            (
                "bash",
                json!({ "command": "echo hi", "timeout": 0 }),
                "timeout must be at least 1",
            ),
            (
                "bash",
                json!({ "command": "echo hi", "timeout": -1 }),
                "timeout must be at least 1",
            ),
            (
                "read",
                json!({ "path": "Cargo.toml", "limit": 0 }),
                "limit must be at least 1",
            ),
            (
                "read",
                json!({ "path": "Cargo.toml", "limit": -1 }),
                "limit must be at least 1",
            ),
            (
                "read",
                json!({ "path": "Cargo.toml", "offset": -5 }),
                "offset must be at least 1",
            ),
        ];
        for (name, input, error) in cases {
            assert_eq!(call(name, &input).await, Err(error.into()), "{input}");
        }
    }

    #[test]
    fn truncates_inside_multibyte_character() {
        let text = super::truncate(format!("a{}", "é".repeat(MAX_OUTPUT)));
        assert!(text.ends_with("\n… output truncated"));
        assert_eq!(text.len(), MAX_OUTPUT - 1 + "\n… output truncated".len());
    }
}
