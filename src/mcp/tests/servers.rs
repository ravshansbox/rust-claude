use super::*;

#[tokio::test]
async fn lists_global_server_tools_before_project_ones() {
    let (url, _) = http_server(http_mcp).await;
    let mut mcp = Mcp::default();
    for (name, scope) in [
        ("alpha", Scope::Project),
        ("zeta", Scope::Global),
        ("beta", Scope::Global),
    ] {
        mcp.add(start(name.into(), scope, http_config(&url)).await);
    }
    let names: Vec<Value> = mcp
        .definitions()
        .map(|definition| definition["name"].clone())
        .collect();
    assert_eq!(
        names,
        ["mcp__beta__echo", "mcp__zeta__echo", "mcp__alpha__echo"]
    );
}

#[tokio::test]
async fn replaces_a_server_that_starts_again() {
    let mut mcp = Mcp::default();
    mcp.add(start("test".into(), Scope::Project, echo_server()).await);
    assert_eq!(
        mcp.replace(start("test".into(), Scope::Project, echo_server()).await)
            .status,
        "loaded project MCP server: test (1 tool)"
    );
    assert_eq!(mcp.definitions().count(), 1);
    mcp.replace(failed_start("test", "gone"));
    assert_eq!(mcp.definitions().count(), 0);
}

#[tokio::test]
async fn reports_each_server_as_it_starts() {
    let mut mcp = Mcp::default();
    let started = start("test".into(), Scope::Project, echo_server()).await;
    assert_eq!(
        mcp.add(started).status,
        "loaded project MCP server: test (1 tool)"
    );
    let mut broken = echo_server();
    broken.command = None;
    let failed = start("broken".into(), Scope::Global, broken).await;
    assert_eq!(
        mcp.add(failed).status,
        "MCP server broken failed: command is missing"
    );
    let mut lost = echo_server();
    lost.cwd = Some("/no/such/folder".into());
    let failed = start("lost".into(), Scope::Global, lost).await;
    assert_eq!(
        mcp.add(failed).status,
        "MCP server lost failed: cwd /no/such/folder is not a folder"
    );
    let clash = start("test".into(), Scope::Global, echo_server()).await;
    let added = mcp.add(clash);
    assert_eq!(
        added.diagnostics,
        ["MCP server test: skipped tool echo because its name clashes with test/echo"]
    );
    assert_eq!(added.status, "loaded global MCP server: test (0 tools)");
    assert_eq!(mcp.diagnostics, Vec::<String>::new());
}

#[test]
fn shortens_the_load_notice_for_servers_that_need_sign_in() {
    let mcp = Mcp::from_started(
        vec!["MCP: invalid mcp.json".into()],
        [
            failed_start("figma", "needs sign-in: run /mcp login figma"),
            failed_start("broken", "command is missing"),
        ],
    );
    assert_eq!(
        mcp.diagnostics,
        [
            "MCP: invalid mcp.json",
            "figma (needs sign-in)",
            "MCP server broken failed: command is missing"
        ]
    );
}

#[tokio::test]
async fn sums_up_each_server_in_short() {
    let mut mcp = Mcp::default();
    let added = mcp.add(start("test".into(), Scope::Project, echo_server()).await);
    assert_eq!(
        (added.summary.as_str(), added.failed),
        ("test (1 tool)", false)
    );
    let added = mcp.add(failed_start("broken", "command is missing"));
    assert_eq!(
        (added.summary.as_str(), added.failed),
        ("broken (failed)", true)
    );
    let added = mcp.add(failed_start("figma", "needs sign-in: run /mcp login figma"));
    assert_eq!(
        (added.summary.as_str(), added.failed),
        ("figma (needs sign-in)", true)
    );
}
