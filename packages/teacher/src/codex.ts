import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";

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
  ProposalSchema,
  Teacher,
  TeacherProposal,
  TeacherRequest,
} from "./types.js";

export interface CodexTeacherOptions {
  command?: string;
  model?: string;
  runner?: CommandRunner;
  reliabilityTracker?: SourceReliabilityTracker;
  now?: Clock;
  idFactory?: IdFactory;
  systemPrompt?: string;
  promptBuilder?: PromptBuilder;
}

export class CodexTeacher implements Teacher {
  readonly #command: string;
  readonly #model?: string;
  readonly #runner: CommandRunner;
  readonly #tracker: SourceReliabilityTracker;
  readonly #now: Clock;
  readonly #idFactory: IdFactory;
  readonly #source: string;
  readonly #systemPrompt: string;
  readonly #promptBuilder: PromptBuilder;

  constructor(options: CodexTeacherOptions = {}) {
    this.#command = options.command ?? "codex";
    this.#model = options.model;
    this.#runner = options.runner ?? runCommand;
    this.#tracker =
      options.reliabilityTracker ?? new SourceReliabilityTracker();
    this.#now = options.now ?? defaultClock;
    this.#idFactory = options.idFactory ?? defaultIdFactory;
    this.#source = `codex:${this.#model ?? "default"}`;
    this.#systemPrompt = options.systemPrompt ?? TEACHER_SYSTEM_PROMPT;
    this.#promptBuilder = options.promptBuilder ?? buildTeacherPrompt;
  }

  async propose(request: TeacherRequest): Promise<TeacherProposal> {
    const directory = await mkdtemp(
      path.join(os.tmpdir(), "spoon-codex-teacher-"),
    );
    const schemaPath = path.join(directory, "proposal.schema.json");
    const outputPath = path.join(directory, "proposal.json");
    try {
      const outputSchema = lowerCodexSchema(request.desiredOutput);
      const usesJsonEnvelope = outputSchema === CODEX_JSON_ENVELOPE_SCHEMA;
      await writeFile(schemaPath, JSON.stringify(outputSchema));
      const args = [
        "exec",
        "--ephemeral",
        "--ignore-user-config",
        "--ignore-rules",
        "--sandbox",
        "read-only",
        "--skip-git-repo-check",
        "--output-schema",
        schemaPath,
        "--output-last-message",
        outputPath,
        "--color",
        "never",
      ];
      if (this.#model !== undefined) args.push("--model", this.#model);
      const providerRequest =
        outputSchema === request.desiredOutput
          ? request
          : { ...request, desiredOutput: outputSchema };
      const envelopeInstruction = usesJsonEnvelope
        ? "The required outer object has proposalJson. Put the complete canonical Spoon proposal JSON as an escaped JSON string in proposalJson; include proposalKind, interpretations, lesson, procedure, answer, and abstainReason."
        : "";
      args.push(
        `${this.#systemPrompt}\n\n${this.#promptBuilder(providerRequest)}\n\n${envelopeInstruction}\n\nReturn only the requested JSON object.`,
      );

      const result = await atProviderBoundary(
        "codex",
        "command invocation failed",
        () => this.#runner({ command: this.#command, args, cwd: directory }),
      );
      if (result.exitCode !== 0) {
        throw new TeacherError(
          "codex",
          `command exited with status ${result.exitCode}: ${result.stderr.trim() || "unknown error"}`,
        );
      }
      const content = await atProviderBoundary(
        "codex",
        "command did not write its final response",
        () => readFile(outputPath, "utf8"),
      );
      const parsed = parseJsonContent("codex", content);
      return makeProposal({
        content: usesJsonEnvelope ? unwrapCodexJsonEnvelope(parsed) : parsed,
        provider: "codex",
        source: this.#source,
        model: this.#model,
        request,
        requestId: this.#idFactory(),
        generatedAt: this.#now(),
      });
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
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

/**
 * Codex CLI currently rejects Spoon's recursive `$defs`/`$ref` lesson
 * grammar. Keep the canonical schema for prompts and local validation, while
 * giving only that provider a non-recursive envelope: the lesson remains an
 * opaque JSON object at the CLI boundary and must still pass the canonical
 * validator before Spoon uses it.
 */
export function lowerCodexSchema(schema: ProposalSchema): ProposalSchema {
  return needsCodexJsonEnvelope(schema) ? CODEX_JSON_ENVELOPE_SCHEMA : schema;
}

const CODEX_JSON_ENVELOPE_SCHEMA: ProposalSchema = {
  type: "object",
  additionalProperties: false,
  properties: { proposalJson: { type: "string" } },
  required: ["proposalJson"],
};

function needsCodexJsonEnvelope(value: unknown): boolean {
  if (Array.isArray(value)) return value.some(needsCodexJsonEnvelope);
  if (typeof value !== "object" || value === null) return false;
  return Object.entries(value).some(
    ([key, child]) =>
      key === "$defs" ||
      key === "$ref" ||
      (key === "type" && Array.isArray(child)) ||
      needsCodexJsonEnvelope(child),
  );
}

function unwrapCodexJsonEnvelope(value: unknown) {
  if (
    typeof value !== "object" ||
    value === null ||
    Array.isArray(value) ||
    typeof (value as { proposalJson?: unknown }).proposalJson !== "string"
  ) {
    throw new TeacherError(
      "codex",
      "structured output did not contain proposalJson",
    );
  }
  return parseJsonContent(
    "codex",
    (value as { proposalJson: string }).proposalJson,
  );
}
