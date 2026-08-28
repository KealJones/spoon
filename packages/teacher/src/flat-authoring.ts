import { TeacherError } from "./errors.js";
import { validateSchema } from "./schema.js";
import { parseJsonContent } from "./shared.js";
import type { JsonObject, JsonValue, ProposalSchema } from "./types.js";

const MAX_LESSON_CONCEPTS = 8;
const MAX_LESSON_PROCEDURES = 4;
const MAX_LESSON_PARAMETERS = 16;
const MAX_LESSON_RELATIONSHIPS = 16;
const MAX_LESSON_NODES = 256;
const MAX_LESSON_CHILDREN = 64;

const binaryOps = [
  "add",
  "subtract",
  "multiply",
  "divide",
  "modulo",
  "equal",
  "not_equal",
  "less_than",
  "less_or_equal",
  "greater_than",
  "greater_or_equal",
  "and",
  "or",
];

const intrinsicOps = [
  "length",
  "text_byte_length",
  "text_scalar_length",
  "text_grapheme_length",
  "text_tokenize",
  "text_split",
  "text_join",
  "text_trim",
  "text_lowercase",
  "text_uppercase",
  "text_contains",
  "text_starts_with",
  "text_ends_with",
  "text_replace",
  "text_url_encode",
  "text_regex_capture",
  "collection_contains",
  "collection_find_index",
  "count_equal",
  "map_keys",
  "map_values",
  "json_parse",
  "json_stringify",
  "path_get",
  "path_get_optional",
  "json_pointer_get",
  "json_pointer_get_optional",
  "json_pointer_set",
  "json_pointer_delete",
  "coalesce",
  "text_normalize_nfc",
  "text_normalize_nfd",
  "text_normalize_nfkc",
  "text_normalize_nfkd",
  "text_trim_start",
  "text_trim_end",
  "text_grapheme_substring",
  "text_index_of",
  "text_count",
  "text_repeat",
  "text_concat_many",
  "map_entries",
  "map_from_entries",
  "map_set",
  "map_delete",
  "map_merge",
  "collection_slice",
  "collection_reverse",
  "collection_sort",
  "collection_unique",
  "collection_flatten",
  "collection_zip",
  "range",
  "type_name",
  "parse_int",
  "parse_float",
  "parse_bool",
  "to_text",
  "numeric_abs",
  "numeric_sign",
  "numeric_min",
  "numeric_max",
  "numeric_clamp",
  "numeric_floor",
  "numeric_ceil",
  "numeric_round",
  "numeric_truncate",
  "numeric_pow_int",
  "numeric_pow_float",
  "integer_quotient",
  "integer_remainder",
];

const string = (): ProposalSchema => ({ type: "string", minLength: 1 });
const reference = (): ProposalSchema => string();
const id = (): ProposalSchema => string();
const ids = (): ProposalSchema => ({
  type: "array",
  maxItems: MAX_LESSON_CHILDREN,
  items: id(),
});
const object = (
  properties: Record<string, ProposalSchema>,
  required: string[],
): ProposalSchema => ({
  type: "object",
  additionalProperties: false,
  properties,
  required,
});

const node = (kind: string, properties: Record<string, ProposalSchema>) =>
  object({ id: id(), kind: { type: "string", const: kind }, ...properties }, [
    "id",
    "kind",
    ...Object.keys(properties),
  ]);

const expressionNode: ProposalSchema = {
  anyOf: [
    node("literal", { valueJson: string() }),
    node("parameter", { name: string() }),
    node("result", {}),
    node("binary", {
      op: { type: "string", enum: binaryOps },
      left: reference(),
      right: reference(),
    }),
    node("unary", {
      op: { type: "string", enum: ["negate", "not"] },
      operand: reference(),
    }),
    node("if", {
      condition: reference(),
      then: reference(),
      else: reference(),
    }),
    node("let", { name: string(), value: reference(), body: reference() }),
    node("list", { items: ids() }),
    node("index", { collection: reference(), index: reference() }),
    node("field", { object: reference(), field: string() }),
    node("map", { collection: reference(), var: string(), body: reference() }),
    node("filter", {
      collection: reference(),
      var: string(),
      predicate: reference(),
    }),
    node("reduce", {
      collection: reference(),
      init: reference(),
      acc: string(),
      var: string(),
      body: reference(),
    }),
    node("intrinsic", {
      version: { type: "integer", const: 1 },
      op: { type: "string", enum: intrinsicOps },
      args: ids(),
    }),
    node("dependency", { alias: string(), args: ids() }),
    node("capability_call", {
      contentId: string(),
      procedureId: string(),
      input: reference(),
    }),
  ],
};

