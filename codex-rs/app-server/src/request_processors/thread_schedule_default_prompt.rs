use super::*;
use tokio::fs;

pub(super) const DEFAULT_LOOP_PROMPT_DISPLAY: &str = "Default loop prompt";
const DEFAULT_LOOP_PROMPT_MAX_BYTES: u64 = 64 * 1024;
const DEFAULT_LOOP_PROMPT_MAX_CHARS: usize = 4_000;

#[derive(Debug)]
pub(super) struct ResolvedDefaultLoopPrompt {
    pub prompt: String,
}

pub(super) async fn resolve_default_loop_prompt_for_thread(
    state_db: &StateDbHandle,
    thread_id: ThreadId,
    fallback_cwd: &Path,
    codex_home: &Path,
) -> anyhow::Result<ResolvedDefaultLoopPrompt> {
    let cwd = match state_db.get_thread(thread_id).await? {
        Some(metadata) if !metadata.cwd.as_os_str().is_empty() => metadata.cwd,
        _ => fallback_cwd.to_path_buf(),
    };
    resolve_default_loop_prompt(cwd.as_path(), codex_home).await
}

pub(super) async fn resolve_default_loop_prompt(
    cwd: &Path,
    codex_home: &Path,
) -> anyhow::Result<ResolvedDefaultLoopPrompt> {
    let project_prompt = cwd.join(".codewith").join("loop.md");
    let user_prompt = codex_home.join("loop.md");
    for path in [&project_prompt, &user_prompt] {
        match read_default_loop_prompt(path).await? {
            Some(prompt) => return Ok(ResolvedDefaultLoopPrompt { prompt }),
            None => continue,
        }
    }
    anyhow::bail!(
        "No default loop prompt found. Create .codewith/loop.md or ~/.codewith/loop.md, or pass an inline prompt."
    );
}

async fn read_default_loop_prompt(path: &Path) -> anyhow::Result<Option<String>> {
    let metadata = match fs::metadata(path).await {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            anyhow::bail!(
                "failed to read default loop prompt {}: {err}",
                path.display()
            )
        }
    };
    if !metadata.is_file() {
        anyhow::bail!("default loop prompt path is not a file: {}", path.display());
    }
    if metadata.len() > DEFAULT_LOOP_PROMPT_MAX_BYTES {
        anyhow::bail!(
            "default loop prompt {} is larger than {DEFAULT_LOOP_PROMPT_MAX_BYTES} bytes",
            path.display()
        );
    }

    let content = fs::read_to_string(path).await?;
    let content = content.trim();
    if content.is_empty() {
        anyhow::bail!("default loop prompt {} is empty", path.display());
    }
    let prompt: String = content
        .chars()
        .take(DEFAULT_LOOP_PROMPT_MAX_CHARS)
        .collect();
    Ok(Some(prompt))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    #[tokio::test]
    async fn project_default_loop_prompt_takes_precedence_over_user_prompt() {
        let codex_home = TempDir::new().expect("codex home");
        let workspace = TempDir::new().expect("workspace");
        fs::write(codex_home.path().join("loop.md"), "user prompt")
            .await
            .expect("write user prompt");
        let project_codex = workspace.path().join(".codewith");
        fs::create_dir_all(&project_codex)
            .await
            .expect("create project prompt dir");
        fs::write(project_codex.join("loop.md"), "  project prompt  \n")
            .await
            .expect("write project prompt");

        let resolved = resolve_default_loop_prompt(workspace.path(), codex_home.path())
            .await
            .expect("default prompt should resolve");

        assert_eq!("project prompt", resolved.prompt);
    }

    #[tokio::test]
    async fn default_loop_prompt_falls_back_to_user_prompt_and_truncates() {
        let codex_home = TempDir::new().expect("codex home");
        let workspace = TempDir::new().expect("workspace");
        let long_prompt = "a".repeat(DEFAULT_LOOP_PROMPT_MAX_CHARS + 8);
        fs::write(codex_home.path().join("loop.md"), long_prompt)
            .await
            .expect("write user prompt");

        let resolved = resolve_default_loop_prompt(workspace.path(), codex_home.path())
            .await
            .expect("default prompt should resolve");

        assert_eq!(
            DEFAULT_LOOP_PROMPT_MAX_CHARS,
            resolved.prompt.chars().count()
        );
    }

    #[tokio::test]
    async fn default_loop_prompt_reports_missing_prompt_locations() {
        let codex_home = TempDir::new().expect("codex home");
        let workspace = TempDir::new().expect("workspace");

        let err = resolve_default_loop_prompt(workspace.path(), codex_home.path())
            .await
            .expect_err("missing prompt should fail");

        assert!(err.to_string().contains(".codewith/loop.md"));
        assert!(err.to_string().contains("~/.codewith/loop.md"));
    }
}
