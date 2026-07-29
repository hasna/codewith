use regex::Regex;
use std::sync::LazyLock;

static OPENAI_KEY_REGEX: LazyLock<Regex> =
    LazyLock::new(|| compile_regex(r"\bsk(?:-[A-Za-z0-9][A-Za-z0-9_-]{7,}){1,}\b"));
static AWS_ACCESS_KEY_ID_REGEX: LazyLock<Regex> =
    LazyLock::new(|| compile_regex(r"\bAKIA[0-9A-Z]{16}\b"));
static AWS_SECRET_ACCESS_KEY_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    compile_regex(concat!(
        r#"(?i)\baws_secret"#,
        r#"_access_key\b(\s*[:=]\s*)(["']?)[^\s"']{20,}"#
    ))
});
static BEARER_TOKEN_REGEX: LazyLock<Regex> =
    LazyLock::new(|| compile_regex(concat!(r"(?i)\bBearer\s+", r"[A-Za-z0-9._\-]{16,}\b")));
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
static OUTPUT_OMISSION_MARKER_LINE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| compile_regex(r"^\.\.\. [0-9]+ bytes omitted \.\.\.$"));
static ENV_ASSIGNMENT_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    compile_regex(concat!(
        r#"(^|[^A-Za-z0-9_])"#,
        r#"([A-Za-z_][A-Za-z0-9_]*)"#,
        r#"(\s*=\s*)"#,
        r#"("(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|[^\s"']+)"#
    ))
});
static ENV_ASSIGNMENT_BOUNDARY_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    compile_regex(concat!(
        r#"(^|[^A-Za-z0-9_])"#,
        r#"([A-Za-z_][A-Za-z0-9_]*)"#,
        r#"(\s*=\s*)"#,
        r#"("(?:\\.|[^"\\])*"?|'(?:\\.|[^'\\])*'?|[^\s"']*)"#,
        r#"\s*$"#
    ))
});
static SECRET_ASSIGNMENT_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    compile_regex(concat!(
        r#"(?i)\b(api[_-]?key|access[_-]?"#,
        r#"token|refresh[_-]?token|id[_-]?"#,
        r#"token|auth(?:orization)?|token|secret|client[_-]?"#,
        r#"secret|password|private[_-]?key)\b(\s*[:=]\s*)(["']?)[^\s"']{8,}"#,
    ))
});

/// Remove secret and keys from a String. This is done on best effort basis following some
/// well-known REGEX.
pub fn redact_secrets(input: String) -> String {
    let input = redact_omission_boundary_secret_fragments(input);
    let redacted = OPENAI_KEY_REGEX.replace_all(&input, "[REDACTED_SECRET]");
    let redacted = AWS_ACCESS_KEY_ID_REGEX.replace_all(&redacted, "[REDACTED_SECRET]");
    let redacted = AWS_SECRET_ACCESS_KEY_REGEX
        .replace_all(&redacted, "aws_secret_access_key$1$2[REDACTED_SECRET]");
    let redacted = BEARER_TOKEN_REGEX.replace_all(&redacted, "Bearer [REDACTED_SECRET]");
    let redacted = GITHUB_TOKEN_REGEX.replace_all(&redacted, "[REDACTED_SECRET]");
    let redacted = GOOGLE_API_KEY_REGEX.replace_all(&redacted, "[REDACTED_SECRET]");
    let redacted = ANTHROPIC_KEY_REGEX.replace_all(&redacted, "[REDACTED_SECRET]");
    let redacted = JWT_REGEX.replace_all(&redacted, "[REDACTED_SECRET]");
    let redacted = redact_sensitive_env_assignments(&redacted);
    let redacted = SECRET_ASSIGNMENT_REGEX.replace_all(&redacted, "$1$2$3[REDACTED_SECRET]");

    redacted.to_string()
}

fn redact_sensitive_env_assignments(input: &str) -> String {
    ENV_ASSIGNMENT_REGEX
        .replace_all(input, |captures: &regex::Captures<'_>| {
            let name = captures.get(2).map_or("", |capture| capture.as_str());
            if is_sensitive_env_name(name) {
                let value = captures.get(4).map_or("", |capture| capture.as_str());
                let redacted = redact_assignment_value(value);
                format!(
                    "{}{}{}{}",
                    captures.get(1).map_or("", |capture| capture.as_str()),
                    name,
                    captures.get(3).map_or("", |capture| capture.as_str()),
                    redacted
                )
            } else {
                captures
                    .get(0)
                    .map_or(String::new(), |capture| capture.as_str().to_string())
            }
        })
        .to_string()
}

