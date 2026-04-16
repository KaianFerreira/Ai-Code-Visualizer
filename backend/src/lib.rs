//! AI-Native Code Visualizer — library API.

pub mod models;
pub mod parser;

pub use models::{AnalysisResult, CodeGraph, Edge, FileNode};
pub use parser::CodeParser;
