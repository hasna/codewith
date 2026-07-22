//! Bounded model-context fragment for queued mailbox delivery.

use codex_protocol::AgentPath;
use codex_protocol::models::ContentItem;
use codex_protocol::models::MessagePhase;
use codex_protocol::models::ResponseInputItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::InterAgentCommunication;

use super::ContextualUserFragment;

pub const MAX_MAILBOX_CONTEXT_PAYLOAD_BYTES: usize = 2 * 1024;
pub const MAX_MAILBOX_STORED_PAYLOAD_BYTES: usize = 8 * 1024;
pub const MAX_MAILBOX_CONTEXT_ITEM_BYTES: usize = 4 * 1024;
pub const MAX_MAILBOX_CONTEXT_QUEUE_ITEMS: usize = 8;

/// Sub-agent completion notifications (`wake_if_idle`) carry the child's FINAL
/// answer, which becomes the parent's only copy of that result once the parent
/// auto-resumes and consumes it. The default 2 KiB content clamp would silently
/// truncate a substantial answer, so completion mail gets a larger budget. This
/// is a deliberate tradeoff: a single completion can inject up to ~16 KiB into
/// the parent's context (vs ~2 KiB for ordinary mail) in exchange for not
/// dropping the sub-agent's work.
pub const MAX_MAILBOX_CONTEXT_COMPLETION_PAYLOAD_BYTES: usize = 16 * 1024;
pub const MAX_MAILBOX_CONTEXT_COMPLETION_ITEM_BYTES: usize = 20 * 1024;

const MAILBOX_TRUNCATED_NOTICE: &str = "\n\n[mailbox message truncated before model context]";
const MAILBOX_OMITTED_NOTICE: &str = "[mailbox message omitted before model context]";

/// Bounded assistant-context fragment for mailbox messages.
///
/// The body remains a serialized `InterAgentCommunication` so existing history
/// and inter-agent parsing continue to recognize mailbox delivery boundaries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailboxContextFragment {
    communication: InterAgentCommunication,
}

impl MailboxContextFragment {
    pub fn new(communication: InterAgentCommunication) -> Self {
        Self {
            communication: bounded_communication(communication),
        }
    }
}

impl ContextualUserFragment for MailboxContextFragment {
    fn role(&self) -> &'static str {
        "assistant"
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn body(&self) -> String {
        serde_json::to_string(&self.communication).unwrap_or_default()
    }

    fn type_markers() -> (&'static str, &'static str) {
        ("", "")
    }

    fn into(self) -> ResponseItem
    where
        Self: Sized,
    {
        ResponseItem::from(self.into_response_input_item())
    }

    fn into_boxed_response_item(self: Box<Self>) -> ResponseItem {
        ResponseItem::from(self.into_response_input_item())
    }

    fn into_response_input_item(self) -> ResponseInputItem
    where
        Self: Sized,
    {
        ResponseInputItem::Message {
            role: self.role().to_string(),
            content: vec![ContentItem::OutputText {
                text: self.render(),
            }],
            phase: Some(MessagePhase::Commentary),
        }
    }
}

fn bounded_communication(mut communication: InterAgentCommunication) -> InterAgentCommunication {
    // Completion notifications get a larger budget so the child's final answer
    // survives the clamp (see the constant docs for the tradeoff).
    let (max_payload_bytes, max_item_bytes) = if communication.wake_if_idle {
        (
            MAX_MAILBOX_CONTEXT_COMPLETION_PAYLOAD_BYTES,
            MAX_MAILBOX_CONTEXT_COMPLETION_ITEM_BYTES,
        )
    } else {
        (
            MAX_MAILBOX_CONTEXT_PAYLOAD_BYTES,
            MAX_MAILBOX_CONTEXT_ITEM_BYTES,
        )
    };

    if serialized_len(&communication) <= max_item_bytes {
        return communication;
    }

    if communication.encrypted_content.is_some() {
        communication.encrypted_content = None;
        if communication.content.is_empty() {
            communication.content = MAILBOX_OMITTED_NOTICE.to_string();
        }
    }

    let mut max_content_bytes = max_payload_bytes;
    while max_content_bytes > MAILBOX_TRUNCATED_NOTICE.len() {
        communication.content = truncate_with_suffix(
            communication.content.as_str(),
            max_content_bytes,
            MAILBOX_TRUNCATED_NOTICE,
        );
        if serialized_len(&communication) <= max_item_bytes {
            return communication;
        }
        max_content_bytes /= 2;
    }

    communication.content = MAILBOX_OMITTED_NOTICE.to_string();
    if serialized_len(&communication) <= max_item_bytes {
        return communication;
    }

    minimal_omitted_communication()
}

fn serialized_len(communication: &InterAgentCommunication) -> usize {
    serde_json::to_string(communication).map_or(usize::MAX, |value| value.len())
}

fn minimal_omitted_communication() -> InterAgentCommunication {
    InterAgentCommunication::new(
        AgentPath::root(),
        AgentPath::root(),
        Vec::new(),
        MAILBOX_OMITTED_NOTICE.to_string(),
        /*trigger_turn*/ false,
    )
}

