//! Parallel tree-sitter parsing and graph construction.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use rayon::prelude::*;
use tree_sitter::Language;
use walkdir::WalkDir;

use crate::models::{AnalysisResult, CodeGraph, Edge, FileNode};

const SOURCE_EXTENSIONS: &[&str] = &["ts", "tsx", "js", "cs"];

/// Directory names we never descend into (much faster than collecting files and filtering later).
const SKIP_DIR_NAMES: &[&str] = &["node_modules", ".git", "dist", "target", "bin", "obj"];

/// Loaded tree-sitter grammars. Cheap to clone (`Language` is a shared pointer).
#[derive(Clone)]
pub struct GrammarSet {
    typescript: Language,
    tsx: Language,
    /// JavaScript is parsed with the TypeScript grammar (same crate; accurate for most `.js`).
    javascript: Language,
    csharp: Language,
}

impl GrammarSet {
    /// Initialize all grammars. Call once per analysis run (or at startup).
    pub fn new() -> Result<Self> {
        let typescript = tree_sitter_typescript::LANGUAGE_TYPESCRIPT
            .into();
        let tsx = tree_sitter_typescript::LANGUAGE_TSX.into();
        let javascript = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
        let csharp = tree_sitter_c_sharp::LANGUAGE.into();

        let mut p = tree_sitter::Parser::new();
        p.set_language(&typescript)
            .context("load tree-sitter TypeScript grammar")?;
        p.set_language(&tsx)
            .context("load tree-sitter TSX grammar")?;
        p.set_language(&javascript)
            .context("load tree-sitter JavaScript (TS) grammar")?;
        p.set_language(&csharp)
            .context("load tree-sitter C# grammar")?;

        Ok(Self {
            typescript,
            tsx,
            javascript,
            csharp,
        })
    }

    fn language_for_extension(&self, ext: &str) -> Option<&Language> {
        match ext {
            "ts" => Some(&self.typescript),
            "tsx" => Some(&self.tsx),
            "js" => Some(&self.javascript),
            "cs" => Some(&self.csharp),
            _ => None,
        }
    }
}

/// High-performance multi-language parser driven by walkdir + rayon + tree-sitter.
pub struct CodeParser {
    grammars: GrammarSet,
}

impl CodeParser {
    pub fn new() -> Result<Self> {
        Ok(Self {
            grammars: GrammarSet::new()?,
        })
    }

    /// Scan `root`, parse every matching source file in parallel, then resolve import edges.
    pub fn parse_directory(&self, root: &Path) -> Result<AnalysisResult> {
        let started = Instant::now();
        let root = root
            .canonicalize()
            .with_context(|| format!("canonicalize root {}", root.display()))?;

        let (paths, total_files_walked) = collect_source_paths(&root)?;
        let grammars = self.grammars.clone();

        let results: Vec<Result<FileNode>> = paths
            .par_iter()
            .map(|path| parse_file_to_node(path, &grammars))
            .collect();

        let mut files = Vec::with_capacity(results.len());
        for r in results {
            files.push(r?);
        }

        let local_source_files = files.len() as u64;
        let (edges, reference_nodes) = build_import_edges(&files, &root);
        files.extend(reference_nodes);

        let total_lines = files
            .iter()
            .map(|f| f.line_count as u64)
            .sum::<u64>();
        let analysis_time_ms = started.elapsed().as_millis() as u64;

        Ok(AnalysisResult::new(
            CodeGraph { files, edges },
            local_source_files,
            total_files_walked,
            total_lines,
            analysis_time_ms,
        ))
    }
}

fn collect_source_paths(root: &Path) -> Result<(Vec<PathBuf>, u64)> {
    let mut out = Vec::new();
    let mut total_files_walked: u64 = 0;
    let walker = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            if e.depth() == 0 {
                return true;
            }
            let name = e.file_name().to_string_lossy();
            !SKIP_DIR_NAMES.iter().any(|&skip| skip == name.as_ref())
        });
    for entry in walker {
        let entry = entry.with_context(|| "walk directory")?;
        if !entry.file_type().is_file() {
            continue;
        }
        total_files_walked += 1;
        let path = entry.path();
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        if SOURCE_EXTENSIONS.contains(&ext) {
            out.push(path.to_path_buf());
        }
    }
    Ok((out, total_files_walked))
}

