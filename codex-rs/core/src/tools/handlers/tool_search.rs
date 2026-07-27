use crate::function_tool::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::ToolSearchOutput;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::tool_search_spec::create_tool_search_tool;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use bm25::Document;
use bm25::Language;
use bm25::SearchEngine;
use bm25::SearchEngineBuilder;
use codex_tools::LoadableToolSpec;
use codex_tools::TOOL_SEARCH_DEFAULT_LIMIT;
use codex_tools::TOOL_SEARCH_MAX_PROJECTION_BYTES;
use codex_tools::TOOL_SEARCH_MAX_RESULTS;
use codex_tools::TOOL_SEARCH_TOOL_NAME;
use codex_tools::ToolName;
use codex_tools::ToolSearchEntry;
use codex_tools::ToolSearchInfo;
use codex_tools::ToolSearchSourceInfo;
use codex_tools::ToolSpec;
use codex_tools::coalesce_loadable_tool_specs;
use std::collections::HashMap;

const TOOL_SEARCH_MAX_INITIAL_SOURCE_INFOS: usize = 64;
const TOOL_SEARCH_MAX_INITIAL_SOURCE_BYTES: usize = TOOL_SEARCH_MAX_PROJECTION_BYTES;

pub struct ToolSearchHandler {
    entries: Vec<ToolSearchEntry>,
    search_source_infos: Vec<ToolSearchSourceInfo>,
    search_engine: SearchEngine<usize>,
}

impl ToolSearchHandler {
    pub(crate) fn new(search_infos: Vec<ToolSearchInfo>) -> Self {
        let mut entries = Vec::with_capacity(search_infos.len());
        let mut search_source_infos = Vec::new();
        let mut source_indices = HashMap::new();
        let mut source_info_bytes = 0usize;
        for search_info in search_infos {
            if search_info.projection_size_bytes() > TOOL_SEARCH_MAX_PROJECTION_BYTES {
                continue;
            }
            entries.push(search_info.entry);
            if let Some(source_info) = search_info.source_info {
                if let Some(&index) = source_indices.get(&source_info.name) {
                    let existing: &mut ToolSearchSourceInfo = &mut search_source_infos[index];
                    if existing.description.is_none()
                        && let Some(description) = source_info.description
                        && source_info_bytes.saturating_add(description.len())
                            <= TOOL_SEARCH_MAX_INITIAL_SOURCE_BYTES
                    {
                        source_info_bytes += description.len();
                        existing.description = Some(description);
                    }
                    continue;
                }
                let candidate_bytes = source_info.name.len()
                    + source_info
                        .description
                        .as_ref()
                        .map_or(0, std::string::String::len);
                if search_source_infos.len() >= TOOL_SEARCH_MAX_INITIAL_SOURCE_INFOS
                    || source_info_bytes.saturating_add(candidate_bytes)
                        > TOOL_SEARCH_MAX_INITIAL_SOURCE_BYTES
                {
                    continue;
                }
                source_info_bytes += candidate_bytes;
                source_indices.insert(source_info.name.clone(), search_source_infos.len());
                search_source_infos.push(source_info);
            }
        }
        let documents: Vec<Document<usize>> = entries
            .iter()
            .map(|entry| entry.search_text.clone())
            .enumerate()
            .map(|(idx, search_text)| Document::new(idx, search_text))
            .collect();
        let search_engine =
            SearchEngineBuilder::<usize>::with_documents(Language::English, documents).build();

        Self {
            entries,
            search_source_infos,
            search_engine,
        }
    }
}

