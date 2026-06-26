//! Session-scoped prompt editing for `ChatWidget`.

use super::*;

const SESSION_PROMPT_USAGE: &str = "Usage: /prompt [edit|show|clear|<session prompt>]";
const SESSION_PROMPT_HINT: &str =
    "Examples: /prompt, /prompt be terse, /prompt show, /prompt clear";
const SESSION_PROMPT_CONTEXT: &str =
    "Applies only to this session. Use /prompt clear to remove it.";

impl ChatWidget {
    pub(crate) fn open_session_prompt_editor(&mut self) {
        if self.thread_id.is_none() {
            self.add_error_message(
                "'/prompt' is unavailable before the session starts.".to_string(),
            );
            return;
        }

        let tx = self.app_event_tx.clone();
        let view = CustomPromptView::new(
            "Session prompt".to_string(),
            "Type a session prompt and press Enter".to_string(),
            self.session_prompt.clone().unwrap_or_default(),
            Some(SESSION_PROMPT_CONTEXT.to_string()),
            Box::new(move |prompt: String| {
                tx.send(AppEvent::SetSessionPrompt {
                    prompt: normalize_session_prompt(Some(prompt)),
                });
            }),
        );
        self.bottom_pane.show_view(Box::new(view));
        self.request_redraw();
    }

    pub(crate) fn handle_session_prompt_inline_args(&mut self, args: &str) {
        let trimmed = args.trim();
        if trimmed.is_empty() {
            self.open_session_prompt_editor();
            return;
        }

        let (verb, rest) = split_first_token(trimmed);
        match verb.to_ascii_lowercase().as_str() {
            "clear" | "reset" | "unset" | "off" if rest.is_empty() => {
                self.apply_session_prompt(None);
            }
            "show" | "status" if rest.is_empty() => {
                self.show_session_prompt();
            }
            "edit" if rest.is_empty() => {
                self.open_session_prompt_editor();
            }
            "set" | "add" | "edit" if !rest.is_empty() => {
                self.apply_session_prompt(Some(rest.to_string()));
            }
            _ => {
                self.apply_session_prompt(Some(trimmed.to_string()));
            }
        }
    }

    pub(crate) fn apply_session_prompt(&mut self, prompt: Option<String>) {
        if self.thread_id.is_none() {
            self.add_error_message(
                "'/prompt' is unavailable before the session starts.".to_string(),
            );
            return;
        }

        let normalized = normalize_session_prompt(prompt);
        self.session_prompt = normalized.clone();
        self.submit_op(AppCommand::override_turn_context(
            /*cwd*/ None,
            /*approval_policy*/ None,
            /*approvals_reviewer*/ None,
            /*permission_profile*/ None,
            /*active_permission_profile*/ None,
            /*windows_sandbox_level*/ None,
            /*model*/ None,
            /*effort*/ None,
            /*summary*/ None,
            /*service_tier*/ None,
            /*collaboration_mode*/ None,
            /*session_prompt*/ Some(normalized.clone()),
            /*personality*/ None,
        ));

        match normalized {
            Some(_) => self.add_info_message(
                "Session prompt updated.".to_string(),
                Some(
                    "Future turns in this session will include it as developer context."
                        .to_string(),
                ),
            ),
            None => self.add_info_message(
                "Session prompt cleared.".to_string(),
                Some(
                    "Future turns in this session will not include an extra session prompt."
                        .to_string(),
                ),
            ),
        }
    }

    pub(crate) fn set_session_prompt_from_settings(&mut self, prompt: Option<String>) {
        self.session_prompt = normalize_session_prompt(prompt);
    }

    #[cfg(test)]
    pub(crate) fn current_session_prompt(&self) -> Option<&str> {
        self.session_prompt.as_deref()
    }

    fn show_session_prompt(&mut self) {
        if let Some(prompt) = self.session_prompt.clone() {
            self.add_info_message("Session prompt:".to_string(), Some(prompt));
        } else {
            self.add_info_message(
                "No session prompt is set.".to_string(),
                Some(format!("{SESSION_PROMPT_USAGE}. {SESSION_PROMPT_HINT}")),
            );
        }
    }
}

fn normalize_session_prompt(prompt: Option<String>) -> Option<String> {
    prompt.and_then(|prompt| {
        let trimmed = prompt.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

fn split_first_token(input: &str) -> (&str, &str) {
    let token_end = input.find(char::is_whitespace).unwrap_or(input.len());
    let (token, rest) = input.split_at(token_end);
    (token, rest.trim_start())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_session_prompt_trims_and_drops_empty_text() {
        assert_eq!(
            normalize_session_prompt(Some("  be terse  ".to_string())),
            Some("be terse".to_string())
        );
        assert_eq!(normalize_session_prompt(Some("   ".to_string())), None);
        assert_eq!(normalize_session_prompt(None), None);
    }

    #[test]
    fn split_first_token_handles_empty_rest() {
        assert_eq!(split_first_token("clear"), ("clear", ""));
        assert_eq!(split_first_token("set be terse"), ("set", "be terse"));
    }
}
