use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::path::Component;
use std::path::Path;

use crate::model::SkillLoadOutcome;
use crate::model::SkillMetadata;
use codex_otel::SessionTelemetry;
use codex_otel::THREAD_SKILLS_DEFERRED_TOTAL_METRIC;
use codex_otel::THREAD_SKILLS_DESCRIPTION_TRUNCATED_CHARS_METRIC;
use codex_otel::THREAD_SKILLS_ENABLED_TOTAL_METRIC;
use codex_otel::THREAD_SKILLS_KEPT_TOTAL_METRIC;
use codex_otel::THREAD_SKILLS_TRUNCATED_METRIC;
use codex_protocol::protocol::SkillScope;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_output_truncation::approx_token_count;

const DEFAULT_SKILL_METADATA_CHAR_BUDGET: usize = 8_000;
const SKILL_METADATA_CONTEXT_WINDOW_PERCENT: usize = 2;
/// Upper bound on the starter list *once deferral is actually warranted*.
///
/// This is a ceiling, not a quota: [`build_available_skills`] only falls back to
/// it when the whole prompt-visible catalog cannot be rendered inside the skills
/// context budget *and* the embedder exposes a catalog-search tool the model can
/// use to reach the rest. Small catalogs keep rendering in full.
pub const MAX_STARTER_SKILLS: usize = 5;
const SKILL_DESCRIPTION_TRUNCATION_WARNING_THRESHOLD_CHARS: usize = 100;
const APPROX_BYTES_PER_TOKEN: usize = 4;
pub const SKILL_DESCRIPTION_TRUNCATED_WARNING: &str = "Starter skill descriptions were shortened to fit the skills context budget. The full catalog remains searchable, but some starter descriptions are shorter.";
pub const SKILL_DESCRIPTION_TRUNCATED_WARNING_WITH_PERCENT: &str = "Starter skill descriptions were shortened to fit the 2% skills context budget. The full catalog remains searchable, but some starter descriptions are shorter.";
pub const SKILL_DESCRIPTIONS_REMOVED_WARNING_PREFIX: &str =
    "Exceeded skills context budget. All skill descriptions were removed and";
/// Opening sentence of the `## Skills` intro, shared by every variant.
pub const SKILLS_INTRO_LEAD: &str =
    "A skill is a set of local instructions to follow that is stored in a `SKILL.md` file.";
/// Completeness claim used when some of the catalog was held back.
pub const SKILLS_INTRO_PARTIAL_LIST: &str =
    "The list below is a small starter or task-relevant subset, not the complete skills catalog.";
/// Completeness claim used when every available skill is rendered below.
/// Asserting incompleteness here when the list is in fact complete pushes the
/// model to hunt for skills that do not exist, and to reach for a
/// catalog-search tool that may not even be installed.
pub const SKILLS_INTRO_COMPLETE_LIST: &str =
    "The list below is the complete set of skills available to you.";
pub const SKILLS_INTRO_TRAILER_WITH_ABSOLUTE_PATHS: &str = "Each entry includes a name, description, and file path so you can open the source for full instructions when using a specific skill.";
pub const SKILLS_INTRO_TRAILER_WITH_ALIASES: &str = "Each entry includes a name, description, and a short path that can be expanded into an absolute path using the skill roots table.";

/// Opening of the `- Discovery:` bullet, shared by every variant.
///
/// Completeness and catalog-search availability are two independent axes -
/// a list can be partial with no way to search past it - so the bullet is
/// composed from parts rather than picked from a matrix of whole sentences.
pub const SKILLS_DISCOVERY_LEAD: &str =
    "- Discovery: Always consider whether a skill is relevant before acting.";
pub const SKILLS_DISCOVERY_PARTIAL_LIST: &str =
    "The list above is only a starter or task-relevant subset, not the complete catalog.";
pub const SKILLS_DISCOVERY_COMPLETE_LIST: &str = "The list above is every available skill.";
pub const SKILLS_DISCOVERY_PATHS_WITH_ABSOLUTE_PATHS: &str = "Each entry is a name, description, and file path, and skill bodies live on disk at those paths.";
pub const SKILLS_DISCOVERY_PATHS_WITH_ALIASES: &str = "Each entry is a name, description, and short path; skill bodies live on disk at those paths after expanding the matching alias from `### Skill roots`.";
/// Appended only when a catalog-search tool is installed *and* there is
/// something outside the rendered list for it to find.
pub const SKILLS_DISCOVERY_CATALOG_SEARCH: &str = "Use `skills.list` to search the full catalog by task, or call it with no `query` to enumerate every skill, then use `skills.read` with the returned opaque handles when needed.";
/// `- Missing/blocked:` bullet, split out because it also names `skills.list`.
pub const SKILLS_MISSING_WITH_CATALOG_SEARCH: &str = "- Missing/blocked: If a named skill isn't listed, search for it with `skills.list`. If it cannot be found or read, say so briefly and continue with the best fallback.";
pub const SKILLS_MISSING_WITHOUT_CATALOG_SEARCH: &str = "- Missing/blocked: If a named skill isn't listed, it is not available. Say so briefly and continue with the best fallback.";

/// `- Trigger rules:` bullet, shared by every variant.
pub const SKILLS_TRIGGER_RULES: &str = "- Trigger rules: Explicit skill mentions win. If the user names a skill (with `$SkillName` or plain text) OR the task clearly matches a listed or discovered skill's description, you must use that skill for that turn. Multiple mentions mean use them all. Do not carry skills across turns unless re-mentioned.";

/// Everything in `### How to use skills` after the completeness-dependent
/// bullets, for a list rendered with absolute paths.
pub const SKILLS_HOW_TO_USE_TAIL_WITH_ABSOLUTE_PATHS: &str = r###"- How to use a skill (progressive disclosure):
  1) After deciding to use a skill, open its `SKILL.md`. Read only enough to follow the workflow.
  2) When `SKILL.md` references relative paths (e.g., `scripts/foo.py`), resolve them relative to the skill directory listed above first, and only consider other paths if needed.
  3) If `SKILL.md` points to extra folders such as `references/`, load only the specific files needed for the request; don't bulk-load everything.
  4) If `scripts/` exist, prefer running or patching them instead of retyping large code blocks.
  5) If `assets/` or templates exist, reuse them instead of recreating from scratch.
- Coordination and sequencing:
  - If multiple skills apply, choose the minimal set that covers the request and state the order you'll use them.
  - Announce which skill(s) you're using and why (one short line). If you skip an obvious skill, say why.
- Context hygiene:
  - Keep context small: summarize long sections instead of pasting them; only load extra files when needed.
  - Avoid deep reference-chasing: prefer opening only files directly linked from `SKILL.md` unless you're blocked.
  - When variants exist (frameworks, providers, domains), pick only the relevant reference file(s) and note that choice.
- Safety and fallback: If a skill can't be applied cleanly (missing files, unclear instructions), state the issue, pick the next-best approach, and continue."###;
/// Everything in `### How to use skills` after the completeness-dependent
/// bullets, for a list rendered with `### Skill roots` aliases.
pub const SKILLS_HOW_TO_USE_TAIL_WITH_ALIASES: &str = r###"- How to use a skill (progressive disclosure):
  1) After deciding to use a skill, expand the listed short `path` with the matching alias from `### Skill roots`, then open its `SKILL.md`. Read only enough to follow the workflow.
  2) When `SKILL.md` references relative paths (e.g., `scripts/foo.py`), resolve them relative to the directory containing that expanded `SKILL.md` first, and only consider other paths if needed.
  3) If `SKILL.md` points to extra folders such as `references/`, load only the specific files needed for the request; don't bulk-load everything.
  4) If `scripts/` exist, prefer running or patching them instead of retyping large code blocks.
  5) If `assets/` or templates exist, reuse them instead of recreating from scratch.
- Coordination and sequencing:
  - If multiple skills apply, choose the minimal set that covers the request and state the order you'll use them.
  - Announce which skill(s) you're using and why (one short line). If you skip an obvious skill, say why.
- Context hygiene:
  - Keep context small: summarize long sections instead of pasting them; only load extra files when needed.
  - Avoid deep reference-chasing: prefer opening only files directly linked from `SKILL.md` unless you're blocked.
  - When variants exist (frameworks, providers, domains), pick only the relevant reference file(s) and note that choice.
- Safety and fallback: If a skill can't be applied cleanly (missing files, unclear instructions), state the issue, pick the next-best approach, and continue."###;

/// Namespace of the extension-provided skill tools (`skills.list`,
/// `skills.read`, ...). Shared so that core can tell whether the catalog-search
/// escape hatch referenced by the prompt text above actually exists for a
/// thread before it defers any skill.
pub const SKILLS_TOOL_NAMESPACE: &str = "skills";
/// Name of the catalog-search tool inside [`SKILLS_TOOL_NAMESPACE`].
pub const SKILLS_LIST_TOOL_NAME: &str = "list";

pub const TASK_RELEVANT_SKILLS_HEADING: &str = "## Task-relevant skills";
pub const TASK_RELEVANT_SKILLS_INTRO: &str = "Additional catalog matches for this turn's request. The discovery, trigger, progressive-disclosure, and context-hygiene rules from the `## Skills` section above apply unchanged and are deliberately not repeated here.";
/// Intro used when there is no `## Skills` section to defer to, so this
/// fragment has to carry the rules itself.
pub const TASK_RELEVANT_SKILLS_STANDALONE_INTRO: &str = "Skills matching this turn's request. A skill is a set of instructions to follow, stored in a `SKILL.md` file. No separate `## Skills` section was emitted for this request, so the rules for using these skills are below.";

/// Whether the `## Skills` developer block — and with it the shared
/// `### How to use skills` preamble — is present in the same request as the
/// per-turn task-relevant fragment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillsPreamble {
    /// The `## Skills` block is present and already carries the rules.
    RenderedElsewhere,
    /// No `## Skills` block was emitted. This happens whenever the *host* skill
    /// outcome is empty but the merged catalog is not, i.e. every skill comes
    /// from a remote or executor provider. Pointing at a section that does not
    /// exist would leave the model with bare skill lines and no usage rules.
    Missing,
}