const graph = (): ProposalSchema =>
  object(
    {
      nodes: {
        type: "array",
        minItems: 1,
        maxItems: MAX_LESSON_NODES,
        items: expressionNode,
      },
      result: reference(),
    },
    ["nodes", "result"],
  );

const conceptReference = (): ProposalSchema => ({
  anyOf: [
    object({ kind: { type: "string", const: "new_concept" }, key: string() }, [
      "kind",
      "key",
    ]),
    object(
      { kind: { type: "string", const: "existing_concept" }, id: string() },
      ["kind", "id"],
    ),
  ],
});

const namedJsonValue = (): ProposalSchema =>
  object({ name: string(), valueJson: string() }, ["name", "valueJson"]);

const flatLesson: ProposalSchema = object(
  {
    primitiveSet: { type: "string", const: "spoon_flat_expr_v1" },
    concepts: {
      type: "array",
      minItems: 1,
      maxItems: MAX_LESSON_CONCEPTS,
      items: object(
        {
          key: string(),
          name: string(),
          description: string(),
          mutability: {
            type: "string",
            enum: ["definitional", "defeasible_general", "procedural"],
          },
        },
        ["key", "name", "description", "mutability"],
      ),
    },
    relationships: {
      type: "array",
      maxItems: MAX_LESSON_RELATIONSHIPS,
      items: object(
        {
          source: conceptReference(),
          target: conceptReference(),
          kind: string(),
          strength: { type: "number", minimum: 0, maximum: 1 },
        },
        ["source", "target", "kind", "strength"],
      ),
    },
    procedures: {
      type: "array",
      minItems: 1,
      maxItems: MAX_LESSON_PROCEDURES,
      items: object(
        {
          key: string(),
          name: string(),
          concept: conceptReference(),
          parameters: {
            type: "array",
            minItems: 1,
            maxItems: MAX_LESSON_PARAMETERS,
            items: object(
              {
                name: string(),
                description: string(),
                valueType: {
                  type: "string",
                  enum: [
                    "any",
                    "null",
                    "bool",
                    "number",
                    "text",
                    "list",
                    "map",
                  ],
                },
              },
              ["name", "description", "valueType"],
            ),
          },
          body: graph(),
          contract: object(
            {
              requires: { type: "array", maxItems: 0, items: { type: "null" } },
              promises: { type: "array", maxItems: 0, items: { type: "null" } },
              failsWhen: {
                type: "array",
                maxItems: 0,
                items: { type: "null" },
              },
            },
            ["requires", "promises", "failsWhen"],
          ),
        },
        ["key", "name", "concept", "parameters", "body", "contract"],
      ),
    },
    invocation: object(
      {
        procedureKey: string(),
        inputs: {
          type: "array",
          maxItems: MAX_LESSON_PARAMETERS,
          items: namedJsonValue(),
        },
      },
      ["procedureKey", "inputs"],
    ),
  },
  ["primitiveSet", "concepts", "relationships", "procedures", "invocation"],
);

/**
 * This wire format deliberately contains no `$ref`, type union, or recursive
 * structure. Codex can therefore enforce its object shapes at generation
 * time, while Spoon converts it to the canonical recursive `pure_expr_v2`
 * grammar before local validation and execution.
 */
export const CODEX_FLAT_AUTHORING_SCHEMA: ProposalSchema = object(
  {
    format: { type: "string", const: "spoon_flat_expr_v1" },
    proposalKind: {
      type: "string",
      enum: [
        "reusable_lesson",
        "external_observation",
        "answer_only",
        "abstain",
      ],
    },
    interpretations: { type: "array", maxItems: 0, items: { type: "null" } },
    lesson: { anyOf: [flatLesson, { type: "null" }] },
    answerJson: { type: "string" },
    abstainReason: { type: "string" },
  },
  [
    "format",
    "proposalKind",
    "interpretations",
    "lesson",
    "answerJson",
    "abstainReason",
  ],
);

