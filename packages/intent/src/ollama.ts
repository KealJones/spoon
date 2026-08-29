import { createHash, randomUUID } from "node:crypto";
import { request as httpRequest } from "node:http";
import type { IncomingMessage } from "node:http";
import { request as httpsRequest } from "node:https";

import {
  LanguageInterpreterError,
  OllamaLanguageInterpreterError,
} from "./errors.js";
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

// Grounding slots onto half-open token indexes needs real instruction
// following: a 4b model picks plausible-looking but wrong spans. This mixture
// of experts activates about 3b parameters per token, so it reasons like a
// large model while evaluating the prompt like a small one.
const DEFAULT_MODEL = "qwen3:30b-a3b";
const DEFAULT_BASE_URL = "http://localhost:11434";
const OLLAMA_FETCH_TIMEOUT_MS = 60 * 60 * 1000;

/**
 * Talks to Ollama over `node:http` rather than the global `fetch`.
 *
 * `fetch` applies undici's 300s header timeout, and Ollama sends no headers
 * until it has finished evaluating the prompt. A large prompt on a local model
 * can exceed that, which aborted the request and turned every slow
 * interpretation into an abstention. A plain client lets the caller own the
 * deadline instead.
 */
function defaultOllamaFetch(
  input: string | URL,
  init?: RequestInit,
): Promise<Response> {
  const url = new URL(String(input));
  const send = url.protocol === "https:" ? httpsRequest : httpRequest;
  const headers = new Headers(init?.headers);
  if (!headers.has("content-type")) {
    headers.set("content-type", "application/json");
  }
  const body =
    typeof init?.body === "string" || init?.body === undefined
      ? init?.body
      : undefined;
  if (init?.body !== undefined && body === undefined) {
    return Promise.reject(
      new OllamaLanguageInterpreterError("generate API request failed"),
    );
  }

  return new Promise((resolve, reject) => {
    const request = send(
      url,
      {
        method: init?.method ?? "GET",
        headers: Object.fromEntries(headers.entries()),
      },
      (response: IncomingMessage) => {
        const chunks: Buffer[] = [];
        response.on("data", (chunk: Buffer | string) => {
          chunks.push(typeof chunk === "string" ? Buffer.from(chunk) : chunk);
        });
        response.on("end", () => {
          resolve(
            new Response(Buffer.concat(chunks), {
              status: response.statusCode ?? 502,
              statusText: response.statusMessage ?? "",
            }),
          );
        });
        response.on("error", reject);
      },
    );
    request.setTimeout(OLLAMA_FETCH_TIMEOUT_MS, () => {
      request.destroy(new Error("ollama generate timed out"));
    });
    request.on("error", reject);
    if (body !== undefined) request.write(body);
    request.end();
  });
}

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
    this.#fetch = options.fetch ?? defaultOllamaFetch;
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
          // A non-streaming generate sends no bytes until the whole completion
          // is built, so a slow local model trips undici's 300s header timeout
          // before the response ever starts. Streaming delivers headers at once
          // and keeps the body flowing, which removes that ceiling.
          stream: true,
          keep_alive: "30m",
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

    const payload = await readOllamaGenerate(response);

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

    const content = ollamaStructuredContent(payload);
    if (
      content === undefined ||
      (typeof content === "string" && content.trim().length === 0)
    ) {
      throw new OllamaLanguageInterpreterError(
        "generate API result did not contain response",
      );
    }

    return wireInterpretation(
      "ollama",
      this.#source,
      this.#model,
      content,
      request,
      this.#idFactory(),
      this.#now(),
    );
  }
}

/**
 * Reads a generate response in either shape. A streaming generate emits one
 * JSON object per line and splits the completion across `response` chunks, so
 * the parts are reassembled here; a single-object body is returned as-is.
 */
