use crate::ManagedWorktreeLifecycleStatus;
use crate::ManagedWorktreeMode;
use sqlx::Row;
use sqlx::SqlitePool;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

/// Returns the display form persisted for a managed-worktree path.
///
/// This resolves existing ancestors and removes lexical aliases, but does not
/// case-fold the resulting display path. Equality checks must use
/// [`managed_worktree_path_key_from_display`] instead.
pub(crate) fn path_to_db_string(path: &Path) -> String {
    path_to_string(&normalize_path_for_db(path))
}

/// Returns the deterministic equality key for a managed-worktree path.
///
/// Windows keys use Rust's locale-independent Unicode lowercase mapping. This
/// handles ASCII and ordinary Unicode case pairs without depending on the
/// process locale, while remaining deterministic across create, reconciliation,
/// and cleanup admission. It intentionally does not attempt to reproduce every
/// filesystem-specific Windows upcase-table edge case; a conservative
/// locale-neutral key is preferable to allowing ordinary spelling aliases to
/// bypass worktree ownership. Unix and macOS keys retain their display casing.
#[cfg(test)]
fn managed_worktree_path_key(path: &Path) -> String {
    managed_worktree_path_key_from_display(path_to_db_string(path).as_str())
}

/// Derives the equality key for an already-normalized display path.
pub(crate) fn managed_worktree_path_key_from_display(display_path: &str) -> String {
    normalize_path_key(display_path.to_owned())
}

#[cfg(windows)]
fn normalize_path_key(path: String) -> String {
    path.to_lowercase()
}

#[cfg(not(windows))]
fn normalize_path_key(path: String) -> String {
    path
}

fn normalize_path_for_db(path: &Path) -> PathBuf {
    if let Ok(canonical_path) = std::fs::canonicalize(path) {
        return normalize_path_components(&canonical_path);
    }

    let components = path.components().collect::<Vec<_>>();
    for existing_component_count in (1..components.len()).rev() {
        let mut existing_ancestor = PathBuf::new();
        for component in &components[..existing_component_count] {
            existing_ancestor.push(component.as_os_str());
        }
        let Ok(canonical_ancestor) = std::fs::canonicalize(existing_ancestor) else {
            continue;
        };

        let mut normalized = normalize_path_components(&canonical_ancestor);
        for component in &components[existing_component_count..] {
            match component {
                Component::CurDir => {}
                Component::ParentDir | Component::Normal(_) => {
                    normalized.push(component.as_os_str());
                }
                Component::RootDir | Component::Prefix(_) => {
                    return normalize_path_components(path);
                }
            }
        }
        return normalize_path_components(&normalized);
    }

    normalize_path_components(path)
}

