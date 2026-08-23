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
  error?: string;
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
    this.#model = options.model ?? "qwen3:8b";
    this.#baseUrl = (options.baseUrl ?? "http://localhost:11434").replace(
      /\/$/,
      "",
    );
    this.#fetch = options.fetch ?? globalThis.fetch;
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
            stream: false,
            format: request.desiredOutput,
          }),
        }),
    );
    let payload: OllamaResponse;
    try {
      payload = (await response.json()) as OllamaResponse;
    } catch (error) {
      throw new TeacherError(
        "ollama",
        "provider returned a non-JSON response",
        { cause: error },
      );
    }
    if (!response.ok || payload.error) {
      throw new TeacherError(
        "ollama",
        `generate API returned ${response.status}: ${payload.error ?? response.statusText}`,
      );
    }
    if (payload.response === undefined) {
      throw new TeacherError(
        "ollama",
        "generate API result did not contain response",
      );
    }

    return makeProposal({
      content: parseJsonContent("ollama", payload.response),
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
