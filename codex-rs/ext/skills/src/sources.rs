use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;

use crate::catalog::SkillAuthority;
use crate::catalog::SkillCatalog;
use crate::catalog::SkillPackageId;
use crate::catalog::SkillProviderError;
use crate::catalog::SkillReadResult;
use crate::catalog::SkillSearchResult;
use crate::catalog::SkillSourceKind;
use crate::provider::SkillListQuery;
use crate::provider::SkillProvider;
use crate::provider::SkillReadRequest;
use crate::provider::SkillSearchRequest;

#[derive(Clone)]
pub struct SkillProviderSource {
    kind: SkillSourceKind,
    label: String,
    provider: Arc<dyn SkillProvider>,
}

impl SkillProviderSource {
    pub fn new(
        kind: SkillSourceKind,
        label: impl Into<String>,
        provider: Arc<dyn SkillProvider>,
    ) -> Self {
        Self {
            kind,
            label: label.into(),
            provider,
        }
    }

    pub fn host(label: impl Into<String>, provider: Arc<dyn SkillProvider>) -> Self {
        Self::new(SkillSourceKind::Host, label, provider)
    }

    pub fn executor(label: impl Into<String>, provider: Arc<dyn SkillProvider>) -> Self {
        Self::new(SkillSourceKind::Executor, label, provider)
    }

    pub fn remote(label: impl Into<String>, provider: Arc<dyn SkillProvider>) -> Self {
        Self::new(SkillSourceKind::Remote, label, provider)
    }

    fn should_list(&self, query: &SkillListQuery) -> bool {
        match &self.kind {
            SkillSourceKind::Host => query.include_host_skills,
            SkillSourceKind::Executor => !query.executor_authorities.is_empty(),
            SkillSourceKind::Remote => query.include_remote_skills,
            SkillSourceKind::Custom(_) => true,
        }
    }

    fn owns_kind(&self, kind: &SkillSourceKind) -> bool {
        &self.kind == kind
    }
}

impl fmt::Debug for SkillProviderSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SkillProviderSource")
            .field("kind", &self.kind)
            .field("label", &self.label)
            .finish()
    }
}

#[derive(Clone, Default, Debug)]
pub struct SkillProviders {
    sources: Vec<SkillProviderSource>,
}

#[derive(Clone, Default)]
pub(crate) struct SkillProviderRoutes {
    routes: HashMap<(SkillAuthority, SkillPackageId), Arc<dyn SkillProvider>>,
}

impl fmt::Debug for SkillProviderRoutes {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SkillProviderRoutes")
            .field("route_count", &self.routes.len())
            .finish()
    }
}

impl SkillProviderRoutes {
    fn push(
        &mut self,
        authority: SkillAuthority,
        package: SkillPackageId,
        provider: Arc<dyn SkillProvider>,
    ) {
        self.routes.entry((authority, package)).or_insert(provider);
    }

    fn provider(
        &self,
        authority: &SkillAuthority,
        package: &SkillPackageId,
    ) -> Option<Arc<dyn SkillProvider>> {
        self.routes
            .get(&(authority.clone(), package.clone()))
            .map(Arc::clone)
    }

    pub(crate) async fn read(
        &self,
        request: SkillReadRequest,
    ) -> Result<SkillReadResult, SkillProviderError> {
        let Some(provider) = self.provider(&request.authority, &request.package) else {
            return Err(SkillProviderError::new(
                "skill package is not available from the requested authority",
            ));
        };
        provider.read(request).await
    }

    pub(crate) async fn search(
        &self,
        request: SkillSearchRequest,
    ) -> Result<SkillSearchResult, SkillProviderError> {
        let Some(provider) = self.provider(&request.authority, &request.package) else {
            return Err(SkillProviderError::new(
                "skill package is not available from the requested authority",
            ));
        };
        provider.search(request).await
    }
}

impl SkillProviders {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_provider(mut self, source: SkillProviderSource) -> Self {
        self.sources.push(source);
        self
    }

    pub fn with_host_provider(mut self, provider: Arc<dyn SkillProvider>) -> Self {
        self.sources
            .push(SkillProviderSource::host("host", provider));
        self
    }

    pub fn with_executor_provider(mut self, provider: Arc<dyn SkillProvider>) -> Self {
        self.sources
            .push(SkillProviderSource::executor("executor", provider));
        self
    }

    pub fn with_remote_provider(mut self, provider: Arc<dyn SkillProvider>) -> Self {
        self.sources
            .push(SkillProviderSource::remote("remote", provider));
        self
    }

    pub(crate) async fn list_for_turn_with_routes(
        &self,
        query: SkillListQuery,
    ) -> (SkillCatalog, SkillProviderRoutes) {
        let mut catalog = SkillCatalog::default();
        let mut routes = SkillProviderRoutes::default();
        let mut seen = HashSet::new();

        for source in self
            .sources
            .iter()
            .filter(|source| source.should_list(&query))
        {
            match source.provider.list(query.clone()).await {
                Ok(source_catalog) => {
                    for entry in source_catalog.entries {
                        if seen.insert((entry.authority.clone(), entry.id.clone())) {
                            routes.push(
                                entry.authority.clone(),
                                entry.id.clone(),
                                Arc::clone(&source.provider),
                            );
                            catalog.entries.push(entry);
                        }
                    }
                    catalog.warnings.extend(source_catalog.warnings);
                }
                Err(err) => catalog.warnings.push(format!(
                    "{} skills unavailable: {}",
                    source.label, err.message
                )),
            }
        }

        (catalog, routes)
    }

    pub async fn search(
        &self,
        request: SkillSearchRequest,
    ) -> Result<SkillSearchResult, SkillProviderError> {
        let mut last_error = None;
        for source in self
            .sources
            .iter()
            .filter(|source| source.owns_kind(&request.authority.kind))
        {
            match source.provider.search(request.clone()).await {
                Ok(result) => return Ok(result),
                Err(err) => last_error = Some(err),
            }
        }

        match last_error {
            Some(err) => Err(err),
            None => Err(SkillProviderError::new(format!(
                "{} skill provider is not configured",
                request.authority.kind
            ))),
        }
    }
}
