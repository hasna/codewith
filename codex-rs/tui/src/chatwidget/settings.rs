//! Runtime settings state and model/collaboration coordination for `ChatWidget`.

use super::*;
use crate::app_event::AppEvent;
use codex_app_server_protocol::AuthMode;
use codex_login::AuthDotJson;
use codex_login::AuthProfileSubscriptionProvider;
use codex_login::load_auth_dot_json;
use codex_login::load_auth_profile;
use codex_login::load_auth_profile_metadata;
use codex_model_provider_info::ModelProviderInfo;

fn resolved_auth_status_mode(auth: &AuthDotJson) -> AuthMode {
    if let Some(mode) = auth.auth_mode {
        return mode;
    }
    if auth.openai_api_key.is_some() {
        AuthMode::ApiKey
    } else {
        AuthMode::Chatgpt
    }
}

fn status_account_display_from_auth(
    auth_mode: AuthMode,
    auth: &AuthDotJson,
) -> StatusAccountDisplay {
    match auth_mode {
        AuthMode::ApiKey => StatusAccountDisplay::ApiKey,
        AuthMode::Chatgpt | AuthMode::ChatgptAuthTokens | AuthMode::PersonalAccessToken => {
            let (email, plan) = auth
                .tokens
                .as_ref()
                .map(|tokens| {
                    (
                        tokens.id_token.email.clone(),
                        tokens.id_token.get_chatgpt_plan_type(),
                    )
                })
                .unwrap_or_default();
            StatusAccountDisplay::ChatGpt { email, plan }
        }
        AuthMode::AgentIdentity => StatusAccountDisplay::ChatGpt {
            email: None,
            plan: None,
        },
    }
}

impl ChatWidget {
    /// Set the approval policy in the widget's config copy.
    pub(crate) fn set_approval_policy(&mut self, policy: AskForApproval) {
        if let Err(err) = self
            .config
            .permissions
            .approval_policy
            .set(policy.to_core())
        {
            tracing::warn!(%err, "failed to set approval_policy on chat config");
        } else {
            self.refresh_status_surfaces();
        }
    }

    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub(crate) fn set_permission_profile_from_session_snapshot(
        &mut self,
        snapshot: PermissionProfileSnapshot,
    ) -> ConstraintResult<()> {
        self.config
            .permissions
            .set_permission_profile_from_session_snapshot(snapshot)?;
        self.refresh_status_surfaces();
        Ok(())
    }

    pub(crate) fn set_permission_profile_with_active_profile(
        &mut self,
        profile: PermissionProfile,
        active_permission_profile: Option<ActivePermissionProfile>,
    ) -> ConstraintResult<()> {
        self.config
            .permissions
            .set_permission_profile_from_session_snapshot(
                PermissionProfileSnapshot::from_session_snapshot(
                    profile,
                    active_permission_profile,
                ),
            )?;
        self.refresh_status_surfaces();
        Ok(())
    }

