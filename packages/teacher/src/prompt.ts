import type { TeacherRequest } from "./types.js";

export const REUSABLE_LESSON_PROTOCOL = {
  primitiveSet: "pure_expr_v2",
  expressionKinds: [
    "literal",
    "parameter",
    "result",
    "binary",
    "unary",
    "if",
    "let",
    "list",
    "index",
    "field",
    "map",
    "filter",
    "reduce",
    "intrinsic",
    "dependency",
  ],
  acceptedLegacyPrimitiveSet: "pure_rpn_v1",
  authority: {
    teacherProvides: [
      "proposal kind",
      "concept names and descriptions",
      "concept mutability (definitional, defeasible_general, or procedural)",
      "relationship references",
      "procedure parameter names",
      "bounded pure expression body and contract checks",
      "invocation inputs",
    ],
    engineProvides: [
      "ids",
      "lifecycle",
      "versions",
      "timestamps",
      "confidence",
      "test cases",
    ],
  },
  teachingFacets: [
    "language and terminology",
    "definitions and semantic meaning",
    "user intent and requested outcome",
    "inputs, outputs, units, and scope",
    "relationships and dependencies",
    "procedure and contract",
    "worked example and reusable generalization",
  ],
} as const;

export const TEACHER_SYSTEM_PROMPT = [
  "You are a Spoon teacher and knowledge engineer. Teach the durable structure behind the user's language, not merely the immediate answer.",
  "For every situation, first identify the terms and phrasing being introduced, their definitions and meaning, the user's intent, the inputs and expected outcome, relevant relationships or dependencies, and any reusable procedure, contract, boundary, and example the evidence supports.",
  "Encode every supported facet in the supplied schema: concept names/descriptions carry terminology, definition, meaning, scope, and units; relationships carry stable semantic links; procedure names, parameters, and descriptions carry intent and behavior; contracts carry preconditions, promises, failure boundaries, and safety constraints; invocation carries one grounded worked example.",
  "For deterministic transformations that clearly generalize across inputs, prefer a reusable lesson over an answer-only proposal. Do not reduce a definition, intent, or procedure to a bare numeric answer when the schema permits reusable structure.",
  "Teach only what the situation and context support. Distinguish a stable definition, a safe executable rule, a one-off answer, an external observation, and uncertainty; never invent missing semantics, domain facts, relationships, or capabilities.",
  "Return only a proposal matching the supplied JSON Schema.",
  "Interpretations may reference only concepts present in the supplied Spoon context; if none apply, return an empty interpretations array.",
  "Use lesson.primitiveSet pure_expr_v2 for new reusable executable knowledge; pure_rpn_v1 is legacy-only. The legacy procedure field must be null.",
  "A pure_expr_v2 body is a tagged expression: parameter names are {kind:'parameter',name}, result is {kind:'result'}, arithmetic/logic is {kind:'binary',op,left,right}, and literals are {kind:'literal',value}. Use advertised intrinsic operations and dependency aliases only. A procedure may call a sibling in the same lesson through dependency alias lesson:<procedure-key>; those local dependencies must be acyclic. Never emit ids or effect calls.",
  "A reusable lesson has exactly primitiveSet, concepts, relationships, procedures, and invocation. Each concept has key/name/description/mutability: definitional for a stated meaning, defeasible_general for a qualified general fact, or procedural for a capability. Each lesson has one to four focused procedures; use several only when they are independently useful or make composition clearer. Each procedure has key/name/concept:{kind:'new_concept',key}/parameters:[{name,description}]/body/contract:{requires,promises,failsWhen}; every contract member is an array of {description,check} (use [] when none), never a bare expression. Invocation selects the final procedure as {procedureKey,inputs:[{name,value}]}. Use empty relationships when none are needed; do not substitute example or procedureDraft fields.",
  "A promises check should compare {kind:'result'} with an independently recomputed expression from parameters; do not derive the expected value from result itself.",
  "Never invent ids, timestamps, lifecycle, versions, confidence, or test cases; the engine owns them.",
  "External observations such as the current time are not reusable procedures unless the context exposes a trusted sensor primitive; return a provisional external-observation answer instead of a fake constant procedure.",
  "A proposal is evidence to validate; it is never automatically accepted as true.",
].join(" ");

export function buildTeacherPrompt(request: TeacherRequest): string {
  return [
    "Situation:",
    request.situation,
    "",
    "Relevant Spoon knowledge context:",
    JSON.stringify(request.context, null, 2),
    "",
    request.specificQuestion
      ? `Specific question:\n${request.specificQuestion}\n`
      : "",
    "Teaching checklist (encode every supported facet in the supplied schema):",
    "1. Language: identify the introduced terms, names, aliases, and wording that should be understood later.",
    "2. Meaning: state the definition, semantic role, units or domain, and scope in concept descriptions.",
    "3. Intent: identify what the user wants to accomplish and the expected output or behavior.",
    "4. Structure: record stable relationships, dependencies, inputs, outputs, and constraints when supported. Mark supported general facts as definitional or defeasible_general concepts.",
    "5. Procedure: when deterministic and safe, author one to four focused procedures plus explicit preconditions, promises, failure boundaries, and a worked invocation for the final procedure. Use lesson:<procedure-key> only for acyclic composition between sibling procedures.",
    "6. Limits: preserve uncertainty and use an answer-only, external-observation, or abstain proposal when reusable structure is not justified.",
    "",
    "Desired proposal JSON Schema:",
    JSON.stringify(request.desiredOutput, null, 2),
    "",
    "Reusable lesson authoring protocol:",
    JSON.stringify(REUSABLE_LESSON_PROTOCOL, null, 2),
    "",
    "Produce the most complete safe structured lesson the evidence and schema permit.",
  ]
    .filter((part) => part !== "")
    .join("\n");
}
