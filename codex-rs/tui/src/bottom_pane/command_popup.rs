use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::WidgetRef;

use super::popup_consts::MAX_POPUP_ROWS;
use super::scroll_state::ScrollState;
use super::selection_popup_common::ColumnWidthConfig;
use super::selection_popup_common::ColumnWidthMode;
use super::selection_popup_common::GenericDisplayRow;
use super::selection_popup_common::measure_rows_height_with_col_width_mode;
use super::selection_popup_common::render_rows_with_col_width_mode;
use super::slash_commands::BuiltinCommandFlags;
use super::slash_commands::ServiceTierCommand;
use super::slash_commands::SlashCommandItem;
use super::slash_commands::commands_for_input;
use crate::render::Insets;
use crate::render::RectExt;
use crate::slash_command::SlashCommand;

const COMMAND_COLUMN_WIDTH: ColumnWidthConfig = ColumnWidthConfig::new(
    ColumnWidthMode::AutoAllRows,
    /*name_column_width*/ None,
);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum CommandMatchKind {
    Exact,
    Prefix,
    Acronym,
    Substring,
    Subsequence,
}

#[derive(Clone, Debug)]
struct CommandMatch {
    item: CommandItem,
    indices: Option<Vec<usize>>,
    kind: CommandMatchKind,
    score: i32,
    start: usize,
    order: usize,
}

/// A selectable item in the popup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CommandItem {
    Builtin(SlashCommand),
    ServiceTier(ServiceTierCommand),
}

pub(crate) struct CommandPopup {
    command_filter: String,
    commands: Vec<CommandItem>,
    state: ScrollState,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct CommandPopupFlags {
    pub(crate) collaboration_modes_enabled: bool,
    pub(crate) connectors_enabled: bool,
    pub(crate) plugins_command_enabled: bool,
    pub(crate) service_tier_commands_enabled: bool,
    pub(crate) goal_command_enabled: bool,
    pub(crate) workflow_command_enabled: bool,
    pub(crate) scheduled_tasks_command_enabled: bool,
    pub(crate) personality_command_enabled: bool,
    pub(crate) realtime_conversation_enabled: bool,
    pub(crate) audio_device_selection_enabled: bool,
    pub(crate) windows_degraded_sandbox_active: bool,
    pub(crate) side_conversation_active: bool,
}

impl From<CommandPopupFlags> for BuiltinCommandFlags {
    fn from(value: CommandPopupFlags) -> Self {
        Self {
            collaboration_modes_enabled: value.collaboration_modes_enabled,
            connectors_enabled: value.connectors_enabled,
            plugins_command_enabled: value.plugins_command_enabled,
            service_tier_commands_enabled: value.service_tier_commands_enabled,
            goal_command_enabled: value.goal_command_enabled,
            workflow_command_enabled: value.workflow_command_enabled,
            scheduled_tasks_command_enabled: value.scheduled_tasks_command_enabled,
            personality_command_enabled: value.personality_command_enabled,
            realtime_conversation_enabled: value.realtime_conversation_enabled,
            audio_device_selection_enabled: value.audio_device_selection_enabled,
            allow_elevate_sandbox: value.windows_degraded_sandbox_active,
            side_conversation_active: value.side_conversation_active,
        }
    }
}

impl CommandPopup {
    pub(crate) fn new(
        flags: CommandPopupFlags,
        service_tier_commands: Vec<ServiceTierCommand>,
    ) -> Self {
        // Keep built-in availability in sync with the composer.
        let commands = commands_for_input(flags.into(), &service_tier_commands)
            .into_iter()
            .map(|command| match command {
                SlashCommandItem::Builtin(cmd) => CommandItem::Builtin(cmd),
                SlashCommandItem::ServiceTier(command) => CommandItem::ServiceTier(command),
            })
            .collect();
        Self {
            command_filter: String::new(),
            commands,
            state: ScrollState::new(),
        }
    }

