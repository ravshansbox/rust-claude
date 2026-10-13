use super::{Request, UiEvent, sign_out_of_mcp_server};
use crate::{agent::AgentEvent, config};

#[tokio::test]
async fn restarts_an_mcp_server_after_signing_out() {
    let path = config::dir().unwrap().join("mcp.json");
    std::fs::write(
        &path,
        r#"{ "mcpServers": { "web": { "url": "http://127.0.0.1:9/mcp", "timeout": 5 } } }"#,
    )
    .unwrap();
    let (events, mut shown) = tokio::sync::mpsc::unbounded_channel();
    let (requests, mut sent) = tokio::sync::mpsc::unbounded_channel();
    sign_out_of_mcp_server("web".into(), events.clone(), requests.clone()).await;
    sign_out_of_mcp_server("missing".into(), events, requests).await;
    std::fs::remove_file(&path).unwrap();
    match shown.try_recv() {
        Ok(UiEvent::McpRestarting { name, notice, .. }) => {
            assert_eq!(name, "web");
            assert_eq!(notice, "MCP server web was not signed in");
        }
        _ => panic!("the server did not restart"),
    }
    assert!(matches!(sent.try_recv(), Ok(Request::ReplaceMcpServer(_))));
    match shown.try_recv() {
        Ok(UiEvent::Agent(AgentEvent::Notice(notice))) => assert_eq!(
            notice,
            "MCP sign-out of missing failed: no MCP server named missing"
        ),
        _ => panic!("no notice for the missing server"),
    }
    assert!(sent.try_recv().is_err());
}
