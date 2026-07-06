use strum::IntoEnumIterator;
use strum_macros::AsRefStr;
use strum_macros::EnumIter;
use strum_macros::EnumString;
use strum_macros::IntoStaticStr;

/// Commands that can be invoked by starting a message with a leading slash.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, EnumString, EnumIter, AsRefStr, IntoStaticStr,
)]
#[strum(serialize_all = "kebab-case")]
pub enum SlashCommand {
    // DO NOT ALPHA-SORT! Enum order is presentation order in the popup, so
    // more frequently used commands should be listed first.
    Model,
    Profile,
    Provider,
    Config,
    Prompt,
    Ide,
    Permissions,
    Keymap,
    Vim,
    #[strum(serialize = "setup-default-sandbox")]
    ElevateSandbox,
    #[strum(serialize = "sandbox-add-read-dir")]
    SandboxReadRoot,
    Experimental,
    #[strum(to_string = "approve")]
    AutoReview,
    Memories,
    Skills,
    Hooks,
    Review,
    Pair,
    #[strum(
        to_string = "pr",
        serialize = "prs",
        serialize = "pull-request",
        serialize = "pull-requests"
    )]
    Pr,
    Rename,
    New,
    Archive,
    Resume,
    Tmux,
    Fork,
    App,
    Init,
    Compact,
    Recap,
    Plan,
    Goal,
    Teach,
    #[strum(
        to_string = "mission-control",
        serialize = "mission",
        serialize = "missions"
    )]
    MissionControl,
    #[strum(to_string = "workflow", serialize = "workflows")]
    Workflow,
    Loop,
    Queued,
    Schedule,
    Monitor,
    #[strum(
        to_string = "session",
        serialize = "sessions",
        serialize = "thread",
        serialize = "threads"
    )]
    Session,
    Agent,
    #[strum(
        to_string = "background-agent",
        serialize = "background-agents",
        serialize = "bg-agent"
    )]
    BackgroundAgent,
    Worktree,
    Variant,
    ExternalAgent,
    Side,
    Btw,
    Copy,
    Raw,
    Diff,
    Mention,
    Status,
    Usage,
    Stats,
    Changelog,
    DebugConfig,
    Title,
    Statusline,
    Summary,
    Theme,
    #[strum(to_string = "pets", serialize = "pet")]
    Pets,
    #[strum(to_string = "mcp", serialize = "mcps")]
    Mcp,
    Apps,
    #[strum(to_string = "webhook", serialize = "webhooks")]
    Webhook,
    Plugins,
    Logout,
    Quit,
    Exit,
    Feedback,
    Rollout,
    Ps,
    Clear,
    Personality,
    Realtime,
    Settings,
    TestApproval,
    #[strum(serialize = "subagents")]
    MultiAgents,
}

