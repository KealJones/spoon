#!/usr/bin/env node

import {
  JsonRpcError,
  SpoonClient,
  StdioTransport,
  type AdaptationPlanInput,
  type FailureAnalysisInput,
  type RecordContradictionInput,
  type RefineContradictionInput,
} from "@spoon/sdk";
import { createInterface } from "node:readline/promises";
import { stdin as input, stdout as output } from "node:process";
import { pathToFileURL } from "node:url";

import {
  createConfiguredInterpreter,
  createConfiguredTeacher,
  runTeaching,
  runCycle,
} from "./cycle.js";
import {
  runBenchmark,
  renderBenchmarkReport,
  loadBenchmarkReport,
} from "./benchmark.js";
import {
  adminTokenFromEnvironment,
  loadProjectEnvironment,
} from "./environment.js";
import {
  applyConfigEnvironment,
  configLayerPath,
  redactedConfig,
  resolveConfig,
  setConfigValue,
  validateConfigMutation,
} from "./config.js";
import { tryHandleAdminRequest } from "./admin.js";
import { parseCommand, type Command } from "./parse.js";

async function execute(
  client: SpoonClient,
  command: Command,
  resolvedConfig: Awaited<ReturnType<typeof resolveConfig>>,
): Promise<unknown> {
  switch (command.kind) {
    case "concept.add":
      return client.createConcept({ name: command.name });
    case "concept.list":
      return client.listConcepts();
    case "relationship.add":
      return client.createRelationship({
        source: command.source,
        kind: command.relationship,
        target: command.target,
      });
    case "graph.traverse":
      return client.traverse(
        command.conceptId,
        command.relationship,
        command.maxHops,
      );
    case "procedure.define":
      return client.createProcedure(command.definition);
    case "procedure.list":
      return client.listProcedures();
    case "procedure.run": {
      const found = await client.getProcedureByName<{ id?: string }>(
        command.procedure,
      );
      const procedureId = found?.id ?? command.procedure;
      try {
        return await client.executeProcedure(
          procedureId,
          command.inputs,
          undefined,
          resolvedConfig.config.capabilities.permissionMode,
        );
      } catch (error) {
        const runtimePermission = runtimePermissionMessage(error);
        if (
          runtimePermission === undefined ||
          resolvedConfig.config.capabilities.permissionMode !== "ask"
        ) {
          throw error;
        }
        const decision = await askRuntimePermission(runtimePermission);
        if (!decision) {
          throw new Error(
            "Runtime capability request denied; the procedure was not executed.",
          );
        }
        return client.executeProcedure(
          procedureId,
          command.inputs,
          undefined,
          "full-access",
        );
      }
    }
    case "teach.run": {
      if (!resolvedConfig.config.teacher.enabled) {
        throw new Error(
          "Teaching is disabled by configuration; enable teacher.enabled first.",
        );
      }
      return runTeaching(
        client,
        command.instruction,
        createConfiguredTeacher(),
        {
          sessionId: command.session,
          recallMode: command.recall,
          permissionMode: command.permissionMode,
        },
        command.forceHeuristic,
      );
    }
    case "episode.list":
      return client.listEpisodes({ limit: command.limit });
    case "episode.get":
      return client.getEpisode(command.episodeId);
    case "failure.analyze":
      return client.analyzeFailure(
        command.request as unknown as FailureAnalysisInput,
      );
    case "failure.plan":
      return client.planAdaptation(
        command.request as unknown as AdaptationPlanInput,
      );
    case "failure.apply":
      return client.applyAdaptation({
        planId: command.planId,
        idempotencyKey: `cli:${command.planId}`,
        appliedAt: Math.floor(Date.now() / 1_000),
      });
    case "failure.apply-offline":
      return client.applyAdaptationOffline({
        planId: command.planId,
        idempotencyKey: `cli:offline:${command.planId}`,
        appliedAt: Math.floor(Date.now() / 1_000),
      });
    case "adaptation.show":
      return client.getAdaptation(command.planId);
    case "contradiction.list":
      return client.listContradictions();
    case "contradiction.get":
      return client.getContradiction(command.contradictionId);
    case "contradiction.record":
      return client.recordContradiction(
        command.request as unknown as RecordContradictionInput,
      );
    case "contradiction.refine":
      return client.refineContradiction(
        command.request as unknown as RefineContradictionInput,
      );
    case "contradiction.uncertainty":
      return client.getClaimUncertainty(command.claimId);
    case "primitive.observe":
      return client.observePrimitive(command.target);
    case "capability.provision-web-fetch":
      return client.provisionWebFetch(command.host);
    case "capability.list":
      return client.listCapabilities();
    case "session.start":
      return client.createSession(
        command.name,
        command.isolated ? "isolated" : "global",
      );
    case "session.list":
      return client.listSessions();
    case "session.show":
      return client.getSession(command.idOrName);
    case "session.end":
      return client.endSession(command.idOrName);
    case "chat.run":
      return runChat(client, command);
    case "cycle.run":
      return runCycle(
        client,
        command.situation,
        command.teacher === "off" ||
          (command.teacher !== "on" && !resolvedConfig.config.teacher.enabled)
          ? undefined
          : createConfiguredTeacher(),
        {
          sessionId: command.session,
          recallMode: command.recall,
          permissionMode: command.permissionMode,
        },
        createConfiguredInterpreter(),
      );
  }
}

