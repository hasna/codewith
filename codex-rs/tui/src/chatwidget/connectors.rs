//! Connector cache, popup, and refresh handling for `ChatWidget`.

use super::*;
use crate::app_event::ConnectorsSnapshot;

#[derive(Debug, Clone, Default)]
pub(super) enum ConnectorsCacheState {
    #[default]
    Uninitialized,
    Loading,
    Ready(ConnectorsSnapshot),
    Failed(String),
}

#[derive(Debug, Default)]
pub(super) struct ConnectorsState {
    pub(super) cache: ConnectorsCacheState,
    pub(super) partial_snapshot: Option<ConnectorsSnapshot>,
    pub(super) prefetch_in_flight: bool,
    pub(super) force_refetch_pending: bool,
}

impl ChatWidget {
    pub(crate) fn refresh_connectors(&mut self, force_refetch: bool) {
        self.queue_connectors_refresh(force_refetch);
    }

    pub(super) fn prefetch_connectors(&mut self) {
        self.queue_connectors_refresh(/*force_refetch*/ false);
    }

    fn queue_connectors_refresh(&mut self, force_refetch: bool) {
        if self.begin_connectors_refresh(force_refetch) {
            self.app_event_tx
                .send(AppEvent::FetchConnectorsList { force_refetch });
        }
    }

    fn begin_connectors_refresh(&mut self, force_refetch: bool) -> bool {
        if !self.connectors_enabled() {
            return false;
        }
        if self.connectors.prefetch_in_flight {
            if force_refetch {
                self.connectors.force_refetch_pending = true;
            }
            return false;
        }

        self.connectors.prefetch_in_flight = true;
        if !matches!(self.connectors.cache, ConnectorsCacheState::Ready(_)) {
            self.connectors.cache = ConnectorsCacheState::Loading;
        }
        true
    }

    pub(super) fn connectors_enabled(&self) -> bool {
        self.config.features.enabled(Feature::Apps) && self.has_chatgpt_account
    }

    pub(super) fn connectors_for_mentions(&self) -> Option<&[AppInfo]> {
        if !self.connectors_enabled() {
            return None;
        }

        if let Some(snapshot) = &self.connectors.partial_snapshot {
            return Some(snapshot.connectors.as_slice());
        }

        match &self.connectors.cache {
            ConnectorsCacheState::Ready(snapshot) => Some(snapshot.connectors.as_slice()),
            _ => None,
        }
    }

    /// Snapshot of connectors used to build the app details drill-down view.
    /// Unlike `connectors_for_mentions`, this does not require the apps feature
    /// to be enabled because the details view is opened from an already-loaded
    /// list.
    pub(super) fn connectors_for_details(&self) -> Option<Vec<AppInfo>> {
        if let Some(snapshot) = &self.connectors.partial_snapshot {
            return Some(snapshot.connectors.clone());
        }
        match &self.connectors.cache {
            ConnectorsCacheState::Ready(snapshot) => Some(snapshot.connectors.clone()),
            _ => None,
        }
    }

    pub(crate) fn add_connectors_output(&mut self) {
        if !self.connectors_enabled() {
            self.add_info_message(
                "Apps are disabled.".to_string(),
                Some("Enable the apps feature to use $ or /apps.".to_string()),
            );
            return;
        }

        let connectors_cache = self.connectors.cache.clone();
        let should_force_refetch = !self.connectors.prefetch_in_flight
            || matches!(connectors_cache, ConnectorsCacheState::Ready(_));
        self.queue_connectors_refresh(should_force_refetch);

        match connectors_cache {
            ConnectorsCacheState::Ready(snapshot) => {
                if snapshot.connectors.is_empty() {
                    self.add_info_message("No apps available.".to_string(), /*hint*/ None);
                } else {
                    self.open_connectors_popup(&snapshot.connectors);
                }
            }
            ConnectorsCacheState::Failed(err) => {
                self.add_to_history(history_cell::new_error_event(err));
            }
            ConnectorsCacheState::Loading | ConnectorsCacheState::Uninitialized => {
                self.open_connectors_loading_popup();
            }
        }
        self.request_redraw();
    }