    pub(crate) fn set_permission_network(
        &mut self,
        network: Option<crate::legacy_core::config::NetworkProxySpec>,
    ) {
        self.config.permissions.network = network;
    }

    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub(crate) fn set_windows_sandbox_mode(&mut self, mode: Option<WindowsSandboxModeToml>) {
        self.config.permissions.windows_sandbox_mode = mode;
        #[cfg(target_os = "windows")]
        self.bottom_pane.set_windows_degraded_sandbox_active(
            crate::legacy_core::windows_sandbox::ELEVATED_SANDBOX_NUX_ENABLED
                && matches!(
                    WindowsSandboxLevel::from_config(&self.config),
                    WindowsSandboxLevel::RestrictedToken
                ),
        );
    }

    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub(crate) fn set_feature_enabled(&mut self, feature: Feature, enabled: bool) -> bool {
        if let Err(err) = self.config.features.set_enabled(feature, enabled) {
            tracing::warn!(
                error = %err,
                feature = feature.key(),
                "failed to update constrained chat widget feature state"
            );
        }
        let enabled = self.config.features.enabled(feature);
        if feature == Feature::RealtimeConversation {
            let realtime_conversation_enabled = self.realtime_conversation_enabled();
            self.bottom_pane
                .set_realtime_conversation_enabled(realtime_conversation_enabled);
            self.bottom_pane
                .set_audio_device_selection_enabled(self.realtime_audio_device_selection_enabled());
            if !realtime_conversation_enabled && self.realtime_conversation.is_live() {
                self.request_realtime_conversation_close(Some(
                    "Realtime voice mode was closed because the feature was disabled.".to_string(),
                ));
            }
        }
        if feature == Feature::FastMode {
            self.refresh_effective_service_tier();
            self.sync_service_tier_commands();
        }
        if feature == Feature::Personality {
            self.sync_personality_command_enabled();
        }
        if feature == Feature::Plugins {
            self.sync_plugins_command_enabled();
            self.refresh_plugin_mentions();
        }
        if feature == Feature::Goals {
            self.sync_goal_command_enabled();
            if !enabled {
                self.current_goal_status_indicator = None;
                self.current_goal_status = None;
                self.current_goal_plan = None;
                self.turn_lifecycle.goal_status_active_turn_started_at = None;
                self.turn_lifecycle.budget_limited_turn_ids.clear();
                self.update_collaboration_mode_indicator();
            }
        }
        if feature == Feature::Workflows {
            self.sync_workflow_command_enabled();
        }
        if feature == Feature::ScheduledTasks {
            self.sync_scheduled_tasks_command_enabled();
        }
        if feature == Feature::MentionsV2 {
            self.sync_mentions_v2_enabled();
        }
        if feature == Feature::PreventIdleSleep {
            self.turn_lifecycle.set_prevent_idle_sleep(enabled);
        }
        #[cfg(target_os = "windows")]
        if matches!(
            feature,
            Feature::WindowsSandbox | Feature::WindowsSandboxElevated
        ) {
            self.bottom_pane.set_windows_degraded_sandbox_active(
                crate::legacy_core::windows_sandbox::ELEVATED_SANDBOX_NUX_ENABLED
                    && matches!(
                        WindowsSandboxLevel::from_config(&self.config),
                        WindowsSandboxLevel::RestrictedToken
                    ),
            );
        }
        enabled
    }

    pub(crate) fn set_approvals_reviewer(&mut self, policy: ApprovalsReviewer) {
        self.config.approvals_reviewer = policy;
        self.refresh_status_surfaces();
    }

    pub(crate) fn set_full_access_warning_acknowledged(&mut self, acknowledged: bool) {
        self.config.notices.hide_full_access_warning = Some(acknowledged);
    }

    pub(crate) fn set_world_writable_warning_acknowledged(&mut self, acknowledged: bool) {
        self.config.notices.hide_world_writable_warning = Some(acknowledged);
    }

    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub(crate) fn world_writable_warning_hidden(&self) -> bool {
        self.config
            .notices
            .hide_world_writable_warning
            .unwrap_or(false)
    }

    /// Override the reasoning effort used when Plan mode is active.
    ///
    /// When the active mask is already Plan, the override is applied immediately
    /// so the footer reflects it without waiting for the next mode switch.
    /// Passing `None` resets to the Plan-mode preset default.
    pub(crate) fn set_plan_mode_reasoning_effort(&mut self, effort: Option<ReasoningEffortConfig>) {
        self.config.plan_mode_reasoning_effort = effort.clone();
        if self.collaboration_modes_enabled()
            && let Some(mask) = self.active_collaboration_mask.as_mut()
            && mask.mode == Some(ModeKind::Plan)
        {
            if let Some(effort) = effort {
                mask.reasoning_effort = Some(Some(effort));
            } else if let Some(plan_mask) =
                collaboration_modes::plan_mask(self.model_catalog.as_ref())
            {
                mask.reasoning_effort = plan_mask.reasoning_effort;
            }
        }
        self.refresh_model_dependent_surfaces();
    }

    /// Set the reasoning effort for the non-Plan collaboration mode.
    ///
    /// Does not touch the active Plan mask — Plan reasoning is controlled
    /// exclusively by the Plan preset and `set_plan_mode_reasoning_effort`.
    pub(crate) fn set_reasoning_effort(&mut self, effort: Option<ReasoningEffortConfig>) {
        self.current_collaboration_mode = self.current_collaboration_mode.with_updates(
            /*model*/ None,
            Some(effort.clone()),
            /*developer_instructions*/ None,
        );
        if self.collaboration_modes_enabled()
            && let Some(mask) = self.active_collaboration_mask.as_mut()
            && mask.mode != Some(ModeKind::Plan)
        {
            // Generic "global default" updates should not mutate the active Plan mask.
            // Plan reasoning is controlled by the Plan preset and Plan-only override updates.
            mask.reasoning_effort = Some(effort);
        }
        self.refresh_model_dependent_surfaces();
    }

