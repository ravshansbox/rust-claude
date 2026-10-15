use super::*;

mod processes;
mod timeouts;

#[tokio::test]
async fn skips_output_lines_that_are_not_utf8() {
    let mcp = started(server_answering_calls_with(
        r#"printf '\377\n{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"pong"}]}}\n' "$id""#,
    ))
    .await;
    assert_eq!(call_test_echo(&mcp).await, Some(Ok("pong".into())));
}

#[tokio::test]
async fn answers_server_requests_and_follows_tool_list_pages() {
    let log = TemporaryFile::new("mcp-replies", "");
    let cases = r#"    *'"result"'*|*'"error"'*) printf '%s\n' "$line" >> 'LOG' ;;
    *'"cursor"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"second"}]}}\n' "$id" ;;
    *'"tools/list"'*)
      printf '{"jsonrpc":"2.0","method":"notifications/message","params":{"level":"info","data":"hi"}}\n'
      printf '{"jsonrpc":"2.0","id":"p1","method":"ping"}\n'
      printf '{"jsonrpc":"2.0","id":%s,"method":"sampling/createMessage","params":{}}\n' "$id"
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo"}],"nextCursor":"next"}}\n' "$id" ;;"#
            .replace("LOG", &log.path().display().to_string());
    let mut mcp = Mcp::default();
    assert_eq!(
        mcp.add(start("test".into(), Scope::Project, server_with(&cases, ":")).await)
            .status,
        "loaded project MCP server: test (2 tools)"
    );
    let names: Vec<Value> = mcp
        .definitions()
        .map(|definition| definition["name"].clone())
        .collect();
    assert_eq!(names, ["mcp__test__echo", "mcp__test__second"]);
    let replies: Vec<Value> = log
        .content()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        replies,
        [
            json!({ "jsonrpc": "2.0", "id": "p1", "result": {} }),
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "error": {
                    "code": -32601,
                    "message": "method not found: sampling/createMessage",
                },
            }),
        ]
    );
}

#[tokio::test]
async fn calls_stdio_server_tools() {
    let mcp = started(echo_server()).await;
    assert_eq!(mcp.definitions().count(), 1);
    assert_eq!(call_test_echo(&mcp).await, Some(Ok("pong".into())));
    assert_eq!(mcp.call("bash", &json!({})).await, None);
}

#[tokio::test]
async fn reports_tool_calls_the_server_marks_as_errors() {
    let server = server_answering_calls_with(
        r#"printf '{"jsonrpc":"2.0","id":%s,"result":{"isError":true,"content":[{"type":"text","text":"no such page"}]}}\n' "$id""#,
    );
    let mcp = started(server).await;
    assert_eq!(call_test_echo(&mcp).await, Some(Err("no such page".into())));
}

#[tokio::test]
async fn reports_tool_calls_the_server_rejects_with_its_message_and_code() {
    let server = server_answering_calls_with(
        r#"printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32602,"message":"unknown tool: echo"}}\n' "$id""#,
    );
    let mcp = started(server).await;
    assert_eq!(
        call_test_echo(&mcp).await,
        Some(Err("unknown tool: echo (-32602)".into()))
    );
}

#[tokio::test]
async fn fails_when_the_server_repeats_a_tool_list_cursor() {
    let config = server_with(
        r#"    *'"tools/list"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[],"nextCursor":"again"}}\n' "$id" ;;"#,
        ":",
    );
    let started = tokio::time::timeout(
        Duration::from_secs(10),
        start("test".into(), Scope::Project, config),
    )
    .await
    .expect("tools/list did not stop");
    assert_eq!(
        Mcp::default().add(started).status,
        "MCP server test failed: tools/list repeated the cursor \"again\""
    );
}

#[tokio::test]
async fn fails_calls_when_the_server_sends_a_line_that_is_too_long() {
    let mut config = server_answering_calls_with(&format!(
        "head -c {} /dev/zero | tr '\\0' a; sleep 60",
        MAX_LINE + 1
    ));
    config.timeout = Some(30);
    let mcp = started(config).await;
    let call = tokio::time::timeout(Duration::from_secs(20), call_test_echo(&mcp)).await;
    assert_eq!(
        call.ok(),
        Some(Some(Err(
            "server sent a line longer than 32 MiB; closed the connection".into()
        )))
    );
}

#[tokio::test]
async fn reads_a_line_as_long_as_the_limit() {
    let mut config = server_answering_calls_with(&format!(
        r#"reply=$(printf '{{"jsonrpc":"2.0","id":%s,"result":{{"content":[{{"type":"text","text":"pong"}}]}}}}' "$id"); printf '%s' "$reply"; head -c $(({MAX_LINE} - ${{#reply}})) /dev/zero | tr '\0' ' '; printf '\n'"#
    ));
    config.timeout = Some(30);
    let mcp = started(config).await;
    assert_eq!(call_test_echo(&mcp).await, Some(Ok("pong".into())));
}

#[tokio::test]
async fn expands_a_leading_tilde_in_arguments_and_cwd() {
    let mut config = server_answering_calls_with(
        r#"printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"%s %s"}]}}\n' "$id" "$PWD" "$0""#,
    );
    config.args.push("~/notes".into());
    config.cwd = Some("~/".into());
    let mcp = started(config).await;
    let home = std::path::PathBuf::from(std::env::var("HOME").unwrap());
    assert_eq!(
        call_test_echo(&mcp).await,
        Some(Ok(format!(
            "{} {}",
            std::fs::canonicalize(&home).unwrap().display(),
            home.join("notes").display()
        )))
    );
}

async fn started(config: ServerConfig) -> Mcp {
    let mut mcp = Mcp::default();
    mcp.add(start("test".into(), Scope::Project, config).await);
    mcp
}

async fn started_with_short_timeout(mut config: ServerConfig) -> Mcp {
    config.timeout = Some(1);
    started(config).await
}

async fn call_test_echo(mcp: &Mcp) -> Option<Result<String, String>> {
    mcp.call("mcp__test__echo", &json!({})).await
}
