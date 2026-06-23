use super::ResponsesApiNamespace;
use super::ResponsesApiWebSearchFilters;
use super::ResponsesApiWebSearchUserLocation;
use super::ToolSpec;
use super::ZaiWebSearchConfig;
use crate::AdditionalProperties;
use crate::FreeformTool;
use crate::FreeformToolFormat;
use crate::JsonSchema;
use crate::ResponsesApiNamespaceTool;
use crate::ResponsesApiTool;
use crate::create_tools_json_for_responses_api;
use codex_protocol::config_types::WebSearchContextSize;
use codex_protocol::config_types::WebSearchFilters as ConfigWebSearchFilters;
use codex_protocol::config_types::WebSearchUserLocation as ConfigWebSearchUserLocation;
use codex_protocol::config_types::WebSearchUserLocationType;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::collections::BTreeMap;

#[test]
fn tool_spec_name_covers_all_variants() {
    assert_eq!(
        ToolSpec::Function(ResponsesApiTool {
            name: "lookup_order".to_string(),
            description: "Look up an order".to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                BTreeMap::new(),
                /*required*/ None,
                /*additional_properties*/ None
            ),
            output_schema: None,
        })
        .name(),
        "lookup_order"
    );
    assert_eq!(
        ToolSpec::Namespace(ResponsesApiNamespace {
            name: "mcp__demo__".to_string(),
            description: "Demo tools".to_string(),
            tools: Vec::new(),
        })
        .name(),
        "mcp__demo__"
    );
    assert_eq!(
        ToolSpec::ToolSearch {
            execution: "sync".to_string(),
            description: "Search for tools".to_string(),
            parameters: JsonSchema::object(
                BTreeMap::new(),
                /*required*/ None,
                /*additional_properties*/ None
            ),
        }
        .name(),
        "tool_search"
    );
    assert_eq!(
        ToolSpec::ImageGeneration {
            output_format: "png".to_string(),
        }
        .name(),
        "image_generation"
    );
    assert_eq!(
        ToolSpec::WebSearch {
            external_web_access: Some(true),
            filters: None,
            user_location: None,
            search_context_size: None,
            search_content_types: None,
        }
        .name(),
        "web_search"
    );
    assert_eq!(
        ToolSpec::Freeform(FreeformTool {
            name: "exec".to_string(),
            description: "Run a command".to_string(),
            format: FreeformToolFormat {
                r#type: "grammar".to_string(),
                syntax: "lark".to_string(),
                definition: "start: \"exec\"".to_string(),
            },
        })
        .name(),
        "exec"
    );
}

#[test]
fn web_search_config_converts_to_responses_api_types() {
    assert_eq!(
        ResponsesApiWebSearchFilters::from(ConfigWebSearchFilters {
            allowed_domains: Some(vec!["example.com".to_string()]),
        }),
        ResponsesApiWebSearchFilters {
            allowed_domains: Some(vec!["example.com".to_string()]),
        }
    );
    assert_eq!(
        ResponsesApiWebSearchUserLocation::from(ConfigWebSearchUserLocation {
            r#type: WebSearchUserLocationType::Approximate,
            country: Some("US".to_string()),
            region: Some("California".to_string()),
            city: Some("San Francisco".to_string()),
            timezone: Some("America/Los_Angeles".to_string()),
        }),
        ResponsesApiWebSearchUserLocation {
            r#type: WebSearchUserLocationType::Approximate,
            country: Some("US".to_string()),
            region: Some("California".to_string()),
            city: Some("San Francisco".to_string()),
            timezone: Some("America/Los_Angeles".to_string()),
        }
    );
}

#[test]
fn create_tools_json_for_responses_api_includes_top_level_name() {
    assert_eq!(
        create_tools_json_for_responses_api(&[ToolSpec::Function(ResponsesApiTool {
            name: "demo".to_string(),
            description: "A demo tool".to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                BTreeMap::from([("foo".to_string(), JsonSchema::string(/*description*/ None),)]),
                /*required*/ None,
                /*additional_properties*/ None
            ),
            output_schema: None,
        })])
        .expect("serialize tools"),
        vec![json!({
            "type": "function",
            "name": "demo",
            "description": "A demo tool",
            "strict": false,
            "parameters": {
                "type": "object",
                "properties": {
                    "foo": { "type": "string" }
                },
            },
        })]
    );
}

#[test]
fn create_tools_json_for_responses_api_accepts_strict_nullable_property() {
    assert_eq!(
        create_tools_json_for_responses_api(&[ToolSpec::Function(ResponsesApiTool {
            name: "demo".to_string(),
            description: "A demo tool".to_string(),
            strict: true,
            defer_loading: None,
            parameters: JsonSchema::object(
                BTreeMap::from([(
                    "query".to_string(),
                    JsonSchema::any_of(
                        vec![
                            JsonSchema::string(/*description*/ None),
                            JsonSchema::null(/*description*/ None),
                        ],
                        Some("Optional query.".to_string()),
                    ),
                )]),
                Some(vec!["query".to_string()]),
                Some(false.into()),
            ),
            output_schema: None,
        })])
        .expect("serialize tools"),
        vec![json!({
            "type": "function",
            "name": "demo",
            "description": "A demo tool",
            "strict": true,
            "parameters": {
                "type": "object",
                "properties": {
                    "query": {
                        "description": "Optional query.",
                        "anyOf": [
                            { "type": "string" },
                            { "type": "null" }
                        ],
                    },
                },
                "required": ["query"],
                "additionalProperties": false,
            },
        })]
    );
}