    /// Set the personality in the widget's config copy.
    pub(crate) fn set_personality(&mut self, personality: Personality) {
        self.config.personality = Some(personality);
    }

    pub(crate) fn set_auth_profile(&mut self, auth_profile: Option<String>) {
        let profile_changed = self.config.selected_auth_profile != auth_profile;
        self.config.selected_auth_profile = auth_profile;
        if profile_changed {
            self.sync_auth_profile_status_account_state();
        } else {
            self.refresh_status_line();
        }
    }

    fn sync_auth_profile_status_account_state(&mut self) {
        let auth = match self.config.selected_auth_profile.as_deref() {
            Some(profile) => match load_auth_profile_metadata(&self.config.codex_home, profile) {
                Ok(metadata)
                    if metadata.subscription_provider
                        != AuthProfileSubscriptionProvider::ChatGpt =>
                {
                    None
                }
                Ok(_) => {
                    match load_auth_profile(
                        &self.config.codex_home,
                        self.config.cli_auth_credentials_store_mode,
                        profile,
                    ) {
                        Ok(auth) => Some(auth),
                        Err(err) => {
                            tracing::warn!(
                                profile,
                                error = %err,
                                "failed to load selected auth profile for status account state"
                            );
                            None
                        }
                    }
                }
                Err(err) => {
                    tracing::warn!(
                        profile,
                        error = %err,
                        "failed to load selected auth profile metadata for status account state"
                    );
                    None
                }
            },
            None => {
                match load_auth_dot_json(
                    &self.config.codex_home,
                    self.config.cli_auth_credentials_store_mode,
                ) {
                    Ok(auth) => auth,
                    Err(err) => {
                        tracing::warn!(
                            error = %err,
                            "failed to load default auth state for status account state"
                        );
                        None
                    }
                }
            }
        };
        let (status_account_display, has_chatgpt_account) = match auth.as_ref() {
            Some(auth) => {
                let auth_mode = resolved_auth_status_mode(auth);
                (
                    Some(status_account_display_from_auth(auth_mode, auth)),
                    matches!(auth_mode, AuthMode::Chatgpt | AuthMode::ChatgptAuthTokens),
                )
            }
            None => (None, false),
        };

        self.update_account_state(
            status_account_display,
            /*plan_type*/ None,
            has_chatgpt_account,
        );
        self.rate_limit_snapshots_by_limit_id.clear();
        self.auth_profile_auto_switch_snapshots_by_limit_id.clear();
        self.refreshing_status_outputs.clear();
        self.refreshing_minimax_usage_status_outputs.clear();
        self.codex_rate_limit_reached_type = None;
        self.rate_limit_warnings = RateLimitWarningState::default();
        self.rate_limit_switch_prompt = RateLimitSwitchPromptState::default();
        self.pending_auth_profile_auto_switch_trigger = None;
        self.prefetch_rate_limits();
        self.refresh_status_line();
    }

    pub(crate) fn status_account_display(&self) -> Option<&StatusAccountDisplay> {
        self.status_account_display.as_ref()
    }

    pub(crate) fn runtime_model_provider_base_url(&self) -> Option<&str> {
        self.runtime_model_provider_base_url.as_deref()
    }

