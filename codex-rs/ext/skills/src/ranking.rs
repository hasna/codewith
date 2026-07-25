use std::collections::HashSet;

use codex_protocol::user_input::UserInput;

use crate::catalog::SkillCatalog;
use crate::catalog::SkillCatalogEntry;

/// Page size used when the model does not ask for one, and the size of the
/// ranked fragment injected into the turn.
pub(crate) const DEFAULT_SKILL_MATCH_LIMIT: usize = 5;
/// Largest page the model may request. Deliberately larger than
/// [`DEFAULT_SKILL_MATCH_LIMIT`]: a `limit` that can only ever shrink the
/// result set is not an escape hatch.
pub(crate) const MAX_SKILL_MATCH_LIMIT: usize = 50;
/// Largest `offset` the model may page to. Together with
/// [`MAX_SKILL_MATCH_LIMIT`] this bounds how deep a single query can walk the
/// ranked catalog.
pub(crate) const MAX_SKILL_MATCH_OFFSET: usize = 1_000;

/// A `limit` that can only shrink the page is not an escape hatch: it must be
/// able to widen the default page too.
const _: () = assert!(MAX_SKILL_MATCH_LIMIT > DEFAULT_SKILL_MATCH_LIMIT);

const MAX_QUERY_CHARS: usize = 4_096;
const MAX_QUERY_TERMS: usize = 128;
const MAX_METADATA_CHARS: usize = 4_096;

pub(crate) fn user_text_query(inputs: &[UserInput]) -> String {
    let mut query = String::new();
    for text in inputs.iter().filter_map(|input| match input {
        UserInput::Text { text, .. } => Some(text.as_str()),
        _ => None,
    }) {
        if !query.is_empty() {
            query.push(' ');
        }
        let remaining = MAX_QUERY_CHARS.saturating_sub(query.chars().count());
        if remaining == 0 {
            break;
        }
        query.extend(text.chars().take(remaining));
    }
    query
}

pub(crate) fn rank_catalog<'a>(
    catalog: &'a SkillCatalog,
    query: &str,
    limit: usize,
) -> Vec<&'a SkillCatalogEntry> {
    let query_lower = query.to_lowercase();
    let query_terms = lexical_terms(query, MAX_QUERY_CHARS, MAX_QUERY_TERMS);
    if query_terms.is_empty() || limit == 0 {
        return Vec::new();
    }

    let mut ranked = catalog
        .entries
        .iter()
        .filter(|entry| entry.is_prompt_visible())
        .filter_map(|entry| {
            relevance_score(entry, &query_lower, &query_terms).map(|score| (score, entry))
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|(left_score, left), (right_score, right)| {
        right_score
            .cmp(left_score)
            .then_with(|| {
                source_kind_key(&left.authority.kind).cmp(&source_kind_key(&right.authority.kind))
            })
            .then_with(|| left.authority.id.cmp(&right.authority.id))
            .then_with(|| left.id.0.cmp(&right.id.0))
            .then_with(|| left.name.cmp(&right.name))
    });
    ranked
        .into_iter()
        .take(limit)
        .map(|(_, entry)| entry)
        .collect()
}

fn relevance_score(
    entry: &SkillCatalogEntry,
    query_lower: &str,
    query_terms: &HashSet<String>,
) -> Option<usize> {
    let name_lower = entry.name.to_lowercase();
    let name_terms = lexical_terms(&entry.name, MAX_METADATA_CHARS, usize::MAX);
    let description_terms = lexical_terms(
        entry
            .short_description
            .as_deref()
            .unwrap_or(entry.description.as_str()),
        MAX_METADATA_CHARS,
        usize::MAX,
    );

    let explicit_name_token = format!("${name_lower}");
    let explicit_name = query_lower
        .split_whitespace()
        .any(|part| part.trim_matches(non_name_char) == explicit_name_token)
        || query_lower.trim() == name_lower;
    let name_phrase = !name_lower.is_empty() && query_lower.contains(&name_lower);
    let name_matches = query_terms.intersection(&name_terms).count();
    let description_matches = query_terms.intersection(&description_terms).count();
    if !explicit_name && !name_phrase && name_matches == 0 && description_matches == 0 {
        return None;
    }

    Some(
        usize::from(explicit_name)
            .saturating_mul(10_000)
            .saturating_add(usize::from(name_phrase).saturating_mul(1_000))
            .saturating_add(name_matches.saturating_mul(100))
            .saturating_add(description_matches.saturating_mul(10)),
    )
}

