use codex_tools::JsonSchema;
use codex_tools::TOOL_SEARCH_DEFAULT_LIMIT;
use codex_tools::TOOL_SEARCH_MAX_DECLARATION_BYTES;
use codex_tools::TOOL_SEARCH_TOOL_NAME;
use codex_tools::ToolSearchSourceInfo;
use codex_tools::ToolSpec;
use codex_tools::tool_spec_to_responses_api_value;
use std::collections::BTreeMap;

pub(crate) const TOOL_SEARCH_MAX_INITIAL_SOURCE_INFOS: usize = 64;

pub(crate) fn bounded_source_infos(
    source_infos: BTreeMap<String, Option<String>>,
) -> Vec<ToolSearchSourceInfo> {
    let mut retained = Vec::new();
    for (name, description) in source_infos {
        if retained.len() >= TOOL_SEARCH_MAX_INITIAL_SOURCE_INFOS {
            break;
        }
        let source = ToolSearchSourceInfo { name, description };
        let mut candidate = retained.clone();
        candidate.push(source.clone());
        if declaration_fits(&candidate) {
            retained = candidate;
            continue;
        }
        if source.description.is_some() {
            candidate.pop();
            candidate.push(ToolSearchSourceInfo {
                description: None,
                ..source
            });
            if declaration_fits(&candidate) {
                retained = candidate;
            }
        }
    }
    retained
}

fn declaration_fits(source_infos: &[ToolSearchSourceInfo]) -> bool {
    tool_spec_to_responses_api_value(&create_tool_search_tool(
        source_infos,
        TOOL_SEARCH_DEFAULT_LIMIT,
    ))
    .ok()
    .and_then(|value| {
        codex_tools::bounded_json_serialized_len(&value, TOOL_SEARCH_MAX_DECLARATION_BYTES)
    })
    .is_some()
}

pub(crate) fn create_tool_search_tool(
    searchable_sources: &[ToolSearchSourceInfo],
    default_limit: usize,
) -> ToolSpec {
    let properties = BTreeMap::from([
        (
            "query".to_string(),
            JsonSchema::string(Some("Search query for deferred tools.".to_string())),
        ),
        (
            "limit".to_string(),
            JsonSchema::integer(Some(format!(
                "Maximum number of tools to return. Defaults to {default_limit}."
            ))),
        ),
    ]);

    let mut source_descriptions = BTreeMap::new();
    for source in searchable_sources {
        source_descriptions
            .entry(source.name.clone())
            .and_modify(|existing: &mut Option<String>| {
                if existing.is_none() {
                    *existing = source.description.clone();
                }
            })
            .or_insert(source.description.clone());
    }

    let source_descriptions = if source_descriptions.is_empty() {
        "None currently enabled.".to_string()
    } else {
        source_descriptions
            .into_iter()
            .map(|(name, description)| match description {
                Some(description) => format!("- {name}: {description}"),
                None => format!("- {name}"),
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let description = format!(
        "# Tool discovery\n\nSearches over deferred tool metadata with BM25 and exposes matching tools for the next model call.\n\nYou have access to tools from the following sources:\n{source_descriptions}\nSome of the tools may not have been provided to you upfront, and you should use this tool (`{TOOL_SEARCH_TOOL_NAME}`) to search for the required tools. For MCP tool discovery, always use `{TOOL_SEARCH_TOOL_NAME}` instead of `list_mcp_resources` or `list_mcp_resource_templates`."
    );

    ToolSpec::ToolSearch {
        execution: "client".to_string(),
        description,
        parameters: JsonSchema::object(
            properties,
            Some(vec!["query".to_string()]),
            Some(false.into()),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_tools::JsonSchema;
    use codex_tools::create_tools_json_for_responses_api;
    use codex_tools::tool_spec_to_chat_api_value;
    use pretty_assertions::assert_eq;
    use std::collections::BTreeMap;

    #[test]
    fn create_tool_search_tool_deduplicates_and_renders_enabled_sources() {
        assert_eq!(
            create_tool_search_tool(
                &[
                    ToolSearchSourceInfo {
                        name: "Google Drive".to_string(),
                        description: Some(
                            "Use Google Drive as the single entrypoint for Drive, Docs, Sheets, and Slides work."
                                .to_string(),
                        ),
                    },
                    ToolSearchSourceInfo {
                        name: "Google Drive".to_string(),
                        description: None,
                    },
                    ToolSearchSourceInfo {
                        name: "docs".to_string(),
                        description: None,
                    },
                ],
                /*default_limit*/ 8,
            ),
            ToolSpec::ToolSearch {
                execution: "client".to_string(),
                description: "# Tool discovery\n\nSearches over deferred tool metadata with BM25 and exposes matching tools for the next model call.\n\nYou have access to tools from the following sources:\n- Google Drive: Use Google Drive as the single entrypoint for Drive, Docs, Sheets, and Slides work.\n- docs\nSome of the tools may not have been provided to you upfront, and you should use this tool (`tool_search`) to search for the required tools. For MCP tool discovery, always use `tool_search` instead of `list_mcp_resources` or `list_mcp_resource_templates`.".to_string(),
                parameters: JsonSchema::object(BTreeMap::from([
                        (
                            "limit".to_string(),
                            JsonSchema::integer(Some(
                                    "Maximum number of tools to return. Defaults to 8."
                                        .to_string(),
                                ),),
                        ),
                        (
                            "query".to_string(),
                            JsonSchema::string(Some("Search query for deferred tools.".to_string()),),
                        ),
                    ]), Some(vec!["query".to_string()]), Some(false.into())),
            }
        );
    }

    #[test]
    fn tool_search_limit_wire_schema_matches_handler_bounds() {
        let spec = create_tool_search_tool(&[], /*default_limit*/ 8);
        let [wire_spec] = create_tools_json_for_responses_api(&[spec])
            .expect("tool_search declaration should serialize")
            .try_into()
            .expect("one declaration");

        assert_eq!(
            wire_spec.pointer("/parameters/properties/limit"),
            Some(&serde_json::json!({
                "type": "integer",
                "description": "Maximum number of tools to return. Defaults to 8.",
                "minimum": 1,
                "maximum": 8,
            }))
        );
    }

    #[test]
    fn tool_search_limit_schema_matches_handler_bounds_for_chat() {
        let spec = create_tool_search_tool(&[], /*default_limit*/ 8);
        let wire_spec =
            tool_spec_to_chat_api_value(&spec).expect("Chat declaration should serialize");

        assert_eq!(
            wire_spec.pointer("/parameters/properties/limit"),
            Some(&serde_json::json!({
                "type": "integer",
                "description": "Maximum number of tools to return. Defaults to 8.",
                "minimum": 1,
                "maximum": 8,
            }))
        );
    }
}
