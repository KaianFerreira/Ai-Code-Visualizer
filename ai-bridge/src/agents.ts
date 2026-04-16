import type { AgentManager } from "./AgentManager.js";
import { AgentApiError, AgentParseError } from "./errors.js";
import { asRecord, parseJsonObject } from "./jsonUtils.js";
import type { FileNode, LayerMap, SecurityFinding, SecurityMap } from "./types.js";

const ARCHITECT_SYSTEM = `You are a software architect. You respond with only valid JSON: a single object whose keys are file paths (exactly as given) and whose values are one of: "Infrastructure", "Domain", "Application", "UI". No markdown, no commentary.`;

const SECURITY_SYSTEM = `You are a security reviewer. Respond with only valid JSON: a single object whose keys are file paths (exactly as given) and whose values are objects with:
- "priority": one of "critical", "high", "medium", "low", "none"
- "sensitiveAreas": string array (e.g. "Authentication", "Database Transactions", "API Secrets")
- optional "rationale": short string
Include an entry for every path you assess; use priority "none" when not sensitive. No markdown.`;

function chunk<T>(items: T[], size: number): T[][] {
  if (size <= 0) {
    throw new Error("batch size must be positive");
  }
  const out: T[][] = [];
  for (let i = 0; i < items.length; i += size) {
    out.push(items.slice(i, i + size));
  }
  return out;
}

function textFromAnthropicContent(
  content: Array<{ type: string; text?: string }>,
): string {
  return content
    .filter((b): b is { type: "text"; text: string } => b.type === "text" && typeof b.text === "string")
    .map((b) => b.text)
    .join("\n");
}

function normalizeLayerMap(raw: unknown): LayerMap {
  const obj = asRecord(raw);
  const out: LayerMap = {};
  for (const [path, v] of Object.entries(obj)) {
    if (typeof v === "string") {
      out[path] = v;
    }
  }
  return out;
}

function readStringArray(v: unknown): string[] {
  if (!Array.isArray(v)) {
    return [];
  }
  return v.filter((x): x is string => typeof x === "string");
}

function normalizeSecurityMap(raw: unknown): SecurityMap {
  let obj = asRecord(raw);
  const nested = obj["files"];
  if (
    nested !== null &&
    typeof nested === "object" &&
    !Array.isArray(nested) &&
    nested !== undefined
  ) {
    obj = nested as Record<string, unknown>;
  }

  const out: SecurityMap = {};
  for (const [path, v] of Object.entries(obj)) {
    if (v === null || typeof v !== "object" || Array.isArray(v)) {
      continue;
    }
    const rec = v as Record<string, unknown>;
    const priorityRaw = rec["priority"];
    let priority: SecurityFinding["priority"] = "none";
    if (
      typeof priorityRaw === "string" &&
      (priorityRaw === "critical" ||
        priorityRaw === "high" ||
        priorityRaw === "medium" ||
        priorityRaw === "low" ||
        priorityRaw === "none")
    ) {
      priority = priorityRaw;
    }
    const sensitiveAreas = readStringArray(rec["sensitiveAreas"]);
    const rationaleRaw = rec["rationale"];
    const finding: SecurityFinding = {
      priority,
      sensitiveAreas,
    };
    if (typeof rationaleRaw === "string" && rationaleRaw.length > 0) {
      finding.rationale = rationaleRaw;
    }
    out[path] = finding;
  }
  return out;
}

function architectPromptForBatch(batch: FileNode[]): string {
  const data = JSON.stringify(batch);
  return `Analyze the following code structure: ${data}. Identify the architectural pattern (e.g., Clean Architecture, MVC) and assign a "layer" (Infrastructure, Domain, Application, or UI) to each file. Return the result strictly as a JSON object mapping file paths to their layers.`;
}

function securityPromptForBatch(batch: FileNode[]): string {
  const slim = batch.map((f) => ({
    path: f.path,
    imports: f.imports,
    functions: f.functions,
    classes: f.classes,
  }));
  const data = JSON.stringify(slim);
  return `Review these imports and function names. Identify files that handle sensitive logic like Authentication, Database Transactions, or API Secrets. Mark them with a priority flag. Data: ${data}`;
}

async function architectBatch(
  manager: AgentManager,
  batch: FileNode[],
): Promise<LayerMap> {
  const prompt = architectPromptForBatch(batch);
  try {
    const msg = await manager.anthropic.messages.create({
      model: manager.anthropicModel,
      max_tokens: 8192,
      system: ARCHITECT_SYSTEM,
      messages: [{ role: "user", content: prompt }],
    });
    const text = textFromAnthropicContent(
      msg.content as Array<{ type: string; text?: string }>,
    );
    if (!text.trim()) {
      throw new AgentParseError("Empty response from Anthropic.", text);
    }
    let parsed: unknown;
    try {
      parsed = parseJsonObject(text);
    } catch (e) {
      throw new AgentParseError(
        `Failed to parse Architect JSON: ${(e as Error).message}`,
        text.slice(0, 2000),
      );
    }
    return normalizeLayerMap(parsed);
  } catch (e) {
    if (e instanceof AgentParseError) {
      throw e;
    }
    const err = e as { status?: number; message?: string };
    throw new AgentApiError("anthropic", err.message ?? "Anthropic request failed", {
      status: err.status,
      cause: e,
    });
  }
}

async function securityBatch(
  manager: AgentManager,
  batch: FileNode[],
): Promise<SecurityMap> {
  const prompt = securityPromptForBatch(batch);
  try {
    const res = await manager.openai.chat.completions.create({
      model: manager.openaiModel,
      messages: [
        { role: "system", content: SECURITY_SYSTEM },
        { role: "user", content: prompt },
      ],
      response_format: { type: "json_object" },
      temperature: 0.2,
    });
    const text = res.choices[0]?.message?.content ?? "";
    if (!text.trim()) {
      throw new AgentParseError("Empty response from OpenAI.", text);
    }
    let parsed: unknown;
    try {
      parsed = parseJsonObject(text);
    } catch (e) {
      throw new AgentParseError(
        `Failed to parse Security JSON: ${(e as Error).message}`,
        text.slice(0, 2000),
      );
    }
    return normalizeSecurityMap(parsed);
  } catch (e) {
    if (e instanceof AgentParseError) {
      throw e;
    }
    const err = e as { status?: number; message?: string };
    throw new AgentApiError("openai", err.message ?? "OpenAI request failed", {
      status: err.status,
      cause: e,
    });
  }
}

/**
 * Anthropic-based architect: maps each file path to an architectural layer.
 * Uses parallel batches to stay within context limits and improve throughput.
 */
export async function architectAgent(
  manager: AgentManager,
  fileNodes: FileNode[],
  options?: { batchSize?: number },
): Promise<LayerMap> {
  if (fileNodes.length === 0) {
    return {};
  }
  const batchSize = options?.batchSize ?? 35;
  const batches = chunk(fileNodes, batchSize);
  const maps = await Promise.all(batches.map((b) => architectBatch(manager, b)));
  return maps.reduce((acc, m) => ({ ...acc, ...m }), {});
}

/**
 * OpenAI GPT-4o security review: flags sensitive files by path.
 */
export async function securityAgent(
  manager: AgentManager,
  fileNodes: FileNode[],
  options?: { batchSize?: number },
): Promise<SecurityMap> {
  if (fileNodes.length === 0) {
    return {};
  }
  const batchSize = options?.batchSize ?? 35;
  const batches = chunk(fileNodes, batchSize);
  const maps = await Promise.all(batches.map((b) => securityBatch(manager, b)));
  return maps.reduce((acc, m) => ({ ...acc, ...m }), {});
}