fn source_kind_key(kind: &crate::catalog::SkillSourceKind) -> (u8, &str) {
    match kind {
        crate::catalog::SkillSourceKind::Host => (0, ""),
        crate::catalog::SkillSourceKind::Executor => (1, ""),
        crate::catalog::SkillSourceKind::Remote => (2, ""),
        crate::catalog::SkillSourceKind::Custom(kind) => (3, kind),
    }
}

fn lexical_terms(value: &str, max_chars: usize, max_terms: usize) -> HashSet<String> {
    value
        .chars()
        .take(max_chars)
        .flat_map(char::to_lowercase)
        .collect::<String>()
        .split(|character: char| !character.is_alphanumeric())
        // Stop words are dropped on both sides of stemming: before, so that
        // `this` is never reduced to a bare `thi`; after, so that inflections
        // such as `uses` collapse onto the listed `use`.
        .filter(|term| term.len() >= 3 && !is_stop_word(term))
        .map(stem)
        .filter(|term| !is_stop_word(term))
        .take(max_terms)
        .collect()
}

/// Fold the most common English inflections onto a shared key so that
/// `charts`/`chart`, `plotting`/`plot`, and `rendered`/`render` match each
/// other.
///
/// This is a deliberately tiny suffix stripper, not a real stemmer: it keeps
/// matching O(1) per term (both sides are normalized into the same `HashSet`)
/// and cannot introduce the quadratic prefix/fuzzy scans that a wider recall
/// fix would. It does nothing for synonyms — see the recall tests below for the
/// behaviour that is deliberately documented rather than fixed here.
fn stem(term: &str) -> String {
    const MIN_STEM_LEN: usize = 3;

    // Plural forms, following Porter step 1a.
    let singular = if let Some(stem) = term.strip_suffix("sses") {
        format!("{stem}ss")
    } else if let Some(stem) = term.strip_suffix("ies") {
        format!("{stem}y")
    } else if term.ends_with("ss") {
        term.to_string()
    } else if let Some(stem) = term.strip_suffix('s') {
        stem.to_string()
    } else {
        term.to_string()
    };
    let singular = if singular.len() >= MIN_STEM_LEN {
        singular
    } else {
        term.to_string()
    };

    // Verb forms, following a trimmed-down Porter step 1b. No consonant
    // un-doubling: `plotting` simply fails to fold onto `plot` rather than
    // risking `passing` -> `pas`.
    for suffix in ["ing", "ed"] {
        if let Some(stem) = singular.strip_suffix(suffix)
            && stem.len() >= MIN_STEM_LEN.max(4)
        {
            return stem.to_string();
        }
    }

    singular
}

fn non_name_char(character: char) -> bool {
    !character.is_alphanumeric() && character != '-' && character != '_' && character != '$'
}