#[test]
fn create_tools_json_for_responses_api_accepts_strict_nested_closed_object() {
    assert_eq!(
        create_tools_json_for_responses_api(&[ToolSpec::Function(ResponsesApiTool {
            name: "demo".to_string(),
            description: "A demo tool".to_string(),
            strict: true,
            defer_loading: None,
            parameters: JsonSchema::object(
                BTreeMap::from([(
                    "payload".to_string(),
                    JsonSchema::object(
                        BTreeMap::from([(
                            "query".to_string(),
                            JsonSchema::string(/*description*/ None),
                        )]),
                        Some(vec!["query".to_string()]),
                        Some(false.into()),
                    ),
                )]),
                Some(vec!["payload".to_string()]),
                Some(false.into()),
            ),
            output_schema: None,
        })])
        .expect("serialize tools"),
        vec![json!({
            "type": "function",
            "name": "demo",
            "description": "A demo tool",
            "strict": true,
            "parameters": {
                "type": "object",
                "properties": {
                    "payload": {
                        "type": "object",
                        "properties": {
                            "query": { "type": "string" },
                        },
                        "required": ["query"],
                        "additionalProperties": false,
                    },
                },
                "required": ["payload"],
                "additionalProperties": false,
            },
        })]
    );
}

#[test]
fn create_tools_json_for_responses_api_rejects_strict_missing_required_property() {
    let err = create_tools_json_for_responses_api(&[ToolSpec::Function(ResponsesApiTool {
        name: "demo".to_string(),
        description: "A demo tool".to_string(),
        strict: true,
        defer_loading: None,
        parameters: JsonSchema::object(
            BTreeMap::from([(
                "query".to_string(),
                JsonSchema::string(/*description*/ None),
            )]),
            Some(Vec::new()),
            Some(false.into()),
        ),
        output_schema: None,
    })])
    .expect_err("strict schema should be rejected locally");

    assert!(
        err.to_string()
            .contains("missing required properties: query"),
        "{err}"
    );
}

#[test]
fn create_tools_json_for_responses_api_rejects_strict_missing_required_array() {
    let err = create_tools_json_for_responses_api(&[ToolSpec::Function(ResponsesApiTool {
        name: "demo".to_string(),
        description: "A demo tool".to_string(),
        strict: true,
        defer_loading: None,
        parameters: JsonSchema::object(
            BTreeMap::from([(
                "query".to_string(),
                JsonSchema::string(/*description*/ None),
            )]),
            /*required*/ None,
            Some(false.into()),
        ),
        output_schema: None,
    })])
    .expect_err("strict schema should be rejected locally");

    assert!(
        err.to_string()
            .contains("object schemas must provide required with every property key"),
        "{err}"
    );
}

#[test]
fn create_tools_json_for_responses_api_rejects_strict_missing_additional_properties() {
    let err = create_tools_json_for_responses_api(&[ToolSpec::Function(ResponsesApiTool {
        name: "demo".to_string(),
        description: "A demo tool".to_string(),
        strict: true,
        defer_loading: None,
        parameters: JsonSchema::object(
            BTreeMap::from([(
                "query".to_string(),
                JsonSchema::string(/*description*/ None),
            )]),
            Some(vec!["query".to_string()]),
            /*additional_properties*/ None,
        ),
        output_schema: None,
    })])
    .expect_err("strict schema should be rejected locally");

    assert!(
        err.to_string()
            .contains("parameters: object schemas must set additionalProperties to false"),
        "{err}"
    );
}

#[test]
fn create_tools_json_for_responses_api_rejects_strict_nested_open_object() {
    let err = create_tools_json_for_responses_api(&[ToolSpec::Function(ResponsesApiTool {
        name: "demo".to_string(),
        description: "A demo tool".to_string(),
        strict: true,
        defer_loading: None,
        parameters: JsonSchema::object(
            BTreeMap::from([(
                "payload".to_string(),
                JsonSchema::object(BTreeMap::new(), Some(Vec::new()), Some(true.into())),
            )]),
            Some(vec!["payload".to_string()]),
            Some(false.into()),
        ),
        output_schema: None,
    })])
    .expect_err("strict nested object should be rejected locally");

    assert!(
        err.to_string().contains(
            "parameters.properties.payload: object schemas must set additionalProperties to false"
        ),
        "{err}"
    );
}