export const CODEX_FLAT_AUTHORING_INSTRUCTION = [
  "Use the spoon_flat_expr_v1 wire schema exactly. This is a flat expression graph, not nested AST JSON.",
  "Every node has a unique id. Node references are ids. An index node has exactly collection and index; a field node has exactly object and field. There is no target property.",
  'Put every literal and invocation value in its valueJson field as JSON text: use "7" for the JSON number 7 and "\\"foo\\"" for the JSON string foo. Put the final answer in answerJson using the same rule.',
  "For arr[0].name, use parameter arr; literal 0; index(collection:arr,index:zero); then field(object:first,field:name).",
  "To build an object input such as {url: value}, use a literal node with valueJson '{}' and then an intrinsic node op map_set with args [emptyObject, literalKey, value]. The result is a normal expression node and can be the input of capability_call.",
  "Use intrinsic text_url_encode on untrusted text before placing it in a URL query component.",
  "For an effectful operation, use one capability_call node with contentId, procedureId, and input (a node id). A capability_call is authorable without performing it; consent and host checks occur only when a later execution reaches that node.",
  "The first flat-wire version accepts only empty contract arrays. Spoon independently checks the worked invocation and answer before learning the procedure.",
  "Use an empty interpretations array. For a reusable lesson, lesson is an object; otherwise lesson is null. Use an empty abstainReason when there is none.",
].join(" ");

export function isCodexFlatAuthoringSchema(schema: ProposalSchema): boolean {
  return schema === CODEX_FLAT_AUTHORING_SCHEMA;
}

export function decodeCodexFlatAuthoring(value: JsonValue): JsonValue {
  const errors = validateSchema(value, CODEX_FLAT_AUTHORING_SCHEMA);
  if (errors.length > 0) {
    throw new TeacherError(
      "codex",
      `flat authoring output failed its provider schema: ${errors[0]}`,
    );
  }
  const proposal = asObject(value, "proposal");
  const lesson = proposal.lesson;
  const proposalKind = requiredString(proposal, "proposalKind");
  if (proposalKind === "reusable_lesson" && lesson === null) {
    throw new TeacherError("codex", "reusable_lesson requires a flat lesson");
  }
  if (proposalKind !== "reusable_lesson" && lesson !== null) {
    throw new TeacherError(
      "codex",
      `${proposalKind} may not include a reusable lesson`,
    );
  }

  return {
    proposalKind,
    interpretations: [],
    lesson: lesson === null ? null : decodeLesson(asObject(lesson, "lesson")),
    procedure: null,
    answer: parseEmbeddedJson(
      requiredString(proposal, "answerJson"),
      "answerJson",
    ),
    abstainReason: emptyToNull(requiredString(proposal, "abstainReason")),
  };
}

function decodeLesson(lesson: JsonObject): JsonObject {
  return {
    primitiveSet: "pure_expr_v2",
    concepts: requiredArray(lesson, "concepts"),
    relationships: requiredArray(lesson, "relationships").map(
      (relationship) => {
        const item = asObject(relationship, "relationship");
        return {
          source: decodeConceptReference(
            asObject(item.source, "relationship source"),
          ),
          target: decodeConceptReference(
            asObject(item.target, "relationship target"),
          ),
          kind: requiredString(item, "kind"),
          strength: requiredNumber(item, "strength"),
        };
      },
    ),
    procedures: requiredArray(lesson, "procedures").map((procedure) =>
      decodeProcedure(asObject(procedure, "procedure")),
    ),
    invocation: decodeInvocation(asObject(lesson.invocation, "invocation")),
  };
}

function decodeProcedure(procedure: JsonObject): JsonObject {
  const contract = asObject(procedure.contract, "contract");
  return {
    key: requiredString(procedure, "key"),
    name: requiredString(procedure, "name"),
    concept: decodeConceptReference(
      asObject(procedure.concept, "procedure concept"),
    ),
    parameters: requiredArray(procedure, "parameters"),
    body: decodeGraph(asObject(procedure.body, "procedure body")),
    contract: {
      requires: requiredArray(contract, "requires"),
      promises: requiredArray(contract, "promises"),
      failsWhen: requiredArray(contract, "failsWhen"),
    },
  };
}

function decodeInvocation(invocation: JsonObject): JsonObject {
  return {
    procedureKey: requiredString(invocation, "procedureKey"),
    inputs: requiredArray(invocation, "inputs").map((input) => {
      const item = asObject(input, "invocation input");
      return {
        name: requiredString(item, "name"),
        value: parseEmbeddedJson(
          requiredString(item, "valueJson"),
          "valueJson",
        ),
      };
    }),
  };
}

function decodeConceptReference(reference: JsonObject): JsonObject {
  const kind = requiredString(reference, "kind");
  return kind === "new_concept"
    ? { kind, key: requiredString(reference, "key") }
    : { kind, id: requiredString(reference, "id") };
}

