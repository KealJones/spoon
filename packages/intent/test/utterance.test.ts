import assert from "node:assert/strict";
import test from "node:test";

import {
  buildRealizationPrompt,
  buildUtteranceAnalysisPrompt,
  looksLikeRealization,
  realizationSchema,
  utteranceAnalysisSchema,
  REALIZATION_TEMPLATE_IDS,
  type LanguageContextPacket,
  type MentionResolutionProposal,
  type RealizationProposal,
  type ResidualProvenanceProposal,
  type SupplementalRequest,
  type TokenStream,
  type UtteranceAnalysisProposal,
} from "../src/index.js";

/**
 * These shapes are checked against real serde output rather than inferred from
 * the Rust structs, so a wire drift on either side shows up here. The exact
 * strings below were produced by serializing the Rust types.
 */

const stream: TokenStream = {
  document: { text: "hey 2", normalization: "Unchanged" },
  tokens: [
    { kind: "Word", span: { start_byte: 0, end_byte: 3 } },
    { kind: "Whitespace", span: { start_byte: 3, end_byte: 4 } },
    { kind: "Number", span: { start_byte: 4, end_byte: 5 } },
  ],
};

const proposal: UtteranceAnalysisProposal = {
  cleaned: "hey 2",
  parts: [
    {
      id: "p0",
      sourceTokens: [{ startToken: 0, endToken: 1 }],
      template: "hey",
      act: "Acknowledge",
      intent: { candidates: [], selected: null, disposition: "abstain" },
    },
    {
      id: "p1",
      sourceTokens: [{ startToken: 2, endToken: 3 }],
      template: "{v0}",
      act: "Inform",
      mentions: [
        {
          key: "v0",
          kind: "value",
          inferred: false,
          sourceTokens: [{ startToken: 2, endToken: 3 }],
          resolved: { literal: { value: 2 } },
        },
      ],
      intent: {
        candidates: [
          {
            name: "candidate_0",
            confidence: 1,
            scope: "CurrentTurn",
            sourceTokens: [],
            slots: [],
            ambiguities: [],
          },
        ],
        selected: 0,
        disposition: "execute",
      },
    },
  ],
};

// ---------------------------------------------------------------------------
// Wire shapes
// ---------------------------------------------------------------------------

test("mention resolutions use the exact externally tagged snake_case shape", () => {
  const literal: MentionResolutionProposal = { literal: { value: 2 } };
  const partRef: MentionResolutionProposal = {
    part_ref: { part: "p1", role: "result" },
  };
  const contextRef: MentionResolutionProposal = {
    context_ref: { alias: "c0" },
  };
  const unresolved: MentionResolutionProposal = {
    unresolved: { ambiguity: "which" },
  };

  assert.equal(JSON.stringify(literal), '{"literal":{"value":2}}');
  assert.equal(
    JSON.stringify(partRef),
    '{"part_ref":{"part":"p1","role":"result"}}',
  );
  assert.equal(JSON.stringify(contextRef), '{"context_ref":{"alias":"c0"}}');
  assert.equal(
    JSON.stringify(unresolved),
    '{"unresolved":{"ambiguity":"which"}}',
  );
});

test("residual provenance uses camelCase variant names", () => {
  const spoken: ResidualProvenanceProposal = {
    utteranceTokens: { startToken: 0, endToken: 1 },
  };
  const cited: ResidualProvenanceProposal = { contextAlias: "f0" };

  assert.equal(
    JSON.stringify(spoken),
    '{"utteranceTokens":{"startToken":0,"endToken":1}}',
  );
  assert.equal(JSON.stringify(cited), '{"contextAlias":"f0"}');
});

test("supplemental requests carry no free-form field", () => {
  const detail: SupplementalRequest = { catalogDetail: { alias: "c0" } };
  const window: SupplementalRequest = { turnWindow: { count: 2 } };
  const terminology: SupplementalRequest = {
    terminology: { sourceTokens: { startToken: 0, endToken: 1 } },
  };

  assert.equal(JSON.stringify(detail), '{"catalogDetail":{"alias":"c0"}}');
  assert.equal(JSON.stringify(window), '{"turnWindow":{"count":2}}');
  assert.equal(
    JSON.stringify(terminology),
    '{"terminology":{"sourceTokens":{"startToken":0,"endToken":1}}}',
  );
});

// ---------------------------------------------------------------------------
// Schemas
// ---------------------------------------------------------------------------

test("the analysis schema closes every object it describes", () => {
  const closed = (node: unknown): void => {
    if (typeof node !== "object" || node === null) return;
    if (Array.isArray(node)) {
      node.forEach(closed);
      return;
    }
    const record = node as Record<string, unknown>;
    if (record.type === "object" && record.properties !== undefined) {
      assert.equal(
        record.additionalProperties,
        false,
        `object schema left open: ${JSON.stringify(record).slice(0, 120)}`,
      );
    }
    Object.values(record).forEach(closed);
  };
  closed(utteranceAnalysisSchema);
});

