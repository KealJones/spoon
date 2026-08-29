import { createHash, randomUUID } from "node:crypto";

import { LanguageInterpreterError, OllamaLanguageInterpreterError } from "./errors.js";
import type {
  Clock,
  EngineRequest,
  IdFactory,
  IntentFetch,
  IntentFrameProposal,
  IntentProvider,
  IntentSlotProposal,
  InterpretationProposal,
  JsonValue,
  LanguageInterpreter,
  IntentProposalWire,
  TokenRange,
} from "./types.js";

export interface OllamaLanguageInterpreterOptions {
  model?: string;
  baseUrl?: string;
  fetch?: IntentFetch;
  now?: Clock;
  idFactory?: IdFactory;
}

interface OllamaGenerateResponse {
  response?: unknown;
  thinking?: unknown;
  error?: unknown;
}

const DEFAULT_MODEL = "qwen2.5:0.5b";
const DEFAULT_BASE_URL = "http://localhost:11434";

export class OllamaLanguageInterpreter implements LanguageInterpreter {
  readonly #model: string;
  readonly #baseUrl: string;
  readonly #fetch: IntentFetch;
  readonly #now: Clock;
  readonly #idFactory: IdFactory;
  readonly #source: string;

  constructor(options: OllamaLanguageInterpreterOptions = {}) {
    this.#model = options.model ?? DEFAULT_MODEL;
    this.#baseUrl = (options.baseUrl ?? DEFAULT_BASE_URL).replace(/\/$/, "");
    this.#fetch = options.fetch ?? globalThis.fetch;
    this.#now = options.now ?? (() => new Date());
    this.#idFactory = options.idFactory ?? randomUUID;
    this.#source = `ollama:${this.#model}`;
  }

  async interpret(request: EngineRequest): Promise<IntentProposalWire> {
    const reconsideration = reconsiderationProposal(
      request,
      this.#idFactory(),
      this.#now(),
    );
    if (reconsideration !== undefined) return reconsideration;
    const prompt = buildIntentPrompt(request);
    let response: Response;
    try {
      response = await this.#fetch(`${this.#baseUrl}/api/generate`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          model: this.#model,
          prompt,
          stream: false,
          think: false,
          options: { temperature: 0, seed: 0 },
          // Ollama treats a JSON schema supplied here as constrained output.
          format: request.desiredOutput,
        }),
      });
    } catch (error) {
      throw new OllamaLanguageInterpreterError("generate API request failed", {
        cause: error,
      });
    }

    let payload: OllamaGenerateResponse;
    try {
      payload = (await response.json()) as OllamaGenerateResponse;
    } catch (error) {
      throw new OllamaLanguageInterpreterError(
        "provider returned a non-JSON response",
        { cause: error },
      );
    }

    if (!isRecord(payload)) {
      throw new OllamaLanguageInterpreterError(
        "provider response did not contain a JSON object envelope",
      );
    }

    if (!response.ok || payload.error !== undefined) {
      const detail =
        typeof payload.error === "string"
          ? payload.error
          : response.statusText || "unknown provider error";
      throw new OllamaLanguageInterpreterError(
        `generate API returned ${response.status}: ${detail}`,
      );
    }

    if (payload.response === undefined && payload.thinking === undefined) {
      throw new OllamaLanguageInterpreterError(
        "generate API result did not contain response",
      );
    }

    return wireInterpretation(
      "ollama",
      this.#source,
      this.#model,
      ollamaStructuredContent(payload),
      request,
      this.#idFactory(),
      this.#now(),
    );
  }
}

export function reconsiderationProposal(
  request: EngineRequest,
  requestId: string,
  generatedAt: Date,
): IntentProposalWire | undefined {
  const reconsideration = buildReconsiderationProposal(request);
  if (reconsideration === undefined) return undefined;
  return {
    content: reconsideration,
    source: "spoon:reconsideration",
    status: "unverified",
    provenance: {
      provider: "spoon",
      model: "reconsideration",
      requestId,
      generatedAt: generatedAt.toISOString(),
      requestHash: fingerprintIntentRequest(request),
    },
  };
}

export function wireInterpretation(
  provider: Exclude<IntentProvider, "spoon">,
  source: string,
  model: string,
  raw: unknown,
  request: EngineRequest,
  requestId: string,
  generatedAt: Date,
): IntentProposalWire {
  const rawContent = parseProposal(raw, provider);
  const content = normalizeProposalSelection(rawContent, request);
  validateProposalForRequest(content, request, provider);
  return {
    content,
    rawContent,
    source,
    status: "unverified",
    provenance: {
      provider,
      model,
      requestId,
      generatedAt: generatedAt.toISOString(),
      requestHash: fingerprintIntentRequest(request),
    },
  };
}

