import type { TeacherRequest } from "./types.js";

export const REUSABLE_LESSON_PROTOCOL = {
  primitiveSet: "pure_rpn_v1",
  instructions: [
    "load_parameter",
    "load_result",
    "push_literal",
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
    "negate",
    "not",
  ],
  authority: {
    teacherProvides: [
      "proposal kind",
      "concept names and descriptions",
      "relationship references",
      "procedure parameter names",
      "pure RPN body and contract checks",
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
  "You are an EKG teacher.",
  "For deterministic transformations that clearly generalize across inputs, prefer a reusable lesson over an answer-only proposal.",
  "Extract the reusable concept, relationship, contract, and procedure draft—not merely the immediate answer.",
  "Return only a proposal matching the supplied JSON Schema.",
  "Interpretations may reference only concepts present in the supplied EKG context; if none apply, return an empty interpretations array.",
  "Use lesson.primitiveSet pure_rpn_v1 exactly when authoring reusable executable knowledge; the legacy procedure field must be null.",
  "In a procedure body, load each declared input with load_parameter before transforming it; load_result is valid only inside contract checks evaluated after the body has produced a result.",
  "A promises check should compare load_result with an independently recomputed expression from load_parameter values; do not derive the expected value from load_result itself.",
  "Never invent ids, timestamps, lifecycle, versions, confidence, mutability, or test cases; the engine owns them.",
  "External observations such as the current time are not reusable procedures unless the context exposes a trusted sensor primitive; return a provisional external-observation answer instead of a fake constant procedure.",
  "A proposal is evidence to validate; it is never automatically accepted as true.",
].join(" ");

export function buildTeacherPrompt(request: TeacherRequest): string {
  return [
    "Situation:",
    request.situation,
    "",
    "Relevant EKG knowledge context:",
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
