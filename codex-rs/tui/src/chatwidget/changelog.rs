//! `/changelog` promptbar browser.

use super::*;
use crate::bottom_pane::SelectionRowDisplay;

const CHANGELOG_SOURCE: &str = include_str!("../../../../CHANGELOG.md");
const RELEASE_SECTION_START: &str = "## [Unreleased]";
pub(crate) const CHANGELOG_BROWSER_VIEW_ID: &str = "changelog-browser";

impl ChatWidget {
    pub(crate) fn open_changelog_browser(&mut self) {
        self.replace_or_show_changelog_view(changelog_browser_params());
    }

    pub(crate) fn open_changelog_release(&mut self, version: String) {
        self.replace_or_show_changelog_view(changelog_release_params(&version));
    }

    fn replace_or_show_changelog_view(&mut self, params: SelectionViewParams) {
        if self.bottom_pane.active_view_id() == Some(CHANGELOG_BROWSER_VIEW_ID) {
            let _ = self
                .bottom_pane
                .replace_selection_view_if_active(CHANGELOG_BROWSER_VIEW_ID, params);
        } else {
            self.bottom_pane.show_selection_view(params);
        }
        self.request_redraw();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ChangelogRelease {
    version: String,
    heading: String,
    date: Option<String>,
    bullets: Vec<String>,
}

fn changelog_browser_params() -> SelectionViewParams {
    let releases = changelog_releases();
    let mut header = ColumnRenderable::new();
    header.push(Line::from("Codewith Changelog".bold()));
    header.push(Line::from(
        "Select a release to view its changelog bullets.".dim(),
    ));

    let items = if releases.is_empty() {
        vec![SelectionItem {
            name: "No releases found".to_string(),
            description: Some("CHANGELOG.md does not contain release headings.".to_string()),
            is_disabled: true,
            ..Default::default()
        }]
    } else {
        releases
            .into_iter()
            .map(|release| {
                let version = release.version;
                let search_value = Some(format!(
                    "{} {}",
                    version,
                    release.date.clone().unwrap_or_default()
                ));
                SelectionItem {
                    name: version.clone(),
                    description: release.date,
                    selected_description: Some(format!("Open {version} release notes.")),
                    search_value,
                    actions: vec![Box::new(move |tx| {
                        tx.send(AppEvent::OpenChangelogRelease {
                            version: version.clone(),
                        });
                    })],
                    ..Default::default()
                }
            })
            .collect()
    };

    SelectionViewParams {
        view_id: Some(CHANGELOG_BROWSER_VIEW_ID),
        header: Box::new(header),
        items,
        is_searchable: true,
        search_placeholder: Some("Search versions".to_string()),
        col_width_mode: ColumnWidthMode::Fixed,
        row_display: SelectionRowDisplay::SingleLine,
        ..Default::default()
    }
}

fn changelog_release_params(version: &str) -> SelectionViewParams {
    let release = changelog_releases()
        .into_iter()
        .find(|release| release.version == version);
    let mut header = ColumnRenderable::new();
    let title = release
        .as_ref()
        .map(|release| release.heading.as_str())
        .unwrap_or(version);
    header.push(Line::from(format!("Codewith {title}").bold()));
    header.push(Line::from("Bullets copied from CHANGELOG.md.".dim()));

    let mut items = vec![
        SelectionItem {
            name: "Back".to_string(),
            description: Some("Return to all changelog versions.".to_string()),
            actions: vec![Box::new(|tx| {
                tx.send(AppEvent::OpenChangelogBrowser);
            })],
            ..Default::default()
        },
        SelectionItem {
            name: "Close".to_string(),
            description: Some("Close the changelog browser.".to_string()),
            dismiss_on_select: true,
            ..Default::default()
        },
    ];

    match release {
        Some(release) if release.bullets.is_empty() => {
            items.push(SelectionItem {
                name: "No bullet entries recorded".to_string(),
                description: Some(
                    "This release section has no bullet items in CHANGELOG.md.".to_string(),
                ),
                is_disabled: true,
                ..Default::default()
            });
        }
        Some(release) => {
            items.extend(release.bullets.into_iter().map(|bullet| SelectionItem {
                name: format!("- {bullet}"),
                search_value: Some(bullet),
                is_disabled: true,
                ..Default::default()
            }));
        }
        None => {
            items.push(SelectionItem {
                name: "Release not found".to_string(),
                description: Some(
                    "The selected version is not present in CHANGELOG.md.".to_string(),
                ),
                is_disabled: true,
                ..Default::default()
            });
        }
    }

    SelectionViewParams {
        view_id: Some(CHANGELOG_BROWSER_VIEW_ID),
        header: Box::new(header),
        items,
        is_searchable: false,
        col_width_mode: ColumnWidthMode::Fixed,
        row_display: SelectionRowDisplay::Wrapped,
        ..Default::default()
    }
}

fn changelog_releases() -> Vec<ChangelogRelease> {
    let mut releases = Vec::<ChangelogRelease>::new();
    for line in release_notes_source().split_inclusive('\n') {
        if let Some((version, date)) = parse_release_heading(line) {
            releases.push(ChangelogRelease {
                version,
                heading: line.trim_start_matches("## ").trim().to_string(),
                date,
                bullets: Vec::new(),
            });
            continue;
        }

        if line.trim().starts_with("## ") && !releases.is_empty() {
            break;
        }

        if let Some(release) = releases.last_mut() {
            push_bullet_line(&mut release.bullets, line);
        }
    }
    releases
}

fn release_notes_source() -> &'static str {
    CHANGELOG_SOURCE
        .find(RELEASE_SECTION_START)
        .map(|idx| &CHANGELOG_SOURCE[idx..])
        .unwrap_or(CHANGELOG_SOURCE)
        .trim()
}

fn parse_release_heading(line: &str) -> Option<(String, Option<String>)> {
    let heading = line.trim().strip_prefix("## ")?;
    let version_end = heading.find(']').filter(|_| heading.starts_with('['))?;
    let version = heading.get(1..version_end)?.to_string();
    let date = heading
        .get(version_end + 1..)
        .and_then(|rest| rest.trim().strip_prefix("- "))
        .map(str::trim)
        .filter(|date| !date.is_empty())
        .map(str::to_string);
    Some((version, date))
}

fn push_bullet_line(bullets: &mut Vec<String>, line: &str) {
    let trimmed = line.trim();
    if trimmed.starts_with("- ") {
        bullets.push(trimmed.trim_start_matches("- ").trim().to_string());
    } else if let Some(last) = bullets.last_mut()
        && !trimmed.is_empty()
        && !trimmed.starts_with('#')
        && !trimmed.starts_with("Tag:")
        && !trimmed.starts_with("npm:")
        && !trimmed.starts_with("Compare:")
    {
        last.push(' ');
        last.push_str(trimmed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_event_sender::AppEventSender;
    use crate::bottom_pane::ListSelectionView;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use tokio::sync::mpsc::unbounded_channel;

    #[test]
    fn changelog_releases_use_release_sections_without_repository_notes() {
        let releases = changelog_releases();

        assert_eq!(
            releases.first().map(|release| release.version.as_str()),
            Some("Unreleased")
        );
        assert!(
            releases
                .iter()
                .any(|release| release.version.starts_with("0.1."))
        );
        assert!(
            releases
                .iter()
                .flat_map(|release| release.bullets.iter())
                .all(|bullet| !bullet.contains("Known evidence gaps"))
        );
    }

    #[test]
    fn changelog_release_parser_collects_real_bullets() {
        let releases = changelog_releases();
        let latest_release = releases
            .iter()
            .find(|release| release.version.starts_with("0.1."))
            .expect("versioned release");

        assert!(!latest_release.bullets.is_empty());
        assert!(
            latest_release
                .bullets
                .iter()
                .any(|bullet| bullet.contains("Release pipeline")),
            "expected parsed real release bullet, got {latest_release:#?}"
        );
    }

    #[test]
    fn changelog_release_parser_stops_before_maintenance_process() {
        let releases = changelog_releases();
        let initial_release = releases
            .iter()
            .find(|release| release.version == "0.1.0")
            .expect("initial release");

        assert!(
            initial_release.bullets.iter().all(|bullet| !bullet
                .contains("Determine published npm versions")
                && !bullet.contains("Maintenance Process")),
            "expected only release bullets, got {initial_release:#?}"
        );
    }

    #[test]
    fn changelog_browser_versions_snapshot() {
        let view = selection_view(changelog_browser_params());

        insta::assert_snapshot!(
            "changelog_browser_versions",
            render_lines(&view, /*width*/ 72)
        );
    }

    #[test]
    fn changelog_browser_release_snapshot() {
        let latest_version = changelog_releases()
            .into_iter()
            .find(|release| release.version.starts_with("0.1."))
            .expect("versioned release")
            .version;
        let view = selection_view(changelog_release_params(&latest_version));

        insta::assert_snapshot!(
            "changelog_browser_release_bullets",
            render_lines(&view, /*width*/ 88)
        );
    }

    fn selection_view(params: SelectionViewParams) -> ListSelectionView {
        let (tx_raw, _rx) = unbounded_channel::<AppEvent>();
        ListSelectionView::new(
            params,
            AppEventSender::new(tx_raw),
            crate::keymap::RuntimeKeymap::defaults().list,
        )
    }

    fn render_lines(view: &ListSelectionView, width: u16) -> String {
        let area = Rect::new(0, 0, width, view.desired_height(width));
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);

        (0..area.height)
            .map(|y| {
                let mut line = String::new();
                for x in 0..area.width {
                    line.push_str(buf[(x, y)].symbol());
                }
                line.trim_end().to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}
