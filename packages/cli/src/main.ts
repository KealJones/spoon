#!/usr/bin/env node

import {
  EkgClient,
  StdioTransport,
  type AdaptationPlanInput,
  type FailureAnalysisInput,
  type RecordContradictionInput,
  type RefineContradictionInput,
} from "@ekg/sdk";

import { createConfiguredTeacher, runCycle } from "./cycle.js";
import {
  adminTokenFromEnvironment,
  loadProjectEnvironment,
} from "./environment.js";
import { parseCommand, type Command } from "./parse.js";

async function execute(client: EkgClient, command: Command): Promise<unknown> {
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
      return client.executeProcedure(
        found?.id ?? command.procedure,
        command.inputs,
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
    case "cycle.run":
      return runCycle(client, command.situation, createConfiguredTeacher());
  }
}

async function main(): Promise<void> {
  loadProjectEnvironment();
  const command = parseCommand(process.argv.slice(2));
  const transport = StdioTransport.spawn(
    process.env.EKG_SERVER ?? "target/debug/ekg-server",
  );
  const adminToken = adminTokenFromEnvironment();
  const client = new EkgClient(
    transport,
    adminToken === undefined ? {} : { adminToken },
  );

  try {
    const result = await execute(client, command);
    if (command.kind === "cycle.run" && command.explain) {
      process.stdout.write(`${formatExplanation(result)}\n`);
    } else if (command.kind === "cycle.run" && command.quiet) {
      process.stdout.write(`${formatQuietAnswer(result)}\n`);
    } else {
      process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
    }
  } finally {
    client.close();
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

function formatExplanation(result: unknown): string {
  if (!isRecord(result)) return `Result: ${JSON.stringify(result)}`;
  const episode = isRecord(result.episode) ? result.episode : {};
  const lines = [
    `Request: ${stringValue(episode.situation) ?? "unknown"}`,
    `Outcome: ${stringValue(result.disposition) ?? "unknown"}`,
    `Answer: ${formatValue(result.answer)}`,
    `Episode: ${stringValue(episode.id) ?? "unknown"}`,
  ];
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
  const teacher = episode.teacher_interaction ?? episode.teacherInteraction;
  if (isRecord(teacher)) {
    const proposal = isRecord(teacher.proposal) ? teacher.proposal : undefined;
    const proposalProvenance = proposal && isRecord(proposal.provenance)
      ? proposal.provenance
      : undefined;
    const provenance = isRecord(teacher.provenance)
      ? teacher.provenance
      : proposalProvenance ?? teacher;
    const content = proposal && isRecord(proposal.content)
      ? proposal.content
      : isRecord(teacher.content)
        ? teacher.content
        : undefined;
    const proposalKind = content && stringValue(content.proposalKind ?? content.proposal_kind);
    const validation = proposal && proposal.validation !== undefined
      ? proposal.validation
      : teacher.validation;
    lines.push(
      `Teacher: ${stringValue(provenance.provider) ?? "unknown"}` +
        (stringValue(provenance.model) ? ` (${stringValue(provenance.model)})` : ""),
    );
    if (stringValue(teacher.source) || stringValue(provenance.teacher)) {
      lines.push(`Teacher source: ${stringValue(teacher.source) ?? stringValue(provenance.teacher)}`);
    }
    if (proposalKind) lines.push(`Teacher proposal: ${proposalKind}`);
    if (content?.answer !== undefined && content?.answer !== null) {
      lines.push(`Teacher answer: ${formatValue(content.answer)}`);
    }
    if (validation !== undefined) lines.push(`Teacher validation: ${formatValue(validation)}`);
  } else {
    lines.push("Teacher: not used");
  }
  const evaluation = episode.evaluation;
  if (isRecord(evaluation)) {
    lines.push(
      `Evaluation: ${stringValue(evaluation.tier) ?? "unknown"} — ${
        evaluation.success === true ? "success" : evaluation.success === false ? "failure" : "unresolved"
      }`,
    );
    if (stringValue(evaluation.details)) lines.push(`Why: ${stringValue(evaluation.details)}`);
  }
  const action = stringValue(episode.action);
  const teacherObservation = action === "teacher-observation:provisional";
  const observed = episode.observed_result ?? episode.observedResult;
  const prediction = episode.prediction;
  if (prediction !== undefined || observed !== undefined) {
    lines.push(`Predicted: ${formatValue(prediction)}`);
    lines.push(`Observed: ${formatValue(observed)}`);
    if (teacherObservation && observed === undefined) {
      lines.push("Verification: provisional — the prediction came from the teacher; no independent observation exists");
    } else if (observed === undefined) {
      lines.push("Verification: no independent observed result was recorded");
    }
  }
  if (teacherObservation) {
    lines.push("Answer source: teacher-provided external observation; no trusted local observation ran");
    lines.push("Learning: none — no reusable lesson or procedure was proposed or admitted");
  } else {
    lines.push(`Learned/reused: ${action && action !== "answer-only" ? action : "no reusable procedure"}`);
  }
  const cost = isRecord(episode.cost) ? episode.cost : {};
  lines.push(`Cost: rung ${stringValue(cost.rung_reached ?? cost.rungReached) ?? "unknown"}, ${stringValue(cost.steps_taken ?? cost.stepsTaken) ?? "?"} steps`);
  return lines.join("\n");
}

function formatValue(value: unknown): string {
  if (value === null || value === undefined) return "none";
  return typeof value === "string" ? value : JSON.stringify(value);
}

function stringValue(value: unknown): string | undefined {
  return typeof value === "string" || typeof value === "number" ? String(value) : undefined;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

main().catch((error: unknown) => {
  process.stderr.write(
    `${error instanceof Error ? error.message : String(error)}\n`,
  );
  process.exitCode = 1;
});
