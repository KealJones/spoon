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
import { runCommand } from "./claude.js";
import type {
  Clock,
  CommandRunner,
  IdFactory,
  PromptBuilder,
  Teacher,
  TeacherProposal,
  TeacherRequest,
} from "./types.js";

/**
 * Adapter for a user-supplied JSON-producing CLI. The command is tokenized
 * without a shell; the structured prompt is appended as its final argument.
 * This keeps the provider boundary compatible with the dedicated Claude and
 * Codex adapters while allowing local tools to participate.
 */
export interface CliTeacherOptions {
  command: string;
  model?: string;
  runner?: CommandRunner;
  reliabilityTracker?: SourceReliabilityTracker;
  now?: Clock;
  idFactory?: IdFactory;
  systemPrompt?: string;
  promptBuilder?: PromptBuilder;
}

export class CliTeacher implements Teacher {
  readonly #command: string;
  readonly #model?: string;
  readonly #runner: CommandRunner;
  readonly #tracker: SourceReliabilityTracker;
  readonly #now: Clock;
  readonly #idFactory: IdFactory;
  readonly #source: string;
  readonly #systemPrompt: string;
  readonly #promptBuilder: PromptBuilder;

  constructor(options: CliTeacherOptions) {
    if (options.command.trim() === "")
      throw new Error("CLI teacher command must be non-empty");
    this.#command = options.command;
    this.#model = options.model;
    this.#runner = options.runner ?? runCommand;
    this.#tracker =
      options.reliabilityTracker ?? new SourceReliabilityTracker();
    this.#now = options.now ?? defaultClock;
    this.#idFactory = options.idFactory ?? defaultIdFactory;
    this.#source = `cli:${this.#command}`;
    this.#systemPrompt = options.systemPrompt ?? TEACHER_SYSTEM_PROMPT;
    this.#promptBuilder = options.promptBuilder ?? buildTeacherPrompt;
  }

  async propose(request: TeacherRequest): Promise<TeacherProposal> {
    const prompt = [
      this.#systemPrompt,
      this.#promptBuilder(request),
      "Return only the requested JSON object.",
    ].join("\n\n");
    const tokens = tokenizeCommand(this.#command);
    if (tokens.length === 0) {
      throw new TeacherError("cli", "command did not contain an executable");
    }
    const [command, ...args] = tokens;
    const result = await atProviderBoundary(
      "cli",
      "command invocation failed",
      () => this.#runner({ command: command!, args: [...args, prompt] }),
    );
    if (result.exitCode !== 0) {
      throw new TeacherError(
        "cli",
        `command exited with status ${result.exitCode}: ${result.stderr.trim() || "unknown error"}`,
      );
    }
    return makeProposal({
      content: parseCliJson(result.stdout),
      provider: "cli",
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
    // The CLI adapter intentionally shares the same reliability/proposal
    // validation behavior as the first-party adapters.
    return new ProposalValidationPipeline({
      ...options,
      reliabilityTracker: this.#tracker,
    });
  }
}

export function parseCliJson(value: string): import("./types.js").JsonValue {
  const trimmed = value.trim();
  try {
    return parseJsonContent("cli", trimmed);
  } catch {
    const fenced = trimmed.match(/```(?:json)?\s*([\s\S]*?)\s*```/i)?.[1];
    if (fenced !== undefined) return parseJsonContent("cli", fenced);
    throw new TeacherError("cli", "command did not return valid JSON");
  }
}

function tokenizeCommand(command: string): string[] {
  const tokens: string[] = [];
  const pattern = /"([^"\\]*(?:\\.[^"\\]*)*)"|'([^']*)'|([^\s]+)/g;
  for (const match of command.matchAll(pattern)) {
    tokens.push(match[1] ?? match[2] ?? match[3] ?? "");
  }
  return tokens;
}
