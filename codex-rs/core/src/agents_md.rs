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

use crate::config::Config;
use codex_app_server_protocol::ConfigLayerSource;
use codex_config::ConfigLayerStackOrdering;
use codex_config::default_project_root_markers;
use codex_config::merge_toml_values;
use codex_config::project_root_markers_from_config;
use codex_exec_server::Environment;
use codex_exec_server::ExecutorFileSystem;
use codex_features::Feature;
use codex_prompts::HIERARCHICAL_AGENTS_MESSAGE;
use codex_utils_absolute_path::AbsolutePathBuf;
use std::collections::HashSet;
use std::io;
use std::path::Component;
use std::path::Path;
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
            let data = match fs.read_file(&path, /*sandbox*/ None).await {
                Ok(data) => data,
                Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
                Err(err) if err.kind() == io::ErrorKind::IsADirectory => continue,
                Err(err) => {
                    startup_warnings.push(format!(
                        "Failed to read global project instructions from `{}`: {err}",
                        path.display()
                    ));
                    continue;
                }
            };
            warn_invalid_utf8(&path, &data, "Global", startup_warnings);
            let contents = String::from_utf8_lossy(&data);
            let trimmed = contents.trim();
            if !trimmed.is_empty() {
                return Some(LoadedAgentsMd::new_user(trimmed.to_string(), path));
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
            Ok(Some(mut docs)) => {
                loaded.entries.append(&mut docs.entries);
                loaded.source_paths.append(&mut docs.source_paths);
            }
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

        let paths = self.agents_md_paths(fs).await?;
        if paths.is_empty() {
            return Ok(None);
        }

        let mut remaining = max_total;
        let mut loaded = LoadedAgentsMd::default();

        for p in paths {
            if remaining == 0 {
                break;
            }

            match fs.get_metadata(&p, /*sandbox*/ None).await {
                Ok(metadata) if !metadata.is_file => continue,
                Ok(_) => {}
                Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
                Err(err) => return Err(err),
            }

            let data = match fs.read_file(&p, /*sandbox*/ None).await {
                Ok(data) => data,
                Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
                Err(err) => return Err(err),
            };
            warn_invalid_utf8(&p, &data, "Project", startup_warnings);

            if data.len() > remaining {
                tracing::warn!(
                    "Project doc `{}` exceeds remaining budget ({} bytes) - truncating.",
                    p.display(),
                    remaining,
                );
            }

            let text = String::from_utf8_lossy(&data);
            let mut expanded = self
                .expand_project_doc(fs, &p, &text, &mut remaining, startup_warnings)
                .await?;
            if !expanded.contents.trim().is_empty() {
                loaded.entries.push(InstructionEntry {
                    contents: expanded.contents,
                    provenance: InstructionProvenance::Project(p),
                });
                loaded.source_paths.append(&mut expanded.source_paths);
            }
        }

        if loaded.is_empty() {
            Ok(None)
        } else {
            Ok(Some(loaded))
        }
    }

    async fn expand_project_doc(
        &self,
        fs: &dyn ExecutorFileSystem,
        instruction_path: &AbsolutePathBuf,
        text: &str,
        remaining: &mut usize,
        startup_warnings: &mut Vec<String>,
    ) -> io::Result<ExpandedProjectDoc> {
        let include_root = include_root_for_instruction_file(instruction_path);
        let mut expanded = ExpandedProjectDoc::default();

        for line in text.split_inclusive('\n') {
            if *remaining == 0 {
                break;
            }

            let (line_without_ending, line_ending) = line
                .strip_suffix('\n')
                .map_or((line, ""), |line| (line, "\n"));
            match parse_include_directive(line_without_ending) {
                IncludeDirective::None => {
                    if append_budgeted(&mut expanded.contents, line, remaining) {
                        expanded.push_source(instruction_path.clone());
                    }
                }
                IncludeDirective::Malformed => {
                    warn_rejected_include(
                        instruction_path,
                        "malformed include directive",
                        startup_warnings,
                    );
                }
                IncludeDirective::Path(include_path) => {
                    let Some(include_root) = include_root.as_ref() else {
                        warn_rejected_include(
                            instruction_path,
                            "include root is unavailable",
                            startup_warnings,
                        );
                        continue;
                    };
                    let Some(included) = self
                        .read_include_file(
                            fs,
                            instruction_path,
                            include_root,
                            include_path,
                            startup_warnings,
                        )
                        .await?
                    else {
                        continue;
                    };
                    let appended_include =
                        append_budgeted(&mut expanded.contents, &included.contents, remaining);
                    if appended_include {
                        expanded.push_source(included.path);
                    }
                    if appended_include
                        && !included.contents.ends_with('\n')
                        && !line_ending.is_empty()
                        && *remaining > 0
                    {
                        append_budgeted(&mut expanded.contents, line_ending, remaining);
                    }
                }
            }
        }

        Ok(expanded)
    }

    async fn read_include_file(
        &self,
        fs: &dyn ExecutorFileSystem,
        instruction_path: &AbsolutePathBuf,
        include_root: &AbsolutePathBuf,
        include_path: &str,
        startup_warnings: &mut Vec<String>,
    ) -> io::Result<Option<IncludedProjectDoc>> {
        let relative_path = match validate_include_path(include_path) {
            Ok(path) => path,
            Err(reason) => {
                warn_rejected_include(instruction_path, reason, startup_warnings);
                return Ok(None);
            }
        };

        let root_metadata = match fs.get_metadata(include_root, /*sandbox*/ None).await {
            Ok(metadata) => metadata,
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) =>
            {
                warn_missing_include(instruction_path, startup_warnings);
                return Ok(None);
            }
            Err(err) => {
                warn_rejected_include(instruction_path, &err.to_string(), startup_warnings);
                return Ok(None);
            }
        };
        if root_metadata.is_symlink || !root_metadata.is_directory {
            warn_rejected_include(
                instruction_path,
                "include root is not a directory",
                startup_warnings,
            );
            return Ok(None);
        }

        let target = include_root.join(relative_path);
        let target_metadata = match fs.get_metadata(&target, /*sandbox*/ None).await {
            Ok(metadata) => metadata,
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) =>
            {
                warn_missing_include(instruction_path, startup_warnings);
                return Ok(None);
            }
            Err(err) => {
                warn_rejected_include(instruction_path, &err.to_string(), startup_warnings);
                return Ok(None);
            }
        };
        if !target_metadata.is_file {
            warn_rejected_include(
                instruction_path,
                "include target is not a regular file",
                startup_warnings,
            );
            return Ok(None);
        }

        let canonical_root = match fs.canonicalize(include_root, /*sandbox*/ None).await {
            Ok(path) => path,
            Err(err) => {
                warn_rejected_include(instruction_path, &err.to_string(), startup_warnings);
                return Ok(None);
            }
        };
        let canonical_target = match fs.canonicalize(&target, /*sandbox*/ None).await {
            Ok(path) => path,
            Err(err) => {
                warn_rejected_include(instruction_path, &err.to_string(), startup_warnings);
                return Ok(None);
            }
        };
        if !canonical_target.starts_with(&canonical_root) {
            warn_rejected_include(
                instruction_path,
                "include target escapes include root",
                startup_warnings,
            );
            return Ok(None);
        }

        let data = match fs.read_file(&target, /*sandbox*/ None).await {
            Ok(data) => data,
            Err(err) => {
                warn_rejected_include(instruction_path, &err.to_string(), startup_warnings);
                return Ok(None);
            }
        };
        warn_invalid_utf8(&target, &data, "Included", startup_warnings);

        Ok(Some(IncludedProjectDoc {
            path: target,
            contents: String::from_utf8_lossy(&data).to_string(),
        }))
    }

    /// Discover the list of project instruction files using the same search rules as
    /// `read_agents_md`, but return the file paths instead of concatenated
    /// contents. The list is ordered from project root to the current working
    /// directory (inclusive). Symlinks are allowed. When `project_doc_max_bytes`
    /// is zero, returns an empty list.
    async fn agents_md_paths(
        &self,
        fs: &dyn ExecutorFileSystem,
    ) -> io::Result<Vec<AbsolutePathBuf>> {
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

        Ok(found)
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

/// Model-visible instructions loaded from AGENTS.md files and internal
/// guidance.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LoadedAgentsMd {
    /// Ordered instructions and their provenance.
    entries: Vec<InstructionEntry>,

    /// Ordered file paths that supplied instruction text.
    source_paths: Vec<AbsolutePathBuf>,
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
                provenance: InstructionProvenance::User(path.clone()),
            }],
            source_paths: vec![path],
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
            source_paths: Vec::new(),
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
        self.source_paths.iter()
    }
}

