use std::sync::Arc;

use codex_extension_api::FunctionCallError;
use codex_extension_api::ResponsesApiTool;
use codex_extension_api::ToolCall;
use codex_extension_api::ToolExecutor;
use codex_extension_api::ToolName;
use codex_extension_api::ToolOutput;
use codex_extension_api::ToolPayload;
use codex_extension_api::ToolSpec;
use codex_extension_api::parse_tool_input_schema;
use codex_protocol::models::ResponseInputItem;
use codex_tools::ResponsesApiNamespace;
use codex_tools::ResponsesApiNamespaceTool;
use codex_tools::default_namespace_description;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use crate::catalog::SkillAuthority;
use crate::catalog::SkillCatalogEntry;
use crate::catalog::SkillSourceKind;
use crate::state::SkillsThreadState;
use crate::state::SkillsToolSnapshot;

mod read;
mod schema;
mod search;

const SKILLS_NAMESPACE: &str = "skills";
const MAX_ARGUMENT_BYTES: usize = 16 * 1024;
const MAX_HANDLE_BYTES: usize = 2_048;
const MAX_OUTPUT_BYTES: usize = 32 * 1024;

pub(crate) fn skill_tools(
    thread_state: Arc<SkillsThreadState>,
) -> Vec<Arc<dyn ToolExecutor<ToolCall>>> {
    let context = SkillToolContext { thread_state };
    vec![
        Arc::new(search::SearchTool {
            context: context.clone(),
        }),
        Arc::new(read::ReadTool { context }),
    ]
}

#[derive(Clone)]
struct SkillToolContext {
    thread_state: Arc<SkillsThreadState>,
}

impl SkillToolContext {
    fn snapshot(&self, turn_id: &str) -> Result<SkillsToolSnapshot, FunctionCallError> {
        self.thread_state.tool_snapshot(turn_id).ok_or_else(|| {
            FunctionCallError::RespondToModel(
                "skill resources are unavailable because the current turn catalog is not loaded"
                    .to_string(),
            )
        })
    }