async function main(): Promise<void> {
  loadProjectEnvironment();
  const resolvedConfig = await resolveConfig();
  applyConfigEnvironment(resolvedConfig);
  const command = parseCommand(process.argv.slice(2));
  if (command.kind === "config.path") {
    process.stdout.write(
      `${JSON.stringify(
        {
          cwd: resolvedConfig.cwd,
          home: resolvedConfig.homeDir,
          files: resolvedConfig.files,
        },
        null,
        2,
      )}\n`,
    );
    return;
  }
  if (command.kind === "config.show") {
    const output = redactedConfig(resolvedConfig);
    if (!command.withSources) delete output.sources;
    process.stdout.write(`${JSON.stringify(output, null, 2)}\n`);
    return;
  }
  if (command.kind === "config.validate") {
    process.stdout.write(
      `Configuration valid (${resolvedConfig.files.length} file layer${resolvedConfig.files.length === 1 ? "" : "s"}).\n`,
    );
    return;
  }
  if (command.kind === "config.set" || command.kind === "config.unset") {
    validateConfigMutation(
      command.layer,
      command.key,
      command.kind === "config.set" ? command.value : undefined,
      resolvedConfig,
    );
    const filePath = configLayerPath(command.layer, resolvedConfig);
    await setConfigValue(
      filePath,
      command.key,
      command.kind === "config.set" ? command.value : undefined,
      command.kind === "config.unset",
    );
    const refreshed = await resolveConfig();
    process.stdout.write(
      `${JSON.stringify(
        {
          changed: true,
          layer: command.layer,
          key: command.key,
          effective: redactedConfig(refreshed).effective,
          applies:
            command.key === "database.path" ? "next-launch" : "next-cycle",
        },
        null,
        2,
      )}\n`,
    );
    return;
  }
  if (command.kind === "benchmark.run") {
    const report = await runBenchmark(command.fixturePath, command.reportPath);
    process.stdout.write(`${renderBenchmarkReport(report)}\n`);
    return;
  }
  if (command.kind === "benchmark.report") {
    process.stdout.write(
      `${renderBenchmarkReport(await loadBenchmarkReport(command.reportPath))}\n`,
    );
    return;
  }
  if (command.kind === "cycle.run") {
    const adminResult = await tryHandleAdminRequest(
      command.situation,
      resolvedConfig,
    );
    if (adminResult !== null) {
      process.stdout.write(`${adminResult}\n`);
      return;
    }
  }
  const transport = StdioTransport.spawn(
    process.env.SPOON_SERVER ?? "target/debug/spoon-server",
  );
  const adminToken = adminTokenFromEnvironment();
  const client = new SpoonClient(
    transport,
    adminToken === undefined ? {} : { adminToken },
  );

  try {
    if (command.kind === "chat.run") {
      await execute(client, command, resolvedConfig);
      return;
    }
    const result = await execute(client, command, resolvedConfig);
    if (command.kind === "cycle.run" && command.explain) {
      process.stdout.write(`${formatExplanation(result)}\n`);
    } else if (command.kind === "cycle.run" && command.quiet) {
      process.stdout.write(`${formatQuietAnswer(result)}\n`);
    } else if (command.kind === "teach.run") {
      process.stdout.write(
        `${command.explain ? formatExplanation(result) : formatTeachingResult(result)}\n`,
      );
    } else {
      process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
    }
  } finally {
    client.close();
  }
}

