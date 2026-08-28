export type JsonPrimitive = null | boolean | number | string;

export type JsonValue =
  JsonPrimitive | JsonValue[] | { [key: string]: JsonValue };

export interface TokenSpan {
  start_byte: number;
  end_byte: number;
}

export interface Token {
  kind: string;
  span: TokenSpan;
}

export interface TokenStream {
  document: {
    text: string;
    normalization: string;
  };
  tokens: Token[];
}

/** The request envelope supplied by the Engine to a language interpreter. */
export interface EngineRequest {
  situation: string;
  tokenStream: TokenStream;
  context: JsonValue;
  desiredOutput: JsonValue;
}

/** Alias used by callers that name the transport boundary explicitly. */
export type LanguageInterpreterRequest = EngineRequest;

export interface TokenRange {
  startToken: number;
  endToken: number;
}

export interface IntentSlotProposal {
  name: string;
  confidence: number;
  /** A contiguous or otherwise explicitly selected range in the token stream. */
  sourceTokens: TokenRange[];
  /** A value supplied by interpretation rather than grounded in source text. */
  inferredValue?: JsonValue;
}

export interface IntentFrameProposal {
  /** Request-local candidate alias, for example `candidate_0`. */
  name: string;
  confidence: number;
  scope: "CurrentTurn" | "Conversation" | "Workspace" | "External";
  sourceTokens: TokenRange[];
  slots: IntentSlotProposal[];
  ambiguities: string[];
}

export type IntentDisposition = "execute" | "clarify" | "abstain";

export interface InterpretationProposal {
  candidates: IntentFrameProposal[];
  selected: number | null;
  disposition: IntentDisposition;
}

/** The provider that produced an interpretation proposal.
 *
 * `spoon` is used for deterministic engine-side repairs (for example a
 * reconsideration of the immediately preceding procedure), so those
 * proposals are not misreported as model output.
 */
export type IntentProvider = "ollama" | "cursor" | "spoon";

export interface IntentProposalProvenance {
  provider: IntentProvider;
  model: string;
  requestId: string;
  generatedAt: string;
  requestHash: string;
}

export interface IntentProposalWire {
  content: InterpretationProposal;
  /** Exact structured payload parsed from the provider before local normalization. */
  rawContent?: InterpretationProposal;
  source: string;
  status: "unverified";
  provenance: IntentProposalProvenance;
}

export interface LanguageInterpreter {
  interpret(request: EngineRequest): Promise<IntentProposalWire>;
}

export type Clock = () => Date;
export type IdFactory = () => string;
export type IntentFetch = (
  input: string | URL,
  init?: RequestInit,
) => Promise<Response>;
