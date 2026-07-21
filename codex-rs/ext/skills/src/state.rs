use codex_core::config::Config;
use codex_core_skills::HostLoadedSkills;
use std::sync::Arc;
use std::sync::Mutex;

use crate::catalog::SkillCatalog;
use crate::catalog::SkillCatalogEntry;
use crate::sources::SkillProviderRoutes;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SkillsExtensionConfig {
    pub(crate) include_instructions: bool,
    pub(crate) bundled_skills_enabled: bool,
}

impl SkillsExtensionConfig {
    pub(crate) fn from_config(config: &Config) -> Self {
        Self {
            include_instructions: config.include_skill_instructions,
            bundled_skills_enabled: config.bundled_skills_enabled(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct SkillsThreadState {
    config: Mutex<SkillsExtensionConfig>,
    tool_snapshot: Mutex<Option<SkillsToolSnapshot>>,
}

impl SkillsThreadState {
    pub(crate) fn new(config: SkillsExtensionConfig) -> Self {
        Self {
            config: Mutex::new(config),
            tool_snapshot: Mutex::new(None),
        }
    }

    pub(crate) fn config(&self) -> SkillsExtensionConfig {
        self.config
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn set_config(&self, config: SkillsExtensionConfig) {
        *self
            .config
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = config;
    }

    pub(crate) fn set_tool_snapshot(
        &self,
        turn_id: String,
        catalog: SkillCatalog,
        host: Option<Arc<HostLoadedSkills>>,
        routes: SkillProviderRoutes,
    ) {
        *self
            .tool_snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(SkillsToolSnapshot {
            turn_id,
            catalog,
            host,
            routes,
        });
    }

    pub(crate) fn tool_snapshot(&self, turn_id: &str) -> Option<SkillsToolSnapshot> {
        self.tool_snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .filter(|snapshot| snapshot.turn_id == turn_id)
            .cloned()
    }

    pub(crate) fn clear_tool_snapshot(&self, turn_id: &str) {
        let mut snapshot = self
            .tool_snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.turn_id == turn_id)
        {
            *snapshot = None;
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct SkillsToolSnapshot {
    pub(crate) turn_id: String,
    pub(crate) catalog: SkillCatalog,
    pub(crate) host: Option<Arc<HostLoadedSkills>>,
    pub(crate) routes: SkillProviderRoutes,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SkillsTurnState {
    pub(crate) catalog: SkillCatalog,
    pub(crate) selected_entries: Vec<SkillCatalogEntry>,
    pub(crate) warnings: Vec<String>,
    pub(crate) main_prompts_injected: bool,
}