/// Render the per-turn, ranked skill matches contributed by the skills
/// extension.
///
/// With [`SkillsPreamble::RenderedElsewhere`] this intentionally omits the
/// `### How to use skills` preamble that [`render_available_skills_body`]
/// emits: on any turn whose text matches a skill, both blocks land in the same
/// request, and repeating ~2.5k characters of identical how-to-use guidance
/// defeats the point of deferring the catalog in the first place.
pub fn render_task_relevant_skills_body(
    skill_lines: &[String],
    preamble: SkillsPreamble,
) -> String {
    let mut lines: Vec<String> = Vec::with_capacity(skill_lines.len().saturating_add(6));
    lines.push(TASK_RELEVANT_SKILLS_HEADING.to_string());
    match preamble {
        SkillsPreamble::RenderedElsewhere => lines.push(TASK_RELEVANT_SKILLS_INTRO.to_string()),
        SkillsPreamble::Missing => lines.push(TASK_RELEVANT_SKILLS_STANDALONE_INTRO.to_string()),
    }
    lines.push("### Available skills".to_string());
    lines.extend(skill_lines.iter().cloned());
    if preamble == SkillsPreamble::Missing {
        lines.push("### How to use skills".to_string());
        // These lines come from the merged catalog, so `skills.list` is
        // installed by construction and the list is a ranked subset.
        lines.push(render_discovery_bullet(
            SkillsListCoverage {
                complete: false,
                catalog_search: SkillCatalogSearch::Available,
            },
            /*aliased*/ false,
        ));
        lines.push(SKILLS_TRIGGER_RULES.to_string());
        lines.push(SKILLS_MISSING_WITH_CATALOG_SEARCH.to_string());
        lines.push(SKILLS_HOW_TO_USE_TAIL_WITH_ABSOLUTE_PATHS.to_string());
    }

    format!("\n{}\n", lines.join("\n"))
}

/// How much of the catalog the rendered `## Skills` list actually covers, and
/// whether the model has a tool to reach whatever is missing.
///
/// The prompt used to assert incompleteness unconditionally, which is a lie
/// whenever the whole catalog fits — and it pointed at `skills.list` even on
/// threads built without the skills extension, where that tool does not exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkillsListCoverage {
    /// Every prompt-visible skill is rendered in the list.
    pub complete: bool,
    /// Whether the catalog-search tool is installed for this thread.
    pub catalog_search: SkillCatalogSearch,
}

impl SkillsListCoverage {
    /// The list is complete *and* nothing can search beyond it: the safest
    /// default for embedders that render a body without a render report.
    pub const COMPLETE_WITHOUT_SEARCH: Self = Self {
        complete: true,
        catalog_search: SkillCatalogSearch::Unavailable,
    };

    fn from_report(report: &SkillRenderReport, catalog_search: SkillCatalogSearch) -> Self {
        Self {
            complete: report.deferred_count == 0 && report.omitted_count == 0,
            catalog_search,
        }
    }

    /// Only claim the list is partial when the model can do something about it.
    fn advertise_catalog_search(self) -> bool {
        !self.complete && self.catalog_search == SkillCatalogSearch::Available
    }
}

fn render_discovery_bullet(coverage: SkillsListCoverage, aliased: bool) -> String {
    let completeness = if coverage.complete {
        SKILLS_DISCOVERY_COMPLETE_LIST
    } else {
        SKILLS_DISCOVERY_PARTIAL_LIST
    };
    let paths = if aliased {
        SKILLS_DISCOVERY_PATHS_WITH_ALIASES
    } else {
        SKILLS_DISCOVERY_PATHS_WITH_ABSOLUTE_PATHS
    };
    let mut bullet = format!("{SKILLS_DISCOVERY_LEAD} {completeness} {paths}");
    if coverage.advertise_catalog_search() {
        bullet.push(' ');
        bullet.push_str(SKILLS_DISCOVERY_CATALOG_SEARCH);
    }
    bullet
}

