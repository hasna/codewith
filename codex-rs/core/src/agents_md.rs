//! CODEWITH.md discovery and user instruction assembly.
//!
//! Project-level documentation is primarily stored in `.codewith/CODEWITH.md`,
//! with root-level `CODEWITH.md` and legacy `AGENTS.md` files read as fallbacks.
//! Additional fallback filenames can be configured via `project_doc_fallback_filenames`.
//! We include the concatenation of all files found along the path from the
//! project root to the current working directory as follows:
//!
//! 1.  Determine the project root by walking upwards from the current working
//!     directory until a configured `project_root_markers` entry is found.
//!     When `project_root_markers` is unset, the default marker list is used
//!     (`.git`). If no marker is found, only the current working directory is
//!     considered. An empty marker list disables parent traversal.
//! 2.  Collect every project instruction document found from the project root
//!     down to the current working directory (inclusive) and concatenate their
//!     contents in that order.
//! 3.  We do **not** walk past the project root.
//! 4.  A whole line of the form `@relative/path.md` imports another instruction
//!     file. Relative imports resolve from the file containing the directive.

use crate::config::Config;
use codex_app_server_protocol::ConfigLayerSource;
use codex_config::ConfigLayerStackOrdering;
use codex_config::config_toml::DEFAULT_PROJECT_DOC_MAX_BYTES;
use codex_config::default_project_root_markers;
use codex_config::merge_toml_values;
use codex_config::project_root_markers_from_config;
use codex_exec_server::Environment;
use codex_exec_server::ExecutorFileSystem;
use codex_features::Feature;
use codex_prompts::HIERARCHICAL_AGENTS_MESSAGE;
use codex_utils_absolute_path::AbsolutePathBuf;
use std::collections::HashSet;
use std::future::Future;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::pin::Pin;
use toml::Value as TomlValue;
use tracing::error;

/// Default filename scanned for Codewith project instructions.
pub const DEFAULT_AGENTS_MD_FILENAME: &str = "CODEWITH.md";
/// Preferred local override for Codewith project instructions.
pub const LOCAL_AGENTS_MD_FILENAME: &str = "CODEWITH.override.md";
/// Default repository-relative path scanned for Codewith project instructions.
pub const DEFAULT_PROJECT_AGENTS_MD_PATH: &str = ".codewith/CODEWITH.md";
/// Preferred repository-relative local override path for Codewith project instructions.
pub const LOCAL_PROJECT_AGENTS_MD_PATH: &str = ".codewith/CODEWITH.override.md";
/// Repository-relative directory for additional Codewith project rules.
pub const PROJECT_RULES_DIR_PATH: &str = ".codewith/rules";
/// Legacy Codewith project instructions filename, read as a fallback.
const LEGACY_DEFAULT_AGENTS_MD_FILENAME: &str = "AGENTS.md";
/// Legacy Codewith local override filename, read as a fallback.
const LEGACY_LOCAL_AGENTS_MD_FILENAME: &str = "AGENTS.override.md";
const MAX_PROJECT_RULES_DIRS: usize = 256;
const MAX_PROJECT_RULES_FILES: usize = 512;
const MAX_PROJECT_RULES_DEPTH: usize = 16;

/// Maximum nested `@` import depth accepted inside CODEWITH.md files.
const AGENTS_MD_IMPORT_MAX_DEPTH: usize = 8;
/// Maximum number of imported files/directories expanded for one load.
const AGENTS_MD_IMPORT_MAX_COUNT: usize = 128;
/// Maximum bytes read from one imported file before truncating it.
const AGENTS_MD_IMPORT_MAX_FILE_BYTES: usize = DEFAULT_PROJECT_DOC_MAX_BYTES;

/// When both user and project instruction docs are present, they will be
/// concatenated with the following separator.
const AGENTS_MD_SEPARATOR: &str = "\n\n--- project-doc ---\n\n";

/// Resolves project docs into model-visible user instructions and source
/// paths.
pub struct AgentsMdManager<'a> {
    config: &'a Config,
}

