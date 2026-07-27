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
use crate::create_tools_json_for_responses_api;
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

#[test]
fn canonical_generic_web_run_serializes_for_chat_and_responses() {
    let namespace = ToolSpec::built_in_web_search()
        .namespace()
        .expect("canonical web search namespace")
        .clone();
    let generic = ToolSpec::Namespace(namespace);

    let serialized =
        serde_json::to_value(&generic).expect("generic canonical web.run should serialize");
    assert_eq!(serialized["type"], "namespace");
    assert_eq!(serialized["name"], "web");
    assert_eq!(serialized["tools"][0]["name"], "run");
    assert_eq!(
        create_tools_json_for_responses_api(&[generic])
            .expect("canonical generic web.run should pass Responses validation"),
        vec![serialized]
    );
}

#[test]
fn canonical_web_run_schema_is_deep_pinned() {
    let namespace = super::canonical_web_search_namespace();
    let [ResponsesApiNamespaceTool::Function(tool)] = namespace.tools.as_slice() else {
        panic!("canonical web namespace should contain exactly one function");
    };
    let expected: serde_json::Value =
        serde_json::from_str(codex_tool_contracts::WEB_RUN_SCHEMA_JSON)
            .expect("valid pinned schema");

    assert_eq!(
        serde_json::to_value(&tool.parameters).expect("serialize canonical schema"),
        expected
    );
}

#[test]
fn canonical_image_generation_schema_is_exact() {
    let namespace = super::canonical_image_generation_namespace();
    let [ResponsesApiNamespaceTool::Function(tool)] = namespace.tools.as_slice() else {
        panic!("canonical images namespace should contain exactly one function");
    };

    assert_eq!(
        serde_json::to_value(&tool.parameters).expect("serialize image schema"),
        serde_json::from_str::<serde_json::Value>(
            codex_tool_contracts::IMAGE_GENERATION_SCHEMA_JSON
        )
        .expect("valid pinned image schema")
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
    let namespace = namespace_tool("image_gen", "imagegen");
    serde_json::to_value(ToolSpec::Namespace(namespace.clone()))
        .expect("generic serialization must remain available for Chat transports");
    let err = create_tools_json_for_responses_api(&[ToolSpec::Namespace(namespace.clone())])
        .expect_err("image_gen.imagegen must fail closed at the Responses boundary");
    assert!(
        err.to_string().contains("image_gen.imagegen"),
        "error should identify the reserved wire name: {err}"
    );

    let deferred = serde_json::to_value(LoadableToolSpec::Namespace(namespace))
        .expect("deferred declarations use generic serialization");
    assert!(crate::is_forbidden_reserved_namespace_value(&deferred));
}

#[test]
fn namespace_serialization_allows_custom_namespaces() {
    for (namespace, tool_name) in [
        ("images", "imagegen"),
        ("mcp__codex_apps__images", "imagegen"),
    ] {
        serde_json::to_value(ToolSpec::Namespace(namespace_tool(namespace, tool_name)))
            .unwrap_or_else(|err| panic!("{namespace}.{tool_name} should serialize: {err}"));
    }
}

#[test]
fn ordinary_function_serialization_is_unchanged() {
    let tool = ToolSpec::Function(ResponsesApiTool {
        name: "ordinary_tool".to_string(),
        description: "An ordinary function tool.".to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::default(),
        output_schema: None,
    });

    let value = serde_json::to_value(tool).expect("ordinary function tool should serialize");
    assert_eq!(value["type"], "function");
    assert_eq!(value["name"], "ordinary_tool");
}

#[test]
fn built_in_web_search_serializes_reserved_namespace_with_typed_provenance() {
    let value = serde_json::to_value(ToolSpec::built_in_web_search())
        .expect("typed built-in web search should serialize");

    assert_eq!(value["type"], "namespace");
    assert_eq!(value["name"], "web");
    assert_eq!(value["description"], "Tools in the web namespace.");
    assert_eq!(value["tools"].as_array().map(Vec::len), Some(1));
    assert_eq!(value["tools"][0]["type"], "function");
    assert_eq!(value["tools"][0]["name"], "run");
    assert_eq!(value["tools"][0]["strict"], false);
    assert!(
        value["tools"][0]["description"]
            .as_str()
            .is_some_and(|description| description.starts_with("Tool for accessing the internet."))
    );
    let properties = value["tools"][0]["parameters"]["properties"]
        .as_object()
        .expect("canonical web.run parameters should have properties");
    assert_eq!(properties.len(), 11);
    for property in [
        "search_query",
        "image_query",
        "open",
        "click",
        "find",
        "screenshot",
        "finance",
        "weather",
        "sports",
        "time",
        "response_length",
    ] {
        assert!(
            properties.contains_key(property),
            "canonical web.run schema should contain `{property}`"
        );
    }
}

#[test]
fn namespace_serialization_rejects_other_tools_in_reserved_namespace() {
    let err = create_tools_json_for_responses_api(&[ToolSpec::Namespace(namespace_tool(
        "web", "imagegen",
    ))])
    .expect_err("web.imagegen must not inherit the web.run exception");

    assert!(err.to_string().contains("web.imagegen"), "{err}");
}

#[test]
fn namespace_serialization_rejects_name_only_web_run_impostors() {
    let namespace = namespace_tool("web", "run");
    let err = create_tools_json_for_responses_api(&[ToolSpec::Namespace(namespace.clone())])
        .expect_err("generic web.run must not bypass reserved schema validation");
    assert!(err.to_string().contains("web.run"), "{err}");

    let deferred = serde_json::to_value(LoadableToolSpec::Namespace(namespace))
        .expect("deferred declarations use generic serialization");
    assert!(crate::is_forbidden_reserved_namespace_value(&deferred));
}