    /// Update the filter string based on the current composer text. The text
    /// passed in is expected to start with a leading '/'. Everything after the
    /// *first* '/' on the *first* line becomes the active filter that is used
    /// to narrow down the list of available commands.
    pub(crate) fn on_composer_text_change(&mut self, text: String) {
        let first_line = text.lines().next().unwrap_or("");
        let previous_filter = self.command_filter.clone();

        if let Some(stripped) = first_line.strip_prefix('/') {
            // Extract the *first* token (sequence of non-whitespace
            // characters) after the slash so that `/clear something` still
            // shows the help for `/clear`.
            let token = stripped.trim_start();
            let cmd_token = token.split_whitespace().next().unwrap_or("");

            // Update the filter keeping the original case (commands are all
            // lower-case for now but this may change in the future).
            self.command_filter = cmd_token.to_string();
        } else {
            // The composer no longer starts with '/'. Reset the filter so the
            // popup shows the *full* command list if it is still displayed
            // for some reason.
            self.command_filter.clear();
        }

        if self.command_filter != previous_filter {
            self.state.reset();
        }

        // Reset or clamp selected index based on new filtered list.
        let matches_len = self.filtered_items().len();
        self.state.clamp_selection(matches_len);
        self.state
            .ensure_visible(matches_len, MAX_POPUP_ROWS.min(matches_len));
    }

    /// Determine the preferred height of the popup for a given width.
    /// Accounts for wrapped descriptions so that long tooltips don't overflow.
    pub(crate) fn calculate_required_height(&self, width: u16) -> u16 {
        let rows = self.rows_from_matches(self.filtered());

        measure_rows_height_with_col_width_mode(
            &rows,
            &self.state,
            MAX_POPUP_ROWS,
            width,
            COMMAND_COLUMN_WIDTH,
        )
    }

    /// Compute ranked matches over built-in commands and service-tier prompts.
    ///
    /// Ranking is intentionally deterministic: exact, prefix, acronym,
    /// substring, then broad subsequence matches. Ties are resolved by tighter fuzzy
    /// score, earlier match start, and finally the original presentation order.
    fn filtered(&self) -> Vec<(CommandItem, Option<Vec<usize>>)> {
        let filter = self.command_filter.trim();
        if filter.is_empty() {
            return self
                .commands
                .iter()
                .cloned()
                .map(|command| (command, None))
                .collect();
        }

        let mut matches = self
            .commands
            .iter()
            .enumerate()
            .filter_map(|(order, command)| command_match(command, filter, order))
            .collect::<Vec<_>>();
        matches.sort_by_key(|command_match| {
            (
                command_match.kind,
                command_match.score,
                command_match.start,
                command_match.order,
            )
        });

        matches
            .into_iter()
            .map(|command_match| (command_match.item, command_match.indices))
            .collect()
    }

    fn filtered_items(&self) -> Vec<CommandItem> {
        self.filtered().into_iter().map(|(c, _)| c).collect()
    }

    fn rows_from_matches(
        &self,
        matches: Vec<(CommandItem, Option<Vec<usize>>)>,
    ) -> Vec<GenericDisplayRow> {
        matches
            .into_iter()
            .map(|(item, indices)| {
                let name = format!("/{}", item.command());
                let description = item.description().to_string();
                GenericDisplayRow {
                    name,
                    name_prefix_spans: Vec::new(),
                    match_indices: indices.map(|v| v.into_iter().map(|i| i + 1).collect()),
                    display_shortcut: None,
                    description: Some(description),
                    category_tag: None,
                    wrap_indent: None,
                    is_disabled: false,
                    disabled_reason: None,
                }
            })
            .collect()
    }

    /// Move the selection cursor one step up.
    pub(crate) fn move_up(&mut self) {
        let len = self.filtered_items().len();
        self.state.move_up_wrap(len);
        self.state.ensure_visible(len, MAX_POPUP_ROWS.min(len));
    }

    /// Move the selection cursor one step down.
    pub(crate) fn move_down(&mut self) {
        let matches_len = self.filtered_items().len();
        self.state.move_down_wrap(matches_len);
        self.state
            .ensure_visible(matches_len, MAX_POPUP_ROWS.min(matches_len));
    }

    /// Return currently selected command, if any.
    pub(crate) fn selected_item(&self) -> Option<CommandItem> {
        let matches = self.filtered_items();
        self.state
            .selected_idx
            .and_then(|idx| matches.get(idx).cloned())
    }
}

impl CommandItem {
    pub(crate) fn command(&self) -> &str {
        match self {
            Self::Builtin(cmd) => cmd.command(),
            Self::ServiceTier(command) => &command.name,
        }
    }

    fn description(&self) -> &str {
        match self {
            Self::Builtin(cmd) => cmd.description(),
            Self::ServiceTier(command) => &command.description,
        }
    }

