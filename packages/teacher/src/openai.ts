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
  Teacher,
  TeacherProposal,
  TeacherRequest,
} from "./types.js";

export interface OpenAITeacherOptions {
  model: string;
  apiKey?: string;
  baseUrl?: string;
  fetch?: Fetch;
  reliabilityTracker?: SourceReliabilityTracker;
  now?: Clock;
  idFactory?: IdFactory;
}

interface OpenAIContent {
  type?: string;
  text?: string;
  refusal?: string;
}

interface OpenAIResponse {
  id?: string;
  output?: Array<{ type?: string; content?: OpenAIContent[] }>;
  error?: { message?: string };
}

export class OpenAITeacher implements Teacher {
  readonly #model: string;
  readonly #apiKey?: string;
  readonly #baseUrl: string;
  readonly #fetch: Fetch;
  readonly #tracker: SourceReliabilityTracker;
  readonly #now: Clock;
  readonly #idFactory: IdFactory;
  readonly #source: string;

  constructor(options: OpenAITeacherOptions) {
    this.#model = options.model;
    this.#apiKey = options.apiKey ?? process.env.OPENAI_API_KEY;
    this.#baseUrl = (options.baseUrl ?? "https://api.openai.com/v1").replace(
      /\/$/,
      "",
    );
    this.#fetch = options.fetch ?? globalThis.fetch;
    this.#tracker =
      options.reliabilityTracker ?? new SourceReliabilityTracker();
    this.#now = options.now ?? defaultClock;
    this.#idFactory = options.idFactory ?? defaultIdFactory;
    this.#source = `openai:${this.#model}`;
  }

  async propose(request: TeacherRequest): Promise<TeacherProposal> {
    if (!this.#apiKey) {
      throw new TeacherError("openai", "OPENAI_API_KEY or apiKey is required");
    }
    const response = await atProviderBoundary(
      "openai",
      "Responses API request failed",
      () =>
        this.#fetch(`${this.#baseUrl}/responses`, {
          method: "POST",
          headers: {
            authorization: `Bearer ${this.#apiKey}`,
            "content-type": "application/json",
          },
          body: JSON.stringify({
            model: this.#model,
            input: [
              { role: "system", content: TEACHER_SYSTEM_PROMPT },
              { role: "user", content: buildTeacherPrompt(request) },
            ],
            text: {
              format: {
                type: "json_schema",
                name: "ekg_teacher_proposal",
                strict: true,
                schema: request.desiredOutput,
              },
            },
          }),
        }),
    );
    const payload = (await readJson(response, "openai")) as OpenAIResponse;
    if (!response.ok) {
      throw new TeacherError(
        "openai",
        `Responses API returned ${response.status}: ${payload.error?.message ?? response.statusText}`,
      );
    }

    const content = payload.output
      ?.flatMap((item) => item.content ?? [])
      .find((item) => item.type === "output_text");
    const refusal = payload.output
      ?.flatMap((item) => item.content ?? [])
      .find((item) => item.type === "refusal");
    if (refusal)
      throw new TeacherError(
        "openai",
        `model refused the request: ${refusal.refusal ?? ""}`,
      );
    if (!content?.text) {
      throw new TeacherError(
        "openai",
        "Responses API result did not contain output_text",
      );
    }

    return makeProposal({
      content: parseJsonContent("openai", content.text),
      provider: "openai",
      source: this.#source,
      model: this.#model,
      request,
      requestId: this.#idFactory(),
      providerRequestId: payload.id,
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

async function readJson(
  response: Response,
  provider: string,
): Promise<unknown> {
  try {
    return await response.json();
  } catch (error) {
    throw new TeacherError(provider, "provider returned a non-JSON response", {
      cause: error,
    });
  }
}