pub fn render_available_skills_body(
    skill_root_lines: &[String],
    skill_lines: &[String],
    coverage: SkillsListCoverage,
) -> String {
    let aliased = !skill_root_lines.is_empty();
    let mut lines: Vec<String> = Vec::new();
    lines.push("## Skills".to_string());
    let completeness = if coverage.complete {
        SKILLS_INTRO_COMPLETE_LIST
    } else {
        SKILLS_INTRO_PARTIAL_LIST
    };
    let trailer = if aliased {
        SKILLS_INTRO_TRAILER_WITH_ALIASES
    } else {
        SKILLS_INTRO_TRAILER_WITH_ABSOLUTE_PATHS
    };
    lines.push(format!("{SKILLS_INTRO_LEAD} {completeness} {trailer}"));
    if aliased {
        lines.push("### Skill roots".to_string());
        lines.extend(skill_root_lines.iter().cloned());
    }
    lines.push("### Available skills".to_string());
    lines.extend(skill_lines.iter().cloned());

    lines.push("### How to use skills".to_string());
    lines.push(render_discovery_bullet(coverage, aliased));
    lines.push(SKILLS_TRIGGER_RULES.to_string());
    lines.push(
        if coverage.catalog_search == SkillCatalogSearch::Available {
            SKILLS_MISSING_WITH_CATALOG_SEARCH
        } else {
            SKILLS_MISSING_WITHOUT_CATALOG_SEARCH
        }
        .to_string(),
    );
    lines.push(
        if aliased {
            SKILLS_HOW_TO_USE_TAIL_WITH_ALIASES
        } else {
            SKILLS_HOW_TO_USE_TAIL_WITH_ABSOLUTE_PATHS
        }
        .to_string(),
    );

    format!("\n{}\n", lines.join("\n"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillMetadataBudget {
    Tokens(usize),
    Characters(usize),
}

impl SkillMetadataBudget {
    fn limit(self) -> usize {
        match self {
            Self::Tokens(limit) | Self::Characters(limit) => limit,
        }
    }

    fn cost(self, text: &str) -> usize {
        match self {
            Self::Tokens(_) => approx_token_count(text),
            Self::Characters(_) => text.chars().count(),
        }
    }

    fn cost_from_counts(self, chars: usize, bytes: usize) -> usize {
        match self {
            Self::Tokens(_) => approx_token_count_from_bytes(bytes),
            Self::Characters(_) => chars,
        }
    }
}

fn approx_token_count_from_bytes(bytes: usize) -> usize {
    bytes.saturating_add(APPROX_BYTES_PER_TOKEN.saturating_sub(1)) / APPROX_BYTES_PER_TOKEN
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillRenderReport {
    pub total_count: usize,
    pub included_count: usize,
    pub deferred_count: usize,
    pub omitted_count: usize,
    pub truncated_description_chars: usize,
    pub truncated_description_count: usize,
}

#[derive(Clone, Copy)]
pub enum SkillRenderSideEffects<'a> {
    None,
    ThreadStart {
        session_telemetry: &'a SessionTelemetry,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailableSkills {
    pub skill_root_lines: Vec<String>,
    pub skill_lines: Vec<String>,
    pub report: SkillRenderReport,
    /// What the rendered list actually covers, so the intro can stop asserting
    /// incompleteness on lists that are complete.
    pub coverage: SkillsListCoverage,
    pub warning_message: Option<String>,
}

/// Whether the embedder exposes a catalog-search tool (`skills.list`) that the
/// model can call to reach skills held back from the rendered starter list.
///
/// The starter cap and the search tool are two halves of one contract: hiding
/// part of the catalog is only acceptable when the model has a way to find the
/// remainder. Embedders that build a thread with `empty_extension_registry()`
/// (or otherwise skip `codex_skills_extension::install`) pass
/// [`SkillCatalogSearch::Unavailable`] and always get the complete list, capped
/// only by the context budget itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillCatalogSearch {
    /// A catalog-search tool is installed for this thread.
    Available,
    /// No catalog-search tool is installed; nothing may be deferred.
    Unavailable,
}

impl SkillCatalogSearch {
    pub fn from_tool_available(tool_available: bool) -> Self {
        if tool_available {
            Self::Available
        } else {
            Self::Unavailable
        }
    }

    fn allows_deferral(self) -> bool {
        matches!(self, Self::Available)
    }
}

pub fn default_skill_metadata_budget(context_window: Option<i64>) -> SkillMetadataBudget {
    context_window
        .and_then(|window| usize::try_from(window).ok())
        .filter(|window| *window > 0)
        .map(|window| {
            SkillMetadataBudget::Tokens(
                window
                    .saturating_mul(SKILL_METADATA_CONTEXT_WINDOW_PERCENT)
                    .saturating_div(100)
                    .max(1),
            )
        })
        .unwrap_or(SkillMetadataBudget::Characters(
            DEFAULT_SKILL_METADATA_CHAR_BUDGET,
        ))
}

pub fn build_available_skills(
    outcome: &SkillLoadOutcome,
    budget: SkillMetadataBudget,
    catalog_search: SkillCatalogSearch,
    side_effects: SkillRenderSideEffects<'_>,
) -> Option<AvailableSkills> {
    build_available_skills_with_starter_cap(
        outcome,
        budget,
        catalog_search,
        MAX_STARTER_SKILLS,
        side_effects,
    )
}

/// [`build_available_skills`] with an explicit starter ceiling, for embedders
/// and tests that need to tune how aggressively the catalog is deferred.
pub fn build_available_skills_with_starter_cap(
    outcome: &SkillLoadOutcome,
    budget: SkillMetadataBudget,
    catalog_search: SkillCatalogSearch,
    max_starter_skills: usize,
    side_effects: SkillRenderSideEffects<'_>,
) -> Option<AvailableSkills> {
    let all_skills = outcome.allowed_skills_for_implicit_invocation();
    let total_count = all_skills.len();
    let starter_limit = starter_limit_for(
        outcome,
        &all_skills,
        budget,
        catalog_search,
        max_starter_skills,
    );
    let skills = starter_skills(&all_skills, starter_limit)
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    if skills.is_empty() {
        record_skill_render_side_effects(
            side_effects,
            /*total_count*/ 0,
            /*included_count*/ 0,
            /*deferred_count*/ 0,
            /*omitted_count*/ 0,
            /*truncated_description_chars*/ 0,
        );
        return None;
    }

    let absolute_lines = ordered_absolute_skill_lines(&skills);
    let absolute = build_available_skills_from_lines(
        absolute_lines,
        skills.len(),
        budget,
        SkillPathAliases::default(),
    )?;

    let mut selected =
        if absolute.report.omitted_count == 0 && absolute.report.truncated_description_chars == 0 {
            absolute
        } else if let Some(aliased) = build_aliased_available_skills(outcome, &skills, budget) {
            if aliased_render_is_better(&aliased, &absolute, budget) {
                aliased
            } else {
                absolute
            }
        } else {
            absolute
        };

    selected.report.total_count = total_count;
    selected.report.deferred_count = total_count.saturating_sub(skills.len());
    // Deferral hides whole skills, which is strictly more surprising than a
    // shortened description, so it must never be silent. `warning_message` is
    // built before the starter cap is known, so fold it in here.
    if selected.report.deferred_count > 0 {
        selected.warning_message = Some(deferred_skills_warning(
            &selected.report,
            selected.warning_message.as_deref(),
        ));
    }
    selected.coverage = SkillsListCoverage::from_report(&selected.report, catalog_search);
    record_available_skills_side_effects(&selected, budget, side_effects);
    Some(selected)
}

/// Append the deferral notice to whatever the budget renderer already had to
/// say.
///
/// Deliberately appended rather than prepended: the budget warning keeps its
/// established opening, and a reader who only skims the first sentence still
/// learns that skills are missing.
fn deferred_skills_warning(report: &SkillRenderReport, existing: Option<&str>) -> String {
    let skill_word = if report.deferred_count == 1 {
        "skill"
    } else {
        "skills"
    };
    let deferral = format!(
        "Showing {} of {} skills in the model-visible list; the other {} {} stay searchable through `skills.list`.",
        report.included_count, report.total_count, report.deferred_count, skill_word
    );
    match existing {
        Some(existing) => format!("{existing} {deferral}"),
        None => deferral,
    }
}

fn build_available_skills_from_lines(
    skill_lines: Vec<SkillLine<'_>>,
    total_count: usize,
    budget: SkillMetadataBudget,
    path_aliases: SkillPathAliases,
) -> Option<AvailableSkills> {
    if total_count == 0 {
        return None;
    }

    let (skill_lines, report) = render_skill_lines_from_lines(skill_lines, total_count, budget);
    let warning_message = if report.omitted_count > 0 {
        let skill_word = if report.omitted_count == 1 {
            "skill"
        } else {
            "skills"
        };
        let verb = if report.omitted_count == 1 {
            "was"
        } else {
            "were"
        };
        Some(format!(
            "{} {} additional {} {} not included in the model-visible skills list.",
            budget_warning_prefix(budget, SKILL_DESCRIPTIONS_REMOVED_WARNING_PREFIX),
            report.omitted_count,
            skill_word,
            verb
        ))
    } else if report.average_truncated_description_chars()
        > SKILL_DESCRIPTION_TRUNCATION_WARNING_THRESHOLD_CHARS
    {
        Some(
            match budget {
                SkillMetadataBudget::Tokens(_) => SKILL_DESCRIPTION_TRUNCATED_WARNING_WITH_PERCENT,
                SkillMetadataBudget::Characters(_) => SKILL_DESCRIPTION_TRUNCATED_WARNING,
            }
            .to_string(),
        )
    } else {
        None
    };
    let coverage = SkillsListCoverage {
        complete: report.omitted_count == 0,
        catalog_search: SkillCatalogSearch::Unavailable,
    };
    let available = AvailableSkills {
        skill_root_lines: path_aliases.skill_root_lines,
        skill_lines,
        report,
        coverage,
        warning_message,
    };
    Some(available)
}

fn record_available_skills_side_effects(
    available: &AvailableSkills,
    budget: SkillMetadataBudget,
    side_effects: SkillRenderSideEffects<'_>,
) {
    record_skill_render_side_effects(
        side_effects,
        available.report.total_count,
        available.report.included_count,
        available.report.deferred_count,
        available.report.omitted_count,
        available.report.truncated_description_chars,
    );
    if available.report.deferred_count > 0
        || available.report.omitted_count > 0
        || available.report.truncated_description_chars > 0
    {
        tracing::info!(
            budget_limit = budget.limit(),
            total_skills = available.report.total_count,
            included_skills = available.report.included_count,
            deferred_skills = available.report.deferred_count,
            omitted_skills = available.report.omitted_count,
            truncated_description_chars_per_skill =
                available.report.average_truncated_description_chars(),
            truncated_skill_descriptions = available.report.truncated_description_count,
            "withheld skill metadata to fit skills context budget"
        );
    }
}

fn budget_warning_prefix(budget: SkillMetadataBudget, prefix: &str) -> String {
    match budget {
        SkillMetadataBudget::Tokens(_) => prefix.replacen(
            "Exceeded skills context budget.",
            "Exceeded skills context budget of 2%.",
            1,
        ),
        SkillMetadataBudget::Characters(_) => prefix.to_string(),
    }
}

fn record_skill_render_side_effects(
    side_effects: SkillRenderSideEffects<'_>,
    total_count: usize,
    included_count: usize,
    deferred_count: usize,
    omitted_count: usize,
    truncated_description_chars: usize,
) {
    match side_effects {
        SkillRenderSideEffects::None => {}
        SkillRenderSideEffects::ThreadStart { session_telemetry } => {
            session_telemetry.histogram(
                THREAD_SKILLS_ENABLED_TOTAL_METRIC,
                i64::try_from(total_count).unwrap_or(i64::MAX),
                &[],
            );
            session_telemetry.histogram(
                THREAD_SKILLS_KEPT_TOTAL_METRIC,
                i64::try_from(included_count).unwrap_or(i64::MAX),
                &[],
            );
            // Skills held back for `skills.list` discovery are reported
            // separately from budget-driven omissions, and both count as
            // "the model did not see the whole catalog" so that a 2105 -> 5
            // drop cannot show up alongside `truncated = 0`.
            session_telemetry.histogram(
                THREAD_SKILLS_DEFERRED_TOTAL_METRIC,
                i64::try_from(deferred_count).unwrap_or(i64::MAX),
                &[],
            );
            session_telemetry.histogram(
                THREAD_SKILLS_TRUNCATED_METRIC,
                i64::from(omitted_count > 0 || deferred_count > 0),
                &[],
            );
            session_telemetry.histogram(
                THREAD_SKILLS_DESCRIPTION_TRUNCATED_CHARS_METRIC,
                i64::try_from(truncated_description_chars).unwrap_or(i64::MAX),
                &[],
            );
        }
    }
}

fn render_skill_lines_from_lines(
    skill_lines: Vec<SkillLine<'_>>,
    total_count: usize,
    budget: SkillMetadataBudget,
) -> (Vec<String>, SkillRenderReport) {
    let full_cost = skill_lines.iter().fold(0usize, |used, line| {
        used.saturating_add(line.full_cost(budget))
    });
    if full_cost <= budget.limit() {
        let included = skill_lines
            .iter()
            .map(SkillLine::render_full)
            .collect::<Vec<_>>();

        return (
            included,
            skill_render_report(
                total_count,
                /*included_count*/ skill_lines.len(),
                /*omitted_count*/ 0,
                /*truncated_description_chars*/ 0,
                /*truncated_description_count*/ 0,
            ),
        );
    }

    let minimum_cost = skill_lines.iter().fold(0usize, |used, line| {
        used.saturating_add(line.minimum_cost(budget))
    });
    if minimum_cost <= budget.limit() {
        let rendered = render_lines_with_description_budget(
            budget,
            &skill_lines,
            budget.limit().saturating_sub(minimum_cost),
        );
        let (truncated_description_chars, truncated_description_count) =
            sum_description_truncation(&rendered);
        let included = rendered
            .into_iter()
            .map(|rendered| rendered.line)
            .collect::<Vec<_>>();

        return (
            included,
            skill_render_report(
                total_count,
                /*included_count*/ skill_lines.len(),
                /*omitted_count*/ 0,
                truncated_description_chars,
                truncated_description_count,
            ),
        );
    }

    render_minimum_skill_lines_until_budget(budget, skill_lines, total_count)
}

fn render_minimum_skill_lines_until_budget(
    budget: SkillMetadataBudget,
    skill_lines: Vec<SkillLine<'_>>,
    total_count: usize,
) -> (Vec<String>, SkillRenderReport) {
    let mut included = Vec::new();
    let mut used = 0usize;
    let mut omitted_count = 0usize;
    let mut truncated_description_chars = 0usize;
    let mut truncated_description_count = 0usize;
    for line in skill_lines {
        let line_cost = line.minimum_cost(budget);
        let description_char_count = line.description_char_count();
        if used.saturating_add(line_cost) <= budget.limit() {
            used = used.saturating_add(line_cost);
            included.push(line.render_minimum());
        } else {
            omitted_count = omitted_count.saturating_add(1);
        }

        truncated_description_chars =
            truncated_description_chars.saturating_add(description_char_count);
        if description_char_count > 0 {
            truncated_description_count = truncated_description_count.saturating_add(1);
        }
    }

    let report = skill_render_report(
        total_count,
        included.len(),
        omitted_count,
        truncated_description_chars,
        truncated_description_count,
    );

    (included, report)
}

fn skill_render_report(
    total_count: usize,
    included_count: usize,
    omitted_count: usize,
    truncated_description_chars: usize,
    truncated_description_count: usize,
) -> SkillRenderReport {
    SkillRenderReport {
        total_count,
        included_count,
        deferred_count: 0,
        omitted_count,
        truncated_description_chars,
        truncated_description_count,
    }
}

impl SkillRenderReport {
    /// Average characters of description dropped per skill that was actually
    /// put through the renderer.
    ///
    /// Deliberately *not* divided by `total_count`: that field is rewritten to
    /// the full catalog size once the starter cap is known, which would dilute
    /// "100 chars trimmed from each of 5 starter skills" into 500/2105 = 0 and
    /// silently disarm both the truncation warning and the metric.
    fn average_truncated_description_chars(&self) -> usize {
        let rendered_count = self.included_count.saturating_add(self.omitted_count);
        if rendered_count == 0 || self.truncated_description_chars == 0 {
            return 0;
        }

        self.truncated_description_chars
            .saturating_add(rendered_count.saturating_sub(1))
            / rendered_count
    }
}

struct SkillLine<'a> {
    name: &'a str,
    description: &'a str,
    path: String,
}

struct RenderedSkillLine {
    line: String,
    truncated_chars: usize,
}

struct DescriptionBudgetLine<'a> {
    line: &'a SkillLine<'a>,
    description_char_count: usize,
    extra_costs: Vec<usize>,
}

fn sum_description_truncation(rendered: &[RenderedSkillLine]) -> (usize, usize) {
    rendered
        .iter()
        .fold((0usize, 0usize), |(chars, count), line| {
            if line.truncated_chars == 0 {
                (chars, count)
            } else {
                (
                    chars.saturating_add(line.truncated_chars),
                    count.saturating_add(1),
                )
            }
        })
}

impl<'a> SkillLine<'a> {
    fn new(skill: &'a SkillMetadata) -> Self {
        Self::with_path(
            skill,
            skill.path_to_skills_md.to_string_lossy().replace('\\', "/"),
        )
    }

    fn with_path(skill: &'a SkillMetadata, path: String) -> Self {
        Self {
            name: skill.name.as_str(),
            description: skill.description.as_str(),
            path,
        }
    }

    fn full_cost(&self, budget: SkillMetadataBudget) -> usize {
        line_cost(budget, &self.render_full())
    }

    fn minimum_cost(&self, budget: SkillMetadataBudget) -> usize {
        line_cost(budget, &self.render_minimum())
    }

    fn description_char_count(&self) -> usize {
        self.description.chars().count()
    }

    fn render_full(&self) -> String {
        self.render_with_description(self.description)
    }

    fn render_minimum(&self) -> String {
        self.render_with_description("")
    }

    fn rendered_description_prefix_len(&self, description_chars: usize) -> usize {
        self.description
            .char_indices()
            .nth(description_chars)
            .map_or(self.description.len(), |(idx, _)| idx)
    }

    fn render_with_description_chars(&self, description_chars: usize) -> String {
        if description_chars == 0 {
            format!("- {}: (file: {})", self.name, self.path)
        } else {
            let end = self.rendered_description_prefix_len(description_chars);
            let description = &self.description[..end];
            format!("- {}: {} (file: {})", self.name, description, self.path)
        }
    }

    fn render_with_description(&self, description: &str) -> String {
        if description.is_empty() {
            format!("- {}: (file: {})", self.name, self.path)
        } else {
            format!("- {}: {} (file: {})", self.name, description, self.path)
        }
    }
}

impl<'a> DescriptionBudgetLine<'a> {
    fn new(line: &'a SkillLine<'a>, budget: SkillMetadataBudget) -> Self {
        let minimum_line = line.render_minimum();
        let minimum_chars = minimum_line.chars().count().saturating_add(1);
        let minimum_bytes = minimum_line.len().saturating_add(1);
        let minimum_cost = budget.cost_from_counts(minimum_chars, minimum_bytes);

        let description_char_count = line.description_char_count();
        let mut extra_costs = Vec::with_capacity(description_char_count.saturating_add(1));
        extra_costs.push(0);

        let mut prefix_chars = 0usize;
        let mut prefix_bytes = 0usize;
        for ch in line.description.chars() {
            prefix_chars = prefix_chars.saturating_add(1);
            prefix_bytes = prefix_bytes.saturating_add(ch.len_utf8());
            let rendered_chars = minimum_chars.saturating_add(prefix_chars).saturating_add(1);
            let rendered_bytes = minimum_bytes.saturating_add(prefix_bytes).saturating_add(1);
            let cost = budget
                .cost_from_counts(rendered_chars, rendered_bytes)
                .saturating_sub(minimum_cost);
            extra_costs.push(cost);
        }

        Self {
            line,
            description_char_count,
            extra_costs,
        }
    }
}

fn line_cost(budget: SkillMetadataBudget, line: &str) -> usize {
    budget.cost(&format!("{line}\n"))
}

fn lines_cost(budget: SkillMetadataBudget, lines: &[String]) -> usize {
    lines.iter().fold(0usize, |used, line| {
        used.saturating_add(line_cost(budget, line))
    })
}

fn render_lines_with_description_budget(
    budget: SkillMetadataBudget,
    skill_lines: &[SkillLine<'_>],
    limit: usize,
) -> Vec<RenderedSkillLine> {
    let budget_lines = skill_lines
        .iter()
        .map(|line| DescriptionBudgetLine::new(line, budget))
        .collect::<Vec<_>>();
    let mut char_allocations = vec![0usize; budget_lines.len()];
    let mut current_extra_costs = vec![0usize; budget_lines.len()];
    let mut remaining = limit;

    // Distribute description space one character at a time across skills.
    // Short descriptions naturally drop out, so their unused share can go to
    // longer descriptions instead of being stranded in a fixed per-skill quota.
    loop {
        let mut changed = false;
        for (index, line) in budget_lines.iter().enumerate() {
            if char_allocations[index] >= line.description_char_count {
                continue;
            }

            let current_cost = current_extra_costs[index];
            let next_chars = char_allocations[index].saturating_add(1);
            let next_cost = line.extra_costs[next_chars];
            let delta = next_cost.saturating_sub(current_cost);
            if delta <= remaining {
                char_allocations[index] = next_chars;
                current_extra_costs[index] = next_cost;
                remaining = remaining.saturating_sub(delta);
                changed = true;
            }
        }

        if !changed {
            break;
        }
    }

    budget_lines
        .iter()
        .zip(char_allocations)
        .map(|(line, description_chars)| {
            let truncated_chars = line
                .description_char_count
                .saturating_sub(description_chars);
            RenderedSkillLine {
                line: line.line.render_with_description_chars(description_chars),
                truncated_chars,
            }
        })
        .collect()
}

fn build_aliased_available_skills(
    outcome: &SkillLoadOutcome,
    skills: &[SkillMetadata],
    budget: SkillMetadataBudget,
) -> Option<AvailableSkills> {
    let plan = build_alias_plan(outcome, skills, budget)?;
    if plan.table_cost >= budget.limit() {
        return None;
    }

    let adjusted_limit = budget.limit().saturating_sub(plan.table_cost);
    let adjusted_budget = match budget {
        SkillMetadataBudget::Tokens(_) => SkillMetadataBudget::Tokens(adjusted_limit),
        SkillMetadataBudget::Characters(_) => SkillMetadataBudget::Characters(adjusted_limit),
    };
    let ordered_skills = ordered_skills_for_budget(skills);
    let skill_lines = ordered_skills
        .into_iter()
        .map(|skill| SkillLine::with_path(skill, render_skill_path_with_aliases(skill, &plan)))
        .collect::<Vec<_>>();
    build_available_skills_from_lines(skill_lines, skills.len(), adjusted_budget, plan.aliases)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct SkillPathAliases {
    skill_root_lines: Vec<String>,
}

struct AliasPlan {
    aliases: SkillPathAliases,
    root_aliases: HashMap<AbsolutePathBuf, String>,
    alias_root_by_path: HashMap<AbsolutePathBuf, AbsolutePathBuf>,
    table_cost: usize,
}

fn build_alias_plan(
    outcome: &SkillLoadOutcome,
    skills: &[SkillMetadata],
    budget: SkillMetadataBudget,
) -> Option<AliasPlan> {
    let skill_paths = skills
        .iter()
        .map(|skill| skill.path_to_skills_md.clone())
        .collect::<HashSet<_>>();
    let skill_root_by_path = outcome
        .skill_root_by_path
        .iter()
        .filter(|(path, _)| skill_paths.contains(*path))
        .map(|(path, root)| (path.clone(), root.clone()))
        .collect::<HashMap<_, _>>();
    let used_roots = outcome
        .skill_roots
        .iter()
        .filter(|root| {
            skill_root_by_path
                .values()
                .any(|skill_root| skill_root == *root)
        })
        .cloned()
        .collect::<Vec<_>>();
    if used_roots.is_empty() {
        return None;
    }

    let plugin_version_skill_counts =
        plugin_version_skill_counts_for_skill_roots(skill_root_by_path.values());
    let alias_root_by_skill_root = used_roots
        .iter()
        .map(|root| {
            (
                root.clone(),
                alias_root_for_skill_root(root, &plugin_version_skill_counts),
            )
        })
        .collect::<HashMap<_, _>>();
    let alias_roots = ordered_alias_roots(&used_roots, &alias_root_by_skill_root)?;
    let root_aliases = alias_roots
        .iter()
        .enumerate()
        .map(|(index, alias_root)| (alias_root.clone(), format!("r{index}")))
        .collect::<HashMap<_, _>>();
    let alias_root_by_path = skill_root_by_path
        .iter()
        .filter_map(|(path, skill_root)| {
            alias_root_by_skill_root
                .get(skill_root)
                .map(|alias_root| (path.clone(), alias_root.clone()))
        })
        .collect::<HashMap<_, _>>();
    let skill_root_lines = build_skill_root_lines(&alias_roots);
    let table_cost = aliased_metadata_overhead_cost(budget, &skill_root_lines);

    Some(AliasPlan {
        aliases: SkillPathAliases { skill_root_lines },
        root_aliases,
        alias_root_by_path,
        table_cost,
    })
}

fn ordered_alias_roots(
    used_roots: &[AbsolutePathBuf],
    alias_root_by_skill_root: &HashMap<AbsolutePathBuf, AbsolutePathBuf>,
) -> Option<Vec<AbsolutePathBuf>> {
    let mut seen = HashSet::new();
    let mut alias_roots = Vec::new();
    for root in used_roots {
        let alias_root = alias_root_by_skill_root.get(root)?.clone();
        if seen.insert(alias_root.clone()) {
            alias_roots.push(alias_root);
        }
    }
    Some(alias_roots)
}

fn alias_root_for_skill_root(
    root: &AbsolutePathBuf,
    plugin_version_skill_counts: &HashMap<AbsolutePathBuf, usize>,
) -> AbsolutePathBuf {
    let Some(plugin_version_base) = plugin_version_base(root.as_path()) else {
        return root.clone();
    };
    let skill_count = plugin_version_skill_counts
        .get(&plugin_version_base)
        .copied()
        .unwrap_or_default();
    if skill_count > 1 {
        root.clone()
    } else {
        plugin_marketplace_base(root.as_path()).unwrap_or_else(|| root.clone())
    }
}

fn plugin_version_skill_counts_for_skill_roots<'a>(
    skill_roots: impl Iterator<Item = &'a AbsolutePathBuf>,
) -> HashMap<AbsolutePathBuf, usize> {
    let mut counts = HashMap::new();
    for root in skill_roots {
        if let Some(plugin_version_base) = plugin_version_base(root.as_path()) {
            let count = counts.entry(plugin_version_base).or_insert(0usize);
            *count = count.saturating_add(1);
        }
    }
    counts
}

fn aliased_metadata_overhead_cost(
    budget: SkillMetadataBudget,
    skill_root_lines: &[String],
) -> usize {
    let empty_skill_lines: &[String] = &[];
    // Only the layout differs between the two calls, so the coverage wording
    // cancels out of the delta as long as both sides use the same value.
    let coverage = SkillsListCoverage::COMPLETE_WITHOUT_SEARCH;
    let absolute_body = render_available_skills_body(&[], empty_skill_lines, coverage);
    let aliased_body = render_available_skills_body(skill_root_lines, empty_skill_lines, coverage);
    budget
        .cost(&aliased_body)
        .saturating_sub(budget.cost(&absolute_body))
}

fn build_skill_root_lines(roots: &[AbsolutePathBuf]) -> Vec<String> {
    roots
        .iter()
        .enumerate()
        .map(|(index, root)| {
            let root_str = root.to_string_lossy().replace('\\', "/");
            format!("- `r{index}` = `{root_str}`")
        })
        .collect()
}

fn plugin_marketplace_base(path: &Path) -> Option<AbsolutePathBuf> {
    let mut candidate = path;
    while let Some(parent) = candidate.parent() {
        if parent.file_name()?.to_str()? == "cache"
            && parent.parent()?.file_name()?.to_str()? == "plugins"
        {
            return AbsolutePathBuf::from_absolute_path(candidate).ok();
        }
        candidate = parent;
    }
    None
}

fn plugin_version_base(path: &Path) -> Option<AbsolutePathBuf> {
    let marketplace_base = plugin_marketplace_base(path)?;
    let mut relative_components = path
        .strip_prefix(marketplace_base.as_path())
        .ok()?
        .components();
    let plugin = match relative_components.next()? {
        Component::Normal(plugin) => plugin,
        _ => return None,
    };
    let version = match relative_components.next()? {
        Component::Normal(version) => version,
        _ => return None,
    };
    AbsolutePathBuf::from_absolute_path(marketplace_base.join(plugin).join(version)).ok()
}

fn render_skill_path_with_aliases(skill: &SkillMetadata, plan: &AliasPlan) -> String {
    outcome_relative_skill_path(skill, plan)
        .unwrap_or_else(|| skill.path_to_skills_md.to_string_lossy().replace('\\', "/"))
}

fn outcome_relative_skill_path(skill: &SkillMetadata, plan: &AliasPlan) -> Option<String> {
    let alias_root = plan.alias_root_by_path.get(&skill.path_to_skills_md)?;
    let alias = plan.root_aliases.get(alias_root)?;
    let relative_path = skill
        .path_to_skills_md
        .as_path()
        .strip_prefix(alias_root.as_path())
        .ok()?;
    let relative_path = relative_path.to_string_lossy().replace('\\', "/");
    Some(format!("{alias}/{relative_path}"))
}

fn aliased_render_is_better(
    aliased: &AvailableSkills,
    absolute: &AvailableSkills,
    budget: SkillMetadataBudget,
) -> bool {
    if aliased.report.included_count != absolute.report.included_count {
        return aliased.report.included_count > absolute.report.included_count;
    }
    if aliased.report.truncated_description_chars != absolute.report.truncated_description_chars {
        return aliased.report.truncated_description_chars
            < absolute.report.truncated_description_chars;
    }
    available_skills_cost(budget, aliased) < available_skills_cost(budget, absolute)
}

fn available_skills_cost(budget: SkillMetadataBudget, available: &AvailableSkills) -> usize {
    let metadata_cost = if available.skill_root_lines.is_empty() {
        0
    } else {
        aliased_metadata_overhead_cost(budget, &available.skill_root_lines)
    };
    metadata_cost.saturating_add(lines_cost(budget, &available.skill_lines))
}

fn ordered_absolute_skill_lines(skills: &[SkillMetadata]) -> Vec<SkillLine<'_>> {
    ordered_skills_for_budget(skills)
        .into_iter()
        .map(SkillLine::new)
        .collect()
}

fn ordered_skills_for_budget(skills: &[SkillMetadata]) -> Vec<&SkillMetadata> {
    let mut ordered = skills.iter().collect::<Vec<_>>();
    ordered.sort_by(|a, b| {
        prompt_scope_rank(a.scope)
            .cmp(&prompt_scope_rank(b.scope))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.path_to_skills_md.cmp(&b.path_to_skills_md))
    });
    ordered
}