test("a part must carry at least one source token range", () => {
  const parts = utteranceAnalysisSchema.properties.parts;
  assert.equal(parts.items.properties.sourceTokens.minItems, 1);
  // A real model returned sourceTokens: [] on every part when this was
  // unconstrained, dropping grounding entirely.
  assert.ok(parts.items.required.includes("sourceTokens"));
});

test("the analysis schema bounds the part count", () => {
  assert.equal(utteranceAnalysisSchema.properties.parts.minItems, 1);
  assert.equal(utteranceAnalysisSchema.properties.parts.maxItems, 8);
});

test("a residual must declare provenance", () => {
  const residual =
    utteranceAnalysisSchema.properties.parts.items.properties.residual.items;
  assert.ok(residual.required.includes("provenance"));
  assert.deepEqual(Object.keys(residual.properties.provenance.properties), [
    "utteranceTokens",
    "contextAlias",
  ]);
});

test("the realizer schema admits only the pinned template ids", () => {
  assert.deepEqual(realizationSchema.properties.templateId.enum, [
    ...REALIZATION_TEMPLATE_IDS,
  ]);
  assert.ok(
    !realizationSchema.properties.templateId.enum.includes(
      "join.freestyle" as never,
    ),
  );
});

test("the realizer schema has no way to express prose", () => {
  // The safety property is structural: with no text property and
  // additionalProperties false, a constrained model cannot emit prose even if
  // it ignores the prompt entirely.
  assert.equal(realizationSchema.additionalProperties, false);
  assert.deepEqual(Object.keys(realizationSchema.properties), [
    "templateId",
    "slotOrder",
    "tone",
  ]);
  assert.ok(!("text" in realizationSchema.properties));
});

test("a realization carrying prose is rejected", () => {
  const withText = {
    templateId: "join.and",
    slotOrder: ["c0", "c1"],
    tone: "Neutral",
    text: "Hey, and here is some invented prose.",
  };
  assert.equal(looksLikeRealization(withText), false);

  const clean: RealizationProposal = {
    templateId: "join.and",
    slotOrder: ["c0", "c1"],
    tone: "Neutral",
  };
  assert.equal(looksLikeRealization(clean), true);
});

test("a realization naming an unpinned template is rejected", () => {
  assert.equal(
    looksLikeRealization({
      templateId: "join.freestyle",
      slotOrder: ["c0"],
      tone: "Neutral",
    }),
    false,
  );
});

// ---------------------------------------------------------------------------
// Prompts
// ---------------------------------------------------------------------------

test("the analysis prompt states the real token index bounds", () => {
  const packet: LanguageContextPacket = { utterance: stream };
  const prompt = buildUtteranceAnalysisPrompt("hey 2", stream, packet);

  assert.match(prompt, /Valid indexes are 0 through 2/);
  assert.match(prompt, /largest valid endToken is 3/);
  // Both rules that a real model got wrong are stated explicitly.
  assert.match(prompt, /Connectives such as 'and' or 'then' still belong/);
  assert.match(prompt, /must never be empty/);
  assert.match(prompt, /NEVER compute or guess that value yourself/);
  assert.match(prompt, /NEVER emit a UUID/);
});

test("the analysis prompt indexes whitespace visibly", () => {
  const packet: LanguageContextPacket = { utterance: stream };
  const prompt = buildUtteranceAnalysisPrompt("hey 2", stream, packet);
  assert.match(prompt, /1: <space>/);
  assert.match(prompt, /0: "hey"/);
});

test("the realization prompt tells the model it writes nothing", () => {
  const prompt = buildRealizationPrompt({
    claims: [{ id: "c0", text: "Hey.", act: "Acknowledge" }],
    dependencies: { c1: ["c0"] },
  });

  assert.match(prompt, /You do not write it/);
  assert.match(prompt, /there is no field to put prose in/);
  assert.match(prompt, /must be a permutation/);
  assert.match(prompt, /never be ordered before the claim it consumes/);
});

test("the proposal fixture matches the declared types", () => {
  // A compile-time check that the exported types describe the shape a real
  // model returns. Serializing round-trips it to catch an undefined leak.
  const encoded = JSON.parse(
    JSON.stringify(proposal),
  ) as UtteranceAnalysisProposal;
  assert.equal(encoded.parts.length, 2);
  assert.deepEqual(encoded.parts[1]?.mentions?.[0]?.resolved, {
    literal: { value: 2 },
  });
});
