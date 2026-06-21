//! `/changelog` transcript output.

use super::*;

const CHANGELOG_SOURCE: &str = include_str!("../../../../CHANGELOG.md");
const RELEASE_SECTION_START: &str = "## [Unreleased]";

impl ChatWidget {
    pub(crate) fn add_changelog_output(&mut self) {
        self.add_to_history(history_cell::AgentMarkdownCell::new(
            changelog_markdown(),
            &self.config.cwd,
        ));
    }
}

fn changelog_markdown() -> String {
    let release_notes = CHANGELOG_SOURCE
        .find(RELEASE_SECTION_START)
        .map(|idx| &CHANGELOG_SOURCE[idx..])
        .unwrap_or(CHANGELOG_SOURCE)
        .trim();

    format!("# Codewith Changelog\n\n{release_notes}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changelog_output_uses_release_sections_without_repository_notes() {
        let markdown = changelog_markdown();

        assert!(markdown.starts_with("# Codewith Changelog\n\n## [Unreleased]"));
        assert!(markdown.contains("## [0.1."));
        assert!(!markdown.contains("Known evidence gaps"));
    }
}