function buildReconsiderationProposal(
  request: EngineRequest,
): InterpretationProposal | undefined {
  if (!isReconsideration(request)) return undefined;
  if (!isRecord(request.context)) return undefined;
  const context = request.context as Record<string, JsonValue>;
  const reconsideration = context.reconsideration;
  const candidates = context.candidates;
  const literalCandidates = context.literalCandidates;
  if (
    !isRecord(reconsideration) ||
    !Array.isArray(candidates) ||
    candidates.length !== 1 ||
    !Array.isArray(literalCandidates)
  ) {
    return undefined;
  }
  const reconsiderationRecord = reconsideration as Record<string, JsonValue>;
  if (!Array.isArray(reconsiderationRecord.previousInputs)) return undefined;
  const previousInputs = reconsiderationRecord.previousInputs as JsonValue[];
  const candidate = candidates[0];
  if (!isRecord(candidate)) {
    return undefined;
  }
  const candidateRecord = candidate as Record<string, JsonValue>;
  if (typeof candidateRecord.alias !== "string") return undefined;
  const procedure = candidateRecord.procedure;
  if (!isRecord(procedure)) return undefined;
  const procedureRecord = procedure as Record<string, JsonValue>;
  if (!Array.isArray(procedureRecord.slots)) return undefined;
  const quoted = literalCandidates
    .filter(isRecord)
    .filter((literal) => typeof literal.text === "string")
    .filter((literal) => /^(["']).*\1$/.test(literal.text as string))
    .at(-1);
  let replacementRange: TokenRange | undefined;
  const replacementRangeValue = quoted?.tokenRange;
  if (isRecord(replacementRangeValue)) {
    const replacementRangeRecord = replacementRangeValue as Record<
      string,
      JsonValue
    >;
    if (
      typeof replacementRangeRecord.startToken === "number" &&
      typeof replacementRangeRecord.endToken === "number"
    ) {
      replacementRange = {
        startToken: replacementRangeRecord.startToken,
        endToken: replacementRangeRecord.endToken,
      };
    }
  }
  const slots = procedureRecord.slots.map((slot, index) => {
    if (!isRecord(slot)) return undefined;
    const slotRecord = slot as Record<string, JsonValue>;
    if (typeof slotRecord.name !== "string") return undefined;
    if (slotRecord.name === "text" && replacementRange !== undefined) {
      return {
        name: slotRecord.name,
        confidence: 1,
        sourceTokens: replacementRange === undefined ? [] : [replacementRange],
      };
    }
    const inferredValue = previousInputs[index];
    if (inferredValue === undefined) return undefined;
    return {
      name: slotRecord.name,
      confidence: 1,
      sourceTokens: [],
      inferredValue,
    };
  });
  if (slots.some((slot) => slot === undefined)) return undefined;
  return {
    candidates: [
      {
        name: candidateRecord.alias,
        confidence: 1,
        scope: "Conversation",
        sourceTokens: replacementRange === undefined ? [] : [replacementRange],
        slots: slots as InterpretationProposal["candidates"][number]["slots"],
        ambiguities: [],
      },
    ],
    selected: 0,
    disposition: "execute",
  };
}

export function fingerprintIntentRequest(request: EngineRequest): string {
  return `sha256:${createHash("sha256")
    .update(canonicalJson(request))
    .digest("hex")}`;
}

export function buildIntentPrompt(request: EngineRequest): string {
  const indexedTokens = request.tokenStream.tokens.map((token, index) => ({
    index,
    kind: token.kind,
    text: sliceUtf8(
      request.tokenStream.document.text,
      token.span.start_byte,
      token.span.end_byte,
    ),
  }));
  return [
    "You are Spoon's bounded language interpreter.",
    "Return only one JSON object matching the supplied schema.",
    `UTTERANCE: ${JSON.stringify(request.situation)}`,
    `INDEXED TOKENS: ${JSON.stringify(indexedTokens)}`,
    `CANDIDATE GUIDE:\n${candidateGuide(request.context)}`,
    `TURN MODE: ${isReconsideration(request) ? "RECONSIDERATION — challenge the most recent prior procedure and repair or re-run it" : "NEW OR STANDALONE REQUEST"}`,
    `AVAILABLE CONTEXT AND CANDIDATES: ${JSON.stringify(request.context)}`,
    "Rules:",
    "1. Candidate names MUST be an exact request-local alias from AVAILABLE CONTEXT, such as candidate_0. Never output a semantic name or database identifier.",
    '2. sourceTokens are half-open INDEX POSITIONS in INDEXED TOKENS, not character or byte offsets. A single token at index 4 is {"startToken":4,"endToken":5}.',
    `3. Valid token indexes are 0 through ${Math.max(0, indexedTokens.length - 1)}; the largest valid endToken is ${indexedTokens.length}.`,
    "4. Use exactly one contiguous sourceTokens range for each frame and grounded slot. A slot sourceTokens value MUST exactly copy tokenRange from one of the supplied literalCandidates.",
    "5. For execute, selected MUST be the zero-based index of the chosen candidate in your candidates array. For clarify or abstain, selected MUST be null.",
    "6. For occurrence/count procedures, the `text` slot is the containing source and the `target` slot is the substring being searched. If the request quotes a target letter and names a larger unquoted word, bind those to target and text respectively; do not bind both slots to the same span unless the utterance explicitly says so.",
    "7. Execute only when a supplied candidate clearly matches and every required slot listed in CANDIDATE GUIDE can be grounded. Never select a candidate merely because one word overlaps.",
    "8. If the current turn corrects or refers to the last turn, preserve that prior operation and carry forward unchanged slot values with inferredValue plus an empty sourceTokens array. Do not invent a new operation from conversational filler such as `are you sure`.",
    "9. In RECONSIDERATION mode, `previousInputs` are listed in the prior procedure's slot order. Preserve them unless the current wording explicitly replaces a value; a newly quoted source word replaces the source-text slot, while an omitted target remains inferred from the prior turn.",
    "10. Emit every required slot exactly once and emit no other slots. For a clear match with grounded required slots, ambiguities MUST be [] and disposition MUST be execute; never copy the utterance into ambiguities.",
    `OUTPUT SCHEMA: ${JSON.stringify(request.desiredOutput)}`,
  ].join("\n\n");
}

function candidateGuide(context: JsonValue): string {
  if (!isRecord(context)) {
    return "No executable candidates are available.";
  }
  const candidates = (context as Record<string, JsonValue>).candidates;
  if (!Array.isArray(candidates))
    return "No executable candidates are available.";
  const lines = candidates.flatMap((candidate: JsonValue) => {
    if (!isRecord(candidate)) return [];
    const candidateRecord = candidate as Record<string, JsonValue>;
    if (typeof candidateRecord.alias !== "string") return [];
    const procedure = isRecord(candidateRecord.procedure)
      ? (candidateRecord.procedure as Record<string, JsonValue>)
      : {};
    const name =
      typeof procedure.name === "string"
        ? procedure.name
        : typeof candidateRecord.name === "string"
          ? candidateRecord.name
          : "unnamed procedure";
    const slots = Array.isArray(procedure.slots)
      ? procedure.slots.flatMap((slot: JsonValue) => {
          if (!isRecord(slot)) return [];
          const slotRecord = slot as Record<string, JsonValue>;
          if (typeof slotRecord.name !== "string") return [];
          const description =
            typeof slotRecord.description === "string"
              ? ` — ${slotRecord.description}`
              : "";
          return [`${slotRecord.name}${description}`];
        })
      : [];
    return [
      `- ${candidateRecord.alias}: ${name}; required slots: ${slots.join(", ") || "none"}`,
    ];
  });
  return lines.join("\n") || "No executable candidates are available.";
}

function isReconsideration(request: EngineRequest): boolean {
  const text = request.situation;
  const safeRecheck = /\b(sure|recheck|again)\b/i.test(text);
  const correctionWithReference =
    /\b(wrong|incorrect|mistake|correct)\b/i.test(text) &&
    /\b(last|previous|earlier|before|answer|result)\b/i.test(text);
  if (!safeRecheck && !correctionWithReference) {
    return false;
  }
  if (!isRecord(request.context)) return false;
  const priorTurns = (request.context as Record<string, JsonValue>).priorTurns;
  return Array.isArray(priorTurns) && priorTurns.length > 0;
}

function validateProposalForRequest(
  proposal: InterpretationProposal,
  request: EngineRequest,
  provider: Exclude<IntentProvider, "spoon">,
): void {
  const aliases = requestCandidateAliases(request.context);
  const tokenCount = request.tokenStream.tokens.length;
  if (
    (proposal.disposition === "execute" && proposal.selected === null) ||
    (proposal.disposition !== "execute" && proposal.selected !== null) ||
    (proposal.selected !== null &&
      (!Number.isInteger(proposal.selected) ||
        proposal.selected < 0 ||
        proposal.selected >= proposal.candidates.length))
  ) {
    throw new LanguageInterpreterError(
      provider,
      `provider returned an invalid disposition selection (disposition=${proposal.disposition}, selected=${String(proposal.selected)}, candidates=${proposal.candidates.length})`,
    );
  }
  for (const candidate of proposal.candidates) {
    if (!aliases.has(candidate.name)) {
      throw new LanguageInterpreterError(
        provider,
        "provider selected an unknown request-local candidate",
      );
    }
    for (const range of [
      ...candidate.sourceTokens,
      ...candidate.slots.flatMap((slot) => slot.sourceTokens),
    ]) {
      if (range.endToken > tokenCount) {
        throw new LanguageInterpreterError(
          provider,
          "provider returned a token range outside the current utterance",
        );
      }
    }
  }
}

function requestCandidateAliases(context: JsonValue): Set<string> {
  if (!isRecord(context)) {
    return new Set();
  }
  const candidates = (context as { [key: string]: JsonValue })["candidates"];
  if (!Array.isArray(candidates)) return new Set();
  return new Set(
    candidates.flatMap((candidate: JsonValue) => {
      if (!isRecord(candidate)) return [];
      const alias = (candidate as { [key: string]: JsonValue })["alias"];
      return typeof alias === "string" ? [alias] : [];
    }),
  );
}

function sliceUtf8(text: string, startByte: number, endByte: number): string {
  return Buffer.from(text, "utf8")
    .subarray(startByte, endByte)
    .toString("utf8");
}

function parseProposal(
  value: unknown,
  provider: Exclude<IntentProvider, "spoon">,
): InterpretationProposal {
  let parsed: unknown = value;
  if (typeof value === "string") {
    try {
      parsed = JSON.parse(value) as unknown;
    } catch (error) {
      throw new LanguageInterpreterError(
        provider,
        "provider response was not valid JSON",
        { cause: error },
      );
    }
  }
  if (!isInterpretationProposal(parsed)) {
    throw new LanguageInterpreterError(
      provider,
      "provider response did not match InterpretationProposal",
    );
  }
  return parsed;
}

/**
 * `selected` repeats information already carried by `disposition` and the
 * candidate list. Small constrained models occasionally emit `execute` with
 * `null` (or an index alongside `abstain`) despite otherwise valid output.
 * Canonicalize only the unambiguous cases before the local grounding boundary;
 * ambiguous or out-of-range selections still fail closed below.
 */
function normalizeProposalSelection(
  proposal: InterpretationProposal,
  request: EngineRequest,
): InterpretationProposal {
  const requiredSlots = requestCandidateSlots(request.context);
  const normalizedCandidates = proposal.candidates.map((candidate) => {
    const permitted = requiredSlots.get(candidate.name);
    const seen = new Set<string>();
    const slots = candidate.slots
      .filter((slot) => permitted === undefined || permitted.has(slot.name))
      .filter((slot) => {
        if (seen.has(slot.name)) return false;
        seen.add(slot.name);
        return true;
      })
      .map((slot) => {
        if (
          slot.sourceTokens.length === 0 ||
          slot.inferredValue === undefined
        ) {
          return slot;
        }
        const { inferredValue: _ignored, ...grounded } = slot;
        return grounded;
      });
    const utterance = request.situation.trim().toLocaleLowerCase();
    const ambiguities = candidate.ambiguities.filter(
      (ambiguity) => ambiguity.trim().toLocaleLowerCase() !== utterance,
    );
    return { ...candidate, slots, ambiguities };
  });
  const normalized = { ...proposal, candidates: normalizedCandidates };
  // The Rust boundary deliberately refuses to execute a frame that still
  // carries unresolved ambiguity. Tiny models sometimes emit that invalid
  // combination anyway; preserve the ambiguity and turn it into the safe
  // clarify disposition instead of producing an opaque application error.
  if (
    normalized.disposition === "execute" &&
    normalized.candidates.some((candidate) => candidate.ambiguities.length > 0)
  ) {
    return { ...normalized, disposition: "clarify", selected: null };
  }
  if (
    normalized.disposition === "execute" &&
    normalized.candidates.length === 1
  ) {
    return { ...normalized, selected: 0 };
  }
  if (
    normalized.disposition === "clarify" &&
    normalized.candidates.length === 1 &&
    normalized.candidates[0]!.ambiguities.length === 0 &&
    candidateHasEveryGroundedSlot(
      normalized.candidates[0]!,
      requiredSlots.get(normalized.candidates[0]!.name),
    )
  ) {
    return { ...normalized, disposition: "execute", selected: 0 };
  }
  if (normalized.disposition !== "execute" && normalized.selected !== null) {
    return { ...normalized, selected: null };
  }
  return normalized;
}

function requestCandidateSlots(context: JsonValue): Map<string, Set<string>> {
  const result = new Map<string, Set<string>>();
  if (!isRecord(context)) return result;
  const candidates = (context as Record<string, JsonValue>).candidates;
  if (!Array.isArray(candidates)) return result;
  for (const candidate of candidates) {
    if (!isRecord(candidate)) continue;
    const candidateRecord = candidate as Record<string, JsonValue>;
    if (typeof candidateRecord.alias !== "string") continue;
    const procedure = candidateRecord.procedure;
    if (!isRecord(procedure)) continue;
    const procedureRecord = procedure as Record<string, JsonValue>;
    if (!Array.isArray(procedureRecord.slots)) continue;
    result.set(
      candidateRecord.alias,
      new Set(
        procedureRecord.slots.flatMap((slot: JsonValue) => {
          if (!isRecord(slot)) return [];
          const slotRecord = slot as Record<string, JsonValue>;
          return typeof slotRecord.name === "string" ? [slotRecord.name] : [];
        }),
      ),
    );
  }
  return result;
}

function candidateHasEveryGroundedSlot(
  candidate: IntentFrameProposal,
  required: Set<string> | undefined,
): boolean {
  if (required === undefined) return false;
  const grounded = new Set(
    candidate.slots.flatMap((slot) =>
      slot.sourceTokens.length > 0 || slot.inferredValue !== undefined
        ? [slot.name]
        : [],
    ),
  );
  return [...required].every((name) => grounded.has(name));
}

function isInterpretationProposal(
  value: unknown,
): value is InterpretationProposal {
  if (!isRecord(value)) return false;
  if (
    !Array.isArray(value.candidates) ||
    (value.selected !== null &&
      (!isFiniteNumber(value.selected) || !Number.isInteger(value.selected))) ||
    !isDisposition(value.disposition)
  ) {
    return false;
  }
  return value.candidates.every(isIntentFrameProposal);
}

function isIntentFrameProposal(value: unknown): value is IntentFrameProposal {
  if (!isRecord(value)) return false;
  return (
    typeof value.name === "string" &&
    isFiniteNumber(value.confidence) &&
    typeof value.scope === "string" &&
    Array.isArray(value.sourceTokens) &&
    value.sourceTokens.every(isTokenRange) &&
    Array.isArray(value.slots) &&
    value.slots.every(isIntentSlotProposal) &&
    Array.isArray(value.ambiguities) &&
    value.ambiguities.every((item) => typeof item === "string")
  );
}

function isIntentSlotProposal(value: unknown): value is IntentSlotProposal {
  if (!isRecord(value)) return false;
  const hasSourceTokens = Array.isArray(value.sourceTokens);
  const hasInferredValue = "inferredValue" in value;
  return (
    typeof value.name === "string" &&
    isFiniteNumber(value.confidence) &&
    hasSourceTokens &&
    value.sourceTokens.every(isTokenRange) &&
    (!hasInferredValue || isJsonValue(value.inferredValue)) &&
    (value.sourceTokens.length > 0 || hasInferredValue)
  );
}

function isTokenRange(value: unknown): value is TokenRange {
  return (
    isRecord(value) &&
    isFiniteNumber(value.startToken) &&
    isFiniteNumber(value.endToken) &&
    Number.isInteger(value.startToken) &&
    Number.isInteger(value.endToken) &&
    value.startToken >= 0 &&
    value.endToken > value.startToken
  );
}

function isDisposition(
  value: unknown,
): value is InterpretationProposal["disposition"] {
  return value === "execute" || value === "clarify" || value === "abstain";
}

function isFiniteNumber(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value);
}

function isJsonValue(value: unknown): value is JsonValue {
  if (
    value === null ||
    typeof value === "string" ||
    typeof value === "boolean"
  ) {
    return true;
  }
  if (isFiniteNumber(value)) return true;
  if (Array.isArray(value)) return value.every(isJsonValue);
  return isRecord(value) && Object.values(value).every(isJsonValue);
}

function isRecord(value: unknown): value is Record<string, any> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
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
  return `{${Object.keys(record)
    .sort()
    .map((key) => `${JSON.stringify(key)}:${canonicalJson(record[key])}`)
    .join(",")}}`;
}
