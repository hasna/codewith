use super::App;
use crate::app_event::ThreadWorkflowAction;
use crate::app_server_session::AppServerSession;
use codex_protocol::ThreadId;

impl App {
    pub(super) async fn open_thread_workflow_manager(
        &mut self,
        app_server: &mut AppServerSession,
        thread_id: ThreadId,
    ) {
        self.chat_widget
            .show_thread_workflow_manager_loading(thread_id);

        let workflow_response = app_server.thread_workflow_list(thread_id).await;
        let run_response = app_server.thread_workflow_run_list(thread_id).await;
        if self.current_displayed_thread_id() != Some(thread_id) {
            return;
        }

        match (workflow_response, run_response) {
            (Ok(workflows), Ok(runs)) => self
                .chat_widget
                .show_thread_workflow_manager(thread_id, workflows, runs),
            (Err(err), _) => self.chat_widget.show_thread_workflow_manager_error(
                thread_id,
                "read workflow specs",
                &err,
            ),
            (_, Err(err)) => self.chat_widget.show_thread_workflow_manager_error(
                thread_id,
                "list workflow runs",
                &err,
            ),
        }
    }

    pub(super) async fn manage_thread_workflow(
        &mut self,
        app_server: &mut AppServerSession,
        thread_id: ThreadId,
        action: ThreadWorkflowAction,
    ) {
        let action_name = thread_workflow_action_name(&action);
        let result = match action {
            ThreadWorkflowAction::List => app_server
                .thread_workflow_list(thread_id)
                .await
                .map(|response| ThreadWorkflowDisplayResponse::List(Box::new(response))),
            ThreadWorkflowAction::Show { workflow_record_id } => app_server
                .thread_workflow_get(thread_id, workflow_record_id)
                .await
                .map(|response| ThreadWorkflowDisplayResponse::Show(Box::new(response))),
            ThreadWorkflowAction::Delete { workflow_record_id } => {
                let record_id = workflow_record_id.clone();
                app_server
                    .thread_workflow_delete(thread_id, workflow_record_id)
                    .await
                    .map(|response| {
                        ThreadWorkflowDisplayResponse::Delete(Box::new(response), record_id)
                    })
            }
            ThreadWorkflowAction::RunList => app_server
                .thread_workflow_run_list(thread_id)
                .await
                .map(|response| ThreadWorkflowDisplayResponse::RunList(Box::new(response))),
            ThreadWorkflowAction::RunShow { run_id } => app_server
                .thread_workflow_run_get(thread_id, run_id)
                .await
                .map(|response| ThreadWorkflowDisplayResponse::RunShow(Box::new(response))),
            ThreadWorkflowAction::RunStart { workflow_record_id } => app_server
                .thread_workflow_run_start(thread_id, workflow_record_id)
                .await
                .map(|response| ThreadWorkflowDisplayResponse::RunStart(Box::new(response))),
            ThreadWorkflowAction::RunPause { run_id } => app_server
                .thread_workflow_run_pause(thread_id, run_id)
                .await
                .map(|response| ThreadWorkflowDisplayResponse::RunPause(Box::new(response))),
            ThreadWorkflowAction::RunResume { run_id } => app_server
                .thread_workflow_run_resume(thread_id, run_id)
                .await
                .map(|response| ThreadWorkflowDisplayResponse::RunResume(Box::new(response))),
            ThreadWorkflowAction::RunCancel { run_id } => app_server
                .thread_workflow_run_cancel(thread_id, run_id)
                .await
                .map(|response| ThreadWorkflowDisplayResponse::RunCancel(Box::new(response))),
        };
        if self.current_displayed_thread_id() != Some(thread_id) {
            return;
        }

        match result {
            Ok(ThreadWorkflowDisplayResponse::List(response)) => {
                self.chat_widget.show_thread_workflow_summary(*response);
            }
            Ok(ThreadWorkflowDisplayResponse::Show(response)) => {
                self.chat_widget.show_thread_workflow_detail(*response);
            }
            Ok(ThreadWorkflowDisplayResponse::Delete(response, workflow_record_id)) => {
                self.chat_widget
                    .show_thread_workflow_deleted(*response, workflow_record_id);
            }
            Ok(ThreadWorkflowDisplayResponse::RunList(response)) => {
                self.chat_widget.show_thread_workflow_run_summary(*response);
            }
            Ok(ThreadWorkflowDisplayResponse::RunShow(response)) => {
                self.chat_widget.show_thread_workflow_run_detail(*response);
            }
            Ok(ThreadWorkflowDisplayResponse::RunStart(response)) => {
                self.chat_widget.show_thread_workflow_run_started(*response);
            }
            Ok(ThreadWorkflowDisplayResponse::RunPause(response)) => {
                self.chat_widget
                    .show_thread_workflow_run_update("Paused workflow run.", response.run);
            }
            Ok(ThreadWorkflowDisplayResponse::RunResume(response)) => {
                self.chat_widget
                    .show_thread_workflow_run_update("Resumed workflow run.", response.run);
            }
            Ok(ThreadWorkflowDisplayResponse::RunCancel(response)) => {
                self.chat_widget.show_thread_workflow_run_update(
                    "Cancel requested for workflow run.",
                    response.run,
                );
            }
            Err(err) => self
                .chat_widget
                .add_error_message(thread_workflow_error_message(action_name, &err)),
        }
    }
}

