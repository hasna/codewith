use crate::model::datetime_to_epoch_millis;
use chrono::DateTime;
use chrono::Utc;

pub(crate) struct MailboxCursor {
    pub(crate) created_at_ms: i64,
    pub(crate) message_id: String,
}

pub(crate) fn encode_mailbox_cursor(created_at: DateTime<Utc>, message_id: &str) -> String {
    format!("{}:{message_id}", datetime_to_epoch_millis(created_at))
}

pub(crate) fn decode_mailbox_cursor(cursor: &str) -> anyhow::Result<MailboxCursor> {
    let Some((created_at_ms, message_id)) = cursor.split_once(':') else {
        anyhow::bail!("invalid mailbox cursor");
    };
    let created_at_ms = created_at_ms.parse::<i64>()?;
    if message_id.trim().is_empty() {
        anyhow::bail!("invalid mailbox cursor");
    }
    Ok(MailboxCursor {
        created_at_ms,
        message_id: message_id.to_string(),
    })
}
