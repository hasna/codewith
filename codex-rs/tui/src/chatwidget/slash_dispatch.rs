//! Slash-command dispatch and local-recall handoff for `ChatWidget`.
//!
//! `ChatComposer` parses slash input and stages recognized command text for local
//! Up-arrow recall before returning an input result. This module owns the app-level
//! dispatch step and records the staged entry once the command has been handled, so
//! slash-command recall follows the same submitted-input rule as ordinary text.

use super::goal_validation::GoalObjectiveValidationSource;
use super::loop_slash::LoopCreateRequest;
use super::loop_slash::LoopIntervalUnit;
use super::loop_slash::LoopManageCommand;
use super::loop_slash::LoopPrompt;
use super::loop_slash::LoopSchedule;
use super::loop_slash::LoopSlashCommand;
use super::loop_slash::ScheduleTime;
use super::loop_slash::parse_loop_slash_args;
use super::loop_slash::parse_schedule_slash_args;
use super::workflow_slash::WORKFLOW_USAGE;
use super::workflow_slash::WORKFLOW_USAGE_HINT;
use super::workflow_slash::WorkflowSlashCommand;
use super::workflow_slash::parse_workflow_slash_args;
use super::workflow_slash::workflow_generation_prompt;
use super::*;
use crate::app_event::MiniMaxUsageRefreshOrigin;
use crate::app_event::ThreadGoalSetMode;
use crate::app_event::ThreadWorkflowAction;
use crate::bottom_pane::prompt_args::parse_slash_name;
use crate::bottom_pane::slash_commands::BuiltinCommandFlags;
use crate::bottom_pane::slash_commands::ServiceTierCommand;
use crate::bottom_pane::slash_commands::SlashCommandItem;
use crate::bottom_pane::slash_commands::find_slash_command;
use crate::external_agents::external_agent_picker_params;
use crate::goal_display::GOAL_USAGE;
use crate::tmux_handoff::TmuxHandoffDestination;
use chrono::Utc;
use codex_app_server_protocol::ThreadExternalAgentMode;
use codex_app_server_protocol::ThreadScheduleIntervalUnit as ApiThreadScheduleIntervalUnit;
use codex_app_server_protocol::ThreadSchedulePromptSource;
use codex_app_server_protocol::ThreadScheduleSpec;
use codex_external_agent::ExternalAgentReadinessStatus;
use codex_external_agent::external_agent_runtime_readiness;
use codex_external_agent::find_external_agent_runtime;
use codex_model_provider_info::MINIMAX_PROVIDER_ID;
use codex_protocol::config_types::SERVICE_TIER_DEFAULT_REQUEST_VALUE;

const DEFAULT_LOOP_PROMPT_DISPLAY: &str = "Default loop prompt";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SlashCommandDispatchSource {
    Live,
    Queued,
}

struct PreparedSlashCommandArgs {
    args: String,
    text_elements: Vec<TextElement>,
    local_images: Vec<LocalImageAttachment>,
    remote_image_urls: Vec<String>,
    mention_bindings: Vec<MentionBinding>,
    source: SlashCommandDispatchSource,
}

#[derive(Debug, PartialEq, Eq)]
enum MonitorSlashCommand {
    Create,
    Manage(MonitorManageCommand),
}

#[derive(Debug, PartialEq, Eq)]
enum MonitorManageCommand {
    List,
    Read { monitor_id: Option<String> },
    Stop { monitor_id: Option<String> },
    Restart { monitor_id: Option<String> },
    Delete { monitor_id: Option<String> },
}

#[derive(Debug, PartialEq, Eq)]
enum BackgroundAgentSlashCommand {
    List,
    Start {
        prompt: String,
        worktree_id: Option<String>,
    },
    Read {
        agent_id: Option<String>,
    },
    Logs {
        agent_id: Option<String>,
    },
    Attach {
        agent_id: Option<String>,
    },
    Detach {
        agent_id: Option<String>,
    },
    Stop {
        agent_id: Option<String>,
    },
    Delete {
        agent_id: Option<String>,
    },
    Diagnostics,
}

#[derive(Debug, PartialEq, Eq)]
enum WorktreeSlashCommand {
    List,
    Reconcile,
    Create {
        name: Option<String>,
        branch: Option<String>,
        start_point: Option<String>,
    },
    Read {
        worktree_id: Option<String>,
    },
    Actions {
        worktree_id: String,
    },
    Use {
        worktree_id: String,
    },
    Release {
        worktree_id: String,
    },
    Cleanup {
        worktree_id: String,
        force_delete: bool,
    },
    Merge {
        worktree_id: String,
        target_ref: Option<String>,
    },
}

#[derive(Debug, PartialEq, Eq)]
enum ActiveSessionSlashCommand {
    List,
    Send {
        target_peer_id: String,
        message: String,
        wake: bool,
    },
}

#[derive(Debug, PartialEq, Eq)]
struct MonitorSlashParseError {
    message: String,
    hint: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
struct TmuxSlashCommand {
    destination: TmuxHandoffDestination,
    replace_existing: bool,
}

const SIDE_STARTING_CONTEXT_LABEL: &str = "Side starting...";
const SIDE_SLASH_COMMAND_UNAVAILABLE_HINT: &str =
    "Press Ctrl+C to return to the main thread first.";
const GOAL_USAGE_HINT: &str = "Example: /goal improve benchmark coverage";
const LOOP_USAGE: &str = "Usage: /loop [interval|cron] <prompt>";
const LOOP_USAGE_HINT: &str =
    "Examples: /loop 5m check CI, /loop every 2 hours review alerts, /loop list, /loop stats <id>";
const SCHEDULE_USAGE: &str = "Usage: /schedule <time> <prompt>";
const SCHEDULE_USAGE_HINT: &str = "Examples: /schedule 5m check CI, /schedule 2026-06-05 09:30 check CI, /schedule tomorrow at 9am review alerts, /schedule list, /schedule stats <id>";
const MONITOR_USAGE: &str =
    "Usage: /monitor <request> | /monitor [list|read|stop|restart|delete] [id]";
const MONITOR_USAGE_HINT: &str =
    "Examples: /monitor watch CI, /monitor list, /monitor read mon-123";
const BACKGROUND_AGENT_USAGE: &str = "Usage: /agent [peers|send [--wake] <peer-id> <message>|list|diagnostics|start [--worktree <id>] <prompt>|read|attach|detach|stop|delete] [id]";
const BACKGROUND_AGENT_USAGE_HINT: &str = "Examples: /agent peers, /agent send <peer-id> hello, /agent start fix the flaky test, /agent start --worktree wt-123 fix tests";
const ACTIVE_SESSION_SEND_USAGE: &str = "Usage: /agent send [--wake] <peer-id> <message>";
const WORKTREE_USAGE: &str =
    "Usage: /worktree [list|create|reconcile|read|actions|use|release|cleanup|merge] [args]";
const WORKTREE_USAGE_HINT: &str = "Examples: /worktree create feature, /worktree read <id>, /worktree use <id>, /worktree cleanup <id>, /worktree merge <id> [target]";
const RAW_USAGE: &str = "Usage: /raw [on|off]";
const EXTERNAL_AGENT_USAGE: &str =
    "Usage: /external-agent [inline|--inline] [plan|propose] [cursor|grok-build|claude] [task]";
const TMUX_USAGE: &str = "Usage: /tmux [--replace|--no-replace] [session-name] | /tmux --session <session> [--window <window>]";

#[derive(Debug, PartialEq, Eq)]
enum ExternalAgentSlashCommand<'a> {
    Runtime {
        runtime_id: &'a str,
        prompt: &'a str,
        mode: ThreadExternalAgentMode,
        inline: bool,
    },
}

fn split_external_agent_token(input: &str) -> Option<(&str, &str)> {
    let input = input.trim_start();
    if input.is_empty() {
        return None;
    }
    let token_end = input.find(char::is_whitespace).unwrap_or(input.len());
    let (token, rest) = input.split_at(token_end);
    Some((token, rest.trim_start()))
}

fn parse_external_agent_mode(token: &str) -> Option<ThreadExternalAgentMode> {
    match token {
        "plan" => Some(ThreadExternalAgentMode::Plan),
        "propose" => Some(ThreadExternalAgentMode::Propose),
        _ => None,
    }
}

fn parse_external_agent_options(
    mut input: &str,
    mut mode: ThreadExternalAgentMode,
    allow_inline: bool,
) -> Option<(ThreadExternalAgentMode, bool, &str)> {
    let mut inline = false;
    loop {
        let Some((token, rest)) = split_external_agent_token(input) else {
            return Some((mode, inline, input.trim()));
        };
        if let Some(mode_token) = token.strip_prefix("--mode=") {
            mode = parse_external_agent_mode(mode_token)?;
            input = rest;
            continue;
        }
        match token {
            "inline" | "--inline" if allow_inline => {
                inline = true;
                input = rest;
            }
            "child" | "--child" if allow_inline => {
                inline = false;
                input = rest;
            }
            "plan" | "--plan" => {
                mode = ThreadExternalAgentMode::Plan;
                input = rest;
            }
            "propose" | "--propose" => {
                mode = ThreadExternalAgentMode::Propose;
                input = rest;
            }
            "--mode" => {
                let (mode_token, mode_rest) = split_external_agent_token(rest)?;
                mode = parse_external_agent_mode(mode_token)?;
                input = mode_rest;
            }
            _ => return Some((mode, inline, input.trim())),
        }
    }
}

fn parse_external_agent_args(trimmed: &str) -> Option<ExternalAgentSlashCommand<'_>> {
    let trimmed = trimmed.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (mode, inline, rest) =
        parse_external_agent_options(trimmed, ThreadExternalAgentMode::Plan, true)?;
    let (head, rest) = split_external_agent_token(rest)?;
    Some(ExternalAgentSlashCommand::Runtime {
        runtime_id: head,
        prompt: rest.trim(),
        mode,
        inline,
    })
}

fn external_agent_mode_label(mode: ThreadExternalAgentMode) -> &'static str {
    match mode {
        ThreadExternalAgentMode::Plan => "plan",
        ThreadExternalAgentMode::Propose => "propose",
    }
}

fn resolve_visible_external_agent_runtime(
    runtime_id: &str,
) -> Result<&'static codex_external_agent::ExternalAgentRuntimeDescriptor, String> {
    if runtime_id == "grok" {
        return Err("Use `/external-agent grok-build` for Grok Build.".to_string());
    }
    find_external_agent_runtime(runtime_id)
        .filter(|runtime| runtime.visible)
        .ok_or_else(|| EXTERNAL_AGENT_USAGE.to_string())
}

fn parse_tmux_slash_args(input: &str) -> Result<TmuxSlashCommand, String> {
    let Some(parts) = shlex::split(input) else {
        return Err(format!("{TMUX_USAGE}. Check quotes in the tmux names."));
    };
    let mut replace_existing = true;
    let mut session_name: Option<String> = None;
    let mut window_name: Option<String> = None;
    let mut name_parts = Vec::new();

    let mut parts = parts.into_iter();
    while let Some(part) = parts.next() {
        match part.as_str() {
            "--replace" | "-r" => replace_existing = true,
            "--no-replace" => replace_existing = false,
            "--session" | "-s" => {
                let Some(value) = parts.next() else {
                    return Err(format!("Missing value for `{part}`. {TMUX_USAGE}"));
                };
                if session_name.replace(value).is_some() {
                    return Err(format!("Duplicate /tmux session target. {TMUX_USAGE}"));
                }
            }
            option if option.starts_with("--session=") => {
                let value = option["--session=".len()..].to_string();
                if session_name.replace(value).is_some() {
                    return Err(format!("Duplicate /tmux session target. {TMUX_USAGE}"));
                }
            }
            "--window" | "-w" => {
                let Some(value) = parts.next() else {
                    return Err(format!("Missing value for `{part}`. {TMUX_USAGE}"));
                };
                if window_name.replace(value).is_some() {
                    return Err(format!("Duplicate /tmux window target. {TMUX_USAGE}"));
                }
            }
            option if option.starts_with("--window=") => {
                let value = option["--window=".len()..].to_string();
                if window_name.replace(value).is_some() {
                    return Err(format!("Duplicate /tmux window target. {TMUX_USAGE}"));
                }
            }
            "--help" | "-h" => return Err(TMUX_USAGE.to_string()),
            option if option.starts_with('-') => {
                return Err(format!("Unknown /tmux option `{option}`. {TMUX_USAGE}"));
            }
            _ => name_parts.push(part),
        }
    }
    let destination = if let Some(session_name) = session_name {
        if !name_parts.is_empty() {
            return Err(format!(
                "Do not combine `--session` with a positional session name. {TMUX_USAGE}"
            ));
        }
        TmuxHandoffDestination::ExistingSession {
            session_name,
            window_name,
        }
    } else {
        if window_name.is_some() {
            return Err(format!("`--window` requires `--session`. {TMUX_USAGE}"));
        }
        TmuxHandoffDestination::NewSession {
            name: (!name_parts.is_empty()).then(|| name_parts.join(" ")),
        }
    };
    Ok(TmuxSlashCommand {
        destination,
        replace_existing,
    })
}

impl ChatWidget {
    /// Dispatch a bare slash command and record its staged local-history entry.
    ///
    /// The composer stages history before returning `InputResult::Command`; this wrapper commits
    /// that staged entry after dispatch so slash-command recall follows the same "submitted input"
    /// rule as normal text.
    pub(super) fn handle_slash_command_dispatch(&mut self, cmd: SlashCommand) {
        self.dispatch_command(cmd);
        if matches!(
            cmd,
            SlashCommand::Goal
                | SlashCommand::MissionControl
                | SlashCommand::Loop
                | SlashCommand::Schedule
                | SlashCommand::Monitor
                | SlashCommand::Workflow
                | SlashCommand::Session
                | SlashCommand::MultiAgents
                | SlashCommand::Agent
                | SlashCommand::BackgroundAgent
                | SlashCommand::Worktree
                | SlashCommand::ExternalAgent
        ) {
            self.bottom_pane.drain_pending_submission_state();
        }
        self.bottom_pane.record_pending_slash_command_history();
    }

