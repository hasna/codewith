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
        header.push(Line::from("Choose a communication style for Codex.".dim()));

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
            subtitle: Some("Configure settings for Codex.".to_string()),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            ..Default::default()
        });
    }

    pub(crate) fn open_config_popup(&mut self) {
        let items = vec![
            SelectionItem {
                name: "Update checks".to_string(),
                description: Some(
                    "Off for this internal app. Updates come from explicit internal releases."
                        .to_string(),
                ),
                selected_description: Some(
                    "Managed by iapp-codex and cannot be enabled here.".to_string(),
                ),
                is_current: true,
                is_disabled: true,
                disabled_reason: Some("Managed by iapp-codex.".to_string()),
                toggle_placeholder: Some("[ ] "),
                ..Default::default()
            },
            config_toggle_item(
                "Auth profile auto-switch",
                "Switch to another configured profile after rate limits are exhausted.",
                "auth_profile_auto_switch.enabled",
                self.config.auth_profile_auto_switch.enabled,
                None,
            ),
            config_toggle_item(
                "Switch on 5h limit",
                "Allow auto-switching when the five-hour limit is exhausted.",
                "auth_profile_auto_switch.on_5h_limit",
                self.config.auth_profile_auto_switch.on_5h_limit,
                None,
            ),
            config_toggle_item(
                "Switch on weekly limit",
                "Allow auto-switching when the weekly limit is exhausted.",
                "auth_profile_auto_switch.on_weekly_limit",
                self.config.auth_profile_auto_switch.on_weekly_limit,
                None,
            ),
            config_toggle_item(
                "Paste burst detection",
                "Detect fast pasted input before inserting it into the composer.",
                "disable_paste_burst",
                !self.config.disable_paste_burst,
                Some(Box::new(|enabled| serde_json::json!(!enabled))),
            ),
            config_toggle_item(
                "Hide reasoning summaries",
                "Hide agent reasoning events from the transcript.",
                "hide_agent_reasoning",
                self.config.hide_agent_reasoning,
                None,
            ),
            config_toggle_item(
                "Show raw reasoning",
                "Show raw reasoning content when the model emits it.",
                "show_raw_agent_reasoning",
                self.config.show_raw_agent_reasoning,
                None,
            ),
            config_toggle_item(
                "Environment context",
                "Include the environment_context block in model-visible context.",
                "include_environment_context",
                self.config.include_environment_context,
                None,
            ),
            config_toggle_item(
                "Permission instructions",
                "Include current sandbox and approval instructions in model-visible context.",
                "include_permissions_instructions",
                self.config.include_permissions_instructions,
                None,
            ),
            config_toggle_item(
                "App instructions",
                "Include app and tool-surface instructions in model-visible context.",
                "include_apps_instructions",
                self.config.include_apps_instructions,
                None,
            ),
            config_toggle_item(
                "Collaboration instructions",
                "Include collaboration-mode instructions in model-visible context.",
                "include_collaboration_mode_instructions",
                self.config.include_collaboration_mode_instructions,
                None,
            ),
            config_toggle_item(
                "Skill instructions",
                "Include installed skill instructions in model-visible context.",
                "skills.include_instructions",
                self.config.include_skill_instructions,
                None,
            ),
            config_toggle_item(
                "Unstable feature warnings",
                "Show warnings for enabled under-development features.",
                "suppress_unstable_features_warning",
                !self.config.suppress_unstable_features_warning,
                Some(Box::new(|enabled| serde_json::json!(!enabled))),
            ),
            config_toggle_item(
                "Analytics",
                "Allow analytics across product surfaces on this machine.",
                "analytics.enabled",
                self.config.analytics_enabled.unwrap_or(true),
                None,
            ),
            config_toggle_item(
                "Feedback",
                "Allow feedback collection from the TUI.",
                "feedback.enabled",
                self.config.feedback_enabled,
                None,
            ),
        ];

        let mut header = ColumnRenderable::new();
        header.push(Line::from("Config".bold()));
        header.push(Line::from(
            "Toggle common config.toml settings for future turns.".dim(),
        ));

        self.bottom_pane.show_selection_view(SelectionViewParams {
            header: Box::new(header),
            footer_hint: Some(Line::from("Press space to toggle; esc to close")),
            items,
            is_searchable: true,
            ..Default::default()
        });
    }

    pub(crate) fn apply_config_popup_value(&mut self, key_path: &str, value: &serde_json::Value) {
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
            "check_for_update_on_startup" => {
                self.config.check_for_update_on_startup = false;
            }
            "disable_paste_burst" => {
                self.config.disable_paste_burst = enabled;
                self.bottom_pane.set_disable_paste_burst(enabled);
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

type ConfigToggleValue = Box<dyn Fn(bool) -> serde_json::Value + Send + Sync>;

fn config_toggle_item(
    label: &'static str,
    description: &'static str,
    key_path: &'static str,
    is_on: bool,
    value_for_state: Option<ConfigToggleValue>,
) -> SelectionItem {
    let value_for_state =
        value_for_state.unwrap_or_else(|| Box::new(|enabled| serde_json::json!(enabled)));
    SelectionItem {
        name: label.to_string(),
        description: Some(description.to_string()),
        toggle: Some(SelectionToggle {
            is_on,
            action: Box::new(move |enabled, tx| {
                tx.send(AppEvent::UpdateConfigValue {
                    key_path: key_path.to_string(),
                    value: value_for_state(enabled),
                    label: label.to_string(),
                });
            }),
        }),
        ..Default::default()
    }
}