async function readOllamaGenerate(
  response: Response,
): Promise<OllamaGenerateResponse> {
  let raw: string;
  try {
    raw = await response.text();
  } catch (error) {
    throw new OllamaLanguageInterpreterError(
      "provider returned a non-JSON response",
      { cause: error },
    );
  }

  const trimmed = raw.trim();
  if (trimmed.length === 0) {
    throw new OllamaLanguageInterpreterError(
      "provider returned a non-JSON response",
    );
  }

  try {
    return JSON.parse(trimmed) as OllamaGenerateResponse;
  } catch {
    // Fall through to the newline-delimited streaming form.
  }

  let assembledResponse = "";
  let assembledThinking = "";
  let error: unknown;
  for (const line of trimmed.split("\n")) {
    const chunk = line.trim();
    if (chunk.length === 0) continue;
    let parsed: OllamaGenerateResponse;
    try {
      parsed = JSON.parse(chunk) as OllamaGenerateResponse;
    } catch (cause) {
      throw new OllamaLanguageInterpreterError(
        "provider returned a non-JSON response",
        { cause },
      );
    }
    if (parsed.error !== undefined) error = parsed.error;
    if (typeof parsed.response === "string") assembledResponse += parsed.response;
    if (typeof parsed.thinking === "string") assembledThinking += parsed.thinking;
  }
  return {
    response: assembledResponse,
    thinking: assembledThinking,
    ...(error === undefined ? {} : { error }),
  };
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

/**
 * The structured payload an Ollama generate call produced.
 *
 * A thinking model returns its constrained output in `thinking` and leaves
 * `response` empty, so falling back is required rather than defensive. This
 * was observed directly: `qwen3.8:27b` returns a complete, schema-valid
 * proposal in `thinking` and an empty string in `response`.
 */
export function ollamaStructuredContent(
  payload: OllamaGenerateResponse,
): unknown {
  if (typeof payload.response === "string" && payload.response.length > 0) {
    return payload.response;
  }
  return payload.thinking ?? payload.response;
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
  // Ordered stable content first, then per-turn content. A local model reuses
  // its KV cache only across an identical prompt prefix, so anything that
  // changes every turn has to sit behind everything that does not. The rules
  // therefore carry no per-utterance numbers; the concrete token bounds travel
  // with the tokens instead.
  return [
    "You are Spoon's bounded language interpreter.",
    "Return only one JSON object matching the supplied schema.",
    "Rules:",
    "1. Candidate names MUST be an exact request-local alias from CANDIDATES, such as candidate_0. Never output a semantic name or database identifier.",
    '2. sourceTokens are half-open INDEX POSITIONS in INDEXED TOKENS, not character or byte offsets. A single token at index 4 is {"startToken":4,"endToken":5}.',
    "3. Valid token indexes are bounded by TOKEN BOUNDS below; never emit an index outside that range.",
    "4. Use exactly one contiguous sourceTokens range for each frame and grounded slot. A slot sourceTokens value MUST exactly copy tokenRange from one of the supplied literalCandidates.",
    "5. For execute, selected MUST be the zero-based index of the chosen candidate in your candidates array. For clarify or abstain, selected MUST be null.",
    "6. For occurrence/count procedures, the `text` slot is the containing source and the `target` slot is the substring being searched. If the request quotes a target letter and names a larger unquoted word, bind those to target and text respectively; do not bind both slots to the same span unless the utterance explicitly says so.",
    "7. Execute only when a supplied candidate clearly matches and every required slot listed in CANDIDATE GUIDE can be grounded. Never select a candidate merely because one word overlaps.",
    "8. If the current turn corrects or refers to the last turn, preserve that prior operation and carry forward unchanged slot values with inferredValue plus an empty sourceTokens array. Do not invent a new operation from conversational filler such as `are you sure`.",
    "9. In RECONSIDERATION mode, `previousInputs` are listed in the prior procedure's slot order. Preserve them unless the current wording explicitly replaces a value; a newly quoted source word replaces the source-text slot, while an omitted target remains inferred from the prior turn.",
    "10. Emit every required slot exactly once and emit no other slots. For a clear match with grounded required slots, ambiguities MUST be [] and disposition MUST be execute; never copy the utterance into ambiguities.",
    // The schema is not repeated here. It travels in the request's `format`
    // field, where Ollama uses it to constrain decoding, so restating it in
    // the prompt only doubled a large document the model cannot deviate from
    // anyway.
    `CANDIDATES: ${JSON.stringify(candidateContext(request.context))}`,
    `CANDIDATE GUIDE:\n${candidateGuide(request.context)}`,
    `TURN CONTEXT: ${JSON.stringify(turnContext(request.context))}`,
    `TURN MODE: ${isReconsideration(request) ? "RECONSIDERATION — challenge the most recent prior procedure and repair or re-run it" : "NEW OR STANDALONE REQUEST"}`,
    `INDEXED TOKENS: ${JSON.stringify(indexedTokens)}`,
    `TOKEN BOUNDS: valid indexes are 0 through ${Math.max(0, indexedTokens.length - 1)}; the largest valid endToken is ${indexedTokens.length}.`,
    `UTTERANCE: ${JSON.stringify(request.situation)}`,
  ].join("\n\n");
}

/** The candidate set, which changes only as the engine learns. */
function candidateContext(context: JsonValue): JsonValue {
  if (!isRecord(context)) return context;
  const { candidates } = context as Record<string, JsonValue>;
  return (candidates ?? []) as JsonValue;
}

/** Everything that can differ from one turn to the next. */
function turnContext(context: JsonValue): JsonValue {
  if (!isRecord(context)) return context;
  const {
    catalog: _catalog,
    candidates: _candidates,
    ...rest
  } = context as Record<string, JsonValue>;
  return rest as JsonValue;
}

/**
 * The slice of the engine context worth spending prompt budget on.
 *
 * The context also carries a full procedure catalog for other consumers. The
 * interpreter routes purely on request-local aliases, so the catalog is a
 * large document the model is never asked to read. Dropping it keeps the
 * prompt from growing with everything Spoon has ever learned.
 */
function promptContext(context: JsonValue): JsonValue {
  if (!isRecord(context)) return context;
  const { catalog: _catalog, ...rest } = context as Record<string, JsonValue>;
  return rest as JsonValue;
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