    pub(crate) fn set_model_provider(
        &mut self,
        provider_id: String,
        provider: ModelProviderInfo,
        runtime_base_url: Option<String>,
    ) {
        self.config.model_provider_id = provider_id;
        self.config.model_provider = provider;
        self.runtime_model_provider_base_url = runtime_base_url;
        self.refresh_status_line();
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn model_catalog(&self) -> Arc<ModelCatalog> {
        self.model_catalog.clone()
    }

    pub(crate) fn set_model_catalog(&mut self, model_catalog: Arc<ModelCatalog>) {
        self.model_catalog = model_catalog;
        self.sync_image_paste_enabled();
        self.sync_service_tier_commands();
    }

    pub(crate) fn current_plan_type(&self) -> Option<PlanType> {
        self.plan_type
    }

    pub(crate) fn has_chatgpt_account(&self) -> bool {
        self.has_chatgpt_account
    }

    pub(crate) fn update_account_state(
        &mut self,
        status_account_display: Option<StatusAccountDisplay>,
        plan_type: Option<PlanType>,
        has_chatgpt_account: bool,
    ) {
        self.status_account_display = status_account_display;
        self.plan_type = plan_type;
        self.has_chatgpt_account = has_chatgpt_account;
        self.bottom_pane
            .set_connectors_enabled(self.connectors_enabled());
    }

    pub(crate) fn set_realtime_audio_device(
        &mut self,
        kind: RealtimeAudioDeviceKind,
        name: Option<String>,
    ) {
        match kind {
            RealtimeAudioDeviceKind::Microphone => self.config.realtime_audio.microphone = name,
            RealtimeAudioDeviceKind::Speaker => self.config.realtime_audio.speaker = name,
        }
    }

    /// Set the syntax theme override in the widget's config copy.
    pub(crate) fn set_tui_theme(&mut self, theme: Option<String>) {
        self.config.tui_theme = theme;
    }

    /// Set the model in the widget's config copy and stored collaboration mode.
    pub(crate) fn set_model(&mut self, model: &str) {
        self.current_collaboration_mode = self.current_collaboration_mode.with_updates(
            Some(model.to_string()),
            /*effort*/ None,
            /*developer_instructions*/ None,
        );
        if self.collaboration_modes_enabled()
            && let Some(mask) = self.active_collaboration_mask.as_mut()
        {
            mask.model = Some(model.to_string());
        }
        self.refresh_effective_service_tier();
        self.refresh_model_dependent_surfaces();
    }

    pub(crate) fn current_model(&self) -> &str {
        if !self.collaboration_modes_enabled() {
            return self.current_collaboration_mode.model();
        }
        self.active_collaboration_mask
            .as_ref()
            .and_then(|mask| mask.model.as_deref())
            .unwrap_or_else(|| self.current_collaboration_mode.model())
    }

    pub(crate) fn realtime_conversation_is_live(&self) -> bool {
        self.realtime_conversation.is_live()
    }

    pub(super) fn current_realtime_audio_device_name(
        &self,
        kind: RealtimeAudioDeviceKind,
    ) -> Option<String> {
        match kind {
            RealtimeAudioDeviceKind::Microphone => self.config.realtime_audio.microphone.clone(),
            RealtimeAudioDeviceKind::Speaker => self.config.realtime_audio.speaker.clone(),
        }
    }

    pub(super) fn current_realtime_audio_selection_label(
        &self,
        kind: RealtimeAudioDeviceKind,
    ) -> String {
        self.current_realtime_audio_device_name(kind)
            .unwrap_or_else(|| "System default".to_string())
    }

    pub(super) fn sync_personality_command_enabled(&mut self) {
        self.bottom_pane
            .set_personality_command_enabled(self.config.features.enabled(Feature::Personality));
    }

    pub(super) fn sync_plugins_command_enabled(&mut self) {
        self.bottom_pane
            .set_plugins_command_enabled(self.config.features.enabled(Feature::Plugins));
    }

    pub(super) fn sync_goal_command_enabled(&mut self) {
        self.bottom_pane
            .set_goal_command_enabled(self.config.features.enabled(Feature::Goals));
    }

    pub(super) fn sync_workflow_command_enabled(&mut self) {
        self.bottom_pane
            .set_workflow_command_enabled(self.config.features.enabled(Feature::Workflows));
    }

    pub(super) fn sync_scheduled_tasks_command_enabled(&mut self) {
        self.bottom_pane.set_scheduled_tasks_command_enabled(
            self.config.features.enabled(Feature::ScheduledTasks),
        );
    }

    pub(super) fn sync_mentions_v2_enabled(&mut self) {
        self.bottom_pane
            .set_mentions_v2_enabled(self.config.features.enabled(Feature::MentionsV2));
    }

    pub(super) fn current_model_supports_personality(&self) -> bool {
        let model = self.current_model();
        self.model_catalog
            .try_list_models()
            .ok()
            .and_then(|models| {
                models
                    .into_iter()
                    .find(|preset| preset.model == model)
                    .map(|preset| preset.supports_personality)
            })
            .unwrap_or(false)
    }

    /// Return whether the effective model currently advertises image-input support.
    ///
    /// We intentionally default to `true` when model metadata cannot be read so transient catalog
    /// failures do not hard-block user input in the UI.
    pub(super) fn current_model_supports_images(&self) -> bool {
        let model = self.current_model();
        self.model_catalog
            .try_list_models()
            .ok()
            .and_then(|models| {
                models
                    .into_iter()
                    .find(|preset| preset.model == model)
                    .map(|preset| preset.input_modalities.contains(&InputModality::Image))
            })
            .unwrap_or(true)
    }

    pub(super) fn sync_image_paste_enabled(&mut self) {
        let enabled = self.current_model_supports_images();
        self.bottom_pane.set_image_paste_enabled(enabled);
    }

    pub(super) fn image_inputs_not_supported_message(&self) -> String {
        format!(
            "Model {} does not support image inputs. Remove images or switch models.",
            self.current_model()
        )
    }

    #[allow(dead_code)] // Used in tests
    pub(crate) fn current_collaboration_mode(&self) -> &CollaborationMode {
        &self.current_collaboration_mode
    }

    pub(crate) fn current_reasoning_effort(&self) -> Option<ReasoningEffortConfig> {
        self.effective_reasoning_effort()
    }

    pub(crate) fn on_thread_settings_updated(
        &mut self,
        notification: ThreadSettingsUpdatedNotification,
    ) {
        let Ok(thread_id) = ThreadId::from_string(&notification.thread_id) else {
            tracing::warn!(
                thread_id = notification.thread_id,
                "ignoring app-server ThreadSettingsUpdated with invalid thread_id"
            );
            return;
        };
        if self.thread_id != Some(thread_id) {
            return;
        }

        self.apply_thread_settings(notification.thread_settings);
    }

    #[cfg(test)]
    pub(crate) fn active_collaboration_mode_kind(&self) -> ModeKind {
        self.active_mode_kind()
    }

    pub(super) fn is_session_configured(&self) -> bool {
        self.thread_id.is_some()
    }

    pub(super) fn collaboration_modes_enabled(&self) -> bool {
        true
    }

    /// Returns the dismissal scope that applies to the currently visible draft.
    fn plan_mode_nudge_scope(&self) -> PlanModeNudgeScope {
        self.thread_id
            .map_or(PlanModeNudgeScope::NewThread, PlanModeNudgeScope::Thread)
    }

    /// Returns whether the current draft should replace the normal footer with the Plan-mode nudge.
    ///
    /// `ChatWidget` owns this policy because it can combine lexical draft matching with mode
    /// availability, interaction state, and thread-scoped dismissal. `ChatComposer` only renders
    /// the resulting visibility bit. Keeping slash and shell drafts out here avoids advertising a
    /// mode switch while the user is intentionally composing another local command.
    pub(super) fn should_show_plan_mode_nudge(&self) -> bool {
        let text = self.bottom_pane.composer_text();
        let trimmed = text.trim_start();
        self.collaboration_modes_enabled()
            && collaboration_modes::plan_mask(self.model_catalog.as_ref()).is_some()
            && self.active_mode_kind() != ModeKind::Plan
            && self.bottom_pane.composer_input_enabled()
            && !self.bottom_pane.is_task_running()
            && self.bottom_pane.no_modal_or_popup_active()
            && !trimmed.starts_with('/')
            && !trimmed.starts_with('!')
            && contains_plan_keyword(&text)
            && !self
                .dismissed_plan_mode_nudge_scopes
                .contains(&self.plan_mode_nudge_scope())
    }

    /// Synchronizes the footer presentation with the current Plan-mode nudge policy.
    pub(super) fn refresh_plan_mode_nudge(&mut self) {
        self.bottom_pane
            .set_plan_mode_nudge_visible(self.should_show_plan_mode_nudge());
    }

    /// Hides the nudge for the current thread scope until the user changes conversation context.
    pub(super) fn dismiss_plan_mode_nudge(&mut self) {
        self.dismissed_plan_mode_nudge_scopes
            .insert(self.plan_mode_nudge_scope());
        self.refresh_plan_mode_nudge();
    }

    pub(super) fn initial_collaboration_mask(
        _config: &Config,
        model_catalog: &ModelCatalog,
        model_override: Option<&str>,
    ) -> Option<CollaborationModeMask> {
        let mut mask = collaboration_modes::default_mask(model_catalog)?;
        if let Some(model_override) = model_override {
            mask.model = Some(model_override.to_string());
        }
        Some(mask)
    }

    pub(super) fn active_mode_kind(&self) -> ModeKind {
        self.active_collaboration_mask
            .as_ref()
            .and_then(|mask| mask.mode)
            .unwrap_or(ModeKind::Default)
    }

    pub(super) fn effective_reasoning_effort(&self) -> Option<ReasoningEffortConfig> {
        if !self.collaboration_modes_enabled() {
            return self.current_collaboration_mode.reasoning_effort();
        }
        let current_effort = self.current_collaboration_mode.reasoning_effort();
        self.active_collaboration_mask
            .as_ref()
            .and_then(|mask| mask.reasoning_effort.clone())
            .unwrap_or(current_effort)
    }

    pub(crate) fn effective_collaboration_mode(&self) -> CollaborationMode {
        if !self.collaboration_modes_enabled() {
            return self.current_collaboration_mode.clone();
        }
        self.active_collaboration_mask.as_ref().map_or_else(
            || self.current_collaboration_mode.clone(),
            |mask| self.current_collaboration_mode.apply_mask(mask),
        )
    }

    pub(super) fn refresh_model_display(&mut self) {
        let effective = self.effective_collaboration_mode();
        self.session_header.set_model(effective.model());
        // Keep composer paste affordances aligned with the currently effective model.
        self.sync_image_paste_enabled();
        self.sync_service_tier_commands();
        self.refresh_terminal_title();
    }

    /// Refresh every UI surface that depends on the effective model, reasoning
    /// effort, or collaboration mode.
    ///
    /// Call this at the end of any setter that mutates `current_collaboration_mode`,
    /// `active_collaboration_mask`, or per-mode reasoning-effort overrides.
    /// Consolidating both refreshes here prevents the bug where callers update the
    /// header/title (`refresh_model_display`) but forget the footer status line
    /// (`refresh_status_line`).
    pub(super) fn refresh_model_dependent_surfaces(&mut self) {
        self.refresh_model_display();
        self.refresh_status_line();
    }

    fn apply_thread_settings(&mut self, mut settings: ThreadSettings) {
        let cwd_changed = self.config.cwd != settings.cwd;
        self.apply_thread_settings_cwd(settings.cwd.clone());
        self.config.model_provider_id = settings.model_provider.clone();
        if let Some(provider) = self
            .config
            .model_providers
            .get(&settings.model_provider)
            .cloned()
        {
            self.config.model_provider = provider;
        }
        self.set_service_tier(settings.service_tier.clone());
        self.set_approval_policy(settings.approval_policy);
        self.set_approvals_reviewer(settings.approvals_reviewer.to_core());
        self.set_auth_profile(settings.auth_profile.clone());
        self.config.personality = settings.personality;
        self.set_auth_profile(settings.auth_profile.clone());

        let permission_profile = PermissionProfile::from_legacy_sandbox_policy_for_cwd(
            &settings.sandbox_policy.to_core(),
            settings.cwd.as_path(),
        );
        let permission_snapshot = PermissionProfileSnapshot::from_session_snapshot(
            permission_profile,
            settings.active_permission_profile.take().map(Into::into),
        );
        if let Err(err) = self
            .config
            .permissions
            .set_permission_profile_from_session_snapshot(permission_snapshot.clone())
        {
            tracing::warn!(%err, "failed to sync permissions from ThreadSettingsUpdated");
            if let Err(replace_err) = self
                .config
                .permissions
                .replace_permission_profile_from_session_snapshot(permission_snapshot)
            {
                tracing::error!(
                    %replace_err,
                    "failed to replace permissions from ThreadSettingsUpdated after constraint fallback"
                );
            }
        }

        settings.collaboration_mode.settings.model = settings.model;
        settings.collaboration_mode.settings.reasoning_effort = settings.effort;
        self.set_effective_collaboration_mode(settings.collaboration_mode);
        self.refresh_effective_service_tier();
        self.refresh_status_surfaces();
        self.sync_service_tier_commands();
        self.sync_personality_command_enabled();
        if cwd_changed {
            self.refresh_skills_for_current_cwd(/*force_reload*/ true);
        }
        self.refresh_plugin_mentions();
        self.request_redraw();
    }

    fn apply_thread_settings_cwd(&mut self, cwd: AbsolutePathBuf) {
        let previous_cwd = std::mem::replace(&mut self.config.cwd, cwd.clone());
        self.current_cwd = Some(cwd.to_path_buf());
        self.status_line_project_root_name_cache = None;

        if !self.config.workspace_roots.contains(&previous_cwd) {
            return;
        }

        let previous_roots = std::mem::take(&mut self.config.workspace_roots);
        self.config.workspace_roots.push(cwd);
        for root in previous_roots {
            if root != previous_cwd && !self.config.workspace_roots.contains(&root) {
                self.config.workspace_roots.push(root);
            }
        }
        self.config
            .permissions
            .set_workspace_roots(self.config.workspace_roots.clone());
    }

    pub(super) fn set_effective_collaboration_mode(&mut self, mode: CollaborationMode) {
        let mode_kind = mode.mode;
        let settings = mode.settings;
        if mode_kind == ModeKind::Default {
            self.current_collaboration_mode = CollaborationMode {
                mode: ModeKind::Default,
                settings: settings.clone(),
            };
        }
        self.active_collaboration_mask = Some(CollaborationModeMask {
            name: mode_kind.display_name().to_string(),
            mode: Some(mode_kind),
            model: Some(settings.model.clone()),
            reasoning_effort: Some(settings.reasoning_effort.clone()),
            developer_instructions: Some(settings.developer_instructions),
        });
        self.update_collaboration_mode_indicator();
        self.refresh_plan_mode_nudge();
        self.refresh_model_dependent_surfaces();
    }

    pub(super) fn model_display_name(&self) -> &str {
        let model = self.current_model();
        if model.is_empty() {
            DEFAULT_MODEL_DISPLAY_NAME
        } else {
            model
        }
    }

    /// Get the label for the current collaboration mode.
    pub(super) fn collaboration_mode_label(&self) -> Option<&'static str> {
        if !self.collaboration_modes_enabled() {
            return None;
        }
        let active_mode = self.active_mode_kind();
        active_mode
            .is_tui_visible()
            .then_some(active_mode.display_name())
    }