#[derive(Default)]
struct ExpandedProjectDoc {
    contents: String,
    source_paths: Vec<AbsolutePathBuf>,
}

impl ExpandedProjectDoc {
    fn push_source(&mut self, path: AbsolutePathBuf) {
        if !self.source_paths.contains(&path) {
            self.source_paths.push(path);
        }
    }
}

struct IncludedProjectDoc {
    path: AbsolutePathBuf,
    contents: String,
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

enum IncludeDirective<'a> {
    None,
    Malformed,
    Path(&'a str),
}

fn parse_include_directive(line: &str) -> IncludeDirective<'_> {
    let trimmed = line.trim();
    let Some(inner) = trimmed
        .strip_prefix("{{")
        .and_then(|line| line.strip_suffix("}}"))
        .map(str::trim)
    else {
        return IncludeDirective::None;
    };

    let Some(rest) = inner.strip_prefix("include") else {
        return IncludeDirective::None;
    };
    if rest
        .chars()
        .next()
        .is_some_and(|first| !first.is_whitespace())
    {
        return IncludeDirective::None;
    }

    let rest = rest.trim();
    let Some(rest) = rest.strip_prefix('"') else {
        return IncludeDirective::Malformed;
    };
    let Some(end_quote) = rest.find('"') else {
        return IncludeDirective::Malformed;
    };
    let path = &rest[..end_quote];
    let after_path = &rest[end_quote + 1..];
    if after_path.trim().is_empty() {
        IncludeDirective::Path(path)
    } else {
        IncludeDirective::Malformed
    }
}