    fn aliases(&self) -> &'static [&'static str] {
        match self {
            Self::Builtin(SlashCommand::Compact) => &["ac"],
            Self::Builtin(SlashCommand::Pets) => &["pet"],
            Self::Builtin(_) | Self::ServiceTier(_) => &[],
        }
    }
}

fn command_match(item: &CommandItem, filter: &str, order: usize) -> Option<CommandMatch> {
    let display = item.command();
    let mut best = name_match(display, filter, /*highlight*/ true);
    for alias in item.aliases() {
        if let Some(alias_match) = name_match(alias, filter, /*highlight*/ false)
            && best
                .as_ref()
                .is_none_or(|best_match| alias_match.sort_key() < best_match.sort_key())
        {
            best = Some(alias_match);
        }
    }

    best.map(|name_match| CommandMatch {
        item: item.clone(),
        indices: name_match.indices,
        kind: name_match.kind,
        score: name_match.score,
        start: name_match.start,
        order,
    })
}

#[derive(Clone, Debug)]
struct NameMatch {
    indices: Option<Vec<usize>>,
    kind: CommandMatchKind,
    score: i32,
    start: usize,
}

impl NameMatch {
    fn sort_key(&self) -> (CommandMatchKind, i32, usize) {
        (self.kind, self.score, self.start)
    }
}

fn name_match(name: &str, filter: &str, highlight: bool) -> Option<NameMatch> {
    let name_lower = name.to_lowercase();
    let filter_lower = filter.to_lowercase();
    let filter_chars = filter.chars().count();
    let indices_for = |offset| highlight.then(|| (offset..offset + filter_chars).collect());

    if name_lower == filter_lower {
        return Some(NameMatch {
            indices: indices_for(/*offset*/ 0),
            kind: CommandMatchKind::Exact,
            score: 0,
            start: 0,
        });
    }

    if name_lower.starts_with(&filter_lower) {
        return Some(NameMatch {
            indices: indices_for(/*offset*/ 0),
            kind: CommandMatchKind::Prefix,
            score: 0,
            start: 0,
        });
    }

    if let Some(indices) = acronym_indices(name, filter) {
        return Some(NameMatch {
            indices: highlight.then_some(indices),
            kind: CommandMatchKind::Acronym,
            score: 0,
            start: 0,
        });
    }

    if let Some(start_byte) = name_lower.find(&filter_lower) {
        let start = name_lower[..start_byte].chars().count();
        return Some(NameMatch {
            indices: indices_for(start),
            kind: CommandMatchKind::Substring,
            score: 0,
            start,
        });
    }

    if let Some((indices, score)) = codex_utils_fuzzy_match::fuzzy_match(name, filter) {
        let start = indices.first().copied().unwrap_or(usize::MAX);
        return Some(NameMatch {
            indices: highlight.then_some(indices),
            kind: CommandMatchKind::Subsequence,
            score,
            start,
        });
    }

    None
}

fn acronym_indices(name: &str, filter: &str) -> Option<Vec<usize>> {
    let name_chars = name.chars().collect::<Vec<_>>();
    let mut boundary_indices = name
        .chars()
        .enumerate()
        .filter_map(|(index, ch)| {
            if index == 0 {
                Some(index)
            } else if matches!(ch, '-' | '_') {
                Some(index + 1)
            } else {
                None
            }
        })
        .filter(|index| *index < name_chars.len());

    let mut matched = Vec::new();
    for filter_char in filter.to_lowercase().chars() {
        let index = boundary_indices.find(|index| {
            name_chars
                .get(*index)
                .is_some_and(|name_char| name_char.to_lowercase().any(|ch| ch == filter_char))
        })?;
        matched.push(index);
    }
    Some(matched)
}

