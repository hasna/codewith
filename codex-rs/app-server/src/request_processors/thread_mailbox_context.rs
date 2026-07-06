use codex_core::context::MAX_MAILBOX_CONTEXT_PAYLOAD_BYTES;
use codex_core::context::MAX_MAILBOX_STORED_PAYLOAD_BYTES;
use serde_json::Value;

const MAX_MAILBOX_CONTEXT_DESCRIPTOR_COMPONENT_BYTES: usize = 256;

pub(crate) fn validate_mailbox_payload_context_size(payload: &Value) -> Result<(), String> {
    let serialized_payload = serde_json::to_vec(payload)
        .map_err(|err| format!("failed to serialize mailbox message payload: {err}"))?;
    if serialized_payload.len() > MAX_MAILBOX_STORED_PAYLOAD_BYTES {
        return Err(format!(
            "mailbox message payload must not exceed {MAX_MAILBOX_STORED_PAYLOAD_BYTES} bytes"
        ));
    }

    let rendered = mailbox_payload_context_text(payload);
    if rendered.len() > MAX_MAILBOX_CONTEXT_PAYLOAD_BYTES {
        return Err(format!(
            "mailbox message rendered for model context must not exceed {MAX_MAILBOX_CONTEXT_PAYLOAD_BYTES} bytes"
        ));
    }
    Ok(())
}

pub(crate) fn mailbox_payload_context_text(payload: &Value) -> String {
    if let Some(text) = payload.as_str() {
        return text.to_string();
    }
    if let Some(text) = payload.get("text").and_then(Value::as_str) {
        return text.to_string();
    }
    serde_json::to_string_pretty(payload).unwrap_or_else(|_| payload.to_string())
}

pub(crate) fn mailbox_context_descriptor_component(value: &str) -> String {
    if value.len() <= MAX_MAILBOX_CONTEXT_DESCRIPTOR_COMPONENT_BYTES {
        return value.to_string();
    }

    let max_without_suffix = MAX_MAILBOX_CONTEXT_DESCRIPTOR_COMPONENT_BYTES.saturating_sub(3);
    let mut truncated = String::new();
    for ch in value.chars() {
        if truncated.len() + ch.len_utf8() > max_without_suffix {
            break;
        }
        truncated.push(ch);
    }
    truncated.push_str("...");
    truncated
}
