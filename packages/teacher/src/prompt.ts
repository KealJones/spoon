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
      "relationship references",
      "procedure parameter names",
      "bounded pure expression body and contract checks",
      "invocation inputs",
    ],
    engineProvides: [
      "ids",
      "mutability",
      "lifecycle",
      "versions",
      "timestamps",
      "confidence",
      "test cases",
    ],
  },
} as const;

export const TEACHER_SYSTEM_PROMPT = [
  "You are a Spoon teacher.",
  "For deterministic transformations that clearly generalize across inputs, prefer a reusable lesson over an answer-only proposal.",
  "Extract the reusable concept, relationship, contract, and procedure draft—not merely the immediate answer.",
  "Return only a proposal matching the supplied JSON Schema.",
  "Interpretations may reference only concepts present in the supplied Spoon context; if none apply, return an empty interpretations array.",
  "Use lesson.primitiveSet pure_expr_v2 for new reusable executable knowledge; pure_rpn_v1 is legacy-only. The legacy procedure field must be null.",
  "A pure_expr_v2 body is a tagged expression: parameter names are {kind:'parameter',name}, result is {kind:'result'}, arithmetic/logic is {kind:'binary',op,left,right}, and literals are {kind:'literal',value}. Use only advertised intrinsic operations and dependency aliases; never emit ids or effect calls.",
  "A reusable lesson has exactly primitiveSet, concepts, relationships, procedures, and invocation. Each concept has key/name/description. Each procedure has key/name/concept:{kind:'new_concept',key}/parameters:[{name,description}]/body/contract:{requires,promises,failsWhen}; every contract member is an array of {description,check} (use [] when none), never a bare expression. Invocation is {procedureKey,inputs:[{name,value}]}. Use empty relationships when none are needed; do not substitute example or procedureDraft fields.",
  "A promises check should compare {kind:'result'} with an independently recomputed expression from parameters; do not derive the expected value from result itself.",
  "Never invent ids, timestamps, lifecycle, versions, confidence, mutability, or test cases; the engine owns them.",
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
    "Desired proposal JSON Schema:",
    JSON.stringify(request.desiredOutput, null, 2),
    "",
    "Reusable lesson authoring protocol:",
    JSON.stringify(REUSABLE_LESSON_PROTOCOL, null, 2),
    "",
    "Extract the reusable lesson and produce the structured proposal.",
  ]
    .filter((part) => part !== "")
    .join("\n");
}
