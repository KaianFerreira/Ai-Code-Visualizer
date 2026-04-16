import type { AgentManager } from "./AgentManager.js";
import { architectAgent, securityAgent } from "./agents.js";
import type { FileNode, OrchestrationResult } from "./types.js";

/**
 * Runs Architect (Anthropic) and Security (OpenAI) agents concurrently.
 * Each agent batches internally; both complete with `Promise.all`.
 */
export async function runOrchestration(
  manager: AgentManager,
  fileNodes: FileNode[],
  options?: { batchSize?: number },
): Promise<OrchestrationResult> {
  const batchSize = options?.batchSize;
  const [layers, security] = await Promise.all([
    architectAgent(manager, fileNodes, { batchSize }),
    securityAgent(manager, fileNodes, { batchSize }),
  ]);
  return { layers, security };
}