enum ThreadWorkflowDisplayResponse {
    List(Box<codex_app_server_protocol::ThreadWorkflowListResponse>),
    Show(Box<codex_app_server_protocol::ThreadWorkflowGetResponse>),
    Delete(
        Box<codex_app_server_protocol::ThreadWorkflowDeleteResponse>,
        String,
    ),
    RunList(Box<codex_app_server_protocol::ThreadWorkflowRunListResponse>),
    RunShow(Box<codex_app_server_protocol::ThreadWorkflowRunGetResponse>),
    RunStart(Box<codex_app_server_protocol::ThreadWorkflowRunStartResponse>),
    RunPause(Box<codex_app_server_protocol::ThreadWorkflowRunPauseResponse>),
    RunResume(Box<codex_app_server_protocol::ThreadWorkflowRunResumeResponse>),
    RunCancel(Box<codex_app_server_protocol::ThreadWorkflowRunCancelResponse>),
}

fn thread_workflow_action_name(action: &ThreadWorkflowAction) -> &'static str {
    match action {
        ThreadWorkflowAction::List => "read",
        ThreadWorkflowAction::Show { .. } => "read",
        ThreadWorkflowAction::Delete { .. } => "delete",
        ThreadWorkflowAction::RunList => "list workflow runs",
        ThreadWorkflowAction::RunShow { .. } => "read workflow run",
        ThreadWorkflowAction::RunStart { .. } => "start workflow run",
        ThreadWorkflowAction::RunPause { .. } => "pause workflow run",
        ThreadWorkflowAction::RunResume { .. } => "resume workflow run",
        ThreadWorkflowAction::RunCancel { .. } => "cancel workflow run",
    }
}

fn thread_workflow_error_message(action: &str, err: &color_eyre::Report) -> String {
    if err
        .to_string()
        .contains("ephemeral thread does not support workflows")
    {
        return concat!(
            "Workflows need a saved session. This session is temporary.\n",
            "Run `codewith` to start a saved session, or `codewith resume` / `/resume` to reopen one.",
        )
        .to_string();
    }
    format!("Failed to {action} workflows: {err}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history_cell::HistoryCell;

    #[test]
    fn thread_workflow_error_message_explains_temporary_session() {
        let err = color_eyre::eyre::eyre!(
            "thread/workflow/list failed in TUI: ephemeral thread does not support workflows: 123"
        );

        assert_eq!(
            thread_workflow_error_message("read", &err),
            "Workflows need a saved session. This session is temporary.\nRun `codewith` to start a saved session, or `codewith resume` / `/resume` to reopen one."
        );
    }

    #[test]
    fn thread_workflow_ephemeral_error_message_renders_snapshot() {
        let err = color_eyre::eyre::eyre!("ephemeral thread does not support workflows: 123");
        let cell =
            crate::history_cell::new_error_event(thread_workflow_error_message("read", &err));
        let rendered = cell
            .display_lines(/*width*/ 80)
            .into_iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        insta::assert_snapshot!(rendered, @r###"
■ Workflows need a saved session. This session is temporary.
Run `codewith` to start a saved session, or `codewith resume` / `/resume` to reopen one.
"###);
    }
}