fn redact_omission_boundary_secret_fragments(input: String) -> String {
    let mut replacements = Vec::new();
    let mut previous_line = None;
    let mut line_start = 0;

    while line_start < input.len() {
        let line_end = input[line_start..]
            .find('\n')
            .map(|idx| line_start + idx)
            .unwrap_or(input.len());
        let next_line_start =
            line_end + usize::from(input.as_bytes().get(line_end) == Some(&b'\n'));
        let line = &input[line_start..line_end];

        if OUTPUT_OMISSION_MARKER_LINE_REGEX.is_match(line) {
            if let Some((previous_line_start, previous_line_end)) = previous_line
                && let Some(value_start) = sensitive_assignment_value_start_at_line_end(
                    &input[previous_line_start..previous_line_end],
                )
            {
                replacements.push((previous_line_start + value_start, previous_line_end));
            }

            let tail_line_start = next_line_start;
            let tail_line_end = input[tail_line_start..]
                .find('\n')
                .map(|idx| tail_line_start + idx)
                .unwrap_or(input.len());
            if tail_line_start < tail_line_end {
                replacements.push((tail_line_start, tail_line_end));
            }
        }

        previous_line = Some((line_start, line_end));
        line_start = next_line_start;
    }

    let mut redacted = String::with_capacity(input.len());
    let mut last = 0;
    for (start, end) in replacements {
        if start < last {
            continue;
        }
        redacted.push_str(&input[last..start]);
        redacted.push_str("[REDACTED_SECRET]");
        last = end;
    }
    redacted.push_str(&input[last..]);
    redacted
}

fn sensitive_assignment_value_start_at_line_end(line: &str) -> Option<usize> {
    ENV_ASSIGNMENT_BOUNDARY_REGEX
        .captures_iter(line)
        .filter_map(|captures| {
            let name = captures.get(2)?.as_str();
            let value = captures.get(4)?;
            is_sensitive_env_name(name).then_some(value.start())
        })
        .last()
}

fn redact_assignment_value(value: &str) -> String {
    let mut chars = value.chars();
    let Some(quote) = chars.next().filter(|quote| *quote == '"' || *quote == '\'') else {
        return "[REDACTED_SECRET]".to_string();
    };

    if value.ends_with(quote) && value.len() > quote.len_utf8() {
        format!("{quote}[REDACTED_SECRET]{quote}")
    } else {
        "[REDACTED_SECRET]".to_string()
    }
}