function formatTeachingResult(result: unknown): string {
  if (!isRecord(result)) return `Procedure taught.\n${JSON.stringify(result)}`;
  const episode = isRecord(result.episode) ? result.episode : {};
  const answer = formatValue(result.answer);
  const procedure = result.procedureIr;
  const action = stringValue(episode.action);
  const deferredEffect =
    action === "teacher-procedure-installed:awaiting-runtime-authorization";
  return [
    deferredEffect
      ? "Effectful procedure taught and installed. It was not run while being taught."
      : "Procedure taught and installed.",
    ...(deferredEffect ? [] : [`Example result: ${answer}`]),
    `Episode: ${stringValue(episode.id) ?? "unknown"}`,
    "Procedure IR:",
    JSON.stringify(procedure, null, 2),
  ].join("\n");
}

async function runChat(
  client: SpoonClient,
  command: Extract<Command, { kind: "chat.run" }>,
): Promise<void> {
  let session =
    command.session === undefined
      ? null
      : await client.getSession(command.session);
  if (session === null) {
    session = await client.createSession(
      command.session,
      command.isolated ? "isolated" : "global",
    );
  } else if (command.isolated && session.visibility !== "isolated") {
    throw new Error("an existing global session cannot be changed to isolated");
  }
  const reader = createInterface({ input, output });
  output.write(
    `Spoon chat — session ${session.name ?? session.id}${session.visibility === "isolated" ? " [ISOLATED]" : ""}\n`,
  );
  try {
    while (true) {
      const question = (await reader.question("spoon> ")).trim();
      if (question === "" || question === ":quit" || question === ":exit")
        break;
      try {
        const currentConfig = await resolveConfig();
        if (question === ":teach" || question.startsWith(":teach ")) {
          if (!currentConfig.config.teacher.enabled) {
            throw new Error(
              "Teaching is disabled by configuration; enable teacher.enabled first.",
            );
          }
          const instruction = question.slice(":teach".length).trim();
          if (instruction === "") {
            output.write(
              "Usage: :teach <what to teach> (the request is shown to the Teacher and validated before admission)\n",
            );
            continue;
          }
          const result = await runTeaching(
            client,
            instruction,
            createConfiguredTeacher(),
            {
              sessionId: session.id,
              recallMode: command.recall,
              permissionMode: command.permissionMode,
            },
          );
          output.write(`${formatTeachingResult(result)}\n`);
          continue;
        }
        const adminResult = await tryHandleAdminRequest(
          question,
          currentConfig,
        );
        if (adminResult !== null) {
          applyConfigEnvironment(currentConfig);
          output.write(`${adminResult}\n`);
          continue;
        }
        const result = await runCycle(
          client,
          question,
          currentConfig.config.teacher.enabled
            ? createConfiguredTeacher()
            : undefined,
          {
            sessionId: session.id,
            recallMode: command.recall,
            permissionMode: command.permissionMode,
          },
          createConfiguredInterpreter(),
        );
        output.write(`${formatQuietAnswer(result)}\n`);
      } catch (error) {
        output.write(
          `Error: ${error instanceof Error ? error.message : String(error)}\n`,
        );
      }
    }
  } finally {
    reader.close();
  }
}

function formatQuietAnswer(result: unknown): string {
  if (!isRecord(result)) return JSON.stringify(result);
  const answer = result.answer;
  if (answer !== null && answer !== undefined) {
    return typeof answer === "string" ? answer : JSON.stringify(answer);
  }
  const disposition = result.disposition;
  return typeof disposition === "string"
    ? `No direct answer (${disposition}).`
    : "No direct answer.";
}