function decodeGraph(graph: JsonObject): JsonObject {
  const nodes = requiredArray(graph, "nodes").map((node) =>
    asObject(node, "node"),
  );
  const nodesById = new Map<string, JsonObject>();
  for (const node of nodes) {
    const nodeId = requiredString(node, "id");
    if (nodesById.has(nodeId)) {
      throw new TeacherError(
        "codex",
        `flat graph has duplicate node id '${nodeId}'`,
      );
    }
    nodesById.set(nodeId, node);
  }

  const active = new Set<string>();
  const resolved = new Map<string, JsonObject>();
  const resolve = (nodeId: string): JsonObject => {
    const cached = resolved.get(nodeId);
    if (cached) return cached;
    if (active.has(nodeId)) {
      throw new TeacherError(
        "codex",
        `flat graph has a cycle at node '${nodeId}'`,
      );
    }
    const node = nodesById.get(nodeId);
    if (!node) {
      throw new TeacherError(
        "codex",
        `flat graph references unknown node '${nodeId}'`,
      );
    }
    active.add(nodeId);
    const expression = decodeNode(node, resolve);
    active.delete(nodeId);
    resolved.set(nodeId, expression);
    return expression;
  };
  return resolve(requiredString(graph, "result"));
}

function decodeNode(
  node: JsonObject,
  resolve: (nodeId: string) => JsonObject,
): JsonObject {
  const kind = requiredString(node, "kind");
  const reference = (key: string) => resolve(requiredString(node, key));
  const references = (key: string) =>
    requiredArray(node, key).map((value) => resolve(asString(value, key)));
  switch (kind) {
    case "literal":
      return {
        kind,
        value: parseEmbeddedJson(
          requiredString(node, "valueJson"),
          "valueJson",
        ),
      };
    case "parameter":
      return { kind, name: requiredString(node, "name") };
    case "result":
      return { kind };
    case "binary":
      return {
        kind,
        op: requiredString(node, "op"),
        left: reference("left"),
        right: reference("right"),
      };
    case "unary":
      return {
        kind,
        op: requiredString(node, "op"),
        operand: reference("operand"),
      };
    case "if":
      return {
        kind,
        condition: reference("condition"),
        then: reference("then"),
        else: reference("else"),
      };
    case "let":
      return {
        kind,
        name: requiredString(node, "name"),
        value: reference("value"),
        body: reference("body"),
      };
    case "list":
      return { kind, items: references("items") };
    case "index":
      return {
        kind,
        collection: reference("collection"),
        index: reference("index"),
      };
    case "field":
      return {
        kind,
        object: reference("object"),
        field: requiredString(node, "field"),
      };
    case "map":
      return {
        kind,
        collection: reference("collection"),
        var: requiredString(node, "var"),
        body: reference("body"),
      };
    case "filter":
      return {
        kind,
        collection: reference("collection"),
        var: requiredString(node, "var"),
        predicate: reference("predicate"),
      };
    case "reduce":
      return {
        kind,
        collection: reference("collection"),
        init: reference("init"),
        acc: requiredString(node, "acc"),
        var: requiredString(node, "var"),
        body: reference("body"),
      };
    case "intrinsic":
      return {
        kind,
        version: requiredNumber(node, "version"),
        op: requiredString(node, "op"),
        args: references("args"),
      };
    case "dependency":
      return {
        kind,
        alias: requiredString(node, "alias"),
        args: references("args"),
      };
    case "capability_call":
      return {
        kind,
        contentId: requiredString(node, "contentId"),
        procedureId: requiredString(node, "procedureId"),
        input: reference("input"),
      };
    default:
      throw new TeacherError(
        "codex",
        `flat graph has unsupported node kind '${kind}'`,
      );
  }
}

function parseEmbeddedJson(value: string, field: string): JsonValue {
  try {
    return parseJsonContent("codex", value);
  } catch (error) {
    throw new TeacherError("codex", `${field} must contain valid JSON`, {
      cause: error,
    });
  }
}

function emptyToNull(value: string): string | null {
  return value.trim() === "" ? null : value;
}

function asObject(value: JsonValue | undefined, field: string): JsonObject {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new TeacherError("codex", `${field} must be an object`);
  }
  return value;
}

function requiredArray(object: JsonObject, key: string): JsonValue[] {
  const value = object[key];
  if (!Array.isArray(value))
    throw new TeacherError("codex", `${key} must be an array`);
  return value;
}

function requiredString(object: JsonObject, key: string): string {
  return asString(object[key], key);
}

function asString(value: JsonValue | undefined, field: string): string {
  if (typeof value !== "string")
    throw new TeacherError("codex", `${field} must be a string`);
  return value;
}

function requiredNumber(object: JsonObject, key: string): number {
  const value = object[key];
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new TeacherError("codex", `${key} must be a finite number`);
  }
  return value;
}