impl<'a> AgentsMdManager<'a> {
    pub fn new(config: &'a Config) -> Self {
        Self { config }
    }

    pub(crate) async fn load_global_instructions(
        fs: &dyn ExecutorFileSystem,
        codex_dir: Option<&AbsolutePathBuf>,
        startup_warnings: &mut Vec<String>,
    ) -> Option<LoadedAgentsMd> {
        let base = codex_dir?;
        for candidate in [
            LOCAL_AGENTS_MD_FILENAME,
            DEFAULT_AGENTS_MD_FILENAME,
            LEGACY_LOCAL_AGENTS_MD_FILENAME,
            LEGACY_DEFAULT_AGENTS_MD_FILENAME,
        ] {
            let path = base.join(candidate);
            match fs.get_metadata(&path, /*sandbox*/ None).await {
                Ok(metadata) if metadata.is_file => {}
                Ok(_) => continue,
                Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
                Err(err) => {
                    startup_warnings.push(format!(
                        "Failed to read global project instructions from `{}`: {err}",
                        path.display()
                    ));
                    continue;
                }
            };
            let root_path = path.clone();

            let root = InstructionRoot {
                path,
                import_root: base.clone(),
                source: InstructionSourceKind::User,
            };
            let loaded = match read_instruction_roots(
                fs,
                vec![root],
                None,
                TrimInstructionEntries::Yes,
                startup_warnings,
                "Global",
            )
            .await
            {
                Ok(loaded) => loaded,
                Err(err) => {
                    startup_warnings.push(format!(
                        "Failed to read global project instructions from `{}`: {err}",
                        root_path.display()
                    ));
                    continue;
                }
            };
            if !loaded.is_empty() {
                return Some(loaded);
            }
        }
        None
    }

    /// Combines configured user instructions and project-doc content into a
    /// single model-visible instruction string.
    pub(crate) async fn user_instructions(
        &self,
        environment: &Environment,
        startup_warnings: &mut Vec<String>,
    ) -> Option<LoadedAgentsMd> {
        let fs = environment.get_filesystem();
        self.user_instructions_with_fs(fs.as_ref(), startup_warnings)
            .await
    }

    async fn user_instructions_with_fs(
        &self,
        fs: &dyn ExecutorFileSystem,
        startup_warnings: &mut Vec<String>,
    ) -> Option<LoadedAgentsMd> {
        let agents_md_docs = self.read_agents_md(fs, startup_warnings).await;

        let mut loaded = self.config.user_instructions.clone().unwrap_or_default();

        match agents_md_docs {
            Ok(Some(docs)) => loaded.entries.extend(docs.entries),
            Ok(None) => {}
            Err(e) => {
                error!("error trying to find project instruction docs: {e:#}");
            }
        };

        if self.config.features.enabled(Feature::ChildAgentsMd) {
            loaded.entries.push(InstructionEntry {
                contents: HIERARCHICAL_AGENTS_MESSAGE.to_string(),
                provenance: InstructionProvenance::Internal,
            });
        }

        (!loaded.is_empty()).then_some(loaded)
    }

    /// Attempt to locate and load project instruction documentation.
    ///
    /// On success returns `Ok(Some(loaded))` where `loaded` contains every
    /// discovered doc. If no documentation file is found the function returns
    /// `Ok(None)`. Unexpected I/O failures bubble up as `Err` so callers can
    /// decide how to handle them.
    async fn read_agents_md(
        &self,
        fs: &dyn ExecutorFileSystem,
        startup_warnings: &mut Vec<String>,
    ) -> io::Result<Option<LoadedAgentsMd>> {
        let max_total = self.config.project_doc_max_bytes;

        if max_total == 0 {
            return Ok(None);
        }

        let docs = self.discover_agents_md(fs).await?;
        if docs.is_empty() {
            return Ok(None);
        }

        let loaded = read_instruction_roots(
            fs,
            docs,
            Some(max_total as u64),
            TrimInstructionEntries::No,
            startup_warnings,
            "Project",
        )
        .await?;

        if loaded.is_empty() {
            Ok(None)
        } else {
            Ok(Some(loaded))
        }
    }