fn normalize_path_components(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
            Component::RootDir | Component::Prefix(_) => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

struct LegacyManagedWorktreePathRow {
    worktree_id: String,
    mode: String,
    base_repo_path: String,
    worktree_path: String,
    worktree_path_key: Option<String>,
    lifecycle_status: String,
    released_at_ms: Option<i64>,
    deleted_at_ms: Option<i64>,
}

/// Reconciles legacy managed-worktree displays and equality keys at startup.
///
/// The preflight leaves colliding legacy rows in place, so cleanup keeps the
/// collision guard effective instead of merging or deleting user data.
pub(crate) async fn normalize_legacy_managed_worktree_paths(
    pool: &SqlitePool,
) -> anyhow::Result<()> {
    let rows = sqlx::query(
        r#"
SELECT
    worktree_id,
    mode,
    base_repo_path,
    worktree_path,
    worktree_path_key,
    lifecycle_status,
    released_at_ms,
    deleted_at_ms
FROM managed_worktrees
ORDER BY worktree_id ASC
        "#,
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|row| {
        Ok(LegacyManagedWorktreePathRow {
            worktree_id: row.try_get("worktree_id")?,
            mode: row.try_get("mode")?,
            base_repo_path: row.try_get("base_repo_path")?,
            worktree_path: row.try_get("worktree_path")?,
            worktree_path_key: row.try_get("worktree_path_key")?,
            lifecycle_status: row.try_get("lifecycle_status")?,
            released_at_ms: row.try_get("released_at_ms")?,
            deleted_at_ms: row.try_get("deleted_at_ms")?,
        })
    })
    .collect::<anyhow::Result<Vec<_>>>()?;

    let normalized_rows = rows
        .iter()
        .map(|row| {
            let base_repo_path = Path::new(&row.base_repo_path);
            let worktree_path = Path::new(&row.worktree_path);
            let normalized_base_repo_path = path_to_db_string(base_repo_path);
            let normalized_worktree_path = path_to_db_string(worktree_path);
            (
                row,
                managed_worktree_path_key_from_display(normalized_base_repo_path.as_str()),
                managed_worktree_path_key_from_display(normalized_worktree_path.as_str()),
                normalized_base_repo_path,
                normalized_worktree_path,
            )
        })
        .collect::<Vec<_>>();

    let mut live_isolated_worktrees = BTreeMap::new();
    let mut active_shared_repositories = BTreeMap::new();
    for (row, normalized_base_repo_path_key, normalized_worktree_path_key, _, _) in &normalized_rows
    {
        if row.mode == ManagedWorktreeMode::IsolatedWorktree.as_str() && row.deleted_at_ms.is_none()
        {
            collect_normalized_managed_worktree_path(
                &mut live_isolated_worktrees,
                normalized_worktree_path_key,
                row.worktree_id.as_str(),
            );
        }
        if row.mode == ManagedWorktreeMode::SharedRepository.as_str()
            && row.deleted_at_ms.is_none()
            && row.released_at_ms.is_none()
            && row.lifecycle_status == ManagedWorktreeLifecycleStatus::Active.as_str()
        {
            collect_normalized_managed_worktree_path(
                &mut active_shared_repositories,
                normalized_base_repo_path_key,
                row.worktree_id.as_str(),
            );
        }
    }

    let collisions = [
        ("live isolated worktree path", live_isolated_worktrees),
        ("active shared repository path", active_shared_repositories),
    ]
    .into_iter()
    .flat_map(|(path_kind, paths)| {
        paths
            .into_iter()
            .filter_map(move |(normalized_path, worktree_ids)| {
                (worktree_ids.len() > 1).then_some((path_kind, normalized_path, worktree_ids))
            })
    })
    .collect::<Vec<_>>();
    let collision_worktree_ids = collisions
        .iter()
        .flat_map(|(_, _, worktree_ids)| worktree_ids.iter().cloned())
        .collect::<BTreeSet<_>>();

    let mut transaction = pool.begin().await?;
    for (
        row,
        normalized_base_repo_path_key,
        normalized_worktree_path_key,
        normalized_base_repo_path,
        normalized_worktree_path,
    ) in normalized_rows
    {
        let admission_path_key = if row.mode == ManagedWorktreeMode::SharedRepository.as_str() {
            normalized_base_repo_path_key
        } else {
            normalized_worktree_path_key
        };
        if collision_worktree_ids.contains(row.worktree_id.as_str()) {
            if row.worktree_path_key.as_deref() != Some(admission_path_key.as_str()) {
                sqlx::query(
                    "UPDATE managed_worktrees SET worktree_path_key = ? WHERE worktree_id = ?",
                )
                .bind(admission_path_key)
                .bind(row.worktree_id.clone())
                .execute(&mut *transaction)
                .await?;
            }
            continue;
        }
        if row.base_repo_path == normalized_base_repo_path
            && row.worktree_path == normalized_worktree_path
            && row.worktree_path_key.as_deref() == Some(admission_path_key.as_str())
        {
            continue;
        }
        sqlx::query(
            r#"
UPDATE managed_worktrees
SET base_repo_path = ?, worktree_path = ?, worktree_path_key = ?
WHERE worktree_id = ?
            "#,
        )
        .bind(normalized_base_repo_path)
        .bind(normalized_worktree_path)
        .bind(admission_path_key)
        .bind(row.worktree_id.clone())
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    for (path_kind, normalized_path, worktree_ids) in collisions {
        tracing::warn!(
            %path_kind,
            %normalized_path,
            ?worktree_ids,
            "managed worktree path normalization collision; retaining legacy rows without merging or deleting them"
        );
    }
    Ok(())
}

fn collect_normalized_managed_worktree_path(
    paths: &mut BTreeMap<String, Vec<String>>,
    normalized_path: &str,
    worktree_id: &str,
) {
    paths
        .entry(normalized_path.to_string())
        .or_default()
        .push(worktree_id.to_string());
}

pub(crate) fn path_to_string(path: &Path) -> String {
    let path = path.to_string_lossy().into_owned();
    strip_windows_verbatim_prefix(path)
}

#[cfg(windows)]
fn strip_windows_verbatim_prefix(path: String) -> String {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{rest}");
    }
    if let Some(rest) = path.strip_prefix(r"\\?\") {
        return rest.to_owned();
    }
    path
}

#[cfg(not(windows))]
fn strip_windows_verbatim_prefix(path: String) -> String {
    path
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    use super::super::ManagedWorktreeCreateParams;
    #[cfg(windows)]
    use super::super::ManagedWorktreeReleaseParams;
    use super::*;
    #[cfg(windows)]
    use crate::ManagedWorktreeCleanupPolicy;
    #[cfg(windows)]
    use crate::ManagedWorktreeMode;
    #[cfg(windows)]
    use crate::ManagedWorktreeOwnerKind;
    #[cfg(windows)]
    use crate::runtime::StateRuntime;
    use crate::runtime::test_support::unique_temp_dir;
    use pretty_assertions::assert_eq;
    #[cfg(windows)]
    use serde_json::json;
    #[cfg(windows)]
    use std::sync::Arc;

    #[cfg(windows)]
    async fn test_runtime() -> Arc<StateRuntime> {
        StateRuntime::init(unique_temp_dir(), "test-provider".to_string())
            .await
            .expect("state db should initialize")
    }

    fn test_temp_dir() -> anyhow::Result<PathBuf> {
        let path = unique_temp_dir();
        std::fs::create_dir_all(&path)?;
        Ok(path)
    }

    #[cfg(windows)]
    fn create_params_for_paths(
        worktree_id: &str,
        base_repo_path: PathBuf,
        worktree_path: PathBuf,
    ) -> ManagedWorktreeCreateParams {
        ManagedWorktreeCreateParams {
            worktree_id: Some(worktree_id.to_string()),
            identity: Some(format!("session:{worktree_id}")),
            mode: ManagedWorktreeMode::IsolatedWorktree,
            base_repo_path,
            worktree_path,
            branch: Some(format!("codewith/{worktree_id}")),
            base_sha: Some("base-sha".to_string()),
            head_sha: Some("head-sha".to_string()),
            status_snapshot_json: json!({}),
            dirty: false,
            cleanup_policy: ManagedWorktreeCleanupPolicy::DeleteIfClean,
            owner_kind: ManagedWorktreeOwnerKind::MainSession,
            owner_thread_id: None,
            owner_agent_run_id: None,
            cleanup_after: None,
        }
    }

    #[cfg(windows)]
    async fn stored_worktree_path_key(
        runtime: &StateRuntime,
        worktree_id: &str,
    ) -> anyhow::Result<Option<String>> {
        sqlx::query_scalar("SELECT worktree_path_key FROM managed_worktrees WHERE worktree_id = ?")
            .bind(worktree_id)
            .fetch_one(runtime.pool.as_ref())
            .await
            .map_err(Into::into)
    }

    #[test]
    fn normalizes_ordinary_parent_components() -> anyhow::Result<()> {
        let temp = test_temp_dir()?;
        let parent = temp.join("parent");
        let child = parent.join("child");
        std::fs::create_dir_all(&child)?;

        assert_eq!(
            path_to_db_string(&parent),
            path_to_db_string(&child.join(".."))
        );
        assert_eq!(
            path_to_db_string(&parent.join("missing")),
            path_to_db_string(&child.join("..").join("missing"))
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn resolves_missing_descendants_after_symlinked_ancestors() -> anyhow::Result<()> {
        use std::os::unix::fs::symlink;

        let temp = test_temp_dir()?;
        let physical_parent = temp.join("physical-parent");
        let target = physical_parent.join("target");
        let alias = temp.join("alias");
        std::fs::create_dir_all(&target)?;
        symlink(&target, &alias)?;

        let missing_leaf = alias.join("..").join("missing").join("leaf");
        let expected = std::fs::canonicalize(&physical_parent)?
            .join("missing")
            .join("leaf");

        assert_eq!(
            path_to_db_string(&expected),
            path_to_db_string(&missing_leaf)
        );
        Ok(())
    }

    #[cfg(not(windows))]
    #[test]
    fn non_windows_path_keys_preserve_case() {
        assert_ne!(
            managed_worktree_path_key(Path::new("/managed-worktrees/RunA")),
            managed_worktree_path_key(Path::new("/managed-worktrees/runa"))
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_path_keys_fold_case_for_missing_leaves_drive_unc_and_verbatim_paths() {
        let drive_run_a = Path::new(r"C:\Managed-Worktrees\RunA\missing\leaf");
        let drive_runa = Path::new(r"c:\managed-worktrees\runa\missing\leaf");
        let verbatim_drive_run_a = Path::new(r"\\?\C:\Managed-Worktrees\RunA\missing\leaf");
        assert_eq!(
            managed_worktree_path_key(drive_run_a),
            managed_worktree_path_key(drive_runa)
        );
        assert_eq!(
            r"c:\managed-worktrees\runa\missing\leaf",
            managed_worktree_path_key(drive_run_a)
        );
        assert_eq!(r"c:\", managed_worktree_path_key(Path::new(r"C:\")));
        assert_eq!(
            path_to_db_string(drive_run_a),
            path_to_db_string(verbatim_drive_run_a)
        );
        assert_eq!(
            managed_worktree_path_key(drive_run_a),
            managed_worktree_path_key(verbatim_drive_run_a)
        );

        let unc_run_a = Path::new(r"\\Server\Share\RunA\missing");
        let verbatim_unc_runa = Path::new(r"\\?\UNC\server\share\runa\missing");
        assert_eq!(
            managed_worktree_path_key(unc_run_a),
            managed_worktree_path_key(verbatim_unc_runa)
        );
        assert_eq!(
            r"\\server\share\runa\missing",
            path_to_db_string(verbatim_unc_runa)
        );
        assert_eq!(
            managed_worktree_path_key(Path::new(r"C:\Managed-Worktrees\RÜN\missing")),
            managed_worktree_path_key(Path::new(r"c:\managed-worktrees\rün\missing"))
        );
        assert_eq!(
            r"C:\Managed-Worktrees\RunA\missing\leaf",
            path_to_db_string(drive_run_a)
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_case_aliases_with_missing_leaves_share_an_admission_key() -> anyhow::Result<()>
    {
        let runtime = test_runtime().await;
        let base_repo_path = test_temp_dir()?.join("repo");
        let run_a = base_repo_path
            .join(".codewith")
            .join("worktrees")
            .join("RunA")
            .join("missing");
        let runa = base_repo_path
            .join(".codewith")
            .join("worktrees")
            .join("runa")
            .join("missing");
        let store = runtime.managed_worktrees();

        assert_ne!(path_to_db_string(&run_a), path_to_db_string(&runa));
        assert_eq!(
            managed_worktree_path_key(&run_a),
            managed_worktree_path_key(&runa)
        );
        let admitted = store
            .create_managed_worktree(create_params_for_paths(
                "wt-run-a",
                base_repo_path.clone(),
                run_a.clone(),
            ))
            .await?;
        assert_eq!(
            path_to_db_string(&run_a),
            path_to_string(&admitted.worktree_path)
        );
        assert_eq!(
            Some(managed_worktree_path_key(&run_a)),
            stored_worktree_path_key(runtime.as_ref(), "wt-run-a").await?
        );

        let error = store
            .create_managed_worktree(create_params_for_paths(
                "wt-runa",
                base_repo_path.clone(),
                runa.clone(),
            ))
            .await
            .expect_err("a live Windows case alias must be rejected");
        assert!(
            format!("{error:#}").contains("normalized isolated worktree path is already live"),
            "unexpected admission error: {error:#}"
        );

        store
            .mark_managed_worktree_deleted("wt-run-a")
            .await?
            .expect("worktree should be marked deleted");
        assert_eq!(
            "wt-runa",
            store
                .create_managed_worktree(create_params_for_paths("wt-runa", base_repo_path, runa,))
                .await?
                .worktree_id
        );
        Ok(())
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_case_aliases_block_cleanup_until_the_live_sibling_is_deleted()
    -> anyhow::Result<()> {
        let runtime = test_runtime().await;
        let base_repo_path = test_temp_dir()?.join("repo");
        let run_a = base_repo_path
            .join(".codewith")
            .join("worktrees")
            .join("RunA")
            .join("missing");
        let runa = base_repo_path
            .join(".codewith")
            .join("worktrees")
            .join("runa")
            .join("missing");
        let stale_path = base_repo_path
            .join(".codewith")
            .join("worktrees")
            .join("stale");
        let store = runtime.managed_worktrees();
        store
            .create_managed_worktree(create_params_for_paths(
                "wt-run-a",
                base_repo_path.clone(),
                run_a,
            ))
            .await?;
        store
            .create_managed_worktree(create_params_for_paths(
                "wt-stale",
                base_repo_path,
                stale_path,
            ))
            .await?;
        sqlx::query(
            "UPDATE managed_worktrees SET worktree_path = ?, worktree_path_key = ? WHERE worktree_id = ?",
        )
        .bind(path_to_db_string(&runa))
        .bind(managed_worktree_path_key(&runa))
        .bind("wt-stale")
        .execute(runtime.pool.as_ref())
        .await?;
        let stale = store
            .release_managed_worktree(ManagedWorktreeReleaseParams {
                worktree_id: "wt-stale".to_string(),
                cleanup_policy: ManagedWorktreeCleanupPolicy::DeleteIfClean,
                force_delete: false,
                status_snapshot_json: json!({"dirty": false}),
                dirty: false,
            })
            .await?
            .expect("stale worktree should be released for cleanup");

        assert_eq!(
            Vec::<crate::ManagedWorktree>::new(),
            store
                .list_cleanup_candidates(chrono::Utc::now(), /*limit*/ 10)
                .await?
        );
        assert_eq!(
            None,
            store
                .get_cleanup_candidate_for_execution("wt-stale", chrono::Utc::now())
                .await?
        );

        store
            .mark_managed_worktree_deleted("wt-run-a")
            .await?
            .expect("live sibling should be marked deleted");
        assert_eq!(
            vec![stale.clone()],
            store
                .list_cleanup_candidates(chrono::Utc::now(), /*limit*/ 10)
                .await?
        );
        assert_eq!(
            Some(stale),
            store
                .get_cleanup_candidate_for_execution("wt-stale", chrono::Utc::now())
                .await?
        );
        Ok(())
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn startup_backfill_preserves_display_case_and_rekeys_missing_case_aliases()
    -> anyhow::Result<()> {
        let temp = test_temp_dir()?;
        let codex_home = temp.join("codewith-home");
        let base_repo_path = temp.join("repo");
        let run_a = base_repo_path
            .join(".codewith")
            .join("worktrees")
            .join("RunA")
            .join("missing");
        let runa = base_repo_path
            .join(".codewith")
            .join("worktrees")
            .join("runa")
            .join("missing");
        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string()).await?;
        runtime
            .managed_worktrees()
            .create_managed_worktree(create_params_for_paths(
                "wt-run-a",
                base_repo_path.clone(),
                run_a.clone(),
            ))
            .await?;
        sqlx::query("UPDATE managed_worktrees SET worktree_path_key = ? WHERE worktree_id = ?")
            .bind(path_to_db_string(&run_a))
            .bind("wt-run-a")
            .execute(runtime.pool.as_ref())
            .await?;
        drop(runtime);

        let runtime = StateRuntime::init(codex_home, "test-provider".to_string()).await?;
        let stored = runtime
            .managed_worktrees()
            .get_managed_worktree("wt-run-a")
            .await?
            .expect("legacy worktree should remain readable");
        assert_eq!(
            path_to_db_string(&run_a),
            path_to_string(&stored.worktree_path)
        );
        assert_eq!(
            Some(managed_worktree_path_key(&run_a)),
            stored_worktree_path_key(runtime.as_ref(), "wt-run-a").await?
        );
        let error = runtime
            .managed_worktrees()
            .create_managed_worktree(create_params_for_paths("wt-runa", base_repo_path, runa))
            .await
            .expect_err("startup-rekeyed alias must block a live admission");
        assert!(
            format!("{error:#}").contains("normalized isolated worktree path is already live"),
            "unexpected admission error: {error:#}"
        );
        Ok(())
    }
}