impl WidgetRef for CommandPopup {
    fn render_ref(&self, area: Rect, buf: &mut Buffer) {
        let rows = self.rows_from_matches(self.filtered());
        render_rows_with_col_width_mode(
            area.inset(Insets::tlbr(
                /*top*/ 0, /*left*/ 2, /*bottom*/ 0, /*right*/ 0,
            )),
            buf,
            &rows,
            &self.state,
            MAX_POPUP_ROWS,
            "no matches",
            COMMAND_COLUMN_WIDTH,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn filtered_command_names(popup: &CommandPopup) -> Vec<String> {
        popup
            .filtered_items()
            .into_iter()
            .map(|item| item.command().to_string())
            .collect()
    }

    #[test]
    fn filter_includes_init_when_typing_prefix() {
        let mut popup = CommandPopup::new(CommandPopupFlags::default(), Vec::new());
        // Simulate the composer line starting with '/in' so the popup filters
        // matching commands by prefix.
        popup.on_composer_text_change("/in".to_string());

        // Access the filtered list via the selected command and ensure that
        // one of the matches is the new "init" command.
        let matches = popup.filtered_items();
        let has_init = matches.iter().any(|item| match item {
            CommandItem::Builtin(cmd) => cmd.command() == "init",
            CommandItem::ServiceTier(_) => false,
        });
        assert!(
            has_init,
            "expected '/init' to appear among filtered commands"
        );
    }

    #[test]
    fn selecting_init_by_exact_match() {
        let mut popup = CommandPopup::new(CommandPopupFlags::default(), Vec::new());
        popup.on_composer_text_change("/init".to_string());

        // When an exact match exists, the selected command should be that
        // command by default.
        let selected = popup.selected_item();
        match selected {
            Some(CommandItem::Builtin(cmd)) => assert_eq!(cmd.command(), "init"),
            Some(CommandItem::ServiceTier(command)) => {
                panic!("expected init command, got service tier {command:?}")
            }
            None => panic!("expected a selected command for exact match"),
        }
    }

    #[test]
    fn model_is_first_suggestion_for_mo() {
        let mut popup = CommandPopup::new(CommandPopupFlags::default(), Vec::new());
        popup.on_composer_text_change("/mo".to_string());
        let matches = popup.filtered_items();
        match matches.first() {
            Some(CommandItem::Builtin(cmd)) => assert_eq!(cmd.command(), "model"),
            Some(CommandItem::ServiceTier(command)) => {
                panic!("expected model command, got service tier {command:?}")
            }
            None => panic!("expected at least one match for '/mo'"),
        }
    }

    #[test]
    fn service_tier_command_uses_catalog_name_and_description() {
        let mut popup = CommandPopup::new(
            CommandPopupFlags {
                service_tier_commands_enabled: true,
                ..CommandPopupFlags::default()
            },
            vec![ServiceTierCommand {
                id: "priority".to_string(),
                name: "fast".to_string(),
                description: "Fastest inference with increased plan usage".to_string(),
            }],
        );
        popup.on_composer_text_change("/fa".to_string());

        match popup.selected_item() {
            Some(CommandItem::ServiceTier(command)) => assert_eq!(
                command,
                ServiceTierCommand {
                    id: "priority".to_string(),
                    name: "fast".to_string(),
                    description: "Fastest inference with increased plan usage".to_string(),
                }
            ),
            other => panic!("expected fast service tier to be selected, got {other:?}"),
        }
        let rows = popup.rows_from_matches(popup.filtered());
        assert_eq!(
            rows.first().and_then(|row| row.description.as_deref()),
            Some("Fastest inference with increased plan usage")
        );
    }

    #[test]
    fn prefix_matches_stay_ahead_of_fuzzy_matches_in_presentation_order() {
        let mut popup = CommandPopup::new(CommandPopupFlags::default(), Vec::new());
        popup.on_composer_text_change("/m".to_string());

        let commands = filtered_command_names(&popup);
        assert_eq!(
            commands
                .iter()
                .take(4)
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["model", "memories", "mission-control", "mention"]
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn app_command_popup_snapshot() {
        let mut popup = CommandPopup::new(CommandPopupFlags::default(), Vec::new());
        popup.on_composer_text_change("/app".to_string());

        let width = 72;
        let area = Rect::new(
            /*x*/ 0,
            /*y*/ 0,
            width,
            popup.calculate_required_height(width),
        );
        let mut buf = Buffer::empty(area);
        popup.render_ref(area, &mut buf);

        insta::assert_snapshot!("command_popup_app", format!("{buf:?}"));
    }

    #[test]
    fn debug_command_popup_snapshot() {
        let mut popup = CommandPopup::new(CommandPopupFlags::default(), Vec::new());
        popup.on_composer_text_change("/debug".to_string());

        let width = 84;
        let area = Rect::new(
            /*x*/ 0,
            /*y*/ 0,
            width,
            popup.calculate_required_height(width),
        );
        let mut buf = Buffer::empty(area);
        popup.render_ref(area, &mut buf);

        insta::assert_snapshot!("command_popup_debug", format!("{buf:?}"));
    }

    #[test]
    fn substring_filter_includes_non_prefix_matches_after_better_matches() {
        let mut popup = CommandPopup::new(CommandPopupFlags::default(), Vec::new());
        popup.on_composer_text_change("/ac".to_string());

        let commands = filtered_command_names(&popup);
        assert_eq!(commands.first().map(String::as_str), Some("compact"));
    }

    #[test]
    fn substring_filter_finds_statusline_from_lin() {
        let mut popup = CommandPopup::new(CommandPopupFlags::default(), Vec::new());
        popup.on_composer_text_change("/lin".to_string());

        assert_eq!(
            filtered_command_names(&popup).first().map(String::as_str),
            Some("statusline")
        );
    }

    #[test]
    fn acronym_filter_finds_service_tier_from_initials() {
        let mut popup = CommandPopup::new(
            CommandPopupFlags {
                service_tier_commands_enabled: true,
                ..CommandPopupFlags::default()
            },
            vec![ServiceTierCommand {
                id: "fast-lane".to_string(),
                name: "fast-lane".to_string(),
                description: "Fast lane".to_string(),
            }],
        );
        popup.on_composer_text_change("/fl".to_string());

        assert_eq!(
            filtered_command_names(&popup).first().map(String::as_str),
            Some("fast-lane")
        );
    }

    #[test]
    fn subsequence_filter_finds_statusline_from_sln() {
        let mut popup = CommandPopup::new(CommandPopupFlags::default(), Vec::new());
        popup.on_composer_text_change("/sln".to_string());

        assert_eq!(
            filtered_command_names(&popup).first().map(String::as_str),
            Some("statusline")
        );
    }

    #[test]
    fn removed_clean_alias_matches_no_command() {
        // Debloat: `/stop` and its `/clean` alias were removed entirely, so the
        // popup must not surface anything for them.
        let mut popup = CommandPopup::new(CommandPopupFlags::default(), Vec::new());
        popup.on_composer_text_change("/clean".to_string());
        assert!(filtered_command_names(&popup).is_empty());

        popup.on_composer_text_change("/stop".to_string());
        assert!(filtered_command_names(&popup).is_empty());
    }

    #[test]
    fn exact_pr_filter_beats_longer_pr_commands() {
        let mut popup = CommandPopup::new(CommandPopupFlags::default(), Vec::new());
        popup.on_composer_text_change("/pr".to_string());

        assert_eq!(
            filtered_command_names(&popup).first().map(String::as_str),
            Some("pr")
        );
    }

    #[test]
    fn pair_command_popup_snapshot() {
        let mut popup = CommandPopup::new(CommandPopupFlags::default(), Vec::new());
        popup.on_composer_text_change("/pa".to_string());

        let width = 84;
        let area = Rect::new(
            /*x*/ 0,
            /*y*/ 0,
            width,
            popup.calculate_required_height(width),
        );
        let mut buf = Buffer::empty(area);
        popup.render_ref(area, &mut buf);

        insta::assert_snapshot!("command_popup_pair", format!("{buf:?}"));
    }

    #[test]
    fn changing_filter_resets_selection_after_scrolling() {
        let mut popup = CommandPopup::new(CommandPopupFlags::default(), Vec::new());
        popup.on_composer_text_change("/".to_string());

        for _ in 0..MAX_POPUP_ROWS {
            popup.move_down();
        }
        assert!(popup.state.scroll_top > 0);

        popup.on_composer_text_change("/st".to_string());

        assert_eq!(
            popup.selected_item(),
            Some(CommandItem::Builtin(SlashCommand::Status))
        );
        assert_eq!(popup.state.scroll_top, 0);
        let width = 72;
        let area = Rect::new(
            /*x*/ 0,
            /*y*/ 0,
            width,
            popup.calculate_required_height(width),
        );
        let mut buf = Buffer::empty(area);
        popup.render_ref(area, &mut buf);
        insta::assert_snapshot!(
            "command_popup_filter_reset_after_scroll",
            format!("{buf:?}")
        );
    }

    #[test]
    fn quit_shown_in_empty_filter_and_for_prefix() {
        let mut popup = CommandPopup::new(CommandPopupFlags::default(), Vec::new());
        popup.on_composer_text_change("/".to_string());
        let items = popup.filtered_items();
        assert!(items.contains(&CommandItem::Builtin(SlashCommand::Quit)));

        popup.on_composer_text_change("/qu".to_string());
        let items = popup.filtered_items();
        assert!(items.contains(&CommandItem::Builtin(SlashCommand::Quit)));
    }

    #[test]
    fn hidden_duplicate_aliases_do_not_surface() {
        let mut popup = CommandPopup::new(CommandPopupFlags::default(), Vec::new());
        popup.on_composer_text_change("/".to_string());
        let items = popup.filtered_items();
        for (alias, canonical) in [
            (SlashCommand::BackgroundAgent, SlashCommand::Agent),
            (SlashCommand::MultiAgents, SlashCommand::Session),
            (SlashCommand::Exit, SlashCommand::Quit),
            (SlashCommand::Btw, SlashCommand::Side),
            (SlashCommand::Stats, SlashCommand::Status),
        ] {
            assert!(!items.contains(&CommandItem::Builtin(alias)));
            assert!(items.contains(&CommandItem::Builtin(canonical)));
        }

        for (text, alias) in [
            ("/background-agent", SlashCommand::BackgroundAgent),
            ("/subagents", SlashCommand::MultiAgents),
            ("/exit", SlashCommand::Exit),
            ("/bt", SlashCommand::Btw),
            ("/stats", SlashCommand::Stats),
        ] {
            popup.on_composer_text_change(text.to_string());
            assert!(
                !popup
                    .filtered_items()
                    .contains(&CommandItem::Builtin(alias))
            );
        }
    }

    #[test]
    fn empty_popup_shows_flat_command_list() {
        let mut popup = CommandPopup::new(CommandPopupFlags::default(), Vec::new());
        popup.on_composer_text_change("/".to_string());
        let items = popup.filtered_items();

        assert_eq!(
            items.first(),
            Some(&CommandItem::Builtin(SlashCommand::Model))
        );
        assert!(items.contains(&CommandItem::Builtin(SlashCommand::Profile)));
        assert!(items.contains(&CommandItem::Builtin(SlashCommand::Provider)));
        assert!(items.contains(&CommandItem::Builtin(SlashCommand::Changelog)));
        assert!(items.contains(&CommandItem::Builtin(SlashCommand::Session)));
        assert!(items.contains(&CommandItem::Builtin(SlashCommand::Agent)));
        assert!(items.contains(&CommandItem::Builtin(SlashCommand::Side)));
        assert!(items.contains(&CommandItem::Builtin(SlashCommand::Quit)));
    }

    #[test]
    fn slash_path_text_is_treated_as_flat_search_text() {
        let mut popup = CommandPopup::new(CommandPopupFlags::default(), Vec::new());
        popup.on_composer_text_change("/session/".to_string());

        assert!(filtered_command_names(&popup).is_empty());
    }

    #[test]
    fn flat_filter_finds_external_agent_without_category_prefix() {
        let mut popup = CommandPopup::new(CommandPopupFlags::default(), Vec::new());
        popup.on_composer_text_change("/external".to_string());

        assert_eq!(
            filtered_command_names(&popup).first().map(String::as_str),
            Some("external-agent")
        );
    }

    #[test]
    fn plan_command_hidden_when_collaboration_modes_disabled() {
        let mut popup = CommandPopup::new(CommandPopupFlags::default(), Vec::new());
        popup.on_composer_text_change("/".to_string());

        let cmds = filtered_command_names(&popup);
        assert!(
            !cmds.iter().any(|cmd| cmd == "plan"),
            "expected '/plan' to be hidden when collaboration modes are disabled, got {cmds:?}"
        );
    }

    #[test]
    fn plan_command_visible_when_collaboration_modes_enabled() {
        let mut popup = CommandPopup::new(
            CommandPopupFlags {
                collaboration_modes_enabled: true,
                connectors_enabled: false,
                plugins_command_enabled: false,
                service_tier_commands_enabled: false,
                goal_command_enabled: false,
                workflow_command_enabled: false,
                scheduled_tasks_command_enabled: false,
                personality_command_enabled: true,
                realtime_conversation_enabled: false,
                audio_device_selection_enabled: false,
                windows_degraded_sandbox_active: false,
                side_conversation_active: false,
            },
            Vec::new(),
        );
        popup.on_composer_text_change("/plan".to_string());

        match popup.selected_item() {
            Some(CommandItem::Builtin(cmd)) => assert_eq!(cmd.command(), "plan"),
            Some(CommandItem::ServiceTier(command)) => {
                panic!("expected plan command, got service tier {command:?}")
            }
            other => panic!("expected plan to be selected for exact match, got {other:?}"),
        }
    }

    #[test]
    fn personality_command_hidden_when_disabled() {
        let mut popup = CommandPopup::new(
            CommandPopupFlags {
                collaboration_modes_enabled: true,
                connectors_enabled: false,
                plugins_command_enabled: false,
                service_tier_commands_enabled: false,
                goal_command_enabled: false,
                workflow_command_enabled: false,
                scheduled_tasks_command_enabled: false,
                personality_command_enabled: false,
                realtime_conversation_enabled: false,
                audio_device_selection_enabled: false,
                windows_degraded_sandbox_active: false,
                side_conversation_active: false,
            },
            Vec::new(),
        );
        popup.on_composer_text_change("/pers".to_string());

        let cmds = filtered_command_names(&popup);
        assert!(
            !cmds.iter().any(|cmd| cmd == "personality"),
            "expected '/personality' to be hidden when disabled, got {cmds:?}"
        );
    }

    #[test]
    fn personality_command_visible_when_enabled() {
        let mut popup = CommandPopup::new(
            CommandPopupFlags {
                collaboration_modes_enabled: true,
                connectors_enabled: false,
                plugins_command_enabled: false,
                service_tier_commands_enabled: false,
                goal_command_enabled: false,
                workflow_command_enabled: false,
                scheduled_tasks_command_enabled: false,
                personality_command_enabled: true,
                realtime_conversation_enabled: false,
                audio_device_selection_enabled: false,
                windows_degraded_sandbox_active: false,
                side_conversation_active: false,
            },
            Vec::new(),
        );
        popup.on_composer_text_change("/personality".to_string());

        match popup.selected_item() {
            Some(CommandItem::Builtin(cmd)) => assert_eq!(cmd.command(), "personality"),
            Some(CommandItem::ServiceTier(command)) => {
                panic!("expected personality command, got service tier {command:?}")
            }
            other => panic!("expected personality to be selected for exact match, got {other:?}"),
        }
    }

    #[test]
    fn settings_command_hidden_when_audio_device_selection_is_disabled() {
        let mut popup = CommandPopup::new(
            CommandPopupFlags {
                collaboration_modes_enabled: false,
                connectors_enabled: false,
                plugins_command_enabled: false,
                service_tier_commands_enabled: false,
                goal_command_enabled: false,
                workflow_command_enabled: false,
                scheduled_tasks_command_enabled: false,
                personality_command_enabled: true,
                realtime_conversation_enabled: true,
                audio_device_selection_enabled: false,
                windows_degraded_sandbox_active: false,
                side_conversation_active: false,
            },
            Vec::new(),
        );
        popup.on_composer_text_change("/aud".to_string());

        let cmds = filtered_command_names(&popup);

        assert!(
            !cmds.iter().any(|cmd| cmd == "settings"),
            "expected '/settings' to be hidden when audio device selection is disabled, got {cmds:?}"
        );
    }

    #[test]
    fn apps_command_visible_when_connectors_enabled() {
        let popup = CommandPopup::new(
            CommandPopupFlags {
                connectors_enabled: true,
                ..CommandPopupFlags::default()
            },
            Vec::new(),
        );
        let cmds = filtered_command_names(&popup);

        assert!(
            cmds.iter().any(|name| name == "apps"),
            "expected '/apps' to be visible when connectors are enabled, got {cmds:?}"
        );
    }

    #[test]
    fn debug_config_command_is_visible_from_popup() {
        let popup = CommandPopup::new(CommandPopupFlags::default(), Vec::new());
        let cmds = filtered_command_names(&popup);

        assert!(
            cmds.iter().any(|name| name == "debug-config"),
            "expected '/debug-config' in popup menu, got {cmds:?}"
        );
    }

    #[test]
    fn internal_memory_debug_commands_stay_out_of_popup() {
        let popup = CommandPopup::new(CommandPopupFlags::default(), Vec::new());
        let cmds = filtered_command_names(&popup);

        assert!(
            !cmds.iter().any(|name| name.starts_with("debug-m-")),
            "expected internal memory debug commands to stay hidden, got {cmds:?}"
        );
    }
}