    /// Discover the list of project instruction files using the same search rules as
    /// `read_agents_md`, but return the file paths instead of concatenated
    /// contents. The list is ordered from project root to the current working
    /// directory (inclusive). Symlinks are allowed. When `project_doc_max_bytes`
    /// is zero, returns an empty list.
    #[cfg(test)]
    async fn agents_md_paths(
        &self,
        fs: &dyn ExecutorFileSystem,
    ) -> io::Result<Vec<AbsolutePathBuf>> {
        Ok(self
            .discover_agents_md(fs)
            .await?
            .into_iter()
            .map(|doc| doc.path)
            .collect())
    }

    async fn discover_agents_md(
        &self,
        fs: &dyn ExecutorFileSystem,
    ) -> io::Result<Vec<InstructionRoot>> {
        if self.config.project_doc_max_bytes == 0 {
            return Ok(Vec::new());
        }

        let dir = self.config.cwd.clone();

        let mut merged = TomlValue::Table(toml::map::Map::new());
        for layer in self.config.config_layer_stack.get_layers(
            ConfigLayerStackOrdering::LowestPrecedenceFirst,
            /*include_disabled*/ false,
        ) {
            if matches!(layer.name, ConfigLayerSource::Project { .. }) {
                continue;
            }
            merge_toml_values(&mut merged, &layer.config);
        }
        let project_root_markers = match project_root_markers_from_config(&merged) {
            Ok(Some(markers)) => markers,
            Ok(None) => default_project_root_markers(),
            Err(err) => {
                tracing::warn!("invalid project_root_markers: {err}");
                default_project_root_markers()
            }
        };
        let mut project_root = None;
        if !project_root_markers.is_empty() {
            for ancestor in dir.ancestors() {
                for marker in &project_root_markers {
                    let marker_path = ancestor.join(marker);
                    let marker_exists = match fs.get_metadata(&marker_path, /*sandbox*/ None).await
                    {
                        Ok(_) => true,
                        Err(err) if err.kind() == io::ErrorKind::NotFound => false,
                        Err(err) => return Err(err),
                    };
                    if marker_exists {
                        project_root = Some(ancestor.clone());
                        break;
                    }
                }
                if project_root.is_some() {
                    break;
                }
            }
        }

        let import_root = project_root.clone().unwrap_or_else(|| dir.clone());
        let search_dirs: Vec<AbsolutePathBuf> = if let Some(root) = project_root {
            let mut dirs = Vec::new();
            let mut cursor = dir.clone();
            loop {
                dirs.push(cursor.clone());
                if cursor == root {
                    break;
                }
                let Some(parent) = cursor.parent() else {
                    break;
                };
                cursor = parent;
            }
            dirs.reverse();
            dirs
        } else {
            vec![dir]
        };

        let mut found: Vec<AbsolutePathBuf> = Vec::new();
        let candidate_filenames = self.candidate_filenames();
        for d in search_dirs {
            for name in &candidate_filenames {
                let candidate = d.join(name);
                match fs.get_metadata(&candidate, /*sandbox*/ None).await {
                    Ok(md) if md.is_file => {
                        found.push(candidate);
                        break;
                    }
                    Ok(_) => {}
                    Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
                    Err(err) => return Err(err),
                }
            }
            found.extend(self.rules_md_paths(fs, &d).await?);
        }

        Ok(found
            .into_iter()
            .map(|path| InstructionRoot {
                path,
                import_root: import_root.clone(),
                source: InstructionSourceKind::Project,
            })
            .collect())
    }