/// Decide how many skills the model-visible list may hold.
///
/// Deferring part of the catalog costs recall, so it only happens when it buys
/// something. Two conditions must both hold:
///
/// 1. the model can reach the rest through a catalog-search tool, and
/// 2. the complete catalog genuinely does not fit the skills context budget.
///
/// Otherwise every prompt-visible skill stays in the list and the existing
/// budget machinery (aliasing, description truncation, omission warnings) does
/// the trimming, exactly as it did before deferral existed.
fn starter_limit_for(
    outcome: &SkillLoadOutcome,
    skills: &[SkillMetadata],
    budget: SkillMetadataBudget,
    catalog_search: SkillCatalogSearch,
    max_starter_skills: usize,
) -> usize {
    if !catalog_search.allows_deferral()
        || skills.len() <= max_starter_skills
        || whole_catalog_can_be_listed(outcome, skills, budget)
    {
        return skills.len();
    }

    max_starter_skills
}

/// Whether every skill can be *listed at all* inside the budget, i.e. with its
/// name and path and its description shortened as far as the empty string,
/// under either the absolute-path or the aliased-path layout.
///
/// Deferral removes whole skills from the model's view; description truncation
/// only makes them terser. Truncation is therefore always preferred, and
/// deferral is reserved for catalogs so large they cannot be enumerated even in
/// that minimal form. Testing the *full* cost here made a realistic twelve-skill
/// workspace with long descriptions silently drop seven of its skills, where the
/// truncation path would have rendered all twelve.
fn whole_catalog_can_be_listed(
    outcome: &SkillLoadOutcome,
    skills: &[SkillMetadata],
    budget: SkillMetadataBudget,
) -> bool {
    let limit = budget.limit();
    let absolute_cost = skills.iter().fold(0usize, |used, skill| {
        used.saturating_add(SkillLine::new(skill).minimum_cost(budget))
    });
    if absolute_cost <= limit {
        return true;
    }

    let Some(plan) = build_alias_plan(outcome, skills, budget) else {
        return false;
    };
    if plan.table_cost >= limit {
        return false;
    }

    let aliased_cost = skills.iter().fold(plan.table_cost, |used, skill| {
        used.saturating_add(
            SkillLine::with_path(skill, render_skill_path_with_aliases(skill, &plan))
                .minimum_cost(budget),
        )
    });
    aliased_cost <= limit
}

