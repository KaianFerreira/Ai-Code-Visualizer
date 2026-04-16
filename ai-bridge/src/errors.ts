export class AgentApiError extends Error {
  readonly provider: "anthropic" | "openai";
  readonly status?: number;
  readonly causeDetail?: string;

  constructor(
    provider: "anthropic" | "openai",
    message: string,
    options?: { status?: number; cause?: unknown; causeDetail?: string },
  ) {
    super(message);
    this.name = "AgentApiError";
    this.provider = provider;
    this.status = options?.status;
    this.causeDetail = options?.causeDetail;
    if (options?.cause !== undefined) {
      (this as Error & { cause?: unknown }).cause = options.cause;
    }
  }
}

export class AgentParseError extends Error {
  readonly rawSnippet: string;

  constructor(message: string, rawSnippet: string) {
    super(message);
    this.name = "AgentParseError";
    this.rawSnippet = rawSnippet;
  }
}