fn parse_file_to_node(path: &Path, grammars: &GrammarSet) -> Result<FileNode> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();
    let language = grammars
        .language_for_extension(ext)
        .with_context(|| format!("no grammar for extension `{ext}`"))?;

    let source = fs::read_to_string(path)
        .with_context(|| format!("read {}", path.display()))?;

    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(language)
        .context("set tree-sitter language on parser")?;

    let tree = parser
        .parse(&source, None)
        .context("tree-sitter parse returned no tree")?;

    let root_node = tree.root_node();
    let line_count = source.lines().count();

    let (classes, functions, imports) = match ext {
        "ts" | "tsx" | "js" => extract_ts_like(&root_node, &source),
        "cs" => extract_csharp(&root_node, &source),
        _ => (Vec::new(), Vec::new(), Vec::new()),
    };

    let language_label = match ext {
        "ts" => "typescript",
        "tsx" => "typescriptreact",
        "js" => "javascript",
        "cs" => "csharp",
        _ => "unknown",
    };

    let path_buf = path
        .canonicalize()
        .unwrap_or_else(|_| path.to_path_buf());

    let name = path_buf
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();

    Ok(FileNode {
        path: path_buf.to_string_lossy().into_owned(),
        name,
        language: language_label.to_string(),
        functions,
        classes,
        imports,
        line_count,
    })
}

// ——— TypeScript / TSX / JS ———

