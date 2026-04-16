import "dotenv/config";

import { access, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";

import { delay } from "./asyncHelpers.js";
import { AgentManager } from "./AgentManager.js";
import { architectAgent, securityAgent } from "./agents.js";
import type {
  AnalysisResult,
  EnrichedAnalysisResult,
  EnrichedFileNode,
  FileNode,
  LayerMap,
  SecurityFinding,
  SecurityMap,
} from "./types.js";

const DEFAULT_CHUNK_SIZE = 10;
const DEFAULT_DELAY_BETWEEN_CHUNKS_MS = 2000;

/** Resolved at runtime so `dotenv/config` has loaded first. */
function orchestrationChunkSize(): number {
  return readPositiveIntFromEnv("CHUNK_SIZE", DEFAULT_CHUNK_SIZE);
}

function orchestrationChunkCooldownMs(): number {
  return readNonNegativeIntFromEnv(
    "DELAY_BETWEEN_CHUNKS_MS",
    DEFAULT_DELAY_BETWEEN_CHUNKS_MS,
  );
}

function readPositiveIntFromEnv(name: string, defaultValue: number): number {
  const raw = process.env[name];
  if (raw === undefined || raw.trim() === "") {
    return defaultValue;
  }
  const n = Number.parseInt(raw.trim(), 10);
  if (!Number.isFinite(n) || n < 1) {
    return defaultValue;
  }
  return n;
}

function readNonNegativeIntFromEnv(name: string, defaultValue: number): number {
  const raw = process.env[name];
  if (raw === undefined || raw.trim() === "") {
    return defaultValue;
  }
  const n = Number.parseInt(raw.trim(), 10);
  if (!Number.isFinite(n) || n < 0) {
    return defaultValue;
  }
  return n;
}

export { delay } from "./asyncHelpers.js";
export { AgentManager } from "./AgentManager.js";
export { architectAgent, securityAgent } from "./agents.js";
export { runOrchestration } from "./orchestrator.js";
export { AgentApiError, AgentParseError } from "./errors.js";
export type {
  AnalysisResult,
  ArchitecturalLayer,
  CodeGraph,
  Edge,
  EnrichedAnalysisResult,
  EnrichedFileNode,
  FileNode,
  FileNodeMetadata,
  LayerMap,
  OrchestrationResult,
  SecurityFinding,
  SecurityMap,
  SecurityPriority,
} from "./types.js";

function splitIntoChunks<T>(items: T[], size: number): T[][] {
  if (size <= 0) {
    throw new Error("chunk size must be positive");
  }
  const out: T[][] = [];
  for (let i = 0; i < items.length; i += size) {
    out.push(items.slice(i, i + size));
  }
  return out;
}

function normalizePathKey(p: string): string {
  return p.replace(/\\/g, "/");
}

function lookupLayer(layers: LayerMap, filePath: string): string | undefined {
  const direct = layers[filePath];
  if (typeof direct === "string" && direct.length > 0) {
    return direct;
  }
  const norm = normalizePathKey(filePath);
  for (const [k, v] of Object.entries(layers)) {
    if (normalizePathKey(k) === norm && typeof v === "string") {
      return v;
    }
  }
  return undefined;
}

function lookupSecurity(
  security: SecurityMap,
  filePath: string,
): SecurityFinding | undefined {
  const direct = security[filePath];
  if (direct) {
    return direct;
  }
  const norm = normalizePathKey(filePath);
  for (const [k, v] of Object.entries(security)) {
    if (normalizePathKey(k) === norm) {
      return v;
    }
  }
  return undefined;
}

/**
 * Paths (normalized) that already have an architectural layer from a prior run.
 */
function analyzedPathSetFromCache(cache: EnrichedAnalysisResult): Set<string> {
  const done = new Set<string>();
  for (const f of cache.graph.files) {
    const layer = f.metadata?.layer;
    if (typeof layer === "string" && layer.trim() !== "") {
      done.add(normalizePathKey(f.path));
    }
  }
  return done;
}

function seedMapsFromCache(
  cache: EnrichedAnalysisResult,
  layers: LayerMap,
  security: SecurityMap,
): void {
  for (const ef of cache.graph.files) {
    const layer = ef.metadata?.layer;
    if (typeof layer === "string" && layer.trim() !== "") {
      layers[ef.path] = layer;
    }
    security[ef.path] = {
      priority: ef.metadata.security_priority,
      sensitiveAreas: [],
    };
  }
}

function filesPendingAnalysis(
  allFiles: FileNode[],
  doneNormalizedPaths: Set<string>,
): FileNode[] {
  return allFiles.filter((f) => !doneNormalizedPaths.has(normalizePathKey(f.path)));
}

async function tryReadEnrichedCheckpoint(outPath: string): Promise<EnrichedAnalysisResult | null> {
  try {
    await access(outPath);
  } catch {
    return null;
  }
  try {
    const raw = await readFile(outPath, "utf8");
    const parsed = JSON.parse(raw) as EnrichedAnalysisResult;
    if (!parsed.graph?.files || !Array.isArray(parsed.graph.files)) {
      return null;
    }
    return parsed;
  } catch {
    return null;
  }
}

function mergeInsightsIntoGraph(
  analysis: AnalysisResult,
  layers: LayerMap,
  security: SecurityMap,
): EnrichedAnalysisResult {
  const files: EnrichedFileNode[] = analysis.graph.files.map((f) => {
    const layer = lookupLayer(layers, f.path);
    const sec = lookupSecurity(security, f.path);
    const metadata: EnrichedFileNode["metadata"] = {
      security_priority: sec?.priority ?? "none",
    };
    if (layer !== undefined) {
      metadata.layer = layer;
    }
    return { ...f, metadata };
  });

  return {
    graph: {
      files,
      edges: analysis.graph.edges,
    },
    total_files: analysis.total_files,
    total_lines: analysis.total_lines,
    analysis_time_ms: analysis.analysis_time_ms,
  };
}

async function resolveGraphInputPath(): Promise<string> {
  const fromEnv = process.env["GRAPH_INPUT"];
  if (fromEnv && fromEnv.trim() !== "") {
    return path.resolve(fromEnv.trim());
  }

  const candidates = [
    path.resolve(process.cwd(), "graph_output.json"),
    path.resolve(process.cwd(), "backend", "graph_output.json"),
    path.resolve(process.cwd(), "..", "backend", "graph_output.json"),
  ];

  for (const p of candidates) {
    try {
      await access(p);
      return p;
    } catch {
      /* try next */
    }
  }

  throw new Error(
    `Could not find graph_output.json. Set GRAPH_INPUT or place the file in one of: ${candidates.join(", ")}`,
  );
}

function resolveEnrichedOutputPath(): string {
  const fromEnv = process.env["ENRICHED_OUTPUT"];
  if (fromEnv && fromEnv.trim() !== "") {
    return path.resolve(fromEnv.trim());
  }
  return path.resolve(process.cwd(), "enriched_graph.json");
}

/**
 * Loads the Rust `graph_output.json`, runs Architect + Security agents per chunk,
 * merges `metadata` onto each `FileNode`, and writes `enriched_graph.json`.
 */
export async function runOrchestrator(): Promise<void> {
  const graphPath = await resolveGraphInputPath();
  const outPath = resolveEnrichedOutputPath();
  const filesPerChunk = orchestrationChunkSize();
  const chunkCooldownMs = orchestrationChunkCooldownMs();

  console.log(`Loading graph from ${graphPath}`);

  const raw = await readFile(graphPath, "utf8");
  const analysis = JSON.parse(raw) as AnalysisResult;

  if (!analysis.graph?.files || !Array.isArray(analysis.graph.files)) {
    throw new Error("Invalid graph JSON: expected graph.files array");
  }

  const fileNodes: FileNode[] = analysis.graph.files;
  const mergedLayers: LayerMap = {};
  const mergedSecurity: SecurityMap = {};

  const existingEnriched = await tryReadEnrichedCheckpoint(outPath);
  if (existingEnriched) {
    seedMapsFromCache(existingEnriched, mergedLayers, mergedSecurity);
    const cachedCount = analyzedPathSetFromCache(existingEnriched).size;
    console.log(
      `Found checkpoint at ${outPath} (${cachedCount} file(s) with layer metadata); resuming incremental analysis.`,
    );
  }

  const pendingFiles = filesPendingAnalysis(
    fileNodes,
    existingEnriched ? analyzedPathSetFromCache(existingEnriched) : new Set(),
  );
  const chunks = splitIntoChunks(pendingFiles, filesPerChunk);

  console.log(
    `Found ${fileNodes.length} file(s) total; ${pendingFiles.length} pending; ${chunks.length} chunk(s) at ${filesPerChunk} files per request (CHUNK_SIZE).`,
  );

  if (fileNodes.length === 0) {
    console.log("No files to analyze; writing enriched graph with empty file list.");
  } else if (pendingFiles.length === 0) {
    console.log("All files already enriched from checkpoint; refreshing merged output only.");
  } else {
    const manager = new AgentManager();

    for (let i = 0; i < chunks.length; i++) {
      const chunk = chunks[i]!;
      const layerNum = i + 1;
      console.log(`Analyzing layer chunk ${layerNum} of ${chunks.length}...`);

      const [layers, sec] = await Promise.all([
        architectAgent(manager, chunk, { batchSize: filesPerChunk }),
        securityAgent(manager, chunk, { batchSize: filesPerChunk }),
      ]);

      Object.assign(mergedLayers, layers);
      Object.assign(mergedSecurity, sec);

      console.log(
        `  Chunk ${layerNum}: architectural labels + security audit finished (${chunk.length} file(s)).`,
      );

      const checkpoint = mergeInsightsIntoGraph(analysis, mergedLayers, mergedSecurity);
      await writeFile(outPath, JSON.stringify(checkpoint, null, 2), "utf8");
      console.log(`  Checkpoint written to ${outPath}`);

      if (i < chunks.length - 1) {
        await delay(chunkCooldownMs);
      }
    }

    console.log("Architectural analysis complete.");
    console.log("Security audit complete.");
  }

  const enriched = mergeInsightsIntoGraph(analysis, mergedLayers, mergedSecurity);

  await writeFile(outPath, JSON.stringify(enriched, null, 2), "utf8");
  console.log(`Wrote enriched graph to ${outPath}`);
}

function isMainModule(): boolean {
  const entry = process.argv[1];
  if (!entry) {
    return false;
  }
  try {
    return import.meta.url === pathToFileURL(path.resolve(entry)).href;
  } catch {
    return false;
  }
}

if (isMainModule()) {
  runOrchestrator().catch((err: unknown) => {
    console.error("Orchestrator failed:", err);
    process.exitCode = 1;
  });
}
