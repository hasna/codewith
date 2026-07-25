use codex_core_skills::SkillLoadOutcome;
use codex_core_skills::SkillMetadata;

use crate::catalog::SkillAuthority;
use crate::catalog::SkillCatalog;
use crate::catalog::SkillCatalogEntry;
use crate::catalog::SkillPackageId;
use crate::catalog::SkillProviderError;
use crate::catalog::SkillReadResult;
use crate::catalog::SkillResourceId;
use crate::catalog::SkillSearchResult;
use crate::catalog::SkillSourceKind;
use crate::provider::SkillListQuery;
use crate::provider::SkillProvider;
use crate::provider::SkillProviderFuture;
use crate::provider::SkillReadRequest;
use crate::provider::SkillSearchRequest;

const HOST_AUTHORITY_ID: &str = "host";

/// Host-owned skill provider backed by the already-loaded turn skills.
///
/// The provider intentionally does not reload or cache host skills. Core owns
/// skill loading, including plugin roots, runtime extra roots, and the primary
/// environment filesystem. This adapter only maps that loaded outcome into the
/// skills-extension catalog/read contract.
#[derive(Clone, Default)]
pub struct HostSkillProvider;

impl HostSkillProvider {
    pub fn new() -> Self {
        Self
    }
}

impl SkillProvider for HostSkillProvider {
    fn list(&self, query: SkillListQuery) -> SkillProviderFuture<'_, SkillCatalog> {
        Box::pin(async move {
            let Some(host_loaded_skills) = query.host else {
                return Err(SkillProviderError::new(
                    "host skill provider requires loaded host skills",
                ));
            };

            Ok(catalog_from_outcome(host_loaded_skills.outcome()))
        })
    }

    fn read(&self, request: SkillReadRequest) -> SkillProviderFuture<'_, SkillReadResult> {
        Box::pin(async move {
            let Some(host_loaded_skills) = request.host else {
                return Err(SkillProviderError::new(
                    "host skill provider requires loaded host skills",
                ));
            };
            let Some(skill) = host_loaded_skills.outcome().skills.iter().find(|skill| {
                let skill_path = skill.path_to_skills_md.to_string_lossy();
                skill_path == request.resource.0.as_str()
                    || skill_path.replace('\\', "/") == request.resource.0
            }) else {
                return Err(SkillProviderError::new(format!(
                    "host skill resource is not loaded: {}",
                    request.resource.0
                )));
            };

            let contents = host_loaded_skills
                .read_skill_text(skill)
                .await
                .map_err(|err| {
                    SkillProviderError::new(format!(
                        "failed to read host skill resource {}: {err}",
                        request.resource.0
                    ))
                })?;

            Ok(SkillReadResult {
                resource: request.resource,
                contents,
            })
        })
    }

    fn search(&self, _request: SkillSearchRequest) -> SkillProviderFuture<'_, SkillSearchResult> {
        Box::pin(async { Ok(SkillSearchResult::default()) })
    }
}

fn catalog_from_outcome(outcome: &SkillLoadOutcome) -> SkillCatalog {
    let mut catalog = SkillCatalog {
        entries: Vec::new(),
        warnings: outcome
            .errors
            .iter()
            .map(|err| {
                format!(
                    "Failed to load skill at {}: {}",
                    err.path.display(),
                    err.message
                )
            })
            .collect(),
    };

    // This runs on every turn, for the entire host catalog. De-duplicating with
    // `push_entry` would be a linear scan per insert, and because every host
    // entry shares `SkillAuthority(Host, "host")` the authority guard always
    // short-circuits true, so each comparison degrades into a full string
    // compare of long common-prefix filesystem paths. `extend` hashes instead.
    catalog.extend(SkillCatalog {
        entries: outcome
            .skills_with_enabled()
            .map(|(skill, enabled)| catalog_entry_from_skill(skill, enabled))
            .collect(),
        warnings: Vec::new(),
    });

    catalog
}

fn catalog_entry_from_skill(skill: &SkillMetadata, enabled: bool) -> SkillCatalogEntry {
    let skill_path = skill.path_to_skills_md.to_string_lossy().into_owned();
    let display_path = skill_path.replace('\\', "/");
    let mut entry = SkillCatalogEntry::new(
        SkillPackageId(skill_path.clone()),
        SkillAuthority::new(SkillSourceKind::Host, HOST_AUTHORITY_ID),
        skill.name.clone(),
        skill.description.clone(),
        SkillResourceId(skill_path),
    )
    .with_short_description(skill.short_description.clone())
    .with_display_path(display_path)
    .with_dependencies(skill.dependencies.clone());

    if !enabled {
        entry = entry.disabled();
    } else if !skill.allows_implicit_invocation() {
        entry = entry.deferred();
    }

    entry
}

#[cfg(test)]
mod tests {
    use std::time::Duration;
    use std::time::Instant;

    use codex_core_skills::SkillLoadOutcome;
    use codex_protocol::protocol::SkillScope;
    use codex_utils_absolute_path::test_support::PathBufExt;
    use codex_utils_absolute_path::test_support::test_path_buf;
    use pretty_assertions::assert_eq;

    use super::*;

    /// Deep enough that a linear-scan de-duplication (`n^2 / 2` full string
    /// compares of long common-prefix paths) cannot finish inside the budget
    /// below, while the hashed path stays in the tens of milliseconds even in a
    /// debug build.
    const SCALE: usize = 20_000;
    const BUDGET: Duration = Duration::from_secs(20);

    fn scale_skill(index: usize) -> SkillMetadata {
        SkillMetadata {
            name: format!("scale-skill-{index:05}"),
            description: "desc".to_string(),
            short_description: None,
            interface: None,
            dependencies: None,
            policy: None,
            path_to_skills_md: test_path_buf(&format!(
                "/Users/xl/.codewith/plugins/cache/openai-curated/example/hash1234567890/skills-with-a-very-long-shared-prefix/scale-skill-{index:05}/SKILL.md"
            ))
            .abs(),
            scope: SkillScope::User,
            plugin_id: None,
        }
    }

    #[test]
    fn catalog_from_outcome_deduplicates_by_identity() {
        let skill = scale_skill(0);
        let mut outcome = SkillLoadOutcome::default();
        outcome.skills = vec![skill.clone(), skill];

        let catalog = catalog_from_outcome(&outcome);

        assert_eq!(catalog.entries.len(), 1);
    }

    #[test]
    fn catalog_from_outcome_stays_linear_at_catalog_scale() {
        // `catalog_from_outcome` runs on the production per-turn path
        // (`TurnInputContributor::contribute` ->
        // `providers.list_for_turn_with_routes`), so a quadratic de-duplication
        // here costs every turn, not just startup.
        let mut outcome = SkillLoadOutcome::default();
        outcome.skills = (0..SCALE).map(scale_skill).collect();

        let started = Instant::now();
        let catalog = catalog_from_outcome(&outcome);
        let elapsed = started.elapsed();

        assert_eq!(catalog.entries.len(), SCALE);
        assert!(
            elapsed < BUDGET,
            "building a {SCALE}-entry host catalog took {elapsed:?}; expected the hashed \
             de-duplication path (under {BUDGET:?}), not a per-insert linear scan"
        );
    }
}
