//! Auth profile picker for `ChatWidget`.

use super::*;
use codex_login::AuthProfile;
use codex_login::list_auth_profiles;

impl ChatWidget {
    pub(crate) fn open_profile_popup(&mut self) {
        if !self.is_session_configured() {
            self.add_info_message(
                "Profile selection is disabled until startup completes.".to_string(),
                /*hint*/ None,
            );
            return;
        }

        let profiles = match list_auth_profiles(
            &self.config.codex_home,
            self.config.cli_auth_credentials_store_mode,
        ) {
            Ok(profiles) => profiles,
            Err(err) => {
                self.add_error_message(format!("Failed to load auth profiles: {err}"));
                return;
            }
        };

        let current = self.config.selected_auth_profile.as_deref();
        let mut items = Vec::with_capacity(profiles.len() + 1);
        items.push(self.default_auth_profile_item(current.is_none()));
        items.extend(
            profiles
                .into_iter()
                .map(|profile| self.named_auth_profile_item(profile, current)),
        );

        let mut header = ColumnRenderable::new();
        header.push(Line::from("Select Profile".bold()));
        header.push(Line::from("Switch auth for this session.".dim()));
        self.bottom_pane.show_selection_view(SelectionViewParams {
            footer_hint: Some(standard_popup_hint_line()),
            items,
            header: Box::new(header),
            ..Default::default()
        });
    }

    fn default_auth_profile_item(&self, is_current: bool) -> SelectionItem {
        let actions: Vec<SelectionAction> = vec![Box::new(|tx| {
            tx.send(AppEvent::SwitchAuthProfile {
                profile: None,
                reason: crate::app_event::AuthProfileSwitchReason::Manual,
            });
        })];
        SelectionItem {
            name: "default".to_string(),
            description: Some("Root login".to_string()),
            selected_description: Some("Use the default auth store".to_string()),
            is_current,
            actions,
            dismiss_on_select: true,
            ..Default::default()
        }
    }

    fn named_auth_profile_item(
        &self,
        profile: AuthProfile,
        current: Option<&str>,
    ) -> SelectionItem {
        let profile_name = profile.name.clone();
        let description = auth_profile_description(&profile);
        let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
            tx.send(AppEvent::SwitchAuthProfile {
                profile: Some(profile_name.clone()),
                reason: crate::app_event::AuthProfileSwitchReason::Manual,
            });
        })];
        SelectionItem {
            name: profile.name.clone(),
            description,
            selected_description: profile.email.clone().or(profile.plan.clone()),
            is_current: current == Some(profile.name.as_str()),
            actions,
            dismiss_on_select: true,
            ..Default::default()
        }
    }
}

fn auth_profile_description(profile: &AuthProfile) -> Option<String> {
    let mut parts = vec![profile.auth_mode.to_string()];
    if let Some(plan) = &profile.plan {
        parts.push(plan.clone());
    }
    if let Some(email) = &profile.email {
        parts.push(email.clone());
    }
    Some(parts.join(" / "))
}
