use super::*;

#[tokio::test]
async fn reads_http_replies_sent_as_events() {
    let (url, requests) = http_server(|request| {
        let id = &request.body["id"];
        match request.body["method"].as_str() {
            Some("tools/call") => HttpReply::events(&[
                json!({ "jsonrpc": "2.0", "method": "notifications/progress", "params": {} }),
                json!({ "jsonrpc": "2.0", "id": "p1", "method": "ping" }),
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": { "content": [{ "type": "text", "text": "streamed" }] },
                }),
            ]),
            _ => http_mcp(request),
        }
    })
    .await;
    let mcp = started(http_config(&url)).await;
    assert_eq!(call_web_echo(&mcp).await, Some(Ok("streamed".into())));
    let replies = || -> Vec<Value> {
        requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.body["id"] == "p1")
            .map(|request| request.body.clone())
            .collect()
    };
    let replies = eventually(replies, |replies| !replies.is_empty()).await;
    assert_eq!(
        replies,
        [json!({ "jsonrpc": "2.0", "id": "p1", "result": {} })]
    );
}

#[tokio::test]
async fn reads_http_replies_sent_as_a_json_batch() {
    let (url, _) = http_server(|request| match request.body["method"].as_str() {
        Some("tools/call") => HttpReply::json(&json!([
            { "jsonrpc": "2.0", "method": "notifications/progress", "params": {} },
            {
                "jsonrpc": "2.0",
                "id": request.body["id"],
                "result": { "content": [{ "type": "text", "text": "batched" }] },
            },
        ])),
        _ => http_mcp(request),
    })
    .await;
    let mcp = started(http_config(&url)).await;
    assert_eq!(call_web_echo(&mcp).await, Some(Ok("batched".into())));
}

#[tokio::test]
async fn rejects_http_replies_with_an_unsupported_content_type() {
    let (url, _) = http_server(|request| match request.body["method"].as_str() {
        Some("tools/call") => {
            HttpReply::status(200, "pong").with_header("content-type", "Text/Plain; charset=utf-8")
        }
        _ => http_mcp(request),
    })
    .await;
    let mcp = started(http_config(&url)).await;
    assert_eq!(
        call_web_echo(&mcp).await,
        Some(Err(
            "server answered tools/call with unsupported content type \"text/plain\"".into()
        ))
    );
}

#[tokio::test]
async fn reads_events_split_over_lines_with_crlf_endings_and_an_empty_type() {
    let (url, _) = http_server(|request| match request.body["method"].as_str() {
        Some("tools/call") => HttpReply {
            status: 200,
            headers: vec![("content-type".into(), "text/event-stream".into())],
            body: format!(
                ": comment\r\nevent: other\r\ndata: {{}}\r\n\r\nevent:\r\ndata: {{\"jsonrpc\": \"2.0\", \"id\": {},\r\ndata: \"result\": {{\"content\": [{{\"type\": \"text\", \"text\": \"split\"}}]}}}}\r\n\r\n",
                request.body["id"]
            ),
            delay: Duration::ZERO,
        },
        _ => http_mcp(request),
    })
    .await;
    let mcp = started(http_config(&url)).await;
    assert_eq!(call_web_echo(&mcp).await, Some(Ok("split".into())));
}

#[tokio::test]
async fn reads_events_with_cr_line_endings() {
    let (url, _) = http_server(|request| match request.body["method"].as_str() {
        Some("tools/call") => HttpReply {
            status: 200,
            headers: vec![("content-type".into(), "text/event-stream".into())],
            body: format!(
                "event: other\rdata: {{}}\r\rdata: {{\"jsonrpc\": \"2.0\", \"id\": {}, \"result\": {{\"content\": [{{\"type\": \"text\", \"text\": \"cr\"}}]}}}}\r\r",
                request.body["id"]
            ),
            delay: Duration::ZERO,
        },
        _ => http_mcp(request),
    })
    .await;
    let mcp = started(http_config(&url)).await;
    assert_eq!(call_web_echo(&mcp).await, Some(Ok("cr".into())));
}

#[tokio::test]
async fn fails_calls_when_an_http_reply_is_too_long() {
    for content_type in ["application/json", "text/event-stream"] {
        let (url, _) = http_server(move |request| match request.body["method"].as_str() {
            Some("tools/call") => HttpReply {
                status: 200,
                headers: vec![("content-type".into(), content_type.into())],
                body: format!("data: {}", "a".repeat(MAX_LINE + 1)),
                delay: Duration::ZERO,
            },
            _ => http_mcp(request),
        })
        .await;
        let mut config = http_config(&url);
        config.timeout = Some(60);
        let mcp = started(config).await;
        assert_eq!(
            call_web_echo(&mcp).await,
            Some(Err("server sent a reply longer than 32 MiB".into())),
            "{content_type}"
        );
    }
}
