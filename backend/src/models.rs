//! Core graph and analysis result types for the AI-Native Code Visualizer.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileNode {
    pub path: String,
    pub name: String,
    pub language: String,
    pub functions: Vec<String>,
    pub classes: Vec<String>,
    pub imports: Vec<String>,
    pub line_count: usize,
}

impl FileNode {
    /// Minimal node for a file that was not parsed (e.g. under a skipped directory) but is
    /// referenced by a relative import.
    pub fn reference_stub(canonical_path: PathBuf) -> Self {
        let name = canonical_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
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
            language: language.to_string(),
            functions: Vec::new(),
            classes: Vec::new(),
            imports: Vec::new(),
            line_count: 0,
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
