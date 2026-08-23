import type { JsonValue } from "@spoon/sdk";
import type { PermissionMode, RecallMode } from "./config.js";

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
  | { kind: "config.path" }
  | { kind: "config.show"; withSources: boolean }
  | { kind: "config.validate" }
  | {
      kind: "config.set";
      key: string;
      value: JsonValue;
      layer: "user" | "project" | "local";
    }
  | { kind: "config.unset"; key: string; layer: "user" | "project" | "local" }
  | { kind: "session.start"; name?: string; isolated: boolean }
  | { kind: "session.list" }
  | { kind: "session.show"; idOrName: string }
  | { kind: "session.end"; idOrName: string }
  | {
      kind: "cycle.run";
      situation: string;
      quiet: boolean;
      explain: boolean;
      teacher?: "on" | "off";
      session?: string;
      recall?: RecallMode;
      permissionMode?: PermissionMode;
    }
  | {
      kind: "chat.run";
      session?: string;
      isolated: boolean;
      recall?: RecallMode;
      permissionMode?: PermissionMode;
    }
  | { kind: "benchmark.run"; fixturePath: string; reportPath?: string }
  | { kind: "benchmark.report"; reportPath: string };

const usage = `Usage:
  spoon concept add <name>
  spoon concept list
  spoon relationship add <source-id> <kind> <target-id>
  spoon graph traverse <concept-id> <kind> [max-hops]
  spoon procedure define '<json>'
  spoon procedure list
  spoon procedure run <name-or-id> [key=json-value ...]
  spoon episode list [limit]
  spoon episode get <episode-id>
  spoon failure analyze '<json>'
  spoon failure plan '<json>'
  spoon failure apply <plan-id>
  spoon failure apply-offline <plan-id>
  spoon adaptation show <plan-id>
  spoon contradiction list
  spoon contradiction get <id>
  spoon contradiction record '<json>'
  spoon contradiction refine '<json>'
  spoon contradiction uncertainty <claim-id>
  spoon primitive observe <target>
  spoon config path|show [--sources]|validate
  spoon config set <key> <json-value> [--layer user|project|local]
  spoon config unset <key> [--layer user|project|local]
  spoon session start [--name <name>] [--isolated]
  spoon session list|show <id-or-name>|end <id-or-name>
  spoon ask [--quiet|-q|--explain] [--teacher on|off] [--session <id>] [--recall global|session|none] [--permission-mode ask|workspace|full-access] <situation>
  spoon chat [--session <id>] [--isolated] [--recall global|session|none] [--permission-mode ask|workspace|full-access]
  spoon benchmark run <fixture.json> [report.json]
  spoon benchmark report <report.json>`;

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
  if (resource === "config" && action === "path" && rest.length === 0) {
    return { kind: "config.path" };
  }
  if (resource === "config" && action === "show") {
    if (rest.length === 0) return { kind: "config.show", withSources: false };
    if (rest.length === 1 && rest[0] === "--sources")
      return { kind: "config.show", withSources: true };
  }
  if (resource === "config" && action === "validate" && rest.length === 0) {
    return { kind: "config.validate" };
  }
  if (resource === "config" && (action === "set" || action === "unset")) {
    const key = rest[0];
    if (!key) throw new Error(usage);
    let layer: "user" | "project" | "local" = "project";
    const layerIndex = rest.indexOf("--layer");
    if (layerIndex >= 0) {
      const value = rest[layerIndex + 1];
      if (value !== "user" && value !== "project" && value !== "local")
        throw new Error(usage);
      layer = value;
    }
    if (action === "set") {
      const rawValue = rest[1];
      if (!rawValue) throw new Error(usage);
      return { kind: "config.set", key, value: parseJson(rawValue), layer };
    }
    return { kind: "config.unset", key, layer };
  }
  if (resource === "session" && action === "list" && rest.length === 0) {
    return { kind: "session.list" };
  }
  if (resource === "session" && action === "show" && rest.length === 1) {
    return { kind: "session.show", idOrName: rest[0]! };
  }
  if (resource === "session" && action === "end" && rest.length === 1) {
    return { kind: "session.end", idOrName: rest[0]! };
  }
  if (resource === "session" && action === "start") {
    let name: string | undefined;
    let isolated = false;
    for (let index = 0; index < rest.length; index += 1) {
      const argument = rest[index]!;
      if (argument === "--isolated") isolated = true;
      else if (argument === "--name") name = rest[++index];
      else if (argument.startsWith("--name="))
        name = argument.slice("--name=".length);
      else throw new Error(usage);
    }
    return {
      kind: "session.start",
      ...(name === undefined ? {} : { name }),
      isolated,
    };
  }
  if (resource === "chat") {
    let session: string | undefined;
    let isolated = false;
    let recall: RecallMode | undefined;
    let permissionMode: PermissionMode | undefined;
    for (
      let index = 0;
      index < (action === undefined ? 0 : [action, ...rest].length);
      index += 1
    ) {
      const argument = [action, ...rest][index];
      if (argument === undefined) continue;
      if (argument === "--isolated") isolated = true;
      else if (argument === "--session") session = [action, ...rest][++index];
      else if (argument === "--recall")
        recall = parseRecall([action, ...rest][++index]);
      else if (argument === "--permission-mode")
        permissionMode = parsePermissionMode([action, ...rest][++index]);
      else throw new Error(usage);
    }
    return {
      kind: "chat.run",
      ...(session === undefined ? {} : { session }),
      isolated,
      ...(recall === undefined ? {} : { recall }),
      ...(permissionMode === undefined ? {} : { permissionMode }),
    };
  }
  if (resource === "ask" && action) {
    const askArgs = [action, ...rest];
    let quiet = false;
    let explain = false;
    let teacher: "on" | "off" | undefined;
    let session: string | undefined;
    let recall: RecallMode | undefined;
    let permissionMode: PermissionMode | undefined;
    const question: string[] = [];
    for (let index = 0; index < askArgs.length; index += 1) {
      const argument = askArgs[index]!;
      if (argument === "--quiet" || argument === "-q") {
        quiet = true;
      } else if (argument === "--explain") {
        explain = true;
      } else if (argument === "--teacher") {
        const value = askArgs[++index];
        if (value !== "on" && value !== "off") throw new Error(usage);
        teacher = value;
      } else if (argument.startsWith("--teacher=")) {
        const value = argument.slice("--teacher=".length);
        if (value !== "on" && value !== "off") throw new Error(usage);
        teacher = value;
      } else if (argument === "--session") {
        session = askArgs[++index];
        if (!session) throw new Error(usage);
      } else if (argument.startsWith("--session=")) {
        session = argument.slice("--session=".length);
        if (!session) throw new Error(usage);
      } else if (argument === "--recall") {
        recall = parseRecall(askArgs[++index]);
      } else if (argument.startsWith("--recall=")) {
        recall = parseRecall(argument.slice("--recall=".length));
      } else if (argument === "--permission-mode") {
        permissionMode = parsePermissionMode(askArgs[++index]);
      } else if (argument.startsWith("--permission-mode=")) {
        permissionMode = parsePermissionMode(
          argument.slice("--permission-mode=".length),
        );
      } else {
        question.push(argument);
      }
    }
    if (question.length === 0) throw new Error(usage);
    return {
      kind: "cycle.run",
      situation: question.join(" "),
      quiet,
      explain,
      ...(teacher === undefined ? {} : { teacher }),
      ...(session === undefined ? {} : { session }),
      ...(recall === undefined ? {} : { recall }),
      ...(permissionMode === undefined ? {} : { permissionMode }),
    };
  }
  if (resource === "benchmark" && action === "run") {
    if (rest.length < 1 || rest.length > 2) throw new Error(usage);
    return {
      kind: "benchmark.run",
      fixturePath: rest[0]!,
      ...(rest[1] === undefined ? {} : { reportPath: rest[1] }),
    };
  }
  if (resource === "benchmark" && action === "report" && rest.length === 1) {
    return { kind: "benchmark.report", reportPath: rest[0]! };
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

function parseRecall(value: string | undefined): RecallMode {
  if (value === "global" || value === "session" || value === "none")
    return value;
  throw new Error(usage);
}

function parsePermissionMode(value: string | undefined): PermissionMode {
  if (value === "ask" || value === "workspace" || value === "full-access")
    return value;
  throw new Error(usage);
}
