//! Model, collaboration, and reasoning popups for `ChatWidget`.
//!
//! These surfaces are tightly related because changing one often redirects
//! into another, especially while Plan mode is active.

use super::*;
use codex_model_provider_info::HASNA_GATEWAY_NAME;
use codex_model_provider_info::OPENAI_PROVIDER_ID;
use codex_model_provider_info::model_gateway_for_provider;
use codex_model_provider_info::model_gateway_name;

const MAX_PICKER_DESCRIPTION_WORDS: usize = 8;

fn short_picker_description(description: &str) -> String {
    description
        .split_whitespace()
        .take(MAX_PICKER_DESCRIPTION_WORDS)
        .collect::<Vec<_>>()
        .join(" ")
}

fn short_picker_description_optional(description: &str) -> Option<String> {
    let description = short_picker_description(description);
    (!description.is_empty()).then_some(description)
}

impl ChatWidget {
    /// Open a popup to choose a quick auto model. Selecting "Model catalog"
    /// opens the full picker with every available preset.
    pub(crate) fn open_model_popup(&mut self) {
        if !self.is_session_configured() {
            self.add_info_message(
                "Model selection is disabled until startup completes.".to_string(),
                /*hint*/ None,
            );
            return;
        }

        let presets: Vec<ModelPreset> = match self.model_catalog.try_list_models() {
            Ok(models) => models,
            Err(_) => {
                self.add_info_message(
                    "Models are being updated; please try /model again in a moment.".to_string(),
                    /*hint*/ None,
                );
                return;
            }
        };
        self.open_model_popup_with_presets(presets);
    }

    /// Open a popup to choose the default model provider.
    pub(crate) fn open_provider_popup(&mut self) {
        if !self.is_session_configured() {
            self.add_info_message(
                "Provider selection is disabled until startup completes.".to_string(),
                /*hint*/ None,
            );
            return;
        }

        let current_provider_id = self
            .model_catalog
            .provider_id()
            .unwrap_or(self.config.model_provider_id.as_str());
        let mut providers = self
            .config
            .model_providers
            .iter()
            .map(|(id, provider)| {
                let name = provider.name.trim();
                let display_name = if name.is_empty() {
                    id.to_string()
                } else {
                    name.to_string()
                };
                let gateway_id = model_gateway_for_provider(id);
                let gateway_name = model_gateway_name(gateway_id).unwrap_or(HASNA_GATEWAY_NAME);
                (
                    id.clone(),
                    format!("{gateway_name} / {display_name}"),
                    provider.requires_openai_auth,
                )
            })
            .collect::<Vec<_>>();
        providers.sort_by(|left, right| {
            (right.0 == current_provider_id)
                .cmp(&(left.0 == current_provider_id))
                .then_with(|| left.1.cmp(&right.1))
                .then_with(|| left.0.cmp(&right.0))
        });

        let items = providers
            .into_iter()
            .map(|(provider_id, display_name, requires_openai_auth)| {
                let auth_label = if requires_openai_auth {
                    "Uses Codewith or OpenAI login"
                } else {
                    "Uses a provider API key"
                };
                let actions: Vec<SelectionAction> = vec![Box::new({
                    let provider_id = provider_id.clone();
                    move |tx| {
                        tx.send(AppEvent::SelectModelProvider {
                            provider_id: provider_id.clone(),
                        });
                    }
                })];
                SelectionItem {
                    name: display_name,
                    description: Some(short_picker_description(&provider_id)),
                    selected_description: Some(auth_label.to_string()),
                    is_current: provider_id == current_provider_id,
                    actions,
                    dismiss_on_select: true,
                    ..Default::default()
                }
            })
            .collect();

        let mut header = ColumnRenderable::new();
        header.push(Line::from("Select Gateway / Provider".bold()));
        header.push(Line::from("Switch gateway, provider, and model.".dim()));
        self.bottom_pane.show_selection_view(SelectionViewParams {
            footer_hint: Some(standard_popup_hint_line()),
            items,
            header: Box::new(header),
            ..Default::default()
        });
    }

