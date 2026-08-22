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
    process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
  } finally {
    client.close();
  }
}

main().catch((error: unknown) => {
  process.stderr.write(
    `${error instanceof Error ? error.message : String(error)}\n`,
  );
  process.exitCode = 1;
});
