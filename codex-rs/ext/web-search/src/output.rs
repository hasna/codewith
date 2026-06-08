use codex_extension_api::ToolOutput;
use codex_extension_api::ToolPayload;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ResponseInputItem;
use codex_utils_output_truncation::TruncationPolicy;
use codex_utils_output_truncation::truncate_text;

const MAX_STANDALONE_SEARCH_OUTPUT_TOKENS: usize = 8_000;

pub(crate) struct SearchOutput {
    content: Vec<FunctionCallOutputContentItem>,
}

impl SearchOutput {
    pub(crate) fn new(
        output: Option<String>,
        encrypted_output: Option<String>,
        truncation_policy: TruncationPolicy,
    ) -> Self {
        let content = match output {
            Some(output) => {
                let token_budget = truncation_policy
                    .token_budget()
                    .min(MAX_STANDALONE_SEARCH_OUTPUT_TOKENS);
                vec![FunctionCallOutputContentItem::InputText {
                    text: truncate_text(&output, TruncationPolicy::Tokens(token_budget)),
                }]
            }
            None => encrypted_output
                .map(|encrypted_content| {
                    vec![FunctionCallOutputContentItem::EncryptedContent { encrypted_content }]
                })
                .unwrap_or_else(|| {
                    vec![FunctionCallOutputContentItem::InputText {
                        text: String::new(),
                    }]
                }),
        };
        Self { content }
    }
}

impl ToolOutput for SearchOutput {
    fn log_preview(&self) -> String {
        "[standalone web search output]".to_string()
    }

    fn success_for_logging(&self) -> bool {
        true
    }

    fn to_response_item(&self, call_id: &str, _payload: &ToolPayload) -> ResponseInputItem {
        // TODO: Make standalone search honor memories.disable_on_external_context,
        // as hosted web search does.
        ResponseInputItem::FunctionCallOutput {
            call_id: call_id.to_string(),
            output: FunctionCallOutputPayload::from_content_items(self.content.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use codex_extension_api::ToolPayload;
    use codex_protocol::models::FunctionCallOutputBody;
    use codex_protocol::models::FunctionCallOutputContentItem;
    use codex_protocol::models::FunctionCallOutputPayload;
    use codex_protocol::models::ResponseInputItem;
    use codex_utils_output_truncation::TruncationPolicy;
    use pretty_assertions::assert_eq;

    use super::SearchOutput;
    use super::ToolOutput;

    #[test]
    fn emits_plaintext_function_call_output() {
        let output = SearchOutput::new(
            Some("search output".to_string()),
            Some("ciphertext".to_string()),
            TruncationPolicy::Tokens(1_000),
        );

        assert_eq!(
            output.to_response_item(
                "call-1",
                &ToolPayload::Function {
                    arguments: "{}".to_string(),
                },
            ),
            ResponseInputItem::FunctionCallOutput {
                call_id: "call-1".to_string(),
                output: FunctionCallOutputPayload::from_content_items(vec![
                    FunctionCallOutputContentItem::InputText {
                        text: "search output".to_string(),
                    },
                ]),
            }
        );
    }

    #[test]
    fn emits_bounded_plaintext_function_call_output() {
        let long_output = "search result ".repeat(20_000);
        let output = SearchOutput::new(
            Some(long_output.clone()),
            None,
            TruncationPolicy::Tokens(20_000),
        );

        let ResponseInputItem::FunctionCallOutput { output, .. } = output.to_response_item(
            "call-1",
            &ToolPayload::Function {
                arguments: "{}".to_string(),
            },
        ) else {
            panic!("expected function call output");
        };
        let FunctionCallOutputBody::ContentItems(items) = output.body else {
            panic!("expected content item output body");
        };
        let [FunctionCallOutputContentItem::InputText { text }] = items.as_slice() else {
            panic!("expected one text output item");
        };

        assert!(text.len() < long_output.len());
    }

    #[test]
    fn emits_encrypted_function_call_output_without_plaintext() {
        let output = SearchOutput::new(
            None,
            Some("ciphertext".to_string()),
            TruncationPolicy::Tokens(1_000),
        );

        assert_eq!(
            output.to_response_item(
                "call-1",
                &ToolPayload::Function {
                    arguments: "{}".to_string(),
                },
            ),
            ResponseInputItem::FunctionCallOutput {
                call_id: "call-1".to_string(),
                output: FunctionCallOutputPayload::from_content_items(vec![
                    FunctionCallOutputContentItem::EncryptedContent {
                        encrypted_content: "ciphertext".to_string(),
                    },
                ]),
            }
        );
    }
}
