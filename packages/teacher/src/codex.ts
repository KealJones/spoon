import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";

import { TeacherError } from "./errors.js";
import {
  CODEX_FLAT_AUTHORING_INSTRUCTION,
  CODEX_FLAT_AUTHORING_SCHEMA,
  decodeCodexFlatAuthoring,
  isCodexFlatAuthoringSchema,
} from "./flat-authoring.js";
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
      const usesFlatAuthoring = isCodexFlatAuthoringSchema(outputSchema);
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
      const promptRequest = usesFlatAuthoring
        ? { ...providerRequest, desiredOutput: CODEX_FLAT_PROMPT_SCHEMA }
        : providerRequest;
      const providerInstruction = usesFlatAuthoring
        ? CODEX_FLAT_AUTHORING_INSTRUCTION
        : usesJsonEnvelope
          ? "The required outer object has proposalJson. Put the complete requested JSON as an escaped JSON string in proposalJson."
          : "";
      args.push(
        `${this.#systemPrompt}\n\n${this.#promptBuilder(promptRequest)}\n\n${providerInstruction}\n\nReturn only the requested JSON object.`,
      );

      const result = await atProviderBoundary(
        "codex",
        "command invocation failed",
        () => this.#runner({ command: this.#command, args, cwd: directory }),
      );
      if (result.exitCode !== 0) {
        throw new TeacherError(
          "codex",
          `command exited with status ${result.exitCode}: ${commandFailureDetail(result.stderr)}`,
        );
      }
      const content = await atProviderBoundary(
        "codex",
        "command did not write its final response",
        () => readFile(outputPath, "utf8"),
      );
      const parsed = parseJsonContent("codex", content);
      return makeProposal({
        content: usesFlatAuthoring
          ? decodeCodexFlatAuthoring(parsed)
          : usesJsonEnvelope
            ? unwrapCodexJsonEnvelope(parsed)
            : parsed,
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
 * grammar. Give it a strict non-recursive node graph instead; the adapter
 * expands that graph into canonical `pure_expr_v2` before local validation.
 */
export function lowerCodexSchema(schema: ProposalSchema): ProposalSchema {
  if (!needsCodexFlatAuthoring(schema)) return schema;
  return isSpoonTeacherProposalSchema(schema)
    ? CODEX_FLAT_AUTHORING_SCHEMA
    : CODEX_JSON_ENVELOPE_SCHEMA;
}

const CODEX_JSON_ENVELOPE_SCHEMA: ProposalSchema = {
  type: "object",
  additionalProperties: false,
  properties: { proposalJson: { type: "string" } },
  required: ["proposalJson"],
};

const CODEX_FLAT_PROMPT_SCHEMA: ProposalSchema = {
  type: "object",
  description:
    "The enforced spoon_flat_expr_v1 output schema is supplied separately through Codex structured output. Follow the flat-wire syntax card below.",
};

function isSpoonTeacherProposalSchema(schema: ProposalSchema): boolean {
  const properties = schema.properties;
  return (
    properties !== undefined &&
    [
      "proposalKind",
      "interpretations",
      "lesson",
      "procedure",
      "answer",
      "abstainReason",
    ].every((key) => key in properties)
  );
}

function needsCodexFlatAuthoring(value: unknown): boolean {
  if (Array.isArray(value)) return value.some(needsCodexFlatAuthoring);
  if (typeof value !== "object" || value === null) return false;
  return Object.entries(value).some(
    ([key, child]) =>
      key === "$defs" ||
      key === "$ref" ||
      (key === "type" && Array.isArray(child)) ||
      needsCodexFlatAuthoring(child),
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

function commandFailureDetail(stderr: string): string {
  const detail = stderr.trim();
  if (!detail) return "unknown error";
  const maxChars = 4_000;
  return detail.length > maxChars ? `…${detail.slice(-maxChars)}` : detail;
}
