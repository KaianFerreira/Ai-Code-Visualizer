//! Shared repository analysis for CLI and HTTP API.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use git2::build::RepoBuilder;
use git2::FetchOptions;

use crate::models::AnalysisResult;
use crate::parser::CodeParser;

pub fn looks_like_git_remote_url(s: &str) -> bool {
    let t = s.trim();
    t.starts_with("https://")
        || t.starts_with("http://")
        || t.starts_with("git@")
}

pub fn clone_git_repository(url: &str, into: &Path) -> Result<()> {
    let url = url.trim();
    let mut fetch_opts = FetchOptions::new();
    fetch_opts.depth(1);

    RepoBuilder::new()
        .fetch_options(fetch_opts)
        .clone(url, into)
        .with_context(|| format!("git clone `{url}` into {}", into.display()))?;

    Ok(())
}

/// Analyze a local path or clone and analyze a git remote.
/// Returns the analysis result and an optional temp directory that must be dropped after the
/// result is fully serialized (keeps cloned repo alive during parsing).
pub fn analyze_source(source: Option<&str>) -> Result<(AnalysisResult, Option<tempfile::TempDir>)> {
    let arg1 = source.filter(|s| !s.is_empty());

    let cloned_temp: Option<tempfile::TempDir> = match arg1 {
        Some(s) if looks_like_git_remote_url(s) => {
            let dir = tempfile::tempdir().context("create temporary directory for git clone")?;
            clone_git_repository(s, dir.path()).context("clone remote repository")?;
            Some(dir)
        }
        _ => None,
    };

    let root: PathBuf = match (&cloned_temp, arg1) {
        (Some(t), _) => t.path().to_path_buf(),
        (None, None) => PathBuf::from("."),
        (None, Some(s)) => PathBuf::from(s),
    };

    let code_parser =
        CodeParser::new().context("initialize code parser and tree-sitter grammars")?;
    let result = code_parser
        .parse_directory(root.as_path())
        .with_context(|| format!("analyze directory {}", root.display()))?;

    Ok((result, cloned_temp))
}