/// Pick the starter subset of skills rendered into the model-visible list.
///
/// Every session also loads the bundled `System` skills, so taking the first
/// `limit` entries of [`ordered_skills_for_budget`] would spend the whole starter
/// budget on built-ins and starve a workspace's own Repo/User/Admin skills. Deal
/// the slots round-robin across the scopes that are actually present instead, so
/// each scope keeps a share, then emit the winners in the usual deterministic
/// order.
fn starter_skills(skills: &[SkillMetadata], limit: usize) -> Vec<&SkillMetadata> {
    let ordered = ordered_skills_for_budget(skills);
    if ordered.len() <= limit {
        return ordered;
    }

    let mut scopes: Vec<u8> = Vec::new();
    let mut buckets: Vec<VecDeque<usize>> = Vec::new();
    for (position, skill) in ordered.iter().enumerate() {
        let scope = prompt_scope_rank(skill.scope);
        match scopes.iter().position(|candidate| *candidate == scope) {
            Some(index) => buckets[index].push_back(position),
            None => {
                scopes.push(scope);
                buckets.push(VecDeque::from([position]));
            }
        }
    }

    let mut selected = Vec::with_capacity(limit);
    while selected.len() < limit {
        let mut dealt = false;
        for bucket in &mut buckets {
            if selected.len() == limit {
                break;
            }
            if let Some(position) = bucket.pop_front() {
                selected.push(position);
                dealt = true;
            }
        }
        if !dealt {
            break;
        }
    }

    selected.sort_unstable();
    selected
        .into_iter()
        .map(|position| ordered[position])
        .collect()
}

