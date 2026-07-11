//! Settings-adjacent popup surfaces for `ChatWidget`.
//!
//! This keeps theme, personality, audio-device, and experimental-feature UI
//! out of the main orchestration module without changing their event wiring.

use super::*;

impl ChatWidget {
    pub(super) fn open_theme_picker(&mut self) {
        let codex_home = codex_utils_home_dir::find_codex_home().ok();
        let terminal_width = self
            .last_rendered_width
            .get()
            .and_then(|width| u16::try_from(width).ok());
        let params = crate::theme_picker::build_theme_picker_params(
            self.config.tui_theme.as_deref(),
            codex_home.as_deref(),
            terminal_width,
        );
        self.bottom_pane.show_selection_view(params);
    }

    pub(crate) fn open_personality_popup(&mut self) {
        if !self.is_session_configured() {
            self.add_info_message(
                "Personality selection is disabled until startup completes.".to_string(),
                /*hint*/ None,
            );
            return;
        }
        if !self.current_model_supports_personality() {
            let current_model = self.current_model();
            self.add_error_message(format!(
                "Current model ({current_model}) doesn't support personalities. Try /model to pick a different model."
            ));
            return;
        }
        self.open_personality_popup_for_current_model();
    }

    fn open_personality_popup_for_current_model(&mut self) {
        let current_personality = self.config.personality.unwrap_or(Personality::Friendly);
        let personalities = [Personality::Friendly, Personality::Pragmatic];
        let supports_personality = self.current_model_supports_personality();

        let items: Vec<SelectionItem> = personalities
            .into_iter()
            .map(|personality| {
                let name = Self::personality_label(personality).to_string();
                let description = Some(Self::personality_description(personality).to_string());
                let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
                    tx.send(AppEvent::CodexOp(AppCommand::override_turn_context(
                        /*cwd*/ None,
                        /*approval_policy*/ None,
                        /*approvals_reviewer*/ None,
                        /*permission_profile*/ None,
                        /*active_permission_profile*/ None,
                        /*windows_sandbox_level*/ None,
                        /*model*/ None,
                        /*effort*/ None,
                        /*summary*/ None,
                        /*service_tier*/ None,
                        /*collaboration_mode*/ None,
                        /*session_prompt*/ None,
                        Some(personality),
                    )));
                    tx.send(AppEvent::UpdatePersonality(personality));
                    tx.send(AppEvent::PersistPersonalitySelection { personality });
                })];
                SelectionItem {
                    name,
                    description,
                    is_current: current_personality == personality,
                    is_disabled: !supports_personality,
                    actions,
                    dismiss_on_select: true,
                    ..Default::default()
                }
            })
            .collect();

        let mut header = ColumnRenderable::new();
        header.push(Line::from("Select Personality".bold()));
        header.push(Line::from(
            "Choose a communication style for Codewith.".dim(),
        ));

        self.bottom_pane.show_selection_view(SelectionViewParams {
            header: Box::new(header),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            ..Default::default()
        });
    }

    pub(crate) fn open_realtime_audio_popup(&mut self) {
        let items = [
            RealtimeAudioDeviceKind::Microphone,
            RealtimeAudioDeviceKind::Speaker,
        ]
        .into_iter()
        .map(|kind| {
            let description = Some(format!(
                "Current: {}",
                self.current_realtime_audio_selection_label(kind)
            ));
            let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
                tx.send(AppEvent::OpenRealtimeAudioDeviceSelection { kind });
            })];
            SelectionItem {
                name: kind.title().to_string(),
                description,
                actions,
                dismiss_on_select: true,
                ..Default::default()
            }
        })
        .collect();

        self.bottom_pane.show_selection_view(SelectionViewParams {
            title: Some("Settings".to_string()),
            subtitle: Some("Configure settings for Codewith.".to_string()),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            ..Default::default()
        });
    }

    pub(crate) fn open_config_popup(&mut self) {
        let mut items: Vec<SelectionItem> = crate::common_config_options::common_config_sections()
            .iter()
            .copied()
            .map(|section| config_section_item(&self.config, section))
            .collect();
        items.push(self.agent_max_threads_menu_item());

        let mut header = ColumnRenderable::new();
        header.push(Line::from("Config".bold()));
        header.push(Line::from(
            "Choose a focused config.toml settings section.".dim(),
        ));

        self.bottom_pane.show_selection_view(SelectionViewParams {
            header: Box::new(header),
            footer_hint: Some(Line::from("Press enter to open; esc to close")),
            items,
            ..Default::default()
        });
    }

    /// Root-menu entry that opens the agent subagent-thread-limit picker.
    ///
    /// When `multi_agent_v2` is enabled the limit is governed by that feature, so the entry is
    /// shown disabled with an explanatory reason rather than writing a conflicting legacy key.
    fn agent_max_threads_menu_item(&self) -> SelectionItem {
        let multi_agent_v2 = self.config.features.enabled(Feature::MultiAgentV2);
        let description = if multi_agent_v2 {
            "Governed by multi_agent_v2 (features.multi_agent_v2.max_concurrent_threads_per_session).".to_string()
        } else {
            match self.config.agent_max_threads {
                Some(threads) => format!(
                    "Max concurrent subagent threads per agent run (currently {threads})."
                ),
                None => {
                    "Max concurrent subagent threads per agent run (currently the built-in default)."
                        .to_string()
                }
            }
        };
        let actions: Vec<SelectionAction> = if multi_agent_v2 {
            Vec::new()
        } else {
            vec![Box::new(|tx| {
                tx.send(AppEvent::OpenAgentMaxThreadsMenu);
            })]
        };
        SelectionItem {
            name: "Agent subagent threads".to_string(),
            description: Some(description),
            is_disabled: multi_agent_v2,
            disabled_reason: multi_agent_v2.then(|| "Managed by multi_agent_v2.".to_string()),
            actions,
            dismiss_on_select: true,
            ..Default::default()
        }
    }

    /// Preset picker for `[agents] max_threads`. Selecting a value persists it to `config.toml`
    /// (via the shared config-write path) and reports that a restart is required to apply it.
    pub(crate) fn open_agent_max_threads_popup(&mut self) {
        if self.config.features.enabled(Feature::MultiAgentV2) {
            self.add_info_message(
                "The subagent thread limit is governed by multi_agent_v2 (features.multi_agent_v2.max_concurrent_threads_per_session); agents.max_threads is not used while multi_agent_v2 is enabled.".to_string(),
                /*hint*/ None,
            );
            return;
        }

        // Mirrors the runtime default cap. When unset, that default is the effective cap, so it is
        // preselected here.
        const DEFAULT_AGENT_MAX_THREADS: usize = 6;
        const PRESETS: [usize; 7] = [1, 2, 3, 4, 6, 8, 12];
        let current = self.config.agent_max_threads;

        let items: Vec<SelectionItem> = PRESETS
            .into_iter()
            .map(|threads| {
                let is_current = current == Some(threads)
                    || (current.is_none() && threads == DEFAULT_AGENT_MAX_THREADS);
                let name = if threads == DEFAULT_AGENT_MAX_THREADS {
                    format!("{threads} (default)")
                } else {
                    threads.to_string()
                };
                let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
                    tx.send(AppEvent::UpdateConfigValue {
                        key_path: "agents.max_threads".to_string(),
                        value: serde_json::json!(threads),
                        label: "Agent subagent thread limit".to_string(),
                    });
                })];
                SelectionItem {
                    name,
                    description: Some(format!(
                        "Allow up to {threads} concurrent subagent threads per agent run."
                    )),
                    is_current,
                    actions,
                    dismiss_on_select: true,
                    ..Default::default()
                }
            })
            .collect();

        let mut header = ColumnRenderable::new();
        header.push(Line::from("Agent subagent threads".bold()));
        header.push(Line::from(
            "Cap concurrent subagent threads per agent run; restart to apply.".dim(),
        ));

        self.bottom_pane.show_selection_view(SelectionViewParams {
            header: Box::new(header),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            ..Default::default()
        });
    }

    pub(crate) fn open_config_section_popup(
        &mut self,
        section: crate::common_config_options::CommonConfigSection,
    ) {
        let mut items: Vec<SelectionItem> =
            crate::common_config_options::common_config_options_for_section(&self.config, section)
                .into_iter()
                .map(config_selection_item)
                .collect();
        items.push(back_to_config_menu_item());

        let mut header = ColumnRenderable::new();
        header.push(Line::from(format!("Config: {}", section.label()).bold()));
        header.push(Line::from(section.description().dim()));

        self.bottom_pane.show_selection_view(SelectionViewParams {
            header: Box::new(header),
            footer_hint: Some(Line::from("Press space to toggle; esc to close")),
            items,
            is_searchable: true,
            search_placeholder: Some(format!("Search {} settings", section.label())),
            ..Default::default()
        });
    }

    pub(crate) fn apply_config_popup_value(&mut self, key_path: &str, value: &serde_json::Value) {
        if key_path == "agents.max_threads" {
            if let Some(threads) = value.as_u64() {
                self.config.agent_max_threads = Some(threads as usize);
            }
            self.add_info_message(
                "Agent subagent thread limit saved. Restart the session to apply it (or set [agents] max_threads in config.toml / use -c agents.max_threads=N).".to_string(),
                /*hint*/ None,
            );
            return;
        }

        if key_path == "goals.auto_execute" {
            let Some(value) = value.as_str() else {
                return;
            };
            self.config.goals.auto_execute = match value {
                "ai-directed" => crate::legacy_core::config::GoalAutoExecuteMode::AiDirected,
                "ready-only" => crate::legacy_core::config::GoalAutoExecuteMode::ReadyOnly,
                "off" => crate::legacy_core::config::GoalAutoExecuteMode::Off,
                _ => self.config.goals.auto_execute,
            };
            return;
        }

        let Some(enabled) = value.as_bool() else {
            return;
        };
        match key_path {
            "auth_profile_auto_switch.enabled" => {
                self.config.auth_profile_auto_switch.enabled = enabled;
            }
            "auth_profile_auto_switch.on_5h_limit" => {
                self.config.auth_profile_auto_switch.on_5h_limit = enabled;
            }
            "auth_profile_auto_switch.on_weekly_limit" => {
                self.config.auth_profile_auto_switch.on_weekly_limit = enabled;
            }
            "usage_limit.auto_reset_enabled" => {
                self.config.usage_limit.auto_reset_enabled = enabled;
            }
            "check_for_update_on_startup" => {
                self.config.check_for_update_on_startup = false;
            }
            "disable_paste_burst" => {
                self.config.disable_paste_burst = enabled;
                self.bottom_pane.set_disable_paste_burst(enabled);
            }
            "session_recap.enabled" => {
                self.config.session_recap.enabled = enabled;
            }
            "tui.animations" => {
                self.config.animations = enabled;
                self.bottom_pane.set_animations_enabled(enabled);
            }
            "tui.show_tooltips" => {
                self.config.show_tooltips = enabled;
            }
            "hide_agent_reasoning" => {
                self.config.hide_agent_reasoning = enabled;
            }
            "show_raw_agent_reasoning" => {
                self.config.show_raw_agent_reasoning = enabled;
            }
            "include_environment_context" => {
                self.config.include_environment_context = enabled;
            }
            "include_permissions_instructions" => {
                self.config.include_permissions_instructions = enabled;
            }
            "include_apps_instructions" => {
                self.config.include_apps_instructions = enabled;
            }
            "include_collaboration_mode_instructions" => {
                self.config.include_collaboration_mode_instructions = enabled;
            }
            "skills.include_instructions" => {
                self.config.include_skill_instructions = enabled;
            }
            "suppress_unstable_features_warning" => {
                self.config.suppress_unstable_features_warning = enabled;
            }
            "analytics.enabled" => {
                self.config.analytics_enabled = Some(enabled);
            }
            "feedback.enabled" => {
                self.config.feedback_enabled = enabled;
            }
            _ => {}
        }
        self.refresh_status_surfaces();
    }

    #[cfg(not(target_os = "linux"))]
    pub(crate) fn open_realtime_audio_device_selection(&mut self, kind: RealtimeAudioDeviceKind) {
        match list_realtime_audio_device_names(kind) {
            Ok(device_names) => {
                self.open_realtime_audio_device_selection_with_names(kind, device_names);
            }
            Err(err) => {
                self.add_error_message(format!(
                    "Failed to load realtime {} devices: {err}",
                    kind.noun()
                ));
            }
        }
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn open_realtime_audio_device_selection(&mut self, kind: RealtimeAudioDeviceKind) {
        let _ = kind;
    }

    #[cfg(not(target_os = "linux"))]
    pub(super) fn open_realtime_audio_device_selection_with_names(
        &mut self,
        kind: RealtimeAudioDeviceKind,
        device_names: Vec<String>,
    ) {
        let current_selection = self.current_realtime_audio_device_name(kind);
        let current_available = current_selection
            .as_deref()
            .is_some_and(|name| device_names.iter().any(|device_name| device_name == name));
        let mut items = vec![SelectionItem {
            name: "System default".to_string(),
            description: Some("Use your operating system default device.".to_string()),
            is_current: current_selection.is_none(),
            actions: vec![Box::new(move |tx| {
                tx.send(AppEvent::PersistRealtimeAudioDeviceSelection { kind, name: None });
            })],
            dismiss_on_select: true,
            ..Default::default()
        }];

        if let Some(selection) = current_selection.as_deref()
            && !current_available
        {
            items.push(SelectionItem {
                name: format!("Unavailable: {selection}"),
                description: Some("Configured device is not currently available.".to_string()),
                is_current: true,
                is_disabled: true,
                disabled_reason: Some("Reconnect the device or choose another one.".to_string()),
                ..Default::default()
            });
        }

        items.extend(device_names.into_iter().map(|device_name| {
            let persisted_name = device_name.clone();
            let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
                tx.send(AppEvent::PersistRealtimeAudioDeviceSelection {
                    kind,
                    name: Some(persisted_name.clone()),
                });
            })];
            SelectionItem {
                is_current: current_selection.as_deref() == Some(device_name.as_str()),
                name: device_name,
                actions,
                dismiss_on_select: true,
                ..Default::default()
            }
        }));

        let mut header = ColumnRenderable::new();
        header.push(Line::from(format!("Select {}", kind.title()).bold()));
        header.push(Line::from(
            "Saved devices apply to realtime voice only.".dim(),
        ));

        self.bottom_pane.show_selection_view(SelectionViewParams {
            header: Box::new(header),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            ..Default::default()
        });
    }

    pub(crate) fn open_realtime_audio_restart_prompt(&mut self, kind: RealtimeAudioDeviceKind) {
        let restart_actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
            tx.send(AppEvent::RestartRealtimeAudioDevice { kind });
        })];
        let items = vec![
            SelectionItem {
                name: "Restart now".to_string(),
                description: Some(format!("Restart local {} audio now.", kind.noun())),
                actions: restart_actions,
                dismiss_on_select: true,
                ..Default::default()
            },
            SelectionItem {
                name: "Apply later".to_string(),
                description: Some(format!(
                    "Keep the current {} until local audio starts again.",
                    kind.noun()
                )),
                dismiss_on_select: true,
                ..Default::default()
            },
        ];

        let mut header = ColumnRenderable::new();
        header.push(Line::from(format!("Restart {} now?", kind.title()).bold()));
        header.push(Line::from(
            "Configuration is saved. Restart local audio to use it immediately.".dim(),
        ));

        self.bottom_pane.show_selection_view(SelectionViewParams {
            header: Box::new(header),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            ..Default::default()
        });
    }

    pub(crate) fn open_experimental_popup(&mut self) {
        let features: Vec<ExperimentalFeatureItem> = FEATURES
            .iter()
            .filter_map(|spec| {
                let name = spec.stage.experimental_menu_name()?;
                let description = spec.stage.experimental_menu_description()?;
                Some(ExperimentalFeatureItem {
                    feature: spec.id,
                    name: name.to_string(),
                    description: description.to_string(),
                    enabled: self.config.features.enabled(spec.id),
                })
            })
            .collect();

        let view = ExperimentalFeaturesView::new(
            features,
            self.app_event_tx.clone(),
            self.bottom_pane.list_keymap(),
        );
        self.bottom_pane.show_view(Box::new(view));
    }

    fn personality_label(personality: Personality) -> &'static str {
        match personality {
            Personality::None => "None",
            Personality::Friendly => "Friendly",
            Personality::Pragmatic => "Pragmatic",
        }
    }

    fn personality_description(personality: Personality) -> &'static str {
        match personality {
            Personality::None => "No personality instructions.",
            Personality::Friendly => "Warm, collaborative, and helpful.",
            Personality::Pragmatic => "Concise, task-focused, and direct.",
        }
    }
}