    fn collaboration_mode_indicator(&self) -> Option<CollaborationModeIndicator> {
        if !self.collaboration_modes_enabled() {
            return None;
        }
        match self.active_mode_kind() {
            ModeKind::Plan => Some(CollaborationModeIndicator::Plan),
            ModeKind::Default | ModeKind::PairProgramming | ModeKind::Execute => None,
        }
    }

    pub(super) fn update_collaboration_mode_indicator(&mut self) {
        let indicator = self.collaboration_mode_indicator();
        let goal_indicator = if indicator.is_none() {
            self.goal_status_indicator(Instant::now())
        } else {
            None
        };
        self.current_goal_status_indicator = goal_indicator.clone();
        self.bottom_pane.set_collaboration_mode_indicator(indicator);
        self.bottom_pane.set_goal_status_indicator(goal_indicator);
    }

    pub(super) fn refresh_goal_status_indicator_for_time_tick(&mut self) {
        if self.collaboration_mode_indicator().is_some() {
            return;
        }
        let goal_indicator = self.goal_status_indicator(Instant::now());
        if goal_indicator != self.current_goal_status_indicator {
            self.current_goal_status_indicator = goal_indicator.clone();
            self.bottom_pane.set_goal_status_indicator(goal_indicator);
        }
    }

    fn goal_status_indicator(&self, now: Instant) -> Option<GoalStatusIndicator> {
        if !self.config.features.enabled(Feature::Goals) {
            return None;
        }
        self.current_goal_status
            .as_ref()
            .and_then(|state| {
                state.indicator(now, self.turn_lifecycle.goal_status_active_turn_started_at)
            })
            .map(|indicator| {
                super::goal_status::goal_status_indicator_with_goal_plan(
                    indicator,
                    self.current_goal_plan.as_ref(),
                )
            })
    }