    async fn rules_md_paths(
        &self,
        fs: &dyn ExecutorFileSystem,
        dir: &AbsolutePathBuf,
    ) -> io::Result<Vec<AbsolutePathBuf>> {
        let rules_dir = dir.join(PROJECT_RULES_DIR_PATH);
        let mut visited_dirs = HashSet::new();
        let mut files = Vec::new();
        let mut dirs = vec![(rules_dir, 0_usize)];
        while let Some((current_dir, depth)) = dirs.pop() {
            if depth > MAX_PROJECT_RULES_DEPTH || visited_dirs.len() >= MAX_PROJECT_RULES_DIRS {
                continue;
            }
            let metadata = match fs.get_metadata(&current_dir, /*sandbox*/ None).await {
                Ok(metadata) => metadata,
                Err(err)
                    if matches!(
                        err.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                    ) =>
                {
                    continue;
                }
                Err(err) => return Err(err),
            };
            if metadata.is_symlink || !metadata.is_directory {
                continue;
            }
            let canonical_dir = fs.canonicalize(&current_dir, /*sandbox*/ None).await?;
            if !visited_dirs.insert(canonical_dir) {
                continue;
            }

            let entries = match fs.read_directory(&current_dir, /*sandbox*/ None).await {
                Ok(entries) => entries,
                Err(err)
                    if matches!(
                        err.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                    ) =>
                {
                    continue;
                }
                Err(err) => return Err(err),
            };

            for entry in entries {
                let path = current_dir.join(entry.file_name);
                if entry.is_directory {
                    dirs.push((path, depth + 1));
                    continue;
                }
                if entry.is_file
                    && path
                        .as_path()
                        .extension()
                        .is_some_and(|extension| extension == "md")
                {
                    let metadata = match fs.get_metadata(&path, /*sandbox*/ None).await {
                        Ok(metadata) => metadata,
                        Err(err)
                            if matches!(
                                err.kind(),
                                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                            ) =>
                        {
                            continue;
                        }
                        Err(err) => return Err(err),
                    };
                    if metadata.is_symlink || !metadata.is_file {
                        continue;
                    }
                    files.push(path);
                    if files.len() >= MAX_PROJECT_RULES_FILES {
                        break;
                    }
                }
            }
            if files.len() >= MAX_PROJECT_RULES_FILES {
                break;
            }
        }
        files.sort();
        Ok(files)
    }

