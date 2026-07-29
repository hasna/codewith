pub(crate) mod code_mode;
pub(crate) mod context;
pub(crate) mod events;
pub(crate) mod handlers;
pub(crate) mod hook_names;
pub(crate) mod hosted_spec;
pub(crate) mod lifecycle;
pub(crate) mod network_approval;
pub(crate) mod orchestrator;
pub(crate) mod parallel;
pub mod policy;
pub(crate) mod registry;
pub(crate) mod router;
pub(crate) mod runtimes;
pub(crate) mod sandboxing;
pub(crate) mod smart_suggest;
pub(crate) mod spec_plan;
pub(crate) mod tool_dispatch_trace;
pub(crate) mod tool_search_history;

use std::borrow::Cow;

use codex_protocol::exec_output::ExecToolCallOutput;
use codex_tools::ToolName;
use codex_utils_output_truncation::TruncationPolicy;
use codex_utils_output_truncation::formatted_truncate_text;
use codex_utils_output_truncation::truncate_text;
pub use router::ToolRouter;

// Telemetry preview limits: keep log events smaller than model budgets.
pub(crate) const TELEMETRY_PREVIEW_MAX_BYTES: usize = 2 * 1024; // 2 KiB
pub(crate) const TELEMETRY_PREVIEW_MAX_LINES: usize = 64; // lines
pub(crate) const TELEMETRY_PREVIEW_TRUNCATION_NOTICE: &str =
    "[... telemetry preview truncated ...]";

/// Legacy boundaries such as hook payloads, telemetry tags, and Responses tool
/// names still require a single flattened string. Keep comparisons and sorting
/// on `ToolName` itself; use this only when crossing those boundaries.
pub(crate) fn flat_tool_name(tool_name: &ToolName) -> Cow<'_, str> {
    match tool_name.namespace.as_deref() {
        Some(namespace) => {
            let mut name = String::with_capacity(namespace.len() + tool_name.name.len());
            name.push_str(namespace);
            name.push_str(&tool_name.name);
            Cow::Owned(name)
        }
        None => Cow::Borrowed(tool_name.name.as_str()),
    }
}

pub(crate) fn tool_user_shell_type(
    user_shell: &crate::shell::Shell,
) -> codex_tools::ToolUserShellType {
    match user_shell.shell_type {
        crate::shell::ShellType::Zsh => codex_tools::ToolUserShellType::Zsh,
        crate::shell::ShellType::Bash => codex_tools::ToolUserShellType::Bash,
        crate::shell::ShellType::PowerShell => codex_tools::ToolUserShellType::PowerShell,
        crate::shell::ShellType::Sh => codex_tools::ToolUserShellType::Sh,
        crate::shell::ShellType::Cmd => codex_tools::ToolUserShellType::Cmd,
    }
}

/// Format the combined exec output for sending back to the model.
/// Includes exit code and duration metadata; truncates large bodies safely.
pub fn format_exec_output_for_model(
    exec_output: &ExecToolCallOutput,
    truncation_policy: TruncationPolicy,
) -> String {
    // round to 1 decimal place
    let duration_seconds = ((exec_output.duration.as_secs_f32()) * 10.0).round() / 10.0;

    let content = build_content_with_timeout(exec_output);

    let total_lines = content.lines().count();

    let formatted_output = truncate_text(&content, truncation_policy);

    let mut sections = Vec::new();

    sections.push(format!("Exit code: {}", exec_output.exit_code));
    sections.push(format!("Wall time: {duration_seconds} seconds"));
    if total_lines != formatted_output.lines().count() {
        sections.push(format!("Total output lines: {total_lines}"));
    }

    sections.push("Output:".to_string());
    sections.push(formatted_output);

    sections.join("\n")
}

pub fn format_exec_output_str(
    exec_output: &ExecToolCallOutput,
    truncation_policy: TruncationPolicy,
) -> String {
    let content = build_content_with_timeout(exec_output);

    // Truncate for model consumption before serialization.
    formatted_truncate_text(&content, truncation_policy)
}

