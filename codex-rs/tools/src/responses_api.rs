use crate::JsonSchema;
use crate::ToolDefinition;
use crate::ToolName;
use crate::ToolSpec;
use crate::parse_dynamic_tool;
use crate::parse_mcp_tool;
use crate::parse_tool_input_schema_without_compaction;
use codex_protocol::dynamic_tools::DynamicToolSpec;
use codex_tool_contracts::IMAGE_GENERATION_DESCRIPTION;
use codex_tool_contracts::IMAGE_GENERATION_SCHEMA_JSON;
use codex_tool_contracts::WEB_RUN_DESCRIPTION;
use codex_tool_contracts::WEB_RUN_SCHEMA_JSON;
use serde::Deserialize;
use serde::Serialize;
use serde::Serializer;
use serde_json::Value;
use std::sync::LazyLock;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FreeformTool {
    pub name: String,
    pub description: String,
    pub format: FreeformToolFormat,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FreeformToolFormat {
    pub r#type: String,
    pub syntax: String,
    pub definition: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ResponsesApiTool {
    pub name: String,
    pub description: String,
    /// When strict is set to true, `create_tools_json_for_responses_api`
    /// validates that the JSON schema is compatible with OpenAI strict mode.
    pub strict: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub defer_loading: Option<bool>,
    pub parameters: JsonSchema,
    #[serde(skip)]
    pub output_schema: Option<Value>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type")]
#[allow(clippy::large_enum_variant)]
pub enum LoadableToolSpec {
    #[allow(dead_code)]
    #[serde(rename = "function")]
    Function(ResponsesApiTool),
    #[serde(rename = "namespace")]
    Namespace(ResponsesApiNamespace),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResponsesApiNamespace {
    pub name: String,
    pub description: String,
    pub tools: Vec<ResponsesApiNamespaceTool>,
}

static WEB_RUN_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(WEB_RUN_SCHEMA_JSON)
        .unwrap_or_else(|err| panic!("canonical web-search schema should deserialize: {err}"))
});

static CANONICAL_WEB_SEARCH_NAMESPACE: LazyLock<ResponsesApiNamespace> = LazyLock::new(|| {
    let parameters = parse_tool_input_schema_without_compaction(&WEB_RUN_SCHEMA)
        .unwrap_or_else(|err| panic!("canonical web-search schema should parse: {err}"));
    ResponsesApiNamespace {
        name: "web".to_string(),
        description: default_namespace_description("web"),
        tools: vec![ResponsesApiNamespaceTool::Function(ResponsesApiTool {
            name: "run".to_string(),
            description: WEB_RUN_DESCRIPTION.to_string(),
            strict: false,
            defer_loading: None,
            parameters,
            output_schema: None,
        })],
    }
});

static CANONICAL_WEB_SEARCH_NAMESPACE_VALUE: LazyLock<Value> = LazyLock::new(|| {
    serde_json::to_value(ToolSpec::Namespace(CANONICAL_WEB_SEARCH_NAMESPACE.clone()))
        .unwrap_or_else(|err| panic!("canonical web-search namespace should serialize: {err}"))
});

impl Serialize for ResponsesApiNamespace {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct Namespace<'a, T> {
            name: &'a str,
            description: &'a str,
            tools: T,
        }

        #[derive(Serialize)]
        struct CanonicalTool<'a> {
            #[serde(rename = "type")]
            tool_type: &'static str,
            name: &'a str,
            description: &'a str,
            strict: bool,
            parameters: &'a Value,
        }

        if is_canonical_web_search_namespace(self) {
            let tool = CanonicalTool {
                tool_type: "function",
                name: "run",
                description: WEB_RUN_DESCRIPTION,
                strict: false,
                parameters: &WEB_RUN_SCHEMA,
            };
            return Namespace {
                name: self.name.as_str(),
                description: self.description.as_str(),
                tools: [tool],
            }
            .serialize(serializer);
        }

        Namespace {
            name: self.name.as_str(),
            description: self.description.as_str(),
            tools: self.tools.as_slice(),
        }
        .serialize(serializer)
    }
}

pub fn canonical_web_search_namespace() -> ResponsesApiNamespace {
    CANONICAL_WEB_SEARCH_NAMESPACE.clone()
}