fn prompt_scope_rank(scope: SkillScope) -> u8 {
    match scope {
        SkillScope::System => 0,
        SkillScope::Admin => 1,
        SkillScope::Repo => 2,
        SkillScope::User => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;

    use codex_utils_absolute_path::test_support::PathBufExt;
    use codex_utils_absolute_path::test_support::test_path_buf;
    use pretty_assertions::assert_eq;

    fn make_skill(name: &str, scope: SkillScope) -> SkillMetadata {
        SkillMetadata {
            name: name.to_string(),
            description: "desc".to_string(),
            short_description: None,
            interface: None,
            dependencies: None,
            policy: None,
            path_to_skills_md: test_path_buf(&format!("/tmp/{name}/SKILL.md")).abs(),
            scope,
            plugin_id: None,
        }
    }

    fn make_skill_with_description(
        name: &str,
        scope: SkillScope,
        description: &str,
    ) -> SkillMetadata {
        let mut skill = make_skill(name, scope);
        skill.description = description.to_string();
        skill
    }

    fn expected_skill_line(skill: &SkillMetadata, description: &str) -> String {
        SkillLine::new(skill).render_with_description(description)
    }

    fn normalized_path(path: &AbsolutePathBuf) -> String {
        path.to_string_lossy().replace('\\', "/")
    }

    fn outcome_with_roots(
        skills: Vec<SkillMetadata>,
        roots: Vec<AbsolutePathBuf>,
    ) -> SkillLoadOutcome {
        let skill_root_by_path = skills
            .iter()
            .filter_map(|skill| {
                roots
                    .iter()
                    .find(|root| {
                        skill
                            .path_to_skills_md
                            .as_path()
                            .starts_with(root.as_path())
                    })
                    .map(|root| (skill.path_to_skills_md.clone(), root.clone()))
            })
            .collect::<HashMap<_, _>>();
        SkillLoadOutcome {
            skills,
            skill_roots: roots,
            skill_root_by_path: Arc::new(skill_root_by_path),
            ..Default::default()
        }
    }

    fn build_available_skills_from_metadata(
        skills: &[SkillMetadata],
        budget: SkillMetadataBudget,
    ) -> Option<AvailableSkills> {
        build_available_skills_from_lines(
            ordered_absolute_skill_lines(skills),
            skills.len(),
            budget,
            SkillPathAliases::default(),
        )
    }

    #[test]
    fn default_budget_uses_two_percent_of_full_context_window() {
        assert_eq!(
            default_skill_metadata_budget(Some(200_000)),
            SkillMetadataBudget::Tokens(4_000)
        );
        assert_eq!(
            default_skill_metadata_budget(Some(99)),
            SkillMetadataBudget::Tokens(1)
        );
    }

    #[test]
    fn default_budget_falls_back_to_characters_without_context_window() {
        assert_eq!(
            default_skill_metadata_budget(/*context_window*/ None),
            SkillMetadataBudget::Characters(DEFAULT_SKILL_METADATA_CHAR_BUDGET)
        );
        assert_eq!(
            default_skill_metadata_budget(Some(-1)),
            SkillMetadataBudget::Characters(DEFAULT_SKILL_METADATA_CHAR_BUDGET)
        );
    }

    #[test]
    fn budgeted_rendering_truncates_descriptions_equally_before_omitting_skills() {
        let alpha = make_skill_with_description("alpha-skill", SkillScope::Repo, "abcdef");
        let beta = make_skill_with_description("beta-skill", SkillScope::Repo, "uvwxyz");
        let minimum_cost = SkillLine::new(&alpha)
            .minimum_cost(SkillMetadataBudget::Characters(usize::MAX))
            + SkillLine::new(&beta).minimum_cost(SkillMetadataBudget::Characters(usize::MAX));
        let budget = SkillMetadataBudget::Characters(minimum_cost + 6);

        let rendered = build_available_skills_from_metadata(&[beta.clone(), alpha.clone()], budget)
            .expect("skills should render");

        assert_eq!(rendered.report.included_count, 2);
        assert_eq!(rendered.report.omitted_count, 0);
        assert_eq!(rendered.report.truncated_description_chars, 8);
        assert_eq!(rendered.warning_message, None);
        assert_eq!(
            rendered.skill_lines,
            vec![
                expected_skill_line(&alpha, "ab"),
                expected_skill_line(&beta, "uv"),
            ]
        );
    }

    #[test]
    fn budgeted_rendering_does_not_warn_when_average_description_truncation_is_within_threshold() {
        let alpha = make_skill_with_description("alpha-skill", SkillScope::Repo, "abcdefghij");
        let beta = make_skill_with_description("beta-skill", SkillScope::Repo, "uvwxyzabcd");
        let minimum_cost = SkillLine::new(&alpha)
            .minimum_cost(SkillMetadataBudget::Characters(usize::MAX))
            + SkillLine::new(&beta).minimum_cost(SkillMetadataBudget::Characters(usize::MAX));
        let budget = SkillMetadataBudget::Characters(minimum_cost + 6);

        let rendered = build_available_skills_from_metadata(&[alpha, beta], budget)
            .expect("skills should render");

        assert_eq!(rendered.report.included_count, 2);
        assert_eq!(rendered.report.omitted_count, 0);
        assert_eq!(rendered.report.truncated_description_chars, 16);
        assert_eq!(rendered.report.truncated_description_count, 2);
        assert_eq!(rendered.warning_message, None);
    }

    #[test]
    fn budgeted_rendering_warns_when_average_description_truncation_exceeds_threshold() {
        let long_description = "a".repeat(250);
        let long_skill =
            make_skill_with_description("long-skill", SkillScope::Repo, &long_description);
        let empty_skill = make_skill_with_description("empty-skill", SkillScope::Repo, "");
        let minimum_cost = SkillLine::new(&long_skill)
            .minimum_cost(SkillMetadataBudget::Characters(usize::MAX))
            + SkillLine::new(&empty_skill)
                .minimum_cost(SkillMetadataBudget::Characters(usize::MAX));
        let budget = SkillMetadataBudget::Characters(minimum_cost + 49);

        let rendered = build_available_skills_from_metadata(&[long_skill, empty_skill], budget)
            .expect("skills should render");

        assert_eq!(rendered.report.total_count, 2);
        assert_eq!(rendered.report.included_count, 2);
        assert_eq!(rendered.report.omitted_count, 0);
        assert_eq!(rendered.report.truncated_description_chars, 202);
        assert_eq!(rendered.report.truncated_description_count, 1);
        assert_eq!(
            rendered.warning_message,
            Some(
                "Starter skill descriptions were shortened to fit the skills context budget. The full catalog remains searchable, but some starter descriptions are shorter."
                    .to_string()
            )
        );
    }

    #[test]
    fn budgeted_rendering_token_budget_truncation_warning_mentions_two_percent() {
        let long_description = "a".repeat(1000);
        let long_skill =
            make_skill_with_description("long-skill", SkillScope::Repo, &long_description);
        let minimum_cost =
            SkillLine::new(&long_skill).minimum_cost(SkillMetadataBudget::Tokens(usize::MAX));
        let budget = SkillMetadataBudget::Tokens(minimum_cost + 1);

        let rendered = build_available_skills_from_metadata(&[long_skill], budget)
            .expect("skills should render");

        assert_eq!(
            rendered.warning_message,
            Some(SKILL_DESCRIPTION_TRUNCATED_WARNING_WITH_PERCENT.to_string())
        );
    }

    #[test]
    fn budgeted_rendering_redistributes_unused_description_budget() {
        let short = make_skill_with_description("short-skill", SkillScope::Repo, "x");
        let long = make_skill_with_description("long-skill", SkillScope::Repo, "abcdefghi");
        let minimum_cost = SkillLine::new(&short)
            .minimum_cost(SkillMetadataBudget::Characters(usize::MAX))
            + SkillLine::new(&long).minimum_cost(SkillMetadataBudget::Characters(usize::MAX));
        let budget = SkillMetadataBudget::Characters(minimum_cost + 11);

        let rendered = build_available_skills_from_metadata(&[short.clone(), long.clone()], budget)
            .expect("skills should render");

        assert_eq!(rendered.report.included_count, 2);
        assert_eq!(rendered.report.omitted_count, 0);
        assert_eq!(rendered.warning_message, None);
        assert_eq!(
            rendered.skill_lines,
            vec![
                expected_skill_line(&long, "abcdefgh"),
                expected_skill_line(&short, "x"),
            ]
        );
    }

    #[test]
    fn budgeted_rendering_preserves_prompt_priority_when_minimum_lines_exceed_budget() {
        let system = make_skill("system-skill", SkillScope::System);
        let user = make_skill("user-skill", SkillScope::User);
        let repo = make_skill("repo-skill", SkillScope::Repo);
        let admin = make_skill("admin-skill", SkillScope::Admin);
        let system_cost = SkillMetadataBudget::Characters(usize::MAX)
            .cost(&format!("{}\n", SkillLine::new(&system).render_minimum()));
        let admin_cost = SkillMetadataBudget::Characters(usize::MAX)
            .cost(&format!("{}\n", SkillLine::new(&admin).render_minimum()));
        let budget = SkillMetadataBudget::Characters(system_cost + admin_cost);

        let rendered = build_available_skills_from_metadata(&[system, user, repo, admin], budget)
            .expect("skills should render");

        assert_eq!(rendered.report.included_count, 2);
        assert_eq!(rendered.report.omitted_count, 2);
        assert_eq!(
            rendered.warning_message,
            Some(
                "Exceeded skills context budget. All skill descriptions were removed and 2 additional skills were not included in the model-visible skills list."
                    .to_string()
            )
        );
        let rendered_text = rendered.skill_lines.join("\n");
        assert!(rendered_text.contains("- system-skill:"));
        assert!(rendered_text.contains("- admin-skill:"));
        assert!(!rendered_text.contains("desc"));
        assert!(!rendered_text.contains("- repo-skill:"));
        assert!(!rendered_text.contains("- user-skill:"));
    }

    #[test]
    fn budgeted_rendering_keeps_scanning_after_oversized_entry() {
        let mut oversized = make_skill("oversized-system-skill", SkillScope::System);
        oversized.description = "desc ".repeat(100);
        let repo = make_skill("repo-skill", SkillScope::Repo);
        let repo_cost = SkillMetadataBudget::Characters(usize::MAX)
            .cost(&format!("{}\n", SkillLine::new(&repo).render_full()));
        let budget = SkillMetadataBudget::Characters(repo_cost);

        let rendered = build_available_skills_from_metadata(&[oversized, repo], budget)
            .expect("skills render");

        assert_eq!(rendered.report.included_count, 1);
        assert_eq!(rendered.report.omitted_count, 1);
        assert_eq!(
            rendered.warning_message,
            Some(
                "Exceeded skills context budget. All skill descriptions were removed and 1 additional skill was not included in the model-visible skills list."
                    .to_string()
            )
        );
        let rendered_text = rendered.skill_lines.join("\n");
        assert!(!rendered_text.contains("- oversized-system-skill:"));
        assert!(rendered_text.contains("- repo-skill:"));
    }

    #[test]
    fn outcome_rendering_omits_aliases_when_absolute_plan_has_no_budget_pressure() {
        let root = test_path_buf("/tmp/skills").abs();
        let alpha_path = root.join("alpha/SKILL.md");
        let beta_path = root.join("beta/SKILL.md");
        let outcome = outcome_with_roots(
            vec![
                skill_with_path("alpha-skill", &alpha_path),
                skill_with_path("beta-skill", &beta_path),
            ],
            vec![root],
        );

        let rendered = build_available_skills(
            &outcome,
            SkillMetadataBudget::Characters(usize::MAX),
            SkillCatalogSearch::Available,
            SkillRenderSideEffects::None,
        )
        .expect("skills should render");

        assert!(rendered.skill_root_lines.is_empty());
        assert_eq!(rendered.report.included_count, 2);
    }

    fn scale_outcome(count: usize) -> SkillLoadOutcome {
        let skills = (0..count)
            .rev()
            .map(|index| {
                let name = format!("scale-skill-{index:04}");
                skill_with_path(
                    &name,
                    &test_path_buf(&format!("/tmp/skills/{name}/SKILL.md")).abs(),
                )
            })
            .collect::<Vec<_>>();
        outcome_with_roots(skills, Vec::new())
    }

    #[test]
    fn outcome_rendering_keeps_a_stable_five_skill_starter_at_large_catalog_scale() {
        let outcome = scale_outcome(2_105);

        let rendered = build_available_skills(
            &outcome,
            // 2% of a 200k context window: the 2105-entry catalog cannot fit,
            // so deferral is warranted.
            default_skill_metadata_budget(Some(200_000)),
            SkillCatalogSearch::Available,
            SkillRenderSideEffects::None,
        )
        .expect("starter skills should render");

        assert_eq!(rendered.report.total_count, 2_105);
        assert_eq!(rendered.report.included_count, MAX_STARTER_SKILLS);
        assert_eq!(rendered.report.deferred_count, 2_100);
        assert_eq!(rendered.report.omitted_count, 0);
        // Hiding 2100 skills must never be silent.
        assert_eq!(
            rendered.warning_message.as_deref(),
            Some(
                "Showing 5 of 2105 skills in the model-visible list; the other 2100 skills stay searchable through `skills.list`."
            )
        );
        let rendered_text = rendered.skill_lines.join("\n");
        for index in 0..MAX_STARTER_SKILLS {
            assert!(rendered_text.contains(&format!("scale-skill-{index:04}")));
        }
        assert!(!rendered_text.contains("scale-skill-0005"));

        let body = render_available_skills_body(
            &rendered.skill_root_lines,
            &rendered.skill_lines,
            rendered.coverage,
        );
        assert!(body.contains("not the complete skills catalog"));
        assert!(body.contains("Always consider whether a skill is relevant before acting"));
        assert!(body.contains("`skills.list`"));
        assert!(body.contains("`skills.read`"));
    }

    #[test]
    fn a_small_catalog_truncates_descriptions_instead_of_hiding_skills() {
        // Twelve skills with realistically long descriptions on a 200k window.
        // The full descriptions do not fit the 2% budget, but the skills
        // themselves easily do, so all twelve must still be listed.
        let outcome = outcome_with_roots(
            (0..12)
                .map(|index| {
                    let mut skill = skill_with_path(
                        &format!("wide-skill-{index:02}"),
                        &test_path_buf(&format!("/tmp/skills/wide-skill-{index:02}/SKILL.md"))
                            .abs(),
                    );
                    skill.description = "d".repeat(1_536);
                    skill
                })
                .collect::<Vec<_>>(),
            Vec::new(),
        );

        let rendered = build_available_skills(
            &outcome,
            default_skill_metadata_budget(Some(200_000)),
            SkillCatalogSearch::Available,
            SkillRenderSideEffects::None,
        )
        .expect("skills should render");

        assert_eq!(rendered.report.total_count, 12);
        assert_eq!(rendered.report.included_count, 12);
        assert_eq!(rendered.report.deferred_count, 0);
        assert_eq!(rendered.report.omitted_count, 0);
        assert!(rendered.report.truncated_description_chars > 0);
        assert!(
            rendered.warning_message.is_some(),
            "shortening descriptions must warn"
        );
    }

    #[test]
    fn a_complete_list_does_not_claim_to_be_a_subset_or_advertise_a_missing_tool() {
        let outcome = scale_outcome(12);

        let rendered = build_available_skills(
            &outcome,
            default_skill_metadata_budget(Some(200_000)),
            SkillCatalogSearch::Unavailable,
            SkillRenderSideEffects::None,
        )
        .expect("skills should render");

        assert_eq!(rendered.report.deferred_count, 0);
        assert_eq!(
            rendered.coverage,
            SkillsListCoverage::COMPLETE_WITHOUT_SEARCH
        );

        let body = render_available_skills_body(
            &rendered.skill_root_lines,
            &rendered.skill_lines,
            rendered.coverage,
        );
        assert!(body.contains(SKILLS_INTRO_COMPLETE_LIST));
        assert!(body.contains(SKILLS_DISCOVERY_COMPLETE_LIST));
        assert!(!body.contains("not the complete"));
        assert!(
            !body.contains("skills.list"),
            "must not point at a tool this thread does not have: {body}"
        );
    }

    #[test]
    fn a_partial_list_keeps_pointing_at_the_catalog_search_escape_hatch() {
        let coverage = SkillsListCoverage {
            complete: false,
            catalog_search: SkillCatalogSearch::Available,
        };

        let body = render_available_skills_body(
            &[],
            &["- alpha: desc (file: /tmp/alpha/SKILL.md)".to_string()],
            coverage,
        );

        assert!(body.contains(SKILLS_INTRO_PARTIAL_LIST));
        assert!(body.contains(SKILLS_DISCOVERY_PARTIAL_LIST));
        assert!(body.contains(SKILLS_DISCOVERY_CATALOG_SEARCH));
        assert!(body.contains(SKILLS_MISSING_WITH_CATALOG_SEARCH));
    }

    #[test]
    fn starter_cap_does_not_fire_when_the_whole_catalog_fits_the_budget() {
        // A dozen skills on a 200k context window fit inside the 2% budget with
        // room to spare, so nothing may be hidden behind `skills.list`.
        let outcome = scale_outcome(12);

        let rendered = build_available_skills(
            &outcome,
            default_skill_metadata_budget(Some(200_000)),
            SkillCatalogSearch::Available,
            SkillRenderSideEffects::None,
        )
        .expect("skills should render");

        assert_eq!(rendered.report.total_count, 12);
        assert_eq!(rendered.report.included_count, 12);
        assert_eq!(rendered.report.deferred_count, 0);
        assert_eq!(rendered.report.omitted_count, 0);
        assert_eq!(rendered.report.truncated_description_chars, 0);
    }

    #[test]
    fn starter_cap_does_not_fire_without_a_catalog_search_tool() {
        // Same oversized catalog as the scale test, but this embedder installed
        // no `skills.list`. Hiding entries would make them unreachable, so the
        // budget machinery has to do the trimming instead.
        let outcome = scale_outcome(2_105);

        let rendered = build_available_skills(
            &outcome,
            default_skill_metadata_budget(Some(200_000)),
            SkillCatalogSearch::Unavailable,
            SkillRenderSideEffects::None,
        )
        .expect("skills should render");

        assert_eq!(rendered.report.total_count, 2_105);
        assert_eq!(rendered.report.deferred_count, 0);
        assert!(
            rendered.report.included_count > MAX_STARTER_SKILLS,
            "expected the budget path, not the starter cap: {:?}",
            rendered.report
        );
        assert!(
            rendered.report.omitted_count > 0,
            "expected budget omissions to be reported: {:?}",
            rendered.report
        );
        assert!(
            rendered
                .warning_message
                .as_deref()
                .is_some_and(|message| message.contains("not included in the model-visible")),
            "expected an omission warning: {:?}",
            rendered.warning_message
        );
    }

    #[test]
    fn budget_negotiation_still_runs_on_the_starter_subset() {
        // Deferral trims the catalog down to the starter cap; aliasing and
        // description truncation must still apply to what is left, otherwise
        // the whole budget path would be dead code.
        let root = test_path_buf(
            "/Users/xl/.codewith/plugins/cache/openai-curated/example/hash1234567890/skills-with-a-very-long-shared-prefix",
        )
        .abs();
        let skills = (0..40)
            .map(|index| {
                let mut skill = skill_with_path(
                    &format!("shared-root-skill-{index:02}"),
                    &root.join(format!("skill-{index:02}/SKILL.md")),
                );
                skill.description = "d".repeat(400);
                skill
            })
            .collect::<Vec<_>>();
        let outcome = outcome_with_roots(skills, vec![root]);

        let rendered = build_available_skills(
            &outcome,
            default_skill_metadata_budget(Some(12_000)),
            SkillCatalogSearch::Available,
            SkillRenderSideEffects::None,
        )
        .expect("skills should render");

        assert_eq!(rendered.report.total_count, 40);
        assert_eq!(rendered.report.included_count, MAX_STARTER_SKILLS);
        assert_eq!(rendered.report.deferred_count, 40 - MAX_STARTER_SKILLS);
        assert!(
            !rendered.skill_root_lines.is_empty(),
            "expected the alias plan to win under budget pressure"
        );
        assert!(
            rendered.report.truncated_description_chars > 0,
            "expected description truncation on the starter subset: {:?}",
            rendered.report
        );
    }

    #[test]
    fn uncapped_catalog_still_aliases_every_skill_under_budget_pressure() {
        // Deterministic mirror of the `codex-core`
        // `skills_use_aliases_in_developer_message_under_budget_pressure`
        // integration test: 12 skills under one long shared root, a 12k context
        // window (240-token budget), and no catalog-search tool. Absolute paths
        // do not fit, so aliasing must kick in and carry all 12 entries.
        let root = test_path_buf(
            "/tmp/.tmp0a1b2c/codex-home-with-long-shared-prefix-for-skill-alias-budget-test/.tmp3d4e5f/skills",
        )
        .abs();
        let skills = (0..12)
            .map(|index| {
                skill_with_path(
                    &format!("s{index:02}"),
                    &root.join(format!("s{index:02}/SKILL.md")),
                )
            })
            .collect::<Vec<_>>();
        let outcome = outcome_with_roots(skills, vec![root]);
        let budget = default_skill_metadata_budget(Some(12_000));

        let rendered = build_available_skills(
            &outcome,
            budget,
            SkillCatalogSearch::Unavailable,
            SkillRenderSideEffects::None,
        )
        .expect("skills should render");

        assert_eq!(rendered.report.total_count, 12);
        assert_eq!(rendered.report.included_count, 12);
        assert_eq!(rendered.report.deferred_count, 0);
        assert_eq!(rendered.report.omitted_count, 0);
        assert_eq!(rendered.skill_root_lines.len(), 1);
        let rendered_text = rendered.skill_lines.join("\n");
        for index in 0..12 {
            assert!(
                rendered_text.contains(&format!("(file: r0/s{index:02}/SKILL.md)")),
                "expected aliased entry for s{index:02} in {rendered_text}"
            );
        }
    }

    #[test]
    fn task_relevant_body_drops_the_duplicated_how_to_use_preamble() {
        let lines = vec!["- alpha-skill: desc (file: /tmp/alpha/SKILL.md)".to_string()];

        let body = render_task_relevant_skills_body(&lines, SkillsPreamble::RenderedElsewhere);

        assert!(body.contains(TASK_RELEVANT_SKILLS_HEADING));
        assert!(body.contains("- alpha-skill: desc (file: /tmp/alpha/SKILL.md)"));
        assert!(!body.contains("### How to use skills"));
        assert!(!body.contains(SKILLS_HOW_TO_USE_TAIL_WITH_ABSOLUTE_PATHS));
        assert!(!body.contains(SKILLS_HOW_TO_USE_TAIL_WITH_ALIASES));
        assert!(
            body.len()
                < render_available_skills_body(
                    &[],
                    &lines,
                    SkillsListCoverage::COMPLETE_WITHOUT_SEARCH
                )
                .len()
                .saturating_sub(2_000),
            "expected the compact body to save the ~2.5k-char preamble"
        );
    }

    #[test]
    fn task_relevant_body_carries_the_rules_when_there_is_no_skills_section() {
        // A purely remote/executor catalog produces no `## Skills` block, so
        // this fragment is the only place the model can learn the rules - and
        // it must not point at a section that was never written.
        let lines = vec!["- alpha-skill: desc (file: remote/alpha)".to_string()];

        let body = render_task_relevant_skills_body(&lines, SkillsPreamble::Missing);

        assert!(body.contains(TASK_RELEVANT_SKILLS_HEADING));
        assert!(!body.contains(TASK_RELEVANT_SKILLS_INTRO));
        assert!(body.contains(TASK_RELEVANT_SKILLS_STANDALONE_INTRO));
        assert!(body.contains("### How to use skills"));
        assert!(body.contains(SKILLS_DISCOVERY_PARTIAL_LIST));
        assert!(body.contains(SKILLS_DISCOVERY_CATALOG_SEARCH));
        assert!(body.contains(SKILLS_TRIGGER_RULES));
        assert!(body.contains(SKILLS_HOW_TO_USE_TAIL_WITH_ABSOLUTE_PATHS));
    }

    #[test]
    fn starter_selection_does_not_let_bundled_system_skills_starve_other_scopes() {
        // Every session loads the bundled `System` skills, and there are already
        // enough of them to fill `MAX_STARTER_SKILLS` on their own. The starter
        // subset must still surface the workspace's own skills.
        let mut skills = (0..MAX_STARTER_SKILLS)
            .map(|index| make_skill(&format!("system-{index}"), SkillScope::System))
            .collect::<Vec<_>>();
        skills.push(make_skill("repo-only", SkillScope::Repo));
        skills.push(make_skill("user-only", SkillScope::User));

        let selected = starter_skills(&skills, MAX_STARTER_SKILLS)
            .into_iter()
            .map(|skill| skill.name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(selected.len(), MAX_STARTER_SKILLS);
        assert!(selected.contains(&"repo-only"), "got {selected:?}");
        assert!(selected.contains(&"user-only"), "got {selected:?}");
        // Deterministic render order is still scope-then-name.
        assert_eq!(
            selected,
            vec!["system-0", "system-1", "system-2", "repo-only", "user-only"]
        );
    }

    #[test]
    fn outcome_rendering_uses_aliases_when_they_allow_more_skills_to_fit() {
        let root = test_path_buf(
            "/Users/xl/.codewith/plugins/cache/openai-curated/example/hash1234567890/skills-with-a-very-long-shared-prefix",
        )
        .abs();
        let skills = (0..12)
            .map(|index| {
                let name = format!("shared-root-skill-{index}");
                skill_with_path(&name, &root.join(format!("skill-{index}/SKILL.md")))
            })
            .collect::<Vec<_>>();
        let outcome = outcome_with_roots(skills.clone(), vec![root]);
        let starter = starter_skills(&skills, MAX_STARTER_SKILLS)
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let absolute_minimum = starter.iter().fold(0usize, |cost, skill| {
            cost.saturating_add(
                SkillLine::new(skill).minimum_cost(SkillMetadataBudget::Characters(usize::MAX)),
            )
        });
        let plan = build_alias_plan(
            &outcome,
            &starter,
            SkillMetadataBudget::Characters(usize::MAX),
        )
        .expect("alias plan should build");
        let alias_minimum = starter.iter().fold(plan.table_cost, |cost, skill| {
            cost.saturating_add(
                SkillLine::with_path(skill, render_skill_path_with_aliases(skill, &plan))
                    .minimum_cost(SkillMetadataBudget::Characters(usize::MAX)),
            )
        });
        assert!(
            alias_minimum < absolute_minimum,
            "test fixture should make aliases cheaper"
        );

        let rendered = build_available_skills(
            &outcome,
            SkillMetadataBudget::Characters(alias_minimum),
            SkillCatalogSearch::Available,
            SkillRenderSideEffects::None,
        )
        .expect("skills should render");

        assert_eq!(rendered.report.total_count, skills.len());
        assert_eq!(rendered.report.included_count, MAX_STARTER_SKILLS);
        assert_eq!(
            rendered.report.deferred_count,
            skills.len() - MAX_STARTER_SKILLS
        );
        assert_eq!(rendered.report.omitted_count, 0);
        assert_eq!(
            rendered.skill_root_lines,
            vec![format!(
                "- `r0` = `{}`",
                normalized_path(
                    &test_path_buf(
                        "/Users/xl/.codewith/plugins/cache/openai-curated/example/hash1234567890/skills-with-a-very-long-shared-prefix"
                    )
                    .abs()
                )
            )]
        );
        let rendered_text = rendered.skill_lines.join("\n");
        assert!(rendered_text.contains("r0/skill-0/SKILL.md"));
        assert!(rendered_text.contains("r0/skill-11/SKILL.md"));
        assert!(!rendered_text.contains("r0/skill-3/SKILL.md"));
    }

    #[test]
    fn outcome_rendering_uses_marketplace_root_for_single_skill_plugin_versions() {
        let github_root =
            test_path_buf("/Users/xl/.codewith/plugins/cache/openai-curated/github/hash123/skills")
                .abs();
        let marketplace_root =
            test_path_buf("/Users/xl/.codewith/plugins/cache/openai-curated").abs();
        let github = skill_with_path("github:gh-fix-ci", &github_root.join("gh-fix-ci/SKILL.md"));
        let outcome = outcome_with_roots(vec![github.clone()], vec![github_root.clone()]);
        let plan = build_alias_plan(
            &outcome,
            &[github],
            SkillMetadataBudget::Characters(usize::MAX),
        )
        .expect("alias plan should build");

        assert_eq!(
            plan.aliases.skill_root_lines,
            vec![format!("- `r0` = `{}`", normalized_path(&marketplace_root))]
        );
        assert_eq!(
            render_skill_path_with_aliases(
                &skill_with_path("github:gh-fix-ci", &github_root.join("gh-fix-ci/SKILL.md")),
                &plan
            ),
            "r0/github/hash123/skills/gh-fix-ci/SKILL.md"
        );
    }

    #[test]
    fn outcome_rendering_uses_skill_root_for_multiple_skills_in_one_plugin_version() {
        let github_root =
            test_path_buf("/Users/xl/.codewith/plugins/cache/openai-curated/github/hash123/skills")
                .abs();
        let fix_ci = skill_with_path("github:gh-fix-ci", &github_root.join("gh-fix-ci/SKILL.md"));
        let yeet = skill_with_path("github:yeet", &github_root.join("yeet/SKILL.md"));
        let outcome = outcome_with_roots(
            vec![fix_ci.clone(), yeet.clone()],
            vec![github_root.clone()],
        );
        let plan = build_alias_plan(
            &outcome,
            &[fix_ci, yeet],
            SkillMetadataBudget::Characters(usize::MAX),
        )
        .expect("alias plan should build");

        assert_eq!(
            plan.aliases.skill_root_lines,
            vec![format!("- `r0` = `{}`", normalized_path(&github_root))]
        );
        assert_eq!(
            render_skill_path_with_aliases(
                &skill_with_path("github:gh-fix-ci", &github_root.join("gh-fix-ci/SKILL.md")),
                &plan
            ),
            "r0/gh-fix-ci/SKILL.md"
        );
        assert_eq!(
            render_skill_path_with_aliases(
                &skill_with_path("github:yeet", &github_root.join("yeet/SKILL.md")),
                &plan
            ),
            "r0/yeet/SKILL.md"
        );
    }

    #[test]
    fn outcome_rendering_counts_plugin_version_skills_before_budget_omission() {
        let root = test_path_buf(
            "/Users/xl/.codewith/plugins/cache/openai-curated/example/hash1234567890/skills-with-a-very-long-shared-prefix",
        )
        .abs();
        let alpha = skill_with_path("alpha-skill", &root.join("alpha/SKILL.md"));
        let beta = skill_with_path("beta-skill", &root.join("beta/SKILL.md"));
        let outcome = outcome_with_roots(vec![alpha.clone(), beta.clone()], vec![root.clone()]);
        let plan = build_alias_plan(
            &outcome,
            &[alpha.clone(), beta.clone()],
            SkillMetadataBudget::Characters(usize::MAX),
        )
        .expect("alias plan should build");
        let alpha_cost = SkillMetadataBudget::Characters(usize::MAX).cost(&format!(
            "{}\n",
            SkillLine::with_path(&alpha, render_skill_path_with_aliases(&alpha, &plan))
                .render_minimum()
        ));
        let rendered = build_aliased_available_skills(
            &outcome,
            &[alpha, beta],
            SkillMetadataBudget::Characters(plan.table_cost + alpha_cost),
        )
        .expect("skills should render");

        assert_eq!(rendered.report.included_count, 1);
        assert_eq!(
            rendered.skill_root_lines,
            vec![format!("- `r0` = `{}`", normalized_path(&root))]
        );
        assert_eq!(
            rendered.skill_lines,
            vec!["- alpha-skill: (file: r0/alpha/SKILL.md)"]
        );
    }

    #[test]
    fn outcome_rendering_uses_each_skill_root_for_multiple_roots_in_one_plugin_version() {
        let skills_root =
            test_path_buf("/Users/xl/.codewith/plugins/cache/openai-curated/github/hash123/skills")
                .abs();
        let extra_root = test_path_buf(
            "/Users/xl/.codewith/plugins/cache/openai-curated/github/hash123/extra-skills",
        )
        .abs();
        let fix_ci = skill_with_path("github:gh-fix-ci", &skills_root.join("gh-fix-ci/SKILL.md"));
        let yeet = skill_with_path("github:yeet", &extra_root.join("yeet/SKILL.md"));
        let outcome = outcome_with_roots(
            vec![fix_ci.clone(), yeet.clone()],
            vec![skills_root.clone(), extra_root.clone()],
        );
        let plan = build_alias_plan(
            &outcome,
            &[fix_ci, yeet],
            SkillMetadataBudget::Characters(usize::MAX),
        )
        .expect("alias plan should build");

        assert_eq!(
            plan.aliases.skill_root_lines,
            vec![
                format!("- `r0` = `{}`", normalized_path(&skills_root)),
                format!("- `r1` = `{}`", normalized_path(&extra_root)),
            ]
        );
        assert_eq!(
            render_skill_path_with_aliases(
                &skill_with_path("github:gh-fix-ci", &skills_root.join("gh-fix-ci/SKILL.md")),
                &plan
            ),
            "r0/gh-fix-ci/SKILL.md"
        );
        assert_eq!(
            render_skill_path_with_aliases(
                &skill_with_path("github:yeet", &extra_root.join("yeet/SKILL.md")),
                &plan
            ),
            "r1/yeet/SKILL.md"
        );
    }

    #[test]
    fn outcome_rendering_extracts_plugin_marketplace_root_for_multiple_plugins() {
        let github_root =
            test_path_buf("/Users/xl/.codewith/plugins/cache/openai-curated/github/hash123/skills")
                .abs();
        let slack_root =
            test_path_buf("/Users/xl/.codewith/plugins/cache/openai-curated/slack/hash456/skills")
                .abs();
        let marketplace_root =
            test_path_buf("/Users/xl/.codewith/plugins/cache/openai-curated").abs();
        let github = skill_with_path("github:gh-fix-ci", &github_root.join("gh-fix-ci/SKILL.md"));
        let slack = skill_with_path(
            "slack:daily-digest",
            &slack_root.join("daily-digest/SKILL.md"),
        );
        let outcome = outcome_with_roots(
            vec![github.clone(), slack.clone()],
            vec![github_root.clone(), slack_root.clone()],
        );
        let plan = build_alias_plan(
            &outcome,
            &[github, slack],
            SkillMetadataBudget::Characters(usize::MAX),
        )
        .expect("alias plan should build");

        assert_eq!(
            plan.aliases.skill_root_lines,
            vec![format!("- `r0` = `{}`", normalized_path(&marketplace_root))]
        );
        assert_eq!(
            render_skill_path_with_aliases(
                &skill_with_path("github:gh-fix-ci", &github_root.join("gh-fix-ci/SKILL.md")),
                &plan
            ),
            "r0/github/hash123/skills/gh-fix-ci/SKILL.md"
        );
        assert_eq!(
            render_skill_path_with_aliases(
                &skill_with_path(
                    "slack:daily-digest",
                    &slack_root.join("daily-digest/SKILL.md")
                ),
                &plan
            ),
            "r0/slack/hash456/skills/daily-digest/SKILL.md"
        );
    }

    #[test]
    fn outcome_rendering_uses_one_marketplace_root_for_multiple_plugin_versions() {
        let skills_root =
            test_path_buf("/Users/xl/.codewith/plugins/cache/openai-curated/github/hash123/skills")
                .abs();
        let extra_root = test_path_buf(
            "/Users/xl/.codewith/plugins/cache/openai-curated/github/hash456/extra-skills",
        )
        .abs();
        let marketplace_root =
            test_path_buf("/Users/xl/.codewith/plugins/cache/openai-curated").abs();
        let fix_ci = skill_with_path("github:gh-fix-ci", &skills_root.join("gh-fix-ci/SKILL.md"));
        let yeet = skill_with_path("github:yeet", &extra_root.join("yeet/SKILL.md"));
        let outcome = outcome_with_roots(
            vec![fix_ci.clone(), yeet.clone()],
            vec![skills_root.clone(), extra_root.clone()],
        );
        let plan = build_alias_plan(
            &outcome,
            &[fix_ci, yeet],
            SkillMetadataBudget::Characters(usize::MAX),
        )
        .expect("alias plan should build");

        assert_eq!(
            plan.aliases.skill_root_lines,
            vec![format!("- `r0` = `{}`", normalized_path(&marketplace_root))]
        );
        assert_eq!(
            render_skill_path_with_aliases(
                &skill_with_path("github:gh-fix-ci", &skills_root.join("gh-fix-ci/SKILL.md")),
                &plan
            ),
            "r0/github/hash123/skills/gh-fix-ci/SKILL.md"
        );
        assert_eq!(
            render_skill_path_with_aliases(
                &skill_with_path("github:yeet", &extra_root.join("yeet/SKILL.md")),
                &plan
            ),
            "r0/github/hash456/extra-skills/yeet/SKILL.md"
        );
    }

    fn skill_with_path(name: &str, path: &AbsolutePathBuf) -> SkillMetadata {
        let mut skill = make_skill(name, SkillScope::User);
        skill.path_to_skills_md = path.clone();
        skill
    }
}
