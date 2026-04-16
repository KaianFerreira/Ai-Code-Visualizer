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

/** Prefer primary key (matches `.env-example`); fall back to legacy name. */
function envModelOrFallback(primary: string, legacy: string, defaultModel: string): string {
  const a = process.env[primary]?.trim();
  if (a) {
    return a;
  }
  const b = process.env[legacy]?.trim();
  if (b) {
    return b;
  }
  return defaultModel;
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
      options?.anthropicModel ??
      envModelOrFallback(
        "ANTHROPIC_ARCHITECT_MODEL",
        "ANTHROPIC_MODEL",
        "claude-3-5-sonnet-latest",
      );
    this.openaiModel =
      options?.openaiModel ??
      envModelOrFallback("OPENAI_SECURITY_MODEL", "OPENAI_MODEL", "gpt-4o");
  }
}
