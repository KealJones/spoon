import type { TokenStream } from "./types.js";
import type {
  LanguageContextPacket,
  RealizationProposal,
} from "./utterance.js";
import { REALIZATION_TEMPLATE_IDS } from "./utterance.js";

/**
 * Prompts for the front language model.
 *
 * Several rules here exist because a real local model got them wrong. Rule 3
 * spells out connectives because `qwen3.8:27b` left "and" and "then" belonging
 * to no part, which the coverage check correctly rejected. Rule 2 repeats the
 * valid index range because a smaller model emitted token indexes past the end
 * of the stream.
 */

function indexTokens(stream: TokenStream): string {
  return stream.tokens
    .map((token, index) => {
      const text = stream.document.text.slice(
        token.span.start_byte,
        token.span.end_byte,
      );
      const shown =
        token.kind === "Whitespace" ? "<space>" : JSON.stringify(text);
      return `  ${index}: ${shown}`;
    })
    .join("\n");
}

export function buildUtteranceAnalysisPrompt(
  situation: string,
  stream: TokenStream,
  packet: LanguageContextPacket,
): string {
  const lastIndex = Math.max(0, stream.tokens.length - 1);
  return [
    "You segment an utterance into speech-act parts for a grounded language engine.",
    "Return only one JSON object matching the supplied schema.",
    `UTTERANCE: ${JSON.stringify(situation)}`,
    `INDEXED TOKENS:\n${indexTokens(stream)}`,
    `CONTEXT PACKET: ${JSON.stringify(packet)}`,
    "Rules:",
    "1. Split the utterance into parts. Each part is ONE speech act: a greeting, a question, a command, or a statement.",
    `2. sourceTokens are half-open INDEX POSITIONS in INDEXED TOKENS, never byte or character offsets. Valid indexes are 0 through ${lastIndex}; the largest valid endToken is ${stream.tokens.length}.`,
    "3. Every non-whitespace token index must be covered by exactly one part, and parts must not overlap. Whitespace may be left uncovered. Connectives such as 'and' or 'then' still belong to a part: attach them to the part that follows.",
    "4. sourceTokens is required on every part and must never be empty.",
    "5. Part ids are p0, p1, p2 in source order.",
    "6. act is the speech act. Use Acknowledge for a greeting, Ask for a question, Inform for a statement, Clarify when you cannot tell what is meant.",
    "7. template is the part's wording with {key} placeholders where a mention appeared.",
    "8. Mention keys are e0/e1 for entities, v0/v1 for values, x0 for a result.",
    '9. If a part uses a value another part has not computed yet, bind it with {"part_ref": {"part": "p1", "role": "result"}} and inferred true. NEVER compute or guess that value yourself.',
    '10. For a value present in the text use {"literal": {"value": 2}} with inferred false and the sourceTokens where it appeared.',
    "11. You may only reference an alias that appears in CONTEXT PACKET. Never invent one.",
    "12. NEVER emit a UUID or any database identifier. The Engine mints identifiers.",
    "13. intent.disposition is execute when the part can be acted on, clarify when it is ambiguous, abstain when it cannot be. For execute, selected MUST be the zero-based index of the chosen candidate; otherwise selected MUST be null.",
    '14. A residual claim is a fact the utterance asserted that is not executable this turn. Every residual needs provenance: either {"utteranceTokens": <range the user actually said>} or {"contextAlias": "<alias from the packet>"}. If you have neither, omit the residual entirely.',
  ].join("\n\n");
}

export interface RealizationRequest {
  /** Claims in the order the deterministic renderer would emit them. */
  claims: { id: string; text: string; act: string }[];
  /** Consumer claim id to the producer claim ids it depends on. */
  dependencies: Record<string, string[]>;
}

export function buildRealizationPrompt(request: RealizationRequest): string {
  return [
    "You choose how an already-written reply is arranged. You do not write it.",
    "Return only one JSON object matching the supplied schema.",
    `CLAIMS: ${JSON.stringify(request.claims)}`,
    `DEPENDENCIES: ${JSON.stringify(request.dependencies)}`,
    `TEMPLATES: ${JSON.stringify(REALIZATION_TEMPLATE_IDS)}`,
    "Rules:",
    "1. You pick a templateId, an order for the claim ids, and a tone. Nothing else. You write no prose, and there is no field to put prose in.",
    "2. slotOrder must be a permutation of every claim id in CLAIMS: no omissions, no repeats, no ids that are not listed.",
    "3. Omitting a claim would drop an answer the user asked for. Repeating one would assert it twice.",
    "4. A claim listed in DEPENDENCIES consumes another claim's result, and must never be ordered before the claim it consumes.",
    "5. join.lead.ack and join.ack.and require an Acknowledge claim in the first slot.",
    "6. join.then asserts that the second claim follows from the first, so only use it when DEPENDENCIES says so.",
    "7. join.and takes exactly 2 claims, join.and.list and join.ack.and take exactly 3, join.sentences takes any number.",
  ].join("\n\n");
}

/**
 * A local shape check for realizer output. The Engine revalidates everything
 * against the pinned template set before rendering, so this only catches an
 * obviously malformed response before it crosses the wire.
 */
export function looksLikeRealization(
  value: unknown,
): value is RealizationProposal {
  if (typeof value !== "object" || value === null) return false;
  const candidate = value as Record<string, unknown>;
  return (
    typeof candidate.templateId === "string" &&
    (REALIZATION_TEMPLATE_IDS as readonly string[]).includes(
      candidate.templateId,
    ) &&
    Array.isArray(candidate.slotOrder) &&
    candidate.slotOrder.every((id) => typeof id === "string") &&
    typeof candidate.tone === "string" &&
    !("text" in candidate)
  );
}