fn config_section_item(
    config: &crate::legacy_core::config::Config,
    section: crate::common_config_options::CommonConfigSection,
) -> SelectionItem {
    let setting_count =
        crate::common_config_options::common_config_options_for_section(config, section).len();
    let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
        tx.send(AppEvent::OpenConfigSection { section });
    })];
    SelectionItem {
        name: section.label().to_string(),
        description: Some(format!(
            "{} {setting_count} settings.",
            section.description()
        )),
        actions,
        dismiss_on_select: true,
        search_value: Some(format!(
            "{} {} {}",
            section.id(),
            section.label(),
            section.description()
        )),
        ..Default::default()
    }
}

fn back_to_config_menu_item() -> SelectionItem {
    let actions: Vec<SelectionAction> = vec![Box::new(|tx| {
        tx.send(AppEvent::OpenConfigMenu);
    })];
    SelectionItem {
        name: "Back to sections".to_string(),
        description: Some("Return to the config section menu.".to_string()),
        actions,
        dismiss_on_select: true,
        ..Default::default()
    }
}

fn config_selection_item(
    option: crate::common_config_options::CommonConfigOption,
) -> SelectionItem {
    if option.is_disabled() {
        return SelectionItem {
            name: option.label.to_string(),
            description: Some(option.description.to_string()),
            selected_description: Some(
                "Managed by Codewith and cannot be enabled here.".to_string(),
            ),
            is_current: true,
            is_disabled: true,
            disabled_reason: option.disabled_reason.map(str::to_string),
            toggle_placeholder: Some("[ ] "),
            ..Default::default()
        };
    }

    let Some(key_path) = option.key_path else {
        return SelectionItem {
            name: option.label.to_string(),
            description: Some(option.description.to_string()),
            selected_description: Some("This option is not available.".to_string()),
            is_current: option.enabled,
            is_disabled: true,
            disabled_reason: Some("Unavailable.".to_string()),
            toggle_placeholder: Some("[ ] "),
            ..Default::default()
        };
    };

    SelectionItem {
        name: option.label.to_string(),
        description: Some(option.description.to_string()),
        toggle: Some(SelectionToggle {
            is_on: option.enabled,
            action: Box::new(move |enabled, tx| {
                tx.send(AppEvent::UpdateConfigValue {
                    key_path: key_path.to_string(),
                    value: option.value_for_enabled(enabled),
                    label: option.label.to_string(),
                });
            }),
        }),
        ..Default::default()
    }
}
