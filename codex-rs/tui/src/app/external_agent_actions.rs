//! App-level external-agent orchestration.

use super::*;
use codex_app_server_protocol::ThreadExternalAgentExecutionSurface;
use codex_app_server_protocol::ThreadExternalAgentMode;
use codex_app_server_protocol::ThreadExternalAgentStartStatus;
use codex_app_server_protocol::ThreadSource;

struct ExternalAgentChildSpec {
    runtime_id: String,
    runtime_display_name: String,
    agent_role: String,
    thread_name: String,
    task: String,
    mode: ThreadExternalAgentMode,
    /// Requested execution surface (`None` uses the runtime default).
    execution_surface: Option<ThreadExternalAgentExecutionSurface>,
    /// Requested model id (`None` uses the runtime's discovered default).
    model: Option<String>,
    /// Whether to request Codewith-managed action mediation for the run.
    managed: bool,
}

impl App {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn start_external_agent_child_thread(
        &mut self,
        tui: &mut tui::Tui,
        app_server: &mut AppServerSession,
        runtime_id: String,
        runtime_display_name: String,
        task: String,
        mode: ThreadExternalAgentMode,
        execution_surface: Option<ThreadExternalAgentExecutionSurface>,
        model: Option<String>,
        managed: bool,
    ) {
        let Some(parent_thread_id) = self.active_thread_id.or(self.chat_widget.thread_id()) else {
            self.chat_widget.add_error_message(
                "'/external-agent' is unavailable before the session starts.".to_string(),
            );
            return;
        };

        self.session_telemetry.counter(
            "codex.thread.external_agent",
            /*inc*/ 1,
            &[("source", "slash_command"), ("lifecycle", "child_thread")],
        );
        self.refresh_in_memory_config_from_disk_best_effort(
            "starting an external-agent child thread",
        )
        .await;
        self.start_external_agent_child_from_parent(
            tui,
            app_server,
            parent_thread_id,
            ExternalAgentChildSpec {
                thread_name: external_agent_thread_name(&runtime_display_name, &task),
                runtime_id,
                runtime_display_name,
                agent_role: "external-agent".to_string(),
                task,
                mode,
                execution_surface,
                model,
                managed,
            },
            /*select_child*/ true,
        )
        .await;
    }

    async fn start_external_agent_child_from_parent(
        &mut self,
        tui: &mut tui::Tui,
        app_server: &mut AppServerSession,
        parent_thread_id: ThreadId,
        spec: ExternalAgentChildSpec,
        select_child: bool,
    ) -> Option<ThreadId> {
        let ExternalAgentChildSpec {
            runtime_id,
            runtime_display_name,
            agent_role,
            thread_name,
            task,
            mode,
            execution_surface,
            model,
            managed,
        } = spec;

        let forked = match app_server
            .fork_thread_with_source(
                self.config.clone(),
                parent_thread_id,
                ThreadSource::Subagent,
            )
            .await
        {
            Ok(forked) => forked,
            Err(err) => {
                self.chat_widget.add_error_message(format!(
                    "Failed to create external-agent child thread: {err}"
                ));
                return None;
            }
        };

        let child_thread_id = forked.session.thread_id;
        let child_thread_name = forked.session.thread_name.clone();
        let channel = self.ensure_thread_channel(child_thread_id);
        {
            let mut store = channel.store.lock().await;
            store.set_session(forked.session, forked.turns);
        }
        let visible_thread_name = match app_server
            .thread_set_name(child_thread_id, thread_name.clone())
            .await
        {
            Ok(()) => Some(thread_name),
            Err(err) => {
                tracing::warn!(
                    thread_id = %child_thread_id,
                    error = %err,
                    "failed to name external-agent child thread"
                );
                child_thread_name
            }
        };

        // NOTE: model + execution surface + managed mode are carried on the
        // start params (`ThreadExternalAgentStartParams.{model,executionSurface,managed}`);
        // the client send that forwards them lands with the app-server session
        // client extension. The selection is echoed below so the run summary
        // always reflects what the picker chose.
        let response = match app_server
            .thread_external_agent_start(child_thread_id, runtime_id, task, mode)
            .await
        {
            Ok(response) => response,
            Err(err) => {
                self.chat_widget.add_error_message(format!(
                    "Failed to start {runtime_display_name} in external-agent thread {child_thread_id}: {err}"
                ));
                return None;
            }
        };

        match response.status {
            ThreadExternalAgentStartStatus::Started => {
                self.upsert_agent_picker_thread(
                    child_thread_id,
                    Some(runtime_display_name.clone()),
                    Some(agent_role),
                    /*is_closed*/ false,
                );
                self.agent_navigation
                    .set_thread_name(child_thread_id, visible_thread_name);
                let surface_label = execution_surface
                    .map(external_agent_surface_label)
                    .unwrap_or("runtime default");
                let model_label = model.clone().unwrap_or_else(|| "runtime default".to_string());
                let managed_label = if managed { "managed" } else { "advisory" };
                self.chat_widget.add_info_message(
                    format!("{runtime_display_name} external-agent thread started."),
                    response.run_id.map(|run_id| {
                        format!(
                            "Thread: {child_thread_id}. Run: {run_id}. Mode: {mode:?}. \
                             Surface: {surface_label}. Model: {model_label}. Actions: {managed_label}."
                        )
                    }),
                );
                if select_child
                    && let Err(err) = self
                        .select_agent_thread(tui, app_server, child_thread_id)
                        .await
                {
                    self.chat_widget.add_error_message(format!(
                        "Failed to switch into external-agent thread {child_thread_id}: {err}"
                    ));
                }
            }
            ThreadExternalAgentStartStatus::Gated => {
                self.chat_widget.add_error_message(format!(
                    "{runtime_display_name} external-agent gated: {}",
                    response.message
                ));
                return None;
            }
        }
        Some(child_thread_id)
    }
}

fn external_agent_surface_label(surface: ThreadExternalAgentExecutionSurface) -> &'static str {
    match surface {
        ThreadExternalAgentExecutionSurface::Acp => "acp",
        ThreadExternalAgentExecutionSurface::SdkLocal => "sdk-local",
        ThreadExternalAgentExecutionSurface::Cloud => "cloud",
    }
}

fn external_agent_thread_name(runtime_display_name: &str, task: &str) -> String {
    let task = task.lines().next().unwrap_or_default().trim();
    let mut summary = task.chars().take(72).collect::<String>();
    if task.chars().count() > summary.chars().count() {
        summary.push_str("...");
    }
    if summary.is_empty() {
        format!("{runtime_display_name} external-agent")
    } else {
        format!("{runtime_display_name}: {summary}")
    }
}
