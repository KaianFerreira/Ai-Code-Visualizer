import Anthropic from "@anthropic-ai/sdk";
import OpenAI from "openai";

import { AgentApiError } from "./errors.js";

function requireEnv(name: string, provider: "anthropic" | "openai"): string {
  const v = process.env[name];
  if (v === undefined || v.trim() === "" || v === "your_key_here") {
    throw new AgentApiError(
      provider,
      `Missing or placeholder environment variable: ${name}. Set it in .env or the process environment.`,
    );
  }
  return v.trim();
}

export class AgentManager {
  readonly anthropic: Anthropic;
  readonly openai: OpenAI;
  readonly anthropicModel: string;
  readonly openaiModel: string;

  constructor(options?: {
    anthropicApiKey?: string;
    openaiApiKey?: string;
    anthropicModel?: string;
    openaiModel?: string;
  }) {
    const anthropicKey =
      options?.anthropicApiKey ?? requireEnv("ANTHROPIC_API_KEY", "anthropic");
    const openaiKey = options?.openaiApiKey ?? requireEnv("OPENAI_API_KEY", "openai");

    this.anthropic = new Anthropic({ apiKey: anthropicKey });
    this.openai = new OpenAI({ apiKey: openaiKey });
    this.anthropicModel =
      options?.anthropicModel ?? process.env["ANTHROPIC_MODEL"] ?? "claude-sonnet-4-20250514";
    this.openaiModel = options?.openaiModel ?? process.env["OPENAI_MODEL"] ?? "gpt-4o";
  }
}
