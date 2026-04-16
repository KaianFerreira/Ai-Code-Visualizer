//! AI-Native Code Visualizer — library API.

pub mod models;
pub mod parser;
pub mod scan;
pub mod server;

pub use models::{AnalysisResult, CodeGraph, Edge, FileNode};
pub use parser::CodeParser;