fn is_stop_word(term: &str) -> bool {
    matches!(
        term,
        "and"
            | "are"
            | "but"
            | "can"
            | "for"
            | "from"
            | "has"
            | "have"
            | "how"
            | "into"
            | "not"
            | "please"
            | "skill"
            | "skills"
            | "that"
            | "the"
            | "this"
            | "use"
            | "using"
            | "was"
            | "what"
            | "when"
            | "where"
            | "which"
            | "with"
            | "you"
    )
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::catalog::SkillAuthority;
    use crate::catalog::SkillPackageId;
    use crate::catalog::SkillResourceId;
    use crate::catalog::SkillSourceKind;

    fn entry(name: &str, description: &str) -> SkillCatalogEntry {
        SkillCatalogEntry::new(
            SkillPackageId(format!("package-{name}")),
            SkillAuthority::new(SkillSourceKind::Host, "host"),
            name,
            description,
            SkillResourceId(format!("{name}/SKILL.md")),
        )
    }

    #[test]
    fn exact_name_precedes_lexical_matches_and_ties_are_stable() {
        let catalog = SkillCatalog {
            entries: vec![
                entry("zeta-review", "Review Rust changes"),
                entry("rust-review", "Review changes"),
                entry("alpha-review", "Review Rust changes"),
            ],
            warnings: Vec::new(),
        };

        let ranked = rank_catalog(&catalog, "please use rust-review for Rust review", 5)
            .into_iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(ranked, vec!["rust-review", "alpha-review", "zeta-review"]);
    }

    #[test]
    fn hidden_entries_and_zero_matches_are_excluded_at_scale() {
        let mut entries = (0..2_100)
            .map(|index| entry(&format!("skill-{index:04}"), "unrelated metadata"))
            .collect::<Vec<_>>();
        entries.push(entry("target-skill", "Operate Blacksmith sandboxes"));
        entries.push(entry("manual-target", "Operate Blacksmith sandboxes").deferred());
        entries.push(entry("disabled-target", "Operate Blacksmith sandboxes").disabled());
        let catalog = SkillCatalog {
            entries,
            warnings: Vec::new(),
        };

        assert_eq!(
            rank_catalog(&catalog, "Blacksmith sandbox", 5)
                .into_iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["target-skill"]
        );
        assert!(rank_catalog(&catalog, "completely unmatched", 5).is_empty());
    }

    #[test]
    fn inflected_query_terms_still_reach_their_skill() {
        let catalog = SkillCatalog {
            entries: vec![
                entry("chart-builder", "Render a chart from a table"),
                entry("query-runner", "Run a query against the warehouse"),
            ],
            warnings: Vec::new(),
        };

        assert_eq!(
            rank_catalog(&catalog, "build some charts", 5)
                .into_iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["chart-builder"]
        );
        assert_eq!(
            rank_catalog(&catalog, "run my queries", 5)
                .into_iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["query-runner"]
        );
    }

    #[test]
    fn paraphrased_queries_do_not_reach_lexically_disjoint_skills() {
        // Documents the real, deliberately-scoped behaviour of this ranker: it
        // is lexical token overlap with light stemming. It has no synonym or
        // embedding layer, so a skill whose metadata shares no token with the
        // request is invisible to `skills.list` and the model has to reword.
        //
        // This is the reason `MAX_SKILL_MATCH_LIMIT` and paging exist: when the
        // top matches are wrong, widening the page is the only recourse the
        // model has. Any future recall work (substring, embeddings) should flip
        // these assertions rather than delete them.
        let catalog = SkillCatalog {
            entries: vec![entry("dataviz", "charts, graphs, plots")],
            warnings: Vec::new(),
        };

        assert!(
            rank_catalog(&catalog, "help me visualize these results", 5).is_empty(),
            "synonym recall is not implemented; update this test when it is"
        );
        assert_eq!(
            rank_catalog(&catalog, "draw me a graph", 5)
                .into_iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["dataviz"],
            "a shared surface token is required for a match"
        );
    }

    #[test]
    fn ranking_can_be_walked_past_the_default_page() {
        let entries = (0..40)
            .map(|index| entry(&format!("blacksmith-{index:02}"), "Operate sandboxes"))
            .collect::<Vec<_>>();
        let catalog = SkillCatalog {
            entries,
            warnings: Vec::new(),
        };

        let first_page = rank_catalog(&catalog, "blacksmith", DEFAULT_SKILL_MATCH_LIMIT)
            .into_iter()
            .map(|entry| entry.name.clone())
            .collect::<Vec<_>>();
        let wide_page = rank_catalog(&catalog, "blacksmith", MAX_SKILL_MATCH_LIMIT)
            .into_iter()
            .map(|entry| entry.name.clone())
            .collect::<Vec<_>>();

        assert_eq!(first_page.len(), DEFAULT_SKILL_MATCH_LIMIT);
        assert_eq!(wide_page.len(), 40);
        assert_eq!(wide_page[..first_page.len()], first_page[..]);
    }
}
