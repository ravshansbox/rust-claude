use super::{argument, bash};
use serde_json::{Value, json};

pub(super) fn definition() -> Value {
    json!({
        "name": "python",
        "description": "Run Python code with python3 in the current project and return stdout and stderr",
        "input_schema": {
            "type": "object",
            "properties": {
                "code": { "type": "string" },
                "timeout": { "type": "integer", "description": "Seconds before the code is killed. No limit when omitted" }
            },
            "required": ["code"]
        }
    })
}

pub(super) async fn run(input: &Value) -> Result<String, String> {
    let timeout = bash::timeout(input)?;
    let code = argument(input, "code")?.to_string();
    let mut command = tokio::process::Command::new("python3");
    command.arg("-");
    bash::run_command(command, Some(code), timeout).await
}

#[cfg(test)]
mod tests {
    use crate::tools::call;
    use serde_json::json;

    #[tokio::test]
    async fn runs_python_code() {
        let input = json!({ "code": "import sys\nfor number in range(2):\n    print(number)\nprint(sys.argv[0])" });
        assert_eq!(call("python", &input).await, Ok("0\n1\n-\n".into()));
    }

    #[tokio::test]
    async fn reports_python_errors_and_exit_status() {
        let input = json!({ "code": "import sys\nsys.stderr.write('problem')\nsys.exit(3)" });
        assert_eq!(
            call("python", &input).await,
            Ok("\nstderr:\nproblem\nexit status: 3".into())
        );
    }

    #[tokio::test]
    async fn times_out_python_code() {
        let input = json!({ "code": "import time\ntime.sleep(5)", "timeout": 1 });
        assert_eq!(
            call("python", &input).await,
            Err("command timed out after 1s".into())
        );
    }

    #[tokio::test]
    async fn requires_python_code() {
        assert_eq!(
            call("python", &json!({})).await,
            Err("missing argument: code".into())
        );
    }
}
