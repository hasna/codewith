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
use crate::ranking::MAX_SKILL_MATCH_OFFSET;
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

const TOOL_NAME: &str = codex_core_skills::SKILLS_LIST_TOOL_NAME;
const MAX_QUERY_BYTES: usize = 4_096;
const MAX_NAME_BYTES: usize = 256;
const MAX_DESCRIPTION_BYTES: usize = 1_024;
/// Deepest rank the tool will materialize for one call: the last reachable page
/// plus one sentinel entry used to detect that more matches exist.
const MAX_INSPECTED_MATCHES: usize = MAX_SKILL_MATCH_OFFSET + MAX_SKILL_MATCH_LIMIT + 1;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListArgs {
    query: String,
    /// Page size. Defaults to `DEFAULT_SKILL_MATCH_LIMIT`, maximum
    /// `MAX_SKILL_MATCH_LIMIT`.
    limit: Option<usize>,
    /// Rank of the first match to return, for paging past the first page.
    /// Defaults to 0, maximum `MAX_SKILL_MATCH_OFFSET`.
    offset: Option<usize>,
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
    /// Value to pass back as `offset` to continue past this page. `None` once
    /// the ranked matches are exhausted.
    next_offset: Option<usize>,
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
            &format!(
                "Search the full current skill catalog by task or capability. Returns deterministic metadata matches plus opaque authority, package, and main_resource handles for skills.read. Ranking is lexical, so if the top matches look wrong, widen `limit` (default {DEFAULT_SKILL_MATCH_LIMIT}, max {MAX_SKILL_MATCH_LIMIT}) or page deeper by passing the returned `next_offset` back as `offset` (max {MAX_SKILL_MATCH_OFFSET}) before rewording the query. Explicit-only and disabled skills are never returned."
            ),
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
        let offset = args.offset.unwrap_or(0);
        if offset > MAX_SKILL_MATCH_OFFSET {
            return Err(FunctionCallError::RespondToModel(format!(
                "offset must be at most {MAX_SKILL_MATCH_OFFSET}"
            )));
        }

        let snapshot = self.context.snapshot(&call.turn_id)?;
        // One past the requested page, so `next_offset` can distinguish "the
        // page is full" from "there is genuinely nothing after it".
        let inspected = offset
            .saturating_add(limit)
            .saturating_add(1)
            .min(MAX_INSPECTED_MATCHES);
        let ranked = rank_catalog(&snapshot.catalog, &args.query, inspected);
        let ranked_len = ranked.len();
        let mut response = ListResponse {
            matches: Vec::new(),
            truncated: false,
            next_offset: None,
        };
        // Ranks consumed from `offset` onwards, including entries skipped for
        // unusable handles, so the caller can resume exactly where this page
        // stopped.
        let mut consumed = 0usize;
        for entry in ranked.into_iter().skip(offset) {
            if response.matches.len() == limit {
                break;
            }
            consumed = consumed.saturating_add(1);
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
                consumed = consumed.saturating_sub(1);
                break;
            }
        }

        let scanned = offset.saturating_add(consumed);
        // Either ranked matches remain past this page, or the ranking itself
        // was clipped at `inspected` and there may be more beyond it. Both mean
        // the model still has somewhere to go.
        let has_more = scanned < ranked_len || ranked_len >= inspected;
        response.truncated |= has_more;
        response.next_offset = has_more.then_some(scanned);

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