fn truncate_with_suffix(value: &str, max_bytes: usize, suffix: &str) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let max_without_suffix = max_bytes.saturating_sub(suffix.len());
    let mut truncated = String::new();
    for ch in value.chars() {
        if truncated.len() + ch.len_utf8() > max_without_suffix {
            break;
        }
        truncated.push(ch);
    }
    truncated.push_str(suffix);
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_protocol::AgentPath;

    fn communication(content: String) -> InterAgentCommunication {
        InterAgentCommunication::new(
            AgentPath::root(),
            AgentPath::root(),
            Vec::new(),
            content,
            /*trigger_turn*/ true,
        )
    }

    #[test]
    fn mailbox_context_fragment_keeps_inter_agent_payload_parseable() {
        let fragment = MailboxContextFragment::new(communication("hello".to_string()));
        let text = fragment.render();
        let parsed = serde_json::from_str::<InterAgentCommunication>(text.as_str())
            .expect("mailbox context should remain parseable");

        assert_eq!(parsed.content, "hello");
    }

    #[test]
    fn mailbox_context_fragment_preserves_assistant_commentary_shape() {
        let item = MailboxContextFragment::new(communication("hello".to_string()))
            .into_response_input_item();

        assert_eq!(
            item,
            ResponseInputItem::Message {
                role: "assistant".to_string(),
                content: vec![ContentItem::OutputText {
                    text: serde_json::to_string(&communication("hello".to_string()))
                        .expect("communication should serialize"),
                }],
                phase: Some(MessagePhase::Commentary),
            }
        );
    }

    #[test]
    fn mailbox_context_fragment_bounds_serialized_item() {
        let fragment = MailboxContextFragment::new(communication("\"".repeat(20_000)));
        let rendered = fragment.render();

        assert!(rendered.len() <= MAX_MAILBOX_CONTEXT_ITEM_BYTES);
        let parsed = serde_json::from_str::<InterAgentCommunication>(rendered.as_str())
            .expect("bounded mailbox context should remain parseable");
        assert!(
            parsed
                .content
                .contains("[mailbox message truncated before model context]")
        );
    }

    #[test]
    fn completion_mail_preserves_answer_that_ordinary_mail_would_truncate() {
        // A ~6 KiB answer exceeds the ordinary 4 KiB item cap but fits the
        // larger completion budget, so it must survive intact on the completion
        // (`wake_if_idle`) path and be truncated on the ordinary path.
        let answer = "answer body ".repeat(500);
        assert!(answer.len() > MAX_MAILBOX_CONTEXT_ITEM_BYTES);
        assert!(answer.len() < MAX_MAILBOX_CONTEXT_COMPLETION_PAYLOAD_BYTES);

        let ordinary = MailboxContextFragment::new(communication(answer.clone()));
        let ordinary_parsed =
            serde_json::from_str::<InterAgentCommunication>(ordinary.render().as_str())
                .expect("ordinary mailbox context should remain parseable");
        assert!(
            ordinary_parsed
                .content
                .contains("[mailbox message truncated before model context]"),
            "ordinary mail should be truncated at the small cap"
        );

        let completion =
            MailboxContextFragment::new(InterAgentCommunication::completion_notification(
                AgentPath::root().join("worker").expect("author path"),
                AgentPath::root(),
                answer.clone(),
            ));
        let completion_rendered = completion.render();
        assert!(completion_rendered.len() <= MAX_MAILBOX_CONTEXT_COMPLETION_ITEM_BYTES);
        let completion_parsed =
            serde_json::from_str::<InterAgentCommunication>(completion_rendered.as_str())
                .expect("completion mailbox context should remain parseable");
        assert_eq!(
            completion_parsed.content, answer,
            "completion mail must preserve the child's final answer intact"
        );
        assert!(completion_parsed.wake_if_idle);
    }

    #[test]
    fn mailbox_context_fragment_omits_content_when_escaping_still_exceeds_item_bound() {
        let fragment = MailboxContextFragment::new(communication("\u{0000}".repeat(20_000)));

        assert!(fragment.render().len() <= MAX_MAILBOX_CONTEXT_ITEM_BYTES);
    }

    #[test]
    fn mailbox_context_fragment_omits_encrypted_content_before_model_context() {
        let mut communication = communication(String::new());
        communication.encrypted_content = Some("x".repeat(20_000));

        let fragment = MailboxContextFragment::new(communication);
        let rendered = fragment.render();
        let parsed = serde_json::from_str::<InterAgentCommunication>(rendered.as_str())
            .expect("bounded mailbox context should remain parseable");

        assert!(rendered.len() <= MAX_MAILBOX_CONTEXT_ITEM_BYTES);
        assert_eq!(parsed.encrypted_content, None);
        assert_eq!(parsed.content, MAILBOX_OMITTED_NOTICE);
    }

    #[test]
    fn mailbox_context_fragment_falls_back_when_metadata_exceeds_item_bound() {
        let long_agent = format!("/root/{}", "a".repeat(20_000));
        let communication = InterAgentCommunication::new(
            AgentPath::try_from(long_agent).expect("long agent path"),
            AgentPath::root(),
            Vec::new(),
            "hello".to_string(),
            /*trigger_turn*/ true,
        );

        let fragment = MailboxContextFragment::new(communication);
        let rendered = fragment.render();
        let parsed = serde_json::from_str::<InterAgentCommunication>(rendered.as_str())
            .expect("bounded mailbox context should remain parseable");

        assert!(rendered.len() <= MAX_MAILBOX_CONTEXT_ITEM_BYTES);
        assert_eq!(parsed.author, AgentPath::root());
        assert_eq!(parsed.content, MAILBOX_OMITTED_NOTICE);
    }
}
