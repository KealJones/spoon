import type { JsonValue } from "@ekg/sdk";

export type Command =
  | { kind: "concept.add"; name: string }
  | { kind: "concept.list" }
  | {
      kind: "relationship.add";
      source: string;
      relationship: string;
      target: string;
    }
  | {
      kind: "graph.traverse";
      conceptId: string;
      relationship: string;
      maxHops: number;
    }
  | { kind: "procedure.define"; definition: Record<string, JsonValue> }
  | { kind: "procedure.list" }
  | {
      kind: "procedure.run";
      procedure: string;
      inputs: Record<string, JsonValue>;
    }
  | { kind: "episode.list"; limit: number }
  | { kind: "episode.get"; episodeId: string }
  | { kind: "failure.analyze"; request: Record<string, JsonValue> }
  | { kind: "failure.plan"; request: Record<string, JsonValue> }
  | { kind: "failure.apply"; planId: string }
  | { kind: "failure.apply-offline"; planId: string }
  | { kind: "adaptation.show"; planId: string }
  | { kind: "contradiction.list" }
  | { kind: "contradiction.get"; contradictionId: number }
  | { kind: "contradiction.record"; request: Record<string, JsonValue> }
  | { kind: "contradiction.refine"; request: Record<string, JsonValue> }
  | { kind: "contradiction.uncertainty"; claimId: string }
  | { kind: "primitive.observe"; target: string }
  | { kind: "cycle.run"; situation: string; quiet: boolean };

const usage = `Usage:
  ekg concept add <name>
  ekg concept list
  ekg relationship add <source-id> <kind> <target-id>
  ekg graph traverse <concept-id> <kind> [max-hops]
  ekg procedure define '<json>'
  ekg procedure list
  ekg procedure run <name-or-id> [key=json-value ...]
  ekg episode list [limit]
  ekg episode get <episode-id>
  ekg failure analyze '<json>'
  ekg failure plan '<json>'
  ekg failure apply <plan-id>
  ekg failure apply-offline <plan-id>
  ekg adaptation show <plan-id>
  ekg contradiction list
  ekg contradiction get <id>
  ekg contradiction record '<json>'
  ekg contradiction refine '<json>'
  ekg contradiction uncertainty <claim-id>
  ekg primitive observe <target>
  ekg ask [--quiet|-q] <situation>`;

export function parseCommand(args: string[]): Command {
  const [resource, action, ...rest] = args;

  if (resource === "concept" && action === "add" && rest[0]) {
    return { kind: "concept.add", name: rest[0] };
  }
  if (resource === "concept" && action === "list" && rest.length === 0) {
    return { kind: "concept.list" };
  }
  if (resource === "relationship" && action === "add" && rest.length === 3) {
    return {
      kind: "relationship.add",
      source: rest[0]!,
      relationship: rest[1]!,
      target: rest[2]!,
    };
  }
  if (
    resource === "graph" &&
    action === "traverse" &&
    rest.length >= 2 &&
    rest.length <= 3
  ) {
    const maxHops = rest[2] === undefined ? 1 : Number.parseInt(rest[2], 10);
    if (Number.isInteger(maxHops) && maxHops >= 0) {
      return {
        kind: "graph.traverse",
        conceptId: rest[0]!,
        relationship: rest[1]!,
        maxHops,
      };
    }
  }
  if (resource === "procedure" && action === "define" && rest.length === 1) {
    return { kind: "procedure.define", definition: parseObject(rest[0]!) };
  }
  if (resource === "procedure" && action === "list" && rest.length === 0) {
    return { kind: "procedure.list" };
  }
  if (resource === "procedure" && action === "run" && rest[0]) {
    return {
      kind: "procedure.run",
      procedure: rest[0],
      inputs: Object.fromEntries(rest.slice(1).map(parseBinding)),
    };
  }
  if (resource === "episode" && action === "list" && rest.length <= 1) {
    const limit = rest[0] === undefined ? 20 : Number.parseInt(rest[0], 10);
    if (Number.isInteger(limit) && limit > 0)
      return { kind: "episode.list", limit };
  }
  if (resource === "episode" && action === "get" && rest.length === 1) {
    return { kind: "episode.get", episodeId: rest[0]! };
  }
  if (
    resource === "failure" &&
    (action === "analyze" || action === "plan") &&
    rest.length === 1
  ) {
    return {
      kind: action === "analyze" ? "failure.analyze" : "failure.plan",
      request: parseObject(rest[0]!),
    };
  }
  if (
    resource === "failure" &&
    (action === "apply" || action === "apply-offline") &&
    rest.length === 1
  ) {
    return {
      kind: action === "apply" ? "failure.apply" : "failure.apply-offline",
      planId: rest[0]!,
    };
  }
  if (resource === "adaptation" && action === "show" && rest.length === 1) {
    return { kind: "adaptation.show", planId: rest[0]! };
  }
  if (resource === "contradiction" && action === "list" && rest.length === 0) {
    return { kind: "contradiction.list" };
  }
  if (resource === "contradiction" && action === "get" && rest.length === 1) {
    const contradictionId = Number.parseInt(rest[0]!, 10);
    if (Number.isSafeInteger(contradictionId) && contradictionId > 0) {
      return { kind: "contradiction.get", contradictionId };
    }
  }
  if (
    resource === "contradiction" &&
    (action === "record" || action === "refine") &&
    rest.length === 1
  ) {
    return {
      kind:
        action === "record" ? "contradiction.record" : "contradiction.refine",
      request: parseObject(rest[0]!),
    };
  }
  if (
    resource === "contradiction" &&
    action === "uncertainty" &&
    rest.length === 1
  ) {
    return { kind: "contradiction.uncertainty", claimId: rest[0]! };
  }
  if (resource === "primitive" && action === "observe" && rest.length === 1) {
    return { kind: "primitive.observe", target: rest[0]! };
  }
  if (resource === "ask" && action) {
    const leadingQuiet = action === "--quiet" || action === "-q";
    const trailingQuiet = rest.at(-1) === "--quiet" || rest.at(-1) === "-q";
    const quiet = leadingQuiet || trailingQuiet;
    const question = leadingQuiet
      ? rest
      : trailingQuiet
        ? [action, ...rest.slice(0, -1)]
        : [action, ...rest];
    if (question.length === 0) throw new Error(usage);
    return { kind: "cycle.run", situation: question.join(" "), quiet };
  }

  throw new Error(usage);
}

function parseBinding(binding: string): [string, JsonValue] {
  const separator = binding.indexOf("=");
  if (separator <= 0)
    throw new Error(`Invalid input binding '${binding}'.\n${usage}`);
  return [binding.slice(0, separator), parseJson(binding.slice(separator + 1))];
}

function parseObject(value: string): Record<string, JsonValue> {
  const parsed = parseJson(value);
  if (parsed === null || Array.isArray(parsed) || typeof parsed !== "object") {
    throw new Error("Procedure definition must be a JSON object");
  }
  return parsed;
}

function parseJson(value: string): JsonValue {
  try {
    return JSON.parse(value) as JsonValue;
  } catch {
    return value;
  }
}