    fn candidate_filenames(&self) -> Vec<&str> {
        let mut names: Vec<&str> =
            Vec::with_capacity(6 + self.config.project_doc_fallback_filenames.len());
        names.push(LOCAL_PROJECT_AGENTS_MD_PATH);
        names.push(DEFAULT_PROJECT_AGENTS_MD_PATH);
        names.push(LOCAL_AGENTS_MD_FILENAME);
        names.push(DEFAULT_AGENTS_MD_FILENAME);
        names.push(LEGACY_LOCAL_AGENTS_MD_FILENAME);
        names.push(LEGACY_DEFAULT_AGENTS_MD_FILENAME);
        for candidate in &self.config.project_doc_fallback_filenames {
            let candidate = candidate.as_str();
            if candidate.is_empty() {
                continue;
            }
            if !names.contains(&candidate) {
                names.push(candidate);
            }
        }
        names
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct InstructionRoot {
    path: AbsolutePathBuf,
    import_root: AbsolutePathBuf,
    source: InstructionSourceKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InstructionSourceKind {
    User,
    Project,
}

impl InstructionSourceKind {
    fn provenance(self, path: AbsolutePathBuf) -> InstructionProvenance {
        match self {
            Self::User => InstructionProvenance::User(path),
            Self::Project => InstructionProvenance::Project(path),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TrimInstructionEntries {
    Yes,
    No,
}

impl TrimInstructionEntries {
    fn should_trim(self) -> bool {
        matches!(self, Self::Yes)
    }
}

struct InstructionLoadContext<'a> {
    fs: &'a dyn ExecutorFileSystem,
    startup_warnings: &'a mut Vec<String>,
    remaining_total_bytes: Option<u64>,
    trim_entries: TrimInstructionEntries,
    active_paths: HashSet<AbsolutePathBuf>,
    import_count: usize,
    source_label: &'static str,
}

async fn read_instruction_roots(
    fs: &dyn ExecutorFileSystem,
    roots: Vec<InstructionRoot>,
    max_total_bytes: Option<u64>,
    trim_entries: TrimInstructionEntries,
    startup_warnings: &mut Vec<String>,
    source_label: &'static str,
) -> io::Result<LoadedAgentsMd> {
    let mut ctx = InstructionLoadContext {
        fs,
        startup_warnings,
        remaining_total_bytes: max_total_bytes,
        trim_entries,
        active_paths: HashSet::new(),
        import_count: 0,
        source_label,
    };
    let mut loaded = LoadedAgentsMd::default();
    for root in roots {
        let entries = load_instruction_path(
            &mut ctx,
            root.path,
            root.import_root,
            root.source,
            /*depth*/ 0,
            /*is_import*/ false,
            /*referenced_from*/ None,
        )
        .await?;
        loaded.entries.extend(entries);
    }
    Ok(loaded)
}

fn load_instruction_path<'ctx, 'env>(
    ctx: &'ctx mut InstructionLoadContext<'env>,
    path: AbsolutePathBuf,
    import_root: AbsolutePathBuf,
    source: InstructionSourceKind,
    depth: usize,
    is_import: bool,
    referenced_from: Option<AbsolutePathBuf>,
) -> Pin<Box<dyn Future<Output = io::Result<Vec<InstructionEntry>>> + Send + 'ctx>> {
    Box::pin(async move {
        if is_import {
            if depth > AGENTS_MD_IMPORT_MAX_DEPTH {
                warn_skipped_import(
                    ctx,
                    &path,
                    referenced_from.as_ref(),
                    format!("maximum import depth of {AGENTS_MD_IMPORT_MAX_DEPTH} was exceeded"),
                );
                return Ok(Vec::new());
            }
            if ctx.import_count >= AGENTS_MD_IMPORT_MAX_COUNT {
                warn_skipped_import(
                    ctx,
                    &path,
                    referenced_from.as_ref(),
                    format!("maximum import count of {AGENTS_MD_IMPORT_MAX_COUNT} was exceeded"),
                );
                return Ok(Vec::new());
            }
            ctx.import_count += 1;
        }

        if !path_is_within_root(&path, &import_root) {
            warn_skipped_import(
                ctx,
                &path,
                referenced_from.as_ref(),
                format!(
                    "resolved path is outside import root `{}`",
                    import_root.display()
                ),
            );
            return Ok(Vec::new());
        }

        if !ctx.active_paths.insert(path.clone()) {
            if is_import {
                warn_skipped_import(
                    ctx,
                    &path,
                    referenced_from.as_ref(),
                    "it was already loaded; this prevents cycles".to_string(),
                );
            }
            return Ok(Vec::new());
        }
        let active_path = path.clone();

        let result = load_instruction_path_contents(
            ctx,
            path,
            import_root,
            source,
            depth,
            is_import,
            referenced_from,
        )
        .await;
        ctx.active_paths.remove(&active_path);
        result
    })
}

fn load_instruction_path_contents<'ctx, 'env>(
    ctx: &'ctx mut InstructionLoadContext<'env>,
    path: AbsolutePathBuf,
    import_root: AbsolutePathBuf,
    source: InstructionSourceKind,
    depth: usize,
    is_import: bool,
    referenced_from: Option<AbsolutePathBuf>,
) -> Pin<Box<dyn Future<Output = io::Result<Vec<InstructionEntry>>> + Send + 'ctx>> {
    Box::pin(async move {
        let metadata = match ctx.fs.get_metadata(&path, /*sandbox*/ None).await {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                if is_import {
                    warn_skipped_import(
                        ctx,
                        &path,
                        referenced_from.as_ref(),
                        "file does not exist".to_string(),
                    );
                }
                return Ok(Vec::new());
            }
            Err(err) => return Err(err),
        };

        if is_import && metadata.is_symlink {
            warn_skipped_import(
                ctx,
                &path,
                referenced_from.as_ref(),
                "symlink imports are not followed".to_string(),
            );
            return Ok(Vec::new());
        }

        if is_import
            && !import_resolves_within_root(ctx, &path, &import_root, referenced_from.as_ref())
                .await?
        {
            return Ok(Vec::new());
        }

        if metadata.is_directory {
            if is_import {
                return load_instruction_directory(ctx, path, import_root, source, depth).await;
            }
            return Ok(Vec::new());
        }
        if !metadata.is_file {
            return Ok(Vec::new());
        }
        if is_import && !is_supported_import_file(&path) {
            warn_skipped_import(
                ctx,
                &path,
                referenced_from.as_ref(),
                "only .md, .mdc, and .txt instruction files can be imported".to_string(),
            );
            return Ok(Vec::new());
        }

        let mut data = match ctx.fs.read_file(&path, /*sandbox*/ None).await {
            Ok(data) => data,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(err),
        };
        warn_invalid_utf8(&path, &data, ctx.source_label, ctx.startup_warnings);

        if is_import && data.len() > AGENTS_MD_IMPORT_MAX_FILE_BYTES {
            ctx.startup_warnings.push(format!(
                "{} project instructions import `{}` exceeds {AGENTS_MD_IMPORT_MAX_FILE_BYTES} bytes; truncating.",
                ctx.source_label,
                path.display(),
            ));
            data.truncate(AGENTS_MD_IMPORT_MAX_FILE_BYTES);
        }

        if let Some(remaining) = ctx.remaining_total_bytes.as_mut() {
            if *remaining == 0 {
                return Ok(Vec::new());
            }
            let size = data.len() as u64;
            if size > *remaining {
                tracing::warn!(
                    "{} project instructions `{}` exceed remaining budget ({} bytes) - truncating.",
                    ctx.source_label,
                    path.display(),
                    remaining,
                );
                data.truncate(*remaining as usize);
            }
            *remaining = remaining.saturating_sub(data.len() as u64);
        }

        let text = String::from_utf8_lossy(&data).to_string();
        expand_instruction_text(ctx, text, path, import_root, source, depth).await
    })
}

fn load_instruction_directory<'ctx, 'env>(
    ctx: &'ctx mut InstructionLoadContext<'env>,
    path: AbsolutePathBuf,
    import_root: AbsolutePathBuf,
    source: InstructionSourceKind,
    depth: usize,
) -> Pin<Box<dyn Future<Output = io::Result<Vec<InstructionEntry>>> + Send + 'ctx>> {
    Box::pin(async move {
        let mut children = ctx
            .fs
            .read_directory(&path, /*sandbox*/ None)
            .await?
            .into_iter()
            .filter(|entry| entry.is_file && is_supported_rule_file(&entry.file_name))
            .collect::<Vec<_>>();
        children.sort_by(|a, b| a.file_name.cmp(&b.file_name));

        let mut entries = Vec::new();
        for child in children {
            let child_path = ctx.fs.join(&path, Path::new(&child.file_name)).await?;
            let child_entries = load_instruction_path(
                ctx,
                child_path,
                import_root.clone(),
                source,
                depth,
                /*is_import*/ true,
                Some(path.clone()),
            )
            .await?;
            entries.extend(child_entries);
        }
        Ok(entries)
    })
}

fn expand_instruction_text<'ctx, 'env>(
    ctx: &'ctx mut InstructionLoadContext<'env>,
    text: String,
    source_path: AbsolutePathBuf,
    import_root: AbsolutePathBuf,
    source: InstructionSourceKind,
    depth: usize,
) -> Pin<Box<dyn Future<Output = io::Result<Vec<InstructionEntry>>> + Send + 'ctx>> {
    Box::pin(async move {
        let mut entries = Vec::new();
        let mut segment = String::new();
        for line in text.split_inclusive('\n') {
            let line_without_newline = line.trim_end_matches('\n').trim_end_matches('\r');
            let Some(import_ref) = parse_import_directive(line_without_newline) else {
                segment.push_str(line);
                continue;
            };

            push_instruction_segment(
                &mut entries,
                &mut segment,
                &source_path,
                source,
                ctx.trim_entries,
            );

            let Some(parent) = source_path.parent() else {
                warn_skipped_import(
                    ctx,
                    &source_path,
                    Some(&source_path),
                    "importing file has no parent directory".to_string(),
                );
                continue;
            };
            if Path::new(import_ref).is_absolute() {
                warn_skipped_import(
                    ctx,
                    &source_path,
                    Some(&source_path),
                    "absolute import paths are not supported".to_string(),
                );
                continue;
            }
            let imported_path = ctx.fs.join(&parent, Path::new(import_ref)).await?;
            let imported_entries = load_instruction_path(
                ctx,
                imported_path,
                import_root.clone(),
                source,
                depth + 1,
                /*is_import*/ true,
                Some(source_path.clone()),
            )
            .await?;
            entries.extend(imported_entries);
        }

        push_instruction_segment(
            &mut entries,
            &mut segment,
            &source_path,
            source,
            ctx.trim_entries,
        );
        Ok(entries)
    })
}

