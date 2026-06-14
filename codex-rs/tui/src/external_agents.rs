use crate::app_event::AppEvent;
use crate::bottom_pane::SelectionAction;
use crate::bottom_pane::SelectionItem;
use crate::bottom_pane::SelectionViewParams;
use codex_external_agent::ExternalAgentReadinessStatus;
use codex_external_agent::ExternalAgentRuntimeDescriptor;
use codex_external_agent::external_agent_runtime_readiness;
use codex_external_agent::visible_external_agent_runtimes;

pub(crate) fn external_agent_picker_params() -> SelectionViewParams {
    let items = visible_external_agent_runtimes()
        .map(|runtime| {
            let command = format!("/external-agent {} ", runtime.id);
            let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
                tx.send(AppEvent::PrefillComposer {
                    text: command.clone(),
                });
            })];
            SelectionItem {
                name: runtime.display_name.to_string(),
                description: Some(runtime_picker_description(runtime)),
                search_value: Some(format!(
                    "{} {} {}",
                    runtime.id, runtime.display_name, runtime.description
                )),
                dismiss_on_select: true,
                actions,
                ..Default::default()
            }
        })
        .collect();

    SelectionViewParams {
        view_id: Some("external-agent-picker"),
        title: Some("External Agent".to_string()),
        subtitle: Some("Choose an external coding-agent runtime.".to_string()),
        items,
        is_searchable: true,
        search_placeholder: Some("Type to filter runtimes...".to_string()),
        initial_selected_idx: Some(0),
        ..Default::default()
    }
}

fn runtime_picker_description(runtime: &'static ExternalAgentRuntimeDescriptor) -> String {
    let readiness = external_agent_runtime_readiness(runtime);
    let status = match readiness.status {
        ExternalAgentReadinessStatus::Ready => "ready",
        ExternalAgentReadinessStatus::MissingRuntime => "missing command",
        ExternalAgentReadinessStatus::MissingAuth => "missing auth",
        ExternalAgentReadinessStatus::Unsupported => "unsupported",
        ExternalAgentReadinessStatus::Disabled => "disabled",
    };
    format!(
        "{} Status: {status}. Command: {} {}",
        runtime.description,
        runtime.command.program,
        runtime.command.args.join(" ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_event_sender::AppEventSender;
    use pretty_assertions::assert_eq;
    use tokio::sync::mpsc::unbounded_channel;

    #[test]
    fn picker_lists_visible_mvp_runtimes_only() {
        let params = external_agent_picker_params();
        let names = params
            .items
            .iter()
            .map(|item| item.name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(names, vec!["Cursor", "Grok Build"]);
    }

    #[test]
    fn picker_descriptions_include_command_readiness_context() {
        let params = external_agent_picker_params();
        let descriptions = params
            .items
            .iter()
            .map(|item| item.description.as_deref().unwrap_or_default())
            .collect::<Vec<_>>();

        assert!(descriptions[0].contains("Command: cursor-agent acp"));
        assert!(descriptions[1].contains("Command: grok agent stdio"));
    }

    #[test]
    fn external_agent_picker_snapshot() {
        let params = external_agent_picker_params();
        let items = params
            .items
            .iter()
            .map(|item| {
                format!(
                    "- {} | {}",
                    item.name,
                    normalize_readiness_status(item.description.as_deref().unwrap_or_default())
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let snapshot = format!(
            "title: {}\nsubtitle: {}\nsearch: {}\nitems:\n{}",
            params.title.as_deref().unwrap_or_default(),
            params.subtitle.as_deref().unwrap_or_default(),
            params.search_placeholder.as_deref().unwrap_or_default(),
            items
        );

        insta::assert_snapshot!(
            snapshot,
            @r###"
        title: External Agent
        subtitle: Choose an external coding-agent runtime.
        search: Type to filter runtimes...
        items:
        - Cursor | Run Cursor's agent through an ACP-compatible harness. Status: <readiness>. Command: cursor-agent acp
        - Grok Build | Run Grok Build through xAI's ACP stdio agent. Status: <readiness>. Command: grok agent stdio
        "###
        );
    }

    #[test]
    fn picker_selection_prefills_external_agent_command() {
        let params = external_agent_picker_params();
        let (tx, mut rx) = unbounded_channel();
        let tx = AppEventSender::new(tx);

        (params.items[1].actions[0])(&tx);

        match rx.try_recv().expect("prefill event") {
            AppEvent::PrefillComposer { text } => {
                assert_eq!(text, "/external-agent grok-build ");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    fn normalize_readiness_status(description: &str) -> String {
        let Some((prefix, status_and_command)) = description.split_once(" Status: ") else {
            return description.to_string();
        };
        let Some((_status, command)) = status_and_command.split_once(". Command: ") else {
            return description.to_string();
        };
        format!("{prefix} Status: <readiness>. Command: {command}")
    }
}
