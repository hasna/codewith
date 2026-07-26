use super::LoadableToolSpec;
use super::ResponsesApiNamespace;
use super::ResponsesApiNamespaceTool;
use super::ResponsesApiTool;
use super::dynamic_tool_to_responses_api_tool;
use super::mcp_tool_to_deferred_responses_api_tool;
use super::tool_definition_to_responses_api_tool;
use crate::JsonSchema;
use crate::ToolDefinition;
use crate::ToolName;
use crate::ToolSpec;
use codex_protocol::dynamic_tools::DynamicToolSpec;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::collections::BTreeMap;

#[test]
fn tool_definition_to_responses_api_tool_omits_false_defer_loading() {
    assert_eq!(
        tool_definition_to_responses_api_tool(ToolDefinition {
            name: "lookup_order".to_string(),
            description: "Look up an order".to_string(),
            input_schema: JsonSchema::object(
                BTreeMap::from([(
                    "order_id".to_string(),
                    JsonSchema::string(/*description*/ None),
                )]),
                Some(vec!["order_id".to_string()]),
                Some(false.into())
            ),
            output_schema: Some(json!({"type": "object"})),
            defer_loading: false,
        }),
        ResponsesApiTool {
            name: "lookup_order".to_string(),
            description: "Look up an order".to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                BTreeMap::from([(
                    "order_id".to_string(),
                    JsonSchema::string(/*description*/ None),
                )]),
                Some(vec!["order_id".to_string()]),
                Some(false.into())
            ),
            output_schema: Some(json!({"type": "object"})),
        }
    );
}

#[test]
fn dynamic_tool_to_responses_api_tool_preserves_defer_loading() {
    let tool = DynamicToolSpec {
        namespace: None,
        name: "lookup_order".to_string(),
        description: "Look up an order".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "order_id": {"type": "string"}
            },
            "required": ["order_id"],
            "additionalProperties": false,
        }),
        defer_loading: true,
    };

    assert_eq!(
        dynamic_tool_to_responses_api_tool(&tool).expect("convert dynamic tool"),
        ResponsesApiTool {
            name: "lookup_order".to_string(),
            description: "Look up an order".to_string(),
            strict: false,
            defer_loading: Some(true),
            parameters: JsonSchema::object(
                BTreeMap::from([(
                    "order_id".to_string(),
                    JsonSchema::string(/*description*/ None),
                )]),
                Some(vec!["order_id".to_string()]),
                Some(false.into())
            ),
            output_schema: None,
        }
    );
}

#[test]
fn mcp_tool_to_deferred_responses_api_tool_sets_defer_loading() {
    let tool = rmcp::model::Tool::new(
        "lookup_order",
        "Look up an order",
        std::sync::Arc::new(rmcp::model::object(json!({
            "type": "object",
            "properties": {
                "order_id": {"type": "string"}
            },
            "required": ["order_id"],
            "additionalProperties": false,
        }))),
    );

    assert_eq!(
        mcp_tool_to_deferred_responses_api_tool(
            &ToolName::namespaced("mcp__codex_apps__", "lookup_order"),
            &tool,
        )
        .expect("convert deferred tool"),
        ResponsesApiTool {
            name: "lookup_order".to_string(),
            description: "Look up an order".to_string(),
            strict: false,
            defer_loading: Some(true),
            parameters: JsonSchema::object(
                BTreeMap::from([(
                    "order_id".to_string(),
                    JsonSchema::string(/*description*/ None),
                )]),
                Some(vec!["order_id".to_string()]),
                Some(false.into())
            ),
            output_schema: None,
        }
    );
}

#[test]
fn loadable_tool_spec_namespace_serializes_with_deferred_child_tools() {
    let namespace = LoadableToolSpec::Namespace(ResponsesApiNamespace {
        name: "mcp__codex_apps__calendar".to_string(),
        description: "Plan events".to_string(),
        tools: vec![ResponsesApiNamespaceTool::Function(ResponsesApiTool {
            name: "create_event".to_string(),
            description: "Create a calendar event.".to_string(),
            strict: false,
            defer_loading: Some(true),
            parameters: JsonSchema::object(
                Default::default(),
                /*required*/ None,
                /*additional_properties*/ None,
            ),
            output_schema: None,
        })],
    });

    let value = serde_json::to_value(namespace).expect("serialize namespace");

    assert_eq!(
        value,
        json!({
            "type": "namespace",
            "name": "mcp__codex_apps__calendar",
            "description": "Plan events",
            "tools": [
                {
                    "type": "function",
                    "name": "create_event",
                    "description": "Create a calendar event.",
                    "strict": false,
                    "defer_loading": true,
                    "parameters": {
                        "type": "object",
                        "properties": {}
                    }
                }
            ]
        })
    );
}

fn namespace_tool(namespace: &str, tool_name: &str) -> ResponsesApiNamespace {
    ResponsesApiNamespace {
        name: namespace.to_string(),
        description: format!("Tools in the {namespace} namespace."),
        tools: vec![ResponsesApiNamespaceTool::Function(ResponsesApiTool {
            name: tool_name.to_string(),
            description: format!("Run {tool_name}."),
            strict: false,
            defer_loading: Some(true),
            parameters: JsonSchema::object(
                Default::default(),
                /*required*/ None,
                /*additional_properties*/ None,
            ),
            output_schema: None,
        })],
    }
}

#[test]
fn namespace_serialization_rejects_reserved_image_gen_for_direct_and_deferred_tools() {
    let results = [
        serde_json::to_value(ToolSpec::Namespace(namespace_tool("image_gen", "imagegen"))),
        serde_json::to_value(LoadableToolSpec::Namespace(namespace_tool(
            "image_gen",
            "imagegen",
        ))),
    ];

    for result in results {
        let err = result.expect_err("image_gen.imagegen must fail closed before reaching the wire");
        assert!(
            err.to_string().contains("image_gen.imagegen"),
            "error should identify the reserved wire name: {err}"
        );
    }
}

#[test]
fn namespace_serialization_allows_custom_and_vetted_reserved_namespaces() {
    for (namespace, tool_name) in [
        ("images", "imagegen"),
        ("mcp__codex_apps__images", "imagegen"),
    ] {
        serde_json::to_value(ToolSpec::Namespace(namespace_tool(namespace, tool_name)))
            .unwrap_or_else(|err| panic!("{namespace}.{tool_name} should serialize: {err}"));
    }
}

#[test]
fn namespace_serialization_rejects_unvetted_tools_in_allowlisted_namespace() {
    let err = serde_json::to_value(ToolSpec::Namespace(namespace_tool("web", "imagegen")))
        .expect_err("web.imagegen must not inherit the web.run exception");

    assert!(err.to_string().contains("web.imagegen"), "{err}");
}

#[test]
fn namespace_serialization_rejects_name_only_web_run_impostors() {
    let results = [
        serde_json::to_value(ToolSpec::Namespace(namespace_tool("web", "run"))),
        serde_json::to_value(LoadableToolSpec::Namespace(namespace_tool("web", "run"))),
    ];

    for result in results {
        let err = result.expect_err("generic web.run must not bypass reserved schema validation");
        assert!(err.to_string().contains("web.run"), "{err}");
    }
}
