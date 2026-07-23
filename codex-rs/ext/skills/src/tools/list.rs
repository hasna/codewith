use codex_extension_api::FunctionCallError;
use codex_extension_api::ToolCall;
use codex_extension_api::ToolExecutor;
use codex_extension_api::ToolName;
use codex_extension_api::ToolOutput;
use codex_extension_api::ToolSpec;
use codex_tools::ToolExposure;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;

use crate::ranking::DEFAULT_SKILL_MATCH_LIMIT;
use crate::ranking::MAX_SKILL_MATCH_LIMIT;
use crate::ranking::rank_catalog;

use super::MAX_HANDLE_BYTES;
use super::MAX_OUTPUT_BYTES;
use super::SkillToolAuthority;
use super::SkillToolContext;
use super::bounded_text;
use super::catalog_tool_handles;
use super::json_output;
use super::parse_args;
use super::serialized_len;
use super::skill_function_tool;
use super::skill_tool_name;

const TOOL_NAME: &str = "list";
const MAX_QUERY_BYTES: usize = 4_096;
const MAX_NAME_BYTES: usize = 256;
const MAX_DESCRIPTION_BYTES: usize = 1_024;
const MAX_INSPECTED_MATCHES: usize = 100;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListArgs {
    query: String,
    limit: Option<usize>,
}

#[derive(Debug, Eq, JsonSchema, PartialEq, Serialize)]
#[schemars(deny_unknown_fields)]
struct ListMatch {
    name: String,
    description: String,
    authority: SkillToolAuthority,
    package: String,
    main_resource: String,
}

#[derive(Debug, Eq, JsonSchema, PartialEq, Serialize)]
#[schemars(deny_unknown_fields)]
struct ListResponse {
    matches: Vec<ListMatch>,
    truncated: bool,
}

#[derive(Clone)]
pub(super) struct ListTool {
    pub(super) context: SkillToolContext,
}

#[async_trait::async_trait]
impl ToolExecutor<ToolCall> for ListTool {
    fn tool_name(&self) -> ToolName {
        skill_tool_name(TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        skill_function_tool::<ListArgs, ListResponse>(
            TOOL_NAME,
            "Search the full current skill catalog by task or capability. Returns at most five deterministic metadata matches and opaque authority, package, and main_resource handles for skills.read. Explicit-only and disabled skills are never returned.",
        )
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::DirectModelOnly
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        true
    }

    async fn handle(&self, call: ToolCall) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
        let args: ListArgs = parse_args(&call)?;
        validate_query(&args.query)?;
        let limit = args.limit.unwrap_or(DEFAULT_SKILL_MATCH_LIMIT);
        if !(1..=MAX_SKILL_MATCH_LIMIT).contains(&limit) {
            return Err(FunctionCallError::RespondToModel(format!(
                "limit must be between 1 and {MAX_SKILL_MATCH_LIMIT}"
            )));
        }

        let snapshot = self.context.snapshot(&call.turn_id)?;
        let ranked = rank_catalog(&snapshot.catalog, &args.query, MAX_INSPECTED_MATCHES);
        let mut response = ListResponse {
            matches: Vec::new(),
            truncated: ranked.len() > limit,
        };
        for entry in ranked {
            if response.matches.len() == limit {
                response.truncated = true;
                break;
            }
            if catalog_tool_handles(entry).is_none()
                || entry.name.is_empty()
                || entry.name.len() > MAX_NAME_BYTES
            {
                response.truncated = true;
                continue;
            }
            let description = entry
                .short_description
                .as_deref()
                .unwrap_or(entry.description.as_str());
            let (description, description_truncated) =
                bounded_text(description, MAX_DESCRIPTION_BYTES);
            let candidate = ListMatch {
                name: entry.name.clone(),
                description,
                authority: SkillToolAuthority::from_authority(&entry.authority),
                package: entry.id.0.clone(),
                main_resource: entry.main_prompt.0.clone(),
            };
            if candidate.package.len() > MAX_HANDLE_BYTES
                || candidate.main_resource.len() > MAX_HANDLE_BYTES
            {
                response.truncated = true;
                continue;
            }
            response.matches.push(candidate);
            response.truncated |= description_truncated;
            if serialized_len(&response)? > MAX_OUTPUT_BYTES {
                response.matches.pop();
                response.truncated = true;
                break;
            }
        }

        json_output(&response)
    }
}

fn validate_query(query: &str) -> Result<(), FunctionCallError> {
    if !query.trim().is_empty()
        && query.len() <= MAX_QUERY_BYTES
        && !query.chars().any(char::is_control)
    {
        return Ok(());
    }

    Err(FunctionCallError::RespondToModel(format!(
        "query must contain non-whitespace text, contain no control characters, and be at most {MAX_QUERY_BYTES} bytes"
    )))
}
