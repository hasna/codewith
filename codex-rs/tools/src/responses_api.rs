use crate::JsonSchema;
use crate::ToolDefinition;
use crate::ToolName;
use crate::parse_dynamic_tool;
use crate::parse_mcp_tool;
use codex_protocol::dynamic_tools::DynamicToolSpec;
use serde::Deserialize;
use serde::Serialize;
use serde::Serializer;
use serde::ser::Error as _;
use serde::ser::SerializeStruct;
use serde_json::Value;

const WEB_RUN_DESCRIPTION: &str = include_str!("../web_run_description.md");

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

/// A namespace spec whose wire identity is reserved for the built-in web search
/// implementation.
///
/// The fields are private so generic extension, MCP, deferred, and persisted
/// namespace specs cannot gain built-in provenance from a matching name alone.
#[derive(Debug, Clone, PartialEq)]
pub struct BuiltInWebSearchToolSpec {
    namespace: ResponsesApiNamespace,
}

impl BuiltInWebSearchToolSpec {
    pub(crate) fn new() -> Self {
        let parameters = crate::parse_tool_input_schema_without_compaction(
            &codex_api::search_commands_tool_schema(),
        )
        .unwrap_or_else(|err| panic!("canonical web-search schema should parse: {err}"));
        Self {
            namespace: ResponsesApiNamespace {
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
            },
        }
    }

    pub fn namespace(&self) -> &ResponsesApiNamespace {
        &self.namespace
    }

    pub(crate) fn set_run_description(&mut self, description: String) {
        let [ResponsesApiNamespaceTool::Function(tool)] = self.namespace.tools.as_mut_slice()
        else {
            unreachable!("built-in web search must contain exactly one function");
        };
        tool.description = description;
    }
}

impl ResponsesApiNamespace {
    /// Returns the first custom tool under a reserved namespace, or `*` when a
    /// reserved namespace has no declared tools.
    pub fn forbidden_reserved_tool_name(&self) -> Option<&str> {
        self.tools
            .iter()
            .find_map(|tool| match tool {
                ResponsesApiNamespaceTool::Function(tool)
                    if crate::is_forbidden_first_party_namespace_tool(&self.name, &tool.name) =>
                {
                    Some(tool.name.as_str())
                }
                ResponsesApiNamespaceTool::Function(_) => None,
            })
            .or_else(|| {
                (self.tools.is_empty() && crate::is_reserved_responses_namespace(&self.name))
                    .then_some("*")
            })
    }
}

impl Serialize for ResponsesApiNamespace {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if let Some(tool_name) = self.forbidden_reserved_tool_name() {
            return Err(S::Error::custom(format!(
                "refusing to serialize custom tool under reserved Responses namespace: {}.{tool_name}",
                self.name
            )));
        }

        serialize_namespace(self, serializer)
    }
}

impl Serialize for BuiltInWebSearchToolSpec {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_namespace(&self.namespace, serializer)
    }
}

fn serialize_namespace<S>(
    namespace: &ResponsesApiNamespace,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let mut state = serializer.serialize_struct("ResponsesApiNamespace", 3)?;
    state.serialize_field("name", &namespace.name)?;
    state.serialize_field("description", &namespace.description)?;
    state.serialize_field("tools", &namespace.tools)?;
    state.end()
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
