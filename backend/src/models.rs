//! Core graph and analysis result types for the AI-Native Code Visualizer.

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
    pub total_lines: u64,
    pub analysis_time_ms: u64,
}

impl AnalysisResult {
    pub fn new(
        graph: CodeGraph,
        total_files: u64,
        total_lines: u64,
        analysis_time_ms: u64,
    ) -> Self {
        Self {
            graph,
            total_files,
            total_lines,
            analysis_time_ms,
        }
    }
}
