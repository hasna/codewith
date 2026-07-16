use codex_prompts::BACKEND_PROMPT;
const DEFAULT_USER_FIRST_NAME: &str = "there";
const USER_FIRST_NAME_PLACEHOLDER: &str = "{{ user_first_name }}";

pub(crate) fn prepare_realtime_backend_prompt(
    prompt: Option<Option<String>>,
    config_prompt: Option<String>,
) -> String {
    if let Some(config_prompt) = config_prompt
        && !config_prompt.trim().is_empty()
    {
        return config_prompt;
    }

    match prompt {
        Some(Some(prompt)) => return prompt,
        Some(None) => return String::new(),
        None => {}
    }

    BACKEND_PROMPT
        .trim_end()
        .replace(USER_FIRST_NAME_PLACEHOLDER, &current_user_first_name())
}

fn current_user_first_name() -> String {
    first_name_from_results([whoami::realname(), whoami::username()])
}

fn first_name_from_results<E>(names: impl IntoIterator<Item = Result<String, E>>) -> String {
    names
        .into_iter()
        .filter_map(Result::ok)
        .filter_map(|name| name.split_whitespace().next().map(str::to_string))
        .find(|name| !name.is_empty())
        .unwrap_or_else(|| DEFAULT_USER_FIRST_NAME.to_string())
}

#[cfg(test)]
mod tests {
    use super::DEFAULT_USER_FIRST_NAME;
    use super::first_name_from_results;
    use super::prepare_realtime_backend_prompt;
    use pretty_assertions::assert_eq;

    #[test]
    fn prepare_realtime_backend_prompt_prefers_config_override() {
        assert_eq!(
            prepare_realtime_backend_prompt(
                Some(Some("prompt from request".to_string())),
                Some("prompt from config".to_string()),
            ),
            "prompt from config"
        );
    }

    #[test]
    fn prepare_realtime_backend_prompt_uses_request_prompt() {
        assert_eq!(
            prepare_realtime_backend_prompt(
                Some(Some("prompt from request".to_string())),
                /*config_prompt*/ None,
            ),
            "prompt from request"
        );
    }

    #[test]
    fn prepare_realtime_backend_prompt_preserves_empty_request_prompt() {
        assert_eq!(
            prepare_realtime_backend_prompt(Some(Some(String::new())), /*config_prompt*/ None),
            ""
        );
        assert_eq!(
            prepare_realtime_backend_prompt(Some(None), /*config_prompt*/ None),
            ""
        );
    }

    #[test]
    fn prepare_realtime_backend_prompt_renders_default() {
        let prompt =
            prepare_realtime_backend_prompt(/*prompt*/ None, /*config_prompt*/ None);

        assert!(prompt.starts_with("## Identity, tone, and role"));
        assert!(prompt.contains("You are Codewith, a Hasna general-purpose agentic assistant"));
        assert!(prompt.contains("The user's name is "));
        assert!(!prompt.contains("{{ user_first_name }}"));
    }

    #[test]
    fn user_first_name_falls_back_when_lookups_fail() {
        let names = [
            Err::<String, _>("real name unavailable"),
            Err("username unavailable"),
        ];

        assert_eq!(first_name_from_results(names), DEFAULT_USER_FIRST_NAME);
    }
}