fn is_sensitive_env_name(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    [
        "API_KEY",
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "ACCESS_TOKEN",
        "AUTH_TOKEN",
        "BEARER_TOKEN",
    ]
    .into_iter()
    .any(|suffix| name == suffix || name.ends_with(suffix))
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
            format!(
                "Authorization: {} {}",
                "Bearer",
                ["token", "token", "token", "token"].join("-")
            ),
            format!("{}={refresh}", "refresh_token"),
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

    #[test]
    fn redacts_argv_embedded_env_assignments_with_sensitive_names() {
        let suffixes = [
            vec!["API", "_", "KEY"],
            vec!["TOKEN"],
            vec!["SECRET"],
            vec!["PASSWORD"],
            vec!["ACCESS", "_", "TOKEN"],
            vec!["AUTH", "_", "TOKEN"],
            vec!["BEARER", "_", "TOKEN"],
        ];
        let value = ["runtime", "fixture", "value", "1234567890"].join("-");
        let mut assignments = Vec::new();
        let mut expected_assignments = Vec::new();

        for suffix_parts in suffixes {
            let suffix = suffix_parts.concat();
            for name in [suffix.clone(), format!("SERVICE_{suffix}")] {
                assignments.push(format!(r#""{name}={value}""#));
                expected_assignments.push(format!(r#""{name}=[REDACTED_SECRET]""#));
            }
        }

        let api_key_name = ["SERVICE", "API", "_", "KEY"].concat();
        assignments.push(format!(r#""{api_key_name}={value}""#));
        expected_assignments.push(format!(r#""{api_key_name}=[REDACTED_SECRET]""#));

        for name_parts in [["MY", "TOKEN"], ["APP", "SECRET"], ["DB", "PASSWORD"]] {
            let name = name_parts.concat();
            assignments.push(format!(r#""{name}={value}""#));
            expected_assignments.push(format!(r#""{name}=[REDACTED_SECRET]""#));
        }

        let redacted = redact_secrets(format!("argv=[{}]", assignments.join(", ")));

        for assignment in expected_assignments {
            assert!(redacted.contains(&assignment));
        }
        assert!(!redacted.contains(&value));
    }

    #[test]
    fn redacts_quoted_and_escaped_sensitive_env_assignment_values() {
        let value = ["quoted", "runtime", "fixture", "1234567890"].join("-");
        let escaped = ["escaped", "runtime", "fixture", "1234567890"].join("-");
        let secret_name = ["SERVICE", "_", "SECRET"].concat();
        let token_name = ["SERVICE", "_", "TOKEN"].concat();
        let input =
            format!(r#"{secret_name}='{value} tail' {token_name}="prefix \"{escaped}\" tail""#);

        let redacted = redact_secrets(input);

        assert!(redacted.contains(&format!("{secret_name}='[REDACTED_SECRET]'")));
        assert!(redacted.contains(&format!("{token_name}=\"[REDACTED_SECRET]\"")));
        assert!(!redacted.contains(&value));
        assert!(!redacted.contains(&escaped));
    }

    #[test]
    fn keeps_benign_assignments_that_only_contain_sensitive_words() {
        let input = [
            "SECRETIVE=visible",
            "PASSWORDLESS=true",
            "TOKENIZER_MODEL=visible",
            "AUTH_TOKENIZER=visible",
            "MYAPI_KEYISH=visible",
            "emoji=\u{1F510}",
        ]
        .join(" ");

        assert_eq!(redact_secrets(input.clone()), input);
    }

    #[test]
    fn redacts_assignment_fragments_around_omission_markers() {
        let name = ["SERVICE", "_", "ACCESS", "_", "TOKEN"].concat();
        let marker = ["...", " 333 bytes omitted ", "..."].concat();
        let head = ["runtime", "fixture"].join("-");
        let tail = ["value", "1234567890"].join("-");
        let input = format!("before\n{name}={head}\n{marker}\n{tail}\nafter");

        let redacted = redact_secrets(input);

        assert!(redacted.contains(&format!("{name}=[REDACTED_SECRET]")));
        assert_eq!(redacted.matches(&marker).count(), 1);
        for fragment in [head, tail] {
            assert!(
                !redacted.contains(&fragment),
                "omitted secret fragment survived in redacted output: {fragment}"
            );
        }
        assert!(redacted.contains("before"));
        assert!(redacted.contains("after"));
    }

    #[test]
    fn redacts_quoted_assignment_tail_line_around_omission_markers() {
        let name = ["SERVICE", "_", "ACCESS", "_", "TOKEN"].concat();
        let marker = ["...", " 333 bytes omitted ", "..."].concat();
        let head = ["runtime", "fixture"].join(" ");
        let tail = ["value", "1234567890"].join(" ");
        let input = format!("before\n{name}=\"{head}\n{marker}\n{tail}\"\nafter");

        let redacted = redact_secrets(input);

        assert!(redacted.contains(&format!("{name}=[REDACTED_SECRET]")));
        assert_eq!(redacted.matches(&marker).count(), 1);
        for fragment in [head, tail] {
            assert!(
                !redacted.contains(&fragment),
                "omitted secret fragment survived in redacted output: {fragment}"
            );
        }
        assert!(redacted.contains("before"));
        assert!(redacted.contains("after"));
    }

    #[test]
    fn redacts_assignment_tail_when_value_starts_in_omitted_bytes() {
        let name = ["SERVICE", "_", "ACCESS", "_", "TOKEN"].concat();
        let marker = ["...", " 333 bytes omitted ", "..."].concat();
        let tail = ["value", "1234567890"].join("-");
        let input = format!("before\n{name}=\n{marker}\n{tail}\nafter");

        let redacted = redact_secrets(input);

        assert!(redacted.contains(&format!("{name}=[REDACTED_SECRET]")));
        assert_eq!(redacted.matches(&marker).count(), 1);
        assert!(
            !redacted.contains(&tail),
            "omitted secret fragment survived in redacted output: {tail}"
        );
        assert!(redacted.contains("before"));
        assert!(redacted.contains("after"));
    }

    #[test]
    fn redacts_first_tail_line_when_omission_marker_splits_sensitive_name() {
        let marker = ["...", " 333 bytes omitted ", "..."].concat();
        let tail = ["unpatterned", "secret", "tail"].join("-");
        let input = format!("before\nSERVICE_ACCESS_\n{marker}\nTOKEN={tail}\nafter");

        let redacted = redact_secrets(input);

        assert!(redacted.contains(&format!(
            "SERVICE_ACCESS_\n{marker}\n[REDACTED_SECRET]\nafter"
        )));
        assert!(!redacted.contains(&tail));
    }

    #[test]
    fn redacts_first_tail_line_after_exact_omission_marker() {
        let marker = ["...", " 333 bytes omitted ", "..."].concat();
        let ambiguous_tail = "plain benign tail";
        let later_line = "later benign line \u{1F510}";
        let input = format!("before\nplain head\n{marker}\n{ambiguous_tail}\n{later_line}");

        let redacted = redact_secrets(input);

        assert_eq!(
            redacted,
            format!("before\nplain head\n{marker}\n[REDACTED_SECRET]\n{later_line}")
        );
    }

    #[test]
    fn preserves_marker_like_inline_output_without_boundary_redaction() {
        let marker = ["...", " 333 bytes omitted ", "..."].concat();
        let input = format!("before {marker} after\nplain tail");

        assert_eq!(redact_secrets(input.clone()), input);
    }
}