export function formatExplanation(result: unknown): string {
  if (!isRecord(result)) return `Result: ${JSON.stringify(result)}`;
  const episode = isRecord(result.episode) ? result.episode : {};
  const lines = [
    `Request: ${stringValue(episode.situation) ?? "unknown"}`,
    `Outcome: ${stringValue(result.disposition) ?? "unknown"}`,
    `Answer: ${formatValue(result.answer)}`,
    `Episode: ${stringValue(episode.id) ?? "unknown"}`,
  ];
  if (result.procedureIr !== undefined) {
    lines.push("Procedure IR:");
    lines.push(JSON.stringify(result.procedureIr, null, 2));
  }
  const context = isRecord(episode.context) ? episode.context : {};
  const considered = Array.isArray(episode.knowledge_considered)
    ? episode.knowledge_considered.length
    : Array.isArray(episode.knowledgeConsidered)
      ? episode.knowledgeConsidered.length
      : 0;
  lines.push(
    `Context: ${considered} knowledge candidates considered, ${
      Array.isArray(context.assumptions) ? context.assumptions.length : 0
    } assumptions`,
  );
  const interaction = episode.teacher_interaction ?? episode.teacherInteraction;
  const languageInterpreter = findLanguageInterpreter(interaction);
  if (languageInterpreter) {
    const provenance = isRecord(languageInterpreter.provenance)
      ? languageInterpreter.provenance
      : {};
    const frames = isRecord(languageInterpreter.frames)
      ? languageInterpreter.frames
      : undefined;
    const request = isRecord(languageInterpreter.request)
      ? languageInterpreter.request
      : undefined;
    const requestContext =
      request && isRecord(request.context) ? request.context : {};
    lines.push(
      `Interpreter: ${stringValue(provenance.provider) ?? (stringValue(languageInterpreter.providerError) ? "attempted" : "unknown")}` +
        (stringValue(provenance.model)
          ? ` (${stringValue(provenance.model)})`
          : ""),
    );
    if (stringValue(languageInterpreter.source)) {
      lines.push(
        `Interpreter source: ${stringValue(languageInterpreter.source)}`,
      );
    }
    if (frames) {
      lines.push(
        `Interpreter decision: ${stringValue(frames.disposition) ?? "unknown"}`,
      );
    }
    lines.push(
      `Interpreter context: ${Array.isArray(requestContext.candidates) ? requestContext.candidates.length : 0} candidates, ${Array.isArray(requestContext.priorTurns) ? requestContext.priorTurns.length : 0} prior turns`,
    );
    if (stringValue(languageInterpreter.providerError)) {
      lines.push(
        `Interpreter fallback: ${stringValue(languageInterpreter.providerError)}`,
      );
    }
  } else {
    lines.push("Interpreter: not used");
  }

  const teacher = isRecord(interaction) ? interaction : undefined;
  const hasTeacher =
    teacher !== undefined &&
    (isRecord(teacher.proposal) ||
      isRecord(teacher.content) ||
      (teacher.request !== undefined && languageInterpreter === undefined));
  if (teacher && hasTeacher) {
    const proposal = isRecord(teacher.proposal) ? teacher.proposal : undefined;
    const proposalProvenance =
      proposal && isRecord(proposal.provenance)
        ? proposal.provenance
        : undefined;
    const provenance = isRecord(teacher.provenance)
      ? teacher.provenance
      : (proposalProvenance ?? teacher);
    const content =
      proposal && isRecord(proposal.content)
        ? proposal.content
        : isRecord(teacher.content)
          ? teacher.content
          : undefined;
    const proposalKind =
      content && stringValue(content.proposalKind ?? content.proposal_kind);
    const validation =
      proposal && proposal.validation !== undefined
        ? proposal.validation
        : teacher.validation;
    lines.push(
      `Teacher: ${stringValue(provenance.provider) ?? "unknown"}` +
        (stringValue(provenance.model)
          ? ` (${stringValue(provenance.model)})`
          : ""),
    );
    if (stringValue(teacher.source) || stringValue(provenance.teacher)) {
      lines.push(
        `Teacher source: ${stringValue(teacher.source) ?? stringValue(provenance.teacher)}`,
      );
    }
    if (proposalKind) lines.push(`Teacher proposal: ${proposalKind}`);
    if (content?.answer !== undefined && content?.answer !== null) {
      lines.push(`Teacher answer: ${formatValue(content.answer)}`);
    }
    if (validation !== undefined)
      lines.push(`Teacher validation: ${formatValue(validation)}`);
  } else {
    lines.push("Teacher: not used");
  }
  const evaluation = episode.evaluation;
  if (isRecord(evaluation)) {
    lines.push(
      `Evaluation: ${stringValue(evaluation.tier) ?? "unknown"} — ${
        evaluation.success === true
          ? "success"
          : evaluation.success === false
            ? "failure"
            : "unresolved"
      }`,
    );
    if (stringValue(evaluation.details))
      lines.push(`Why: ${stringValue(evaluation.details)}`);
  }
  const action = stringValue(episode.action);
  const teacherObservation = action === "teacher-observation:provisional";
  const observed = episode.observed_result ?? episode.observedResult;
  const prediction = episode.prediction;
  if (prediction !== undefined || observed !== undefined) {
    lines.push(`Predicted: ${formatValue(prediction)}`);
    lines.push(`Observed: ${formatValue(observed)}`);
    if (teacherObservation && observed === undefined) {
      lines.push(
        "Verification: provisional — the prediction came from the teacher; no independent observation exists",
      );
    } else if (observed === undefined) {
      lines.push("Verification: no independent observed result was recorded");
    }
  }
  if (teacherObservation) {
    lines.push(
      "Answer source: teacher-provided external observation; no trusted local observation ran",
    );
    lines.push(
      "Learning: none — no reusable lesson or procedure was proposed or admitted",
    );
  } else {
    lines.push(
      `Learned/reused: ${action && action !== "answer-only" ? action : "no reusable procedure"}`,
    );
  }
  const cost = isRecord(episode.cost) ? episode.cost : {};
  lines.push(
    `Cost: rung ${stringValue(cost.rung_reached ?? cost.rungReached) ?? "unknown"}, ${stringValue(cost.steps_taken ?? cost.stepsTaken) ?? "?"} steps`,
  );
  return lines.join("\n");
}

