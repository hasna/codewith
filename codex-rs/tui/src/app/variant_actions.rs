use super::App;
use crate::app_server_session::AppServerSession;
use codex_app_server_protocol::AgentRun;
use codex_app_server_protocol::Worktree;
use codex_app_server_protocol::WorktreeCleanupPolicy;

const VARIANT_BRANCH_PREFIX: &str = "codewith/variant";
const VARIANT_SLUG_FALLBACK: &str = "implementation";
const VARIANT_SLUG_MAX_LEN: usize = 48;

impl App {
    pub(super) async fn start_variants(
        &mut self,
        app_server: &mut AppServerSession,
        count: u8,
        name: Option<String>,
        start_point: Option<String>,
        prompt: String,
    ) {
        if !(2..=5).contains(&count) {
            self.chat_widget
                .add_error_message("Variant count must be between 2 and 5.".to_string());
            return;
        }

        let slug = variant_slug(name.as_deref(), prompt.as_str());
        let base_repo_path = self.current_worktree_base_repo_path().await;
        let mut started_agents: Vec<AgentRun> = Vec::new();
        let mut created = Vec::new();
        let mut failures = Vec::new();

        for index in 1..=count {
            let branch = variant_branch(slug.as_str(), index);
            let worktree_name = variant_worktree_name(slug.as_str(), index);
            let create_response = app_server
                .worktree_create(
                    base_repo_path.clone(),
                    Some(worktree_name.clone()),
                    Some(branch.clone()),
                    start_point.clone(),
                    Some(WorktreeCleanupPolicy::DeleteIfClean),
                    /*thread_id*/ None,
                )
                .await;
            let worktree = match create_response {
                Ok(response) => response.worktree,
                Err(err) => {
                    failures.push(format!(
                        "v{index}: failed to create worktree for branch {branch}: {err}"
                    ));
                    continue;
                }
            };

            let agent_prompt = variant_agent_prompt(
                index,
                count,
                prompt.as_str(),
                branch.as_str(),
                worktree_name.as_str(),
                worktree.worktree_path.as_str(),
            );
            let worktree_path = worktree.worktree_path.clone();
            let agent_response = app_server
                .agent_start(
                    agent_prompt,
                    /*initial_goal_objective*/ None,
                    Some(worktree_path.clone()),
                    Some(vec![worktree_path]),
                    self.active_thread_id,
                    self.config.selected_auth_profile.clone(),
                )
                .await;
            let agent = match agent_response {
                Ok(response) => response.agent,
                Err(err) => {
                    failures.push(format!(
                        "v{index}: created worktree {} on {branch}, but agent start failed: {err}",
                        short_id(worktree.worktree_id.as_str())
                    ));
                    created.push(created_variant_summary(
                        index, branch, &worktree, /*agent_id*/ None,
                    ));
                    continue;
                }
            };

            let agent_id = agent.agent_id.clone();
            if let Err(err) = app_server
                .worktree_attach(
                    worktree.worktree_id.clone(),
                    /*thread_id*/ None,
                    Some(agent_id.clone()),
                )
                .await
            {
                let stop_result = app_server.agent_stop(agent_id.clone()).await;
                let stop_suffix = match stop_result {
                    Ok(_) => " Agent stop was requested.".to_string(),
                    Err(stop_err) => format!(" Agent stop failed: {stop_err}"),
                };
                failures.push(format!(
                    "v{index}: created worktree {} on {branch} and started agent {}, but assignment failed: {err}.{stop_suffix}",
                    short_id(worktree.worktree_id.as_str()),
                    short_id(agent_id.as_str())
                ));
                created.push(created_variant_summary(
                    index,
                    branch,
                    &worktree,
                    Some(agent_id),
                ));
                continue;
            }

            created.push(created_variant_summary(
                index,
                branch,
                &worktree,
                Some(agent_id),
            ));
            started_agents.push(agent);
        }

        if !started_agents.is_empty() {
            self.chat_widget
                .show_background_agent_summary(started_agents);
        }

        if !created.is_empty() {
            self.chat_widget
                .add_info_message("Variants started".to_string(), Some(created.join("\n")));
        }

        if failures.is_empty() {
            self.chat_widget.add_info_message(
                "Variant run queued".to_string(),
                Some(format!(
                    "{count} variant agents were started. No variant PRs were opened."
                )),
            );
        } else {
            self.chat_widget.add_error_message(format!(
                "Variant run incomplete. No cleanup was performed.\n{}",
                failures.join("\n")
            ));
        }
    }
}

