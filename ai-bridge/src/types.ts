/** Mirrors `backend/src/models.rs` / `graph_output.json` shape. */

export type ArchitecturalLayer =
  | "Infrastructure"
  | "Domain"
  | "Application"
  | "UI";

export interface FileNode {
  path: string;
  name: string;
  language: string;
  functions: string[];
  classes: string[];
  imports: string[];
  line_count: number;
}

export interface Edge {
  source: string;
  target: string;
  edge_type: string;
}

export interface CodeGraph {
  files: FileNode[];
  edges: Edge[];
}

export interface AnalysisResult {
  graph: CodeGraph;
  total_files: number;
  total_lines: number;
  analysis_time_ms: number;
}

/** Path → architectural layer (Architect agent). */
export type LayerMap = Record<string, ArchitecturalLayer | string>;

export type SecurityPriority = "critical" | "high" | "medium" | "low" | "none";

export interface SecurityFinding {
  priority: SecurityPriority;
  /** e.g. Authentication, Database Transactions, API Secrets */
  sensitiveAreas: string[];
  rationale?: string;
}

/** Path → security assessment (Security agent). */
export type SecurityMap = Record<string, SecurityFinding>;

export interface OrchestrationResult {
  layers: LayerMap;
  security: SecurityMap;
}

/** Per-file AI enrichment written to `enriched_graph.json`. */
export interface FileNodeMetadata {
  /** Architectural layer from the Architect agent (when returned). */
  layer?: string;
  security_priority: SecurityPriority;
}

export interface EnrichedFileNode extends FileNode {
  metadata: FileNodeMetadata;
}

export interface EnrichedAnalysisResult {
  graph: {
    files: EnrichedFileNode[];
    edges: Edge[];
  };
  total_files: number;
  total_lines: number;
  analysis_time_ms: number;
}
