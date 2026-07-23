use codex_core_skills::render_available_skills_body;
use codex_extension_api::ContextualUserFragment;
use codex_protocol::protocol::SKILLS_INSTRUCTIONS_CLOSE_TAG;
use codex_protocol::protocol::SKILLS_INSTRUCTIONS_OPEN_TAG;

use crate::catalog::SkillCatalog;
use crate::ranking::rank_catalog;

const MAX_MAIN_PROMPT_CHARS: usize = 40_000;
const MAX_SKILL_NAME_CHARS: usize = 256;
const MAX_SKILL_DESCRIPTION_CHARS: usize = 1_024;
const MAX_SKILL_PATH_CHARS: usize = 2_048;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AvailableSkillsFragment {
    body: String,
}

impl ContextualUserFragment for AvailableSkillsFragment {
    fn role(&self) -> &'static str {
        "developer"
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn body(&self) -> String {
        self.body.clone()
    }

    fn type_markers() -> (&'static str, &'static str) {
        (SKILLS_INSTRUCTIONS_OPEN_TAG, SKILLS_INSTRUCTIONS_CLOSE_TAG)
    }
}

pub(crate) fn available_skills_fragment(
    catalog: &SkillCatalog,
    query: &str,
    limit: usize,
) -> Option<AvailableSkillsFragment> {
    let skill_lines = rank_catalog(catalog, query, limit)
        .into_iter()
        .map(|entry| {
            let description = entry
                .short_description
                .as_deref()
                .unwrap_or(entry.description.as_str());
            render_skill_line(entry, description)
        })
        .collect::<Vec<_>>();

    if skill_lines.is_empty() {
        return None;
    }

    Some(AvailableSkillsFragment {
        body: render_available_skills_body(&[], &skill_lines),
    })
}

fn render_skill_line(entry: &crate::catalog::SkillCatalogEntry, description: &str) -> String {
    let name = bounded_chars(&entry.name, MAX_SKILL_NAME_CHARS);
    let description = bounded_chars(description, MAX_SKILL_DESCRIPTION_CHARS);
    let file = format!(
        "file: {}",
        bounded_chars(entry.rendered_path(), MAX_SKILL_PATH_CHARS)
    );
    let handles = crate::tools::catalog_tool_handles(entry).map_or(file.clone(), |tool_handles| {
        format!("{file}; {tool_handles}")
    });
    if description.is_empty() {
        format!("- {name}: ({handles})")
    } else {
        format!("- {name}: {description} ({handles})")
    }
}

fn bounded_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

pub(crate) fn truncate_main_prompt_contents(contents: &str) -> (String, bool) {
    let mut chars = 0usize;
    for (index, _) in contents.char_indices() {
        if chars == MAX_MAIN_PROMPT_CHARS {
            return (contents[..index].to_string(), true);
        }
        chars = chars.saturating_add(1);
    }
    (contents.to_string(), false)
}
