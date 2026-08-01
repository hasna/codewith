use super::*;
use codex_protocol::protocol::ReviewImplementerProvenance;
use codex_protocol::protocol::ReviewImplementerProvenanceSource;
use pretty_assertions::assert_eq;

#[test]
fn review_prompt_template_renders_base_branch_backup_variant() {
    assert_eq!(
        render_review_prompt(&BASE_BRANCH_PROMPT_BACKUP_TEMPLATE, [("branch", "main")]),
        "Review the code changes against the base branch 'main'. Start by finding the merge diff between the current branch and main's upstream e.g. (`git merge-base HEAD \"$(git rev-parse --abbrev-ref \"main@{upstream}\")\"`), then run `git diff` against that SHA to see what changes we would merge into the main branch. Provide prioritized, actionable findings."
    );
}

#[test]
fn review_prompt_template_renders_base_branch_variant() {
    assert_eq!(
        render_review_prompt(
            &BASE_BRANCH_PROMPT_TEMPLATE,
            [("base_branch", "main"), ("merge_base_sha", "abc123")]
        ),
        "Review the code changes against the base branch 'main'. The merge base commit for this comparison is abc123. Run `git diff abc123` to inspect the changes relative to main. Provide prioritized, actionable findings."
    );
}

#[test]
fn review_prompt_template_renders_commit_variant() {
    assert_eq!(
        review_prompt(
            &ReviewTarget::Commit {
                sha: "deadbeef".to_string(),
                title: None,
            },
            &AbsolutePathBuf::current_dir().expect("cwd"),
        )
        .expect("commit prompt should render"),
        "Review the code changes introduced by commit deadbeef. Provide prioritized, actionable findings."
    );
}

#[test]
fn review_prompt_template_renders_commit_variant_with_title() {
    assert_eq!(
        review_prompt(
            &ReviewTarget::Commit {
                sha: "deadbeef".to_string(),
                title: Some("Fix bug".to_string()),
            },
            &AbsolutePathBuf::current_dir().expect("cwd"),
        )
        .expect("commit prompt should render"),
        "Review the code changes introduced by commit deadbeef (\"Fix bug\"). Provide prioritized, actionable findings."
    );
}

#[test]
fn resolved_prompt_carries_the_exact_typed_review_envelope() {
    let envelope = ReviewEnvelope {
        schema_version: "codewith-review-envelope-v1".to_string(),
        repository_origin: "github.com/hasna/codewith".to_string(),
        pull_request_number: 17,
        base_ref: "refs/remotes/origin/main".to_string(),
        reviewed_base_sha: "a".repeat(40),
        head_sha: "b".repeat(40),
        merge_result_tree_sha: "c".repeat(40),
        candidate_sha256: "d".repeat(64),
        acceptance_scope_id: "codewith-review-envelope-v1".to_string(),
        acceptance_scope_sha256: "e".repeat(64),
        implementer: ReviewImplementerProvenance {
            source: ReviewImplementerProvenanceSource::GitAgentTrailer,
            agent: "Herminia".to_string(),
            commit_sha: "b".repeat(40),
        },
        envelope_sha256: "f".repeat(64),
    };
    let canonical = envelope.canonical_json().expect("canonical envelope");
    let resolved = resolve_review_request(
        ReviewRequest {
            target: ReviewTarget::Commit {
                sha: envelope.head_sha.clone(),
                title: None,
            },
            user_facing_hint: None,
            review_envelope: Some(envelope.clone()),
        },
        &AbsolutePathBuf::current_dir().expect("cwd"),
    )
    .expect("resolved review request");

    assert_eq!(resolved.review_envelope, Some(envelope));
    assert_eq!(resolved.prompt.matches(canonical.as_str()).count(), 1);
    assert!(
        resolved
            .prompt
            .contains("do not infer identity from comments")
    );
}
