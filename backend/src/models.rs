//! Core graph and analysis result types for the AI-Native Code Visualizer.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileNode {
    pub path: String,
    pub name: String,
    /// Parent path relative to the analysis root, forward slashes, trailing `/` (e.g. `src/components/`).
    #[serde(default)]
    pub directory: String,
    /// Path relative to the analysis root including the file name (e.g. `src/components/ui/Button.tsx`).
    #[serde(default)]
    pub relative_path: String,
    /// Number of folder segments from the repo root to this file's parent (`0` = file at root).
    #[serde(default = "default_depth")]
    pub depth: u32,
    /// Stable key for the parent folder (no trailing slash); `"."` for files at repo root. Use for clustering.
    #[serde(default)]
    pub folder_group: String,
    pub language: String,
    pub functions: Vec<String>,
    pub classes: Vec<String>,
    pub imports: Vec<String>,
    pub line_count: usize,
}

impl FileNode {
    /// Minimal node for a file that was not parsed (e.g. under a skipped directory) but is
    /// referenced by a relative import.
    pub fn reference_stub(canonical_path: PathBuf, root: &std::path::Path) -> Self {
        let name = canonical_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let directory = relative_directory_for_file_path(&canonical_path, root);
        let relative_path = relative_path_for_file_path(&canonical_path, root);
        let (depth, folder_group) = folder_hierarchy_from_relative_path(&relative_path);
        let ext = canonical_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        let language = match ext {
            "ts" => "typescript",
            "tsx" => "typescriptreact",
            "js" => "javascript",
            "cs" => "csharp",
            _ => "reference",
        };
        Self {
            path: canonical_path.to_string_lossy().into_owned(),
            name,
            directory,
            relative_path,
            depth,
            folder_group,
            language: language.to_string(),
            functions: Vec::new(),
            classes: Vec::new(),
            imports: Vec::new(),
            line_count: 0,
        }
    }
}

/// Parent directory of `file_path` relative to `root`, `/`-separated, with trailing `/`.
pub(crate) fn relative_directory_for_file_path(
    file_path: &std::path::Path,
    root: &std::path::Path,
) -> String {
    let file_path = file_path
        .canonicalize()
        .unwrap_or_else(|_| file_path.to_path_buf());
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let Some(parent) = file_path.parent() else {
        return String::new();
    };
    let Ok(rel) = parent.strip_prefix(&root) else {
        return String::new();
    };
    let s = rel.to_string_lossy().replace('\\', "/");
    let trimmed = s.trim_matches('/');
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("{trimmed}/")
    }
}

/// File path relative to `root`, `/`-separated, including file name.
pub(crate) fn relative_path_for_file_path(
    file_path: &std::path::Path,
    root: &std::path::Path,
) -> String {
    let file_path = file_path
        .canonicalize()
        .unwrap_or_else(|_| file_path.to_path_buf());
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let Ok(rel) = file_path.strip_prefix(&root) else {
        return String::new();
    };
    rel.to_string_lossy().replace('\\', "/")
}

fn default_depth() -> u32 {
    0
}

/// Derives folder depth and a clustering key from a repo-relative file path (forward slashes).
/// - `depth`: count of parent directory segments from root (`src/a` → 1, root file → 0).
/// - `folder_group`: parent path without trailing slash, or `"."` when the file sits at the repo root.
pub(crate) fn folder_hierarchy_from_relative_path(relative_path: &str) -> (u32, String) {
    let normalized = relative_path.trim().replace('\\', "/");
    let path_only = normalized.trim_start_matches('/').trim_end_matches('/');
    if path_only.is_empty() {
        return (0, ".".to_string());
    }
    match path_only.rsplit_once('/') {
        None => (0, ".".to_string()),
        Some((parent, _file)) => {
            let parent = parent.trim().trim_end_matches('/');
            if parent.is_empty() {
                (0, ".".to_string())
            } else {
                let depth = parent.split('/').filter(|s| !s.is_empty()).count() as u32;
                (depth, parent.to_string())
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Edge {
    pub source: String,
    pub target: String,
    pub edge_type: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CodeGraph {
    pub files: Vec<FileNode>,
    pub edges: Vec<Edge>,
}

impl CodeGraph {
    pub fn new() -> Self {
        Self {
            files: Vec::new(),
            edges: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AnalysisResult {
    pub graph: CodeGraph,
    pub total_files: u64,
    /// Every file entry seen during the directory walk (after directory filtering), any extension.
    #[serde(default)]
    pub total_files_walked: u64,
    pub total_lines: u64,
    pub analysis_time_ms: u64,
}

impl AnalysisResult {
    pub fn new(
        graph: CodeGraph,
        total_files: u64,
        total_files_walked: u64,
        total_lines: u64,
        analysis_time_ms: u64,
    ) -> Self {
        Self {
            graph,
            total_files,
            total_files_walked,
            total_lines,
            analysis_time_ms,
        }
    }
}
