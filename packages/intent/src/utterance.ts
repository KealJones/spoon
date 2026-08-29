import type { JsonValue, TokenRange, TokenStream } from "./types.js";

/**
 * Wire types for utterance-level analysis.
 *
 * Every shape here mirrors what serde actually emits on the Rust side, checked
 * against real output rather than inferred from the struct definitions. The
 * enum representations differ deliberately by type: mention resolutions are
 * snake_case because they read as data, language write kinds are kebab-case
 * because they are relationship names, and dialogue acts keep their Rust
 * spelling because they are a closed vocabulary shared with the renderer.
 */

export type MentionKind = "entity" | "value" | "expression" | "result";

export type PartRefRole = "mention" | "result";

/** Externally tagged, snake_case, matching `MentionResolutionProposal`. */
export type MentionResolutionProposal =
  | { literal: { value: JsonValue } }
  | { part_ref: { part: string; role: PartRefRole } }
  | { context_ref: { alias: string } }
  | { unresolved: { ambiguity: string } };

export interface MentionProposal {
  /** `e0` for an entity, `v0` for a value, `x0` for a result. */
  key: string;
  kind: MentionKind;
  sourceTokens?: TokenRange[];
  inferred?: boolean;
  resolved: MentionResolutionProposal;
}

export type ResidualPolarity = "assert" | "deny";

/**
 * Where a proposed fact came from. There is no retrieval in this design, so a
 * residual with neither variant is model-weight recall and is refused.
 */
export type ResidualProvenanceProposal =
  { utteranceTokens: TokenRange } | { contextAlias: string };

export interface ResidualProposal {
  id: string;
  predicate: string;
  value: JsonValue;
  scope?: Record<string, JsonValue>;
  polarity: ResidualPolarity;
  provenance: ResidualProvenanceProposal;
}

export type LanguageWriteKind = "alias-of" | "termed" | "intent-of";

export interface LanguageWriteProposal {
  kind: LanguageWriteKind;
  surface: string;
  /** A request-local packet alias. Never a durable identifier. */
  targetAlias: string;
  sourceTokens?: TokenRange[];
}

export interface AlignmentProposal {
  cleanedStart: number;
  cleanedEnd: number;
  sourceTokens: TokenRange;
}

export type DialogueAct =
  | "Inform"
  | "Ask"
  | "Clarify"
  | "Confirm"
  | "Correct"
  | "Acknowledge"
  | "Refuse"
  | "Abstain";

export interface PartProposal {
  /** `p0`, `p1`, `p2` in source order. */
  id: string;
  sourceTokens: TokenRange[];
  template: string;
  act: DialogueAct;
  mentions?: MentionProposal[];
  contextBindings?: MentionProposal[];
  intent: import("./types.js").InterpretationProposal;
  residual?: ResidualProposal[];
}

export interface UtteranceAnalysisProposal {
  cleaned: string;
  alignment?: AlignmentProposal[];
  parts: PartProposal[];
  languageWrites?: LanguageWriteProposal[];
}

// ---------------------------------------------------------------------------
// Context packet
// ---------------------------------------------------------------------------

export type TurnRole = "user" | "spoon";

export interface PacketFact {
  alias: string;
  predicate: string;
  value: JsonValue;
}

export interface PacketTurn {
  alias: string;
  role: TurnRole;
  summary: string;
  facts?: PacketFact[];
}

export interface PacketSlot {
  name: string;
  required: boolean;
  valueKind: string;
}

export interface PacketCatalogEntry {
  alias: string;
  /** A stable semantic key such as `arithmetic.multiply`, never a UUID. */
  key: string;
  slots: PacketSlot[];
  patterns: string[];
  bound: boolean;
}

export interface PacketAlias {
  alias: string;
  surface: string;
  refersTo: string;
}

export interface PacketEnvFact {
  alias: string;
  predicate: string;
  value: JsonValue;
}

/** What a bound removed, so a trimmed context cannot read as a complete one. */
export interface TruncationFlag {
  group: string;
  dropped: number;
}

export interface LanguageContextPacket {
  utterance: TokenStream;
  turns?: PacketTurn[];
  catalog?: PacketCatalogEntry[];
  terminology?: PacketAlias[];
  environment?: PacketEnvFact[];
  truncation?: TruncationFlag[];
}

/**
 * The one follow-up an interpreter may make. Every variant names something the
 * packet already surfaced, so a model can ask for detail but cannot widen its
 * own reach. There is deliberately no free-form field.
 */
export type SupplementalRequest =
  | { catalogDetail: { alias: string } }
  | { turnWindow: { count: number } }
  | { terminology: { sourceTokens: TokenRange } };

export const MAX_TURN_WINDOW = 4;

// ---------------------------------------------------------------------------
// Realization
// ---------------------------------------------------------------------------

export type ResponseTone = "Neutral" | "Direct" | "Warm" | "Formal";

/** The pinned template set. A template id outside this list is refused. */
export const REALIZATION_TEMPLATE_IDS = [
  "join.sentences",
  "join.and",
  "join.and.list",
  "join.then",
  "join.lead.ack",
  "join.ack.and",
] as const;

export type RealizationTemplateId = (typeof REALIZATION_TEMPLATE_IDS)[number];

/**
 * Realizer output. There is deliberately no text field: the model picks a
 * shape and an order, and the Engine supplies every character. Fabrication is
 * not checked for, it is structurally impossible.
 */
export interface RealizationProposal {
  templateId: RealizationTemplateId;
  /** A permutation of the plan's grounded claim ids. */
  slotOrder: string[];
  tone: ResponseTone;
}
