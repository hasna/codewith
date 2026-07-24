use std::collections::HashSet;

use codex_protocol::user_input::UserInput;

use crate::catalog::SkillCatalog;
use crate::catalog::SkillCatalogEntry;

pub(crate) const DEFAULT_SKILL_MATCH_LIMIT: usize = 5;
pub(crate) const MAX_SKILL_MATCH_LIMIT: usize = 5;

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
        .filter(|term| term.len() >= 3 && !is_stop_word(term))
        .take(max_terms)
        .map(ToOwned::to_owned)
        .collect()
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
}
