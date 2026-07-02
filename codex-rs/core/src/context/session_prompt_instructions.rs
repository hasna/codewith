use super::ContextualUserFragment;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SessionPromptInstructions {
    instructions: String,
}

impl SessionPromptInstructions {
    pub(crate) fn from_prompt(prompt: &str) -> Option<Self> {
        let prompt = prompt.trim();
        if prompt.is_empty() {
            return None;
        }
        Some(Self {
            instructions: prompt.to_string(),
        })
    }

    pub(crate) fn cleared() -> Self {
        Self {
            instructions: "No session-scoped extra prompt is currently set. Ignore any earlier session-scoped extra prompt instructions for this thread.".to_string(),
        }
    }
}

impl ContextualUserFragment for SessionPromptInstructions {
    fn role(&self) -> &'static str {
        "developer"
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        ("<session_prompt>", "</session_prompt>")
    }

    fn body(&self) -> String {
        self.instructions.clone()
    }
}
