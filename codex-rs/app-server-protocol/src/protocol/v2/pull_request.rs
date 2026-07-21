use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

/// In-band outcome of a pull-request query.
///
/// Pull-request data is sourced asynchronously and may be unavailable (for
/// example when no GitHub remote is configured or the backing processor has not
/// been implemented yet). Rather than surfacing these outcomes as JSON-RPC
/// errors, every pull-request response carries a [`PullRequestQueryState`] so
/// clients can render empty/loading/degraded states in-band.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum PullRequestQueryState {
    /// The response data reflects the current, fully-loaded state.
    Ready,
    /// The response data is a partial snapshot while a refresh is in flight.
    Loading,
    /// Pull-request data could not be produced; see the accompanying message.
    Unavailable,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum PullRequestState {
    Open,
    Closed,
    Merged,
    Draft,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum PullRequestReviewDecision {
    Approved,
    ChangesRequested,
    ReviewRequired,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct PullRequest {
    pub id: String,
    #[ts(type = "number")]
    pub number: i64,
    pub title: String,
    pub state: PullRequestState,
    pub is_draft: bool,
    #[ts(type = "string | null")]
    pub author: Option<String>,
    #[ts(type = "string | null")]
    pub url: Option<String>,
    #[ts(type = "string | null")]
    pub repository: Option<String>,
    #[ts(type = "string | null")]
    pub head_ref: Option<String>,
    #[ts(type = "string | null")]
    pub base_ref: Option<String>,
    pub review_decision: Option<PullRequestReviewDecision>,
    #[ts(type = "number")]
    pub created_at: i64,
    #[ts(type = "number")]
    pub updated_at: i64,
}

/// Aggregate view of the pull requests relevant to the current repository.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct PullRequestOverview {
    pub total_open: u32,
    pub total_draft: u32,
    pub authored_by_viewer: u32,
    pub awaiting_viewer_review: u32,
    pub recently_updated: Vec<PullRequest>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct PullRequestOverviewParams {
    #[ts(optional = nullable)]
    pub base_repo_path: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct PullRequestOverviewResponse {
    pub query_state: PullRequestQueryState,
    pub overview: Option<PullRequestOverview>,
    #[ts(type = "string | null")]
    pub message: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct PullRequestListParams {
    #[ts(optional = nullable)]
    pub base_repo_path: Option<String>,
    #[ts(optional = nullable)]
    pub state: Option<PullRequestState>,
    #[ts(optional = nullable)]
    pub cursor: Option<String>,
    #[ts(optional = nullable)]
    pub limit: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct PullRequestListResponse {
    pub query_state: PullRequestQueryState,
    pub data: Vec<PullRequest>,
    pub next_cursor: Option<String>,
    #[ts(type = "string | null")]
    pub message: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct PullRequestReadParams {
    #[ts(type = "number")]
    pub number: i64,
    #[ts(optional = nullable)]
    pub base_repo_path: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct PullRequestReadResponse {
    pub query_state: PullRequestQueryState,
    pub pull_request: Option<PullRequest>,
    #[ts(type = "string | null")]
    pub message: Option<String>,
}