impl SlashCommand {
    /// User-visible description shown in the popup.
    pub fn description(self) -> &'static str {
        match self {
            SlashCommand::Feedback => "prepare Codewith feedback",
            SlashCommand::New => "start a new chat during a conversation",
            SlashCommand::Init => "create .codewith/CODEWITH.md with instructions for Codewith",
            SlashCommand::Compact => "summarize conversation to prevent hitting the context limit",
            SlashCommand::Recap => "show a one-line summary or answer a session recap question",
            SlashCommand::Review => "review my current changes and find issues",
            SlashCommand::Pair => "start a paired watcher agent for this session",
            SlashCommand::Pr => "inspect GitHub pull requests",
            SlashCommand::Rename => "rename the current thread",
            SlashCommand::Resume => "resume a saved chat",
            SlashCommand::Tmux => "move this session into tmux",
            SlashCommand::Archive => "archive this session and exit",
            SlashCommand::Clear => "clear the terminal and start a new chat",
            SlashCommand::Fork => "fork the current chat",
            SlashCommand::App => "continue this session in Codewith Desktop",
            SlashCommand::Quit | SlashCommand::Exit => "exit Codewith",
            SlashCommand::Copy => "copy last response as markdown",
            SlashCommand::Raw => "toggle raw scrollback mode for copy-friendly terminal selection",
            SlashCommand::Diff => "show git diff (including untracked files)",
            SlashCommand::Mention => "mention a file",
            SlashCommand::Skills => "use skills to improve how Codewith performs specific tasks",
            SlashCommand::Hooks => "view and manage lifecycle hooks",
            SlashCommand::Status => "show current session configuration and token usage",
            SlashCommand::Usage => "show usage, context, and rate limits",
            SlashCommand::Stats => "show session stats and provider usage",
            SlashCommand::Changelog => "show what changed in Codewith releases",
            SlashCommand::DebugConfig => "show config layers and requirement sources for debugging",
            SlashCommand::Title => "configure which items appear in the terminal title",
            SlashCommand::Statusline => "configure which items appear in the status line",
            SlashCommand::Summary => "configure what appears after final messages",
            SlashCommand::Theme => "choose a syntax highlighting theme",
            SlashCommand::Pets => "choose or hide the terminal pet",
            SlashCommand::Ps => "list background terminals",
            SlashCommand::Model => "choose what model and reasoning effort to use",
            SlashCommand::Profile => "choose the auth profile for this session",
            SlashCommand::Provider => "choose the default model provider",
            SlashCommand::Config => "configure config.toml interactively",
            SlashCommand::Prompt => "set a session-scoped prompt for future turns",
            SlashCommand::Ide => {
                "include current selection, open files, and other context from your IDE"
            }
            SlashCommand::Personality => "choose a communication style for Codewith",
            SlashCommand::Realtime => "toggle realtime voice mode (experimental)",
            SlashCommand::Settings => "configure realtime microphone/speaker",
            SlashCommand::Plan => "switch to Plan mode",
            SlashCommand::Goal => "set or view the goal for a long-running task",
            SlashCommand::Teach => "toggle teaching callouts for future replies",
            SlashCommand::MissionControl => "show orchestration sessions and projects",
            SlashCommand::Workflow => "manage workflow specs and runs for this thread",
            SlashCommand::Loop => "schedule recurring prompts for the current thread",
            SlashCommand::Queued => "view and manage queued messages",
            SlashCommand::Schedule => "schedule and manage prompts for the current thread",
            SlashCommand::Monitor => "create and manage dynamic monitors for this thread",
            SlashCommand::Session => "switch the active session or agent thread",
            SlashCommand::Agent => "manage durable background agents",
            SlashCommand::MultiAgents => "switch the active agent thread",
            SlashCommand::BackgroundAgent => "manage durable background agents",
            SlashCommand::Worktree => "manage Codewith-managed worktrees",
            SlashCommand::Variant => "spawn variants in managed worktrees",
            SlashCommand::ExternalAgent => "stage an external coding-agent task",
            SlashCommand::Side | SlashCommand::Btw => {
                "start a side conversation in an ephemeral fork"
            }
            SlashCommand::Permissions => "choose what Codewith is allowed to do",
            SlashCommand::Keymap => "remap TUI shortcuts",
            SlashCommand::Vim => "toggle Vim mode for the composer",
            SlashCommand::ElevateSandbox => "set up elevated agent sandbox",
            SlashCommand::SandboxReadRoot => {
                "let sandbox read a directory: /sandbox-add-read-dir <absolute_path>"
            }
            SlashCommand::Experimental => "toggle experimental features",
            SlashCommand::AutoReview => "approve one retry of a recent auto-review denial",
            SlashCommand::Memories => "configure memory use and generation",
            SlashCommand::Mcp => "open the MCP control center",
            SlashCommand::Apps => "manage apps",
            SlashCommand::Webhook => "show app webhook and event inbox",
            SlashCommand::Plugins => "browse plugins",
            SlashCommand::Logout => "log out of Codewith",
            SlashCommand::Rollout => "print the rollout file path",
            SlashCommand::TestApproval => "test approval request",
        }
    }

    /// Command string without the leading '/'. Provided for compatibility with
    /// existing code that expects a method named `command()`.
    pub fn command(self) -> &'static str {
        self.into()
    }

    /// Whether this command supports inline args (for example `/review ...`).
    pub fn supports_inline_args(self) -> bool {
        matches!(
            self,
            SlashCommand::Review
                | SlashCommand::Pair
                | SlashCommand::Rename
                | SlashCommand::Prompt
                | SlashCommand::Plan
                | SlashCommand::Goal
                | SlashCommand::Teach
                | SlashCommand::Workflow
                | SlashCommand::Loop
                | SlashCommand::Queued
                | SlashCommand::Schedule
                | SlashCommand::Monitor
                | SlashCommand::Agent
                | SlashCommand::BackgroundAgent
                | SlashCommand::Worktree
                | SlashCommand::Pr
                | SlashCommand::Variant
                | SlashCommand::Recap
                | SlashCommand::Ide
                | SlashCommand::Keymap
                | SlashCommand::Mcp
                | SlashCommand::ExternalAgent
                | SlashCommand::Raw
                | SlashCommand::Pets
                | SlashCommand::Side
                | SlashCommand::Btw
                | SlashCommand::Resume
                | SlashCommand::Tmux
                | SlashCommand::SandboxReadRoot
        )
    }

    /// Whether this command remains available inside an active side conversation.
    pub fn available_in_side_conversation(self) -> bool {
        matches!(
            self,
            SlashCommand::Copy
                | SlashCommand::Raw
                | SlashCommand::Diff
                | SlashCommand::Mention
                | SlashCommand::Status
                | SlashCommand::Usage
                | SlashCommand::Stats
                | SlashCommand::Changelog
                | SlashCommand::Ide
        )
    }

    /// Whether this command can be run while a task is in progress.
    pub fn available_during_task(self) -> bool {
        match self {
            SlashCommand::New
            | SlashCommand::Archive
            | SlashCommand::Resume
            | SlashCommand::Tmux
            | SlashCommand::Fork
            | SlashCommand::Init
            | SlashCommand::Compact
            | SlashCommand::Model
            | SlashCommand::Provider
            | SlashCommand::Config
            | SlashCommand::ExternalAgent
            | SlashCommand::Personality
            | SlashCommand::Permissions
            | SlashCommand::Keymap
            | SlashCommand::Vim
            | SlashCommand::ElevateSandbox
            | SlashCommand::SandboxReadRoot
            | SlashCommand::Experimental
            | SlashCommand::Memories
            | SlashCommand::Review
            | SlashCommand::Plan
            | SlashCommand::Variant
            | SlashCommand::Clear
            | SlashCommand::Logout => false,
            SlashCommand::Diff
            | SlashCommand::Copy
            | SlashCommand::Raw
            | SlashCommand::Profile
            | SlashCommand::Prompt
            | SlashCommand::Recap
            | SlashCommand::Rename
            | SlashCommand::Mention
            | SlashCommand::Skills
            | SlashCommand::Hooks
            | SlashCommand::Status
            | SlashCommand::Usage
            | SlashCommand::Stats
            | SlashCommand::Changelog
            | SlashCommand::DebugConfig
            | SlashCommand::Pr
            | SlashCommand::Ps
            | SlashCommand::App
            | SlashCommand::Goal
            | SlashCommand::Teach
            | SlashCommand::Workflow
            | SlashCommand::MissionControl
            | SlashCommand::Loop
            | SlashCommand::Queued
            | SlashCommand::Schedule
            | SlashCommand::Monitor
            | SlashCommand::Pair
            | SlashCommand::BackgroundAgent
            | SlashCommand::Worktree
            | SlashCommand::Session
            | SlashCommand::Mcp
            | SlashCommand::Apps
            | SlashCommand::Webhook
            | SlashCommand::Plugins
            | SlashCommand::Title
            | SlashCommand::Statusline
            | SlashCommand::Summary
            | SlashCommand::AutoReview
            | SlashCommand::Feedback
            | SlashCommand::Ide
            | SlashCommand::Quit
            | SlashCommand::Exit
            | SlashCommand::Side
            | SlashCommand::Btw => true,
            SlashCommand::Rollout => true,
            SlashCommand::TestApproval => true,
            SlashCommand::Realtime => true,
            SlashCommand::Settings => true,
            SlashCommand::Agent | SlashCommand::MultiAgents => true,
            SlashCommand::Theme | SlashCommand::Pets => false,
        }
    }

    /// If this command is a backwards-compatible duplicate, return the visible command it aliases.
    pub fn hidden_alias_target(self) -> Option<SlashCommand> {
        match self {
            SlashCommand::BackgroundAgent => Some(SlashCommand::Agent),
            SlashCommand::MultiAgents => Some(SlashCommand::Session),
            SlashCommand::Exit => Some(SlashCommand::Quit),
            SlashCommand::Btw => Some(SlashCommand::Side),
            SlashCommand::Stats => Some(SlashCommand::Status),
            _ => None,
        }
    }

    pub(crate) fn is_visible(self) -> bool {
        match self {
            SlashCommand::SandboxReadRoot => cfg!(target_os = "windows"),
            SlashCommand::Copy => !cfg!(target_os = "android"),
            SlashCommand::App => cfg!(any(target_os = "macos", target_os = "windows")),
            SlashCommand::Rollout | SlashCommand::TestApproval => cfg!(debug_assertions),
            // Hidden aliases: these still parse and dispatch (see
            // `find_builtin_command`) but are kept out of the completion popup to
            // debloat it.
            command if command.hidden_alias_target().is_some() => false,
            _ => true,
        }
    }
}

