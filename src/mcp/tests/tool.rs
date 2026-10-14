use super::super::tool::{MAX_TOOL_NAME, qualified_name, tool_definition};
use super::*;

#[test]
fn qualifies_tool_names() {
    assert_eq!(qualified_name("docs", "search"), "mcp__docs__search");
    assert_eq!(
        qualified_name("docs", "get.page/v2"),
        "mcp__docs__get_page_v2"
    );
    assert_eq!(
        qualified_name("docs", &"a".repeat(100)).len(),
        MAX_TOOL_NAME
    );
}

#[test]
fn makes_object_schemas() {
    let definition = tool_definition(
        "mcp__docs__search",
        json!({ "name": "search", "inputSchema": { "type": "object" } }),
    );
    assert_eq!(
        definition,
        json!({
            "name": "mcp__docs__search",
            "description": "search",
            "input_schema": { "type": "object", "properties": {} },
        })
    );
}

#[test]
fn drops_top_level_schema_combinators() {
    let definition = tool_definition(
        "mcp__docs__search",
        json!({
            "name": "search",
            "inputSchema": {
                "type": "object",
                "properties": { "a": { "type": "string" }, "b": { "type": "string" } },
                "anyOf": [{ "required": ["a"] }, { "required": ["b"] }],
                "oneOf": [{ "required": ["a"] }],
                "allOf": [{ "required": ["b"] }],
            },
        }),
    );
    assert_eq!(
        definition["input_schema"],
        json!({
            "type": "object",
            "properties": { "a": { "type": "string" }, "b": { "type": "string" } },
        })
    );
}

#[test]
fn converts_results_to_text() {
    let result = json!({
        "content": [
            { "type": "text", "text": "hello" },
            { "type": "image", "data": "", "mimeType": "image/png" },
            { "type": "resource", "resource": { "uri": "file:///a", "text": "body" } },
            { "type": "resource_link", "uri": "file:///b", "name": "b" },
            { "type": "resource", "resource": { "uri": "file:///c", "blob": "" } },
            { "type": "resource_link" },
        ]
    });
    assert_eq!(
        result_text(&result),
        "hello\n[image: image/png]\nbody\n[resource link: file:///b]\n[resource: file:///c]\n[resource link: unknown]"
    );
    assert_eq!(
        result_text(&json!({ "content": [], "structuredContent": { "a": 1 } })),
        r#"{"a":1}"#
    );
}