/**
 * Teacher handoff records the interpreter failure under `priorFailure` so the
 * complete chain is durable. Walk that bounded wrapper when rendering an
 * explanation; otherwise a real attempted interpreter is misreported as
 * "not used".
 */
function findLanguageInterpreter(
  value: unknown,
  depth = 0,
): Record<string, unknown> | undefined {
  if (!isRecord(value) || depth > 4) return undefined;
  if (isRecord(value.languageInterpreter)) return value.languageInterpreter;
  for (const key of ["priorFailure", "rejectedTeacherInteraction"]) {
    const found = findLanguageInterpreter(value[key], depth + 1);
    if (found !== undefined) return found;
  }
  return undefined;
}

function formatValue(value: unknown): string {
  if (value === null || value === undefined) return "none";
  return typeof value === "string" ? value : JSON.stringify(value);
}

function stringValue(value: unknown): string | undefined {
  return typeof value === "string" || typeof value === "number"
    ? String(value)
    : undefined;
}

function runtimePermissionMessage(error: unknown): string | undefined {
  if (!(error instanceof JsonRpcError) || !isRecord(error.data))
    return undefined;
  const cause = stringValue(error.data.cause);
  return cause?.startsWith("runtime permission required for native capability")
    ? cause
    : undefined;
}

async function askRuntimePermission(message: string): Promise<boolean> {
  if (!input.isTTY || !output.isTTY) {
    throw new Error(
      `${message}. Re-run interactively or set SPOON_PERMISSION_MODE=full-access.`,
    );
  }
  const terminal = createInterface({ input, output });
  try {
    const answer = await terminal.question(
      `${message}\nAllow this one operation? [y/N] `,
    );
    return /^(y|yes)$/i.test(answer.trim());
  } finally {
    terminal.close();
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

if (
  process.argv[1] !== undefined &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  main().catch((error: unknown) => {
    const cause =
      error instanceof JsonRpcError && isRecord(error.data)
        ? stringValue(error.data.cause)
        : undefined;
    process.stderr.write(
      `${cause ?? (error instanceof Error ? error.message : String(error))}\n`,
    );
    process.exitCode = 1;
  });
}
