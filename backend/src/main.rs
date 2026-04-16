use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::time::Instant;

use anyhow::{Context, Result};
use backend::{models, scan};

const OUTPUT_FILE: &str = "graph_output.json";

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "serve") {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        if let Err(e) = rt.block_on(backend::server::run()) {
            eprintln!("Server error: {e:#}");
            std::process::exit(1);
        }
        return;
    }

    if let Err(e) = run_cli(args.first().map(|s| s.as_str())) {
        eprintln!("Parsing failed: {e:#}");
        std::process::exit(1);
    }
}

fn run_cli(arg1: Option<&str>) -> Result<()> {
    let timer = Instant::now();
    let (result, cloned_temp) = scan::analyze_source(arg1).context("analyze source")?;

    let elapsed_ms = timer.elapsed().as_millis() as u64;
    print_summary(&result, elapsed_ms);

    let json =
        serde_json::to_string_pretty(&result).context("serialize analysis result to JSON")?;
    fs::write(OUTPUT_FILE, json).with_context(|| format!("write {}", OUTPUT_FILE))?;

    println!("Wrote {}.", OUTPUT_FILE);

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