fn extract_ts_like(root: &tree_sitter::Node<'_>, source: &str) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut classes = Vec::new();
    let mut functions = Vec::new();
    let mut imports = Vec::new();

    let mut stack: Vec<tree_sitter::Node<'_>> = vec![*root];
    while let Some(node) = stack.pop() {
        match node.kind() {
            "class_declaration" => {
                if let Some(n) = node.child_by_field_name("name") {
                    if let Some(s) = node_text(&n, source) {
                        classes.push(s);
                    }
                }
            }
            "interface_declaration" | "enum_declaration" => {
                if let Some(n) = node.child_by_field_name("name") {
                    if let Some(s) = node_text(&n, source) {
                        classes.push(s);
                    }
                }
            }
            "function_declaration" | "generator_function" => {
                if let Some(n) = node.child_by_field_name("name") {
                    if let Some(s) = node_text(&n, source) {
                        functions.push(s);
                    }
                }
            }
            "method_definition" => {
                if let Some(n) = node.child_by_field_name("name") {
                    if let Some(s) = node_text(&n, source) {
                        functions.push(s);
                    }
                }
            }
            "arrow_function" => {
                if let Some(parent) = node.parent() {
                    if parent.kind() == "variable_declarator" {
                        if let Some(n) = parent.child_by_field_name("name") {
                            if let Some(s) = node_text(&n, source) {
                                functions.push(s);
                            }
                        }
                    }
                }
            }
            "import_statement" => {
                if let Some(src) = node.child_by_field_name("source") {
                    if let Some(s) = string_literal_contents(&src, source) {
                        imports.push(s);
                    }
                }
            }
            "call_expression" => {
                if let Some(f) = node.child_by_field_name("function") {
                    if f.kind() == "identifier" && node_text(&f, source).as_deref() == Some("require") {
                        if let Some(args) = node.child_by_field_name("arguments") {
                            for i in 0..args.child_count() {
                                if let Some(c) = args.child(i as u32) {
                                    if let Some(s) = string_literal_contents(&c, source) {
                                        imports.push(s);
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }

        let mut cursor = node.walk();
        if cursor.goto_first_child() {
            loop {
                stack.push(cursor.node());
                if !cursor.goto_next_sibling() {
                    break;
                }
            }
        }
    }

    (classes, functions, imports)
}

// ——— C# ———

fn extract_csharp(root: &tree_sitter::Node<'_>, source: &str) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut classes = Vec::new();
    let mut functions = Vec::new();
    let mut imports = Vec::new();

    let mut stack: Vec<tree_sitter::Node<'_>> = vec![*root];
    while let Some(node) = stack.pop() {
        match node.kind() {
            "class_declaration" | "interface_declaration" | "struct_declaration" | "record_declaration"
            | "enum_declaration" => {
                if let Some(n) = node.child_by_field_name("name") {
                    if let Some(s) = node_text(&n, source) {
                        classes.push(s);
                    }
                }
            }
            "method_declaration" | "constructor_declaration" | "destructor_declaration" => {
                if let Some(n) = node.child_by_field_name("name") {
                    if let Some(s) = node_text(&n, source) {
                        functions.push(s);
                    }
                }
            }
            "using_directive" => {
                if let Some(s) = csharp_using_clause(&node, source) {
                    imports.push(s);
                }
            }
            _ => {}
        }

        let mut cursor = node.walk();
        if cursor.goto_first_child() {
            loop {
                stack.push(cursor.node());
                if !cursor.goto_next_sibling() {
                    break;
                }
            }
        }
    }

    (classes, functions, imports)
}

fn csharp_using_clause(node: &tree_sitter::Node<'_>, source: &str) -> Option<String> {
    let text = node.utf8_text(source.as_bytes()).ok()?.trim();
    let rest = text.strip_prefix("using")?.trim();
    let rest = rest
        .strip_prefix("static")
        .map(str::trim)
        .unwrap_or(rest);
    let rest = rest.trim_end_matches(';').trim();
    if rest.is_empty() {
        return None;
    }
    // `Alias = Namespace` → use RHS for resolution attempts
    if let Some((_, rhs)) = rest.split_once('=') {
        return Some(rhs.trim().to_string());
    }
    Some(rest.to_string())
}

// ——— Edges ———

struct FileIndex<'a> {
    by_path: HashMap<String, &'a FileNode>,
}

impl<'a> FileIndex<'a> {
    fn new(files: &'a [FileNode]) -> Self {
        let mut by_path = HashMap::with_capacity(files.len());
        for f in files {
            by_path.insert(normalize_path_key(Path::new(&f.path)), f);
        }
        Self { by_path }
    }

    fn node_for_normalized_path(&self, key: &str) -> Option<&'a FileNode> {
        self.by_path.get(key).copied()
    }

    fn resolve(&self, from_path: &str, spec: &str) -> Option<&'a FileNode> {
        if spec.starts_with('.') {
            let base = Path::new(from_path).parent()?;
            let joined = normalize_path_key(base.join(spec).as_path());
            for candidate in candidate_paths(&joined) {
                if let Some(n) = self.by_path.get(&candidate) {
                    return Some(n);
                }
            }
        }
        None
    }
}

fn normalize_path_key(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn candidate_paths(base: &str) -> Vec<String> {
    let mut v = vec![base.to_string()];
    for ext in SOURCE_EXTENSIONS {
        v.push(format!("{base}.{ext}"));
    }
    for ext in SOURCE_EXTENSIONS {
        v.push(format!("{base}/index.{ext}"));
    }
    v
}

fn import_path_candidates(joined: PathBuf) -> Vec<PathBuf> {
    let mut v = vec![joined.clone()];
    for ext in SOURCE_EXTENSIONS {
        v.push(joined.with_extension(ext));
    }
    for ext in SOURCE_EXTENSIONS {
        v.push(joined.join(format!("index.{ext}")));
    }
    v
}

/// Resolve a relative import to an on-disk file under `root` (e.g. into a skipped folder).
fn resolve_relative_on_disk(from_path: &str, spec: &str, root: &Path) -> Option<PathBuf> {
    let base = Path::new(from_path).parent()?;
    let joined = base.join(spec);
    for cand in import_path_candidates(joined) {
        if cand.is_file() {
            let ok = cand.canonicalize().ok()?;
            if ok.starts_with(root) {
                return Some(ok);
            }
        }
    }
    None
}

fn build_import_edges(files: &[FileNode], root: &Path) -> (Vec<Edge>, Vec<FileNode>) {
    let root = root
        .canonicalize()
        .unwrap_or_else(|_| root.to_path_buf());
    let index = FileIndex::new(files);
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut edges = Vec::new();
    let mut stub_map: HashMap<String, FileNode> = HashMap::new();

    for file in files {
        for spec in &file.imports {
            let spec_trim = spec.trim();
            if spec_trim.is_empty() {
                continue;
            }

            let target_path: Option<String> =
                if let Some(target) = index.resolve(&file.path, spec_trim) {
                    Some(target.path.clone())
                } else if spec_trim.starts_with('.') {
                    resolve_relative_on_disk(&file.path, spec_trim, &root).map(|p| {
                        let nk = normalize_path_key(&p);
                        if let Some(n) = index.node_for_normalized_path(&nk) {
                            n.path.clone()
                        } else {
                            stub_map
                                .entry(nk.clone())
                                .or_insert_with(|| FileNode::reference_stub(p))
                                .path
                                .clone()
                        }
                    })
                } else {
                    None
                };

            if let Some(tp) = target_path {
                let pair = (file.path.clone(), tp);
                if seen.insert(pair.clone()) {
                    edges.push(Edge {
                        source: pair.0,
                        target: pair.1,
                        edge_type: "import".to_string(),
                    });
                }
            }
        }
    }

    let reference_nodes: Vec<FileNode> = stub_map.into_values().collect();
    (edges, reference_nodes)
}

fn node_text(node: &tree_sitter::Node<'_>, source: &str) -> Option<String> {
    node.utf8_text(source.as_bytes()).ok().map(|s| s.to_string())
}

fn string_literal_contents(node: &tree_sitter::Node<'_>, source: &str) -> Option<String> {
    match node.kind() {
        "string" | "string_literal" => {
            let t = node.utf8_text(source.as_bytes()).ok()?;
            Some(t.trim_matches(|c| c == '\'' || c == '"' || c == '`').to_string())
        }
        _ => None,
    }
}