    pub(super) fn handle_service_tier_command_dispatch(&mut self, command: ServiceTierCommand) {
        if self.active_side_conversation {
            self.add_error_message(format!(
                "'/{}' is unavailable in side conversations. {SIDE_SLASH_COMMAND_UNAVAILABLE_HINT}",
                command.name
            ));
            self.bottom_pane.drain_pending_submission_state();
            self.bottom_pane.record_pending_slash_command_history();
            return;
        }
        self.toggle_service_tier_from_ui(command);
        self.bottom_pane.record_pending_slash_command_history();
    }

    pub(super) fn handle_service_tier_command_with_args_dispatch(
        &mut self,
        command: ServiceTierCommand,
        args: String,
    ) {
        if self.active_side_conversation {
            self.add_error_message(format!(
                "'/{}' is unavailable in side conversations. {SIDE_SLASH_COMMAND_UNAVAILABLE_HINT}",
                command.name
            ));
            self.bottom_pane.drain_pending_submission_state();
            self.bottom_pane.record_pending_slash_command_history();
            return;
        }

        match parse_service_tier_state_arg(&args) {
            Some(ServiceTierStateArg::On) => {
                self.set_service_tier_selection(Some(command.id));
            }
            Some(ServiceTierStateArg::Off) => {
                self.set_service_tier_selection(Some(
                    SERVICE_TIER_DEFAULT_REQUEST_VALUE.to_string(),
                ));
            }
            None => {
                let command_name = command.name;
                self.add_error_message(format!("Unrecognized /{command_name} option: {args}"));
                self.add_info_message(
                    format!("Usage: /{command_name} [on|off]"),
                    Some(format!(
                        "Examples: /{command_name}, /{command_name} on, /{command_name} off"
                    )),
                );
            }
        }
        self.bottom_pane.drain_pending_submission_state();
        self.bottom_pane.record_pending_slash_command_history();
    }

    /// Dispatch an inline slash command and record its staged local-history entry.
    ///
    /// Inline command arguments may later be prepared through the normal submission pipeline, but
    /// local command recall still tracks the original command invocation. Treating this wrapper as
    /// the only input-result entry point avoids double-recording commands with inline args.
    pub(super) fn handle_slash_command_with_args_dispatch(
        &mut self,
        cmd: SlashCommand,
        args: String,
        text_elements: Vec<TextElement>,
    ) {
        self.dispatch_command_with_args(cmd, args, text_elements);
        self.bottom_pane.record_pending_slash_command_history();
    }

    fn apply_plan_slash_command(&mut self) -> bool {
        if !self.collaboration_modes_enabled() {
            self.add_info_message(
                "Collaboration modes are disabled.".to_string(),
                Some("Enable collaboration modes to use /plan.".to_string()),
            );
            return false;
        }
        if let Some(mask) = collaboration_modes::plan_mask(self.model_catalog.as_ref()) {
            self.set_collaboration_mask_from_user_action(mask);
            true
        } else {
            self.add_info_message(
                "Plan mode unavailable right now.".to_string(),
                /*hint*/ None,
            );
            false
        }
    }

    fn request_side_conversation(
        &mut self,
        parent_thread_id: ThreadId,
        user_message: Option<UserMessage>,
    ) {
        self.set_side_conversation_context_label(Some(SIDE_STARTING_CONTEXT_LABEL.to_string()));
        self.request_redraw();
        self.app_event_tx.send(AppEvent::StartSide {
            parent_thread_id,
            user_message,
        });
    }

    fn request_empty_side_conversation(&mut self, cmd: SlashCommand) {
        let Some(parent_thread_id) = self.thread_id else {
            let command = cmd.command();
            self.add_error_message(format!(
                "'/{command}' is unavailable before the session starts."
            ));
            return;
        };

        self.request_side_conversation(parent_thread_id, /*user_message*/ None);
    }

    fn emit_raw_output_mode_changed(&self, enabled: bool) {
        self.app_event_tx
            .send(AppEvent::RawOutputModeChanged { enabled });
    }

    fn next_status_request_id(&mut self) -> u64 {
        let request_id = self.next_status_refresh_request_id;
        self.next_status_refresh_request_id = self.next_status_refresh_request_id.wrapping_add(1);
        request_id
    }

    fn is_minimax_provider_active(&self) -> bool {
        self.config
            .model_provider_id
            .eq_ignore_ascii_case(MINIMAX_PROVIDER_ID)
    }

    fn dispatch_status_command(&mut self, command_label: &'static str) {
        let rate_limit_request_id = if self.should_prefetch_rate_limits() {
            Some(self.next_status_request_id())
        } else {
            None
        };
        let minimax_request_id = if self.is_minimax_provider_active() {
            Some(self.next_status_request_id())
        } else {
            None
        };

        self.add_status_output_with_command(
            command_label,
            rate_limit_request_id.is_some(),
            rate_limit_request_id,
            minimax_request_id.is_some(),
            minimax_request_id.is_some(),
            minimax_request_id,
        );

        if let Some(request_id) = rate_limit_request_id {
            self.app_event_tx.send(AppEvent::RefreshRateLimits {
                origin: RateLimitRefreshOrigin::StatusCommand { request_id },
                target: RateLimitRefreshTarget::Selected,
            });
        }
        if let Some(request_id) = minimax_request_id {
            self.app_event_tx.send(AppEvent::RefreshMiniMaxUsage {
                origin: MiniMaxUsageRefreshOrigin::StatusCommand { request_id },
            });
        }
    }

    fn open_external_agent_picker(&mut self) {
        self.bottom_pane
            .show_selection_view(external_agent_picker_params());
    }

    fn handle_external_agent_command_args(&mut self, trimmed: &str) {
        let Some(command) = parse_external_agent_args(trimmed) else {
            self.add_error_message(EXTERNAL_AGENT_USAGE.to_string());
            return;
        };
        match command {
            ExternalAgentSlashCommand::Runtime {
                runtime_id,
                prompt,
                mode,
                inline,
            } => {
                let runtime = match resolve_visible_external_agent_runtime(runtime_id) {
                    Ok(runtime) => runtime,
                    Err(message) => {
                        self.add_error_message(message);
                        return;
                    }
                };
                let readiness = external_agent_runtime_readiness(runtime);
                let readiness_status = match readiness.status {
                    ExternalAgentReadinessStatus::Ready => "command ready",
                    ExternalAgentReadinessStatus::MissingRuntime => "missing command",
                    ExternalAgentReadinessStatus::MissingAuth => "missing auth",
                    ExternalAgentReadinessStatus::Unsupported => "unsupported",
                    ExternalAgentReadinessStatus::Disabled => "disabled",
                };
                let command = if runtime.command.args.is_empty() {
                    runtime.command.program.to_string()
                } else {
                    format!(
                        "{} {}",
                        runtime.command.program,
                        runtime.command.args.join(" ")
                    )
                };
                if prompt.is_empty() {
                    self.add_info_message(
                        format!(
                            "External agent runtime `{runtime_id}` selected ({readiness_status})."
                        ),
                        Some(format!(
                            "Command: {command}. Add a task after the runtime id to stage it."
                        )),
                    );
                } else {
                    if self.thread_id.is_none() {
                        self.add_error_message(
                            "'/external-agent' is unavailable before the session starts."
                                .to_string(),
                        );
                        return;
                    }
                    if inline {
                        self.submit_op(AppCommand::start_external_agent(
                            runtime_id.to_string(),
                            prompt.to_string(),
                            mode,
                        ));
                        self.add_info_message(
                            format!(
                                "{} external-agent task routed inline.",
                                runtime.display_name
                            ),
                            Some(format!(
                                "Mode: {}. The run stays in the current thread.",
                                external_agent_mode_label(mode)
                            )),
                        );
                    } else {
                        self.app_event_tx
                            .send(AppEvent::StartExternalAgentChildThread {
                                runtime_id: runtime_id.to_string(),
                                runtime_display_name: runtime.display_name.to_string(),
                                task: prompt.to_string(),
                                mode,
                            });
                        self.add_info_message(
                            format!(
                                "{} external-agent child thread requested.",
                                runtime.display_name
                            ),
                            Some(format!(
                                "Mode: {}. A linked agent thread will open for the run.",
                                external_agent_mode_label(mode)
                            )),
                        );
                    }
                }
            }
        }
    }

