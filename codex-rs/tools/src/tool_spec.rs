use crate::AdditionalProperties;
use crate::FreeformTool;
use crate::JsonSchema;
use crate::JsonSchemaPrimitiveType;
use crate::JsonSchemaType;
use crate::LoadableToolSpec;
use crate::ResponsesApiNamespace;
use crate::ResponsesApiNamespaceTool;
use crate::ResponsesApiTool;
use codex_protocol::config_types::WebSearchContextSize;
use codex_protocol::config_types::WebSearchFilters as ConfigWebSearchFilters;
use codex_protocol::config_types::WebSearchUserLocation as ConfigWebSearchUserLocation;
use codex_protocol::config_types::WebSearchUserLocationType;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeSet;

/// When serialized as JSON, this produces a valid "Tool" in the OpenAI
/// Responses API.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type")]
pub enum ToolSpec {
    #[serde(rename = "function")]
    Function(ResponsesApiTool),
    #[serde(rename = "namespace")]
    Namespace(ResponsesApiNamespace),
    #[serde(rename = "tool_search")]
    ToolSearch {
        execution: String,
        description: String,
        parameters: JsonSchema,
    },
    #[serde(rename = "image_generation")]
    ImageGeneration { output_format: String },
    // TODO: Understand why we get an error on web_search although the API docs
    // say it's supported.
    // https://platform.openai.com/docs/guides/tools-web-search?api-mode=responses#:~:text=%7B%20type%3A%20%22web_search%22%20%7D%2C
    // The `external_web_access` field determines whether the web search is over
    // cached or live content.
    // https://platform.openai.com/docs/guides/tools-web-search#live-internet-access
    #[serde(rename = "web_search")]
    WebSearch {
        #[serde(skip_serializing_if = "Option::is_none")]
        external_web_access: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        filters: Option<ResponsesApiWebSearchFilters>,
        #[serde(skip_serializing_if = "Option::is_none")]
        user_location: Option<ResponsesApiWebSearchUserLocation>,
        #[serde(skip_serializing_if = "Option::is_none")]
        search_context_size: Option<WebSearchContextSize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        search_content_types: Option<Vec<String>>,
    },
    #[serde(rename = "web_search_20260209")]
    AnthropicWebSearch {
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        max_uses: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        allowed_domains: Option<Vec<String>>,
    },
    #[serde(rename = "openrouter:web_search")]
    OpenRouterWebSearch {},
    #[serde(rename = "web_search")]
    XaiWebSearch {},
    #[serde(rename = "web_search")]
    XiaomiWebSearch {},
    #[serde(rename = "web_search")]
    QwenWebSearch {},
    #[serde(rename = "web_search")]
    ZaiWebSearch { web_search: ZaiWebSearchConfig },
    #[serde(rename = "custom")]
    Freeform(FreeformTool),
}

impl ToolSpec {
    pub fn name(&self) -> &str {
        match self {
            ToolSpec::Function(tool) => tool.name.as_str(),
            ToolSpec::Namespace(namespace) => namespace.name.as_str(),
            ToolSpec::ToolSearch { .. } => "tool_search",
            ToolSpec::ImageGeneration { .. } => "image_generation",
            ToolSpec::WebSearch { .. }
            | ToolSpec::AnthropicWebSearch { .. }
            | ToolSpec::OpenRouterWebSearch { .. }
            | ToolSpec::XaiWebSearch { .. }
            | ToolSpec::XiaomiWebSearch { .. }
            | ToolSpec::QwenWebSearch { .. }
            | ToolSpec::ZaiWebSearch { .. } => "web_search",
            ToolSpec::Freeform(tool) => tool.name.as_str(),
        }
    }
}

impl From<LoadableToolSpec> for ToolSpec {
    fn from(value: LoadableToolSpec) -> Self {
        match value {
            LoadableToolSpec::Function(tool) => ToolSpec::Function(tool),
            LoadableToolSpec::Namespace(namespace) => ToolSpec::Namespace(namespace),
        }
    }
}

/// Returns JSON values that are compatible with Function Calling in the
/// Responses API:
/// https://platform.openai.com/docs/guides/function-calling?api-mode=responses
pub fn create_tools_json_for_responses_api(
    tools: &[ToolSpec],
) -> Result<Vec<Value>, serde_json::Error> {
    let mut tools_json = Vec::new();

    for tool in tools {
        validate_tool_spec_for_responses_api(tool)?;
        let json = serde_json::to_value(tool)?;
        tools_json.push(json);
    }

    Ok(tools_json)
}

fn validate_tool_spec_for_responses_api(tool: &ToolSpec) -> Result<(), serde_json::Error> {
    match tool {
        ToolSpec::Function(tool) => validate_responses_api_tool(tool),
        ToolSpec::Namespace(namespace) => {
            for tool in &namespace.tools {
                match tool {
                    ResponsesApiNamespaceTool::Function(tool) => validate_responses_api_tool(tool)?,
                }
            }
            Ok(())
        }
        ToolSpec::ToolSearch { .. }
        | ToolSpec::ImageGeneration { .. }
        | ToolSpec::WebSearch { .. }
        | ToolSpec::AnthropicWebSearch { .. }
        | ToolSpec::OpenRouterWebSearch { .. }
        | ToolSpec::XaiWebSearch { .. }
        | ToolSpec::XiaomiWebSearch { .. }
        | ToolSpec::QwenWebSearch { .. }
        | ToolSpec::ZaiWebSearch { .. }
        | ToolSpec::Freeform(_) => Ok(()),
    }
}