fn push_instruction_segment(
    entries: &mut Vec<InstructionEntry>,
    segment: &mut String,
    path: &AbsolutePathBuf,
    source: InstructionSourceKind,
    trim_entries: TrimInstructionEntries,
) {
    let raw = std::mem::take(segment);
    let contents = if trim_entries.should_trim() {
        raw.trim().to_string()
    } else {
        raw
    };
    if contents.trim().is_empty() {
        return;
    }
    entries.push(InstructionEntry {
        contents,
        provenance: source.provenance(path.clone()),
    });
}

fn parse_import_directive(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    let import_ref = trimmed.strip_prefix('@')?.trim();
    if import_ref.is_empty()
        || import_ref.chars().any(char::is_whitespace)
        || import_ref.starts_with("http://")
        || import_ref.starts_with("https://")
    {
        return None;
    }
    Some(import_ref)
}

fn path_is_within_root(path: &AbsolutePathBuf, root: &AbsolutePathBuf) -> bool {
    path.as_path().starts_with(root.as_path())
}

async fn import_resolves_within_root(
    ctx: &mut InstructionLoadContext<'_>,
    path: &AbsolutePathBuf,
    import_root: &AbsolutePathBuf,
    referenced_from: Option<&AbsolutePathBuf>,
) -> io::Result<bool> {
    let canonical_path = match ctx.fs.canonicalize(path, /*sandbox*/ None).await {
        Ok(path) => path,
        Err(err) => {
            warn_skipped_import(
                ctx,
                path,
                referenced_from,
                format!("failed to resolve import target: {err}"),
            );
            return Ok(false);
        }
    };
    let canonical_root = match ctx.fs.canonicalize(import_root, /*sandbox*/ None).await {
        Ok(root) => root,
        Err(err) => {
            warn_skipped_import(
                ctx,
                path,
                referenced_from,
                format!(
                    "failed to resolve import root `{}`: {err}",
                    import_root.display()
                ),
            );
            return Ok(false);
        }
    };

    if path_is_within_root(&canonical_path, &canonical_root) {
        return Ok(true);
    }

    warn_skipped_import(
        ctx,
        path,
        referenced_from,
        format!(
            "resolved path `{}` is outside resolved import root `{}`",
            canonical_path.display(),
            canonical_root.display()
        ),
    );
    Ok(false)
}