pub fn canonical_image_generation_namespace() -> ResponsesApiNamespace {
    let schema = serde_json::from_str(IMAGE_GENERATION_SCHEMA_JSON).unwrap_or_else(|err| {
        panic!("canonical image-generation schema should deserialize: {err}")
    });
    let parameters = parse_tool_input_schema_without_compaction(&schema)
        .unwrap_or_else(|err| panic!("canonical image-generation schema should parse: {err}"));
    ResponsesApiNamespace {
        name: "images".to_string(),
        description: default_namespace_description("images"),
        tools: vec![ResponsesApiNamespaceTool::Function(ResponsesApiTool {
            name: "imagegen".to_string(),
            description: IMAGE_GENERATION_DESCRIPTION.to_string(),
            strict: false,
            defer_loading: None,
            parameters,
            output_schema: None,
        })],
    }
}

pub fn is_canonical_web_search_namespace(namespace: &ResponsesApiNamespace) -> bool {
    namespace == &*CANONICAL_WEB_SEARCH_NAMESPACE
}

pub fn is_canonical_image_generation_namespace(namespace: &ResponsesApiNamespace) -> bool {
    namespace == &canonical_image_generation_namespace()
}

pub fn is_canonical_web_search_namespace_value(value: &Value) -> bool {
    value == &*CANONICAL_WEB_SEARCH_NAMESPACE_VALUE
}

pub fn default_namespace_description(namespace_name: &str) -> String {
    format!("Tools in the {namespace_name} namespace.")
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type")]
pub enum ResponsesApiNamespaceTool {
    #[serde(rename = "function")]
    Function(ResponsesApiTool),
}

pub fn dynamic_tool_to_responses_api_tool(
    tool: &DynamicToolSpec,
) -> Result<ResponsesApiTool, serde_json::Error> {
    Ok(tool_definition_to_responses_api_tool(parse_dynamic_tool(
        tool,
    )?))
}

pub fn coalesce_loadable_tool_specs(
    specs: impl IntoIterator<Item = LoadableToolSpec>,
) -> Vec<LoadableToolSpec> {
    let mut coalesced_specs = Vec::new();
    for spec in specs {
        match spec {
            LoadableToolSpec::Function(tool) => {
                coalesced_specs.push(LoadableToolSpec::Function(tool));
            }
            LoadableToolSpec::Namespace(mut namespace) => {
                if let Some(existing_namespace) =
                    coalesced_specs.iter_mut().find_map(|spec| match spec {
                        LoadableToolSpec::Namespace(existing_namespace)
                            if existing_namespace.name == namespace.name =>
                        {
                            Some(existing_namespace)
                        }
                        LoadableToolSpec::Function(_) | LoadableToolSpec::Namespace(_) => None,
                    })
                {
                    existing_namespace.tools.append(&mut namespace.tools);
                } else {
                    coalesced_specs.push(LoadableToolSpec::Namespace(namespace));
                }
            }
        }
    }
    coalesced_specs
}

pub fn mcp_tool_to_responses_api_tool(
    tool_name: &ToolName,
    tool: &rmcp::model::Tool,
) -> Result<ResponsesApiTool, serde_json::Error> {
    Ok(tool_definition_to_responses_api_tool(
        parse_mcp_tool(tool)?.renamed(tool_name.name.clone()),
    ))
}

pub fn mcp_tool_to_deferred_responses_api_tool(
    tool_name: &ToolName,
    tool: &rmcp::model::Tool,
) -> Result<ResponsesApiTool, serde_json::Error> {
    Ok(tool_definition_to_responses_api_tool(
        parse_mcp_tool(tool)?
            .renamed(tool_name.name.clone())
            .into_deferred(),
    ))
}

pub fn tool_definition_to_responses_api_tool(tool_definition: ToolDefinition) -> ResponsesApiTool {
    ResponsesApiTool {
        name: tool_definition.name,
        description: tool_definition.description,
        strict: false,
        defer_loading: tool_definition.defer_loading.then_some(true),
        parameters: tool_definition.input_schema,
        output_schema: tool_definition.output_schema,
    }
}

#[cfg(test)]
#[path = "responses_api_tests.rs"]
mod tests;
