use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use backend::{models, parser};

const OUTPUT_FILE: &str = "graph_output.json";

fn main() {
    if let Err(e) = run() {
        eprintln!("Parsing failed: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let root = env::args()
        .nth(1)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));

    let timer = Instant::now();

    let code_parser =
        parser::CodeParser::new().context("initialize code parser and tree-sitter grammars")?;
    let result = code_parser
        .parse_directory(&root)
        .with_context(|| format!("analyze directory {}", root.display()))?;

    let elapsed_ms = timer.elapsed().as_millis() as u64;

    print_summary(&result, elapsed_ms);

    let json =
        serde_json::to_string_pretty(&result).context("serialize analysis result to JSON")?;
    fs::write(OUTPUT_FILE, json)
        .with_context(|| format!("write {}", OUTPUT_FILE))?;

    println!("Wrote {}.", OUTPUT_FILE);

    Ok(())
}

fn print_summary(result: &models::AnalysisResult, elapsed_ms: u64) {
    println!("Analysis summary");
    println!("  Local Source Files: {}", result.total_files);
    println!("  Total Files: {}", result.total_files_walked);
    println!("  Total lines of code: {}", result.total_lines);
    println!("  Time (ms): {}", elapsed_ms);
}