    fn available_package<'a>(
        &self,
        snapshot: &'a SkillsToolSnapshot,
        authority: &SkillAuthority,
        package: &str,
    ) -> Option<&'a SkillCatalogEntry> {
        snapshot.catalog.entries.iter().find(|entry| {
            entry.is_explicitly_loadable() && &entry.authority == authority && entry.id.0 == package
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[schemars(deny_unknown_fields)]
#[serde(deny_unknown_fields)]
pub(crate) struct SkillToolAuthority {
    kind: SkillToolAuthorityKind,
    id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
enum SkillToolAuthorityKind {
    Host,
    Executor,
    Remote,
    Custom(String),
}

impl SkillToolAuthority {
    pub(crate) fn from_authority(authority: &SkillAuthority) -> Self {
        Self {
            kind: match &authority.kind {
                SkillSourceKind::Host => SkillToolAuthorityKind::Host,
                SkillSourceKind::Executor => SkillToolAuthorityKind::Executor,
                SkillSourceKind::Remote => SkillToolAuthorityKind::Remote,
                SkillSourceKind::Custom(kind) => SkillToolAuthorityKind::Custom(kind.clone()),
            },
            id: authority.id.clone(),
        }
    }

    fn to_authority(&self) -> Result<SkillAuthority, FunctionCallError> {
        validate_handle("authority.id", &self.id, MAX_HANDLE_BYTES)?;
        let kind = match &self.kind {
            SkillToolAuthorityKind::Host => SkillSourceKind::Host,
            SkillToolAuthorityKind::Executor => SkillSourceKind::Executor,
            SkillToolAuthorityKind::Remote => SkillSourceKind::Remote,
            SkillToolAuthorityKind::Custom(kind) => {
                validate_handle("authority.kind.value", kind, MAX_HANDLE_BYTES)?;
                SkillSourceKind::custom(kind.clone())
            }
        };
        Ok(SkillAuthority::new(kind, self.id.clone()))
    }
}

pub(crate) fn catalog_tool_handles(entry: &SkillCatalogEntry) -> Option<String> {
    let authority = SkillToolAuthority::from_authority(&entry.authority);
    authority.to_authority().ok()?;
    if !is_bounded_handle(&entry.id.0, MAX_HANDLE_BYTES)
        || !is_bounded_handle(&entry.main_prompt.0, MAX_HANDLE_BYTES)
    {
        return None;
    }
    let authority = serde_json::to_string(&authority).ok()?;
    let package = serde_json::to_string(&entry.id.0).ok()?;
    let main_resource = serde_json::to_string(&entry.main_prompt.0).ok()?;
    Some(format!(
        "authority: {authority}; package: {package}; main resource: {main_resource}"
    ))
}

fn skill_tool_name(name: &str) -> ToolName {
    ToolName::namespaced(SKILLS_NAMESPACE, name)
}

fn skill_function_tool<I: JsonSchema, O: JsonSchema>(name: &str, description: &str) -> ToolSpec {
    let tool = ResponsesApiTool {
        name: name.to_string(),
        description: description.to_string(),
        strict: false,
        defer_loading: None,
        parameters: parse_tool_input_schema(&schema::input_schema_for::<I>())
            .unwrap_or_else(|err| panic!("generated input schema for {name} should parse: {err}")),
        output_schema: Some(schema::output_schema_for::<O>()),
    };

    ToolSpec::Namespace(ResponsesApiNamespace {
        name: SKILLS_NAMESPACE.to_string(),
        description: default_namespace_description(SKILLS_NAMESPACE),
        tools: vec![ResponsesApiNamespaceTool::Function(tool)],
    })
}

fn parse_args<T: for<'de> Deserialize<'de>>(call: &ToolCall) -> Result<T, FunctionCallError> {
    let arguments = call.function_arguments()?;
    if arguments.len() > MAX_ARGUMENT_BYTES {
        return Err(FunctionCallError::RespondToModel(format!(
            "skill tool arguments must be at most {MAX_ARGUMENT_BYTES} bytes"
        )));
    }
    let value = if arguments.trim().is_empty() {
        Value::Object(serde_json::Map::new())
    } else {
        serde_json::from_str(arguments)
            .map_err(|err| FunctionCallError::RespondToModel(err.to_string()))?
    };
    serde_json::from_value(value).map_err(|err| FunctionCallError::RespondToModel(err.to_string()))
}

fn validate_handle(name: &str, value: &str, max_bytes: usize) -> Result<(), FunctionCallError> {
    if is_bounded_handle(value, max_bytes) {
        return Ok(());
    }

    Err(FunctionCallError::RespondToModel(format!(
        "{name} must be non-empty, contain no control characters, and be at most {max_bytes} bytes"
    )))
}

fn is_bounded_handle(value: &str, max_bytes: usize) -> bool {
    !value.is_empty() && value.len() <= max_bytes && !value.chars().any(char::is_control)
}

fn bounded_text(value: &str, max_bytes: usize) -> (String, bool) {
    if value.len() <= max_bytes {
        return (value.to_string(), false);
    }

    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    (value[..end].to_string(), true)
}

fn serialized_len<T: Serialize>(value: &T) -> Result<usize, FunctionCallError> {
    serde_json::to_vec(value)
        .map(|value| value.len())
        .map_err(|err| FunctionCallError::Fatal(format!("failed to serialize tool output: {err}")))
}

fn json_output<T: Serialize>(value: &T) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
    let value = serde_json::to_value(value).map_err(|err| {
        FunctionCallError::Fatal(format!("failed to serialize tool output: {err}"))
    })?;
    Ok(Box::new(SkillJsonToolOutput { value }))
}

struct SkillJsonToolOutput {
    value: Value,
}

impl ToolOutput for SkillJsonToolOutput {
    fn log_preview(&self) -> String {
        "[skill resource output]".to_string()
    }

    fn success_for_logging(&self) -> bool {
        true
    }

    fn to_response_item(&self, call_id: &str, payload: &ToolPayload) -> ResponseInputItem {
        codex_extension_api::JsonToolOutput::new(self.value.clone())
            .to_response_item(call_id, payload)
    }

    fn post_tool_use_response(&self, call_id: &str, payload: &ToolPayload) -> Option<Value> {
        codex_extension_api::JsonToolOutput::new(self.value.clone())
            .post_tool_use_response(call_id, payload)
    }

    fn code_mode_result(&self, _payload: &ToolPayload) -> Value {
        self.value.clone()
    }
}
