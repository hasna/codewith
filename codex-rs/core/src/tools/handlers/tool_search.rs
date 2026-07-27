use crate::function_tool::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::ToolSearchOutput;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::tool_search_spec::bounded_source_infos;
use crate::tools::handlers::tool_search_spec::create_tool_search_tool;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use crate::tools::tool_search_history::ReservedNamespacePolicy;
use crate::tools::tool_search_history::ToolSearchHistoryBudget;
use bm25::Document;
use bm25::Language;
use bm25::SearchEngine;
use bm25::SearchEngineBuilder;
use codex_model_provider_info::WireApi;
use codex_tools::LoadableToolSpec;
use codex_tools::TOOL_SEARCH_DEFAULT_LIMIT;
use codex_tools::TOOL_SEARCH_MAX_DECLARATION_BYTES;
use codex_tools::TOOL_SEARCH_MAX_PROJECTION_BYTES;
use codex_tools::TOOL_SEARCH_MAX_RESULTS;
use codex_tools::TOOL_SEARCH_MAX_SEARCH_TEXT_BYTES;
use codex_tools::TOOL_SEARCH_TOOL_NAME;
use codex_tools::ToolName;
use codex_tools::ToolSearchEntry;
use codex_tools::ToolSearchInfo;
use codex_tools::ToolSearchSourceInfo;
use codex_tools::ToolSpec;
use codex_tools::coalesce_loadable_tool_specs;
use codex_utils_string::take_bytes_at_char_boundary;
use std::collections::BTreeMap;
use tokio::sync::Mutex;
use tokio::sync::OnceCell;

pub struct ToolSearchHandler {
    entries: Vec<ToolSearchEntry>,
    search_source_infos: Vec<ToolSearchSourceInfo>,
    search_engine: SearchEngine<usize>,
    history_budget: OnceCell<Mutex<ToolSearchHistoryBudget>>,
}

impl ToolSearchHandler {
    pub(crate) fn new(search_infos: Vec<ToolSearchInfo>) -> Self {
        let mut entries = Vec::with_capacity(search_infos.len());
        let mut source_infos = BTreeMap::<String, Option<String>>::new();
        for mut search_info in search_infos {
            if search_info.projection_size_bytes() > TOOL_SEARCH_MAX_PROJECTION_BYTES {
                continue;
            }
            search_info.entry.search_text = take_bytes_at_char_boundary(
                &search_info.entry.search_text,
                TOOL_SEARCH_MAX_SEARCH_TEXT_BYTES,
            )
            .to_string();
            entries.push(search_info.entry);
            if let Some(source_info) = search_info.source_info {
                if source_info.name.len() > TOOL_SEARCH_MAX_DECLARATION_BYTES {
                    continue;
                }
                let description = source_info.description.map(|description| {
                    take_bytes_at_char_boundary(&description, TOOL_SEARCH_MAX_DECLARATION_BYTES)
                        .to_string()
                });
                source_infos
                    .entry(source_info.name)
                    .and_modify(|existing| match (existing.as_ref(), description.as_ref()) {
                        (None, Some(_)) => *existing = description.clone(),
                        (Some(current), Some(candidate)) if candidate < current => {
                            *existing = description.clone();
                        }
                        (None, None) | (Some(_), None) | (Some(_), Some(_)) => {}
                    })
                    .or_insert(description);
            }
        }
        let search_source_infos = bounded_source_infos(source_infos);
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
            history_budget: OnceCell::new(),
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
        let ToolInvocation {
            payload,
            session,
            turn,
            ..
        } = invocation;

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
        let reserved_namespace_policy = match turn.provider.info().wire_api {
            WireApi::Chat => ReservedNamespacePolicy::Allow,
            WireApi::Responses => ReservedNamespacePolicy::Reject,
        };
        let history_budget = self
            .history_budget
            .get_or_init(|| async {
                let history = session.clone_history().await;
                Mutex::new(ToolSearchHistoryBudget::from_history(
                    history.raw_items(),
                    reserved_namespace_policy,
                ))
            })
            .await;
        let tools = history_budget
            .lock()
            .await
            .retain_loadable_tools(tools, reserved_namespace_policy);

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
    use codex_tools::tool_spec_to_responses_api_value;
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
            handler.search_source_infos.len()
                <= crate::tools::handlers::tool_search_spec::TOOL_SEARCH_MAX_INITIAL_SOURCE_INFOS,
            "tool_search advertised too many unique sources: {}",
            handler.search_source_infos.len()
        );
    }

    #[test]
    fn initial_declaration_source_cap_is_independent_of_input_order() {
        let ascending = (0..128)
            .map(|index| search_info_with_source(index, 8))
            .collect::<Vec<_>>();
        let descending = ascending.iter().cloned().rev().collect::<Vec<_>>();

        assert_eq!(
            ToolSearchHandler::new(ascending).search_source_infos,
            ToolSearchHandler::new(descending).search_source_infos,
        );
    }

    #[test]
    fn valid_projection_is_not_dropped_for_large_local_search_metadata() {
        let mut search_info = search_info_with_source(0, 20_000);
        search_info.entry.search_text = format!("needle {}", "x".repeat(20_000));

        let handler = ToolSearchHandler::new(vec![search_info]);

        assert_eq!(handler.entries.len(), 1);
        assert_eq!(
            handler
                .search("needle", 1)
                .expect("valid projection should remain searchable")
                .len(),
            1
        );
    }

    #[test]
    fn initial_declaration_bounds_aggregate_source_bytes() {
        let handler = ToolSearchHandler::new(
            (0..64)
                .map(|index| search_info_with_source(index, 1_000))
                .collect(),
        );
        let declaration = tool_spec_to_responses_api_value(&handler.spec())
            .expect("tool_search declaration should serialize");
        let aggregate_bytes = serde_json::to_vec(&declaration)
            .expect("tool_search declaration should serialize")
            .len();

        assert!(
            aggregate_bytes <= TOOL_SEARCH_MAX_DECLARATION_BYTES,
            "tool_search declaration exceeded its complete model-visible budget: \
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