#[async_trait::async_trait]
impl ToolExecutor<ToolInvocation> for ToolSearchHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(TOOL_SEARCH_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        create_tool_search_tool(&self.search_source_infos, TOOL_SEARCH_DEFAULT_LIMIT)
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        true
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
    ) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
        let ToolInvocation { payload, .. } = invocation;

        let args = match payload {
            ToolPayload::ToolSearch { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::Fatal(format!(
                    "{TOOL_SEARCH_TOOL_NAME} handler received unsupported payload"
                )));
            }
        };

        let query = args.query.trim();
        if query.is_empty() {
            return Err(FunctionCallError::RespondToModel(
                "query must not be empty".to_string(),
            ));
        }
        let limit = args.limit.unwrap_or(TOOL_SEARCH_DEFAULT_LIMIT);

        if limit == 0 {
            return Err(FunctionCallError::RespondToModel(
                "limit must be greater than zero".to_string(),
            ));
        }

        if self.entries.is_empty() {
            return Ok(boxed_tool_output(ToolSearchOutput { tools: Vec::new() }));
        }

        let tools = self.search(query, limit)?;

        Ok(boxed_tool_output(ToolSearchOutput { tools }))
    }
}

impl CoreToolRuntime for ToolSearchHandler {}

impl ToolSearchHandler {
    fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<LoadableToolSpec>, FunctionCallError> {
        if limit > TOOL_SEARCH_MAX_RESULTS {
            return Err(FunctionCallError::RespondToModel(format!(
                "limit must not exceed {TOOL_SEARCH_MAX_RESULTS}"
            )));
        }
        let results = self
            .search_engine
            .search(query, limit)
            .into_iter()
            .map(|result| result.document.id)
            .filter_map(|id| self.entries.get(id));
        self.search_output_tools(results)
    }