fn validate_include_path(path: &str) -> Result<&Path, &'static str> {
    if path.is_empty() {
        return Err("include path is empty");
    }
    if path.starts_with('~') {
        return Err("include path cannot start with `~`");
    }
    if path.contains("://") {
        return Err("include path cannot be URL-like");
    }
    if path.contains('\\') {
        return Err("include path cannot contain Windows separators");
    }
    if path.len() >= 2 && path.as_bytes()[0].is_ascii_alphabetic() && path.as_bytes()[1] == b':' {
        return Err("include path cannot use a Windows prefix");
    }

    let path = Path::new(path);
    if path.is_absolute() {
        return Err("include path cannot be absolute");
    }
    if path.extension().is_none_or(|extension| extension != "md") {
        return Err("include path must point to a `.md` file");
    }
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_) | Component::CurDir
        )
    }) {
        return Err("include path must stay within `.codewith/instructions`");
    }

    Ok(path)
}

fn include_root_for_instruction_file(path: &AbsolutePathBuf) -> Option<AbsolutePathBuf> {
    let parent = path.parent()?;
    let mut cursor = Some(parent.clone());
    while let Some(dir) = cursor {
        if dir
            .file_name()
            .is_some_and(|file_name| file_name == ".codewith")
        {
            return Some(dir.join("instructions"));
        }
        cursor = dir.parent();
    }

    Some(parent.join(".codewith/instructions"))
}

fn append_budgeted(output: &mut String, text: &str, remaining: &mut usize) -> bool {
    if *remaining == 0 || text.is_empty() {
        return false;
    }
    if text.len() <= *remaining {
        output.push_str(text);
        *remaining -= text.len();
        return true;
    }

    let mut end = *remaining;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    if end == 0 {
        *remaining = 0;
        return false;
    }
    output.push_str(&text[..end]);
    *remaining = 0;
    true
}

fn warn_rejected_include(
    instruction_path: &AbsolutePathBuf,
    reason: &str,
    startup_warnings: &mut Vec<String>,
) {
    startup_warnings.push(format!(
        "Skipped CODEWITH.md include in `{}`: {reason}.",
        instruction_path.display()
    ));
}

fn warn_missing_include(instruction_path: &AbsolutePathBuf, startup_warnings: &mut Vec<String>) {
    warn_rejected_include(
        instruction_path,
        "included file was not found",
        startup_warnings,
    );
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
