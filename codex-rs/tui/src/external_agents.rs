use crate::app_event::AppEvent;
use crate::bottom_pane::SelectionAction;
use crate::bottom_pane::SelectionItem;
use crate::bottom_pane::SelectionViewParams;
use codex_external_agent::ExternalAgentMode;
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
                    "{} {} {} {} {} {}",
                    runtime.id,
                    runtime.display_name,
                    runtime.description,
                    runtime_surfaces_display(runtime),
                    runtime_managed_display(runtime),
                    runtime_default_model_display(runtime),
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
        subtitle: Some("Choose a runtime; tasks open in linked agent threads.".to_string()),
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
        ExternalAgentReadinessStatus::Ready => "command ready",
        ExternalAgentReadinessStatus::MissingRuntime => "missing command",
        ExternalAgentReadinessStatus::MissingAuth => "missing auth",
        ExternalAgentReadinessStatus::Unsupported => "unsupported",
        ExternalAgentReadinessStatus::Disabled => "disabled",
    };
    format!(
        "{} Status: {status}. Command: {}. Surfaces: {}. Managed: {}. Model: {}",
        runtime.description,
        runtime_command_display(runtime),
        runtime_surfaces_display(runtime),
        runtime_managed_display(runtime),
        runtime_default_model_display(runtime),
    )
}

/// Execution surfaces the runtime advertises, e.g. `acp, sdk-local, cloud`.
fn runtime_surfaces_display(runtime: &'static ExternalAgentRuntimeDescriptor) -> String {
    runtime
        .execution_surfaces
        .iter()
        .map(|surface| surface.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Whether the runtime can run in Codewith-managed action-mediation mode.
fn runtime_managed_display(runtime: &'static ExternalAgentRuntimeDescriptor) -> &'static str {
    if runtime.supports_mode(ExternalAgentMode::Managed) {
        "yes"
    } else {
        "no"
    }
}

/// The runtime's advertised default model id (or a fallback label).
fn runtime_default_model_display(runtime: &'static ExternalAgentRuntimeDescriptor) -> &'static str {
    runtime
        .default_model()
        .map(|model| model.id)
        .unwrap_or("runtime default")
}

fn runtime_command_display(runtime: &'static ExternalAgentRuntimeDescriptor) -> String {
    if runtime.command.args.is_empty() {
        runtime.command.program.to_string()
    } else {
        format!(
            "{} {}",
            runtime.command.program,
            runtime.command.args.join(" ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_event_sender::AppEventSender;
    use pretty_assertions::assert_eq;
    use tokio::sync::mpsc::unbounded_channel;

    #[test]
    fn picker_lists_visible_subscription_runtimes() {
        let params = external_agent_picker_params();
        let names = params
            .items
            .iter()
            .map(|item| item.name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(names, vec!["Cursor", "Grok Build", "Claude Code"]);
    }

    #[test]
    fn picker_descriptions_include_command_readiness_context() {
        let params = external_agent_picker_params();
        let descriptions = params
            .items
            .iter()
            .map(|item| item.description.as_deref().unwrap_or_default())
            .collect::<Vec<_>>();

        assert!(descriptions[0].contains("Command: agent acp"));
        assert!(descriptions[1].contains("Command: grok --no-auto-update agent stdio"));
        assert!(descriptions[2].contains("Command: claude"));

        // Cursor surfaces every execution surface, managed mode, and its default model.
        assert!(descriptions[0].contains("Surfaces: acp, sdk-local, cloud"));
        assert!(descriptions[0].contains("Managed: yes"));
        assert!(descriptions[0].contains("Model: auto"));
        // Grok Build and Claude Code stay gated out of managed mode for now.
        assert!(descriptions[1].contains("Managed: no"));
        assert!(descriptions[2].contains("Managed: no"));
        assert!(descriptions[2].contains("Surfaces: sdk-local, cloud"));
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
        subtitle: Choose a runtime; tasks open in linked agent threads.
        search: Type to filter runtimes...
        items:
        - Cursor | Run Cursor's agent through an ACP-compatible harness. Status: <readiness>. Command: agent acp. Surfaces: acp, sdk-local, cloud. Managed: yes. Model: auto
        - Grok Build | Run Grok Build through xAI's ACP stdio agent. Status: <readiness>. Command: grok --no-auto-update agent stdio. Surfaces: acp, cloud. Managed: no. Model: auto
        - Claude Code | Run Claude Code through Claude's CLI/Agent SDK stream. Status: <readiness>. Command: claude. Surfaces: sdk-local, cloud. Managed: no. Model: default
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

        (params.items[2].actions[0])(&tx);
        match rx.try_recv().expect("prefill event") {
            AppEvent::PrefillComposer { text } => {
                assert_eq!(text, "/external-agent claude ");
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