#[test]
fn create_tools_json_for_responses_api_validates_strict_namespace_child_tools() {
    let err = create_tools_json_for_responses_api(&[ToolSpec::Namespace(ResponsesApiNamespace {
        name: "mcp__demo__".to_string(),
        description: "Demo tools".to_string(),
        tools: vec![ResponsesApiNamespaceTool::Function(ResponsesApiTool {
            name: "lookup_order".to_string(),
            description: "Look up an order".to_string(),
            strict: true,
            defer_loading: None,
            parameters: JsonSchema::object(
                BTreeMap::from([(
                    "order_id".to_string(),
                    JsonSchema::string(/*description*/ None),
                )]),
                Some(Vec::new()),
                Some(false.into()),
            ),
            output_schema: None,
        })],
    })])
    .expect_err("strict namespace child schema should be rejected locally");

    assert!(
        err.to_string()
            .contains("missing required properties: order_id"),
        "{err}"
    );
}

#[test]
fn namespace_tool_spec_serializes_expected_wire_shape() {
    assert_eq!(
        serde_json::to_value(ToolSpec::Namespace(ResponsesApiNamespace {
            name: "mcp__demo__".to_string(),
            description: "Demo tools".to_string(),
            tools: vec![ResponsesApiNamespaceTool::Function(ResponsesApiTool {
                name: "lookup_order".to_string(),
                description: "Look up an order".to_string(),
                strict: false,
                defer_loading: None,
                parameters: JsonSchema::object(
                    BTreeMap::from([(
                        "order_id".to_string(),
                        JsonSchema::string(/*description*/ None),
                    )]),
                    /*required*/ None,
                    /*additional_properties*/ None,
                ),
                output_schema: None,
            })],
        }))
        .expect("serialize namespace tool"),
        json!({
            "type": "namespace",
            "name": "mcp__demo__",
            "description": "Demo tools",
            "tools": [
                {
                    "type": "function",
                    "name": "lookup_order",
                    "description": "Look up an order",
                    "strict": false,
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "order_id": { "type": "string" },
                        },
                    },
                },
            ],
        })
    );
}

#[test]
fn web_search_tool_spec_serializes_expected_wire_shape() {
    assert_eq!(
        serde_json::to_value(ToolSpec::WebSearch {
            external_web_access: Some(true),
            filters: Some(ResponsesApiWebSearchFilters {
                allowed_domains: Some(vec!["example.com".to_string()]),
            }),
            user_location: Some(ResponsesApiWebSearchUserLocation {
                r#type: WebSearchUserLocationType::Approximate,
                country: Some("US".to_string()),
                region: Some("California".to_string()),
                city: Some("San Francisco".to_string()),
                timezone: Some("America/Los_Angeles".to_string()),
            }),
            search_context_size: Some(WebSearchContextSize::High),
            search_content_types: Some(vec!["text".to_string(), "image".to_string()]),
        })
        .expect("serialize web_search"),
        json!({
            "type": "web_search",
            "external_web_access": true,
            "filters": {
                "allowed_domains": ["example.com"],
            },
            "user_location": {
                "type": "approximate",
                "country": "US",
                "region": "California",
                "city": "San Francisco",
                "timezone": "America/Los_Angeles",
            },
            "search_context_size": "high",
            "search_content_types": ["text", "image"],
        })
    );
}

#[test]
fn provider_native_web_search_tool_specs_serialize_expected_wire_shapes() {
    assert_eq!(
        serde_json::to_value(ToolSpec::AnthropicWebSearch {
            name: "web_search".to_string(),
            max_uses: None,
            allowed_domains: Some(vec!["example.com".to_string()]),
        })
        .expect("serialize anthropic web_search"),
        json!({
            "type": "web_search_20260209",
            "name": "web_search",
            "allowed_domains": ["example.com"],
        })
    );
    assert_eq!(
        serde_json::to_value(ToolSpec::OpenRouterWebSearch {})
            .expect("serialize openrouter web_search"),
        json!({"type": "openrouter:web_search"})
    );
    assert_eq!(
        serde_json::to_value(ToolSpec::ZaiWebSearch {
            web_search: ZaiWebSearchConfig {
                enable: true,
                search_engine: "search-prime".to_string(),
                search_result: true,
            },
        })
        .expect("serialize zai web_search"),
        json!({
            "type": "web_search",
            "web_search": {
                "enable": true,
                "search_engine": "search-prime",
                "search_result": true,
            },
        })
    );
}

#[test]
fn tool_search_tool_spec_serializes_expected_wire_shape() {
    assert_eq!(
        serde_json::to_value(ToolSpec::ToolSearch {
            execution: "sync".to_string(),
            description: "Search app tools".to_string(),
            parameters: JsonSchema::object(
                BTreeMap::from([(
                    "query".to_string(),
                    JsonSchema::string(Some("Tool search query".to_string()),),
                )]),
                Some(vec!["query".to_string()]),
                Some(AdditionalProperties::Boolean(false))
            ),
        })
        .expect("serialize tool_search"),
        json!({
            "type": "tool_search",
            "execution": "sync",
            "description": "Search app tools",
            "parameters": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Tool search query",
                    }
                },
                "required": ["query"],
                "additionalProperties": false,
            },
        })
    );
}
