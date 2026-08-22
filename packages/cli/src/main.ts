#!/usr/bin/env node

import { EkgClient, StdioTransport } from "@ekg/sdk";

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
  }
}

async function main(): Promise<void> {
  const command = parseCommand(process.argv.slice(2));
  const transport = StdioTransport.spawn(
    process.env.EKG_SERVER ?? "target/debug/ekg-server",
  );
  const client = new EkgClient(transport);

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