fn is_supported_rule_file(file_name: &str) -> bool {
    Path::new(file_name)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "md" | "mdc" | "txt"
            )
        })
}

fn is_supported_import_file(path: &AbsolutePathBuf) -> bool {
    path.as_path()
        .file_name()
        .and_then(|file_name| file_name.to_str())
        .is_some_and(is_supported_rule_file)
}

fn warn_skipped_import(
    ctx: &mut InstructionLoadContext<'_>,
    path: &AbsolutePathBuf,
    referenced_from: Option<&AbsolutePathBuf>,
    reason: String,
) {
    let from = referenced_from.map_or_else(
        || "unknown source".to_string(),
        |source| format!("`{}`", source.display()),
    );
    ctx.startup_warnings.push(format!(
        "Skipping CODEWITH.md import `{}` referenced from {from}: {reason}.",
        path.display(),
    ));
}

/// Model-visible instructions loaded from AGENTS.md files and internal
/// guidance.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LoadedAgentsMd {
    /// Ordered instructions and their provenance.
    entries: Vec<InstructionEntry>,
}

impl LoadedAgentsMd {
    /// Creates loaded instructions containing one user-level AGENTS.md entry.
    pub fn new_user(contents: String, path: AbsolutePathBuf) -> Self {
        if contents.trim().is_empty() {
            return Self::default();
        }
        Self {
            entries: vec![InstructionEntry {
                contents,
                provenance: InstructionProvenance::User(path),
            }],
        }
    }