    fn search_output_tools<'a>(
        &self,
        results: impl IntoIterator<Item = &'a ToolSearchEntry>,
    ) -> Result<Vec<LoadableToolSpec>, FunctionCallError> {
        let mut output = Vec::new();
        for entry in results.into_iter().take(TOOL_SEARCH_MAX_RESULTS) {
            let candidate = coalesce_loadable_tool_specs(
                output
                    .iter()
                    .cloned()
                    .chain(std::iter::once(entry.output.clone())),
            );
            let within_budget = serde_json::to_vec(&candidate)
                .is_ok_and(|serialized| serialized.len() <= TOOL_SEARCH_MAX_PROJECTION_BYTES);
            if within_budget {
                output = candidate;
            }
        }
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::handlers::DynamicToolHandler;
    use crate::tools::handlers::McpHandler;
    use codex_mcp::ToolInfo;
    use codex_protocol::dynamic_tools::DynamicToolSpec;
    use codex_tools::ResponsesApiNamespace;
    use codex_tools::ResponsesApiNamespaceTool;
    use codex_tools::ResponsesApiTool;
    use pretty_assertions::assert_eq;
    use rmcp::model::Tool;
    use std::sync::Arc;

    #[test]
    fn mixed_search_results_coalesce_mcp_namespaces() {
        let dynamic_tools = [DynamicToolSpec {
            namespace: Some("codex_app".to_string()),
            name: "automation_update".to_string(),
            description: "Create, update, view, or delete recurring automations.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "mode": { "type": "string" },
                },
                "required": ["mode"],
                "additionalProperties": false,
            }),
            defer_loading: true,
        }];
        let mcp_tools = [
            tool_info("calendar", "create_event", "Create events"),
            tool_info("calendar", "list_events", "List events"),
        ];
        let mut search_infos = mcp_tools
            .iter()
            .map(|tool| {
                McpHandler::new(tool.clone())
                    .expect("MCP tool should convert")
                    .search_info()
                    .expect("MCP handler should return search info")
            })
            .collect::<Vec<_>>();
        search_infos.extend(dynamic_tools.iter().map(|tool| {
            DynamicToolHandler::new(tool)
                .expect("dynamic tool should convert")
                .search_info()
                .expect("dynamic handler should return search info")
        }));
        let handler = ToolSearchHandler::new(search_infos);
        let results = [
            &handler.entries[0],
            &handler.entries[2],
            &handler.entries[1],
        ];

        let tools = handler
            .search_output_tools(results)
            .expect("mixed search output should serialize");

        assert_eq!(
            tools,
            vec![
                LoadableToolSpec::Namespace(ResponsesApiNamespace {
                    name: "mcp__calendar".to_string(),
                    description: "Tools in the mcp__calendar namespace.".to_string(),
                    tools: vec![
                        ResponsesApiNamespaceTool::Function(ResponsesApiTool {
                            name: "create_event".to_string(),
                            description: "Create events desktop tool".to_string(),
                            strict: false,
                            defer_loading: Some(true),
                            parameters: codex_tools::JsonSchema::object(
                                Default::default(),
                                /*required*/ None,
                                Some(false.into()),
                            ),
                            output_schema: None,
                        }),
                        ResponsesApiNamespaceTool::Function(ResponsesApiTool {
                            name: "list_events".to_string(),
                            description: "List events desktop tool".to_string(),
                            strict: false,
                            defer_loading: Some(true),
                            parameters: codex_tools::JsonSchema::object(
                                Default::default(),
                                /*required*/ None,
                                Some(false.into()),
                            ),
                            output_schema: None,
                        }),
                    ],
                }),
                LoadableToolSpec::Namespace(ResponsesApiNamespace {
                    name: "codex_app".to_string(),
                    description: "Tools in the codex_app namespace.".to_string(),
                    tools: vec![ResponsesApiNamespaceTool::Function(ResponsesApiTool {
                        name: "automation_update".to_string(),
                        description: "Create, update, view, or delete recurring automations."
                            .to_string(),
                        strict: false,
                        defer_loading: Some(true),
                        parameters: codex_tools::JsonSchema::object(
                            std::collections::BTreeMap::from([(
                                "mode".to_string(),
                                codex_tools::JsonSchema::string(/*description*/ None),
                            )]),
                            Some(vec!["mode".to_string()]),
                            Some(false.into()),
                        ),
                        output_schema: None,
                    })],
                }),
            ],
        );
    }

    #[test]
    fn search_output_is_bounded_by_result_count_and_serialized_bytes() {
        let entries = (0..9)
            .map(|index| ToolSearchEntry {
                search_text: format!("bounded tool {index}"),
                output: LoadableToolSpec::Function(ResponsesApiTool {
                    name: format!("bounded_tool_{index}"),
                    description: "x".repeat(6_000),
                    strict: false,
                    defer_loading: Some(true),
                    parameters: codex_tools::JsonSchema::default(),
                    output_schema: None,
                }),
            })
            .collect::<Vec<_>>();
        let handler = ToolSearchHandler::new(
            entries
                .iter()
                .cloned()
                .map(|entry| ToolSearchInfo {
                    entry,
                    source_info: None,
                })
                .collect(),
        );

        let output = handler
            .search_output_tools(entries.iter())
            .expect("bounded search output");
        assert!(output.len() <= 8, "tool_search returned too many results");
        assert!(
            serde_json::to_vec(&output)
                .expect("tool_search output should serialize")
                .len()
                <= 10_000,
            "tool_search exceeded the aggregate context ceiling"
        );
    }

    #[test]
    fn initial_declaration_bounds_unique_source_count() {
        let handler = ToolSearchHandler::new(
            (0..128)
                .map(|index| search_info_with_source(index, 8))
                .collect(),
        );

        assert!(
            handler.search_source_infos.len() <= TOOL_SEARCH_MAX_INITIAL_SOURCE_INFOS,
            "tool_search advertised too many unique sources: {}",
            handler.search_source_infos.len()
        );
    }

    #[test]
    fn initial_declaration_bounds_aggregate_source_bytes() {
        let handler = ToolSearchHandler::new(
            (0..64)
                .map(|index| search_info_with_source(index, 1_000))
                .collect(),
        );
        let aggregate_bytes = handler
            .search_source_infos
            .iter()
            .map(|source| {
                source.name.len()
                    + source
                        .description
                        .as_ref()
                        .map_or(0, std::string::String::len)
            })
            .sum::<usize>();

        assert!(
            aggregate_bytes <= TOOL_SEARCH_MAX_INITIAL_SOURCE_BYTES,
            "tool_search source descriptions exceeded the aggregate declaration budget: \
             {aggregate_bytes}"
        );
    }

    #[test]
    fn initial_declaration_deduplicates_sources_before_budgeting() {
        let mut search_infos = (0..128)
            .map(|index| search_info_with_source(index, 8))
            .collect::<Vec<_>>();
        for search_info in &mut search_infos {
            let source_info = search_info
                .source_info
                .as_mut()
                .expect("test search info should have a source");
            source_info.name = "shared-source".to_string();
        }

        let handler = ToolSearchHandler::new(search_infos);

        assert_eq!(
            handler.search_source_infos,
            vec![ToolSearchSourceInfo {
                name: "shared-source".to_string(),
                description: Some("x".repeat(8)),
            }]
        );
        assert_eq!(handler.entries.len(), 128);
    }

    #[test]
    fn initial_declaration_retains_later_source_description() {
        let mut without_description = search_info_with_source(0, 8);
        without_description
            .source_info
            .as_mut()
            .expect("test search info should have a source")
            .description = None;
        let mut with_description = search_info_with_source(1, 8);
        with_description
            .source_info
            .as_mut()
            .expect("test search info should have a source")
            .name = "source-0".to_string();

        let handler = ToolSearchHandler::new(vec![without_description, with_description]);

        assert_eq!(
            handler.search_source_infos,
            vec![ToolSearchSourceInfo {
                name: "source-0".to_string(),
                description: Some("x".repeat(8)),
            }]
        );
    }

    #[test]
    fn search_rejects_limit_above_maximum() {
        let entry = ToolSearchEntry {
            search_text: "bounded tool".to_string(),
            output: LoadableToolSpec::Function(ResponsesApiTool {
                name: "bounded_tool".to_string(),
                description: "Bounded tool.".to_string(),
                strict: false,
                defer_loading: Some(true),
                parameters: codex_tools::JsonSchema::default(),
                output_schema: None,
            }),
        };
        let handler = ToolSearchHandler::new(vec![ToolSearchInfo {
            entry,
            source_info: None,
        }]);

        let error = handler
            .search("bounded", 9)
            .expect_err("tool_search must reject a limit above the protocol maximum");
        assert!(error.to_string().contains("limit"));
    }

    fn tool_info(server_name: &str, tool_name: &str, description_prefix: &str) -> ToolInfo {
        ToolInfo {
            server_name: server_name.to_string(),
            supports_parallel_tool_calls: false,
            server_origin: None,
            callable_name: tool_name.to_string(),
            callable_namespace: format!("mcp__{server_name}"),
            namespace_description: None,
            tool: Tool::new(
                tool_name.to_string(),
                format!("{description_prefix} desktop tool"),
                Arc::new(rmcp::model::object(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false,
                }))),
            ),
            connector_id: None,
            connector_name: None,
            plugin_display_names: Vec::new(),
        }
    }

    fn search_info_with_source(index: usize, description_bytes: usize) -> ToolSearchInfo {
        ToolSearchInfo {
            entry: ToolSearchEntry {
                search_text: format!("source tool {index}"),
                output: LoadableToolSpec::Function(ResponsesApiTool {
                    name: format!("source_tool_{index}"),
                    description: "Source test tool.".to_string(),
                    strict: false,
                    defer_loading: Some(true),
                    parameters: codex_tools::JsonSchema::default(),
                    output_schema: None,
                }),
            },
            source_info: Some(ToolSearchSourceInfo {
                name: format!("source-{index}"),
                description: Some("x".repeat(description_bytes)),
            }),
        }
    }
}