fn validate_responses_api_tool(tool: &ResponsesApiTool) -> Result<(), serde_json::Error> {
    if !tool.strict {
        return Ok(());
    }
    if !is_object_schema(&tool.parameters) {
        return Err(strict_schema_error(
            tool.name.as_str(),
            "parameters",
            "strict function parameters must be an object schema",
        ));
    }
    validate_strict_schema(tool.name.as_str(), "parameters", &tool.parameters)
}

fn validate_strict_schema(
    tool_name: &str,
    path: &str,
    schema: &JsonSchema,
) -> Result<(), serde_json::Error> {
    if is_object_schema(schema) {
        match schema.additional_properties.as_ref() {
            Some(AdditionalProperties::Boolean(false)) => {}
            _ => {
                return Err(strict_schema_error(
                    tool_name,
                    path,
                    "object schemas must set additionalProperties to false",
                ));
            }
        }

        let properties = schema.properties.as_ref();
        let property_names = properties
            .map(|properties| {
                properties
                    .keys()
                    .map(String::as_str)
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default();
        let Some(required) = schema.required.as_ref() else {
            return Err(strict_schema_error(
                tool_name,
                path,
                "object schemas must provide required with every property key",
            ));
        };
        let required_names = required.iter().map(String::as_str).collect::<BTreeSet<_>>();
        if property_names != required_names {
            let missing = property_names
                .difference(&required_names)
                .copied()
                .collect::<Vec<_>>();
            let extra = required_names
                .difference(&property_names)
                .copied()
                .collect::<Vec<_>>();
            let mut details = Vec::new();
            if !missing.is_empty() {
                details.push(format!(
                    "missing required properties: {}",
                    missing.join(", ")
                ));
            }
            if !extra.is_empty() {
                details.push(format!("unknown required properties: {}", extra.join(", ")));
            }
            return Err(strict_schema_error(tool_name, path, details.join("; ")));
        }
    }

    if let Some(properties) = schema.properties.as_ref() {
        for (name, property) in properties {
            validate_strict_schema(
                tool_name,
                format!("{path}.properties.{name}").as_str(),
                property,
            )?;
        }
    }
    if let Some(items) = schema.items.as_ref() {
        validate_strict_schema(tool_name, format!("{path}.items").as_str(), items)?;
    }
    if let Some(variants) = schema.any_of.as_ref() {
        for (index, variant) in variants.iter().enumerate() {
            validate_strict_schema(
                tool_name,
                format!("{path}.anyOf[{index}]").as_str(),
                variant,
            )?;
        }
    }
    if let Some(defs) = schema.defs.as_ref() {
        for (name, definition) in defs {
            validate_strict_schema(
                tool_name,
                format!("{path}.$defs.{name}").as_str(),
                definition,
            )?;
        }
    }
    if let Some(definitions) = schema.definitions.as_ref() {
        for (name, definition) in definitions {
            validate_strict_schema(
                tool_name,
                format!("{path}.definitions.{name}").as_str(),
                definition,
            )?;
        }
    }

    Ok(())
}

fn is_object_schema(schema: &JsonSchema) -> bool {
    matches!(
        schema.schema_type.as_ref(),
        Some(JsonSchemaType::Single(JsonSchemaPrimitiveType::Object))
    ) || matches!(
        schema.schema_type.as_ref(),
        Some(JsonSchemaType::Multiple(types)) if types.contains(&JsonSchemaPrimitiveType::Object)
    ) || schema.properties.is_some()
}

fn strict_schema_error(
    tool_name: &str,
    path: &str,
    message: impl Into<String>,
) -> serde_json::Error {
    serde_json::Error::io(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        format!(
            "strict tool schema for '{tool_name}' is invalid at {path}: {}",
            message.into()
        ),
    ))
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ResponsesApiWebSearchFilters {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_domains: Option<Vec<String>>,
}

impl From<ConfigWebSearchFilters> for ResponsesApiWebSearchFilters {
    fn from(filters: ConfigWebSearchFilters) -> Self {
        Self {
            allowed_domains: filters.allowed_domains,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ResponsesApiWebSearchUserLocation {
    #[serde(rename = "type")]
    pub r#type: WebSearchUserLocationType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ZaiWebSearchConfig {
    pub enable: bool,
    pub search_engine: String,
    pub search_result: bool,
}

impl From<ConfigWebSearchUserLocation> for ResponsesApiWebSearchUserLocation {
    fn from(user_location: ConfigWebSearchUserLocation) -> Self {
        Self {
            r#type: user_location.r#type,
            country: user_location.country,
            region: user_location.region,
            city: user_location.city,
            timezone: user_location.timezone,
        }
    }
}

#[cfg(test)]
#[path = "tool_spec_tests.rs"]
mod tests;
