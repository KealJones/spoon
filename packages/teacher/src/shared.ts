import { createHash, randomUUID } from "node:crypto";

import { TeacherError } from "./errors.js";
import type {
  Clock,
  IdFactory,
  JsonValue,
  ProposalProvenance,
  ProviderKind,
  TeacherProposal,
  TeacherRequest,
} from "./types.js";

export const defaultClock: Clock = () => new Date();
export const defaultIdFactory: IdFactory = () => randomUUID();

export function parseJsonContent(provider: string, input: string): JsonValue {
  try {
    const content: unknown = JSON.parse(input);
    if (!isJsonValue(content)) throw new Error("value is not JSON-compatible");
    return content;
  } catch (error) {
    throw new TeacherError(provider, "teacher output was not valid JSON", {
      cause: error,
    });
  }
}

export function isJsonValue(value: unknown): value is JsonValue {
  if (
    value === null ||
    typeof value === "string" ||
    typeof value === "boolean" ||
    (typeof value === "number" && Number.isFinite(value))
  ) {
    return true;
  }
  if (Array.isArray(value)) return value.every(isJsonValue);
  if (typeof value !== "object") return false;
  const prototype = Object.getPrototypeOf(value) as unknown;
  if (prototype !== Object.prototype && prototype !== null) return false;
  return Object.values(value).every(isJsonValue);
}

export async function atProviderBoundary<T>(
  provider: ProviderKind,
  message: string,
  operation: () => Promise<T>,
): Promise<T> {
  try {
    return await operation();
  } catch (error) {
    if (error instanceof TeacherError) throw error;
    throw new TeacherError(provider, message, { cause: error });
  }
}

export function fingerprintTeacherRequest(request: TeacherRequest): string {
  const input: Record<string, unknown> = {
    situation: request.situation,
    context: request.context,
    desiredOutput: request.desiredOutput,
  };
  if (request.specificQuestion !== undefined) {
    input.specificQuestion = request.specificQuestion;
  }
  return `sha256:${createHash("sha256").update(canonicalJson(input)).digest("hex")}`;
}

function canonicalJson(value: unknown): string {
  if (value === null || typeof value !== "object") {
    const encoded = JSON.stringify(value);
    if (encoded === undefined)
      throw new TypeError("Value is not JSON-compatible");
    return encoded;
  }
  if (Array.isArray(value)) {
    return `[${value.map((item) => canonicalJson(item)).join(",")}]`;
  }
  const record = value as Record<string, unknown>;
  const entries = Object.keys(record)
    .filter((key) => record[key] !== undefined)
    .sort()
    .map((key) => `${JSON.stringify(key)}:${canonicalJson(record[key])}`);
  return `{${entries.join(",")}}`;
}

export function makeProposal(options: {
  content: JsonValue;
  provider: ProviderKind;
  source: string;
  model?: string;
  request: TeacherRequest;
  requestId: string;
  providerRequestId?: string;
  generatedAt: Date;
}): TeacherProposal {
  const provenance: ProposalProvenance = {
    provider: options.provider,
    teacher: options.source,
    requestId: options.requestId,
    requestHash: fingerprintTeacherRequest(options.request),
    generatedAt: options.generatedAt.toISOString(),
    situation: options.request.situation,
  };
  if (options.model !== undefined) provenance.model = options.model;
  if (options.providerRequestId !== undefined) {
    provenance.providerRequestId = options.providerRequestId;
  }
  if (options.request.specificQuestion !== undefined) {
    provenance.specificQuestion = options.request.specificQuestion;
  }

  return {
    content: options.content,
    source: options.source,
    status: "unverified",
    provenance,
  };
}
