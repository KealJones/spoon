import { stdin, stdout } from "node:process";
import { createInterface } from "node:readline/promises";

import { TeacherError } from "./errors.js";
import { buildTeacherPrompt } from "./prompt.js";
import { SourceReliabilityTracker } from "./reliability.js";
import {
  ProposalValidationPipeline,
  type ProposalValidationPipelineOptions,
} from "./validation.js";
import {
  atProviderBoundary,
  defaultClock,
  defaultIdFactory,
  isJsonValue,
  makeProposal,
  parseJsonContent,
} from "./shared.js";
import type {
  Clock,
  HumanPrompt,
  IdFactory,
  JsonValue,
  PromptBuilder,
  Teacher,
  TeacherProposal,
  TeacherRequest,
} from "./types.js";

export interface HumanTeacherOptions {
  name?: string;
  prompt?: HumanPrompt;
  reliabilityTracker?: SourceReliabilityTracker;
  now?: Clock;
  idFactory?: IdFactory;
  promptBuilder?: PromptBuilder;
}

export class HumanTeacher implements Teacher {
  readonly #prompt: HumanPrompt;
  readonly #tracker: SourceReliabilityTracker;
  readonly #now: Clock;
  readonly #idFactory: IdFactory;
  readonly #source: string;
  readonly #promptBuilder: PromptBuilder;

  constructor(options: HumanTeacherOptions = {}) {
    this.#source = `human:${options.name ?? "cli"}`;
    this.#prompt = options.prompt ?? promptInTerminal;
    this.#tracker =
      options.reliabilityTracker ?? new SourceReliabilityTracker();
    this.#now = options.now ?? defaultClock;
    this.#idFactory = options.idFactory ?? defaultIdFactory;
    this.#promptBuilder = options.promptBuilder ?? buildTeacherPrompt;
  }

  async propose(request: TeacherRequest): Promise<TeacherProposal> {
    const answer = await atProviderBoundary("human", "prompt failed", () =>
      this.#prompt(
        `${this.#promptBuilder(request)}\n\nEnter a JSON response matching the schema:\n`,
      ),
    );
    let content: JsonValue;
    if (typeof answer === "string") {
      content = parseJsonContent("human", answer);
    } else if (isJsonValue(answer)) {
      content = answer;
    } else {
      throw new TeacherError(
        "human",
        "prompt returned a value that is not JSON-compatible",
      );
    }

    return makeProposal({
      content,
      provider: "human",
      source: this.#source,
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

async function promptInTerminal(message: string): Promise<string> {
  const terminal = createInterface({ input: stdin, output: stdout });
  try {
    return await terminal.question(message);
  } finally {
    terminal.close();
  }
}