    fn model_menu_header(&self, title: &str, subtitle: &str) -> Box<dyn Renderable> {
        let title = title.to_string();
        let subtitle = subtitle.to_string();
        let mut header = ColumnRenderable::new();
        header.push(Line::from(title.bold()));
        header.push(Line::from(subtitle.dim()));
        if let Some(warning) = self.model_menu_warning_line() {
            header.push(warning);
        }
        Box::new(header)
    }

    fn model_menu_warning_line(&self) -> Option<Line<'static>> {
        let base_url = self.custom_openai_base_url()?;
        let warning = format!(
            "Warning: OpenAI base URL is overridden to {base_url}. Selecting a model may not be supported or work properly."
        );
        Some(Line::from(warning.red()))
    }

    fn custom_openai_base_url(&self) -> Option<String> {
        if !self.config.model_provider.is_openai() {
            return None;
        }

        let base_url = self.config.model_provider.base_url.as_ref()?;
        let trimmed = base_url.trim();
        if trimmed.is_empty() {
            return None;
        }

        let normalized = trimmed.trim_end_matches('/');
        if normalized == DEFAULT_OPENAI_BASE_URL {
            return None;
        }

        Some(trimmed.to_string())
    }

    pub(crate) fn open_model_popup_with_presets(&mut self, presets: Vec<ModelPreset>) {
        let presets: Vec<ModelPreset> = presets
            .into_iter()
            .filter(|preset| preset.show_in_picker)
            .collect();

        let catalog_provider_id = self.model_catalog.provider_id().map(str::to_string);
        let current_provider_id = self.config.model_provider_id.clone();
        let catalog_uses_current_provider = self.model_catalog_uses_current_provider();
        let current_model = self.current_model();
        let current_label = presets
            .iter()
            .find(|preset| catalog_uses_current_provider && preset.model.as_str() == current_model)
            .map(|preset| preset.model.to_string())
            .unwrap_or_else(|| self.model_display_name().to_string());

        let (mut auto_presets, other_presets): (Vec<ModelPreset>, Vec<ModelPreset>) = presets
            .into_iter()
            .partition(|preset| Self::is_auto_model(&preset.model));

        if auto_presets.is_empty() {
            self.open_all_models_popup(other_presets);
            return;
        }

        auto_presets.sort_by_key(|preset| Self::auto_model_order(&preset.model));
        let mut items: Vec<SelectionItem> = auto_presets
            .into_iter()
            .map(|preset| {
                let description = short_picker_description_optional(&preset.description);
                let model = preset.model.clone();
                let should_prompt_plan_mode_scope = self.should_prompt_plan_mode_reasoning_scope(
                    model.as_str(),
                    Some(preset.default_reasoning_effort.clone()),
                );
                let actions = Self::model_selection_actions(
                    model.clone(),
                    Some(preset.default_reasoning_effort.clone()),
                    should_prompt_plan_mode_scope,
                    catalog_provider_id.clone(),
                    current_provider_id.clone(),
                );
                SelectionItem {
                    name: model.clone(),
                    description,
                    is_current: catalog_uses_current_provider && model.as_str() == current_model,
                    is_default: preset.is_default,
                    actions,
                    dismiss_on_select: true,
                    ..Default::default()
                }
            })
            .collect();

        if !other_presets.is_empty() {
            let all_models = other_presets;
            let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
                tx.send(AppEvent::OpenAllModelsPopup {
                    models: all_models.clone(),
                });
            })];

            let is_current = !items.iter().any(|item| item.is_current);
            let description = Some(format!("Browse catalog; current {current_label}"));

            items.push(SelectionItem {
                name: "Model catalog".to_string(),
                description,
                is_current,
                actions,
                dismiss_on_select: true,
                ..Default::default()
            });
        }

        let header = self.model_menu_header("Select Model", "Pick from model catalog.");
        self.bottom_pane.show_selection_view(SelectionViewParams {
            footer_hint: Some(standard_popup_hint_line()),
            items,
            header,
            ..Default::default()
        });
    }

    fn is_auto_model(model: &str) -> bool {
        model.starts_with("codex-auto-")
    }

    fn auto_model_order(model: &str) -> usize {
        match model {
            "codex-auto-fast" => 0,
            "codex-auto-balanced" => 1,
            "codex-auto-thorough" => 2,
            _ => 3,
        }
    }

    pub(crate) fn open_all_models_popup(&mut self, presets: Vec<ModelPreset>) {
        if presets.is_empty() {
            self.add_info_message(
                "No model catalog entries are available right now.".to_string(),
                /*hint*/ None,
            );
            return;
        }

        let mut items: Vec<SelectionItem> = Vec::new();
        for preset in presets.into_iter() {
            let description = short_picker_description_optional(&preset.description);
            let is_current = preset.model.as_str() == self.current_model();
            let direct_effort_selection = preset.supported_reasoning_efforts.len() <= 1;
            let select_model_directly = direct_effort_selection
                || self
                    .model_catalog
                    .provider_id()
                    .is_some_and(|provider_id| provider_id != OPENAI_PROVIDER_ID);
            let should_prompt_plan_mode_scope = self.should_prompt_plan_mode_reasoning_scope(
                preset.model.as_str(),
                Some(preset.default_reasoning_effort.clone()),
            );
            let actions: Vec<SelectionAction> = if select_model_directly {
                Self::model_selection_actions(
                    preset.model.clone(),
                    Some(preset.default_reasoning_effort),
                    should_prompt_plan_mode_scope,
                    self.model_catalog.provider_id().map(str::to_string),
                    self.config.model_provider_id.clone(),
                )
            } else {
                let preset_for_action = preset.clone();
                vec![Box::new(move |tx| {
                    let preset_for_event = preset_for_action.clone();
                    tx.send(AppEvent::OpenReasoningPopup {
                        model: preset_for_event,
                    });
                })]
            };
            items.push(SelectionItem {
                name: preset.model.clone(),
                description,
                is_current,
                is_default: preset.is_default,
                actions,
                dismiss_on_select: select_model_directly,
                dismiss_parent_on_child_accept: !select_model_directly,
                ..Default::default()
            });
        }

        let subtitle = self
            .model_catalog
            .provider_id()
            .map(|provider_id| format!("{} / {provider_id}", self.model_catalog.gateway_name()))
            .unwrap_or_else(|| self.model_catalog.gateway_name().to_string());
        let header = self.model_menu_header("Select Model and Effort", &subtitle);
        self.bottom_pane.show_selection_view(SelectionViewParams {
            footer_hint: Some(self.bottom_pane.standard_popup_hint_line()),
            items,
            header,
            ..Default::default()
        });
    }

    fn model_selection_actions(
        model_for_action: String,
        effort_for_action: Option<ReasoningEffortConfig>,
        should_prompt_plan_mode_scope: bool,
        provider_id: Option<String>,
        current_provider_id: String,
    ) -> Vec<SelectionAction> {
        vec![Box::new(move |tx| {
            let switches_provider = provider_id
                .as_deref()
                .is_some_and(|provider_id| provider_id != current_provider_id);
            if !switches_provider && should_prompt_plan_mode_scope {
                tx.send(AppEvent::OpenPlanReasoningScopePrompt {
                    model: model_for_action.clone(),
                    effort: effort_for_action.clone(),
                });
                return;
            }

            if let Some(provider_id) = provider_id.clone() {
                tx.send(AppEvent::SelectModelProviderModel {
                    provider_id,
                    model: model_for_action.clone(),
                    effort: effort_for_action.clone(),
                });
            } else {
                tx.send(AppEvent::UpdateModel(model_for_action.clone()));
                tx.send(AppEvent::UpdateReasoningEffort(effort_for_action.clone()));
                tx.send(AppEvent::PersistModelSelection {
                    model: model_for_action.clone(),
                    effort: effort_for_action.clone(),
                });
            }
        })]
    }

    fn model_catalog_uses_current_provider(&self) -> bool {
        self.model_catalog
            .provider_id()
            .is_none_or(|provider_id| provider_id == self.config.model_provider_id)
    }

    fn should_prompt_plan_mode_reasoning_scope(
        &self,
        selected_model: &str,
        selected_effort: Option<ReasoningEffortConfig>,
    ) -> bool {
        if !self.collaboration_modes_enabled()
            || self.active_mode_kind() != ModeKind::Plan
            || selected_model != self.current_model()
        {
            return false;
        }

        // Prompt whenever the selection is not a true no-op for both:
        // 1) the active Plan-mode effective reasoning, and
        // 2) the stored global defaults that would be updated by the fallback path.
        selected_effort != self.effective_reasoning_effort()
            || selected_model != self.current_collaboration_mode.model()
            || selected_effort != self.current_collaboration_mode.reasoning_effort()
    }

    pub(crate) fn open_plan_reasoning_scope_prompt(
        &mut self,
        model: String,
        effort: Option<ReasoningEffortConfig>,
    ) {
        let reasoning_phrase = match effort.as_ref() {
            Some(ReasoningEffortConfig::None) => "no reasoning".to_string(),
            Some(selected_effort) => {
                format!(
                    "{} reasoning",
                    Self::reasoning_effort_sentence_label(selected_effort)
                )
            }
            None => "the selected reasoning".to_string(),
        };
        let plan_only_description = format!("Always use {reasoning_phrase} in Plan mode.");
        let plan_reasoning_source = if let Some(plan_override) =
            self.config.plan_mode_reasoning_effort.as_ref()
        {
            format!(
                "user-chosen Plan override ({})",
                Self::reasoning_effort_sentence_label(plan_override)
            )
        } else if let Some(plan_mask) = collaboration_modes::plan_mask(self.model_catalog.as_ref())
        {
            match plan_mask
                .reasoning_effort
                .as_ref()
                .and_then(|effort| effort.as_ref())
            {
                Some(plan_effort) => format!(
                    "built-in Plan default ({})",
                    Self::reasoning_effort_sentence_label(plan_effort)
                ),
                None => "built-in Plan default (no reasoning)".to_string(),
            }
        } else {
            "built-in Plan default".to_string()
        };
        let all_modes_description = format!(
            "Set the global default reasoning level and the Plan mode override. This replaces the current {plan_reasoning_source}."
        );
        let subtitle = format!("Choose where to apply {reasoning_phrase}.");

        let plan_only_actions: Vec<SelectionAction> = vec![Box::new({
            let model = model.clone();
            let effort = effort.clone();
            move |tx| {
                tx.send(AppEvent::UpdateModel(model.clone()));
                tx.send(AppEvent::UpdatePlanModeReasoningEffort(effort.clone()));
                tx.send(AppEvent::PersistPlanModeReasoningEffort(effort.clone()));
            }
        })];
        let all_modes_actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
            tx.send(AppEvent::UpdateModel(model.clone()));
            tx.send(AppEvent::UpdateReasoningEffort(effort.clone()));
            tx.send(AppEvent::UpdatePlanModeReasoningEffort(effort.clone()));
            tx.send(AppEvent::PersistPlanModeReasoningEffort(effort.clone()));
            tx.send(AppEvent::PersistModelSelection {
                model: model.clone(),
                effort: effort.clone(),
            });
        })];

        self.bottom_pane.show_selection_view(SelectionViewParams {
            title: Some(PLAN_MODE_REASONING_SCOPE_TITLE.to_string()),
            subtitle: Some(subtitle),
            footer_hint: Some(standard_popup_hint_line()),
            items: vec![
                SelectionItem {
                    name: PLAN_MODE_REASONING_SCOPE_PLAN_ONLY.to_string(),
                    description: Some(plan_only_description),
                    actions: plan_only_actions,
                    dismiss_on_select: true,
                    ..Default::default()
                },
                SelectionItem {
                    name: PLAN_MODE_REASONING_SCOPE_ALL_MODES.to_string(),
                    description: Some(all_modes_description),
                    actions: all_modes_actions,
                    dismiss_on_select: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        self.notify(Notification::PlanModePrompt {
            title: PLAN_MODE_REASONING_SCOPE_TITLE.to_string(),
        });
    }

    /// Open a popup to choose the reasoning effort (stage 2) for the given model.
    pub(crate) fn open_reasoning_popup(&mut self, preset: ModelPreset) {
        let catalog_provider_id = self.model_catalog.provider_id().map(str::to_string);
        let catalog_uses_current_provider = self.model_catalog_uses_current_provider();
        let default_effort = preset.default_reasoning_effort;
        let supported = preset.supported_reasoning_efforts;
        let in_plan_mode =
            self.collaboration_modes_enabled() && self.active_mode_kind() == ModeKind::Plan;

        let warn_effort = if supported
            .iter()
            .any(|option| option.effort == ReasoningEffortConfig::XHigh)
        {
            Some(ReasoningEffortConfig::XHigh)
        } else if supported
            .iter()
            .any(|option| option.effort == ReasoningEffortConfig::High)
        {
            Some(ReasoningEffortConfig::High)
        } else {
            None
        };
        let warning_text = warn_effort.as_ref().map(|effort| {
            let effort_label = Self::reasoning_effort_label(effort);
            format!("⚠ {effort_label} reasoning effort can quickly consume Plus plan rate limits.")
        });
        let warn_for_model = preset.model.starts_with("gpt-5.1-codex")
            || preset.model.starts_with("gpt-5.1-codex-max")
            || preset.model.starts_with("gpt-5.2");

        let mut choices: Vec<ReasoningEffortConfig> = supported
            .iter()
            .map(|option| option.effort.clone())
            .collect();
        if choices.is_empty() {
            choices.push(default_effort.clone());
        }

        if choices.len() == 1 {
            let selected_effort = choices.first().cloned();
            let selected_model = preset.model;
            if catalog_uses_current_provider
                && self.should_prompt_plan_mode_reasoning_scope(
                    &selected_model,
                    selected_effort.clone(),
                )
            {
                self.app_event_tx
                    .send(AppEvent::OpenPlanReasoningScopePrompt {
                        model: selected_model,
                        effort: selected_effort,
                    });
            } else {
                self.apply_model_and_effort(selected_model, selected_effort, catalog_provider_id);
            }
            return;
        }

        let default_choice = choices
            .contains(&default_effort)
            .then(|| default_effort.clone())
            .or_else(|| choices.first().cloned())
            .or(Some(default_effort));

        let model_slug = preset.model.to_string();
        let is_current_model =
            catalog_uses_current_provider && self.current_model() == preset.model.as_str();
        let highlight_choice = if is_current_model {
            if in_plan_mode {
                self.config
                    .plan_mode_reasoning_effort
                    .clone()
                    .or_else(|| self.effective_reasoning_effort())
            } else {
                self.effective_reasoning_effort()
            }
        } else {
            default_choice.clone()
        };
        let selection_choice = highlight_choice.clone().or_else(|| default_choice.clone());
        let initial_selected_idx = choices
            .iter()
            .position(|choice| Some(choice) == selection_choice.as_ref());
        let mut items: Vec<SelectionItem> = Vec::new();
        for choice in choices.iter() {
            let effort = choice.clone();
            let mut effort_label = Self::reasoning_effort_label(&effort);
            if Some(choice) == default_choice.as_ref() {
                effort_label.push_str(" (default)");
            }

            let description = supported
                .iter()
                .find(|option| option.effort == effort)
                .map(|option| option.description.to_string())
                .filter(|text| !text.is_empty());

            let show_warning = warn_for_model && warn_effort.as_ref() == Some(&effort);
            let selected_description = if show_warning {
                warning_text.as_ref().map(|warning_message| {
                    description.as_ref().map_or_else(
                        || warning_message.clone(),
                        |d| format!("{d}\n{warning_message}"),
                    )
                })
            } else {
                None
            };

            let model_for_action = model_slug.clone();
            let choice_effort = Some(effort);
            let should_prompt_plan_mode_scope = catalog_uses_current_provider
                && self.should_prompt_plan_mode_reasoning_scope(
                    model_slug.as_str(),
                    choice_effort.clone(),
                );
            let provider_id = catalog_provider_id.clone();
            let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
                if should_prompt_plan_mode_scope {
                    tx.send(AppEvent::OpenPlanReasoningScopePrompt {
                        model: model_for_action.clone(),
                        effort: choice_effort.clone(),
                    });
                } else if let Some(provider_id) = provider_id.clone() {
                    tx.send(AppEvent::SelectModelProviderModel {
                        provider_id,
                        model: model_for_action.clone(),
                        effort: choice_effort.clone(),
                    });
                } else {
                    tx.send(AppEvent::UpdateModel(model_for_action.clone()));
                    tx.send(AppEvent::UpdateReasoningEffort(choice_effort.clone()));
                    tx.send(AppEvent::PersistModelSelection {
                        model: model_for_action.clone(),
                        effort: choice_effort.clone(),
                    });
                }
            })];

            items.push(SelectionItem {
                name: effort_label,
                description,
                selected_description,
                is_current: is_current_model && Some(choice) == highlight_choice.as_ref(),
                actions,
                dismiss_on_select: true,
                ..Default::default()
            });
        }

        let mut header = ColumnRenderable::new();
        header.push(Line::from(
            format!("Select Reasoning Level for {model_slug}").bold(),
        ));

        self.bottom_pane.show_selection_view(SelectionViewParams {
            header: Box::new(header),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            initial_selected_idx,
            ..Default::default()
        });
    }

    pub(super) fn reasoning_effort_label(effort: &ReasoningEffortConfig) -> String {
        match effort {
            ReasoningEffortConfig::None => "None".to_string(),
            ReasoningEffortConfig::Minimal => "Minimal".to_string(),
            ReasoningEffortConfig::Low => "Low".to_string(),
            ReasoningEffortConfig::Medium => "Medium".to_string(),
            ReasoningEffortConfig::High => "High".to_string(),
            ReasoningEffortConfig::XHigh => "Extra high".to_string(),
            ReasoningEffortConfig::Custom(value) => value.clone(),
        }
    }

    pub(super) fn reasoning_effort_sentence_label(effort: &ReasoningEffortConfig) -> String {
        match effort {
            ReasoningEffortConfig::Custom(value) => value.clone(),
            effort => Self::reasoning_effort_label(effort).to_lowercase(),
        }
    }

    pub(super) fn apply_model_and_effort_without_persist(
        &self,
        model: String,
        effort: Option<ReasoningEffortConfig>,
    ) {
        self.app_event_tx.send(AppEvent::UpdateModel(model));
        self.app_event_tx
            .send(AppEvent::UpdateReasoningEffort(effort));
    }

    fn apply_model_and_effort(
        &self,
        model: String,
        effort: Option<ReasoningEffortConfig>,
        provider_id: Option<String>,
    ) {
        if let Some(provider_id) = provider_id {
            self.app_event_tx.send(AppEvent::SelectModelProviderModel {
                provider_id,
                model,
                effort,
            });
        } else {
            self.apply_model_and_effort_without_persist(model.clone(), effort.clone());
            self.app_event_tx
                .send(AppEvent::PersistModelSelection { model, effort });
        }
    }
}
