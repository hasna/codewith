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
use crate::ranking::enumerate_catalog;
use crate::ranking::rank_catalog_all;

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
/// Headroom left under [`MAX_OUTPUT_BYTES`] for the response fields that are
/// only filled in after the page has been assembled (`has_more`,
/// `next_offset`).
const RESPONSE_ENVELOPE_RESERVE_BYTES: usize = 128;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListArgs {
    /// Task or capability to rank the catalog against. Omit it (or pass an
    /// empty string) to enumerate the whole catalog alphabetically instead of
    /// searching.
    query: Option<String>,
    /// Page size. Defaults to `DEFAULT_SKILL_MATCH_LIMIT`, maximum
    /// `MAX_SKILL_MATCH_LIMIT`.
    limit: Option<usize>,
    /// Rank of the first result to return, for paging past the first page.
    /// Defaults to 0. There is no ceiling: any `next_offset` this tool hands
    /// back is accepted, and an offset past the end returns an empty page
    /// rather than an error.
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
    /// Total number of catalog entries this call can reach, across every page.
    total_matches: usize,
    /// Whether anything on *this page* was shortened or skipped: a description
    /// clipped to its byte cap, or an entry dropped because its handles were
    /// unusable. Says nothing about whether further pages exist; that is
    /// `has_more`.
    truncated: bool,
    /// Whether this page stopped before the end of the result set. `false`
    /// means the results really are exhausted.
    has_more: bool,
    /// Value to pass back as `offset` to continue past this page. `None` once
    /// the results are exhausted. Always strictly greater than the `offset`
    /// that produced it, and never a value this tool would reject.
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
                "Search or enumerate the full current skill catalog. Pass `query` to rank by task or capability; omit `query` to list the entire catalog alphabetically, which is the reliable way to reach skills whose wording you cannot guess. Returns deterministic metadata matches plus opaque authority, package, and main_resource handles for skills.read. `total_matches` is the size of the whole result set and `has_more` says whether further pages exist. Ranking is lexical, so if the top matches look wrong, widen `limit` (default {DEFAULT_SKILL_MATCH_LIMIT}, max {MAX_SKILL_MATCH_LIMIT}), page deeper by passing the returned `next_offset` straight back as `offset` (always accepted, no ceiling), or drop `query` and enumerate. Explicit-only and disabled skills are never returned."
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
        let query = args.query.unwrap_or_default();
        validate_query(&query)?;
        let limit = args.limit.unwrap_or(DEFAULT_SKILL_MATCH_LIMIT);
        if !(1..=MAX_SKILL_MATCH_LIMIT).contains(&limit) {
            return Err(FunctionCallError::RespondToModel(format!(
                "limit must be between 1 and {MAX_SKILL_MATCH_LIMIT}"
            )));
        }
        // No offset ceiling: the model is told to feed `next_offset` straight
        // back, so rejecting an offset the tool itself produced would strand it
        // mid-walk. An offset past the end simply yields an empty final page.
        let offset = args.offset.unwrap_or(0);

        let snapshot = self.context.snapshot(&call.turn_id)?;
        // A blank query means "enumerate", not "match nothing": lexical ranking
        // cannot surface a skill whose wording the request does not share, so
        // enumeration is the only path that reaches the whole catalog.
        let results = if query.trim().is_empty() {
            enumerate_catalog(&snapshot.catalog)
        } else {
            rank_catalog_all(&snapshot.catalog, &query)
        };
        let total_matches = results.len();
        let mut response = ListResponse {
            matches: Vec::new(),
            total_matches,
            truncated: false,
            has_more: false,
            next_offset: None,
        };
        // Ranks consumed from `offset` onwards, including entries skipped for
        // unusable handles, so the caller can resume exactly where this page
        // stopped.
        let mut consumed = 0usize;
        for entry in results.into_iter().skip(offset) {
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
            if serialized_len(&response)?
                > MAX_OUTPUT_BYTES.saturating_sub(RESPONSE_ENVELOPE_RESERVE_BYTES)
            {
                response.matches.pop();
                response.truncated = true;
                // The cursor must strictly advance. If this entry was the only
                // one on the page, rewinding past it would hand back the same
                // `offset` and loop a cursor-following model forever, so leave
                // it consumed and let the caller resume after it instead.
                if !response.matches.is_empty() {
                    consumed = consumed.saturating_sub(1);
                }
                break;
            }
        }

        let scanned = offset.saturating_add(consumed);
        let has_more = scanned < total_matches;
        response.has_more = has_more;
        response.next_offset = has_more.then_some(scanned);
        debug_assert!(
            response.next_offset.is_none_or(|next| next > offset),
            "next_offset must strictly advance past the requested offset"
        );

        json_output(&response)
    }
}

/// A blank query is deliberately *not* an error: it selects enumeration. Only
/// genuinely unusable input is rejected.
fn validate_query(query: &str) -> Result<(), FunctionCallError> {
    if query.len() <= MAX_QUERY_BYTES && !query.chars().any(char::is_control) {
        return Ok(());
    }

    Err(FunctionCallError::RespondToModel(format!(
        "query must contain no control characters and be at most {MAX_QUERY_BYTES} bytes; omit it entirely to enumerate the whole catalog"
    )))
}
