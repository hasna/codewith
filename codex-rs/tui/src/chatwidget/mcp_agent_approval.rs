use super::*;

const MCP_AGENT_APPROVAL_VIEW_ID: &str = "mcp-agent-approval";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct McpAgentMutationApprovalSummary {
    pub(crate) title: String,
    pub(crate) rows: Vec<(String, String)>,
}

impl ChatWidget {
    pub(crate) fn open_mcp_agent_mutation_confirmation(
        &mut self,
        request_id: AppServerRequestId,
        summary: McpAgentMutationApprovalSummary,
    ) {
        let mut header = ColumnRenderable::new();
        header.push(Line::from("Approve MCP Config Change".bold()));
        header.push(Line::from(
            "An agent requested a persistent MCP server configuration change.".dim(),
        ));
        header.push(Line::from(summary.title));

        let mut items = vec![
            deny_agent_mcp_mutation_item(request_id.clone()),
            confirm_agent_mcp_mutation_item(request_id),
        ];
        items.extend(summary.rows.into_iter().map(approval_detail_item));

        self.bottom_pane.show_selection_view(SelectionViewParams {
            view_id: Some(MCP_AGENT_APPROVAL_VIEW_ID),
            header: Box::new(header),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            is_searchable: false,
            col_width_mode: ColumnWidthMode::Fixed,
            ..Default::default()
        });
        self.request_redraw();
    }

    pub(crate) fn dismiss_mcp_agent_mutation_confirmation(&mut self) {
        if self
            .bottom_pane
            .dismiss_active_view_if_id(MCP_AGENT_APPROVAL_VIEW_ID)
        {
            self.request_redraw();
        }
    }
}

fn deny_agent_mcp_mutation_item(request_id: AppServerRequestId) -> SelectionItem {
    SelectionItem {
        name: "Deny".to_string(),
        description: Some("Do not write this MCP configuration.".to_string()),
        actions: vec![Box::new(move |tx| {
            tx.send(AppEvent::DenyAgentMcpMutation {
                request_id: request_id.clone(),
            });
        })],
        dismiss_on_select: true,
        ..Default::default()
    }
}

fn confirm_agent_mcp_mutation_item(request_id: AppServerRequestId) -> SelectionItem {
    SelectionItem {
        name: "Approve and save".to_string(),
        description: Some(
            "Persist this config.toml change and refresh loaded MCP tools.".to_string(),
        ),
        actions: vec![Box::new(move |tx| {
            tx.send(AppEvent::ConfirmAgentMcpMutation {
                request_id: request_id.clone(),
            });
        })],
        dismiss_on_select: true,
        ..Default::default()
    }
}

fn approval_detail_item((label, value): (String, String)) -> SelectionItem {
    SelectionItem {
        name: label,
        description: Some(value),
        is_disabled: true,
        ..Default::default()
    }
}
