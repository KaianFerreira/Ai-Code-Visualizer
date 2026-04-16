/**
 * Strips optional markdown fences and parses a single JSON object from model output.
 */
export function parseJsonObject(raw: string): unknown {
  let text = raw.trim();
  const fence = /^```(?:json)?\s*\n?([\s\S]*?)\n?```\s*$/im.exec(text);
  if (fence?.[1]) {
    text = fence[1].trim();
  }
  return JSON.parse(text) as unknown;
}

export function asRecord(value: unknown): Record<string, unknown> {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("Expected a JSON object at the top level.");
  }
  return value as Record<string, unknown>;
}