    /// Creates source-less user instructions for tests.
    ///
    /// This cannot be gated with `#[cfg(test)]` because integration tests
    /// compile `codex-core` as a normal dependency without that configuration.
    pub fn from_text_for_testing(contents: impl Into<String>) -> Self {
        let contents = contents.into();
        if contents.trim().is_empty() {
            return Self::default();
        }
        Self {
            entries: vec![InstructionEntry {
                contents,
                provenance: InstructionProvenance::Internal,
            }],
        }
    }

    fn is_empty(&self) -> bool {
        self.entries
            .iter()
            .all(|entry| entry.contents.trim().is_empty())
    }

    /// Returns the concatenated model-visible instruction text.
    pub fn text(&self) -> String {
        let mut output = String::new();
        let mut previous_provenance: Option<&InstructionProvenance> = None;
        for entry in &self.entries {
            if let Some(previous_provenance) = previous_provenance {
                // The project-doc marker tells the model where workspace-scoped
                // instructions begin, so it is only needed on the transition
                // from user or internal instructions to project instructions.
                let separator = match (previous_provenance, &entry.provenance) {
                    (
                        InstructionProvenance::User(_) | InstructionProvenance::Internal,
                        InstructionProvenance::Project(_),
                    ) => AGENTS_MD_SEPARATOR,
                    _ => "\n\n",
                };
                output.push_str(separator);
            }
            output.push_str(&entry.contents);
            previous_provenance = Some(&entry.provenance);
        }
        output
    }

    /// Returns the AGENTS.md files that supplied instruction entries.
    pub fn sources(&self) -> impl Iterator<Item = &AbsolutePathBuf> {
        let mut seen = HashSet::<PathBuf>::new();
        self.entries.iter().filter_map(move |entry| {
            let path = entry.provenance.path()?;
            if seen.insert(path.to_path_buf()) {
                Some(path)
            } else {
                None
            }
        })
    }
}

/// One model-visible instruction and its provenance.
#[derive(Clone, Debug, PartialEq, Eq)]
struct InstructionEntry {
    /// Model-visible instruction text.
    contents: String,

    /// Origin of the instruction.
    provenance: InstructionProvenance,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum InstructionProvenance {
    /// User-level instructions, normally loaded from CODEX_HOME.
    User(AbsolutePathBuf),

    /// Workspace instructions discovered from project AGENTS.md files.
    Project(AbsolutePathBuf),

    /// Instructions without a file source, including internally defined guidance.
    Internal,
}

impl InstructionProvenance {
    fn path(&self) -> Option<&AbsolutePathBuf> {
        match self {
            Self::User(path) | Self::Project(path) => Some(path),
            Self::Internal => None,
        }
    }
}

fn warn_invalid_utf8(
    path: &AbsolutePathBuf,
    data: &[u8],
    source: &str,
    startup_warnings: &mut Vec<String>,
) {
    if let Err(err) = std::str::from_utf8(data) {
        startup_warnings.push(format!(
            "{source} project instructions from `{}` contain invalid UTF-8: {err}. Invalid byte sequences were replaced.",
            path.display()
        ));
    }
}

#[cfg(test)]
#[path = "agents_md_tests.rs"]
mod tests;