/// Extracts exec output content and prepends a timeout message if the command timed out.
fn build_content_with_timeout(exec_output: &ExecToolCallOutput) -> String {
    let content = if exec_output.timed_out {
        format!(
            "command timed out after {} milliseconds\n{}",
            exec_output.duration.as_millis(),
            exec_output.aggregated_output.text
        )
    } else {
        exec_output.aggregated_output.text.clone()
    };

    codex_secrets::redact_secrets(content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_protocol::exec_output::StreamOutput;
    use pretty_assertions::assert_eq;
    use std::time::Duration;

    fn exec_output(aggregated_output: String) -> ExecToolCallOutput {
        ExecToolCallOutput {
            exit_code: 0,
            stdout: StreamOutput::new(String::new()),
            stderr: StreamOutput::new(String::new()),
            aggregated_output: StreamOutput::new(aggregated_output),
            duration: Duration::from_millis(1250),
            timed_out: false,
        }
    }

    fn openai_key(ch: char) -> String {
        format!("{}{}", "sk-proj-", ch.to_string().repeat(32))
    }

    fn github_token(ch: char) -> String {
        format!("{}{}", "ghp_", ch.to_string().repeat(36))
    }

    fn assert_secret_redacted(output: &str, secret: &str) {
        assert!(
            !output.contains(secret),
            "formatted output should not contain secret: {output}"
        );
        assert!(
            output.contains("[REDACTED_SECRET]"),
            "formatted output should include redaction marker: {output}"
        );
    }

    #[test]
    fn model_exec_output_redacts_secrets_before_headers_and_truncation() {
        let exact_assignment = format!("token={}", "x".repeat(20));
        let suffix_assignment_secret = openai_key('a');
        let quoted_secret = openai_key('b');
        let escaped_value_secret = github_token('c');
        let content = format!(
            "ready\n{exact_assignment}\nSERVICE_TOKEN={suffix_assignment_secret}\napi_key=\"{quoted_secret}\"\nescaped={escaped_value_secret}\\ with-space\nbenign output\n"
        );
        let output = exec_output(content);

        let formatted = format_exec_output_for_model(&output, TruncationPolicy::Bytes(512));

        assert!(formatted.starts_with("Exit code: 0\nWall time: 1.3 seconds\nOutput:\n"));
        assert_secret_redacted(&formatted, &"x".repeat(20));
        assert_secret_redacted(&formatted, &suffix_assignment_secret);
        assert_secret_redacted(&formatted, &quoted_secret);
        assert_secret_redacted(&formatted, &escaped_value_secret);
        assert!(formatted.contains("ready"));
        assert!(formatted.contains("benign output"));
    }

    #[test]
    fn formatted_exec_output_str_redacts_timeout_output_before_truncation() {
        let secret = openai_key('d');
        let mut output = exec_output(format!(
            "prefix\n{}{secret}\ntail survives",
            "middle\n".repeat(30)
        ));
        output.timed_out = true;
        output.duration = Duration::from_millis(2500);

        let formatted = format_exec_output_str(&output, TruncationPolicy::Bytes(80));

        assert!(formatted.starts_with("Total output lines: "));
        assert!(formatted.contains("command timed out"));
        assert!(formatted.contains("chars truncated"));
        assert_secret_redacted(&formatted, &secret);
        assert!(formatted.contains("tail survives"));
    }

    #[test]
    fn formatted_exec_output_preserves_benign_output() {
        let output = exec_output("PATH=/tmp/bin\nstatus=ok\nthread_id=abc123".to_string());

        assert_eq!(
            format_exec_output_str(&output, TruncationPolicy::Bytes(512)),
            "PATH=/tmp/bin\nstatus=ok\nthread_id=abc123"
        );
    }
}