    pub(super) fn dispatch_command(&mut self, cmd: SlashCommand) {
        if !self.ensure_slash_command_allowed_in_side_conversation(cmd) {
            return;
        }
        if !self.ensure_side_command_allowed_outside_review(cmd) {
            return;
        }
        if !cmd.available_during_task() && self.bottom_pane.is_task_running() {
            let message = format!(
                "'/{}' is disabled while a task is in progress.",
                cmd.command()
            );
            self.add_to_history(history_cell::new_error_event(message));
            self.bottom_pane.drain_pending_submission_state();
            self.request_redraw();
            return;
        }

        match cmd {
            SlashCommand::Feedback => {
                if !self.config.feedback_enabled {
                    let params = crate::bottom_pane::feedback_disabled_params();
                    self.bottom_pane.show_selection_view(params);
                    self.request_redraw();
                    return;
                }
                // Step 1: pick a category (UI built in feedback_view)
                let params =
                    crate::bottom_pane::feedback_selection_params(self.app_event_tx.clone());
                self.bottom_pane.show_selection_view(params);
                self.request_redraw();
            }
            SlashCommand::New => {
                self.app_event_tx.send(AppEvent::NewSession);
            }
            SlashCommand::Archive => {
                self.bottom_pane.show_selection_view(SelectionViewParams {
                    title: Some("Archive this session?".to_string()),
                    subtitle: Some(
                        "Are you sure? This will archive the current session and exit Codewith"
                            .to_string(),
                    ),
                    footer_hint: Some(standard_popup_hint_line()),
                    items: vec![
                        SelectionItem {
                            name: "No, don't archive".to_string(),
                            description: Some("Return to the current session".to_string()),
                            dismiss_on_select: true,
                            ..Default::default()
                        },
                        SelectionItem {
                            name: "Yes, archive and exit".to_string(),
                            description: Some("Archive this session now".to_string()),
                            actions: vec![Box::new(|tx| {
                                tx.send(AppEvent::ArchiveCurrentThread);
                            })],
                            dismiss_on_select: true,
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                });
                self.request_redraw();
            }
            SlashCommand::Clear => {
                self.app_event_tx.send(AppEvent::ClearUi);
            }
            SlashCommand::Resume => {
                self.app_event_tx.send(AppEvent::OpenResumePicker);
            }
            SlashCommand::Tmux => {
                self.app_event_tx.send(AppEvent::OpenInTmux {
                    destination: TmuxHandoffDestination::default(),
                    replace_existing: true,
                });
            }
            SlashCommand::Fork => {
                if self.thread_id.is_some() && self.current_rollout_path.is_none() {
                    self.add_error_message(
                        "This session is still starting and cannot be forked yet. Send a message first, then try /fork again."
                            .to_string(),
                    );
                    return;
                }
                self.app_event_tx.send(AppEvent::ForkCurrentSession);
            }
            SlashCommand::App => {
                let Some(thread_id) = self.thread_id else {
                    self.add_error_message(
                        "Session is still starting; try /app again in a moment.".to_string(),
                    );
                    return;
                };
                self.app_event_tx
                    .send(AppEvent::OpenDesktopThread { thread_id });
            }
            SlashCommand::Init => {
                let init_target = self.config.cwd.join(DEFAULT_PROJECT_AGENTS_MD_PATH);
                if init_target.exists() {
                    let message = format!(
                        "{DEFAULT_PROJECT_AGENTS_MD_PATH} already exists here. Skipping /init to avoid overwriting it."
                    );
                    self.add_info_message(message, /*hint*/ None);
                    return;
                }
                const INIT_PROMPT: &str = include_str!("../../prompt_for_init_command.md");
                self.submit_user_message(INIT_PROMPT.to_string().into());
            }
            SlashCommand::Compact => {
                self.clear_token_usage();
                if !self.bottom_pane.is_task_running() {
                    self.bottom_pane.set_task_running(/*running*/ true);
                }
                self.app_event_tx.compact();
            }
            SlashCommand::Recap => {
                self.dispatch_recap_slash_command(/*prompt*/ None);
            }
            SlashCommand::Review => {
                self.open_review_popup();
            }
            SlashCommand::Rename => {
                self.session_telemetry
                    .counter("codex.thread.rename", /*inc*/ 1, &[]);
                self.show_rename_prompt();
            }
            SlashCommand::Model => {
                self.open_model_popup();
            }
            SlashCommand::Profile => {
                self.open_profile_popup();
            }
            SlashCommand::Provider => {
                self.open_provider_popup();
            }
            SlashCommand::Config => {
                self.open_config_popup();
            }
            SlashCommand::Realtime => {
                if !self.realtime_conversation_enabled() {
                    return;
                }
                if self.realtime_conversation.is_live() {
                    self.stop_realtime_conversation_from_ui();
                } else {
                    self.start_realtime_conversation();
                }
            }
            SlashCommand::Settings => {
                if !self.realtime_audio_device_selection_enabled() {
                    return;
                }
                self.open_realtime_audio_popup();
            }
            SlashCommand::Personality => {
                self.open_personality_popup();
            }
            SlashCommand::Plan => {
                self.apply_plan_slash_command();
            }
            SlashCommand::Goal => {
                if !self.config.features.enabled(Feature::Goals) {
                    return;
                }
                if let Some(thread_id) = self.thread_id {
                    self.app_event_tx
                        .send(AppEvent::OpenThreadGoalMenu { thread_id });
                    self.append_message_history_entry("/goal".to_string());
                } else {
                    self.add_info_message(
                        GOAL_USAGE.to_string(),
                        Some(GOAL_USAGE_HINT.to_string()),
                    );
                }
            }
            SlashCommand::MissionControl => {
                self.app_event_tx.send(AppEvent::OpenMissionControlOverview);
                self.append_message_history_entry("/mission-control".to_string());
            }
            SlashCommand::Workflow => {
                if !self.config.features.enabled(Feature::Workflows) {
                    return;
                }
                if let Some(thread_id) = self.thread_id {
                    self.app_event_tx.send(AppEvent::ManageThreadWorkflow {
                        thread_id,
                        action: ThreadWorkflowAction::List,
                    });
                    self.append_message_history_entry("/workflow".to_string());
                } else {
                    self.add_info_message(
                        WORKFLOW_USAGE.to_string(),
                        Some(WORKFLOW_USAGE_HINT.to_string()),
                    );
                }
            }
            SlashCommand::Worktree => {
                self.app_event_tx.send(AppEvent::OpenWorktreeManager);
                self.append_message_history_entry("/worktree".to_string());
            }
            SlashCommand::Loop => {
                if !self.config.features.enabled(Feature::ScheduledTasks) {
                    return;
                }
                if let Some(thread_id) = self.thread_id {
                    self.app_event_tx
                        .send(AppEvent::OpenThreadLoopManager { thread_id });
                    self.append_message_history_entry("/loop".to_string());
                } else {
                    self.add_info_message(
                        LOOP_USAGE.to_string(),
                        Some(LOOP_USAGE_HINT.to_string()),
                    );
                }
            }
            SlashCommand::Schedule => {
                if !self.config.features.enabled(Feature::ScheduledTasks) {
                    return;
                }
                if let Some(thread_id) = self.thread_id {
                    self.app_event_tx
                        .send(AppEvent::OpenThreadScheduleManager { thread_id });
                    self.append_message_history_entry("/schedule".to_string());
                } else {
                    self.add_info_message(
                        SCHEDULE_USAGE.to_string(),
                        Some(SCHEDULE_USAGE_HINT.to_string()),
                    );
                }
            }
            SlashCommand::Monitor => {
                if !self.config.features.enabled(Feature::ScheduledTasks) {
                    return;
                }
                if let Some(thread_id) = self.thread_id {
                    self.app_event_tx
                        .send(AppEvent::OpenThreadMonitorManager { thread_id });
                    self.append_message_history_entry("/monitor".to_string());
                } else {
                    self.add_info_message(
                        MONITOR_USAGE.to_string(),
                        Some(MONITOR_USAGE_HINT.to_string()),
                    );
                }
            }
            SlashCommand::Side | SlashCommand::Btw => {
                self.request_empty_side_conversation(cmd);
            }
            SlashCommand::Session | SlashCommand::MultiAgents => {
                self.app_event_tx.send(AppEvent::OpenAgentPicker);
                self.append_message_history_entry(format!("/{}", cmd.command()));
            }
            SlashCommand::Agent => {
                self.app_event_tx.send(AppEvent::OpenBackgroundAgentManager);
                self.append_message_history_entry("/agent".to_string());
            }
            SlashCommand::BackgroundAgent => {
                self.app_event_tx.send(AppEvent::OpenBackgroundAgentManager);
                self.append_message_history_entry("/agent".to_string());
            }
            SlashCommand::ExternalAgent => {
                self.open_external_agent_picker();
            }
            SlashCommand::Permissions => {
                self.open_permissions_popup();
            }
            SlashCommand::Vim => {
                self.toggle_vim_mode_and_notify();
            }
            SlashCommand::Keymap => {
                self.open_keymap_picker();
            }
            SlashCommand::ElevateSandbox => {
                #[cfg(target_os = "windows")]
                {
                    let windows_sandbox_level = WindowsSandboxLevel::from_config(&self.config);
                    let windows_degraded_sandbox_enabled =
                        matches!(windows_sandbox_level, WindowsSandboxLevel::RestrictedToken);
                    if !windows_degraded_sandbox_enabled
                        || !crate::legacy_core::windows_sandbox::ELEVATED_SANDBOX_NUX_ENABLED
                    {
                        // This command should not be visible/recognized outside degraded mode,
                        // but guard anyway in case something dispatches it directly.
                        return;
                    }

                    let Some(preset) = builtin_approval_presets()
                        .into_iter()
                        .find(|preset| preset.id == "auto")
                    else {
                        // Avoid panicking in interactive UI; treat this as a recoverable
                        // internal error.
                        self.add_error_message(
                            "Internal error: missing the 'auto' approval preset.".to_string(),
                        );
                        return;
                    };

                    if let Err(err) = self
                        .config
                        .permissions
                        .approval_policy
                        .can_set(&preset.approval)
                    {
                        self.add_error_message(err.to_string());
                        return;
                    }

                    self.session_telemetry.counter(
                        "codex.windows_sandbox.setup_elevated_sandbox_command",
                        /*inc*/ 1,
                        &[],
                    );
                    self.app_event_tx
                        .send(AppEvent::BeginWindowsSandboxElevatedSetup {
                            preset,
                            profile_selection: None,
                        });
                }
                #[cfg(not(target_os = "windows"))]
                {
                    let _ = &self.session_telemetry;
                    // Not supported; on non-Windows this command should never be reachable.
                }
            }
            SlashCommand::SandboxReadRoot => {
                self.add_error_message(
                    "Usage: /sandbox-add-read-dir <absolute-directory-path>".to_string(),
                );
            }
            SlashCommand::Experimental => {
                self.open_experimental_popup();
            }
            SlashCommand::AutoReview => {
                self.open_auto_review_denials_popup();
            }
            SlashCommand::Memories => {
                self.open_memories_popup();
            }
            SlashCommand::Quit | SlashCommand::Exit => {
                self.request_quit_without_confirmation();
            }
            SlashCommand::Logout => {
                let mut header = ColumnRenderable::new();
                header.push(Line::from("Log out of Codewith?".bold()));
                header.push(Line::from(
                    "This clears the active login and exits the TUI after logout succeeds.".dim(),
                ));

                self.bottom_pane.show_selection_view(SelectionViewParams {
                    view_id: Some("logout-confirmation"),
                    footer_hint: Some(standard_popup_hint_line()),
                    header: Box::new(header),
                    items: vec![
                        SelectionItem {
                            name: "No, keep working".to_string(),
                            description: Some(
                                "Return to this session without logging out".to_string(),
                            ),
                            dismiss_on_select: true,
                            ..Default::default()
                        },
                        SelectionItem {
                            name: "Yes, log out and exit".to_string(),
                            description: Some(
                                "Clear the active login and shut down Codewith".to_string(),
                            ),
                            actions: vec![Box::new(|tx| {
                                tx.send(AppEvent::Logout);
                            })],
                            dismiss_on_select: true,
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                });
                self.request_redraw();
            }
            SlashCommand::Copy => {
                self.copy_last_agent_markdown();
            }
            SlashCommand::Raw => {
                let enabled = self.toggle_raw_output_mode_and_notify();
                self.emit_raw_output_mode_changed(enabled);
            }
            SlashCommand::Diff => {
                self.add_diff_in_progress();
                let tx = self.app_event_tx.clone();
                let runner = self.workspace_command_runner.clone();
                let cwd = self
                    .current_cwd
                    .clone()
                    .unwrap_or_else(|| self.config.cwd.to_path_buf());
                tokio::spawn(async move {
                    let text = match runner {
                        Some(runner) => match get_git_diff(runner.as_ref(), &cwd).await {
                            Ok((is_git_repo, diff_text)) => {
                                if is_git_repo {
                                    diff_text
                                } else {
                                    "`/diff` — _not inside a git repository_".to_string()
                                }
                            }
                            Err(e) => format!("Failed to compute diff: {e}"),
                        },
                        None => "Failed to compute diff: workspace command runner unavailable"
                            .to_string(),
                    };
                    tx.send(AppEvent::DiffResult(text));
                });
            }
            SlashCommand::Mention => {
                self.insert_str("@");
            }
            SlashCommand::Skills => {
                self.open_skills_menu();
            }
            SlashCommand::Hooks => {
                self.add_hooks_output();
            }
            SlashCommand::Status | SlashCommand::Stats => {
                self.dispatch_status_command(cmd.command());
            }
            SlashCommand::Changelog => {
                self.add_changelog_output();
            }
            SlashCommand::Ide => {
                self.handle_ide_command();
            }
            SlashCommand::DebugConfig => {
                self.add_debug_config_output();
            }
            SlashCommand::Title => {
                self.open_terminal_title_setup();
            }
            SlashCommand::Statusline => {
                self.open_status_line_setup();
            }
            SlashCommand::Summary => {
                self.open_message_summary_setup();
            }
            SlashCommand::Theme => {
                self.open_theme_picker();
            }
            SlashCommand::Pets => {
                self.open_pets_picker();
            }
            SlashCommand::Ps => {
                self.open_background_terminal_manager();
            }
            SlashCommand::Stop => {
                self.stop_background_terminals();
            }
            SlashCommand::MemoryDrop => {
                self.add_app_server_stub_message("Memory maintenance");
            }
            SlashCommand::MemoryUpdate => {
                self.add_app_server_stub_message("Memory maintenance");
            }
            SlashCommand::Mcp => {
                self.open_mcp_control_center();
            }
            SlashCommand::Apps => {
                self.add_connectors_output();
            }
            SlashCommand::Plugins => {
                self.add_plugins_output();
            }
            SlashCommand::Rollout => {
                if let Some(path) = self.rollout_path() {
                    self.add_info_message(
                        format!("Current rollout path: {}", path.display()),
                        /*hint*/ None,
                    );
                } else {
                    self.add_info_message(
                        "Rollout path is not available yet.".to_string(),
                        /*hint*/ None,
                    );
                }
            }
            SlashCommand::TestApproval => {
                use std::collections::HashMap;

                use crate::approval_events::ApplyPatchApprovalRequestEvent;
                use crate::diff_model::FileChange;

                self.on_apply_patch_approval_request(
                    "1".to_string(),
                    ApplyPatchApprovalRequestEvent {
                        call_id: "1".to_string(),
                        turn_id: "turn-1".to_string(),
                        changes: HashMap::from([
                            (
                                PathBuf::from("/tmp/test.txt"),
                                FileChange::Add {
                                    content: "test".to_string(),
                                },
                            ),
                            (
                                PathBuf::from("/tmp/test2.txt"),
                                FileChange::Update {
                                    unified_diff: "+test\n-test2".to_string(),
                                    move_path: None,
                                },
                            ),
                        ]),
                        reason: None,
                        grant_root: Some(PathBuf::from("/tmp")),
                    },
                );
            }
        }
    }

    /// Run an inline slash command.
    ///
    /// Branches that prepare arguments should pass `record_history: false` to the composer because
    /// the staged slash-command entry is the recall record; using the normal submission-history
    /// path as well would make a single command appear twice during Up-arrow navigation.
    pub(super) fn dispatch_command_with_args(
        &mut self,
        cmd: SlashCommand,
        args: String,
        text_elements: Vec<TextElement>,
    ) {
        if !self.ensure_slash_command_allowed_in_side_conversation(cmd) {
            return;
        }
        if !self.ensure_side_command_allowed_outside_review(cmd) {
            return;
        }
        if !cmd.supports_inline_args() {
            self.dispatch_command(cmd);
            return;
        }
        if !cmd.available_during_task() && self.bottom_pane.is_task_running() {
            let message = format!(
                "'/{}' is disabled while a task is in progress.",
                cmd.command()
            );
            self.add_to_history(history_cell::new_error_event(message));
            self.request_redraw();
            return;
        }

        let trimmed = args.trim();
        if trimmed.is_empty() {
            self.dispatch_command(cmd);
            return;
        }

        if cmd == SlashCommand::Goal
            && !self.goal_objective_with_pending_pastes_is_allowed(&args, &text_elements)
        {
            return;
        }

        let Some((prepared_args, prepared_elements)) =
            self.prepare_live_inline_args(args, text_elements)
        else {
            return;
        };
        self.dispatch_prepared_command_with_args(
            cmd,
            PreparedSlashCommandArgs {
                args: prepared_args,
                text_elements: prepared_elements,
                local_images: Vec::new(),
                remote_image_urls: Vec::new(),
                mention_bindings: Vec::new(),
                source: SlashCommandDispatchSource::Live,
            },
        );
    }

    fn prepare_live_inline_args(
        &mut self,
        args: String,
        text_elements: Vec<TextElement>,
    ) -> Option<(String, Vec<TextElement>)> {
        if self.bottom_pane.composer_text().is_empty() {
            Some((args, text_elements))
        } else {
            self.bottom_pane
                .prepare_inline_args_submission(/*record_history*/ false)
        }
    }

    fn prepared_inline_user_message(
        &mut self,
        args: String,
        text_elements: Vec<TextElement>,
        mut local_images: Vec<LocalImageAttachment>,
        mut remote_image_urls: Vec<String>,
        mut mention_bindings: Vec<MentionBinding>,
        source: SlashCommandDispatchSource,
    ) -> UserMessage {
        if source == SlashCommandDispatchSource::Live {
            local_images = self
                .bottom_pane
                .take_recent_submission_images_with_placeholders();
            remote_image_urls = self.take_remote_image_urls();
            mention_bindings = self.bottom_pane.take_recent_submission_mention_bindings();
        }
        UserMessage {
            text: args,
            local_images,
            remote_image_urls,
            text_elements,
            mention_bindings,
        }
    }

    fn dispatch_prepared_command_with_args(
        &mut self,
        cmd: SlashCommand,
        prepared: PreparedSlashCommandArgs,
    ) {
        let PreparedSlashCommandArgs {
            args,
            text_elements,
            local_images,
            remote_image_urls,
            mention_bindings,
            source,
        } = prepared;
        let trimmed = args.trim();
        match cmd {
            SlashCommand::Ide => {
                self.handle_ide_command_args(trimmed);
            }
            SlashCommand::Mcp => match trimmed.to_ascii_lowercase().as_str() {
                "" | "s" => self.open_mcp_control_center(),
                "verbose" | "manager" | "view" | "all" | "view all" => {
                    self.open_mcp_manager(McpServerStatusDetail::Full)
                }
                "list" => self.add_mcp_output(McpServerStatusDetail::ToolsAndAuthOnly),
                "list verbose" | "verbose list" => self.add_mcp_output(McpServerStatusDetail::Full),
                "reload" | "refresh tools" => self.app_event_tx.send(AppEvent::ReloadMcpServers),
                "add" | "setup" => self.app_event_tx.send(AppEvent::OpenMcpAddServer),
                _ if trimmed.starts_with("add ") => {
                    self.app_event_tx.send(AppEvent::AddMcpServer {
                        spec: trimmed["add ".len()..].trim().to_string(),
                    });
                }
                _ => self.add_error_message(
                    "Usage: /mcp [manager|list|list verbose|reload|add [name spec...]]".to_string(),
                ),
            },
            SlashCommand::Keymap => match trimmed.to_ascii_lowercase().as_str() {
                "" => self.open_keymap_picker(),
                "debug" => {
                    match crate::keymap::RuntimeKeymap::from_config(&self.config.tui_keymap) {
                        Ok(runtime_keymap) => self.open_keymap_debug(&runtime_keymap),
                        Err(err) => {
                            self.add_error_message(format!(
                                "Invalid `tui.keymap` configuration: {err}"
                            ));
                        }
                    }
                }
                _ => self.add_error_message("Usage: /keymap [debug]".to_string()),
            },
            SlashCommand::Raw => match trimmed.to_ascii_lowercase().as_str() {
                "on" => {
                    self.set_raw_output_mode_and_notify(/*enabled*/ true);
                    self.emit_raw_output_mode_changed(/*enabled*/ true);
                }
                "off" => {
                    self.set_raw_output_mode_and_notify(/*enabled*/ false);
                    self.emit_raw_output_mode_changed(/*enabled*/ false);
                }
                _ => self.add_error_message(RAW_USAGE.to_string()),
            },
            SlashCommand::ExternalAgent => {
                self.handle_external_agent_command_args(trimmed);
            }
            SlashCommand::Rename if !trimmed.is_empty() => {
                if !self.ensure_thread_rename_allowed() {
                    return;
                }
                self.session_telemetry
                    .counter("codex.thread.rename", /*inc*/ 1, &[]);
                let Some(name) = crate::legacy_core::util::normalize_thread_name(&args) else {
                    self.add_error_message("Thread name cannot be empty.".to_string());
                    return;
                };
                self.app_event_tx.set_thread_name(name);
            }
            SlashCommand::Plan if !trimmed.is_empty() => {
                if !self.apply_plan_slash_command() {
                    return;
                }
                let user_message = self.prepared_inline_user_message(
                    args,
                    text_elements,
                    local_images,
                    remote_image_urls,
                    mention_bindings,
                    source,
                );
                if self.is_session_configured() {
                    self.reasoning_buffer.clear();
                    self.full_reasoning_buffer.clear();
                    self.set_status_header(String::from("Working"));
                    self.submit_user_message(user_message);
                } else {
                    self.queue_user_message(user_message);
                }
            }
            SlashCommand::Goal if !trimmed.is_empty() => {
                if !self.config.features.enabled(Feature::Goals) {
                    return;
                }
                enum GoalControlCommand {
                    Clear,
                    SetStatus(AppThreadGoalStatus),
                }
                let control_command = match trimmed.to_ascii_lowercase().as_str() {
                    "clear" => Some(GoalControlCommand::Clear),
                    "cancel" | "cancelled" | "canceled" => Some(GoalControlCommand::SetStatus(
                        AppThreadGoalStatus::Cancelled,
                    )),
                    "edit" => {
                        self.app_event_tx.send(AppEvent::OpenThreadGoalEditor {
                            thread_id: self.thread_id,
                        });
                        if source == SlashCommandDispatchSource::Live {
                            self.bottom_pane.drain_pending_submission_state();
                        }
                        return;
                    }
                    "pause" => Some(GoalControlCommand::SetStatus(AppThreadGoalStatus::Paused)),
                    "resume" => Some(GoalControlCommand::SetStatus(AppThreadGoalStatus::Active)),
                    _ => None,
                };
                if let Some(command) = control_command {
                    let Some(thread_id) = self.thread_id else {
                        self.add_info_message(
                            GOAL_USAGE.to_string(),
                            Some(
                                "The session must start before you can change a goal.".to_string(),
                            ),
                        );
                        return;
                    };
                    match command {
                        GoalControlCommand::Clear => {
                            self.app_event_tx
                                .send(AppEvent::ClearThreadGoal { thread_id });
                        }
                        GoalControlCommand::SetStatus(status) => {
                            self.app_event_tx
                                .send(AppEvent::SetThreadGoalStatus { thread_id, status });
                        }
                    }
                    self.append_message_history_entry(format!("/goal {trimmed}"));
                    if source == SlashCommandDispatchSource::Live {
                        self.bottom_pane.drain_pending_submission_state();
                    }
                    return;
                }
                let objective = args.trim();
                if objective.is_empty() {
                    self.add_error_message("Goal objective must not be empty.".to_string());
                    self.add_info_message(
                        GOAL_USAGE.to_string(),
                        Some(GOAL_USAGE_HINT.to_string()),
                    );
                    if source == SlashCommandDispatchSource::Live {
                        self.bottom_pane.drain_pending_submission_state();
                    }
                    return;
                }
                let validation_source = match source {
                    SlashCommandDispatchSource::Live => GoalObjectiveValidationSource::Live,
                    SlashCommandDispatchSource::Queued => GoalObjectiveValidationSource::Queued,
                };
                if !self.goal_objective_is_allowed(objective, validation_source) {
                    return;
                }
                let Some(thread_id) = self.thread_id else {
                    if source == SlashCommandDispatchSource::Live {
                        self.queue_user_message_with_options(
                            UserMessage {
                                text: format!("/goal {args}"),
                                local_images: Vec::new(),
                                remote_image_urls: Vec::new(),
                                text_elements: Vec::new(),
                                mention_bindings: Vec::new(),
                            },
                            QueuedInputAction::ParseSlash,
                        );
                        self.bottom_pane.drain_pending_submission_state();
                    } else {
                        self.add_info_message(
                            GOAL_USAGE.to_string(),
                            Some("The session must start before you can set a goal.".to_string()),
                        );
                    }
                    return;
                };
                self.app_event_tx.send(AppEvent::SetThreadGoalObjective {
                    thread_id,
                    objective: objective.to_string(),
                    mode: ThreadGoalSetMode::ConfirmIfExists,
                });
                self.append_message_history_entry(format!("/goal {trimmed}"));
                if source == SlashCommandDispatchSource::Live {
                    self.bottom_pane.drain_pending_submission_state();
                }
            }
            SlashCommand::Workflow if !trimmed.is_empty() => {
                if !self.config.features.enabled(Feature::Workflows) {
                    return;
                }
                match parse_workflow_slash_args(trimmed) {
                    Ok(WorkflowSlashCommand::Draft { request }) => {
                        if self.bottom_pane.is_task_running() {
                            self.add_error_message(
                                "'/workflow draft' is disabled while a task is in progress."
                                    .to_string(),
                            );
                            if source == SlashCommandDispatchSource::Live {
                                self.bottom_pane.drain_pending_submission_state();
                            }
                            self.request_redraw();
                            return;
                        }
                        let workflow_prompt = workflow_generation_prompt(request);
                        self.app_event_tx.send(AppEvent::PrefillComposer {
                            text: workflow_prompt,
                        });
                        self.add_info_message(
                            "Workflow draft prompt prepared.".to_string(),
                            Some(
                                "Review the YAML-only prompt in the composer, then submit it when ready."
                                    .to_string(),
                            ),
                        );
                        if source == SlashCommandDispatchSource::Live {
                            self.bottom_pane.drain_pending_submission_state();
                        }
                    }
                    Ok(command) => {
                        let Some(thread_id) = self.thread_id else {
                            self.add_info_message(
                                WORKFLOW_USAGE.to_string(),
                                Some(
                                    "The session must start before you can manage workflows."
                                        .to_string(),
                                ),
                            );
                            if source == SlashCommandDispatchSource::Live {
                                self.bottom_pane.drain_pending_submission_state();
                            }
                            return;
                        };
                        self.app_event_tx.send(AppEvent::ManageThreadWorkflow {
                            thread_id,
                            action: workflow_slash_command_to_action(command),
                        });
                        self.append_message_history_entry(format!("/workflow {trimmed}"));
                        if source == SlashCommandDispatchSource::Live {
                            self.bottom_pane.drain_pending_submission_state();
                        }
                    }
                    Err(message) => {
                        self.add_error_message(message);
                        self.add_info_message(
                            WORKFLOW_USAGE.to_string(),
                            Some(WORKFLOW_USAGE_HINT.to_string()),
                        );
                        if source == SlashCommandDispatchSource::Live {
                            self.bottom_pane.drain_pending_submission_state();
                        }
                    }
                }
            }
            SlashCommand::Worktree if !trimmed.is_empty() => {
                let command = match parse_worktree_slash_args(trimmed) {
                    Ok(command) => command,
                    Err(err) => {
                        self.add_error_message(err.message);
                        if let Some(hint) = err.hint {
                            self.add_info_message(WORKTREE_USAGE.to_string(), Some(hint));
                        }
                        if source == SlashCommandDispatchSource::Live {
                            self.bottom_pane.drain_pending_submission_state();
                        }
                        return;
                    }
                };
                match command {
                    WorktreeSlashCommand::List => {
                        self.app_event_tx.send(AppEvent::OpenWorktreeManager);
                    }
                    WorktreeSlashCommand::Reconcile => {
                        self.app_event_tx.send(AppEvent::ReconcileWorktrees);
                    }
                    WorktreeSlashCommand::Create {
                        name,
                        branch,
                        start_point,
                    } => {
                        self.app_event_tx.send(AppEvent::CreateWorktree {
                            name,
                            branch,
                            start_point,
                        });
                    }
                    WorktreeSlashCommand::Read { worktree_id } => {
                        self.app_event_tx.send(AppEvent::ReadWorktree {
                            worktree_id,
                            base_repo_path: None,
                        });
                    }
                    WorktreeSlashCommand::Actions { worktree_id } => {
                        self.app_event_tx.send(AppEvent::OpenWorktreeActions {
                            worktree_id,
                            base_repo_path: None,
                        });
                    }
                    WorktreeSlashCommand::Use { worktree_id } => {
                        self.app_event_tx.send(AppEvent::UseWorktree {
                            worktree_id,
                            base_repo_path: None,
                        });
                    }
                    WorktreeSlashCommand::Release { worktree_id } => {
                        self.app_event_tx.send(AppEvent::ReleaseWorktree {
                            worktree_id,
                            base_repo_path: None,
                        });
                    }
                    WorktreeSlashCommand::Cleanup {
                        worktree_id,
                        force_delete,
                    } => {
                        self.app_event_tx.send(AppEvent::CleanupWorktree {
                            worktree_id,
                            base_repo_path: None,
                            force_delete,
                        });
                    }
                    WorktreeSlashCommand::Merge {
                        worktree_id,
                        target_ref,
                    } => {
                        self.app_event_tx
                            .send(AppEvent::RefreshWorktreeMergeCandidate {
                                worktree_id,
                                base_repo_path: None,
                                target_ref,
                            });
                    }
                }
                self.append_message_history_entry(format!("/worktree {trimmed}"));
                if source == SlashCommandDispatchSource::Live {
                    self.bottom_pane.drain_pending_submission_state();
                }
            }
            SlashCommand::Loop if !trimmed.is_empty() => {
                if !self.config.features.enabled(Feature::ScheduledTasks) {
                    return;
                }
                let command = match parse_loop_slash_args(trimmed) {
                    Ok(command) => command,
                    Err(err) => {
                        self.add_error_message(err.message);
                        if let Some(hint) = err.hint {
                            self.add_info_message(LOOP_USAGE.to_string(), Some(hint));
                        }
                        if source == SlashCommandDispatchSource::Live {
                            self.bottom_pane.drain_pending_submission_state();
                        }
                        return;
                    }
                };
                self.dispatch_loop_slash_command(command, trimmed, source);
            }
            SlashCommand::Schedule if !trimmed.is_empty() => {
                if !self.config.features.enabled(Feature::ScheduledTasks) {
                    return;
                }
                let command = match parse_schedule_slash_args(trimmed) {
                    Ok(command) => command,
                    Err(err) => {
                        self.add_error_message(err.message);
                        if let Some(hint) = err.hint {
                            self.add_info_message(SCHEDULE_USAGE.to_string(), Some(hint));
                        }
                        if source == SlashCommandDispatchSource::Live {
                            self.bottom_pane.drain_pending_submission_state();
                        }
                        return;
                    }
                };
                self.dispatch_schedule_slash_command(command, trimmed, source);
            }
            SlashCommand::Monitor if !trimmed.is_empty() => {
                if !self.config.features.enabled(Feature::ScheduledTasks) {
                    return;
                }
                let command = match parse_monitor_slash_args(trimmed) {
                    Ok(MonitorSlashCommand::Create) => {
                        let user_message = self.prepared_inline_user_message(
                            monitor_setup_prompt(trimmed),
                            text_elements,
                            local_images,
                            remote_image_urls,
                            mention_bindings,
                            source,
                        );
                        if self.is_session_configured() {
                            self.reasoning_buffer.clear();
                            self.full_reasoning_buffer.clear();
                            self.set_status_header(String::from("Working"));
                            self.submit_user_message(user_message);
                        } else {
                            self.queue_user_message(user_message);
                        }
                        return;
                    }
                    Ok(MonitorSlashCommand::Manage(command)) => command,
                    Err(err) => {
                        self.add_error_message(err.message);
                        if let Some(hint) = err.hint {
                            self.add_info_message(MONITOR_USAGE.to_string(), Some(hint));
                        }
                        if source == SlashCommandDispatchSource::Live {
                            self.bottom_pane.drain_pending_submission_state();
                        }
                        return;
                    }
                };
                self.dispatch_monitor_slash_command(command, trimmed, source);
            }
            SlashCommand::Agent | SlashCommand::BackgroundAgent if !trimmed.is_empty() => {
                if cmd == SlashCommand::Agent {
                    match parse_active_session_slash_args(trimmed) {
                        Ok(Some(command)) => {
                            self.dispatch_active_session_slash_command(command, trimmed, source);
                            return;
                        }
                        Ok(None) => {}
                        Err(err) => {
                            self.add_error_message(err.message);
                            if let Some(hint) = err.hint {
                                self.add_info_message(
                                    BACKGROUND_AGENT_USAGE.to_string(),
                                    Some(hint),
                                );
                            }
                            if source == SlashCommandDispatchSource::Live {
                                self.bottom_pane.drain_pending_submission_state();
                            }
                            return;
                        }
                    }
                }
                let command = match parse_background_agent_slash_args(trimmed) {
                    Ok(command) => command,
                    Err(err) => {
                        self.add_error_message(err.message);
                        if let Some(hint) = err.hint {
                            self.add_info_message(BACKGROUND_AGENT_USAGE.to_string(), Some(hint));
                        }
                        if source == SlashCommandDispatchSource::Live {
                            self.bottom_pane.drain_pending_submission_state();
                        }
                        return;
                    }
                };
                self.dispatch_background_agent_slash_command(
                    cmd.command(),
                    command,
                    trimmed,
                    source,
                );
            }
            SlashCommand::Recap if !trimmed.is_empty() => {
                self.dispatch_recap_slash_command(Some(trimmed.to_string()));
            }
            SlashCommand::Side | SlashCommand::Btw if !trimmed.is_empty() => {
                let Some(parent_thread_id) = self.thread_id else {
                    let command = cmd.command();
                    self.add_error_message(format!(
                        "'/{command}' is unavailable before the session starts."
                    ));
                    return;
                };
                let user_message = self.prepared_inline_user_message(
                    args,
                    text_elements,
                    local_images,
                    remote_image_urls,
                    mention_bindings,
                    source,
                );
                self.request_side_conversation(parent_thread_id, Some(user_message));
            }
            SlashCommand::Review if !trimmed.is_empty() => {
                self.submit_op(AppCommand::review(ReviewTarget::Custom {
                    instructions: args,
                }));
            }
            SlashCommand::Resume if !trimmed.is_empty() => {
                self.app_event_tx
                    .send(AppEvent::ResumeSessionByIdOrName(args));
            }
            SlashCommand::Tmux if !trimmed.is_empty() => match parse_tmux_slash_args(trimmed) {
                Ok(command) => {
                    self.app_event_tx.send(AppEvent::OpenInTmux {
                        destination: command.destination,
                        replace_existing: command.replace_existing,
                    });
                }
                Err(message) => self.add_error_message(message),
            },
            SlashCommand::SandboxReadRoot if !trimmed.is_empty() => {
                self.app_event_tx
                    .send(AppEvent::BeginWindowsSandboxGrantReadRoot { path: args });
            }
            SlashCommand::Pets
                if matches!(
                    args.trim().to_ascii_lowercase().as_str(),
                    "disable" | "disabled" | "hide" | "hidden" | "off" | "none"
                ) =>
            {
                self.app_event_tx.send(AppEvent::PetDisabled);
            }
            SlashCommand::Pets if !trimmed.is_empty() => {
                self.select_pet_by_id(args);
            }
            _ => self.dispatch_command(cmd),
        }
        if source == SlashCommandDispatchSource::Live && cmd != SlashCommand::Goal {
            self.bottom_pane.drain_pending_submission_state();
        }
    }

    fn dispatch_recap_slash_command(&mut self, prompt: Option<String>) {
        let Some(thread_id) = self.thread_id else {
            self.add_error_message(
                "'/recap' is unavailable before the session starts.".to_string(),
            );
            return;
        };
        self.add_info_message("Generating recap...".to_string(), /*hint*/ None);
        self.app_event_tx.send(AppEvent::RequestSessionRecap {
            thread_id,
            prompt,
            automatic: false,
        });
    }

    fn dispatch_loop_slash_command(
        &mut self,
        command: LoopSlashCommand,
        trimmed: &str,
        source: SlashCommandDispatchSource,
    ) {
        let Some(thread_id) = self.thread_id else {
            if source == SlashCommandDispatchSource::Live {
                self.queue_user_message_with_options(
                    UserMessage {
                        text: format!("/loop {trimmed}"),
                        local_images: Vec::new(),
                        remote_image_urls: Vec::new(),
                        text_elements: Vec::new(),
                        mention_bindings: Vec::new(),
                    },
                    QueuedInputAction::ParseSlash,
                );
                self.bottom_pane.drain_pending_submission_state();
            } else {
                self.add_info_message(LOOP_USAGE.to_string(), Some(LOOP_USAGE_HINT.to_string()));
            }
            return;
        };

        match command {
            LoopSlashCommand::Default => {
                self.app_event_tx
                    .send(AppEvent::OpenThreadLoopManager { thread_id });
            }
            LoopSlashCommand::Create(request) => {
                let Ok((prompt, prompt_source, schedule)) = loop_create_request_to_api(request)
                else {
                    self.add_error_message("One-time schedules belong in /schedule.".to_string());
                    return;
                };
                self.app_event_tx.send(AppEvent::CreateThreadLoopSchedule {
                    thread_id,
                    prompt,
                    prompt_source,
                    schedule,
                });
            }
            LoopSlashCommand::Manage(manage) => match manage {
                LoopManageCommand::List => {
                    self.app_event_tx
                        .send(AppEvent::OpenThreadLoopManager { thread_id });
                }
                LoopManageCommand::Pause { schedule_id } => {
                    self.app_event_tx.send(AppEvent::PauseThreadLoopSchedule {
                        thread_id,
                        schedule_id,
                    });
                }
                LoopManageCommand::Resume { schedule_id } => {
                    self.app_event_tx.send(AppEvent::ResumeThreadLoopSchedule {
                        thread_id,
                        schedule_id,
                    });
                }
                LoopManageCommand::Delete { schedule_id } => {
                    self.app_event_tx.send(AppEvent::DeleteThreadLoopSchedule {
                        thread_id,
                        schedule_id,
                    });
                }
                LoopManageCommand::RunNow { schedule_id } => {
                    self.app_event_tx.send(AppEvent::RunThreadLoopScheduleNow {
                        thread_id,
                        schedule_id,
                    });
                }
                LoopManageCommand::Edit { schedule_id } => {
                    self.app_event_tx.send(AppEvent::OpenThreadLoopEditor {
                        thread_id,
                        schedule_id,
                    });
                }
                LoopManageCommand::Stats { schedule_id } => {
                    self.app_event_tx
                        .send(AppEvent::OpenThreadLoopScheduleStats {
                            thread_id,
                            schedule_id,
                        });
                }
            },
        }
        self.append_message_history_entry(format!("/loop {trimmed}"));
        if source == SlashCommandDispatchSource::Live {
            self.bottom_pane.drain_pending_submission_state();
        }
    }

    fn dispatch_schedule_slash_command(
        &mut self,
        command: LoopSlashCommand,
        trimmed: &str,
        source: SlashCommandDispatchSource,
    ) {
        let Some(thread_id) = self.thread_id else {
            if source == SlashCommandDispatchSource::Live {
                self.queue_user_message_with_options(
                    UserMessage {
                        text: format!("/schedule {trimmed}"),
                        local_images: Vec::new(),
                        remote_image_urls: Vec::new(),
                        text_elements: Vec::new(),
                        mention_bindings: Vec::new(),
                    },
                    QueuedInputAction::ParseSlash,
                );
                self.bottom_pane.drain_pending_submission_state();
            } else {
                self.add_info_message(
                    SCHEDULE_USAGE.to_string(),
                    Some(SCHEDULE_USAGE_HINT.to_string()),
                );
            }
            return;
        };

        match command {
            LoopSlashCommand::Default => {
                self.app_event_tx
                    .send(AppEvent::OpenThreadScheduleManager { thread_id });
            }
            LoopSlashCommand::Create(request) => {
                let Ok((prompt, prompt_source, schedule, next_run_at)) =
                    schedule_create_request_to_api(request)
                else {
                    self.add_error_message("Schedule delay is too large.".to_string());
                    return;
                };
                self.app_event_tx.send(AppEvent::CreateThreadSchedule {
                    thread_id,
                    prompt,
                    prompt_source,
                    schedule,
                    next_run_at,
                });
            }
            LoopSlashCommand::Manage(manage) => match manage {
                LoopManageCommand::List => {
                    self.app_event_tx
                        .send(AppEvent::OpenThreadScheduleManager { thread_id });
                }
                LoopManageCommand::Pause { schedule_id } => {
                    self.app_event_tx.send(AppEvent::PauseThreadSchedule {
                        thread_id,
                        schedule_id,
                    });
                }
                LoopManageCommand::Resume { schedule_id } => {
                    self.app_event_tx.send(AppEvent::ResumeThreadSchedule {
                        thread_id,
                        schedule_id,
                    });
                }
                LoopManageCommand::Delete { schedule_id } => {
                    self.app_event_tx.send(AppEvent::DeleteThreadSchedule {
                        thread_id,
                        schedule_id,
                    });
                }
                LoopManageCommand::RunNow { schedule_id } => {
                    self.app_event_tx.send(AppEvent::RunThreadScheduleNow {
                        thread_id,
                        schedule_id,
                    });
                }
                LoopManageCommand::Edit { schedule_id } => {
                    self.app_event_tx.send(AppEvent::OpenThreadScheduleEditor {
                        thread_id,
                        schedule_id,
                    });
                }
                LoopManageCommand::Stats { schedule_id } => {
                    self.app_event_tx.send(AppEvent::OpenThreadScheduleStats {
                        thread_id,
                        schedule_id,
                    });
                }
            },
        }
        self.append_message_history_entry(format!("/schedule {trimmed}"));
        if source == SlashCommandDispatchSource::Live {
            self.bottom_pane.drain_pending_submission_state();
        }
    }

    fn dispatch_monitor_slash_command(
        &mut self,
        command: MonitorManageCommand,
        trimmed: &str,
        source: SlashCommandDispatchSource,
    ) {
        let Some(thread_id) = self.thread_id else {
            if source == SlashCommandDispatchSource::Live {
                self.queue_user_message_with_options(
                    UserMessage {
                        text: format!("/monitor {trimmed}"),
                        local_images: Vec::new(),
                        remote_image_urls: Vec::new(),
                        text_elements: Vec::new(),
                        mention_bindings: Vec::new(),
                    },
                    QueuedInputAction::ParseSlash,
                );
                self.bottom_pane.drain_pending_submission_state();
            } else {
                self.add_info_message(
                    MONITOR_USAGE.to_string(),
                    Some(MONITOR_USAGE_HINT.to_string()),
                );
            }
            return;
        };

        match command {
            MonitorManageCommand::List => {
                self.app_event_tx
                    .send(AppEvent::OpenThreadMonitorManager { thread_id });
            }
            MonitorManageCommand::Read { monitor_id } => {
                self.app_event_tx.send(AppEvent::ReadThreadMonitor {
                    thread_id,
                    monitor_id,
                });
            }
            MonitorManageCommand::Stop { monitor_id } => {
                self.app_event_tx.send(AppEvent::StopThreadMonitor {
                    thread_id,
                    monitor_id,
                });
            }
            MonitorManageCommand::Restart { monitor_id } => {
                self.app_event_tx.send(AppEvent::RestartThreadMonitor {
                    thread_id,
                    monitor_id,
                });
            }
            MonitorManageCommand::Delete { monitor_id } => {
                self.app_event_tx.send(AppEvent::DeleteThreadMonitor {
                    thread_id,
                    monitor_id,
                });
            }
        }
        self.append_message_history_entry(format!("/monitor {trimmed}"));
        if source == SlashCommandDispatchSource::Live {
            self.bottom_pane.drain_pending_submission_state();
        }
    }

    fn dispatch_background_agent_slash_command(
        &mut self,
        command_name: &str,
        command: BackgroundAgentSlashCommand,
        trimmed: &str,
        source: SlashCommandDispatchSource,
    ) {
        match command {
            BackgroundAgentSlashCommand::List => {
                self.app_event_tx.send(AppEvent::OpenBackgroundAgentManager);
            }
            BackgroundAgentSlashCommand::Start {
                prompt,
                worktree_id,
            } => {
                self.app_event_tx.send(AppEvent::StartBackgroundAgent {
                    prompt,
                    worktree_id,
                });
            }
            BackgroundAgentSlashCommand::Read { agent_id } => {
                self.app_event_tx
                    .send(AppEvent::ReadBackgroundAgent { agent_id });
            }
            BackgroundAgentSlashCommand::Logs { agent_id } => {
                self.app_event_tx
                    .send(AppEvent::ShowBackgroundAgentLogs { agent_id });
            }
            BackgroundAgentSlashCommand::Attach { agent_id } => {
                self.app_event_tx
                    .send(AppEvent::AttachBackgroundAgent { agent_id });
            }
            BackgroundAgentSlashCommand::Detach { agent_id } => {
                self.app_event_tx
                    .send(AppEvent::DetachBackgroundAgent { agent_id });
            }
            BackgroundAgentSlashCommand::Stop { agent_id } => {
                self.app_event_tx
                    .send(AppEvent::StopBackgroundAgent { agent_id });
            }
            BackgroundAgentSlashCommand::Delete { agent_id } => {
                self.app_event_tx
                    .send(AppEvent::DeleteBackgroundAgent { agent_id });
            }
            BackgroundAgentSlashCommand::Diagnostics => {
                self.app_event_tx
                    .send(AppEvent::ShowBackgroundAgentDiagnostics);
            }
        }
        self.append_message_history_entry(format!("/{command_name} {trimmed}"));
        if source == SlashCommandDispatchSource::Live {
            self.bottom_pane.drain_pending_submission_state();
        }
    }

    fn dispatch_active_session_slash_command(
        &mut self,
        command: ActiveSessionSlashCommand,
        trimmed: &str,
        source: SlashCommandDispatchSource,
    ) {
        match command {
            ActiveSessionSlashCommand::List => {
                self.app_event_tx.send(AppEvent::ListActiveSessions);
            }
            ActiveSessionSlashCommand::Send {
                target_peer_id,
                message,
                wake,
            } => {
                self.app_event_tx.send(AppEvent::SendActiveSessionMessage {
                    target_peer_id,
                    message,
                    wake,
                });
            }
        }
        self.append_message_history_entry(format!("/agent {trimmed}"));
        if source == SlashCommandDispatchSource::Live {
            self.bottom_pane.drain_pending_submission_state();
        }
    }

    pub(super) fn submit_queued_slash_prompt(&mut self, user_message: UserMessage) -> QueueDrain {
        let UserMessage {
            text,
            local_images,
            remote_image_urls,
            text_elements,
            mention_bindings,
        } = user_message;
        let Some((name, rest, rest_offset)) = parse_slash_name(&text) else {
            self.submit_user_message(UserMessage {
                text,
                local_images,
                remote_image_urls,
                text_elements,
                mention_bindings,
            });
            return QueueDrain::Stop;
        };

        if name.contains('/') {
            self.submit_user_message(UserMessage {
                text,
                local_images,
                remote_image_urls,
                text_elements,
                mention_bindings,
            });
            return QueueDrain::Stop;
        }

        let service_tier_commands = self.current_model_service_tier_commands();
        let Some(command) =
            find_slash_command(name, self.builtin_command_flags(), &service_tier_commands)
        else {
            self.add_info_message(
                format!(
                    r#"Unrecognized command '/{name}'. Type "/" for a list of supported commands."#
                ),
                /*hint*/ None,
            );
            return QueueDrain::Continue;
        };

        if rest.is_empty() {
            return match command {
                SlashCommandItem::Builtin(cmd) => {
                    self.dispatch_command(cmd);
                    self.queued_command_drain_result(cmd)
                }
                SlashCommandItem::ServiceTier(command) => {
                    self.handle_service_tier_command_dispatch(command);
                    QueueDrain::Continue
                }
            };
        }

        if let SlashCommandItem::ServiceTier(command) = command {
            self.handle_service_tier_command_with_args_dispatch(command, rest.trim().to_string());
            return QueueDrain::Continue;
        }

        if !command.supports_inline_args() {
            self.submit_user_message(UserMessage {
                text,
                local_images,
                remote_image_urls,
                text_elements,
                mention_bindings,
            });
            return QueueDrain::Stop;
        }
        let SlashCommandItem::Builtin(cmd) = command else {
            self.submit_user_message(UserMessage {
                text,
                local_images,
                remote_image_urls,
                text_elements,
                mention_bindings,
            });
            return QueueDrain::Stop;
        };

        let trimmed_start = rest.trim_start();
        let leading_trimmed = rest.len().saturating_sub(trimmed_start.len());
        let trimmed_rest = trimmed_start.trim_end();
        let args_elements = Self::slash_command_args_elements(
            trimmed_rest,
            rest_offset + leading_trimmed,
            &text_elements,
        );
        if cmd == SlashCommand::Goal
            && !self.goal_objective_is_allowed(trimmed_rest, GoalObjectiveValidationSource::Queued)
        {
            return QueueDrain::Continue;
        }
        self.dispatch_prepared_command_with_args(
            cmd,
            PreparedSlashCommandArgs {
                args: trimmed_rest.to_string(),
                text_elements: args_elements,
                local_images,
                remote_image_urls,
                mention_bindings,
                source: SlashCommandDispatchSource::Queued,
            },
        );
        self.queued_command_drain_result(cmd)
    }

    fn builtin_command_flags(&self) -> BuiltinCommandFlags {
        #[cfg(target_os = "windows")]
        let allow_elevate_sandbox = {
            let windows_sandbox_level = WindowsSandboxLevel::from_config(&self.config);
            matches!(windows_sandbox_level, WindowsSandboxLevel::RestrictedToken)
        };
        #[cfg(not(target_os = "windows"))]
        let allow_elevate_sandbox = false;

        BuiltinCommandFlags {
            collaboration_modes_enabled: self.collaboration_modes_enabled(),
            connectors_enabled: self.connectors_enabled(),
            plugins_command_enabled: self.config.features.enabled(Feature::Plugins),
            goal_command_enabled: self.config.features.enabled(Feature::Goals),
            workflow_command_enabled: self.config.features.enabled(Feature::Workflows),
            scheduled_tasks_command_enabled: self.config.features.enabled(Feature::ScheduledTasks),
            service_tier_commands_enabled: self.fast_mode_enabled(),
            personality_command_enabled: self.config.features.enabled(Feature::Personality),
            realtime_conversation_enabled: self.realtime_conversation_enabled(),
            audio_device_selection_enabled: self.realtime_audio_device_selection_enabled(),
            allow_elevate_sandbox,
            side_conversation_active: self.active_side_conversation,
        }
    }

    fn queued_command_drain_result(&self, cmd: SlashCommand) -> QueueDrain {
        if self.is_user_turn_pending_or_running() || !self.bottom_pane.no_modal_or_popup_active() {
            return QueueDrain::Stop;
        }
        match cmd {
            SlashCommand::Ide
            | SlashCommand::Status
            | SlashCommand::Stats
            | SlashCommand::Changelog
            | SlashCommand::DebugConfig
            | SlashCommand::Ps
            | SlashCommand::Stop
            | SlashCommand::MemoryDrop
            | SlashCommand::MemoryUpdate
            | SlashCommand::Mcp
            | SlashCommand::Apps
            | SlashCommand::Plugins
            | SlashCommand::Rollout
            | SlashCommand::Copy
            | SlashCommand::Raw
            | SlashCommand::Vim
            | SlashCommand::Diff
            | SlashCommand::Recap
            | SlashCommand::App
            | SlashCommand::Rename
            | SlashCommand::TestApproval => QueueDrain::Continue,
            SlashCommand::Feedback
            | SlashCommand::New
            | SlashCommand::Archive
            | SlashCommand::Clear
            | SlashCommand::Resume
            | SlashCommand::Fork
            | SlashCommand::Init
            | SlashCommand::Compact
            | SlashCommand::Review
            | SlashCommand::Model
            | SlashCommand::Profile
            | SlashCommand::Provider
            | SlashCommand::Config
            | SlashCommand::Tmux
            | SlashCommand::Realtime
            | SlashCommand::Settings
            | SlashCommand::Personality
            | SlashCommand::Plan
            | SlashCommand::Goal
            | SlashCommand::MissionControl
            | SlashCommand::Workflow
            | SlashCommand::Loop
            | SlashCommand::Schedule
            | SlashCommand::Monitor
            | SlashCommand::Session
            | SlashCommand::Worktree
            | SlashCommand::Side
            | SlashCommand::Btw
            | SlashCommand::Keymap
            | SlashCommand::Agent
            | SlashCommand::BackgroundAgent
            | SlashCommand::ExternalAgent
            | SlashCommand::MultiAgents
            | SlashCommand::Permissions
            | SlashCommand::ElevateSandbox
            | SlashCommand::SandboxReadRoot
            | SlashCommand::Experimental
            | SlashCommand::AutoReview
            | SlashCommand::Memories
            | SlashCommand::Quit
            | SlashCommand::Exit
            | SlashCommand::Logout
            | SlashCommand::Mention
            | SlashCommand::Skills
            | SlashCommand::Hooks
            | SlashCommand::Title
            | SlashCommand::Statusline
            | SlashCommand::Summary
            | SlashCommand::Theme
            | SlashCommand::Pets => QueueDrain::Stop,
        }
    }

    fn slash_command_args_elements(
        rest: &str,
        rest_offset: usize,
        text_elements: &[TextElement],
    ) -> Vec<TextElement> {
        if rest.is_empty() || text_elements.is_empty() {
            return Vec::new();
        }
        text_elements
            .iter()
            .filter_map(|elem| {
                if elem.byte_range.end <= rest_offset {
                    return None;
                }
                let start = elem.byte_range.start.saturating_sub(rest_offset);
                let mut end = elem.byte_range.end.saturating_sub(rest_offset);
                if start >= rest.len() {
                    return None;
                }
                end = end.min(rest.len());
                (start < end).then_some(elem.map_range(|_| ByteRange { start, end }))
            })
            .collect()
    }

    fn ensure_slash_command_allowed_in_side_conversation(&mut self, cmd: SlashCommand) -> bool {
        if !self.active_side_conversation || cmd.available_in_side_conversation() {
            return true;
        }
        self.add_error_message(format!(
            "'/{}' is unavailable in side conversations. {SIDE_SLASH_COMMAND_UNAVAILABLE_HINT}",
            cmd.command()
        ));
        self.bottom_pane.drain_pending_submission_state();
        false
    }

    fn ensure_side_command_allowed_outside_review(&mut self, cmd: SlashCommand) -> bool {
        if !matches!(cmd, SlashCommand::Side | SlashCommand::Btw) || !self.review.is_review_mode {
            return true;
        }

        let command = cmd.command();
        self.add_error_message(format!(
            "'/{command}' is unavailable while code review is running."
        ));
        self.bottom_pane.drain_pending_submission_state();
        false
    }
}

fn workflow_slash_command_to_action(command: WorkflowSlashCommand<'_>) -> ThreadWorkflowAction {
    match command {
        WorkflowSlashCommand::List => ThreadWorkflowAction::List,
        WorkflowSlashCommand::Show { workflow_record_id } => ThreadWorkflowAction::Show {
            workflow_record_id: workflow_record_id.to_string(),
        },
        WorkflowSlashCommand::Draft { .. } => ThreadWorkflowAction::List,
        WorkflowSlashCommand::RunList => ThreadWorkflowAction::RunList,
        WorkflowSlashCommand::RunShow { run_id } => ThreadWorkflowAction::RunShow {
            run_id: run_id.to_string(),
        },
        WorkflowSlashCommand::RunStart { workflow_record_id } => ThreadWorkflowAction::RunStart {
            workflow_record_id: workflow_record_id.to_string(),
        },
        WorkflowSlashCommand::RunPause { run_id } => ThreadWorkflowAction::RunPause {
            run_id: run_id.to_string(),
        },
        WorkflowSlashCommand::RunResume { run_id } => ThreadWorkflowAction::RunResume {
            run_id: run_id.to_string(),
        },
        WorkflowSlashCommand::RunCancel { run_id } => ThreadWorkflowAction::RunCancel {
            run_id: run_id.to_string(),
        },
    }
}

fn monitor_setup_prompt(request: &str) -> String {
    format!(
        "\
Set up a Codewith monitor for this request.

Use the `manage_monitor` tool to create exactly one monitor unless you need to ask a clarification. The monitor should be implemented dynamically from the user's request using a shell command or script you design. Do not choose from predefined monitor categories or hardcoded source types.

Prefer a command that emits concise one-line stdout updates when something relevant happens. Use stream routing unless the user requested a file, in which case choose file or both routing and set output_file.

User monitor request:
{request}"
    )
}

fn parse_monitor_slash_args(input: &str) -> Result<MonitorSlashCommand, MonitorSlashParseError> {
    let trimmed = input.trim();
    let Some(first) = trimmed.split_whitespace().next() else {
        return Ok(MonitorSlashCommand::Create);
    };
    let rest = trimmed[first.len()..].trim();

    match first.to_ascii_lowercase().as_str() {
        "list" | "ls" => {
            if rest.is_empty() {
                Ok(MonitorSlashCommand::Manage(MonitorManageCommand::List))
            } else {
                Err(monitor_usage_error("Usage: /monitor list"))
            }
        }
        "read" | "show" => Ok(MonitorSlashCommand::Manage(MonitorManageCommand::Read {
            monitor_id: parse_optional_monitor_id(rest, "Usage: /monitor read [id]")?,
        })),
        "stop" | "pause" => Ok(MonitorSlashCommand::Manage(MonitorManageCommand::Stop {
            monitor_id: parse_optional_monitor_id(rest, "Usage: /monitor stop [id]")?,
        })),
        "restart" | "start" => Ok(MonitorSlashCommand::Manage(MonitorManageCommand::Restart {
            monitor_id: parse_optional_monitor_id(rest, "Usage: /monitor restart [id]")?,
        })),
        "delete" | "remove" | "rm" | "clear" => {
            Ok(MonitorSlashCommand::Manage(MonitorManageCommand::Delete {
                monitor_id: parse_optional_monitor_id(rest, "Usage: /monitor delete [id]")?,
            }))
        }
        _ => Ok(MonitorSlashCommand::Create),
    }
}

fn parse_optional_monitor_id(
    rest: &str,
    usage: &'static str,
) -> Result<Option<String>, MonitorSlashParseError> {
    if rest.is_empty() {
        return Ok(None);
    }
    let mut parts = rest.split_whitespace();
    let Some(monitor_id) = parts.next() else {
        return Ok(None);
    };
    if parts.next().is_some() {
        return Err(monitor_usage_error(usage));
    }
    Ok(Some(monitor_id.to_string()))
}

fn monitor_usage_error(usage: &'static str) -> MonitorSlashParseError {
    MonitorSlashParseError {
        message: usage.to_string(),
        hint: Some(MONITOR_USAGE_HINT.to_string()),
    }
}

fn parse_background_agent_slash_args(
    input: &str,
) -> Result<BackgroundAgentSlashCommand, MonitorSlashParseError> {
    let trimmed = input.trim();
    let Some(first) = trimmed.split_whitespace().next() else {
        return Ok(BackgroundAgentSlashCommand::List);
    };
    let rest = trimmed[first.len()..].trim();

    match first.to_ascii_lowercase().as_str() {
        "list" | "ls" => {
            if rest.is_empty() {
                Ok(BackgroundAgentSlashCommand::List)
            } else {
                Err(background_agent_usage_error(
                    "Usage: /background-agent list",
                ))
            }
        }
        "diagnostics" | "diag" | "daemon" => {
            if rest.is_empty() {
                Ok(BackgroundAgentSlashCommand::Diagnostics)
            } else {
                Err(background_agent_usage_error(
                    "Usage: /background-agent diagnostics",
                ))
            }
        }
        "start" | "run" | "spawn" => parse_background_agent_start_args(rest),
        "read" | "show" => Ok(BackgroundAgentSlashCommand::Read {
            agent_id: parse_optional_background_agent_id(
                rest,
                "Usage: /background-agent read [id]",
            )?,
        }),
        "logs" | "log" => Ok(BackgroundAgentSlashCommand::Logs {
            agent_id: parse_optional_background_agent_id(
                rest,
                "Usage: /background-agent logs [id]",
            )?,
        }),
        "attach" => Ok(BackgroundAgentSlashCommand::Attach {
            agent_id: parse_optional_background_agent_id(
                rest,
                "Usage: /background-agent attach [id]",
            )?,
        }),
        "detach" => Ok(BackgroundAgentSlashCommand::Detach {
            agent_id: parse_optional_background_agent_id(
                rest,
                "Usage: /background-agent detach [id]",
            )?,
        }),
        "stop" | "cancel" => Ok(BackgroundAgentSlashCommand::Stop {
            agent_id: parse_optional_background_agent_id(
                rest,
                "Usage: /background-agent stop [id]",
            )?,
        }),
        "delete" | "remove" | "rm" => Ok(BackgroundAgentSlashCommand::Delete {
            agent_id: parse_optional_background_agent_id(
                rest,
                "Usage: /background-agent delete [id]",
            )?,
        }),
        _ => Err(background_agent_usage_error(BACKGROUND_AGENT_USAGE)),
    }
}

fn parse_worktree_slash_args(input: &str) -> Result<WorktreeSlashCommand, MonitorSlashParseError> {
    let trimmed = input.trim();
    let Some(first) = trimmed.split_whitespace().next() else {
        return Ok(WorktreeSlashCommand::List);
    };
    let rest = trimmed[first.len()..].trim();

    match first.to_ascii_lowercase().as_str() {
        "list" | "ls" => {
            if rest.is_empty() {
                Ok(WorktreeSlashCommand::List)
            } else {
                Err(worktree_usage_error("Usage: /worktree list"))
            }
        }
        "reconcile" | "sync" => {
            if rest.is_empty() {
                Ok(WorktreeSlashCommand::Reconcile)
            } else {
                Err(worktree_usage_error("Usage: /worktree reconcile"))
            }
        }
        "create" | "new" => parse_worktree_create_args(rest),
        "read" | "show" => Ok(WorktreeSlashCommand::Read {
            worktree_id: parse_optional_worktree_id(rest, "Usage: /worktree read [id]")?,
        }),
        "actions" | "manage" => Ok(WorktreeSlashCommand::Actions {
            worktree_id: parse_required_worktree_id(rest, "Usage: /worktree actions <id>")?,
        }),
        "use" | "switch" => Ok(WorktreeSlashCommand::Use {
            worktree_id: parse_required_worktree_id(rest, "Usage: /worktree use <id>")?,
        }),
        "release" => Ok(WorktreeSlashCommand::Release {
            worktree_id: parse_required_worktree_id(rest, "Usage: /worktree release <id>")?,
        }),
        "cleanup" | "clean" | "delete" | "remove" | "rm" => parse_worktree_cleanup_args(rest),
        "merge" | "candidate" => parse_worktree_merge_args(rest),
        _ => Err(worktree_usage_error(WORKTREE_USAGE)),
    }
}

fn parse_worktree_create_args(input: &str) -> Result<WorktreeSlashCommand, MonitorSlashParseError> {
    let mut remaining = input.trim();
    let mut branch = None;
    let mut start_point = None;
    let mut name = None;

    while let Some((token, rest)) = split_external_agent_token(remaining) {
        if let Some(value) = token.strip_prefix("--branch=") {
            branch = Some(parse_required_option_value(
                value,
                "Usage: /worktree create [name] [--branch <branch>] [--start-point <ref>]",
            )?);
            remaining = rest;
            continue;
        }
        if let Some(value) = token.strip_prefix("--start-point=") {
            start_point = Some(parse_required_option_value(
                value,
                "Usage: /worktree create [name] [--branch <branch>] [--start-point <ref>]",
            )?);
            remaining = rest;
            continue;
        }
        if let Some(value) = token.strip_prefix("--start=") {
            start_point = Some(parse_required_option_value(
                value,
                "Usage: /worktree create [name] [--branch <branch>] [--start-point <ref>]",
            )?);
            remaining = rest;
            continue;
        }
        match token {
            "--branch" => {
                let Some((value, next_rest)) = split_external_agent_token(rest) else {
                    return Err(worktree_usage_error(
                        "Usage: /worktree create [name] [--branch <branch>] [--start-point <ref>]",
                    ));
                };
                branch = Some(value.to_string());
                remaining = next_rest;
            }
            "--start-point" | "--start" => {
                let Some((value, next_rest)) = split_external_agent_token(rest) else {
                    return Err(worktree_usage_error(
                        "Usage: /worktree create [name] [--branch <branch>] [--start-point <ref>]",
                    ));
                };
                start_point = Some(value.to_string());
                remaining = next_rest;
            }
            _ if name.is_none() => {
                name = Some(token.to_string());
                remaining = rest;
            }
            _ => {
                return Err(worktree_usage_error(
                    "Usage: /worktree create [name] [--branch <branch>] [--start-point <ref>]",
                ));
            }
        }
    }

    Ok(WorktreeSlashCommand::Create {
        name,
        branch,
        start_point,
    })
}

fn parse_worktree_cleanup_args(
    input: &str,
) -> Result<WorktreeSlashCommand, MonitorSlashParseError> {
    let mut force_delete = false;
    let mut worktree_id = None;
    for token in input.split_whitespace() {
        match token {
            "--force" | "-f" => force_delete = true,
            _ if worktree_id.is_none() => worktree_id = Some(token.to_string()),
            _ => {
                return Err(worktree_usage_error(
                    "Usage: /worktree cleanup [--force] <id>",
                ));
            }
        }
    }
    let Some(worktree_id) = worktree_id else {
        return Err(worktree_usage_error(
            "Usage: /worktree cleanup [--force] <id>",
        ));
    };
    Ok(WorktreeSlashCommand::Cleanup {
        worktree_id,
        force_delete,
    })
}

fn parse_worktree_merge_args(input: &str) -> Result<WorktreeSlashCommand, MonitorSlashParseError> {
    let mut parts = input.split_whitespace();
    let Some(worktree_id) = parts.next() else {
        return Err(worktree_usage_error("Usage: /worktree merge <id> [target]"));
    };
    let target_ref = parts.next().map(str::to_string);
    if parts.next().is_some() {
        return Err(worktree_usage_error("Usage: /worktree merge <id> [target]"));
    }
    Ok(WorktreeSlashCommand::Merge {
        worktree_id: worktree_id.to_string(),
        target_ref,
    })
}

fn parse_required_option_value(
    value: &str,
    usage: &'static str,
) -> Result<String, MonitorSlashParseError> {
    if value.trim().is_empty() {
        Err(worktree_usage_error(usage))
    } else {
        Ok(value.to_string())
    }
}

fn parse_background_agent_start_args(
    input: &str,
) -> Result<BackgroundAgentSlashCommand, MonitorSlashParseError> {
    let mut remaining = input.trim();
    let mut worktree_id = None;

    if let Some(after_equals) = remaining.strip_prefix("--worktree=") {
        let value_end = after_equals
            .find(char::is_whitespace)
            .unwrap_or(after_equals.len());
        let value = &after_equals[..value_end];
        if value.is_empty() {
            return Err(background_agent_usage_error(
                "Usage: /background-agent start [--worktree <id>] <prompt>",
            ));
        }
        worktree_id = Some(value.to_string());
        remaining = after_equals[value_end..].trim_start();
    } else if let Some(after_flag) = remaining.strip_prefix("--worktree") {
        if !after_flag.is_empty() && !after_flag.starts_with(char::is_whitespace) {
            // Preserve unknown dash-leading prompt text for backwards compatibility.
        } else {
            let after_flag = after_flag.trim_start();
            let value_end = after_flag
                .find(char::is_whitespace)
                .unwrap_or(after_flag.len());
            let value = &after_flag[..value_end];
            if value.is_empty() || value.starts_with('-') {
                return Err(background_agent_usage_error(
                    "Usage: /background-agent start [--worktree <id>] <prompt>",
                ));
            }
            worktree_id = Some(value.to_string());
            remaining = after_flag[value_end..].trim_start();
        }
    }

    if let Some(after_terminator) = remaining.strip_prefix("--")
        && (after_terminator.is_empty() || after_terminator.starts_with(char::is_whitespace))
    {
        remaining = after_terminator.trim_start();
    }

    let prompt = remaining.trim();
    if prompt.trim().is_empty() {
        return Err(background_agent_usage_error(
            "Usage: /background-agent start [--worktree <id>] <prompt>",
        ));
    }
    Ok(BackgroundAgentSlashCommand::Start {
        prompt: prompt.to_string(),
        worktree_id,
    })
}

fn parse_active_session_slash_args(
    input: &str,
) -> Result<Option<ActiveSessionSlashCommand>, MonitorSlashParseError> {
    let trimmed = input.trim();
    let Some(first) = trimmed.split_whitespace().next() else {
        return Ok(None);
    };
    let rest = trimmed[first.len()..].trim();

    match first.to_ascii_lowercase().as_str() {
        "peers" | "sessions" | "active" => {
            if rest.is_empty() {
                Ok(Some(ActiveSessionSlashCommand::List))
            } else {
                Err(active_session_usage_error("Usage: /agent peers"))
            }
        }
        "send" | "message" | "msg" => parse_active_session_send_args(rest).map(Some),
        _ => Ok(None),
    }
}

fn parse_active_session_send_args(
    input: &str,
) -> Result<ActiveSessionSlashCommand, MonitorSlashParseError> {
    let Some(parts) = shlex::split(input) else {
        return Err(active_session_usage_error(
            "Check quotes in the active-session message.",
        ));
    };
    let mut wake = false;
    let mut index = 0;
    while index < parts.len() {
        match parts[index].as_str() {
            "--wake" | "-w" => {
                wake = true;
                index += 1;
            }
            flag if flag.starts_with('-') => {
                return Err(active_session_usage_error(ACTIVE_SESSION_SEND_USAGE));
            }
            _ => break,
        }
    }
    if parts.len().saturating_sub(index) < 2 {
        return Err(active_session_usage_error(ACTIVE_SESSION_SEND_USAGE));
    }
    let target_peer_id = parts[index].clone();
    let message = parts[index + 1..].join(" ");
    if message.trim().is_empty() {
        return Err(active_session_usage_error(ACTIVE_SESSION_SEND_USAGE));
    }
    Ok(ActiveSessionSlashCommand::Send {
        target_peer_id,
        message,
        wake,
    })
}

fn parse_optional_background_agent_id(
    rest: &str,
    usage: &'static str,
) -> Result<Option<String>, MonitorSlashParseError> {
    if rest.is_empty() {
        return Ok(None);
    }
    let mut parts = rest.split_whitespace();
    let Some(agent_id) = parts.next() else {
        return Ok(None);
    };
    if parts.next().is_some() {
        return Err(background_agent_usage_error(usage));
    }
    Ok(Some(agent_id.to_string()))
}

fn parse_optional_worktree_id(
    rest: &str,
    usage: &'static str,
) -> Result<Option<String>, MonitorSlashParseError> {
    if rest.is_empty() {
        return Ok(None);
    }
    parse_required_worktree_id(rest, usage).map(Some)
}

fn parse_required_worktree_id(
    rest: &str,
    usage: &'static str,
) -> Result<String, MonitorSlashParseError> {
    let mut parts = rest.split_whitespace();
    let Some(worktree_id) = parts.next() else {
        return Err(worktree_usage_error(usage));
    };
    if parts.next().is_some() {
        return Err(worktree_usage_error(usage));
    }
    Ok(worktree_id.to_string())
}

fn active_session_usage_error(usage: &'static str) -> MonitorSlashParseError {
    MonitorSlashParseError {
        message: usage.to_string(),
        hint: Some(BACKGROUND_AGENT_USAGE_HINT.to_string()),
    }
}

fn background_agent_usage_error(usage: &'static str) -> MonitorSlashParseError {
    MonitorSlashParseError {
        message: usage.to_string(),
        hint: Some(BACKGROUND_AGENT_USAGE_HINT.to_string()),
    }
}

fn worktree_usage_error(usage: &'static str) -> MonitorSlashParseError {
    MonitorSlashParseError {
        message: usage.to_string(),
        hint: Some(WORKTREE_USAGE_HINT.to_string()),
    }
}
fn loop_create_request_to_api(
    request: LoopCreateRequest,
) -> Result<(String, ThreadSchedulePromptSource, ThreadScheduleSpec), ()> {
    let (prompt, prompt_source) = match request.prompt {
        LoopPrompt::Inline(prompt) => (prompt, ThreadSchedulePromptSource::Inline),
        LoopPrompt::Default => (
            DEFAULT_LOOP_PROMPT_DISPLAY.to_string(),
            ThreadSchedulePromptSource::Default,
        ),
    };
    let schedule = match request.schedule {
        LoopSchedule::Once(_) => return Err(()),
        LoopSchedule::Dynamic => ThreadScheduleSpec::Dynamic,
        LoopSchedule::Interval(interval) => ThreadScheduleSpec::Interval {
            amount: i64::from(interval.amount),
            unit: match interval.unit {
                LoopIntervalUnit::Minutes => ApiThreadScheduleIntervalUnit::Minutes,
                LoopIntervalUnit::Hours => ApiThreadScheduleIntervalUnit::Hours,
                LoopIntervalUnit::Days => ApiThreadScheduleIntervalUnit::Days,
            },
        },
        LoopSchedule::Cron(cron) => ThreadScheduleSpec::Cron {
            expression: cron.expression,
        },
    };
    Ok((prompt, prompt_source, schedule))
}

fn schedule_create_request_to_api(
    request: LoopCreateRequest,
) -> Result<
    (
        String,
        ThreadSchedulePromptSource,
        ThreadScheduleSpec,
        Option<i64>,
    ),
    (),
> {
    let (prompt, prompt_source) = match request.prompt {
        LoopPrompt::Inline(prompt) => (prompt, ThreadSchedulePromptSource::Inline),
        LoopPrompt::Default => (
            DEFAULT_LOOP_PROMPT_DISPLAY.to_string(),
            ThreadSchedulePromptSource::Default,
        ),
    };
    match request.schedule {
        LoopSchedule::Once(time) => {
            let next_run_at = match time {
                ScheduleTime::Delay(interval) => schedule_delay_next_run_at(&interval).ok_or(())?,
                ScheduleTime::At(next_run_at) => next_run_at,
            };
            Ok((
                prompt,
                prompt_source,
                ThreadScheduleSpec::Once,
                Some(next_run_at),
            ))
        }
        LoopSchedule::Dynamic | LoopSchedule::Interval(_) | LoopSchedule::Cron(_) => Err(()),
    }
}

fn schedule_delay_next_run_at(interval: &super::loop_slash::LoopInterval) -> Option<i64> {
    let amount = i64::from(interval.amount);
    let seconds = match interval.unit {
        LoopIntervalUnit::Minutes => amount.checked_mul(60)?,
        LoopIntervalUnit::Hours => amount.checked_mul(60 * 60)?,
        LoopIntervalUnit::Days => amount.checked_mul(24 * 60 * 60)?,
    };
    Utc::now().timestamp().checked_add(seconds)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ServiceTierStateArg {
    On,
    Off,
}

fn parse_service_tier_state_arg(args: &str) -> Option<ServiceTierStateArg> {
    match args.trim().to_ascii_lowercase().as_str() {
        "on" | "enable" | "enabled" => Some(ServiceTierStateArg::On),
        "off" | "disable" | "disabled" | "default" => Some(ServiceTierStateArg::Off),
        _ => None,
    }
}

#[cfg(test)]
mod external_agent_arg_tests {
    use super::BackgroundAgentSlashCommand;
    use super::ExternalAgentSlashCommand;
    use super::TmuxSlashCommand;
    use super::WorktreeSlashCommand;
    use super::parse_background_agent_slash_args;
    use super::parse_external_agent_args;
    use super::parse_tmux_slash_args;
    use super::parse_worktree_slash_args;
    use crate::tmux_handoff::TmuxHandoffDestination;
    use codex_app_server_protocol::ThreadExternalAgentMode;

    #[test]
    fn parses_external_agent_runtime_and_prompt() {
        assert_eq!(
            parse_external_agent_args("grok-build inspect this repo"),
            Some(ExternalAgentSlashCommand::Runtime {
                runtime_id: "grok-build",
                prompt: "inspect this repo",
                mode: ThreadExternalAgentMode::Plan,
                inline: false,
            })
        );
        assert_eq!(
            parse_external_agent_args("cursor"),
            Some(ExternalAgentSlashCommand::Runtime {
                runtime_id: "cursor",
                prompt: "",
                mode: ThreadExternalAgentMode::Plan,
                inline: false,
            })
        );
        assert_eq!(
            parse_external_agent_args("claude inspect this repo"),
            Some(ExternalAgentSlashCommand::Runtime {
                runtime_id: "claude",
                prompt: "inspect this repo",
                mode: ThreadExternalAgentMode::Plan,
                inline: false,
            })
        );
        assert_eq!(parse_external_agent_args("   "), None);
    }

    #[test]
    fn parses_external_agent_inline_modes() {
        assert_eq!(
            parse_external_agent_args("inline propose claude inspect this repo"),
            Some(ExternalAgentSlashCommand::Runtime {
                runtime_id: "claude",
                prompt: "inspect this repo",
                mode: ThreadExternalAgentMode::Propose,
                inline: true,
            })
        );
        assert_eq!(
            parse_external_agent_args("inline --mode=propose claude inspect this repo"),
            Some(ExternalAgentSlashCommand::Runtime {
                runtime_id: "claude",
                prompt: "inspect this repo",
                mode: ThreadExternalAgentMode::Propose,
                inline: true,
            })
        );
    }

    #[test]
    fn parses_tmux_name_and_replace_flags() {
        assert_eq!(
            parse_tmux_slash_args("worktree").expect("tmux args"),
            TmuxSlashCommand {
                destination: TmuxHandoffDestination::NewSession {
                    name: Some("worktree".to_string()),
                },
                replace_existing: true,
            }
        );
        assert_eq!(
            parse_tmux_slash_args("--no-replace \"named session\"").expect("tmux args"),
            TmuxSlashCommand {
                destination: TmuxHandoffDestination::NewSession {
                    name: Some("named session".to_string()),
                },
                replace_existing: false,
            }
        );
        assert_eq!(
            parse_tmux_slash_args("-r named session").expect("tmux args"),
            TmuxSlashCommand {
                destination: TmuxHandoffDestination::NewSession {
                    name: Some("named session".to_string()),
                },
                replace_existing: true,
            }
        );
        assert_eq!(
            parse_tmux_slash_args("--session dev --window \"Codewith main\"").expect("tmux args"),
            TmuxSlashCommand {
                destination: TmuxHandoffDestination::ExistingSession {
                    session_name: "dev".to_string(),
                    window_name: Some("Codewith main".to_string()),
                },
                replace_existing: true,
            }
        );
        assert_eq!(
            parse_tmux_slash_args("--session=dev").expect("tmux args"),
            TmuxSlashCommand {
                destination: TmuxHandoffDestination::ExistingSession {
                    session_name: "dev".to_string(),
                    window_name: None,
                },
                replace_existing: true,
            }
        );
        assert!(parse_tmux_slash_args("--bad").is_err());
        assert!(parse_tmux_slash_args("--window codewith").is_err());
        assert!(parse_tmux_slash_args("--session dev stray-name").is_err());
    }

    #[test]
    fn parses_worktree_manager_commands() {
        assert_eq!(
            parse_worktree_slash_args("").expect("empty worktree args"),
            WorktreeSlashCommand::List
        );
        assert_eq!(
            parse_worktree_slash_args("list").expect("list args"),
            WorktreeSlashCommand::List
        );
        assert_eq!(
            parse_worktree_slash_args("reconcile").expect("reconcile args"),
            WorktreeSlashCommand::Reconcile
        );
        assert_eq!(
            parse_worktree_slash_args("create feature --branch codewith/feature --start main")
                .expect("create args"),
            WorktreeSlashCommand::Create {
                name: Some("feature".to_string()),
                branch: Some("codewith/feature".to_string()),
                start_point: Some("main".to_string()),
            }
        );
        assert_eq!(
            parse_worktree_slash_args("read").expect("read args"),
            WorktreeSlashCommand::Read { worktree_id: None }
        );
        assert_eq!(
            parse_worktree_slash_args("show wt-123").expect("show args"),
            WorktreeSlashCommand::Read {
                worktree_id: Some("wt-123".to_string()),
            }
        );
        assert_eq!(
            parse_worktree_slash_args("manage wt-456").expect("manage args"),
            WorktreeSlashCommand::Actions {
                worktree_id: "wt-456".to_string(),
            }
        );
        assert_eq!(
            parse_worktree_slash_args("use wt-789").expect("use args"),
            WorktreeSlashCommand::Use {
                worktree_id: "wt-789".to_string(),
            }
        );
        assert_eq!(
            parse_worktree_slash_args("release wt-999").expect("release args"),
            WorktreeSlashCommand::Release {
                worktree_id: "wt-999".to_string(),
            }
        );
        assert_eq!(
            parse_worktree_slash_args("cleanup --force wt-999").expect("cleanup args"),
            WorktreeSlashCommand::Cleanup {
                worktree_id: "wt-999".to_string(),
                force_delete: true,
            }
        );
        assert_eq!(
            parse_worktree_slash_args("merge wt-999 main").expect("merge args"),
            WorktreeSlashCommand::Merge {
                worktree_id: "wt-999".to_string(),
                target_ref: Some("main".to_string()),
            }
        );
    }

    #[test]
    fn rejects_invalid_worktree_manager_commands() {
        assert!(parse_worktree_slash_args("list extra").is_err());
        assert!(parse_worktree_slash_args("reconcile extra").is_err());
        assert!(parse_worktree_slash_args("actions").is_err());
        assert!(parse_worktree_slash_args("use").is_err());
        assert!(parse_worktree_slash_args("read one two").is_err());
        assert!(parse_worktree_slash_args("cleanup").is_err());
        assert!(parse_worktree_slash_args("merge").is_err());
        assert!(parse_worktree_slash_args("create one two").is_err());
        assert!(parse_worktree_slash_args("unknown").is_err());
    }

    #[test]
    fn parses_background_agent_start_worktree_flag() {
        assert_eq!(
            parse_background_agent_slash_args("start fix tests").expect("plain start"),
            BackgroundAgentSlashCommand::Start {
                prompt: "fix tests".to_string(),
                worktree_id: None,
            }
        );
        assert_eq!(
            parse_background_agent_slash_args("start --worktree wt-123 fix tests")
                .expect("worktree flag"),
            BackgroundAgentSlashCommand::Start {
                prompt: "fix tests".to_string(),
                worktree_id: Some("wt-123".to_string()),
            }
        );
        assert_eq!(
            parse_background_agent_slash_args("start --worktree=wt-456 fix tests")
                .expect("worktree equals flag"),
            BackgroundAgentSlashCommand::Start {
                prompt: "fix tests".to_string(),
                worktree_id: Some("wt-456".to_string()),
            }
        );
        assert_eq!(
            parse_background_agent_slash_args("start --worktree wt-123   fix \"quoted\"  tests")
                .expect("worktree preserves raw prompt"),
            BackgroundAgentSlashCommand::Start {
                prompt: "fix \"quoted\"  tests".to_string(),
                worktree_id: Some("wt-123".to_string()),
            }
        );
        assert_eq!(
            parse_background_agent_slash_args("start --audit config loading")
                .expect("dash-leading prompt"),
            BackgroundAgentSlashCommand::Start {
                prompt: "--audit config loading".to_string(),
                worktree_id: None,
            }
        );
        assert_eq!(
            parse_background_agent_slash_args("start -- --prompt-looking text")
                .expect("flag terminator"),
            BackgroundAgentSlashCommand::Start {
                prompt: "--prompt-looking text".to_string(),
                worktree_id: None,
            }
        );
        assert_eq!(
            parse_background_agent_slash_args("start --worktree wt-123 -- --prompt-looking text")
                .expect("worktree flag terminator"),
            BackgroundAgentSlashCommand::Start {
                prompt: "--prompt-looking text".to_string(),
                worktree_id: Some("wt-123".to_string()),
            }
        );
    }

    #[test]
    fn rejects_invalid_background_agent_start_worktree_flag() {
        assert!(parse_background_agent_slash_args("start --worktree").is_err());
        assert!(parse_background_agent_slash_args("start --worktree wt-123").is_err());
    }
}
