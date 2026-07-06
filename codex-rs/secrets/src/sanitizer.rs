use regex::Regex;
use std::sync::LazyLock;

static OPENAI_KEY_REGEX: LazyLock<Regex> =
    LazyLock::new(|| compile_regex(r"\bsk(?:-[A-Za-z0-9][A-Za-z0-9_-]{7,}){1,}\b"));
static AWS_ACCESS_KEY_ID_REGEX: LazyLock<Regex> =
    LazyLock::new(|| compile_regex(r"\bAKIA[0-9A-Z]{16}\b"));
static AWS_SECRET_ACCESS_KEY_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    compile_regex(r#"(?i)\baws_secret_access_key\b(\s*[:=]\s*)(["']?)[^\s"']{20,}"#)
});
static BEARER_TOKEN_REGEX: LazyLock<Regex> =
    LazyLock::new(|| compile_regex(r"(?i)\bBearer\s+[A-Za-z0-9._\-]{16,}\b"));
static GITHUB_TOKEN_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    compile_regex(r"\b(?:gh[pousr]_[A-Za-z0-9_]{20,}|github_pat_[A-Za-z0-9_]{30,})\b")
});
static GOOGLE_API_KEY_REGEX: LazyLock<Regex> =
    LazyLock::new(|| compile_regex(r"\bAIza[0-9A-Za-z_-]{32,}\b"));
static ANTHROPIC_KEY_REGEX: LazyLock<Regex> =
    LazyLock::new(|| compile_regex(r"\bsk-ant-[A-Za-z0-9_-]{20,}\b"));
static JWT_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    compile_regex(r"\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\b")
});
static SECRET_ASSIGNMENT_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    compile_regex(
        r#"(?i)\b(api[_-]?key|access[_-]?token|refresh[_-]?token|id[_-]?token|auth(?:orization)?|token|secret|client[_-]?secret|password|private[_-]?key)\b(\s*[:=]\s*)(["']?)[^\s"']{8,}"#,
    )
});

/// Remove secret and keys from a String. This is done on best effort basis following some
/// well-known REGEX.
pub fn redact_secrets(input: String) -> String {
    let redacted = OPENAI_KEY_REGEX.replace_all(&input, "[REDACTED_SECRET]");
    let redacted = AWS_ACCESS_KEY_ID_REGEX.replace_all(&redacted, "[REDACTED_SECRET]");
    let redacted = AWS_SECRET_ACCESS_KEY_REGEX
        .replace_all(&redacted, "aws_secret_access_key$1$2[REDACTED_SECRET]");
    let redacted = BEARER_TOKEN_REGEX.replace_all(&redacted, "Bearer [REDACTED_SECRET]");
    let redacted = GITHUB_TOKEN_REGEX.replace_all(&redacted, "[REDACTED_SECRET]");
    let redacted = GOOGLE_API_KEY_REGEX.replace_all(&redacted, "[REDACTED_SECRET]");
    let redacted = ANTHROPIC_KEY_REGEX.replace_all(&redacted, "[REDACTED_SECRET]");
    let redacted = JWT_REGEX.replace_all(&redacted, "[REDACTED_SECRET]");
    let redacted = SECRET_ASSIGNMENT_REGEX.replace_all(&redacted, "$1$2$3[REDACTED_SECRET]");

    redacted.to_string()
}

fn compile_regex(pattern: &str) -> Regex {
    match Regex::new(pattern) {
        Ok(regex) => regex,
        // Panic is ok thanks to `load_regex` test.
        Err(err) => panic!("invalid regex pattern `{pattern}`: {err}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_regex() {
        // The goal of this test is just to compile all the regex to prevent the panic
        let _ = redact_secrets("secret".to_string());
    }

    #[test]
    fn redacts_common_local_state_secret_shapes() {
        let openai = format!("{}{}", "sk-proj-", "a".repeat(32));
        let anthropic = format!("{}{}", "sk-ant-", "b".repeat(24));
        let github = format!("{}{}", "ghp_", "c".repeat(36));
        let google = format!("{}{}", "AIza", "D".repeat(36));
        let refresh = "e".repeat(20);
        let jwt = format!(
            "{}.{}.{}",
            "eyJhbGciOiJIUzI1NiJ9", "eyJzdWIiOiIxMjM0In0", "signature01"
        );
        let input = [
            format!("openai={openai}"),
            format!("anthropic={anthropic}"),
            format!("github={github}"),
            format!("google={google}"),
            "Authorization: Bearer token-token-token-token".to_string(),
            format!("refresh_token={refresh}"),
            format!("jwt={jwt}"),
        ]
        .join("\n");

        let redacted = redact_secrets(input);

        assert!(redacted.contains("[REDACTED_SECRET]"));
        assert!(!redacted.contains("sk-proj-"));
        assert!(!redacted.contains("sk-ant-"));
        assert!(!redacted.contains("ghp_"));
        assert!(!redacted.contains("AIza"));
        assert!(!redacted.contains("token-token-token-token"));
        assert!(!redacted.contains("eeeeeeeeeeeeeeeeeeee"));
        assert!(!redacted.contains("eyJhbGci"));
    }

    #[test]
    fn does_not_redact_benign_identifiers() {
        let input = "thread_id=00000000-0000-0000-0000-000000000001 path=/tmp/codewith";
        assert_eq!(redact_secrets(input.to_string()), input);
    }
}
