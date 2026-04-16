use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use backend::{models, parser};
use git2::build::RepoBuilder;
use git2::FetchOptions;

const OUTPUT_FILE: &str = "graph_output.json";

fn main() {
    if let Err(e) = run() {
        eprintln!("Parsing failed: {e:#}");
        std::process::exit(1);
    }
}

fn looks_like_git_remote_url(s: &str) -> bool {
    let t = s.trim();
    t.starts_with("https://")
        || t.starts_with("http://")
        || t.starts_with("git@")
}

fn clone_git_repository(url: &str, into: &Path) -> Result<()> {
    let url = url.trim();
    let mut fetch_opts = FetchOptions::new();
    fetch_opts.depth(1);

    RepoBuilder::new()
        .fetch_options(fetch_opts)
        .clone(url, into)
        .with_context(|| format!("git clone `{url}` into {}", into.display()))?;

    Ok(())
}

fn run() -> Result<()> {
    let timer = Instant::now();
    let arg1 = env::args().nth(1).filter(|s| !s.is_empty());

    let cloned_temp: Option<tempfile::TempDir> = match arg1.as_deref() {
        Some(s) if looks_like_git_remote_url(s) => {
            let dir = tempfile::tempdir().context("create temporary directory for git clone")?;
            clone_git_repository(s, dir.path()).context("clone remote repository")?;
            Some(dir)
        }
        _ => None,
    };

    let root: PathBuf = match (&cloned_temp, arg1.as_ref()) {
        (Some(t), _) => t.path().to_path_buf(),
        (None, None) => PathBuf::from("."),
        (None, Some(s)) => PathBuf::from(s),
    };

    let code_parser =
        parser::CodeParser::new().context("initialize code parser and tree-sitter grammars")?;
    let result = code_parser
        .parse_directory(root.as_path())
        .with_context(|| format!("analyze directory {}", root.display()))?;

    let elapsed_ms = timer.elapsed().as_millis() as u64;

    print_summary(&result, elapsed_ms);

    let json =
        serde_json::to_string_pretty(&result).context("serialize analysis result to JSON")?;
    fs::write(OUTPUT_FILE, json).with_context(|| format!("write {}", OUTPUT_FILE))?;

    println!("Wrote {}.", OUTPUT_FILE);

    // `TempDir` removes the clone directory when dropped (after JSON is written).
    drop(cloned_temp);

    Ok(())
}

fn print_summary(result: &models::AnalysisResult, elapsed_ms: u64) {
    println!("Analysis summary");
    println!("  Local Source Files: {}", result.total_files);
    println!("  Total Files: {}", result.total_files_walked);
    println!("  Total lines of code: {}", result.total_lines);
    println!("  Time (ms): {}", elapsed_ms);

    let mut by_dir: BTreeMap<String, u64> = BTreeMap::new();
    for f in &result.graph.files {
        let key = if f.directory.is_empty() {
            ".".to_string()
        } else {
            f.directory.clone()
        };
        *by_dir.entry(key).or_insert(0) += 1;
    }
    println!("  Files by directory:");
    for (dir, n) in &by_dir {
        println!("    {dir} {n}");
    }
}