    fn open_connectors_loading_popup(&mut self) {
        if !self.bottom_pane.replace_selection_view_if_active(
            CONNECTORS_SELECTION_VIEW_ID,
            self.connectors_loading_popup_params(),
        ) {
            self.bottom_pane
                .show_selection_view(self.connectors_loading_popup_params());
        }
    }

    fn open_connectors_popup(&mut self, connectors: &[AppInfo]) {
        self.bottom_pane.show_selection_view(
            self.connectors_popup_params(connectors, /*selected_connector_id*/ None),
        );
    }

    fn connectors_loading_popup_params(&self) -> SelectionViewParams {
        let mut header = ColumnRenderable::new();
        header.push(Line::from("Apps".bold()));
        header.push(Line::from("Loading installed and available apps...".dim()));

        SelectionViewParams {
            view_id: Some(CONNECTORS_SELECTION_VIEW_ID),
            header: Box::new(header),
            items: vec![SelectionItem {
                name: "Loading apps...".to_string(),
                description: Some("This updates when the full list is ready.".to_string()),
                is_disabled: true,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn connectors_popup_params(
        &self,
        connectors: &[AppInfo],
        selected_connector_id: Option<&str>,
    ) -> SelectionViewParams {
        let total = connectors.len();
        let installed = connectors
            .iter()
            .filter(|connector| connector.is_accessible)
            .count();
        let mut header = ColumnRenderable::new();
        header.push(Line::from("Apps".bold()));
        header.push(Line::from(
            "Use $ to insert an installed app into your prompt.".dim(),
        ));
        header.push(Line::from(
            format!("Installed {installed} of {total} available apps.").dim(),
        ));
        let initial_selected_idx = selected_connector_id.and_then(|selected_connector_id| {
            connectors
                .iter()
                .position(|connector| connector.id == selected_connector_id)
        });
        let mut items: Vec<SelectionItem> = Vec::with_capacity(connectors.len());
        for connector in connectors {
            let connector_label = codex_connectors::metadata::connector_display_label(connector);
            let connector_title = connector_label.clone();
            let link_description = Self::connector_description(connector);
            let description = Self::connector_brief_description(connector);
            let status_label = Self::connector_status_label(connector);
            let search_value = format!("{connector_label} {}", connector.id);
            let mut item = SelectionItem {
                name: connector_label,
                description: Some(description),
                search_value: Some(search_value),
                ..Default::default()
            };
            let selected_label = if connector.is_accessible {
                format!(
                    "{status_label}. Press Enter to open details (install, manage, or enable/disable)."
                )
            } else {
                format!("{status_label}. Press Enter to open details and install this app.")
            };
            let missing_label = format!("{status_label}. App link unavailable.");
            // The list row always drills down into the app details view. The
            // install/manage browser link lives one level deeper, inside the
            // details view.
            let app_id = connector.id.clone();
            item.actions = vec![Box::new(move |tx| {
                tx.send(AppEvent::OpenAppDetails {
                    app_id: app_id.clone(),
                });
            })];
            item.dismiss_on_select = false;
            item.selected_description = Some(selected_label);
            // Keep `missing_label` referenced for clarity in non-installed
            // connectors without an install URL; the details view surfaces the
            // same message.
            let _ = &link_description;
            let _ = &connector_title;
            let _ = &missing_label;
            items.push(item);
        }

        SelectionViewParams {
            view_id: Some(CONNECTORS_SELECTION_VIEW_ID),
            header: Box::new(header),
            footer_hint: Some(self.bottom_pane.standard_popup_hint_line()),
            items,
            is_searchable: true,
            search_placeholder: Some("Type to search apps".to_string()),
            col_width_mode: ColumnWidthMode::AutoAllRows,
            initial_selected_idx,
            ..Default::default()
        }
    }

    /// Build a drill-down details view for a single connector. The view is
    /// pushed on top of the apps list, so Esc returns to the list. Each row is
    /// a piece of app metadata; the install/manage row opens the app link view
    /// for browser-based install or management.
    pub(super) fn app_details_popup_params(&self, connector: &AppInfo) -> SelectionViewParams {
        let connector_label = codex_connectors::metadata::connector_display_label(connector);
        let status_label = Self::connector_status_label(connector);
        let mut header = ColumnRenderable::new();
        header.push(Line::from(format!("Apps · {connector_label}").bold()));
        header.push(Line::from(
            "Drill-down view. Press Esc to return to the apps list.".dim(),
        ));
        header.push(Line::from(
            format!("{status_label} · id: {}", connector.id).dim(),
        ));
        let mut items: Vec<SelectionItem> = Vec::new();

        if let Some(description) = Self::connector_description(connector) {
            items.push(SelectionItem {
                name: "Description".to_string(),
                description: Some(description),
                is_disabled: true,
                ..Default::default()
            });
        }

        if let Some(metadata) = &connector.app_metadata {
            if let Some(categories) = &metadata.categories
                && !categories.is_empty()
            {
                items.push(SelectionItem {
                    name: "Categories".to_string(),
                    description: Some(categories.join(", ")),
                    is_disabled: true,
                    ..Default::default()
                });
            }
            if let Some(developer) = &metadata.developer {
                let value = developer.trim();
                if !value.is_empty() {
                    items.push(SelectionItem {
                        name: "Developer".to_string(),
                        description: Some(value.to_string()),
                        is_disabled: true,
                        ..Default::default()
                    });
                }
            }
            if let Some(version) = &metadata.version {
                let value = version.trim();
                if !value.is_empty() {
                    items.push(SelectionItem {
                        name: "Version".to_string(),
                        description: Some(value.to_string()),
                        is_disabled: true,
                        ..Default::default()
                    });
                }
            }
            if let Some(notes) = &metadata.version_notes {
                let value = notes.trim();
                if !value.is_empty() {
                    items.push(SelectionItem {
                        name: "Version notes".to_string(),
                        description: Some(value.to_string()),
                        is_disabled: true,
                        ..Default::default()
                    });
                }
            }
        }

        if !connector.plugin_display_names.is_empty() {
            items.push(SelectionItem {
                name: "Plugins".to_string(),
                description: Some(connector.plugin_display_names.join(", ")),
                is_disabled: true,
                ..Default::default()
            });
        }

        items.push(SelectionItem {
            name: "Status".to_string(),
            description: Some(status_label.to_string()),
            is_disabled: true,
            ..Default::default()
        });

        // Install / manage action row. Opens the existing app link view (a
        // further drill-down level) for browser-based install or management.
        let install_instructions = if connector.is_accessible {
            "Manage this app in your browser."
        } else {
            "Install this app in your browser, then reload Codewith."
        };
        if let Some(install_url) = connector.install_url.clone() {
            let app_id = connector.id.clone();
            let title = connector_label;
            let link_description = Self::connector_description(connector);
            let is_installed = connector.is_accessible;
            let is_enabled = connector.is_enabled;
            let instructions = install_instructions.to_string();
            let mut install_item = SelectionItem {
                name: if connector.is_accessible {
                    "Open manage link".to_string()
                } else {
                    "Open install link".to_string()
                },
                description: Some(install_instructions.to_string()),
                dismiss_on_select: true,
                ..Default::default()
            };
            install_item.actions = vec![Box::new(move |tx| {
                tx.send(AppEvent::OpenAppLink {
                    app_id: app_id.clone(),
                    title: title.clone(),
                    description: link_description.clone(),
                    instructions: instructions.clone(),
                    url: install_url.clone(),
                    is_installed,
                    is_enabled,
                });
            })];
            install_item.selected_description = Some(
                "Press Enter to open the app page to install, manage, or enable/disable this app."
                    .to_string(),
            );
            items.push(install_item);
        } else {
            items.push(SelectionItem {
                name: "Install link".to_string(),
                description: Some("App link unavailable.".to_string()),
                is_disabled: true,
                ..Default::default()
            });
        }

        SelectionViewParams {
            view_id: Some(APP_DETAILS_SELECTION_VIEW_ID),
            header: Box::new(header),
            footer_hint: Some(self.bottom_pane.standard_popup_hint_line()),
            items,
            is_searchable: true,
            search_placeholder: Some("Type to filter details".to_string()),
            col_width_mode: ColumnWidthMode::AutoAllRows,
            ..Default::default()
        }
    }

    fn refresh_connectors_popup_if_open(&mut self, connectors: &[AppInfo]) {
        let selected_connector_id =
            if let (Some(selected_index), ConnectorsCacheState::Ready(snapshot)) = (
                self.bottom_pane
                    .selected_index_for_active_view(CONNECTORS_SELECTION_VIEW_ID),
                &self.connectors.cache,
            ) {
                snapshot
                    .connectors
                    .get(selected_index)
                    .map(|connector| connector.id.as_str())
            } else {
                None
            };
        let _ = self.bottom_pane.replace_selection_view_if_active(
            CONNECTORS_SELECTION_VIEW_ID,
            self.connectors_popup_params(connectors, selected_connector_id),
        );
    }

    fn connector_brief_description(connector: &AppInfo) -> String {
        let status_label = Self::connector_status_label(connector);
        match Self::connector_description(connector) {
            Some(description) => format!("{status_label} · {description}"),
            None => status_label.to_string(),
        }
    }

    fn connector_status_label(connector: &AppInfo) -> &'static str {
        if connector.is_accessible {
            if connector.is_enabled {
                "Installed"
            } else {
                "Installed · Disabled"
            }
        } else {
            "Can be installed"
        }
    }

    fn connector_description(connector: &AppInfo) -> Option<String> {
        connector
            .description
            .as_deref()
            .map(str::trim)
            .filter(|description| !description.is_empty())
            .map(str::to_string)
    }

    pub(crate) fn on_connectors_loaded(
        &mut self,
        result: Result<ConnectorsSnapshot, String>,
        is_final: bool,
    ) {
        let mut trigger_pending_force_refetch = false;
        if is_final {
            self.connectors.prefetch_in_flight = false;
            if self.connectors.force_refetch_pending {
                self.connectors.force_refetch_pending = false;
                trigger_pending_force_refetch = true;
            }
        }

        match result {
            Ok(mut snapshot) => {
                if let ConnectorsCacheState::Ready(existing_snapshot) = &self.connectors.cache {
                    let enabled_by_id: HashMap<&str, bool> = existing_snapshot
                        .connectors
                        .iter()
                        .map(|connector| (connector.id.as_str(), connector.is_enabled))
                        .collect();
                    for connector in &mut snapshot.connectors {
                        if let Some(is_enabled) = enabled_by_id.get(connector.id.as_str()) {
                            connector.is_enabled = *is_enabled;
                        }
                    }
                }
                if is_final {
                    self.connectors.partial_snapshot = None;
                    self.refresh_connectors_popup_if_open(&snapshot.connectors);
                    self.connectors.cache = ConnectorsCacheState::Ready(snapshot.clone());
                } else {
                    self.connectors.partial_snapshot = Some(snapshot.clone());
                }
                self.bottom_pane.set_connectors_snapshot(Some(snapshot));
            }
            Err(err) => {
                let partial_snapshot = self.connectors.partial_snapshot.take();
                if let ConnectorsCacheState::Ready(snapshot) = &self.connectors.cache {
                    warn!("failed to refresh apps list; retaining current apps snapshot: {err}");
                    self.bottom_pane
                        .set_connectors_snapshot(Some(snapshot.clone()));
                } else if let Some(snapshot) = partial_snapshot {
                    warn!(
                        "failed to load full apps list; falling back to installed apps snapshot: {err}"
                    );
                    self.refresh_connectors_popup_if_open(&snapshot.connectors);
                    self.connectors.cache = ConnectorsCacheState::Ready(snapshot.clone());
                    self.bottom_pane.set_connectors_snapshot(Some(snapshot));
                } else {
                    self.connectors.cache = ConnectorsCacheState::Failed(err);
                    self.bottom_pane.set_connectors_snapshot(/*snapshot*/ None);
                }
            }
        }

        if trigger_pending_force_refetch {
            self.queue_connectors_refresh(/*force_refetch*/ true);
        }
    }

    pub(crate) fn update_connector_enabled(&mut self, connector_id: &str, enabled: bool) {
        let ConnectorsCacheState::Ready(mut snapshot) = self.connectors.cache.clone() else {
            return;
        };

        let mut changed = false;
        for connector in &mut snapshot.connectors {
            if connector.id == connector_id {
                changed = connector.is_enabled != enabled;
                connector.is_enabled = enabled;
                break;
            }
        }

        if !changed {
            return;
        }

        self.refresh_connectors_popup_if_open(&snapshot.connectors);
        self.connectors.cache = ConnectorsCacheState::Ready(snapshot.clone());
        self.bottom_pane.set_connectors_snapshot(Some(snapshot));
    }
}
