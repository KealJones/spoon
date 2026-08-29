import type { JsonValue } from "./types.js";
import { REALIZATION_TEMPLATE_IDS } from "./utterance.js";

/**
 * Structured-output schemas for the front language model.
 *
 * Both mirror `deny_unknown_fields` on the Rust side by setting
 * `additionalProperties: false` everywhere. A provider that constrains
 * generation to these schemas cannot emit a field the trusted boundary would
 * reject, which turns a class of grounding failures into an impossibility
 * rather than a retry.
 */

const tokenRange = {
  type: "object",
  additionalProperties: false,
  properties: {
    startToken: { type: "integer", minimum: 0 },
    endToken: { type: "integer", minimum: 1 },
  },
  required: ["startToken", "endToken"],
} as const satisfies JsonValue;

const mentionResolution = {
  type: "object",
  additionalProperties: false,
  properties: {
    literal: {
      type: "object",
      additionalProperties: false,
      properties: { value: {} },
      required: ["value"],
    },
    part_ref: {
      type: "object",
      additionalProperties: false,
      properties: {
        part: { type: "string" },
        role: { enum: ["mention", "result"] },
      },
      required: ["part", "role"],
    },
    context_ref: {
      type: "object",
      additionalProperties: false,
      properties: { alias: { type: "string" } },
      required: ["alias"],
    },
    unresolved: {
      type: "object",
      additionalProperties: false,
      properties: { ambiguity: { type: "string" } },
      required: ["ambiguity"],
    },
  },
} as const satisfies JsonValue;

const mention = {
  type: "object",
  additionalProperties: false,
  properties: {
    key: { type: "string" },
    kind: { enum: ["entity", "value", "expression", "result"] },
    sourceTokens: { type: "array", items: tokenRange },
    inferred: { type: "boolean" },
    resolved: mentionResolution,
  },
  required: ["key", "kind", "inferred", "resolved"],
} as const satisfies JsonValue;

const residual = {
  type: "object",
  additionalProperties: false,
  properties: {
    id: { type: "string" },
    predicate: { type: "string" },
    value: {},
    scope: { type: "object" },
    polarity: { enum: ["assert", "deny"] },
    // Provenance is required, because a fact with no source is model-weight
    // recall rather than something the user said.
    provenance: {
      type: "object",
      additionalProperties: false,
      properties: {
        utteranceTokens: tokenRange,
        contextAlias: { type: "string" },
      },
    },
  },
  required: ["id", "predicate", "value", "polarity", "provenance"],
} as const satisfies JsonValue;

const intent = {
  type: "object",
  additionalProperties: false,
  properties: {
    candidates: {
      type: "array",
      items: {
        type: "object",
        additionalProperties: false,
        properties: {
          name: { type: "string" },
          confidence: { type: "number", minimum: 0, maximum: 1 },
          scope: {
            enum: ["CurrentTurn", "Conversation", "Workspace", "External"],
          },
          sourceTokens: { type: "array", items: tokenRange },
          slots: { type: "array" },
          ambiguities: { type: "array", items: { type: "string" } },
        },
        required: ["name", "confidence", "scope"],
      },
    },
    selected: { type: ["integer", "null"] },
    disposition: { enum: ["execute", "clarify", "abstain"] },
  },
  required: ["candidates", "disposition"],
} as const satisfies JsonValue;

export const utteranceAnalysisSchema = {
  type: "object",
  additionalProperties: false,
  properties: {
    cleaned: { type: "string" },
    alignment: {
      type: "array",
      items: {
        type: "object",
        additionalProperties: false,
        properties: {
          cleanedStart: { type: "integer", minimum: 0 },
          cleanedEnd: { type: "integer", minimum: 0 },
          sourceTokens: tokenRange,
        },
        required: ["cleanedStart", "cleanedEnd", "sourceTokens"],
      },
    },
    parts: {
      type: "array",
      minItems: 1,
      maxItems: 8,
      items: {
        type: "object",
        additionalProperties: false,
        properties: {
          id: { type: "string" },
          // minItems matters: a model told only that the field exists will
          // happily return an empty array and drop grounding entirely.
          sourceTokens: { type: "array", items: tokenRange, minItems: 1 },
          template: { type: "string" },
          act: {
            enum: [
              "Inform",
              "Ask",
              "Clarify",
              "Confirm",
              "Correct",
              "Acknowledge",
              "Refuse",
              "Abstain",
            ],
          },
          mentions: { type: "array", items: mention },
          contextBindings: { type: "array", items: mention },
          intent,
          residual: { type: "array", items: residual },
        },
        required: ["id", "sourceTokens", "template", "act", "intent"],
      },
    },
    languageWrites: {
      type: "array",
      items: {
        type: "object",
        additionalProperties: false,
        properties: {
          kind: { enum: ["alias-of", "termed", "intent-of"] },
          surface: { type: "string" },
          targetAlias: { type: "string" },
          sourceTokens: { type: "array", items: tokenRange },
        },
        required: ["kind", "surface", "targetAlias"],
      },
    },
  },
  required: ["cleaned", "parts"],
} as const satisfies JsonValue;

/**
 * The realizer schema has no `text` property, and `additionalProperties` is
 * false, so a model constrained by it cannot emit prose even if the prompt
 * were ignored entirely.
 */
export const realizationSchema = {
  type: "object",
  additionalProperties: false,
  properties: {
    templateId: { enum: [...REALIZATION_TEMPLATE_IDS] },
    slotOrder: { type: "array", items: { type: "string" } },
    tone: { enum: ["Neutral", "Direct", "Warm", "Formal"] },
  },
  required: ["templateId", "slotOrder", "tone"],
} as const satisfies JsonValue;