fn created_variant_summary(
    index: u8,
    branch: String,
    worktree: &Worktree,
    agent_id: Option<String>,
) -> String {
    let agent = agent_id
        .map(|agent_id| format!("; agent {}", short_id(agent_id.as_str())))
        .unwrap_or_default();
    format!(
        "v{index}: worktree {} on {branch} at {}{agent}",
        short_id(worktree.worktree_id.as_str()),
        worktree.worktree_path
    )
}

fn variant_worktree_name(slug: &str, index: u8) -> String {
    format!("variant-{slug}-v{index}")
}

fn variant_branch(slug: &str, index: u8) -> String {
    format!("{VARIANT_BRANCH_PREFIX}/{slug}-v{index}")
}

fn variant_slug(name: Option<&str>, prompt: &str) -> String {
    let source = name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| prompt.trim());
    let mut slug = String::new();
    let mut previous_was_dash = false;
    for ch in source.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            slug.push(lower);
            previous_was_dash = false;
        } else if !previous_was_dash && !slug.is_empty() {
            slug.push('-');
            previous_was_dash = true;
        }
        if slug.len() >= VARIANT_SLUG_MAX_LEN {
            break;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        VARIANT_SLUG_FALLBACK.to_string()
    } else {
        slug
    }
}

fn variant_agent_prompt(
    index: u8,
    count: u8,
    original_prompt: &str,
    branch: &str,
    worktree_name: &str,
    worktree_path: &str,
) -> String {
    format!(
        "You are implementation variant {index} of {count} for a Codewith /variant run.\n\nOriginal request:\n{original_prompt}\n\nBranch/worktree context:\n- Branch: {branch}\n- Worktree name: {worktree_name}\n- Worktree path: {worktree_path}\n\nCreate this variant only in the assigned worktree and branch. Leave code, tests, notes, and artifacts for later inspection. Do not open GitHub PRs. Do not create, request, or prepare a pull request for this generated variant."
    )
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

#[cfg(test)]
mod tests {
    use super::variant_agent_prompt;
    use super::variant_branch;
    use super::variant_slug;
    use super::variant_worktree_name;

    #[test]
    fn variant_slug_sanitizes_name_or_prompt() {
        assert_eq!(
            variant_slug(Some("Parser + Logging!"), "ignored"),
            "parser-logging"
        );
        assert_eq!(
            variant_slug(None, "Improve SQL/cache behavior"),
            "improve-sql-cache-behavior"
        );
        assert_eq!(variant_slug(Some("!!!"), "???"), "implementation");
    }

    #[test]
    fn variant_branch_and_worktree_names_are_deterministic() {
        assert_eq!(
            variant_branch("parser-logging", 3),
            "codewith/variant/parser-logging-v3"
        );
        assert_eq!(
            variant_worktree_name("parser-logging", 3),
            "variant-parser-logging-v3"
        );
    }

    #[test]
    fn variant_agent_prompt_contains_context_and_no_pr_instruction() {
        let prompt = variant_agent_prompt(
            2,
            4,
            "implement alternatives",
            "codewith/variant/parser-v2",
            "variant-parser-v2",
            "/repo/.codewith/worktrees/variant-parser-v2",
        );

        assert!(prompt.contains("implementation variant 2 of 4"));
        assert!(prompt.contains("Original request:\nimplement alternatives"));
        assert!(prompt.contains("Branch: codewith/variant/parser-v2"));
        assert!(prompt.contains("Worktree name: variant-parser-v2"));
        assert!(prompt.contains("Do not open GitHub PRs."));
        assert!(prompt.contains("Do not create, request, or prepare a pull request"));
    }
}