/// Return all built-in commands in a Vec paired with their command string.
pub fn built_in_slash_commands() -> Vec<(&'static str, SlashCommand)> {
    SlashCommand::iter()
        .filter(|command| command.is_visible())
        .map(|c| (c.command(), c))
        .collect()
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use std::str::FromStr;

    use super::SlashCommand;

    #[test]
    fn removed_commands_no_longer_parse() {
        // Debloat: `/stop` (+ `/clean` alias) and the dead `/debug-m-*` stubs were
        // removed entirely. `/clear` is the surviving "start fresh" command.
        assert!(SlashCommand::from_str("stop").is_err());
        assert!(SlashCommand::from_str("clean").is_err());
        assert!(SlashCommand::from_str("debug-m-drop").is_err());
        assert!(SlashCommand::from_str("debug-m-update").is_err());
        assert_eq!(SlashCommand::from_str("clear"), Ok(SlashCommand::Clear));
    }

    #[test]
    fn hidden_duplicate_aliases_parse_but_are_not_listed() {
        // Hidden duplicates still dispatch but are kept out of the completion
        // popup to debloat it; their canonical twins stay visible.
        for (alias_name, alias, canonical) in [
            (
                "background-agent",
                SlashCommand::BackgroundAgent,
                SlashCommand::Agent,
            ),
            (
                "subagents",
                SlashCommand::MultiAgents,
                SlashCommand::Session,
            ),
            ("exit", SlashCommand::Exit, SlashCommand::Quit),
            ("btw", SlashCommand::Btw, SlashCommand::Side),
            ("stats", SlashCommand::Stats, SlashCommand::Status),
        ] {
            assert_eq!(SlashCommand::from_str(alias_name), Ok(alias));
            assert_eq!(alias.hidden_alias_target(), Some(canonical));
            let listed = super::built_in_slash_commands();
            assert!(
                !listed.iter().any(|(_, c)| *c == alias),
                "/{} should be hidden from the popup",
                alias.command()
            );
            assert!(
                listed.iter().any(|(_, c)| *c == canonical),
                "/{} should remain listed",
                canonical.command()
            );
        }
    }

    #[test]
    fn pet_alias_parses_to_pets_command() {
        assert_eq!(SlashCommand::Pets.command(), "pets");
        assert_eq!(SlashCommand::from_str("pet"), Ok(SlashCommand::Pets));
    }

    #[test]
    fn mcps_alias_parses_to_mcp_command() {
        assert_eq!(SlashCommand::Mcp.command(), "mcp");
        assert_eq!(SlashCommand::from_str("mcps"), Ok(SlashCommand::Mcp));
        assert_eq!(
            SlashCommand::Mcp.description(),
            "open the MCP control center"
        );
    }

    #[test]
    fn webhooks_alias_parses_to_webhook_command() {
        assert_eq!(SlashCommand::Webhook.command(), "webhook");
        assert_eq!(
            SlashCommand::from_str("webhooks"),
            Ok(SlashCommand::Webhook)
        );
        assert_eq!(
            SlashCommand::Webhook.description(),
            "show app webhook and event inbox"
        );
        assert!(SlashCommand::Webhook.available_during_task());
        assert!(!SlashCommand::Webhook.available_in_side_conversation());
        assert!(!SlashCommand::Webhook.supports_inline_args());
    }

    #[test]
    fn pr_command_has_pull_request_aliases() {
        assert_eq!(SlashCommand::Pr.command(), "pr");
        assert_eq!(SlashCommand::from_str("prs"), Ok(SlashCommand::Pr));
        assert_eq!(SlashCommand::from_str("pull-request"), Ok(SlashCommand::Pr));
        assert_eq!(
            SlashCommand::from_str("pull-requests"),
            Ok(SlashCommand::Pr)
        );
        assert_eq!(
            SlashCommand::Pr.description(),
            "inspect GitHub pull requests"
        );
        assert!(SlashCommand::Pr.available_during_task());
        assert!(!SlashCommand::Pr.available_in_side_conversation());
        assert!(SlashCommand::Pr.supports_inline_args());
    }

    #[test]
    fn teach_command_supports_session_teaching_mode_controls() {
        assert_eq!(SlashCommand::Teach.command(), "teach");
        assert_eq!(SlashCommand::from_str("teach"), Ok(SlashCommand::Teach));
        assert_eq!(
            SlashCommand::Teach.description(),
            "toggle teaching callouts for future replies"
        );
        assert!(SlashCommand::Teach.supports_inline_args());
        assert!(SlashCommand::Teach.available_during_task());
        assert!(!SlashCommand::Teach.available_in_side_conversation());
    }

    #[test]
    fn model_command_is_singular() {
        assert_eq!(SlashCommand::Model.command(), "model");
        assert!(SlashCommand::from_str("models").is_err());
    }

    #[test]
    fn provider_command_is_singular() {
        assert_eq!(SlashCommand::Provider.command(), "provider");
        assert!(SlashCommand::from_str("providers").is_err());
    }

    #[test]
    fn profile_command_is_available_as_profile() {
        assert_eq!(SlashCommand::Profile.command(), "profile");
        assert_eq!(
            SlashCommand::Profile.description(),
            "choose the auth profile for this session"
        );
        assert!(SlashCommand::Profile.available_during_task());
    }

    #[test]
    fn tmux_command_supports_inline_args_and_waits_for_idle_session() {
        assert_eq!(SlashCommand::Tmux.command(), "tmux");
        assert_eq!(SlashCommand::from_str("tmux"), Ok(SlashCommand::Tmux));
        assert!(SlashCommand::Tmux.supports_inline_args());
        assert!(!SlashCommand::Tmux.available_during_task());
    }

    #[test]
    fn config_command_is_available_as_config() {
        assert_eq!(SlashCommand::Config.command(), "config");
        assert_eq!(
            SlashCommand::Config.description(),
            "configure config.toml interactively"
        );
        assert!(!SlashCommand::Config.available_during_task());
    }

    #[test]
    fn prompt_command_is_session_scoped_and_inline_capable() {
        assert_eq!(SlashCommand::Prompt.command(), "prompt");
        assert_eq!(
            SlashCommand::Prompt.description(),
            "set a session-scoped prompt for future turns"
        );
        assert!(SlashCommand::Prompt.supports_inline_args());
        assert!(SlashCommand::Prompt.available_during_task());
        assert!(!SlashCommand::Prompt.available_in_side_conversation());
        assert_eq!(SlashCommand::from_str("session"), Ok(SlashCommand::Session));
    }

    #[test]
    fn summary_command_configures_final_message_summary() {
        assert_eq!(SlashCommand::Summary.command(), "summary");
        assert_eq!(
            SlashCommand::Summary.description(),
            "configure what appears after final messages"
        );
        assert!(!SlashCommand::Summary.supports_inline_args());
        assert!(SlashCommand::Summary.available_during_task());
        assert!(!SlashCommand::Summary.available_in_side_conversation());
        assert!(
            super::built_in_slash_commands()
                .iter()
                .any(|(name, command)| *name == "summary" && *command == SlashCommand::Summary)
        );
    }

    #[test]
    fn certain_commands_are_available_during_task() {
        assert!(SlashCommand::Goal.available_during_task());
        assert!(SlashCommand::Workflow.available_during_task());
        assert!(SlashCommand::Queued.available_during_task());
        assert!(SlashCommand::Queued.supports_inline_args());
        assert!(!SlashCommand::Variant.available_during_task());
        assert!(SlashCommand::Ide.available_during_task());
        assert_eq!(SlashCommand::from_str("usage"), Ok(SlashCommand::Usage));
        assert_eq!(
            SlashCommand::Usage.description(),
            "show usage, context, and rate limits"
        );
        assert!(SlashCommand::Usage.available_during_task());
        assert!(SlashCommand::Usage.available_in_side_conversation());
        assert!(!SlashCommand::Usage.supports_inline_args());
        assert!(
            super::built_in_slash_commands()
                .iter()
                .any(|(name, command)| *name == "usage" && *command == SlashCommand::Usage)
        );
        assert!(SlashCommand::Stats.available_during_task());
        assert!(SlashCommand::Stats.available_in_side_conversation());
        assert!(SlashCommand::Changelog.available_during_task());
        assert!(SlashCommand::Changelog.available_in_side_conversation());
        assert!(SlashCommand::Title.available_during_task());
        assert!(SlashCommand::Statusline.available_during_task());
        assert!(SlashCommand::Summary.available_during_task());
        assert!(SlashCommand::MissionControl.available_during_task());
        assert!(SlashCommand::Raw.available_during_task());
        assert!(SlashCommand::Raw.available_in_side_conversation());
        assert!(SlashCommand::Raw.supports_inline_args());
        assert!(SlashCommand::Teach.available_during_task());
        assert!(SlashCommand::Teach.supports_inline_args());
        assert!(SlashCommand::Recap.supports_inline_args());
        assert!(SlashCommand::App.available_during_task());
        assert!(SlashCommand::Webhook.available_during_task());
    }

    #[test]
    fn requested_running_task_slash_command_matrix_is_explicit() {
        let available = [
            SlashCommand::Profile,
            SlashCommand::Goal,
            SlashCommand::Loop,
            SlashCommand::Queued,
            SlashCommand::Workflow,
            SlashCommand::MissionControl,
            SlashCommand::Pr,
            SlashCommand::Teach,
            SlashCommand::Webhook,
            SlashCommand::Worktree,
            SlashCommand::Status,
            SlashCommand::Usage,
            SlashCommand::Statusline,
            SlashCommand::Summary,
            SlashCommand::Changelog,
        ];
        for command in available {
            assert!(
                command.available_during_task(),
                "/{} should be available while a task is running",
                command.command()
            );
        }

        let unavailable = [
            SlashCommand::Permissions,
            SlashCommand::Model,
            SlashCommand::Provider,
            SlashCommand::Config,
            SlashCommand::Resume,
            SlashCommand::Tmux,
            SlashCommand::Variant,
        ];
        for command in unavailable {
            assert!(
                !command.available_during_task(),
                "/{} should wait for an idle task",
                command.command()
            );
        }

        assert_eq!(
            SlashCommand::from_str("changelog"),
            Ok(SlashCommand::Changelog)
        );
    }

    #[test]
    fn mission_control_command_uses_kebab_case_and_aliases() {
        assert_eq!(SlashCommand::MissionControl.command(), "mission-control");
        assert_eq!(
            SlashCommand::from_str("mission"),
            Ok(SlashCommand::MissionControl)
        );
        assert_eq!(
            SlashCommand::from_str("missions"),
            Ok(SlashCommand::MissionControl)
        );
        assert_eq!(
            SlashCommand::MissionControl.description(),
            "show orchestration sessions and projects"
        );
    }

    #[test]
    fn pair_command_supports_inline_watcher_prompt() {
        assert_eq!(SlashCommand::Pair.command(), "pair");
        assert_eq!(
            SlashCommand::Pair.description(),
            "start a paired watcher agent for this session"
        );
        assert!(SlashCommand::Pair.supports_inline_args());
        assert!(SlashCommand::Pair.available_during_task());
        assert!(!SlashCommand::Pair.available_in_side_conversation());
        assert!(
            super::built_in_slash_commands()
                .iter()
                .any(|(name, command)| *name == "pair" && *command == SlashCommand::Pair)
        );
    }

    #[test]
    fn external_agent_command_uses_kebab_case_and_args() {
        assert_eq!(SlashCommand::ExternalAgent.command(), "external-agent");
        assert_eq!(
            SlashCommand::ExternalAgent.description(),
            "stage an external coding-agent task"
        );
        assert!(SlashCommand::ExternalAgent.supports_inline_args());
        assert!(!SlashCommand::ExternalAgent.available_during_task());
    }

    #[test]
    fn variant_command_supports_inline_args_and_waits_for_idle_session() {
        assert_eq!(SlashCommand::Variant.command(), "variant");
        assert_eq!(SlashCommand::from_str("variant"), Ok(SlashCommand::Variant));
        assert_eq!(
            SlashCommand::Variant.description(),
            "spawn variants in managed worktrees"
        );
        assert!(SlashCommand::Variant.supports_inline_args());
        assert!(!SlashCommand::Variant.available_during_task());
    }

    #[test]
    fn background_agent_command_uses_kebab_case_aliases_and_args() {
        assert_eq!(SlashCommand::BackgroundAgent.command(), "background-agent");
        assert_eq!(
            SlashCommand::from_str("background-agents"),
            Ok(SlashCommand::BackgroundAgent)
        );
        assert_eq!(
            SlashCommand::from_str("bg-agent"),
            Ok(SlashCommand::BackgroundAgent)
        );
        assert_eq!(
            SlashCommand::BackgroundAgent.description(),
            "manage durable background agents"
        );
        assert!(SlashCommand::BackgroundAgent.supports_inline_args());
        assert!(SlashCommand::BackgroundAgent.available_during_task());
        assert!(
            !super::built_in_slash_commands()
                .iter()
                .any(|(name, _)| *name == "background-agent")
        );
    }

    #[test]
    fn session_command_is_visible_thread_switcher() {
        assert_eq!(SlashCommand::Session.command(), "session");
        assert_eq!(
            SlashCommand::Session.description(),
            "switch the active session or agent thread"
        );
        assert_eq!(
            SlashCommand::from_str("sessions"),
            Ok(SlashCommand::Session)
        );
        assert_eq!(SlashCommand::from_str("thread"), Ok(SlashCommand::Session));
        assert_eq!(SlashCommand::from_str("threads"), Ok(SlashCommand::Session));
        assert!(SlashCommand::Session.available_during_task());
        assert!(
            super::built_in_slash_commands()
                .iter()
                .any(|(name, command)| *name == "session" && *command == SlashCommand::Session)
        );
    }

    #[test]
    fn subagents_alias_parses_but_is_hidden_from_visible_commands() {
        assert_eq!(
            SlashCommand::from_str("subagents"),
            Ok(SlashCommand::MultiAgents)
        );
        assert!(
            !super::built_in_slash_commands()
                .iter()
                .any(|(name, _)| *name == "subagents")
        );
        assert!(
            super::built_in_slash_commands()
                .iter()
                .any(|(name, command)| *name == "session" && *command == SlashCommand::Session)
        );
    }

    #[test]
    fn auto_review_command_is_approve() {
        assert_eq!(SlashCommand::AutoReview.command(), "approve");
        assert_eq!(
            SlashCommand::from_str("approve"),
            Ok(SlashCommand::AutoReview)
        );
    }

    #[test]
    fn workflow_command_is_singular_and_accepts_args() {
        assert_eq!(SlashCommand::Workflow.command(), "workflow");
        assert_eq!(
            SlashCommand::from_str("workflows"),
            Ok(SlashCommand::Workflow)
        );
        assert_eq!(
            SlashCommand::Workflow.description(),
            "manage workflow specs and runs for this thread"
        );
        assert!(SlashCommand::Workflow.supports_inline_args());
        assert!(SlashCommand::Workflow.available_during_task());
    }

    #[test]
    fn changelog_command_is_visible_session_information() {
        assert_eq!(SlashCommand::Changelog.command(), "changelog");
        assert_eq!(
            SlashCommand::Changelog.description(),
            "show what changed in Codewith releases"
        );
        assert!(SlashCommand::Changelog.available_during_task());
        assert!(SlashCommand::Changelog.available_in_side_conversation());
    }
}