    pub(super) fn on_thread_goal_updated(&mut self, goal: AppThreadGoal, turn_id: Option<String>) {
        if let Some(active_thread_id) = self.thread_id
            && active_thread_id.to_string() != goal.thread_id
        {
            return;
        }
        if !self.config.features.enabled(Feature::Goals) {
            self.current_goal_status_indicator = None;
            self.current_goal_status = None;
            self.update_collaboration_mode_indicator();
            return;
        }
        if goal.status == AppThreadGoalStatus::BudgetLimited
            && let Some(turn_id) = turn_id
        {
            self.turn_lifecycle.mark_budget_limited(turn_id);
        }
        let now = Instant::now();
        self.current_goal_status = Some(GoalStatusState::updated(
            self.current_goal_status.as_ref(),
            goal,
            now,
            self.turn_lifecycle.goal_status_active_turn_started_at,
        ));
        self.update_collaboration_mode_indicator();
    }

    pub(crate) fn on_thread_goal_plan_updated(&mut self, plan: AppThreadGoalPlan) {
        if let Some(active_thread_id) = self.thread_id
            && active_thread_id.to_string() != plan.thread_id
        {
            return;
        }
        if !self.config.features.enabled(Feature::Goals) {
            self.current_goal_plan = None;
            return;
        }
        self.current_goal_plan = Some(plan);
        self.update_collaboration_mode_indicator();
    }

