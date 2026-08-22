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
}

export class CodexTeacher implements Teacher {
  readonly #command: string;
  readonly #model?: string;
  readonly #runner: CommandRunner;
  readonly #tracker: SourceReliabilityTracker;
  readonly #now: Clock;
  readonly #idFactory: IdFactory;
  readonly #source: string;

  constructor(options: CodexTeacherOptions = {}) {
    this.#command = options.command ?? "codex";
    this.#model = options.model;
    this.#runner = options.runner ?? runCommand;
    this.#tracker =
      options.reliabilityTracker ?? new SourceReliabilityTracker();
    this.#now = options.now ?? defaultClock;
    this.#idFactory = options.idFactory ?? defaultIdFactory;
    this.#source = `codex:${this.#model ?? "default"}`;
  }

  async propose(request: TeacherRequest): Promise<TeacherProposal> {
    const directory = await mkdtemp(
      path.join(os.tmpdir(), "ekg-codex-teacher-"),
    );
    const schemaPath = path.join(directory, "proposal.schema.json");
    const outputPath = path.join(directory, "proposal.json");
    try {
      await writeFile(schemaPath, JSON.stringify(request.desiredOutput));
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
      args.push(
        `${TEACHER_SYSTEM_PROMPT}\n\n${buildTeacherPrompt(request)}\n\nReturn only the requested JSON object.`,
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
      return makeProposal({
        content: parseJsonContent("codex", content),
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
