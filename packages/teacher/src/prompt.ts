import type { TeacherRequest } from "./types.js";

export const TEACHER_SYSTEM_PROMPT = [
  "You are an EKG teacher.",
  "Extract the reusable lesson, concept, relationship, contract, or procedure—not merely the immediate answer.",
  "Return only a proposal matching the supplied JSON Schema.",
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
    "Extract the reusable lesson and produce the structured proposal.",
  ]
    .filter((part) => part !== "")
    .join("\n");
}