    pub(super) fn current_goal_plan_progress_summary(&self) -> Option<String> {
        let plan = self.current_goal_plan.as_ref()?;
        let mut parts = vec![
            format!(
                "{}/{} goals complete",
                plan.completed_node_count, plan.node_count
            ),
            format!(
                "{} tokens",
                crate::status::format_tokens_compact(plan.total_tokens_used)
            ),
        ];
        if plan.total_time_used_seconds > 0 {
            parts.push(crate::goal_display::format_goal_elapsed_seconds(
                plan.total_time_used_seconds,
            ));
        }
        if let Some(max_tokens) = plan.max_tokens {
            let remaining = plan.remaining_tokens.unwrap_or(0);
            parts.push(format!(
                "{} remaining of {}",
                crate::status::format_tokens_compact(remaining),
                crate::status::format_tokens_compact(max_tokens)
            ));
        }
        if let Some(active_node) = plan
            .nodes
            .iter()
            .find(|node| node.status == codex_app_server_protocol::ThreadGoalPlanNodeStatus::Active)
        {
            parts.push(format!("active {}", active_node.key));
        }
        Some(parts.join(" | "))
    }

    pub(crate) fn set_collaboration_mask_from_user_action(&mut self, mask: CollaborationModeMask) {
        self.set_collaboration_mask(mask);
        self.submit_collaboration_mode_settings_update();
    }

