export {
  ClaudeTeacher,
  runCommand,
  type ClaudeTeacherOptions,
} from "./claude.js";
export { TeacherError } from "./errors.js";
export { HumanTeacher, type HumanTeacherOptions } from "./human.js";
export { OllamaTeacher, type OllamaTeacherOptions } from "./ollama.js";
export { OpenAITeacher, type OpenAITeacherOptions } from "./openai.js";
export { buildTeacherPrompt, TEACHER_SYSTEM_PROMPT } from "./prompt.js";
export { SourceReliabilityTracker } from "./reliability.js";
export { validateSchema } from "./schema.js";
export { fingerprintTeacherRequest } from "./shared.js";
export {
  ProposalValidationPipeline,
  type ProposalValidationPipelineOptions,
} from "./validation.js";
export type {
  Clock,
  CommandInvocation,
  CommandResult,
  CommandRunner,
  Fetch,
  HumanPrompt,
  IdFactory,
  JsonObject,
  JsonPrimitive,
  JsonSchemaType,
  JsonValue,
  KnowledgeContext,
  ProposalProvenance,
  ProposalSchema,
  ProposalValidation,
  ProposalValidator,
  ProposalValidatorResult,
  ProviderKind,
  SourceReliability,
  Teacher,
  TeacherProposal,
  TeacherRequest,
  ValidatedProposal,
  ValidationCheck,
  ValidationStatus,
} from "./types.js";
