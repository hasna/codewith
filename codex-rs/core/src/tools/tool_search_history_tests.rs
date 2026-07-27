use super::*;
use pretty_assertions::assert_eq;
use serde_json::json;

#[test]
fn sanitization_preserves_the_oldest_request_prefix() {
    let mut history = vec![
        ResponseItem::ToolSearchOutput {
            call_id: Some("first".to_string()),
            status: "completed".to_string(),
            execution: "client".to_string(),
            tools: (0..TOOL_SEARCH_MAX_HISTORY_RESULTS)
                .map(|index| json!({"type": "function", "name": format!("first_{index}")}))
                .collect(),
        },
        ResponseItem::ToolSearchOutput {
            call_id: Some("later".to_string()),
            status: "completed".to_string(),
            execution: "client".to_string(),
            tools: vec![json!({"type": "function", "name": "later"})],
        },
    ];
    let expected_first = history[0].clone();

    sanitize_client_tool_search_history(&mut history, ReservedNamespacePolicy::Reject);

    assert_eq!(history[0], expected_first);
    let ResponseItem::ToolSearchOutput { tools, .. } = &history[1] else {
        panic!("expected tool_search output");
    };
    assert!(tools.is_empty());
}

#[test]
fn chat_history_keeps_namespaces_reserved_only_by_responses() {
    let reserved = json!({
        "type": "namespace",
        "name": "image_gen",
        "tools": [{"type": "function", "name": "imagegen"}],
    });
    let mut chat_history = vec![ResponseItem::ToolSearchOutput {
        call_id: Some("chat".to_string()),
        status: "completed".to_string(),
        execution: "client".to_string(),
        tools: vec![reserved.clone()],
    }];
    let mut responses_history = chat_history.clone();

    sanitize_client_tool_search_history(&mut chat_history, ReservedNamespacePolicy::Allow);
    sanitize_client_tool_search_history(&mut responses_history, ReservedNamespacePolicy::Reject);

    assert_eq!(
        chat_history,
        vec![ResponseItem::ToolSearchOutput {
            call_id: Some("chat".to_string()),
            status: "completed".to_string(),
            execution: "client".to_string(),
            tools: vec![reserved],
        }]
    );
    let ResponseItem::ToolSearchOutput { tools, .. } = &responses_history[0] else {
        panic!("expected tool_search output");
    };
    assert!(tools.is_empty());
}