    /// Update the active collaboration mask.
    ///
    /// When collaboration modes are enabled and a preset is selected,
    /// the current mode is attached to submissions as `Op::UserTurn { collaboration_mode: Some(...) }`.
    pub(crate) fn set_collaboration_mask(&mut self, mut mask: CollaborationModeMask) {
        if !self.collaboration_modes_enabled() {
            return;
        }
        let previous_mode = self.active_mode_kind();
        let previous_model = self.current_model().to_string();
        let previous_effort = self.effective_reasoning_effort();
        if mask.mode == Some(ModeKind::Plan)
            && let Some(effort) = self.config.plan_mode_reasoning_effort.clone()
        {
            mask.reasoning_effort = Some(Some(effort));
        }
        if mask.mode == Some(ModeKind::Plan) {
            self.dismissed_plan_mode_nudge_scopes
                .insert(self.plan_mode_nudge_scope());
        }
        self.active_collaboration_mask = Some(mask);
        self.update_collaboration_mode_indicator();
        self.refresh_plan_mode_nudge();
        self.refresh_model_dependent_surfaces();
        let next_mode = self.active_mode_kind();
        let next_model = self.current_model();
        let next_effort = self.effective_reasoning_effort();
        if previous_mode != next_mode
            && (previous_model != next_model || previous_effort != next_effort)
        {
            let mut message = format!("Model changed to {next_model}");
            if !next_model.starts_with("codex-auto-") {
                let reasoning_label = match next_effort.as_ref() {
                    None | Some(ReasoningEffortConfig::None) => "default",
                    Some(effort) => effort.as_str(),
                };
                message.push(' ');
                message.push_str(reasoning_label);
            }
            message.push_str(" for ");
            message.push_str(next_mode.display_name());
            message.push_str(" mode.");
            self.add_info_message(message, /*hint*/ None);
        }
        self.request_redraw();
    }

    fn submit_collaboration_mode_settings_update(&self) {
        let Some(thread_id) = self.thread_id else {
            return;
        };
        self.app_event_tx.send(AppEvent::SubmitThreadOp {
            thread_id,
            op: AppCommand::override_turn_context(
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
                Some(self.effective_collaboration_mode()),
                /*personality*/ None,
            ),
        });
    }
}
