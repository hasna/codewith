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

use crate::catalog::SkillPackageId;
use crate::provider::SkillSearchRequest;

use super::MAX_HANDLE_BYTES;
use super::MAX_OUTPUT_BYTES;
use super::SkillToolAuthority;
use super::SkillToolContext;
use super::bounded_text;
use super::is_bounded_handle;
use super::json_output;
use super::parse_args;
use super::serialized_len;
use super::skill_function_tool;
use super::skill_tool_name;
use super::validate_handle;

const TOOL_NAME: &str = "search";
const MAX_QUERY_BYTES: usize = 1_024;
const MAX_MATCHES: usize = 20;
const MAX_INSPECTED_MATCHES: usize = 100;
const MAX_TITLE_BYTES: usize = 512;
const MAX_SNIPPET_BYTES: usize = 2_048;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SearchArgs {
    authority: SkillToolAuthority,
    package: String,
    query: String,
}

#[derive(Debug, Eq, JsonSchema, PartialEq, Serialize)]
#[schemars(deny_unknown_fields)]
struct SearchMatch {
    resource: String,
    title: String,
    snippet: String,
}

#[derive(Debug, Eq, JsonSchema, PartialEq, Serialize)]
#[schemars(deny_unknown_fields)]
struct SearchResponse {
    authority: SkillToolAuthority,
    package: String,
    matches: Vec<SearchMatch>,
    truncated: bool,
}

#[derive(Clone)]
pub(super) struct SearchTool {
    pub(super) context: SkillToolContext,
}

#[async_trait::async_trait]
impl ToolExecutor<ToolCall> for SearchTool {
    fn tool_name(&self) -> ToolName {
        skill_tool_name(TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        skill_function_tool::<SearchArgs, SearchResponse>(
            TOOL_NAME,
            "Search one available skill package for supporting resources. Copy the opaque authority and package identifiers from the current skills catalog; do not derive paths from them. Results are bounded and return opaque resource identifiers for skills.read.",
        )
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::DirectModelOnly
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        true
    }

    async fn handle(&self, call: ToolCall) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
        let args: SearchArgs = parse_args(&call)?;
        let authority = args.authority.to_authority()?;
        validate_handle("package", &args.package, MAX_HANDLE_BYTES)?;
        validate_query(&args.query)?;
        let snapshot = self.context.snapshot(&call.turn_id)?;
        if self
            .context
            .available_package(&snapshot, &authority, &args.package)
            .is_none()
        {
            return Err(FunctionCallError::RespondToModel(
                "skill package is not available from the requested authority in this turn"
                    .to_string(),
            ));
        }

        let result = snapshot
            .routes
            .search(SkillSearchRequest {
                authority: authority.clone(),
                package: SkillPackageId(args.package.clone()),
                query: args.query,
            })
            .await
            .map_err(|_| {
                FunctionCallError::RespondToModel(
                    "skill provider could not search the requested package".to_string(),
                )
            })?;

        let mut response = SearchResponse {
            authority: SkillToolAuthority::from_authority(&authority),
            package: args.package,
            matches: Vec::new(),
            truncated: result.matches.len() > MAX_INSPECTED_MATCHES,
        };
        for search_match in result.matches.into_iter().take(MAX_INSPECTED_MATCHES) {
            if response.matches.len() == MAX_MATCHES {
                response.truncated = true;
                break;
            }
            if !is_bounded_handle(&search_match.resource.0, MAX_HANDLE_BYTES) {
                response.truncated = true;
                continue;
            }
            let (title, title_truncated) = bounded_text(&search_match.title, MAX_TITLE_BYTES);
            let (snippet, snippet_truncated) =
                bounded_text(&search_match.snippet, MAX_SNIPPET_BYTES);
            response.matches.push(SearchMatch {
                resource: search_match.resource.0,
                title,
                snippet,
            });
            response.truncated |= title_truncated || snippet_truncated;
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
