use codex_protocol::models::ResponseItem;
use codex_tools::LoadableToolSpec;
use codex_tools::TOOL_SEARCH_MAX_HISTORY_BYTES;
use codex_tools::TOOL_SEARCH_MAX_HISTORY_RESULTS;
use serde_json::Value;

#[derive(Clone, Copy)]
pub(crate) enum ReservedNamespacePolicy {
    Allow,
    Reject,
}

impl ReservedNamespacePolicy {
    fn rejects(self, tool: &Value) -> bool {
        matches!(self, Self::Reject) && codex_tools::is_forbidden_reserved_namespace_value(tool)
    }
}

pub(crate) struct ToolSearchHistoryBudget {
    retained_count: usize,
    retained_bytes: usize,
}

impl Default for ToolSearchHistoryBudget {
    fn default() -> Self {
        Self {
            retained_count: 0,
            // Account for the brackets in the serialized aggregate list.
            retained_bytes: 2,
        }
    }
}

impl ToolSearchHistoryBudget {
    pub(crate) fn from_history(
        history: &[ResponseItem],
        reserved_namespace_policy: ReservedNamespacePolicy,
    ) -> Self {
        let mut budget = Self::default();
        for item in history {
            let ResponseItem::ToolSearchOutput {
                execution, tools, ..
            } = item
            else {
                continue;
            };
            if execution != "client" {
                continue;
            }
            for tool in tools {
                budget.try_reserve(tool, reserved_namespace_policy);
            }
        }
        budget
    }

    pub(crate) fn retain_loadable_tools(
        &mut self,
        tools: Vec<LoadableToolSpec>,
        reserved_namespace_policy: ReservedNamespacePolicy,
    ) -> Vec<LoadableToolSpec> {
        tools
            .into_iter()
            .filter_map(|tool| {
                let value = serde_json::to_value(&tool).ok()?;
                self.try_reserve(&value, reserved_namespace_policy)
                    .then_some(tool)
            })
            .collect()
    }

    fn retain_values(
        &mut self,
        tools: Vec<Value>,
        reserved_namespace_policy: ReservedNamespacePolicy,
    ) -> Vec<Value> {
        tools
            .into_iter()
            .filter(|tool| self.try_reserve(tool, reserved_namespace_policy))
            .collect()
    }

    fn try_reserve(
        &mut self,
        tool: &Value,
        reserved_namespace_policy: ReservedNamespacePolicy,
    ) -> bool {
        if self.retained_count >= TOOL_SEARCH_MAX_HISTORY_RESULTS {
            return false;
        }
        let separator_bytes = usize::from(self.retained_count > 0);
        let Some(available_bytes) = TOOL_SEARCH_MAX_HISTORY_BYTES
            .checked_sub(self.retained_bytes.saturating_add(separator_bytes))
        else {
            return false;
        };
        let Some(serialized_bytes) =
            codex_tools::bounded_json_serialized_len(tool, available_bytes)
        else {
            return false;
        };
        if reserved_namespace_policy.rejects(tool) {
            return false;
        }

        self.retained_count += 1;
        self.retained_bytes = self
            .retained_bytes
            .saturating_add(separator_bytes)
            .saturating_add(serialized_bytes);
        true
    }
}

pub(crate) fn sanitize_client_tool_search_history(
    input: &mut [ResponseItem],
    reserved_namespace_policy: ReservedNamespacePolicy,
) {
    let mut budget = ToolSearchHistoryBudget::default();
    for item in input {
        let ResponseItem::ToolSearchOutput {
            execution, tools, ..
        } = item
        else {
            continue;
        };
        if execution == "client" {
            *tools = budget.retain_values(std::mem::take(tools), reserved_namespace_policy);
        }
    }
}

#[cfg(test)]
#[path = "tool_search_history_tests.rs"]
mod tests;
