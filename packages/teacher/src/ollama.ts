import { request as httpRequest } from "node:http";
import type { IncomingMessage } from "node:http";
import { request as httpsRequest } from "node:https";

import { TeacherError } from "./errors.js";
import { buildTeacherPrompt, TEACHER_SYSTEM_PROMPT } from "./prompt.js";
import { SourceReliabilityTracker } from "./reliability.js";
import {
  ProposalValidationPipeline,
  type ProposalValidationPipelineOptions,
} from "./validation.js";
import {
  atProviderBoundary,
  defaultClock,
  defaultIdFactory,
  makeProposal,
  parseJsonContent,
} from "./shared.js";
import type {
  Clock,
  Fetch,
  IdFactory,
  PromptBuilder,
  Teacher,
  TeacherProposal,
  TeacherRequest,
} from "./types.js";

const OLLAMA_FETCH_TIMEOUT_MS = 60 * 60 * 1000;

function defaultOllamaFetch(
  input: string | URL,
  init?: RequestInit,
): Promise<Response> {
  const url = new URL(String(input));
  const send = url.protocol === "https:" ? httpsRequest : httpRequest;
  const headers = new Headers(init?.headers);
  if (!headers.has("content-type")) {
    headers.set("content-type", "application/json");
  }
  const headerRecord = Object.fromEntries(headers.entries());
  const body =
    typeof init?.body === "string" || init?.body === undefined
      ? init?.body
      : undefined;
  if (init?.body !== undefined && body === undefined) {
    return Promise.reject(
      new TeacherError("ollama", "generate API request failed"),
    );
  }

  return new Promise((resolve, reject) => {
    const request = send(
      url,
      {
        method: init?.method ?? "GET",
        headers: headerRecord,
      },
      (response: IncomingMessage) => {
        const chunks: Buffer[] = [];
        response.on("data", (chunk: Buffer | string) => {
          chunks.push(typeof chunk === "string" ? Buffer.from(chunk) : chunk);
        });
        response.on("end", () => {
          resolve(
            new Response(Buffer.concat(chunks), {
              status: response.statusCode ?? 502,
              statusText: response.statusMessage ?? "",
            }),
          );
        });
        response.on("error", reject);
      },
    );
    request.setTimeout(OLLAMA_FETCH_TIMEOUT_MS, () => {
      request.destroy(new Error("ollama generate timed out"));
    });
    request.on("error", reject);
    if (body !== undefined) request.write(body);
    request.end();
  });
}

export interface OllamaTeacherOptions {
  model?: string;
  baseUrl?: string;
  fetch?: Fetch;
  reliabilityTracker?: SourceReliabilityTracker;
  now?: Clock;
  idFactory?: IdFactory;
  systemPrompt?: string;
  promptBuilder?: PromptBuilder;
}

interface OllamaResponse {
  response?: string;
  thinking?: string;
  error?: string;
  done?: boolean;
}

export class OllamaTeacher implements Teacher {
  readonly #model: string;
  readonly #baseUrl: string;
  readonly #fetch: Fetch;
  readonly #tracker: SourceReliabilityTracker;
  readonly #now: Clock;
  readonly #idFactory: IdFactory;
  readonly #source: string;
  readonly #systemPrompt: string;
  readonly #promptBuilder: PromptBuilder;

  constructor(options: OllamaTeacherOptions = {}) {
    this.#model = options.model ?? "qwen2.5:1.5b";
    this.#baseUrl = (options.baseUrl ?? "http://localhost:11434").replace(
      /\/$/,
      "",
    );
    this.#fetch = options.fetch ?? defaultOllamaFetch;
    this.#tracker =
      options.reliabilityTracker ?? new SourceReliabilityTracker();
    this.#now = options.now ?? defaultClock;
    this.#idFactory = options.idFactory ?? defaultIdFactory;
    this.#source = `ollama:${this.#model}`;
    this.#systemPrompt = options.systemPrompt ?? TEACHER_SYSTEM_PROMPT;
    this.#promptBuilder = options.promptBuilder ?? buildTeacherPrompt;
  }

  async propose(request: TeacherRequest): Promise<TeacherProposal> {
    const response = await atProviderBoundary(
      "ollama",
      "generate API request failed",
      () =>
        this.#fetch(`${this.#baseUrl}/api/generate`, {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({
            model: this.#model,
            prompt: this.#promptBuilder(request),
            system: this.#systemPrompt,
            stream: true,
            keep_alive: "30m",
            format: request.desiredOutput,
            think: false,
          }),
        }),
    );
    const payload = await readOllamaGenerate(response);
    if (!response.ok || payload.error) {
      throw new TeacherError(
        "ollama",
        `generate API returned ${response.status}: ${payload.error ?? response.statusText}`,
      );
    }
    const content = ollamaStructuredContent(payload);
    if (content.trim().length === 0) {
      throw new TeacherError(
        "ollama",
        "generate API result did not contain response",
      );
    }

    return makeProposal({
      content: parseJsonContent("ollama", content),
      provider: "ollama",
      source: this.#source,
      model: this.#model,
      request,
      requestId: this.#idFactory(),
      generatedAt: this.#now(),
    });
  }

  reliability() {
    return this.#tracker.get(this.#source);
  }

  validationPipeline(
    options: Omit<ProposalValidationPipelineOptions, "reliabilityTracker"> = {},
  ) {
    return new ProposalValidationPipeline({
      ...options,
      reliabilityTracker: this.#tracker,
    });
  }
}

async function readOllamaGenerate(response: Response): Promise<OllamaResponse> {
  let raw: string;
  try {
    raw = await response.text();
  } catch (error) {
    throw new TeacherError("ollama", "provider returned a non-JSON response", {
      cause: error,
    });
  }

  const trimmed = raw.trim();
  if (trimmed.length === 0) {
    throw new TeacherError("ollama", "provider returned a non-JSON response");
  }

  try {
    return JSON.parse(trimmed) as OllamaResponse;
  } catch {
    // Streaming generate emits one JSON object per line.
  }

  let assembledResponse = "";
  let assembledThinking = "";
  let error: string | undefined;
  for (const line of trimmed.split("\n")) {
    const chunk = line.trim();
    if (chunk.length === 0) continue;
    let parsed: OllamaResponse;
    try {
      parsed = JSON.parse(chunk) as OllamaResponse;
    } catch (cause) {
      throw new TeacherError(
        "ollama",
        "provider returned a non-JSON response",
        { cause },
      );
    }
    if (parsed.error) error = parsed.error;
    if (parsed.response !== undefined) assembledResponse += parsed.response;
    if (parsed.thinking !== undefined) assembledThinking += parsed.thinking;
  }
  return { response: assembledResponse, thinking: assembledThinking, error };
}

function ollamaStructuredContent(payload: OllamaResponse): string {
  const response = payload.response ?? "";
  if (response.trim().length > 0) return response;
  return payload.thinking ?? "";
}
