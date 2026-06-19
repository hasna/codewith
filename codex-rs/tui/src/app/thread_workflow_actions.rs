use super::App;
use crate::app_server_session::AppServerSession;
use codex_protocol::ThreadId;

impl App {
    pub(super) async fn open_thread_workflow_manager(
        &mut self,
        app_server: &mut AppServerSession,
        thread_id: ThreadId,
    ) {
        let result = app_server.thread_workflow_list(thread_id).await;
        if self.current_displayed_thread_id() != Some(thread_id) {
            return;
        }

        match result {
            Ok(response) => self.chat_widget.show_thread_workflow_summary(response),
            Err(err) => self
                .chat_widget
                .add_error_message(thread_workflow_error_message("read", &err)),
        }
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
